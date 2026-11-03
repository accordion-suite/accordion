//! Creates an app password the way Settings does, then uses it to open
//! Drive over WebDAV through the whole server.

use accordion_core::db::Database;
use accordion_core::events::Events;
use accordion_core::identity::{Identity, NewUser};
use accordion_core::storage::Storage;
use accordion_core::thumbnails::{Thumbnails, Tools};
use accordion_gateway::Peer;
use accordion_module::{Services, Stopping};
use accordion_server::{AppState, Site, modules, router};
use accordion_types::Role;
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, Response, StatusCode, header};
use base64ct::{Base64, Encoding};
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

/// Starts a server with every module, whose requests come from outside the
/// home network.
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
    let (stop, stopping) = watch::channel(false);
    let modules = modules::start(
        modules::all(),
        &Services {
            db: db.clone(),
            identity: identity.clone(),
            storage: storage.clone(),
            events: events.clone(),
            thumbnails: Thumbnails::open(
                db,
                storage,
                dir.path(),
                Tools::installed(env!("CARGO_BIN_EXE_accordion")),
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
            web: None,
        },
        &Site::named(HOST.parse().unwrap(), 443),
    )
    .layer(MockConnectInfo(Peer {
        local: "192.168.1.20:443".parse().unwrap(),
        remote: "203.0.113.5:50000".parse().unwrap(),
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

/// A request from a WebDAV client, such as Finder.
fn dav(method: &str, path: &str, password: &str) -> axum::http::request::Builder {
    let credentials = Base64::encode_string(format!("alex:{password}").as_bytes());
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, HOST)
        .header(header::AUTHORIZATION, format!("Basic {credentials}"))
}

fn json_body(value: Value) -> Body {
    Body::from(value.to_string())
}

async fn json(response: Response<Body>) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn an_app_password_opens_drive_over_webdav() {
    let server = start().await;
    let cookie = server.sign_in().await;

    // The old page's address leads to Settings in the web app.
    let page = server.send(browser("GET", "/account"), Body::empty()).await;
    assert_eq!(page.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(page.headers()[header::LOCATION], "/settings/account");

    let response = server
        .send(
            browser("POST", "/api/app-passwords").header(header::COOKIE, &cookie),
            json_body(json!({ "device_name": "Alex's laptop", "use_for": "dav" })),
        )
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = json(response).await;
    let password = created["password"].as_str().unwrap().to_owned();
    assert_eq!(password.len(), 19);
    let id = created["app_password"]["id"].as_str().unwrap().to_owned();

    let response = server
        .send(
            dav("PUT", "/dav/drive/hello.txt", &password),
            Body::from("hi"),
        )
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let response = server
        .send(
            dav("PROPFIND", "/dav/drive/", &password).header("depth", "1"),
            Body::empty(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let listing = response.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&listing).contains("/dav/drive/hello.txt"));

    // The account password never opens WebDAV.
    let response = server
        .send(
            dav("PROPFIND", "/dav/drive/", "correct horse"),
            Body::empty(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let listed = json(
        server
            .send(
                browser("GET", "/api/app-passwords").header(header::COOKIE, &cookie),
                Body::empty(),
            )
            .await,
    )
    .await;
    assert_eq!(listed[0]["device_name"], "Alex's laptop");
    assert!(listed[0]["last_used_at"].is_string());

    let response = server
        .send(
            browser("DELETE", &format!("/api/app-passwords/{id}")).header(header::COOKIE, &cookie),
            Body::empty(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = server
        .send(dav("GET", "/dav/drive/hello.txt", &password), Body::empty())
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn confirming_the_password_needs_the_right_one() {
    let server = start().await;
    let cookie = server.sign_in().await;
    for (password, status) in [
        ("wrong horse", StatusCode::UNAUTHORIZED),
        ("correct horse", StatusCode::NO_CONTENT),
    ] {
        let response = server
            .send(
                browser("POST", "/api/session/confirm").header(header::COOKIE, &cookie),
                json_body(json!({ "password": password })),
            )
            .await;
        assert_eq!(response.status(), status, "{password}");
    }
}

#[tokio::test]
async fn module_routes_get_the_servers_checks() {
    let server = start().await;
    let cookie = server.sign_in().await;
    let created = json(
        server
            .send(
                browser("POST", "/api/app-passwords").header(header::COOKIE, &cookie),
                json_body(json!({ "device_name": "Laptop", "use_for": "dav" })),
            )
            .await,
    )
    .await;
    let password = created["password"].as_str().unwrap();

    // Another name for the server, as in a DNS rebinding attack.
    let mut request = dav("PROPFIND", "/dav/drive/", password);
    request.headers_mut().unwrap().insert(
        header::HOST,
        header::HeaderValue::from_static("attacker.example"),
    );
    let response = server.send(request, Body::empty()).await;
    assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST);

    // A website trying to use a browser's saved WebDAV password.
    let response = server
        .send(
            dav("PUT", "/dav/drive/planted.txt", password).header("sec-fetch-site", "cross-site"),
            Body::from("boo"),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // The JSON API needs a session, not an app password.
    let response = server
        .send(dav("GET", "/api/drive/top", password), Body::empty())
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = server
        .send(
            browser("GET", "/api/drive/top").header(header::COOKIE, &cookie),
            Body::empty(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn the_activity_log_shows_sign_ins_and_app_passwords() {
    let server = start().await;
    let wrong = server
        .send(
            browser("POST", "/api/session"),
            json_body(json!({ "username": "alex", "password": "not it" })),
        )
        .await;
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    let cookie = server.sign_in().await;
    let created = server
        .send(
            browser("POST", "/api/app-passwords").header(header::COOKIE, &cookie),
            json_body(json!({ "device_name": "Laptop", "use_for": "dav" })),
        )
        .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    let log = json(
        server
            .send(
                browser("GET", "/api/account/activity").header(header::COOKIE, &cookie),
                Body::empty(),
            )
            .await,
    )
    .await;
    let events = log["events"].as_array().unwrap();
    let actions: Vec<_> = events.iter().map(|event| &event["action"]).collect();
    assert_eq!(
        actions,
        ["app_password_created", "signed_in", "sign_in_failed"]
    );
    assert_eq!(events[0]["details"]["device"], "Laptop");
    assert_eq!(events[1]["actor"]["name"], "Alex Kim");
    assert_eq!(events[1]["details"]["address"], "203.0.113.5");
    assert_eq!(events[2]["actor"], Value::Null);
    assert_eq!(events[2]["other"]["name"], "Alex Kim");

    // The admin log is for the home network only.
    let admin_log = server
        .send(
            browser("GET", "/api/admin/log").header(header::COOKIE, &cookie),
            Body::empty(),
        )
        .await;
    assert_eq!(admin_log.status(), StatusCode::FORBIDDEN);

    let signed_out = server
        .send(
            browser("DELETE", "/api/session").header(header::COOKIE, &cookie),
            Body::empty(),
        )
        .await;
    assert_eq!(signed_out.status(), StatusCode::NO_CONTENT);
}
