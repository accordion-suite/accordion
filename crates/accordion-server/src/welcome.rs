//! `/welcome`: where someone sets their password from the link an admin
//! gave them, for a new account or after a reset.
//!
//! The link's secret is in the `#` part of the address, which browsers never
//! send to the server or put in `Referer`, so it stays out of logs. The page
//! sends it in the request body instead. Anyone could be holding the link, so
//! unlike the admin pages, this one opens from anywhere.

use accordion_core::identity::{AuthMethod, Client, LinkPurpose};
use accordion_module::ApiError;
use accordion_module::auth;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::header::{SET_COOKIE, USER_AGENT};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::device;
use crate::session::SessionResponse;
use crate::setup::asset;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/welcome",
            get(|| async { asset(WELCOME_HTML, "text/html; charset=utf-8") }),
        )
        .route(
            "/welcome.js",
            get(|| async { asset(WELCOME_JS, "text/javascript") }),
        )
        .route("/api/welcome", post(set_password))
        .route("/api/welcome/check", post(check))
}

const WELCOME_HTML: &str = include_str!("welcome/welcome.html");
const WELCOME_JS: &str = include_str!("welcome/welcome.js");

#[derive(Deserialize)]
struct CheckRequest {
    token: String,
}

#[derive(Serialize)]
struct LinkFor {
    username: String,
    display_name: String,
    /// `new_account` or `reset`.
    purpose: &'static str,
}

/// `POST /api/welcome/check`: whose link it is, so the page can greet them.
/// A used or expired link gets `410 Gone`.
async fn check(
    State(state): State<AppState>,
    Json(request): Json<CheckRequest>,
) -> Result<Json<LinkFor>, ApiError> {
    let (user, purpose) = state
        .identity
        .check_password_link(&request.token)
        .await?
        .ok_or(accordion_core::identity::Error::LinkNotValid)?;
    Ok(Json(LinkFor {
        username: user.username,
        display_name: user.display_name,
        purpose: match purpose {
            LinkPurpose::NewAccount => "new_account",
            LinkPurpose::Reset => "reset",
        },
    }))
}

#[derive(Deserialize)]
struct SetPassword {
    token: String,
    password: String,
}

/// `POST /api/welcome`: sets the password from the link, uses the link up,
/// and signs the person in on this browser.
async fn set_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SetPassword>,
) -> Result<Response, ApiError> {
    let user = state
        .identity
        .use_password_link(&request.token, request.password)
        .await?;
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
    tracing::info!(user = %user.id, session = %session.id, "set a password from a link");
    Ok((
        [(SET_COOKIE, auth::session_cookie(&token))],
        Json(SessionResponse::new(user, session)),
    )
        .into_response())
}
