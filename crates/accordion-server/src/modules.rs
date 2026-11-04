//! The modules this server includes, such as Drive, and how they're started
//! and mounted (decision 7 in `docs/decisions`).
//!
//! Every module in [`all`] starts when the server does. Turning modules on
//! and off comes with the admin's **Modules** page, which will start and
//! stop them without a restart.

use std::error::Error;
use std::sync::Arc;

use accordion_module::{Module, Routes, Services, Stopping};
use axum::Router;
use tokio::task::JoinSet;

/// Every module built into this server.
pub fn all() -> Vec<Box<dyn Module>> {
    vec![Box::new(accordion_drive::Drive)]
}

/// The routes of the modules that are running. Cloning it is cheap.
#[derive(Clone, Default)]
pub struct Mounted(Arc<Vec<(&'static str, Routes)>>);

/// Starts `modules`, and runs their background work in `tasks` until
/// `stopping` fires.
pub async fn start(
    modules: Vec<Box<dyn Module>>,
    services: &Services,
    stopping: &Stopping,
    tasks: &mut JoinSet<()>,
) -> Result<Mounted, Box<dyn Error>> {
    let mut mounted = Vec::new();
    for module in modules {
        let id = module.id();
        check_id(id);
        if mounted.iter().any(|&(other, _)| other == id) {
            panic!("two modules have the ID {id}");
        }
        let started = module
            .start(services.clone(), stopping.clone())
            .await
            .map_err(|err| format!("Accordion couldn't start {}: {err}", module.name()))?;
        if let Some(background) = started.background {
            tasks.spawn(background);
        }
        tracing::info!("started {}", module.name());
        mounted.push((id, started.routes));
    }
    check_dav_names(&mounted);
    Ok(Mounted(Arc::new(mounted)))
}

/// Each module's extra WebDAV addresses are named like IDs, and mustn't
/// clash with a module's own address or with each other.
fn check_dav_names(mounted: &[(&'static str, Routes)]) {
    let mut taken: Vec<&str> = mounted.iter().map(|&(id, _)| id).collect();
    for (id, routes) in mounted {
        for &(name, _) in &routes.more_dav {
            check_id(name);
            assert!(
                !taken.contains(&name),
                "{id} wants /dav/{name}, which another module already has"
            );
            taken.push(name);
        }
    }
}

impl Mounted {
    /// Mounts each module's routes under paths named after its ID, so no
    /// module can claim another's paths or the server's.
    ///
    /// Each set is mounted as a service, so it also answers the path with a
    /// trailing slash, such as `/dav/drive/`, which is where WebDAV clients
    /// start.
    pub fn mount<S: Clone + Send + Sync + 'static>(&self, mut router: Router<S>) -> Router<S> {
        for (id, routes) in self.0.iter() {
            if let Some(api) = &routes.api {
                router = router.nest_service(&format!("/api/{id}"), api.clone());
            }
            if let Some(dav) = &routes.dav {
                router = router.nest_service(&format!("/dav/{id}"), dav.clone());
            }
            for (name, dav) in &routes.more_dav {
                router = router.nest_service(&format!("/dav/{name}"), dav.clone());
            }
            if let Some(pages) = &routes.pages {
                router = router.nest_service(&format!("/{id}"), pages.clone());
            }
        }
        router
    }

    /// The link pages of the modules that have them, for the link sharing
    /// listener (decision 22). Nothing else goes on that listener.
    pub fn links(&self) -> Option<Router> {
        self.0
            .iter()
            .filter_map(|(_, routes)| routes.links.clone())
            .reduce(Router::merge)
    }

    /// The modules whose pages are in the web app (decision 13), rather
    /// than routes of their own.
    pub fn in_web_app(&self) -> Vec<&'static str> {
        self.0
            .iter()
            .filter(|(_, routes)| routes.pages.is_none())
            .map(|&(id, _)| id)
            .collect()
    }
}

/// IDs name URL paths and database tables, so they're lowercase letters
/// only. A wrong ID is a bug in the module, so it stops the server at once.
fn check_id(id: &str) {
    assert!(
        !id.is_empty() && id.bytes().all(|b| b.is_ascii_lowercase()),
        "module IDs are lowercase letters, not {id:?}"
    );
    assert!(
        !matches!(id, "api" | "dav" | "setup" | "health" | "account"),
        "the server uses /{id} itself, so a module can't have the ID {id:?}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_module_has_a_good_id() {
        let mut ids: Vec<&str> = all().iter().map(|module| module.id()).collect();
        for id in &ids {
            check_id(id);
        }
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "two modules share an ID");
    }

    #[test]
    fn every_required_module_is_built_in() {
        let modules = all();
        for module in &modules {
            for required in module.requires() {
                assert!(
                    modules.iter().any(|other| other.id() == *required),
                    "{} requires {required}, which isn't in the list",
                    module.name()
                );
            }
        }
    }

    #[test]
    #[should_panic(expected = "which another module already has")]
    fn refuses_a_webdav_address_another_module_has() {
        let routes = |more: &'static str| Routes {
            more_dav: vec![(more, Router::new())],
            ..Routes::default()
        };
        check_dav_names(&[("drive", routes("shared")), ("notes", routes("drive"))]);
    }

    #[test]
    #[should_panic(expected = "lowercase letters")]
    fn refuses_ids_that_arent_lowercase_letters() {
        check_id("my-drive");
    }

    #[test]
    #[should_panic(expected = "uses /api itself")]
    fn refuses_ids_the_server_uses() {
        check_id("api");
    }
}
