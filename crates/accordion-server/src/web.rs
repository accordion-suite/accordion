//! The web app (decision 13 in `docs/decisions`): its page at each module's
//! address, such as `/drive/`, and its scripts and styles under `/app/`.
//!
//! The release package installs the app's files in `/usr/lib/accordion/web`.
//! `ACCORDION_WEB_DIR` points the server at another build instead, such as
//! `accordion-web`'s `dist/` folder during development. The build writes a
//! gzip and a Brotli copy of each file, so the server never compresses on a
//! request.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path as UrlPath, State};
use axum::http::header::{
    ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_SECURITY_POLICY, CONTENT_TYPE,
    REFERRER_POLICY, VARY, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use crate::AppState;
use crate::setup::CONTENT_SECURITY_POLICY_VALUE;

/// Where the release package installs the app.
const INSTALLED: &str = "/usr/lib/accordion/web";

/// The app's built files.
#[derive(Clone, Debug)]
pub struct WebApp {
    dir: Arc<Path>,
}

impl WebApp {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: Arc::from(dir.into()),
        }
    }

    /// The build in `ACCORDION_WEB_DIR`, or the installed one. `None` when
    /// there's neither, such as on a development server without the app.
    pub fn from_env() -> Result<Option<Self>, String> {
        if let Some(dir) = std::env::var_os("ACCORDION_WEB_DIR").filter(|dir| !dir.is_empty()) {
            let dir = PathBuf::from(dir);
            if !dir.join("index.html").is_file() {
                return Err(format!(
                    "ACCORDION_WEB_DIR is \"{}\", which has no index.html. Set it to the dist \
                     folder of a build of accordion-web, made with npm run build.",
                    dir.display()
                ));
            }
            return Ok(Some(Self::new(dir)));
        }
        let installed = Path::new(INSTALLED);
        Ok(installed
            .join("index.html")
            .is_file()
            .then(|| Self::new(installed)))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// The app's page at each of `pages`, the IDs of the modules that are on,
/// and its files under `/app/`.
pub(crate) fn routes<'a>(pages: impl Iterator<Item = &'a str>) -> Router<AppState> {
    let mut router = Router::new().route("/app/{file}", get(app_file));
    for id in pages {
        router = router
            .route(&format!("/{id}"), get(page))
            .route(&format!("/{id}/"), get(page))
            .route(&format!("/{id}/{{*rest}}"), get(page));
    }
    router
}

/// The app's one page. The app shows the right view for the address, so a
/// reload or a shared link opens the same place.
async fn page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(web) = state.web else {
        return (
            StatusCode::NOT_FOUND,
            [(
                CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            )],
            "This server doesn't have the web app. Set ACCORDION_WEB_DIR to a build of \
             accordion-web, then restart Accordion.",
        )
            .into_response();
    };
    // Checked on each visit, because the page names the newest scripts.
    serve(
        &web.dir.join("index.html"),
        "text/html; charset=utf-8",
        "no-cache",
        &headers,
    )
    .await
}

/// A script, style, or image. Its name includes a hash of its contents, so
/// browsers can keep it for a year.
async fn app_file(
    State(state): State<AppState>,
    UrlPath(file): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    let (Some(web), Some(content_type)) = (state.web, content_type(&file)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !file
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        || file.starts_with('.')
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    serve(
        &web.dir.join("app").join(&file),
        content_type,
        "public, max-age=31536000, immutable",
        &headers,
    )
    .await
}

/// The types of file the app has. Anything else isn't served.
fn content_type(file: &str) -> Option<&'static str> {
    Some(match file.rsplit_once('.')?.1 {
        "js" => "text/javascript",
        "css" => "text/css",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "webmanifest" => "application/manifest+json",
        _ => return None,
    })
}

/// Sends `path`, or its compressed copy if the browser accepts it.
async fn serve(
    path: &Path,
    content_type: &'static str,
    cache: &'static str,
    headers: &HeaderMap,
) -> Response {
    let accepts = |encoding: &str| {
        headers
            .get_all(ACCEPT_ENCODING)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .any(|item| {
                let mut parts = item.split(';').map(str::trim);
                parts.next() == Some(encoding) && !parts.any(|part| part.replace(' ', "") == "q=0")
            })
    };
    let mut body = None;
    for (encoding, suffix) in [("br", "br"), ("gzip", "gz")] {
        if accepts(encoding) {
            let mut compressed = path.as_os_str().to_owned();
            compressed.push(".");
            compressed.push(suffix);
            if let Ok(bytes) = tokio::fs::read(&compressed).await {
                body = Some((bytes, Some(encoding)));
                break;
            }
        }
    }
    let (bytes, encoding) = match body {
        Some(found) => found,
        None => match tokio::fs::read(path).await {
            Ok(bytes) => (bytes, None),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return StatusCode::NOT_FOUND.into_response();
            }
            Err(err) => {
                tracing::error!(path = %path.display(), %err, "couldn't read a web app file");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        },
    };
    let mut response = (
        [
            (CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (CACHE_CONTROL, HeaderValue::from_static(cache)),
            (VARY, HeaderValue::from_static("accept-encoding")),
            (X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
            (REFERRER_POLICY, HeaderValue::from_static("no-referrer")),
            (
                CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(CONTENT_SECURITY_POLICY_VALUE),
            ),
        ],
        bytes,
    )
        .into_response();
    if let Some(encoding) = encoding {
        response
            .headers_mut()
            .insert(CONTENT_ENCODING, HeaderValue::from_static(encoding));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_only_the_apps_kinds_of_file() {
        assert_eq!(content_type("main-JHQCIFJL.js"), Some("text/javascript"));
        assert_eq!(content_type("main-TSNOJFU5.css"), Some("text/css"));
        assert_eq!(content_type("main-JHQCIFJL.js.gz"), None);
        assert_eq!(content_type("notes.txt"), None);
        assert_eq!(content_type("README"), None);
    }
}
