//! Creates an app password the way Settings does, then uses it to open
//! Drive over WebDAV through the whole server.

use accordion_core::db::Database;
use accordion_core::events::Events;
use accordion_core::identity::{Identity, NewUser};
use accordion_core::live::Live;
use accordion_core::storage::Storage;
use accordion_core::texts::{self, Texts};
use accordion_core::thumbnails::{Thumbnails, Tools};
use accordion_gateway::Peer;
use accordion_module::{Services, Stopping};
use accordion_server::{AppState, Site, modules, router};
use accordion_types::Role;
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, Response, StatusCode, header};
use base64ct::{Base64, Encoding};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tower::ServiceExt;

const HOST: &str = "accordion.example.com";

struct Server {
    _dir: tempfile::TempDir,
    _stop: watch::Sender<bool>,
    app: Router,
}

/// Starts a server with every module, whose requests come from outside the
/// home network.
async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("accordion.db")).unwrap();
    let identity = Identity::new(db.clone()).await.unwrap();
    let storage = Storage::open(db.clone(), identity.clone(), &dir.path().join("storage"))
        .await
        .unwrap();
    identity
        .create_user(NewUser {
            username: "alex".into(),
            display_name: "Alex Kim".into(),
            role: Role::Owner,
            password: "correct horse".into(),
        })
        .await
        .unwrap();
    let events = Events::open(db.clone()).await.unwrap();
    let live = Live::new(db.clone(), &identity);
    let (stop, stopping) = watch::channel(false);
    let mut live_stopping = stop.subscribe();
    tokio::spawn(live.clone().run(async move {
        let _ = live_stopping.wait_for(|&stopped| stopped).await;
    }));
    let thumbnails = Thumbnails::open(
        db.clone(),
        storage.clone(),
        dir.path(),
        Tools::installed(env!("CARGO_BIN_EXE_accordion")),
    )
    .await
    .unwrap();
    let modules = modules::start(
        modules::all(),
        &Services {
            db: db.clone(),
            identity: identity.clone(),
            storage: storage.clone(),
            events: events.clone(),
            live: live.clone(),
            link_origin: tokio::sync::watch::channel(None).1,
            thumbnails: thumbnails.clone(),
            texts: Texts::open(
                db,
                storage,
                thumbnails,
                dir.path(),
                texts::Tools::installed(env!("CARGO_BIN_EXE_accordion")),
            )
            .await
            .unwrap(),
        },
        &Stopping::new(stopping),
        &mut JoinSet::new(),
    )
    .await
    .unwrap();
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
            events: Some(events),
            live: Some(live),
            remote: None,
            link_sharing: None,
            web: None,
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
                json_body(json!({ "username": "alex", "password": "correct horse" })),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        set_cookie.split(';').next().unwrap().to_owned()
    }
}

/// A request from Settings in the web app.
fn browser(method: &str, path: &str) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, HOST)
        .header("sec-fetch-site", "same-origin")
        .header(header::CONTENT_TYPE, "application/json")
}

/// A request from a WebDAV client, such as Finder.
fn dav(method: &str, path: &str, password: &str) -> axum::http::request::Builder {
    let credentials = Base64::encode_string(format!("alex:{password}").as_bytes());
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, HOST)
        .header(header::AUTHORIZATION, format!("Basic {credentials}"))
}

fn json_body(value: Value) -> Body {
    Body::from(value.to_string())
}

async fn json(response: Response<Body>) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn an_app_password_opens_drive_over_webdav() {
    let server = start().await;
    let cookie = server.sign_in().await;

    // The old page's address leads to Settings in the web app.
    let page = server.send(browser("GET", "/account"), Body::empty()).await;
    assert_eq!(page.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(page.headers()[header::LOCATION], "/settings/account");

    let response = server
        .send(
            browser("POST", "/api/app-passwords").header(header::COOKIE, &cookie),
            json_body(json!({ "device_name": "Alex's laptop", "use_for": "dav" })),
        )
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = json(response).await;
    let password = created["password"].as_str().unwrap().to_owned();
    assert_eq!(password.len(), 19);
    let id = created["app_password"]["id"].as_str().unwrap().to_owned();

    let response = server
        .send(
            dav("PUT", "/dav/drive/hello.txt", &password),
            Body::from("hi"),
        )
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let response = server
        .send(
            dav("PROPFIND", "/dav/drive/", &password).header("depth", "1"),
            Body::empty(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let listing = response.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&listing).contains("/dav/drive/hello.txt"));

    // The account password never opens WebDAV.
    let response = server
        .send(
            dav("PROPFIND", "/dav/drive/", "correct horse"),
            Body::empty(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let listed = json(
        server
            .send(
                browser("GET", "/api/app-passwords").header(header::COOKIE, &cookie),
                Body::empty(),
            )
            .await,
    )
    .await;
    assert_eq!(listed[0]["device_name"], "Alex's laptop");
    assert!(listed[0]["last_used_at"].is_string());

    let response = server
        .send(
            browser("DELETE", &format!("/api/app-passwords/{id}")).header(header::COOKIE, &cookie),
            Body::empty(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = server
        .send(dav("GET", "/dav/drive/hello.txt", &password), Body::empty())
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// A request from one of Accordion's apps, which sends no browser headers.
fn app(method: &str, path: &str) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, HOST)
        .header(header::CONTENT_TYPE, "application/json")
}

#[tokio::test]
async fn a_code_from_settings_signs_in_an_app() {
    let server = start().await;
    let cookie = server.sign_in().await;
    let response = server
        .send(
            browser("POST", "/api/app-codes").header(header::COOKIE, &cookie),
            Body::empty(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let made = json(response).await;
    let code = made["code"].as_str().unwrap().to_owned();
    assert_eq!(code.len(), 14, "three groups of four");
    assert!(made["expires_at"].is_string());

    let response = server
        .send(
            app("POST", "/api/app-codes/redeem"),
            json_body(json!({ "code": code, "device_name": "Alex's laptop" })),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let signed_in = json(response).await;
    assert_eq!(signed_in["username"], "alex");
    let token = signed_in["token"].as_str().unwrap().to_owned();

    // The app's token works for Drive, and the app shows in the devices.
    let response = server
        .send(
            app("GET", "/api/drive/sync/start")
                .header(header::AUTHORIZATION, format!("Bearer {token}")),
            Body::empty(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(json(response).await["cursor"].is_string());

    // It shows among the sessions, and signing it out stops its token.
    let listed = json(
        server
            .send(
                browser("GET", "/api/sessions").header(header::COOKIE, &cookie),
                Body::empty(),
            )
            .await,
    )
    .await;
    let laptop = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["device_name"] == "Alex's laptop")
        .unwrap();
    assert_eq!(laptop["client"], "app");
    assert_eq!(laptop["current"], false);
    let response = server
        .send(
            browser(
                "DELETE",
                &format!("/api/sessions/{}", laptop["id"].as_str().unwrap()),
            )
            .header(header::COOKIE, &cookie),
            Body::empty(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = server
        .send(
            app("GET", "/api/drive/sync/start")
                .header(header::AUTHORIZATION, format!("Bearer {token}")),
            Body::empty(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // A code works once.
    let response = server
        .send(
            app("POST", "/api/app-codes/redeem"),
            json_body(json!({ "code": code, "device_name": "Someone else" })),
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        json(response).await["error"]
            .as_str()
            .unwrap()
            .contains("Apps and devices")
    );
}

#[tokio::test]
async fn confirming_the_password_needs_the_right_one() {
    let server = start().await;
    let cookie = server.sign_in().await;
    for (password, status) in [
        ("wrong horse", StatusCode::UNAUTHORIZED),
        ("correct horse", StatusCode::NO_CONTENT),
    ] {
        let response = server
            .send(
                browser("POST", "/api/session/confirm").header(header::COOKIE, &cookie),
                json_body(json!({ "password": password })),
            )
            .await;
        assert_eq!(response.status(), status, "{password}");
    }
}

#[tokio::test]
async fn module_routes_get_the_servers_checks() {
    let server = start().await;
    let cookie = server.sign_in().await;
    let created = json(
        server
            .send(
                browser("POST", "/api/app-passwords").header(header::COOKIE, &cookie),
                json_body(json!({ "device_name": "Laptop", "use_for": "dav" })),
            )
            .await,
    )
    .await;
    let password = created["password"].as_str().unwrap();

    // Another name for the server, as in a DNS rebinding attack.
    let mut request = dav("PROPFIND", "/dav/drive/", password);
    request.headers_mut().unwrap().insert(
        header::HOST,
        header::HeaderValue::from_static("attacker.example"),
    );
    let response = server.send(request, Body::empty()).await;
    assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST);

    // A website trying to use a browser's saved WebDAV password.
    let response = server
        .send(
            dav("PUT", "/dav/drive/planted.txt", password).header("sec-fetch-site", "cross-site"),
            Body::from("boo"),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // The JSON API needs a session, not an app password.
    let response = server
        .send(dav("GET", "/api/drive/top", password), Body::empty())
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = server
        .send(
            browser("GET", "/api/drive/top").header(header::COOKIE, &cookie),
            Body::empty(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn the_activity_log_shows_sign_ins_and_app_passwords() {
    let server = start().await;
    let wrong = server
        .send(
            browser("POST", "/api/session"),
            json_body(json!({ "username": "alex", "password": "not it" })),
        )
        .await;
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    let cookie = server.sign_in().await;
    let created = server
        .send(
            browser("POST", "/api/app-passwords").header(header::COOKIE, &cookie),
            json_body(json!({ "device_name": "Laptop", "use_for": "dav" })),
        )
        .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    let log = json(
        server
            .send(
                browser("GET", "/api/account/activity").header(header::COOKIE, &cookie),
                Body::empty(),
            )
            .await,
    )
    .await;
    let events = log["events"].as_array().unwrap();
    let actions: Vec<_> = events.iter().map(|event| &event["action"]).collect();
    assert_eq!(
        actions,
        ["app_password_created", "signed_in", "sign_in_failed"]
    );
    assert_eq!(events[0]["details"]["device"], "Laptop");
    assert_eq!(events[1]["actor"]["name"], "Alex Kim");
    assert_eq!(events[1]["details"]["address"], "203.0.113.5");
    assert_eq!(events[2]["actor"], Value::Null);
    assert_eq!(events[2]["other"]["name"], "Alex Kim");

    // The admin log is for the home network only.
    let admin_log = server
        .send(
            browser("GET", "/api/admin/log").header(header::COOKIE, &cookie),
            Body::empty(),
        )
        .await;
    assert_eq!(admin_log.status(), StatusCode::FORBIDDEN);

    let signed_out = server
        .send(
            browser("DELETE", "/api/session").header(header::COOKIE, &cookie),
            Body::empty(),
        )
        .await;
    assert_eq!(signed_out.status(), StatusCode::NO_CONTENT);
}

/// Reads the live stream until `wanted` turns up, and returns everything
/// read so far.
async fn read_until(body: &mut Body, wanted: &str) -> String {
    let mut read = String::new();
    while !read.contains(wanted) {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), body.frame())
            .await
            .unwrap_or_else(|_| panic!("no {wanted:?} in {read:?}"));
        let Some(frame) = frame else {
            panic!("the stream ended before {wanted:?}: {read:?}");
        };
        if let Ok(data) = frame.unwrap().into_data() {
            read.push_str(std::str::from_utf8(&data).unwrap());
        }
    }
    read
}

#[tokio::test]
async fn live_updates_need_signing_in_and_end_with_the_session() {
    let server = start().await;
    let anonymous = server
        .send(browser("GET", "/api/live"), Body::empty())
        .await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let cookie = server.sign_in().await;
    let response = server
        .send(
            browser("GET", "/api/live").header(header::COOKIE, &cookie),
            Body::empty(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream"
    );
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    let mut body = response.into_body();
    read_until(&mut body, "retry:").await;

    // A new folder in Alex's top folder is a hint naming it.
    let top = json(
        server
            .send(
                browser("GET", "/api/drive/top").header(header::COOKIE, &cookie),
                Body::empty(),
            )
            .await,
    )
    .await;
    let top = top["id"].as_str().unwrap().to_owned();
    let created = server
        .send(
            browser("POST", &format!("/api/drive/items/{top}/children"))
                .header(header::COOKIE, &cookie),
            json_body(json!({ "name": "Recipes" })),
        )
        .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let read = read_until(&mut body, &top).await;
    assert!(read.contains("event: drive.items"), "{read}");

    // Signing out ends the stream.
    let signed_out = server
        .send(
            browser("DELETE", "/api/session").header(header::COOKIE, &cookie),
            Body::empty(),
        )
        .await;
    assert_eq!(signed_out.status(), StatusCode::NO_CONTENT);
    let ended = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(frame) = body.frame().await {
            frame.unwrap();
        }
    })
    .await;
    assert!(ended.is_ok(), "the stream is still open");
}

/// RFC 7636's example PKCE secret, and its challenge.
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
const REDIRECT: &str = "http://127.0.0.1:53412/callback";

fn authorize_query(client_id: &str, redirect_uri: &str) -> String {
    format!(
        "response_type=code&client_id={client_id}&redirect_uri={}&code_challenge={CHALLENGE}\
         &code_challenge_method=S256",
        redirect_uri.replace(':', "%3A").replace('/', "%2F")
    )
}

fn token_form(code: &str, verifier: &str) -> Body {
    Body::from(format!(
        "grant_type=authorization_code&code={code}&client_id=accordion-sync\
         &redirect_uri={}&code_verifier={verifier}",
        REDIRECT.replace(':', "%3A").replace('/', "%2F")
    ))
}

#[tokio::test]
async fn an_app_signs_in_through_the_browser() {
    let server = start().await;

    // The sign-in page is a page of the web app, which this test server
    // doesn't have, so it says so.
    let page = server
        .send(
            browser(
                "GET",
                &format!(
                    "/oauth/authorize?{}",
                    authorize_query("accordion-sync", REDIRECT)
                ),
            ),
            Body::empty(),
        )
        .await;
    let bytes = page.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&bytes).contains("doesn't have the web app"));

    // The page checks the request once someone is signed in.
    let check = format!(
        "/api/oauth/check?{}",
        authorize_query("accordion-sync", REDIRECT)
    );
    let response = server.send(browser("GET", &check), Body::empty()).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let cookie = server.sign_in().await;
    let response = server
        .send(
            browser("GET", &check).header(header::COOKIE, &cookie),
            Body::empty(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Requests from other apps, or for other addresses, never get a code.
    for (client, redirect) in [
        ("someone-else", REDIRECT),
        ("accordion-sync", "https://evil.example/callback"),
        ("accordion-sync", "http://localhost:53412/callback"),
    ] {
        let response = server
            .send(
                browser(
                    "GET",
                    &format!("/api/oauth/check?{}", authorize_query(client, redirect)),
                )
                .header(header::COOKIE, &cookie),
                Body::empty(),
            )
            .await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{client} {redirect}"
        );
        let response = server
            .send(
                browser("POST", "/api/oauth/approve").header(header::COOKIE, &cookie),
                json_body(json!({
                    "response_type": "code",
                    "client_id": client,
                    "redirect_uri": redirect,
                    "code_challenge": CHALLENGE,
                    "code_challenge_method": "S256",
                    "device_name": "Alex's laptop",
                })),
            )
            .await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{client} {redirect}"
        );
    }

    // Allowing the app sends the browser to it, with a code and its state.
    let approve = || {
        json_body(json!({
            "response_type": "code",
            "client_id": "accordion-sync",
            "redirect_uri": REDIRECT,
            "code_challenge": CHALLENGE,
            "code_challenge_method": "S256",
            "state": "xyz 1",
            "device_name": "Alex's laptop",
        }))
    };
    let response = server
        .send(
            browser("POST", "/api/oauth/approve").header(header::COOKIE, &cookie),
            approve(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let redirect = json(response).await["redirect"]
        .as_str()
        .unwrap()
        .to_owned();
    let rest = redirect
        .strip_prefix(&format!("{REDIRECT}?code="))
        .unwrap_or_else(|| panic!("{redirect}"));
    let (code, state) = rest.split_once("&state=").unwrap();
    assert_eq!(state, "xyz%201");

    // The code needs the app's secret, and works once.
    let token = |code: &str, verifier: &str| {
        Request::builder()
            .method("POST")
            .uri("/api/oauth/token")
            .header(header::HOST, HOST)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(token_form(code, verifier))
            .unwrap()
    };
    let response = server
        .app
        .clone()
        .oneshot(token(code, VERIFIER))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let signed_in = json(response).await;
    assert_eq!(signed_in["token_type"], "Bearer");
    assert_eq!(signed_in["username"], "alex");
    let session = signed_in["access_token"].as_str().unwrap().to_owned();
    let response = server
        .send(
            app("GET", "/api/drive/sync/start")
                .header(header::AUTHORIZATION, format!("Bearer {session}")),
            Body::empty(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = server
        .app
        .clone()
        .oneshot(token(code, VERIFIER))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(response).await["error"], "invalid_grant");

    let response = server
        .send(
            browser("POST", "/api/oauth/approve").header(header::COOKIE, &cookie),
            approve(),
        )
        .await;
    let redirect = json(response).await["redirect"]
        .as_str()
        .unwrap()
        .to_owned();
    let code = redirect
        .split("code=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_owned();
    let response = server
        .app
        .clone()
        .oneshot(token(&code, &"a".repeat(43)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // It shows where Alex is signed in, as an app.
    let listed = json(
        server
            .send(
                browser("GET", "/api/sessions").header(header::COOKIE, &cookie),
                Body::empty(),
            )
            .await,
    )
    .await;
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|session| session["device_name"] == "Alex's laptop" && session["client"] == "app"),
        "{listed}"
    );
}
