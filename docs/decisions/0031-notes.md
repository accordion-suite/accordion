# 31. Notes

- **Status:** Accepted. Where notes live is replaced by [decision 32](0032-a-place-for-each-apps-files.md).
- **Date:** November 14, 2026

## Context

Notes replaces Google Keep. The spec asks for:

- Quick capture: text, checklists, images, and voice memos, transcribed on the server.
- Pin, color, label, and archive notes, and reminders at a time.
- Sharing a note with another person and editing it together.
- Offline-first on phones, and a home-screen widget for capture.
- **Notes are Markdown files with YAML front matter in a Drive folder,** readable by Obsidian or any editor.
- Takeout import keeps Keep's labels, colors, checklists, images, and pinned state.

The setup wizard turns Notes on with Drive by default, and Notes requires Drive ([decision 7](0007-module-interface.md)).

Some pieces exist already:

- **Core's storage** holds files, versions, the trash, and shares, and modules use it through its API ([decision 6](0006-file-storage.md), [decision 16](0016-sharing.md)).
- **Drive has a Markdown editor** with Write and Preview tabs, which saves with `write_version`, so it never saves over changes it didn't see. The web app has its own Markdown renderer.
- **Search** already reads Markdown files ([decision 20](0020-search.md)), and **live updates** send hints when files change ([decision 19](0019-live-updates.md)).
- **[Decision 30](0030-documents-and-co-editing.md)** puts a co-editing service in core, with Yjs, for Docs, Notes, and Sheets. Each module gives it a format, and Notes' is Markdown.
- **The desktop app syncs Drive,** so a Notes folder in Drive reaches computers, where Obsidian can open it.

The server only runs Drive today. Notes is the second module, so the web app needs a way to move between modules, which the spec describes as an app grid in the top bar.

## Decision drivers

1. **A note is a Markdown file** that Obsidian or any editor reads, and edits without breaking.
2. **Fast to capture.** Opening Notes and typing takes a moment, on a phone too.
3. **Fast to list** a few thousand notes on a Raspberry Pi.
4. **Nothing leaks.** A note shared with one person shows nobody else anything.
5. **Keep's notes come over whole** through Takeout.
6. **Build once.** Notes is the first module to use decision 30's co-editing service, so building it there proves the service before Docs.
7. **One developer.**

## Options: where notes live

### Option A: A Notes folder in each person's My Drive

Pros:

- **What the spec says.** Notes sync to computers with Drive, and Obsidian opens the folder as a vault.
- **Drive's features come free:** versions, the trash, sharing, backups, and search.

Cons:

- **People can move, rename, or delete the folder,** so Notes has to cope.

### Option B: Storage of Notes' own, outside Drive

Pros:

- **Nothing in Drive to disturb.**

Cons:

- **Not files people can reach,** which breaks the spec and driver 1. Versions, the trash, and sharing would have to be built again.

### Option C: Any Markdown file anywhere in Drive

Pros:

- **Every Markdown file someone already has** shows in Notes.

Cons:

- **README files, code docs, and exported pages** would crowd the notes, and listing them means scanning all of Drive.

## Options: where colors, pins, labels, and reminders live

### Option 1: Front matter only

The file's YAML front matter holds everything.

Pros:

- **All in the file,** so it syncs, copies, and backs up with it.

Cons:

- **Listing reads every file,** which is slow on a Pi with thousands of notes.
- **Shared notes have one front matter for everyone,** but in Keep, each person pins, archives, and labels a shared note for themselves.

### Option 2: Notes' tables only

Pros:

- **Fast to list,** and each person has their own.

Cons:

- **Lost when the file is copied, synced, or restored,** and invisible in Obsidian.

### Option 3: Front matter, with Notes' tables as an index

The owner's choices are in the front matter. Notes' tables keep an index of every note, read from its file, and hold what other people choose for notes shared with them.

Pros:

- **The file is still the truth** for the owner's notes, and Obsidian sees labels as tags.
- **Fast to list,** from the index.
- **Each person has their own** pin, archive, and labels on a shared note, as in Keep.

Cons:

- **The index must follow the files,** including changes made in Obsidian or over WebDAV.

## Comparison

| Driver | A: Notes folder | B: Its own storage | C: Any Markdown file |
|---|---|---|---|
| Readable by Obsidian | Yes | No | Yes |
| Drive's features | Yes | Built again | Yes |
| Only notes in Notes | Yes | Yes | No |

| Driver | 1: Front matter | 2: Tables | 3: Front matter and an index |
|---|---|---|---|
| Travels with the file | Yes | No | Yes, for the owner |
| Fast to list on a Pi | No | Yes | Yes |
| Each person's own choices on a shared note | No | Yes | Yes |

## Recommendation

**Each person's notes are Markdown files in a Notes folder in their My Drive (option A). The owner's colors, pins, labels, and archive are in each file's front matter, and Notes keeps an index of them, plus what other people choose for notes shared with them (option 3). Reminders are each person's own, in Notes' tables only. Shared notes are edited together through decision 30's co-editing service, which Notes builds first.**

### The folder

- **Notes makes a folder called Notes** in someone's My Drive the first time they open Notes, and remembers it by its ID. Renaming or moving it is fine.
- **If it's deleted,** Notes makes a new one when it's next opened, and the old notes are in Drive's trash as usual.
- **Notes reads Markdown files in subfolders too,** since Obsidian users make them. New notes go at the top.
- **Images go in an `Attachments` subfolder,** as regular Drive files, linked from the note with ordinary Markdown, such as `![](Attachments/receipt.jpg)`. Obsidian follows these links.

### A note's file

```markdown
---
color: yellow
pinned: true
tags: [Home, Groceries]
---
- [ ] Milk
- [x] Bread
```

- **The title is the file's name,** as in Obsidian. A note saved without a title is named after its first words, and the card doesn't show a title that only repeats them.
- **Labels are `tags`,** the property Obsidian shows as tags.
- **Color is one of Keep's names,** such as `yellow` or `teal`, so Takeout's colors carry over. Each color has a light and a dark shade in the web app.
- **`archived: true`** archives a note. Archiving is a state, not a folder, so a note doesn't move on someone's computer when it's archived.
- **Checklists are Markdown task lists.** Keep moves checked items to the bottom, and Notes shows them that way without moving them in the file. As in Keep, each person's Notes settings can leave checked items where they are instead, and put new items at the top. The settings change only how lists show and where **Add item** puts an item.
- **Front matter Notes doesn't know is kept,** in its order, so Obsidian's own properties survive.
- **A file without front matter is a plain note,** with nothing set.
- **Deleting a note moves its file to Drive's trash,** where Notes' Trash shows it.

### The index

- **Notes' table has one row for each note:** its file, its contents' hash, title, color, pin, labels, archive, a short excerpt, and when it changed.
- **It follows the files.** Core's storage tells Notes when a file in a Notes folder changes, from any app, and Notes reads that file again. A check when the server starts catches anything missed.
- **Other people's choices** for a shared note (pin, archive, and labels) are rows of their own, never written to the owner's file. Color belongs to the note, as in Keep.

### Pages in the web app

- **Notes:** a box at the top to take a note, then pinned notes, then the others, as cards in columns that fit the screen.
- **Reminders, each label, Archive, and Trash,** in a sidebar, as in Keep.
- **Opening a note** shows it in a dialog to edit: the title, the text or checklist, and buttons for color, labels, reminder, archive, sharing, and delete.
- **Search** in Notes' top bar looks through notes only, through core's search, filtered to the person's Notes folder and their shared notes.
- **An app grid in the top bar** moves between Drive and Notes, and later modules join it. It shows only the modules that are on.

### Editing

- **The editor is plain text,** with Markdown shown as formatted on the cards. Checklist notes edit as a list of items with checkboxes. ProseMirror isn't needed, so Notes' code stays small.
- **A note saves as someone types,** a second after they stop, with `write_version`. A shared note saves through the co-editing service instead.
- **Notes shared with someone use the co-editing service** from decision 30, with one Yjs text for the body and another for the front matter, kept exactly as the file has it, so Obsidian's properties keep their order and spelling. The web app never edits the front matter: a color, a title, or a label changes through Notes' API, which first saves what people typed, then changes the file, and the change reaches everyone as edits. When the last person leaves, the service writes the Markdown file, with no data block, since notes have no comments. Changes from Obsidian or another app reach the note as edits, as decision 30 describes.
- **Viewers see the note but can't change it,** and editors edit, as in Drive.

### Sharing

- **Sharing a note shares its file,** with decision 16's Viewer and Editor.
- **Notes shared with someone show in their Notes,** with the owner's picture on the card, as in Keep.
- **A shared note's attachments are shared with it,** since its images are separate files.

### Reminders

- **Each reminder is one person's own,** the owner's too, as in Keep. Someone a note is shared with sets their own, a viewer included, and nobody sees anyone else's. Reminders live in Notes' tables, not in the file: a reminder is someone's alarm rather than part of the note, and Obsidian couldn't ring it. This changed on November 16, 2026, from the owner's reminder in the front matter, which would have shown the owner's plans to everyone who can open the file.
- **A reminder keeps its clock time and its time zone,** such as 09:00 in `Europe/Amsterdam`, from the browser that set it. A repeating one follows that clock through daylight saving time, so "every Monday at 9:00" stays at 9:00. A time the clocks skip rings once they've moved on, and a time they pass twice rings the first time. A monthly reminder on the 31st comes back on the 31st, or the month's last day.
- **A reminder notifies through the bell** and the event log ([decision 18](0018-the-event-log.md)) at its time, and as a browser notification if the web app is open and allowed to. Only its own person hears about it, and it's never in anyone's Activity.
- **Notes checks for due reminders at the start of every minute,** and right after starting, so a reminder can't be missed by a restart. A repeating one that missed several times while the server was off rings once, then waits for its next time.
- **Repeating reminders** come back daily, weekly, monthly, or yearly.
- **Snooze and done,** as in Keep, on the card, in the editor, and in the bell. Snooze rings again later: in an hour, later today, tomorrow morning, or next week. Done takes away a reminder that doesn't repeat, and lets one that does wait for its next time.
- **Phones get reminders** once the phone app exists, under its own decision on push.

### The Modules page

Notes is the first module an admin could turn off, so the **Modules** page that [decision 7](0007-module-interface.md) planned comes with it, in Settings under Server.

- **Each module is listed** with its name, a line about what it does, whether it's on, and how much space its data takes.
- **A switch turns a module on or off,** without a restart, as decision 7 says. Turning one off says first what happens: it disappears from everyone's apps, and its data stays.
- **Drive can't be turned off while Notes is on.** Its switch says why. Turning Notes on also turns on Drive, after asking.
- **The page shows the server's memory use,** as the spec asks.
- **Turning a module off keeps its data** in Drive and in its tables, and in backups. Turning it back on brings it back as it was.
- **Changes go in the admin log,** like other server settings.
- **The web app follows at once:** a live update tells every open tab, and the app grid and pages of a module that's off disappear.
- **The setup wizard turns on Drive and Notes,** and on servers set up before Notes, an update turns Notes on too, so it appears once the update installs.

### Later steps

- **Voice memos** record in the browser, save as an audio file in `Attachments`, and get a transcript in the note if the server runs the optional local speech model.
- **Keep's Takeout import,** which reads Keep's JSON for each note: title, text or checklist, color, pinned, archived, labels, images, and dates.
- **Offline editing** in the web app waits for its service worker, as Docs' does. Phones come with the phone app.

### Build order

1. **The module, the folder, and the index.** Notes in the web app: the list, taking a note, editing, colors, pins, labels, archive, the trash, search, and the app grid. The Modules page comes in this step too.
2. **The co-editing service in core,** and sharing notes.
3. **Reminders.**
4. **Images,** then voice memos.
5. **Keep's import,** with Takeout import for files.

### Testing

- **Files:** notes read from and written to Markdown keep unknown front matter, task lists, and links unchanged, and files made in Obsidian read correctly.
- **The index** follows notes changed through Drive, WebDAV, and sync, and the check at startup catches a change made while the server was off.
- **Access:** every address for the owner, an editor, a viewer, and someone with no access, as Drive's access test does. Another person's pins and labels never reach the owner's file.
- **Co-editing:** two editors typing in one note end up with the same file.
- **Reminders** fire once, at their time, after a restart too.

### Revisit this decision if

- People keep their notes in many folders, and want Notes to read any folder they choose.
- The index can't keep up with notes edited elsewhere, such as an Obsidian vault of tens of thousands of notes.

## Consequences

- **A new repo,** `accordion-notes`, made with `scripts/new-repo.sh`, with its own tables and addresses.
- **Core** gains the co-editing service from decision 30, and a way for a module to hear about changes to files in a folder.
- **The server** includes Notes, turned on with Drive, and gains the table of modules that are on and the Modules page.
- **The web app** gains Notes' pages, the app grid, and the note editor.
- **Drive** shows a note's Markdown as before, and offers **Open in Notes** for files in a Notes folder.

## Open questions

- **The home-screen widget,** which belongs with the phone app.
- **End-to-end encrypted notes,** which the spec lists as an option and the vaults decision will cover.
