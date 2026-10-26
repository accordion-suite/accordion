//! Checks for updates over HTTP, the way the updates page does, against a
//! stand-in for GitHub's releases on this machine, signed with a test key.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use accordion_core::db::Database;
use accordion_core::identity::{AuthMethod, Client, Identity, NewUser};
use accordion_gateway::Peer;
use accordion_server::health::Health;
use accordion_server::network::AdminAccess;
use accordion_server::updates::{UpdateSource, Updates};
use accordion_server::{AppState, Site, router};
use accordion_types::Role;
use accordion_update::PublicKey;
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use http_body_util::BodyExt;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{Value, json};
use tower::ServiceExt;

const HOST: &str = "accordion.example.com";

/// Each file of one release, by name.
type ReleaseFiles = HashMap<String, Vec<u8>>;

/// The files of each release, by tag, and which tag is the newest.
#[derive(Clone, Default)]
struct Releases {
    files: Arc<Mutex<HashMap<String, ReleaseFiles>>>,
    latest: Arc<Mutex<Option<String>>>,
}

impl Releases {
    /// Publishes a release with `update.json`, signed with `key`.
    fn publish(&self, key: &Ed25519KeyPair, version: &str, security: bool) {
        let manifest = json!({
            "version": version,
            "published": "2026-11-02",
            "security": security,
            "installs_itself": true,
            "notes": "Drive shows photos in a grid.",
            "link": format!("https://github.com/accordion-suite/accordion/releases/tag/v{version}"),
        })
        .to_string()
        .into_bytes();
        self.publish_files(key, version, vec![("update.json".into(), manifest)]);
    }

    fn publish_files(&self, key: &Ed25519KeyPair, version: &str, files: Vec<(String, Vec<u8>)>) {
        let mut sums = String::new();
        for (name, contents) in &files {
            let hex: String = accordion_update::sha256(contents)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            sums.push_str(&format!("{hex}  {name}\n"));
        }
        let signature = key.sign(sums.as_bytes()).as_ref().to_vec();
        let mut release: ReleaseFiles = files.into_iter().collect();
        release.insert("SHA256SUMS".into(), sums.into_bytes());
        release.insert("SHA256SUMS.sig".into(), signature);
        let tag = format!("v{version}");
        self.files.lock().unwrap().insert(tag.clone(), release);
        *self.latest.lock().unwrap() = Some(tag);
    }

    fn replace(&self, version: &str, name: &str, contents: &[u8]) {
        self.files
            .lock()
            .unwrap()
            .get_mut(&format!("v{version}"))
            .unwrap()
            .insert(name.into(), contents.to_vec());
    }

    /// Serves files the way GitHub does: the newest release's address
    /// redirects to the release's own folder.
    async fn serve(self) -> String {
        async fn latest(State(releases): State<Releases>, Path(name): Path<String>) -> Response {
            match releases.latest.lock().unwrap().clone() {
                Some(tag) => Redirect::to(&format!("/download/{tag}/{name}")).into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }
        async fn file(
            State(releases): State<Releases>,
            Path((tag, name)): Path<(String, String)>,
        ) -> Response {
            match releases
                .files
                .lock()
                .unwrap()
                .get(&tag)
                .and_then(|files| files.get(&name))
            {
                Some(contents) => contents.clone().into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }
        let app = Router::new()
            .route("/latest/download/{name}", get(latest))
            .route("/download/{tag}/{name}", get(file))
            .with_state(self);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{address}/latest/download")
    }
}

fn key_pair() -> Ed25519KeyPair {
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
}

struct Server {
    data: tempfile::TempDir,
    app: Router,
    owner: String,
    user: String,
    updates: Updates,
}

async fn open_updates(
    data: &tempfile::TempDir,
    base: &str,
    key: &Ed25519KeyPair,
    installed: &str,
) -> Updates {
    let db = Database::open(&data.path().join("accordion.db")).unwrap();
    Updates::open(
        db,
        UpdateSource {
            base: base.to_owned(),
            key: PublicKey(key.public_key().as_ref().try_into().unwrap()),
            allow_http: true,
        },
        installed.parse().unwrap(),
    )
    .await
    .unwrap()
}

/// Starts a server running `installed`, with an owner and a standard user,
/// that looks for releases at `base`.
async fn start(base: &str, key: &Ed25519KeyPair, installed: &str) -> Server {
    let data = tempfile::tempdir().unwrap();
    let db = Database::open(&data.path().join("accordion.db")).unwrap();
    let identity = Identity::new(db).await.unwrap();
    let updates = open_updates(&data, base, key, installed).await;
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
                updates: Some(updates.status()),
                ..Health::default()
            },
            admin_access: AdminAccess::HomeNetwork,
            modules: Default::default(),
            backup: None,
            storage: None,
            updates: Some(updates.clone()),
        },
        &Site::named(HOST.parse().unwrap(), 443),
    )
    .layer(MockConnectInfo(Peer {
        local: "192.168.1.20:443".parse().unwrap(),
        remote: "192.168.1.30:50000".parse().unwrap(),
    }));
    let user = tokens.pop().unwrap();
    let owner = tokens.pop().unwrap();
    Server {
        data,
        app,
        owner,
        user,
        updates,
    }
}

impl Server {
    async fn send(&self, method: &str, path: &str, token: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, HOST)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn check(&self) -> Value {
        let (status, body) = self.send("POST", "/api/updates/check", &self.owner).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    async fn health(&self) -> String {
        let (_, report) = self.send("GET", "/api/health", &self.owner).await;
        let check = report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["id"] == "updates")
            .unwrap()
            .clone();
        format!(
            "{}: {}",
            check["state"].as_str().unwrap(),
            check["summary"].as_str().unwrap()
        )
    }
}

#[tokio::test]
async fn an_admin_sees_a_newer_release() {
    let key = key_pair();
    let releases = Releases::default();
    let base = releases.clone().serve().await;
    let server = start(&base, &key, "0.1.0").await;

    let (status, before) = server.send("GET", "/api/updates", &server.owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(before["installed"], "0.1.0");
    assert_eq!(before["last_check"], Value::Null);
    assert!(server.health().await.contains("hasn't checked yet"));

    // Before the first full release, there's nothing to update to.
    let none = server.check().await;
    assert_eq!(none["last_check"]["problem"], Value::Null, "{none}");
    assert_eq!(none["newest"], Value::Null);
    assert_eq!(none["waiting"], false);

    releases.publish(&key, "0.2.0", false);
    let found = server.check().await;
    assert_eq!(found["waiting"], true, "{found}");
    assert_eq!(found["newest"]["version"], "0.2.0");
    assert_eq!(found["newest"]["published"], "2026-11-02");
    assert_eq!(found["newest"]["notes"], "Drive shows photos in a grid.");
    assert_eq!(found["newest"]["security"], false);
    assert_eq!(
        server.health().await,
        "ok: Accordion 0.2.0 is available, released November 2, 2026. This server runs 0.1.0. \
         The Updates page says how to install it."
    );

    releases.publish(&key, "0.2.1", true);
    let security = server.check().await;
    assert_eq!(security["newest"]["version"], "0.2.1");
    assert!(
        server
            .health()
            .await
            .starts_with("warning: Accordion 0.2.1 is available")
            && server.health().await.contains("fixes a security problem"),
    );

    // What it found is kept, and the release keeps the day it was first
    // seen, which the wait before installing automatically counts from.
    let first_seen = server
        .updates
        .status()
        .borrow()
        .newest
        .clone()
        .unwrap()
        .first_seen;
    let reopened = open_updates(&server.data, &base, &key, "0.1.0").await;
    let newest = reopened.status().borrow().newest.clone().unwrap();
    assert_eq!(newest.version.to_string(), "0.2.1");
    assert_eq!(newest.first_seen, first_seen);
    reopened.check().await.unwrap();
    assert_eq!(
        reopened
            .status()
            .borrow()
            .newest
            .clone()
            .unwrap()
            .first_seen,
        first_seen
    );
}

#[tokio::test]
async fn a_server_on_the_newest_version_is_up_to_date() {
    let key = key_pair();
    let releases = Releases::default();
    let base = releases.clone().serve().await;
    releases.publish(&key, "0.2.0", false);

    let current = start(&base, &key, "0.2.0").await;
    let status = current.check().await;
    assert_eq!(status["waiting"], false);
    assert_eq!(status["newest"]["version"], "0.2.0");
    assert_eq!(
        current.health().await,
        "ok: This server runs Accordion 0.2.0, which is up to date."
    );

    // Someone serving an old release can't make a newer server go back.
    let newer = start(&base, &key, "0.3.0-rc.1").await;
    assert_eq!(newer.check().await["waiting"], false);
}

#[tokio::test]
async fn refuses_releases_that_arent_signed_with_the_release_key() {
    let key = key_pair();
    let releases = Releases::default();
    let base = releases.clone().serve().await;
    let server = start(&base, &key, "0.1.0").await;

    // Signed, but with someone else's key.
    releases.publish(&key_pair(), "0.2.0", false);
    let other_key = server.check().await;
    assert_eq!(other_key["waiting"], false);
    assert_eq!(other_key["newest"], Value::Null);
    let problem = other_key["last_check"]["problem"].as_str().unwrap();
    assert!(
        problem.contains("doesn't match Accordion's release key"),
        "{problem}"
    );
    assert!(server.health().await.starts_with("warning:"));

    // Signed with the right key, but update.json changed afterward.
    releases.publish(&key, "0.2.0", false);
    releases.replace("0.2.0", "update.json", br#"{"version": "9.9.9"}"#);
    let changed = server.check().await;
    assert_eq!(changed["newest"], Value::Null);
    let problem = changed["last_check"]["problem"].as_str().unwrap();
    assert!(
        problem.contains("update.json doesn't match its checksum"),
        "{problem}"
    );

    // Not signed yet, such as a draft that's still being checked.
    releases.publish(&key, "0.2.1", false);
    releases
        .files
        .lock()
        .unwrap()
        .get_mut("v0.2.1")
        .unwrap()
        .remove("SHA256SUMS.sig");
    let unsigned = server.check().await;
    let problem = unsigned["last_check"]["problem"].as_str().unwrap();
    assert!(problem.contains("isn't signed yet"), "{problem}");

    // Once a proper release is out, it's found.
    releases.publish(&key, "0.2.2", false);
    let good = server.check().await;
    assert_eq!(good["last_check"]["problem"], Value::Null);
    assert_eq!(good["newest"]["version"], "0.2.2");
}

#[tokio::test]
async fn says_why_it_couldnt_check() {
    let key = key_pair();
    // Port 1 on this machine never answers.
    let server = start("http://127.0.0.1:1/latest/download", &key, "0.1.0").await;
    let status = server.check().await;
    let problem = status["last_check"]["problem"].as_str().unwrap();
    assert!(
        problem.starts_with("Accordion couldn't reach 127.0.0.1"),
        "{problem}"
    );
    assert!(
        server
            .health()
            .await
            .ends_with("Accordion tries again in a few hours.")
    );
}

#[tokio::test]
async fn only_admins_manage_updates() {
    let key = key_pair();
    let server = start("http://127.0.0.1:1/latest/download", &key, "0.1.0").await;
    for (method, path) in [("GET", "/api/updates"), ("POST", "/api/updates/check")] {
        let (status, body) = server.send(method, path, &server.user).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
        assert_eq!(
            body["error"],
            "Only admins can manage updates. Ask your admin to check them."
        );
    }
    let (status, _) = server.send("GET", "/api/updates", "not-a-token").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
