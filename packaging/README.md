# Packaging

Files for installing Accordion on a Linux machine as a system service. The .deb and .rpm packages will install them. Until those exist, you can install them by hand, as described below.

| File | Installed as | What it does |
|---|---|---|
| `systemd/accordion.service` | `/etc/systemd/system/accordion.service` | Starts Accordion when the machine starts, and restarts it if it stops unexpectedly |
| `systemd/accordion.sysusers` | `/etc/sysusers.d/accordion.conf` | Creates the `accordion` user that Accordion runs as |

Accordion also needs [restic](https://restic.net) 0.19.1, the program that makes backups ([decision 8](../docs/decisions/0008-backups.md)). The service expects it at `/usr/lib/accordion/restic`, apart from any restic your distribution installs, so Accordion always runs the version it was tested with.

## What the service does

- **Accordion runs as its own user,** `accordion`, never as root. Its data lives in `/var/lib/accordion`, which only that user can open, because it holds the database, the DNS token, and private keys.
- **It can use ports 80 and 443.** Ports below 1024 normally need root. The service grants only the one capability that allows them, `CAP_NET_BIND_SERVICE`, and Accordion can't gain any other.
- **It can't touch the rest of the machine.** The service sees the system as read-only, can't see home folders, and can't load kernel modules, change the clock, or use most system calls. If Accordion were ever compromised, this limits what an attacker could reach.
- **It can write backups to disks mounted under `/media` or `/mnt`,** such as a USB disk, and nowhere else outside its data folder. Mount backup disks there, and make the backup folder writable by the `accordion` user.

## Installing by hand

These steps need `sudo`. Run them from the root of this repo.

1. Build the server:

   ```sh
   cargo build --release
   ```

2. Install the server, the service, and the user it runs as:

   ```sh
   sudo install -m 0755 target/release/accordion /usr/bin/accordion
   sudo install -m 0644 packaging/systemd/accordion.service /etc/systemd/system/accordion.service
   sudo install -D -m 0644 packaging/systemd/accordion.sysusers /etc/sysusers.d/accordion.conf
   sudo systemd-sysusers
   ```

   Some distributions, such as Ubuntu, don't create `/etc/sysusers.d` until something needs it. The `-D` creates it.

3. Install restic, checking the download against the checksum from restic's signed list of checksums. On a Raspberry Pi or another ARM64 machine, use the second pair of lines instead of the first:

   ```sh
   file=restic_0.19.1_linux_amd64.bz2 sha256=f415415624dcc452f2a02b8c33641791a8c6d6d3b65bbb3543fcf9a25151585c
   # file=restic_0.19.1_linux_arm64.bz2 sha256=a5f64aaab53d51e311fa3829124c5b703f2d14cf187d8640b6be3b2b49376465
   curl -fsSLO "https://github.com/restic/restic/releases/download/v0.19.1/$file"
   echo "$sha256  $file" | sha256sum --check
   bunzip2 "$file"
   sudo install -D -m 0755 "${file%.bz2}" /usr/lib/accordion/restic
   rm "${file%.bz2}"
   ```

   If `sha256sum` doesn't say `OK`, stop: the download isn't the file restic published.

4. Start Accordion now, and every time the machine starts:

   ```sh
   sudo systemctl daemon-reload
   sudo systemctl enable --now accordion
   ```

5. Find the setup wizard's address and code in the log:

   ```sh
   journalctl -u accordion
   ```

   You can also show the code with this command:

   ```sh
   sudo -u accordion env ACCORDION_DATA_DIR=/var/lib/accordion accordion setup-code
   ```

   Run other `accordion` commands the same way, as the `accordion` user, so the files they create belong to the service.

To change a setting, such as `ACCORDION_ADDRESS`, run `sudo systemctl edit accordion` and add it under `[Service]`, for example `Environment=ACCORDION_ADDRESS=192.168.1.20`. Then run `sudo systemctl restart accordion`. `accordion --help` lists every setting.

## Updating

Build the new version, install it over the old one, and restart the service:

```sh
cargo build --release
sudo install -m 0755 target/release/accordion /usr/bin/accordion
sudo systemctl restart accordion
```
