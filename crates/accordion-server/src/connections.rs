//! `/network`: every connection Accordion makes to other computers, and
//! switches to turn them off (decision 11).
//!
//! Like the other admin pages, it opens only from the home network, and only
//! for admins. Backup targets and the update check keep their own switches,
//! in their services. This module keeps the rest.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use accordion_core::backup::{self, Target, TargetKind, TargetStatus};
use accordion_core::db::rusqlite::{self, OptionalExtension};
use accordion_core::db::{self, Database, Migration};
use accordion_core::identity::SignedIn;
use accordion_module::{ApiError, Authenticated};
use accordion_types::{BackupTargetId, Role};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::AppState;
use crate::setup::asset;

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "network switches",
    sql: "
    -- Connections an admin turned off on the Network page. A purpose
    -- without a row is on.
    CREATE TABLE network_switches (
        purpose TEXT PRIMARY KEY,
        turned_on INTEGER NOT NULL CHECK (turned_on IN (0, 1))
    );
    ",
}];

/// Keeping the name's address record pointed at the server.
const ADDRESS: &str = "address";

/// The switches no other service keeps. Cloning it is cheap.
#[derive(Clone)]
pub struct Switches {
    db: Database,
    address: Arc<watch::Sender<bool>>,
}

impl Switches {
    pub async fn open(db: Database) -> Result<Self, db::Error> {
        db.migrate("network", MIGRATIONS).await?;
        let address = db.read(|conn| turned_on(conn, ADDRESS)).await?;
        Ok(Self {
            db,
            address: Arc::new(watch::channel(address).0),
        })
    }

    /// Whether Accordion keeps the name's address record current.
    pub fn address(&self) -> watch::Receiver<bool> {
        self.address.subscribe()
    }

    pub async fn set_address(&self, on: bool) -> Result<(), db::Error> {
        self.db
            .write(move |tx| {
                tx.execute(
                    "INSERT INTO network_switches (purpose, turned_on) VALUES (?1, ?2)
                     ON CONFLICT (purpose) DO UPDATE SET turned_on = excluded.turned_on",
                    (ADDRESS, on),
                )
            })
            .await?;
        self.address.send_replace(on);
        Ok(())
    }
}

fn turned_on(conn: &rusqlite::Connection, purpose: &str) -> Result<bool, db::Error> {
    Ok(conn
        .query_row(
            "SELECT turned_on FROM network_switches WHERE purpose = ?1",
            [purpose],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(true))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/network",
            get(|| async { asset(NETWORK_HTML, "text/html; charset=utf-8") }),
        )
        .route(
            "/network.js",
            get(|| async { asset(NETWORK_JS, "text/javascript") }),
        )
        .route("/api/network", get(status))
        .route("/api/network/updates", put(set_updates))
        .route("/api/network/address", put(set_address))
        .route("/api/network/backups/{id}", put(set_backup))
}

const NETWORK_HTML: &str = include_str!("connections/network.html");
const NETWORK_JS: &str = include_str!("connections/network.js");

fn require_admin(user: &SignedIn) -> Result<(), ApiError> {
    if matches!(user.user.role, Role::Owner | Role::Admin) {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::FORBIDDEN,
        "Only admins can see and change the server's connections. Ask your admin.",
    ))
}

fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
    })
}

#[derive(Serialize)]
struct NetworkView {
    /// `None` on a server without the update checker.
    updates: Option<UpdatesView>,
    /// `None` until the server has a name.
    address: Option<AddressView>,
    certificate: Option<CertificateView>,
    /// Targets that go over the network. Folders aren't listed.
    backups: Vec<BackupView>,
    /// The hour nightly backups start, from 0 to 23.
    backup_hour: u8,
}

/// The last time a connection was made, and whether it worked.
#[derive(Serialize)]
struct LastView {
    /// Milliseconds since 1970, for the page to show in local time.
    at: u64,
    ok: bool,
    problem: Option<String>,
}

#[derive(Serialize)]
struct UpdatesView {
    on: bool,
    host: String,
    last: Option<LastView>,
}

#[derive(Serialize)]
struct AddressView {
    on: bool,
    /// The server's settings leave the record alone, so there's no switch.
    off_by_setting: bool,
    host: &'static str,
    /// Since the server started.
    last: Option<LastView>,
}

#[derive(Serialize)]
struct CertificateView {
    authority_host: String,
    provider_host: &'static str,
    /// Since the server started.
    last: Option<LastView>,
    expires_at: Option<u64>,
}

#[derive(Serialize)]
struct BackupView {
    id: BackupTargetId,
    name: String,
    host: String,
    on: bool,
    /// The newest backup, check, or restore test.
    last: Option<LastView>,
}

/// Where a target connects to, or `None` for a folder on this server.
fn target_host(target: &Target) -> Option<String> {
    match target.kind {
        TargetKind::Folder => None,
        TargetKind::S3 => target.bucket().map(|(host, _)| host.to_owned()),
        TargetKind::Sftp => target.sftp().map(|(_, host, _)| host.to_owned()),
        TargetKind::Rest => target.rest().map(|address| {
            let rest = address.split_once("://").map_or(address, |(_, rest)| rest);
            let authority = rest.split('/').next().unwrap_or(rest);
            // Leave out a username and password, if the address has them.
            let host = authority
                .rsplit_once('@')
                .map_or(authority, |(_, host)| host);
            host.to_owned()
        }),
    }
    .or_else(|| (target.kind != TargetKind::Folder).then(|| target.location.clone()))
}

fn backup_view(status: &TargetStatus) -> Option<BackupView> {
    let host = target_host(&status.target)?;
    let last = [
        &status.last_backup,
        &status.last_check,
        &status.last_restore_test,
    ]
    .into_iter()
    .flatten()
    .max_by_key(|outcome| outcome.finished_at)
    .map(|outcome| LastView {
        at: millis(outcome.finished_at),
        ok: outcome.ok,
        problem: outcome.problem.clone(),
    });
    Some(BackupView {
        id: status.target.id,
        name: status.target.name.clone(),
        host,
        on: !status.target.paused,
        last,
    })
}

/// `GET /api/network`: each kind of connection, where it goes, and whether
/// it's on.
async fn status(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Json<NetworkView>, ApiError> {
    require_admin(&me)?;
    let updates = state.updates.as_ref().map(|updates| {
        let status = updates.status();
        let status = status.borrow();
        UpdatesView {
            on: status.check_on,
            host: updates.source_host(),
            last: status.last_check.as_ref().map(|check| LastView {
                at: millis(check.at),
                ok: check.problem.is_none(),
                problem: check.problem.clone(),
            }),
        }
    });
    let (address, certificate) = match &state.health.name {
        None => (None, None),
        Some(name) => {
            let provider_host = name.provider.api_host();
            let address = match &name.address {
                None => AddressView {
                    on: false,
                    off_by_setting: true,
                    host: provider_host,
                    last: None,
                },
                Some(address) => {
                    let address = address.borrow();
                    AddressView {
                        // The switch, rather than the publisher's status, so
                        // the page shows a change right away.
                        on: state
                            .switches
                            .as_ref()
                            .is_none_or(|switches| *switches.address().borrow()),
                        off_by_setting: false,
                        host: provider_host,
                        last: address.last_attempt.map(|at| LastView {
                            at: millis(at),
                            ok: address.failures == 0,
                            problem: address.last_error.clone(),
                        }),
                    }
                }
            };
            let certificate = name.certificate.borrow();
            let certificate = CertificateView {
                authority_host: name.authority.directory().host().to_owned(),
                provider_host,
                last: certificate.last_attempt.map(|at| LastView {
                    at: millis(at),
                    ok: certificate.failures == 0,
                    problem: certificate.last_error.clone(),
                }),
                expires_at: certificate
                    .validity
                    .map(|validity| millis(validity.expires_at)),
            };
            (Some(address), Some(certificate))
        }
    };
    let (backups, backup_hour) = match &state.backup {
        None => (Vec::new(), 0),
        Some(backup) => {
            let status = backup.status();
            let status = status.borrow();
            (
                status.targets.iter().filter_map(backup_view).collect(),
                status.hour,
            )
        }
    };
    Ok(Json(NetworkView {
        updates,
        address,
        certificate,
        backups,
        backup_hour,
    }))
}

#[derive(Deserialize)]
struct Switch {
    on: bool,
}

/// Whether to say "on" or "off" in the log.
fn word(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// `PUT /api/network/updates`: turns the update check on or off.
async fn set_updates(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Json(switch): Json<Switch>,
) -> Result<StatusCode, ApiError> {
    require_admin(&me)?;
    let updates = state.updates.ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Updates aren't available on this server.",
        )
    })?;
    updates.set_check_on(switch.on).await.map_err(|err| {
        tracing::error!(%err, "couldn't save the update check switch");
        ApiError::server_problem()
    })?;
    tracing::info!(user = %me.user.id, "turned the update check {}", word(switch.on));
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/network/address`: turns updates to the name's address record
/// on or off.
async fn set_address(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Json(switch): Json<Switch>,
) -> Result<StatusCode, ApiError> {
    require_admin(&me)?;
    let switches = match (&state.switches, &state.health.name) {
        (Some(switches), Some(name)) if name.address.is_some() => switches,
        (_, Some(_)) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "The ACCORDION_ADDRESS setting on the server turns address updates off, so \
                 this page can't turn them on. Change that setting, then restart Accordion.",
            ));
        }
        (_, None) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "This server doesn't have a name yet, so there's no address to keep current.",
            ));
        }
    };
    switches.set_address(switch.on).await.map_err(|err| {
        tracing::error!(%err, "couldn't save the address switch");
        ApiError::server_problem()
    })?;
    tracing::info!(user = %me.user.id, "turned address updates {}", word(switch.on));
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/network/backups/{id}`: pauses or resumes backups to one
/// location.
async fn set_backup(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Path(id): Path<BackupTargetId>,
    Json(switch): Json<Switch>,
) -> Result<StatusCode, ApiError> {
    require_admin(&me)?;
    let backup = state.backup.ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Backups aren't available on this server.",
        )
    })?;
    backup
        .set_paused(id, !switch.on)
        .await
        .map_err(|err| match err {
            backup::Error::NotFound => ApiError::new(StatusCode::NOT_FOUND, err.to_string()),
            err => {
                tracing::error!(%err, "couldn't save a backup location's switch");
                ApiError::server_problem()
            }
        })?;
    tracing::info!(
        user = %me.user.id,
        target = %id,
        "turned backups to a location {}",
        word(switch.on)
    );
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use accordion_types::BackupTargetId;

    use super::*;

    fn target(kind: TargetKind, location: &str) -> Target {
        Target {
            id: BackupTargetId::generate(),
            name: "Offsite".into(),
            kind,
            location: location.into(),
            created_at: "2026-10-27T00:00:00.000Z".into(),
            append_only: false,
            paused: false,
        }
    }

    #[test]
    fn names_where_each_kind_of_target_connects() {
        let host = |kind, location| target_host(&target(kind, location));
        assert_eq!(host(TargetKind::Folder, "/media/usb/backups"), None);
        assert_eq!(
            host(
                TargetKind::S3,
                "s3:https://s3.us-west-004.backblazeb2.com/family-backups"
            )
            .as_deref(),
            Some("s3.us-west-004.backblazeb2.com")
        );
        assert_eq!(
            host(TargetKind::Sftp, "sftp:backup@nas.local:/srv/accordion").as_deref(),
            Some("nas.local")
        );
        assert_eq!(
            host(TargetKind::Rest, "rest:https://nas.local:8000/accordion").as_deref(),
            Some("nas.local:8000")
        );
    }
}
