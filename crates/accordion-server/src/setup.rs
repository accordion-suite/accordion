//! The setup wizard (decision 5 in `docs/decisions`).
//!
//! A new server has no name and no owner. The wizard runs on plain HTTP at
//! `http://accordion.local`. It answers only requests from the home network,
//! and it needs the setup code before it changes anything. It asks for the
//! server's name and DNS token, and gets the certificate. Then it hands off to
//! `https://<name>/setup`, where the owner creates their account, so the
//! owner's password never crosses plain HTTP.
//!
//! An appliance with no screen has nowhere to show the code. When the admin
//! turns on the setup window, the server accepts one claim without the code
//! for 30 minutes after it starts.
//!
//! Instead of a name, the wizard can restore a backup (decision 8). It
//! brings back the owner, the name, and everything else, so once the restore
//! is done, the server starts again with the restored data and setup is over.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use accordion_core::backup::{self, Backup, Place};
use accordion_core::identity::{self, AuthMethod, Client, NewUser};
use accordion_gateway::ServerName;
use accordion_gateway::address::{AddressSource, detect_address};
use accordion_gateway::certificates::{Authority, Renewer, Settings};
use accordion_gateway::dns::{DnsProvider, Provider, ProviderKind};
use accordion_gateway::local_name::is_local_name;
use accordion_gateway::tls::CertStore;
use accordion_module::{ApiError, auth};
use accordion_types::Role;
use axum::extract::{Request, State};
use axum::http::header::{
    AUTHORIZATION, CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, HOST, REFERRER_POLICY,
    SET_COOKIE, USER_AGENT, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::middleware::{Next, from_fn, from_fn_with_state};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::backups::{PlaceRequest, SshKeyView};
use crate::session::SessionResponse;
use crate::{AppState, device, network, origin};

/// Holds the setup code, so `accordion setup-code` can show it.
pub const SETUP_CODE_FILE: &str = "setup-code";

const CODE_LEN: usize = 10;
const CLAIM_LIFETIME: Duration = Duration::from_secs(60 * 60);
const HANDOFF_LIFETIME: Duration = Duration::from_secs(5 * 60);
pub const SETUP_WINDOW: Duration = Duration::from_secs(30 * 60);

/// Wrong codes allowed before claims pause. Guessing a 10-letter code is
/// hopeless anyway, so this mostly keeps the log quiet.
const MAX_WRONG_CODES: u32 = 5;
const LOCKOUT: Duration = Duration::from_secs(60);

/// How long the server waits after a restore before it starts again, so the
/// wizard, which checks every few seconds, can say it's done first.
const RESTART_DELAY: Duration = Duration::from_secs(5);

/// The setup wizard's state, shared by the plain HTTP wizard and the HTTPS
/// owner step.
pub struct Setup {
    code: String,
    code_path: PathBuf,
    gateway_dir: PathBuf,
    authority: Authority,
    https_port: u16,
    address: Option<AddressSource>,
    backup: Option<Backup>,
    state: Mutex<Inner>,
    named: watch::Sender<Option<Settings>>,
    restored: watch::Sender<bool>,
}

pub struct SetupOptions {
    pub data_dir: PathBuf,
    pub gateway_dir: PathBuf,
    pub authority: Authority,
    /// The port that HTTPS will use, for the handoff address.
    pub https_port: u16,
    /// Accept one claim without the code for 30 minutes.
    pub open_window: bool,
    /// Where the name's address record points, or `None` to leave it alone.
    pub address: Option<AddressSource>,
    /// For restoring a backup instead of setting up from scratch.
    pub backup: Option<Backup>,
}

#[derive(Default)]
struct Inner {
    /// When the window for claiming without the code closes.
    open_until: Option<Instant>,
    /// Tokens from claiming the wizard, with when each expires.
    claims: HashMap<String, Instant>,
    /// Tokens that carry a claim over to HTTPS, with when each expires.
    handoffs: HashMap<String, Instant>,
    wrong_codes: u32,
    locked_until: Option<Instant>,
    progress: Progress,
    restore: RestoreProgress,
    /// The server's HTTPS origin, once it has a name.
    origin: Option<String>,
}

/// How the name step is going.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Progress {
    #[default]
    NotStarted,
    Working {
        name: String,
    },
    Failed {
        error: String,
    },
    /// `url` carries the handoff token after `#`, so it never reaches a log.
    Done {
        url: String,
    },
}

/// How a restore from a backup is going.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RestoreProgress {
    #[default]
    NotStarted,
    Working {
        percent: u8,
    },
    Failed {
        error: String,
    },
    /// `url` is the restored server's address, if it had a name.
    Done {
        url: Option<String>,
    },
}

impl Setup {
    /// Starts the wizard, keeping the setup code from an earlier start if
    /// there is one. The receiver gets the settings once the server has a
    /// name and a certificate.
    pub fn start(
        options: SetupOptions,
    ) -> io::Result<(Arc<Self>, watch::Receiver<Option<Settings>>)> {
        let code_path = options.data_dir.join(SETUP_CODE_FILE);
        let code = match read_code(&code_path)? {
            Some(code) => code,
            None => {
                let code = new_code();
                write_private(&code_path, &code)?;
                code
            }
        };
        let (named, receiver) = watch::channel(None);
        let state = Inner {
            open_until: options.open_window.then(|| Instant::now() + SETUP_WINDOW),
            ..Inner::default()
        };
        let setup = Arc::new(Self {
            code,
            code_path,
            gateway_dir: options.gateway_dir,
            authority: options.authority,
            https_port: options.https_port,
            address: options.address,
            backup: options.backup,
            state: Mutex::new(state),
            named,
            restored: watch::Sender::new(false),
        });
        Ok((setup, receiver))
    }

    /// The code, grouped so it's easier to read aloud, such as `abcde-fghij`.
    pub fn display_code(&self) -> String {
        group_code(&self.code)
    }

    /// Changes to `true` once a restore is done, when the server should
    /// start again to open the restored data.
    pub fn restored(&self) -> watch::Receiver<bool> {
        self.restored.subscribe()
    }

    /// Whether the window for claiming without the code is open.
    pub fn window_open(&self) -> bool {
        self.lock().window_open()
    }

    /// Tells the wizard the server already has a name, so it hands off to
    /// HTTPS, such as after a restart between the name and owner steps.
    pub fn set_origin(&self, origin: String) {
        self.lock().origin = Some(origin);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Checks the setup code, or the window. Each window lets in one person.
    fn check_code(&self, state: &mut Inner, code: Option<&str>) -> Result<(), ApiError> {
        let now = Instant::now();
        if state.window_open() {
            state.open_until = None;
            return Ok(());
        }
        if state.locked_until.is_some_and(|until| now < until) {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "That's too many wrong setup codes. Wait a minute, then try again.",
            ));
        }
        let entered = normalize_code(code.unwrap_or(""));
        if constant_time_eq(entered.as_bytes(), self.code.as_bytes()) {
            state.wrong_codes = 0;
            return Ok(());
        }
        state.wrong_codes += 1;
        if state.wrong_codes >= MAX_WRONG_CODES {
            state.wrong_codes = 0;
            state.locked_until = Some(now + LOCKOUT);
            tracing::warn!("someone entered {MAX_WRONG_CODES} wrong setup codes in a row");
        }
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "That setup code isn't right. Check it on the server's screen or in its log, then \
             try again.",
        ))
    }

    /// Claims the wizard, and returns a token for the next steps.
    fn claim(&self, code: Option<&str>) -> Result<String, ApiError> {
        let mut state = self.lock();
        self.check_code(&mut state, code)?;
        let token = random_token();
        let now = Instant::now();
        state.claims.retain(|_, expires| *expires > now);
        state.claims.insert(token.clone(), now + CLAIM_LIFETIME);
        Ok(token)
    }

    fn check_claim(&self, headers: &HeaderMap) -> Result<(), ApiError> {
        let token = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));
        let state = self.lock();
        let valid = token
            .and_then(|token| state.claims.get(token))
            .is_some_and(|expires| *expires > Instant::now());
        if valid {
            Ok(())
        } else {
            Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "This setup page has expired. Reload it, then enter the setup code again.",
            ))
        }
    }

    pub fn progress(&self) -> Progress {
        self.lock().progress.clone()
    }

    /// Gets the certificate in the background, because it can take several
    /// minutes. The wizard asks for progress meanwhile.
    fn start_naming(self: &Arc<Self>, settings: Settings) -> Result<(), ApiError> {
        {
            let mut state = self.lock();
            match state.progress {
                Progress::Working { .. } => {
                    return Err(ApiError::new(
                        StatusCode::CONFLICT,
                        "Accordion is already getting a certificate. Wait for it to finish.",
                    ));
                }
                Progress::Done { .. } => {
                    return Err(ApiError::new(
                        StatusCode::CONFLICT,
                        "The server already has a name. Continue to the next step.",
                    ));
                }
                Progress::NotStarted | Progress::Failed { .. } => {}
            }
            check_not_restoring(&state)?;
            state.progress = Progress::Working {
                name: settings.name.to_string(),
            };
        }
        tracing::info!(name = %settings.name, "getting the server's first certificate");
        let setup = Arc::clone(self);
        tokio::spawn(async move {
            let result = setup.get_certificate(&settings).await;
            let mut state = setup.lock();
            match result {
                Ok(()) => {
                    let origin = https_origin(&settings.name, setup.https_port);
                    let handoff = random_token();
                    state
                        .handoffs
                        .insert(handoff.clone(), Instant::now() + HANDOFF_LIFETIME);
                    state.progress = Progress::Done {
                        url: format!("{origin}/setup#{handoff}"),
                    };
                    state.origin = Some(origin);
                    drop(state);
                    setup.named.send_replace(Some(settings));
                }
                Err(error) => {
                    tracing::warn!("the setup wizard couldn't get a certificate: {error}");
                    state.progress = Progress::Failed { error };
                }
            }
        });
        Ok(())
    }

    async fn get_certificate(&self, settings: &Settings) -> Result<(), String> {
        // Point the name at the server first. The record then spreads to
        // every nameserver while the certificate is on its way, and a token
        // that can't change it fails before the slow part.
        if let Some(source) = self.address {
            let address = match source {
                AddressSource::Detect => detect_address().map(IpAddr::V4).map_err(|err| {
                    format!(
                        "Accordion couldn't find the server's network address ({err}). Check \
                         that the server is connected to your home network, then try again."
                    )
                })?,
                AddressSource::Fixed(address) => address,
            };
            let dns = Provider::new(settings.provider, settings.token.clone())
                .map_err(|err| err.to_string())?;
            dns.set_address(settings.name.as_str(), address)
                .await
                .map_err(|err| err.to_string())?;
            tracing::info!(name = %settings.name, %address, "pointed the name at the server");
        }
        let renewer = Renewer::from_settings(&self.gateway_dir, settings, CertStore::new())
            .map_err(|err| err.to_string())?;
        renewer.renew().await.map_err(|err| err.to_string())?;
        settings
            .save(&self.gateway_dir)
            .map_err(|err| err.to_string())
    }

    fn backup(&self) -> Result<Backup, ApiError> {
        self.backup.clone().ok_or_else(|| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Restoring from a backup isn't available on this server.",
            )
        })
    }

    /// Restores a backup in the background, because a large one can take an
    /// hour. The wizard asks for progress meanwhile. Once it's done, the
    /// server starts again with the restored data.
    fn start_restore(
        self: &Arc<Self>,
        place: Place,
        key: String,
        snapshot: String,
    ) -> Result<(), ApiError> {
        let backup = self.backup()?;
        {
            let mut state = self.lock();
            check_not_restoring(&state)?;
            if matches!(
                state.progress,
                Progress::Working { .. } | Progress::Done { .. }
            ) {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "This server already has a name, so it can't restore a backup now. \
                     Continue setting it up instead.",
                ));
            }
            state.restore = RestoreProgress::Working { percent: 0 };
        }
        tracing::info!(?place, "restoring a backup");
        let setup = Arc::clone(self);
        tokio::spawn(async move {
            let reporter = Arc::clone(&setup);
            let result = backup
                .restore(&place, &key, &snapshot, move |done| {
                    // Restic reaches 100% before Accordion checks the
                    // database, so the last percent waits for that.
                    let percent = (done * 100.0).clamp(0.0, 99.0) as u8;
                    reporter.lock().restore = RestoreProgress::Working { percent };
                })
                .await;
            drop(backup);
            match result {
                Ok(restored) => {
                    let url = match Settings::load(&restored.join("gateway")) {
                        Ok(settings) => {
                            settings.map(|settings| https_origin(&settings.name, setup.https_port))
                        }
                        Err(err) => {
                            tracing::warn!("couldn't read the restored server's name: {err}");
                            None
                        }
                    };
                    setup.lock().restore = RestoreProgress::Done { url };
                    tracing::info!("restored the backup, so Accordion is starting again");
                    tokio::time::sleep(RESTART_DELAY).await;
                    setup.restored.send_replace(true);
                }
                Err(err) => {
                    tracing::warn!("the setup wizard couldn't restore the backup: {err}");
                    setup.lock().restore = RestoreProgress::Failed {
                        error: err.to_string(),
                    };
                }
            }
        });
        Ok(())
    }

    /// Checks that whoever is creating the owner account claimed the
    /// wizard: with the handoff from the name step, or else the setup code.
    fn check_owner_step(&self, handoff: Option<&str>, code: Option<&str>) -> Result<(), ApiError> {
        let mut state = self.lock();
        let now = Instant::now();
        let handed_off = handoff
            .and_then(|handoff| state.handoffs.get(handoff))
            .is_some_and(|expires| *expires > now);
        if handed_off {
            return Ok(());
        }
        self.check_code(&mut state, code)
    }

    /// Ends setup once the owner exists.
    fn finish(&self) {
        let mut state = self.lock();
        state.claims.clear();
        state.handoffs.clear();
        state.open_until = None;
        drop(state);
        if let Err(err) = std::fs::remove_file(&self.code_path)
            && err.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(
                "Accordion couldn't delete {}: {err}. Delete it yourself.",
                self.code_path.display()
            );
        }
    }

    #[cfg(test)]
    fn add_handoff(&self, handoff: &str) {
        self.lock()
            .handoffs
            .insert(handoff.to_owned(), Instant::now() + HANDOFF_LIFETIME);
    }
}

impl Inner {
    fn window_open(&self) -> bool {
        self.open_until.is_some_and(|until| Instant::now() < until)
    }
}

/// Refuses to start anything else while a restore runs or after it's done.
fn check_not_restoring(state: &Inner) -> Result<(), ApiError> {
    match state.restore {
        RestoreProgress::Working { .. } => Err(ApiError::new(
            StatusCode::CONFLICT,
            "Accordion is restoring a backup. Wait for it to finish.",
        )),
        RestoreProgress::Done { .. } => Err(ApiError::new(
            StatusCode::CONFLICT,
            "Accordion restored a backup and is starting again with it.",
        )),
        RestoreProgress::NotStarted | RestoreProgress::Failed { .. } => Ok(()),
    }
}

/// Reads the saved setup code, for `accordion setup-code`.
pub fn read_code(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(code) => Ok(Some(normalize_code(&code))),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Splits a code in two, such as `abcde-fghij`, so it's easier to read aloud.
pub fn group_code(code: &str) -> String {
    match code.get(..5) {
        Some(first) => format!("{first}-{}", &code[5..]),
        None => code.to_owned(),
    }
}

/// Ten random lowercase letters: about 47 bits, and easy to type on a phone.
fn new_code() -> String {
    let mut code = String::with_capacity(CODE_LEN);
    while code.len() < CODE_LEN {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("the OS random number generator failed");
        // 234 is the largest multiple of 26 below 256. Dropping larger bytes
        // keeps every letter equally likely.
        for byte in bytes.into_iter().filter(|&byte| byte < 234) {
            if code.len() < CODE_LEN {
                code.push(char::from(b'a' + byte % 26));
            }
        }
    }
    code
}

/// Accepts the code with capitals, spaces, or the hyphen in the middle.
fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphabetic)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS random number generator failed");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Compares without stopping at the first difference, so response times
/// don't reveal how much of a guess was right.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |diff, (x, y)| diff | (x ^ y)) == 0
}

fn write_private(path: &Path, contents: &str) -> io::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path)?.write_all(contents.as_bytes())
}

pub fn https_origin(name: &ServerName, port: u16) -> String {
    if port == 443 {
        format!("https://{name}")
    } else {
        format!("https://{name}:{port}")
    }
}

/// Whether the `Host` header names the wizard: `accordion.local` (or the
/// name Accordion announces instead after a conflict), `localhost`, or an
/// address. Websites can't use an address to rebind their own name to the
/// server, so addresses are safe to allow.
fn is_wizard_host(authority: &str) -> bool {
    let host = if let Some(rest) = authority.strip_prefix('[') {
        match rest.split_once(']') {
            Some((address, _)) => return address.parse::<Ipv6Addr>().is_ok(),
            None => return false,
        }
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) if port.bytes().all(|b| b.is_ascii_digit()) => host,
            _ => authority,
        }
    };
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    is_local_name(&host) || host == "localhost" || host.parse::<IpAddr>().is_ok()
}

/// Middleware for the wizard: only the home network, and only the wizard's
/// own host names.
async fn only_home_network(request: Request, next: Next) -> Response {
    let from_home = network::peer(request.extensions()).is_some_and(|peer| peer.on_home_network());
    if !from_home {
        return (
            StatusCode::FORBIDDEN,
            "Accordion's setup wizard only answers devices on the same network as the server.",
        )
            .into_response();
    }
    let host_ok = request
        .headers()
        .get(HOST)
        .and_then(|host| host.to_str().ok())
        .is_some_and(is_wizard_host);
    if !host_ok {
        return (
            StatusCode::MISDIRECTED_REQUEST,
            "Open the setup wizard at http://accordion.local or at the server's address.",
        )
            .into_response();
    }
    next.run(request).await
}

/// The wizard on plain HTTP, for port 80 until the server has an owner.
pub fn wizard_router(setup: Arc<Setup>) -> Router {
    Router::new()
        .route("/", get(wizard_page))
        .route(
            "/wizard.js",
            get(|| async { asset(WIZARD_JS, "text/javascript") }),
        )
        .route("/setup.css", get(|| async { asset(SETUP_CSS, "text/css") }))
        .route(
            "/favicon.svg",
            get(|| async { asset(ICON_SVG, "image/svg+xml") }),
        )
        .route("/api/setup", get(wizard_status))
        .route("/api/setup/claim", post(claim))
        .route("/api/setup/name", post(set_name))
        .route("/api/setup/restore/find", post(find_backups))
        .route("/api/setup/restore/ssh-key", get(restore_ssh_key))
        .route("/api/setup/restore", post(restore))
        .fallback(after_naming)
        .layer(from_fn_with_state(None, origin::reject_cross_origin))
        .layer(from_fn(only_home_network))
        .with_state(setup)
}

/// The owner step, on HTTPS.
pub fn owner_routes() -> Router<AppState> {
    Router::new()
        .route("/setup", get(owner_page))
        .route(
            "/owner.js",
            get(|| async { asset(OWNER_JS, "text/javascript") }),
        )
        .route("/api/setup", get(owner_status))
        .route("/api/setup/owner", post(create_owner))
}

const WIZARD_HTML: &str = include_str!("setup/wizard.html");
const WIZARD_JS: &str = include_str!("setup/wizard.js");
const OWNER_HTML: &str = include_str!("setup/owner.html");
const OWNER_JS: &str = include_str!("setup/owner.js");
pub(crate) const SETUP_CSS: &str = include_str!("setup/setup.css");
/// The tab icon, a copy of the web app's `src/icon/icon.svg`. The wizard
/// opens before the web app is there, so the server keeps its own.
pub(crate) const ICON_SVG: &str = include_str!("setup/icon.svg");

/// Scripts, styles, and frames come only from the server itself, and no
/// other site can show these pages in a frame. Drive's web pages show PDFs
/// in a frame, in the browser's own PDF viewer.
pub(crate) const CONTENT_SECURITY_POLICY_VALUE: &str = "default-src 'none'; script-src 'self'; \
     style-src 'self'; connect-src 'self'; img-src 'self'; frame-src 'self'; \
     manifest-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'";

pub(crate) fn asset(body: &'static str, content_type: &'static str) -> Response {
    (
        [
            (CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
            (REFERRER_POLICY, HeaderValue::from_static("no-referrer")),
            (
                CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(CONTENT_SECURITY_POLICY_VALUE),
            ),
        ],
        body,
    )
        .into_response()
}

/// Once the server has a name, the wizard sends everyone to HTTPS.
fn https_redirect(setup: &Setup, path: &str) -> Option<Response> {
    let origin = setup.lock().origin.clone()?;
    Some(Redirect::temporary(&format!("{origin}{path}")).into_response())
}

async fn wizard_page(State(setup): State<Arc<Setup>>) -> Response {
    https_redirect(&setup, "/setup")
        .unwrap_or_else(|| asset(WIZARD_HTML, "text/html; charset=utf-8"))
}

async fn after_naming(State(setup): State<Arc<Setup>>, uri: Uri) -> Response {
    let path = uri.path_and_query().map_or("/", |path| path.as_str());
    https_redirect(&setup, path).unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}

#[derive(Serialize)]
struct WizardStatus {
    code_needed: bool,
    /// Only for whoever claimed the wizard.
    #[serde(skip_serializing_if = "Option::is_none")]
    progress: Option<Progress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restore: Option<RestoreProgress>,
}

/// `GET /api/setup`: whether the code is needed, and for whoever claimed
/// the wizard, how the name step is going.
async fn wizard_status(State(setup): State<Arc<Setup>>, headers: HeaderMap) -> Json<WizardStatus> {
    let claimed = setup.check_claim(&headers).is_ok();
    Json(WizardStatus {
        code_needed: !setup.window_open(),
        progress: claimed.then(|| setup.progress()),
        restore: claimed.then(|| setup.lock().restore.clone()),
    })
}

#[derive(Deserialize)]
struct ClaimRequest {
    code: Option<String>,
}

#[derive(Serialize)]
struct ClaimResponse {
    token: String,
}

/// `POST /api/setup/claim`: checks the setup code.
async fn claim(
    State(setup): State<Arc<Setup>>,
    Json(request): Json<ClaimRequest>,
) -> Result<Json<ClaimResponse>, ApiError> {
    let token = setup.claim(request.code.as_deref())?;
    Ok(Json(ClaimResponse { token }))
}

#[derive(Deserialize)]
struct NameRequest {
    name: String,
    provider: String,
    token: String,
}

/// `POST /api/setup/name`: sets the name and starts getting the certificate.
async fn set_name(
    State(setup): State<Arc<Setup>>,
    headers: HeaderMap,
    Json(request): Json<NameRequest>,
) -> Result<StatusCode, ApiError> {
    setup.check_claim(&headers)?;
    let name: ServerName = request
        .name
        .trim()
        .parse()
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, format!("{err}")))?;
    let provider: ProviderKind = request.provider.parse().map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "Choose deSEC or Cloudflare as the DNS provider.",
        )
    })?;
    let token = request.token.trim().to_owned();
    if token.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("Enter the API token from {provider}."),
        ));
    }
    setup.start_naming(Settings {
        name,
        provider,
        token,
        authority: setup.authority,
    })?;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
struct FindRequest {
    #[serde(flatten)]
    place: PlaceRequest,
    recovery_key: String,
}

#[derive(Serialize)]
struct FoundBackups {
    snapshots: Vec<SnapshotView>,
}

#[derive(Serialize)]
struct SnapshotView {
    id: String,
    /// Milliseconds since 1970, for the page to show in local time.
    time: u64,
    size: Option<u64>,
}

fn restore_error(err: backup::Error) -> ApiError {
    use backup::Error as E;
    match err {
        E::Db(_) | E::Sqlite(_) | E::Io(_) => {
            tracing::error!(%err, "restoring a backup failed");
            ApiError::server_problem()
        }
        E::Busy => ApiError::new(StatusCode::CONFLICT, err.to_string()),
        err => ApiError::new(StatusCode::BAD_REQUEST, err.to_string()),
    }
}

/// `GET /api/setup/restore/ssh-key`: this server's SSH public key, for the
/// owner to add to the computer that holds backups before restoring over
/// SFTP. The old server's key is inside the backup, so it can't be used yet.
async fn restore_ssh_key(
    State(setup): State<Arc<Setup>>,
    headers: HeaderMap,
) -> Result<Json<SshKeyView>, ApiError> {
    setup.check_claim(&headers)?;
    let public_key = setup
        .backup()?
        .ssh_public_key()
        .await
        .map_err(restore_error)?;
    Ok(Json(SshKeyView { public_key }))
}

/// `POST /api/setup/restore/find`: lists the backups in a folder or bucket,
/// once the recovery key opens them.
async fn find_backups(
    State(setup): State<Arc<Setup>>,
    headers: HeaderMap,
    Json(request): Json<FindRequest>,
) -> Result<Json<FoundBackups>, ApiError> {
    setup.check_claim(&headers)?;
    let backup = setup.backup()?;
    let snapshots = backup
        .find_snapshots(&request.place.into(), &request.recovery_key)
        .await
        .map_err(restore_error)?;
    Ok(Json(FoundBackups {
        snapshots: snapshots
            .into_iter()
            .map(|snapshot| SnapshotView {
                id: snapshot.id,
                time: snapshot.time.duration_since(UNIX_EPOCH).map_or(0, |since| {
                    u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
                }),
                size: snapshot.size,
            })
            .collect(),
    }))
}

#[derive(Deserialize)]
struct RestoreRequest {
    #[serde(flatten)]
    place: PlaceRequest,
    recovery_key: String,
    snapshot: String,
}

/// `POST /api/setup/restore`: starts restoring a backup.
async fn restore(
    State(setup): State<Arc<Setup>>,
    headers: HeaderMap,
    Json(request): Json<RestoreRequest>,
) -> Result<StatusCode, ApiError> {
    setup.check_claim(&headers)?;
    setup.start_restore(request.place.into(), request.recovery_key, request.snapshot)?;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Serialize)]
struct OwnerStatus {
    owner_needed: bool,
    code_needed: bool,
}

/// `GET /api/setup` on HTTPS: whether the owner step is still open.
async fn owner_status(State(state): State<AppState>) -> Result<Json<OwnerStatus>, ApiError> {
    let owner_needed = match &state.setup {
        Some(_) => !state.identity.has_owner().await?,
        None => false,
    };
    Ok(Json(OwnerStatus {
        owner_needed,
        code_needed: state
            .setup
            .as_ref()
            .is_none_or(|setup| !setup.window_open()),
    }))
}

async fn owner_page() -> Response {
    asset(OWNER_HTML, "text/html; charset=utf-8")
}

#[derive(Deserialize)]
struct OwnerRequest {
    handoff: Option<String>,
    code: Option<String>,
    username: String,
    display_name: String,
    password: String,
}

fn already_set_up() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "This server is already set up. Sign in with the owner account instead.",
    )
}

/// `POST /api/setup/owner`: creates the owner account and signs them in.
async fn create_owner(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<OwnerRequest>,
) -> Result<Response, ApiError> {
    let setup = state.setup.as_ref().ok_or_else(already_set_up)?;
    if state.identity.has_owner().await? {
        return Err(already_set_up());
    }
    setup.check_owner_step(request.handoff.as_deref(), request.code.as_deref())?;
    let user = match state
        .identity
        .create_user(NewUser {
            username: request.username,
            display_name: request.display_name,
            role: Role::Owner,
            password: request.password,
        })
        .await
    {
        Ok(user) => user,
        Err(identity::Error::OwnerExists) => return Err(already_set_up()),
        Err(err) => return Err(err.into()),
    };
    setup.finish();
    tracing::info!(user = %user.id, "created the owner account, so setup is finished");

    let user_agent = headers.get(USER_AGENT).and_then(|ua| ua.to_str().ok());
    let (token, session) = state
        .identity
        .create_session(
            user.id,
            Client::Browser,
            &device::describe_browser(user_agent),
            AuthMethod::Password,
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        [(SET_COOKIE, auth::session_cookie(&token))],
        // Setup only opens where the admin pages do.
        Json(SessionResponse::new(user, session, true)),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(open_window: bool) -> (tempfile::TempDir, Arc<Setup>) {
        let dir = tempfile::tempdir().unwrap();
        let (setup, _) = Setup::start(SetupOptions {
            data_dir: dir.path().to_owned(),
            gateway_dir: dir.path().join("gateway"),
            authority: Authority::LetsEncryptStaging,
            https_port: 443,
            open_window,
            address: None,
            backup: None,
        })
        .unwrap();
        (dir, setup)
    }

    #[test]
    fn codes_are_ten_lowercase_letters() {
        for _ in 0..100 {
            let code = new_code();
            assert_eq!(code.len(), CODE_LEN);
            assert!(code.bytes().all(|b| b.is_ascii_lowercase()), "{code}");
        }
        assert_ne!(new_code(), new_code());
    }

    #[test]
    fn keeps_the_code_across_restarts_in_a_private_file() {
        let (dir, first) = setup(false);
        let path = dir.path().join(SETUP_CODE_FILE);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let (second, _) = Setup::start(SetupOptions {
            data_dir: dir.path().to_owned(),
            gateway_dir: dir.path().join("gateway"),
            authority: Authority::LetsEncryptStaging,
            https_port: 443,
            open_window: false,
            address: None,
            backup: None,
        })
        .unwrap();
        assert_eq!(first.code, second.code);
        assert_eq!(read_code(&path).unwrap().unwrap(), first.code);

        first.finish();
        assert!(!path.exists());
    }

    #[test]
    fn accepts_the_code_however_its_typed() {
        let (_dir, setup) = setup(false);
        let typed = format!(" {} ", setup.display_code().to_uppercase());
        assert!(setup.claim(Some(&typed)).is_ok());
        assert!(setup.claim(None).is_err());
    }

    #[test]
    fn pauses_after_too_many_wrong_codes() {
        let (_dir, setup) = setup(false);
        let wrong = if setup.code == "aaaaaaaaaa" {
            "bbbbbbbbbb"
        } else {
            "aaaaaaaaaa"
        };
        for _ in 0..MAX_WRONG_CODES {
            let err = setup.claim(Some(wrong)).unwrap_err();
            assert_eq!(err.status(), StatusCode::FORBIDDEN);
        }
        // Even the right code waits until the pause ends.
        let err = setup.claim(Some(&setup.code.clone())).unwrap_err();
        assert_eq!(err.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn the_window_lets_in_one_person_without_the_code() {
        let (_dir, setup) = setup(true);
        assert!(setup.window_open());
        assert!(setup.claim(None).is_ok());
        assert!(!setup.window_open());
        assert!(setup.claim(None).is_err());
    }

    #[test]
    fn the_owner_step_needs_the_handoff_or_the_code() {
        let (_dir, setup) = setup(false);
        setup.add_handoff("handoff");
        assert!(setup.check_owner_step(Some("handoff"), None).is_ok());
        assert!(setup.check_owner_step(Some("other"), None).is_err());
        assert!(
            setup
                .check_owner_step(None, Some(&setup.code.clone()))
                .is_ok()
        );
    }

    #[test]
    fn answers_only_wizard_host_names() {
        for host in [
            "accordion.local",
            "ACCORDION.LOCAL.",
            "accordion.local:8080",
            "accordion-2.local",
            "localhost",
            "192.168.1.20",
            "192.168.1.20:80",
            "[fd00::20]",
            "[fd00::20]:8080",
        ] {
            assert!(is_wizard_host(host), "{host}");
        }
        for host in [
            "evil.example",
            "accordion.local.evil.example",
            "[evil]",
            "fd00::20",
        ] {
            assert!(!is_wizard_host(host), "{host}");
        }
    }
}
