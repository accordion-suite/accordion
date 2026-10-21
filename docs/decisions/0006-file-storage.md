# 6. How file contents are stored on disk

- **Status:** Accepted
- **Date:** October 21, 2026

## Context

The spec asks two things of file storage that pull against each other. Its principles say "Files stay as files on disk" and "Leaving Accordion must be as easy as joining". Its stack table says "Plain filesystem on an encrypted volume, content-hashed for dedup". Storing files by their content hash saves space, but leaves names like `3f9a…` on disk, which nobody can browse.

Drive is the first module that stores files, but not the only one. The spec says every Docs document is also a file in Drive, Notes are Markdown files, and Photos and Takeout import add more files later. Drive also needs:

- Version history (30 days by default) and a trash (30 days by default).
- Per-user storage quotas, and shared drives owned by a group.
- Resumable uploads of files up to 50 GB.
- WebDAV and the desktop sync client, which need stable file IDs and content hashes to detect changes.

[Decision 2](0002-metadata-database.md) keeps metadata in SQLite and user content outside it. [Decision 4](0004-encryption-at-rest.md) encrypts the whole data volume, so this decision doesn't add encryption of its own. Mail stays in Maildir, which the mail engine manages, so it's out of scope. So are sharing permissions and how sync resolves conflicts, which each need their own design.

## Decision drivers

1. **Readable without Accordion.** If Accordion is broken or gone, the owner can still find and copy their files with ordinary tools.
2. **No data loss on power cut.** The spec requires it, so the disk and the database must agree after a crash.
3. **Cheap versions and trash.** Keeping 30 days of versions shouldn't mean copying whole files on every save.
4. **Deduplication,** as the spec asks. Takeout imports, camera uploads, and shared copies often repeat the same file.
5. **Any file name users bring,** including from Takeout, in any script.
6. **Works on every install target:** ext4 on a Raspberry Pi, ZFS on TrueNAS, Btrfs on Synology, Docker volumes, and other NAS file systems.
7. **One developer.** Every structure on disk has to be kept consistent by one person.
8. **Any module can store files** through core's public API, under the rules in [decision 1](0001-where-core-services-run.md).

## Options

### Option 1: Plain folders are the source of truth

Each file lives at its real path, such as `files/alex/Documents/report.pdf`. The database indexes the folders. Old versions and the trash are kept in hidden folders.

Pros:

- **The most readable.** The disk looks like the user's Drive.
- **Leaving takes nothing.** Copy the folder.

Cons:

- **Two sources of truth.** The database and the folders can disagree after a crash in the middle of a rename or upload. If anyone edits the folders directly, Accordion has to notice and catch up, which means watching the file system and rescanning.
- **No deduplication,** except through file system features that not every install has, such as reflinks on Btrfs and XFS.
- **Moving a large folder is a file system operation** that can fail halfway, rather than one database transaction.
- **Desktop sync is harder.** Stable file IDs and change history have to be rebuilt from paths that can change under Accordion.

### Option 2: Content-addressed files only

Each file's contents are stored once, named by their SHA-256 hash, such as `blobs/3f/9a/3f9a…`. Names, folders, versions, and the trash exist only in the database.

Pros:

- **One source of truth.** A rename, a move, or a restore is one database transaction.
- **Deduplication and versions are free.** A new version adds one file. An identical upload adds none.
- **Corruption can be found.** Rehashing a file shows whether the disk has damaged it, so the health page can check stored files regularly.
- **Any name works,** because names never reach the file system.

Cons:

- **Not readable without Accordion.** Without the database, the disk is a pile of hashes. An export command helps someone leaving, but not someone whose Accordion won't start.

### Option 3: Content-addressed files, with a read-only plain view

Option 2, plus a folder that Accordion keeps in step with the database. It holds a hard link to the current version of each file at its real path, such as `files/alex/Documents/report.pdf`. A hard link is a second name for the same data, so the view takes no extra space.

Pros:

- **Everything option 2 has.** The database and the hashed files stay the only source of truth.
- **Readable without Accordion.** The view shows each user's current files with their real names, and ordinary tools can copy them.
- **The view can be rebuilt.** If it's damaged or out of date after a crash, Accordion recreates it from the database.

Cons:

- **More to keep in step.** Every rename, move, delete, and restore also updates the view, after the database commits.
- **The view must stay read-only.** An edit through a hard link changes the stored file itself, which would break its hash and every version that shares it. Only the `accordion` user can write to it, and Accordion checks hashes before trusting a file.
- **Hard links need one file system.** The view and the hashed files must live on the same volume. Most NAS and Linux file systems support hard links, but each platform needs checking.
- **Timestamps aren't per file.** All links to the same data share one modification time, so a copy taken from the view doesn't keep each file's own date.
- **Only current files appear.** Old versions and the trash are only reachable through Accordion.

## Comparison

| Driver | Option 1: plain folders | Option 2: hashed only | Option 3: hashed, plus view |
|---|---|---|---|
| Readable without Accordion | Yes | No | Yes, current files |
| Consistent after a crash | Hard: two sources of truth | Yes | Yes. The view is rebuilt. |
| Cheap versions and trash | Moves or copies | Yes | Yes |
| Deduplication | No, without reflinks | Yes | Yes |
| Any file name | Limited by the file system | Yes | Yes in Accordion. The view has the file system's limits. |
| Every install target | Yes | Yes | Needs hard links on the data volume |
| Work for one developer | High: watching and reconciling | Lowest | Medium |

## Recommendation

**Option 3: content-addressed files, with a read-only plain view.**

Option 2 is the simplest and safest, but it breaks the principle that files stay files. Option 1 keeps the principle at the cost of two sources of truth, which is where data loss bugs come from. Option 3 keeps one source of truth and still leaves readable files on disk. The view is derived data, so a bug in it can't lose anything.

### Layout

```
<data>/storage/
  blobs/sha256/3f/9a/3f9a…    file contents, named by hash, never changed
  uploads/<id>                uploads in progress, for resuming
  files/<username>/…          the read-only view of each user's current files
  files/shared/<drive name>/… the view of each shared drive
```

### Writing a file

1. Stream the upload into `uploads/`, hashing it on the way.
2. Flush it to disk, then rename it into `blobs/`. If the hash already exists, discard the upload instead.
3. Commit the file's row in SQLite.
4. Update the view.

A crash before step 3 leaves at most an unused file, which cleanup removes. A crash after step 3 leaves at most a stale view, which Accordion repairs when it starts.

### Hashes

SHA-256, because sync clients, WebDAV, and S3-compatible backup targets already use it, and `accordion-core` already depends on it. The Raspberry Pi 5's processor has SHA-256 instructions, so hashing keeps up with the disk.

### Versions, trash, and cleanup

- **A version is a database row that points at a blob.** Saving a file adds a row and, unless the contents already exist, one blob.
- **Deleting moves a file to the trash.** Its rows stay until the trash period ends.
- **Cleanup removes blobs that nothing points at,** and only after they've been unused for a day. That keeps it from racing with an upload that's about to commit.
- **Modules record which blobs they use** through core's API, so cleanup never needs to read a module's tables.

### Names

- **Names are UTF-8, up to 255 bytes,** the limit of most file systems, so the view can show every name exactly.
- **Names can't contain `/` or NUL, and can't be `.` or `..`.**
- **Takeout import shortens longer names** and lists each one in its import report.

### Quotas

Each file counts at its full size against its owner's quota, including its versions and anything in the trash, even when the contents are shared with another file. Quotas then stay predictable, and deduplication saves space on the server's disk without changing anyone's numbers.

### Who owns what

Core's storage service owns the blobs, the folder tree, versions, the trash, and the view, because Drive, Docs, Notes, Photos, and Takeout import all need them. Drive's module owns its interface: the web pages, WebDAV, and share links. This follows decision 1. Modules use storage only through core's public API.

### Revisit this decision if

- A supported install target can't use hard links on its data volume.
- Benchmarks on the Raspberry Pi 5 show that hashing slows uploads noticeably.
- Households ask for old versions or the trash in the view.

## Consequences

- **A `storage` module in `accordion-core`,** with its own tables for files, folders, versions, the trash, and blob references, and an API for modules.
- **Startup checks.** The server refuses to start if the view and the blobs aren't on the same file system, or if the data volume doesn't support hard links. The health page shows when the view is being rebuilt.
- **A regular integrity check** rehashes stored files a few at a time, and the health page reports any damage.
- **Backups copy the database and `blobs/`,** and skip the view, which can be rebuilt. The backup decision confirms this.
- **The admin guide explains the view:** where it is, that it only holds current files, and that it's read-only.
- **An export command** copies a user's files, with their own dates, for people leaving Accordion.

## Open questions

- **What Docs puts in the view.** A document's real contents are its editing history (a CRDT), which isn't readable on its own. Docs could also keep an HTML or Markdown copy of each document's current text. The Docs decision should settle it.
- **File names that differ only in case,** such as `Report.pdf` and `report.pdf`. Linux allows both, but Windows and macOS don't, so sync clients need a rule. The sync decision should settle it.
