//! A log line for each request, for finding out what a client such as File
//! Explorer asks for and what it gets back. It's off unless `RUST_LOG` turns
//! it on, such as `RUST_LOG=info,accordion_server::requests=debug`, because
//! paths can hold the names of people's files.

use axum::extract::Request;
use axum::http::HeaderMap;
use axum::http::header::{AUTHORIZATION, USER_AGENT};
use axum::middleware::Next;
use axum::response::Response;

pub async fn log(request: Request, next: Next) -> Response {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return next.run(request).await;
    }
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let headers = request.headers();
    // Only the scheme, such as Basic, never the credentials.
    let auth = header(headers, AUTHORIZATION.as_str())
        .and_then(|value| value.split_whitespace().next())
        .unwrap_or("none")
        .to_owned();
    let depth = header(headers, "depth").unwrap_or("").to_owned();
    let client = header(headers, USER_AGENT.as_str())
        .unwrap_or("")
        .to_owned();
    let response = next.run(request).await;
    tracing::debug!(
        %method,
        path,
        status = response.status().as_u16(),
        auth,
        depth,
        client,
        "request"
    );
    response
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}
