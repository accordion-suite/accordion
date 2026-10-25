# 9. How Accordion is released and installed

- **Status:** Accepted
- **Date:** October 25, 2026

## Context

The spec's first goal is a "one-command install on Linux, a NAS app store, or a flashable Raspberry Pi image", with a setup wizard that finishes in under 15 minutes. It lists four install methods:

1. **Appliance image:** a flashable image for a Raspberry Pi 5 and generic x86_64 machines.
2. **Container stack:** one Docker Compose file, compatible with Podman.
3. **NAS app stores:** TrueNAS SCALE, Unraid, and Synology.
4. **Bare metal:** .deb and .rpm packages with systemd units.

It also asks for:

- **Signed releases and reproducible builds.**
- **Every install method ships all released modules in one versioned bundle.**
- **Updates that are signed and staged:** snapshot, update, health check, and automatic rollback on failure, on a schedule the admin picks.
- **No update pings.** An update check fetches a signed static file and sends no identifier or version.

Today, installing Accordion means building it from source, copying files by hand, and running restic's download steps from [`packaging/README.md`](../../packaging/README.md). That's fine for development, but nobody else can install Accordion that way. Some groundwork is done:

- **A systemd unit** with a tight sandbox, tested on WSL with SFTP backups.
- **A sysusers file** that creates the `accordion` user.
- **The setup wizard,** which prints its address and a setup code in the log.
- **Static ARM64 builds** with `cargo zigbuild`, which ran on the Raspberry Pi 5 for every live test so far.

Accordion is one program plus restic ([decision 8](0008-backups.md)), so a release is small: two programs, a service file, and a user.

This decision covers what a release contains, how it's signed, and which install method comes first. How updates run is left to a later decision, but the choices here have to make staged updates with rollback possible.

## Decision drivers

1. **One command on the hardware people already have:** a Raspberry Pi 5 running Raspberry Pi OS, or a spare PC running Debian or Ubuntu.
2. **A path to staged updates with rollback** on the admin's schedule, without the admin opening a terminal.
3. **Signed and checkable.** A release that someone tampered with must not install, even if the download site is compromised.
4. **Keeps the sandbox** that the systemd unit already gives Accordion.
5. **A base for the other methods.** The appliance image, container, and NAS packages should reuse the same build, not each need their own.
6. **Little to maintain** for one developer.

## Options

### Option 1: A Debian package and an install command

Build a .deb for amd64 and arm64 that installs the server, restic, the systemd unit, and the sysusers file. A short install script checks the system, downloads the package for the machine, checks its signature, and installs it with apt.

Pros:

- **Covers the target hardware.** Raspberry Pi OS, Debian, and Ubuntu all install .deb packages, and between them they cover most home servers.
- **Uses the unit that's already written and tested,** sandbox included.
- **apt handles the details:** dependencies such as `ca-certificates`, upgrades in place, and clean removal.
- **The appliance image can be Debian with this package installed,** so option 3 in the spec builds on it.

Cons:

- **Doesn't cover other distributions,** such as Fedora or Arch, until an .rpm or another package exists.
- **Needs root to install and update.** Accordion itself runs unprivileged, so a later update service needs a small part that runs as root.

### Option 2: A container image and a Compose file first

Publish a multi-architecture image to a registry, with a Compose file that starts it.

Pros:

- **Runs on any Linux with Docker or Podman,** whatever the distribution.
- **NAS app stores and umbrelOS build on containers,** so it's a step toward option 3 in the spec.

Cons:

- **Updating from inside is awkward.** To replace its own container, Accordion needs the Docker socket, which gives root on the host. Without that, updates fall to the admin or a third-party tool, with no health check or rollback.
- **A different sandbox.** Docker's default isolation is weaker than the systemd unit's in places, and announcing `accordion.local` and binding ports 80 and 443 need host networking or extra setup.
- **A registry to depend on,** such as GitHub's container registry, and image signing on top of release signing.

### Option 3: The appliance image first

A flashable image with the OS, Accordion, and full-disk encryption set up.

Pros:

- **The fewest steps for a new owner:** flash, boot, and open the wizard.
- **Accordion controls the whole system,** including encryption and OS updates.

Cons:

- **Much more to build and maintain:** OS updates, full-disk encryption with TPM unlock, and images for two architectures.
- **Needs a package or install step inside it anyway,** so it comes after option 1 rather than instead of it.

### Option 4: A tarball and a script that copies files

Ship the programs and service files in an archive, and have a script copy them into place, with no package manager.

Pros:

- **Works on any Linux with systemd.**

Cons:

- **Repeats what a package manager does,** such as tracking files, upgrading, and removing, and does it worse.
- **Nothing records what's installed,** so removal and support are harder.

## Comparison

| Driver | Option 1: .deb | Option 2: container | Option 3: image | Option 4: tarball |
|---|---|---|---|---|
| One command on a Pi or spare PC | Yes | Yes, once Docker is installed | Flash instead | Yes |
| Staged updates with rollback | Yes, with a root update service | Hard without the Docker socket | Yes | Yes, but by hand |
| Keeps the systemd sandbox | Yes | No | Yes | Yes |
| Base for the other methods | Image builds on it | NAS stores build on it | No | Partly |
| Work for one developer | Low | Medium | High | Medium |

## Recommendation

**Option 1: a Debian package and an install command, first.** The container comes next, for NAS app stores and other distributions, and the appliance image after that, built on the package.

The Raspberry Pi 5 is the main target, and Raspberry Pi OS is Debian. The package reuses the tested systemd unit, and the image can build on it later. Updates are the deciding factor. An update service on the machine can stage an update and roll it back. A container can't do that without root on the host.

### What a release contains

Each release is a Git tag on the `accordion` repo, such as `v0.1.0`. The release on GitHub has these files:

| File | What it is |
|---|---|
| `accordion_0.1.0_amd64.deb` | The package for x86_64 machines |
| `accordion_0.1.0_arm64.deb` | The package for ARM64 machines, such as a Raspberry Pi 5 |
| `SHA256SUMS` | The checksum of each package and of `install.sh` |
| `SHA256SUMS.sig` | The release signature over `SHA256SUMS` |
| `install.sh` | The install command's script, with this release's version and the release key in it |

Each package installs:

| Path | What it is |
|---|---|
| `/usr/bin/accordion` | The server, with every released module built in |
| `/usr/lib/accordion/restic` | restic, in the version this release was tested with |
| `/usr/lib/systemd/system/accordion.service` | The service |
| `/usr/lib/sysusers.d/accordion.conf` | The `accordion` user |
| `/usr/share/doc/accordion/copyright` | The licenses of Accordion and restic |

The server is a static build, so the same package runs on every supported release of Debian, Ubuntu, and Raspberry Pi OS. A later container can use the same program.

The package depends on `systemd` and `ca-certificates`, because Accordion and restic trust the system's certificates. It recommends `openssh-client`, which SFTP backups need.

### Installing

The install command is:

```sh
curl -fsSL https://github.com/accordion-suite/accordion/releases/latest/download/install.sh | sudo sh
```

The script:

1. Checks that the system is supported: Debian 12 or later, Ubuntu 24.04 or later, or Raspberry Pi OS based on Debian 12 or later, on amd64 or arm64, with systemd running.
2. Downloads the package for the machine, `SHA256SUMS`, and `SHA256SUMS.sig` from the same release.
3. Checks the signature with `openssl`, against the release key in the script, then checks the package's checksum. If either check fails, it stops without installing anything.
4. Installs the package with `apt-get install`, which pulls in its dependencies.
5. Shows the setup wizard's address and setup code.

For people who'd rather not pipe a script into `sudo`, the install guide also gives the same steps as separate commands.

When the package is installed for the first time, it creates the `accordion` user, then turns on the service and starts it. On an upgrade, it restarts the service, unless the admin turned it off. Removing the package stops the service, and installing it again starts it again. Purging the package turns the service off. **Removing it, even with `apt purge`, never deletes `/var/lib/accordion`.** That folder holds the family's data, so deleting it is a separate, deliberate step that the guide explains.

### Signing

Releases are signed with an Ed25519 key, the release key. Its public half is in three places: the repo, the install script, and the server program, so the update service can check updates without trusting the download site.

The private half stays offline, on the developer's computer, protected by a passphrase. It is never stored in GitHub. Publishing a release takes these steps:

1. Pushing a version tag starts a CI workflow that builds both packages and `install.sh`, and creates a draft release with `SHA256SUMS`.
2. The developer builds the same version locally and checks that the checksums match CI's. A match shows that the packages were built from the tagged code and nothing else.
3. The developer signs `SHA256SUMS` with the release key, uploads `SHA256SUMS.sig`, and publishes the release.

If the release key were held in CI, anyone who took over the GitHub account or a workflow could sign a release that every server would install. With the key offline, they'd also need the developer's computer and passphrase. The cost is a few minutes of manual work per release.

The signature is a plain Ed25519 signature, so `openssl` can check it in the install script, and `ring`, which the server already uses for TLS, can check it in the server.

### Reproducible builds

Step 2 of signing needs builds that come out identical on CI and on the developer's computer:

- **A pinned toolchain,** in `rust-toolchain.toml`, and `cargo build --locked`.
- **Paths stripped from the program,** so the build folder doesn't change the result.
- **Fixed timestamps and file order in the package,** from the release commit's date.
- **restic downloaded from its release and checked** against restic's signed checksums, never rebuilt.

### Revisit this decision if

- Most people asking about Accordion run a distribution that isn't Debian-based. Then the container or an .rpm moves up.
- A NAS app store becomes the most common way people want to install Accordion.
- Keeping the release key offline blocks an urgent security release. A second, offline backup key would help there before CI signing would.

## Consequences

- **The `accordion` repo gains** a release workflow, a script that builds the packages, `install.sh`, the release key's public half, and `rust-toolchain.toml`.
- **The systemd unit and sysusers file move** to the paths packages use, `/usr/lib/systemd/system` and `/usr/lib/sysusers.d`. The by-hand steps in `packaging/README.md` change to installing the package.
- **The README's install section** describes the install command once the first release is out, and says which systems it supports.
- **The update decision builds on this one:** signed releases with the key in the server, and packages that an update service running as root can install and roll back.
- **Each release needs the developer's computer,** for the reproducibility check and the signature.

## Open questions

- **Testing on the Raspberry Pi.** The Pi used for tests runs umbrelOS, which might replace its system files when it updates, so a package installed there might not last. Package tests use Raspberry Pi OS Lite on a spare microSD card instead.
- **Memory allocation in the static build.** musl's memory allocator is slower than glibc's for some workloads. Drive's speed on the Pi needs measuring, and a different allocator may help.
- **The .rpm.** It can follow from the same files once someone asks for it.
- **Where the install command lives.** A GitHub release URL works now. A short domain would be easier to type, but it's one more thing to keep running.
