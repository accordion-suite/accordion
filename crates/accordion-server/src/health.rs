//! Server health, in the web app's Settings: how the server's certificate,
//! its name's address record, `accordion.local`, stored files, backups, and
//! updates are doing, for the household admin.
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
use accordion_gateway::dns::ProviderKind;
use accordion_gateway::local_name::{LOCAL_NAME, LocalNameStatus};
use accordion_types::Role;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tokio::sync::watch;

use crate::AppState;
use crate::link_sharing::{LinkHealth, LinkSharing};
use crate::remote::{RemoteAccess, RemoteHealth};
use crate::updates::{CheckResult, UpdateStatus};
use crate::web::WebApp;
use accordion_core::texts::TextStatus;
use accordion_core::thumbnails::ThumbnailStatus;
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
    /// Set when the server has the web app.
    pub web: Option<WebApp>,
    /// Set once the thumbnail worker is running.
    pub thumbnails: Option<watch::Receiver<ThumbnailStatus>>,
    pub search: Option<watch::Receiver<TextStatus>>,
    /// Remote access, which reports once the server serves its name.
    pub remote: Option<RemoteAccess>,
    /// Link sharing, which reports once the server serves its routes.
    pub link_sharing: Option<LinkSharing>,
}

/// The server's name and the background work that keeps it usable.
#[derive(Clone)]
pub struct NameHealth {
    pub name: ServerName,
    pub authority: Authority,
    /// The DNS provider that holds the name's records.
    pub provider: ProviderKind,
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
        if let Some(web) = &self.web {
            checks.push(web_check(web));
        }
        if let Some(thumbnails) = &self.thumbnails {
            checks.push(thumbnails_check(&thumbnails.borrow()));
        }
        if let Some(search) = &self.search {
            checks.push(search_check(&search.borrow()));
        }
        if let Some(remote) = self.remote.as_ref().and_then(RemoteAccess::health) {
            checks.push(remote_check(&remote, now));
        }
        if let Some(links) = self.link_sharing.as_ref().and_then(LinkSharing::health) {
            checks.push(link_check(&links, now));
        }
        checks
    }
}

pub(crate) fn link_check(health: &LinkHealth, now: SystemTime) -> Check {
    let check = |state, summary| Check {
        id: "link_sharing",
        title: "Link sharing",
        state,
        summary,
    };
    if !health.on {
        return check(
            Level::Ok,
            "Link sharing is off, so files can be shared only with people who have an account. \
             An admin can turn it on in Settings, under Remote access."
                .to_owned(),
        );
    }
    if let Some(problem) = &health.listener.problem {
        return check(Level::Problem, problem.clone());
    }
    if !health.named {
        return check(
            Level::Ok,
            format!(
                "Link sharing is on for testing. This server doesn't have a name, so links work \
                 only on this network, over plain HTTP on port {}.",
                health.listen_port
            ),
        );
    }
    let mut state = Level::Ok;
    let mut summary = String::from("Link sharing is on.");
    let certificate = &health.certificate;
    match (&certificate.validity, &certificate.last_error) {
        (None, Some(error)) => {
            state = Level::Problem;
            let _ = write!(
                summary,
                " Accordion couldn't get the certificate for links, so they can't open yet. \
                 {error}"
            );
        }
        (None, None) => summary.push_str(" Accordion is getting the certificate for links."),
        (Some(validity), error) => {
            let left = validity.expires_at.duration_since(now).unwrap_or_default();
            if left < EXPIRY_PROBLEM {
                state = Level::Problem;
                let _ = write!(
                    summary,
                    " The certificate for links expires in {}.",
                    in_words(left)
                );
                if let Some(error) = error {
                    let _ = write!(summary, " Renewing it didn't work. {error}");
                }
            }
        }
    }
    let router = &health.router;
    let port = health.port;
    match router.mapping {
        Some(_) if router.shared_address => {
            state = state.max(Level::Warning);
            summary.push_str(
                " Your internet provider shares one public address among many homes, or the \
                 router is behind another router, so links can't reach people outside the home.",
            );
        }
        Some(mapping) => {
            let _ = write!(
                summary,
                " The router forwards TCP port {port} to the server, through {}.",
                mapping.method
            );
        }
        None if router.problem.is_some() => {
            state = state.max(Level::Warning);
            let home = health
                .home
                .map_or_else(|| "the server".to_owned(), |home| home.to_string());
            let _ = write!(
                summary,
                " The router didn't forward TCP port {port} by itself, so links may not reach \
                 people outside the home. In your router's settings, forward TCP port {port} to \
                 port {} on {home}.",
                health.listen_port
            );
        }
        None => summary.push_str(" Accordion is asking the router to forward the port."),
    }
    if let Some(problem) = &health.record.problem {
        state = state.max(Level::Warning);
        let _ = write!(
            summary,
            " Accordion couldn't update the address record for links. {problem}"
        );
    }
    check(state, summary)
}

/// A device that connected this recently shows the port works, even if the
/// router never said it forwards it.
const RECENTLY_CONNECTED: Duration = Duration::from_secs(24 * 60 * 60);

pub(crate) fn remote_check(health: &RemoteHealth, now: SystemTime) -> Check {
    let check = |state, summary| Check {
        id: "remote_access",
        title: "Remote access",
        state,
        summary,
    };
    if !health.on {
        return check(
            Level::Ok,
            "Remote access is off, so devices can reach the server only at home. An admin can \
             turn it on in Settings, under Remote access."
                .to_owned(),
        );
    }
    if let Some(problem) = &health.home_problem {
        return check(Level::Problem, problem.clone());
    }
    if let Some(problem) = &health.tunnel.problem {
        return check(Level::Problem, problem.clone());
    }
    let port = health.port.unwrap_or_default();
    let last = health.tunnel.connected.values().max().copied();
    let connected = last.map(|at| now.duration_since(at).unwrap_or_default());
    let mut state = Level::Ok;
    let mut summary = String::from("Remote access is on.");
    let router = &health.router;
    match router.mapping {
        _ if !health.named => summary.push_str(
            " This server doesn't have a name, so it works only for testing, and Accordion \
             doesn't ask the router to forward the port.",
        ),
        Some(_) if router.shared_address => {
            state = Level::Warning;
            summary.push_str(
                " Your internet provider shares one public address among many homes, or the \
                 router is behind another router, so devices away from home can't reach the \
                 server through it.",
            );
            if health.record.ipv6.is_some() {
                summary.push_str(" Devices on networks with IPv6 can still reach it.");
            }
            summary.push_str(
                " Other ways in are asking your provider for your own public address, a \
                 tailnet such as Tailscale, or a small server you rent as a relay.",
            );
        }
        Some(mapping) => {
            let _ = write!(
                summary,
                " The router forwards UDP port {} to the server, through {}.",
                mapping.external_port, mapping.method
            );
        }
        None if router.problem.is_some() => {
            let home = health
                .home
                .map_or_else(|| "the server".to_owned(), |home| home.to_string());
            if connected.is_some_and(|since| since < RECENTLY_CONNECTED) {
                let _ = write!(
                    summary,
                    " The router didn't say it forwards UDP port {port}, but a device connected \
                     from away, so the port works."
                );
            } else {
                state = Level::Warning;
                let _ = write!(
                    summary,
                    " The router didn't forward UDP port {port} by itself. In your router's \
                     settings, forward UDP port {port} to {home}. The router's manual or its \
                     maker's website shows where that setting is."
                );
            }
        }
        None => summary.push_str(" Accordion is asking the router to forward the port."),
    }
    if let Some(problem) = &health.record.problem {
        state = state.max(Level::Warning);
        let _ = write!(
            summary,
            " Accordion couldn't update the address record that devices find home at. {problem}"
        );
    }
    match connected {
        Some(since) => {
            let _ = write!(summary, " A device last connected {} ago.", in_words(since));
        }
        None => summary.push_str(" No device has connected since the server started."),
    }
    check(state, summary)
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

fn web_check(web: &WebApp) -> Check {
    let (state, summary) = match web.damaged() {
        [] => (
            Level::Ok,
            "The web app that shows Drive and Settings in a browser has all its files.".to_owned(),
        ),
        [file] => (
            Level::Problem,
            format!(
                "The web app's file {file} in {} is missing or changed, so Drive and Settings \
                 don't open in a browser. {}",
                web.dir().display(),
                web.fix()
            ),
        ),
        [first, ..] => (
            Level::Problem,
            format!(
                "Accordion found {} in {} missing or changed, such as {first}, so Drive and \
                 Settings don't open in a browser. {}",
                count(web.damaged().len() as u64, "web app file"),
                web.dir().display(),
                web.fix()
            ),
        ),
    };
    Check {
        id: "web_app",
        title: "Web app",
        state,
        summary,
    }
}

fn search_check(status: &TextStatus) -> Check {
    let (state, summary) = if let Some(reason) = &status.unavailable {
        (
            Level::Warning,
            format!(
                "Drive searches names and plain text files, but not inside other files, because \
                 {reason}"
            ),
        )
    } else if let Some(error) = &status.last_error {
        (
            Level::Warning,
            format!(
                "Accordion ran into a problem reading files for search ({error}). It'll try again."
            ),
        )
    } else {
        let mut summary = if status.waiting > 0 {
            let verb = if status.waiting == 1 { "is" } else { "are" };
            format!(
                "Accordion is reading files for search, and {} {verb} waiting. Until then, \
                 they're found by name.",
                count(status.waiting, "file")
            )
        } else {
            "Drive can search inside every file it can read.".to_owned()
        };
        if status.failed > 0 {
            summary.push_str(&format!(
                " Accordion couldn't read {}, so {} found by name only. The server's log says why.",
                count(status.failed, "file"),
                if status.failed == 1 {
                    "it's"
                } else {
                    "they're"
                }
            ));
        }
        // Not a warning: LibreOffice is optional.
        if let Some(documents) = &status.documents {
            summary.push_str(&format!(
                " Older Office files, such as .doc files, are found by name only, because \
                 {documents}"
            ));
        }
        (Level::Ok, summary)
    };
    Check {
        id: "search",
        title: "Search",
        state,
        summary,
    }
}

fn thumbnails_check(status: &ThumbnailStatus) -> Check {
    let (state, summary) = if let Some(reason) = &status.unavailable {
        (
            Level::Warning,
            format!("Drive shows icons instead of thumbnails, because {reason}"),
        )
    } else if let Some(error) = &status.last_error {
        (
            Level::Warning,
            format!("Accordion ran into a problem making thumbnails ({error}). It'll try again."),
        )
    } else {
        let mut summary = if status.waiting > 0 {
            let verb = if status.waiting == 1 { "is" } else { "are" };
            format!(
                "Accordion is making thumbnails, and {} {verb} waiting.",
                count(status.waiting, "file")
            )
        } else {
            "Every file that can have a thumbnail has one.".to_owned()
        };
        if status.failed > 0 {
            summary.push_str(&format!(
                " Accordion couldn't make thumbnails of {}, so {} an icon instead. The server's log \
                 says why.",
                count(status.failed, "file"),
                if status.failed == 1 { "it shows" } else { "they show" }
            ));
        }
        // Not warnings: ffmpeg and LibreOffice are optional, and an admin
        // may leave them out.
        if let Some(videos) = &status.videos {
            summary.push_str(&format!(" Videos show an icon instead, because {videos}"));
        }
        if let Some(documents) = &status.documents {
            summary.push_str(&format!(
                " Office documents show an icon instead, because {documents}"
            ));
        }
        (Level::Ok, summary)
    };
    Check {
        id: "thumbnails",
        title: "Thumbnails",
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
             them on in Backups settings."
                .to_owned(),
        );
    }
    if status.targets.is_empty() {
        return check(
            Level::Warning,
            "Backups are on, but there's nowhere for them to go. Add a location, such as a USB \
             disk, in Backups settings."
                .to_owned(),
        );
    }

    if status.targets.iter().all(|target| target.target.paused) {
        return check(
            Level::Warning,
            "Backups are paused for every location, so nothing is being backed up. Turn one \
             back on in Network settings."
                .to_owned(),
        );
    }

    let mut state = Level::Ok;
    let mut notes = Vec::new();
    for target in &status.targets {
        let name = &target.target.name;
        if target.target.paused {
            notes.push(format!("Backups to {name} are paused in Network settings."));
            continue;
        }
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
            "Confirm in Backups settings that you've saved the recovery key. You'll need it to \
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
        if !status.check_on {
            return check(
                Level::Ok,
                format!(
                    "This server runs Accordion {installed}. Accordion doesn't check for updates \
                     on its own, because the update check is turned off in Network settings."
                ),
            );
        }
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
        format!("{available}{urgency} Updates in Settings says how to install it."),
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
    Router::new().route("/api/health", get(report))
}

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
                provider: ProviderKind::Desec,
                certificate: watch::channel(certificate).1,
                address: address.map(|address| watch::channel(address).1),
            }),
            local_name: None,
            storage: None,
            backup: None,
            updates: None,
            web: None,
            thumbnails: None,
            search: None,
            remote: None,
            link_sharing: None,
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
                paused: false,
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
    fn says_how_thumbnails_are_doing() {
        let check = thumbnails_check(&ThumbnailStatus {
            unavailable: Some("the programs that make thumbnails aren't installed.".into()),
            ..ThumbnailStatus::default()
        });
        assert_eq!(check.state, Level::Warning);
        assert_eq!(
            check.summary,
            "Drive shows icons instead of thumbnails, because the programs that make \
             thumbnails aren't installed."
        );

        let check = thumbnails_check(&ThumbnailStatus {
            waiting: 3,
            failed: 1,
            ..ThumbnailStatus::default()
        });
        assert_eq!(check.state, Level::Ok);
        assert_eq!(
            check.summary,
            "Accordion is making thumbnails, and three files are waiting. Accordion couldn't \
             make thumbnails of one file, so it shows an icon instead. The server's log says why."
        );

        let check = thumbnails_check(&ThumbnailStatus::default());
        assert_eq!(
            check.summary,
            "Every file that can have a thumbnail has one."
        );

        let check = thumbnails_check(&ThumbnailStatus {
            videos: Some(
                "ffmpeg isn't installed. Install it with apt to show a frame from each video."
                    .into(),
            ),
            ..ThumbnailStatus::default()
        });
        assert_eq!(check.state, Level::Ok);
        assert_eq!(
            check.summary,
            "Every file that can have a thumbnail has one. Videos show an icon instead, because \
             ffmpeg isn't installed. Install it with apt to show a frame from each video."
        );

        let check = thumbnails_check(&ThumbnailStatus {
            documents: Some("LibreOffice isn't installed.".into()),
            ..ThumbnailStatus::default()
        });
        assert_eq!(check.state, Level::Ok);
        assert_eq!(
            check.summary,
            "Every file that can have a thumbnail has one. Office documents show an icon \
             instead, because LibreOffice isn't installed."
        );
    }

    #[test]
    fn says_how_search_is_doing() {
        let check = search_check(&TextStatus {
            unavailable: Some("pdftotext isn't installed. Install poppler-utils with apt.".into()),
            ..TextStatus::default()
        });
        assert_eq!(check.state, Level::Warning);
        assert_eq!(
            check.summary,
            "Drive searches names and plain text files, but not inside other files, because \
             pdftotext isn't installed. Install poppler-utils with apt."
        );

        let check = search_check(&TextStatus {
            waiting: 1,
            failed: 2,
            documents: Some("LibreOffice isn't installed.".into()),
            ..TextStatus::default()
        });
        assert_eq!(check.state, Level::Ok);
        assert_eq!(
            check.summary,
            "Accordion is reading files for search, and one file is waiting. Until then, they're \
             found by name. Accordion couldn't read two files, so they're found by name only. The \
             server's log says why. Older Office files, such as .doc files, are found by name \
             only, because LibreOffice isn't installed."
        );

        let check = search_check(&TextStatus::default());
        assert_eq!(
            check.summary,
            "Drive can search inside every file it can read."
        );
    }

    #[test]
    fn says_how_remote_access_is_doing() {
        use accordion_gateway::endpoint::EndpointStatus;
        use accordion_gateway::port_mapping::{Mapping, Method, PortStatus};
        use accordion_gateway::tunnel::{Key, TunnelStatus};

        let healthy = || RemoteHealth {
            on: true,
            named: true,
            port: Some(51820),
            home: Some("192.168.1.20".parse().unwrap()),
            home_problem: None,
            tunnel: TunnelStatus {
                listening: Some(51820),
                ..TunnelStatus::default()
            },
            router: PortStatus {
                port: Some(51820),
                mapping: Some(Mapping {
                    method: Method::Pcp,
                    external_address: Some("203.0.113.7".parse().unwrap()),
                    external_port: 51820,
                    lifetime: Duration::from_secs(3600),
                }),
                ..PortStatus::default()
            },
            record: EndpointStatus::default(),
        };
        let check = remote_check(&healthy(), now());
        assert_eq!(check.state, Level::Ok);
        assert_eq!(
            check.summary,
            "Remote access is on. The router forwards UDP port 51820 to the server, through \
             PCP. No device has connected since the server started."
        );

        // The router said no, and nobody has connected, so the admin
        // forwards the port by hand.
        let mut refused = healthy();
        refused.router = PortStatus {
            port: Some(51820),
            problem: Some("The router said no.".into()),
            ..PortStatus::default()
        };
        let check = remote_check(&refused, now());
        assert_eq!(check.state, Level::Warning);
        assert!(
            check
                .summary
                .contains("forward UDP port 51820 to 192.168.1.20")
        );

        // A device that connected shows the port works anyway.
        refused.tunnel.connected.insert(
            Key::generate().public(),
            now() - Duration::from_secs(5 * 60),
        );
        let check = remote_check(&refused, now());
        assert_eq!(check.state, Level::Ok);
        assert!(
            check
                .summary
                .ends_with("A device last connected five minutes ago.")
        );

        let mut shared = healthy();
        shared.router.shared_address = true;
        let check = remote_check(&shared, now());
        assert_eq!(check.state, Level::Warning);
        assert!(
            check
                .summary
                .contains("shares one public address among many homes")
        );

        let mut taken = healthy();
        taken.tunnel.problem = Some("The port is taken.".into());
        assert_eq!(remote_check(&taken, now()).state, Level::Problem);

        let off = RemoteHealth {
            on: false,
            ..healthy()
        };
        assert!(
            remote_check(&off, now())
                .summary
                .starts_with("Remote access is off")
        );
    }

    #[test]
    fn says_how_link_sharing_is_doing() {
        use crate::link_sharing::ListenerStatus;
        use accordion_gateway::acme::Validity;
        use accordion_gateway::endpoint::EndpointStatus;
        use accordion_gateway::port_mapping::{Mapping, Method, PortStatus};

        let healthy = || LinkHealth {
            on: true,
            named: true,
            port: 443,
            listen_port: 8443,
            home: Some("192.168.1.20".parse().unwrap()),
            listener: ListenerStatus {
                listening: true,
                problem: None,
            },
            certificate: CertificateStatus {
                validity: Some(Validity {
                    expires_at: now() + Duration::from_secs(60 * 24 * 60 * 60),
                    renew_at: now() + Duration::from_secs(30 * 24 * 60 * 60),
                }),
                ..CertificateStatus::default()
            },
            router: PortStatus {
                port: Some(443),
                mapping: Some(Mapping {
                    method: Method::Upnp,
                    external_address: Some("203.0.113.7".parse().unwrap()),
                    external_port: 443,
                    lifetime: Duration::from_secs(3600),
                }),
                ..PortStatus::default()
            },
            record: EndpointStatus::default(),
        };
        let check = link_check(&healthy(), now());
        assert_eq!(check.state, Level::Ok);
        assert_eq!(
            check.summary,
            "Link sharing is on. The router forwards TCP port 443 to the server, through UPnP."
        );

        let mut refused = healthy();
        refused.router = PortStatus {
            port: Some(443),
            problem: Some("The router said no.".into()),
            ..PortStatus::default()
        };
        let check = link_check(&refused, now());
        assert_eq!(check.state, Level::Warning);
        assert!(
            check
                .summary
                .contains("forward TCP port 443 to port 8443 on 192.168.1.20")
        );

        let mut no_certificate = healthy();
        no_certificate.certificate = CertificateStatus {
            last_error: Some("deSEC refused the change.".into()),
            ..CertificateStatus::default()
        };
        assert_eq!(link_check(&no_certificate, now()).state, Level::Problem);

        let off = LinkHealth {
            on: false,
            ..healthy()
        };
        assert!(
            link_check(&off, now())
                .summary
                .starts_with("Link sharing is off")
        );
    }

    #[test]
    fn counts_in_words() {
        assert_eq!(in_words(Duration::from_secs(30)), "a minute");
        assert_eq!(in_words(Duration::from_secs(5 * 60)), "five minutes");
        assert_eq!(in_words(Duration::from_secs(90 * 60)), "one hour");
        assert_eq!(in_words(30 * DAY), "30 days");
    }
}
