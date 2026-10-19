//! Blocks cross-site requests that would change something.
//!
//! Browsers attach the session cookie to requests that other sites trigger,
//! such as a hidden form that posts to Accordion. This middleware rejects
//! those before any handler runs. It follows the same rules as Go's
//! `http.CrossOriginProtection`:
//!
//! 1. Requests that only read (GET, HEAD, and OPTIONS) are allowed.
//! 2. If the browser sent `Sec-Fetch-Site`, only `same-origin` and `none`
//!    (typed into the address bar or opened from a bookmark) are allowed.
//! 3. Otherwise, if it sent `Origin`, it must be the server's own origin,
//!    such as `https://accordion.example.com`. Before a name is set, it must
//!    match the `Host` header instead.
//! 4. Requests with neither header aren't from a browser, so they can't carry
//!    a stolen cookie by accident. They're allowed.
//!
//! WebSocket upgrades are GET requests, so they need their own `Origin` check
//! when Docs adds them.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::header::{HOST, ORIGIN};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::ApiError;

/// `expected` is the server's origin, or `None` before a name is set.
pub async fn reject_cross_origin(
    State(expected): State<Option<Arc<str>>>,
    request: Request,
    next: Next,
) -> Response {
    if is_cross_origin(&request, expected.as_deref()) {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "Accordion blocked this request because it came from another website. \
             Open Accordion directly, then try again.",
        )
        .into_response();
    }
    next.run(request).await
}

fn is_cross_origin(request: &Request, expected: Option<&str>) -> bool {
    if matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) {
        return false;
    }
    let headers = request.headers();
    if let Some(site) = headers.get("sec-fetch-site") {
        return !(site == "same-origin" || site == "none");
    }
    let Some(origin) = headers.get(ORIGIN) else {
        return false;
    };
    if let Some(expected) = expected {
        return !origin
            .to_str()
            .is_ok_and(|origin| origin.eq_ignore_ascii_case(expected));
    }
    // HTTP/2 requests carry the host in the URI instead of a Host header.
    let host = headers
        .get(HOST)
        .and_then(|host| host.to_str().ok())
        .or_else(|| {
            request
                .uri()
                .authority()
                .map(|authority| authority.as_str())
        });
    let origin_host = origin.to_str().ok().and_then(|origin| {
        origin
            .strip_prefix("https://")
            .or_else(|| origin.strip_prefix("http://"))
    });
    match (origin_host, host) {
        (Some(origin_host), Some(host)) => !origin_host.eq_ignore_ascii_case(host),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;

    use super::*;

    fn request(method: Method, headers: &[(&str, &str)]) -> Request {
        let mut builder = Request::builder().method(method).uri("/api/session");
        for &(name, value) in headers {
            builder = builder.header(name, value);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[test]
    fn reads_are_always_allowed() {
        let req = request(Method::GET, &[("sec-fetch-site", "cross-site")]);
        assert!(!is_cross_origin(&req, None));
    }

    #[test]
    fn uses_sec_fetch_site_when_present() {
        for (site, blocked) in [
            ("same-origin", false),
            ("none", false),
            ("same-site", true),
            ("cross-site", true),
        ] {
            let req = request(Method::POST, &[("sec-fetch-site", site)]);
            assert_eq!(is_cross_origin(&req, None), blocked, "{site}");
        }
    }

    #[test]
    fn falls_back_to_comparing_origin_with_host() {
        let host = ("host", "accordion.local:8443");
        for (origin, blocked) in [
            ("https://accordion.local:8443", false),
            ("https://ACCORDION.local:8443", false),
            ("https://accordion.local", true),
            ("https://evil.example", true),
            ("null", true),
        ] {
            let req = request(Method::POST, &[host, ("origin", origin)]);
            assert_eq!(is_cross_origin(&req, None), blocked, "{origin}");
        }
    }

    #[test]
    fn compares_origin_with_the_servers_own_origin_once_named() {
        let expected = Some("https://accordion.example.com");
        for (origin, blocked) in [
            ("https://accordion.example.com", false),
            ("https://Accordion.Example.com", false),
            ("http://accordion.example.com", true),
            ("https://accordion.example.com:8443", true),
            ("https://evil.example", true),
        ] {
            // The Host header doesn't matter once the server has a name.
            let req = request(
                Method::POST,
                &[("host", "evil.example"), ("origin", origin)],
            );
            assert_eq!(is_cross_origin(&req, expected), blocked, "{origin}");
        }
    }

    #[test]
    fn allows_clients_that_arent_browsers() {
        let req = request(Method::DELETE, &[("host", "accordion.local")]);
        assert!(!is_cross_origin(&req, None));
    }
}
