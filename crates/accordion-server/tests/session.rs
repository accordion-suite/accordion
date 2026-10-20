//! Signs in, checks the session, and signs out over HTTP.

use accordion_core::db::Database;
use accordion_core::identity::limits::ADDRESS_LIMIT;
use accordion_core::identity::{Identity, NewUser};
use accordion_gateway::Peer;
use accordion_server::{AppState, Site, router};
use accordion_types::Role;
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, Response, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const HOST: &str = "accordion.local";

async fn start() -> (tempfile::TempDir, Router) {
    start_with(&Site::Unnamed).await
}

async fn start_with(site: &Site) -> (tempfile::TempDir, Router) {
    let (dir, app) = start_server(site).await;
    (dir, app.layer(MockConnectInfo(peer("192.168.1.30"))))
}

/// A connection to the server at 192.168.1.20 from `client`.
fn peer(client: &str) -> Peer {
    Peer {
        local: "192.168.1.20:443".parse().unwrap(),
        remote: format!("{client}:50000").parse().unwrap(),
    }
}

async fn start_server(site: &Site) -> (tempfile::TempDir, Router) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("accordion.db")).unwrap();
    let identity = Identity::new(db).await.unwrap();
    identity
        .create_user(NewUser {
            username: "alex".into(),
            display_name: "Alex Kim".into(),
            role: Role::Owner,
            password: "correct horse".into(),
        })
        .await
        .unwrap();
    (
        dir,
        router(
            AppState {
                identity,
                setup: None,
                health: Default::default(),
                admin_access: Default::default(),
            },
            site,
        ),
    )
}

fn request(method: &str) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri("/api/session")
        .header(header::HOST, HOST)
}

async fn sign_in(app: &Router, password: &str) -> Response<Body> {
    let body = json!({ "username": "Alex", "password": password }).to_string();
    app.clone()
        .oneshot(
            request("POST")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ORIGIN, format!("https://{HOST}"))
                .header(
                    header::USER_AGENT,
                    "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0",
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

/// Returns the `name=value` part of the session cookie.
fn session_cookie(response: &Response<Body>) -> String {
    let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    set_cookie.split(';').next().unwrap().to_owned()
}

async fn json(response: Response<Body>) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn current(app: &Router, header_name: header::HeaderName, value: &str) -> StatusCode {
    app.clone()
        .oneshot(
            request("GET")
                .header(header_name, value)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn signs_in_and_out_with_a_cookie() {
    let (_dir, app) = start().await;

    let response = sign_in(&app, "correct horse").await;
    assert_eq!(response.status(), StatusCode::OK);
    let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    for attribute in ["Path=/", "Secure", "HttpOnly", "SameSite=Lax"] {
        assert!(set_cookie.contains(attribute), "missing {attribute}");
    }
    let cookie = session_cookie(&response);
    assert!(cookie.starts_with("__Host-accordion-session=acs_"));
    let body = json(response).await;
    assert_eq!(body["user"]["username"], "alex");
    assert_eq!(body["user"]["role"], "owner");
    assert_eq!(body["session"]["device_name"], "Firefox on Linux");

    assert_eq!(current(&app, header::COOKIE, &cookie).await, StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            request("DELETE")
                .header(header::COOKIE, &cookie)
                .header("sec-fetch-site", "same-origin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );

    assert_eq!(
        current(&app, header::COOKIE, &cookie).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn accepts_the_token_as_a_bearer_token() {
    let (_dir, app) = start().await;
    let cookie = session_cookie(&sign_in(&app, "correct horse").await);
    let token = cookie.split_once('=').unwrap().1;
    assert_eq!(
        current(&app, header::AUTHORIZATION, &format!("Bearer {token}")).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn rejects_a_wrong_password_without_saying_which_part_was_wrong() {
    let (_dir, app) = start().await;
    let response = sign_in(&app, "wrong horse").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.headers().get(header::SET_COOKIE).is_none());
    assert_eq!(
        json(response).await["error"],
        "The username or password is incorrect."
    );
}

#[tokio::test]
async fn requires_a_session() {
    let (_dir, app) = start().await;
    let status = app
        .clone()
        .oneshot(request("GET").body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let unknown = "__Host-accordion-session=acs_unknown";
    assert_eq!(
        current(&app, header::COOKIE, unknown).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn blocks_sign_in_from_another_site() {
    let (_dir, app) = start().await;
    let body = json!({ "username": "alex", "password": "correct horse" }).to_string();
    let response = app
        .oneshot(
            request("POST")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ORIGIN, "https://evil.example")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(response.headers().get(header::SET_COOKIE).is_none());
}

#[tokio::test]
async fn a_named_server_answers_only_to_its_name() {
    let site = Site::named("accordion.example.com".parse().unwrap(), 443);
    let (_dir, app) = start_with(&site).await;
    for (host, expected) in [
        ("accordion.example.com", StatusCode::UNAUTHORIZED),
        ("accordion.local", StatusCode::MISDIRECTED_REQUEST),
        ("192.168.1.20", StatusCode::MISDIRECTED_REQUEST),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::get("/api/session")
                    .header(header::HOST, host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{host}");
    }
}

#[tokio::test]
async fn pauses_sign_ins_after_too_many_wrong_passwords() {
    let (_dir, server) = start_server(&Site::Unnamed).await;
    let stranger = server.clone().layer(MockConnectInfo(peer("203.0.113.5")));
    for _ in 0..ADDRESS_LIMIT {
        let response = sign_in(&stranger, "wrong horse").await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    let response = sign_in(&stranger, "correct horse").await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()[header::RETRY_AFTER], "900");
    assert!(response.headers().get(header::SET_COOKIE).is_none());
    assert_eq!(
        json(response).await["error"],
        "There have been too many wrong passwords. Wait 15 minutes, then try again."
    );

    // Someone at home can still sign in.
    let home = server.layer(MockConnectInfo(peer("192.168.1.30")));
    assert_eq!(
        sign_in(&home, "correct horse").await.status(),
        StatusCode::OK
    );
}
