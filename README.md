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
- **Zero telemetry.** No analytics, crash reporting, update pings, or third-party fonts and CDNs. Every outbound connection is listed in the admin panel, and you can turn each one off.
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

- TLS 1.3 everywhere, and WireGuard for remote access
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

There's nothing to install yet, but you can build the server and try signing in. You need [Rust](https://rustup.rs) and bash (on Windows, use WSL).

```sh
scripts/clone-workspace.sh                         # Clone the other Accordion repos next to this one
cargo run -- create-owner <username> "<Your name>"  # Create the owner account (asks for a password)
cargo run                                          # Start the server on http://127.0.0.1:8080
```

To try HTTPS, you need a name under a domain whose DNS is hosted at deSEC or Cloudflare, and an API token that can change the name's A record and its `_acme-challenge` TXT record. Run `cargo run -- set-name <name> <desec|cloudflare> --staging`. It asks for the token and gets a test certificate from Let's Encrypt, which browsers don't trust. From then on, the server serves `https://<name>` on port 443 and redirects port 80 there. To use higher ports, set `ACCORDION_HTTPS_LISTEN` and `ACCORDION_HTTP_LISTEN`. `cargo run -- --help` lists every setting. The server points the name at its own address on your network, and updates the record if the address changes. In a container, or to use a different address, set `ACCORDION_ADDRESS`.

To try the setup wizard instead, start with an empty data folder and skip `create-owner`. Set `ACCORDION_CERTIFICATE_AUTHORITY=lets-encrypt-staging`, then run `cargo run`. The log shows the wizard's address and setup code. Other devices on your network can also open it at `http://accordion.local`, which the server announces with multicast DNS. The wizard asks for the name and token, gets the certificate, and then opens `https://<name>/setup`, where you create the owner account.

Once the server has a name, `https://<name>/health` shows how its certificate, its address record, and `accordion.local` are doing. Sign in with the owner account to see it.

To run the server as a system service, see [packaging](packaging/README.md).

`clone-workspace.sh` also makes local builds use your checkouts of the other repos. Before you commit `Cargo.lock`, push those repos and run `scripts/pin-versions.sh`, so it pins the versions on GitHub. Don't build between pinning and committing, because a build removes the pins again. A pre-commit hook refuses a `Cargo.lock` without them.

## Contributing

Accordion is at an early stage. The best way to help right now is to open an issue with feedback on the plan, especially if you've tried to leave Google before and know what made it hard. Contribution guidelines will follow once there's code to contribute to.

## Security

Please don't report security vulnerabilities in public issues. See [SECURITY.md](SECURITY.md) to learn how to report them privately.

## License

[AGPL-3.0 or later](LICENSE)
