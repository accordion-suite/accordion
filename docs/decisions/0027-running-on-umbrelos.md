# 27. Running on umbrelOS

- **Status:** Accepted
- **Date:** November 12, 2026

## Context

The plan for the friends-and-family preview ([decision 25](0025-the-browser-away-from-home.md)) starts with the developer's own household using a v0.2.0 server on a Raspberry Pi 5. That Pi runs umbrelOS from an NVMe SSD, with four or five Umbrel apps that have to stay up all day. A Pi boots one system at a time, so Accordion can't take the Pi over. A spare SD card with Raspberry Pi OS Lite works for test sessions ([decision 9](0009-releases-and-installing.md#open-questions) planned that), but not for a household that uses Accordion every day.

umbrelOS is a home server system built on Debian and Docker. People install apps from its App Store, or from community app stores that anyone can publish as a Git repo. Many people who self-host start there, so it's also a likely way for other households to find Accordion.

Decision 9 chose the .deb package first and said a container image comes next, for NAS app stores and other distributions. It listed what a container costs: no self-update without the Docker socket, a different sandbox, and extra setup for `accordion.local` and ports 80 and 443. umbrelOS is a NAS-style app store, so this decision is the first container install.

### How Umbrel apps work

From the packaging guide in Umbrel's app repo ([getumbrel/umbrel-apps](https://github.com/getumbrel/umbrel-apps)):

- **An app is a folder** with `umbrel-app.yml`, the manifest, and `docker-compose.yml`, plus optional `exports.sh`, hooks, and templates.
- **Images must be prebuilt, public, multi-architecture** (`linux/amd64` and `linux/arm64`), and pinned by tag and digest. No building on the device.
- **Data lives under `${APP_DATA_DIR}/data`.** Containers are made again on every restart and update. umbrelOS 2.0 lets the owner move an app's data folder to an external drive.
- **The browser reaches an app through `app_proxy`,** on a port listed in the manifest. By default the owner's Umbrel login guards it, and umbrelOS 2.0 adds HTTPS with Umbrel's own certificate.
- **Ports 80, 443, and 2000 belong to umbrelOS.** Apps can publish other ports for protocols that aren't web pages.
- **Host networking is allowed when an app needs it** for finding devices on the home network, multicast, or broadcast. Such an app serves plain HTTP on its manifest port, and umbrelOS adds HTTPS in front. The Docker socket, privileged mode, and broad host mounts aren't allowed.
- **Updates appear when the store's manifest changes its `version`.** The owner selects **Update** in the dashboard, and umbrelOS pulls the new image. The guide doesn't describe an automatic rollback.
- **Umbrel's backups copy each app's data folder** unless the manifest's `backupIgnore` leaves parts out.
- **The App Store standard** asks that an app open to a web page with a clear next step, without SSH, a terminal, or reading logs.

## Decision drivers

1. **The household uses Accordion every day on the Pi it already has,** while its Umbrel apps keep running.
2. **The same Accordion as other installs:** the same name and certificate, remote access, links, sync, and previews.
3. **Keeps the sandbox for workers** ([decision 14](0014-thumbnails-and-sandboxed-workers.md)) and keeps data safe across updates.
4. **No Umbrel login for family members.** They have Accordion accounts, not Umbrel ones.
5. **Setup in the browser,** without a terminal, as Umbrel's standard and the spec's 15-minute setup both ask.
6. **Little to maintain** for one developer, and a step toward the spec's other container installs.

## Options

### Option A: An Umbrel app in Accordion's own community app store

Publish a multi-architecture container image from each release, and a Git repo that's an Umbrel community app store with one app, Accordion. The owner adds the store's address in umbrelOS once, then installs and updates Accordion from the dashboard.

Pros:

- **Runs beside the household's other Umbrel apps,** on hardware it already has.
- **Accordion's developer controls the store,** so a release reaches Umbrel when it's ready, with no review queue.
- **The image is the spec's container install too.** Docker Compose, TrueNAS SCALE, and Unraid can reuse it.

Cons:

- **New work:** the image, its CI, a container mode in the server, and the store repo.
- **umbrelOS does updates,** so Accordion's own update service ([decision 12](0012-updates.md)), with its snapshot and rollback, doesn't apply.
- **Shares the Pi's 8 GB of memory** with the other apps, which matters for previews of video and Office files.
- **People have to find the store address,** since community stores aren't listed in Umbrel's App Store.

### Option B: Go straight to the official Umbrel App Store

The same package, sent to Umbrel's own App Store repo as a pull request.

Pros:

- **Every umbrelOS owner can find Accordion** in the App Store.

Cons:

- **Umbrel's team reviews each change,** so a fix can wait days.
- **Reviewers question host networking** and other host access, which Accordion may need (see below).
- **Too early.** Accordion is a test release, and the App Store is for apps people can rely on.

### Option C: Install the .deb on umbrelOS itself

umbrelOS is Debian 13, so the package installs with `sudo`.

Pros:

- **Uses the existing package and update service,** with no new work.

Cons:

- **umbrelOS manages its own system and may replace it on an update,** taking Accordion with it, as decision 9 noted.
- **Ports 80 and 443 are taken,** and the package expects them.
- **Not a supported way to use umbrelOS,** so it could break the owner's other apps.

### Option D: Separate hardware

A second Raspberry Pi 5 or a small used mini PC, with Raspberry Pi OS or Debian and the .deb.

Pros:

- **Exactly what other households will run,** and what the appliance image targets.
- **No new work,** and Accordion gets the whole machine.

Cons:

- **Costs money:** around 100–150 USD for a Pi 5 with a case, power supply, and SSD, or a similar amount for a used mini PC.
- **Doesn't help umbrelOS owners** find or run Accordion.

## Comparison

| Driver | A: Community store | B: Official store | C: .deb on umbrelOS | D: Separate hardware |
|---|---|---|---|---|
| Daily use, Umbrel apps keep running | Yes | Yes | Risky | Yes |
| Same Accordion as other installs | Mostly (see below) | Mostly | Yes, until umbrelOS updates | Yes |
| Worker sandbox and safe updates | Sandbox yes, updates through Umbrel | Same as A | Yes | Yes |
| Setup in the browser | Yes | Yes | Needs a terminal | Yes |
| Work for one developer | Medium | Medium, plus reviews | Low, but fragile | None |
| Step toward the container installs | Yes | Yes | No | No |

## Options: networking

Option A needs one more choice, because umbrelOS keeps ports 80 and 443 and the app runs in a container.

### Option N1: Docker's network, with published ports

The browser reaches Accordion through `app_proxy`, and the container publishes the remote access UDP port and the links port.

- **Pros:** the container has its own network, as Umbrel prefers.
- **Cons:** Accordion can't see the home network. It doesn't know the Pi's home address for its DNS record, it can't ask the router for ports ([decision 21](0021-remote-access.md)), and it can't announce `accordion.local`. The admin would type the address and forward ports by hand.

### Option N2: Host networking

The container uses the Pi's network directly, as the .deb install does, but on ports that umbrelOS doesn't use.

- **Pros:** Accordion finds the home address, asks the router for ports, and announces `accordion.local`, exactly as on other installs. This is the case Umbrel's guide allows host networking for.
- **Cons:** less separation from the host's network, so Accordion could reach services the Pi listens on only for itself. Umbrel's login can't guard the app, since `app_proxy` isn't used.

## Recommendation

**Option A, a community app store, with host networking (N2).** Option B follows once Accordion leaves testing. Option D stays the fallback if the container can't keep the worker sandbox or the household finds the Pi too slow with everything on it.

Option A is the only option that gives the household daily use on the Pi it has, without risking its other apps. It also builds the container image that decision 9 put next. Host networking keeps remote access and links working the way decisions 21 and 22 designed them, instead of turning them into steps the admin does by hand.

### The image

- **Built from the release's signed .deb,** so the container runs the same program. The image's build checks `SHA256SUMS.sig` against the release key before it installs the package.
- **Debian 13 slim as the base,** with the package's dependencies and recommended programs: libvips, poppler, ffmpeg, LibreOffice, and the others from decision 26. LibreOffice makes the image large, around 1 GB.
- **Published to GitHub's container registry** under `accordion-suite`, for `linux/amd64` and `linux/arm64`, after the release is signed. The store pins it by tag and digest.
- **Runs as user 1000,** as Umbrel expects, with no extra capabilities.
- **The compose file stands in for the systemd unit's limits on the server:** `no-new-privileges`, every capability dropped, and a read-only file system except the data folder and a temporary folder. Docker has nothing like systemd's `MemoryDenyWriteExecute`, so the server loses that one limit. Workers keep all of theirs, since the launcher applies them itself.

The image is signed only by the registry's digest, unlike the .deb, whose signature comes from a key kept offline. Signing the image the same way, for example with an attestation made from the offline key, can come later.

### Ports and addresses

- **Accordion's HTTPS listener moves off 443,** to 8443 by default, so the server's address becomes `https://<name>:8443`. Passkeys work, since a passkey belongs to a name, not a port. The tunnel for remote access carries the same port.
- **The manifest's port serves plain HTTP,** for the setup wizard and, after setup, a page that sends people to the HTTPS address. umbrelOS puts its own HTTPS in front of it.
- **Remote access and links publish their ports** as on other installs: UDP 51820, and the links port.
- **`accordion.local` is announced** next to Umbrel's own `umbrel.local`. Both use multicast DNS on the same machine, which needs testing.

### Setup

The Umbrel launch button opens Accordion's setup wizard. Decision 5's claim without a setup code applies: for 30 minutes after the app starts, the first person to open the wizard can claim it. Restarting the app from the Umbrel dashboard opens that window again. Only the Umbrel owner can do that, so it proves ownership the way restarting a headless appliance does. Nobody reads a code from the logs.

### Data and backups

- **All data lives in `${APP_DATA_DIR}/data`,** with `storage: dataRoot: data`, so the owner can move it to an external drive in umbrelOS 2.0.
- **`backupIgnore` leaves out the cache:** thumbnails, previews, and the search index, which Accordion makes again.
- **Accordion's own backups** ([decision 8](0008-backups.md)) stay the recommended backup. They take consistent snapshots of the database, which a copy of a running app's folder might not.

### Updates

- **Each signed release updates the store repo:** the manifest's version, its release notes, and the image's tag and digest. A script in the `accordion` repo does it, as one more step after signing.
- **The Updates page says umbrelOS updates this server,** and links to the release notes, instead of offering to install.
- **Before a database migration, Accordion copies the database** next to itself, so a failed update can be undone by hand. umbrelOS doesn't roll back.

### The worker sandbox

Docker's default seccomp profile has allowed Landlock's system calls since 2022 ([moby/moby, "seccomp: add support for Landlock syscalls in default policy"](https://git.causa-arcana.com/kotovalexarian-likes-github/moby--moby/commit/af819bf623a8d414289b24f9817c6317cf3f96d5)), and a container can add its own seccomp filter on top. So the sandbox launcher from decision 14 should work unchanged inside the container. This is the first thing to test, because if it fails, workers can't run safely, and option D becomes the answer.

### Revisit this decision if

- The sandbox launcher doesn't work in Umbrel's containers.
- The Pi can't run Accordion's previews alongside the household's other apps without slowing them.
- Umbrel's App Store rules change to forbid host networking for apps like this.
- umbrelOS adds a rollback for app updates, which would replace Accordion's database copy.

## Consequences

- **A new repo,** `accordion-umbrel`, made with `scripts/new-repo.sh`, holding the community app store. umbrelOS adds a store by its Git address and reads `umbrel-app-store.yml` at its root, so the store can't be a folder in the `accordion` repo without every Umbrel device cloning the whole suite.
- **The store's ID is `accordion` and the app's is `accordion-suite`,** since an app's ID has to start with its store's ID. The app's ID names its data folder and can't change after release. An app in the official App Store gets an ID without the prefix, so moving there later means moving each install's data, which the official listing has to explain.
- **The app's icon and screenshots live in its folder** in the store repo, and the manifest's `icon` and `gallery` give their full raw GitHub addresses, as other community stores do. Without `icon`, umbrelOS falls back to `https://getumbrel.github.io/umbrel-apps-gallery/<app-id>/icon.svg`, Umbrel's own gallery, which has no Accordion. A separate gallery repo served by GitHub Pages, as Umbrel runs for its App Store, isn't needed for one app. The official App Store hosts its own images.
- **The release workflow builds and pushes the image** after signing, and `packaging/README.md` gains the step that updates the store.
- **The server gains a container mode:** HTTPS on a port other than 443, an address that includes the port, a plain HTTP page for the launch button, an Updates page that defers to umbrelOS, and a database copy before migrations.
- **Server health** checks for the image's programs as it checks the .deb's dependencies.
- **The admin guide** gets a page for umbrelOS: adding the store, installing, and where the data lives.
- **Decision 9's open question about testing on the Pi** gets a second answer: test sessions from the SD card for the .deb, and the Umbrel app for daily use.

## Open questions

- **Does the sandbox launcher work in Umbrel's containers?** Docker isn't available in this WSL setup, so it hasn't been tried. It's the first step, on the Pi itself.
- **Do Accordion's and Umbrel's multicast DNS responders get along** on the same machine?
- **Which umbrelOS version is on the Pi?** HTTPS and moving app data need umbrelOS 2.0.
- **Does every client handle a port in the server's address?** That includes File Explorer's WebDAV, the Windows app, and later the phone apps.
- **umbrelOS reserves ports for virtual machines,** so it may be able to run one. A virtual machine with Raspberry Pi OS and the .deb would avoid the container work. This decision didn't look into it.
