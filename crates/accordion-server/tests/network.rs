//! Turns connections off and on over HTTP, the way Network settings do.
//! Adding a backup location runs the real restic program. Set
//! `ACCORDION_RESTIC` to its path, or put it on `PATH`.

use std::path::PathBuf;

use accordion_core::backup::{self, Backup};
use accordion_core::db::Database;
use accordion_core::identity::{AuthMethod, Client, Identity, NewUser};
use accordion_core::storage::Storage;
use accordion_gateway::Peer;
use accordion_gateway::address::AddressStatus;
use accordion_gateway::certificates::{Authority, CertificateStatus};
use accordion_gateway::dns::ProviderKind;
use accordion_server::connections::Switches;
use accordion_server::health::{Health, NameHealth};
use accordion_server::network::AdminAccess;
use accordion_server::updates::{UpdateSource, Updates};
use accordion_server::{AppState, Site, router};
use accordion_types::Role;
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tokio::sync::watch;
use tower::ServiceExt;

const HOST: &str = "accordion.example.com";

struct Server {
    data: tempfile::TempDir,
    app: Router,
    backup: Backup,
    switches: Switches,
    owner: String,
    user: String,
}

fn restic() -> PathBuf {
    if let Some(path) = std::env::var_os("ACCORDION_RESTIC") {
        return path.into();
    }
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join("restic"))
                .find(|path| path.is_file())
        })
        .expect("These tests run restic. Install restic 0.19.1, or set ACCORDION_RESTIC.")
}

/// Starts a named server with an owner and a standard user. `address` is
/// `false` for a server whose settings leave the address record alone.
async fn start(address: bool) -> Server {
    let data = tempfile::tempdir().unwrap();
    let db = Database::open(&data.path().join("accordion.db")).unwrap();
    let identity = Identity::new(db.clone()).await.unwrap();
    let storage = Storage::open(db.clone(), identity.clone(), &data.path().join("storage"))
        .await
        .unwrap();
    let backup = Backup::open(db.clone(), storage, data.path(), restic())
        .await
        .unwrap();
    // Nothing here runs the checker, so it never connects to GitHub.
    let updates = Updates::open(
        db.clone(),
        UpdateSource::github(),
        "0.1.0".parse().unwrap(),
        data.path(),
    )
    .await
    .unwrap();
    let switches = Switches::open(db).await.unwrap();
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
            authority: Authority::LetsEncrypt,
            provider: ProviderKind::Desec,
            certificate: watch::channel(CertificateStatus::default()).1,
            address: address.then(|| watch::channel(AddressStatus::default()).1),
        }),
        backup: Some(backup.status()),
        updates: Some(updates.status()),
        ..Health::default()
    };
    let app = router(
        AppState {
            identity,
            setup: None,
            health,
            admin_access: AdminAccess::HomeNetwork,
            modules: Default::default(),
            backup: Some(backup.clone()),
            storage: None,
            updates: Some(updates),
            switches: Some(switches.clone()),
            web: None,
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
        backup,
        switches,
        owner,
        user,
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

    async fn network(&self) -> Value {
        let (status, body) = self.send("GET", "/api/network", &self.owner, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    async fn turn(&self, path: &str, on: bool) -> (StatusCode, Value) {
        self.send("PUT", path, &self.owner, Some(json!({ "on": on })))
            .await
    }
}

#[tokio::test]
async fn lists_each_connection_and_where_it_goes() {
    let server = start(true).await;
    let network = server.network().await;
    assert_eq!(network["updates"]["on"], true);
    assert_eq!(network["updates"]["host"], "github.com");
    assert_eq!(network["updates"]["last"], Value::Null);
    assert_eq!(network["address"]["on"], true);
    assert_eq!(network["address"]["off_by_setting"], false);
    assert_eq!(network["address"]["host"], "desec.io");
    assert_eq!(
        network["certificate"]["authority_host"],
        "acme-v02.api.letsencrypt.org"
    );
    assert_eq!(network["certificate"]["provider_host"], "desec.io");
    assert_eq!(network["backups"], json!([]));

    // Only admins see the page's data.
    let (status, _) = server.send("GET", "/api/network", &server.user, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = server
        .send(
            "PUT",
            "/api/network/updates",
            &server.user,
            Some(json!({ "on": false })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn turns_the_update_check_and_address_updates_off_and_on() {
    let server = start(true).await;
    let (status, _) = server.turn("/api/network/updates", false).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = server.turn("/api/network/address", false).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let network = server.network().await;
    assert_eq!(network["updates"]["on"], false);
    assert_eq!(network["address"]["on"], false);
    assert!(!*server.switches.address().borrow());
    let (_, updates) = server
        .send("GET", "/api/updates", &server.owner, None)
        .await;
    assert_eq!(updates["check_on"], false);

    // The switches are in the database, so they last across restarts.
    let db = Database::open(&server.data.path().join("accordion.db")).unwrap();
    let reopened = Switches::open(db.clone()).await.unwrap();
    assert!(!*reopened.address().borrow());
    let updates = Updates::open(
        db,
        UpdateSource::github(),
        "0.1.0".parse().unwrap(),
        server.data.path(),
    )
    .await
    .unwrap();
    assert!(!updates.status().borrow().check_on);

    server.turn("/api/network/updates", true).await;
    server.turn("/api/network/address", true).await;
    let network = server.network().await;
    assert_eq!(network["updates"]["on"], true);
    assert_eq!(network["address"]["on"], true);
}

#[tokio::test]
async fn explains_an_address_switch_the_settings_turn_off() {
    let server = start(false).await;
    let network = server.network().await;
    assert_eq!(network["address"]["off_by_setting"], true);
    let (status, body) = server.turn("/api/network/address", true).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("ACCORDION_ADDRESS")
    );
}

#[tokio::test]
async fn pauses_a_backup_location() {
    let server = start(true).await;
    server.backup.create_recovery_key().await.unwrap();
    let disk = tempfile::tempdir().unwrap();
    let target = server
        .backup
        .add_target("USB disk", &backup::Place::Folder(disk.path().into()))
        .await
        .unwrap();
    let path = format!("/api/network/backups/{}", target.id);

    let (status, _) = server.turn(&path, false).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, backups) = server
        .send("GET", "/api/backups", &server.owner, None)
        .await;
    assert_eq!(backups["targets"][0]["paused"], true);
    // With every location paused, Back up now explains why it can't.
    let (status, body) = server
        .send("POST", "/api/backups/run", &server.owner, None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"].as_str().unwrap().contains("Network settings"));

    let (status, _) = server.turn(&path, true).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!server.backup.status().borrow().targets[0].target.paused);

    server.backup.remove_target(target.id).await.unwrap();
    let (status, _) = server.turn(&path, false).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
