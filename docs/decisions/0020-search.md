# 20. Search

- **Status:** Accepted
- **Date:** November 3, 2026

## Context

Drive's search only looks at names. `GET /api/drive/search` reads every item the person can open and keeps the ones whose names contain the words, ignoring case. A letter about the mortgage is found only if "mortgage" is in its name.

The search page has a row of filter chips, as Google Drive does: **Type**, **People**, **Modified**, and **Source**. The server applies **Type** and **Modified**. The web app applies the other two to the results it gets back, and the server returns at most 200 results. So a filter on **People** can miss matches beyond the first 200.

The spec asks for more:

- **Full-text search,** including the text inside PDFs and Office files, and on-box OCR for scans later.
- **First results in under 500 ms** across 100,000 emails and 50,000 files, on the family hardware tier.
- **One search index** that Drive, Mail, and Notes share. The spec's stack table names Tantivy for it.

Some pieces exist already:

- **Workers in a sandbox** ([decision 14](0014-thumbnails-and-sandboxed-workers.md)). Files are untrusted, so reading them runs in a separate, locked-down process ([decision 1](0001-where-core-services-run.md)). `pdftotext` comes with `poppler-utils`, which the package already installs for PDF thumbnails. LibreOffice is a recommended package for Office previews ([decision 17](0017-office-previews.md)).
- **Work kept by content hash.** Thumbnails and previews are made once for each file's contents ([decision 6](0006-file-storage.md)), in a cache folder that backups leave out.
- **SQLite with FTS5.** SQLite's full-text search is built into the copy of SQLite that's compiled into the server.

## Decision drivers

1. **Nothing leaks.** Results, and the text shown from them, come only from items the person can open now.
2. **Untrusted files are read in the sandbox.**
3. **Fast enough on a Raspberry Pi:** first results in under 500 ms, as the spec asks.
4. **Small on a Raspberry Pi,** in memory most of all, since the thumbnail and Office workers already need a lot of it.
5. **Never wrong for long.** A renamed, moved, deleted, or unshared item drops out of results at once, even if the index is behind.
6. **Backups stay small.** An index can be made again from the files, so it doesn't need backing up.
7. **Filters without learning a syntax.** Family members choose filters from a form, as in Google Drive's **Advanced search**.
8. **One developer.**

## Options: where the index lives

### Option A: Tantivy

A search library in Rust, like a small Elasticsearch built into the program. The spec names it.

Pros:

- **Fastest for common words.** In the measurements below, a word in 70% of one person's documents took 1.4 ms, against 29 ms for SQLite.
- **Smaller when the text is kept,** since it compresses the text it stores.
- **More ways to match:** fuzzy matching for typing mistakes, and word stems in many languages.

Cons:

- **About 10 MB more in the server program,** which is 30 MB today, and more code to build in CI.
- **More memory.** Indexing used 107–155 MB in the measurements, against 16 MB for SQLite. Its writer can be held to about 15 MB, which makes indexing slower.
- **Each change is a commit of its own,** about 7–10 ms, and its files are merged in the background.
- **A second store beside the database.** It can't share SQLite's transactions, so access checks need either a copy of each item's owner and folders in the index, or a second step that checks each result in SQLite.
- **A new file format that can change** between Tantivy versions, which means rebuilding the index after some updates.

### Option B: SQLite FTS5 in the main database

A full-text table beside Drive's other tables.

Pros:

- **Nothing new to build or run.** FTS5 is already in the server.
- **Little memory:** 16 MB in the measurements, while indexing and while searching.
- **One query does it all:** matching words, checking access with the same SQL rules sharing uses, and filtering.

Cons:

- **Slower for common words:** 29 ms on the development computer, so perhaps 100–150 ms on a Raspberry Pi.
- **Makes the main database much larger.** The text of 150,000 documents added 722 MB, all of which every backup would copy.
- **Indexing takes the database's one writer** ([decision 2](0002-metadata-database.md)) for each file, which delays other changes a little.

### Option C: SQLite FTS5 in a separate file in the cache folder

The same as option B, in its own database file, `cache/search.db`, which the server opens beside the main one.

Pros:

- **Everything option B has,** except one transaction for both files.
- **Backups leave it out,** as they leave out thumbnails. After a restore, the server makes it again.
- **Indexing has its own writer,** so it never holds up other changes.

Cons:

- **Slower for common words,** as option B.
- **Changes to the two files can't commit together.** This doesn't matter if the index is keyed by content hash, as thumbnails are: an item's current contents decide what it matches, so text for contents no item has any more matches nothing.
- **Rebuilding after a restore takes time.** Searching names works at once, and searching inside files catches up as the worker goes through them.

## Options: how filters are chosen

### Option 1: A search options panel

A button at the end of the search box opens a panel with a field for each filter, as Google Drive's **Advanced search** does. The chips on the search page show what's chosen.

Pros:

- **Everything in one place,** with words for each choice. Nothing to remember.
- **Works on phones,** where typing special words is slow.

Cons:

- **More to build than chips alone.**

### Option 2: Search words, such as `type:pdf owner:me`

Google Drive and Gmail understand these in the search box.

Pros:

- **Quick for people who know them.**

Cons:

- **Nobody in a family knows them,** and a typing mistake searches for the wrong thing without saying so.
- **Every word needs translating** into each language Accordion's interface is in.

## Comparison

| Driver | A: Tantivy | B: FTS5 in the main database | C: FTS5 in its own file |
|---|---|---|---|
| Nothing leaks | Second check in SQLite | Checked in the same query | Checked in the same query |
| Fast enough | 0.4–2.3 ms | 0.8–29 ms | 0.8–29 ms |
| Memory | 107–155 MB indexing | 16 MB | 16 MB |
| Never wrong for long | Keyed by content | Same transaction | Keyed by content |
| Backups stay small | Left out | 722 MB more | Left out |
| Work for one developer | Medium | Small | Small |

## Measurements

On the development computer, an AMD Ryzen 5 3600 under WSL, with 150,000 made-up documents: about the spec's 100,000 emails and 50,000 files. They hold 57 million words in all, from 25 to 1,600 words each, with words as common or rare as in real writing. Each search looked only at one person's documents, one in 10, which stands in for the access check, and showed the 50 best matches with a line of text from each.

| | Tantivy 0.27 | FTS5, text kept | FTS5, text not kept |
|---|---|---|---|
| Time to index everything | 12 s | 31 s | 29 s |
| Size on disk | 401 MB | 722 MB | 186 MB |
| Peak memory | 107 MB indexing, 155 MB updating | 16 MB | 16 MB |
| Replacing one file's text | 7–10 ms | 0.2 ms | |

| Search | Matches | Tantivy | FTS5 |
|---|---|---|---|
| A word in 70% of documents | 10,574 | 1.4 ms | 29 ms |
| A word in 1.6% | 242 | 0.8 ms | 5.4 ms |
| A rare word | 21 | 1.1 ms | 0.8 ms |
| Two words | 20 | 1.5 ms | 1.4 ms |
| Two words, each as the start of a word | 45 | | 4.0 ms |
| A phrase in quotes | 149 | 0.4 ms | 2.7 ms |
| The start of a word, such as "budg" | 242 | 2.3 ms | 5.9 ms |

- **Keeping the text** is what lets results show the words in context. FTS5 keeps it uncompressed, so it's the larger.
- **The made-up words compress better than real text,** so Tantivy's real size would be larger. A family's real files, most of them photos with no text, would make a much smaller index than this test.
- **A Raspberry Pi 5 is slower.** If it's four times slower, FTS5's worst case is about 120 ms, well under the spec's 500 ms. These figures join the Raspberry Pi measurements already planned.

## Recommendation

**SQLite FTS5 in its own file in the cache folder (option C), keyed by content hash, with a search options panel (option 1).**

Every search was far under the spec's 500 ms with FTS5, in a seventh of Tantivy's memory, with nothing new in the server. Tantivy's speed would matter for millions of documents, which a family doesn't have. Tantivy is still the choice if a Raspberry Pi turns out too slow, and the index is a cache that can be thrown away and rebuilt, so changing later costs only the work of changing.

### What's searched

- **Names, as now,** for every item, from the main database. A rename is found at once.
- **The text inside files:**

  | Files | How the text is read |
  |---|---|
  | Text, Markdown, CSV, code, and other plain text | By the server itself, as UTF-8, the same check that text thumbnails use. Reading text needs no decoder, so it skips the sandbox |
  | PDFs | `pdftotext` in the sandbox |
  | Word, Excel, PowerPoint, and OpenDocument files (`.docx`, `.xlsx`, `.pptx`, `.odt`, `.ods`, `.odp`) | A sandboxed copy of the server, which reads the XML inside them. These are zip files of XML, so this needs no LibreOffice |
  | Older Office files and Rich Text (`.doc`, `.xls`, `.ppt`, `.rtf`) | `pdftotext` in the sandbox, from the preview LibreOffice makes for [decision 17](0017-office-previews.md), when it's installed. Only the preview's first 50 pages are searched |

- **Only the current version** of each file. Older versions aren't searched.
- **The first 1 MB of text in each file,** about 500 pages. Longer files are found by what's in that first 1 MB.
- **Not yet:** text in photos and scans. OCR with Tesseract follows as its own decision.

### Matching

- **Every word has to match,** in the name or the text. The last word also matches words that start with it, so results appear while someone is still typing "budg".
- **Case and accents are ignored,** so "resume" finds "Résumé".
- **Quotes match a phrase,** such as `"tax return"`.
- **Words are taken as words, never as commands.** The server quotes each one before giving it to FTS5, so typing `AND`, `NEAR`, or a stray `*` can't cause an error.
- **No word stems.** FTS5's stemmer is English only, and families write in many languages. Since the last word matches as a start, "invoice" still finds "invoices".

### Results

- **Best matches first,** using FTS5's own measure of how well text matches. Matches in names come before matches in text. The list's sort menu can still sort by name or date.
- **Matches in text show a line around the words,** with the words in bold, under the item's name.
- **Up to 200 results,** as now. Every filter is applied on the server before the limit, so none of them miss matches.

### Who sees what

- **A search returns only items the person can open now:** their own outside the trash, and items in what's shared with them, using the same rule as sharing and live updates.
- **The index never says who can open what.** It holds a content hash and its text. Each search finds the hashes that match, then the main database finds the current items with those contents that the person can open. A file that was deleted, moved out of a shared folder, or unshared drops out at once, whatever the index holds.
- **A line of text is shown only for an item the person can open,** who could read the whole file anyway.

### Keeping the index up to date

- **Text is read in the background,** one file at a time and at low priority, in the same way as thumbnails. An upload or a new version queues its contents. After the update that adds search, the server works through existing files, newest first.
- **Contents are read once,** however many files have them, since the index is keyed by content hash.
- **A file that fails isn't tried again** until the worker changes, as for thumbnails.
- **Text goes when the contents go,** as thumbnails do: when no file or older version has them. The hourly tidy-up removes it.
- **Limits for each file:** 512 MB of writable memory, 30 seconds of CPU time, 60 seconds in all, and 1 MB of text. Files over 200 MB keep their name search only. These are starting points, checked on the Raspberry Pi.
- **The search page doesn't update itself.** Live updates aren't sent when text is ready, since nothing on screen depends on it.

### The search options panel

A **Search options** button, with a sliders icon, sits at the right end of the search box. It opens a panel under the box:

| Field | Choices | Like Google Drive's |
|---|---|---|
| **Type** | Any, Folders, Documents, Spreadsheets, Presentations, PDFs, Photos and images, Videos, Audio, or Archives | **Type** |
| **Owner** | Anyone, Owned by me, Not owned by me, or someone on the server | **Owner** |
| **Has the words** | Words found inside the file | **Has the words** |
| **Item name** | Words in the name | **Item name** |
| **Location** | Anywhere, My Drive, Shared with me, or a folder chosen with **Choose a folder**, which includes the folders inside it | **Location** |
| | **In the trash** and **Starred** check boxes | **In trash** and **Starred** |
| **Date modified** | Any time, Today, Last 7 days, Last 30 days, This year, Last year, or between two dates | **Date modified** |
| **Shared with** | Someone on the server, to find what you shared with them | **Shared to** |

- **Three of Google's fields are left out.** **Approvals and eSignatures** and **Follow-ups** need features Accordion doesn't have. **Encrypted** waits for encrypted vaults, which the spec plans.
- **Shared with is a list of people,** not a box for email addresses, since Accordion shares only with people on the server.
- **Reset** clears the fields, and **Search** opens the search page with them. The words in the search box fill **Has the words**, so typing in the box still searches names and text together, as now.
- **The panel and the chips are the same filters.** The page's address holds them all, so a search can be bookmarked and **Back** undoes a change. The chips show the ones that are set, and opening the panel from the search page shows the ones in use.
- **The chips stay,** for changing one filter without opening the panel. **Source** becomes **Location**, and **People** becomes **Owner**.
- **In the trash** finds only the person's own items, since only they can restore them.
- **On a phone,** the panel fills the screen.

### The API

`GET /api/drive/search` takes every filter, so the server applies them all:

| Parameter | Meaning |
|---|---|
| `q` | Words in the name or the text |
| `name` | Words in the name only |
| `words` | Words in the text only |
| `type` | A type, as now |
| `owner` | `me`, `others`, or a username |
| `in` | `my-drive`, `shared`, or a folder's ID |
| `starred`, `trash` | `true` to find only starred items, or only items in the trash |
| `after`, `before` | Times, as now |
| `shared_with` | A username |

Each result says whether it matched by name or by text, with its line of text.

### Later modules

- **Core owns the index,** as it owns thumbnails, so Notes and Docs add their own texts through core's API. A module's texts are kept apart from Drive's, and each module checks access to its own results, as it does for its live update hints.
- **Mail's search** is decided with its mail engine. Stalwart, which the spec is evaluating, has a full-text index of its own.

### Testing

- **Core's tests** check that each kind of file's text is read, in the sandbox where it should be, and that a file that's too large or fails is skipped and not tried again.
- **Access tests** check that a search never returns, or shows text from, an item someone can't open: someone else's file, a file in a folder that's no longer shared, a deleted file, or someone else's trash. Drive's access test gains the new parameters.
- **A test types FTS5's special words and characters** and expects ordinary results, not errors.
- **The web app's tests** cover the panel, turning its fields into the address and back, and the chips.
- **The upgrade test** checks that a PDF and a `.docx` it uploads are found by words inside them.

### Revisit this decision if

- A Raspberry Pi takes more than 500 ms for a common word, with a family's real files.
- Families ask for matching with typing mistakes, which Tantivy does and FTS5 doesn't.
- Families search a lot in Chinese, Japanese, or Korean, which don't put spaces between words. FTS5's trigram tokenizer handles them, with a larger index.

## Consequences

- **Spec update:** change the stack table's search row from Tantivy to SQLite FTS5.
- **Core** gains a text service: a queue and failures in its own table, `cache/search.db`, the workers for each kind of file, and an API for modules to search.
- **The sandboxed copy of the server** gains a command that reads text from Office files.
- **Drive's search** takes every filter on the server, and its results say how each matched.
- **The web app** gains the **Search options** panel, the new chips, and lines of text in results. The search page stops filtering in the browser.
- **Backups** already leave out `cache/`, so nothing changes there.
- **Server health** gains a **Search** check: how many files are waiting, and whether older Office files are skipped because LibreOffice isn't installed.

## Open questions

- **Speed on a Raspberry Pi,** with the same test and with a real family's files.
- **The worker's limits,** with large real PDFs and spreadsheets.
- **Text that isn't UTF-8,** such as old text files saved in Windows-1252. They're skipped at first.
- **Suggestions while typing,** as Google Drive shows under its search box.
