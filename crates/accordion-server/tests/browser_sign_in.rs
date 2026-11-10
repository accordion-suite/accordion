//! Accordion's sync client signing in through the browser (decision 3), on
//! a real server: the client waits on this computer, the test plays the
//! browser, and the client trades the code for a session.

use std::time::Duration;

use accordion_core::db::Database;
use accordion_core::identity::{AuthMethod, Client as SessionClient, Identity, NewUser};
use accordion_gateway::Peer;
use accordion_server::health::Health;
use accordion_server::network::AdminAccess;
use accordion_server::{AppState, Site, router};
use accordion_sync::oauth::Pending;
use accordion_sync::{Client, Error, Reach};
use accordion_types::Role;
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

struct Server {
    _data: tempfile::TempDir,
    origin: String,
    /// The routes, as the browser reaches them.
    app: Router,
    browser: String,
}

async fn start() -> Server {
    let data = tempfile::tempdir().unwrap();
    let db = Database::open(&data.path().join("accordion.db")).unwrap();
    let identity = Identity::new(db).await.unwrap();
    let user = identity
        .create_user(NewUser {
            username: "alex".into(),
            display_name: "Alex".into(),
            role: Role::Owner,
            password: "correct horse".into(),
        })
        .await
        .unwrap();
    let (token, _) = identity
        .create_session(
            user.id,
            SessionClient::Browser,
            "Firefox",
            AuthMethod::Passkey,
        )
        .await
        .unwrap();
    let app = router(
        AppState {
            identity,
            setup: None,
            health: Health::default(),
            admin_access: AdminAccess::HomeNetwork,
            modules: Default::default(),
            backup: None,
            storage: None,
            updates: None,
            switches: None,
            events: None,
            live: None,
            remote: None,
            link_sharing: None,
            thumbnails: None,
            web: None,
        },
        &Site::Unnamed,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let served = app.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            served.into_make_service_with_connect_info::<Peer>(),
        )
        .await
        .unwrap();
    });
    Server {
        _data: data,
        origin,
        app: app.layer(MockConnectInfo(Peer {
            local: "127.0.0.1:8080".parse().unwrap(),
            remote: "127.0.0.1:50000".parse().unwrap(),
        })),
        browser: token.as_str().to_owned(),
    }
}

impl Server {
    /// Does what the sign-in page does when the person selects **Allow**
    /// on the page at `url`, and returns where it sends the browser.
    async fn allow(&self, url: &str) -> String {
        let query = url.split_once('?').unwrap().1;
        let check = format!("/api/oauth/check?{query}");
        let (status, _) = self.send("GET", &check, None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let mut fields = serde_json::Map::new();
        for pair in query.split('&') {
            let (name, value) = pair.split_once('=').unwrap();
            fields.insert(name.into(), Value::String(decode(value)));
        }
        let (status, approved) = self
            .send("POST", "/api/oauth/approve", Some(Value::Object(fields)))
            .await;
        assert_eq!(status, StatusCode::OK, "{approved}");
        approved["redirect"].as_str().unwrap().to_owned()
    }

    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, "127.0.0.1:8080")
            .header(header::AUTHORIZATION, format!("Bearer {}", self.browser))
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

fn decode(text: &str) -> String {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            out.push(u8::from_str_radix(&text[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

/// The browser following a redirect to the app on this computer.
async fn follow(redirect: &str) -> String {
    let rest = redirect.strip_prefix("http://").unwrap();
    let (host, path) = rest.split_once('/').unwrap();
    let mut stream = tokio::net::TcpStream::connect(host).await.unwrap();
    stream
        .write_all(format!("GET /{path} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut page = String::new();
    stream.read_to_string(&mut page).await.unwrap();
    page
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sync_client_signs_in_through_the_browser() {
    let server = start().await;
    let pending = Pending::start(&server.origin, "Alex's laptop", Reach::default())
        .await
        .unwrap();
    assert!(
        pending
            .url
            .starts_with(&format!("{}/oauth/authorize?", server.origin))
    );
    let url = pending.url.clone();

    let browser = async {
        let redirect = server.allow(&url).await;
        assert!(redirect.starts_with("http://127.0.0.1:"), "{redirect}");
        follow(&redirect).await
    };
    let (added, page) = tokio::join!(pending.finish(), browser);
    // The browser ends on the server's own page, once the app has its
    // session.
    assert!(
        page.contains(&format!("Location: {}/oauth/done\r\n", server.origin)),
        "{page}"
    );
    let added = added.unwrap();
    assert_eq!(added.server, server.origin);
    assert_eq!(added.signed_in.username, "alex");
    // No remote access on this server, so no tunnel.
    assert!(added.tunnel.is_none());

    let client = Client::new(&server.origin, Some(added.signed_in.token)).unwrap();
    assert_eq!(client.me().await.unwrap().username, "alex");
    let (_, sessions) = server.send("GET", "/api/sessions", None).await;
    assert!(
        sessions
            .as_array()
            .unwrap()
            .iter()
            .any(|session| session["device_name"] == "Alex's laptop" && session["client"] == "app"),
        "{sessions}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceling_in_the_browser_tells_the_client() {
    let server = start().await;
    let pending = Pending::start(&server.origin, "Alex's laptop", Reach::default())
        .await
        .unwrap();
    let url = pending.url.clone();
    let browser = async {
        // What the page's Cancel does: back to the app, with the error.
        let query = url.split_once('?').unwrap().1;
        let mut redirect = None;
        let mut state = None;
        for pair in query.split('&') {
            let (name, value) = pair.split_once('=').unwrap();
            match name {
                "redirect_uri" => redirect = Some(decode(value)),
                "state" => state = Some(value.to_owned()),
                _ => {}
            }
        }
        follow(&format!(
            "{}?error=access_denied&state={}",
            redirect.unwrap(),
            state.unwrap()
        ))
        .await
    };
    let (added, page) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(pending.finish(), browser)
    })
    .await
    .unwrap();
    assert!(page.contains("/oauth/done?result=canceled"), "{page}");
    match added {
        Err(Error::Refused(message)) => assert!(message.contains("didn't allow"), "{message}"),
        other => panic!("{:?}", other.err()),
    }
}
