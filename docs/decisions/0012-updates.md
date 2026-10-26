# 12. How Accordion updates itself

- **Status:** Accepted
- **Date:** October 26, 2026

## Context

The spec asks for updates that are "signed, staged (snapshot -> update -> health check -> auto-rollback on failure), on a schedule the admin picks." It also says that "update checks fetch a signed static manifest; no identifier or version is sent." The household admin's must-haves include "update control," and their deal-breaker is "weekend-long debugging after an update."

[Decision 9](0009-releases-and-installing.md) set up what this decision builds on:

- **Each release is a .deb** for amd64 and arm64, installed by `install.sh` with `apt-get`. The package's install script restarts the service on an upgrade.
- **`SHA256SUMS` is signed with the release key,** an Ed25519 key kept offline. Decision 9 put the key's public half in the server program, so the server can check updates without trusting the download site. That part isn't built yet.
- **Updating needs root,** because the package installs into `/usr`. Accordion runs as the unprivileged `accordion` user, in a tight systemd sandbox, so decision 9 planned a small part that runs as root.

Three facts about today's server shape how a rollback can work:

- **Migrations only go forward.** A new version adds migrations, and an older version refuses to open a database with migrations it doesn't know. So going back to the previous version means going back to the database from before the update too.
- **File contents never change once written.** [Decision 6](0006-file-storage.md) stores each file's contents under its hash in `storage/blobs`. A rollback can leave them in place: cleanup removes the ones nothing points at, and the read-only view of each user's files is rebuilt from the database.
- **The rest of the data folder is small:** the database, certificates, the DNS token, and keys.

v0.1.0 is installed on test machines and has no way to update itself, so its owners will update once by running the install command again.

## Decision drivers

1. **An update never leaves a household with a server that won't start.** If the new version fails, the old one comes back on its own.
2. **No terminal.** The admin picks when updates install, or installs one with a button, and sees what happened.
3. **Nothing installs unless the release key signed it,** even if GitHub or the download is compromised.
4. **The least code running as root,** and the server itself stays unprivileged and sandboxed.
5. **No identifiers sent.** Checking for updates reveals nothing about the server except that a computer at its IP address asked.
6. **Little to maintain** for one developer.

## Options

### Option 1: Accordion's own updater, with a small root helper

The server checks for updates, downloads them, and checks their signatures, all without any extra privileges. When it's time to install, it hands the files to a small helper program that runs as root, with no network. The helper checks the signature again, takes a snapshot, installs the package, waits for the new version to pass a health check, and rolls back if it doesn't.

Pros:

- **Does everything the spec asks,** including the snapshot, health check, and rollback.
- **The admin controls it from Accordion,** and the health page shows what happened.
- **Root does very little,** never touches the network, and trusts nothing the server hands it without checking.

Cons:

- **A new program to build and keep correct,** and its mistakes run as root. It has to stay small and well tested.
- **Only works where Accordion is installed as a .deb.** A container or NAS package will need its own way to update.

### Option 2: An apt repository, with the system's automatic updates

Publish an apt repository, and have `install.sh` add it to the system's sources. Debian's `unattended-upgrades` then installs new versions along with the system's own updates.

Pros:

- **Standard Debian tooling,** which admins may already know.
- **No updater code** in Accordion.

Cons:

- **No snapshot, health check, or rollback.** A version that fails to start stays installed until someone fixes it from a terminal.
- **The schedule lives in the system, not in Accordion,** so the admin can't change it from the admin pages, and the health page can't say what happened.
- **A repository to host and sign** with a separate GPG key, kept up to date with each release.

### Option 3: Updates by hand

The server checks for updates and shows them on the health page. The admin installs them by running the install command again.

Pros:

- **Least to build.** The check and the page only.

Cons:

- **Needs a terminal,** which decision 9's drivers ruled out.
- **Security fixes wait** until the admin gets to it.
- **No rollback,** so a bad release still means debugging.

### Option 4: The server replaces its own program

The server downloads the new program into its data folder, and the service runs whichever copy is newest.

Pros:

- **No root needed** at all.

Cons:

- **A compromised server could replace its own program,** and keep control after a restart or an update. The sandbox exists to stop exactly that.
- **The package no longer describes what's installed,** so removing it leaves programs behind.

## Comparison

| Driver | Option 1: own updater | Option 2: apt repository | Option 3: by hand | Option 4: self-replacing |
|---|---|---|---|---|
| Rolls back on failure | Yes | No | No | Possible |
| No terminal | Yes | Yes | No | Yes |
| Only signed releases install | Yes | Yes, with a second key | Yes | Yes |
| Little runs as root | A small helper | apt | apt, by hand | Nothing, but the sandbox is weakened |
| Work for one developer | Medium | Medium | Low | Medium |

## Recommendation

**Option 1: Accordion's own updater, with a small root helper.**

### Checking for updates

Once a day, at a random time, the server downloads three files from the newest full release on GitHub: `SHA256SUMS`, `SHA256SUMS.sig`, and a new file, `update.json`. GitHub serves them from fixed addresses under `releases/latest/download/`, so no server of Accordion's own is needed.

The requests send nothing about the server. There are no cookies, and the user agent is "Accordion", with no version. The only thing GitHub learns is the IP address, which any connection reveals. GitHub counts each file's downloads, which gives the spec's retention measure without telemetry.

The server checks the signature on `SHA256SUMS` with the release key built into the program, then checks that `update.json` is listed in it with the right checksum. `update.json` says:

- **The version** and the date it was published
- **Whether it's a security release**
- **Short release notes,** in plain words, and a link to the full notes
- **Whether it can install on its own.** A release that needs something new from the distribution, such as another package, can't. The page explains how to install it with the install command instead.

The release workflow writes `update.json` from a short notes file committed with the release. It's covered by the signature on `SHA256SUMS`, so signing a release stays one step.

The server ignores any version that isn't newer than the one installed, so someone who controls the download site can't push an old release with a known flaw. They could keep serving an old release so that servers don't see newer ones. The updates page shows the newest release the server has seen, and its date, so that would show.

The update check is a connection the admin can turn off. Decision 11 builds the Network page with this decision, and this is its first switch.

### When updates install

A new **Updates** admin page, which opens only from the home network, offers two choices:

- **Install updates automatically** (the default). Updates install right after the nightly backup, at the hour set on the backups page, so there's always a backup from right before it. If that night's backup fails, the update waits for a night when it works. On a server without backups, the update still installs, and the page says there's no backup to fall back on.
- **Ask me first.** The page and the health page show that an update is waiting, and the admin selects **Install now**.

Automatic updates wait three days after a release is published, so a bad release can be replaced before most servers install it. Security releases skip the wait and install that night.

**Install now** is on the page with either choice, for any update that's waiting.

### Installing, in the helper

The helper is a separate program, `/usr/lib/accordion/accordion-update`, kept small because it runs as root. A systemd path unit starts it when the server writes a request file into its updates folder. The helper's service turns off its network entirely, so it can't connect anywhere.

1. **Check.** It copies the package, `SHA256SUMS`, and the signature into its own folder, which only root can open, then checks all three. It checks the signature with its own copy of the release key, the package's checksum, and that the version is newer than the installed one. Copying first means the server can't swap a file after it's checked.
2. **Snapshot.** It stops Accordion, then copies the data folder, except for `storage/` and restic's cache, into its own folder. File contents in `storage/` never change, so they don't need copying. The snapshot is small, mostly the database.
3. **Install.** It installs the package with `dpkg`, not `apt-get`, because it has no network. That's why `update.json` marks a release that needs a new dependency.
4. **Health check.** It waits for Accordion to report that it's ready, which the server does only after its migrations, storage checks, and listeners are done. The check passes if Accordion is ready within 10 minutes, and is still running, without having restarted, five minutes later.
5. **Roll back if needed.** If the check fails, it stops Accordion, reinstalls the previous package, puts the snapshot back, and starts Accordion again. It records why, and that version is never installed automatically again. A newer release is.

The helper keeps the installed package so it can roll back. `install.sh` saves a copy at install time, so a server can roll back from its first update too. The snapshot and the previous package stay until the next update succeeds.

A rollback undoes anything saved in the minutes between the new version starting and the check failing. That window only exists when a version starts and then crashes, and the server was stopped for the update right before. The page says so when it happens.

### What the admin sees

- **The health page** shows a waiting update, and the result of the last one.
- **The updates page** shows the installed version, the newest release, its notes, the schedule choice, **Install now**, and the history of updates and rollbacks.
- **After a rollback,** both pages say what happened in plain words. For example: "Accordion 0.3.0 didn't start, so Accordion went back to 0.2.1. Changes from the five minutes it ran weren't kept. Accordion won't try 0.3.0 again."

### Testing

Tests use their own signing key and a stand-in for GitHub on the test machine, as decision 11's CI check requires. The upgrade test that the full release waits for builds on this: it installs the previous release, adds data, updates through the helper, and checks the data. A second run installs a package that fails its health check, and checks that the rollback brings back the old version and its data.

### Revisit this decision if

- **The container or NAS packages are built.** They can't use `dpkg`, so they'll need their own updater, probably by replacing the container image with the same check and rollback.
- **The appliance image is built.** It will update the operating system too, which may call for two system partitions, one running and one being updated.
- **The release key has to change.** A release signed with the old key would carry the new one, so servers move over with their next update.

## Consequences

- **The suite gains** the update check, the updates page, and the helper program, with its service and path unit.
- **The server reports when it's ready to systemd.** The service changes from `Type=exec` to `Type=notify`.
- **The release key's public half is built into the server and the helper,** as decision 9 planned.
- **The release workflow writes `update.json`,** and lists it in `SHA256SUMS`. Each release gains a short notes file.
- **`install.sh` saves the installed package** for the helper.
- **The Network page is built with this,** as decision 11 says, with the update check as its first switch.
- **v0.1.0 servers update once by hand,** by running the install command, to get the updater.
- **The upgrade test** runs the real helper, and goes in CI once it's reliable there.

## Open questions

- **Pre-releases.** GitHub's `latest` address skips pre-releases, so servers only see full releases. Testers who want pre-releases install them with the install command. A test channel could come later.
- **A server whose backups keep failing** never installs updates automatically, including security releases. The health page already reports the failing backups, but security releases might need to install after a few nights anyway.
