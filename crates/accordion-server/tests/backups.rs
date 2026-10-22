//! Sets up backups over HTTP, the way the backups page does, with the real
//! restic program. Set `ACCORDION_RESTIC` to its path, or put it on `PATH`.

use std::path::PathBuf;
use std::time::Duration;

use accordion_core::backup::Backup;
use accordion_core::db::Database;
use accordion_core::identity::{AuthMethod, Client, Identity, NewUser};
use accordion_core::storage::Storage;
use accordion_gateway::Peer;
use accordion_server::health::Health;
use accordion_server::network::AdminAccess;
use accordion_server::{AppState, Site, router};
use accordion_types::Role;
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const HOST: &str = "accordion.example.com";

struct Server {
    _data: tempfile::TempDir,
    disk: tempfile::TempDir,
    app: Router,
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

/// Starts a server with an owner and a standard user, whose requests come
/// from `client`.
async fn start(client: &str) -> Server {
    let data = tempfile::tempdir().unwrap();
    let db = Database::open(&data.path().join("accordion.db")).unwrap();
    let identity = Identity::new(db.clone()).await.unwrap();
    let storage = Storage::open(db.clone(), identity.clone(), &data.path().join("storage"))
        .await
        .unwrap();
    let backup = Backup::open(db, storage, data.path(), restic())
        .await
        .unwrap();
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
                backup: Some(backup.status()),
                ..Health::default()
            },
            admin_access: AdminAccess::HomeNetwork,
            modules: Default::default(),
            backup: Some(backup),
        },
        &Site::named(HOST.parse().unwrap(), 443),
    )
    .layer(MockConnectInfo(Peer {
        local: "192.168.1.20:443".parse().unwrap(),
        remote: format!("{client}:50000").parse().unwrap(),
    }));
    let user = tokens.pop().unwrap();
    let owner = tokens.pop().unwrap();
    Server {
        _data: data,
        disk: tempfile::tempdir().unwrap(),
        app,
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

    /// Waits for a backup or test started over HTTP to finish.
    async fn wait_until_idle(&self) -> Value {
        for _ in 0..600 {
            let (_, status) = self.send("GET", "/api/backups", &self.owner, None).await;
            if status["running"] == false {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("the backup didn't finish within a minute");
    }
}

#[tokio::test]
async fn an_admin_sets_up_and_runs_backups() {
    let server = start("192.168.1.30").await;
    let (status, body) = server
        .send("GET", "/api/backups", &server.owner, None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["key"], "none");
    assert_eq!(body["hour"], 3);

    let (status, body) = server
        .send("POST", "/api/backups/key", &server.owner, None)
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let key = body["recovery_key"].as_str().unwrap().to_owned();
    assert_eq!(key.len(), 35);
    let (status, _) = server
        .send("POST", "/api/backups/key", &server.owner, None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, body) = server
        .send("GET", "/api/backups", &server.owner, None)
        .await;
    assert_eq!(
        (status, &body["key"]),
        (StatusCode::OK, &json!("unconfirmed"))
    );

    let (status, body) = server
        .send(
            "POST",
            "/api/backups/key/confirm",
            &server.owner,
            Some(json!({ "recovery_key": "not the key" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("isn't the recovery key")
    );
    let (status, _) = server
        .send(
            "POST",
            "/api/backups/key/confirm",
            &server.owner,
            Some(json!({ "recovery_key": key })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = server
        .send("POST", "/api/backups/run", &server.owner, None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let usb = server.disk.path().join("backups");
    let (status, _) = server
        .send(
            "POST",
            "/api/backups/targets",
            &server.owner,
            Some(json!({ "name": "Blue USB disk", "path": usb })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = server
        .send(
            "POST",
            "/api/backups/targets",
            &server.owner,
            Some(json!({ "name": "Nowhere", "path": "backups" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("full path"));

    let (status, _) = server
        .send("POST", "/api/backups/run", &server.owner, None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let body = server.wait_until_idle().await;
    let target = &body["targets"][0];
    assert_eq!(target["name"], "Blue USB disk");
    assert_eq!(target["last_backup"]["ok"], true, "{target}");
    assert!(target["last_good_backup"].is_u64());

    let id = target["id"].as_str().unwrap().to_owned();
    let path = format!("/api/backups/targets/{id}/test");
    let (status, _) = server.send("POST", &path, &server.owner, None).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let body = server.wait_until_idle().await;
    assert_eq!(body["targets"][0]["last_restore_test"]["ok"], true);

    let (status, _) = server
        .send(
            "PUT",
            "/api/backups/settings",
            &server.owner,
            Some(json!({ "hour": 2 })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = server
        .send("GET", "/api/backups", &server.owner, None)
        .await;
    assert_eq!(body["hour"], 2);

    // The health page reports the backup.
    let (_, health) = server.send("GET", "/api/health", &server.owner, None).await;
    let check = health["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "backups")
        .unwrap();
    assert_eq!(check["state"], "ok", "{check}");

    let (status, _) = server
        .send(
            "DELETE",
            &format!("/api/backups/targets/{id}"),
            &server.owner,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn only_admins_at_home_manage_backups() {
    let server = start("192.168.1.30").await;
    let (status, body) = server.send("GET", "/api/backups", &server.user, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body["error"].as_str().unwrap().contains("Only admins"));
    let (status, _) = server
        .send("POST", "/api/backups/key", &server.user, None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let away = start("203.0.113.5").await;
    let (status, _) = away.send("GET", "/api/backups", &away.owner, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = away
        .send("POST", "/api/backups/key", &away.owner, None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
