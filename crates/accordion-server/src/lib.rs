//! The Accordion server: one process that serves every module, built on the
//! core services in `accordion-core` (decision 1 in `docs/decisions`).

mod auth;
mod device;
mod error;
mod origin;
mod session;

use accordion_core::identity::Identity;
use axum::Router;
use axum::routing::get;

/// Shared state for every request handler. Cloning it is cheap.
#[derive(Clone)]
pub struct AppState {
    pub identity: Identity,
}

/// Builds the server's routes.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/session",
            get(session::current)
                .post(session::sign_in)
                .delete(session::sign_out),
        )
        .layer(axum::middleware::from_fn(origin::reject_cross_origin))
        .with_state(state)
}
