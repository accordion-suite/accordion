//! Manages the people on a server over HTTP, the way the people, welcome,
//! and account pages do.

use accordion_core::db::Database;
use accordion_core::identity::{AppPasswordUse, AuthMethod, Client, Identity, NewUser};
use accordion_core::storage::Storage;
use accordion_gateway::Peer;
use accordion_server::health::Health;
use accordion_server::network::AdminAccess;
use accordion_server::{AppState, Site, router};
use accordion_types::Role;
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const HOST: &str = "accordion.example.com";
const ORIGIN: &str = "https://accordion.example.com";

struct Server {
    data: tempfile::TempDir,
    identity: Identity,
    storage: Storage,
    app: Router,
    away: Router,
    owner: String,
}

/// Starts a server with an owner, signed in from the home network.
async fn start() -> Server {
    let data = tempfile::tempdir().unwrap();
    let db = Database::open(&data.path().join("accordion.db")).unwrap();
    let identity = Identity::new(db.clone()).await.unwrap();
    let storage = Storage::open(db, identity.clone(), &data.path().join("storage"))
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
        .unwrap();
    let app = router(
        AppState {
            identity: identity.clone(),
            setup: None,
            health: Health::default(),
            admin_access: AdminAccess::HomeNetwork,
            modules: Default::default(),
            backup: None,
            storage: Some(storage.clone()),
            updates: None,
        },
        &Site::named(HOST.parse().unwrap(), 443),
    );
    let from = |client: &str| {
        MockConnectInfo(Peer {
            local: "192.168.1.20:443".parse().unwrap(),
            remote: format!("{client}:50000").parse().unwrap(),
        })
    };
    let server = Server {
        data,
        storage,
        away: app.clone().layer(from("203.0.113.5")),
        app: app.layer(from("192.168.1.30")),
        identity,
        owner: String::new(),
    };
    let owner = server.session_for(alex.id).await;
    Server { owner, ..server }
}

impl Server {
    async fn session_for(&self, id: accordion_types::UserId) -> String {
        let (token, _) = self
            .identity
            .create_session(id, Client::Browser, "Test", AuthMethod::Password)
            .await
            .unwrap();
        token.as_str().to_owned()
    }

    async fn send(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        send(&self.app, method, path, token, body).await
    }

    /// Adds someone as `by`, has them set their password from the link, and
    /// returns a session for them.
    async fn add(&self, by: &str, username: &str, role: &str) -> String {
        let (status, link) = self
            .send(
                "POST",
                "/api/people",
                Some(by),
                Some(json!({ "username": username, "display_name": username, "role": role })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{link}");
        let (status, _) = self
            .send(
                "POST",
                "/api/welcome",
                None,
                Some(json!({ "token": link["token"], "password": "battery staple" })),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let user = self
            .identity
            .user_by_username(username)
            .await
            .unwrap()
            .unwrap();
        self.session_for(user.id).await
    }

    async fn user_id(&self, username: &str) -> accordion_types::UserId {
        let user = self
            .identity
            .user_by_username(username)
            .await
            .unwrap()
            .unwrap();
        user.id
    }

    async fn id_of(&self, username: &str) -> String {
        self.user_id(username).await.to_string()
    }

    async fn signed_in(&self, token: &str) -> bool {
        self.send("GET", "/api/session", Some(token), None).await.0 == StatusCode::OK
    }
}

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let request = request
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn error(body: &Value) -> &str {
    body["error"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn new_people_set_their_own_password_from_a_link() {
    let server = start().await;
    let (status, link) = server
        .send(
            "POST",
            "/api/people",
            Some(&server.owner),
            Some(json!({ "username": "Sam", "display_name": "Sam Lee", "role": "user" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(link["purpose"], "new_account");
    let token = link["token"].as_str().unwrap().to_owned();

    let (status, people) = server
        .send("GET", "/api/people", Some(&server.owner), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(people["can_add_admins"], true);
    let sam = &people["people"][1];
    assert_eq!(
        (&sam["username"], &sam["state"], &sam["can_manage"]),
        (&json!("sam"), &json!("waiting"), &json!(true))
    );
    assert_eq!(sam["password_link"]["purpose"], "new_account");
    // The owner can't change their own account here.
    assert_eq!(people["people"][0]["can_manage"], false);

    // The welcome page needs no sign-in, and works from anywhere.
    let (status, found) = send(
        &server.away,
        "POST",
        "/api/welcome/check",
        None,
        Some(json!({ "token": token })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        found,
        json!({ "username": "sam", "display_name": "Sam Lee", "purpose": "new_account" })
    );
    let (status, body) = server
        .send(
            "POST",
            "/api/welcome",
            None,
            Some(json!({ "token": token, "password": "short" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(error(&body).contains("at least 8"));
    let (status, body) = server
        .send(
            "POST",
            "/api/welcome",
            None,
            Some(json!({ "token": token, "password": "battery staple" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["user"]["username"], "sam");

    // Signed in, and the password works from the sign-in page too.
    let (status, _) = server
        .send(
            "POST",
            "/api/session",
            None,
            Some(json!({ "username": "sam", "password": "battery staple" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    for path in ["/api/welcome/check", "/api/welcome"] {
        let (status, body) = server
            .send(
                "POST",
                path,
                None,
                Some(json!({ "token": token, "password": "battery staple" })),
            )
            .await;
        assert_eq!(status, StatusCode::GONE, "{path}");
        assert!(error(&body).contains("expired or was already used"));
    }
}

#[tokio::test]
async fn admins_manage_standard_accounts_and_only_the_owner_manages_admins() {
    let server = start().await;
    let admin = server.add(&server.owner, "jo", "admin").await;
    let sam = server.add(&admin, "sam", "user").await;
    let owner_id = server.id_of("alex").await;
    let jo_id = server.id_of("jo").await;
    let sam_id = server.id_of("sam").await;

    let (_, people) = server.send("GET", "/api/people", Some(&admin), None).await;
    assert_eq!(people["can_add_admins"], false);
    let manageable: Vec<_> = people["people"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["username"].as_str().unwrap(),
                p["can_manage"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(manageable, [("alex", false), ("jo", false), ("sam", true)]);

    let (status, body) = server
        .send(
            "POST",
            "/api/people",
            Some(&admin),
            Some(json!({ "username": "kim", "display_name": "Kim", "role": "admin" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(error(&body).contains("Only the owner"));
    for path in [
        format!("/api/people/{owner_id}/password-link"),
        format!("/api/people/{owner_id}/turn-off"),
        format!("/api/people/{jo_id}/turn-off"),
    ] {
        let (status, _) = server
            .send("POST", &path, Some(&admin), Some(json!({})))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    let (status, _) = server
        .send(
            "PUT",
            &format!("/api/people/{sam_id}/role"),
            Some(&admin),
            Some(json!({ "role": "admin" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A reset signs Sam out everywhere until the new link is used.
    let (status, link) = server
        .send(
            "POST",
            &format!("/api/people/{sam_id}/password-link"),
            Some(&admin),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(link["purpose"], "reset");
    assert!(!server.signed_in(&sam).await);
    let (status, _) = server
        .send(
            "POST",
            "/api/session",
            None,
            Some(json!({ "username": "sam", "password": "battery staple" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = server
        .send(
            "POST",
            "/api/welcome",
            None,
            Some(json!({ "token": link["token"], "password": "new staple" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Turning Sam off signs them out, and their password stops working.
    let sam = server.session_for(server.user_id("sam").await).await;
    let (status, _) = server
        .send(
            "POST",
            &format!("/api/people/{sam_id}/turn-off"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!server.signed_in(&sam).await);
    let (status, body) = server
        .send(
            "POST",
            "/api/session",
            None,
            Some(json!({ "username": "sam", "password": "new staple" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(error(&body).contains("turned off"));
    let (_, people) = server.send("GET", "/api/people", Some(&admin), None).await;
    assert_eq!(people["people"][2]["state"], "turned_off");
    let (status, _) = server
        .send(
            "POST",
            &format!("/api/people/{sam_id}/turn-on"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = server
        .send(
            "POST",
            "/api/session",
            None,
            Some(json!({ "username": "sam", "password": "new staple" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // The owner manages admins, but not their own account.
    let (status, _) = server
        .send(
            "PUT",
            &format!("/api/people/{jo_id}/role"),
            Some(&server.owner),
            Some(json!({ "role": "user" })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        server
            .identity
            .user_by_username("jo")
            .await
            .unwrap()
            .unwrap()
            .role,
        Role::User
    );
    let (status, body) = server
        .send(
            "POST",
            &format!("/api/people/{owner_id}/turn-off"),
            Some(&server.owner),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(error(&body).contains("your own account"));
    // Jo is a standard account now, so the page is closed to them.
    let (status, _) = server.send("GET", "/api/people", Some(&admin), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn only_admins_at_home_manage_people() {
    let server = start().await;
    let sam = server.add(&server.owner, "sam", "user").await;
    let (status, body) = server.send("GET", "/api/people", Some(&sam), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(error(&body).contains("Only admins"));
    let (status, _) = server
        .send(
            "POST",
            "/api/people",
            Some(&sam),
            Some(json!({ "username": "kim", "display_name": "Kim", "role": "user" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &server.away,
        "GET",
        "/api/people",
        Some(&server.owner),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(&server.away, "GET", "/people", None, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(&server.away, "GET", "/welcome", None, None).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = server
        .send(
            "POST",
            "/api/people",
            Some(&server.owner),
            Some(json!({ "username": "Sam", "display_name": "Sam Again", "role": "user" })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(error(&body).contains("already taken"));
    let (status, _) = server
        .send(
            "POST",
            "/api/people",
            Some(&server.owner),
            Some(json!({ "username": "kim", "display_name": "Kim", "role": "owner" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn people_change_their_own_password() {
    let server = start().await;
    let sam = server.add(&server.owner, "sam", "user").await;
    let other_browser = server.session_for(server.user_id("sam").await).await;

    let (status, body) = server
        .send(
            "POST",
            "/api/account/password",
            Some(&sam),
            Some(json!({ "current_password": "wrong horse", "new_password": "new staple" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(error(&body).contains("current password isn't right"));
    let (status, _) = server
        .send(
            "POST",
            "/api/account/password",
            Some(&sam),
            Some(json!({ "current_password": "battery staple", "new_password": "new staple" })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(server.signed_in(&sam).await);
    assert!(!server.signed_in(&other_browser).await);
    let (status, _) = server
        .send(
            "POST",
            "/api/session",
            None,
            Some(json!({ "username": "sam", "password": "new staple" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_reset_can_disconnect_every_device() {
    let server = start().await;
    server.add(&server.owner, "sam", "user").await;
    let sam = server.user_id("sam").await;
    let (secret, _) = server
        .identity
        .create_app_password(sam, AppPasswordUse::Dav, "Laptop")
        .await
        .unwrap();
    let connected = async || {
        server
            .identity
            .check_app_password("sam", secret.as_str(), AppPasswordUse::Dav)
            .await
            .unwrap()
            .is_some()
    };
    let path = format!("/api/people/{sam}/password-link");

    let (status, _) = server
        .send("POST", &path, Some(&server.owner), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(connected().await, "a reset keeps devices connected");

    let (status, link) = server
        .send(
            "POST",
            &path,
            Some(&server.owner),
            Some(json!({ "delete_app_passwords": true })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(link["purpose"], "reset");
    assert!(!connected().await, "the app password was deleted");
}

impl Server {
    async fn sign_in(&self, username: &str, password: &str) -> StatusCode {
        self.send(
            "POST",
            "/api/session",
            None,
            Some(json!({ "username": username, "password": password })),
        )
        .await
        .0
    }

    /// Gives `username` a file in their top folder.
    async fn give_file(&self, username: &str, name: &str) {
        let top = self
            .storage
            .top_folder(self.user_id(username).await)
            .await
            .unwrap();
        self.storage
            .write_file(top, name, &b"contents"[..])
            .await
            .unwrap();
    }

    /// The names in `username`'s top folder.
    async fn top_names(&self, username: &str) -> Vec<String> {
        let top = self
            .storage
            .top_folder(self.user_id(username).await)
            .await
            .unwrap();
        let children = self.storage.children(top).await.unwrap();
        children.into_iter().map(|item| item.name).collect()
    }
}

#[tokio::test]
async fn admins_rename_people() {
    let server = start().await;
    let admin = server.add(&server.owner, "jo", "admin").await;
    server.add(&admin, "sam", "user").await;
    let sam_id = server.id_of("sam").await;
    let jo_id = server.id_of("jo").await;
    let owner_id = server.id_of("alex").await;
    server.give_file("sam", "notes.txt").await;
    let rename = |id: &str| format!("/api/people/{id}/name");

    let (status, _) = server
        .send(
            "PUT",
            &rename(&sam_id),
            Some(&admin),
            Some(json!({ "username": "Samuel", "display_name": "Samuel Lee" })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let sam = server
        .identity
        .user_by_username("samuel")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sam.display_name, "Samuel Lee");
    assert_eq!(
        server.sign_in("samuel", "battery staple").await,
        StatusCode::OK
    );
    assert_eq!(
        server.sign_in("sam", "battery staple").await,
        StatusCode::UNAUTHORIZED
    );
    // Their folder in the plain view follows the new username.
    let view = server.data.path().join("storage/files");
    assert!(view.join("samuel/notes.txt").exists());
    assert!(!view.join("sam").exists());

    let (status, body) = server
        .send(
            "PUT",
            &rename(&sam_id),
            Some(&admin),
            Some(json!({ "username": "jo", "display_name": "Sam" })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(error(&body).contains("already taken"));
    // The same rules as the page's other changes: admins can't rename an
    // admin or the owner, and nobody renames themselves here.
    for (id, by) in [
        (&jo_id, &admin),
        (&owner_id, &admin),
        (&owner_id, &server.owner),
    ] {
        let (status, _) = server
            .send(
                "PUT",
                &rename(id),
                Some(by),
                Some(json!({ "username": "new", "display_name": "New" })),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{id}");
    }
    let (status, _) = server
        .send(
            "PUT",
            &rename(&jo_id),
            Some(&server.owner),
            Some(json!({ "username": "jo", "display_name": "Jo Park" })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn deleting_someone_can_give_their_files_to_someone_else() {
    let server = start().await;
    let sam = server.add(&server.owner, "sam", "user").await;
    let sam_id = server.id_of("sam").await;
    server.give_file("sam", "notes.txt").await;
    let path = format!("/api/people/{sam_id}");
    let owner_id = server.id_of("alex").await;

    // Nothing happens without the username typed, or a choice about the files.
    let (status, body) = server
        .send(
            "DELETE",
            &path,
            Some(&server.owner),
            Some(json!({ "username": "kim", "files": { "give_to": owner_id } })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        error(&body).contains("type sam's username exactly: sam"),
        "{body}"
    );
    let (status, _) = server
        .send(
            "DELETE",
            &path,
            Some(&server.owner),
            Some(json!({ "username": "sam" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(server.signed_in(&sam).await);

    let (status, body) = server
        .send(
            "DELETE",
            &path,
            Some(&server.owner),
            Some(json!({ "username": " Sam ", "files": { "give_to": owner_id } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["folder"], "sam's files");
    assert!(!server.signed_in(&sam).await);
    assert_eq!(server.identity.user_by_username("sam").await.unwrap(), None);
    assert_eq!(server.top_names("alex").await, ["sam's files"]);
    let (_, people) = server
        .send("GET", "/api/people", Some(&server.owner), None)
        .await;
    assert_eq!(people["people"].as_array().unwrap().len(), 1);
    assert_eq!(
        server.sign_in("sam", "battery staple").await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn deleting_someone_can_delete_their_files() {
    let server = start().await;
    server.add(&server.owner, "sam", "user").await;
    let sam = server.user_id("sam").await;
    server.give_file("sam", "notes.txt").await;
    let top = server.storage.top_folder(sam).await.unwrap();

    let (status, body) = server
        .send(
            "DELETE",
            &format!("/api/people/{sam}"),
            Some(&server.owner),
            Some(json!({ "username": "sam", "files": "delete" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["folder"], Value::Null);
    assert_eq!(server.storage.item(top).await.unwrap(), None);
    assert!(server.top_names("alex").await.is_empty());
}

#[tokio::test]
async fn only_the_owner_deletes_admins_and_nobody_deletes_the_owner() {
    let server = start().await;
    let admin = server.add(&server.owner, "jo", "admin").await;
    let jo_id = server.id_of("jo").await;
    let owner_id = server.id_of("alex").await;
    let delete = |username: &str| Some(json!({ "username": username, "files": "delete" }));

    for (id, username, by) in [
        (&jo_id, "jo", &admin),
        (&owner_id, "alex", &admin),
        (&owner_id, "alex", &server.owner),
    ] {
        let (status, _) = server
            .send(
                "DELETE",
                &format!("/api/people/{id}"),
                Some(by),
                delete(username),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{username}");
    }
    assert!(server.signed_in(&admin).await);
    let (status, _) = server
        .send(
            "DELETE",
            &format!("/api/people/{jo_id}"),
            Some(&server.owner),
            delete("jo"),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!server.signed_in(&admin).await);
}

#[tokio::test]
async fn people_change_their_own_name() {
    let server = start().await;
    let sam = server.add(&server.owner, "sam", "user").await;
    let rename = |body| server.send("PUT", "/api/account/name", Some(&sam), Some(body));

    // A new name needs no password.
    let (status, _) = rename(json!({ "username": "sam", "display_name": "Sam Lee" })).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        server
            .identity
            .user_by_username("sam")
            .await
            .unwrap()
            .unwrap()
            .display_name,
        "Sam Lee"
    );

    // A new username, which everyone signs in with, does.
    let (status, body) = rename(json!({ "username": "samuel", "display_name": "Sam Lee" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(error(&body).contains("current password isn't right"));
    let (status, _) = rename(json!({
        "username": "samuel",
        "display_name": "Sam Lee",
        "current_password": "battery staple",
    }))
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(server.signed_in(&sam).await, "the browser stays signed in");
    assert_eq!(
        server.sign_in("samuel", "battery staple").await,
        StatusCode::OK
    );

    // The owner renames themselves here, since the People page can't.
    let (status, _) = server
        .send(
            "PUT",
            "/api/account/name",
            Some(&server.owner),
            Some(json!({ "username": "alex", "display_name": "Alex" })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}
