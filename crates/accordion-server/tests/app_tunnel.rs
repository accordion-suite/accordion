//! Accordion's sync client reaching two servers at once through tunnels of
//! its own (decision 23), the way Sam's laptop reaches the family's server
//! and an employer's from anywhere.
//!
//! Both servers really run their tunnels, and serve HTTPS through them.
//! They have the same home address and the same tunnel addresses, so the
//! only thing that tells them apart is which tunnel a request goes in. Their
//! names don't resolve, so nothing reaches them directly: the client has to
//! use the tunnels.

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use accordion_core::db::Database;
use accordion_core::identity::{AuthMethod, Client as SessionClient, Identity, NewUser};
use accordion_gateway::Peer;
use accordion_gateway::address::AddressSource;
use accordion_gateway::tls::{self, CertStore};
use accordion_gateway::tunnel::Key;
use accordion_server::health::Health;
use accordion_server::network::AdminAccess;
use accordion_server::remote::{RemoteAccess, Site as RemoteSite};
use accordion_server::{AppState, Site, router};
use accordion_sync::add::{CodeSetup, add};
use accordion_sync::{Client, Error, Reach, Via};
use accordion_types::Role;
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use rustls::pki_types::CertificateDer;
use serde_json::Value;
use tower::ServiceExt;

/// Both servers' address on their home networks.
const HOME: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
/// Both servers' tunnel addresses.
const NETWORK: &str = "10.77.3.1";

struct Server {
    _data: tempfile::TempDir,
    name: &'static str,
    /// The routes, as a browser at home reaches them.
    app: Router,
    /// A browser's session, for making codes.
    browser: String,
    /// An app's session, as for an app added at home.
    app_session: String,
    cert: CertificateDer<'static>,
    /// Stops serving directly, as if the device left home.
    stop_direct: Option<tokio::sync::oneshot::Sender<()>>,
}

/// How a test server serves.
enum Serve {
    /// HTTPS for its name, only through the tunnel.
    TunnelOnly,
    /// Plain HTTP on 127.0.0.1 at this port, and through the tunnel, as a
    /// development server does.
    Both(std::net::TcpListener),
}

/// A UDP port that was free a moment ago.
fn free_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A server called `name`, with remote access on and one person, who has
/// signed in from a browser and an app.
async fn start(name: &'static str, username: &str, serve: Serve) -> Server {
    let data = tempfile::tempdir().unwrap();
    let db = Database::open(&data.path().join("accordion.db")).unwrap();
    let identity = Identity::new(db.clone()).await.unwrap();
    // Remote access on, with the same tunnel addresses as the other server.
    RemoteAccess::open(db.clone(), identity.clone())
        .await
        .unwrap();
    let port = free_port();
    let key = Key::generate().to_base64();
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO remote_access (id, turned_on, port, network, private_key, label)
             VALUES (1, 1, ?1, ?2, ?3, 'testlabel0')",
            (port, NETWORK, key),
        )
    })
    .await
    .unwrap();
    let remote = RemoteAccess::open(db, identity.clone()).await.unwrap();
    let web_port = match &serve {
        Serve::TunnelOnly => 443,
        Serve::Both(listener) => listener.local_addr().unwrap().port(),
    };
    let (mut connections, work) = remote.start(
        RemoteSite::Unnamed,
        Some(AddressSource::Fixed(HOME)),
        &[web_port],
        std::future::pending::<()>,
    );
    tokio::spawn(work);

    let user = identity
        .create_user(NewUser {
            username: username.into(),
            display_name: username.into(),
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
            AuthMethod::Password,
        )
        .await
        .unwrap();
    let (app_session, _) = identity
        .create_session(
            user.id,
            SessionClient::App,
            "Alex's tablet",
            AuthMethod::Password,
        )
        .await
        .unwrap();
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
            remote: Some(remote.clone()),
            link_sharing: None,
            thumbnails: None,
            web: None,
        },
        &Site::Unnamed,
    );

    let certified = rcgen::generate_simple_self_signed(vec![name.to_owned()]).unwrap();
    let certs = CertStore::new();
    certs
        .set_pem(
            certified.cert.pem().as_bytes(),
            certified.signing_key.serialize_pem().as_bytes(),
        )
        .unwrap();
    let mut stop_direct = None;
    let tls_config = match serve {
        Serve::TunnelOnly => Some(tls::server_config(certs)),
        Serve::Both(listener) => {
            listener.set_nonblocking(true).unwrap();
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
            stop_direct = Some(stop);
            let direct = app.clone();
            tokio::spawn(async move {
                axum::serve(
                    listener,
                    direct.into_make_service_with_connect_info::<Peer>(),
                )
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
            });
            None
        }
    };
    tokio::spawn(tls::serve_tunnel(
        connections.pop().unwrap(),
        tls_config,
        app.clone(),
        std::future::pending::<()>(),
    ));
    for _ in 0..100 {
        if remote
            .health()
            .is_some_and(|health| health.tunnel.listening.is_some())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    Server {
        _data: data,
        name,
        app: app.layer(MockConnectInfo(Peer {
            local: "192.168.1.20:443".parse().unwrap(),
            remote: "192.168.1.30:50000".parse().unwrap(),
        })),
        browser: token.as_str().to_owned(),
        app_session: app_session.as_str().to_owned(),
        cert: certified.cert.der().clone(),
        stop_direct,
    }
}

impl Server {
    async fn send(&self, method: &str, path: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, self.name)
            .header(header::AUTHORIZATION, format!("Bearer {}", self.browser))
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

    /// Makes a code in **Apps and devices**, and returns its whole setup.
    async fn code(&self) -> String {
        let (status, made) = self.send("POST", "/api/app-codes").await;
        assert_eq!(status, StatusCode::CREATED, "{made}");
        made["setup"].as_str().unwrap().to_owned()
    }

    fn reach(&self) -> Reach {
        Reach {
            trust: vec![self.cert.clone()],
            ..Reach::default()
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_app_reaches_two_servers_with_the_same_addresses_through_their_tunnels() {
    let home = start("home.test", "alex", Serve::TunnelOnly).await;
    let work = start("work.test", "sam", Serve::TunnelOnly).await;

    // A code's temporary key only redeems the code, even with a session.
    let setup = home.code().await;
    let parsed = CodeSetup::parse(&setup).unwrap();
    assert_eq!(parsed.server, "https://home.test");
    let temporary = Client::with_reach(
        &parsed.server,
        Some(home.browser.clone()),
        Reach {
            tunnel: Some(parsed.tunnel),
            ..home.reach()
        },
    )
    .unwrap();
    match temporary.me().await {
        Err(Error::Refused(message)) => {
            assert!(message.contains("only add an Accordion app"), "{message}");
        }
        other => panic!("the temporary key reached the server: {:?}", other.err()),
    }
    assert_eq!(temporary.via(), Some(Via::Tunnel));
    // One tunnel for each key, as on a real device: WireGuard sends replies
    // to wherever the key last spoke from.
    drop(temporary);

    // Adding both accounts with their codes' setups, through the codes'
    // temporary tunnels.
    let added_home = add(&setup, None, "Sam's laptop", home.reach())
        .await
        .unwrap();
    let added_work = add(&work.code().await, None, "Sam's laptop", work.reach())
        .await
        .unwrap();
    assert_eq!(added_home.server, "https://home.test");
    assert_eq!(added_work.server, "https://work.test");
    let home_tunnel = added_home.tunnel.unwrap();
    let work_tunnel = added_work.tunnel.unwrap();
    // The same addresses on both tunnels: only the tunnel tells them apart.
    assert_eq!(home_tunnel.address, work_tunnel.address);
    assert_eq!(home_tunnel.gateway, work_tunnel.gateway);
    assert_ne!(home_tunnel.server_key, work_tunnel.server_key);

    // Both at once, each through its own tunnel, each its own account.
    let client = |server: &Server, token: &str, tunnel| {
        Client::with_reach(
            &format!("https://{}", server.name),
            Some(token.to_owned()),
            Reach {
                tunnel: Some(tunnel),
                ..server.reach()
            },
        )
        .unwrap()
    };
    let home_client = client(&home, &added_home.signed_in.token, home_tunnel.clone());
    let work_client = client(&work, &added_work.signed_in.token, work_tunnel.clone());
    for _ in 0..3 {
        let (at_home, at_work) = tokio::join!(home_client.me(), work_client.me());
        assert_eq!(at_home.unwrap().username, "alex");
        assert_eq!(at_work.unwrap().username, "sam");
    }
    assert_eq!(home_client.via(), Some(Via::Tunnel));
    assert_eq!(work_client.via(), Some(Via::Tunnel));

    // Registering the key again, as the app does when it starts, keeps
    // the setup.
    let public = accordion_sync::tunnel::public_key(&home_tunnel.private_key).unwrap();
    let again = home_client.register_tunnel(&public).await.unwrap();
    assert_eq!(again.address, home_tunnel.address);

    // Signing the laptop out at home cuts off its tunnel there, and only
    // there.
    let (_, sessions) = home.send("GET", "/api/sessions").await;
    let laptop = sessions
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["device_name"] == "Sam's laptop")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, _) = home
        .send("DELETE", &format!("/api/sessions/{laptop}"))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // A new client, as after a restart, so no connection is left open.
    let signed_out = client(&home, &added_home.signed_in.token, home_tunnel);
    assert!(matches!(signed_out.me().await, Err(Error::Unreachable(_))));
    assert_eq!(work_client.me().await.unwrap().username, "sam");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_app_goes_direct_at_home_and_through_the_tunnel_away() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mut home = start("home.test", "alex", Serve::Both(listener)).await;

    // An app added at home registers its key when it starts.
    let (private_key, public_key) = accordion_sync::tunnel::new_key();
    let at_home = Client::new(&origin, Some(home.app_session.clone())).unwrap();
    let tunnel = at_home
        .register_tunnel(&public_key)
        .await
        .unwrap()
        .with_key(private_key);

    // At home, the direct way answers first.
    let client = Client::with_reach(
        &origin,
        Some(home.app_session.clone()),
        Reach {
            tunnel: Some(tunnel.clone()),
            ..Reach::default()
        },
    )
    .unwrap();
    let client_tunnel = tunnel;
    assert_eq!(client.me().await.unwrap().username, "alex");
    assert_eq!(client.via(), Some(Via::Direct));

    // Away from home, the direct way stops answering, and the next
    // connection goes through the tunnel, with nothing to turn on.
    home.stop_direct.take().unwrap().send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut reached = None;
    for _ in 0..5 {
        if let Ok(me) = client.me().await {
            reached = Some(me.username);
            break;
        }
    }
    assert_eq!(reached.as_deref(), Some("alex"));
    assert_eq!(client.via(), Some(Via::Tunnel));

    // The app starting again makes a new tunnel with the same key, from a
    // new port, which the server follows.
    drop(client);
    let again = Client::with_reach(
        &origin,
        Some(home.app_session.clone()),
        Reach {
            tunnel: Some(client_tunnel),
            only: Some(Via::Tunnel),
            ..Reach::default()
        },
    )
    .unwrap();
    assert_eq!(again.me().await.unwrap().username, "alex");
}
