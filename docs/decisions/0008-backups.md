# 8. How backups work

- **Status:** Accepted
- **Date:** October 22, 2026

## Context

The spec's goals include "Encrypted, versioned, automatic backups to a second disk or an off-site target". It also asks for:

- **Nightly, versioned, and encrypted** backups, on by default.
- **Four kinds of target:** a local disk, another Accordion box, an S3-compatible bucket, or an SFTP server.
- **Encryption before anything leaves the box,** in a "restic-compatible encrypted repository" that's "deduplicated, verifiable", with "many targets".
- **A one-click restore test.**
- **A restore to a fresh box in under an hour** for 500 GB.
- **A health page warning** when the last backup is more than 48 hours old.
- **The backup target listed on the Network page,** as an allowed outbound connection that the admin can turn off.

Everything Accordion keeps is in its data folder:

| Path | What it holds | Back it up? |
|---|---|---|
| `accordion.db` | Users, sessions, app passwords, and every module's tables ([decision 2](0002-metadata-database.md)) | Yes, as a consistent copy |
| `storage/blobs/` | File contents, named by hash and never changed ([decision 6](0006-file-storage.md)) | Yes |
| `gateway/` | The server's name, its DNS provider token, the Let's Encrypt account, and the certificate ([decision 5](0005-server-name-and-certificates.md)) | Yes |
| `storage/files/` | The read-only plain view | No. It's rebuilt from the database. |
| `storage/uploads/` | Uploads in progress | No |

Mail's Maildir and the search index will join the data folder later. Restoring one file or one old version doesn't need a backup: Drive's versions and trash already cover that for 30 days. Backups are for losing the disk, the box, or the house.

[Decision 4](0004-encryption-at-rest.md) encrypts the data volume, but that protects the disk in the box, not a copy somewhere else. So backups need their own encryption, and the target must learn nothing from what it stores.

## Decision drivers

1. **No lost data.** A backup must be consistent, and Accordion must check that it can be restored, not assume so.
2. **Restorable without Accordion.** If Accordion is gone, the owner can still get their files back with a standard, documented tool.
3. **Encrypted before it leaves the box.** A target, and anyone who steals from it, sees only encrypted data.
4. **Unattended.** Backups run every night with nobody signed in.
5. **Fast enough on a Raspberry Pi 5** to back up nightly and restore 500 GB in under an hour.
6. **Every target the spec lists,** without a different code path for each.
7. **No homemade cryptography.** One developer can't review a new encryption format well enough to trust a family's only copy to it.
8. **Little to maintain,** and nothing that phones home.

## Options

### Option 1: Ship restic and run it as a separate process

[restic](https://restic.net) is the program that defines the restic repository format. It's written in Go, released under the BSD 2-Clause license, and widely used. Accordion would ship a pinned, checksummed restic binary, and run it the way it would run any other tool.

Pros:

- **The reference implementation.** restic's encryption, deduplication, and repository checks have years of real use, and its format is documented.
- **Restoring without Accordion is a standard command.** With the repository and the key, `restic restore` works on any computer.
- **Every target the spec lists:** local folders, SFTP, S3-compatible storage, and restic's REST server protocol, which another Accordion box can serve.
- **Structured progress.** restic can report progress and results as JSON, so Accordion can show them on the health page.

Cons:

- **A second program to ship.** Each package, from the appliance image to the container, has to include restic for both x86_64 and ARM64, and Accordion has to track its releases.
- **Errors arrive as process output,** not as typed Rust errors, so Accordion has to map them to messages people can act on.
- **Memory use grows with the repository.** restic keeps its index in memory, which can reach a few hundred MB for a large repository. A Raspberry Pi 5 has room for that, but it needs measuring.

### Option 2: Use rustic's library inside the server

[rustic](https://rustic.cli.rs) is a Rust implementation of the same repository format, and `rustic_core` is its library. Accordion would link it into the server.

Pros:

- **One binary.** Nothing extra to ship or track.
- **Typed errors and progress** directly in Rust.
- **The same format,** so `restic restore` still works without Accordion.

Cons:

- **Not mature enough for a family's only copy yet.** Its library documentation calls it early-stage, with an API that will change. The rustic tool's own README has said it isn't yet recommended for production backups.
- **Backups run inside the server process,** so a bug or a crash in the backup can take the server down with it.

### Option 3: Accordion's own format

Accordion would chunk, deduplicate, and encrypt the data itself, with well-known building blocks such as XChaCha20-Poly1305.

Pros:

- **Full control** over the format, the memory use, and the targets.

Cons:

- **Homemade cryptography and a homemade format.** Both need expert review, and any mistake can make every backup unreadable or readable by others.
- **Not restorable without Accordion.**
- **Doesn't meet the spec,** which asks for a restic-compatible repository.

## Comparison

| Driver | Option 1: restic process | Option 2: rustic library | Option 3: own format |
|---|---|---|---|
| Proven with real data | Yes, for years | Early | No |
| Restorable without Accordion | Yes, with `restic` | Yes, with `restic` or `rustic` | No |
| Encrypted before leaving the box | Yes | Yes | Yes, if done right |
| Every target the spec lists | Yes | Most, through its backends | Only what Accordion builds |
| No homemade cryptography | Yes | Yes | No |
| A failure can't stop the server | Yes, separate process | No | Depends |
| Extra program to ship | Yes | No | No |
| Work for one developer | Low: run it and read its output | Medium: follow a changing API | High |

## Recommendation

**Option 1: ship restic and run it as a separate process.**

Backups are the last line of defense, so the most proven tool wins, even though it means shipping a second program. Running it as a separate process also keeps a backup problem from taking the server down. Option 2 is the better design once rustic is mature. It uses the same format, so switching later wouldn't touch anyone's backups.

### What a backup contains

1. Accordion copies the database with SQLite's online backup API, into a staging folder in the data folder.
2. restic backs up that copy, `storage/blobs/`, and `gateway/` as one snapshot.
3. Accordion deletes the copy.

Stored contents never change, so restic can read them while Accordion runs. New uploads during a backup add contents that the database copy doesn't use yet, which is harmless. To keep contents the copy does use from being removed partway through, storage pauses its cleanup of unused contents until the backup finishes.

Modules keep their data in the database and in storage, so they need nothing extra. Mail's Maildir is the first exception, and the module interface will gain a way to name extra folders when Mail needs it.

### The recovery key

Each repository is locked with a password, which Accordion calls the **recovery key**.

- **Accordion creates it** when backups are turned on: 30 random lowercase letters, shown in groups of five. That's about 141 bits.
- **The owner keeps a copy off the box,** printed or written down. Without it, no backup can be restored. Accordion says this plainly when it shows the key, and the health page warns until the owner confirms they've saved it.
- **The box keeps a copy too,** readable only by the `accordion` user, so nightly backups run unattended. Anyone who controls the running box can read it, which matches the trust in [decision 4](0004-encryption-at-rest.md).
- **One key for every target,** so the owner has only one thing to keep.

### Targets

The first three, all built into restic:

- **A local disk,** such as a USB drive plugged into the box.
- **An SFTP server,** such as a NAS or a friend's computer.
- **An S3-compatible bucket,** such as Backblaze B2 or a self-hosted MinIO.

**Another Accordion box** comes after those. It will serve restic's REST protocol in append-only mode over WireGuard, so the box being backed up can add snapshots but not delete them.

An owner can set up more than one target, such as a USB disk and an off-site bucket. Each target is its own repository, backed up in turn.

### Schedule and retention

- **Nightly,** at 3 a.m. by default, at a time the admin can change.
- **Snapshots kept:** seven daily, four weekly, and 12 monthly. Old snapshots are removed weekly.
- **A deleted user's data** stays in backups for up to 12 months, until the last monthly snapshot that holds it expires. The admin guide says so, as decision 4 asks.

### Checking and testing

- **After each backup,** restic checks the repository's structure.
- **Each week,** restic reads back an eighth of the stored data, so all of it is reread about every two months.
- **The restore test,** both monthly and on demand with one button, restores the newest database copy and a sample of files into a temporary folder. It checks the database's integrity and each file's SHA-256 hash, then deletes the test folder.

### Restoring

- **To a fresh box:** the setup wizard offers **Restore from a backup**. The owner picks the target, enters the recovery key, and picks a snapshot. Accordion restores the data folder, rebuilds the plain view, and starts.
- **Without Accordion:** the admin guide shows the `restic restore` command, and how to read files from `storage/blobs/` with the database.

### The health page

- **When the last backup finished,** with a warning after 48 hours.
- **The last check and the last restore test,** and whether they passed.
- **Space left on each target,** where the target can report it.
- **Whether the owner has confirmed saving the recovery key.**

### Revisit this decision if

- rustic's library reaches a stable 1.0 and holds up in testing. Moving to it would remove the second program without changing the format.
- restic's license or maintenance changes.
- Restoring 500 GB on a Raspberry Pi 5 takes longer than an hour.

## Consequences

- **Packages ship restic,** pinned and checked against its published checksum, for x86_64 and ARM64. The `accordion` repo's packaging builds track its releases.
- **Backups live in `accordion-core`,** as a `backup` service that takes the database copy, pauses storage cleanup, runs restic, and records results. The server adds the schedule, the admin pages, and the health checks.
- **Storage gains a way to pause the cleanup of unused contents.**
- **The setup wizard gains Restore from a backup,** and the admin panel gains backup settings.
- **Each backup target appears on the Network page,** and the CI check for outbound connections allows it.
- **The admin guide covers** the recovery key, choosing targets, retention, and restoring by hand.

## Open questions

- **Protection from a compromised box.** The box holds credentials that can delete its own backups, so ransomware on the box could too. Append-only REST targets and S3 Object Lock both prevent that. Which should Accordion recommend, and should it be on by default where a target supports it? [Decision 10](0010-backups-a-compromised-server-cant-delete.md) answers this.
- **How the owner confirms the recovery key.** Typing back one group might prove they wrote it down, without being a chore.
- **Restore speed.** The one-hour target for 500 GB needs measuring on a Raspberry Pi 5, with a USB disk and with an off-site bucket.
