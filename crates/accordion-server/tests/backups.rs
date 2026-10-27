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
            storage: None,
            updates: None,
            switches: None,
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
            Some(json!({ "name": "Blue USB disk", "kind": "folder", "path": usb })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = server
        .send(
            "POST",
            "/api/backups/targets",
            &server.owner,
            Some(json!({ "name": "Nowhere", "kind": "folder", "path": "backups" })),
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

/// rclone's S3 server, standing in for a storage provider, with one empty
/// bucket. Set `ACCORDION_RCLONE` to its path, or put it on `PATH`. It stops
/// when dropped.
struct Provider {
    _root: tempfile::TempDir,
    server: std::process::Child,
    endpoint: String,
}

impl Provider {
    fn start() -> Self {
        let rclone = std::env::var_os("ACCORDION_RCLONE")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::split_paths(&std::env::var_os("PATH")?)
                    .map(|dir| dir.join("rclone"))
                    .find(|path| path.is_file())
            })
            .expect(
                "These tests run rclone's S3 server. Install rclone 1.75.2, or set \
                 ACCORDION_RCLONE.",
            );
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("household-backups")).unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let address = format!("127.0.0.1:{port}");
        let server = std::process::Command::new(rclone)
            .args([
                "serve",
                "s3",
                "--auth-key",
                "test-key,test-secret-0123456789",
            ])
            .args(["--addr", &address])
            .arg(root.path())
            .env("RCLONE_CONFIG", "/dev/null")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        while std::net::TcpStream::connect(&address).is_err() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "rclone didn't start"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        Self {
            _root: root,
            server,
            endpoint: format!("http://{address}"),
        }
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
    }
}

#[tokio::test]
async fn an_admin_backs_up_to_a_bucket() {
    let provider = Provider::start();
    let server = start("192.168.1.30").await;
    let (status, _) = server
        .send("POST", "/api/backups/key", &server.owner, None)
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let bucket = |secret: &str| {
        json!({
            "name": "Backblaze",
            "kind": "s3",
            "endpoint": provider.endpoint,
            "bucket": "household-backups",
            "key_id": "test-key",
            "secret_key": secret,
        })
    };
    let (status, body) = server
        .send(
            "POST",
            "/api/backups/targets",
            &server.owner,
            Some(bucket("not-the-secret")),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("didn't accept the key ID and secret key"),
        "{body}"
    );

    let (status, _) = server
        .send(
            "POST",
            "/api/backups/targets",
            &server.owner,
            Some(bucket("test-secret-0123456789")),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = server
        .send("POST", "/api/backups/run", &server.owner, None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let body = server.wait_until_idle().await;
    let target = &body["targets"][0];
    let host = provider.endpoint.trim_start_matches("http://");
    assert_eq!(
        target["location"],
        format!("Bucket household-backups at {host}")
    );
    assert_eq!(target["last_backup"]["ok"], true, "{target}");
    // The page never gets the secret back.
    assert!(!body.to_string().contains("test-secret"), "{body}");
}

#[tokio::test]
async fn an_admin_backs_up_to_another_computer_over_sftp() {
    let server = start("192.168.1.30").await;
    let (status, _) = server
        .send("POST", "/api/backups/key", &server.owner, None)
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = server
        .send("GET", "/api/backups/ssh-key", &server.owner, None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let public = body["public_key"].as_str().unwrap().to_owned();
    let (status, _) = server
        .send("GET", "/api/backups/ssh-key", &server.user, None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // rclone's SFTP server, which lets in only a key that isn't Accordion's
    // until the owner adds it.
    let root = tempfile::tempdir().unwrap();
    let host_key = root.path().join("host");
    let status = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&host_key)
        .status()
        .expect("These tests run ssh-keygen. Install OpenSSH's client.");
    assert!(status.success());
    let authorized = root.path().join("authorized_keys");
    std::fs::write(
        &authorized,
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOpxJSehVegSset/luntvtC4D8ejfU/a2K5lLh1qACf6\n",
    )
    .unwrap();
    std::fs::create_dir(root.path().join("files")).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let address = format!("127.0.0.1:{port}");
    let rclone = std::env::var_os("ACCORDION_RCLONE")
        .map(PathBuf::from)
        .unwrap_or_else(|| "rclone".into());
    let serve = || {
        let server = std::process::Command::new(&rclone)
            .args(["serve", "sftp", "--addr", &address, "--key"])
            .arg(&host_key)
            .arg("--authorized-keys")
            .arg(&authorized)
            .arg(root.path().join("files"))
            .env("RCLONE_CONFIG", "/dev/null")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        while std::net::TcpStream::connect(&address).is_err() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "rclone didn't start"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        server
    };
    let mut sftp = serve();
    let folder = json!({
        "name": "NAS",
        "kind": "sftp",
        "server": address,
        "username": "accordion",
        "folder": "backups/accordion",
    });
    let (status, body) = server
        .send(
            "POST",
            "/api/backups/targets",
            &server.owner,
            Some(folder.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("didn't accept Accordion's SSH key"),
        "{body}"
    );

    sftp.kill().unwrap();
    sftp.wait().unwrap();
    std::fs::write(&authorized, format!("{public}\n")).unwrap();
    let mut sftp = serve();
    let (status, body) = server
        .send("POST", "/api/backups/targets", &server.owner, Some(folder))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, _) = server
        .send("POST", "/api/backups/run", &server.owner, None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let body = server.wait_until_idle().await;
    let target = &body["targets"][0];
    assert_eq!(target["location"], "Folder backups/accordion on 127.0.0.1");
    assert_eq!(target["last_backup"]["ok"], true, "{target}");
    sftp.kill().unwrap();
    sftp.wait().unwrap();
}

#[tokio::test]
async fn an_admin_backs_up_to_an_append_only_rest_server() {
    // rclone's REST server, standing in for rest-server on a NAS, with a
    // self-signed certificate like most servers on a home network.
    let root = tempfile::tempdir().unwrap();
    let certified = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).unwrap();
    std::fs::write(root.path().join("cert.pem"), certified.cert.pem()).unwrap();
    std::fs::write(
        root.path().join("key.pem"),
        certified.signing_key.serialize_pem(),
    )
    .unwrap();
    std::fs::create_dir(root.path().join("repos")).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let address = format!("127.0.0.1:{port}");
    let rclone = std::env::var_os("ACCORDION_RCLONE")
        .map(PathBuf::from)
        .unwrap_or_else(|| "rclone".into());
    let mut rest = std::process::Command::new(&rclone)
        .args(["serve", "restic", "--append-only", "--addr", &address])
        .arg("--cert")
        .arg(root.path().join("cert.pem"))
        .arg("--key")
        .arg(root.path().join("key.pem"))
        .args(["--user", "household", "--pass", "rest-password-0123"])
        .arg(root.path().join("repos"))
        .env("RCLONE_CONFIG", "/dev/null")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    while std::net::TcpStream::connect(&address).is_err() {
        assert!(started.elapsed().as_secs() < 10, "rclone didn't start");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let server = start("192.168.1.30").await;
    let (status, _) = server
        .send("POST", "/api/backups/key", &server.owner, None)
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let target = |password: &str| {
        json!({
            "name": "NAS",
            "kind": "rest",
            "address": format!("https://{address}/household"),
            "username": "household",
            "password": password,
        })
    };
    let (status, body) = server
        .send(
            "POST",
            "/api/backups/targets",
            &server.owner,
            Some(target("not-the-password")),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("didn't accept the username and password"),
        "{body}"
    );
    let (status, body) = server
        .send(
            "POST",
            "/api/backups/targets",
            &server.owner,
            Some(target("rest-password-0123")),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status, _) = server
        .send("POST", "/api/backups/run", &server.owner, None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let body = server.wait_until_idle().await;
    let target = &body["targets"][0];
    assert_eq!(target["kind"], "rest");
    assert_eq!(target["append_only"], true);
    assert_eq!(
        target["location"],
        format!("REST server at https://{address}/household")
    );
    assert_eq!(target["last_backup"]["ok"], true, "{target}");
    // The page never gets the password back.
    assert!(!body.to_string().contains("rest-password"), "{body}");
    let _ = rest.kill();
    let _ = rest.wait();
}
