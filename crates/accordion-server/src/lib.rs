//! The Accordion server: one process that serves every module, built on the
//! core services in `accordion-core` (decision 1 in `docs/decisions`).

mod account;
mod backups;
pub mod connections;
mod device;
pub mod health;
mod home;
pub mod modules;
pub mod network;
mod origin;
mod people;
mod requests;
mod session;
pub mod setup;
pub mod updates;
mod welcome;

use std::sync::Arc;

use accordion_core::backup::Backup;
use accordion_core::identity::Identity;
use accordion_core::storage::Storage;
use accordion_gateway::host::reject_unknown_hosts;
use accordion_gateway::{AllowedHosts, ServerName};
use axum::Router;
use axum::extract::FromRef;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{get, post};

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
    /// The routes of the modules that are running, such as Drive.
    pub modules: modules::Mounted,
    /// Backups, once the server is running with an owner.
    pub backup: Option<Backup>,
    /// Everyone's files, for the People page when it deletes an account.
    pub storage: Option<Storage>,
    /// Checks for newer releases, once the server is running.
    pub updates: Option<updates::Updates>,
    /// The Network page's switches, once the server is running.
    pub switches: Option<connections::Switches>,
}

/// Lets the sign-in checks from `accordion-module` find identity.
impl FromRef<AppState> for Identity {
    fn from_ref(state: &AppState) -> Self {
        state.identity.clone()
    }
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
    // Module routes sit inside the same host and cross-site checks as the
    // server's own (decision 7).
    let router = state
        .modules
        .mount(Router::new())
        .route(
            "/api/session",
            get(session::current)
                .post(session::sign_in)
                .delete(session::sign_out),
        )
        .route("/api/session/confirm", post(session::confirm))
        .merge(home::routes())
        .merge(account::routes())
        .merge(welcome::routes())
        .merge(
            setup::owner_routes()
                .merge(health::routes())
                .merge(backups::routes())
                .merge(people::routes())
                .merge(updates::routes())
                .merge(connections::routes())
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
    let router = match site {
        Site::Unnamed => router,
        // Outside every other check, so requests for other names never reach
        // a handler. That blocks DNS rebinding (decision 5).
        Site::Named { name, .. } => router.layer(from_fn_with_state(
            AllowedHosts::new([name.as_str()]),
            reject_unknown_hosts,
        )),
    };
    router.layer(from_fn(requests::log))
}
