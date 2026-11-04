//! Turns link sharing on and off over HTTP, the way Remote access settings
//! do, and opens link pages on the real link listener. The server has no
//! name, so the listener serves plain HTTP and asks no router.

use std::net::IpAddr;
use std::time::Duration;

use accordion_core::db::Database;
use accordion_core::identity::{AuthMethod, Client, Identity, NewUser};
use accordion_gateway::Peer;
use accordion_gateway::address::AddressSource;
use accordion_server::health::Health;
use accordion_server::link_sharing::{LinkSharing, Site as LinkSite};
use accordion_server::network::AdminAccess;
use accordion_server::{AppState, Site, router};
use accordion_types::Role;
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode, header};
use axum::routing::get;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

const HOST: &str = "192.168.1.20:8080";

struct Server {
    _data: tempfile::TempDir,
    app: Router,
    sharing: LinkSharing,
    owner: String,
    user: String,
    port: u16,
}

/// A TCP port that was free a moment ago.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn start() -> Server {
    let data = tempfile::tempdir().unwrap();
    let db = Database::open(&data.path().join("accordion.db")).unwrap();
    let identity = Identity::new(db.clone()).await.unwrap();
    let sharing = LinkSharing::open(db).await.unwrap();
    // A stand-in for the modules' link pages.
    let pages = Router::new().route("/s/{token}", get(|| async { "a link page" }));
    let port = free_port();
    let home: IpAddr = "192.168.1.20".parse().unwrap();
    tokio::spawn(sharing.start(
        LinkSite::Unnamed,
        Some(AddressSource::Fixed(home)),
        ([127, 0, 0, 1], port).into(),
        pages,
        std::future::pending::<()>,
    ));

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
    let app = router(
        AppState {
            identity,
            setup: None,
            health: Health {
                link_sharing: Some(sharing.clone()),
                ..Health::default()
            },
            admin_access: AdminAccess::HomeNetwork,
            modules: Default::default(),
            backup: None,
            storage: None,
            updates: None,
            switches: None,
            events: None,
            live: None,
            remote: None,
            link_sharing: Some(sharing.clone()),
            web: None,
        },
        &Site::Unnamed,
    )
    .layer(MockConnectInfo(Peer {
        local: HOST.parse().unwrap(),
        remote: "192.168.1.30:50000".parse().unwrap(),
    }));
    let user = tokens.pop().unwrap();
    let owner = tokens.pop().unwrap();
    Server {
        _data: data,
        app,
        sharing,
        owner,
        user,
        port,
    }
}

impl Server {
    async fn send(
        &self,
        method: &str,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, HOST)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
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

    async fn wait_for_listening(&self, listening: bool) {
        for _ in 0..50 {
            let (_, status) = self
                .send("GET", "/api/link-sharing", &self.owner, None)
                .await;
            if status["listening"] == listening {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("the link listener's listening didn't become {listening}");
    }
}

/// One plain HTTP request to the link listener.
async fn http_get(port: u16, path: &str) -> std::io::Result<String> {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await?;
    let mut response = String::new();
    stream.read_to_string(&mut response).await?;
    Ok(response)
}

#[tokio::test]
async fn an_admin_turns_it_on_and_links_open_on_their_own_port() {
    let server = start().await;
    let mut origin = server.sharing.origin();
    assert_eq!(*origin.borrow(), None);

    let (status, sharing) = server
        .send("GET", "/api/link-sharing", &server.owner, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{sharing}");
    assert_eq!(sharing["available"], true);
    assert_eq!(sharing["named"], false);
    assert_eq!(sharing["on"], false);
    assert!(
        http_get(server.port, "/s/abc").await.is_err(),
        "nothing listens while it's off"
    );

    // Only admins turn it on.
    let (status, _) = server
        .send(
            "PUT",
            "/api/link-sharing",
            &server.user,
            Some(json!({ "on": true })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = server
        .send(
            "PUT",
            "/api/link-sharing",
            &server.owner,
            Some(json!({ "on": true })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    server.wait_for_listening(true).await;
    let expected = format!("http://127.0.0.1:{}", server.port);
    origin
        .wait_for(|origin| origin.as_deref() == Some(expected.as_str()))
        .await
        .unwrap();

    let page = http_get(server.port, "/s/abc").await.unwrap();
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.ends_with("a link page"), "{page}");
    // The server's own routes aren't there.
    let api = http_get(server.port, "/api/session").await.unwrap();
    assert!(api.starts_with("HTTP/1.1 404"), "{api}");

    let (_, health) = server.send("GET", "/api/health", &server.owner, None).await;
    let check = health["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "link_sharing")
        .unwrap()
        .clone();
    assert_eq!(check["state"], "ok");

    // Turning it off closes the port, and links have nowhere to go.
    let (status, _) = server
        .send(
            "PUT",
            "/api/link-sharing",
            &server.owner,
            Some(json!({ "on": false })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    server.wait_for_listening(false).await;
    origin.wait_for(Option::is_none).await.unwrap();
    assert!(http_get(server.port, "/s/abc").await.is_err());
}
