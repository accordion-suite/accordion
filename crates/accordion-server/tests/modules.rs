//! Turning modules on and off from the Modules page, without a restart
//! (decisions 7 and 31).

use accordion_core::coedit::{CoEditing, Timing};
use accordion_core::db::Database;
use accordion_core::events::Events;
use accordion_core::identity::{Identity, NewUser};
use accordion_core::live::Live;
use accordion_core::storage::Storage;
use accordion_core::texts::{self, Texts};
use accordion_core::thumbnails::{Thumbnails, Tools};
use accordion_gateway::Peer;
use accordion_module::{Services, Stopping};
use accordion_server::{AppState, Site, modules, router};
use accordion_types::Role;
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, Response, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tower::ServiceExt;

const HOST: &str = "accordion.example.com";

struct Server {
    _dir: tempfile::TempDir,
    _stop: watch::Sender<bool>,
    app: Router,
}

/// Starts a server with every module, whose requests come from the home
/// network, where the admin pages open.
async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("accordion.db")).unwrap();
    let identity = Identity::new(db.clone()).await.unwrap();
    let storage = Storage::open(db.clone(), identity.clone(), &dir.path().join("storage"))
        .await
        .unwrap();
    identity
        .create_user(NewUser {
            username: "alex".into(),
            display_name: "Alex Kim".into(),
            role: Role::Owner,
            password: "correct horse".into(),
        })
        .await
        .unwrap();
    let events = Events::open(db.clone()).await.unwrap();
    let live = Live::new(db.clone(), &identity);
    let (stop, stopping) = watch::channel(false);
    let mut live_stopping = stop.subscribe();
    tokio::spawn(live.clone().run(async move {
        let _ = live_stopping.wait_for(|&stopped| stopped).await;
    }));
    let thumbnails = Thumbnails::open(
        db.clone(),
        storage.clone(),
        dir.path(),
        Tools::installed(env!("CARGO_BIN_EXE_accordion")),
    )
    .await
    .unwrap();
    let modules = modules::start(
        modules::all(),
        &Services {
            // Nothing is imported here, so the reader points nowhere.
            takeout: accordion_core::takeout::Imports::open(
                db.clone(),
                storage.clone(),
                accordion_core::events::Events::open(db.clone())
                    .await
                    .unwrap(),
                accordion_core::takeout::Takeout::new(
                    accordion_core::takeout::Tools::installed("/nonexistent"),
                    dir.path(),
                ),
            )
            .await
            .unwrap(),
            db: db.clone(),
            coediting: CoEditing::open(
                db.clone(),
                storage.clone(),
                identity.clone(),
                Timing::default(),
            )
            .await
            .unwrap(),
            identity: identity.clone(),
            storage: storage.clone(),
            transcripts: accordion_core::transcripts::Transcripts::open(
                db.clone(),
                storage.clone(),
                dir.path(),
                accordion_core::transcripts::Tools::installed("/nonexistent"),
                None,
                accordion_core::transcripts::MODELS_URL,
            )
            .await
            .unwrap(),
            events: events.clone(),
            live: live.clone(),
            link_origin: tokio::sync::watch::channel(None).1,
            thumbnails: thumbnails.clone(),
            texts: Texts::open(
                db,
                storage,
                thumbnails,
                dir.path(),
                texts::Tools::installed(env!("CARGO_BIN_EXE_accordion")),
            )
            .await
            .unwrap(),
        },
        &Stopping::new(stopping),
        &mut JoinSet::new(),
    )
    .await
    .unwrap();
    let app = router(
        AppState {
            identity,
            setup: None,
            health: Default::default(),
            admin_access: Default::default(),
            modules,
            backup: None,
            storage: None,
            updates: None,
            switches: None,
            events: Some(events),
            live: Some(live),
            remote: None,
            link_sharing: None,
            thumbnails: None,
            takeout: None,
            web: None,
        },
        &Site::named(HOST.parse().unwrap(), 443),
    )
    .layer(MockConnectInfo(Peer {
        local: "192.168.1.20:443".parse().unwrap(),
        remote: "192.168.1.30:50000".parse().unwrap(),
    }));
    Server {
        _dir: dir,
        _stop: stop,
        app,
    }
}

impl Server {
    async fn send(&self, request: axum::http::request::Builder, body: Body) -> Response<Body> {
        self.app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap()
    }

    /// Signs in as the web app does, and returns the session cookie.
    async fn sign_in(&self) -> String {
        let response = self
            .send(
                browser("POST", "/api/session"),
                json_body(json!({ "username": "alex", "password": "correct horse" })),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        set_cookie.split(';').next().unwrap().to_owned()
    }
}

/// A request from Settings in the web app.
fn browser(method: &str, path: &str) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, HOST)
        .header("sec-fetch-site", "same-origin")
        .header(header::CONTENT_TYPE, "application/json")
}

fn json_body(value: Value) -> Body {
    Body::from(value.to_string())
}

async fn json(response: Response<Body>) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

impl Server {
    async fn get(&self, cookie: &str, path: &str) -> Response<Body> {
        self.send(
            browser("GET", path).header(header::COOKIE, cookie),
            Body::empty(),
        )
        .await
    }

    async fn switch(&self, cookie: &str, id: &str, body: Value) -> Response<Body> {
        self.send(
            browser("PUT", &format!("/api/admin/modules/{id}")).header(header::COOKIE, cookie),
            json_body(body),
        )
        .await
    }

    async fn session_modules(&self, cookie: &str) -> Value {
        json(self.get(cookie, "/api/session").await).await["modules"].clone()
    }
}

#[tokio::test]
async fn turns_notes_off_and_on_without_a_restart() {
    let server = start().await;
    let cookie = server.sign_in().await;
    assert_eq!(
        server.session_modules(&cookie).await,
        json!(["drive", "notes", "docs"])
    );
    assert_eq!(
        server.get(&cookie, "/api/notes/notes").await.status(),
        StatusCode::OK
    );

    let response = server.get(&cookie, "/api/admin/modules").await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = json(response).await;
    assert_eq!(page["modules"][0]["id"], "drive");
    assert_eq!(page["modules"][0]["on"], true);
    assert_eq!(page["modules"][1]["id"], "notes");
    assert_eq!(page["modules"][1]["requires"], json!(["drive"]));
    assert!(page["memory"].as_u64().is_some());

    assert_eq!(page["modules"][2]["id"], "docs");

    // Drive can't go while Notes and Docs keep their files there.
    let response = server
        .switch(&cookie, "drive", json!({ "on": false }))
        .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        json(response).await["error"],
        "Notes and Docs need it. Turn those off first."
    );

    let response = server
        .switch(&cookie, "notes", json!({ "on": false }))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json(response).await["modules"][1]["on"], false);
    assert_eq!(
        server.get(&cookie, "/api/notes/notes").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server.session_modules(&cookie).await,
        json!(["drive", "docs"])
    );
    let response = server.switch(&cookie, "docs", json!({ "on": false })).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(server.session_modules(&cookie).await, json!(["drive"]));
    // Drive keeps working.
    assert_eq!(
        server.get(&cookie, "/api/drive/top").await.status(),
        StatusCode::OK
    );

    let response = server
        .switch(&cookie, "drive", json!({ "on": false }))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        server.get(&cookie, "/api/drive/top").await.status(),
        StatusCode::NOT_FOUND
    );

    // Notes needs Drive, which the admin agrees to turn on with it.
    let response = server.switch(&cookie, "notes", json!({ "on": true })).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        json(response).await["error"],
        "It needs Drive, which is off. Turn that on too."
    );
    let response = server
        .switch(
            &cookie,
            "notes",
            json!({ "on": true, "with_required": true }),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        server.session_modules(&cookie).await,
        json!(["drive", "notes"])
    );
    assert_eq!(
        server.get(&cookie, "/api/notes/notes").await.status(),
        StatusCode::OK
    );

    let log = json(server.get(&cookie, "/api/admin/log").await).await;
    let changes = log["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["action"] == "module_changed")
        .count();
    assert_eq!(changes, 5);
}

#[tokio::test]
async fn unknown_modules_arent_found() {
    let server = start().await;
    let cookie = server.sign_in().await;
    let response = server.switch(&cookie, "mail", json!({ "on": true })).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
