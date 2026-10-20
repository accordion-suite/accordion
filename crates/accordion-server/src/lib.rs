//! The Accordion server: one process that serves every module, built on the
//! core services in `accordion-core` (decision 1 in `docs/decisions`).

mod auth;
mod device;
mod error;
pub mod health;
mod home;
pub mod network;
mod origin;
mod session;
pub mod setup;

use std::sync::Arc;

use accordion_core::identity::Identity;
use accordion_gateway::host::reject_unknown_hosts;
use accordion_gateway::{AllowedHosts, ServerName};
use axum::Router;
use axum::middleware::from_fn_with_state;
use axum::routing::get;

/// Shared state for every request handler. Cloning it is cheap.
#[derive(Clone)]
pub struct AppState {
    pub identity: Identity,
    /// The setup wizard, until the server has an owner.
    pub setup: Option<Arc<setup::Setup>>,
    /// What the health page reports.
    pub health: health::Health,
    /// Who can open the setup and admin pages.
    pub admin_access: network::AdminAccess,
}

/// How clients reach the server.
#[derive(Clone, Debug)]
pub enum Site {
    /// No name is set yet, so the server serves plain HTTP for development
    /// and compares `Origin` with the `Host` header.
    Unnamed,
    /// The server answers only to `name`, over HTTPS at `origin`.
    Named { name: ServerName, origin: String },
}

impl Site {
    /// The site for `name`, served on HTTPS `port`.
    pub fn named(name: ServerName, port: u16) -> Self {
        let origin = if port == 443 {
            format!("https://{name}")
        } else {
            format!("https://{name}:{port}")
        };
        Self::Named { name, origin }
    }
}

/// Builds the server's routes.
pub fn router(state: AppState, site: &Site) -> Router {
    let expected_origin = match site {
        Site::Unnamed => None,
        Site::Named { origin, .. } => Some(Arc::<str>::from(origin.as_str())),
    };
    let router = Router::new()
        .route(
            "/api/session",
            get(session::current)
                .post(session::sign_in)
                .delete(session::sign_out),
        )
        .merge(home::routes())
        .merge(
            setup::owner_routes()
                .merge(health::routes())
                .route_layer(from_fn_with_state(
                    state.admin_access,
                    network::only_home_network,
                )),
        )
        .layer(from_fn_with_state(
            expected_origin,
            origin::reject_cross_origin,
        ))
        .with_state(state);
    match site {
        Site::Unnamed => router,
        // The outermost layer, so requests for other names never reach a
        // handler. That blocks DNS rebinding (decision 5).
        Site::Named { name, .. } => router.layer(from_fn_with_state(
            AllowedHosts::new([name.as_str()]),
            reject_unknown_hosts,
        )),
    }
}
