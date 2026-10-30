//! Where requests come from, and keeping setup and admin pages on the home
//! network.
//!
//! The spec keeps admin pages off the internet by default. People reach them
//! at home, or later over Accordion's VPN, whose addresses are private too.
//!
//! Behind a reverse proxy, every request would seem to come from the proxy,
//! so these checks and the sign-in limits need the proxy's forwarded address
//! first. Accordion doesn't read it yet.

use accordion_core::identity::SignInFrom;
use accordion_gateway::Peer;
use axum::extract::connect_info::MockConnectInfo;
use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{Extensions, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use accordion_module::ApiError;

/// Who can open the setup and admin pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AdminAccess {
    /// Only devices on the home network or VPN.
    #[default]
    HomeNetwork,
    /// Any device, for a server with no home network, such as a rented one.
    Anywhere,
}

/// The connection a request came in on. The gateway's TLS server adds it,
/// and so does `into_make_service_with_connect_info::<Peer>()` for plain
/// HTTP. Tests stand in for it with `MockConnectInfo`.
pub(crate) fn peer(extensions: &Extensions) -> Option<Peer> {
    extensions
        .get::<ConnectInfo<Peer>>()
        .map(|ConnectInfo(peer)| *peer)
        .or_else(|| {
            extensions
                .get::<MockConnectInfo<Peer>>()
                .map(|MockConnectInfo(peer)| *peer)
        })
}

/// A request's connection, as a handler argument.
pub struct Connection(pub Peer);

impl Connection {
    pub fn sign_in_from(&self) -> SignInFrom {
        SignInFrom {
            address: self.0.remote.ip(),
            home_network: self.0.on_home_network(),
        }
    }
}

impl<S: Sync> FromRequestParts<S> for Connection {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, ApiError> {
        peer(&parts.extensions).map(Self).ok_or_else(|| {
            tracing::error!("a request arrived without its connection details");
            ApiError::server_problem()
        })
    }
}

impl AdminAccess {
    /// Whether the setup and admin pages open on `peer`'s connection.
    pub(crate) fn allows(self, peer: Option<Peer>) -> bool {
        self == Self::Anywhere || peer.is_some_and(|peer| peer.on_home_network())
    }
}

/// Middleware for the setup and admin pages.
pub async fn only_home_network(
    State(access): State<AdminAccess>,
    request: Request,
    next: Next,
) -> Response {
    if access.allows(peer(request.extensions())) {
        return next.run(request).await;
    }
    let message = "Accordion's admin pages only open on the same network as the server. \
                   Connect to your home network, then try again.";
    if request.uri().path().starts_with("/api/") {
        ApiError::new(StatusCode::FORBIDDEN, message).into_response()
    } else {
        (StatusCode::FORBIDDEN, message).into_response()
    }
}
