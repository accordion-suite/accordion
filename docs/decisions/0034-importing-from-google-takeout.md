# 34. Importing from Google Takeout

- **Status:** Accepted
- **Date:** November 17, 2026
- **Updated:** November 24, 2026, with what a real export showed (step 1)

## Context

The spec promises "full import from Google Takeout without data loss for mail, files, notes, contacts and calendars," and puts "Takeout import for files and notes" in the alpha. Its migration section says the importer "accepts a Google Takeout archive and maps every product to its Accordion module; nothing is lost silently — anything it cannot convert lands in an 'Unconverted' folder with a report." [Decision 31](0031-notes.md) makes Keep's import the last step of Notes, together with Takeout import for files. [Decision 1](0001-where-core-services-run.md) says Takeout import runs in a sandboxed process, as previews do, and [decision 6](0006-file-storage.md) says it shortens names longer than 255 bytes and lists each one in its report.

This decision covers the importer that every product shares, and its first two products: Keep, into Notes, and Drive's files, into Drive. Mail, Contacts, Calendar, and Photos each get their part when their module is built.

What Takeout gives people:

- **One export is one or more archives,** as `.zip` or `.tgz`. Each archive is 2 GB by default, and up to 50 GB if the person chooses. A big export splits into several parts, and one product's files can be spread across them.
- **Everything sits under `Takeout/`, one folder per product.** Folder names follow the account's language. German exports name the Photos folder `Google Fotos`, for example, so the importer can't rely on names.
- **Keep gives a `.json` and an `.html` file for each note,** with its pictures and recordings beside them, and a text file that lists the labels. Each JSON file has the title, the text or checklist items, the color, whether the note is pinned, archived, or in the trash, its labels, the times it was made and last edited (in microseconds), its attachments (file name and media type), its link previews, and its collaborators' email addresses. Newer exports add the text as HTML too, with Keep's bold, italics, and headings.
- **Drive gives the original files in their folder tree,** with Google Docs, Sheets, and Slides converted to Office files.

Some details aren't documented anywhere, and the community tools disagree about them. Step 1 settled them against real exports, as the next section says:

- Which audio format Keep's voice recordings come in. Sources say 3GP with AMR or AAC audio, and Chrome and Firefox can't play AMR.
- How Takeout names a note's files when two notes share a title, and whether attachments' extensions on disk match their JSON.
- Whether Keep's indented checklist items, reminders, and drawings appear in the JSON.
- Whether Drive's files keep their modified dates in the archive.

### What a real export showed

On November 24, 2026, the developer exported Keep and Drive from a test account twice, as two `.zip` parts and as three `.tgz` parts. The two exports held the same 21 notes and about 170 files. Part 1 of each was Keep, and Drive filled the rest, with one Drive folder split across two `.tgz` parts. The export itself stays out of the repos, since it has real email addresses in it.

The test account had no voice recording, drawing, link preview, repeating reminder, Google Doc, Sheet, or Slides file, two Drive files with one name, file shared with it, or shortcut, and only one color. The developer added them, and a second export the same day, as two `.zip` parts, covered them. A third, of Drive alone, covered a comment on a Doc.

**Keep:**

- **Voice recordings are AMR.** Each is raw AMR-NB (8 kHz, mono, about 12 kbit/s), starting with `#!AMR`, though it's named `.3gp` and its JSON says `audio/3gp`. Chrome and Firefox can't play it. The note's text held what looks like Keep's own transcript.
- **Drawings are flat PNG pictures,** listed as attachments like any picture, with no strokes to edit.
- **Link previews** are `annotations` with `"source": "WEBLINK"`, the `url`, its page's `title`, and a `description`, but no picture.
- **Reminders aren't exported,** repeating or not. A note with one gets only `"tasks": [{"id": "…"}]`, a link to Google Tasks with no time in it.
- **Colors** use Keep's own names: Coral is `RED`, Sand is `YELLOW`, Storm is `CERULEAN`, and Chalk is `GRAY`.
- **Indented checklist items come out flat,** in both the JSON and the HTML. Each item has only `text`, `textHtml`, and `isChecked`, and checked items come last, as Keep shows them.
- **Formatting is in each line's `style`, not in tags.** `textContentHtml` and each item's `textHtml` mark bold as `font-weight:700`, italics as `font-style:italic`, and underline as `text-decoration:underline`. Headings are `<h1>` and `<h2>`.
- **Background pictures aren't exported,** and nothing in the JSON says a note had one. Keep's Coral is `RED`.
- **File names follow the title, but aren't stable.** `:`, `/`, and `?` become `_`, a long title is cut to 50 characters, and an untitled note is named after when it was made, such as `2026-11-24T14_38_14.946+08_00.json`. Notes that share a title get `(1)`, and the two exports gave it to different notes. The JSON always has the full title.
- **Attachments** are named by a number, such as `1795501617103.1120585488.jpg`, and their extensions match the media type in the JSON.
- **Sharing:** `sharees` lists each person's email address, with `isOwner` true for the owner, so a note shared with the account names someone else as owner.

**Drive:**

- **Each file and folder has an `-info.json` beside it,** such as `Report.pdf-info.json`. It holds the full `title`, `created` and `content_last_modified` in UTC to the millisecond, `starred`, `description` (Drive filled one from a photo's camera data), `mime_type`, and `permissions` with each person's email address.
- **Google's own files come out as Office files:** a Doc as `.docx`, a Sheet as `.xlsx`, Slides as `.pptx`, and a Drawing as `.jpg`. Their info gives the Office type, and only `download_url` (such as `docs.google.com/document/…`) says they were Google's. A Doc's comments come out twice: inside the `.docx` as Word comments, anchored to their text, and as `<name>-comments.html` beside it, Google's own page listing each comment's author, date, the text it's on, its status, and its replies, with a script of Google's in it.
- **Two files with one name** come out as `Monthly budget.xlsx` and `Monthly budget(1).xlsx`, with infos named `Monthly budget-info.json` and `Monthly budget-info(1).json`, so the number comes after `-info`.
- **A shortcut comes out as a small `.html` bookmark file,** such as `BSOD3.jpg.html`, holding a link to the file on Google Drive. Its info says `application/vnd.google-apps.shortcut`. The file shared with the account isn't in the export at all.
- **The archive's own dates can't be trusted.** The `.zip` stores Google's local time (Pacific) with no time zone, while the `.tgz` stores UTC.
- **Kept versions come out as copies,** named `<name>-at-<time>-pinned.<ext>`, with no info file. All 73 matched their current file byte for byte. Takeout put some beside their file and others in a top-level `Unorganized` folder, and each export left out a different few.
- **Trash comes out as a top-level `Trash` folder.** `Trash` and `Unorganized` are the only folders with no info file of their own, which tells them apart from the person's folders in any language.
- **The `.tgz` cuts each name to about 100 bytes,** while the `.zip` kept names up to 404 bytes. Shortened, two different kept versions got the same path in one archive, and a shortened `-info.json` lost its `-info` and ends in plain `.json`. Its `title` still has the full name, and it starts the same as its file's shortened name.

## Decision drivers

1. **Nothing is lost silently,** as the spec says. What can't be converted is kept and named in a report.
2. **Untrusted input stays sandboxed** (decision 1). An archive is someone else's file format, and a crafted one can try to escape its folder, fill the disk, or exhaust memory.
3. **Big archives work on a Raspberry Pi,** whose disk may hold little more than the household's data.
4. **An import finishes unattended.** Closing the browser or restarting the server doesn't lose progress.
5. **Any account language works.**
6. **Running it again doesn't make copies,** such as when someone imports a newer export, or a second part of the same one.
7. **One developer.** It reuses what exists: Drive's uploads, the sandbox, and the way Notes saves notes and pictures.

## Options: where the archives come from

### Option 1: Uploads to a holding area outside Drive

An import page uploads the archives to a folder of their own on the server, and removes them after the import.

Pros:

- **The person's Drive stays tidy.**

Cons:

- **A second upload path,** with its own resuming, limits, and space accounting, beside Drive's.
- **Space it takes is hidden** from the person's storage numbers while it's there.

### Option 2: Archives are files in Drive

The import page lists `.zip` and `.tgz` files in the person's Drive to choose from. Uploading on the page puts them in a **Google Takeout** folder in My Drive, through Drive's own resumable uploads. The desktop app can sync them in too, which suits a 50 GB archive better than a browser tab.

Pros:

- **No new upload path.** Big uploads already resume, and the desktop app already syncs.
- **Space is honest.** The archive counts against the person's storage until they remove it.
- **The archive stays** until the person is satisfied, so an import can run again.

Cons:

- **The archive takes space twice:** the archive itself and what comes out of it. Importing one part at a time, and trashing each part after, keeps that to one part's size.

### Option 3: A folder on the server that an admin fills

An admin copies the archives to a folder on the server, and starts the import from the admin pages.

Pros:

- **Fastest** for very large exports, over the network or from a USB drive.

Cons:

- **Family members can't do it themselves,** and admins would handle other people's private data.

## Options: how the archives are read

### Option A: The server reads them

The server opens each archive with Rust's zip and tar readers, which are memory-safe, and stores what it finds.

Pros:

- **Least work.**

Cons:

- **Breaks decision 1.** A memory-safe reader still runs in the server's process, with access to every file and key, and parses Keep's HTML and JSON there too. A mistake in path handling writes outside its folder.

### Option B: A sandboxed copy of the server reads them

The server runs a copy of itself in the worker service (decision 29), as it already does for Office files' text. The copy reads one archive on its standard input, unpacks the files a product needs into its job folder, and turns each product's own formats, such as Keep's JSON and HTML, into a plain list of items: one line of JSON each, with set fields and lengths. The server reads that list, checks every field again, and stores the items through each module.

Pros:

- **Keeps decision 1.** Reading the archive, its names, and Keep's HTML all happen with no network and no access to the data folder.
- **Reuses the worker service,** its job folders, and its limits on memory, time, and output.
- **The server only handles plain, checked items,** so the code that stores them is the same code that stores notes and files today.

Cons:

- **More machinery:** an import job that remembers its place, and a list format between the copy and the server.
- **Disk for unpacking.** The copy unpacks in batches, up to 1 GB at a time, and the server moves each batch into storage before the next.

### Option C: The browser reads them

The web app opens the archive on the person's computer, and sends each note and file through the APIs it already uses.

Pros:

- **The server never parses an archive,** and never stores one.

Cons:

- **The tab has to stay open** for the whole import, which takes hours for a big Drive export.
- **A phone can't do it,** and a Pi's slow upload is no faster.
- **Later products, such as mail,** need the server's sandbox anyway, so this adds a second way to import rather than replacing one.

## Comparison

| | 1: Holding area | 2: Files in Drive | 3: Admin's folder |
|---|---|---|---|
| Family members can import | Yes | Yes | No |
| New upload path | Yes | No | No |
| Space shows in storage | No | Yes | No |
| Desktop app can bring the archive | No | Yes | No |

| | A: The server | B: Sandboxed copy | C: The browser |
|---|---|---|---|
| Decision 1 kept | No | Yes | Yes |
| Finishes unattended | Yes | Yes | No |
| Works for later products, such as mail | Yes | Yes | No |
| Work | Least | Most | Middle |

## Recommendation

**Option 2 with option B.** Archives are files in Drive, and a sandboxed copy of the server reads them. Option 1 adds a second upload path for nothing, and option 3 shuts out family members. Option A breaks decision 1, and option C can't run unattended or carry later products.

### The import page

Each person imports their own export, from **Settings** > **Move from Google**. Notes' and Drive's empty pages link there too.

1. The page explains how to make an export, with a link to Takeout, and suggests `.zip` parts of 2 GB or 10 GB.
2. The person uploads the archives or chooses ones already in Drive. Uploads go to **Google Takeout** in My Drive.
3. The server lists what's inside, without unpacking it, and the page shows each product it found, with its size:
   - Products Accordion converts, such as **Keep → Notes** and **Drive → Drive**, are selected.
   - Products Accordion can't convert yet, such as Gmail or Calendar, say so, and can be kept as files in **Unconverted**. They're not selected, so a 20 GB mailbox doesn't fill the disk unless the person asks.
4. The page checks that the person's storage has room for what's selected, and says how much is missing if it doesn't.
5. The import runs in the background. The page shows its progress, and the bell says when it's done.
6. The page links to the report, and offers to move the archives to the trash.

### Finding products

The importer recognizes each product by what's in its files, not by folder names, so any account language works. A Keep note is a `.json` file with Keep's fields, such as `createdTimestampUsec` and either `textContent` or `listContent`. Drive's folder is the product folder whose files have `-info.json` files beside them with Drive's fields, such as `drive_id` and `content_last_modified`. Inside it, a top-level folder with no info file of its own is Takeout's: the one holding only `-pinned` copies is `Unorganized`, and the other is the trash. Archives from one export are read together, so a note's pictures in another part still reach it. A part that's missing shows in the report.

### Reading safely

- **Names are checked before anything is written.** An entry whose path leaves its folder, or names a link or a device, is skipped and reported.
- **Entries are read one by one, never unpacked by name,** since a `.tgz` can hold two entries with one shortened path. Each entry gets a name of its own in the job folder, and the list carries its path from the archive.
- **Limits:** each item's text, each list's length, the number of entries, and the total unpacked size, which can't exceed what the archive's listing claimed or the room in the person's storage.
- **The server checks everything again.** The list from the copy is untrusted too.

### Keep into Notes

Each note becomes a Markdown file in the person's Notes place, as decision 31 describes:

- **Title** is the file's name, taken from the JSON's `title`, never from the archive's file name, which is cut short and has `_` in place of some characters. An untitled note is named after its first words, as Notes does today. A name that's taken gets a number.
- **Text** is the note's body. Where the export has HTML, headings and the bold and italics in each line's style become Markdown. Underline has no Markdown, so it's kept as plain text.
- **A checklist** becomes a task list, with each item checked or not, in the export's order. Keep doesn't export indents, so every item is at the top level.
- **Color** keeps Keep's name, in lowercase, and Keep's `CERULEAN` becomes Notes' `dark-blue`. Background pictures aren't exported, so they can't come over.
- **Reminders** aren't exported either. The report lists each note that had one, so the person can set it again in Notes.
- **Pinned, archived, and labels** go to the front matter as decision 31 sets them.
- **The dates the note was made and last edited** stay, so notes keep Keep's order. Core's storage gains a way to save a file with its original dates.
- **Pictures and drawings** go beside the note, named after it, as pictures added in Notes do.
- **Voice recordings** play only if browsers can. Keep's are AMR, which they can't, so each is converted to Opus in the sandbox with the ffmpeg Accordion already uses, and saved beside the note as a memo. The importer goes by the file's first bytes, not its `.3gp` name. The original goes to **Unconverted**. Keep puts its own transcript in the note's text, so that comes over too.
- **Link previews** are added at the end of the note as links titled with the page's title, unless the text already has the address.
- **Notes in Keep's trash** go to Notes' trash, where they're kept for Accordion's usual 30 days.
- **Collaborators** are listed in the report with their email addresses. Accordion doesn't know who they are, so nothing is shared automatically. When two people in the household import the same shared note, each gets their own copy, and the report says which notes were shared.

### Drive's files into Drive

- **Files keep their folder tree** under a **From Google Drive** folder in My Drive, so they don't mix with what's already there.
- **Each file's info gives its name, dates, and star.** The name is the info's `title`, the modified date its `content_last_modified`, and the made date its `created`. Stars come over. Descriptions and who the file was shared with go in the report, since Drive has no place for them. Info files themselves aren't stored. A file with no info keeps its name from the archive and the date the `.tgz` gives, and the report lists it.
- **Shortened `.tgz` names are matched back** to their info by the start of the name, then the info's full title is used. An info numbered by Takeout, such as `Monthly budget-info(1).json`, belongs to the file with the same number, `Monthly budget(1).xlsx`. Two entries that still can't be told apart both come over, numbered, and the report says so.
- **Kept versions** (`-at-<time>-pinned`) that match their current file are dropped, wherever Takeout put them. One that differs is stored as an older version of its file, with the time from its name. One whose file isn't in the export goes to **Unconverted**.
- **Drive's trash** goes to the person's trash in Accordion, kept for the usual 30 days, as Keep's trashed notes do.
- **Names longer than 255 bytes are shortened,** as decision 6 says, and listed in the report. The `.zip` export had names up to 404 bytes.
- **Office files converted from Google Docs, Sheets, and Slides** are stored as they are, under the name in their info. A Doc can then be opened with **Open with Docs** (decision 30). The report lists which files were Google's, by their info's `download_url`.
- **Files shared with the person** aren't in Takeout, and the report's last section says so, with how to download them from Google.
- **Comments** stay in the Office file, where Word and LibreOffice show them. The `-comments.html` beside a `.docx` that holds the comments isn't stored, since it repeats them and carries Google's script. One beside any other file goes to **Unconverted**, since only a Doc's comments were seen in the Office file. The report lists each file that had comments. Open with Docs leaves comments out, and says so, until Docs has comments of its own.
- **Shortcuts** aren't stored as files. The report lists each one with its link on Google Drive, beside the shared files, since a shortcut's file is usually one of them.

### The report and Unconverted

Each import writes a report as a Markdown file in **Google Takeout** in My Drive, named with its date. It lists:
- how many items each product brought
- each item that wasn't converted, and why
- each name that was shortened
- each Keep note that was shared, and with whom
- each item skipped because an earlier import brought it

Files that weren't converted go to **Google Takeout/Unconverted**, in their folder from the archive.

### Running again

Core keeps a table of what each import brought, by product and by a key from the archive: a Keep note's creation time and title, or a Drive file's `drive_id` from its info. Not a note's file name, since Takeout numbered two same-titled notes differently in two exports. A Drive file with no info uses its path, size, and date. A later import skips what's already there and says so, even from a newer export. An import that stops, because the server restarted or the person canceled, picks up from the last item it stored.

### Who does what

- **Core** owns the import job, the table of what's imported, the sandboxed reader's command, the report, and storage's dated saves.
- **Each module** says which products it converts. It gives the reader a function that turns its entries into items, and the server a function that stores them. Notes does Keep, and Drive does Drive's files. The module interface gains this as an optional part, so modules without an import don't change.
- **The web app** has the import page.

### Testing

- **Archives made for the tests,** shaped like the real export from step 1, in `.zip` and `.tgz`, split across parts, and in another language. They use made-up names and email addresses. They include `.tgz` names cut at 100 bytes with two entries on one path, an `-info.json` cut to `.json`, kept versions beside their files and in `Unorganized`, `.zip` dates in Pacific time, a trash folder, two notes with one title numbered both ways, an AMR recording named `.3gp`, two Drive files with one name and their numbered infos, a shortcut's bookmark file, and a Doc with comments and its `-comments.html`.
- **Hostile archives:** paths that leave the folder, links, a zip bomb, huge JSON, names over 255 bytes, and lists longer than the limits. Each is refused or skipped and reported, and nothing is written outside the job's folder.
- **Keep:** every field above, a note in each color, untitled notes, two notes with one title, a note with pictures in another part, and a voice recording that's converted.
- **Running again** brings nothing twice, and an import stopped halfway finishes after a restart.
- **Space:** an import larger than the person's storage is refused before it starts.
- **On the Pi:** a 2 GB part with a few thousand notes and files, for time and memory.

## Steps

1. **A real export.** The developer makes a Takeout of Keep and Drive from a Google account with a few test notes: colors, a checklist, labels, a picture, a drawing, a voice recording, a reminder, a shared note, and two notes with one title. Accordion settles the open details above against it and shapes the test archives after it. The export itself stays out of the repos. Done November 24, with three exports (see "What a real export showed").
2. **Core:** dated saves in storage, the import job and its table, the sandboxed reader, the module interface's import part, and the report.
3. **Notes:** Keep.
4. **Drive:** its files.
5. **The web app:** the import page.

## Later

- **Photos,** into Drive's Photos folder, with capture dates and places from Takeout's JSON files, as the spec describes. It waits for Photos.
- **Mail, Contacts, and Calendar,** each with its module.
- **Sharing Keep's shared notes again,** by matching collaborators' email addresses to people in the household, once people have email addresses in Accordion.
- **An admin importing for a family member** who can't do it themselves.

## Revisit this decision if

- Google changes Takeout's formats, or adds an API that moves data directly.
- Unpacking in the sandbox is too slow on a Pi to finish a 2 GB part in an evening.
- Most people would rather import from a phone.

## Consequences

- People move their notes and files from Google on their own, from one page, and see what didn't come over.
- An archive takes space in Drive until the person removes it.
- Core grows an import job and a module hook that Mail, Contacts, Calendar, and Photos reuse.
- Storage can save a file with its original dates, which sync and WebDAV may use later.
- Shared Keep notes arrive unshared, and household members who shared a note each get their own copy.
