//! `/`: where people land after following `accordion.local` or the
//! server's name.
//!
//! Until the web app exists, it sends people to the owner step while the
//! server has no owner, and otherwise shows a short page that links to the
//! health page, rather than a bare 404.

use axum::Router;
use axum::extract::State;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;

use crate::AppState;
use crate::error::ApiError;
use crate::setup::{SETUP_CSS, asset};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(home))
        // Shared by the home, setup, and health pages. Admin pages only open
        // on the home network, so the home page's styles can't live there.
        .route("/setup.css", get(|| async { asset(SETUP_CSS, "text/css") }))
}

const HOME_HTML: &str = include_str!("home/home.html");

async fn home(State(state): State<AppState>) -> Result<Response, ApiError> {
    if !state.identity.has_owner().await? {
        return Ok(Redirect::temporary("/setup").into_response());
    }
    Ok(asset(HOME_HTML, "text/html; charset=utf-8"))
}
