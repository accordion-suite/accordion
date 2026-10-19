use std::error::Error;
use std::future::IntoFuture;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use accordion_core::db::Database;
use accordion_core::identity::{Identity, NewUser};
use accordion_gateway::ServerName;
use accordion_gateway::certificates::{Authority, Renewer, Settings};
use accordion_gateway::dns::ProviderKind;
use accordion_gateway::redirect::redirect_to_https;
use accordion_gateway::tls::{self, CertStore};
use accordion_server::{AppState, Site, router};
use accordion_types::Role;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

const USAGE: &str = "\
Usage:
  accordion                                    Start the server
  accordion create-owner <username> <name>     Create the owner account
  accordion set-name <name> <dns-provider>     Set the server's name and get its
                                               certificate. The DNS provider is
                                               desec or cloudflare. Add --staging
                                               to test with a certificate that
                                               browsers don't trust.

Settings, from environment variables:
  ACCORDION_DATA_DIR      Where Accordion keeps its data (default: ./data)
  ACCORDION_LISTEN        Address for plain HTTP, used until a name is set
                          (default: 127.0.0.1:8080)
  ACCORDION_HTTPS_LISTEN  Address for HTTPS once a name is set (default: port 443)
  ACCORDION_HTTP_LISTEN   Address for the redirect to HTTPS (default: port 80)
  RUST_LOG                Log detail, such as info or debug (default: info)";

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
        ["set-name", name, provider] => set_name(name, provider, Authority::LetsEncrypt).await,
        ["set-name", name, provider, "--staging"] => {
            set_name(name, provider, Authority::LetsEncryptStaging).await
        }
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
    let data_dir = data_dir()?;
    let state = AppState {
        identity: start_identity(&data_dir).await?,
    };
    let gateway_dir = gateway_dir(&data_dir);
    match Settings::load(&gateway_dir)? {
        Some(settings) => serve_https(state, &gateway_dir, settings).await,
        None => serve_http(state).await,
    }
}

/// Serves plain HTTP, for development and until the server has a name.
async fn serve_http(state: AppState) -> Result<(), Box<dyn Error>> {
    let listen = std::env::var("ACCORDION_LISTEN").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let listener = bind(&listen, "ACCORDION_LISTEN").await?;
    tracing::info!(
        "no server name is set, so serving plain HTTP on http://{}",
        listener.local_addr()?
    );

    axum::serve(listener, router(state, &Site::Unnamed))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("stopped");
    Ok(())
}

/// Serves HTTPS for the server's name, redirects plain HTTP there, and keeps
/// the certificate current.
async fn serve_https(
    state: AppState,
    gateway_dir: &Path,
    settings: Settings,
) -> Result<(), Box<dyn Error>> {
    let https = listen("ACCORDION_HTTPS_LISTEN", 443).await?;
    let http = listen("ACCORDION_HTTP_LISTEN", 80).await?;
    let https_port = https.local_addr()?.port();
    let site = Site::named(settings.name.clone(), https_port);
    let certs = CertStore::new();
    let renewer = Renewer::from_settings(gateway_dir, &settings, certs.clone())?;
    tracing::info!(
        "serving https://{} on {}, and redirecting plain HTTP on {}",
        settings.name,
        https.local_addr()?,
        http.local_addr()?
    );

    let (stop, stopped) = watch::channel(false);
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = stop.send(true);
    });
    let until_stopped = || {
        let mut stopped = stopped.clone();
        async move {
            let _ = stopped.wait_for(|&stopped| stopped).await;
        }
    };

    let (_, redirected, ()) = tokio::join!(
        tls::serve(
            https,
            tls::server_config(certs),
            router(state, &site),
            until_stopped(),
        ),
        axum::serve(http, redirect_to_https(settings.name, https_port))
            .with_graceful_shutdown(until_stopped())
            .into_future(),
        renewer.run(until_stopped()),
    );
    redirected?;
    tracing::info!("stopped");
    Ok(())
}

/// Creates the owner account from the command line.
///
/// This stands in for the setup wizard until it exists. The password is read
/// from standard input, so it can be piped in rather than typed, because
/// typing shows it on screen.
async fn create_owner(username: &str, display_name: &str) -> Result<(), Box<dyn Error>> {
    let identity = start_identity(&data_dir()?).await?;
    let password = read_line(&format!("Password for {username}: "))?;

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

/// Sets the server's name and DNS provider, and gets its first certificate.
///
/// This stands in for the setup wizard's name step until it exists. The API
/// token is read from standard input, like the owner's password. The settings
/// are saved only after a certificate arrives, so a mistyped token isn't kept.
async fn set_name(name: &str, provider: &str, authority: Authority) -> Result<(), Box<dyn Error>> {
    let name: ServerName = name.parse()?;
    let provider: ProviderKind = provider.parse()?;
    let gateway_dir = gateway_dir(&data_dir()?);
    if let Some(current) = Settings::load(&gateway_dir)?
        && current.name != name
    {
        eprintln!("This replaces the server's current name, {}.", current.name);
    }

    eprintln!(
        "Accordion gets its certificate from Let's Encrypt. Continuing means agreeing to the Let's \
         Encrypt Subscriber Agreement: https://letsencrypt.org/repository/"
    );
    let token = read_line(&format!("{provider} API token: "))?;
    let token = token.trim().to_owned();
    if token.is_empty() {
        return Err(format!(
            "Accordion needs an API token from {provider}. Create one, then run this command again."
        )
        .into());
    }

    let settings = Settings {
        name,
        provider,
        token,
        authority,
    };
    let renewer = Renewer::from_settings(&gateway_dir, &settings, CertStore::new())?;
    eprintln!(
        "Getting a certificate for {}. This can take a few minutes while the DNS change reaches \
         all of {provider}'s nameservers.",
        settings.name
    );
    renewer.renew().await?;
    settings.save(&gateway_dir)?;
    eprintln!(
        "Got a certificate for {name}. Start Accordion, or restart it if it's running, to serve \
         https://{name}.",
        name = settings.name
    );
    Ok(())
}

/// Shows `prompt` and reads one line from standard input.
fn read_line(prompt: &str) -> io::Result<String> {
    eprint!("{prompt}");
    io::stderr().flush()?;
    let mut line = String::new();
    let stdin = io::stdin();
    stdin.lock().read_line(&mut line)?;
    // Piped input isn't echoed, so end the prompt's line here instead.
    if !stdin.is_terminal() {
        eprintln!();
    }
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

fn data_dir() -> Result<PathBuf, Box<dyn Error>> {
    let data_dir =
        PathBuf::from(std::env::var_os("ACCORDION_DATA_DIR").unwrap_or_else(|| "data".into()));
    std::fs::create_dir_all(&data_dir).map_err(|err| {
        format!(
            "Accordion couldn't create its data folder {}: {err}",
            data_dir.display()
        )
    })?;
    Ok(data_dir)
}

/// The gateway keeps the server's name, DNS token, and certificate here.
fn gateway_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("gateway")
}

async fn start_identity(data_dir: &Path) -> Result<Identity, Box<dyn Error>> {
    let db = Database::open(&data_dir.join("accordion.db"))?;
    Ok(Identity::new(db).await?)
}

/// Listens on the address in `variable`, or on `port` at every address.
async fn listen(variable: &str, port: u16) -> Result<TcpListener, String> {
    if let Ok(address) = std::env::var(variable) {
        return bind(&address, variable).await;
    }
    // On Linux, listening on [::] accepts IPv4 connections too. It fails
    // where IPv6 is turned off, so fall back to IPv4 alone.
    match TcpListener::bind(("::", port)).await {
        Ok(listener) => Ok(listener),
        Err(_) => bind(&format!("0.0.0.0:{port}"), variable).await,
    }
}

async fn bind(address: &str, variable: &str) -> Result<TcpListener, String> {
    TcpListener::bind(address).await.map_err(|err| {
        let next = match err.kind() {
            io::ErrorKind::PermissionDenied => format!(
                " Ports below 1024 need the CAP_NET_BIND_SERVICE capability. Grant it to \
                 Accordion, or set {variable} to a higher port."
            ),
            io::ErrorKind::AddrInUse => format!(
                " Another program is using that port. Stop it, or set {variable} to another \
                 address."
            ),
            _ => String::new(),
        };
        format!("Accordion couldn't listen on {address}: {err}.{next}")
    })
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
