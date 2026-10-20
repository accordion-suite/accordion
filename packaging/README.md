# Packaging

Files for installing Accordion on a Linux machine as a system service. The .deb and .rpm packages will install them. Until those exist, you can install them by hand, as described below.

| File | Installed as | What it does |
|---|---|---|
| `systemd/accordion.service` | `/etc/systemd/system/accordion.service` | Starts Accordion when the machine starts, and restarts it if it stops unexpectedly |
| `systemd/accordion.sysusers` | `/etc/sysusers.d/accordion.conf` | Creates the `accordion` user that Accordion runs as |

## What the service does

- **Accordion runs as its own user,** `accordion`, never as root. Its data lives in `/var/lib/accordion`, which only that user can open, because it holds the database, the DNS token, and private keys.
- **It can use ports 80 and 443.** Ports below 1024 normally need root. The service grants only the one capability that allows them, `CAP_NET_BIND_SERVICE`, and Accordion can't gain any other.
- **It can't touch the rest of the machine.** The service sees the system as read-only, can't see home folders, and can't load kernel modules, change the clock, or use most system calls. If Accordion were ever compromised, this limits what an attacker could reach.

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
   sudo install -m 0644 packaging/systemd/accordion.sysusers /etc/sysusers.d/accordion.conf
   sudo systemd-sysusers
   ```

3. Start Accordion now, and every time the machine starts:

   ```sh
   sudo systemctl daemon-reload
   sudo systemctl enable --now accordion
   ```

4. Find the setup wizard's address and code in the log:

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
