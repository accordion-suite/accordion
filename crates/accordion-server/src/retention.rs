//! How long Drive keeps trash and older versions, at
//! `/api/admin/storage/retention`. Core keeps the setting and empties what's
//! older in its hourly upkeep.

use accordion_core::events::{Action, NewEvent};
use accordion_core::identity::SignedIn;
use accordion_core::storage::{Retention, Storage};
use accordion_module::{ApiError, Authenticated};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;

/// Routes for admins, on the home network.
pub fn admin_routes() -> Router<AppState> {
    Router::new().route(
        "/api/admin/storage/retention",
        get(retention).put(set_retention),
    )
}

#[derive(Serialize, Deserialize)]
struct RetentionView {
    /// Days an item stays in the trash: 7, 30, 90, or 365.
    trash_days: u32,
    /// Days an older version is kept after a newer one replaces it: 7, 30,
    /// 90, or 365.
    version_days: u32,
}

impl From<Retention> for RetentionView {
    fn from(retention: Retention) -> Self {
        Self {
            trash_days: retention.trash_days,
            version_days: retention.version_days,
        }
    }
}

fn check_admin(me: &SignedIn) -> Result<(), ApiError> {
    if me.user.role.is_admin() {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::FORBIDDEN,
        "Only admins can change how long trash and versions are kept.",
    ))
}

fn storage(state: &AppState) -> Result<&Storage, ApiError> {
    state.storage.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "File storage isn't available on this server.",
        )
    })
}

/// `GET /api/admin/storage/retention`: how long trash and older versions
/// are kept.
async fn retention(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Json<RetentionView>, ApiError> {
    check_admin(&me)?;
    Ok(Json(storage(&state)?.retention().await?.into()))
}

/// `PUT /api/admin/storage/retention`: changes how long trash and older
/// versions are kept. What's now too old goes within the hour.
async fn set_retention(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Json(request): Json<RetentionView>,
) -> Result<Json<RetentionView>, ApiError> {
    check_admin(&me)?;
    let retention = Retention {
        trash_days: request.trash_days,
        version_days: request.version_days,
    };
    if storage(&state)?.set_retention(retention).await? {
        tracing::info!(
            user = %me.user.id,
            trash_days = retention.trash_days,
            version_days = retention.version_days,
            "changed how long trash and versions are kept"
        );
        let event = NewEvent::new(Action::RetentionChanged).details(serde_json::json!({
            "trash_days": retention.trash_days,
            "version_days": retention.version_days,
        }));
        crate::log::record(&state, &me, event).await;
    }
    Ok(Json(request))
}
