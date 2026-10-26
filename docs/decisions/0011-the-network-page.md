# 11. The Network page

- **Status:** Accepted
- **Date:** October 26, 2026

## Context

The spec promises zero telemetry: "Any outbound connection is listed in the admin panel and can be turned off." Its privacy section names the connections and the page: "Outbound connections are allow-listed and shown on an admin 'Network' page: update check, mail delivery, DNS updates, certificate renewal, backup target. Each can be disabled." It also asks for a CI test that "fails the build if any component opens an unlisted outbound connection." [Decision 5](0005-server-name-and-certificates.md) and [decision 8](0008-backups.md) both rely on this page, and neither the page nor the check exists yet.

Today, Accordion connects to other machines for three reasons:

| Purpose | Connects to | When |
|---|---|---|
| Keeping the name pointed at the server | The DNS provider's API: `desec.io` or `api.cloudflare.com` | When the server's address on the home network changes. Accordion checks it locally each minute. |
| Renewing the certificate | Let's Encrypt, the DNS provider's API for the challenge record, and the zone's nameservers to see that record is live | About once every two months |
| Backups | Each S3 bucket, SFTP server, and REST server the admin adds | Every night, and for restore tests |

All three go to places the owner chose: the DNS provider in setup, and each backup target on the backups page. The setup wizard, the health page, and the backups page already show them. Nothing contacts Accordion's own servers, or anything the owner didn't pick.

That changes with the update check, which the spec lists first. It will be the first connection to something the owner didn't choose, and the one a privacy-minded owner will most want to turn off. Mail delivery comes later and is similar. The release plan already waits on the decision about updates.

Two facts limit what turning a connection off can mean today:

- **Renewal needs the DNS provider.** The certificate comes through the DNS challenge, so renewing it writes a record at the DNS provider, even with address updates turned off.
- **Nothing replaces a certificate that isn't renewed.** Decision 5 plans a local certificate authority as the fallback for households that keep the server offline, but it isn't built. Until it is, turning off renewal breaks HTTPS within about two months, and devices stop connecting.

## Decision drivers

1. **"Zero telemetry" stays true as dependencies change.** It's the promise people install Accordion for, and a library update that starts phoning home would break it without anyone noticing.
2. **Each switch is worth having.** A switch that only breaks the server, or repeats a button on another page, adds work and confuses the owner.
3. **The owner can see every connection,** where it goes, why, and whether it worked last time, once there's something on the list they didn't choose.
4. **Little to build and maintain** for one developer.

## Options

### Option 1: Build the full page now

Core keeps a list of connection purposes, each with a switch and its last attempt. The gateway checks the switches for name updates and renewal. Backup targets gain a paused state. The suite gains the Network page, and CI gains the check.

Pros:

- **Matches the spec today.**

Cons:

- **The switches are weak.** Turning off renewal only breaks the server. Name updates are only safe to turn off when the address never changes, and then they rarely run anyway. Pausing a backup target is close to **Remove**, which keeps the backups that are already there.
- **The page repeats other pages,** since every connection on it is one the owner set up.
- **Work across core, the gateway, and the suite,** while the release waits on other things.

### Option 2: Build the CI check now, and the page with the update check

CI gains the check now, in the shared workflow, so every repo gets it. The Network page is built with the update check, when the list first holds a connection the owner didn't choose. Renewal is listed without a switch until the local certificate authority exists.

Pros:

- **Protects the promise now,** on every push, in every repo, without anyone having to remember it.
- **Every switch the page offers is worth having,** because the page arrives with the update check.
- **The page's design is settled now,** so building it later doesn't reopen the decision.

Cons:

- **Until the page exists, the spec's "listed in the admin panel" isn't met.** The README says the page is planned, and this record lists today's connections.

### Option 3: Build the CI check only, and list connections in the docs

CI gains the check. The docs list the connections, and settings pages turn off what they can.

Pros:

- **Least to build.**

Cons:

- **No single place to look,** which is what the spec promises.
- **Nothing shows what actually happened,** and the docs fall behind the code.

### Also considered: a firewall that Accordion manages

Accordion could write firewall rules, such as with nftables, that allow only the listed connections. Firewalls work with addresses, not names, and Let's Encrypt and S3 providers change their addresses often. It also needs root access, works differently across distributions, containers, and NAS systems, and would fight with firewalls the owner already has. It can't tell the owner why a connection exists, so it would still need the page.

## Comparison

| Driver | Option 1: full page now | Option 2: check now, page with updates | Option 3: check and docs |
|---|---|---|---|
| The promise stays true | Yes | Yes | Yes |
| Every switch is worth having | No | Yes | Not applicable |
| The owner sees every connection | Yes | Once updates exist | Partly |
| Work for one developer | High | Low now, medium later | Low |

## Recommendation

**Option 2: build the CI check now, and the Network page with the update check.**

### The CI check

The shared CI workflow runs each repo's tests under `strace`, which records every connection a program opens and every packet it sends. It follows the programs the tests start, such as restic, rclone, and ssh. The check fails on any connection to an address outside this machine, and names the address and the program that made it.

These are allowed:

- This machine: `127.0.0.0/8`, `::1`, and local sockets
- The DNS servers in `/etc/resolv.conf`, since tests look up names. A program that phones home still gets caught when it connects to the address it looked up.
- Multicast that routers never forward (`224.0.0.0/24` and `ff02::/16`), which stays on the home network, such as the mDNS announcements that let devices find the server
- The ranges reserved for documentation (`192.0.2.0/24`, `198.51.100.0/24`, `203.0.113.0/24`, and `2001:db8::/32`), which tests use to stand for a computer that doesn't answer. Nothing on the internet uses them.

The tests already use stand-ins on this machine for every outside service, such as a local S3 server and a local certificate authority. So a connection anywhere else means code reached out on its own, such as a library that phones home.

Steps that download restic, rclone, and other test tools run before the check starts. Tests that need a real account, such as the live DNS tests, are marked to be skipped in CI and are run by hand, so the check doesn't cover them.

The check only covers code the tests run. A new feature that connects out also has to add its entry to the page, which the code review for that feature checks.

### What the page will show

A **Network** admin page, at `/network`, opens only from the home network like the other admin pages. It lists one entry for each purpose:

- **What it's for,** in plain words, such as "Renews the certificate that lets your devices connect securely."
- **Where it connects,** such as `acme-v02.api.letsencrypt.org` and `desec.io`.
- **How often,** and **the last time it connected,** with whether that worked.
- **A switch** to turn it off and what that means, or why it can't be turned off yet.

Each backup target is its own entry, so the owner can turn off one without the others.

The page also says what it doesn't cover: connections the operating system makes on its own, such as system updates and time sync, and name lookups, which go through the system's DNS server, usually the router.

### What turning each off will do

- **The update check:** decided with the update check itself.
- **A backup target:** backups to it pause. Nightly backups and **Back up now** skip it, and so do restore tests. The backups page shows it as paused. If every target is paused, the health page reports that nothing is being backed up.
- **Keeping the name pointed at the server:** Accordion stops updating the address record. The page says this is fine for a server whose address on the home network never changes, and that devices stop finding the server if it does change.
- **Renewing the certificate:** no switch. The page says why: without renewal, the certificate expires and devices can't connect. Once the local certificate authority exists, the page offers to switch to it, which stops renewal and contact with the DNS provider.

Switches are stored in the database, so they survive restarts and come back with a restore. Turning a connection back on takes effect right away.

### Revisit this decision if

- The update decision is made. The page is built with it, and this record's design is the starting point.
- The local certificate authority is built. Renewal gains a switch that moves the server to it.
- Accordion runs as its own system user on an appliance image, where a managed firewall could back up the list without fighting other software.

## Consequences

- **The shared CI workflow gains the `strace` check now,** for every repo.
- **The README describes the Network page as planned** until it's built. Today it says the admin panel lists every connection.
- **Decision 8's Network page entries wait for the page.** Until then, the backups page shows each target, and **Remove** stops backups to it.
- **When the update check is built,** core gains the list of connection purposes, with their switches and last attempts. The gateway checks the switch for name updates and reports each attempt. Backup targets gain a paused state. The suite gains the Network page, and the health page links to it.
- **Mail delivery adds its entry** when it's built.

## Open questions

- **Should a paused backup target stay paused after a restore?** Restoring brings back the switches with everything else, which seems right, but a household restoring after a disaster might not expect it. This can wait until the page is built.
