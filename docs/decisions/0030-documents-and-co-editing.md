# 30. Documents and co-editing

- **Status:** Accepted
- **Date:** November 14, 2026

## Context

Docs is the next module after Drive. The spec asks for:

- A rich-text editor: headings, lists, tables, images, code blocks, checklists, links, and comments.
- Real-time co-editing with presence cursors, built on CRDTs (data structures whose copies merge the same way on every device) so offline edits merge cleanly.
- Suggesting mode (tracked changes), and comment threads with @mentions that can be resolved.
- Version history with named versions and restore.
- Templates, page setup, and print-faithful PDF export.
- **Every document is also a file in Drive,** so sharing and permissions work the same way.

It also names the tools: a ProseMirror editor with Yjs for co-editing. And it says files stay files on disk, in open formats.

Notes needs part of the same thing. The spec lets people share a note and edit it together. Sheets will need it later.

Some pieces exist already:

- **[Decision 19](0019-live-updates.md)** sends hints over server-sent events, and planned a WebSocket for each open document for Docs, with the session checked on the upgrade, a matching `Origin` header, and the same access rule as the file in Drive. Ending the session closes it.
- **[Decision 16](0016-sharing.md)** has Viewer and Editor, and planned a commenter level for when Docs has comments.
- **Drive keeps every version of a file,** for as long as the admin's retention setting says, and an editor can restore one.
- **Drive's text editor** already saves with `write_version`, which refuses to save over contents it never saw.
- **[Decision 17](0017-office-previews.md)** runs LibreOffice in a sandboxed worker for Office previews, which can also convert files.
- **Search** reads text from files by their contents ([decision 20](0020-search.md)).

## Decision drivers

1. **A document is a file.** It can be copied, moved, shared, synced to a computer, backed up, and restored like any file, and still opens in Docs afterward.
2. **Open format.** Without Accordion, someone can still open and read their documents.
3. **Nothing leaks.** Only people who can open the file in Drive see the document, its comments, or who else is in it.
4. **No lost work.** A crash, a dropped connection, or a laptop closed mid-sentence loses at most a second or two of typing.
5. **Small on a Raspberry Pi,** and quick to load on a phone.
6. **Safe to open.** A document can't run code in someone else's browser, even if a hostile file was imported.
7. **One developer.** Use maintained libraries, and build the shared parts once, for Docs, Notes, and Sheets.

## Options: what the file in Drive holds

### Option A: A pointer to the document, kept in Docs' database

The file in Drive is a small stub with the document's ID, as Google Drive's `.gdoc` files are. The document itself lives in Docs' tables.

Pros:

- **Simple saving.** Docs' tables are the only copy.

Cons:

- **The file isn't the document.** Copying, backing up, or syncing it copies a pointer. Every Drive feature needs a Docs hook, and a stub synced to a computer opens nothing without the server. That breaks drivers 1 and 2.

### Option B: HTML, with comments and co-editing state in Docs' database

The file is an HTML page that any browser opens. Comments and the CRDT's state live in Docs' tables.

Pros:

- **An open format** that opens anywhere, offline, with no app.

Cons:

- **Two halves to keep together.** Make a copy, restore a version, move a file to another server, or restore a backup, and the comments stay behind, unless every one of those paths learns about Docs.

### Option C: HTML that carries its own co-editing state

The same HTML page, with the document's CRDT state and comment threads embedded in it as a data block, which browsers don't run or show. Docs' tables only hold what hasn't been saved to the file yet.

Pros:

- **The file is the whole document.** Make a copy, restore a version, sync it, or restore a backup, and the comments and history come with it, through Drive features that know nothing about Docs.
- **An open format.** The body is plain HTML that any browser shows, with a print stylesheet.
- **Docs' tables stay small,** a log of edits waiting to be saved, not a second home for documents.

Cons:

- **Bigger files.** The state is about 1.6 times the text, base64 adds a third, and the HTML body is there too. A 10-page document is about 100 KB.
- **Outside edits need care.** If another program changes the body, the embedded state no longer matches it, so Docs has to notice and bring the state in line.

### Option D: Word or OpenDocument files

`.docx` or `.odt` as the format on disk, as Office and LibreOffice use.

Pros:

- **Opens in Office and LibreOffice** with every feature.

Cons:

- **Writing them faithfully is enormous,** and every save through a converter loses something. Co-editing state has no place in them.

### Option E: Markdown

Pros:

- **The plainest format,** and the one Notes uses.

Cons:

- **Can't hold what Docs needs:** merged table cells, colors, comments, page setup, or images inside the file.

## Options: how edits merge

### Option 1: Yjs in the browser, its Rust port yrs on the server

Pros:

- **The most used CRDT for editors,** with a ProseMirror binding, cursors, and undo that only undoes your own edits.
- **Small:** no WebAssembly, so it adds about 37 KB compressed.
- **The server can read documents** with yrs, the same format in Rust.
- **Yjs 14 adds attributions and change sets,** which are what suggesting mode and "who wrote this" in version history need.

Cons:

- **Yjs 14 isn't final.** On November 13, 2026, npm's stable Yjs is 13.6.33, and 14 is a release candidate under a new name, `@y/y`, with its ProseMirror binding in beta. It isn't known yet when yrs follows.
- **yrs has unsafe code,** and AFFiNE's developers wrote their own Rust port partly over it. The server reads updates sent by signed-in editors, so a crafted update could crash it.

### Option 2: Automerge

Pros:

- **A Rust core,** with JavaScript through WebAssembly, and a ProseMirror binding.

Cons:

- **Much bigger in the browser,** about 320 KB compressed of WebAssembly, by one 2026 comparison.
- **Its ProseMirror binding is young,** at version 0.2.0.

### Option 3: Loro

Pros:

- **Rust first,** with rich text built on the Peritext and Fugue research, and good merging of formatting.

Cons:

- **WebAssembly again,** about 180 KB compressed by the same comparison. Its npm package unpacks to 22 MB.
- **Its ProseMirror binding is at 0.4.4.**

### Option 4: ProseMirror's own collaboration

The server puts every edit in order, and clients rebase theirs on it, a form of operational transformation.

Pros:

- **No CRDT at all.**

Cons:

- **The server must run ProseMirror's transforms,** which are JavaScript. A Rust server would have to reimplement them.
- **Long offline edits merge poorly,** which the spec asks for.

## Options: what the server does with edits

### Option i: Pass them on and store them

The server keeps each document's updates and sends them to everyone else, without reading them. Clients write the HTML.

Pros:

- **Least server code.**

Cons:

- **Clients write the HTML,** so a hostile editor could put a script in the file that runs when someone opens it elsewhere. That breaks driver 6.
- **The server can't merge outside edits,** import files, or check that an update fits the document's schema.

### Option ii: Read them with yrs

The server holds each open document in memory, applies every update, and writes the file itself.

Pros:

- **The server writes the HTML** from the document's schema, so only known elements and attributes reach the file.
- **Outside edits, imports, and checks** happen in one place.

Cons:

- **The schema exists twice,** in ProseMirror in the browser and in the server's HTML writer, and the two must agree.

## Comparison

| Driver | A: Pointer | B: HTML and database | C: HTML with its state | D: Office | E: Markdown |
|---|---|---|---|---|---|
| A document is a file | No | Comments stay behind | Yes | Yes | Yes |
| Open format | No | Yes | Yes | Yes | Yes |
| Holds what Docs needs | Yes | Yes | Yes | Yes, but hard to write | No |

| Driver | 1: Yjs and yrs | 2: Automerge | 3: Loro | 4: ProseMirror's own |
|---|---|---|---|---|
| Size in the browser | About 37 KB | About 320 KB | About 180 KB | Smallest |
| Offline merging | Yes | Yes | Yes | Poor |
| Rust server can read it | Yes | Yes | Yes | No |
| Suggesting mode | With Yjs 14 | Build it | Build it | Build it |

## Measurements

**The editor in the browser,** bundled and minified with esbuild 0.28.2, then compressed with gzip:

| What | Minified | Compressed |
|---|---|---|
| ProseMirror with lists, tables, history, and its usual plugins | 248 KB | 77 KB |
| The same, with Yjs 13.6.33, y-prosemirror 1.3.7, and y-protocols | 367 KB | 114 KB |

The web app's first load is 78 KB of its 150 KB budget, so Docs' code loads only when someone opens a document, as Settings' does now.

**Documents in yrs 0.28.1,** typed one character at a time as an editor sends them, with some words corrected and some made bold, on the development machine:

| Document | Text | Saved state | Loading the state | Memory of the whole program |
|---|---|---|---|---|
| About 1 page | 3 KB | 5 KB | 0.1 ms | 3 MB |
| About 10 pages | 32 KB | 51 KB | 1.3 ms | 5 MB |
| About 50 pages | 162 KB | 265 KB | 5.6 ms | 12 MB |

The state is about 1.6 times the text, and loading is cheap enough to do each time someone opens a document. A Raspberry Pi 5 is slower, so these join the Pi measurements.

## Recommendation

**Each document is one HTML file in Drive that carries its own co-editing state (option C). Edits merge with Yjs, starting on 13.6 and moving to 14 before suggesting mode (option 1). The server reads every edit with yrs and writes the file itself (option ii). The co-editing machinery lives in core, so Notes and Sheets use it too.**

### The file

- **Named like any file,** with `.html` on its name in Drive and Docs too, as the developer chose on November 17, 2026. It's the name a computer that syncs it shows.
- **An ordinary HTML page.** It has a small stylesheet for screen and print, and no scripts.
- **A data block holds the rest,** as `<script type="application/x-accordion-docs">`, which browsers neither run nor show. It holds the CRDT state, the comment threads, and a format version.
- **Each image is stored once,** in the body as a `data:` address. The state refers to it by its hash.
- **Drive knows a document by the marker at its start,** checked when the file is saved, so a web page someone uploads stays a web page.
- **At most 50 MB,** and images are scaled down to 2,560 pixels on their long side when they're added, by the sandboxed worker, which also checks them.

### Opening and editing

- **Opening a document opens a WebSocket** at `/api/docs/{item}/live`. It follows decision 19's rules: the session is checked on the upgrade, the `Origin` header must match, and ending the session closes it.
- **The messages are Yjs's standard sync and awareness messages,** so the browser uses the usual Yjs client code. Awareness shows each person's name, color, and cursor.
- **Access follows Drive.** Editors edit. Viewers see the document live, but the server ignores any edits they send. When someone's access changes, core's live hint makes the server check again, and closes their connection or makes it read-only.
- **The first person to open a document loads it** from the file's data block. Everyone after joins the copy in memory. When the last person leaves, the server saves and lets it go.
- **Over HTTP/1.1.** Browsers open a separate connection for each WebSocket when the server doesn't offer WebSockets over HTTP/2, so the gateway stays as it is.

### Saving

- **Every accepted update goes into Docs' log in SQLite** before it's sent on, so a crash loses nothing that anyone else saw.
- **The file is written 30 seconds after the last edit,** at most every 5 minutes while editing goes on, and when the last person leaves. It's saved with `write_version`, against the contents Docs last saw. Once the file is written, the log for it is emptied.
- **Saves from one editing session become one version in Drive,** not one version every 30 seconds. Core gains a way to replace the latest version when it came from the same session less than 30 minutes ago.
- **Named versions** are Drive versions with a name, which core adds to versions. Restoring one works as restoring any version does. See [Version history](#version-history).

### Changes from outside Docs

A document's file can change without Docs: someone restores an older version, a computer syncs an edited copy back, or it's saved over WebDAV.

- **If the file's state already contains everything Docs has,** Docs adopts it. This covers a server restored from a backup.
- **Otherwise, Docs takes in the change to the file's body.** It works out the difference between the body as it was last saved and the file, and applies it as edits amid any edits made since, as a three-way merge would. Anyone in the document sees the change happen, typing that wasn't saved yet isn't lost, and comments stay wherever their text survives. This is what makes restoring an old version work: merging an older state would bring nothing back, but the difference from the saved body takes the later text out.
- **The file's body always wins over its state,** since the body is what another program edited.

### Safety

- **The server writes the body from the schema.** Only the schema's elements and attributes reach it, never a script, a style, or an event attribute.
- **Imported HTML goes through the same schema,** so a hostile page loses everything that isn't document content.
- **The server reads updates with panics caught,** so a crafted update fails its own document's connection and nothing else. Fuzz tests of the update reader run in CI.
- **Limits:** 1 MB for each update (images upload separately), and 50 people in one document.

### Comments, mentions, and suggesting

- **Comment threads live in Docs' tables while a document is open,** and in the file's data block once it's saved, so they travel with it. Each thread is anchored by Yjs relative positions, which stay with the text as others edit around it.
  - Built November 25, 2026: the table, `docs_comments`, holds each document's threads. It takes them in from the file the first time they're needed, and wins from then on, so restoring an older version of the file neither brings back deleted comments nor takes away new ones, as in Google Docs. Each change writes the threads into the file at once, after saving any typing not saved yet, since a file changed under a document that hasn't been saved yet would otherwise replace the typing. A comment's text in the file has `<` written as `\u003c`, so it can't end the data block.
  - Anyone who can open a document sees its comments, viewers too, since the file they can download carries them. Its editors comment, reply, resolve, and reopen; a comment's author edits it; and its author or any editor deletes it, as in Google Docs. Deleting a thread's first comment deletes the thread. A reply to a resolved thread opens it again.
  - **Make a copy** leaves comments out unless asked, as Google Docs does.
  - The editor highlights each open thread's text, with cards beside the page where the window is wide enough and a panel at the side otherwise. The panel lists every thread, resolved ones and ones whose text was deleted too. Comments go in Ctrl+Alt+M, the toolbar, and **Insert** > **Comment**.
- **Comments use ordinary requests,** not the document's WebSocket, so a commenter can comment without being able to edit. The Commenter level joins Viewer and Editor in sharing, as decision 16 planned.
  - Built November 25, 2026: core's share levels and access gain Commenter between Viewer and Editor, so the highest share still wins and every check for "editor or above" stays as it was. Commenters do what viewers do in Drive, Notes, WebDAV, and sync, which tells devices "viewer" since they don't know the level. In Docs they start threads, reply, resolve, reopen, and edit or delete their own comments; deleting someone else's is for editors. Their editor is read-only and says **Comment only**, with **Insert** > **Comment** and Ctrl+Alt+M. Drive's Share dialog offers **Commenter** for documents, the only files with comments so far.
  - **Make a copy** gained **Copy comments**, off unless chosen.
  - On the way, core stopped losing typing to a change from outside before a new document's first save. A room without a saved state to merge from made the document match the file; it now keeps the state it reads the file into as that base, and keeps it across a restart too.
- **@mentions** notify through the event log ([decision 18](0018-the-event-log.md)), and only people who can open the document can be mentioned.
  - Built November 26, 2026: typing @ in a comment box lists everyone else who can open the document, by name or username, and choosing one writes `@` and their name. The comment carries who each mention is, in Docs' table and the file, and the editor shows them as names. Only names still in the text when it's sent count, and the server refuses anyone who can't open the document.
  - Core's event log gains two personal events, as reminders are: `mentioned`, for each person a new comment or an edit newly mentions, and `replied`, for everyone else who wrote in a thread someone replies to, as Google Docs tells people about comments that involve them. Only that person's bell rings, they're never in activity, each is its own line in the bell with the start of the comment, and it leads to the thread. Someone who can no longer open the document hears nothing.
  - Writing the comments into the file no longer records an edit, since the document says the same. Before, a comment rang the owner's bell as "Sam edited Plan.html" beside the mention. Core's `Storage::write_unrecorded_edit` saves the version without an event, and Docs writes comments with it.
- **Comments travel in Word and OpenDocument files, both ways.**
  - Built November 26, 2026: Word and OpenDocument downloads carry every thread as the format's own comments. Replies answer their thread's first comment, as Word 2013 and later mark them (`commentsExtended`) and as LibreOffice does (`loext:parent-name`), and resolved threads are marked done. Each thread is about the text it's about in Docs, found from the document's co-editing state, and one whose text is gone sits at the document's start. LibreOffice reads both back with the same authors, replies, and text.
  - **Open with Docs** brings comments in from Word files, as Google Docs' and Word's exports write them, and from OpenDocument files, as LibreOffice writes them, with replies, resolved threads, authors' names, and times. Word's own local times give way to the UTC ones it keeps beside them (`commentsExtensible`). A comment marked only where it is, with no text, becomes a thread about no text, which the editor lists in its panel. Their writers have no account here, so they're known by name, as comments from a file made on another server are. Comments are no longer on the warning's list.
  - The new document gets its co-editing state as it's made, so each thread has positions in it from the start. A Google Docs resume from the developer's Takeout export came in with both comments on their words, "Hello" and "Education". A comment added in the browser went out with them in both downloads.
- **Suggesting mode waits for Yjs 14,** whose attributions record who made each change. If yrs hasn't caught up by then, this decision is revisited.

### Version history

Settled with the developer on November 27, 2026, for step 7. It works as Google Docs' does, with three differences: a named version is kept past the retention period, any file in Drive can have names, and changes are compared from saved versions, since who typed what needs Yjs 14.

- **Editors only,** as in Drive's **Versions** dialog and Google Docs. Viewers and commenters don't see the menu items.
- **Where:** **File** > **Version history** > **See version history** (Ctrl+Alt+Shift+H, as in Google Docs) and **Name current version**. The history has its own address, `/docs/d/{id}/history`, with `?version=` for the version shown, so the browser's Back button and links work.
- **A full-page view.** The page swaps to the chosen version, read-only, laid out as the editor lays it out: the same paper, pages, headers, footers, footnotes, and table of contents. Comment highlights and carets aren't shown. A bar at the top has a back arrow to the document, the version's name or date, **Restore this version**, and **Make a copy**. On phones, the list comes first, and selecting a version shows it.
- **The list,** at the right, newest first, under day headings (**Today**, **Yesterday**, then dates). The top one reads **Current version**. Each shows its date and time, its name above the date if it has one, and the people whose changes went into it, each with a dot in their caret color. A ⋮ menu on each has **Name this version** (or **Rename** and **Remove name**), **Restore this version**, and **Make a copy**. **Only named versions** at the bottom filters the list. Versions are already one per editing session ([Saving](#saving)), so there's no second level to expand, unlike Google Docs.
- **Show changes,** a switch at the bottom of the list, off at first. On, the version shown is compared with the one just before it in the full list, even while only named versions are listed. Added words are underlined in color and removed ones struck through. The color is the person's when one person made the version, and the app's accent when several did, since without Yjs 14 Docs can't tell whose a word is. Paragraphs are matched first, then words within them (`Intl.Segmenter`). A picture, table, or line that changed is outlined whole, and formatting-only changes aren't marked. The browser compares the two versions, which the server sends through the schema.
- **Naming.** A name is up to 100 characters, and two versions can share one. **Name current version** saves typing first (`save_now`), then names the newest version, so the name covers what's on screen. Naming records no event and rings no bell.
- **Restoring** saves typing first, then makes the version current as a new version (`restore_version`), as Drive does. Docs takes it in as a change from outside ([Changes from outside Docs](#changes-from-outside-docs)), so anyone with the document open sees it change, and comments stay where their text survives. The page goes back to the editor with a notice naming the version and an **Undo**, which restores the version that was current before. Nothing asks first, since every version stays in the list.
- **Make a copy** makes a new document from the version, beside the original if the person can add there, else in their Docs place, as **File** > **Make a copy** does. It has no comments, since comments belong to the document, not to its versions.
- **Kept until unnamed.** A named version isn't deleted when the admin's retention period ends, as Google Drive's **Keep forever** works. It still counts toward the owner's storage. Removing the name lets it expire again on the usual schedule, counted from when it was saved. Deleting the file for good deletes its named versions too. Drive's **Versions** dialog says so.
- **Who made a version.** Core records each person whose changes went into a version: the person saving, for an upload or a WebDAV or sync save, and for co-editing, everyone who edited since the last save, since a room now keeps only the last one. A version that replaces the session's last one ([Saving](#saving)) adds its people to that one's. Restoring records the person who restored. Versions saved before this change, and versions from a Takeout import, list no one.
- **Drive** shows names and people in its **Versions** dialog for every file, and gains **Name this version** there, so a spreadsheet or photo can be kept too.

The build splits in three parts:

1. **Core and Drive:** version names and people in storage (a migration adds `name` to `storage_versions` and a `storage_version_people` table), retention skipping named versions, co-editing rooms passing everyone who edited, Drive's API (`PUT` and `DELETE` on `/items/{id}/versions/{version}/name`, names and people in the list), and Drive's **Versions** dialog. Built November 27, 2026. A session's next save doesn't replace a named version, so a name always keeps what it was given to. `write_edit` takes the room's editors, and comments saved with `write_unrecorded_edit` name no one, since they don't change what the document says. In Drive's dialog, **Name** or **Rename** on each version opens a box in its row, and an empty name removes it.
2. **Docs' history page:** Docs' `/documents/{id}/versions` routes (the list with people's names, a version through the schema, restore and name after `save_now`, copy), and the web's history page, list, restore with **Undo**, names, and **Only named versions**. Built November 27, 2026. A version reaches the page as a co-editing state made from its file through the schema, which the page lays out with the print view's code, so it has the editor's pages, headers, footers, and footnotes. `versions/current/name` names what's on screen. Restoring answers with the version that was current before, for **Undo**. Three fixes in core came with it: restoring a version now wakes an open document, which didn't notice a restore from Drive either; the empty update every editor sends as it joins is no longer an edit, which had put people who only opened a document among a version's people and saved a version of its own after a change from outside; and a format can say two files hold the same document (`Format::same_document`), so taking in a restored version, whose co-editing state differs from the room's, doesn't save it again as another version.
3. **Show changes:** web only (`docs/changes.ts`). Built November 28, 2026. The page merges the two versions into one document, the newer one's blocks and words with the older one's removed ones put back where they stood, and marks it with decorations, so the print view lays it out on pages as any version. Blocks are matched by their text, then blocks left between are paired where they share enough words, and paired paragraphs, list items, and quotes are compared word by word. Removed words come before the words that took their place, and changes with only spaces or marks between them read as one. A table whose text changed is outlined whole, as is a picture, line, page break, or table of contents that came or went. Only the lines take the person's color, not the text, so it stays readable whatever the color. The hint under the switch names the version compared with, says when the first version has none, and says when only formatting changed.

### Import and export

- **Open with Docs** makes a new document from an HTML, Markdown, Word, OpenDocument, Rich Text, or plain text file, as Google Docs does. The file stays as it is, and the document goes beside it, or in the person's own Docs when they can't add to its folder.
- **Docs reads Word and OpenDocument files itself,** chosen by the developer on November 23, 2026, in place of LibreOffice, which this decision first named. LibreOffice is optional on a server, so with it, Word files couldn't be opened where it isn't installed, and its HTML loses footnotes, headers and footers, weights, and page setup. Docs already wrote both formats itself. Reading them is Docs' own code, run by the server's own program in the sandbox as `docs-import` (decision 1), the way core reads Office files' text for search. It writes Docs' page, which the server reads through the schema again, and the server shrinks its pictures as it does pictures people add. LibreOffice is used only for old `.doc` and `.rtf` files, when it's installed: core's `Thumbnails::office_to_word` has it save them as Word files in the sandbox, with only the reader for the file's format and the same settings as for thumbnails, and Docs reads the Word file. Without LibreOffice, Docs says it's needed and what to do instead.
- **Open with Docs warns before it converts** when the file has things the editor can't keep, such as text boxes, Word's tracked changes, shapes, equations, or fields other than page numbers and a table of contents, or paper Docs doesn't have. The warning also lists the fonts Docs uses in place of ones it doesn't have (see **Fonts** under the build order). A file with nothing to lose opens at once, and the editor names the fonts it replaced in its status line, as it does for a paste.
- **A Word or OpenDocument file Docs wrote comes back the same:** Docs' own styles, list numberings, table style, page fields, and table of contents read as Docs' defaults.
- **OpenDocument files are read from their styles,** as LibreOffice lays them out: common and automatic styles with their parents, list styles and how lists carry on, the outline's numbering on headings, master pages for the paper, headers, and footers (a first page with a master page of its own counts as a different first page), and LibreOffice's way of saving a Word file's page break and headings. A Word file LibreOffice saves as OpenDocument opens as the Word file does.
- **Download as** HTML (the file itself), Word, OpenDocument, Markdown, or PDF. PDF comes from the browser's print of the editor's own pages (see **Pages** under the build order).
- **The original Word file is still edited in Word or LibreOffice** on a computer, through the desktop app or the network drive.

### Editing Word files in place, a later step

Google Docs edits a `.docx` as a `.docx` by default, with a badge by its name. It converts the file to its own format while it's open, and writes Word format back on save. Its help says some Word features are lost or can't be edited that way, and a warning on the badge says when editing would change something.

Accordion can do the same once Docs is built, with one rule stricter than Google's: **edit in place only when nothing would be lost.**

1. **The converter reports what it would drop.** Converting to HTML and back to Word lists every part of the file the editor doesn't keep.
2. **A file with nothing to drop opens for editing in place.** Docs keeps a hidden copy of the document's co-editing state, and saves each version of the file as `.docx` through LibreOffice. Co-editing works as for any document.
3. **A file with something to drop opens read-only,** with a note listing what's missing and an **Open with Docs** button that makes a copy.
4. **Word file versions stay in Drive,** so a save that went wrong can be undone like any other.

This waits until Docs' own documents work. It also needs a test set of real Word files, to check that converting to HTML and back keeps them the same.

### Where it lives

- **Core gains the co-editing service:** documents in memory, the WebSocket and its messages, the edit log, saving, and checking access. Each module gives it a format: Docs turns a document into HTML and back, and Notes into Markdown. Notes keeps plain Markdown files, without a data block, since notes have no comments.
- **The `accordion-docs` repo** holds the schema, the HTML writer and reader, comments, and Docs' addresses, made with `scripts/new-repo.sh`.
- **The editor is in `accordion-web`,** under `src/docs/`, loaded only when a document opens.
- **Search** reads the body's text and skips the data block. **Thumbnails** show the first lines, as text files' do.

### Offline

- **Synced to a computer, a document opens in any browser,** read-only, without the server.
- **Editing offline in the web app** waits for its service worker, which offline files in Drive also need. The CRDT state merges when it reconnects.

### Testing

- **The two schemas agree:** a set of sample documents goes through ProseMirror's HTML output and the server's, and the results must match.
- **Co-editing:** two and three simulated editors type at once, with dropped connections and a server restart in the middle, and end up with the same document and file.
- **Outside changes:** restoring an older version, a synced edit, and a WebDAV save each make the document match the file, and comments survive where their text does.
- **Access:** every address and the WebSocket, for the owner, an editor, a commenter, a viewer, and someone with no access, as Drive's access test does. Losing access closes the connection.
- **Safety:** hostile HTML and Word files import without scripts, and the update reader's fuzz test.

### Build order

Agreed November 17, 2026. On November 20, after a sweep of Google Docs' and Proton Docs' features, the developer added steps 3 and 4.

1. **The module and the editor.** The `accordion-docs` repo, Docs' own place beside My Drive ([decision 32](0032-a-place-for-each-apps-files.md)), Docs' home page, the HTML file with its data block, and a co-edited editor with headings, text styles, links, lists, checklists, tables, and code blocks. Drive recognizes documents and opens them in Docs.
2. **Images, page setup, print to PDF, and Download as.**
3. **Editing basics,** in three parts. It comes before Open with Docs, so a converted Word file keeps its colors, sizes, fonts, and alignment.
   1. **Text:** font, font size, text color, highlight color, superscript and subscript, and clear formatting.
   2. **Paragraphs:** alignment, indent, line and paragraph spacing, the Title and Subtitle styles and headings 4 to 6, list styles and restarting numbering, and page breaks.
   3. **Tools:** find and replace, word count, an outline of the headings, smart quotes and automatic links, a list of keyboard shortcuts, cell colors and merging cells, and **Move to trash** and **Make a copy** in the File menu.
4. **Pages,** in three parts:
   1. **Pages, headers, and footers:** the editor shows separate pages, and printing prints those pages, with headers, footers, a different first page, page numbers, and the number of pages.
   2. **Footnotes.**
   3. **A table of contents.**
5. **Open with Docs** for HTML, Markdown, Word, and OpenDocument files, in two parts:
   1. **Word, web pages, Markdown, and plain text,** built November 23, 2026: **Open with Docs** in Drive's menus and viewer, the warning, and the font notices.
   2. **OpenDocument files,** which Docs reads itself, and old `.doc` and `.rtf` files, through LibreOffice where it's installed, built November 25, 2026.
6. **Comments,** the Commenter level in sharing, and @mentions, in four parts:
   1. **Comment threads,** built November 25, 2026: comments on text, replies, resolving, editing, and deleting, kept with the document and in its file, and live for everyone who has it open.
   2. **The Commenter level** in sharing, across core, Drive, and Docs, built November 25, 2026.
   3. **@mentions** and notifications in the bell, built November 26, 2026.
   4. **Comments in Word and OpenDocument files,** both ways: written into downloads, and read from files opened with Docs, which Google Docs' Takeout exports carry, built November 26, 2026.
7. **Named versions** and version history in Docs, in three parts, as [Version history](#version-history) describes: names and people in core and Drive, Docs' history page, and **Show changes**.
8. **Templates.**

**Fonts.** Docs bundles 40 open fonts from Fontsource, in every weight and language each one has, and the browser downloads only the ones a page shows. Google Fonts isn't used, since loading from it would tell Google who's reading. They include every open font in Google Docs' font menu. Raleway joined on November 22, 2026, and Alegreya, Bree Serif, Comic Relief, Permanent Marker, Pinyon Script, Ultra, and Varela Round joined later that day, with weights. Fontsource's variable font is used where it has one, so a font's nine weights cost about what its regular and bold did.

- **Docs always names the font it shows,** chosen by the developer on November 22, 2026, for transparency and consistency. Before, documents named the Microsoft fonts that six of Docs' fonts match and showed the open ones. Now the font menu, the file, and the downloads say Arimo, not Arial. Six fonts are made to the letter widths of Word's: Arimo (Arial), Carlito (Calibri), Caladea (Cambria), Cousine (Courier New), Gelasio (Georgia), and Tinos (Times New Roman), and Comic Relief matches Comic Sans MS. Text set in them breaks into lines the same way in Docs, in print, and in Word. Text with no font of its own is Arimo at 11 points, as Google Docs uses Arial.
- **A font Docs doesn't have gets one it has,** wherever it comes from: a paste, an opened file, an outside change to the file, or a document saved before this change. A font with the same letter widths comes first, such as Arimo for Arial or Helvetica. Then a font later in the CSS list that Docs has, then one that looks like the first (Noto Sans for Verdana, Oswald for Impact, Montserrat for Proxima Nova), then one of its kind (Tinos for serif, Cousine for monospace), then Arimo. The editor says what it did in the status line, once per font while the page is open: "Docs doesn't have Verdana, so it uses Noto Sans, which looks like it." The editor (`fonts.ts`) and the server (`fonts.rs`) keep the same lists.
- **Weights,** as in Google Docs: a font with more than one weight has a submenu, from **Thin** to **Black**, each shown in its weight. Choosing a font sets it in regular. A weight other than regular and bold is the `font_weight` mark (`weight`, 100 to 900), and bold stays the `bold` mark. Bold makes a weight bolder as Google Docs does: under 400 shows as regular, under 700 as bold, and heavier ones stay. Only `font-weight: 700` or `bold` reads as bold, so pasted 600 is Semibold. A name with a weight in it, such as "Raleway Light", is the font in that weight. A weight a font doesn't have becomes its nearest.
- **In the file,** a font is `font-family:'Carlito','Calibri'`, with the font of the same widths after it for browsers without the font, and a weight is `<span data-weight="300" style="font-weight:400">` inside `<strong>`: Docs' own weight, and the weight it shows. Word downloads name a weight in the font, such as "Raleway Light", as Word names weights, and gain a font table that gives each font a family and pitch, and its Microsoft twin as its other name, for Word to pick a font like it where it isn't installed. Whether Word uses the other name hasn't been tried. OpenDocument downloads give the weight as a number. A download looks right where its fonts are installed: on Linux, Arimo, Tinos, and Cousine fall back to the Liberation fonts, which have the same widths.

**Paragraphs.** Lines are 1.15 apart, where single spacing is Arial's own line height, as in Word. Paragraphs have 6 points after them, and headings, the title, and the subtitle have Google Docs' space around them, the same in the editor, the file, and the downloads. A paragraph's alignment, indents, line spacing, and space before and after are its own, and Enter carries them to the next line, as in Google Docs and Word. After a heading, a title, or a subtitle, the next line is normal text. Headings have Google Docs' look, chosen by the developer on November 22, 2026: regular weight, with headings 3 to 6 gray, so bold is a choice made with **B**, as in Google Docs and Word. Text pasted from Google Docs keeps the parts of a heading it has as regular. A paragraph that goes into a list leaves its indents behind, since the list places its text. Page breaks go only between the document's own blocks, never in a list, a quote, or a table, where printing can't break a page.

**Tables and lines as Google Docs copies them.** Added November 22, 2026, after a resume pasted from a Google Docs template came out unlike it. Google Docs lays such pages out with a table without lines and copies a horizontal line as a picture named "horizontal line". So a cell keeps a border for each side (`border_top`, `border_right`, `border_bottom`, `border_left`, in points: none is the usual thin gray line and zero is no line), one `border_color`, and its `padding`. A row keeps its least height (`height`), and a line keeps its `thickness` and `color`. A pasted table takes its columns' widths from its `<col>`s, as Google Docs copies them, or from each cell's width, as Word does, and shrinks to fit the page if it's wider. A table that names `border:none` and cells that name no border have no lines. The table menu gains **Hide borders** and **Show borders**. The file, Word, and OpenDocument keep all of it. A document with borders or lines of a color of its own has no dark mode, like one with colored cells. Page margins aren't copied, so a template made for narrower margins needs them set in **Page setup**.

**Menus.** On a document, the top bar shows the document's icon and name where other pages show **Accordion Docs**, as Google Docs does, with Share just left of the app picker. The icon leads back to Docs' home. On phones the name and Share wrap to a row of their own. Under the top bar is a menu bar, as in Google Docs: **File** (New document, Share, Download, Rename, Page setup, Print), **Edit** (Undo, Redo, Cut, Copy, Paste, Paste without formatting, Select all, Delete), **View** (Full screen), **Insert** (Picture, Table, Link, Horizontal line, Page break), and **Format** (Text, Paragraph styles, Align and indent, Line and paragraph spacing, Bullets and numbering, Quote, Table and Picture when one is selected, Clear formatting), with submenus. People who can only view get File, Edit, and View. Browsers let a page paste only after asking, or not at all, so **Paste** asks for the clipboard and, if refused, says to press Ctrl+V. Part 3's tools add to these menus: find and replace to Edit, the outline to View, and a **Tools** menu with word count and the list of keyboard shortcuts.

**Pages.** Started November 21, 2026. On a computer, the editor shows the document on sheets of its paper, with a gap between them, as Google Docs does. On phones the text takes the screen's width, without pages, headers, or footers.

- **One document, broken by spacers.** The text stays one ProseMirror document. After each change, the editor measures where its lines fall as though nothing pushed them down, picks where each page ends, and puts a spacer there that reaches the top of the next sheet's text. Between blocks it's a block. Inside a paragraph it's a float as wide as the line, so the line before keeps its alignment. A list item, quote, or table that starts a page moves whole, with its bullet or checkbox. A heading at the end of a page goes to the next with the text after it, as Word keeps headings with what follows: it goes when the first line after it doesn't fit, and stays once that line is on its page.
- **Table rows break between lines,** as Word and Google Docs break them by default, so a row taller than a page goes on over as many pages as it needs. Each cell's text flows on its own, with a spacer in each cell that goes on, and continues at the top of the next page under the cell's padding. A row whose first lines don't fit starts the next page whole, and the table's first row takes the table with it. Where a page cuts a row, the row ends under the last of its text on the page with its bottom borders, and starts the next page with its top borders, as in Word. A cell that spans rows doesn't break. A row that moves whole has its spacers before each cell's first block, not inside its first paragraph, since Chrome prints a row whose cells start with a float on the next sheet of paper, spacers and all.
- **Sheets behind the text, margins over it.** Whatever crosses the end of a page, such as a table's borders, never shows in the margins or the gap between sheets.
- **Printing prints the same pages.** A hidden frame as wide as the paper lays the document out again with the same code and styles, with no gap between sheets and `@page` margins of zero, so each sheet is a sheet of paper, whatever the window's width. The server's print page is gone.
- **Headers and footers** are Yjs fragments of their own beside the document's (`header`, `footer`, `first_header`, `first_footer`), edited together like the document, and `different_first` in the page map. Each sits half an inch from the paper's edge, as in Google Docs and Word, and pushes the text away when it's taller than the margin. Double-clicking a margin, or **Insert** > **Header** or **Footer**, opens it in place, with **Different first page** and **Remove header** beside it. The toolbar formats whatever is open. A header left empty goes.
- **Page numbers and counts** are a `field` mark on a stand-in character, so they keep their font, size, and color like any text, which y-prosemirror wouldn't keep on an inline node. Each page shows its own number. **Insert** > **Page numbers** puts one at the right of the header or footer, on every page or from page 2.
- **In the file,** headers and footers are `<header data-part>` and `<footer data-part>` in `<main>`, before and after the text, and a page number is `<span data-field="page">`. Word and OpenDocument downloads get real headers and footers, a different first page, and PAGE and NUMPAGES fields. Markdown leaves them out.
- **Printing breaks only where the editor does.** The text has no orphans or widows of the browser's own (`orphans: 1`, `widows: 1`), which would move a paragraph's last line to the next sheet of paper. Printing lays the pages out again until they stop moving.
- **Footnotes,** built November 22, 2026, as in Google Docs: **Insert** > **Footnote** (Ctrl+Alt+F) puts a number where the caret is and opens its note at the bottom of the page, under a short line, in 10 point. Notes are numbered in the order they come. Clicking a note opens it there, Escape goes back to the text after its number, and deleting the number deletes the note.
  - **A footnote holds its note.** It's an inline node, `footnote`, whose content is the note's text and line breaks, so it moves, copies, and undoes with its number, and the server keeps it where it is (`Inline::Footnote`). Its number steps over as one letter. The note is edited in an editor of its own whose changes go into the document's as steps, so undo and co-editing work as for any text. Someone else's change replaces y-prosemirror's whole document, so an open note finds its footnote again by its Yjs element.
  - **Each page leaves room for its notes.** A line goes to the next page when it and its notes don't both fit, so a note is always on its number's page. Notes don't continue onto the next page as Word's can. Table rows hold their cells' notes the same way. Without pages, as on phones, the notes follow the text.
  - **In the file,** each number is `<sup><a href="#fn-1" id="fnref-1" data-footnote-ref>1</a></sup>`, and the notes are a `<section data-footnotes>` at the end of `<main>`, as GitHub writes footnotes, so Docs reads GitHub's too. Word and OpenDocument downloads get their own footnotes, and Markdown GitHub's `[^1]`. Headers and footers have no footnotes. Word count leaves notes out, as Google Docs does.
- **A table of contents,** built November 22, 2026, as in Google Docs: **Insert** > **Table of contents** puts one where the caret is, as **Plain text** (page numbers), **Dotted** (dots leading to them), or **With blue links** (no page numbers). It lists headings 1 to 6 that have text, wherever they are, indented by level, and not the title or subtitle. Selecting an entry's text goes to its heading. Selecting the table elsewhere shows buttons for its style.
  - **It keeps itself up to date.** Google Docs and Word keep a copy of the entries and need **Update**. Docs keeps only the table's style (`table_of_contents` in the editor and Yjs, `Block::Contents` on the server), and the editor and the server each find the entries again from the headings. So the table is never out of date, and two people editing at once never write two lists into the document. It goes only between the document's own blocks, as a page break does, and headers and footers have none.
  - **On pages,** each entry shows its heading's page number, and a long table breaks between entries where a page ends, the table drawing the spacer before the entry. Without pages, as on phones, entries have no numbers. Printed PDFs link each entry to its heading.
  - **In the file,** it's `<nav data-contents="dotted">` with a link to each heading, and the headings get IDs named as GitHub names Markdown's headings, so a Markdown download's list of links works there too. Reading the file keeps only the style. Word downloads get Word's TOC field, with each heading bookmarked and a PAGEREF for its page, and OpenDocument gets its table of contents with the same template, so Word and LibreOffice can update them. Their page numbers are the editor's, which the download carries as `?pages=`, as Google Docs' Word download carries its own. Word and LibreOffice add a paragraph's space after to the next one's space before, where the editor takes the larger, as browsers do, so their pages can come out longer than Docs' until they update the table.

The developer wants as many open fonts as possible. Google Fonts has about 1,900 families under open licenses, about 1 GB in all, so they can't all be bundled. After Editing basics, before Pages, Docs plans an optional font pack: an admin turns it on in Settings, the server downloads it from Accordion's own release (never from Google), and the font menu gains **More fonts** with a search.

Suggesting mode (Yjs 14), editing Word files in place, and offline editing come later.

### Revisit this decision if

- Yjs 14 or yrs stall, or yrs can't read Yjs 14's attributions. y-octo, AFFiNE's Rust port, is the first place to look.
- Documents with many images make files too big to sync and version comfortably. Images could then move to their own files.
- Many people want Word as the format on disk.

## Consequences

- **Core** gains the co-editing service, a commenter level in sharing, names on versions, and grouping a session's saves into one version.
- **A new repo,** `accordion-docs`, made with `scripts/new-repo.sh`.
- **The web app** gains the editor, loaded when a document opens, and Docs' pages.
- **Drive** recognizes documents, shows their icon, opens them in Docs, and offers **Open with Docs** for HTML, Markdown, Word, and OpenDocument files.
- **Later, Word files** with nothing the editor would drop open for editing in place, as `.docx`.
- **The server** routes the WebSocket, and includes Docs.
- **Notes** reuses the co-editing service with Markdown.
- **The gateway** is unchanged.

## Open questions

- **Templates and page setup:** where templates live, and which page sizes and margins to offer.
- **Who typed each change** in **Show changes,** which Yjs 14's attributions would make possible.
- **When to move to Yjs 14,** and whether yrs reads what it writes.
- **Where the co-editing state of a Word file edited in place lives,** since a `.docx` has no place for the data block. Docs' tables are the likely answer, which brings back option B's problem for those files only.
- **The font pack:** how it's packaged, how fonts reach the editor and the print page, and whether a document can name a pack font when the pack is off.
- **Collabora Online,** LibreOffice in the browser, for people who need every Word feature in the browser. It's heavy (about 1 GB of memory while editing), so it would be an optional add-on with its own decision.
