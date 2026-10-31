//! The API behind Updates in the web app's Settings: which version this
//! server runs, whether a newer one is out, and when updates install
//! (decision 12).
//!
//! Like the other admin APIs, it answers only on the home network, and only
//! for admins.

mod fetch;
mod install;
mod service;

use std::time::{SystemTime, UNIX_EPOCH};

use accordion_core::identity::SignedIn;
use accordion_module::{ApiError, Authenticated};
use accordion_types::Role;
use accordion_update::protocol::{Outcome, Report};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

pub use service::{
    CheckResult, Error, Installing, Release, UpdateSource, UpdateStatus, Updates, WAIT,
};

use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/updates", get(status))
        .route("/api/updates/check", post(check))
        .route("/api/updates/install", post(install))
        .route("/api/updates/settings", put(settings))
}

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

fn api_error(err: Error) -> ApiError {
    let status = match &err {
        Error::Db(_) | Error::Sqlite(_) | Error::Io(_) | Error::Setup(_) => {
            tracing::error!(%err, "update service error");
            return ApiError::server_problem();
        }
        Error::NothingWaiting | Error::Busy(_) => StatusCode::CONFLICT,
        Error::NeedsInstallCommand(_) | Error::NoHelper => StatusCode::UNPROCESSABLE_ENTITY,
        Error::Download(_) => StatusCode::BAD_GATEWAY,
    };
    ApiError::new(status, err.to_string())
}

#[derive(Serialize)]
struct StatusView {
    installed: String,
    checking: bool,
    last_check: Option<CheckView>,
    newest: Option<ReleaseView>,
    /// Whether `newest` is newer than this server.
    waiting: bool,
    automatic: bool,
    /// Accordion checks for updates on its own. The Network page turns it
    /// off.
    check_on: bool,
    /// Whether this page can install the waiting release, rather than the
    /// install command.
    can_install: bool,
    /// When the waiting release installs on its own, at the earliest:
    /// after the nightly backup on or after this time.
    installs_from: Option<u64>,
    installing: Option<InstallingView>,
    install_problem: Option<String>,
    /// What the update helper did, newest first.
    history: Vec<ReportView>,
    /// The helper's last word on the waiting release, if it didn't install
    /// it.
    failed: Option<ReportView>,
}

#[derive(Serialize)]
struct InstallingView {
    version: String,
    since: u64,
}

#[derive(Serialize)]
struct ReportView {
    version: String,
    from: String,
    /// `installed`, `rolled-back`, or `refused`.
    outcome: &'static str,
    finished_at: u64,
    reason: Option<String>,
}

impl From<&Report> for ReportView {
    fn from(report: &Report) -> Self {
        Self {
            version: report.version.clone(),
            from: report.from.clone(),
            outcome: match report.outcome {
                Outcome::Installed => "installed",
                Outcome::RolledBack => "rolled-back",
                Outcome::Refused => "refused",
            },
            finished_at: millis(report.finished_at),
            reason: report.reason.clone(),
        }
    }
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
            automatic: status.automatic,
            check_on: status.check_on,
            can_install: status.installable().is_some(),
            installs_from: status.installs_from().map(millis),
            installing: status.installing.as_ref().map(|installing| InstallingView {
                version: installing.version.clone(),
                since: millis(installing.since),
            }),
            install_problem: status.install_problem.clone(),
            history: status.history.iter().map(Into::into).collect(),
            failed: status
                .waiting()
                .and_then(|release| status.failed(&release.version))
                .map(Into::into),
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
    updates.check().await.map_err(api_error)?;
    tracing::info!(user = %me.user.id, "checked for updates");
    Ok(Json(StatusView::from(&*updates.status().borrow())))
}

/// `POST /api/updates/install`: downloads the waiting release and hands it to
/// the update helper, which stops the server and installs it. Answers once
/// the download is done.
async fn install(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<StatusCode, ApiError> {
    let updates = service(&state, &me)?;
    updates.install().await.map_err(api_error)?;
    tracing::info!(user = %me.user.id, "started an update");
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
struct Settings {
    automatic: bool,
}

/// `PUT /api/updates/settings`: whether updates install after the nightly
/// backup, or wait for an admin.
async fn settings(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Json(request): Json<Settings>,
) -> Result<StatusCode, ApiError> {
    let updates = service(&state, &me)?;
    updates
        .set_automatic(request.automatic)
        .await
        .map_err(api_error)?;
    tracing::info!(user = %me.user.id, automatic = request.automatic, "changed when updates install");
    Ok(StatusCode::NO_CONTENT)
}
