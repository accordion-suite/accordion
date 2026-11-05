//! Remote access (decision 21 in `docs/decisions`): the admin's switch and
//! port, each person's devices, and, on a server with a name, the tunnel,
//! the router's port mapping, and the address record that run while it's
//! on.
//!
//! The admin's routes answer only on the home network, like the other admin
//! APIs. Devices on the tunnel count as the home network. Each person adds
//! and removes their own devices from anywhere they're signed in.
//!
//! Accordion's own apps register their own tunnel key for their session
//! (decision 23), and a code that adds an app away from home carries a
//! temporary key, which reaches only the address that redeems codes.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use accordion_core::db::rusqlite::OptionalExtension;
use accordion_core::db::{self, Database, Migration};
use accordion_core::events::{Action, NewEvent};
use accordion_core::identity::{
    Identity, MAX_REMOTE_DEVICES, NewRemoteDevice, RemoteDevice, Session, SignedIn,
};
use accordion_gateway::ServerName;
use accordion_gateway::address::AddressSource;
use accordion_gateway::dns::{Provider, ProviderKind};
use accordion_gateway::endpoint::{EndpointInput, EndpointPublisher, EndpointStatus};
use accordion_gateway::port_mapping::{
    Forward, PortMapper, PortStatus, Protocol, Router as HomeRouter,
};
use accordion_gateway::tunnel::{
    Accepted, DEFAULT_PORT, DevicePeer, DeviceSetup, Key, Listeners, Network, PublicKey,
    RemoteTunnel, TunnelConfig, TunnelStatus,
};
use accordion_module::auth::actor;
use accordion_module::{ApiError, Authenticated};
use accordion_types::{RemoteDeviceId, Role};
use axum::extract::{Path, Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};

use crate::{AppState, log};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "remote access",
    sql: "
    -- One row, made the first time an admin turns remote access on.
    CREATE TABLE remote_access (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        turned_on INTEGER NOT NULL CHECK (turned_on IN (0, 1)),
        -- The UDP port the tunnel listens on.
        port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
        -- The tunnel's /24 in 10.0.0.0/8, such as 10.42.7.0.
        network TEXT NOT NULL,
        -- The server's WireGuard private key, in base64.
        private_key TEXT NOT NULL,
        -- The random label of the record devices find home at, such as
        -- k3x9q2wm7f in k3x9q2wm7f.ourhome.dedyn.io.
        label TEXT NOT NULL
    ) STRICT;
    ",
}];

/// How often the server checks its own home address, which the router can
/// change after a restart.
const HOME_CHECK: Duration = Duration::from_secs(60);

/// Connection times are saved when they've moved on by this much, so the
/// database isn't written every few minutes for each device.
const CONNECTED_PRECISION: Duration = Duration::from_secs(10 * 60);

/// Room for connections waiting for the gateway's servers.
const WAITING_CONNECTIONS: usize = 64;

/// How often the tunnel's keys are loaded again, so a code's temporary key
/// stops working soon after the code expires.
const PEERS_CHECK: Duration = Duration::from_secs(30);

/// The only address a code's temporary key reaches.
pub(crate) const REDEEM_PATH: &str = "/api/app-codes/redeem";

/// The saved settings.
#[derive(Clone, Debug, PartialEq)]
struct Saved {
    on: bool,
    port: u16,
    network: Network,
    key: Key,
    label: String,
}

/// Remote access's settings and, once the server serves its name, what's
/// running. Cloning it is cheap.
#[derive(Clone)]
pub struct RemoteAccess {
    db: Database,
    identity: Identity,
    saved: Arc<watch::Sender<Option<Saved>>>,
    running: Arc<OnceLock<Running>>,
    /// The tunnel addresses of devices and apps, as opposed to codes'
    /// temporary keys. Anything else on the tunnel may only redeem a code.
    trusted: Arc<watch::Sender<HashSet<Ipv4Addr>>>,
}

/// What runs once the server serves its routes.
struct Running {
    /// The name, with its DNS provider. A server without one, for
    /// development and tests, runs only the tunnel, and devices find it at
    /// its home address.
    name: Option<(ServerName, ProviderKind)>,
    /// Where devices find home, such as `k3x9q2wm7f.ourhome.dedyn.io`.
    endpoint: String,
    home: watch::Receiver<Home>,
    tunnel: watch::Receiver<TunnelStatus>,
    router: watch::Receiver<PortStatus>,
    record: watch::Receiver<EndpointStatus>,
    /// The router's address, for the Network page.
    router_address: Option<IpAddr>,
}

/// The server's address on the home network, or why it's not known.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Home {
    Address(Ipv4Addr),
    Unknown(String),
}

impl Home {
    fn find(source: Option<AddressSource>) -> Self {
        // Without a setting, detect it, as for a server whose name points
        // elsewhere. In a container, that finds the container's address,
        // so the setting is needed there.
        let source = source.unwrap_or(AddressSource::Detect);
        match source.address() {
            Ok(IpAddr::V4(address)) => Self::Address(address),
            Ok(IpAddr::V6(_)) => Self::Unknown(
                "Remote access needs the server's IPv4 address on the home network. Set \
                 ACCORDION_ADDRESS to it, then restart Accordion."
                    .into(),
            ),
            Err(err) => Self::Unknown(format!(
                "Accordion couldn't find the server's address on the home network ({err}). \
                 Set ACCORDION_ADDRESS to it, then restart Accordion."
            )),
        }
    }

    fn address(&self) -> Option<Ipv4Addr> {
        match self {
            Self::Address(address) => Some(*address),
            Self::Unknown(_) => None,
        }
    }
}

/// How the server is reached, for [`RemoteAccess::start`].
pub enum Site {
    /// By its name, whose DNS provider holds the record devices find home
    /// at.
    Named {
        name: ServerName,
        dns: Box<Provider>,
    },
    /// By its address, over plain HTTP, for development and tests. The
    /// router isn't asked to forward anything.
    Unnamed,
}

impl RemoteAccess {
    pub async fn open(db: Database, identity: Identity) -> Result<Self, db::Error> {
        db.migrate("remote_access", MIGRATIONS).await?;
        let saved = db
            .read(|conn| {
                conn.query_row(
                    "SELECT turned_on, port, network, private_key, label FROM remote_access",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, bool>(0)?,
                            row.get::<_, u16>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
                .optional()
            })
            .await?;
        let saved = saved.and_then(|(on, port, network, key, label)| {
            let network = Network::containing(network.parse().ok()?)?;
            let key = Key::from_base64(&key).ok()?;
            Some(Saved {
                on,
                port,
                network,
                key,
                label,
            })
        });
        Ok(Self {
            db,
            identity,
            saved: Arc::new(watch::channel(saved).0),
            running: Arc::default(),
            trusted: Arc::new(watch::channel(HashSet::new()).0),
        })
    }

    /// Turns remote access on or off, and sets its port. The first time,
    /// it picks the tunnel's addresses, the server's key, and the record's
    /// label.
    async fn set(&self, on: bool, port: u16) -> Result<(), db::Error> {
        let current = self.saved.borrow().clone();
        let saved = match current {
            Some(saved) => Saved { on, port, ..saved },
            None => Saved {
                on,
                port,
                network: Network::random(),
                key: Key::generate(),
                label: random_label(),
            },
        };
        let row = saved.clone();
        self.db
            .write(move |tx| {
                tx.execute(
                    "INSERT INTO remote_access (id, turned_on, port, network, private_key, label)
                     VALUES (1, ?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT (id) DO UPDATE SET turned_on = excluded.turned_on,
                         port = excluded.port",
                    (
                        row.on,
                        row.port,
                        row.network.server().to_string(),
                        row.key.to_base64(),
                        row.label,
                    ),
                )
            })
            .await?;
        self.saved.send_replace(Some(saved));
        Ok(())
    }

    fn on(&self) -> bool {
        self.saved.borrow().as_ref().is_some_and(|saved| saved.on)
    }

    /// Starts remote access's work. The tunnel hands connections for each
    /// of `ports` to the receiver at the same place in the list it returns,
    /// for the gateway's servers. The work runs until each future from
    /// `stopped` completes.
    pub fn start<F, S>(
        &self,
        site: Site,
        home: Option<AddressSource>,
        ports: &[u16],
        stopped: F,
    ) -> (
        Vec<mpsc::Receiver<Accepted>>,
        impl Future<Output = ()> + use<F, S>,
    )
    where
        F: Fn() -> S,
        S: Future<Output = ()>,
    {
        let (listeners, connections): (Vec<_>, Vec<_>) = ports
            .iter()
            .map(|port| {
                let (sender, receiver) = mpsc::channel(WAITING_CONNECTIONS);
                ((*port, sender), receiver)
            })
            .unzip();
        let (config, config_receiver) = watch::channel(None);
        let (peers, peers_receiver) = watch::channel(Vec::new());
        let (port, port_receiver) = watch::channel(None);
        let (endpoint_input, endpoint_receiver) = watch::channel(None);
        let home_now = Home::find(home);
        let (home_sender, home_receiver) = watch::channel(home_now.clone());

        let tunnel = RemoteTunnel::new(config_receiver, peers_receiver, Listeners(listeners));
        let label = self
            .saved
            .borrow()
            .as_ref()
            .map_or_else(random_label, |saved| saved.label.clone());
        let (name, endpoint, router, mapper, publisher) = match site {
            Site::Named { name, dns } => {
                let router = HomeRouter::find();
                let mapper = PortMapper::new(router, Protocol::Udp, port_receiver);
                let provider = dns.kind();
                let publisher = EndpointPublisher::new(*dns, endpoint_receiver);
                (
                    Some((name.clone(), provider)),
                    Some(format!("{label}.{name}")),
                    router,
                    Some(mapper),
                    Some(publisher),
                )
            }
            Site::Unnamed => (None, None, None, None, None),
        };
        let running = Running {
            name,
            endpoint: endpoint.clone().unwrap_or_else(|| {
                home_now
                    .address()
                    .map_or_else(|| "localhost".to_owned(), |home| home.to_string())
            }),
            home: home_receiver,
            tunnel: tunnel.status(),
            router: mapper.as_ref().map_or_else(
                || watch::channel(PortStatus::default()).1,
                PortMapper::status,
            ),
            record: publisher.as_ref().map_or_else(
                || watch::channel(EndpointStatus::default()).1,
                EndpointPublisher::status,
            ),
            router_address: router.map(|router| router.pcp.ip()),
        };
        let drive = Drive {
            access: self.clone(),
            home: home_sender,
            home_source: home,
            config,
            peers,
            port,
            endpoint: endpoint_input,
            tunnel: running.tunnel.clone(),
            router: running.router.clone(),
            endpoint_name: endpoint,
        };
        let _ = self.running.set(running);
        let work = async move {
            let mapper = async {
                if let Some(mapper) = mapper {
                    mapper.run(stopped()).await;
                }
            };
            let publisher = async {
                if let Some(publisher) = publisher {
                    publisher.run(stopped()).await;
                }
            };
            tokio::join!(
                tunnel.run(stopped()),
                mapper,
                publisher,
                drive.run(stopped()),
            );
        };
        (connections, work)
    }

    /// How remote access is doing, for the health page. `None` on a server
    /// without a name.
    pub fn health(&self) -> Option<RemoteHealth> {
        let running = self.running.get()?;
        let saved = self.saved.borrow().clone();
        Some(RemoteHealth {
            on: saved.as_ref().is_some_and(|saved| saved.on),
            named: running.name.is_some(),
            port: saved.map(|saved| saved.port),
            home: running.home.borrow().clone().address(),
            home_problem: match &*running.home.borrow() {
                Home::Unknown(problem) => Some(problem.clone()),
                Home::Address(_) => None,
            },
            tunnel: running.tunnel.borrow().clone(),
            router: running.router.borrow().clone(),
            record: running.record.borrow().clone(),
        })
    }

    /// Remote access's connections, for the Network page. `None` while it's
    /// off.
    pub fn network(&self) -> Option<RemoteNetwork> {
        let running = self.running.get()?;
        let (_, provider) = running.name.as_ref()?;
        if !self.on() {
            return None;
        }
        let router = running.router.borrow();
        let record = running.record.borrow();
        Some(RemoteNetwork {
            router: running.router_address.map(|address| address.to_string()),
            router_last: router.last_attempt.map(|at| crate::connections::LastView {
                at: crate::connections::millis(at),
                ok: router.mapping.is_some(),
                problem: router.problem.clone(),
            }),
            record_host: provider.api_host(),
            lookup_host: provider.public_address_host(),
            uses_lookup: router
                .mapping
                .is_none_or(|mapping| mapping.external_address.is_none()),
            record_last: record.last_attempt.map(|at| crate::connections::LastView {
                at: crate::connections::millis(at),
                ok: record.problem.is_none(),
                problem: record.problem.clone(),
            }),
        })
    }

    /// The port devices connect to: the router's choice when it forwards
    /// the port, or the tunnel's own.
    fn endpoint_port(&self, running: &Running) -> Option<u16> {
        let port = self.saved.borrow().as_ref()?.port;
        Some(
            running
                .router
                .borrow()
                .mapping
                .map_or(port, |mapping| mapping.external_port),
        )
    }

    /// How an app reaches the server through the tunnel, while remote
    /// access is on and the server knows its home address.
    pub(crate) fn reachable(&self) -> Result<Reachable, ApiError> {
        let unavailable = |message: &str| ApiError::new(StatusCode::CONFLICT, message);
        let off = "Remote access is off. Ask your admin to turn it on.";
        let (Some(running), Some(saved)) = (self.running.get(), self.saved.borrow().clone()) else {
            return Err(unavailable(off));
        };
        if !saved.on {
            return Err(unavailable(off));
        }
        let Some(home) = running.home.borrow().address() else {
            return Err(unavailable(
                "Accordion doesn't know the server's address on the home network, so it can't \
                 set up a device. Ask your admin to check Server health.",
            ));
        };
        Ok(Reachable {
            port: self.endpoint_port(running).unwrap_or(saved.port),
            network: saved.network,
            server: saved.key.public(),
            endpoint_host: running.endpoint.clone(),
            home,
        })
    }

    /// Registers the tunnel key of the app signed in as `session`, and says
    /// how it reaches the server.
    pub(crate) async fn set_app_tunnel(
        &self,
        identity: &Identity,
        session: &Session,
        public_key: PublicKey,
    ) -> Result<AppTunnelView, ApiError> {
        let reach = self.reachable()?;
        let device = identity
            .set_app_tunnel(
                session,
                *public_key.as_bytes(),
                &reach.home.to_string(),
                reach.port,
            )
            .await?;
        Ok(AppTunnelView {
            server_key: reach.server.to_base64(),
            endpoint: reach.endpoint(),
            address: reach.network.host(device.host).to_string(),
            gateway: reach.network.server().to_string(),
        })
    }

    /// Whether `address` is on the tunnel without being a device or an
    /// app, so it's a code's temporary key.
    fn temporary(&self, address: IpAddr) -> bool {
        let IpAddr::V4(address) = address.to_canonical() else {
            return false;
        };
        let Some(network) = self.saved.borrow().as_ref().map(|saved| saved.network) else {
            return false;
        };
        Network::containing(address) == Some(network)
            && address != network.server()
            && !self.trusted.borrow().contains(&address)
    }
}

/// How an app reaches the server through the tunnel.
pub(crate) struct Reachable {
    pub port: u16,
    pub network: Network,
    pub server: PublicKey,
    /// Where apps find home, such as `k3x9q2wm7f.ourhome.dedyn.io`.
    pub endpoint_host: String,
    pub home: Ipv4Addr,
}

impl Reachable {
    /// The endpoint with its port, such as `k3x9q2wm7f.ourhome.dedyn.io:51820`.
    pub fn endpoint(&self) -> String {
        if self.endpoint_host.contains(':') {
            format!("[{}]:{}", self.endpoint_host, self.port)
        } else {
            format!("{}:{}", self.endpoint_host, self.port)
        }
    }

    /// What a code carries for an app away from home: everything its
    /// temporary tunnel needs, and the code. Apps read it from the QR code,
    /// or from the text copied into them.
    pub fn code_setup(&self, origin: &str, code: &str, key: &Key, host: u8) -> String {
        format!(
            "accordion:add?server={origin}&code={code}&key={key}&peer={peer}&endpoint={endpoint}\
             &address={address}&gateway={gateway}",
            origin = encode(origin),
            key = encode(&key.to_base64()),
            peer = encode(&self.server.to_base64()),
            endpoint = encode(&self.endpoint()),
            address = self.network.host(host),
            gateway = self.network.server(),
        )
    }
}

/// Percent-encodes what isn't a letter, digit, or one of `-._~:/`.
fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~:/".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// What an app needs to reach the server through its own tunnel.
#[derive(Serialize)]
pub(crate) struct AppTunnelView {
    /// The server's WireGuard public key, in base64.
    server_key: String,
    /// Where the app finds home, such as `k3x9q2wm7f.ourhome.dedyn.io:51820`.
    endpoint: String,
    /// The app's own address in the tunnel.
    address: String,
    /// The server's address in the tunnel, which the app connects to.
    gateway: String,
}

/// Middleware that lets a code's temporary key reach only the address that
/// redeems codes. Outside every other check, so nothing else sees it.
pub async fn limit_temporary_keys(
    State(remote): State<Option<RemoteAccess>>,
    request: Request,
    next: Next,
) -> Response {
    let temporary = remote.as_ref().is_some_and(|remote| {
        crate::network::peer(request.extensions())
            .is_some_and(|peer| remote.temporary(peer.remote.ip()))
    });
    if temporary && !(request.method() == Method::POST && request.uri().path() == REDEEM_PATH) {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "This connection can only add an Accordion app. Finish adding the app, then try \
             again.",
        )
        .into_response();
    }
    next.run(request).await
}

/// How remote access is doing, as of now.
pub struct RemoteHealth {
    pub on: bool,
    /// The server has a name, so Accordion asks the router for the port.
    pub named: bool,
    pub port: Option<u16>,
    pub home: Option<Ipv4Addr>,
    pub home_problem: Option<String>,
    pub tunnel: TunnelStatus,
    pub router: PortStatus,
    pub record: EndpointStatus,
}

/// Remote access's connections, for the Network page.
#[derive(Serialize)]
pub struct RemoteNetwork {
    /// The router's address, when the server found it.
    router: Option<String>,
    router_last: Option<crate::connections::LastView>,
    /// The DNS provider's API, for the address record.
    record_host: &'static str,
    /// Where the public address comes from when the router doesn't say.
    lookup_host: &'static str,
    uses_lookup: bool,
    record_last: Option<crate::connections::LastView>,
}

/// Ten random lowercase letters and digits. Nobody can guess the record's
/// name from the server's.
fn random_label() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = [0u8; 10];
    getrandom::fill(&mut bytes).expect("the operating system's random number generator works");
    bytes
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(*byte) % ALPHABET.len()]))
        .collect()
}

/// Keeps the tunnel's config, devices, port, and record in step with the
/// settings, the home address, and the device list.
struct Drive {
    access: RemoteAccess,
    home: watch::Sender<Home>,
    home_source: Option<AddressSource>,
    config: watch::Sender<Option<TunnelConfig>>,
    peers: watch::Sender<Vec<DevicePeer>>,
    port: watch::Sender<Option<Forward>>,
    endpoint: watch::Sender<Option<EndpointInput>>,
    tunnel: watch::Receiver<TunnelStatus>,
    router: watch::Receiver<PortStatus>,
    /// The record, on a server with a name.
    endpoint_name: Option<String>,
}

impl Drive {
    async fn run(mut self, stopped: impl Future<Output = ()>) {
        let mut stopped = std::pin::pin!(stopped);
        let mut saved = self.access.saved.subscribe();
        let mut devices = self.access.identity.remote_devices_changed();
        let mut home_check = tokio::time::interval(HOME_CHECK);
        home_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut recorded: HashMap<PublicKey, SystemTime> = HashMap::new();
        let mut load_devices = true;
        let mut peers_check = tokio::time::interval(PEERS_CHECK);
        peers_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let settings = saved.borrow_and_update().clone();
            let on = settings.as_ref().filter(|saved| saved.on);
            let home = self.home.borrow().address();
            self.config.send_if_modified(|config| {
                let wanted = on.zip(home).map(|(saved, home)| TunnelConfig {
                    key: saved.key.clone(),
                    port: saved.port,
                    network: saved.network,
                    home,
                });
                let changed = *config != wanted;
                *config = wanted;
                changed
            });
            if load_devices {
                load_devices = false;
                devices.mark_unchanged();
                match self.access.identity.tunnel_peers().await {
                    Ok(list) => {
                        let network = settings.as_ref().map(|saved| saved.network);
                        let wanted: Vec<DevicePeer> = network
                            .map(|network| {
                                list.iter()
                                    .map(|peer| DevicePeer {
                                        public_key: PublicKey::from_bytes(peer.public_key),
                                        address: network.host(peer.host),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        // Before the tunnel lets anyone new in, so a new
                        // device is trusted by the time it connects, and an
                        // address a code takes over isn't.
                        let trusted: HashSet<Ipv4Addr> = network
                            .map(|network| {
                                list.iter()
                                    .filter(|peer| !peer.temporary)
                                    .map(|peer| network.host(peer.host))
                                    .collect()
                            })
                            .unwrap_or_default();
                        self.access.trusted.send_if_modified(|current| {
                            let changed = *current != trusted;
                            *current = trusted;
                            changed
                        });
                        self.peers.send_if_modified(|peers| {
                            let changed = *peers != wanted;
                            *peers = wanted;
                            changed
                        });
                    }
                    Err(err) => tracing::error!(%err, "couldn't load the remote access devices"),
                }
            }
            let port = on.map(|saved| Forward::same(saved.port));
            self.port.send_if_modified(|current| {
                let changed = *current != port;
                *current = port;
                changed
            });
            let router_address = self
                .router
                .borrow_and_update()
                .mapping
                .and_then(|mapping| mapping.external_address);
            let input = on
                .and(self.endpoint_name.clone())
                .map(|name| EndpointInput {
                    name,
                    router_address,
                });
            self.endpoint.send_if_modified(|current| {
                let changed = *current != input;
                *current = input;
                changed
            });

            tokio::select! {
                () = &mut stopped => return,
                Ok(()) = saved.changed() => {
                    // A first turn-on picks the tunnel's addresses, which
                    // the devices' addresses depend on.
                    load_devices = true;
                }
                Ok(()) = devices.changed() => load_devices = true,
                _ = peers_check.tick() => load_devices = true,
                Ok(()) = self.router.changed() => {}
                _ = home_check.tick() => {
                    let home = Home::find(self.home_source);
                    self.home.send_if_modified(|current| {
                        let changed = *current != home;
                        if changed {
                            tracing::info!(?home, "the server's home address changed");
                        }
                        *current = home;
                        changed
                    });
                }
                Ok(()) = self.tunnel.changed() => {
                    let connected: Vec<([u8; 32], SystemTime)> = self
                        .tunnel
                        .borrow_and_update()
                        .connected
                        .iter()
                        .filter(|(key, at)| {
                            recorded.get(key).is_none_or(|before| {
                                at.duration_since(*before).unwrap_or_default() >= CONNECTED_PRECISION
                            })
                        })
                        .map(|(key, at)| (*key.as_bytes(), *at))
                        .collect();
                    for (key, at) in &connected {
                        recorded.insert(PublicKey::from_bytes(*key), *at);
                    }
                    if let Err(err) = self.access.identity.record_remote_connections(connected).await {
                        tracing::warn!(%err, "couldn't save when devices connected");
                    }
                }
            }
        }
    }
}

pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/api/remote-access", get(status).put(set))
        .route("/api/remote-access/devices/{id}", delete(remove_any))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/remote-devices", get(my_devices).post(add_device))
        .route("/api/remote-devices/{id}", delete(remove_mine))
        .route("/api/app-tunnel", post(register_app_tunnel))
}

fn require_admin(user: &SignedIn) -> Result<(), ApiError> {
    if matches!(user.user.role, Role::Owner | Role::Admin) {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::FORBIDDEN,
        "Only admins can change remote access. Ask your admin.",
    ))
}

fn remote(state: &AppState) -> Result<&RemoteAccess, ApiError> {
    state.remote.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Remote access isn't available on this server.",
        )
    })
}

fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
    })
}

#[derive(Serialize)]
struct StatusView {
    /// Whether remote access is running on this server.
    available: bool,
    /// The server has a name, so the router is asked for the port and the
    /// record is kept. Without one, the tunnel works only for testing.
    named: bool,
    on: bool,
    port: u16,
    /// The tunnel's addresses, once it's been turned on.
    network: Option<String>,
    /// Where devices find home.
    endpoint: Option<String>,
    /// The server's address on the home network, which the tunnel carries.
    home_address: Option<String>,
    home_problem: Option<String>,
    /// The tunnel is listening on its port.
    listening: bool,
    /// Why the tunnel isn't running, while it's on.
    tunnel_problem: Option<String>,
    router: Option<RouterView>,
    record: Option<RecordView>,
    devices: Vec<AdminDeviceView>,
}

#[derive(Serialize)]
struct RouterView {
    /// "PCP", "NAT-PMP", or "UPnP", when the router forwards the port.
    method: Option<String>,
    external_port: Option<u16>,
    external_address: Option<String>,
    /// The router's public address is shared with other homes, or private.
    shared_address: bool,
    problem: Option<String>,
    no_answer: bool,
    last_attempt: Option<u64>,
}

#[derive(Serialize)]
struct RecordView {
    ipv4: Option<String>,
    ipv4_set_by_provider: bool,
    ipv6: Option<String>,
    problem: Option<String>,
}

#[derive(Serialize)]
struct DeviceView {
    id: RemoteDeviceId,
    name: String,
    created_at: String,
    last_connected_at: Option<String>,
    /// The server's home address or port changed since the device was set
    /// up, so it needs a new setup.
    out_of_date: bool,
}

#[derive(Serialize)]
struct AdminDeviceView {
    #[serde(flatten)]
    device: DeviceView,
    owner: String,
}

/// What a device's setup has to match to still work.
struct Current {
    home: Option<Ipv4Addr>,
    port: Option<u16>,
}

impl Current {
    fn of(remote: &RemoteAccess) -> Self {
        match remote.running.get() {
            Some(running) => Self {
                home: running.home.borrow().address(),
                port: remote.endpoint_port(running),
            },
            None => Self {
                home: None,
                port: None,
            },
        }
    }

    fn view(&self, device: RemoteDevice) -> DeviceView {
        let out_of_date = self
            .home
            .is_some_and(|home| device.home_address != home.to_string())
            || self.port.is_some_and(|port| device.port != port);
        DeviceView {
            id: device.id,
            name: device.name,
            created_at: device.created_at,
            last_connected_at: device.last_connected_at,
            out_of_date,
        }
    }
}

/// `GET /api/remote-access`: the settings, how each part is doing, and
/// everyone's devices.
async fn status(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Json<StatusView>, ApiError> {
    require_admin(&me)?;
    let remote = remote(&state)?;
    let saved = remote.saved.borrow().clone();
    let current = Current::of(remote);
    let devices = state
        .identity
        .all_remote_devices()
        .await?
        .into_iter()
        .map(|(device, owner)| AdminDeviceView {
            device: current.view(device),
            owner: owner.display_name,
        })
        .collect();
    let on = saved.as_ref().is_some_and(|saved| saved.on);
    let running = remote.running.get();
    Ok(Json(StatusView {
        available: running.is_some(),
        named: running.is_some_and(|running| running.name.is_some()),
        on,
        port: saved.as_ref().map_or(DEFAULT_PORT, |saved| saved.port),
        network: saved.as_ref().map(|saved| saved.network.to_string()),
        endpoint: running.map(|running| running.endpoint.clone()),
        home_address: current.home.map(|home| home.to_string()),
        home_problem: running.and_then(|running| match &*running.home.borrow() {
            Home::Unknown(problem) => Some(problem.clone()),
            Home::Address(_) => None,
        }),
        listening: running.is_some_and(|running| running.tunnel.borrow().listening.is_some()),
        tunnel_problem: running
            .filter(|_| on)
            .and_then(|running| running.tunnel.borrow().problem.clone()),
        router: running
            .filter(|running| on && running.name.is_some())
            .map(|running| {
                let router = running.router.borrow();
                RouterView {
                    method: router.mapping.map(|mapping| mapping.method.to_string()),
                    external_port: router.mapping.map(|mapping| mapping.external_port),
                    external_address: router
                        .mapping
                        .and_then(|mapping| mapping.external_address)
                        .map(|address| address.to_string()),
                    shared_address: router.shared_address,
                    problem: router.problem.clone(),
                    no_answer: router.no_answer,
                    last_attempt: router.last_attempt.map(millis),
                }
            }),
        record: running
            .filter(|running| on && running.name.is_some())
            .map(|running| {
                let record = running.record.borrow();
                RecordView {
                    ipv4: record.ipv4.map(|address| address.to_string()),
                    ipv4_set_by_provider: record.ipv4_set_by_provider,
                    ipv6: record.ipv6.map(|address| address.to_string()),
                    problem: record.problem.clone(),
                }
            }),
        devices,
    }))
}

#[derive(Deserialize)]
struct Settings {
    on: bool,
    #[serde(default = "default_port")]
    port: u16,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

/// `PUT /api/remote-access`: turns remote access on or off, and sets its
/// UDP port.
async fn set(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Json(settings): Json<Settings>,
) -> Result<StatusCode, ApiError> {
    require_admin(&me)?;
    let remote = remote(&state)?;
    if remote.running.get().is_none() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Remote access isn't running on this server yet. Try again in a moment.",
        ));
    }
    if settings.port == 0 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Choose a port from 1 to 65535.",
        ));
    }
    let before = remote.saved.borrow().clone();
    remote
        .set(settings.on, settings.port)
        .await
        .map_err(|err| {
            tracing::error!(%err, "couldn't save the remote access settings");
            ApiError::server_problem()
        })?;
    let changed =
        before.is_none_or(|before| before.on != settings.on || before.port != settings.port);
    if changed {
        let details = if settings.on {
            serde_json::json!({ "on": true, "port": settings.port })
        } else {
            serde_json::json!({ "on": false })
        };
        log::record(
            &state,
            &me,
            NewEvent::new(Action::RemoteAccessChanged).details(details),
        )
        .await;
    }
    tracing::info!(
        user = %me.user.id,
        port = settings.port,
        "turned remote access {}",
        if settings.on { "on" } else { "off" }
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/remote-access/devices/{id}`: removes anyone's device.
async fn remove_any(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Path(id): Path<RemoteDeviceId>,
) -> Result<StatusCode, ApiError> {
    require_admin(&me)?;
    let owner = state
        .identity
        .all_remote_devices()
        .await?
        .into_iter()
        .find(|(device, _)| device.id == id)
        .map(|(device, _)| device.user_id)
        .ok_or_else(ApiError::not_found)?;
    state
        .identity
        .acting_as(actor(&me))
        .remove_remote_device(owner, id)
        .await?;
    tracing::info!(user = %me.user.id, device = %id, "removed a remote access device");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct MyDevicesView {
    /// Whether the admin turned remote access on.
    on: bool,
    max: usize,
    devices: Vec<DeviceView>,
}

/// `GET /api/remote-devices`: the signed-in user's devices.
async fn my_devices(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Json<MyDevicesView>, ApiError> {
    let remote = remote(&state)?;
    let current = Current::of(remote);
    let devices = state.identity.remote_devices(me.user.id).await?;
    Ok(Json(MyDevicesView {
        on: remote.on() && remote.running.get().is_some(),
        max: MAX_REMOTE_DEVICES,
        devices: devices
            .into_iter()
            .map(|device| current.view(device))
            .collect(),
    }))
}

#[derive(Deserialize)]
struct NewDevice {
    name: String,
}

#[derive(Serialize)]
struct AddedDevice {
    device: DeviceView,
    /// The device's whole setup, for the WireGuard apps. It holds the
    /// device's private key, so it's shown only now.
    config: String,
    /// The setup as a QR code, in SVG.
    qr_code: String,
    /// A file name for the setup, which the WireGuard apps show as the
    /// tunnel's name.
    file_name: String,
}

/// `POST /api/remote-devices`: adds a device for the signed-in user, and
/// returns its setup once. Like an app password, it needs a sign-in from
/// the last 15 minutes.
async fn add_device(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Json(request): Json<NewDevice>,
) -> Result<(StatusCode, Json<AddedDevice>), ApiError> {
    let remote = remote(&state)?;
    let reach = remote.reachable()?;
    let (Some(running), Some(saved)) = (remote.running.get(), remote.saved.borrow().clone()) else {
        return Err(ApiError::server_problem());
    };
    let home = reach.home;
    if !me.recently_authenticated {
        return Err(ApiError::password_needed(
            "For your security, enter your password again before you add a device.",
        ));
    }
    let port = reach.port;
    let key = Key::generate();
    let added = state
        .identity
        .acting_as(actor(&me))
        .add_remote_device(
            me.user.id,
            NewRemoteDevice {
                name: &request.name,
                public_key: *key.public().as_bytes(),
                home_address: &home.to_string(),
                port,
            },
        )
        .await?;
    let config = DeviceSetup {
        key: &key,
        address: saved.network.host(added.host),
        server: saved.key.public(),
        network: saved.network,
        home,
        endpoint_host: &running.endpoint,
        endpoint_port: port,
    }
    .to_config();
    let qr_code = qr_svg(&config).map_err(|err| {
        tracing::error!(%err, "couldn't make a device's QR code");
        ApiError::server_problem()
    })?;
    tracing::info!(user = %me.user.id, device = %added.id, "added a remote access device");
    let file_name = file_name(running.name.as_ref().map(|(name, _)| name));
    Ok((
        StatusCode::CREATED,
        Json(AddedDevice {
            device: Current {
                home: Some(home),
                port: Some(port),
            }
            .view(added),
            config,
            qr_code,
            file_name,
        }),
    ))
}

/// `DELETE /api/remote-devices/{id}`: removes one of the signed-in user's
/// devices, which cuts it off at once.
async fn remove_mine(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Path(id): Path<RemoteDeviceId>,
) -> Result<StatusCode, ApiError> {
    if !state
        .identity
        .acting_as(actor(&me))
        .remove_remote_device(me.user.id, id)
        .await?
    {
        return Err(ApiError::not_found());
    }
    tracing::info!(user = %me.user.id, device = %id, "removed a remote access device");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct NewAppTunnel {
    /// The app's WireGuard public key, in base64. The app made the key
    /// pair, and its private key never leaves the device.
    public_key: String,
}

/// `POST /api/app-tunnel`: registers the tunnel key of the Accordion app
/// that's signed in, replacing any key it registered before, and says how
/// it reaches the server. Apps call it whenever they start, so their setup
/// follows the server's address and port.
async fn register_app_tunnel(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Json(request): Json<NewAppTunnel>,
) -> Result<Json<AppTunnelView>, ApiError> {
    if me.session.client != accordion_core::identity::Client::App {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Only Accordion's apps can register a tunnel key. To reach Accordion from a \
             browser away from home, add a device under Apps and devices.",
        ));
    }
    let key = PublicKey::from_base64(&request.public_key)
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err.to_string()))?;
    let tunnel = remote(&state)?
        .set_app_tunnel(&state.identity, &me.session, key)
        .await?;
    tracing::info!(user = %me.user.id, session = %me.session.id, "registered an app's tunnel key");
    Ok(Json(tunnel))
}

/// A QR code of `text`, in SVG, black on white so every camera reads it in
/// either theme.
pub(crate) fn qr_svg(text: &str) -> Result<String, qrcode::types::QrError> {
    use qrcode::render::svg;
    let code = qrcode::QrCode::with_error_correction_level(text, qrcode::EcLevel::L)?;
    Ok(code
        .render::<svg::Color<'_>>()
        .min_dimensions(256, 256)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build())
}

/// The setup's file name, which the WireGuard apps use as the tunnel's
/// name: the server's first label, such as "ourhome", since names can't
/// be long or have spaces.
fn file_name(name: Option<&ServerName>) -> String {
    let first = name
        .and_then(|name| name.as_str().split('.').next())
        .unwrap_or("accordion");
    let short: String = first
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(15)
        .collect();
    format!(
        "{}.conf",
        if short.is_empty() {
            "accordion"
        } else {
            &short
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_labels_and_file_names() {
        let label = random_label();
        assert_eq!(label.len(), 10);
        assert!(
            label
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
        assert_ne!(random_label(), label);
        let name: ServerName = "ourhome.dedyn.io".parse().unwrap();
        assert_eq!(file_name(Some(&name)), "ourhome.conf");
        assert_eq!(file_name(None), "accordion.conf");
    }

    #[test]
    fn draws_a_qr_code() {
        let svg = qr_svg("[Interface]\nPrivateKey = x\n").unwrap();
        assert!(svg.starts_with("<?xml"));
        assert!(svg.contains("<svg"));
    }
}
