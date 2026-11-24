//! Importing from Google Takeout through the **Import from Google** page's
//! API (decision 34), with every module, and this server's own program as
//! the sandbox that reads the archives, as on a real server.

use std::io::Write;
use std::time::Duration;

use accordion_core::coedit::{CoEditing, Timing};
use accordion_core::db::Database;
use accordion_core::events::Events;
use accordion_core::identity::{Identity, NewUser};
use accordion_core::live::Live;
use accordion_core::storage::Storage;
use accordion_core::takeout::{self, Imports, Takeout};
use accordion_core::texts::{self, Texts};
use accordion_core::thumbnails::{Thumbnails, Tools};
use accordion_gateway::Peer;
use accordion_module::{Services, Stopping};
use accordion_server::{AppState, Site, modules, router};
use accordion_types::{ItemId, Role, UserId};
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, Response, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tower::ServiceExt;
use zip::write::SimpleFileOptions;

const HOST: &str = "accordion.example.com";
const LAUNCHER: &str = env!("CARGO_BIN_EXE_accordion");

struct Server {
    _dir: tempfile::TempDir,
    _stop: watch::Sender<bool>,
    /// The modules' background work, such as their importers.
    _background: JoinSet<()>,
    app: Router,
    storage: Storage,
    alex: UserId,
}

async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("accordion.db")).unwrap();
    let identity = Identity::new(db.clone()).await.unwrap();
    let storage = Storage::open(db.clone(), identity.clone(), &dir.path().join("storage"))
        .await
        .unwrap();
    let alex = identity
        .create_user(NewUser {
            username: "alex".into(),
            display_name: "Alex Kim".into(),
            role: Role::Owner,
            password: "correct horse".into(),
        })
        .await
        .unwrap()
        .id;
    let events = Events::open(db.clone()).await.unwrap();
    let live = Live::new(db.clone(), &identity);
    let (stop, stopping) = watch::channel(false);
    let thumbnails = Thumbnails::open(
        db.clone(),
        storage.clone(),
        dir.path(),
        Tools::installed(LAUNCHER),
    )
    .await
    .unwrap();
    let scratch = dir.path().join("takeout");
    std::fs::create_dir_all(&scratch).unwrap();
    let imports = Imports::open(
        db.clone(),
        storage.clone(),
        events.clone(),
        Takeout::new(takeout::Tools::installed(LAUNCHER), scratch),
    )
    .await
    .unwrap();
    let mut background = JoinSet::new();
    let modules = modules::start(
        modules::all(),
        &Services {
            takeout: imports.clone(),
            db: db.clone(),
            coediting: CoEditing::open(
                db.clone(),
                storage.clone(),
                identity.clone(),
                Timing::default(),
            )
            .await
            .unwrap(),
            identity: identity.clone(),
            storage: storage.clone(),
            transcripts: accordion_core::transcripts::Transcripts::open(
                db.clone(),
                storage.clone(),
                dir.path(),
                accordion_core::transcripts::Tools::installed("/nonexistent"),
                None,
                accordion_core::transcripts::MODELS_URL,
            )
            .await
            .unwrap(),
            events: events.clone(),
            live: live.clone(),
            link_origin: watch::channel(None).1,
            thumbnails: thumbnails.clone(),
            texts: Texts::open(
                db,
                storage.clone(),
                thumbnails.clone(),
                dir.path(),
                texts::Tools::installed(LAUNCHER),
            )
            .await
            .unwrap(),
        },
        &Stopping::new(stopping.clone()),
        &mut background,
    )
    .await
    .unwrap();
    let mut stopped = stopping;
    background.spawn(imports.clone().run(async move {
        let _ = stopped.wait_for(|&stopped| stopped).await;
    }));
    let app = router(
        AppState {
            identity,
            setup: None,
            health: Default::default(),
            admin_access: Default::default(),
            modules,
            backup: None,
            storage: Some(storage.clone()),
            updates: None,
            switches: None,
            events: Some(events),
            live: Some(live),
            remote: None,
            link_sharing: None,
            thumbnails: Some(thumbnails),
            takeout: Some(imports),
            web: None,
        },
        &Site::named(HOST.parse().unwrap(), 443),
    )
    .layer(MockConnectInfo(Peer {
        local: "192.168.1.20:443".parse().unwrap(),
        remote: "192.168.1.30:50000".parse().unwrap(),
    }));
    Server {
        _dir: dir,
        _stop: stop,
        _background: background,
        app,
        storage,
        alex,
    }
}

/// An export with a Keep note, a Drive file, and a Gmail folder no app
/// converts yet.
fn export() -> Vec<u8> {
    let note = json!({
        "title": "Groceries",
        "textContent": "Milk",
        "createdTimestampUsec": 1_762_000_000_000_000u64,
        "userEditedTimestampUsec": 1_762_000_060_000_000u64,
        "isPinned": true,
    });
    let info = json!({
        "title": "Report.pdf",
        "drive_id": "r1",
        "mime_type": "application/pdf",
        "created": "2023-05-01T08:00:00.000Z",
        "content_last_modified": "2023-05-06T09:30:00.000Z",
    });
    let files: [(&str, Vec<u8>); 4] = [
        ("Takeout/Keep/Groceries.json", note.to_string().into_bytes()),
        ("Takeout/Drive/Report.pdf", b"%PDF report".to_vec()),
        (
            "Takeout/Drive/Report.pdf-info.json",
            info.to_string().into_bytes(),
        ),
        ("Takeout/Mail/All mail.mbox", b"From someone\n".to_vec()),
    ];
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, contents) in files {
        zip.start_file(name, options).unwrap();
        zip.write_all(&contents).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

impl Server {
    async fn send(&self, request: axum::http::request::Builder, body: Body) -> Response<Body> {
        self.app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap()
    }

    /// Signs in as the web app does, and returns the session cookie.
    async fn sign_in(&self) -> String {
        let response = self
            .send(
                browser("POST", "/api/session"),
                Body::from(json!({ "username": "alex", "password": "correct horse" }).to_string()),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        set_cookie.split(';').next().unwrap().to_owned()
    }

    async fn call(
        &self,
        cookie: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let body = body.map_or(Body::empty(), |body| Body::from(body.to_string()));
        let response = self
            .send(browser(method, path).header(header::COOKIE, cookie), body)
            .await;
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    }
}

/// A request from the web app.
fn browser(method: &str, path: &str) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, HOST)
        .header("sec-fetch-site", "same-origin")
        .header(header::CONTENT_TYPE, "application/json")
}

#[tokio::test]
async fn imports_an_export_from_the_page() {
    let server = start().await;
    let cookie = server.sign_in().await;

    // The page makes **Google Takeout** and uploads the archive there.
    let (status, folder) = server
        .call(&cookie, "POST", "/api/takeout/folder", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let folder: ItemId = serde_json::from_value(folder["id"].clone()).unwrap();
    let part = server
        .storage
        .create_file(
            folder,
            "takeout-20261124T065651Z-1-001.zip",
            export().as_slice(),
        )
        .await
        .unwrap();
    let (_, again) = server
        .call(&cookie, "POST", "/api/takeout/folder", None)
        .await;
    assert_eq!(again["id"], json!(folder));

    let (status, archives) = server
        .call(&cookie, "GET", "/api/takeout/archives", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(archives["folder"], json!(folder));
    assert_eq!(archives["archives"][0]["id"], json!(part.id));
    assert_eq!(archives["archives"][0]["place"], "takeout");

    let (status, plan) = server
        .call(
            &cookie,
            "POST",
            "/api/takeout/plan",
            Some(json!({ "parts": [part.id] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    let folders = plan["folders"].as_array().unwrap();
    let found = |name: &str| {
        folders
            .iter()
            .find(|folder| folder["folder"] == name)
            .unwrap()
    };
    assert_eq!(found("Keep")["product"], "keep");
    assert_eq!(found("Keep")["app"], "Notes");
    assert_eq!(found("Drive")["product"], "drive");
    assert_eq!(found("Mail")["product"], Value::Null);
    assert_eq!(plan["room"], Value::Null);

    let take = json!([{ "product": "keep" }, { "product": "drive" }, { "other": "Mail" }]);
    let (status, started) = server
        .call(
            &cookie,
            "POST",
            "/api/takeout/imports",
            Some(json!({ "parts": [part.id], "take": take })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{started}");

    let import = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let (_, list) = server
                .call(&cookie, "GET", "/api/takeout/imports", None)
                .await;
            let latest = list["imports"][0].clone();
            if latest["state"] != "running" && latest["state"] != "canceling" {
                break latest;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(import["state"], "done", "{import}");
    assert_eq!(import["id"], started["id"]);
    assert!(
        import["report"]["name"]
            .as_str()
            .unwrap()
            .starts_with("Takeout import ")
    );

    // The note is in Notes, the file in Drive, and Gmail in Unconverted.
    let (_, notes) = server.call(&cookie, "GET", "/api/notes/notes", None).await;
    assert_eq!(notes["notes"][0]["title"], "Groceries");
    assert_eq!(notes["notes"][0]["pinned"], true);
    let top = server.storage.top_folder(server.alex).await.unwrap();
    let find = |path: &[&str]| {
        let path = path.iter().map(|name| (*name).to_owned()).collect();
        let storage = server.storage.clone();
        async move { storage.find(top, path).await.unwrap() }
    };
    assert!(find(&["From Google Drive", "Report.pdf"]).await.is_some());
    assert!(
        find(&["Google Takeout", "Unconverted", "Mail", "All mail.mbox"])
            .await
            .is_some()
    );

    // An import that's over can't be canceled.
    let id = import["id"].as_str().unwrap();
    let (status, _) = server
        .call(
            &cookie,
            "POST",
            &format!("/api/takeout/imports/{id}/cancel"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn says_what_it_cant_import() {
    let server = start().await;
    let cookie = server.sign_in().await;
    let (status, error) = server
        .call(
            &cookie,
            "POST",
            "/api/takeout/plan",
            Some(json!({ "parts": [] })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"], "Choose the export's archives to import.");

    let top = server.storage.top_folder(server.alex).await.unwrap();
    let not_an_archive = server
        .storage
        .create_file(top, "takeout.zip", b"not a zip".as_slice())
        .await
        .unwrap();
    let (status, error) = server
        .call(
            &cookie,
            "POST",
            "/api/takeout/plan",
            Some(json!({ "parts": [not_an_archive.id] })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert!(
        error["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty())
    );

    let (status, error) = server
        .call(
            &cookie,
            "POST",
            "/api/takeout/imports",
            Some(json!({ "parts": [not_an_archive.id], "take": [{ "product": "photos" }] })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
}
