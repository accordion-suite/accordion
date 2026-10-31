//! The web app (decision 13 in `docs/decisions`): its page at each module's
//! address, such as `/drive/`, and at `/settings/`, its scripts, styles, and
//! icons under `/app/`, and the manifest that lets phones and computers
//! install it.
//!
//! The release package installs the app's files in `/usr/lib/accordion/web`.
//! `ACCORDION_WEB_DIR` points the server at another build instead, such as
//! `accordion-web`'s `dist/` folder during development. The build writes a
//! gzip and a Brotli copy of each file, so the server never compresses on a
//! request.
//!
//! The build also writes `files.json`, every file's SHA-256 hash. The server
//! checks the files against it as it starts. If any is missing or changed,
//! such as after a disk problem or an install that stopped partway, the
//! app's pages say so and how to fix it, and Server health shows a problem.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use accordion_update::Sha256;
use axum::Router;
use axum::extract::{Path as UrlPath, State};
use axum::http::header::{
    ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_SECURITY_POLICY, CONTENT_TYPE,
    REFERRER_POLICY, VARY, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use serde::Deserialize;

use crate::AppState;
use crate::setup::{CONTENT_SECURITY_POLICY_VALUE, asset};

/// Where the release package installs the app.
const INSTALLED: &str = "/usr/lib/accordion/web";

/// The build's list of its files.
const FILE_LIST: &str = "files.json";

#[derive(Deserialize)]
struct FileList {
    /// Each file's path in the build, such as `app/main-HSNK3GWX.js`, and
    /// its SHA-256 hash in lowercase hex.
    files: BTreeMap<String, String>,
}

/// The app's built files.
#[derive(Clone, Debug)]
pub struct WebApp {
    dir: Arc<Path>,
    /// The files that are missing or changed, as of when the server started.
    damaged: Arc<[String]>,
}

impl WebApp {
    /// The build in `dir`, after checking its files against its list.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir: PathBuf = dir.into();
        let damaged = damaged_files(&dir).into();
        Self {
            dir: Arc::from(dir),
            damaged,
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
            return Ok(Some(Self::new(dir).logged()));
        }
        let installed = Path::new(INSTALLED);
        Ok(installed
            .join("index.html")
            .is_file()
            .then(|| Self::new(installed).logged()))
    }

    fn logged(self) -> Self {
        if !self.damaged.is_empty() {
            tracing::error!(
                files = ?self.damaged,
                "the web app in {} is missing files or has files that changed, so its pages \
                 don't open. {}",
                self.dir.display(),
                self.fix()
            );
        }
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The files that are missing or changed. Empty when the app is whole.
    pub fn damaged(&self) -> &[String] {
        &self.damaged
    }

    /// How an admin puts the files back.
    pub fn fix(&self) -> &'static str {
        if *self.dir == *Path::new(INSTALLED) {
            "Run Accordion's install command again on the server. It puts the files back and \
             keeps everyone's data."
        } else {
            "Build accordion-web again with npm run build, then restart Accordion."
        }
    }
}

/// The files in `dir` that don't match its list. The list itself counts as
/// missing if it's not there or can't be read.
fn damaged_files(dir: &Path) -> Vec<String> {
    let list = std::fs::read(dir.join(FILE_LIST))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<FileList>(&bytes).ok());
    let Some(list) = list else {
        return vec![FILE_LIST.to_owned()];
    };
    list.files
        .into_iter()
        .filter(|(path, hash)| {
            // The list names files inside the build, never outside it.
            let inside = Path::new(path)
                .components()
                .all(|part| matches!(part, Component::Normal(_)));
            !inside || !std::fs::read(dir.join(path)).is_ok_and(|bytes| sha256_hex(&bytes) == *hash)
        })
        .map(|(path, _)| path)
        .collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut sum = Sha256::default();
    sum.update(bytes);
    let mut hex = String::with_capacity(64);
    for byte in sum.finish() {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// What the app's pages show while its files are damaged.
const DAMAGED_HTML: &str = include_str!("web/damaged.html");

/// The server's own pages from before the web app had Settings, and the
/// sections of Settings that took their place, so bookmarks still work.
const MOVED: [(&str, &str); 6] = [
    ("/account", "/settings/account"),
    ("/people", "/settings/people"),
    ("/health", "/settings/health"),
    ("/backups", "/settings/backups"),
    ("/updates", "/settings/updates"),
    ("/network", "/settings/network"),
];

/// The app's page at each of `pages`, the IDs of the modules that are on,
/// and at Settings, its files under `/app/`, and its manifest.
pub(crate) fn routes<'a>(pages: impl Iterator<Item = &'a str>) -> Router<AppState> {
    let mut router = Router::new()
        .route("/app/{file}", get(app_file))
        .route("/manifest.webmanifest", get(manifest));
    for (from, to) in MOVED {
        router = router.route(from, get(move || async move { Redirect::permanent(to) }));
    }
    for id in pages.chain(["settings"]) {
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
    if !web.damaged.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            asset(DAMAGED_HTML, "text/html; charset=utf-8"),
        )
            .into_response();
    }
    // Checked on each visit, because the page names the newest scripts.
    serve(
        &web.dir.join("index.html"),
        "text/html; charset=utf-8",
        "no-cache",
        &headers,
    )
    .await
}

/// The manifest. Its name can't change, so browsers check it on each visit,
/// and see new icons after an update.
async fn manifest(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(web) = state.web else {
        return StatusCode::NOT_FOUND.into_response();
    };
    serve(
        &web.dir.join("manifest.webmanifest"),
        "application/manifest+json",
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
