use std::error::Error;
use std::future::IntoFuture;
use std::io::{self, BufRead, IsTerminal, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use accordion_core::backup::{self, Backup};
use accordion_core::db::Database;
use accordion_core::identity::{AppPasswords, Identity, NewUser};
use accordion_core::storage::Storage;
use accordion_gateway::address::{AddressPublisher, AddressSource};
use accordion_gateway::certificates::{Authority, Renewer, Settings};
use accordion_gateway::dns::ProviderKind;
use accordion_gateway::local_name::{LOCAL_NAME, LocalName, LocalNameStatus};
use accordion_gateway::redirect::redirect_to_https;
use accordion_gateway::tls::{self, CertStore};
use accordion_gateway::{Peer, ServerName};
use accordion_module::{Services, Stopping};
use accordion_server::connections::Switches;
use accordion_server::health::{Health, NameHealth};
use accordion_server::network::AdminAccess;
use accordion_server::setup::{self, SETUP_CODE_FILE, Setup, SetupOptions};
use accordion_server::updates::{UpdateSource, Updates};
use accordion_server::web::WebApp;
use accordion_server::{AppState, Site, modules, router};
use accordion_types::Role;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tracing_subscriber::EnvFilter;

const USAGE: &str = "\
Usage:
  accordion                                    Start the server. Until it has an
                                               owner, it serves the setup wizard
                                               at http://accordion.local.
  accordion setup-code                         Show the setup wizard's code
  accordion create-owner <username> <name>     Create the owner account
  accordion reset-owner-password               Make a link for the owner to
                                               choose a new password
  accordion set-name <name> <dns-provider>     Set the server's name and get its
                                               certificate. The DNS provider is
                                               desec or cloudflare. Add --staging
                                               to test with a certificate that
                                               browsers don't trust.

Settings, from environment variables:
  ACCORDION_DATA_DIR      Where Accordion keeps its data (default: ./data)
  ACCORDION_LISTEN        Address for plain HTTP, used for development when
                          there's an owner but no name (default: 127.0.0.1:8080)
  ACCORDION_HTTPS_LISTEN  Address for HTTPS once a name is set (default: port 443)
  ACCORDION_HTTP_LISTEN   Address for the setup wizard, and then for the
                          redirect to HTTPS (default: port 80)
  ACCORDION_SETUP_WINDOW  on lets the setup wizard skip the setup code for 30
                          minutes after the server starts, for a server with
                          no screen (default: off)
  ACCORDION_CERTIFICATE_AUTHORITY
                          lets-encrypt, or lets-encrypt-staging for test
                          certificates that browsers don't trust
                          (default: lets-encrypt)
  ACCORDION_ADDRESS       The address to point the server's name at: auto, off,
                          or an address such as 192.168.1.20 (default: auto,
                          or off in a container)
  ACCORDION_ADMIN_ACCESS  home-network lets only devices on the home network
                          or VPN open the setup and admin pages. Use anywhere
                          for a server with no home network, such as a rented
                          one (default: home-network)
  ACCORDION_LOCAL_NAME    on announces accordion.local on the home network,
                          where people find the setup wizard and, later, the
                          redirect to the server's name (default: on)
  ACCORDION_RESTIC        The full path to restic, the program that makes
                          backups (default: restic next to accordion, or on
                          PATH)
  ACCORDION_UPDATE_URL    Where to look for updates, for trying a test release.
                          Releases still have to be signed with the release
                          key (default: GitHub's newest release)
  ACCORDION_WEB_DIR       A build of the web app to serve, such as
                          accordion-web's dist folder during development
                          (default: /usr/lib/accordion/web)
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
        [] => match serve().await {
            Ok(Served::Stopped) => Ok(()),
            Ok(Served::Restored) => Err(start_again()),
            Err(err) => Err(err),
        },
        ["setup-code"] => show_setup_code(),
        ["create-owner", username, display_name] => create_owner(username, display_name).await,
        ["reset-owner-password"] => reset_owner_password().await,
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

/// How the server stopped.
enum Served {
    Stopped,
    /// The setup wizard restored a backup, which the server opens when it
    /// starts again.
    Restored,
}

async fn serve() -> Result<Served, Box<dyn Error>> {
    let data_dir = data_dir()?;
    // Before anything opens the database, which a restore replaces.
    let restored = backup::apply_restore(&data_dir).map_err(|err| {
        format!(
            "Accordion couldn't finish restoring the backup into {}: {err}. Check that the \
             disk has space, then start Accordion again.",
            data_dir.display()
        )
    })?;
    if restored {
        let _ = std::fs::remove_file(data_dir.join(SETUP_CODE_FILE));
        tracing::info!("finished restoring the backup");
    }
    let db = open_database(&data_dir)?;
    let identity = Identity::new(db.clone()).await?;
    let storage = Storage::open(db.clone(), identity.clone(), &data_dir.join("storage")).await?;
    let backup = Backup::open(db.clone(), storage.clone(), &data_dir, restic_program()?).await?;
    let updates = Updates::open(
        db.clone(),
        UpdateSource::from_env()?,
        env!("CARGO_PKG_VERSION").parse()?,
        &data_dir,
    )
    .await?;
    let switches = Switches::open(db.clone()).await?;
    let web = WebApp::from_env()?;
    match &web {
        Some(web) => tracing::info!("serving the web app from {}", web.dir().display()),
        None => tracing::info!("this server doesn't have the web app"),
    }
    let gateway_dir = gateway_dir(&data_dir);
    let has_owner = identity.has_owner().await?;
    let settings = Settings::load(&gateway_dir)?;

    let (stop, stopped) = watch::channel(false);
    let stop = Arc::new(stop);
    let signal_stop = Arc::clone(&stop);
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = signal_stop.send(true);
    });
    let until_stopped = || {
        let mut stopped = stopped.clone();
        async move {
            let _ = stopped.wait_for(|&stopped| stopped).await;
        }
    };

    // Upkeep starts by rebuilding the plain view of everyone's files, in
    // case the server stopped partway through a change.
    let health = Health {
        storage: Some(storage.status()),
        backup: Some(backup.status()),
        updates: Some(updates.status()),
        ..Health::default()
    };
    let mut background = JoinSet::new();
    background.spawn(storage.clone().maintain(until_stopped()));
    background.spawn(backup.clone().maintain(until_stopped()));
    background.spawn(updates.clone().run(Some(backup.status()), until_stopped()));
    let modules = modules::start(
        modules::all(),
        &Services {
            db,
            identity: identity.clone(),
            storage: storage.clone(),
        },
        &Stopping::new(stopped.clone()),
        &mut background,
    )
    .await?;

    if settings.is_none() && has_owner {
        serve_http(
            AppState {
                identity,
                setup: None,
                health,
                admin_access: admin_access()?,
                modules,
                backup: Some(backup),
                storage: Some(storage),
                updates: Some(updates),
                switches: Some(switches),
                web: web.clone(),
            },
            until_stopped(),
        )
        .await?;
        finish(background).await?;
        tracing::info!("stopped");
        return Ok(Served::Stopped);
    }

    let https = listen("ACCORDION_HTTPS_LISTEN", 443).await?;
    let http = listen("ACCORDION_HTTP_LISTEN", 80).await?;
    notify_ready();
    let https_port = https.local_addr()?.port();
    let address = address_source()?;
    let setup = if has_owner {
        None
    } else {
        Some(Setup::start(SetupOptions {
            data_dir: data_dir.clone(),
            gateway_dir: gateway_dir.clone(),
            authority: certificate_authority()?,
            https_port,
            open_window: setup_window()?,
            address,
            backup: Some(backup.clone()),
        })?)
    };
    let local_name_on = local_name_on()?;
    let mut state = AppState {
        identity,
        setup: setup.as_ref().map(|(setup, _)| Arc::clone(setup)),
        health,
        admin_access: admin_access()?,
        modules,
        backup: Some(backup),
        storage: Some(storage),
        updates: Some(updates),
        switches: Some(switches),
        web: web.clone(),
    };

    let local_name = if local_name_on {
        announce_local_name(
            &mut state.health,
            http.local_addr()?.port(),
            until_stopped(),
        )
    } else {
        None
    };

    // Port 80 serves the wizard until the server has a name, then the
    // redirect to HTTPS. After setup in this run, the wizard keeps serving it
    // and sends everyone to HTTPS itself.
    let (settings, port_80) = match (settings, setup) {
        (Some(settings), setup) => {
            if let Some((setup, _)) = setup {
                let origin = setup::https_origin(&settings.name, https_port);
                setup.set_origin(origin.clone());
                tracing::info!(
                    "Accordion has a name but no owner yet. Open {origin}/setup to create the \
                     owner account. The setup code is {}.",
                    setup.display_code()
                );
            }
            let redirect = axum::serve(http, redirect_to_https(settings.name.clone(), https_port))
                .with_graceful_shutdown(until_stopped())
                .into_future();
            (settings, tokio::spawn(redirect))
        }
        (None, Some((setup, mut named))) => {
            log_wizard(&setup, http.local_addr()?.port());
            let mut restored = setup.restored();
            let wizard = axum::serve(
                http,
                setup::wizard_router(setup).into_make_service_with_connect_info::<Peer>(),
            )
            .with_graceful_shutdown(until_stopped())
            .into_future();
            let wizard = tokio::spawn(wizard);
            let named = async { named.wait_for(Option::is_some).await.ok()?.clone() };
            let restored = async { restored.wait_for(|&restored| restored).await.is_ok() };
            let served = tokio::select! {
                Some(settings) = named => Err(settings),
                true = restored => Ok(Served::Restored),
                () = until_stopped() => Ok(Served::Stopped),
            };
            match served {
                Err(settings) => (settings, wizard),
                Ok(served) => {
                    let _ = stop.send(true);
                    wizard.await??;
                    if let Some(local_name) = local_name {
                        local_name.await?;
                    }
                    finish(background).await?;
                    tracing::info!("stopped");
                    return Ok(served);
                }
            }
        }
        (None, None) => unreachable!("a server without a name or owner runs the wizard"),
    };

    serve_https(state, &gateway_dir, settings, address, https, until_stopped).await?;
    port_80.await??;
    if let Some(local_name) = local_name {
        local_name.await?;
    }
    finish(background).await?;
    tracing::info!("stopped");
    Ok(Served::Stopped)
}

/// Starts Accordion again in place of this process, after the setup wizard
/// restored a backup. Nothing from this run stays open, such as a database
/// connection that could write to the restored database's files. Returns only
/// if it couldn't.
fn start_again() -> Box<dyn Error> {
    use std::os::unix::process::CommandExt;

    tracing::info!("starting again to open the restored backup");
    let err = match std::env::current_exe() {
        Ok(program) => std::process::Command::new(program)
            .args(std::env::args_os().skip(1))
            .exec(),
        Err(err) => err,
    };
    format!(
        "Accordion restored the backup, but couldn't start again ({err}). Start Accordion \
         again to finish."
    )
    .into()
}

/// Waits for background work, such as storage upkeep, to finish after the
/// server stops.
async fn finish(mut background: JoinSet<()>) -> Result<(), Box<dyn Error>> {
    while let Some(result) = background.join_next().await {
        result?;
    }
    Ok(())
}

/// Starts announcing `accordion.local`, and returns the task that keeps it
/// going until `stopped` completes.
fn announce_local_name(
    health: &mut Health,
    http_port: u16,
    stopped: impl Future<Output = ()> + Send + 'static,
) -> Option<tokio::task::JoinHandle<()>> {
    match LocalName::announce(http_port) {
        Ok(local_name) => {
            health.local_name = Some(local_name.status());
            Some(tokio::spawn(local_name.run(stopped)))
        }
        Err(err) => {
            tracing::warn!(
                "Accordion couldn't announce {LOCAL_NAME} on the network ({err}). Devices can \
                 still reach the server by its address."
            );
            let (_, status) = watch::channel(LocalNameStatus::Failed {
                error: err.to_string(),
            });
            health.local_name = Some(status);
            None
        }
    }
}

/// Logs where to find the wizard. Without a screen, the log is where the
/// admin finds the setup code.
fn log_wizard(setup: &Setup, port: u16) {
    let port = if port == 80 {
        String::new()
    } else {
        format!(":{port}")
    };
    // A server can be on more than one network, such as a Raspberry Pi
    // that's also a Wi-Fi access point, so offer its address on each.
    let addresses: Vec<String> = std::iter::once(format!("http://accordion.local{port}"))
        .chain(
            accordion_gateway::address::network_addresses()
                .into_iter()
                .map(|address| format!("http://{address}{port}")),
        )
        .collect();
    let window = if setup.window_open() {
        " For the next 30 minutes, the wizard doesn't need the code."
    } else {
        ""
    };
    tracing::info!(
        "Accordion isn't set up yet. On a device on the same network, open {}, and enter \
         the setup code {}.{window}",
        or_list(&addresses),
        setup.display_code()
    );
}

/// "a", "a or b", or "a, b, or c".
fn or_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [first, second] => format!("{first} or {second}"),
        [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
    }
}

/// Serves plain HTTP, for development when there's an owner but no name.
async fn serve_http(
    state: AppState,
    stopped: impl Future<Output = ()> + Send + 'static,
) -> Result<(), Box<dyn Error>> {
    let listen = std::env::var("ACCORDION_LISTEN").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let listener = bind(&listen, "ACCORDION_LISTEN").await?;
    notify_ready();
    tracing::info!(
        "no server name is set, so serving plain HTTP on http://{}",
        listener.local_addr()?
    );

    let app = router(state, &Site::Unnamed).into_make_service_with_connect_info::<Peer>();
    axum::serve(listener, app)
        .with_graceful_shutdown(stopped)
        .await?;
    Ok(())
}

/// Serves HTTPS for the server's name, and keeps the certificate and the
/// name's address record current.
async fn serve_https<F, S>(
    mut state: AppState,
    gateway_dir: &Path,
    settings: Settings,
    address: Option<AddressSource>,
    https: TcpListener,
    until_stopped: F,
) -> Result<(), Box<dyn Error>>
where
    F: Fn() -> S,
    S: Future<Output = ()>,
{
    let https_port = https.local_addr()?.port();
    let site = Site::named(settings.name.clone(), https_port);
    let certs = CertStore::new();
    let renewer = Renewer::from_settings(gateway_dir, &settings, certs.clone())?;
    let publisher = match address {
        Some(source) => {
            let publisher = AddressPublisher::from_settings(&settings, source)?;
            Some(match &state.switches {
                Some(switches) => publisher.with_switch(switches.address()),
                None => publisher,
            })
        }
        None => None,
    };
    state.health.name = Some(NameHealth {
        name: settings.name.clone(),
        authority: settings.authority,
        provider: settings.provider,
        certificate: renewer.status(),
        address: publisher.as_ref().map(AddressPublisher::status),
    });
    tracing::info!(
        "serving https://{} on {}",
        settings.name,
        https.local_addr()?
    );

    let publish = async {
        if let Some(publisher) = publisher {
            publisher.run(until_stopped()).await;
        }
    };
    tokio::join!(
        tls::serve(
            https,
            tls::server_config(certs),
            router(state, &site),
            until_stopped(),
        ),
        renewer.run(until_stopped()),
        publish,
    );
    Ok(())
}

/// Shows the setup code, for a server whose log is hard to reach.
fn show_setup_code() -> Result<(), Box<dyn Error>> {
    let path = data_dir()?.join(SETUP_CODE_FILE);
    let code = setup::read_code(&path)
        .map_err(|err| format!("Accordion couldn't read {}: {err}", path.display()))?;
    match code {
        Some(code) => {
            println!("{}", setup::group_code(&code));
            Ok(())
        }
        None => Err(
            "There's no setup code. Either Accordion is already set up, or it hasn't \
                     started yet. Start Accordion, then try again."
                .into(),
        ),
    }
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
    // The wizard's code isn't needed once there's an owner.
    let _ = std::fs::remove_file(data_dir()?.join(SETUP_CODE_FILE));
    Ok(())
}

/// Makes a link for the owner to choose a new password, for when they've
/// forgotten it.
///
/// Nobody can reset the owner's password on `/people`, so an admin can't take
/// over the server. This command needs access to the server itself, which can
/// already read everything, so it gives no one new power. It prints a link to
/// `/welcome` rather than asking for the password here, so the password isn't
/// shown on screen and a password manager can save it.
async fn reset_owner_password() -> Result<(), Box<dyn Error>> {
    let data_dir = data_dir()?;
    let identity = start_identity(&data_dir).await?;
    let owner = identity
        .members()
        .await?
        .into_iter()
        .map(|member| member.user)
        .find(|user| user.role == Role::Owner)
        .ok_or(
            "This server doesn't have an owner yet, so there's no password to reset. Start \
             Accordion and open the setup wizard to create the owner account.",
        )?;
    let origin = match Settings::load(&gateway_dir(&data_dir))? {
        Some(settings) => {
            setup::https_origin(&settings.name, listen_port("ACCORDION_HTTPS_LISTEN", 443))
        }
        None => plain_http_origin(),
    };
    // They forgot it, so their devices stay connected. If someone else might
    // know it, they can delete app passwords once they're signed in.
    let (token, _) = identity
        .make_password_link(owner.id, AppPasswords::Keep)
        .await?;
    eprintln!(
        "Your password for {} is reset, and you're signed out everywhere. Devices that use app \
         passwords stay connected.\n\nTo choose a new password, open this link in a browser. It \
         works once, in the next 24 hours:\n",
        owner.username
    );
    println!("{origin}/welcome#{}", token.as_str());
    eprintln!(
        "\nIf someone else might know the old password, delete your app passwords on your account \
         page after you sign in."
    );
    Ok(())
}

/// The port in an address setting such as `ACCORDION_HTTPS_LISTEN`.
fn listen_port(variable: &str, default: u16) -> u16 {
    std::env::var(variable)
        .ok()
        .and_then(|address| address.parse::<SocketAddr>().ok())
        .map_or(default, |address| address.port())
}

/// Where a server with no name serves plain HTTP. An address that means
/// every interface, such as `0.0.0.0`, isn't one a browser can open, so it
/// becomes `localhost`.
fn plain_http_origin() -> String {
    let listen = std::env::var("ACCORDION_LISTEN").unwrap_or_else(|_| "127.0.0.1:8080".into());
    match listen.parse::<SocketAddr>() {
        Ok(address) if address.ip().is_unspecified() => {
            format!("http://localhost:{}", address.port())
        }
        _ => format!("http://{listen}"),
    }
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

fn open_database(data_dir: &Path) -> Result<Database, Box<dyn Error>> {
    Ok(Database::open(&data_dir.join("accordion.db"))?)
}

async fn start_identity(data_dir: &Path) -> Result<Identity, Box<dyn Error>> {
    Ok(Identity::new(open_database(data_dir)?).await?)
}

/// Whether the wizard skips the setup code for its first 30 minutes.
/// Where restic is: `ACCORDION_RESTIC`, or next to this program, where
/// packages install it, or on `PATH`.
fn restic_program() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("ACCORDION_RESTIC").filter(|path| !path.is_empty()) {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(format!(
                "ACCORDION_RESTIC is \"{}\". Set it to restic's full path, starting with /.",
                path.display()
            ));
        }
        return Ok(path);
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("restic")));
    let on_path = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|dir| dir.join("restic"));
    match beside
        .into_iter()
        .chain(on_path)
        .find(|path| path.is_file())
    {
        Some(path) => Ok(path),
        None => {
            // The server still starts, and the backups page and health page
            // say what's wrong once a backup tries to run.
            tracing::warn!(
                "Accordion couldn't find restic, so backups won't work. Install restic, or \
                 set ACCORDION_RESTIC to its full path."
            );
            Ok(PathBuf::from("restic"))
        }
    }
}

fn setup_window() -> Result<bool, String> {
    match std::env::var("ACCORDION_SETUP_WINDOW")
        .as_deref()
        .map(str::trim)
    {
        Err(_) | Ok("off") => Ok(false),
        Ok("on") => Ok(true),
        Ok(other) => Err(format!(
            "ACCORDION_SETUP_WINDOW is \"{other}\". Set it to on or off."
        )),
    }
}

fn certificate_authority() -> Result<Authority, String> {
    match std::env::var("ACCORDION_CERTIFICATE_AUTHORITY")
        .as_deref()
        .map(str::trim)
    {
        Err(_) | Ok("lets-encrypt") => Ok(Authority::LetsEncrypt),
        Ok("lets-encrypt-staging") => Ok(Authority::LetsEncryptStaging),
        Ok(other) => Err(format!(
            "ACCORDION_CERTIFICATE_AUTHORITY is \"{other}\". Set it to lets-encrypt, or to \
             lets-encrypt-staging for test certificates that browsers don't trust."
        )),
    }
}

fn admin_access() -> Result<AdminAccess, String> {
    match std::env::var("ACCORDION_ADMIN_ACCESS")
        .as_deref()
        .map(str::trim)
    {
        Err(_) | Ok("home-network") => Ok(AdminAccess::HomeNetwork),
        Ok("anywhere") => Ok(AdminAccess::Anywhere),
        Ok(other) => Err(format!(
            "ACCORDION_ADMIN_ACCESS is \"{other}\". Set it to home-network, or to anywhere \
             for a server with no home network."
        )),
    }
}

/// Whether to announce `accordion.local`.
fn local_name_on() -> Result<bool, String> {
    match std::env::var("ACCORDION_LOCAL_NAME")
        .as_deref()
        .map(str::trim)
    {
        Err(_) | Ok("on") => Ok(true),
        Ok("off") => Ok(false),
        Ok(other) => Err(format!(
            "ACCORDION_LOCAL_NAME is \"{other}\". Set it to on or off."
        )),
    }
}

/// Where the address for the name's DNS record comes from, or `None` to
/// leave the record alone.
fn address_source() -> Result<Option<AddressSource>, String> {
    let Ok(value) = std::env::var("ACCORDION_ADDRESS") else {
        if in_container() {
            tracing::warn!(
                "Accordion is running in a container, where it can't see the machine's network \
                 address, so it won't update the name's DNS record. Set ACCORDION_ADDRESS to the \
                 machine's address on the home network, or point the name at it yourself."
            );
            return Ok(None);
        }
        return Ok(Some(AddressSource::Detect));
    };
    match value.trim() {
        "auto" => Ok(Some(AddressSource::Detect)),
        "off" => Ok(None),
        other => other
            .parse::<IpAddr>()
            .map(|address| Some(AddressSource::Fixed(address)))
            .map_err(|_| {
                format!(
                    "ACCORDION_ADDRESS is \"{other}\", which isn't an address. Set it to auto, \
                     off, or the server's address on the home network, such as 192.168.1.20."
                )
            }),
    }
}

/// Docker and Podman each leave a file at the root of the container.
fn in_container() -> bool {
    Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists()
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

/// Tells systemd the server is ready: its database is up to date and it's
/// listening. The update helper waits for this before it trusts a new
/// version (decision 12). Outside systemd, there's no one to tell.
fn notify_ready() {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixDatagram};

    let Some(path) = std::env::var_os("NOTIFY_SOCKET") else {
        return;
    };
    let path = path.to_string_lossy().into_owned();
    // A name that starts with @ is in the abstract namespace, not a file.
    let address = match path.strip_prefix('@') {
        Some(name) => SocketAddr::from_abstract_name(name.as_bytes()),
        None => SocketAddr::from_pathname(&path),
    };
    let sent =
        address.and_then(|address| UnixDatagram::unbound()?.send_to_addr(b"READY=1", &address));
    if let Err(err) = sent {
        tracing::warn!(%err, "couldn't tell systemd that Accordion is ready");
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_addresses_with_a_serial_comma() {
        let items = |list: &[&str]| list.iter().map(|item| item.to_string()).collect::<Vec<_>>();
        assert_eq!(or_list(&items(&["a"])), "a");
        assert_eq!(or_list(&items(&["a", "b"])), "a or b");
        assert_eq!(or_list(&items(&["a", "b", "c"])), "a, b, or c");
    }
}
