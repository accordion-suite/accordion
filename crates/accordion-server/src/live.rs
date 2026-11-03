//! Live updates (decision 19): `GET /api/live`, a stream of server-sent
//! events that tells the web app and Accordion's apps what to load again.
//!
//! Core decides who hears what. This turns each message into an event named
//! after its kind, with the IDs it names as `{"ids": [...]}`.

use std::convert::Infallible;
use std::time::Duration;

use accordion_core::live::{Live, LiveStream};
use accordion_module::{ApiError, Authenticated};
use axum::Router;
use axum::extract::State;
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::stream::{self, Stream, StreamExt};

use crate::AppState;

/// How often a stream says it's still there. Clients that hear nothing for
/// 75 seconds reconnect, so this also finds streams that stopped.
pub const PING_EVERY: Duration = Duration::from_secs(30);

/// Asks reverse proxies such as nginx to pass each event on at once.
const NO_BUFFERING: HeaderName = HeaderName::from_static("x-accel-buffering");

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/live", get(open))
}

async fn open(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Response, ApiError> {
    let live: &Live = state.live.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Live updates aren't available on this server.",
        )
    })?;
    let stream = live.open(me.user.id, Some(me.session.id));
    Ok((
        [
            (CACHE_CONTROL, HeaderValue::from_static("no-cache")),
            (NO_BUFFERING, HeaderValue::from_static("no")),
        ],
        Sse::new(events(stream)).keep_alive(
            KeepAlive::new()
                .interval(PING_EVERY)
                // An event rather than a comment, since browsers don't show
                // comments to the page.
                .event(Event::default().event("ping").data("{}")),
        ),
    )
        .into_response())
}

/// The stream's events, starting with how long to wait before reconnecting.
fn events(stream: LiveStream) -> impl Stream<Item = Result<Event, Infallible>> {
    let first = Event::default().retry(reconnect_after());
    stream::once(async move { Ok(first) }).chain(stream::unfold(stream, |mut stream| async move {
        let message = stream.next().await?;
        let data = serde_json::json!({ "ids": message.ids }).to_string();
        let event = Event::default().event(message.kind).data(data);
        Some((Ok(event), stream))
    }))
}

/// Between one and five seconds, chosen at random, so clients don't all
/// return at the same moment after an update restarts the server.
fn reconnect_after() -> Duration {
    let random = getrandom::u32().unwrap_or(0);
    Duration::from_millis(1000 + u64::from(random % 4000))
}
