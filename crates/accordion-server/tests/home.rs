//! Opens `/`, where `accordion.local` and the server's name lead.

use accordion_core::db::Database;
use accordion_core::identity::{Identity, NewUser};
use accordion_gateway::Peer;
use accordion_server::{AppState, Site, router};
use accordion_types::Role;
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use tower::ServiceExt;

const HOST: &str = "accordion.example.com";

/// Starts a server, with an owner if `with_owner`, whose requests come from
/// outside the home network.
async fn start(with_owner: bool) -> (tempfile::TempDir, Router) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("accordion.db")).unwrap();
    let identity = Identity::new(db).await.unwrap();
    if with_owner {
        identity
            .create_user(NewUser {
                username: "alex".into(),
                display_name: "Alex Kim".into(),
                role: Role::Owner,
                password: "correct horse".into(),
            })
            .await
            .unwrap();
    }
    let app = router(
        AppState {
            identity,
            setup: None,
            health: Default::default(),
            admin_access: Default::default(),
            modules: Default::default(),
            backup: None,
            storage: None,
            updates: None,
            switches: None,
            web: None,
        },
        &Site::named(HOST.parse().unwrap(), 443),
    )
    .layer(MockConnectInfo(Peer {
        local: "192.168.1.20:443".parse().unwrap(),
        remote: "203.0.113.5:50000".parse().unwrap(),
    }));
    (dir, app)
}

async fn get(app: &Router, path: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::get(path)
                .header(header::HOST, HOST)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn sends_people_to_the_owner_step_until_there_is_an_owner() {
    let (_dir, app) = start(false).await;
    let response = get(&app, "/").await;
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(response.headers()[header::LOCATION], "/setup");
}

#[tokio::test]
async fn explains_a_server_without_the_web_app() {
    let (_dir, app) = start(true).await;
    let response = get(&app, "/").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(body.contains("ACCORDION_WEB_DIR"));

    // The page's styles and tab icon load from anywhere, unlike the admin
    // APIs.
    assert_eq!(get(&app, "/setup.css").await.status(), StatusCode::OK);
    let icon = get(&app, "/favicon.svg").await;
    assert_eq!(icon.status(), StatusCode::OK);
    assert_eq!(icon.headers()[header::CONTENT_TYPE], "image/svg+xml");
}
