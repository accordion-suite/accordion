//! Signing in Accordion's apps through the system browser (decision 3 in
//! `docs/decisions`): OAuth 2.0's authorization code flow with PKCE, for
//! native apps as RFC 8252 describes.
//!
//! 1. The app opens `/oauth/authorize` in the system browser. That's a page
//!    of the web app, which signs the person in if they aren't, checks the
//!    request with `GET /api/oauth/check`, and asks whether to allow the
//!    app.
//! 2. **Allow** calls `POST /api/oauth/approve`, which makes a code and
//!    says where to send the browser: the app's own address on this
//!    computer, such as `http://127.0.0.1:53412/callback`.
//! 3. The app trades the code, with the secret its challenge came from, at
//!    `POST /api/oauth/token`, and gets a session token.
//!
//! Each app is a public client with a fixed ID, so there are no client
//! secrets. Codes only go back to the loopback address on the device the
//! browser runs on, never to a website.

use std::net::IpAddr;

use accordion_core::events::{Actor, Via};
use accordion_core::identity::{AppRequest, Session, is_challenge};
use accordion_module::{ApiError, Authenticated};
use accordion_types::{SessionId, UserId};
use axum::extract::{Query, State};
use axum::http::header::{CACHE_CONTROL, PRAGMA};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::account::app_tunnel;
use crate::network::Connection;
use crate::remote::AppTunnelView;

/// Accordion's apps, as OAuth clients.
const CLIENTS: &[&str] = &[
    // `accordion-sync`, the command-line client for Linux (decision 23).
    "accordion-sync",
    // The desktop app for Windows and macOS (decision 23).
    "accordion-desktop",
];

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/oauth/check", get(check))
        .route("/api/oauth/approve", post(approve))
        .route("/api/oauth/token", post(token))
}

/// What the app asks for, from the address it opened.
#[derive(Deserialize)]
struct Authorize {
    #[serde(default)]
    response_type: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    redirect_uri: String,
    #[serde(default)]
    code_challenge: String,
    #[serde(default)]
    code_challenge_method: String,
}

impl Authorize {
    /// Checks the request. A problem here is never sent back to the app's
    /// address, since that address may be the problem.
    fn check(&self) -> Result<(), ApiError> {
        let bad = |message: &str| ApiError::new(StatusCode::BAD_REQUEST, message);
        if !CLIENTS.contains(&self.client_id.as_str()) {
            return Err(bad(
                "This sign-in request isn't from one of Accordion's apps, so Accordion won't \
                 sign it in. Close this page.",
            ));
        }
        if !is_loopback_redirect(&self.redirect_uri) {
            return Err(bad(
                "This sign-in request would send you somewhere other than the app on this \
                 computer, so Accordion won't sign it in. Close this page.",
            ));
        }
        if self.response_type != "code"
            || self.code_challenge_method != "S256"
            || !is_challenge(&self.code_challenge)
        {
            return Err(bad(
                "The app sent a sign-in request Accordion can't use. Update the app, then try \
                 again.",
            ));
        }
        Ok(())
    }
}

/// Whether `uri` is an address on this computer, with a port, as native apps
/// use to receive the code (RFC 8252, section 7.3): `http://127.0.0.1` or
/// `http://[::1]`, with no query or fragment. `localhost` isn't allowed,
/// since a name can resolve to somewhere else.
fn is_loopback_redirect(uri: &str) -> bool {
    if uri.contains('#') || uri.contains('@') {
        return false;
    }
    let Ok(parsed) = uri.parse::<Uri>() else {
        return false;
    };
    let host = parsed
        .host()
        .unwrap_or_default()
        .trim_start_matches('[')
        .trim_end_matches(']');
    parsed.scheme_str() == Some("http")
        && host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
        && parsed.port_u16().is_some()
        && parsed.query().is_none()
}

/// `GET /api/oauth/check`: whether the web app should offer to allow this
/// request. It needs a session, so a page that isn't signed in asks the
/// person to sign in first.
async fn check(
    Authenticated(_me): Authenticated,
    Query(request): Query<Authorize>,
) -> Result<StatusCode, ApiError> {
    request.check()?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct Approve {
    #[serde(flatten)]
    request: Authorize,
    #[serde(default)]
    state: Option<String>,
    /// What the app calls the device, such as "Sam's laptop".
    #[serde(default)]
    device_name: String,
}

#[derive(Serialize)]
struct Approved {
    /// Where to send the browser: the app's address, with the code.
    redirect: String,
}

/// `POST /api/oauth/approve`: the person allowed the app, so make a code
/// and say where to send it. Adding a device is a sensitive action, so it
/// needs a sign-in from the last 15 minutes, as for app codes.
async fn approve(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Json(approve): Json<Approve>,
) -> Result<Json<Approved>, ApiError> {
    approve.request.check()?;
    if !me.recently_authenticated {
        return Err(ApiError::password_needed(
            "For your security, enter your password again before you sign in the app.",
        ));
    }
    let device_name = if approve.device_name.trim().is_empty() {
        "Accordion app"
    } else {
        &approve.device_name
    };
    let code = state
        .identity
        .approve_app(
            &me,
            AppRequest {
                client_id: &approve.request.client_id,
                redirect_uri: &approve.request.redirect_uri,
                code_challenge: &approve.request.code_challenge,
                device_name,
            },
        )
        .await?;
    let mut redirect = format!("{}?code={}", approve.request.redirect_uri, code.as_str());
    if let Some(app_state) = &approve.state {
        redirect.push_str("&state=");
        redirect.push_str(&encode(app_state));
    }
    tracing::info!(user = %me.user.id, client = approve.request.client_id, "allowed an app to sign in");
    Ok(Json(Approved { redirect }))
}

/// Percent-encodes everything but letters, digits, and `-._~`.
fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The token request, as a form (RFC 6749, section 4.1.3).
#[derive(Deserialize)]
struct TokenRequest {
    #[serde(default)]
    grant_type: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    redirect_uri: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    code_verifier: String,
    /// The app's own WireGuard public key, in base64, to register for its
    /// new session (decision 23).
    #[serde(default)]
    tunnel_key: Option<String>,
}

#[derive(Serialize)]
struct TokenReply {
    /// The app's session token. Send it as `Authorization: Bearer <token>`,
    /// and keep it in the system's keychain.
    access_token: String,
    token_type: &'static str,
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

/// An error in OAuth's own format (RFC 6749, section 5.2).
struct OAuthError {
    error: &'static str,
    description: &'static str,
}

impl IntoResponse for OAuthError {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            [(CACHE_CONTROL, "no-store"), (PRAGMA, "no-cache")],
            Json(serde_json::json!({
                "error": self.error,
                "error_description": self.description,
            })),
        )
            .into_response()
    }
}

/// Why the token endpoint said no: in OAuth's format for the app's mistakes,
/// and Accordion's own for the server's problems.
enum TokenError {
    OAuth(OAuthError),
    Api(ApiError),
}

impl From<ApiError> for TokenError {
    fn from(err: ApiError) -> Self {
        Self::Api(err)
    }
}

impl IntoResponse for TokenError {
    fn into_response(self) -> Response {
        match self {
            Self::OAuth(err) => err.into_response(),
            Self::Api(err) => err.into_response(),
        }
    }
}

/// `POST /api/oauth/token`: trades a code and the app's secret for a
/// session. Nobody is signed in yet, since the code and secret are the
/// proof.
async fn token(
    State(state): State<AppState>,
    connection: Connection,
    Form(request): Form<TokenRequest>,
) -> Result<Response, TokenError> {
    if request.grant_type != "authorization_code" {
        return Err(TokenError::OAuth(OAuthError {
            error: "unsupported_grant_type",
            description: "Accordion's apps sign in with a code from the browser.",
        }));
    }
    let from = connection.sign_in_from();
    let exchanged = state
        .identity
        .acting_as(Actor::visitor(Via::App, from.address))
        .exchange_app_code(
            &request.code,
            &request.client_id,
            &request.redirect_uri,
            &request.code_verifier,
        )
        .await
        .map_err(ApiError::from)?;
    let Some((token, session)) = exchanged else {
        return Err(TokenError::OAuth(OAuthError {
            error: "invalid_grant",
            description: "That sign-in didn't work. It may have taken too long. Sign in from \
                          the app again.",
        }));
    };
    let user = state
        .identity
        .user(session.user_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(ApiError::not_found)?;
    tracing::info!(user = %user.id, session = %session.id, "signed in an app through the browser");
    let (tunnel, tunnel_problem) = registered_tunnel(&state, &session, request.tunnel_key).await;
    Ok((
        [(CACHE_CONTROL, "no-store"), (PRAGMA, "no-cache")],
        Json(TokenReply {
            access_token: token.as_str().to_owned(),
            token_type: "Bearer",
            session_id: session.id,
            user_id: user.id,
            username: user.username,
            display_name: user.display_name,
            tunnel,
            tunnel_problem,
        }),
    )
        .into_response())
}

async fn registered_tunnel(
    state: &AppState,
    session: &Session,
    key: Option<String>,
) -> (Option<AppTunnelView>, Option<String>) {
    match key {
        None => (None, None),
        Some(key) => match app_tunnel(state, session, &key).await {
            Ok(tunnel) => (Some(tunnel), None),
            Err(problem) => (None, Some(problem)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sends_codes_only_to_this_computer() {
        for good in [
            "http://127.0.0.1:53412/callback",
            "http://127.0.0.1:1/",
            "http://[::1]:53412/callback",
        ] {
            assert!(is_loopback_redirect(good), "{good}");
        }
        for bad in [
            "http://localhost:53412/callback",
            "https://evil.example/callback",
            "http://127.0.0.1/callback",
            "http://127.0.0.1:53412/callback?next=x",
            "http://127.0.0.1:53412/callback#x",
            "http://user@127.0.0.1:53412/callback",
            "http://192.168.1.20:53412/callback",
            "accordion://callback",
            "",
        ] {
            assert!(!is_loopback_redirect(bad), "{bad}");
        }
    }

    #[test]
    fn encodes_the_apps_state() {
        assert_eq!(encode("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(encode("Az09-._~"), "Az09-._~");
    }
}
