//! The admin's choices for previews, at `/api/admin/previews`: when videos
//! the browser can't play are converted, and how much of the data disk
//! previews may use (decision 26). Core keeps the settings and does the
//! work.

use accordion_core::events::{Action, NewEvent};
use accordion_core::identity::SignedIn;
use accordion_core::thumbnails::{self, PreviewSettings, Thumbnails, VideoConversion};
use accordion_module::{ApiError, Authenticated};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;

/// The shares of the data disk an admin can choose, in percent.
const SPACE_CHOICES: [u32; 4] = [5, 10, 20, 30];

/// Routes for admins, on the home network.
pub fn admin_routes() -> Router<AppState> {
    Router::new().route("/api/admin/previews", get(previews).put(set_previews))
}

#[derive(Serialize)]
struct PreviewsView {
    /// "upload", "opened", or "never", or null to follow what the server's
    /// hardware suits.
    convert_videos: Option<&'static str>,
    /// What the server's hardware suits, which null follows.
    automatic: &'static str,
    /// The server's processor cores, which `automatic` follows.
    cores: usize,
    /// The share of the data disk previews may use: 5, 10, 20, or 30.
    space_percent: u32,
    /// How many bytes previews use, and may use, and the data disk's size.
    used: u64,
    limit: u64,
    disk: u64,
}

#[derive(Deserialize)]
struct NewPreviews {
    convert_videos: Option<String>,
    space_percent: u32,
}

fn check_admin(me: &SignedIn) -> Result<(), ApiError> {
    if me.user.role.is_admin() {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::FORBIDDEN,
        "Only admins can change how previews are made.",
    ))
}

fn service(state: &AppState) -> Result<&Thumbnails, ApiError> {
    state.thumbnails.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Previews aren't available on this server.",
        )
    })
}

async fn view(thumbnails: &Thumbnails) -> Result<PreviewsView, thumbnails::Error> {
    let settings = thumbnails.settings().await?;
    let space = thumbnails.space().await?;
    Ok(PreviewsView {
        convert_videos: settings.convert_videos.map(VideoConversion::as_str),
        automatic: VideoConversion::automatic().as_str(),
        cores: thumbnails::cores(),
        space_percent: settings.space_percent,
        used: space.used,
        limit: space.limit,
        disk: space.disk,
    })
}

/// `GET /api/admin/previews`: the settings, and how much room previews
/// take.
async fn previews(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Json<PreviewsView>, ApiError> {
    check_admin(&me)?;
    Ok(Json(view(service(&state)?).await?))
}

/// `PUT /api/admin/previews`: changes the settings. Previews over a smaller
/// limit go now, the least recently watched videos first.
async fn set_previews(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Json(request): Json<NewPreviews>,
) -> Result<Json<PreviewsView>, ApiError> {
    check_admin(&me)?;
    let thumbnails = service(&state)?;
    let convert_videos = match request.convert_videos.as_deref() {
        None => None,
        Some(choice) => Some(VideoConversion::parse(choice).ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "Choose when to convert videos: after upload, when first opened, or never.",
            )
        })?),
    };
    if !SPACE_CHOICES.contains(&request.space_percent) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Choose 5, 10, 20, or 30 percent of the disk for previews.",
        ));
    }
    let settings = PreviewSettings {
        convert_videos,
        space_percent: request.space_percent,
    };
    if thumbnails.set_settings(settings).await? {
        let choice = convert_videos.map(VideoConversion::as_str);
        tracing::info!(
            user = %me.user.id,
            convert_videos = choice.unwrap_or("automatic"),
            space_percent = settings.space_percent,
            "changed how previews are made"
        );
        let event = NewEvent::new(Action::PreviewsChanged).details(serde_json::json!({
            "convert_videos": choice,
            "space_percent": settings.space_percent,
        }));
        crate::log::record(&state, &me, event).await;
    }
    Ok(Json(view(thumbnails).await?))
}
