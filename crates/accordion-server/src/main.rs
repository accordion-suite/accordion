use std::error::Error;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use accordion_core::db::Database;
use accordion_core::identity::{Identity, NewUser};
use accordion_server::{AppState, router};
use accordion_types::Role;
use tracing_subscriber::EnvFilter;

const USAGE: &str = "\
Usage:
  accordion                                     Start the server
  accordion create-owner <username> <name>      Create the owner account

Settings, from environment variables:
  ACCORDION_DATA_DIR   Where Accordion keeps its data (default: ./data)
  ACCORDION_LISTEN     Address to listen on (default: 127.0.0.1:8080)
  RUST_LOG             Log detail, such as info or debug (default: info)";

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        // Colors only on a terminal, so logs in files and journald stay plain.
        .with_ansi(io::stdout().is_terminal())
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => serve().await,
        ["create-owner", username, display_name] => create_owner(username, display_name).await,
        ["--help" | "-h" | "help"] => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

async fn serve() -> Result<(), Box<dyn Error>> {
    let identity = start_identity().await?;
    let listen = std::env::var("ACCORDION_LISTEN").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .map_err(|err| format!("Accordion couldn't listen on {listen}: {err}"))?;
    tracing::info!("listening on http://{}", listener.local_addr()?);

    axum::serve(listener, router(AppState { identity }))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("stopped");
    Ok(())
}

/// Creates the owner account from the command line.
///
/// This stands in for the setup wizard until it exists. The password is read
/// from standard input, so it can be piped in rather than typed, because
/// typing shows it on screen.
async fn create_owner(username: &str, display_name: &str) -> Result<(), Box<dyn Error>> {
    let identity = start_identity().await?;
    eprint!("Password for {username}: ");
    io::stderr().flush()?;
    let mut password = String::new();
    io::stdin().lock().read_line(&mut password)?;
    let password = password.trim_end_matches(['\r', '\n']).to_owned();

    let user = identity
        .create_user(NewUser {
            username: username.to_owned(),
            display_name: display_name.to_owned(),
            role: Role::Owner,
            password,
        })
        .await?;
    eprintln!("Created the owner account {}.", user.username);
    Ok(())
}

async fn start_identity() -> Result<Identity, Box<dyn Error>> {
    let data_dir =
        PathBuf::from(std::env::var_os("ACCORDION_DATA_DIR").unwrap_or_else(|| "data".into()));
    std::fs::create_dir_all(&data_dir).map_err(|err| {
        format!(
            "Accordion couldn't create its data folder {}: {err}",
            data_dir.display()
        )
    })?;
    let db = Database::open(&data_dir.join("accordion.db"))?;
    Ok(Identity::new(db).await?)
}

/// Waits for Ctrl+C, or for SIGTERM from systemd or Docker.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("couldn't listen for Ctrl+C");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("couldn't listen for SIGTERM")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
