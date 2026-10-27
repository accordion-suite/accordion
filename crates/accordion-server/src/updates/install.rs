//! Hands an update to the update helper, and records what it did (decision
//! 12).
//!
//! The server can't install anything itself. It downloads the package, puts
//! it in its inbox with the release's signed checksums, and writes a request
//! last. The helper, running as root, checks everything again before it
//! installs, so a server that's been taken over can't install anything the
//! release key didn't sign.

use std::io;
use std::time::SystemTime;

use accordion_core::backup::{NightlyBackup, NightlyRun};
use accordion_core::db::rusqlite::params;
use accordion_update::protocol::{REQUEST, RESULT, Report, Request};
use accordion_update::{SIGNATURE_FILE, SUMS_FILE, SignedSums, debian_architecture, package_name};

use super::service::{
    Error, HELPER_TIMEOUT, Installing, Release, Updates, from_millis, millis, read_history,
};

/// The biggest package the server downloads. Today's are about 15 MB.
const PACKAGE_LIMIT: u64 = 500_000_000;

impl Updates {
    /// Downloads the waiting release and asks the helper to install it. The
    /// helper stops the server soon after this returns.
    pub async fn install(&self) -> Result<(), Error> {
        let _one_at_a_time = self.inner.busy.lock().await;
        let release = {
            let status = self.inner.status.borrow();
            if let Some(installing) = &status.installing {
                return Err(Error::Busy(installing.version.clone()));
            }
            let release = status.waiting().ok_or(Error::NothingWaiting)?;
            if !release.installs_itself {
                return Err(Error::NeedsInstallCommand(release.version.to_string()));
            }
            if !status.can_install {
                return Err(Error::NoHelper);
            }
            release.clone()
        };
        let since = from_millis(millis(SystemTime::now()));
        self.inner.status.send_modify(|status| {
            status.installing = Some(Installing {
                version: release.version.to_string(),
                since,
            });
            status.install_problem = None;
        });
        let started = self.hand_over(&release, since).await;
        if let Err(err) = &started {
            self.inner.status.send_modify(|status| {
                status.installing = None;
                status.install_problem = Some(err.to_string());
            });
        }
        self.inner.wake.notify_one();
        started
    }

    async fn hand_over(&self, release: &Release, since: SystemTime) -> Result<(), Error> {
        let version = release.version.clone();
        let architecture = debian_architecture().ok_or(Error::NoHelper)?;
        let (sums, signature, folder) = self
            .inner
            .db
            .read(|conn| {
                conn.query_row(
                    "SELECT newest_sums, newest_signature, newest_folder FROM update_state
                     WHERE id = 1",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, Option<Vec<u8>>>(0)?,
                            row.get::<_, Option<Vec<u8>>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                        ))
                    },
                )
                .map_err(Error::from)
            })
            .await?;
        let (Some(sums), Some(signature), Some(folder)) = (sums, signature, folder) else {
            return Err(Error::Download(
                "Accordion needs to check for updates again first. Select Check now".into(),
            ));
        };
        let package = package_name(&version, architecture);
        let signed = SignedSums::verify(&sums, &signature, &self.inner.source.key)
            .map_err(|err| Error::Download(err.to_string()))?;
        let expected = signed
            .checksum(&package)
            .map_err(|err| Error::Download(err.to_string()))?;

        // Start from an empty inbox, except for a result the server hasn't
        // read yet.
        let inbox = &self.inner.inbox;
        tokio::fs::create_dir_all(inbox).await?;
        let mut entries = tokio::fs::read_dir(inbox).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_name() != RESULT {
                remove(&entry.path()).await?;
            }
        }

        let part = inbox.join(format!("{package}.part"));
        let url = format!("{folder}/{package}");
        let sum = self
            .inner
            .fetcher
            .download(&url, &part, PACKAGE_LIMIT)
            .await
            .map_err(Error::Download)?;
        if sum != expected {
            remove(&part).await?;
            return Err(Error::Download(
                accordion_update::Error::WrongChecksum(package).to_string(),
            ));
        }
        tokio::fs::rename(&part, inbox.join(&package)).await?;
        tokio::fs::write(inbox.join(SUMS_FILE), &sums).await?;
        tokio::fs::write(inbox.join(SIGNATURE_FILE), &signature).await?;

        let requested = version.to_string();
        let at = millis(since);
        self.inner
            .db
            .write(move |tx| {
                tx.execute(
                    "UPDATE update_state SET requested_version = ?1, requested_at = ?2
                     WHERE id = 1",
                    params![requested, at],
                )
                .map_err(Error::from)
            })
            .await?;
        // Last, and whole, so the helper never reads a request early.
        let request = Request { version, package };
        let draft = inbox.join(format!("{REQUEST}.part"));
        tokio::fs::write(&draft, request.to_text()).await?;
        tokio::fs::rename(&draft, inbox.join(REQUEST)).await?;
        tracing::info!(version = %request.version, "asked the update helper to install an update");
        Ok(())
    }

    /// Reads what the helper did, if it's done, and stops waiting for a
    /// helper that never answered.
    pub async fn collect_result(&self) -> Result<(), Error> {
        let path = self.inner.inbox.join(RESULT);
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return self.give_up_if_unanswered().await;
            }
            Err(err) => return Err(err.into()),
        };
        let report = Report::parse(&text);
        remove(&path).await?;
        let report = match report {
            Ok(report) => report,
            Err(err) => {
                tracing::warn!(%err, "couldn't read the update helper's result");
                return Ok(());
            }
        };
        match &report.reason {
            Some(reason) => tracing::warn!(
                version = %report.version,
                outcome = report.outcome.as_str(),
                %reason,
                "the update helper finished"
            ),
            None => tracing::info!(
                version = %report.version,
                outcome = report.outcome.as_str(),
                "the update helper finished"
            ),
        }
        let row = report.clone();
        let history = self
            .inner
            .db
            .write(move |tx| {
                tx.execute(
                    "INSERT INTO update_history (version, from_version, outcome, reason,
                                                 finished_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        row.version,
                        row.from,
                        row.outcome.as_str(),
                        row.reason,
                        millis(row.finished_at)
                    ],
                )?;
                tx.execute(
                    "UPDATE update_state SET requested_version = NULL, requested_at = NULL
                     WHERE id = 1",
                    [],
                )?;
                read_history(tx).map_err(Error::from)
            })
            .await?;
        self.inner.status.send_modify(|status| {
            status.installing = None;
            status.history = history;
        });
        Ok(())
    }

    async fn give_up_if_unanswered(&self) -> Result<(), Error> {
        let Some(installing) = self.inner.status.borrow().installing.clone() else {
            return Ok(());
        };
        let unanswered = installing
            .since
            .elapsed()
            .is_ok_and(|waited| waited > HELPER_TIMEOUT);
        if !unanswered {
            return Ok(());
        }
        remove(&self.inner.inbox.join(REQUEST)).await?;
        self.inner
            .db
            .write(|tx| {
                tx.execute(
                    "UPDATE update_state SET requested_version = NULL, requested_at = NULL
                     WHERE id = 1",
                    [],
                )
                .map_err(Error::from)
            })
            .await?;
        let problem = format!(
            "Accordion asked its update helper to install {}, but the helper didn't answer \
             within an hour. Check that it's running with: sudo systemctl status \
             accordion-update.path",
            installing.version
        );
        tracing::warn!("{problem}");
        self.inner.status.send_modify(|status| {
            status.installing = None;
            status.install_problem = Some(problem);
        });
        Ok(())
    }

    /// Installs the waiting release after a night's backup, if it's time.
    pub(super) async fn after_nightly(&self, night: &NightlyRun) {
        if !self.inner.status.borrow().due(SystemTime::now()) {
            return;
        }
        if night.backup == NightlyBackup::Failed {
            tracing::info!(
                "an update is waiting, but tonight's backup didn't work, so it waits for a night \
                 when it does"
            );
            return;
        }
        if let Err(err) = self.install().await {
            tracing::warn!(%err, "couldn't start the automatic update");
        }
    }

    /// Whether updates install after the nightly backup, or wait for an
    /// admin.
    pub async fn set_automatic(&self, automatic: bool) -> Result<(), Error> {
        self.inner
            .db
            .write(move |tx| {
                tx.execute(
                    "UPDATE update_state SET automatic = ?1 WHERE id = 1",
                    [automatic],
                )
                .map_err(Error::from)
            })
            .await?;
        self.inner
            .status
            .send_modify(|status| status.automatic = automatic);
        Ok(())
    }

    /// Whether Accordion checks for and downloads updates on its own.
    /// Turned off, it waits for an admin to select **Check now** or
    /// **Install now**.
    pub async fn set_check_on(&self, on: bool) -> Result<(), Error> {
        self.inner
            .db
            .write(move |tx| {
                tx.execute("UPDATE update_state SET check_on = ?1 WHERE id = 1", [on])
                    .map_err(Error::from)
            })
            .await?;
        self.inner.status.send_modify(|status| status.check_on = on);
        self.inner.wake.notify_one();
        Ok(())
    }
}

async fn remove(path: &std::path::Path) -> io::Result<()> {
    match tokio::fs::remove_file(path).await {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}
