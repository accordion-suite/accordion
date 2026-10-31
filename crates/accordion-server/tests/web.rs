//! Opens the web app (decision 13): its page at Drive's address, and its
//! scripts, from a folder laid out like `accordion-web`'s build.

use std::path::Path;
use std::time::SystemTime;

use accordion_core::db::Database;
use accordion_core::identity::{Identity, NewUser};
use accordion_core::storage::Storage;
use accordion_gateway::Peer;
use accordion_module::{Services, Stopping};
use accordion_server::health::{Health, Level};
use accordion_server::web::WebApp;
use accordion_server::{AppState, Site, modules, router};
use accordion_types::Role;
use accordion_update::Sha256;
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, Response, StatusCode, header};
use http_body_util::BodyExt;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tower::ServiceExt;

const HOST: &str = "accordion.example.com";
const PAGE: &str = "<!doctype html><title>Accordion</title>";
const SCRIPT: &str = "console.log(1)";
const MANIFEST: &str = r#"{"name":"Accordion"}"#;

struct Server {
    _dir: tempfile::TempDir,
    _stop: watch::Sender<bool>,
    app: Router,
}

/// Starts a set-up server with every module, and with a web app build if
/// `web`.
async fn start(web: bool) -> Server {
    start_with(web, |_| {}).await
}

/// Like `start`, with a build that `change` alters after it's written, as a
/// disk problem or an install that stopped partway would.
async fn start_with(web: bool, change: impl FnOnce(&Path)) -> Server {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("accordion.db")).unwrap();
    let identity = Identity::new(db.clone()).await.unwrap();
    identity
        .create_user(NewUser {
            username: "alex".into(),
            display_name: "Alex Kim".into(),
            role: Role::Owner,
            password: "correct horse".into(),
        })
        .await
        .unwrap();
    let storage = Storage::open(db.clone(), identity.clone(), &dir.path().join("storage"))
        .await
        .unwrap();
    let (stop, stopping) = watch::channel(false);
    let modules = modules::start(
        modules::all(),
        &Services {
            db,
            identity: identity.clone(),
            storage,
        },
        &Stopping::new(stopping),
        &mut JoinSet::new(),
    )
    .await
    .unwrap();

    let build = dir.path().join("dist");
    std::fs::create_dir_all(build.join("app")).unwrap();
    std::fs::write(build.join("index.html"), PAGE).unwrap();
    // Not really compressed. The test only checks which file is sent.
    std::fs::write(build.join("index.html.br"), "brotli page").unwrap();
    std::fs::write(build.join("app/main-ABC123.js"), SCRIPT).unwrap();
    std::fs::write(build.join("app/main-ABC123.js.gz"), "gzip script").unwrap();
    std::fs::write(build.join("app/notes.txt"), "not part of the app").unwrap();
    std::fs::write(build.join("manifest.webmanifest"), MANIFEST).unwrap();
    write_file_list(&build);
    change(&build);

    let app = router(
        AppState {
            identity,
            setup: None,
            health: Default::default(),
            admin_access: Default::default(),
            modules,
            backup: None,
            storage: None,
            updates: None,
            switches: None,
            web: web.then(|| WebApp::new(build)),
        },
        &Site::named(HOST.parse().unwrap(), 443),
    )
    .layer(MockConnectInfo(Peer {
        local: "192.168.1.20:443".parse().unwrap(),
        remote: "203.0.113.5:50000".parse().unwrap(),
    }));
    Server {
        _dir: dir,
        _stop: stop,
        app,
    }
}

/// Lists the build's files with their hashes, as the build does.
fn write_file_list(build: &Path) {
    let mut files = serde_json::Map::new();
    for path in [
        "index.html",
        "index.html.br",
        "app/main-ABC123.js",
        "app/main-ABC123.js.gz",
        "manifest.webmanifest",
    ] {
        let mut sum = Sha256::default();
        sum.update(&std::fs::read(build.join(path)).unwrap());
        let hex: String = sum.finish().iter().map(|b| format!("{b:02x}")).collect();
        files.insert(path.into(), hex.into());
    }
    std::fs::write(
        build.join("files.json"),
        serde_json::json!({ "files": files }).to_string(),
    )
    .unwrap();
}

impl Server {
    async fn get(&self, path: &str, encoding: Option<&str>) -> (Response<Body>, String) {
        let mut request = Request::get(path).header(header::HOST, HOST);
        if let Some(encoding) = encoding {
            request = request.header(header::ACCEPT_ENCODING, encoding);
        }
        let response = self
            .app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let (parts, body) = response.into_parts();
        let bytes = body.collect().await.unwrap().to_bytes();
        (
            Response::from_parts(parts, Body::empty()),
            String::from_utf8_lossy(&bytes).into_owned(),
        )
    }
}

#[tokio::test]
async fn sends_the_apps_page_for_every_address_in_drive() {
    let server = start(true).await;
    for path in ["/drive", "/drive/", "/drive/folder/abc"] {
        let (response, body) = server.get(path, None).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(body, PAGE, "{path}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
        let policy = response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        assert!(policy.contains("script-src 'self'"));
        // Drive shows PDFs in a frame.
        assert!(policy.contains("frame-src 'self'"));
    }
    // Only modules have pages in the app.
    let (response, _) = server.get("/photos/", None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn sends_a_compressed_copy_when_the_browser_takes_one() {
    let server = start(true).await;
    let (response, body) = server.get("/drive/", Some("gzip, deflate, br")).await;
    assert_eq!(response.headers()[header::CONTENT_ENCODING], "br");
    assert_eq!(body, "brotli page");

    // The page has no gzip copy, so a browser without Brotli gets it as it is.
    let (response, body) = server.get("/drive/", Some("gzip")).await;
    assert!(response.headers().get(header::CONTENT_ENCODING).is_none());
    assert_eq!(body, PAGE);

    let (response, body) = server
        .get("/app/main-ABC123.js", Some("gzip, br;q=0"))
        .await;
    assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
    assert_eq!(body, "gzip script");
    assert_eq!(response.headers()[header::VARY], "accept-encoding");
}

#[tokio::test]
async fn lets_browsers_keep_scripts_for_a_year() {
    let server = start(true).await;
    let (response, body) = server.get("/app/main-ABC123.js", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body, SCRIPT);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "text/javascript");
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
}

#[tokio::test]
async fn sends_only_the_apps_own_files() {
    let server = start(true).await;
    for path in [
        "/app/notes.txt",
        "/app/missing-XYZ.js",
        "/app/..%2Findex.html",
        "/app/.hidden.js",
        "/app/main-ABC123.js.gz",
    ] {
        let (response, _) = server.get(path, None).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn sends_the_manifest_that_installs_the_app() {
    let (response, body) = start(true).await.get("/manifest.webmanifest", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/manifest+json"
    );
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
    assert_eq!(body, MANIFEST);

    let (response, _) = start(false).await.get("/manifest.webmanifest", None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn explains_a_server_without_the_app() {
    let server = start(false).await;
    let (response, body) = server.get("/drive/", None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(body.contains("ACCORDION_WEB_DIR"));
}

#[tokio::test]
async fn opens_drive_from_the_servers_address() {
    let (response, _) = start(true).await.get("/", None).await;
    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(response.headers()[header::LOCATION], "/drive/start");

    // Without the app, the server's own page says what's missing.
    let (response, body) = start(false).await.get("/", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body.contains("ACCORDION_WEB_DIR"));
}

#[tokio::test]
async fn sends_the_apps_page_for_settings_and_the_pages_it_replaced() {
    let server = start(true).await;
    for path in ["/settings", "/settings/", "/settings/people"] {
        let (response, body) = server.get(path, None).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(body, PAGE, "{path}");
    }
    for (old, new) in [
        ("/account", "/settings/account"),
        ("/people", "/settings/people"),
        ("/health", "/settings/health"),
        ("/backups", "/settings/backups"),
        ("/updates", "/settings/updates"),
        ("/network", "/settings/network"),
    ] {
        let (response, _) = server.get(old, None).await;
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT, "{old}");
        assert_eq!(response.headers()[header::LOCATION], new);
    }
}

#[tokio::test]
async fn says_so_when_the_apps_files_are_missing_or_changed() {
    let server = start_with(true, |build| {
        std::fs::write(build.join("app/main-ABC123.js"), "changed").unwrap();
        std::fs::remove_file(build.join("index.html.br")).unwrap();
    })
    .await;
    for path in ["/drive/", "/settings/account"] {
        let (response, body) = server.get(path, None).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{path}");
        assert!(body.contains("Drive can't open right now"), "{path}");
    }
}

#[test]
fn checks_the_apps_files_against_the_builds_list() {
    let dir = tempfile::tempdir().unwrap();
    let build = dir.path();
    std::fs::create_dir_all(build.join("app")).unwrap();
    for path in [
        "index.html",
        "index.html.br",
        "app/main-ABC123.js",
        "app/main-ABC123.js.gz",
        "manifest.webmanifest",
    ] {
        std::fs::write(build.join(path), path).unwrap();
    }
    let health = |web: WebApp| {
        Health {
            web: Some(web),
            ..Health::default()
        }
        .checks(SystemTime::now())
        .into_iter()
        .find(|check| check.id == "web_app")
        .unwrap()
    };

    // A build from before the list, or with a list that's gone.
    let web = WebApp::new(build);
    assert_eq!(web.damaged(), ["files.json"]);
    let check = health(web);
    assert_eq!(check.state, Level::Problem);
    assert!(
        check.summary.starts_with("The web app's file files.json"),
        "{}",
        check.summary
    );

    write_file_list(build);
    let web = WebApp::new(build);
    assert!(web.damaged().is_empty());
    assert_eq!(health(web).state, Level::Ok);

    std::fs::write(build.join("app/main-ABC123.js"), "changed").unwrap();
    std::fs::remove_file(build.join("index.html.br")).unwrap();
    let web = WebApp::new(build);
    assert_eq!(web.damaged(), ["app/main-ABC123.js", "index.html.br"]);
    let check = health(web);
    assert_eq!(check.state, Level::Problem);
    assert!(
        check
            .summary
            .starts_with("Accordion found two web app files"),
        "{}",
        check.summary
    );
    assert!(check.summary.contains("npm run build"), "{}", check.summary);

    // The list can't send the check outside the build.
    std::fs::write(
        build.join("files.json"),
        r#"{"files": {"../outside.txt": "00"}}"#,
    )
    .unwrap();
    assert_eq!(WebApp::new(build).damaged(), ["../outside.txt"]);
}
