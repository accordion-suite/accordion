//! Reads the health report over HTTP.

use accordion_core::db::Database;
use accordion_core::identity::{AuthMethod, Client, Identity, NewUser};
use accordion_gateway::certificates::{Authority, CertificateStatus};
use accordion_server::health::{Health, NameHealth};
use accordion_server::{AppState, Site, router};
use accordion_types::Role;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::Value;
use tokio::sync::watch;
use tower::ServiceExt;

const HOST: &str = "accordion.example.com";

/// Starts a server with an owner and a standard user, and returns a bearer
/// token for each.
async fn start() -> (tempfile::TempDir, Router, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("accordion.db")).unwrap();
    let identity = Identity::new(db).await.unwrap();
    let mut tokens = Vec::new();
    for (username, role) in [("alex", Role::Owner), ("sam", Role::User)] {
        let user = identity
            .create_user(NewUser {
                username: username.into(),
                display_name: username.into(),
                role,
                password: "correct horse".into(),
            })
            .await
            .unwrap();
        let (token, _) = identity
            .create_session(user.id, Client::Browser, "Test", AuthMethod::Password)
            .await
            .unwrap();
        tokens.push(token.as_str().to_owned());
    }

    let health = Health {
        name: Some(NameHealth {
            name: HOST.parse().unwrap(),
            authority: Authority::LetsEncryptStaging,
            certificate: watch::channel(CertificateStatus::default()).1,
            address: None,
        }),
        local_name: None,
    };
    let app = router(
        AppState {
            identity,
            setup: None,
            health,
        },
        &Site::named(HOST.parse().unwrap(), 443),
    );
    let user = tokens.pop().unwrap();
    let owner = tokens.pop().unwrap();
    (dir, app, owner, user)
}

async fn get(app: &Router, path: &str, token: Option<&str>) -> (StatusCode, Value) {
    let mut request = Request::get(path).header(header::HOST, HOST);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn only_admins_see_the_report() {
    let (_dir, app, owner, user) = start().await;

    let (status, _) = get(&app, "/api/health", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = get(&app, "/api/health", Some(&user)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body["error"].as_str().unwrap().contains("Only admins"));

    let (status, body) = get(&app, "/api/health", Some(&owner)).await;
    assert_eq!(status, StatusCode::OK);
    // No certificate yet is a problem, whatever else is going on.
    assert_eq!(body["state"], "problem");
    let checks = body["checks"].as_array().unwrap();
    let ids: Vec<_> = checks.iter().map(|check| check["id"].clone()).collect();
    assert_eq!(ids, ["certificate", "address"]);
    assert_eq!(checks[0]["state"], "problem");
}

#[tokio::test]
async fn the_page_loads_nothing_from_other_sites() {
    let (_dir, app, _, _) = start().await;
    let response = app
        .oneshot(
            Request::get("/health")
                .header(header::HOST, HOST)
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
