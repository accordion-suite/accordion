# Accordion

**Self-hosted Drive, Docs, Mail, and Notes on hardware you own. One install, one sign-in, one backup, zero telemetry.**

Accordion is a local-first replacement for Google Workspace. It gives a household or small team the convenience of Workspace (one sign-in, sync across devices, real-time collaboration, and search) without an account at a company that reads, profiles, or monetizes your data.

> [!WARNING]
> Accordion is in early development. Two test releases are out for the developer's own testing, but there's no release for households yet. Star or watch this repository to follow progress toward the first public alpha.

## Why Accordion

Leaving Google today means stitching together five or six self-hosted projects, each with its own sign-in, update cycle, and backup story. Most people give up and stay.

Accordion's bet is that **one install, one admin panel, and one backup that covers everything** makes leaving Google realistic for non-experts. The goal for 1.0: a technically curious person moves a family of five off Google in one weekend and never touches a terminal afterward.

Like the instrument, the suite folds out. Start with Drive and Notes on a Raspberry Pi, then add Mail, Calendar, and more as your needs grow. Every module shares one identity, one storage layer, and one search index.

## Modules

| Module | Replaces | Planned for | Where it stands | Open protocols and formats |
|---|---|---|---|---|
| **Accordion Drive** | Google Drive | 1.0 | In testing | WebDAV, plain files on disk |
| **Accordion Docs** | Google Docs | 1.0 | Being built | HTML files with comments; opens .docx, .odt, .rtf, .md, and web pages; .docx, .odt, .md, and .pdf downloads |
| **Accordion Mail** | Gmail | 1.0 | Planned | IMAP, SMTP, JMAP, Maildir, Sieve |
| **Accordion Notes** | Google Keep | 1.0 | Being built | Markdown files with YAML front matter |
| **Accordion Calendar** | Google Calendar | 1.5 | Planned | CalDAV, iCal |
| **Accordion Contacts** | Google Contacts | 1.5 | Planned | CardDAV, vCard |
| **Accordion Photos** | Google Photos | 2.0 | Planned | Original files with sidecar metadata |
| **Accordion Sheets** | Google Sheets | 2.0 | Planned | .xlsx, .ods, .csv |

Every module ships with the suite. The admin turns modules on or off in Settings, under **Modules**, without a restart, and users see only the ones that are on. A module that's off uses no memory, and its data stays on disk and in backups.

Each app keeps its files in a top folder of its own, which Drive shows as a tab below **My Drive**, such as **Notes** or **Docs**. They're ordinary files, so they sync to computers and open in other programs ([decision 32](docs/decisions/0032-a-place-for-each-apps-files.md)).

## Principles

- **Local-first -** Your data lives on your box. Apps keep an offline copy and sync when they can reach it.
- **Zero telemetry -** No analytics, crash reporting, or third-party fonts and CDNs. Accordion connects only to your DNS provider, Let's Encrypt, and your backup locations, plus GitHub once a day to check for updates, and once to download the speech model if an admin turns on voice memo transcripts. With remote access or link sharing on, it also asks your router to forward their ports. The admin **Network** page lists each connection and why, and turns off the ones that can be.
- **Open formats -** Files stay as files, mail is Maildir, notes are Markdown, and contacts and calendars are vCard and iCal. Leaving Accordion should be as easy as joining.
- **Standards over lock-in -** Accordion speaks WebDAV, IMAP, SMTP, JMAP, CalDAV, and CardDAV, so the apps you already use, such as Thunderbird, iOS Mail, and DAVx5, keep working.
- **Boring to run -** Automatic updates on a schedule you choose, automatic encrypted backups, and a health page anyone can read.

## How it works

Everything runs on one machine you own. A single gateway handles TLS, remote access over WireGuard, and routing. Behind it, one server program runs every module, and the code that handles untrusted files and incoming mail runs in separate, locked-down processes. All modules share one identity provider, one search index, one sync engine, one co-editing service, and one storage layer, so adding a module never adds a new sign-in, database, or backup.

You reach your server from:

- **Any browser**, through one web app with an app switcher, installable as a PWA
- **The desktop sync app** for Windows, in early testing, with macOS planned. On Linux, a command-line client syncs a plain folder.
- **Mobile apps** for iOS and Android (planned)
- **Standard clients** over WebDAV today, and IMAP, CalDAV, and CardDAV with their modules

Away from home, Accordion's apps connect through the WireGuard server built into Accordion, so nobody has to install anything else. The Windows app already carries its own tunnel. Remote access needs one UDP port, which Accordion asks your router to forward, and it reaches Accordion and nothing else on your network.

Accordion connects directly, with no relay in between. If your internet provider shares one public address among many homes (carrier-grade NAT), remote access and link sharing can't work from outside, and **Server health** explains what to ask your provider for ([decision 28](docs/decisions/0028-carrier-grade-nat.md)).

## Installing

There's no release for households yet. The test releases are Debian packages for x86_64 and ARM64, which run on Debian 12, Ubuntu 24.04, Raspberry Pi OS, and later versions. [Packaging](packaging/README.md) explains what they install.

The planned install options are:

1. **Appliance image -** Flash it to a Raspberry Pi 5 or an x86_64 machine, then finish setup in a browser.
2. **Docker Compose -** One file for existing Linux servers, also compatible with Podman.
3. **App stores -** An app for umbrelOS ([decision 27](docs/decisions/0027-running-on-umbrelos.md)), then packages for TrueNAS SCALE, Unraid, and Synology.
4. **Linux packages -** .deb packages with systemd units, as the test releases use, then .rpm packages.

| Tier | Users | CPU | RAM | Storage |
|---|---|---|---|---|
| Minimum | 1–3 | 4-core ARM64 or x86_64 | 4 GB | 64 GB SSD plus 1 data drive |
| Family | Up to 10 | Raspberry Pi 5 (8 GB) or 4-core x86_64 | 8 GB | 256 GB NVMe plus 2 mirrored drives |
| Small team | Up to 25 | 6-core or better x86_64 | 16 GB | 512 GB NVMe plus 2 or more mirrored drives |

These are design targets. Benchmarking will confirm them.

## Privacy and security

Accordion trusts the server owner and no one else: not the network, not your internet provider, not someone who steals the disk, and not Accordion's own developers.

- TLS everywhere (TLS 1.3, or 1.2 for older clients such as File Explorer on Windows), and WireGuard for remote access
- Backups encrypted before they leave the box
- One sign-in for every module, and app passwords for standard clients
- Programs that read people's files, such as thumbnail makers, run as a user of their own, with no network, seeing only the files for their job ([decision 29](docs/decisions/0029-sandboxing-workers-without-landlock.md))
- A CI check that fails the build if any component makes an unlisted outbound connection
- Planned: passkeys and TOTP, full-disk encryption set up by the appliance image, and optional end-to-end encrypted vaults for data that even your server can't read

## Moving from Google

Accordion imports a Google Takeout archive under **Settings** > **Import from Google** and maps each product to its module. Keep's notes go to Notes, with their colors, labels, checklists, pictures, and voice recordings, and Drive's files go to Accordion Drive, with their dates, stars, and older versions. Gmail, Contacts, Calendar, and Photos will go to their own modules once those are built. Until then, and for anything else Accordion can't convert, the files go into an **Unconverted** folder, and a report says what happened to each one, so nothing is lost silently. Running the import again skips what it already brought. [Decision 34](docs/decisions/0034-importing-from-google-takeout.md) explains how it works.

## Roadmap

| Phase | Focus |
|---|---|
| **Foundation** | Gateway, identity, storage, installer, setup wizard, encrypted backups, and health page |
| **Alpha** | Drive with desktop sync, Notes, and Takeout import for files and notes |
| **Beta** | Docs with real-time co-editing, Mail with guided DNS setup, and iOS and Android apps |
| **1.0** | Appliance images, NAS packages, accessibility (WCAG 2.2 AA), and five languages |
| **1.5** | Calendar, Contacts, and optional on-box AI for search and summaries |
| **2.0** | Photos, Sheets editing, and federation between Accordion servers |

Each phase starts only when the previous one meets its quality gate. If time runs short, features move to the next phase. The gate is never skipped.

## Repositories

This repository holds the suite: the server, installer, setup wizard, packaging, and documentation. Each module lives in its own repository and has its own version number. The server is one program built from a set of module versions that were tested together, and that's the only combination your server will install.

| Repository | Contents |
|---|---|
| `accordion` | Server, installer, setup wizard, packaging, and docs (this repo) |
| `accordion-core` | Shared identity, search, sync, jobs, and storage |
| `accordion-gateway` | TLS, WireGuard, and reverse proxy |
| `accordion-drive` | Drive |
| `accordion-docs` | Docs |
| `accordion-notes` | Notes |
| `accordion-web` | Web app, with every module's pages |
| `accordion-desktop` | Desktop sync app |
| `accordion-umbrel` | The umbrelOS app store |
| `accordion-mail` | Mail (planned) |
| `accordion-android`, `accordion-ios` | Mobile apps (planned) |

Please open issues here, whichever module they're about.

## Building from source

You can build the server and try it. You need [Rust](https://rustup.rs) and bash (on Windows, use WSL).

```sh
scripts/clone-workspace.sh                         # Clone the other Accordion repos next to this one
cargo run -- create-owner <username> "<Your name>"  # Create the owner account (asks for a password)
cargo run                                          # Start the server on http://127.0.0.1:8080
```

`cargo run -- --help` lists every command and setting. To run the server as a system service, see [packaging](packaging/README.md).

Drive, Docs, Notes, and Settings open in the web app, which is in its own repo. Build it in `accordion-web` with `npm ci` and `npm run build` (you need Node.js 22 or later), then start the server with `ACCORDION_WEB_DIR` set to the build's full path, such as `ACCORDION_WEB_DIR=$PWD/../accordion-web/dist cargo run`. Without it, the server's address shows a page that says the web app is missing.

### Trying each part

Each part below is in Settings. Its Server sections open only from your home network, unless you set `ACCORDION_ADMIN_ACCESS=anywhere`.

**HTTPS -** Run `cargo run -- set-name <name> <desec|cloudflare> --staging`. It asks for an API token that can change the name's A record and `_acme-challenge` TXT record, then gets a test certificate from Let's Encrypt. The server serves `https://<name>` on port 443 and redirects port 80 there. To change the ports or the address, set `ACCORDION_HTTPS_LISTEN`, `ACCORDION_HTTP_LISTEN`, or `ACCORDION_ADDRESS`.

**The setup wizard -** Skip `create-owner`, and start with an empty data folder and `ACCORDION_CERTIFICATE_AUTHORITY=lets-encrypt-staging`. The log shows the wizard's address and setup code. Other devices on your network can open it at `http://accordion.local`.

**Your household -** Under **People**, add, rename, turn off, and delete accounts, and make reset links. Each new person chooses a password at `/welcome`. If the owner forgets theirs, `cargo run -- reset-owner-password` prints a reset link.

**Docs and Notes -** Turn them on or off under **Modules**, and open them from the app picker. To edit together, share a note or document, then open it in a second browser as someone else.

**Importing from Google -** Export Keep and Drive at [Google Takeout](https://takeout.google.com) as `.zip` or `.tgz` files. Under **Import from Google**, upload them, select **Check archives**, then **Import**. The archives are read in the same sandbox as thumbnails, and voice recordings need ffmpeg.

**WebDAV -** Make an app password under **Apps and devices**, then connect to `/dav/drive/` for My Drive, `/dav/shared/` for what's shared with you, or `/dav/places/` for each app's folder. File Explorer needs HTTPS. Over plain HTTP, try `curl -u <username>:<app password> -X PROPFIND http://127.0.0.1:8080/dav/drive/`.

**Remote access -** Turn it on under **Remote access**. Then add a device under **Apps and devices**, and scan its QR code with the WireGuard app, which stands in for Accordion's phone app until it's built. On a server without a name, Accordion doesn't ask the router for the port, so devices reach it only at its home address.

**Link sharing -** Turn it on under **Remote access**, then select **Make a link** in an item's **Share** dialog. On a server without a name, links open at `http://127.0.0.1:8443`, or wherever `ACCORDION_LINKS_LISTEN` says.

**Backups -** Install [restic](https://restic.net) 0.19.1, or set `ACCORDION_RESTIC` to its full path. Under **Backups**, turn backups on, save the recovery key, and add a place for backups to go: a folder, an S3 bucket, a folder on another computer over SFTP (add the SSH key the page shows), or a [REST server](https://github.com/restic/rest-server). Run a REST server with HTTPS and `--append-only`, so a server taken over by an attacker can't delete its backups ([decision 10](docs/decisions/0010-backups-a-compromised-server-cant-delete.md)). To try one locally, run `rclone serve restic --append-only` with a certificate. Backups run at 3 a.m., and **Back up now** runs one right away.

**Restoring -** Start with an empty data folder, and select **Restore from a backup** in the setup wizard. For a backup over SFTP, first add the new SSH key the wizard shows.

**Thumbnails -** Install the programs that make them, which the release package depends on or recommends:

```sh
sudo apt install libvips-tools libheif-plugin-libde265 poppler-utils ffmpeg libraw-bin librsvg2-bin djvulibre-bin sox \
    libreoffice-writer-nogui libreoffice-calc-nogui libreoffice-impress-nogui
```

A server you start with `cargo run` runs them in a sandbox that needs Linux 5.13 or later with Landlock turned on. The package runs them in the worker service instead ([decision 29](docs/decisions/0029-sandboxing-workers-without-landlock.md)).

**Voice memo transcripts -** Build whisper.cpp with `scripts/build-whisper.sh amd64 <folder>`, which needs zig 0.14.1, and set `ACCORDION_WHISPER` to the `whisper` program in that folder. Then, under **Notes** > **Transcripts**, choose **Faster** or **More accurate**, and Accordion downloads that model ([decision 33](docs/decisions/0033-transcribing-voice-memos.md)). To use models from another computer, set `ACCORDION_MODELS_URL` to a folder of them.

**Server health -** It checks the certificate, the address record, `accordion.local`, remote access, link sharing, backups, updates, the web app, search, thumbnails, and transcripts, and whether any stored file changed or went missing. When something isn't working, it says why.

### Working across repos

`clone-workspace.sh` makes local builds use your checkouts of the other repos. To ship a change that spans repos, commit in each repo, then run `scripts/ship.sh`. It pushes `accordion-core`, `accordion-gateway`, the modules, and `accordion-web` in that order, pins this repo to what it pushed, and pushes this repo. Give it a message to commit this repo's own changes together with the pins:

```sh
scripts/ship.sh "Add Settings, and pin core and web"
```

To pin `Cargo.lock` by hand instead:

1. Push the other repos.
2. Run `scripts/pin-versions.sh`, which pins the versions on GitHub.
3. Commit without building first. A build removes the pins, and a pre-commit hook refuses a `Cargo.lock` without them.

## Contributing

Accordion is at an early stage. The best way to help right now is to open an issue with feedback on the plan, especially if you've tried to leave Google before and know what made it hard. Contribution guidelines, including whether contributions need a contributor agreement, aren't decided yet.

## Security

Please don't report security vulnerabilities in public issues. See [SECURITY.md](SECURITY.md) to learn how to report them privately.

## License

[AGPL-3.0 or later](LICENSE)
