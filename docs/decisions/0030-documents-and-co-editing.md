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
- **Named versions** are Drive versions with a name, which core adds to versions. Restoring one works as restoring any version does.

### Changes from outside Docs

A document's file can change without Docs: someone restores an older version, a computer syncs an edited copy back, or it's saved over WebDAV.

- **If the file's state already contains everything Docs has,** Docs adopts it. This covers a server restored from a backup.
- **Otherwise, Docs makes the document match the file's body.** It works out the difference and applies it as edits, so anyone in the document sees the change happen. Comments stay wherever their text survives. This is what makes restoring an old version work: merging an older state would bring nothing back.
- **The file's body always wins over its state,** since the body is what another program edited.

### Safety

- **The server writes the body from the schema.** Only the schema's elements and attributes reach it, never a script, a style, or an event attribute.
- **Imported HTML goes through the same schema,** so a hostile page loses everything that isn't document content.
- **The server reads updates with panics caught,** so a crafted update fails its own document's connection and nothing else. Fuzz tests of the update reader run in CI.
- **Limits:** 1 MB for each update (images upload separately), and 50 people in one document.

### Comments, mentions, and suggesting

- **Comment threads live in Docs' tables while a document is open,** and in the file's data block once it's saved, so they travel with it. Each thread is anchored by Yjs relative positions, which stay with the text as others edit around it.
- **Comments use ordinary requests,** not the document's WebSocket, so a commenter can comment without being able to edit. The Commenter level joins Viewer and Editor in sharing, as decision 16 planned.
- **@mentions** notify through the event log ([decision 18](0018-the-event-log.md)), and only people who can open the document can be mentioned.
- **Suggesting mode waits for Yjs 14,** whose attributions record who made each change. If yrs hasn't caught up by then, this decision is revisited.

### Import and export

- **Open with Docs** makes a new document from an HTML, Markdown, Word, or OpenDocument file, as Google Docs does. Word and OpenDocument go through LibreOffice in the sandboxed worker, to HTML, then through the schema.
- **Open with Docs warns before it converts** when the file has things the editor can't keep, such as headers and footers, footnotes, text boxes, Word's tracked changes, or fields like a table of contents. Old `.doc` files convert the same way.
- **Download as** HTML (the file itself), Word, OpenDocument, Markdown, or PDF. PDF comes from the browser's print, with the document's page setup in print CSS.
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

- **Whether Drive shows `.html` on a document's name.** Family members don't expect it, but the file synced to a computer needs it.
- **Templates and page setup:** where templates live, and which page sizes and margins to offer.
- **Version history inside Docs,** showing what changed between versions, which Yjs 14's change sets would make easier.
- **When to move to Yjs 14,** and whether yrs reads what it writes.
- **Where the co-editing state of a Word file edited in place lives,** since a `.docx` has no place for the data block. Docs' tables are the likely answer, which brings back option B's problem for those files only.
- **Collabora Online,** LibreOffice in the browser, for people who need every Word feature in the browser. It's heavy (about 1 GB of memory while editing), so it would be an optional add-on with its own decision.
