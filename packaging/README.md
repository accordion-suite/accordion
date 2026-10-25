# Packaging

Files for installing Accordion on a Linux machine as a system service, and the Debian package that installs them ([decision 9](../docs/decisions/0009-releases-and-installing.md)).

| File | Installed as | What it does |
|---|---|---|
| `systemd/accordion.service` | `/usr/lib/systemd/system/accordion.service` | Starts Accordion when the machine starts, and restarts it if it stops unexpectedly |
| `systemd/accordion.sysusers` | `/usr/lib/sysusers.d/accordion.conf` | Creates the `accordion` user that Accordion runs as |
| `debian/postinst`, `prerm`, `postrm` | Package scripts | Create the user, start the service when installed, stop it when removed, and turn it off when purged |
| `debian/copyright` | `/usr/share/doc/accordion/copyright` | Accordion's and restic's licenses |
| `install.sh` | Not installed | The install command's script, which downloads, checks, and installs the package for the machine |
| `third-party-licenses.py` | `/usr/share/doc/accordion/third-party-licenses.txt` | Collects the licenses of the Rust libraries built into Accordion |

The package also includes [restic](https://restic.net) 0.19.1, the program that makes backups ([decision 8](../docs/decisions/0008-backups.md)), as `/usr/lib/accordion/restic`. It's kept apart from any restic your distribution installs, so Accordion always runs the version it was tested with. Backups to another computer over SFTP also need OpenSSH's client, which the package recommends.

The package runs on Debian 12 or later, Ubuntu 24.04 or later, and Raspberry Pi OS based on Debian 12 or later, on x86_64 and ARM64.

## What the service does

- **Accordion runs as its own user,** `accordion`, never as root. Its data lives in `/var/lib/accordion`, which only that user can open, because it holds the database, the DNS token, and private keys.
- **It can use ports 80 and 443.** Ports below 1024 normally need root. The service grants only the one capability that allows them, `CAP_NET_BIND_SERVICE`, and Accordion can't gain any other.
- **It can't touch the rest of the machine.** The service sees the system as read-only, can't see home folders, and can't load kernel modules, change the clock, or use most system calls. If Accordion were ever compromised, this limits what an attacker could reach.
- **It can write backups to disks mounted under `/media` or `/mnt`,** such as a USB disk, and nowhere else outside its data folder. Mount backup disks there, and make the backup folder writable by the `accordion` user.

## Building the packages

`scripts/build-packages.sh` builds `accordion_<version>_amd64.deb` and `accordion_<version>_arm64.deb` in `target/packages`, with `install.sh` and the `SHA256SUMS` of all three. The same commit gives the same files on any machine, which lets the developer check CI's release packages before signing them. For that, it needs:

- **The Rust version in `rust-toolchain.toml`.** rustup installs it.
- **zig 0.14.1 and cargo-zigbuild 0.23.4,** which build both architectures from one machine. Install cargo-zigbuild with `cargo install --locked cargo-zigbuild --version 0.23.4`.
- **The release key's public half** in `packaging/release-key.pem`, which goes into `install.sh`. To test with another key, set `ACCORDION_RELEASE_KEY` to its public PEM file.
- **A clone outside the workspace,** with no changes that aren't committed. In the workspace, Cargo builds your checkouts of the other repos instead of the versions in `Cargo.lock`.

```sh
git clone https://github.com/accordion-suite/accordion ~/accordion-release
cd ~/accordion-release
scripts/build-packages.sh
```

## Installing a package

Install the package for your machine with apt, which also installs what it needs:

```sh
sudo apt install ./accordion_0.1.0_arm64.deb
```

Accordion starts right away, and every time the machine starts. Find the setup wizard's address and code in the log:

```sh
journalctl -u accordion
```

You can also show the code with this command:

```sh
sudo -u accordion env ACCORDION_DATA_DIR=/var/lib/accordion accordion setup-code
```

Run other `accordion` commands the same way, as the `accordion` user, so the files they create belong to the service.

To change a setting, such as `ACCORDION_ADDRESS`, run `sudo systemctl edit accordion` and add it under `[Service]`, for example `Environment=ACCORDION_ADDRESS=192.168.1.20`. Then run `sudo systemctl restart accordion`. `accordion --help` lists every setting.

To update, install the newer package the same way. Accordion restarts into the new version.

## Removing Accordion

```sh
sudo apt remove accordion
```

This stops Accordion and removes its programs, but **never deletes `/var/lib/accordion`**, even with `apt purge`. That folder holds everyone's files, the database, and the server's keys, and installing Accordion again uses it. If you don't need it, make sure you have a backup, then delete it with `sudo rm -r /var/lib/accordion`.
