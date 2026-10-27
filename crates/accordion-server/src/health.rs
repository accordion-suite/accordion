//! The health page: how the server's certificate, its name's address record,
//! `accordion.local`, stored files, backups, and updates are doing, for the
//! household admin.
//!
//! The spec's health page also covers disk space, SMART status, encryption,
//! and mail. Each will add a check here once it exists.

use std::fmt::Write as _;
use std::time::{Duration, SystemTime};

use accordion_core::backup::{BackupStatus, KeyState};
use accordion_core::identity::SignedIn;
use accordion_core::storage::StorageStatus;
use accordion_gateway::ServerName;
use accordion_gateway::address::AddressStatus;
use accordion_gateway::certificates::{Authority, CertificateStatus};
use accordion_gateway::local_name::{LOCAL_NAME, LocalNameStatus};
use accordion_types::Role;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tokio::sync::watch;

use crate::AppState;
use crate::setup::asset;
use crate::updates::{CheckResult, UpdateStatus};
use accordion_module::ApiError;
use accordion_module::Authenticated;
use accordion_update::protocol::Outcome;

/// A certificate that expires sooner than this is a problem, not a warning.
/// Renewal starts when a third of the certificate's lifetime is left, so this
/// means about three weeks of failed renewals.
const EXPIRY_PROBLEM: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// A newest working backup older than this is a problem, as the spec asks.
const BACKUP_PROBLEM: Duration = Duration::from_secs(48 * 60 * 60);

/// Where the health page gets its news. Cloning it is cheap.
#[derive(Clone, Default)]
pub struct Health {
    /// Set once the server has a name.
    pub name: Option<NameHealth>,
    /// Set while the server announces `accordion.local`.
    pub local_name: Option<watch::Receiver<LocalNameStatus>>,
    /// Set once storage upkeep is running.
    pub storage: Option<watch::Receiver<StorageStatus>>,
    /// Set once the backup schedule is running.
    pub backup: Option<watch::Receiver<BackupStatus>>,
    /// Set once update checks are running.
    pub updates: Option<watch::Receiver<UpdateStatus>>,
}

/// The server's name and the background work that keeps it usable.
#[derive(Clone)]
pub struct NameHealth {
    pub name: ServerName,
    pub authority: Authority,
    pub certificate: watch::Receiver<CertificateStatus>,
    /// `None` when Accordion leaves the name's address record alone.
    pub address: Option<watch::Receiver<AddressStatus>>,
}

/// How a check is doing. Ordered from best to worst.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Ok,
    Warning,
    Problem,
}

#[derive(Debug, Serialize)]
pub struct Check {
    pub id: &'static str,
    pub title: &'static str,
    pub state: Level,
    /// Full sentences for the admin: what's going on, and what to do about
    /// it if anything.
    pub summary: String,
}

#[derive(Serialize)]
struct Report {
    /// The worst state of any check.
    state: Level,
    checks: Vec<Check>,
}

impl Health {
    /// Every check, as of `now`.
    pub fn checks(&self, now: SystemTime) -> Vec<Check> {
        let mut checks = Vec::new();
        match &self.name {
            Some(name) => {
                checks.push(certificate_check(name, &name.certificate.borrow(), now));
                checks.push(address_check(
                    &name.name,
                    name.address
                        .as_ref()
                        .map(|address| address.borrow().clone()),
                    now,
                ));
            }
            None => checks.push(Check {
                id: "certificate",
                title: "Certificate",
                state: Level::Warning,
                summary: "This server doesn't have a name yet, so it has no certificate and \
                          serves plain HTTP. Set its name with accordion set-name."
                    .to_owned(),
            }),
        }
        if let Some(local_name) = &self.local_name {
            checks.push(local_name_check(&local_name.borrow(), self.name.as_ref()));
        }
        if let Some(storage) = &self.storage {
            checks.push(storage_check(&storage.borrow()));
        }
        if let Some(backup) = &self.backup {
            checks.push(backup_check(&backup.borrow(), now));
        }
        if let Some(updates) = &self.updates {
            checks.push(updates_check(&updates.borrow(), now));
        }
        checks
    }
}

fn certificate_check(health: &NameHealth, status: &CertificateStatus, now: SystemTime) -> Check {
    let name = &health.name;
    let (mut state, mut summary) = match status.validity {
        None if status.renewing => (
            Level::Warning,
            format!("Accordion is getting a certificate for {name}. This can take a few minutes."),
        ),
        None => (
            Level::Problem,
            format!("Accordion doesn't have a certificate for {name}, so apps can't connect."),
        ),
        Some(validity) => match validity.expires_at.duration_since(now) {
            Err(_) => (
                Level::Problem,
                format!("The certificate for {name} has expired, so apps can't connect safely."),
            ),
            Ok(left) if left < EXPIRY_PROBLEM => (
                Level::Problem,
                format!("The certificate for {name} expires in {}.", in_words(left)),
            ),
            Ok(_) if status.failures > 0 => (
                Level::Warning,
                format!(
                    "The certificate for {name} is due for renewal, and Accordion couldn't \
                     renew it."
                ),
            ),
            Ok(left) => {
                let renewal = match validity.renew_at.duration_since(now) {
                    _ if status.renewing => "Accordion is renewing it now.".to_owned(),
                    Ok(wait) if !wait.is_zero() => {
                        format!("Accordion renews it in {}.", in_words(wait))
                    }
                    _ => "Accordion renews it soon.".to_owned(),
                };
                (
                    Level::Ok,
                    format!(
                        "The certificate for {name} expires in {}. {renewal}",
                        in_words(left)
                    ),
                )
            }
        },
    };

    // Say why renewal is failing whenever it is.
    if let Some(error) = &status.last_error {
        let _ = write!(summary, " The last try failed. {error}");
        if let Some(retry_at) = status.retry_at {
            let wait = retry_at.duration_since(now).unwrap_or_default();
            let _ = write!(summary, " Accordion will try again in {}.", in_words(wait));
        }
    }
    if health.authority == Authority::LetsEncryptStaging {
        state = state.max(Level::Warning);
        summary.push_str(
            " It's a test certificate, which browsers don't trust. To get a trusted one, run \
             accordion set-name again without --staging.",
        );
    }
    Check {
        id: "certificate",
        title: "Certificate",
        state,
        summary,
    }
}

fn address_check(name: &ServerName, status: Option<AddressStatus>, now: SystemTime) -> Check {
    let (state, summary) = match status {
        None => (
            Level::Ok,
            format!(
                "Accordion doesn't update the DNS record for {name}, because ACCORDION_ADDRESS \
                 is off. Make sure the record holds the server's address on the home network."
            ),
        ),
        Some(status) => {
            if let Some(error) = status.not_found {
                (
                    Level::Warning,
                    format!(
                        "Accordion can't find its own network address ({error}), so it can't \
                         keep the DNS record for {name} current. Check the server's network \
                         connection."
                    ),
                )
            } else if let Some(error) = status.last_error {
                let mut summary =
                    format!("Accordion couldn't update the DNS record for {name}. {error}");
                if let Some(retry_at) = status.retry_at {
                    let wait = retry_at.duration_since(now).unwrap_or_default();
                    let _ = write!(summary, " It'll try again in {}.", in_words(wait));
                }
                (Level::Warning, summary)
            } else if let Some(address) = status.published {
                (
                    Level::Ok,
                    format!("{name} points at {address}, the server's address."),
                )
            } else {
                (
                    Level::Ok,
                    format!("Accordion is pointing {name} at the server."),
                )
            }
        }
    };
    Check {
        id: "address",
        title: "Address record",
        state,
        summary,
    }
}

fn local_name_check(status: &LocalNameStatus, name: Option<&NameHealth>) -> Check {
    let (state, summary) = match status {
        LocalNameStatus::Announcing { name: local } if local == LOCAL_NAME => (
            Level::Ok,
            format!("Devices on the home network can also find the server at http://{local}."),
        ),
        LocalNameStatus::Announcing { name: local } => (
            Level::Warning,
            format!(
                "Another device on the network already uses {LOCAL_NAME}, so devices find this \
                 server at http://{local} instead. If it's another Accordion server, turn off \
                 the one you don't use."
            ),
        ),
        LocalNameStatus::Failed { error } => {
            let reach = match name {
                Some(name) => format!("at https://{}", name.name),
                None => "by its address".to_owned(),
            };
            (
                Level::Warning,
                format!(
                    "Accordion couldn't announce {LOCAL_NAME} on the network ({error}). Devices \
                     can still reach the server {reach}."
                ),
            )
        }
    };
    Check {
        id: "local_name",
        title: "Local name",
        state,
        summary,
    }
}

fn storage_check(status: &StorageStatus) -> Check {
    let (state, summary) = if status.damaged > 0 {
        (
            Level::Problem,
            format!(
                "Accordion found {} that changed or went missing on the disk after they were \
                 saved. The disk may be failing, so check its health. Then restore the damaged \
                 files from a backup. Uploading a damaged file again also repairs it.",
                count(status.damaged, "stored file")
            ),
        )
    } else if let Some(error) = &status.last_error {
        (
            Level::Warning,
            format!(
                "Accordion's hourly storage upkeep failed ({error}). It'll try again in an hour."
            ),
        )
    } else if status.rebuilding_view {
        (
            Level::Ok,
            "Accordion is rebuilding the plain copy of everyone's files in storage/files. Saving \
             files waits until it's done."
                .to_owned(),
        )
    } else {
        (
            Level::Ok,
            "Accordion checks a few stored files each hour for damage, and none has turned up."
                .to_owned(),
        )
    };
    Check {
        id: "storage",
        title: "Stored files",
        state,
        summary,
    }
}

fn backup_check(status: &BackupStatus, now: SystemTime) -> Check {
    let check = |state, summary: String| Check {
        id: "backups",
        title: "Backups",
        state,
        summary,
    };
    if status.key == KeyState::None {
        return check(
            Level::Warning,
            "Backups are off, so a failing disk could lose everything on this server. Turn \
             them on from the Backups page."
                .to_owned(),
        );
    }
    if status.targets.is_empty() {
        return check(
            Level::Warning,
            "Backups are on, but there's nowhere for them to go. Add a location, such as a USB \
             disk, on the Backups page."
                .to_owned(),
        );
    }

    let mut state = Level::Ok;
    let mut notes = Vec::new();
    for target in &status.targets {
        let name = &target.target.name;
        let problem =
            |outcome: &accordion_core::backup::Outcome| outcome.problem.clone().unwrap_or_default();
        let age = target
            .last_good_backup
            .map(|good| now.duration_since(good).unwrap_or_default());
        match (&target.last_backup, age) {
            (None, _) => notes.push(format!(
                "Nothing has been backed up to {name} yet. The first backup starts at {}.",
                hour_label(status.hour)
            )),
            (Some(last), Some(age)) if age <= BACKUP_PROBLEM => {
                if last.ok {
                    notes.push(format!("Backed up to {name} {} ago.", in_words(age)));
                } else {
                    state = state.max(Level::Warning);
                    notes.push(format!(
                        "The last backup to {name} didn't work. {} The last one that worked \
                         was {} ago.",
                        problem(last),
                        in_words(age)
                    ));
                }
            }
            (Some(last), Some(age)) => {
                state = Level::Problem;
                let mut note = format!(
                    "The last backup to {name} that worked was {} ago.",
                    in_words(age)
                );
                if !last.ok {
                    let _ = write!(note, " {}", problem(last));
                }
                notes.push(note);
            }
            (Some(last), None) => {
                state = Level::Problem;
                notes.push(format!(
                    "No backup to {name} has worked yet. {}",
                    problem(last)
                ));
            }
        }
        if let Some(check) = target.last_check.as_ref().filter(|check| !check.ok) {
            state = Level::Problem;
            notes.push(format!(
                "The last check of the backups in {name} found a problem. {}",
                problem(check)
            ));
        }
        if let Some(test) = target.last_restore_test.as_ref().filter(|test| !test.ok) {
            state = Level::Problem;
            notes.push(format!(
                "The last restore test from {name} didn't pass. {}",
                problem(test)
            ));
        }
    }
    if status.key == KeyState::Unconfirmed {
        state = state.max(Level::Warning);
        notes.push(
            "Confirm on the Backups page that you've saved the recovery key. You'll need it to \
             restore a backup."
                .to_owned(),
        );
    }
    check(state, notes.join(" "))
}

/// Such as "3 a.m." or "midnight".
fn updates_check(status: &UpdateStatus, now: SystemTime) -> Check {
    let check = |state, summary: String| Check {
        id: "updates",
        title: "Updates",
        state,
        summary,
    };
    let installed = &status.installed;
    if let Some(installing) = &status.installing {
        return check(
            Level::Ok,
            format!(
                "Accordion is installing {}. It restarts while it updates, so pages may stop \
                 answering for a few minutes.",
                installing.version
            ),
        );
    }
    if let Some(problem) = &status.install_problem {
        return check(Level::Warning, problem.clone());
    }
    let Some(release) = status.waiting() else {
        return match &status.last_check {
            None => check(
                Level::Ok,
                format!(
                    "This server runs Accordion {installed}. Accordion checks for updates once a \
                     day, and hasn't checked yet."
                ),
            ),
            Some(CheckResult {
                problem: Some(problem),
                ..
            }) => check(
                Level::Warning,
                format!(
                    "This server runs Accordion {installed}. The last check for updates didn't \
                     work. {problem}. Accordion tries again in a few hours."
                ),
            ),
            Some(_) => check(
                Level::Ok,
                format!("This server runs Accordion {installed}, which is up to date."),
            ),
        };
    };
    let version = &release.version;
    if let Some(failed) = status.failed(version) {
        let reason = failed
            .reason
            .as_deref()
            .unwrap_or("The helper didn't say why");
        let what = match failed.outcome {
            Outcome::RolledBack => format!(
                "Accordion {version} didn't work, so Accordion went back to {installed}. {reason}."
            ),
            _ => format!("Accordion didn't update to {version}. {reason}."),
        };
        return check(
            Level::Warning,
            format!(
                "{what} Accordion won't install {version} on its own, but it will install a \
                 newer release."
            ),
        );
    }
    let available = format!(
        "Accordion {version} is available, released {}. This server runs {installed}.",
        long_date(&release.published)
    );
    if let Some(from) = status.installs_from() {
        let when = match from.duration_since(now) {
            Err(_) => "after tonight's backup".to_owned(),
            Ok(left) => {
                let days = left.as_secs().div_ceil(24 * 60 * 60);
                format!(
                    "after the nightly backup, starting in {}",
                    count(days, "day")
                )
            }
        };
        return check(
            Level::Ok,
            format!("{available} It installs on its own {when}."),
        );
    }
    let (state, urgency) = if release.security {
        (
            Level::Warning,
            " It fixes a security problem, so install it soon.",
        )
    } else {
        (Level::Ok, "")
    };
    check(
        state,
        format!("{available}{urgency} The Updates page says how to install it."),
    )
}

/// A date such as `2026-11-02`, as "November 2, 2026".
fn long_date(date: &str) -> String {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let mut parts = date.split('-').map(str::parse::<usize>);
    match (parts.next(), parts.next(), parts.next()) {
        (Some(Ok(year)), Some(Ok(month @ 1..=12)), Some(Ok(day))) => {
            format!("{} {day}, {year}", MONTHS[month - 1])
        }
        _ => date.to_owned(),
    }
}

fn hour_label(hour: u8) -> String {
    match hour {
        0 => "midnight".to_owned(),
        12 => "noon".to_owned(),
        1..=11 => format!("{hour} a.m."),
        _ => format!("{} p.m.", hour - 12),
    }
}

/// A rough length of time, such as "three days" or "12 hours".
fn in_words(duration: Duration) -> String {
    let minutes = duration.as_secs().div_ceil(60);
    let hours = duration.as_secs() / (60 * 60);
    let days = duration.as_secs() / (24 * 60 * 60);
    if minutes < 2 {
        "a minute".to_owned()
    } else if minutes < 60 {
        count(minutes, "minute")
    } else if hours < 48 {
        count(hours, "hour")
    } else {
        count(days, "day")
    }
}

/// Spells out zero through nine, as the style guide asks.
fn count(n: u64, unit: &str) -> String {
    const WORDS: [&str; 10] = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ];
    let number = usize::try_from(n)
        .ok()
        .and_then(|n| WORDS.get(n))
        .map_or_else(|| n.to_string(), |word| (*word).to_owned());
    let plural = if n == 1 { "" } else { "s" };
    format!("{number} {unit}{plural}")
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/health",
            get(|| async { asset(HEALTH_HTML, "text/html; charset=utf-8") }),
        )
        .route(
            "/health.js",
            get(|| async { asset(HEALTH_JS, "text/javascript") }),
        )
        .route("/api/health", get(report))
}

const HEALTH_HTML: &str = include_str!("health/health.html");
const HEALTH_JS: &str = include_str!("health/health.js");

/// `GET /api/health`: every check, for admins.
async fn report(
    State(state): State<AppState>,
    Authenticated(SignedIn { user, .. }): Authenticated,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !matches!(user.role, Role::Owner | Role::Admin) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Only admins can see the server's health. Ask your admin to check it.",
        ));
    }
    let checks = state.health.checks(SystemTime::now());
    let report = Report {
        state: checks
            .iter()
            .map(|check| check.state)
            .max()
            .unwrap_or(Level::Ok),
        checks,
    };
    Ok(Json(
        serde_json::to_value(report).expect("reports always serialize"),
    ))
}

#[cfg(test)]
mod tests {
    use accordion_gateway::acme::Validity;

    use super::*;

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    fn named(
        authority: Authority,
        certificate: CertificateStatus,
        address: Option<AddressStatus>,
    ) -> Health {
        Health {
            name: Some(NameHealth {
                name: "accordion.example.com".parse().unwrap(),
                authority,
                certificate: watch::channel(certificate).1,
                address: address.map(|address| watch::channel(address).1),
            }),
            local_name: None,
            storage: None,
            backup: None,
            updates: None,
        }
    }

    fn valid_for(left: Duration, renew_in: Duration) -> CertificateStatus {
        CertificateStatus {
            validity: Some(Validity {
                renew_at: now() + renew_in,
                expires_at: now() + left,
            }),
            ..CertificateStatus::default()
        }
    }

    fn certificate(health: &Health) -> Check {
        health.checks(now()).remove(0)
    }

    #[test]
    fn a_fresh_certificate_is_ok() {
        let check = certificate(&named(
            Authority::LetsEncrypt,
            valid_for(80 * DAY, 50 * DAY),
            None,
        ));
        assert_eq!(check.state, Level::Ok);
        assert_eq!(
            check.summary,
            "The certificate for accordion.example.com expires in 80 days. Accordion renews \
             it in 50 days."
        );
    }

    #[test]
    fn failed_renewals_warn_then_become_a_problem() {
        let failing = |left| CertificateStatus {
            failures: 3,
            last_error: Some("Accordion couldn't reach deSEC.".into()),
            retry_at: Some(now() + Duration::from_secs(2 * 60 * 60)),
            ..valid_for(left, Duration::ZERO)
        };

        let check = certificate(&named(Authority::LetsEncrypt, failing(20 * DAY), None));
        assert_eq!(check.state, Level::Warning);
        assert!(
            check.summary.ends_with(
                "The last try failed. Accordion couldn't reach deSEC. Accordion will try again \
                 in two hours."
            ),
            "{}",
            check.summary
        );

        let check = certificate(&named(Authority::LetsEncrypt, failing(3 * DAY), None));
        assert_eq!(check.state, Level::Problem);
        assert!(check.summary.contains("expires in three days."));

        let check = certificate(&named(
            Authority::LetsEncrypt,
            CertificateStatus {
                validity: Some(Validity {
                    renew_at: now() - 40 * DAY,
                    expires_at: now() - DAY,
                }),
                ..CertificateStatus::default()
            },
            None,
        ));
        assert_eq!(check.state, Level::Problem);
        assert!(check.summary.contains("has expired"));
    }

    #[test]
    fn a_test_certificate_is_a_warning() {
        let check = certificate(&named(
            Authority::LetsEncryptStaging,
            valid_for(80 * DAY, 50 * DAY),
            None,
        ));
        assert_eq!(check.state, Level::Warning);
        assert!(check.summary.contains("browsers don't trust"));
    }

    #[test]
    fn reports_the_address_record() {
        let address = |status| {
            named(
                Authority::LetsEncrypt,
                valid_for(80 * DAY, 50 * DAY),
                status,
            )
            .checks(now())
            .remove(1)
        };

        let check = address(Some(AddressStatus {
            published: Some("192.168.1.20".parse().unwrap()),
            ..AddressStatus::default()
        }));
        assert_eq!(check.state, Level::Ok);
        assert!(check.summary.contains("points at 192.168.1.20"));

        let check = address(Some(AddressStatus {
            failures: 1,
            last_error: Some("deSEC refused the change.".into()),
            retry_at: Some(now() + Duration::from_secs(60)),
            ..AddressStatus::default()
        }));
        assert_eq!(check.state, Level::Warning);
        assert!(
            check
                .summary
                .ends_with("deSEC refused the change. It'll try again in a minute."),
            "{}",
            check.summary
        );

        let check = address(None);
        assert_eq!(check.state, Level::Ok);
        assert!(check.summary.contains("ACCORDION_ADDRESS is off"));
    }

    #[test]
    fn reports_the_local_name() {
        let mut health = named(Authority::LetsEncrypt, valid_for(80 * DAY, 50 * DAY), None);
        let local = |health: &Health| health.checks(now()).pop().unwrap();

        health.local_name = Some(
            watch::channel(LocalNameStatus::Announcing {
                name: LOCAL_NAME.into(),
            })
            .1,
        );
        assert_eq!(local(&health).state, Level::Ok);

        health.local_name = Some(
            watch::channel(LocalNameStatus::Announcing {
                name: "accordion-2.local".into(),
            })
            .1,
        );
        let check = local(&health);
        assert_eq!(check.state, Level::Warning);
        assert!(check.summary.contains("http://accordion-2.local"));

        health.local_name = Some(
            watch::channel(LocalNameStatus::Failed {
                error: "port in use".into(),
            })
            .1,
        );
        assert!(
            local(&health)
                .summary
                .ends_with("at https://accordion.example.com.")
        );
    }

    #[test]
    fn reports_stored_files() {
        let check = |status: StorageStatus| {
            let health = Health {
                storage: Some(watch::channel(status).1),
                ..Health::default()
            };
            health.checks(now()).pop().unwrap()
        };

        let ok = check(StorageStatus::default());
        assert_eq!((ok.id, ok.state), ("storage", Level::Ok));

        let failed = check(StorageStatus {
            last_error: Some("No space left on device (os error 28)".into()),
            ..StorageStatus::default()
        });
        assert_eq!(failed.state, Level::Warning);
        assert!(
            failed
                .summary
                .contains("(No space left on device (os error 28)).")
        );

        let damaged = check(StorageStatus {
            damaged: 2,
            last_error: Some("ignored".into()),
            ..StorageStatus::default()
        });
        assert_eq!(damaged.state, Level::Problem);
        assert!(
            damaged
                .summary
                .starts_with("Accordion found two stored files")
        );
    }

    #[test]
    fn reports_backups() {
        use accordion_core::backup::{Outcome, Target, TargetKind, TargetStatus};

        let check = |status: BackupStatus| {
            let health = Health {
                backup: Some(watch::channel(status).1),
                ..Health::default()
            };
            health.checks(now()).pop().unwrap()
        };
        let usb = |last_backup: Option<Outcome>, last_good: Option<Duration>| TargetStatus {
            target: Target {
                id: accordion_types::BackupTargetId::generate(),
                name: "Blue USB disk".into(),
                kind: TargetKind::Folder,
                location: "/media/usb/backups".into(),
                created_at: "2026-10-22T00:00:00.000Z".into(),
                append_only: false,
            },
            last_backup,
            last_good_backup: last_good.map(|ago| now() - ago),
            last_check: None,
            last_restore_test: None,
            stored_bytes: None,
            oldest_backup: None,
        };
        let outcome = |ago: Duration, ok: bool| Outcome {
            finished_at: now() - ago,
            ok,
            problem: (!ok).then(|| "The disk is full.".to_owned()),
        };
        let on = |targets| BackupStatus {
            key: KeyState::Confirmed,
            hour: 3,
            running: false,
            targets,
            last_nightly: None,
        };

        let off = check(BackupStatus::default());
        assert_eq!((off.id, off.state), ("backups", Level::Warning));
        assert!(off.summary.starts_with("Backups are off"));
        assert_eq!(check(on(vec![])).state, Level::Warning);

        let waiting = check(on(vec![usb(None, None)]));
        assert_eq!(waiting.state, Level::Ok);
        assert!(waiting.summary.contains("starts at 3 a.m."));

        let hour = Duration::from_secs(60 * 60);
        let fine = check(on(vec![usb(Some(outcome(5 * hour, true)), Some(5 * hour))]));
        assert_eq!(fine.state, Level::Ok);
        assert_eq!(fine.summary, "Backed up to Blue USB disk five hours ago.");

        let failed_once = check(on(vec![usb(Some(outcome(hour, false)), Some(25 * hour))]));
        assert_eq!(failed_once.state, Level::Warning);
        assert!(failed_once.summary.contains("The disk is full."));

        let stale = check(on(vec![usb(Some(outcome(hour, false)), Some(3 * DAY))]));
        assert_eq!(stale.state, Level::Problem);
        assert!(stale.summary.contains("three days ago"));

        let mut tested = usb(Some(outcome(hour, true)), Some(hour));
        tested.last_restore_test = Some(outcome(hour, false));
        assert_eq!(check(on(vec![tested])).state, Level::Problem);

        let mut unconfirmed = on(vec![usb(Some(outcome(hour, true)), Some(hour))]);
        unconfirmed.key = KeyState::Unconfirmed;
        let unconfirmed = check(unconfirmed);
        assert_eq!(unconfirmed.state, Level::Warning);
        assert!(unconfirmed.summary.ends_with("restore a backup."));

        assert_eq!(hour_label(0), "midnight");
        assert_eq!(hour_label(15), "3 p.m.");
    }

    #[test]
    fn counts_in_words() {
        assert_eq!(in_words(Duration::from_secs(30)), "a minute");
        assert_eq!(in_words(Duration::from_secs(5 * 60)), "five minutes");
        assert_eq!(in_words(Duration::from_secs(90 * 60)), "one hour");
        assert_eq!(in_words(30 * DAY), "30 days");
    }
}
