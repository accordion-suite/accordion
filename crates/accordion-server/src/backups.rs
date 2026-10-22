//! `/backups`: where the admin turns on backups, keeps the recovery key,
//! chooses where backups go, and runs a backup or restore test by hand
//! (decision 8 in `docs/decisions`).
//!
//! Like the health page, it opens only from the home network, and only for
//! admins.

use std::time::{SystemTime, UNIX_EPOCH};

use accordion_core::backup::{self, Backup, BackupStatus, KeyState, Outcome, TargetStatus};
use accordion_core::identity::SignedIn;
use accordion_module::{ApiError, Authenticated};
use accordion_types::{BackupTargetId, Role};
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::setup::asset;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/backups",
            get(|| async { asset(BACKUPS_HTML, "text/html; charset=utf-8") }),
        )
        .route(
            "/backups.js",
            get(|| async { asset(BACKUPS_JS, "text/javascript") }),
        )
        .route("/api/backups", get(status))
        .route("/api/backups/key", get(show_key).post(create_key))
        .route("/api/backups/key/confirm", post(confirm_key))
        .route("/api/backups/targets", post(add_target))
        .route("/api/backups/targets/{id}", delete(remove_target))
        .route("/api/backups/targets/{id}/test", post(test_restore))
        .route("/api/backups/run", post(run_now))
        .route("/api/backups/settings", put(settings))
}

const BACKUPS_HTML: &str = include_str!("backups/backups.html");
const BACKUPS_JS: &str = include_str!("backups/backups.js");

/// The backup service, for an admin. Anyone else gets `403 Forbidden`.
fn service(state: &AppState, user: &SignedIn) -> Result<Backup, ApiError> {
    if !matches!(user.user.role, Role::Owner | Role::Admin) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Only admins can manage backups. Ask your admin to check them.",
        ));
    }
    state.backup.clone().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Backups aren't available on this server.",
        )
    })
}

/// Showing or creating the recovery key needs a sign-in from the last 15
/// minutes, like other sensitive actions.
fn check_recent(user: &SignedIn) -> Result<(), ApiError> {
    if user.recently_authenticated {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::FORBIDDEN,
        "For your security, enter your password again before you see the recovery key.",
    ))
}

fn api_error(err: backup::Error) -> ApiError {
    use backup::Error as E;
    let status = match &err {
        E::Db(_) | E::Sqlite(_) | E::Io(_) => {
            tracing::error!(%err, "backup service error");
            return ApiError::server_problem();
        }
        E::BadName | E::NotAbsolute | E::InsideData | E::NoSuchFolder(_) | E::BadHour => {
            StatusCode::BAD_REQUEST
        }
        E::KeyMismatch => StatusCode::BAD_REQUEST,
        E::NotFound => StatusCode::NOT_FOUND,
        E::NoRecoveryKey
        | E::KeyExists
        | E::NoTargets
        | E::Busy
        | E::WrongKey(_)
        | E::TargetExists => StatusCode::CONFLICT,
        E::Restic(_) => StatusCode::UNPROCESSABLE_ENTITY,
    };
    ApiError::new(status, err.to_string())
}

#[derive(Serialize)]
struct StatusView {
    key: &'static str,
    hour: u8,
    running: bool,
    targets: Vec<TargetView>,
}

#[derive(Serialize)]
struct TargetView {
    id: BackupTargetId,
    name: String,
    location: String,
    last_backup: Option<OutcomeView>,
    /// Milliseconds since 1970, for the page to show in local time.
    last_good_backup: Option<u64>,
    last_check: Option<OutcomeView>,
    last_restore_test: Option<OutcomeView>,
}

#[derive(Serialize)]
struct OutcomeView {
    finished_at: u64,
    ok: bool,
    problem: Option<String>,
}

fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
    })
}

impl From<&Outcome> for OutcomeView {
    fn from(outcome: &Outcome) -> Self {
        Self {
            finished_at: millis(outcome.finished_at),
            ok: outcome.ok,
            problem: outcome.problem.clone(),
        }
    }
}

impl From<&TargetStatus> for TargetView {
    fn from(status: &TargetStatus) -> Self {
        Self {
            id: status.target.id,
            name: status.target.name.clone(),
            location: status.target.location.clone(),
            last_backup: status.last_backup.as_ref().map(Into::into),
            last_good_backup: status.last_good_backup.map(millis),
            last_check: status.last_check.as_ref().map(Into::into),
            last_restore_test: status.last_restore_test.as_ref().map(Into::into),
        }
    }
}

impl From<&BackupStatus> for StatusView {
    fn from(status: &BackupStatus) -> Self {
        Self {
            key: match status.key {
                KeyState::None => "none",
                KeyState::Unconfirmed => "unconfirmed",
                KeyState::Confirmed => "confirmed",
            },
            hour: status.hour,
            running: status.running,
            targets: status.targets.iter().map(Into::into).collect(),
        }
    }
}

/// `GET /api/backups`: how backups are doing.
async fn status(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Json<StatusView>, ApiError> {
    let backup = service(&state, &me)?;
    Ok(Json(StatusView::from(&*backup.status().borrow())))
}

#[derive(Serialize)]
struct KeyView {
    /// In groups of five letters.
    recovery_key: String,
}

/// A response the browser mustn't keep, because it holds the recovery key.
fn secret(view: KeyView, status: StatusCode) -> Response {
    let mut response = (status, Json(view)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// `POST /api/backups/key`: turns on backups by creating the recovery key,
/// and returns it once so the admin can keep a copy.
async fn create_key(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Response, ApiError> {
    let backup = service(&state, &me)?;
    check_recent(&me)?;
    let key = backup.create_recovery_key().await.map_err(api_error)?;
    tracing::info!(user = %me.user.id, "created the backup recovery key");
    Ok(secret(
        KeyView {
            recovery_key: key.grouped(),
        },
        StatusCode::CREATED,
    ))
}

/// `GET /api/backups/key`: shows the recovery key again.
async fn show_key(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Response, ApiError> {
    let backup = service(&state, &me)?;
    check_recent(&me)?;
    let key = backup
        .recovery_key()
        .await
        .map_err(api_error)?
        .ok_or_else(|| api_error(backup::Error::NoRecoveryKey))?;
    tracing::info!(user = %me.user.id, "showed the backup recovery key");
    Ok(secret(
        KeyView {
            recovery_key: key.grouped(),
        },
        StatusCode::OK,
    ))
}

#[derive(Deserialize)]
struct ConfirmKey {
    recovery_key: String,
}

/// `POST /api/backups/key/confirm`: records that the admin saved the
/// recovery key, once they type it back.
async fn confirm_key(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Json(request): Json<ConfirmKey>,
) -> Result<StatusCode, ApiError> {
    let backup = service(&state, &me)?;
    backup
        .confirm_recovery_key(&request.recovery_key)
        .await
        .map_err(api_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct NewTarget {
    name: String,
    /// A folder's full path, such as on a USB disk.
    path: String,
}

/// `POST /api/backups/targets`: adds a folder for backups to go to.
async fn add_target(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Json(request): Json<NewTarget>,
) -> Result<StatusCode, ApiError> {
    let backup = service(&state, &me)?;
    let target = backup
        .add_folder(&request.name, std::path::Path::new(request.path.trim()))
        .await
        .map_err(api_error)?;
    tracing::info!(user = %me.user.id, target = %target.id, "added a backup location");
    Ok(StatusCode::CREATED)
}

/// `DELETE /api/backups/targets/{id}`: stops backing up to a location. The
/// backups already there stay.
async fn remove_target(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Path(id): Path<BackupTargetId>,
) -> Result<StatusCode, ApiError> {
    let backup = service(&state, &me)?;
    backup.remove_target(id).await.map_err(api_error)?;
    tracing::info!(user = %me.user.id, target = %id, "removed a backup location");
    Ok(StatusCode::NO_CONTENT)
}

/// Refuses to start a second run while one is going.
fn check_idle(backup: &Backup) -> Result<(), ApiError> {
    if backup.status().borrow().running {
        return Err(api_error(backup::Error::Busy));
    }
    Ok(())
}

/// `POST /api/backups/run`: starts a backup to every location now. It runs
/// in the background, and `GET /api/backups` shows when it's done.
async fn run_now(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<StatusCode, ApiError> {
    let backup = service(&state, &me)?;
    check_idle(&backup)?;
    if backup.status().borrow().targets.is_empty() {
        return Err(api_error(backup::Error::NoTargets));
    }
    tracing::info!(user = %me.user.id, "started a backup");
    tokio::spawn(async move {
        if let Err(err) = backup.back_up().await {
            tracing::warn!(%err, "a backup started by hand failed");
        }
    });
    Ok(StatusCode::ACCEPTED)
}

/// `POST /api/backups/targets/{id}/test`: starts a restore test of one
/// location, in the background.
async fn test_restore(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Path(id): Path<BackupTargetId>,
) -> Result<StatusCode, ApiError> {
    let backup = service(&state, &me)?;
    check_idle(&backup)?;
    let target = backup
        .status()
        .borrow()
        .targets
        .iter()
        .find(|status| status.target.id == id)
        .map(|status| status.target.clone())
        .ok_or_else(ApiError::not_found)?;
    tracing::info!(user = %me.user.id, target = %id, "started a restore test");
    tokio::spawn(async move {
        if let Err(err) = backup.test_restore(&target).await {
            tracing::warn!(%err, "a restore test started by hand failed to run");
        }
    });
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
struct Settings {
    hour: u8,
}

/// `PUT /api/backups/settings`: changes the hour backups start.
async fn settings(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Json(request): Json<Settings>,
) -> Result<StatusCode, ApiError> {
    let backup = service(&state, &me)?;
    backup.set_hour(request.hour).await.map_err(api_error)?;
    Ok(StatusCode::NO_CONTENT)
}
