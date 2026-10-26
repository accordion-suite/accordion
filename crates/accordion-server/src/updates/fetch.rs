//! Downloads release files, following redirects, with a size limit.
//!
//! Requests carry no cookies and a user agent with no version, so they say
//! nothing about the server (decision 12).

use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Empty, Limited};
use hyper::body::Bytes;
use hyper::header::{LOCATION, USER_AGENT};
use hyper::{Request, StatusCode, Uri};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;

/// A download that takes longer than this has stalled.
const TIMEOUT: Duration = Duration::from_secs(60);

/// GitHub sends a download through two redirects.
const MAX_REDIRECTS: usize = 5;

#[derive(Clone)]
pub(crate) struct Fetcher {
    client: Client<HttpsConnector<HttpConnector>, Empty<Bytes>>,
}

pub(crate) enum Fetched {
    Found {
        body: Bytes,
        /// Where the first redirect pointed, if there was one.
        first_redirect: Option<Uri>,
    },
    NotFound,
}

impl Fetcher {
    /// Trusts the operating system's certificate authorities, like the
    /// gateway's clients. `allow_http` is for tests against a local server.
    pub(crate) fn new(allow_http: bool) -> Result<Self, String> {
        let builder = HttpsConnectorBuilder::new()
            .with_provider_and_platform_verifier(Arc::new(rustls::crypto::ring::default_provider()))
            .map_err(|err| format!("Accordion couldn't load the system's certificates: {err}"))?;
        let builder = if allow_http {
            builder.https_or_http()
        } else {
            builder.https_only()
        };
        let connector = builder.enable_http1().enable_http2().build();
        Ok(Self {
            client: Client::builder(TokioExecutor::new()).build(connector),
        })
    }

    /// Downloads `url`, up to `limit` bytes.
    pub(crate) async fn get(&self, url: &str, limit: usize) -> Result<Fetched, String> {
        let uri: Uri = url
            .parse()
            .map_err(|_| format!("{url} isn't a web address"))?;
        let host = uri.host().unwrap_or("the update server").to_owned();
        tokio::time::timeout(TIMEOUT, self.follow(uri, limit))
            .await
            .map_err(|_| {
                format!(
                    "{host} didn't finish sending the update information within {} seconds",
                    TIMEOUT.as_secs()
                )
            })?
    }

    async fn follow(&self, mut uri: Uri, limit: usize) -> Result<Fetched, String> {
        let mut first_redirect = None;
        for _ in 0..=MAX_REDIRECTS {
            let host = uri.host().unwrap_or("the update server").to_owned();
            let request = Request::get(uri.clone())
                .header(USER_AGENT, "Accordion")
                .body(Empty::new())
                .map_err(|err| err.to_string())?;
            let response =
                self.client.request(request).await.map_err(|err| {
                    format!("Accordion couldn't reach {host} ({})", innermost(&err))
                })?;
            let status = response.status();
            if status.is_redirection() {
                let next = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|location| location.to_str().ok())
                    .and_then(|location| resolve(&uri, location))
                    .ok_or_else(|| format!("{host} sent a redirect without a valid address"))?;
                first_redirect.get_or_insert_with(|| next.clone());
                uri = next;
                continue;
            }
            if status == StatusCode::NOT_FOUND {
                return Ok(Fetched::NotFound);
            }
            if !status.is_success() {
                return Err(format!("{host} answered with an error ({status})"));
            }
            let body = Limited::new(response.into_body(), limit)
                .collect()
                .await
                .map_err(|err| {
                    if err.is::<http_body_util::LengthLimitError>() {
                        format!("{host} sent more update information than expected")
                    } else {
                        format!("the download from {host} stopped partway ({err})")
                    }
                })?
                .to_bytes();
            return Ok(Fetched::Found {
                body,
                first_redirect,
            });
        }
        Err(format!(
            "the download redirected more than {MAX_REDIRECTS} times"
        ))
    }
}

/// A `Location` header's address, which can be relative to `base`.
fn resolve(base: &Uri, location: &str) -> Option<Uri> {
    if location.starts_with('/') && !location.starts_with("//") {
        let scheme = base.scheme_str()?;
        let authority = base.authority()?;
        return format!("{scheme}://{authority}{location}").parse().ok();
    }
    let uri: Uri = location.parse().ok()?;
    (uri.scheme().is_some() && uri.authority().is_some()).then_some(uri)
}

/// The most specific reason in an error's chain, such as "connection
/// refused", rather than hyper's "client error (Connect)".
fn innermost(err: &(dyn std::error::Error + 'static)) -> String {
    let mut current = err;
    while let Some(source) = current.source() {
        current = source;
    }
    current.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_redirects() {
        let base: Uri = "https://github.com/a/b/releases/latest/download/SHA256SUMS"
            .parse()
            .unwrap();
        assert_eq!(
            resolve(&base, "/a/b/releases/download/v0.2.0/SHA256SUMS").unwrap(),
            "https://github.com/a/b/releases/download/v0.2.0/SHA256SUMS"
        );
        assert_eq!(
            resolve(&base, "https://objects.example/x?y=1").unwrap(),
            "https://objects.example/x?y=1"
        );
        assert!(resolve(&base, "//elsewhere.example/x").is_none());
        assert!(resolve(&base, "relative/path").is_none());
    }
}
