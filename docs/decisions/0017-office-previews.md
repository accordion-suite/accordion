# 17. Office previews

- **Status:** Accepted
- **Date:** November 2, 2026

## Context

Drive shows thumbnails of photos, PDFs, text files, and videos ([decisions 14](0014-thumbnails-and-sandboxed-workers.md) and [15](0015-video-thumbnails.md)), but Word, Excel, and PowerPoint files still show an icon, and the viewer can only offer to download them. Google Drive shows each document's first page in the grid and opens it in its viewer. The spec asks for both: in-browser previews of Office files, made by headless LibreOffice in the sandboxed workers. Decision 14 left Office files for their own decision, in the same sandbox.

The documents families have:

- **Microsoft Office** saves `.docx`, `.xlsx`, and `.pptx`. Older copies saved `.doc`, `.xls`, and `.ppt`, which families still keep from years ago.
- **Google Takeout** exports Google Docs, Sheets, and Slides as `.docx`, `.xlsx`, and `.pptx`.
- **LibreOffice** saves `.odt`, `.ods`, and `.odp`.
- **Anything** can save `.rtf`.

Office files are a larger attack surface than photos. They can hold macros, links to other files and web addresses, and embedded objects, and the programs that read them are large C++ code bases with a history of security fixes.

## Decision drivers

1. **A compromised converter reaches nothing.** The same as decision 14: no data folder, no network, and no other programs.
2. **It looks like the document,** with its fonts, layout, and pictures, as in Google Drive.
3. **Only the formats families have.**
4. **Fast and small enough for a Raspberry Pi.**
5. **Optional.** An admin with no Office files, or no space, should be able to leave it out.
6. **Security fixes come with system updates,** as for libvips and ffmpeg.

## Options

### Option A: LibreOffice from the distribution, in the sandbox

The worker runs LibreOffice's own program, `soffice.bin`, through `accordion sandbox`, and converts each document to PDF once. The PDF's first page becomes the thumbnail, the way a PDF's does now, and the viewer shows the PDF.

Pros:

- **It looks like the document.** LibreOffice lays out pages as Office does, and with the fonts below, a Calibri or Arial document keeps its line breaks.
- **Every format above,** including the old ones.
- **One conversion does three jobs:** the thumbnail, the viewer's preview, and later, the text that search reads from Office files.
- **Fixes come with system updates.**

Cons:

- **A large install:** about 348 MB in 66 packages on Ubuntu, measured, with the fonts.
- **C++ code,** so everything rests on the sandbox, as with libvips and ffmpeg.
- **The sandbox needs more for it than for other workers:** a scratch folder, and a socket of its own. These are measured below.
- **150–220 MB of memory** for each document.

### Option B: Previews from the document's text

A Rust worker opens the document's ZIP file and draws its first lines, as text files show now. Spreadsheets show their first cells, and presentations their slide titles.

Pros:

- **Nothing to install,** and memory-safe code.
- **Fast,** a few milliseconds a document.

Cons:

- **It doesn't look like the document.** No fonts, layout, pictures, or slide designs.
- **No preview in the viewer** that looks like the file.
- **No old formats.** `.doc`, `.xls`, and `.ppt` aren't ZIP files.
- **A lot of Accordion's own code** to keep working with each Office version.

### Option C: The preview picture inside the file

LibreOffice saves a small picture of the first page inside every `.odt`, `.ods`, and `.odp`. Office saves one only when asked to.

Pros:

- **Tiny,** and libvips already reads the picture.

Cons:

- **Most `.docx` and `.xlsx` files have none.** None of the five test files below had one.
- **No preview in the viewer.**

### Option D: No Office previews

Office files keep their icons, as now.

Pros:

- **Nothing new to install or attack.**

Cons:

- **Drive looks unfinished next to Google Drive,** for the files families open most after photos.

## Comparison

| Driver | A: LibreOffice | B: Text | C: Built-in picture | D: None |
|---|---|---|---|---|
| Looks like the document | Yes | No | Sometimes | No |
| Viewer preview | Yes | No | No | No |
| Old formats | Yes | No | No | No |
| Install size | About 348 MB | None | None | None |
| Optional | Yes | Not needed | Not needed | Not needed |
| Code an attacker can aim at | Large, in the sandbox | Small, memory-safe | libvips, already there | None |

## Recommendation

**LibreOffice from the distribution (option A), in the sandbox from decision 14, as an optional package.**

### Converting a document

- **One conversion for each version of a document:** the first 50 pages to PDF. Page 1 becomes the thumbnail through `pdftoppm` and libvips, in their own sandbox run, as a PDF's does now. Core keeps the PDF beside the thumbnail, in the thumbnail cache, so it doesn't count toward anyone's storage. Backups leave the cache out, so after a restore the worker makes both again, as it does for other thumbnails. Drive's lists can then tell which documents have a preview from the cache, without asking the database for each file.
- **At most 50 pages,** so a huge spreadsheet can't make a huge PDF. The whole of a 50,000-row spreadsheet took 7.4 seconds and 536 MB, and made a 67 MB PDF. Its first 50 pages took 1 second and 200 MB, and made 172 KB.
- **One document at a time,** so two large ones can't use the Pi's memory together.

### Only the formats families have

- **Accordion picks LibreOffice's reader from the file's extension** and tells LibreOffice to use only that one. A file named `.docx` is only ever read as Word, so a disguised file can't reach LibreOffice's other readers.

  | Extensions | Reader |
  |---|---|
  | `.docx` | Word 2007 and later |
  | `.doc` | Word 97–2003 |
  | `.odt` | OpenDocument text |
  | `.rtf` | Rich Text |
  | `.xlsx` | Excel 2007 and later |
  | `.xls` | Excel 97–2003 |
  | `.ods` | OpenDocument spreadsheet |
  | `.pptx` | PowerPoint 2007 and later |
  | `.ppt` | PowerPoint 97–2003 |
  | `.odp` | OpenDocument presentation |

- **Apple's Pages, Numbers, and Keynote are left out.** LibreOffice reads only some of them, and badly.
- **`.csv` files keep their text thumbnails** from decision 14.

### Macros and links

- **Macros never run.** Accordion's LibreOffice profile sets macro security to its highest level and turns macros off. A conversion isn't meant to run macros anyway, but the setting doesn't rely on that.
- **Links are never updated.** A document can link to other files or web addresses, and LibreOffice is told never to fetch them. The sandbox blocks the network as well, and allows reading only the one file.

### The sandbox

LibreOffice was tried in the sandbox, with each kind of access taken away in turn, to find what it needs:

| Access | Why |
|---|---|
| Read the system's folders | Programs and libraries, as for every worker |
| Read `/etc/libreoffice` and `/etc/fonts` | LibreOffice's settings, which Debian keeps there, and the list of fonts |
| Read `/dev/urandom` | Random numbers. LibreOffice stops without them |
| Read `/etc/passwd` | The list of accounts, which has no passwords. The service can't make memory both writable and runnable, so LibreOffice maps a file in its home folder twice instead, and it finds its home folder through this list. Without it, it tries `/tmp` and stops |
| Read the one document | By its path, as for videos |
| Write a new, empty scratch folder | Its profile and the PDF. The folder is deleted afterward, as decision 14 planned. The worker can't make links there, or run what it writes |
| Make a local socket in `/tmp` | LibreOffice won't start without a socket of its own, to check whether another copy is running. Its folder is always `/tmp` or `/var/tmp`, whatever `TMPDIR` says |

- **The socket can't reach anything.** The system call filter allows only local stream sockets, and only to listen, not to connect. A datagram socket could send to another program without connecting, so it's refused. LibreOffice opens its own socket, finds no other copy, and carries on. It can't connect to any other program on the computer, local or not.
- **The server removes the socket afterward.** The sandbox lets LibreOffice make files in `/tmp` but not remove them, so the server removes the sockets that appeared during the run.
- **`/tmp` is Accordion's own.** The service runs with systemd's `PrivateTmp`, so the worker can't see or touch other programs' temporary files. In tests and development, `/tmp` is shared, and the worker can make sockets there but not read or change files.
- **A ready-made profile.** On a new profile, LibreOffice sets it up and exits, asking to be started again. Accordion has LibreOffice make a profile in the sandbox the first time a document needs one after the server starts, adds the settings above, and gives each document a fresh copy in its scratch folder.
- **It works under the service's own limits.** It runs under systemd's `MemoryDenyWriteExecute` and system call filter, which every worker also runs under. The first upgrade test found that the memory rule hadn't been in force when LibreOffice was tried, so the sandbox launcher now turns it on itself, and tests run every worker the way the service does.

### Limits

- **1 GB of writable memory, 60 seconds of CPU time, and 90 seconds in all.** Larger than for photos and videos, for long documents on a Pi.
- **Documents up to 100 MB.** LibreOffice reads the whole file. Larger ones keep their icon.
- **Measured in the sandbox,** on an x86_64 computer, with LibreOffice 26.2 and a fresh copy of the profile each time:

  | Document | Time | Peak memory |
  |---|---|---|
  | Two-page `.docx` letter with a table | 0.5 s | 195 MB |
  | 30-chapter `.docx` report | 0.8 s | 223 MB |
  | 12-row `.xlsx` budget | 0.4 s | 161 MB |
  | 50,000-row `.xlsx` ledger, first 50 pages | 1.0 s | 200 MB |
  | Three-slide `.pptx` | 0.5 s | 151 MB |

### In Drive

- **The grid shows the first page,** as for a PDF.
- **The viewer shows the PDF,** with a note that it's a preview and **Download** to get the file itself. When the document is longer than 50 pages, the note says only the first 50 are shown.
- **Before the preview is made,** or if it can't be, the viewer shows the page it shows now, with the icon and **Download**.
- **The Thumbnails setting** says Office files show their first page.

### Without LibreOffice

- **The package recommends LibreOffice rather than depending on it,** as for ffmpeg. apt installs recommended packages by default, so the install command brings it in. An admin who doesn't want it can remove it, and Accordion stays installed.
- **Office files then keep their icons,** and **Server health** says so without a warning.
- **Installing LibreOffice later** makes the worker try the documents again within an hour, without a restart.

### Testing

- CI installs LibreOffice. Tests make documents with LibreOffice itself and check:
  - that each format gets a thumbnail and a preview
  - that a long spreadsheet stops at 50 pages
  - that a file with the wrong extension fails cleanly: an OpenDocument file named `.docx` fails in Word's reader, and a web page named `.docx` never reaches LibreOffice, since it doesn't start like a Word file
- **No test for macros.** LibreOffice didn't run a document's on-opening macro during a conversion even with macro security at its lowest, outside the sandbox, so a test can't show that Accordion's settings are what stop it. The settings stay as a second guard.
- The sandbox tests check that the worker can't connect to a local socket, besides decision 14's checks.
- The upgrade test checks that a `.docx` gets a thumbnail and a preview. It installs LibreOffice before updating, as an admin would from the release notes.

### Revisit this decision if

- A Raspberry Pi takes more than a few seconds for an ordinary document.
- LibreOffice gets a way to run without its socket, so the sandbox can take sockets away again.
- Families ask for Apple's formats.

## Consequences

- **New recommended packages:** `libreoffice-writer-nogui`, `libreoffice-calc-nogui`, `libreoffice-impress-nogui`, `fonts-crosextra-carlito`, `fonts-crosextra-caladea`, and `fonts-liberation2`.
- **The sandbox launcher** gains a scratch folder a worker can write to, and a switch for local sockets that only the LibreOffice worker uses.
- **Core's thumbnail service** learns Office formats, keeps each document's PDF, and raises its worker version, so documents it skipped get another try.
- **Drive's API** serves the PDF from `/items/{id}/preview`, and the web app's viewer shows it.
- **Server health** says when Office files have no previews because LibreOffice isn't installed.
- **CI installs LibreOffice** for core's tests, which makes them slower to set up.
- **The next release's notes say `Installs itself: no`,** as for ffmpeg, since the update helper doesn't install new packages.

## Open questions

- **Speed on a Raspberry Pi.** Measure there, with the same documents.
- **Debian 12's LibreOffice** is version 7.4. Check that it takes the page range in the same form, and runs in the sandbox with the same access.
- **Real documents.** The measurements above used documents LibreOffice made. Check documents from Word, Excel, PowerPoint, and Google Takeout.
