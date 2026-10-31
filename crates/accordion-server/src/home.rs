//! `/`: where people land after following `accordion.local` or the
//! server's name.
//!
//! It sends people to the owner step while the server has no owner, and
//! then to Drive, as Google's sites open on an app. A server without the web
//! app, such as one for development, shows a short page that says so instead.

use axum::Router;
use axum::extract::State;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;

use crate::AppState;
use crate::setup::{SETUP_CSS, asset};
use accordion_module::ApiError;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(home))
        // Shared by the home, setup, and welcome pages, which open from
        // anywhere.
        .route("/setup.css", get(|| async { asset(SETUP_CSS, "text/css") }))
}

const HOME_HTML: &str = include_str!("home/home.html");

async fn home(State(state): State<AppState>) -> Result<Response, ApiError> {
    if !state.identity.has_owner().await? {
        return Ok(Redirect::temporary("/setup").into_response());
    }
    // Temporary, so browsers ask again once other apps can be chosen here.
    if state.web.is_some() && state.modules.in_web_app().contains(&"drive") {
        // The app opens the person's start page from here, Home or My Drive.
        return Ok(Redirect::temporary("/drive/start").into_response());
    }
    Ok(asset(HOME_HTML, "text/html; charset=utf-8"))
}
