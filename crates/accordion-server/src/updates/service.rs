//! Checks once a day for a newer release (decision 12).

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use accordion_core::db::rusqlite::{self, params};
use accordion_core::db::{self, Database, Migration};
use accordion_update::{
    MANIFEST_FILE, PublicKey, RELEASE_KEY, SIGNATURE_FILE, SUMS_FILE, SignedSums, Version,
};
use hyper::Uri;
use serde::Deserialize;
use tokio::sync::{Mutex, watch};

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

/// Each file is small. The limits keep a bad download from filling memory.
const SUMS_LIMIT: usize = 64 * 1024;
const SIGNATURE_LIMIT: usize = 1024;
const MANIFEST_LIMIT: usize = 64 * 1024;
const NOTES_LIMIT: usize = 4000;

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "update checks",
    sql: "
    -- One row: when this server checks, and what it last found.
    CREATE TABLE update_state (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        -- The minute of the day, in UTC, that this server checks at. Picked
        -- at random, so servers don't all check at once.
        check_minute INTEGER NOT NULL CHECK (check_minute BETWEEN 0 AND 1439),
        -- Milliseconds since 1970.
        last_check_at INTEGER,
        -- Why the last check didn't work, or NULL if it did.
        last_check_problem TEXT,
        -- The newest release's update.json, SHA256SUMS, and signature, as
        -- downloaded, so they can be checked again before installing.
        newest_manifest BLOB,
        newest_sums BLOB,
        newest_signature BLOB,
        -- When this server first saw that release.
        newest_first_seen INTEGER
    );
    ",
}];

/// Where to look for releases, and the key they must be signed with.
#[derive(Clone)]
pub struct UpdateSource {
    /// The folder of the newest release, without a trailing `/`.
    pub base: String,
    pub key: PublicKey,
    /// For tests against a local server. Releases always come over HTTPS.
    pub allow_http: bool,
}

impl UpdateSource {
    pub fn github() -> Self {
        Self {
            base: GITHUB.to_owned(),
            key: RELEASE_KEY,
            allow_http: false,
        }
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

#[derive(Clone, Debug)]
pub struct UpdateStatus {
    /// The version running now.
    pub installed: Version,
    /// The newest release the last working check found.
    pub newest: Option<Release>,
    pub last_check: Option<CheckResult>,
    pub checking: bool,
}

impl UpdateStatus {
    /// The release waiting to be installed, if there's one newer than this
    /// server.
    pub fn waiting(&self) -> Option<&Release> {
        self.newest
            .as_ref()
            .filter(|release| release.version > self.installed)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Db(#[from] db::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("{0}")]
    Setup(String),
}

/// The update checker. Cloning it is cheap.
#[derive(Clone)]
pub struct Updates {
    inner: Arc<Inner>,
}

struct Inner {
    db: Database,
    source: UpdateSource,
    fetcher: Fetcher,
    check_minute: u32,
    status: watch::Sender<UpdateStatus>,
    /// One check at a time, whether scheduled or from the updates page.
    checking: Mutex<()>,
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
    manifest: Vec<u8>,
    sums: Vec<u8>,
    signature: Vec<u8>,
    release: Release,
}

fn millis(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
    })
}

fn from_millis(millis: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(u64::try_from(millis).unwrap_or(0))
}

impl Updates {
    /// Opens the checker's state. `installed` is the version running now.
    pub async fn open(
        db: Database,
        source: UpdateSource,
        installed: Version,
    ) -> Result<Self, Error> {
        db.migrate("updates", MIGRATIONS).await?;
        let fetcher = Fetcher::new(source.allow_http).map_err(Error::Setup)?;
        let minute = {
            let mut bytes = [0; 4];
            getrandom::fill(&mut bytes).expect("the OS random number generator failed");
            u32::from_le_bytes(bytes) % 1440
        };
        let (check_minute, last_check, newest) = db
            .write(move |tx| {
                tx.execute(
                    "INSERT OR IGNORE INTO update_state (id, check_minute) VALUES (1, ?1)",
                    [minute],
                )?;
                tx.query_row(
                    "SELECT check_minute, last_check_at, last_check_problem, newest_manifest,
                            newest_first_seen
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
                        Ok((row.get::<_, u32>(0)?, last_check, manifest.zip(first_seen)))
                    },
                )
                .map_err(Error::from)
            })
            .await?;
        // A manifest that no longer reads, such as one an older version
        // wrote, is dropped. The next check finds the newest release again.
        let newest = newest.and_then(|(manifest, first_seen)| {
            Manifest::read(&manifest)
                .and_then(|manifest| manifest.release(from_millis(first_seen)))
                .ok()
        });
        let (status, _) = watch::channel(UpdateStatus {
            installed,
            newest,
            last_check,
            checking: false,
        });
        Ok(Self {
            inner: Arc::new(Inner {
                db,
                source,
                fetcher,
                check_minute,
                status,
                checking: Mutex::new(()),
            }),
        })
    }

    pub fn status(&self) -> watch::Receiver<UpdateStatus> {
        self.inner.status.subscribe()
    }

    /// Checks on schedule until `stopped` completes.
    pub async fn run(self, stopped: impl Future<Output = ()>) {
        tokio::pin!(stopped);
        loop {
            let wait = self.until_next_check(SystemTime::now());
            tokio::select! {
                () = &mut stopped => return,
                () = tokio::time::sleep(wait) => {
                    if let Err(err) = self.check().await {
                        tracing::error!(%err, "couldn't record an update check");
                        // Don't spin on a broken database.
                        tokio::time::sleep(RETRY).await;
                    }
                }
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
        let _one_at_a_time = self.inner.checking.lock().await;
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
                let (manifest, sums, signature, first_seen) = match &newest {
                    Some(found) => (
                        Some(found.manifest.clone()),
                        Some(found.sums.clone()),
                        Some(found.signature.clone()),
                        Some(millis(found.release.first_seen)),
                    ),
                    None => (None, None, None, None),
                };
                self.inner
                    .db
                    .write(move |tx| {
                        tx.execute(
                            "UPDATE update_state SET last_check_at = ?1, last_check_problem = NULL,
                                 newest_manifest = ?2, newest_sums = ?3, newest_signature = ?4,
                                 newest_first_seen = ?5
                             WHERE id = 1",
                            params![at, manifest, sums, signature, first_seen],
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
