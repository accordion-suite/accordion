//! The modules this server includes, such as Drive and Notes, and turning
//! them on and off (decision 7 in `docs/decisions`, and the Modules page in
//! decision 31).
//!
//! A module that's off isn't started, so it runs nothing and answers no
//! requests. Turning one on or off takes effect without a restart: the
//! server's routes are built again with or without it, and requests that
//! arrive after that see the new set.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use accordion_core::db::{self, Database, Migration};
use accordion_core::events::{Action, NewEvent};
use accordion_core::live::{Audience, Hint};
use accordion_module::{ApiError, Authenticated, Module, Routes, Services, Stopping};
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};
use tower::{Service, ServiceExt};

use crate::AppState;

/// Every module built into this server, each after the modules it requires.
pub fn all() -> Vec<Box<dyn Module>> {
    vec![
        Box::new(accordion_drive::Drive),
        Box::new(accordion_notes::Notes),
    ]
}

/// The live update kind that tells every open tab the modules changed.
const MODULES_HINT: &str = "modules";
/// How long a module's background work has to finish when it's turned off.
const STOP_WAIT: Duration = Duration::from_secs(10);

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "module switches",
    sql: "
    -- Modules an admin turned on or off on the Modules page. A module
    -- without a row is on, so a new module is on after an update.
    CREATE TABLE modules_switches (
        id TEXT PRIMARY KEY,
        turned_on INTEGER NOT NULL CHECK (turned_on IN (0, 1))
    );
    ",
}];

/// The routes of the modules that are running, at one moment. Cloning it
/// is cheap.
#[derive(Clone, Default)]
pub struct Mounted(Arc<Vec<(&'static str, Routes)>>);

/// The server's modules: which are running, and turning them on and off.
/// Cloning it is cheap. The default has no modules, for tests that don't
/// need any.
#[derive(Clone, Default)]
pub struct Modules(Option<Arc<Inner>>);

struct Inner {
    all: Vec<Box<dyn Module>>,
    services: Services,
    /// When the server stops.
    server_stopping: Stopping,
    running: tokio::sync::Mutex<HashMap<&'static str, Running>>,
    mounted: watch::Sender<Mounted>,
}

struct Running {
    routes: Routes,
    stop: watch::Sender<bool>,
    background: Option<JoinHandle<()>>,
}

/// Starts the modules that are on, and runs their background work until
/// `stopping` fires. The server waits for it through `tasks`.
pub async fn start(
    modules: Vec<Box<dyn Module>>,
    services: &Services,
    stopping: &Stopping,
    tasks: &mut JoinSet<()>,
) -> Result<Modules, Box<dyn Error>> {
    let mut ids = HashSet::new();
    for module in &modules {
        let id = module.id();
        check_id(id);
        assert!(ids.insert(id), "two modules have the ID {id}");
    }
    services.db.migrate("modules", MIGRATIONS).await?;
    let off = turned_off(&services.db).await?;
    let inner = Arc::new(Inner {
        all: modules,
        services: services.clone(),
        server_stopping: stopping.clone(),
        running: tokio::sync::Mutex::new(HashMap::new()),
        mounted: watch::channel(Mounted::default()).0,
    });
    {
        let mut running = inner.running.lock().await;
        for module in &inner.all {
            if off.contains(module.id()) {
                tracing::info!("{} is turned off", module.name());
                continue;
            }
            if let Some(missing) = module
                .requires()
                .iter()
                .find(|required| !running.contains_key(**required))
            {
                tracing::warn!(
                    "{} is on, but {missing}, which it needs, is off, so {} isn't started",
                    module.name(),
                    module.name()
                );
                continue;
            }
            let started = inner.start_one(module.as_ref()).await?;
            running.insert(module.id(), started);
        }
        inner.publish(&running);
    }
    let modules = Modules(Some(inner));
    tasks.spawn(modules.clone().wait_stopped());
    Ok(modules)
}

/// The modules an admin turned off.
async fn turned_off(db: &Database) -> Result<HashSet<String>, db::Error> {
    db.read(|conn| {
        let mut statement = conn.prepare("SELECT id FROM modules_switches WHERE turned_on = 0")?;
        let off = statement
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(off)
    })
    .await
}

impl Inner {
    async fn start_one(&self, module: &dyn Module) -> Result<Running, Box<dyn Error>> {
        let (stop, stopping) = watch::channel(false);
        // The module also stops when the server does.
        let server = self.server_stopping.clone();
        let forward = stop.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = server.wait() => { let _ = forward.send(true); }
                () = forward.closed() => {}
            }
        });
        let started = module
            .start(self.services.clone(), Stopping::new(stopping))
            .await
            .map_err(|err| format!("Accordion couldn't start {}: {err}", module.name()))?;
        tracing::info!("started {}", module.name());
        Ok(Running {
            routes: started.routes,
            stop,
            background: started.background.map(tokio::spawn),
        })
    }

    /// Makes the running modules' routes the ones requests see.
    fn publish(&self, running: &HashMap<&'static str, Running>) {
        let mounted: Vec<(&'static str, Routes)> = self
            .all
            .iter()
            .filter_map(|module| {
                let routes = &running.get(module.id())?.routes;
                Some((module.id(), clone_routes(routes)))
            })
            .collect();
        check_dav_names(&mounted);
        self.mounted.send_replace(Mounted(Arc::new(mounted)));
    }

    fn module(&self, id: &str) -> Option<&dyn Module> {
        self.all
            .iter()
            .find(|module| module.id() == id)
            .map(AsRef::as_ref)
    }
}

fn clone_routes(routes: &Routes) -> Routes {
    Routes {
        api: routes.api.clone(),
        dav: routes.dav.clone(),
        more_dav: routes.more_dav.clone(),
        pages: routes.pages.clone(),
        links: routes.links.clone(),
    }
}

/// Why a module couldn't be turned on or off.
#[derive(Debug)]
pub enum SwitchError {
    UnknownModule,
    /// Turning it off would leave these running modules without it.
    NeededBy(Vec<&'static str>),
    /// Turning it on needs these, which are off.
    Needs(Vec<&'static str>),
    Start(Box<dyn Error>),
    Database(db::Error),
}

/// A module, as the Modules page shows it.
#[derive(Serialize)]
pub struct ModuleView {
    pub id: &'static str,
    pub name: &'static str,
    pub requires: Vec<&'static str>,
    pub on: bool,
    /// How much space its data takes, if it can tell.
    pub size: Option<u64>,
}

impl Modules {
    /// The routes of the modules running now.
    pub fn mounted(&self) -> Mounted {
        match &self.0 {
            Some(inner) => inner.mounted.borrow().clone(),
            None => Mounted::default(),
        }
    }

    /// Changes each time modules are turned on or off.
    pub fn changes(&self) -> watch::Receiver<Mounted> {
        match &self.0 {
            Some(inner) => inner.mounted.subscribe(),
            None => watch::channel(Mounted::default()).1,
        }
    }

    /// The link pages of the running modules, for the link sharing listener
    /// (decision 22), following modules as they're turned on and off.
    pub fn links(&self) -> Option<Router> {
        self.0.as_ref()?;
        let modules = self.clone();
        Some(swapping(self.changes(), move |_| {
            modules.mounted().links().unwrap_or_default()
        }))
    }

    /// Every built-in module, with whether it's on and its data's size.
    pub async fn list(&self) -> Vec<ModuleView> {
        let Some(inner) = &self.0 else {
            return Vec::new();
        };
        let running: HashSet<&'static str> = inner.running.lock().await.keys().copied().collect();
        let mut views = Vec::new();
        for module in &inner.all {
            views.push(ModuleView {
                id: module.id(),
                name: module.name(),
                requires: module.requires().to_vec(),
                on: running.contains(module.id()),
                size: module.data_size(&inner.services).await,
            });
        }
        views
    }

    /// Turns a module on or off, and returns the IDs of every module it
    /// changed. Turning one on with `with_required` turns on what it needs
    /// first.
    pub async fn set(
        &self,
        id: &str,
        on: bool,
        with_required: bool,
    ) -> Result<Vec<&'static str>, SwitchError> {
        let inner = self.0.as_ref().ok_or(SwitchError::UnknownModule)?;
        let module = inner.module(id).ok_or(SwitchError::UnknownModule)?;
        let id = module.id();
        let mut running = inner.running.lock().await;
        let mut changed = Vec::new();
        if on {
            if running.contains_key(id) {
                return Ok(changed);
            }
            let mut wanted = Vec::new();
            needed(inner, id, &mut wanted);
            let missing: Vec<&'static str> = wanted
                .iter()
                .copied()
                .filter(|&other| other != id && !running.contains_key(other))
                .collect();
            if !missing.is_empty() && !with_required {
                return Err(SwitchError::Needs(missing));
            }
            for other in wanted {
                if running.contains_key(other) {
                    continue;
                }
                let module = inner.module(other).expect("a built-in module");
                let started = inner.start_one(module).await.map_err(SwitchError::Start)?;
                running.insert(other, started);
                save(&inner.services.db, other, true).await?;
                changed.push(other);
            }
        } else {
            let Some(_) = running.get(id) else {
                return Ok(changed);
            };
            let needed_by: Vec<&'static str> = inner
                .all
                .iter()
                .filter(|other| running.contains_key(other.id()) && other.requires().contains(&id))
                .map(|other| other.id())
                .collect();
            if !needed_by.is_empty() {
                return Err(SwitchError::NeededBy(needed_by));
            }
            let stopped = running.remove(id).expect("checked above");
            // Requests stop reaching it before its work is told to stop.
            inner.publish(&running);
            stop(module.name(), stopped).await;
            save(&inner.services.db, id, false).await?;
            changed.push(id);
            tracing::info!("turned off {}", module.name());
        }
        inner.publish(&running);
        Ok(changed)
    }

    /// Waits until the server stops, then for every module's background
    /// work to finish.
    async fn wait_stopped(self) {
        let Some(inner) = &self.0 else { return };
        inner.server_stopping.clone().wait().await;
        let mut running = inner.running.lock().await;
        for (_, module) in running.drain() {
            if let Some(background) = module.background {
                let _ = background.await;
            }
        }
    }
}

/// `id` and everything it needs, the needed ones first.
fn needed(inner: &Inner, id: &'static str, into: &mut Vec<&'static str>) {
    if into.contains(&id) {
        return;
    }
    if let Some(module) = inner.module(id) {
        for required in module.requires() {
            needed(inner, required, into);
        }
    }
    into.push(id);
}

async fn stop(name: &str, module: Running) {
    let _ = module.stop.send(true);
    if let Some(background) = module.background
        && tokio::time::timeout(STOP_WAIT, background).await.is_err()
    {
        tracing::warn!("{name}'s background work didn't stop in time");
    }
}

async fn save(db: &Database, id: &'static str, on: bool) -> Result<(), SwitchError> {
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO modules_switches (id, turned_on) VALUES (?1, ?2)
             ON CONFLICT (id) DO UPDATE SET turned_on = excluded.turned_on",
            (id, on),
        )?;
        Ok(())
    })
    .await
    .map_err(SwitchError::Database)
}

/// A router that answers with the one `build` makes from the newest value
/// of `changes`, built again each time it changes.
pub(crate) fn swapping<T>(
    changes: watch::Receiver<T>,
    build: impl Fn(&T) -> Router + Send + Sync + 'static,
) -> Router
where
    T: Send + Sync + 'static,
{
    Router::new().fallback_service(Swapping {
        changes,
        build: Arc::new(build),
        built: Arc::new(Mutex::new(None)),
    })
}

type Build<T> = Arc<dyn Fn(&T) -> Router + Send + Sync>;

struct Swapping<T> {
    changes: watch::Receiver<T>,
    build: Build<T>,
    built: Arc<Mutex<Option<Router>>>,
}

impl<T> Clone for Swapping<T> {
    fn clone(&self) -> Self {
        Self {
            changes: self.changes.clone(),
            build: Arc::clone(&self.build),
            built: Arc::clone(&self.built),
        }
    }
}

impl<T: Send + Sync + 'static> Service<Request<Body>> for Swapping<T> {
    type Response = Response;
    type Error = Infallible;
    type Future = std::pin::Pin<Box<dyn Future<Output = Result<Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let router = {
            let mut built = self.built.lock().expect("the router lock isn't poisoned");
            if self.changes.has_changed().unwrap_or(false) || built.is_none() {
                let value = self.changes.borrow_and_update();
                *built = Some((self.build)(&value));
            }
            built.clone().expect("built above")
        };
        Box::pin(router.oneshot(request))
    }
}

/// The admin's Modules page, at `/api/admin/modules`.
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/api/admin/modules", get(list))
        .route("/api/admin/modules/{id}", put(switch))
}

#[derive(Serialize)]
struct ModulesView {
    modules: Vec<ModuleView>,
    /// How much memory the server is using, if it can tell.
    memory: Option<u64>,
}

fn check_admin(me: &accordion_core::identity::SignedIn) -> Result<(), ApiError> {
    if me.user.role.is_admin() {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::FORBIDDEN,
        "Only admins can turn modules on and off.",
    ))
}

/// `GET /api/admin/modules`: every module, and the server's memory use.
async fn list(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Json<ModulesView>, ApiError> {
    check_admin(&me)?;
    Ok(Json(ModulesView {
        modules: state.modules.list().await,
        memory: memory_used(),
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Switch {
    on: bool,
    /// Turn on what the module needs too, which the admin agreed to.
    #[serde(default)]
    with_required: bool,
}

/// `PUT /api/admin/modules/{id}`: turns a module on or off.
async fn switch(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Path(id): Path<String>,
    Json(request): Json<Switch>,
) -> Result<Json<ModulesView>, ApiError> {
    check_admin(&me)?;
    let names = |ids: &[&'static str]| -> String {
        let names: Vec<&str> = ids
            .iter()
            .filter_map(|id| {
                state
                    .modules
                    .0
                    .as_ref()?
                    .module(id)
                    .map(|module| module.name())
            })
            .collect();
        names.join(" and ")
    };
    let changed = match state
        .modules
        .set(&id, request.on, request.with_required)
        .await
    {
        Ok(changed) => changed,
        Err(SwitchError::UnknownModule) => return Err(ApiError::not_found()),
        Err(SwitchError::NeededBy(ids)) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                format!("{} needs it. Turn that off first.", names(&ids)),
            ));
        }
        Err(SwitchError::Needs(ids)) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                format!("It needs {}, which is off. Turn that on too.", names(&ids)),
            ));
        }
        Err(SwitchError::Start(err)) => {
            tracing::error!("{err}");
            return Err(ApiError::server_problem());
        }
        Err(SwitchError::Database(err)) => {
            tracing::error!(%err, "couldn't save a module switch");
            return Err(ApiError::server_problem());
        }
    };
    for module in &changed {
        let event = NewEvent::new(Action::ModuleChanged).details(serde_json::json!({
            "module": module,
            "on": request.on,
        }));
        crate::log::record(&state, &me, event).await;
    }
    if !changed.is_empty() {
        tell_everyone(&state).await;
    }
    list(State(state), Authenticated(me)).await
}

/// Tells every open tab that modules changed, so apps and menus follow.
async fn tell_everyone(state: &AppState) {
    let Some(live) = &state.live else { return };
    match state.identity.members().await {
        Ok(members) => live.send(Hint {
            kind: MODULES_HINT,
            id: None,
            audience: Audience::People(members.into_iter().map(|member| member.user.id).collect()),
        }),
        Err(err) => tracing::warn!(%err, "couldn't tell everyone that modules changed"),
    }
}

/// The server process's resident memory, from Linux.
fn memory_used() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kilobytes: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kilobytes * 1024)
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
    fn every_required_module_comes_first() {
        let modules = all();
        for (at, module) in modules.iter().enumerate() {
            for required in module.requires() {
                assert!(
                    modules[..at].iter().any(|other| other.id() == *required),
                    "{} requires {required}, which has to come before it in the list",
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
