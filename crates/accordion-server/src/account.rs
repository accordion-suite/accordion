//! The API behind the web app's Account and Apps and devices settings:
//! app passwords, so standard clients such as File Explorer, Finder, and
//! phone apps can reach Drive (decision 3 in `docs/decisions`), codes that
//! add Accordion's own apps (decision 23), changing a password, and changing
//! a name.
//!
//! Every user can use it from anywhere, unlike the admin APIs.

use accordion_core::events::{Actor, Via};
use accordion_core::identity::{self, AppPassword, AppPasswordUse, Client, Session};
use accordion_gateway::tunnel::{Key, PublicKey};
use accordion_module::auth::actor;
use accordion_module::{ApiError, Authenticated};
use accordion_types::{AppPasswordId, SessionId, UserId};
use axum::extract::{Path, State};
use axum::http::header::{HOST, ORIGIN};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::network::Connection;
use crate::remote::AppTunnelView;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/app-passwords",
            get(app_passwords).post(create_app_password),
        )
        .route("/api/app-passwords/{id}", delete(delete_app_password))
        .route("/api/sessions", get(sessions))
        .route("/api/sessions/{id}", delete(end_session))
        .route("/api/app-codes", post(make_app_code))
        .route("/api/app-codes/redeem", post(redeem_app_code))
        .route("/api/account/password", post(change_password))
        .route("/api/account/name", put(rename))
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum UseFor {
    /// WebDAV, CalDAV, and CardDAV: files, calendars, and contacts.
    Dav,
    Mail,
}

impl From<UseFor> for AppPasswordUse {
    fn from(use_for: UseFor) -> Self {
        match use_for {
            UseFor::Dav => Self::Dav,
            UseFor::Mail => Self::Mail,
        }
    }
}

#[derive(Serialize)]
struct AppPasswordView {
    id: AppPasswordId,
    device_name: String,
    use_for: UseFor,
    created_at: String,
    last_used_at: Option<String>,
}

impl From<AppPassword> for AppPasswordView {
    fn from(password: AppPassword) -> Self {
        Self {
            id: password.id,
            device_name: password.device_name,
            use_for: match password.use_for {
                AppPasswordUse::Dav => UseFor::Dav,
                AppPasswordUse::Mail => UseFor::Mail,
            },
            created_at: password.created_at,
            last_used_at: password.last_used_at,
        }
    }
}

/// `GET /api/app-passwords`: the signed-in user's app passwords, newest
/// first.
async fn app_passwords(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Json<Vec<AppPasswordView>>, ApiError> {
    let passwords = state.identity.app_passwords(me.user.id).await?;
    Ok(Json(passwords.into_iter().map(Into::into).collect()))
}

#[derive(Deserialize)]
struct NewAppPassword {
    device_name: String,
    use_for: UseFor,
}

#[derive(Serialize)]
struct CreatedAppPassword {
    app_password: AppPasswordView,
    /// Shown once, in groups of four letters.
    password: String,
}

/// `POST /api/app-passwords`: creates an app password for one device. It's
/// a sensitive action, so it needs a sign-in from the last 15 minutes.
/// Otherwise the answer is `403 Forbidden`, and the user confirms their
/// password with `POST /api/session/confirm` first.
async fn create_app_password(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Json(request): Json<NewAppPassword>,
) -> Result<(StatusCode, Json<CreatedAppPassword>), ApiError> {
    if !me.recently_authenticated {
        return Err(ApiError::password_needed(
            "For your security, enter your password again before you create an app password.",
        ));
    }
    let (secret, created) = state
        .identity
        .acting_as(actor(&me))
        .create_app_password(me.user.id, request.use_for.into(), &request.device_name)
        .await?;
    tracing::info!(
        user = %me.user.id,
        app_password = %created.id,
        "created an app password"
    );
    Ok((
        StatusCode::CREATED,
        Json(CreatedAppPassword {
            app_password: created.into(),
            password: secret.grouped(),
        }),
    ))
}

/// `DELETE /api/app-passwords/{id}`: deletes one of the signed-in user's app
/// passwords, so its device can't sign in with it again.
async fn delete_app_password(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Path(id): Path<AppPasswordId>,
) -> Result<StatusCode, ApiError> {
    if !state
        .identity
        .acting_as(actor(&me))
        .delete_app_password(me.user.id, id)
        .await?
    {
        return Err(ApiError::not_found());
    }
    tracing::info!(user = %me.user.id, app_password = %id, "deleted an app password");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct SessionView {
    id: SessionId,
    device_name: String,
    /// "browser" or "app".
    client: &'static str,
    created_at: String,
    last_used_at: String,
    /// The session making this request.
    current: bool,
}

impl SessionView {
    fn new(session: Session, current: SessionId) -> Self {
        Self {
            current: session.id == current,
            id: session.id,
            device_name: session.device_name,
            client: match session.client {
                Client::Browser => "browser",
                Client::App => "app",
            },
            created_at: session.created_at,
            last_used_at: session.last_used_at,
        }
    }
}

/// `GET /api/sessions`: the browsers and apps the signed-in user is signed
/// in on, most recently used first (decision 3).
async fn sessions(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Json<Vec<SessionView>>, ApiError> {
    let sessions = state.identity.sessions(me.user.id).await?;
    Ok(Json(
        sessions
            .into_iter()
            .map(|session| SessionView::new(session, me.session.id))
            .collect(),
    ))
}

/// `DELETE /api/sessions/{id}`: signs out one of the signed-in user's
/// browsers or apps. It stops working at once, and its live updates and
/// syncing end.
async fn end_session(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Path(id): Path<SessionId>,
) -> Result<StatusCode, ApiError> {
    if !state
        .identity
        .acting_as(actor(&me))
        .end_session(me.user.id, id)
        .await?
    {
        return Err(ApiError::not_found());
    }
    tracing::info!(user = %me.user.id, session = %id, "signed out a device");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct MadeAppCode {
    /// Shown once, in groups of four letters. Typing it works where the app
    /// reaches the server directly, such as at home.
    code: String,
    expires_at: String,
    /// With remote access on, the code with everything an app away from
    /// home needs to reach the server, including a temporary tunnel key.
    /// It's shown once, as a QR code and as text to copy.
    setup: Option<String>,
    /// The setup as a QR code, in SVG.
    qr_code: Option<String>,
}

/// `POST /api/app-codes`: makes a code that adds one of Accordion's apps
/// for the signed-in user, replacing any code they made before. It works
/// once, for 10 minutes. Adding a device is a sensitive action, so it needs
/// a sign-in from the last 15 minutes, as for app passwords.
///
/// With remote access on, the code also carries a temporary tunnel key
/// (decision 23), which reaches only the address that redeems the code.
async fn make_app_code(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<MadeAppCode>), ApiError> {
    if !me.recently_authenticated {
        return Err(ApiError::password_needed(
            "For your security, enter your password again before you add an app.",
        ));
    }
    let reach = state
        .remote
        .as_ref()
        .and_then(|remote| remote.reachable().ok());
    let key = reach.as_ref().map(|_| Key::generate());
    let made = state
        .identity
        .make_app_code(&me, key.as_ref().map(|key| *key.public().as_bytes()))
        .await?;
    let setup = match (&reach, &key, made.tunnel_host) {
        (Some(reach), Some(key), Some(host)) => {
            Some(reach.code_setup(&origin_of(&headers), made.code.plain(), key, host))
        }
        _ => None,
    };
    let qr_code = setup
        .as_deref()
        .map(crate::remote::qr_svg)
        .transpose()
        .map_err(|err| {
            tracing::error!(%err, "couldn't make an app code's QR code");
            ApiError::server_problem()
        })?;
    tracing::info!(user = %me.user.id, tunnel = setup.is_some(), "made a code to add an app");
    Ok((
        StatusCode::CREATED,
        Json(MadeAppCode {
            code: made.code.grouped(),
            expires_at: made.expires_at,
            setup,
            qr_code,
        }),
    ))
}

/// The address the app should use for the server: the one the browser
/// asking for the code used.
fn origin_of(headers: &HeaderMap) -> String {
    if let Some(origin) = headers.get(ORIGIN).and_then(|value| value.to_str().ok())
        && origin != "null"
    {
        return origin.to_owned();
    }
    let host = headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("localhost");
    // A server with a name answers only to it, over HTTPS. One reached at
    // an address is a development server, which serves plain HTTP.
    let bare = host
        .rsplit_once(':')
        .filter(|(_, port)| port.parse::<u16>().is_ok())
        .map_or(host, |(bare, _)| bare);
    let bare = bare.trim_start_matches('[').trim_end_matches(']');
    let unnamed = bare == "localhost" || bare.parse::<std::net::IpAddr>().is_ok();
    format!("{}://{host}", if unnamed { "http" } else { "https" })
}

#[derive(Deserialize)]
struct RedeemAppCode {
    code: String,
    /// What the app calls the device, such as "Sam's laptop".
    device_name: String,
    /// The app's own WireGuard public key, in base64, to register for its
    /// new session.
    #[serde(default)]
    tunnel_key: Option<String>,
}

#[derive(Serialize)]
struct AppSignedIn {
    /// The app's session token. Send it as `Authorization: Bearer <token>`,
    /// and keep it in the system's keychain.
    token: String,
    session_id: SessionId,
    user_id: UserId,
    username: String,
    display_name: String,
    /// How the app reaches the server away from home, if it sent a tunnel
    /// key and remote access is on.
    tunnel: Option<AppTunnelView>,
    /// Why it can't, if it sent a key.
    tunnel_problem: Option<String>,
}

/// `POST /api/app-codes/redeem`: trades a code from **Apps and devices** for
/// an app session. Nobody is signed in yet, since the code is the proof.
async fn redeem_app_code(
    State(state): State<AppState>,
    connection: Connection,
    Json(request): Json<RedeemAppCode>,
) -> Result<Json<AppSignedIn>, ApiError> {
    let from = connection.sign_in_from();
    let identity = state
        .identity
        .acting_as(Actor::visitor(Via::App, from.address));
    let Some((token, session)) = identity
        .redeem_app_code(&request.code, &request.device_name)
        .await?
    else {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "That code doesn't work. It may have expired or been used already. Make a new one \
             in Settings, under Apps and devices, then try again.",
        ));
    };
    let user = state
        .identity
        .user(session.user_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tracing::info!(user = %user.id, session = %session.id, "added an app with a code");
    let (tunnel, tunnel_problem) = match request.tunnel_key {
        None => (None, None),
        Some(key) => match app_tunnel(&state, &session, &key).await {
            Ok(tunnel) => (Some(tunnel), None),
            Err(problem) => (None, Some(problem)),
        },
    };
    Ok(Json(AppSignedIn {
        token: token.as_str().to_owned(),
        session_id: session.id,
        user_id: user.id,
        username: user.username,
        display_name: user.display_name,
        tunnel,
        tunnel_problem,
    }))
}

/// Registers a new app's tunnel key. A problem doesn't stop the app being
/// added, since it can still sync at home, so it's returned as a message.
pub(crate) async fn app_tunnel(
    state: &AppState,
    session: &Session,
    key: &str,
) -> Result<AppTunnelView, String> {
    let key = PublicKey::from_base64(key).map_err(|err| err.to_string())?;
    let remote = state
        .remote
        .as_ref()
        .ok_or("Remote access isn't available on this server.")?;
    remote
        .set_app_tunnel(&state.identity, session, key)
        .await
        .map_err(|err| err.message().to_owned())
}

/// Checks the signed-in user's password, for changes that need it.
async fn check_current_password(
    state: &AppState,
    me: &identity::SignedIn,
    password: &str,
    connection: &Connection,
) -> Result<(), ApiError> {
    match state
        .identity
        .check_password(&me.user.username, password, connection.sign_in_from())
        .await
    {
        Ok(_) => Ok(()),
        Err(identity::Error::WrongCredentials) => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Your current password isn't right. Check it, then try again.",
        )),
        Err(err) => Err(err.into()),
    }
}

#[derive(Deserialize)]
struct NewPassword {
    current_password: String,
    new_password: String,
}

/// `POST /api/account/password`: changes the signed-in user's password, and
/// signs out their other browsers and apps, in case someone else knew the
/// old one. App passwords keep working.
async fn change_password(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    connection: Connection,
    Json(request): Json<NewPassword>,
) -> Result<StatusCode, ApiError> {
    check_current_password(&state, &me, &request.current_password, &connection).await?;
    state
        .identity
        .acting_as(actor(&me))
        .set_password(me.user.id, request.new_password, Some(me.session.id))
        .await?;
    tracing::info!(user = %me.user.id, "changed their password");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct NewName {
    username: String,
    display_name: String,
    /// Needed only for a new username, which is what everyone signs in with.
    #[serde(default)]
    current_password: String,
}

/// `PUT /api/account/name`: changes the signed-in user's name, and their
/// username if they give their password.
async fn rename(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    connection: Connection,
    Json(request): Json<NewName>,
) -> Result<StatusCode, ApiError> {
    let new_username = request.username.trim().to_lowercase() != me.user.username;
    if new_username {
        check_current_password(&state, &me, &request.current_password, &connection).await?;
    }
    let user = state
        .identity
        .acting_as(actor(&me))
        .rename_member(me.user.id, &request.username, &request.display_name)
        .await?;
    if user.username != me.user.username {
        crate::people::follow_new_username(&state).await;
    }
    tracing::info!(user = %me.user.id, "changed their name");
    Ok(StatusCode::NO_CONTENT)
}
