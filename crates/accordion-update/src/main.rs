//! The update helper: installs a release the server downloaded, and rolls it
//! back if it doesn't start (decision 12).
//!
//! It runs as root, from `accordion-update.service`, which a path unit starts
//! when the server writes its request. That service has no network, so the
//! helper can't download anything: it only checks and installs what the
//! server already downloaded, with its own copy of the release key.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, chown};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;

use accordion_update::install::{self, Active, Paths, ServiceState, System};
use accordion_update::protocol::{INBOX, Outcome, RESULT, Report};
use accordion_update::{Version, release_key};

const SERVICE: &str = "accordion.service";

/// dpkg and systemd, on Debian and its relatives.
struct Debian;

/// Runs `program` and returns what it printed, or what went wrong.
fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .output()
        .map_err(|err| format!("{program} didn't run: {err}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned());
    }
    // dpkg ends with a list of the packages that failed, so the line that
    // says why is the last one that mentions an error.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lines = || {
        stderr
            .lines()
            .rev()
            .map(str::trim)
            .filter(|line| !line.is_empty())
    };
    let why = lines()
        .find(|line| line.contains("error") && !line.starts_with("Errors were encountered"))
        .or_else(|| lines().next())
        .unwrap_or("no message");
    Err(format!("{program} failed ({why})"))
}

fn version(text: &str) -> Result<Version, String> {
    text.parse()
        .map_err(|err: accordion_update::Error| err.to_string())
}

impl System for Debian {
    fn architecture(&self) -> Result<String, String> {
        run("dpkg", &["--print-architecture"])
    }

    fn installed_version(&self) -> Result<Version, String> {
        version(&run(
            "dpkg-query",
            &["--show", "--showformat=${Version}", "accordion"],
        )?)
    }

    fn package_version(&self, package: &Path) -> Result<Version, String> {
        let path = package.to_string_lossy();
        version(&run("dpkg-deb", &["--field", &path, "Version"])?)
    }

    fn stop(&self) -> Result<(), String> {
        run("systemctl", &["stop", SERVICE]).map(drop)
    }

    fn start(&self) -> Result<(), String> {
        run("systemctl", &["start", SERVICE]).map(drop)
    }

    fn install(&self, package: &Path) -> Result<(), String> {
        run("dpkg", &["--install", &package.to_string_lossy()]).map(drop)
    }

    fn service(&self) -> Result<ServiceState, String> {
        let shown = run(
            "systemctl",
            &[
                "show",
                SERVICE,
                "--property=ActiveState",
                "--property=NRestarts",
            ],
        )?;
        let mut active = None;
        let mut restarts = 0;
        for line in shown.lines() {
            match line.split_once('=') {
                Some(("ActiveState", state)) => {
                    active = Some(match state {
                        "active" => Active::Running,
                        "activating" | "reloading" => Active::Starting,
                        "failed" => Active::Failed,
                        _ => Active::Stopped,
                    });
                }
                Some(("NRestarts", count)) => restarts = count.parse().unwrap_or(0),
                _ => {}
            }
        }
        Ok(ServiceState {
            active: active.ok_or("systemctl didn't say whether Accordion is running")?,
            restarts,
        })
    }

    fn wait(&self, how_long: Duration) {
        std::thread::sleep(how_long);
    }
}

/// Leaves the report where the server reads it, owned by the server's user
/// like the rest of its data folder.
fn write_result(data: &Path, report: &Report) -> std::io::Result<()> {
    let inbox = data.join(INBOX);
    let owner = fs::metadata(data)?;
    let draft = inbox.join(format!("{RESULT}.part"));
    let _ = fs::remove_file(&draft);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&draft)?;
    file.write_all(report.to_text().as_bytes())?;
    file.sync_all()?;
    chown(&draft, Some(owner.uid()), Some(owner.gid()))?;
    fs::rename(&draft, inbox.join(RESULT))
}

fn main() -> ExitCode {
    let folder = |variable: &str, default: &str| {
        std::env::var_os(variable)
            .filter(|value| !value.is_empty())
            .map_or_else(|| PathBuf::from(default), PathBuf::from)
    };
    let paths = Paths {
        data: folder("ACCORDION_DATA_DIR", "/var/lib/accordion"),
        // systemd sets this from the service's StateDirectory.
        state: folder("STATE_DIRECTORY", "/var/lib/accordion-update"),
    };
    let Some(report) = install::run(&paths, &release_key(), &Debian, &install::TIMING) else {
        return ExitCode::SUCCESS;
    };
    let reason = report.reason.as_deref().unwrap_or("");
    match report.outcome {
        Outcome::Installed => eprintln!(
            "Updated Accordion from {} to {}.",
            report.from, report.version
        ),
        Outcome::RolledBack => eprintln!(
            "Accordion {} didn't work, so Accordion went back to {}. {reason}.",
            report.version, report.from
        ),
        Outcome::Refused => eprintln!("Accordion didn't update to {}. {reason}.", report.version),
    }
    if let Err(err) = write_result(&paths.data, &report) {
        eprintln!("The helper couldn't leave its report for the server: {err}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
