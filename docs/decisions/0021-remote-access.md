# 21. Remote access

- **Status:** Accepted
- **Date:** November 3, 2026

## Context

Accordion works only at home. The server's name points at its address on the home network, such as `192.168.1.20` ([decision 5](0005-server-name-and-certificates.md)). Away from home, a phone or laptop looks up the name and gets an address it can't reach, so the web app, WebDAV, and everything else stop working.

The spec promises "secure remote access from phones and laptops without opening router ports":

- **A built-in WireGuard server.** The mobile and desktop apps enroll by QR code and connect automatically when they're off the home network.
- **Optional:** a tailnet the household already runs (Tailscale or Headscale), or a reverse proxy on a rented server.
- **Optional, later:** a public endpoint for link shares only, with everything else still reachable only through the VPN.

It also leaves a question open. A WireGuard server at home needs one inbound UDP port. The spec asks whether Accordion should open it automatically (UPnP or NAT-PMP), use a relay, or ask the owner to forward it.

Some pieces exist already:

- **A name and a trusted certificate** (decision 5). Let's Encrypt issues it through the DNS challenge, so no port is open for it. Decision 5 left two questions for this decision: how the name resolves over the VPN, and what happens when the network a device is on uses the same addresses as the home network.
- **The gateway** is a library in the server program. It ends TLS for every request and checks that each one is for the server's own name.
- **Admin pages open only from the home network or the VPN.** The gateway counts loopback, private, and link-local addresses as home.
- **The Network page** ([decision 11](0011-the-network-page.md)) will list every connection the server makes out of the home, and why.
- **Live updates, the upcoming sync client, and the apps** all need the server reachable from away to be much use.

## Decision drivers

1. **Nothing leaves the box.** No company sees who connects or when, in line with the spec's zero-telemetry rules.
2. **Nothing else at home is exposed.** A device on the VPN reaches Accordion, not the router, the printer, or the server's other programs.
3. **The same name everywhere.** Bookmarks, passkeys, mapped drives, and apps keep working away from home without changing anything.
4. **No router settings when that's possible,** and clear steps when it isn't.
5. **Works when the internet provider shares one address among many homes** (carrier-grade NAT), or at least says so plainly.
6. **Removing a device cuts it off at once.**
7. **The same in tests, development, Docker, and the package.**
8. **Small on a Raspberry Pi.**
9. **One developer.**

## Options: where the tunnel runs

### Option A: The kernel's WireGuard, set up by a helper

Linux has had WireGuard since 5.6. A small privileged helper, like the update helper ([decision 12](0012-updates.md)), would make a `wg0` network interface and add a peer for each device.

Pros:

- **The fastest and the most proven.** The kernel moves packets with no copies through the server.
- **Little protocol code in Accordion.**

Cons:

- **Needs privileges.** Making interfaces needs `CAP_NET_ADMIN`, so either the server gets it or the helper does, with a way for the server to ask it for each change.
- **Exposes the whole machine.** Traffic on `wg0` reaches every program listening on the server, such as SSH. Keeping it to Accordion needs firewall rules, which differ across systems and can be undone by other software.
- **Not the same everywhere.** Tests and development would need root, and Docker needs `NET_ADMIN` and the kernel module on the host. Some NAS systems don't have it.

### Option B: WireGuard in the server, with its own small network stack

The server runs WireGuard itself, in Rust, with a user-space TCP/IP stack. Packets arrive on an ordinary UDP socket. The stack turns them into TCP connections and hands them straight to the gateway's TLS server, as if they'd come in on port 443. The Rust libraries are Cloudflare's boringtun and GotaTun, Mullvad's fork of it, with smoltcp as the network stack.

Pros:

- **Needs no privileges.** No network interface is made, and nothing on the machine changes.
- **Reaches only Accordion,** by construction. The stack only connects to the gateway, so nothing else on the machine or the home network can be reached through it, whatever the firewall says.
- **The same everywhere,** in tests, development, Docker, NAS packages, and the Raspberry Pi.
- **Devices are the server's own data.** Adding or removing one changes the server's list at once, with nothing to send to a helper.

Cons:

- **Slower than the kernel.** Every packet is copied through the server. The measurements below show plenty of room for a household's internet connection.
- **More code to own:** the tunnel, the network stack, and their timers.
- **One more thing in the server process** that reads packets from anyone on the internet, though WireGuard ignores any packet that doesn't come from a known device.

### Option C: The household's own tailnet only

Accordion does nothing itself. A household that wants remote access runs Tailscale, or its own Headscale server, on the Accordion machine and its devices, and shares the server's home address with them.

Pros:

- **No port at all.** Tailscale gets through most routers without port forwarding, using relays when it has to.
- **Nothing to build** beyond a guide.

Cons:

- **Tailscale's coordination servers see every device and when it connects,** unless the household runs Headscale, which is another server to look after.
- **One more account and app for every family member,** set up outside Accordion.
- **Removing a device in Accordion doesn't remove it from the tailnet.**

## Options: how devices reach home

### Option 1: The admin forwards a port

The setup steps tell the admin which UDP port to forward on their router, with help for common routers.

Pros: works with any router that allows it. Cons: a router setting that many admins have never seen, and it doesn't help with carrier-grade NAT.

### Option 2: Accordion asks the router

Most home routers accept requests from the home network to open a port, through the Port Control Protocol (PCP), NAT-PMP, or UPnP. Accordion asks for the one UDP port, renews the request before it expires, and removes it when remote access is turned off. The same answer says what the home's public address is.

Pros: no router settings, on most routers. Cons: some routers have it turned off, and some admins turn it off on purpose. It doesn't help with carrier-grade NAT either.

### Option 3: A relay

Devices connect to a server on the internet, which passes traffic to the home over a connection the home keeps open.

Pros: no port, and it works through carrier-grade NAT. Cons: a relay run by Accordion's developers would see when every household connects, which breaks driver 1. A relay the household rents, such as a small virtual server, works, but it's another machine to run.

### Option 4: IPv6

Many homes have public IPv6 addresses, which need no NAT. The router still needs to let the port through its firewall, which PCP and UPnP can ask for.

Pros: works through carrier-grade NAT when the provider gives IPv6. Cons: not every provider, phone network, or Wi-Fi network has IPv6.

## Options: which address the name means away from home

### Option I: The tunnel carries the one home address

Each device sends only the server's home address through the tunnel, such as `192.168.1.20/32`. The name keeps pointing at that address everywhere, so nothing about names changes. A route to one address beats the network the device is on, even when that network uses the same range, such as a café's `192.168.1.0/24`.

Pros: no DNS changes, and the same name, certificate, and passkeys work as they are. Cons: if the café's own network has a device at that exact address, such as its router at `192.168.1.1`, the device can't reach that one café device while the tunnel is up. Servers rarely have that address, and the admin guide already recommends reserving one for the server.

### Option II: The server answers DNS for devices on the tunnel

Devices ask the server, through the tunnel, for every name. It answers its own name with its tunnel address and passes other questions on.

Pros: no clash at all. Cons: every name a device looks up while the tunnel is on goes through the home, which is slower and more for the server to do. WireGuard apps set the DNS server for the whole device.

## Comparison

| Driver | A: Kernel WireGuard | B: WireGuard in the server | C: Own tailnet only |
|---|---|---|---|
| Nothing leaves the box | Yes | Yes | Tailscale sees devices, unless Headscale |
| Nothing else at home is exposed | Needs firewall rules | By construction | Needs tailnet rules |
| Removing a device cuts it off | Through the helper | At once | Separately, in the tailnet |
| The same everywhere | Root, `NET_ADMIN`, the kernel module | Yes | Depends on the household |
| Work for one developer | Medium | Medium | Small |

| Driver | 1: Forward a port | 2: Ask the router | 3: Relay | 4: IPv6 |
|---|---|---|---|---|
| No router settings | No | Yes, on most routers | Yes | Usually, through the router |
| Carrier-grade NAT | No | No | Yes | Yes, with IPv6 |
| Nothing leaves the box | Yes | Yes | Only a relay the household runs | Yes |

## Measurements

On the development computer, an AMD Ryzen 5 3600 under WSL, in one thread:

| What | Speed |
|---|---|
| WireGuard (boringtun 0.7), sealing and opening 1,420-byte packets | 6.8 Gbit/s |
| WireGuard, sealing only, as the server does for what it sends | 12.6 Gbit/s |
| TCP through smoltcp 0.14 in 1,420-byte packets, both ends in smoltcp | 29 Gbit/s |

- **The whole path** (UDP socket, tunnel, network stack, then TLS) wasn't measured. The parts are far faster than a home's internet connection, whose upload speed sets the limit for devices away from home: often 10–100 Mbit/s, and up to 1 Gbit/s on fiber.
- **A Raspberry Pi 5 is slower.** At a fifth of this speed, the tunnel still carries more than a gigabit. These figures join the Raspberry Pi measurements already planned.

## Recommendation

**WireGuard in the server (option B), with the router asked for the port (option 2), forwarding by hand when that fails (option 1), IPv6 where the home has it (option 4), and the tunnel carrying the one home address (option I).** Accordion doesn't run a relay. A household's own tailnet (option C) and a relay it rents (option 3) go in the admin guide as alternatives.

Option B is the only tunnel that reaches nothing but Accordion without firewall rules, needs no privileges, and works the same in tests and Docker. Its cost is speed, and the measurements leave a wide margin above any home's upload speed.

### The tunnel

- **One UDP port,** 51820 by default, which the admin can change. WireGuard answers nothing to a packet that doesn't come from a known device, so a scan of the port learns nothing.
- **The tunnel reaches the gateway only:** ports 443 and 80 on the server's home address. Mail's ports join when Mail is built. Packets to anything else are dropped.
- **Its own addresses.** The server picks a random `/24` in `10.0.0.0/8` the first time remote access is turned on, and gives each device one address in it, so clashes with networks that devices join are unlikely.
- **The crate is chosen when it's built:** boringtun or GotaTun, whichever is better maintained then, with `ring` for its cryptography, as rustls already uses. Both licenses are on cargo-deny's list.
- **When the server stops,** the tunnel stops with it, and devices reconnect when it's back, as WireGuard does.

### Names and addresses

- **The name doesn't change.** It still points at the server's home address. Each device's tunnel carries only that address and the server's tunnel address.
- **Devices on the tunnel count as the home network,** as the spec says, so admin pages open from them. Signing in is still needed: the tunnel only makes the server reachable.
- **The home's public address goes in a second DNS record,** under a random label of the server's name, such as `k3x9q2wm7f.ourhome.dedyn.io`. Devices find home through it. The label keeps the address out of certificate logs, which list the server's name. The server updates it like the first record, through the same DNS provider. An owner whose provider has only the `_acme-challenge` CNAME adds one more CNAME for this name, and the setup steps say so.
- **The public address comes from the router** when it answers PCP, NAT-PMP, or UPnP. Otherwise, deSEC's update service sets it from where the update comes from, and Cloudflare's trace page says what it is. Each of these joins the Network page.

### Reaching home

1. **The admin turns remote access on** in Settings, from the home network. It's off until then.
2. **Accordion asks the router for the port** through PCP or NAT-PMP, then UPnP, renews the request before it expires, and removes it when remote access is turned off.
3. **If the router says no,** the page shows how to forward the port by hand, with steps for common routers.
4. **If the home is behind carrier-grade NAT,** which shows as a router address in `100.64.0.0/10` or a private range, the page says plainly that the internet provider shares one address among many homes. It offers IPv6 if the home has it, and points to the admin guide's alternatives: a tailnet or a rented relay.
5. **Accordion never checks from the outside** whether the port is open, since that would mean asking a service on the internet. The first device that connects from away proves it, and **Server health** says so.

### Devices

- **Each person adds their own devices** in Settings, under **Apps and devices**: they name it, such as "Sam's phone", and get a QR code.
- **The QR code works with the WireGuard apps** for iPhone, Android, Windows, macOS, and Linux, so remote access works before Accordion's own apps exist. It holds the device's whole setup, including a private key the server makes for it. The server shows the code once and keeps only the public key.
- **Accordion's own apps, when they come,** make their key on the device and send only the public half, so the private key never leaves it.
- **A device is removed at once** when its person or an admin removes it, or the account is deleted. Its key stops working on the next packet.
- **Up to 10 devices for each person.**
- **The list shows when each device last connected from away,** which helps people spot one they don't use any more.

### At home and away

- **The WireGuard apps don't know when they're home.** On iPhone and Mac, their on-demand setting can turn the tunnel off on the home Wi-Fi. On Android and Windows, people turn it on when they leave. The user guide says how for each.
- **A tunnel left on at home still works** on routers that loop traffic back to their own public address, which many do. On those that don't, it stops working at home until it's turned off, and the user guide says so.
- **Accordion's own apps will switch by themselves:** they use the home address directly when they can reach it, and the tunnel when they can't.

### Security

- **WireGuard's handshake has its own limit** on how fast unknown devices can try, so floods of handshakes cost the server little.
- **The tunnel adds a way in, not a way past signing in.** Everything still needs a session or an app password, as at home.
- **A stolen phone** is removed from the device list, which ends its tunnel at once. Signing it out ends its sessions, as now.
- **Routers keep the mapping only while it's renewed,** so a server that's unplugged doesn't leave the port open for long.

### Testing

- **Tests run a real WireGuard client** in the test, through the same code path: a handshake, then an HTTPS request to the server's name, which has to reach the gateway and nothing else.
- **A test sends packets for other addresses and ports** through the tunnel and checks they're dropped.
- **A removed device's next packet** has to be ignored.
- **Port mapping is tested against a stand-in router** in the test, for PCP, NAT-PMP, and UPnP, including refusals and carrier-grade NAT.
- **The strace check** from decision 11 confirms the server only connects out to the DNS provider and the router.
- **The upgrade test** turns remote access on and connects through the tunnel from the same machine.

### Revisit this decision if

- A Raspberry Pi can't move more than about 100 Mbit/s through the tunnel.
- Many households are behind carrier-grade NAT without IPv6. A relay they can rent cheaply, with Accordion setting it up, could then be worth building.
- The WireGuard crates stop being maintained.

## Consequences

- **The gateway** gains the tunnel, the network stack, the device list's keys, the second DNS record, and port mapping.
- **Core's identity** gains each person's remote access devices, removed with the account.
- **The server's settings** gain the remote access switch, the UDP port, and the tunnel's address range.
- **The web app** gains **Remote access** in Settings for admins, and adding and removing devices under **Apps and devices**.
- **The Network page** lists the router requests, the public address lookups, and the second DNS record.
- **Server health** gains a **Remote access** check: whether it's on, whether the router opened the port, the public address, and when a device last connected from away.
- **The package** opens nothing by itself. The admin guide covers forwarding the port by hand, a tailnet, and a rented relay.
- **Docker and NAS installs** publish the UDP port as well as 443.
- **The spec's open question** about the inbound port is answered: ask the router, then forward by hand, with no relay run by Accordion's developers.

## Open questions

- **The tunnel's speed on a Raspberry Pi,** end to end, from a phone on mobile data.
- **Which crate:** boringtun or GotaTun, judged on maintenance when it's built.
- **IPv6 for the tunnel's own addresses,** besides reaching home over IPv6.
- **Link sharing's public endpoint,** which the next decision covers. It answers on the internet, so it needs its own rules, separate from the tunnel's.
