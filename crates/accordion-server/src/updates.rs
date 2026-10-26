//! `/updates`: which version this server runs, and whether a newer one is
//! out (decision 12).
//!
//! Like the other admin pages, it opens only from the home network, and only
//! for admins.

mod fetch;
mod service;

use std::time::{SystemTime, UNIX_EPOCH};

use accordion_core::identity::SignedIn;
use accordion_module::{ApiError, Authenticated};
use accordion_types::Role;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

pub use service::{CheckResult, Error, Release, UpdateSource, UpdateStatus, Updates};

use crate::AppState;
use crate::setup::asset;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/updates",
            get(|| async { asset(UPDATES_HTML, "text/html; charset=utf-8") }),
        )
        .route(
            "/updates.js",
            get(|| async { asset(UPDATES_JS, "text/javascript") }),
        )
        .route("/api/updates", get(status))
        .route("/api/updates/check", post(check))
}

const UPDATES_HTML: &str = include_str!("updates/updates.html");
const UPDATES_JS: &str = include_str!("updates/updates.js");

/// The update checker, for an admin. Anyone else gets `403 Forbidden`.
fn service(state: &AppState, user: &SignedIn) -> Result<Updates, ApiError> {
    if !matches!(user.user.role, Role::Owner | Role::Admin) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Only admins can manage updates. Ask your admin to check them.",
        ));
    }
    state.updates.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Updates aren't available on this server.",
        )
    })
}

#[derive(Serialize)]
struct StatusView {
    installed: String,
    checking: bool,
    last_check: Option<CheckView>,
    newest: Option<ReleaseView>,
    /// Whether `newest` is newer than this server.
    waiting: bool,
}

#[derive(Serialize)]
struct CheckView {
    /// Milliseconds since 1970, for the page to show in local time.
    at: u64,
    problem: Option<String>,
}

#[derive(Serialize)]
struct ReleaseView {
    version: String,
    /// Such as `2026-11-02`.
    published: String,
    security: bool,
    installs_itself: bool,
    notes: String,
    link: String,
}

fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
    })
}

impl From<&UpdateStatus> for StatusView {
    fn from(status: &UpdateStatus) -> Self {
        Self {
            installed: status.installed.to_string(),
            checking: status.checking,
            last_check: status.last_check.as_ref().map(|check| CheckView {
                at: millis(check.at),
                problem: check.problem.clone(),
            }),
            newest: status.newest.as_ref().map(|release| ReleaseView {
                version: release.version.to_string(),
                published: release.published.clone(),
                security: release.security,
                installs_itself: release.installs_itself,
                notes: release.notes.clone(),
                link: release.link.clone(),
            }),
            waiting: status.waiting().is_some(),
        }
    }
}

/// `GET /api/updates`: the installed version and the newest release.
async fn status(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Json<StatusView>, ApiError> {
    let updates = service(&state, &me)?;
    Ok(Json(StatusView::from(&*updates.status().borrow())))
}

/// `POST /api/updates/check`: checks for a newer release now, and answers
/// with what it found. A check that doesn't work still answers `200 OK`, with
/// its problem in `last_check`, like a scheduled one.
async fn check(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Json<StatusView>, ApiError> {
    let updates = service(&state, &me)?;
    updates.check().await.map_err(|err| {
        tracing::error!(%err, "couldn't record an update check");
        ApiError::server_problem()
    })?;
    tracing::info!(user = %me.user.id, "checked for updates");
    Ok(Json(StatusView::from(&*updates.status().borrow())))
}
