# 32. A place for each app's files

- **Status:** Accepted
- **Date:** November 14, 2026
- **Replaces:** the "The folder" part of [decision 31](0031-notes.md)

## Context

[Decision 31](0031-notes.md) keeps each person's notes in a folder called Notes in their My Drive. Notes makes the folder the first time someone opens Notes, or takes over a folder called Notes that's already at the top of My Drive. Step 1 is built that way, and nothing has shipped.

Keeping notes as ordinary files in Drive is right: Drive shows everything in one place, with versions, the trash, sharing, search, and backups for free. Docs, Sheets, and Slides will store their files the same way ([decision 30](0030-documents-and-co-editing.md)).

But a folder in My Drive has problems:

- **It isn't obvious.** Someone opens Notes, and a folder appears in My Drive that they didn't make.
- **It can take over someone's own folder.** Plenty of people already have a folder called Notes, for notes that have nothing to do with the Notes app.
- **It doesn't scale.** Docs, Sheets, and Slides would each add another folder, and My Drive would fill up with folders the apps own.

Some pieces exist already:

- **Storage gives each person one top folder,** their My Drive: an item with no parent. About 20 places in core's storage, and three in Drive, assume there's only one.
- **Sync already has several roots** ([decision 23](0023-sync.md)): the person's top folder, then each item shared with them. The desktop app puts them side by side, in `My Drive` and `Shared with me`.
- **The plain view** ([decision 6](0006-file-storage.md)) shows each person's My Drive under their username. It's rebuilt from the database whenever it might be out of date.

## Decision drivers

1. **Everything in one place.** Drive shows every file a person has, including the files apps keep.
2. **My Drive holds only what the person put there.**
3. **Nothing hidden.** An app's files stay reachable in Drive, on computers, and in backups, even when the app is off.
4. **Obsidian still works.** Notes reach computers through the desktop app as a folder Obsidian can open.
5. **Scales to Docs, Sheets, and Slides** with no new design each time.
6. **One developer.**

## Options

### Option A: Each app gets its own place, shown as a tab in Drive

Each person gets one top folder for each app that keeps files, beside My Drive. Drive's sidebar shows it as its own tab, such as **Notes** under **My Drive**.

- **My Drive stays the person's own.** A folder called Notes in it is just a folder.
- **Every Drive feature works** in an app's place: versions, the trash, sharing, search, and moving files.
- **Fits sync as it is,** since sync already has more than one root.
- **The biggest change in core:** storage has to know which place each top folder is, and the code that assumes one top folder per person changes.

### Option B: App folders inside My Drive, hidden from its list

Storage keeps one top folder. Each app's folder sits in My Drive, marked as the app's, and Drive's list of My Drive leaves it out.

- **A smaller change in core.**
- **Still in My Drive everywhere else.** The desktop app, WebDAV, and the plain view show paths from My Drive, so the hidden folder appears on computers, inside My Drive.
- **Names still clash,** since two items in one folder can't share a name. Someone's own Notes folder would need another name.

### Option C: Keep decision 31, and ask which folder

Notes asks the first time which folder in My Drive to use, and suggests a new one called Notes.

- **The least work.**
- **Doesn't fix the scale problem.** Docs, Sheets, and Slides would each ask too, and My Drive still fills up with app folders.

## Comparison

| Driver | A: Its own place | B: Hidden in My Drive | C: Ask which folder |
|---|---|---|---|
| 1: One place | Yes | Yes | Yes |
| 2: My Drive is the person's own | Yes | Only in the web app | No |
| 3: Nothing hidden | Yes | Yes | Yes |
| 4: Obsidian | Yes | Yes, inside My Drive | Yes |
| 5: Scales | Yes | Yes | No |
| 6: One developer | Largest change | Medium | Smallest |

## Decision

**Each app that keeps files gets its own place for each person, beside My Drive, and Drive shows it as its own tab (option A).** Notes is the first. Docs, Sheets, and Slides follow the same way.

### Storage

- **Each top folder has a place:** `drive` for My Drive, or the app's name, such as `notes`. A person has at most one top folder for each place, made the first time it's needed.
- **`Storage::top_folder` still means My Drive.** A new `Storage::place_folder(owner, place)` gives an app its place's top folder.
- **Every path knows its place,** so search, **Recent**, and **Starred** say "Notes > Recipes", not "My Drive > Recipes".
- **Moving between places is allowed,** like moving between folders, as long as the owner stays the same. A Markdown file moved into the Notes tab becomes a note, and moving it out stops it being one.
- **One trash and one quota** cover every place. A restored item goes back to the folder it was in, as today.
- **When someone's account is deleted and their files go to someone else,** each place goes to the same place of the person receiving them, as a folder named after the person who left. Today, that only happens for My Drive.

### Drive

- **Each place is a tab** in Drive's sidebar, below **My Drive**, named after its app.
- **A tab shows while its app is on, or while its place holds files,** so turning Notes off never hides anyone's notes.
- **A tab works like My Drive's.** Items drag onto it and off it to My Drive or any folder, files from the computer drop into it to upload, and its folders open below it in the sidebar.
- **Moving something where more people can open it asks first.** Moving an item into a folder that's shared with someone who can't open the item yet, or that has a link, names who would gain access and offers **Move and share** or **Cancel**. The server makes the check, so every app gets it: `/move-check` says who would gain access, a move names the people the person agreed to, and the server refuses one that reaches anyone else. Undo and redo name the people the first move showed, so they ask again only about someone added since. Moving out of a shared folder doesn't ask, but its notice says who can't open the item anymore. A move made through the desktop app or WebDAV can't ask, so it names no one and goes ahead, as before.
- **WebDAV** serves each place at `/dav/places/<place>`, such as `/dav/places/notes`, beside `/dav/drive` and `/dav/shared`.

### Notes

- **Notes keeps its notes at the top of the Notes place.** It reads Markdown files in subfolders, and keeps images in an `Attachments` folder, as decision 31 says.
- **Folders in Notes.** Notes' sidebar lists the place's folders under a **Folders** heading, last, after **Trash**. Selecting one shows only the notes inside it and the folders below it, and a note taken there is saved there. Folders can be made, renamed, moved, and deleted in Notes as well as in Drive, and notes moved between them from a note's menu, the note itself, or by dragging it onto a folder. Notes moves things through Drive's API, so a move that shares something asks first, as it does in Drive. Labels stay the way to sort notes in Notes, since a note can have several labels but sits in only one folder.
- **Notes no longer makes or takes over a folder in My Drive.** Its table of folders goes away, since storage finds the place.

### Computers

- **The desktop app syncs each place as its own folder,** beside `My Drive` and `Shared with me`, such as `Accordion/Notes`. Obsidian opens that folder as a vault.
- **The plain view matches:** `<username>/My Drive` and `<username>/Notes`. It's rebuilt when the server starts, so it moves without a migration. Anything an admin points at the old layout has to change once.

### Notes made in step 1

Notes hasn't shipped, so there's nothing to migrate. Notes made on a test server stay where they are, in a folder called Notes in My Drive. Moving that folder's contents to the Notes tab in Drive makes them notes again.

## Consequences

- **Core:** a migration adds the place to top folders. Storage's top-folder code, search paths, sync roots, the plain view, and deleting an account all change, and storage says who a move would let in. Core's storage tests cover each place.
- **Drive:** the sidebar tabs, moving between places, WebDAV, and the paths in search and **Recent**.
- **Sync and the desktop app:** each place is a root with its own local folder. Older desktop apps would put a place in `Shared with me`, so the desktop app updates with the server.
- **Notes:** simpler, with no folder to find or remake.
- **Decision 31:** its status points here for where notes live. Everything else in it stands.
- **Decision 30:** Docs, Sheets, and Slides keep their files in places of their own.

### Testing

- **Storage:** a person's My Drive and Notes place are separate, with one of each. Paths, search, **Recent**, the trash, moving between places, sharing from a place, quotas, and deleting an account with each place in use.
- **Sync:** a new device gets every place as a root, and changes in a place reach it.
- **Drive:** the tabs appear and disappear with Notes, except when the place holds files. WebDAV reads and writes a place.
- **Notes:** a folder called Notes in My Drive is never touched. A folder in the sidebar shows only its notes, and a note taken there lands there.
- **Moving into a shared folder:** a move that reaches someone new is refused until they're named, through the API and the web app. Undo and redo don't ask again, unless someone was added in between. Moving between folders shared with the same people, or into one only the owner opens, doesn't ask.
