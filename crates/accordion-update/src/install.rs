//! What the update helper does, as root, when the server asks it to install
//! a release (decision 12).
//!
//! 1. Copy the release into a folder only root can open, then check it, so
//!    the server can't change a file after it's checked.
//! 2. Stop Accordion, and copy its data folder, except file contents and
//!    caches, which a rollback doesn't need.
//! 3. Install the package.
//! 4. Wait for Accordion to say it's ready, and to keep running.
//! 5. If any of that fails, put back the previous package and the copy of
//!    the data.
//!
//! Everything that touches the system goes through [`System`], so tests can
//! stand in for dpkg and systemd.

use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt, chown, symlink};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::protocol::{INBOX, Outcome, REQUEST, Report, Request};
use crate::{
    PublicKey, SIGNATURE_FILE, SUMS_FILE, SignedSums, Version, copy_hashing, package_name,
};

/// The biggest package the helper accepts. Today's are about 15 MB.
const PACKAGE_LIMIT: u64 = 500_000_000;
const SUMS_LIMIT: u64 = 64 * 1024;
const SIGNATURE_LIMIT: u64 = 1024;

/// Parts of the data folder the snapshot leaves out, relative to it. File
/// contents never change once written (decision 6), restic's cache and
/// temporary files can be made again, and the inbox holds the release
/// itself.
const NOT_IN_SNAPSHOT: &[&str] = &["storage", INBOX, "backup/cache", "backup/temp"];

pub struct Paths {
    /// Accordion's data folder, `/var/lib/accordion`.
    pub data: PathBuf,
    /// The helper's own folder, `/var/lib/accordion-update`, which only root
    /// can open.
    pub state: PathBuf,
}

impl Paths {
    fn inbox(&self) -> PathBuf {
        self.data.join(INBOX)
    }

    /// The release being installed, copied from the inbox.
    fn incoming(&self) -> PathBuf {
        self.state.join("incoming")
    }

    /// The package that's installed now, to go back to.
    fn installed(&self) -> PathBuf {
        self.state.join("installed")
    }

    /// The copy of the data folder from before the update.
    fn snapshot(&self) -> PathBuf {
        self.state.join("snapshot")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Active {
    Running,
    Starting,
    Stopped,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceState {
    pub active: Active,
    /// How many times systemd restarted it after a crash.
    pub restarts: u32,
}

/// The system calls the helper makes, which tests replace.
pub trait System {
    /// Such as `arm64`.
    fn architecture(&self) -> Result<String, String>;
    fn installed_version(&self) -> Result<Version, String>;
    fn package_version(&self, package: &Path) -> Result<Version, String>;
    fn stop(&self) -> Result<(), String>;
    fn start(&self) -> Result<(), String>;
    fn install(&self, package: &Path) -> Result<(), String>;
    fn service(&self) -> Result<ServiceState, String>;
    fn wait(&self, how_long: Duration);
}

pub struct Timing {
    /// How long Accordion has to say it's ready after the install.
    pub start_within: Duration,
    /// How long it then has to keep running without a restart.
    pub stay_up: Duration,
    pub poll: Duration,
}

pub const TIMING: Timing = Timing {
    start_within: Duration::from_secs(10 * 60),
    stay_up: Duration::from_secs(5 * 60),
    poll: Duration::from_secs(5),
};

/// Installs the release the server asked for, if it asked. Returns what
/// happened, for the server, or `None` if there was no request.
pub fn run(paths: &Paths, key: &PublicKey, system: &dyn System, timing: &Timing) -> Option<Report> {
    let request_path = paths.inbox().join(REQUEST);
    let text = match fs::read_to_string(&request_path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return None,
        Err(err) => format!("unreadable {err}"),
    };
    // First, so the path unit doesn't start the helper again for it.
    let _ = fs::remove_file(&request_path);

    let from = system.installed_version();
    let report = |version: String, outcome, reason: Option<String>| Report {
        version,
        from: from
            .as_ref()
            .map_or_else(|_| "unknown".to_owned(), Version::to_string),
        outcome,
        finished_at: SystemTime::now(),
        reason,
    };
    let request = match Request::parse(&text) {
        Ok(request) => request,
        Err(err) => {
            return Some(report(
                "unknown".into(),
                Outcome::Refused,
                Some(format!("Accordion couldn't read the update request: {err}")),
            ));
        }
    };
    let version = request.version.to_string();
    let from = match &from {
        Ok(from) => from.clone(),
        Err(err) => {
            return Some(report(version, Outcome::Refused, Some(err.clone())));
        }
    };

    let (new, previous) = match prepare(paths, key, system, &request, &from) {
        Ok(packages) => packages,
        Err(reason) => return Some(report(version, Outcome::Refused, Some(reason))),
    };

    if let Err(err) = system.stop() {
        let _ = system.start();
        return Some(report(
            version,
            Outcome::Refused,
            Some(format!(
                "Accordion couldn't be stopped for the update: {err}"
            )),
        ));
    }
    if let Err(err) = take_snapshot(paths) {
        let _ = system.start();
        return Some(report(
            version,
            Outcome::Refused,
            Some(format!(
                "Accordion couldn't copy its data before updating, so it didn't update: {err}. \
                 Check that the disk has space"
            )),
        ));
    }

    let failure = match system.install(&new) {
        Err(err) => Some(format!("The package didn't install: {err}")),
        Ok(()) => {
            // dpkg starts the new version, unless the admin turned the
            // service off. Start it either way, so it can be checked.
            let _ = system.start();
            healthy(system, timing).err()
        }
    };
    match failure {
        None => {
            if let Err(err) = keep_installed(paths, &new) {
                eprintln!("The update worked, but the helper couldn't keep its package: {err}");
            }
            Some(report(version, Outcome::Installed, None))
        }
        Some(reason) => {
            let reason = match roll_back(paths, system, timing, &previous) {
                Ok(()) => reason,
                Err(err) => format!("{reason}. Going back to {from} didn't work either: {err}"),
            };
            Some(report(version, Outcome::RolledBack, Some(reason)))
        }
    }
}

/// Copies the release into the helper's folder and checks it. Returns the
/// new package and the installed one.
fn prepare(
    paths: &Paths,
    key: &PublicKey,
    system: &dyn System,
    request: &Request,
    from: &Version,
) -> Result<(PathBuf, PathBuf), String> {
    let architecture = system.architecture()?;
    let expected = package_name(&request.version, &architecture);
    if request.package != expected {
        return Err(format!(
            "The server asked for {}, but this machine needs {expected}",
            request.package
        ));
    }
    if request.version <= *from {
        return Err(format!(
            "Accordion {from} is already installed, which isn't older than {}",
            request.version
        ));
    }
    let previous = paths.installed().join(package_name(from, &architecture));
    if !previous.is_file() {
        return Err(format!(
            "There's no copy of the installed version, {from}, to go back to if the update \
             fails, so Accordion didn't update. Install {} with the install command instead, \
             which keeps a copy for next time",
            request.version
        ));
    }

    let incoming = paths.incoming();
    remove_all(&incoming)
        .map_err(|err| format!("Accordion couldn't clear {}: {err}", incoming.display()))?;
    create_private_dir(&incoming)
        .map_err(|err| format!("Accordion couldn't create {}: {err}", incoming.display()))?;
    let copy = |name: &str, limit| {
        copy_in(&paths.inbox().join(name), &incoming.join(name), limit)
            .map_err(|err| format!("Accordion couldn't read the downloaded {name}: {err}"))
    };
    copy(SUMS_FILE, SUMS_LIMIT)?;
    copy(SIGNATURE_FILE, SIGNATURE_LIMIT)?;
    let package_sum = copy(&request.package, PACKAGE_LIMIT)?;

    let read = |name: &str| {
        fs::read(incoming.join(name))
            .map_err(|err| format!("Accordion couldn't read {name}: {err}"))
    };
    let signed = SignedSums::verify(&read(SUMS_FILE)?, &read(SIGNATURE_FILE)?, key)
        .map_err(|err| err.to_string())?;
    if signed
        .checksum(&request.package)
        .map_err(|err| err.to_string())?
        != package_sum
    {
        return Err(crate::Error::WrongChecksum(request.package.clone()).to_string());
    }
    let new = incoming.join(&request.package);
    let inside = system.package_version(&new)?;
    if inside != request.version {
        return Err(format!(
            "The package says it's version {inside}, not {}",
            request.version
        ));
    }
    Ok((new, previous))
}

/// Copies one file the server downloaded, without following a link, and
/// returns its SHA-256.
fn copy_in(from: &Path, to: &Path, limit: u64) -> io::Result<[u8; 32]> {
    if fs::symlink_metadata(from)?.file_type().is_symlink() {
        return Err(io::Error::other("it's a link, not a file"));
    }
    let mut source = File::open(from)?;
    let mut target = File::create(to)?;
    let sum = copy_hashing(&mut source, &mut target, limit)?;
    target.sync_all()?;
    Ok(sum)
}

/// Waits for Accordion to say it's ready, then to stay up.
fn healthy(system: &dyn System, timing: &Timing) -> Result<(), String> {
    let mut waited = Duration::ZERO;
    let mut state = system.service()?;
    let first_restarts = state.restarts;
    loop {
        match state.active {
            Active::Running => break,
            Active::Failed => return Err("It stopped with an error while it was starting".into()),
            Active::Starting | Active::Stopped => {}
        }
        if state.restarts != first_restarts {
            return Err("It stopped with an error while it was starting".into());
        }
        if waited >= timing.start_within {
            return Err(format!(
                "It didn't finish starting within {}",
                minutes(timing.start_within)
            ));
        }
        system.wait(timing.poll);
        waited += timing.poll;
        state = system.service()?;
    }
    let restarts = state.restarts;
    let mut watched = Duration::ZERO;
    while watched < timing.stay_up {
        system.wait(timing.poll);
        watched += timing.poll;
        let now = system.service()?;
        if now.active != Active::Running || now.restarts != restarts {
            return Err(format!(
                "It started, then stopped within {}",
                minutes(timing.stay_up)
            ));
        }
    }
    Ok(())
}

/// Such as "five minutes" or "10 minutes".
fn minutes(duration: Duration) -> String {
    const WORDS: [&str; 10] = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ];
    let minutes = duration.as_secs().div_ceil(60);
    let number = WORDS
        .get(minutes as usize)
        .map_or_else(|| minutes.to_string(), |word| (*word).to_owned());
    if minutes == 1 {
        format!("{number} minute")
    } else {
        format!("{number} minutes")
    }
}

fn roll_back(
    paths: &Paths,
    system: &dyn System,
    timing: &Timing,
    previous: &Path,
) -> Result<(), String> {
    let _ = system.stop();
    restore_snapshot(paths)
        .map_err(|err| format!("the copy of the data couldn't be put back ({err})"))?;
    system
        .install(previous)
        .map_err(|err| format!("the previous package didn't install ({err})"))?;
    let _ = system.start();
    healthy(system, timing).map_err(|err| err.to_lowercase())
}

/// Makes the new package the one to go back to next time.
fn keep_installed(paths: &Paths, new: &Path) -> io::Result<()> {
    let installed = paths.installed();
    let kept = paths.state.join("installed.new");
    remove_all(&kept)?;
    create_private_dir(&kept)?;
    let name = new.file_name().expect("packages have names");
    fs::rename(new, kept.join(name))?;
    remove_all(&installed)?;
    fs::rename(&kept, &installed)
}

fn take_snapshot(paths: &Paths) -> io::Result<()> {
    let snapshot = paths.snapshot();
    remove_all(&snapshot)?;
    copy_tree(&paths.data, &snapshot, Path::new(""))
}

fn restore_snapshot(paths: &Paths) -> io::Result<()> {
    clear_tree(&paths.data, Path::new(""))?;
    copy_tree(&paths.snapshot(), &paths.data, Path::new(""))
}

/// Whether `relative` is left out of snapshots, and whether something inside
/// it is.
fn left_out(relative: &Path) -> (bool, bool) {
    let left_out = NOT_IN_SNAPSHOT
        .iter()
        .any(|path| relative == Path::new(path));
    let holds_left_out = NOT_IN_SNAPSHOT
        .iter()
        .any(|path| Path::new(path).starts_with(relative) && relative != Path::new(path));
    (left_out, holds_left_out)
}

/// Copies `from` to `to`, keeping owners and permissions, and skipping what
/// snapshots leave out. `relative` is where `from` is in the data folder.
fn copy_tree(from: &Path, to: &Path, relative: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(from)?;
    if !to.exists() {
        fs::create_dir(to)?;
    }
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let inside = relative.join(entry.file_name());
        let (left_out, _) = left_out(&inside);
        if left_out {
            continue;
        }
        let source = entry.path();
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&source, &target, &inside)?;
        } else if kind.is_symlink() {
            let _ = fs::remove_file(&target);
            symlink(fs::read_link(&source)?, &target)?;
            let link = fs::symlink_metadata(&source)?;
            std::os::unix::fs::lchown(&target, Some(link.uid()), Some(link.gid()))?;
        } else if kind.is_file() {
            fs::copy(&source, &target)?;
            let file = fs::metadata(&source)?;
            chown(&target, Some(file.uid()), Some(file.gid()))?;
            fs::set_permissions(&target, fs::Permissions::from_mode(file.mode()))?;
        }
        // Sockets and other special files aren't data.
    }
    chown(to, Some(meta.uid()), Some(meta.gid()))?;
    fs::set_permissions(to, fs::Permissions::from_mode(meta.mode()))
}

/// Removes everything in `dir` that snapshots cover, so a restore leaves
/// nothing from the failed version behind.
fn clear_tree(dir: &Path, relative: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let inside = relative.join(entry.file_name());
        match left_out(&inside) {
            (true, _) => {}
            (false, true) => clear_tree(&entry.path(), &inside)?,
            (false, false) => remove_all(&entry.path())?,
        }
    }
    Ok(())
}

fn remove_all(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
    }
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use ring::rand::SystemRandom;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    use super::*;

    /// Stands in for dpkg and systemd. Installing a package "runs" the
    /// version named in it, and `states` is what systemd reports after each
    /// install, one state per poll.
    struct Fake {
        installed: RefCell<Version>,
        data: PathBuf,
        log: RefCell<Vec<String>>,
        install_fails: Option<String>,
        /// For each install, in order, the states systemd reports.
        states: RefCell<VecDeque<Vec<ServiceState>>>,
        current: RefCell<VecDeque<ServiceState>>,
    }

    const RUNNING: ServiceState = ServiceState {
        active: Active::Running,
        restarts: 0,
    };

    impl System for Fake {
        fn architecture(&self) -> Result<String, String> {
            Ok("arm64".into())
        }
        fn installed_version(&self) -> Result<Version, String> {
            Ok(self.installed.borrow().clone())
        }
        fn package_version(&self, package: &Path) -> Result<Version, String> {
            let text = fs::read_to_string(package).map_err(|err| err.to_string())?;
            text.trim()
                .parse()
                .map_err(|err: crate::Error| err.to_string())
        }
        fn stop(&self) -> Result<(), String> {
            self.log.borrow_mut().push("stop".into());
            Ok(())
        }
        fn start(&self) -> Result<(), String> {
            self.log.borrow_mut().push("start".into());
            Ok(())
        }
        fn install(&self, package: &Path) -> Result<(), String> {
            let version = self.package_version(package)?;
            self.log.borrow_mut().push(format!("install {version}"));
            if version > *self.installed.borrow()
                && let Some(err) = &self.install_fails
            {
                return Err(err.clone());
            }
            // A newer version changes the data, as a migration would. An
            // older one leaves it, so only a restore puts it back.
            if version > *self.installed.borrow() {
                fs::write(
                    self.data.join("accordion.db"),
                    format!("schema of {version}"),
                )
                .unwrap();
                fs::write(self.data.join("gateway/new-file"), "x").unwrap();
            }
            *self.installed.borrow_mut() = version.clone();
            let states = self
                .states
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| vec![RUNNING]);
            *self.current.borrow_mut() = states.into();
            Ok(())
        }
        fn service(&self) -> Result<ServiceState, String> {
            let mut current = self.current.borrow_mut();
            // The last state stays.
            Ok(if current.len() > 1 {
                current.pop_front().unwrap()
            } else {
                *current.front().unwrap_or(&RUNNING)
            })
        }
        fn wait(&self, _: Duration) {}
    }

    const FAST: Timing = Timing {
        start_within: Duration::from_secs(60),
        stay_up: Duration::from_secs(30),
        poll: Duration::from_secs(5),
    };

    struct Setup {
        _dir: tempfile::TempDir,
        paths: Paths,
        key: Ed25519KeyPair,
        fake: Fake,
    }

    /// A server running 0.2.0, with a copy of its package, and data that
    /// the update changes.
    fn setup() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            data: dir.path().join("data"),
            state: dir.path().join("state"),
        };
        for folder in ["storage/blobs", "backup/cache", "gateway", "updates"] {
            fs::create_dir_all(paths.data.join(folder)).unwrap();
        }
        fs::write(paths.data.join("accordion.db"), "schema of 0.2.0").unwrap();
        fs::write(paths.data.join("gateway/cert.pem"), "certificate").unwrap();
        fs::write(paths.data.join("storage/blobs/ab"), "file contents").unwrap();
        fs::write(paths.data.join("backup/cache/x"), "cache").unwrap();
        fs::create_dir_all(paths.installed()).unwrap();
        fs::write(paths.installed().join("accordion_0.2.0_arm64.deb"), "0.2.0").unwrap();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        let key = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let fake = Fake {
            installed: RefCell::new("0.2.0".parse().unwrap()),
            data: paths.data.clone(),
            log: RefCell::default(),
            install_fails: None,
            states: RefCell::default(),
            current: RefCell::default(),
        };
        Setup {
            _dir: dir,
            paths,
            key,
            fake,
        }
    }

    impl Setup {
        fn public(&self) -> PublicKey {
            PublicKey(self.key.public_key().as_ref().try_into().unwrap())
        }

        /// Puts a signed release of `version` in the inbox, the way the
        /// server does, and asks for it.
        fn request(&self, version: &str) {
            let inbox = self.paths.inbox();
            let package = format!("accordion_{version}_arm64.deb");
            fs::write(inbox.join(&package), version).unwrap();
            let hex: String = crate::sha256(version.as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let sums = format!("{hex}  {package}\n");
            fs::write(inbox.join(SUMS_FILE), &sums).unwrap();
            fs::write(inbox.join(SIGNATURE_FILE), self.key.sign(sums.as_bytes())).unwrap();
            let request = Request {
                version: version.parse().unwrap(),
                package,
            };
            fs::write(inbox.join(REQUEST), request.to_text()).unwrap();
        }

        fn run(&self) -> Report {
            run(&self.paths, &self.public(), &self.fake, &FAST).unwrap()
        }

        fn db(&self) -> String {
            fs::read_to_string(self.paths.data.join("accordion.db")).unwrap()
        }

        fn log(&self) -> Vec<String> {
            self.fake.log.borrow().clone()
        }
    }

    #[test]
    fn does_nothing_without_a_request() {
        let setup = setup();
        assert_eq!(run(&setup.paths, &setup.public(), &setup.fake, &FAST), None);
        assert!(setup.log().is_empty());
    }

    #[test]
    fn installs_a_signed_release_that_starts() {
        let setup = setup();
        setup.request("0.3.0");
        let report = setup.run();
        assert_eq!(report.outcome, Outcome::Installed, "{report:?}");
        assert_eq!(
            (report.version.as_str(), report.from.as_str()),
            ("0.3.0", "0.2.0")
        );
        assert_eq!(setup.log(), ["stop", "install 0.3.0", "start"]);
        assert_eq!(setup.db(), "schema of 0.3.0");
        assert!(!setup.paths.inbox().join(REQUEST).exists());
        // The new package is the one to go back to next time.
        assert!(
            setup
                .paths
                .installed()
                .join("accordion_0.3.0_arm64.deb")
                .is_file()
        );
        assert!(
            !setup
                .paths
                .installed()
                .join("accordion_0.2.0_arm64.deb")
                .exists()
        );
        // The snapshot has the data, but not file contents or the cache.
        let snapshot = setup.paths.snapshot();
        assert_eq!(
            fs::read_to_string(snapshot.join("accordion.db")).unwrap(),
            "schema of 0.2.0"
        );
        assert!(snapshot.join("gateway/cert.pem").is_file());
        assert!(!snapshot.join("storage").exists());
        assert!(!snapshot.join("backup/cache").exists());
        assert!(!snapshot.join("updates").exists());
    }

    #[test]
    fn rolls_back_a_release_that_fails_to_start() {
        let setup = setup();
        let failed = ServiceState {
            active: Active::Failed,
            restarts: 0,
        };
        setup.fake.states.borrow_mut().push_back(vec![
            ServiceState {
                active: Active::Starting,
                restarts: 0,
            },
            failed,
        ]);
        setup.request("0.3.0");
        let report = setup.run();
        assert_eq!(report.outcome, Outcome::RolledBack);
        assert_eq!(
            report.reason.as_deref(),
            Some("It stopped with an error while it was starting")
        );
        assert_eq!(
            setup.log(),
            [
                "stop",
                "install 0.3.0",
                "start",
                "stop",
                "install 0.2.0",
                "start"
            ]
        );
        assert_eq!(*setup.fake.installed.borrow(), "0.2.0".parse().unwrap());
        assert_eq!(setup.db(), "schema of 0.2.0");
        // The new version's file goes, since it wasn't there before.
        assert!(!setup.paths.data.join("gateway/new-file").exists());
        // What the snapshot leaves out stays as it was.
        assert!(setup.paths.data.join("storage/blobs/ab").is_file());
        assert!(setup.paths.data.join("backup/cache/x").is_file());
        assert!(
            setup
                .paths
                .installed()
                .join("accordion_0.2.0_arm64.deb")
                .is_file()
        );
    }

    #[test]
    fn rolls_back_a_release_that_keeps_restarting() {
        let setup = setup();
        let restarted = ServiceState {
            active: Active::Running,
            restarts: 1,
        };
        setup
            .fake
            .states
            .borrow_mut()
            .push_back(vec![RUNNING, RUNNING, RUNNING, restarted]);
        setup.request("0.3.0");
        let report = setup.run();
        assert_eq!(report.outcome, Outcome::RolledBack);
        assert_eq!(
            report.reason.as_deref(),
            Some("It started, then stopped within one minute")
        );
        assert_eq!(setup.db(), "schema of 0.2.0");
    }

    #[test]
    fn rolls_back_a_package_that_doesnt_install() {
        let mut setup = setup();
        setup.fake.install_fails = Some("dpkg: error: disk full".into());
        setup.request("0.3.0");
        let report = setup.run();
        assert_eq!(report.outcome, Outcome::RolledBack);
        assert_eq!(
            report.reason.as_deref(),
            Some("The package didn't install: dpkg: error: disk full")
        );
        assert_eq!(
            setup.log(),
            ["stop", "install 0.3.0", "stop", "install 0.2.0", "start"]
        );
    }

    #[test]
    fn refuses_a_release_that_doesnt_check_out() {
        // Signed with another key.
        let setup = setup();
        setup.request("0.3.0");
        let other = Ed25519KeyPair::from_pkcs8(
            Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
                .unwrap()
                .as_ref(),
        )
        .unwrap();
        let sums = fs::read(setup.paths.inbox().join(SUMS_FILE)).unwrap();
        fs::write(setup.paths.inbox().join(SIGNATURE_FILE), other.sign(&sums)).unwrap();
        let report = setup.run();
        assert_eq!(report.outcome, Outcome::Refused);
        assert!(
            report
                .reason
                .unwrap()
                .contains("doesn't match Accordion's release key")
        );
        assert!(setup.log().is_empty(), "nothing was stopped or installed");

        // A package changed after it was signed.
        setup.request("0.3.0");
        fs::write(
            setup.paths.inbox().join("accordion_0.3.0_arm64.deb"),
            "0.3.1",
        )
        .unwrap();
        let report = setup.run();
        assert_eq!(report.outcome, Outcome::Refused);
        assert!(
            report
                .reason
                .unwrap()
                .contains("doesn't match its checksum")
        );

        // A link to a file the server shouldn't be able to read.
        setup.request("0.3.0");
        let package = setup.paths.inbox().join("accordion_0.3.0_arm64.deb");
        fs::remove_file(&package).unwrap();
        symlink(
            setup.paths.installed().join("accordion_0.2.0_arm64.deb"),
            &package,
        )
        .unwrap();
        let report = setup.run();
        assert!(report.reason.unwrap().contains("it's a link, not a file"));

        // An older release.
        setup.request("0.1.0");
        let report = setup.run();
        assert_eq!(report.outcome, Outcome::Refused);
        assert!(report.reason.unwrap().contains("isn't older than 0.1.0"));

        // The wrong architecture.
        setup.request("0.3.0");
        let request = Request {
            version: "0.3.0".parse().unwrap(),
            package: "accordion_0.3.0_amd64.deb".into(),
        };
        fs::write(setup.paths.inbox().join(REQUEST), request.to_text()).unwrap();
        let report = setup.run();
        assert!(
            report
                .reason
                .unwrap()
                .contains("this machine needs accordion_0.3.0_arm64.deb")
        );
        assert!(setup.log().is_empty());
        assert_eq!(setup.db(), "schema of 0.2.0");
    }

    #[test]
    fn needs_a_package_to_go_back_to() {
        let setup = setup();
        fs::remove_file(setup.paths.installed().join("accordion_0.2.0_arm64.deb")).unwrap();
        setup.request("0.3.0");
        let report = setup.run();
        assert_eq!(report.outcome, Outcome::Refused);
        assert!(
            report
                .reason
                .unwrap()
                .contains("There's no copy of the installed version")
        );
        assert!(setup.log().is_empty());
    }
}
