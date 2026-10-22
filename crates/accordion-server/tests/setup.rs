//! Claims the setup wizard and creates the owner over HTTP.

use std::sync::Arc;

use accordion_core::db::Database;
use accordion_core::identity::Identity;
use accordion_gateway::Peer;
use accordion_gateway::certificates::Authority;
use accordion_server::setup::{self, SETUP_CODE_FILE, Setup, SetupOptions};
use accordion_server::{AppState, Site, router};
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

async fn start(open_window: bool) -> (tempfile::TempDir, Arc<Setup>, Identity) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("accordion.db")).unwrap();
    let identity = Identity::new(db).await.unwrap();
    let (setup, _) = Setup::start(SetupOptions {
        data_dir: dir.path().to_owned(),
        gateway_dir: dir.path().join("gateway"),
        authority: Authority::LetsEncryptStaging,
        https_port: 443,
        open_window,
        address: None,
    })
    .unwrap();
    (dir, setup, identity)
}

fn wizard(setup: &Arc<Setup>, client: &str) -> Router {
    let peer = Peer {
        local: "192.168.1.20:80".parse().unwrap(),
        remote: format!("{client}:50000").parse().unwrap(),
    };
    setup::wizard_router(Arc::clone(setup)).layer(MockConnectInfo(peer))
}

async fn call(
    app: &Router,
    method: &str,
    host: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, host)
        .header(header::CONTENT_TYPE, "application/json")
        .body(if method == "GET" {
            Body::empty()
        } else {
            Body::from(body.to_string())
        })
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn code(dir: &tempfile::TempDir) -> String {
    std::fs::read_to_string(dir.path().join(SETUP_CODE_FILE)).unwrap()
}

#[tokio::test]
async fn the_wizard_answers_only_the_home_network_by_its_own_names() {
    let (_dir, setup, _) = start(false).await;
    let home = wizard(&setup, "192.168.1.30");
    for (host, expected) in [
        ("accordion.local", StatusCode::OK),
        ("192.168.1.20", StatusCode::OK),
        ("evil.example", StatusCode::MISDIRECTED_REQUEST),
    ] {
        let (status, _) = call(&home, "GET", host, "/api/setup", Value::Null).await;
        assert_eq!(status, expected, "{host}");
    }

    let outside = wizard(&setup, "203.0.113.5");
    let (status, _) = call(
        &outside,
        "GET",
        "accordion.local",
        "/api/setup",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn claiming_needs_the_code_and_unlocks_progress() {
    let (dir, setup, _) = start(false).await;
    let app = wizard(&setup, "192.168.1.30");

    let (status, body) = call(&app, "GET", "accordion.local", "/api/setup", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "code_needed": true }));

    let (status, _) = call(
        &app,
        "POST",
        "accordion.local",
        "/api/setup/claim",
        json!({ "code": "wrong" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = call(
        &app,
        "POST",
        "accordion.local",
        "/api/setup/claim",
        json!({ "code": code(&dir) }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let token = body["token"].as_str().unwrap().to_owned();

    let request = Request::get("/api/setup")
        .header(header::HOST, "accordion.local")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["progress"], json!({ "state": "not_started" }));

    // The name step needs a claim, and checks the name before going online.
    let (status, _) = call(
        &app,
        "POST",
        "accordion.local",
        "/api/setup/name",
        json!({ "name": "accordion.example.com", "provider": "desec", "token": "x" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let request = Request::post("/api/setup/name")
        .header(header::HOST, "accordion.local")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(
            json!({ "name": "accordion", "provider": "desec", "token": "x" }).to_string(),
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_owner_step_creates_one_owner_and_signs_them_in() {
    let (dir, setup, identity) = start(false).await;
    let site = Site::named("accordion.example.com".parse().unwrap(), 443);
    let app = router(
        AppState {
            identity: identity.clone(),
            setup: Some(setup),
            health: Default::default(),
            admin_access: Default::default(),
            modules: Default::default(),
            backup: None,
        },
        &site,
    )
    .layer(MockConnectInfo(Peer {
        local: "192.168.1.20:443".parse().unwrap(),
        remote: "192.168.1.30:50000".parse().unwrap(),
    }));
    let host = "accordion.example.com";
    let owner = |code: String| {
        json!({
            "code": code,
            "username": "alex",
            "display_name": "Alex Kim",
            "password": "correct horse",
        })
    };

    let (status, body) = call(&app, "GET", host, "/api/setup", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "owner_needed": true, "code_needed": true }));

    let (status, _) = call(
        &app,
        "POST",
        host,
        "/api/setup/owner",
        owner("wrong".into()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let request = Request::post("/api/setup/owner")
        .header(header::HOST, host)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(owner(code(&dir)).to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    assert!(
        response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .starts_with("__Host-accordion-session=acs_")
    );
    assert!(identity.has_owner().await.unwrap());
    assert!(!dir.path().join(SETUP_CODE_FILE).exists());

    let (status, body) = call(&app, "GET", host, "/api/setup", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["owner_needed"], false);
    let (status, _) = call(&app, "POST", host, "/api/setup/owner", owner(String::new())).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn the_pages_load_nothing_from_other_sites() {
    let (_dir, setup, _) = start(false).await;
    let app = wizard(&setup, "192.168.1.30");
    let response = app
        .oneshot(
            Request::get("/")
                .header(header::HOST, "accordion.local")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let policy = response.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    assert!(policy.starts_with("default-src 'none'; script-src 'self'"));
}
