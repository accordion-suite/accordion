# Accordion

**Self-hosted Drive, Docs, Mail, and Notes on hardware you own. One install, one sign-in, one backup, zero telemetry.**

Accordion is a local-first replacement for Google Workspace. It gives a household or small team the convenience of Workspace (one sign-in, sync across devices, real-time collaboration, and search) without an account at a company that reads, profiles, or monetizes your data.

> [!WARNING]
> Accordion is in early development. There's nothing to install yet. Star or watch this repository to follow progress toward the first public alpha.

## Why Accordion

Leaving Google today means stitching together five or six self-hosted projects, each with its own sign-in, update cycle, and backup story. Most people give up and stay.

Accordion's bet is that **one install, one admin panel, and one backup that covers everything** makes leaving Google realistic for non-experts. The goal for 1.0: a technically curious person moves a family of five off Google in one weekend and never touches a terminal afterward.

Like the instrument, the suite folds out. Start with Drive and Notes on a Raspberry Pi, then add Mail, Calendar, and more as your needs grow. Every module shares one identity, one storage layer, and one search index.

## Modules

| Module | Replaces | Planned for | Open protocols and formats |
|---|---|---|---|
| **Accordion Drive** | Google Drive | 1.0 | WebDAV, plain files on disk |
| **Accordion Docs** | Google Docs | 1.0 | Markdown and HTML; .docx, .odt, and .pdf import and export |
| **Accordion Mail** | Gmail | 1.0 | IMAP, SMTP, JMAP, Maildir, Sieve |
| **Accordion Notes** | Google Keep | 1.0 | Markdown files with YAML front matter |
| **Accordion Calendar** | Google Calendar | 1.5 | CalDAV, iCal |
| **Accordion Contacts** | Google Contacts | 1.5 | CardDAV, vCard |
| **Accordion Photos** | Google Photos | 2.0 | Original files with sidecar metadata |
| **Accordion Sheets** | Google Sheets | 2.0 | .xlsx, .ods, .csv |

Every module ships with the suite. The admin turns modules on or off, and users see only the ones that are on. A module that's off uses no memory, and its data stays on disk and in backups.

## Principles

- **Local-first.** Your data lives on your box. Apps keep an offline copy and sync when they can reach it.
- **Zero telemetry.** No analytics, crash reporting, update pings, or third-party fonts and CDNs. Accordion only connects to places you choose, such as your DNS provider and backup locations. An admin **Network** page that lists every connection and turns them off is planned.
- **Open formats.** Files stay as files, mail is Maildir, notes are Markdown, and contacts and calendars are vCard and iCal. Leaving Accordion should be as easy as joining.
- **Standards over lock-in.** Accordion speaks WebDAV, IMAP, SMTP, JMAP, CalDAV, and CardDAV, so the apps you already use, such as Thunderbird, iOS Mail, and DAVx5, keep working.
- **Boring to run.** Automatic updates on a schedule you choose, automatic encrypted backups, and a health page anyone can read.

## How it works

Everything runs on one machine you own. A single gateway handles TLS, remote access over WireGuard, and routing. Behind it, one server program runs every module, and the code that handles untrusted files and incoming mail runs in separate, locked-down processes. All modules share one identity provider, one search index, one sync engine, and one storage layer, so adding a module never adds a new sign-in, database, or backup.

You reach your server from:

- **Any browser**, through one web app with an app switcher, installable as a PWA
- **Desktop sync clients** for Windows, macOS, and Linux
- **Mobile apps** for iOS and Android
- **Standard clients** over IMAP, WebDAV, CalDAV, and CardDAV

Nothing needs an open router port. Phones and laptops enroll by scanning a QR code, then connect over the built-in VPN when they're away from home.

## Installing

Not available yet. The planned install options are:

1. **Appliance image.** Flash it to a Raspberry Pi 5 or an x86_64 machine, then finish setup in a browser.
2. **Docker Compose.** One file for existing Linux servers, also compatible with Podman.
3. **NAS app stores.** Packages for TrueNAS SCALE, Unraid, and Synology.
4. **Linux packages.** .deb and .rpm packages with systemd units, for servers without containers.

| Tier | Users | CPU | RAM | Storage |
|---|---|---|---|---|
| Minimum | 1–3 | 4-core ARM64 or x86_64 | 4 GB | 64 GB SSD plus 1 data drive |
| Family | Up to 10 | Raspberry Pi 5 (8 GB) or 4-core x86_64 | 8 GB | 256 GB NVMe plus 2 mirrored drives |
| Small team | Up to 25 | 6-core or better x86_64 | 16 GB | 512 GB NVMe plus 2 or more mirrored drives |

These are design targets. Benchmarking will confirm them.

## Privacy and security

Accordion trusts the server owner and no one else: not the network, not your internet provider, not someone who steals the disk, and not Accordion's own developers.

- TLS everywhere (TLS 1.3, or 1.2 for older clients such as File Explorer on Windows), and WireGuard for remote access
- Full-disk encryption, set up by the appliance image
- Optional end-to-end encrypted vaults for data that even your server can't read
- Backups encrypted before they leave the box
- Passkeys, TOTP, and per-app passwords, with one sign-in for every module
- A CI check that fails the build if any component makes an unlisted outbound connection

## Moving from Google

Accordion imports a Google Takeout archive and maps each product to its module: Gmail to Mail, Drive files to Accordion Drive, Keep to Notes, and Contacts, Calendar, and Photos to their own modules. Anything it can't convert goes into an "Unconverted" folder with a report, so nothing is lost silently.

## Roadmap

| Phase | Focus |
|---|---|
| **Foundation** | Gateway, identity, storage, installer, setup wizard, encrypted backups, and health page |
| **Alpha** | Drive with desktop sync, Notes, and Takeout import for files and notes |
| **Beta** | Docs with real-time co-editing, Mail with guided DNS setup, and iOS and Android apps |
| **1.0** | Appliance images, NAS packages, accessibility (WCAG 2.2 AA), and five languages |
| **1.5** | Calendar, Contacts, and optional on-box AI for search, summaries, and transcription |
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
| `accordion-mail` | Mail |
| `accordion-notes` | Notes |
| `accordion-web` | Web app |
| `accordion-desktop` | Desktop sync client |
| `accordion-android`, `accordion-ios` | Mobile apps |

Please open issues here, whichever module they're about.

## Building from source

There's nothing to install yet, but you can build the server and try it. You need [Rust](https://rustup.rs) and bash (on Windows, use WSL).

```sh
scripts/clone-workspace.sh                         # Clone the other Accordion repos next to this one
cargo run -- create-owner <username> "<Your name>"  # Create the owner account (asks for a password)
cargo run                                          # Start the server on http://127.0.0.1:8080
```

`cargo run -- --help` lists every command and setting. To run the server as a system service, see [packaging](packaging/README.md).

### Trying each part

**HTTPS.** You need a name under a domain whose DNS is at deSEC or Cloudflare, and an API token that can change the name's A record and its `_acme-challenge` TXT record. Run `cargo run -- set-name <name> <desec|cloudflare> --staging`, which asks for the token and gets a test certificate from Let's Encrypt that browsers don't trust. The server then serves `https://<name>` on port 443, redirects port 80 there, and keeps the name pointed at its address on your network.

- For other ports, set `ACCORDION_HTTPS_LISTEN` and `ACCORDION_HTTP_LISTEN`.
- For a different address, such as in a container, set `ACCORDION_ADDRESS`.

**The setup wizard.** Skip `create-owner`, and start with an empty data folder and `ACCORDION_CERTIFICATE_AUTHORITY=lets-encrypt-staging`. The log shows the wizard's address and setup code. Other devices on your network can also open it at `http://accordion.local`. The wizard gets the name and certificate, then moves to `https://<name>/setup` to create the owner account.

**The health page.** `https://<name>/health` shows how the certificate, the address record, `accordion.local`, and backups are doing. Like every admin page, it opens only from your home network unless you set `ACCORDION_ADMIN_ACCESS=anywhere`.

**Your household.** At `/people`, the owner or an admin adds people, renames them, makes reset links, turns accounts off and on, and deletes them. Deleting someone gives their files to another person or deletes them. Each new person gets a one-time link to `/welcome`, where they choose their own password. Everyone can change their password and name at `/account`. If the owner forgets theirs, `cargo run -- reset-owner-password` prints a link for choosing a new one.

**Backups.** Install [restic](https://restic.net) 0.19.1, and set `ACCORDION_RESTIC` to its full path if it isn't on your `PATH`. At `/backups`, turn backups on, save the recovery key, and add a place for backups to go:

- **A folder on another disk.**
- **An S3 bucket,** such as at Backblaze B2. Make the bucket private, and give the key access to that bucket only. At Backblaze, set the bucket's lifecycle to **Keep only the last version**, or the data restic deletes keeps taking up space you pay for.
- **A folder on another computer over SFTP,** such as a NAS. Add the SSH key the page shows to `~/.ssh/authorized_keys` for the user Accordion signs in as.
- **A REST server on another computer,** such as [rest-server](https://github.com/restic/rest-server) on a NAS or a Raspberry Pi. Run it with HTTPS and `--append-only`, and a server taken over by an attacker can't delete its backups there ([decision 10](docs/decisions/0010-backups-a-compromised-server-cant-delete.md)). To try one locally, run `rclone serve restic --append-only` with a certificate.

Backups run every night at 3 a.m., and **Back up now** runs one right away.

**Restoring.** Start with an empty data folder, and select **Restore from a backup** in the setup wizard. For a backup on another computer, first add the new SSH key the wizard shows. Accordion restores everything, including accounts and the server's name, then starts again. Sign in with the account you used before.

**Drive over WebDAV.** Create an app password at `/account`, then connect File Explorer, Finder, or another WebDAV client to `/dav/drive/` with your username and that app password. File Explorer needs HTTPS, so use a server with a name. Over plain HTTP, try `curl -u <username>:<app password> -X PROPFIND http://127.0.0.1:8080/dav/drive/`.

### Working across repos

`clone-workspace.sh` makes local builds use your checkouts of the other repos. To commit `Cargo.lock`:

1. Push the other repos.
2. Run `scripts/pin-versions.sh`, which pins the versions on GitHub.
3. Commit without building first. A build removes the pins, and a pre-commit hook refuses a `Cargo.lock` without them.

## Contributing

Accordion is at an early stage. The best way to help right now is to open an issue with feedback on the plan, especially if you've tried to leave Google before and know what made it hard. Contribution guidelines will follow once there's code to contribute to.

## Security

Please don't report security vulnerabilities in public issues. See [SECURITY.md](SECURITY.md) to learn how to report them privately.

## License

[AGPL-3.0 or later](LICENSE)
