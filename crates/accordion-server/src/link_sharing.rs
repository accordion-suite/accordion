//! Link sharing (decision 22 in `docs/decisions`): the admin's switch and
//! port, and, while it's on, the listener for link pages, its wildcard
//! certificate, the router's TCP port mapping, and the address record of
//! the links name.
//!
//! The listener serves the modules' link pages and nothing else, under a
//! random label of the server's name, such as `f8q2m.ourhome.dedyn.io`.
//! On a server without a name, for development and tests, it serves them
//! over plain HTTP at the server's home address, and asks no router.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use accordion_core::db::rusqlite::OptionalExtension;
use accordion_core::db::{self, Database, Migration};
use accordion_core::events::{Action, NewEvent};
use accordion_core::identity::SignedIn;
use accordion_gateway::AllowedHosts;
use accordion_gateway::address::AddressSource;
use accordion_gateway::certificates::{CertificateStatus, Renewer, Settings};
use accordion_gateway::dns::{Provider, ProviderKind};
use accordion_gateway::endpoint::{EndpointInput, EndpointPublisher, EndpointStatus};
use accordion_gateway::host::reject_unknown_hosts;
use accordion_gateway::port_mapping::{
    Forward, PortMapper, PortStatus, Protocol, Router as HomeRouter,
};
use accordion_gateway::tls::{self, CertStore};
use accordion_module::{ApiError, Authenticated};
use accordion_types::Role;
use axum::extract::State;
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::{AppState, log};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "link sharing",
    sql: "
    -- One row, made the first time an admin turns link sharing on.
    CREATE TABLE link_sharing (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        turned_on INTEGER NOT NULL CHECK (turned_on IN (0, 1)),
        -- The TCP port links use on the home's public address.
        port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
        -- The random label of the links name, such as f8q2m in
        -- f8q2m.ourhome.dedyn.io.
        label TEXT NOT NULL
    ) STRICT;
    ",
}];

/// The port links use on the home's public address, unless the admin picks
/// another because their provider blocks it.
pub const DEFAULT_PORT: u16 = 443;

/// The port the link listener uses on the server, which the router
/// forwards to.
pub const DEFAULT_LISTEN_PORT: u16 = 8443;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Saved {
    on: bool,
    port: u16,
    label: String,
}

/// Link sharing's settings and, once the server serves its routes, what's
/// running. Cloning it is cheap.
#[derive(Clone)]
pub struct LinkSharing {
    db: Database,
    saved: Arc<watch::Sender<Option<Saved>>>,
    origin: Arc<watch::Sender<Option<String>>>,
    running: Arc<OnceLock<Running>>,
}

/// What runs once the server serves its routes.
struct Running {
    /// The links name and the DNS provider, on a server with a name.
    named: Option<(String, ProviderKind)>,
    home_source: Option<AddressSource>,
    listen_port: u16,
    listener: watch::Receiver<ListenerStatus>,
    certificate: watch::Receiver<CertificateStatus>,
    router: watch::Receiver<PortStatus>,
    record: watch::Receiver<EndpointStatus>,
    router_address: Option<IpAddr>,
}

/// Whether the link listener is up, or why it isn't.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListenerStatus {
    pub listening: bool,
    pub problem: Option<String>,
}

/// How the server is reached, for [`LinkSharing::start`].
pub enum Site {
    /// By its name: links use a random label of it, with a wildcard
    /// certificate, and the router forwards the port.
    Named {
        settings: Box<Settings>,
        gateway_dir: PathBuf,
    },
    /// By its address, over plain HTTP, for development and tests.
    Unnamed,
}

impl LinkSharing {
    pub async fn open(db: Database) -> Result<Self, db::Error> {
        db.migrate("link_sharing", MIGRATIONS).await?;
        let saved = db
            .read(|conn| {
                conn.query_row(
                    "SELECT turned_on, port, label FROM link_sharing",
                    [],
                    |row| {
                        Ok(Saved {
                            on: row.get(0)?,
                            port: row.get(1)?,
                            label: row.get(2)?,
                        })
                    },
                )
                .optional()
            })
            .await?;
        // Until it's first turned on, the label is only in memory, so a
        // server that never shares links never saves one.
        let saved = saved.unwrap_or_else(|| Saved {
            on: false,
            port: DEFAULT_PORT,
            label: random_label(),
        });
        Ok(Self {
            db,
            saved: Arc::new(watch::channel(Some(saved)).0),
            origin: Arc::new(watch::channel(None).0),
            running: Arc::default(),
        })
    }

    /// Where link pages are, while link sharing is on, for the modules.
    pub fn origin(&self) -> watch::Receiver<Option<String>> {
        self.origin.subscribe()
    }

    fn on(&self) -> bool {
        self.saved.borrow().as_ref().is_some_and(|saved| saved.on)
    }

    async fn set(&self, on: bool, port: u16) -> Result<(), db::Error> {
        let label = self
            .saved
            .borrow()
            .as_ref()
            .map_or_else(random_label, |saved| saved.label.clone());
        let saved = Saved { on, port, label };
        let row = saved.clone();
        self.db
            .write(move |tx| {
                tx.execute(
                    "INSERT INTO link_sharing (id, turned_on, port, label) VALUES (1, ?1, ?2, ?3)
                     ON CONFLICT (id) DO UPDATE SET turned_on = excluded.turned_on,
                         port = excluded.port",
                    (row.on, row.port, row.label),
                )
            })
            .await?;
        self.saved.send_replace(Some(saved));
        Ok(())
    }

    /// Starts link sharing's work: the listener on `listen` for `app`, the
    /// modules' link pages, and on a server with a name, its certificate,
    /// port mapping, and record. It runs until each future from `stopped`
    /// completes.
    pub fn start<F, S>(
        &self,
        site: Site,
        home_source: Option<AddressSource>,
        listen: SocketAddr,
        app: Router,
        stopped: F,
    ) -> impl Future<Output = ()> + use<F, S>
    where
        F: Fn() -> S,
        S: Future<Output = ()>,
    {
        let label = self.saved_label();
        let (listener_status, listener) = watch::channel(ListenerStatus::default());
        let (forward, forward_receiver) = watch::channel(None);
        let (input, input_receiver) = watch::channel(None);
        let certificate_status = watch::Sender::new(CertificateStatus::default());
        let certificate = certificate_status.subscribe();
        let certs = CertStore::new();

        let (named, origin, app, router, mapper, publisher, renewing) = match site {
            Site::Named {
                settings,
                gateway_dir,
            } => {
                let links_name = format!("{label}.{}", settings.name);
                let router = HomeRouter::find();
                let mapper = PortMapper::new(router, Protocol::Tcp, forward_receiver);
                let publisher = Provider::new(settings.provider, settings.token.clone())
                    .ok()
                    .map(|dns| EndpointPublisher::new(dns, input_receiver));
                let app = app.layer(from_fn_with_state(
                    AllowedHosts::new([links_name.as_str()]),
                    reject_unknown_hosts,
                ));
                (
                    Some((links_name.clone(), settings.provider)),
                    Origin::Named(links_name),
                    app,
                    router,
                    Some(mapper),
                    publisher,
                    Some((settings, gateway_dir)),
                )
            }
            Site::Unnamed => {
                // The listen address, unless it's every address, in which
                // case the server's home address.
                let home = if listen.ip().is_unspecified() {
                    home_source
                        .unwrap_or(AddressSource::Detect)
                        .address()
                        .map_or_else(|_| "localhost".to_owned(), |home| home.to_string())
                } else {
                    listen.ip().to_string()
                };
                (
                    None,
                    Origin::Unnamed(format!("http://{home}:{}", listen.port())),
                    app,
                    None,
                    None,
                    None,
                    None,
                )
            }
        };
        let running = Running {
            named,
            home_source,
            listen_port: listen.port(),
            listener,
            certificate,
            router: mapper.as_ref().map_or_else(
                || watch::channel(PortStatus::default()).1,
                PortMapper::status,
            ),
            record: publisher.as_ref().map_or_else(
                || watch::channel(EndpointStatus::default()).1,
                EndpointPublisher::status,
            ),
            router_address: router.map(|router| router.pcp.ip()),
        };
        let links_name = running.named.as_ref().map(|(name, _)| name.clone());
        let router_status = running.router.clone();
        let _ = self.running.set(running);

        let sharing = self.clone();
        let tls_config = tls::server_config(certs.clone());
        let named = renewing.is_some();
        async move {
            let drive = sharing.drive(
                origin,
                listen.port(),
                links_name,
                forward,
                input,
                router_status,
                stopped(),
            );
            let listen_loop = sharing.listen_loop(
                listen,
                app,
                named.then_some(tls_config),
                listener_status,
                &stopped,
            );
            let certificate_loop = async {
                if let Some((settings, gateway_dir)) = renewing {
                    sharing
                        .certificate_loop(
                            &settings,
                            &gateway_dir,
                            certs,
                            certificate_status,
                            &stopped,
                        )
                        .await;
                }
            };
            let mapper = async {
                if let Some(mapper) = mapper {
                    mapper.run(stopped()).await;
                }
            };
            let publisher = async {
                if let Some(publisher) = publisher {
                    publisher.run(stopped()).await;
                }
            };
            tokio::join!(drive, listen_loop, certificate_loop, mapper, publisher);
        }
    }

    fn saved_label(&self) -> String {
        self.saved
            .borrow()
            .as_ref()
            .map_or_else(String::new, |saved| saved.label.clone())
    }

    /// Keeps the origin, the port mapping, and the record in step with the
    /// settings and the router.
    #[allow(clippy::too_many_arguments)]
    async fn drive(
        &self,
        origin: Origin,
        listen_port: u16,
        links_name: Option<String>,
        forward: watch::Sender<Option<Forward>>,
        input: watch::Sender<Option<EndpointInput>>,
        mut router: watch::Receiver<PortStatus>,
        stopped: impl Future<Output = ()>,
    ) {
        let mut stopped = std::pin::pin!(stopped);
        let mut saved = self.saved.subscribe();
        loop {
            let settings = saved.borrow_and_update().clone();
            let on = settings.filter(|saved| saved.on);
            let wanted_origin = on.as_ref().map(|saved| origin.with_port(saved.port));
            self.origin.send_if_modified(|current| {
                let changed = *current != wanted_origin;
                *current = wanted_origin;
                changed
            });
            let wanted = on.as_ref().map(|saved| Forward {
                internal: listen_port,
                external: saved.port,
            });
            forward.send_if_modified(|current| {
                let changed = *current != wanted;
                *current = wanted;
                changed
            });
            let router_address = router
                .borrow_and_update()
                .mapping
                .and_then(|mapping| mapping.external_address);
            let wanted = on.and(links_name.clone()).map(|name| EndpointInput {
                name,
                router_address,
            });
            input.send_if_modified(|current| {
                let changed = *current != wanted;
                *current = wanted;
                changed
            });
            tokio::select! {
                () = &mut stopped => return,
                Ok(()) = saved.changed() => {}
                Ok(()) = router.changed() => {}
            }
        }
    }

    /// Waits until link sharing is turned off, or the server stops.
    async fn until_off<S: Future<Output = ()>>(&self, stopped: S) {
        let mut saved = self.saved.subscribe();
        tokio::select! {
            () = stopped => {}
            _ = saved.wait_for(|saved| !saved.as_ref().is_some_and(|saved| saved.on)) => {}
        }
    }

    /// Waits until link sharing is on. Returns `false` if the server
    /// stopped first.
    async fn until_on<S: Future<Output = ()>>(&self, stopped: S) -> bool {
        let mut saved = self.saved.subscribe();
        tokio::select! {
            () = stopped => false,
            on = saved.wait_for(|saved| saved.as_ref().is_some_and(|saved| saved.on)) => on.is_ok(),
        }
    }

    /// Listens for link pages while link sharing is on.
    async fn listen_loop<F, S>(
        &self,
        listen: SocketAddr,
        app: Router,
        tls_config: Option<Arc<rustls::ServerConfig>>,
        status: watch::Sender<ListenerStatus>,
        stopped: &F,
    ) where
        F: Fn() -> S,
        S: Future<Output = ()>,
    {
        while self.until_on(stopped()).await {
            match TcpListener::bind(listen).await {
                Ok(listener) => {
                    tracing::info!(%listen, "serving link pages");
                    status.send_replace(ListenerStatus {
                        listening: true,
                        problem: None,
                    });
                    match &tls_config {
                        Some(config) => {
                            tls::serve(
                                listener,
                                config.clone(),
                                app.clone(),
                                self.until_off(stopped()),
                            )
                            .await;
                        }
                        None => {
                            tls::serve_plain(listener, app.clone(), self.until_off(stopped()))
                                .await;
                        }
                    }
                    status.send_replace(ListenerStatus::default());
                    tracing::info!("stopped serving link pages");
                }
                Err(err) => {
                    let problem = format!(
                        "Accordion couldn't listen for links on port {}: {err}. If another \
                         program uses that port, stop it, or set ACCORDION_LINKS_LISTEN to \
                         another address, then restart Accordion.",
                        listen.port()
                    );
                    tracing::warn!("{problem}");
                    status.send_replace(ListenerStatus {
                        listening: false,
                        problem: Some(problem),
                    });
                    self.until_off(stopped()).await;
                    status.send_replace(ListenerStatus::default());
                }
            }
        }
    }

    /// Keeps the wildcard certificate current while link sharing is on.
    async fn certificate_loop<F, S>(
        &self,
        settings: &Settings,
        gateway_dir: &std::path::Path,
        certs: Arc<CertStore>,
        status: watch::Sender<CertificateStatus>,
        stopped: &F,
    ) where
        F: Fn() -> S,
        S: Future<Output = ()>,
    {
        while self.until_on(stopped()).await {
            let renewer = match Renewer::from_settings(gateway_dir, settings, certs.clone()) {
                Ok(renewer) => renewer.wildcard(),
                Err(err) => {
                    tracing::error!(%err, "couldn't set up the links certificate");
                    status.send_modify(|status| status.last_error = Some(err.to_string()));
                    self.until_off(stopped()).await;
                    continue;
                }
            };
            let mut from = renewer.status();
            let forward = async {
                loop {
                    status.send_replace(from.borrow_and_update().clone());
                    if from.changed().await.is_err() {
                        std::future::pending::<()>().await;
                    }
                }
            };
            tokio::select! {
                () = renewer.run(self.until_off(stopped())) => {}
                () = forward => {}
            }
        }
    }

    /// How link sharing is doing, for the health page. `None` before the
    /// server serves its routes.
    pub fn health(&self) -> Option<LinkHealth> {
        let running = self.running.get()?;
        let saved = self.saved.borrow().clone();
        Some(LinkHealth {
            on: saved.as_ref().is_some_and(|saved| saved.on),
            named: running.named.is_some(),
            port: saved.map_or(DEFAULT_PORT, |saved| saved.port),
            listen_port: running.listen_port,
            home: home_address(running.home_source),
            listener: running.listener.borrow().clone(),
            certificate: running.certificate.borrow().clone(),
            router: running.router.borrow().clone(),
            record: running.record.borrow().clone(),
        })
    }

    /// Link sharing's connections, for the Network page, while it's on.
    pub fn network(&self) -> Option<LinkNetwork> {
        let running = self.running.get()?;
        let (name, provider) = running.named.as_ref()?;
        if !self.on() {
            return None;
        }
        let router = running.router.borrow();
        let record = running.record.borrow();
        let certificate = running.certificate.borrow();
        Some(LinkNetwork {
            name: name.clone(),
            router: running.router_address.map(|address| address.to_string()),
            router_last: last(
                router.last_attempt,
                router.mapping.is_some(),
                &router.problem,
            ),
            record_host: provider.api_host(),
            lookup_host: provider.public_address_host(),
            uses_lookup: router
                .mapping
                .is_none_or(|mapping| mapping.external_address.is_none()),
            record_last: last(
                record.last_attempt,
                record.problem.is_none(),
                &record.problem,
            ),
            certificate_last: last(
                certificate.last_attempt,
                certificate.failures == 0,
                &certificate.last_error,
            ),
        })
    }
}

fn last(
    at: Option<SystemTime>,
    ok: bool,
    problem: &Option<String>,
) -> Option<crate::connections::LastView> {
    at.map(|at| crate::connections::LastView {
        at: crate::connections::millis(at),
        ok,
        problem: problem.clone(),
    })
}

fn home_address(source: Option<AddressSource>) -> Option<IpAddr> {
    source.unwrap_or(AddressSource::Detect).address().ok()
}

/// Where link pages are, before the port is known.
enum Origin {
    /// The links name.
    Named(String),
    /// The whole origin, on a server without a name.
    Unnamed(String),
}

impl Origin {
    fn with_port(&self, port: u16) -> String {
        match self {
            Self::Named(name) if port == 443 => format!("https://{name}"),
            Self::Named(name) => format!("https://{name}:{port}"),
            Self::Unnamed(origin) => origin.clone(),
        }
    }
}

/// How link sharing is doing, as of now.
pub struct LinkHealth {
    pub on: bool,
    pub named: bool,
    pub port: u16,
    pub listen_port: u16,
    pub home: Option<IpAddr>,
    pub listener: ListenerStatus,
    pub certificate: CertificateStatus,
    pub router: PortStatus,
    pub record: EndpointStatus,
}

/// Link sharing's connections, for the Network page.
#[derive(Serialize)]
pub struct LinkNetwork {
    name: String,
    router: Option<String>,
    router_last: Option<crate::connections::LastView>,
    record_host: &'static str,
    lookup_host: &'static str,
    uses_lookup: bool,
    record_last: Option<crate::connections::LastView>,
    certificate_last: Option<crate::connections::LastView>,
}

/// Five random lowercase letters and digits: short, since it's in every
/// link, and still not one a scanner would try.
fn random_label() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = [0u8; 5];
    getrandom::fill(&mut bytes).expect("the operating system's random number generator works");
    bytes
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(*byte) % ALPHABET.len()]))
        .collect()
}

pub fn admin_routes() -> Router<AppState> {
    Router::new().route("/api/link-sharing", get(status).put(set))
}

fn require_admin(user: &SignedIn) -> Result<(), ApiError> {
    if matches!(user.user.role, Role::Owner | Role::Admin) {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::FORBIDDEN,
        "Only admins can change link sharing. Ask your admin.",
    ))
}

fn sharing(state: &AppState) -> Result<&LinkSharing, ApiError> {
    state.link_sharing.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Link sharing isn't available on this server.",
        )
    })
}

fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
    })
}

#[derive(Serialize)]
struct StatusView {
    available: bool,
    named: bool,
    on: bool,
    port: u16,
    listen_port: Option<u16>,
    /// Where link pages are, while it's on.
    origin: Option<String>,
    home_address: Option<String>,
    listening: bool,
    listen_problem: Option<String>,
    certificate: Option<CertificateView>,
    router: Option<RouterView>,
    record: Option<RecordView>,
}

#[derive(Serialize)]
struct CertificateView {
    renewing: bool,
    expires_at: Option<u64>,
    problem: Option<String>,
}

#[derive(Serialize)]
struct RouterView {
    method: Option<String>,
    external_address: Option<String>,
    shared_address: bool,
    problem: Option<String>,
}

#[derive(Serialize)]
struct RecordView {
    ipv4: Option<String>,
    ipv4_set_by_provider: bool,
    ipv6: Option<String>,
    problem: Option<String>,
}

/// `GET /api/link-sharing`: the settings, and how each part is doing.
async fn status(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Json<StatusView>, ApiError> {
    require_admin(&me)?;
    let sharing = sharing(&state)?;
    let saved = sharing.saved.borrow().clone();
    let on = saved.as_ref().is_some_and(|saved| saved.on);
    let running = sharing.running.get();
    let named = running.is_some_and(|running| running.named.is_some());
    let shown = |_: &&Running| on && named;
    Ok(Json(StatusView {
        available: running.is_some(),
        named,
        on,
        port: saved.as_ref().map_or(DEFAULT_PORT, |saved| saved.port),
        listen_port: running.map(|running| running.listen_port),
        origin: sharing.origin.borrow().clone(),
        home_address: running
            .and_then(|running| home_address(running.home_source))
            .map(|home| home.to_string()),
        listening: running.is_some_and(|running| running.listener.borrow().listening),
        listen_problem: running.and_then(|running| running.listener.borrow().problem.clone()),
        certificate: running.filter(shown).map(|running| {
            let certificate = running.certificate.borrow();
            CertificateView {
                renewing: certificate.renewing,
                expires_at: certificate
                    .validity
                    .map(|validity| millis(validity.expires_at)),
                problem: certificate.last_error.clone(),
            }
        }),
        router: running.filter(shown).map(|running| {
            let router = running.router.borrow();
            RouterView {
                method: router.mapping.map(|mapping| mapping.method.to_string()),
                external_address: router
                    .mapping
                    .and_then(|mapping| mapping.external_address)
                    .map(|address| address.to_string()),
                shared_address: router.shared_address,
                problem: router.problem.clone(),
            }
        }),
        record: running.filter(shown).map(|running| {
            let record = running.record.borrow();
            RecordView {
                ipv4: record.ipv4.map(|address| address.to_string()),
                ipv4_set_by_provider: record.ipv4_set_by_provider,
                ipv6: record.ipv6.map(|address| address.to_string()),
                problem: record.problem.clone(),
            }
        }),
    }))
}

#[derive(Deserialize)]
struct Change {
    on: bool,
    #[serde(default = "default_port")]
    port: u16,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

/// `PUT /api/link-sharing`: turns link sharing on or off, and sets the
/// port links use on the home's public address.
async fn set(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Json(change): Json<Change>,
) -> Result<StatusCode, ApiError> {
    require_admin(&me)?;
    let sharing = sharing(&state)?;
    if sharing.running.get().is_none() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Link sharing isn't running on this server yet. Try again in a moment.",
        ));
    }
    if change.port == 0 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Choose a port from 1 to 65535.",
        ));
    }
    let before = sharing.saved.borrow().clone();
    sharing.set(change.on, change.port).await.map_err(|err| {
        tracing::error!(%err, "couldn't save the link sharing settings");
        ApiError::server_problem()
    })?;
    if before.is_none_or(|before| before.on != change.on || before.port != change.port) {
        let details = if change.on {
            serde_json::json!({ "on": true, "port": change.port })
        } else {
            serde_json::json!({ "on": false })
        };
        log::record(
            &state,
            &me,
            NewEvent::new(Action::LinkSharingChanged).details(details),
        )
        .await;
    }
    tracing::info!(
        user = %me.user.id,
        port = change.port,
        "turned link sharing {}",
        if change.on { "on" } else { "off" }
    );
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_short_labels_and_origins() {
        let label = random_label();
        assert_eq!(label.len(), 5);
        assert!(
            label
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
        let named = Origin::Named("f8q2m.ourhome.dedyn.io".into());
        assert_eq!(named.with_port(443), "https://f8q2m.ourhome.dedyn.io");
        assert_eq!(named.with_port(8443), "https://f8q2m.ourhome.dedyn.io:8443");
    }
}
