//! Runs the server's commands the way an admin would, in a terminal.

use std::process::{Command, Output};

use accordion_core::db::Database;
use accordion_core::identity::{
    AppPasswordUse, AuthMethod, Client, Error, Identity, NewUser, SignInFrom,
};
use accordion_types::Role;

const HOME: SignInFrom = SignInFrom {
    address: std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 30)),
    home_network: true,
};

fn accordion(data: &tempfile::TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_accordion"))
        .args(args)
        .env("ACCORDION_DATA_DIR", data.path())
        .env("ACCORDION_LISTEN", "0.0.0.0:9000")
        .env_remove("ACCORDION_HTTPS_LISTEN")
        .output()
        .unwrap()
}

async fn identity(data: &tempfile::TempDir) -> Identity {
    let db = Database::open(&data.path().join("accordion.db")).unwrap();
    Identity::new(db).await.unwrap()
}

#[tokio::test]
async fn the_owner_resets_their_password_from_the_server() {
    let data = tempfile::tempdir().unwrap();
    let identity = identity(&data).await;
    let alex = identity
        .create_user(NewUser {
            username: "alex".into(),
            display_name: "Alex Kim".into(),
            role: Role::Owner,
            password: "correct horse".into(),
        })
        .await
        .unwrap();
    let (session, _) = identity
        .create_session(alex.id, Client::Browser, "Test", AuthMethod::Password)
        .await
        .unwrap();
    let (app_password, _) = identity
        .create_app_password(alex.id, AppPasswordUse::Dav, "Laptop")
        .await
        .unwrap();

    let output = accordion(&data, &["reset-owner-password"]);
    assert!(output.status.success(), "{output:?}");
    let link = String::from_utf8(output.stdout).unwrap();
    let token = link
        .trim()
        .strip_prefix("http://localhost:9000/welcome#")
        .unwrap_or_else(|| panic!("unexpected link: {link}"));

    assert!(matches!(
        identity.check_password("alex", "correct horse", HOME).await,
        Err(Error::WrongCredentials)
    ));
    assert!(
        identity
            .check_session(session.as_str())
            .await
            .unwrap()
            .is_none()
    );
    identity
        .use_password_link(token, "battery staple".into())
        .await
        .unwrap();
    identity
        .check_password("alex", "battery staple", HOME)
        .await
        .unwrap();
    // They forgot it, so their devices stay connected.
    assert!(
        identity
            .check_app_password("alex", app_password.as_str(), AppPasswordUse::Dav)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn theres_nothing_to_reset_without_an_owner() {
    let data = tempfile::tempdir().unwrap();
    let output = accordion(&data, &["reset-owner-password"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let message = String::from_utf8(output.stderr).unwrap();
    assert!(message.contains("doesn't have an owner yet"), "{message}");
}
