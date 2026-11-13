# 28. Reaching a home behind carrier-grade NAT

- **Status:** Accepted
- **Date:** November 13, 2026

## Context

Accordion's apps reach home through their own WireGuard tunnel ([decision 23](0023-sync.md)), and on Windows they carry the browser through it too ([decision 25](0025-the-browser-away-from-home.md)). Every tunnel ends at the home server, which waits for devices on one UDP port ([decision 21](0021-remote-access.md)). That only works when the internet can reach the home.

Behind carrier-grade NAT, it can't. The internet provider shares one public address among many homes, so no port at home can be opened from outside. The first live test on a real router, on November 12, 2026, hit exactly this: Accordion asked the router for the port through PCP, and the router agreed, but the router's own address was `100.112.x.x`, in the shared range. There was no public IPv6 either. Carrier-grade NAT is common in Australia and growing elsewhere as IPv4 addresses run out.

Decision 21 left this case to the household: ask the provider for a public address, use IPv6, or run a tailnet or a rented relay, from the admin guide. Decision 22 planned a rented relay for links. Getting through carrier-grade NAT always needs something with a public address in the middle, so this decision settles whether Accordion should provide one.

## Decision drivers

1. **Privacy.** No company, including Accordion's developers, sees who connects to a household's server or when.
2. **Ownership.** The household runs everything Accordion needs. Nothing depends on a server or account someone else controls.
3. **Nothing else to install or sign up for.** Someone runs the Accordion installer, enters their details, and starts syncing. Family members install Accordion's app and nothing else.
4. **No extra cost for owners** beyond their own hardware and internet.
5. **Removing a device cuts it off at once,** in Accordion, with nothing to remove anywhere else.
6. **One developer.**

## Options

### Option A: A relay the household rents, set up by Accordion

The admin pastes an API token from a hosting company. Accordion creates a small virtual server there, installs a relay on it, and keeps it updated. The relay forwards the tunnel's encrypted packets, so it can't read anything.

Pros: the household controls the relay, and no one but its hosting company sees anything.

Cons:

- **An account and a monthly bill** at a hosting company, about 5 USD a month. That breaks drivers 3 and 4.
- **The hosting company sees** which devices connect and when.
- **A second machine** for Accordion to create, update, and support at each hosting company.

### Option B: Relays run by Accordion's developers

Accordion's developers run a few public relays. A home behind carrier-grade NAT keeps a connection open to one, and devices reach home through it. The relay introduces both sides so they can try a direct path through their NATs (hole punching), and carries the traffic when that fails.

Pros: nothing for the household to do or pay.

Cons:

- **Accordion's developers see** which homes and devices connect, from which addresses, when, and how much they send, though never what. That breaks driver 1.
- **Every household behind carrier-grade NAT depends on the developers' servers,** which breaks driver 2.
- **A monthly cost and an ongoing job** for one developer, with relays near each region to avoid slow detours.

### Option C: Tailscale built into the server and the apps

The server and every app run Tailscale's client library. Tailscale's servers introduce them and relay when they must.

Pros: mature NAT traversal, and free relays.

Cons:

- **A Tailscale account for every household,** which breaks drivers 2 and 3.
- **Tailscale sees** every device and when it connects, which breaks driver 1.
- **Removing a device in Accordion doesn't remove it from the tailnet,** which breaks driver 5.
- **A company's free plan,** which can change, under every household's remote access.
- **A Go library in a Rust project,** run as a helper on every platform, phones included.

### Option D: Direct connections only

Accordion keeps decision 21's approach: the router opens the port, the admin forwards it by hand, or the home uses IPv6. A home behind carrier-grade NAT without IPv6 gets no remote access, and Accordion says so plainly.

Pros:

- **Meets every driver.** Nothing in the middle, nobody watching, nothing to sign up for, and no cost.
- **Nothing new to build or run.**

Cons:

- **Households behind carrier-grade NAT without IPv6** can use Accordion only at home, until their provider gives them a public address or IPv6. Some providers do that on request, and some charge for it.

## Comparison

| Driver | A: Household rents | B: Developers' relays | C: Tailscale | D: Direct only |
|---|---|---|---|---|
| Privacy | The hosting company sees connections | The developers see connections | Tailscale sees connections | Yes |
| Ownership | Yes | No | No | Yes |
| Nothing else to install or sign up for | No | Yes | No | Yes |
| No extra cost for owners | No, about 5 USD a month | Yes | Yes | Yes |
| Removing a device cuts it off | Yes | Yes | No | Yes |
| Works behind carrier-grade NAT | Yes | Yes | Yes | Only with IPv6 |
| Work for one developer | Medium | Medium, plus relays to run | Large | None |

## Decision

**Direct connections only (option D).** Accordion doesn't provide, recommend, or depend on a relay, a tailnet, or a tunnel company.

Privacy and ownership are Accordion's core goals. Every way through carrier-grade NAT puts someone else's server in the middle, which costs one or both of them, or adds setup and a bill for the owner. A household behind carrier-grade NAT keeps everything else Accordion does at home, and gets remote access when its connection allows it.

### What households see

- **The Remote access page says plainly** when the home is behind carrier-grade NAT: devices away from home can't reach the server through this internet connection. It says the fix is a public address or IPv6 from the internet provider, and that some providers do that on request.
- **It doesn't suggest** a tailnet, a rented relay, or a tunnel company.
- **It doesn't read like success** when the router opened the port but the home's address is shared. Today's page says "Your router forwards UDP port 51820" before saying no device can reach it.
- **Link sharing** says the same: links can't reach people outside the home through this connection ([decision 22](0022-link-sharing.md)).
- **Remote access works as soon as the connection allows it.** When the provider gives a public address or IPv6, the router check finds it, with nothing to change in Accordion.

### Revisit this decision if

- A way through carrier-grade NAT appears that needs no server in the middle, no account, and no cost, such as IPv6 becoming common on Australian home and mobile networks.

## Consequences

- **Decision 21's admin guide** no longer covers a tailnet or a rented relay as alternatives.
- **Decision 22's planned relay for links** is dropped. Links behind carrier-grade NAT stay unavailable.
- **The Remote access page and Server health** change their carrier-grade NAT message as described above.
- **Testing remote access and links on a real router** waits for a connection with a public address or IPv6, such as the developer's next home.
