//! Turns remote access on and adds devices over HTTP, the way Remote access
//! and Apps and devices settings do. The tunnel really runs, on a server
//! without a name, so it listens on a UDP port but asks no router and keeps
//! no DNS record.

use std::net::IpAddr;
use std::time::Duration;

use accordion_core::db::Database;
use accordion_core::identity::{AuthMethod, Client, Identity, NewUser};
use accordion_gateway::Peer;
use accordion_gateway::address::AddressSource;
use accordion_server::health::Health;
use accordion_server::network::AdminAccess;
use accordion_server::remote::{RemoteAccess, Site as RemoteSite};
use accordion_server::{AppState, Site, router};
use accordion_types::Role;
use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const HOST: &str = "192.168.1.20:8080";

struct Server {
    _data: tempfile::TempDir,
    app: Router,
    owner: String,
    user: String,
}

async fn start() -> Server {
    let data = tempfile::tempdir().unwrap();
    let db = Database::open(&data.path().join("accordion.db")).unwrap();
    let identity = Identity::new(db.clone()).await.unwrap();
    let remote = RemoteAccess::open(db, identity.clone()).await.unwrap();
    let home: IpAddr = "192.168.1.20".parse().unwrap();
    let (_connections, work) = remote.start(
        RemoteSite::Unnamed,
        Some(AddressSource::Fixed(home)),
        &[8080],
        std::future::pending::<()>,
    );
    tokio::spawn(work);

    let mut tokens = Vec::new();
    for (username, display_name, role) in
        [("alex", "Alex", Role::Owner), ("sam", "Sam", Role::User)]
    {
        let user = identity
            .create_user(NewUser {
                username: username.into(),
                display_name: display_name.into(),
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
                remote: Some(remote.clone()),
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
            remote: Some(remote),
            link_sharing: None,
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
        self.send_from("192.168.1.30", method, path, Some(token), body)
            .await
    }

    /// Sends a request from `address`, such as a device on the tunnel.
    async fn send_from(
        &self,
        address: &str,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, HOST)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let mut request = request
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(Peer {
            local: HOST.parse().unwrap(),
            remote: format!("{address}:50000").parse().unwrap(),
        }));
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

/// A UDP port that was free a moment ago.
fn free_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
async fn an_admin_turns_it_on_and_people_add_devices() {
    let server = start().await;

    let (status, access) = server
        .send("GET", "/api/remote-access", &server.owner, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{access}");
    assert_eq!(access["available"], true);
    assert_eq!(access["named"], false);
    assert_eq!(access["on"], false);
    assert_eq!(access["port"], 51820);
    assert_eq!(access["home_address"], "192.168.1.20");

    // Nobody can add a device while it's off, and only admins turn it on.
    let (status, _) = server
        .send(
            "POST",
            "/api/remote-devices",
            &server.user,
            Some(json!({ "name": "Sam's phone" })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let port = free_port();
    let (status, _) = server
        .send(
            "PUT",
            "/api/remote-access",
            &server.user,
            Some(json!({ "on": true, "port": port })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = server
        .send(
            "PUT",
            "/api/remote-access",
            &server.owner,
            Some(json!({ "on": true, "port": port })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, added) = server
        .send(
            "POST",
            "/api/remote-devices",
            &server.user,
            Some(json!({ "name": "Sam's phone" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{added}");
    assert_eq!(added["device"]["name"], "Sam's phone");
    assert_eq!(added["device"]["out_of_date"], false);
    assert_eq!(added["file_name"], "accordion.conf");
    let config = added["config"].as_str().unwrap();
    assert!(config.contains("PrivateKey = "), "{config}");
    assert!(
        config.contains(&format!("Endpoint = 192.168.1.20:{port}")),
        "{config}"
    );
    let network = access_network(&server).await;
    assert!(
        config.contains(&format!(
            "AllowedIPs = 192.168.1.20/32, {}",
            network.replace(".0/24", ".1/32")
        )),
        "{config}"
    );
    assert!(config.contains(&format!("Address = {}", network.replace(".0/24", ".2/32"))));
    assert!(added["qr_code"].as_str().unwrap().contains("<svg"));

    // The setup's private key is shown only once.
    let (_, mine) = server
        .send("GET", "/api/remote-devices", &server.user, None)
        .await;
    assert_eq!(mine["on"], true);
    assert_eq!(mine["max"], 10);
    assert_eq!(mine["devices"].as_array().unwrap().len(), 1);
    assert!(!mine.to_string().contains("PrivateKey"));

    // The tunnel listens on the port.
    wait_for_listening(&server, true).await;
    assert!(std::net::UdpSocket::bind(("0.0.0.0", port)).is_err());

    // Admins see everyone's devices, and can remove them.
    let (_, access) = server
        .send("GET", "/api/remote-access", &server.owner, None)
        .await;
    assert_eq!(access["on"], true);
    assert_eq!(access["tunnel_problem"], Value::Null);
    let device = &access["devices"][0];
    assert_eq!(device["owner"], "Sam");
    let id = device["id"].as_str().unwrap();
    let (status, _) = server
        .send(
            "DELETE",
            &format!("/api/remote-access/devices/{id}"),
            &server.owner,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, mine) = server
        .send("GET", "/api/remote-devices", &server.user, None)
        .await;
    assert_eq!(mine["devices"], json!([]));

    // People remove their own.
    let (_, added) = server
        .send(
            "POST",
            "/api/remote-devices",
            &server.user,
            Some(json!({ "name": "Sam's laptop" })),
        )
        .await;
    let id = added["device"]["id"].as_str().unwrap();
    let (status, _) = server
        .send(
            "DELETE",
            &format!("/api/remote-devices/{id}"),
            &server.owner,
            None,
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "only through the admin's route"
    );
    let (status, _) = server
        .send(
            "DELETE",
            &format!("/api/remote-devices/{id}"),
            &server.user,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Turning it off closes the port.
    let (status, _) = server
        .send(
            "PUT",
            "/api/remote-access",
            &server.owner,
            Some(json!({ "on": false, "port": port })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    wait_for_listening(&server, false).await;
    std::net::UdpSocket::bind(("0.0.0.0", port)).expect("the port is free again");
}

#[tokio::test]
async fn apps_add_themselves_away_from_home() {
    let server = start().await;

    // Without remote access, a code is only the letters to type.
    let (status, made) = server
        .send("POST", "/api/app-codes", &server.user, None)
        .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["setup"], Value::Null);

    let port = free_port();
    let (status, _) = server
        .send(
            "PUT",
            "/api/remote-access",
            &server.owner,
            Some(json!({ "on": true, "port": port })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    wait_for_listening(&server, true).await;
    let network = access_network(&server).await;
    let prefix = network.trim_end_matches("0/24");
    let gateway = format!("{prefix}1");

    // With it, the code carries a temporary tunnel key and its address.
    let (status, made) = server
        .send("POST", "/api/app-codes", &server.user, None)
        .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let setup = made["setup"].as_str().unwrap();
    let code = made["code"].as_str().unwrap().replace(' ', "");
    assert!(
        setup.starts_with("accordion:add?server=http://192.168.1.20:8080&code="),
        "{setup}"
    );
    assert!(setup.contains(&format!("&code={code}&key=")), "{setup}");
    assert!(
        setup.contains(&format!("&endpoint=192.168.1.20:{port}")),
        "{setup}"
    );
    let temporary = format!("{prefix}254");
    assert!(
        setup.ends_with(&format!("&address={temporary}&gateway={gateway}")),
        "{setup}"
    );
    assert!(made["qr_code"].as_str().unwrap().contains("<svg"));

    // The temporary key reaches nothing but the address that redeems it.
    for (method, path) in [
        ("GET", "/api/session"),
        ("GET", "/api/remote-access"),
        ("POST", "/api/app-codes"),
        ("GET", "/api/app-codes/redeem"),
    ] {
        let (status, body) = server
            .send_from(&temporary, method, path, Some(&server.user), None)
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {body}");
    }

    // Redeeming it registers the app's own key, at an address of its own.
    let app_key = accordion_gateway::tunnel::Key::generate();
    let (status, signed_in) = server
        .send_from(
            &temporary,
            "POST",
            "/api/app-codes/redeem",
            None,
            Some(json!({
                "code": code,
                "device_name": "Sam's laptop",
                "tunnel_key": app_key.public().to_base64(),
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{signed_in}");
    let tunnel = &signed_in["tunnel"];
    assert_eq!(tunnel["gateway"], gateway.as_str());
    assert_eq!(tunnel["endpoint"], format!("192.168.1.20:{port}"));
    let address = tunnel["address"].as_str().unwrap().to_owned();
    assert_ne!(address, temporary);
    assert!(address.starts_with(prefix), "{address}");
    let token = signed_in["token"].as_str().unwrap().to_owned();

    // From its own address, it reaches everything, as at home, once the
    // tunnel lets it in.
    let mut status = StatusCode::FORBIDDEN;
    for _ in 0..50 {
        (status, _) = server
            .send_from(&address, "GET", "/api/session", Some(&token), None)
            .await;
        if status == StatusCode::OK {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(status, StatusCode::OK);
    // A freed temporary address is a stranger again.
    let (status, _) = server
        .send_from(&temporary, "GET", "/api/session", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Registering again keeps its address. Only apps register keys.
    let (status, again) = server
        .send(
            "POST",
            "/api/app-tunnel",
            &token,
            Some(json!({ "public_key": app_key.public().to_base64() })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["address"], address.as_str());
    let (status, _) = server
        .send(
            "POST",
            "/api/app-tunnel",
            &server.user,
            Some(json!({ "public_key": app_key.public().to_base64() })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // It's listed where Sam is signed in, not among the WireGuard devices.
    let (_, mine) = server
        .send("GET", "/api/remote-devices", &server.user, None)
        .await;
    assert_eq!(mine["devices"], json!([]));

    // Signing it out ends its tunnel, so its address is a stranger's.
    let (_, sessions) = server
        .send("GET", "/api/sessions", &server.user, None)
        .await;
    let laptop = sessions
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["device_name"] == "Sam's laptop")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, _) = server
        .send(
            "DELETE",
            &format!("/api/sessions/{laptop}"),
            &server.user,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    for _ in 0..50 {
        let (status, _) = server
            .send_from(&address, "GET", "/api/session", Some(&server.user), None)
            .await;
        if status == StatusCode::FORBIDDEN {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the signed-out app's address still reaches the server");
}

async fn wait_for_listening(server: &Server, listening: bool) {
    for _ in 0..50 {
        let (_, access) = server
            .send("GET", "/api/remote-access", &server.owner, None)
            .await;
        if access["listening"] == listening {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the tunnel's listening didn't become {listening}");
}

async fn access_network(server: &Server) -> String {
    let (_, access) = server
        .send("GET", "/api/remote-access", &server.owner, None)
        .await;
    access["network"].as_str().unwrap().to_owned()
}
