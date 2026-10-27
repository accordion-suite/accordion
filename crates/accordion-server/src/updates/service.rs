//! Checks once a day for a newer release, and asks the update helper to
//! install it (decision 12).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use accordion_core::backup::BackupStatus;
use accordion_core::db::rusqlite::{self, params};
use accordion_core::db::{self, Database, Migration};
use accordion_update::protocol::{HELPER, INBOX, Outcome, Report};
use accordion_update::{
    MANIFEST_FILE, PublicKey, SIGNATURE_FILE, SUMS_FILE, SignedSums, Version, release_key,
};
use hyper::Uri;
use serde::Deserialize;
use tokio::sync::{Mutex, Notify, watch};

use super::fetch::{Fetched, Fetcher};

/// Where releases are, without the file name. GitHub sends this address to
/// the newest full release, skipping pre-releases.
const GITHUB: &str = "https://github.com/accordion-suite/accordion/releases/latest/download";

const DAY: Duration = Duration::from_secs(24 * 60 * 60);
/// Checks are about a day apart, and never closer than this.
const MIN_GAP: Duration = Duration::from_secs(12 * 60 * 60);
/// After a check that didn't work, such as while the internet is down.
const RETRY: Duration = Duration::from_secs(4 * 60 * 60);
/// The first check on a new server comes between these times after it
/// starts, so a server that's started for a moment, such as in a test,
/// doesn't check.
const FIRST_CHECK_SOONEST: Duration = Duration::from_secs(5 * 60);
const FIRST_CHECK_LATEST: Duration = Duration::from_secs(60 * 60);
/// Automatic updates wait this long after a release is first seen, so a bad
/// release can be replaced before most servers install it. Security
/// releases don't wait.
pub const WAIT: Duration = Duration::from_secs(3 * 24 * 60 * 60);
/// How often to look for the helper's result while it's installing.
const RESULT_POLL: Duration = Duration::from_secs(60);
/// The helper takes at most about 40 minutes, including a rollback.
pub(super) const HELPER_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// How much of the helper's history the page shows.
const HISTORY: usize = 10;

/// Each file is small. The limits keep a bad download from filling memory.
const SUMS_LIMIT: usize = 64 * 1024;
const SIGNATURE_LIMIT: usize = 1024;
const MANIFEST_LIMIT: usize = 64 * 1024;
const NOTES_LIMIT: usize = 4000;

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "update checks",
        sql: "
        -- One row: when this server checks, and what it last found.
        CREATE TABLE update_state (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            -- The minute of the day, in UTC, that this server checks at.
            -- Picked at random, so servers don't all check at once.
            check_minute INTEGER NOT NULL CHECK (check_minute BETWEEN 0 AND 1439),
            -- Milliseconds since 1970.
            last_check_at INTEGER,
            -- Why the last check didn't work, or NULL if it did.
            last_check_problem TEXT,
            -- The newest release's update.json, SHA256SUMS, and signature,
            -- as downloaded, so they can be checked again before installing.
            newest_manifest BLOB,
            newest_sums BLOB,
            newest_signature BLOB,
            -- When this server first saw that release.
            newest_first_seen INTEGER
        );
        ",
    },
    Migration {
        version: 2,
        name: "installing updates",
        sql: "
        -- 1 installs updates after the nightly backup, 0 waits for an admin.
        ALTER TABLE update_state ADD COLUMN automatic INTEGER NOT NULL DEFAULT 1
            CHECK (automatic IN (0, 1));
        -- Where the newest release's files are, to download its package.
        ALTER TABLE update_state ADD COLUMN newest_folder TEXT;
        -- The release the update helper was asked to install, and when,
        -- until it says how that went.
        ALTER TABLE update_state ADD COLUMN requested_version TEXT;
        ALTER TABLE update_state ADD COLUMN requested_at INTEGER;
        -- What the update helper did with each request.
        CREATE TABLE update_history (
            id INTEGER PRIMARY KEY,
            version TEXT NOT NULL,
            from_version TEXT NOT NULL,
            outcome TEXT NOT NULL CHECK (outcome IN ('installed', 'rolled-back', 'refused')),
            reason TEXT,
            -- Milliseconds since 1970.
            finished_at INTEGER NOT NULL
        );
        ",
    },
];

/// Where to look for releases, and the key they must be signed with.
#[derive(Clone)]
pub struct UpdateSource {
    /// The folder of the newest release, without a trailing `/`.
    pub base: String,
    pub key: PublicKey,
    /// For tests against a local server. Releases come over HTTPS.
    pub allow_http: bool,
    /// The update helper, which only exists where the package installed it.
    pub helper: PathBuf,
}

impl UpdateSource {
    pub fn github() -> Self {
        Self {
            base: GITHUB.to_owned(),
            key: release_key(),
            allow_http: false,
            helper: PathBuf::from(HELPER),
        }
    }

    /// GitHub, or the folder in `ACCORDION_UPDATE_URL`, for trying a test
    /// release on a test machine. Either way, releases have to be signed
    /// with the key built into this program.
    pub fn from_env() -> Result<Self, String> {
        let mut source = Self::github();
        if let Ok(url) = std::env::var("ACCORDION_UPDATE_URL") {
            let url = url.trim().trim_end_matches('/');
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err(format!(
                    "ACCORDION_UPDATE_URL is \"{url}\". Set it to the web address of a folder \
                     of release files, such as http://192.168.1.20:8000/release."
                ));
            }
            source.allow_http = url.starts_with("http://");
            source.base = url.to_owned();
        }
        Ok(source)
    }
}

/// A release, as its `update.json` describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// Such as `2026-11-02`.
    pub published: String,
    pub security: bool,
    /// False when the release needs something new from the distribution, so
    /// it has to be installed with the install command.
    pub installs_itself: bool,
    pub notes: String,
    /// The full release notes.
    pub link: String,
    pub first_seen: SystemTime,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckResult {
    pub at: SystemTime,
    /// Why it didn't work, in plain words, or `None` if it did.
    pub problem: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installing {
    pub version: String,
    pub since: SystemTime,
}

#[derive(Clone, Debug)]
pub struct UpdateStatus {
    /// The version running now.
    pub installed: Version,
    /// The newest release the last working check found.
    pub newest: Option<Release>,
    pub last_check: Option<CheckResult>,
    pub checking: bool,
    /// Updates install after the nightly backup, rather than waiting for an
    /// admin.
    pub automatic: bool,
    /// The update helper is installed, so this server can install updates
    /// itself.
    pub can_install: bool,
    /// An update that's downloading, or that the helper is installing.
    pub installing: Option<Installing>,
    /// Why the last update didn't reach the helper, such as a download that
    /// stopped partway.
    pub install_problem: Option<String>,
    /// What the helper did with each update, newest first.
    pub history: Vec<Report>,
}

impl UpdateStatus {
    /// The release waiting to be installed, if there's one newer than this
    /// server.
    pub fn waiting(&self) -> Option<&Release> {
        self.newest
            .as_ref()
            .filter(|release| release.version > self.installed)
    }

    /// The waiting release, if this server can install it itself rather
    /// than with the install command.
    pub fn installable(&self) -> Option<&Release> {
        self.waiting()
            .filter(|release| release.installs_itself && self.can_install)
    }

    /// The helper's last word on `version`, if it didn't install it.
    pub fn failed(&self, version: &Version) -> Option<&Report> {
        let version = version.to_string();
        self.history
            .iter()
            .find(|report| report.version == version)
            .filter(|report| report.outcome != Outcome::Installed)
    }

    /// When the waiting release installs on its own, at the earliest, if it
    /// will: after the nightly backup on or after this time.
    pub fn installs_from(&self) -> Option<SystemTime> {
        if !self.automatic {
            return None;
        }
        let release = self.installable()?;
        if self.failed(&release.version).is_some() {
            return None;
        }
        Some(if release.security {
            release.first_seen
        } else {
            release.first_seen + WAIT
        })
    }

    /// Whether to install the waiting release now, after a nightly backup.
    pub fn due(&self, now: SystemTime) -> bool {
        self.installing.is_none() && self.installs_from().is_some_and(|from| from <= now)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Db(#[from] db::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Setup(String),
    #[error("There's no newer release to install.")]
    NothingWaiting,
    #[error(
        "Accordion {0} needs something new from the system, so this page can't install it. \
         Install it with the install command instead."
    )]
    NeedsInstallCommand(String),
    #[error(
        "This server can't install updates itself, because Accordion's update helper isn't \
         installed. Install the update with the install command instead."
    )]
    NoHelper,
    #[error("Accordion is already installing {0}.")]
    Busy(String),
    #[error("Accordion couldn't download the update: {0}. Try again later.")]
    Download(String),
}

/// The update checker. Cloning it is cheap.
#[derive(Clone)]
pub struct Updates {
    pub(super) inner: Arc<Inner>,
}

pub(super) struct Inner {
    pub(super) db: Database,
    pub(super) source: UpdateSource,
    pub(super) fetcher: Fetcher,
    check_minute: u32,
    pub(super) status: watch::Sender<UpdateStatus>,
    /// One check or download at a time, whether scheduled or from the
    /// updates page.
    pub(super) busy: Mutex<()>,
    /// Where the server leaves updates for the helper.
    pub(super) inbox: PathBuf,
    /// Wakes the schedule after the page asks for an update.
    pub(super) wake: Notify,
}

/// What `update.json` holds.
#[derive(Deserialize)]
struct Manifest {
    version: String,
    published: String,
    security: bool,
    installs_itself: bool,
    notes: String,
    link: String,
}

impl Manifest {
    fn read(bytes: &[u8]) -> Result<Self, String> {
        let manifest: Self = serde_json::from_slice(bytes).map_err(|_| {
            "The release's update.json isn't in the format Accordion expects".to_owned()
        })?;
        let date = manifest.published.as_bytes();
        let date_ok = date.len() == 10
            && date.iter().enumerate().all(|(i, &b)| match i {
                4 | 7 => b == b'-',
                _ => b.is_ascii_digit(),
            });
        if !date_ok || manifest.notes.len() > NOTES_LIMIT || !manifest.link.starts_with("https://")
        {
            return Err("The release's update.json isn't in the format Accordion expects".into());
        }
        Ok(manifest)
    }

    fn release(self, first_seen: SystemTime) -> Result<Release, String> {
        Ok(Release {
            version: self
                .version
                .parse()
                .map_err(|err: accordion_update::Error| err.to_string())?,
            published: self.published,
            security: self.security,
            installs_itself: self.installs_itself,
            notes: self.notes,
            link: self.link,
            first_seen,
        })
    }
}

/// What a working check downloaded.
struct Found {
    /// The address of the release's folder, without a trailing `/`.
    folder: String,
    manifest: Vec<u8>,
    sums: Vec<u8>,
    signature: Vec<u8>,
    release: Release,
}

/// The checker's row, as stored.
struct Stored {
    check_minute: u32,
    last_check: Option<CheckResult>,
    newest: Option<(Vec<u8>, i64)>,
    automatic: bool,
    installing: Option<Installing>,
}

pub(super) fn read_history(conn: &rusqlite::Connection) -> Result<Vec<Report>, rusqlite::Error> {
    let mut statement = conn.prepare(
        "SELECT version, from_version, outcome, reason, finished_at FROM update_history
         ORDER BY id DESC LIMIT ?1",
    )?;
    statement
        .query_map([HISTORY as i64], |row| {
            let outcome: String = row.get(2)?;
            Ok(Report {
                version: row.get(0)?,
                from: row.get(1)?,
                outcome: Outcome::parse(&outcome).unwrap_or(Outcome::Refused),
                reason: row.get(3)?,
                finished_at: from_millis(row.get(4)?),
            })
        })?
        .collect()
}

pub(super) fn millis(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
    })
}

pub(super) fn from_millis(millis: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(u64::try_from(millis).unwrap_or(0))
}

impl Updates {
    /// Opens the checker's state. `installed` is the version running now,
    /// and `data_dir` is the data folder, where the helper's inbox is.
    pub async fn open(
        db: Database,
        source: UpdateSource,
        installed: Version,
        data_dir: &Path,
    ) -> Result<Self, Error> {
        db.migrate("updates", MIGRATIONS).await?;
        let fetcher = Fetcher::new(source.allow_http).map_err(Error::Setup)?;
        let minute = {
            let mut bytes = [0; 4];
            getrandom::fill(&mut bytes).expect("the OS random number generator failed");
            u32::from_le_bytes(bytes) % 1440
        };
        let (state, history) = db
            .write(move |tx| {
                tx.execute(
                    "INSERT OR IGNORE INTO update_state (id, check_minute) VALUES (1, ?1)",
                    [minute],
                )?;
                let state = tx.query_row(
                    "SELECT check_minute, last_check_at, last_check_problem, newest_manifest,
                            newest_first_seen, automatic, requested_version, requested_at
                     FROM update_state WHERE id = 1",
                    [],
                    |row| {
                        let at: Option<i64> = row.get(1)?;
                        let problem: Option<String> = row.get(2)?;
                        let last_check = at.map(|at| CheckResult {
                            at: from_millis(at),
                            problem,
                        });
                        let manifest: Option<Vec<u8>> = row.get(3)?;
                        let first_seen: Option<i64> = row.get(4)?;
                        let requested: Option<String> = row.get(6)?;
                        let requested_at: Option<i64> = row.get(7)?;
                        Ok(Stored {
                            check_minute: row.get(0)?,
                            last_check,
                            newest: manifest.zip(first_seen),
                            automatic: row.get(5)?,
                            installing: requested.zip(requested_at).map(|(version, at)| {
                                Installing {
                                    version,
                                    since: from_millis(at),
                                }
                            }),
                        })
                    },
                )?;
                let history = read_history(tx)?;
                Ok::<_, Error>((state, history))
            })
            .await?;
        // A manifest that no longer reads, such as one an older version
        // wrote, is dropped. The next check finds the newest release again.
        let newest = state.newest.and_then(|(manifest, first_seen)| {
            Manifest::read(&manifest)
                .and_then(|manifest| manifest.release(from_millis(first_seen)))
                .ok()
        });
        let can_install =
            source.helper.is_file() && accordion_update::debian_architecture().is_some();
        let (status, _) = watch::channel(UpdateStatus {
            installed,
            newest,
            last_check: state.last_check,
            checking: false,
            automatic: state.automatic,
            can_install,
            installing: state.installing,
            install_problem: None,
            history,
        });
        let updates = Self {
            inner: Arc::new(Inner {
                db,
                source,
                fetcher,
                check_minute: state.check_minute,
                status,
                busy: Mutex::new(()),
                inbox: data_dir.join(INBOX),
                wake: Notify::new(),
            }),
        };
        // The helper may have finished while the server was stopped for it.
        updates.collect_result().await?;
        Ok(updates)
    }

    pub fn status(&self) -> watch::Receiver<UpdateStatus> {
        self.inner.status.subscribe()
    }

    /// Checks on schedule, installs updates after the nightly backup, and
    /// collects the helper's results, until `stopped` completes. `backup` is
    /// the backup status, where nightly backups show up.
    pub async fn run(
        self,
        mut backup: Option<watch::Receiver<BackupStatus>>,
        stopped: impl Future<Output = ()>,
    ) {
        enum Event {
            Stop,
            Check,
            Collect,
            Nightly(bool),
            Wake,
        }
        tokio::pin!(stopped);
        let mut seen_nightly = backup.as_ref().and_then(|backup| {
            backup
                .borrow()
                .last_nightly
                .as_ref()
                .map(|night| night.finished_at)
        });
        loop {
            let wait = self.until_next_check(SystemTime::now());
            let collect = if self.inner.status.borrow().installing.is_some() {
                RESULT_POLL
            } else {
                DAY
            };
            let event = tokio::select! {
                () = &mut stopped => Event::Stop,
                () = tokio::time::sleep(wait) => Event::Check,
                () = tokio::time::sleep(collect) => Event::Collect,
                () = self.inner.wake.notified() => Event::Wake,
                changed = async {
                    match backup.as_mut() {
                        Some(backup) => backup.changed().await.is_ok(),
                        None => std::future::pending().await,
                    }
                } => Event::Nightly(changed),
            };
            let result = match event {
                Event::Stop => return,
                Event::Wake => Ok(()),
                Event::Check => self.check().await.map(drop),
                Event::Collect => self.collect_result().await,
                Event::Nightly(false) => {
                    backup = None;
                    Ok(())
                }
                Event::Nightly(true) => {
                    let night = backup
                        .as_ref()
                        .and_then(|backup| backup.borrow().last_nightly.clone());
                    match night {
                        Some(night) if Some(night.finished_at) != seen_nightly => {
                            seen_nightly = Some(night.finished_at);
                            self.after_nightly(&night).await;
                        }
                        _ => {}
                    }
                    Ok(())
                }
            };
            if let Err(err) = result {
                tracing::error!(%err, "couldn't record update work");
                // Don't spin on a broken database.
                tokio::time::sleep(RETRY).await;
            }
        }
    }

    fn until_next_check(&self, now: SystemTime) -> Duration {
        let status = self.inner.status.borrow();
        let next = match &status.last_check {
            None => {
                let mut bytes = [0; 4];
                getrandom::fill(&mut bytes).expect("the OS random number generator failed");
                let range = FIRST_CHECK_LATEST - FIRST_CHECK_SOONEST;
                let spread = u64::from(u32::from_le_bytes(bytes)) % range.as_secs();
                now + FIRST_CHECK_SOONEST + Duration::from_secs(spread)
            }
            Some(CheckResult {
                at,
                problem: Some(_),
            }) => *at + RETRY,
            Some(CheckResult { at, problem: None }) => {
                next_daily(*at + MIN_GAP, self.inner.check_minute)
            }
        };
        next.duration_since(now).unwrap_or_default()
    }

    /// Checks for a newer release now, and records what it found. A check
    /// that doesn't work isn't an error here: its problem goes in the status
    /// for the updates page and the health page. Only failing to record it
    /// is.
    pub async fn check(&self) -> Result<CheckResult, Error> {
        let _one_at_a_time = self.inner.busy.lock().await;
        self.inner
            .status
            .send_modify(|status| status.checking = true);
        // To the millisecond, as the database keeps it.
        let now = from_millis(millis(SystemTime::now()));
        let found = self.fetch_newest(now).await;
        let previous = self.inner.status.borrow().newest.clone();
        let result = self.record(found, previous, now).await;
        self.inner
            .status
            .send_modify(|status| status.checking = false);
        result
    }

    async fn record(
        &self,
        found: Result<Option<Found>, String>,
        previous: Option<Release>,
        now: SystemTime,
    ) -> Result<CheckResult, Error> {
        let result = CheckResult {
            at: now,
            problem: found.as_ref().err().cloned(),
        };
        let at = millis(now);
        let problem = result.problem.clone();
        match found {
            Err(_) => {
                self.inner
                    .db
                    .write(move |tx| {
                        tx.execute(
                            "UPDATE update_state SET last_check_at = ?1, last_check_problem = ?2
                             WHERE id = 1",
                            params![at, problem],
                        )
                        .map_err(Error::from)
                    })
                    .await?;
                self.inner
                    .status
                    .send_modify(|status| status.last_check = Some(result.clone()));
            }
            Ok(found) => {
                // The same release keeps the day it was first seen, which is
                // when the wait before installing it automatically starts.
                let newest = found.map(|mut found| {
                    if let Some(previous) = &previous
                        && previous.version == found.release.version
                    {
                        found.release.first_seen = previous.first_seen;
                    }
                    found
                });
                let (manifest, sums, signature, first_seen, folder) = match &newest {
                    Some(found) => (
                        Some(found.manifest.clone()),
                        Some(found.sums.clone()),
                        Some(found.signature.clone()),
                        Some(millis(found.release.first_seen)),
                        Some(found.folder.clone()),
                    ),
                    None => (None, None, None, None, None),
                };
                self.inner
                    .db
                    .write(move |tx| {
                        tx.execute(
                            "UPDATE update_state SET last_check_at = ?1, last_check_problem = NULL,
                                 newest_manifest = ?2, newest_sums = ?3, newest_signature = ?4,
                                 newest_first_seen = ?5, newest_folder = ?6
                             WHERE id = 1",
                            params![at, manifest, sums, signature, first_seen, folder],
                        )
                        .map_err(Error::from)
                    })
                    .await?;
                self.inner.status.send_modify(|status| {
                    status.last_check = Some(result.clone());
                    status.newest = newest.map(|found| found.release);
                });
            }
        }
        Ok(result)
    }

    /// Downloads and checks the newest release's description. `None` means
    /// there's no full release yet.
    async fn fetch_newest(&self, now: SystemTime) -> Result<Option<Found>, String> {
        let source = &self.inner.source;
        let fetcher = &self.inner.fetcher;
        let sums_url = format!("{}/{SUMS_FILE}", source.base);
        let (sums, first_redirect) = match fetcher.get(&sums_url, SUMS_LIMIT).await? {
            Fetched::NotFound => return Ok(None),
            Fetched::Found {
                body,
                first_redirect,
            } => (body, first_redirect),
        };
        // GitHub's first redirect names the release, such as
        // .../download/v0.2.0/SHA256SUMS. The other files come from the same
        // release, even if a newer one is published partway through.
        let folder = first_redirect
            .as_ref()
            .map(Uri::to_string)
            .and_then(|url| {
                url.strip_suffix(&format!("/{SUMS_FILE}"))
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| source.base.clone());
        let signature = match fetcher
            .get(&format!("{folder}/{SIGNATURE_FILE}"), SIGNATURE_LIMIT)
            .await?
        {
            Fetched::Found { body, .. } => body,
            Fetched::NotFound => {
                return Err(
                    "The newest release isn't signed yet, so Accordion can't use it".into(),
                );
            }
        };
        let signed =
            SignedSums::verify(&sums, &signature, &source.key).map_err(|err| err.to_string())?;
        let manifest = match fetcher
            .get(&format!("{folder}/{MANIFEST_FILE}"), MANIFEST_LIMIT)
            .await?
        {
            Fetched::Found { body, .. } => body,
            Fetched::NotFound => {
                return Err(format!(
                    "The newest release has no {MANIFEST_FILE}, so Accordion can't tell what's in \
                     it"
                ));
            }
        };
        signed
            .check(MANIFEST_FILE, &manifest)
            .map_err(|err| err.to_string())?;
        let release = Manifest::read(&manifest)?.release(now)?;
        Ok(Some(Found {
            folder,
            manifest: manifest.to_vec(),
            sums: sums.to_vec(),
            signature: signature.to_vec(),
            release,
        }))
    }
}

/// The first time at or after `after` whose minute of the day, in UTC, is
/// `minute`.
fn next_daily(after: SystemTime, minute: u32) -> SystemTime {
    let secs = after
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let day_start = secs - secs % DAY.as_secs();
    let mut next = day_start + u64::from(minute) * 60;
    if next < secs {
        next += DAY.as_secs();
    }
    UNIX_EPOCH + Duration::from_secs(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_at_the_servers_minute_at_least_12_hours_apart() {
        let at = |secs: u64| UNIX_EPOCH + Duration::from_secs(secs);
        let day = DAY.as_secs();
        // A check at 03:00 UTC on day 100, by a server that checks at 02:30,
        // which is too soon for that day.
        let last = at(100 * day + 3 * 3600);
        assert_eq!(next_daily(last + MIN_GAP, 150), at(101 * day + 150 * 60));
        // A server that checks at 20:00 or 15:00 checks again that day, after
        // its first check came early, such as on a new server.
        assert_eq!(
            next_daily(last + MIN_GAP, 20 * 60),
            at(100 * day + 20 * 3600)
        );
        assert_eq!(
            next_daily(last + MIN_GAP, 15 * 60),
            at(100 * day + 15 * 3600)
        );
        assert_eq!(
            next_daily(last + MIN_GAP, 14 * 60 + 59),
            at(101 * day + (14 * 60 + 59) * 60)
        );
    }

    #[test]
    fn reads_update_json() {
        let json = |published: &str, link: &str| {
            format!(
                r#"{{"version": "0.2.0", "published": "{published}", "security": true,
                    "installs_itself": true, "notes": "Fixes things.", "link": "{link}",
                    "something_new": 1}}"#
            )
        };
        let good = Manifest::read(json("2026-11-02", "https://github.com/x").as_bytes()).unwrap();
        let release = good.release(UNIX_EPOCH).unwrap();
        assert_eq!(release.version, "0.2.0".parse().unwrap());
        assert!(release.security);
        assert!(Manifest::read(json("Nov 2", "https://github.com/x").as_bytes()).is_err());
        assert!(Manifest::read(json("2026-11-02", "http://github.com/x").as_bytes()).is_err());
        assert!(Manifest::read(b"{}").is_err());
    }
}
