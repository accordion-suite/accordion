# 18. The event log

- **Status:** Accepted
- **Date:** November 2, 2026

## Context

Accordion keeps no record of what happened to a file, so three planned features have nothing to show:

- **Activity:** a tab in Drive's details panel that says who changed what and when, such as "Sam Rivera renamed Budget.xlsx".
- **Notifications:** a bell in the top bar for things that happen to you, such as someone sharing a folder with you.
- **The audit log:** the spec lists a "per-user audit log (logins, shares, admin actions) kept on the box, never exported automatically" for privacy professionals.

Later modules need the same things. Mail, Docs, and Notes will each have changes to show and people to notify.

Some pieces exist already:

- **Core records sharing history** in `storage_share_events`, as [decision 16](0016-sharing.md) promised for the audit log.
- **The server writes a technical log** through `tracing`, such as "shared an item". It goes to the system journal for admins fixing problems. It isn't meant for family members, and the journal rotates it away on its own schedule.

The spec's privacy rules apply: zero telemetry, and nothing leaves the box. The log is also a record of what people in a household do, so it needs clear rules about who can read it.

## Decision drivers

1. **Never misses a change.** An audit log with gaps can't be trusted.
2. **Nothing leaks.** Nobody learns about a file, or a folder's name, through the log that they couldn't already open.
3. **Not surveillance.** A family server records what changed, not what each person looked at.
4. **One log for every module,** as decision 16 made one sharing model.
5. **Small on a Raspberry Pi,** in space and in time.
6. **One developer.**

## Options: where events are kept

### Option A: A table in the main database

Core adds an `events` table to the database that holds everything else ([decision 2](0002-metadata-database.md)). Core writes each event in the same transaction as the change it records.

Pros:

- **No change without its event.** If the change is saved, so is the event, even after a power cut.
- **Backed up and encrypted with everything else,** with no new files to track.
- **Queries join the folder tree,** so a folder's activity covers what's inside it, and the reader's access is checked in the same query.

Cons:

- **The main database grows.** At the measured size below, that's tens of megabytes a year for a family.
- **Pruning old events writes to the main database,** which upkeep has to spread out.

### Option B: A separate database file

Core keeps events in their own SQLite file next to the main one.

Pros:

- **The main database stays the same size,** and the log can be deleted or moved on its own.

Cons:

- **Two transactions for every change.** A crash between them loses the event, or records a change that didn't happen.
- **Backups and restores have to keep the two files in step.**
- **No joins with the folder tree** for access checks, so each event is checked one at a time.

### Option C: Log files

Core appends one line of JSON for each event to a file, and starts a new file each month.

Pros:

- **The simplest writes,** and old months are deleted whole.
- **Readable with ordinary tools.**

Cons:

- **The same gaps as option B,** since the file and the database are written separately.
- **Slow to search.** Showing one file's activity means reading every line.
- **A second format to back up, encrypt, and keep private.**

## Options: what's recorded

### Option 1: Changes only

Adding, editing, renaming, moving, deleting, and sharing items, plus sign-ins and admin actions.

Pros:

- **Answers what families ask,** such as "who moved the tax folder?"
- **Records nothing about reading,** so a parent can't see which files a teenager opened.
- **Small,** since sync clients and WebDAV apps read far more often than they write.

Cons:

- **Can't answer "who downloaded this?",** which a business audit might need.

### Option 2: Changes and views

Also opens, previews, and downloads, as business audit logs often do.

Pros:

- **The fullest record,** for privacy professionals who want to know if a file was read.

Cons:

- **Surveillance inside a family.** It records what each person looks at.
- **Much larger.** Every preview, thumbnail, and sync read is a view.
- **Hard to make meaningful,** since a sync client reading a file isn't a person reading it.

## Comparison

| Driver | A: Main database | B: Separate database | C: Log files |
|---|---|---|---|
| Never misses a change | Yes | No | No |
| Nothing leaks | Checked in one query | Checked one event at a time | Checked one event at a time |
| One log for every module | Yes | Yes | Yes |
| Small on a Raspberry Pi | About 220 bytes for each event | The same | Smaller files, slow reads |
| Work for one developer | Small | Medium | Medium |

| Driver | 1: Changes only | 2: Changes and views |
|---|---|---|
| Not surveillance | Yes | No |
| Small | Yes | No |
| Answers "who read this?" | No | Yes |

## Measurements

On the development machine, with SQLite in WAL mode as the server uses it, a table of 1,000,000 events spread over 20,000 files and five people. Each event has the time, who did it, how, the action, the item, the item's owner, another person for shares, and a little JSON for renames and moves. It has three indexes: by item, by who did it, and by owner.

| Measurement | Result |
|---|---|
| Size | 217 MB, or 217 bytes for each event, indexes included |
| Writing one event in its own transaction | 0.03 ms |
| One file's latest 50 events | 0.12 ms |
| A folder of 100 files, latest 50 events inside it | 21 ms |
| One person's latest 50 events | 0.09 ms |
| Deleting the oldest 500,000 events at once | 3.7 s |

A family that uploads 50 photos a day between them, and edits, moves, and shares as usual, makes about 100,000 events a year. That's about 22 MB a year, less than a few minutes of phone video. A Raspberry Pi 5 is slower than the development machine, so these figures join the Raspberry Pi measurements already planned.

## Recommendation

**Keep events in a table in the main database, written in the same transaction as each change (option A). Record changes, not views (option 1).**

### What's recorded

| Area | Events |
|---|---|
| Files and folders | Added (uploaded, created, or copied), edited (a new version), renamed, moved, moved to the trash, restored, deleted for good, an earlier version restored, description changed |
| Sharing | Shared, access changed, sharing stopped |
| Your account | Signed in, sign-in failed, signed out, a device's sign-in ended, password changed, app password created or removed |
| Admin actions | Account created, deleted, or changed (role, quota, password reset), module turned on or off, backup settings changed, backup restored, update installed |

Not recorded:

- **Opening, previewing, downloading, searching, and listing,** in the web app, over WebDAV, or from sync clients.
- **Stars and folder colors,** which only their owner sees.

### What later modules record

Each module adds its own events and notifications when it's built, under the same rules: changes, not views, and only to people who can open what the event is about.

| Module | Events | Notifications |
|---|---|---|
| Docs | Commented, replied, resolved, and reopened a comment. Edited, with a co-editing session counting as one edit for each person, as for files. | You're mentioned in a comment, someone replies in a thread you're in, or someone comments on a document you own. |
| Notes | Edited a shared note, shared it, or stopped sharing it. | Someone shares a note with you, or edits one you shared. |
| Mail | Forwarding, filters, and app passwords changed, since a stolen account often sets up forwarding first. Sent and received mail aren't events. | None from the log. Mail apps already show new mail. |

- **Events point to content, they don't copy it.** An event about a comment names the comment, and Docs keeps its text. Deleting the comment deletes its words everywhere, including the Activity tab and notifications, which read the text from Docs when they show it.
- **Version history stays with each module.** Docs' snapshots, like Drive's file versions, are kept by the module. The log only records that an edit happened.

### How events are kept

- **One `events` table in core,** with a number that gives the order, the time, who did it (none for the server itself), how (web app, WebDAV, Accordion's apps, or the server; the desktop and phone apps can be told apart once they exist), the action, the item, the item's owner, another person for shares and admin actions, and a few details as JSON, such as an old and a new name.
- **Folders are stored by ID** and named when the event is read, so a later rename shows the folder's current name, and a folder the reader can't open is never named.
- **Core writes its own events** in the same transaction as the change: storage for files and sharing, identity for accounts. `storage_share_events` moves into the new table.
- **Modules record their own events through core's API.** Core's API doesn't join a module's own transactions ([decision 7](0007-module-interface.md)), so a module records the event right after its change, before answering the request. A crash in between can lose that event. Changes to files, which matter most, happen inside core, so their events can't be lost.
- **Repeated edits become one event.** A desktop app that saves a document every few seconds would otherwise fill the log, so edits to the same file by the same person within 10 minutes update one event.
- **Putting something back right away leaves no trace.** Restoring an item within 10 minutes of the same person moving it to the trash, such as with **Undo**, removes the trash event instead of adding another. File Explorer deletes a folder one item at a time, and Drive puts the items back so the folder goes to the trash whole, which this keeps out of the log too.
- **Large actions keep one event for each item,** so each file's own history is complete, and are grouped when shown, such as "Sam Rivera uploaded 212 files".

### Who sees what

- **Activity,** in Drive's details panel:
  - An item's events, and for a folder everything inside it, newest first.
  - Shown to anyone who can open the item now, including events from before they had access, so a shared folder's history makes sense to everyone in it.
  - A move to or from a folder the reader can't open says "moved out of this folder" or "moved here", without naming the other folder.
- **Notifications,** behind a bell in the top bar:
  - Someone shared something with you, or changed your access.
  - Someone added or changed files in something you shared, grouped by folder for each day, such as "Sam Rivera added 12 files to Holiday".
  - Opening the bell clears its count. Each notification stays highlighted until the person selects it, and they can remove one, or clear them all.
  - Removing someone's access doesn't notify them.
- **Your activity log,** in **Settings**, under **You**:
  - Your own sign-ins, with the device, browser, and its address on the home network or VPN, failed sign-ins, and changes to your password and app passwords.
  - What you shared with others, and what others shared with you.
- **The admin log,** in **Settings**, under **Server**, for admins only:
  - Every admin action, with who did it.
  - Failed sign-ins for every account, so an admin notices someone guessing passwords.
  - It doesn't show anyone's file activity. An admin can read the database directly, as the server's owner can read everything (the spec puts a malicious owner out of scope), but Accordion doesn't make watching family members a feature.

### How long events are kept

- **One year by default.** The admin can choose 90 days, one year, two years, or forever in **Settings**. Upkeep deletes expired events once a day, a thousand at a time, so the server stays responsive.
- **Read notifications are deleted after 90 days.**
- **Deleting an item for good deletes its activity.** Events about it elsewhere, such as a share in someone's activity log, keep the action but lose the name.
- **Deleting an account** deletes its own activity log. Its events in other people's history stay, under the name "Deleted account", so a shared folder's history still makes sense.
- **Backups include the log,** since it's in the main database, and it's encrypted at rest the same way ([decision 4](0004-encryption-at-rest.md)). Nothing is exported automatically.

### Live updates

The bell checks for new notifications every 60 seconds while the web app is open, as thumbnails use a timer today. A decision on pushing changes from the server follows this one. The event log is what that push channel would send.

### Testing

- **Core's tests** check that:
  - each change to storage, sharing, and accounts writes exactly one event, in the same transaction, and a failed change writes none
  - repeated edits become one event
  - expired events are deleted, and deleting an item or an account removes or hides what it should
  - someone without access to an item sees none of its events, and a folder they can't open is never named
- **Drive's access test** gains the new activity and notification addresses.
- **The web app's tests** cover the activity tab, grouping, and the bell.

### Revisit this decision if

- The log grows much faster than measured.
- Families or privacy professionals ask for a record of downloads. It could come as a setting that's off by default.
- The sync protocol needs a list of changes. Sync needs every change, kept until each device has seen it, while this log is pruned, so the sync decision will likely keep its own list.

## Consequences

- **Core** gains the `events` table and its migration, which moves `storage_share_events` into it. It also gains the activity, notification, and log queries with their access checks, the retention setting, and nightly pruning. Storage and identity record events in their transactions.
- **Drive** gains addresses for an item's activity and for notifications, and records its own events, such as description changes.
- **The server** records admin actions, and shows the admin log and the retention setting in **Settings**.
- **The web app** gains the **Activity** tab in the details panel, the bell and its list, and the activity log in **Settings**.
- **Unchanged:** the technical log in the system journal, which stays for admins fixing problems.

## Open questions

- **Notifying about failed sign-ins.** Whether someone should get a notification when their account has failed sign-ins, or only see them in their activity log.
- **Email notifications,** once Mail exists.
- **Exporting your own log** by hand, which the spec allows as long as it's never automatic.
