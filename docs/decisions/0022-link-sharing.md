# 22. Link sharing

- **Status:** Accepted
- **Date:** November 4, 2026

## Context

People often need to share a file with someone who isn't on the server: holiday photos for grandparents, a tax return for the accountant, a lease for the landlord, a video too big for email. In Google Drive, that's "Anyone with the link". Without it, people email attachments, which fail above about 25 MB, or copy the file into Google Drive or WeTransfer, which hands it back to the companies Accordion replaces.

[Decision 16](0016-sharing.md) designed the link itself and left one thing for this decision:

- **A link gives view-only access** to a file or folder. It can have a password, an expiry date, and a download limit, as the spec asks.
- **Its address carries a random 128-bit token,** stored only as a hash. Password guesses are rate-limited. The admin can turn links off for the whole server.
- **People who open a link get no account.** The spec's guest role stays for later.
- **Links reach only people who can reach the server.** Today that's the home network and remote access ([decision 21](0021-remote-access.md)), which only enrolled devices can use. Reaching anyone else needs the spec's "optional public sharing endpoint for link shares only, with everything else still VPN-only". Decision 21 left that open, because it answers on the internet and needs its own rules.

This is the first time Accordion would answer someone it doesn't know, on the internet. Everything else needs a sign-in, a device key, or the home network.

Some pieces exist already:

- **Port mapping** (decision 21) asks the router to forward a UDP port through PCP, NAT-PMP, or UPnP. Forwarding a TCP port works the same way.
- **The address record** (decision 21) keeps a random label of the server's name pointed at the home's public address.
- **Certificates** come from Let's Encrypt through the DNS challenge ([decision 5](0005-server-name-and-certificates.md)). Let's Encrypt issues wildcard certificates only through that challenge, which Accordion already uses.
- **Drive** already checks access by walking up the folder tree, makes thumbnails and previews, and streams folders as ZIP files.
- **The event log** ([decision 18](0018-the-event-log.md)) and live updates ([decision 19](0019-live-updates.md)) can tell an owner when a link is opened.

## Decision drivers

1. **Nothing else is exposed.** The internet reaches link pages and nothing else: no sign-in, no admin pages, no API, and no WebDAV.
2. **Nothing leaves the box.** No company sees the shared files or who opens them.
3. **It works for the person opening the link** on any phone or computer, with nothing to install and no account.
4. **Off until an admin turns it on.** Like remote access, it opens a port, so it's a choice.
5. **Links that are safe to forget.** They expire by default, and the owner sees and removes them.
6. **A shared file can't attack the person who opens it,** or the household through the server's own name.
7. **Works behind carrier-grade NAT,** or says plainly that it can't.
8. **One developer.**

## Options: how people outside reach a link

### Option A: The router forwards a TCP port to the server

Accordion asks the router to forward TCP port 443 to a port the server listens on for links only, the way it asks for remote access's UDP port. Links use a name that points at the home's public address.

Pros:

- **No other company** sees the files or who opens them.
- **Nothing to rent or run.**
- **The same port mapping** as remote access, with the same fallback to forwarding by hand.

Cons:

- **The server answers anyone on the internet** on that port, including scanners, so the link server's code is the attack surface.
- **No help with carrier-grade NAT.** IPv6 only helps the people opening links who have it, and many don't.
- **Some providers block incoming port 443,** so links would need another port, such as `https://f8q2m.ourhome.dedyn.io:8443/`.
- **The home's public address** is in every link's DNS, which anyone with a link can look up.

### Option B: A relay the household rents

A small rented server, such as a 3-dollar-a-month virtual server, answers on port 443 and passes each connection, still encrypted, to the home server over a WireGuard tunnel. The rented server reads only the name in the TLS handshake (SNI), to check that it's the links name, and never has the certificate's key.

Pros:

- **Works behind carrier-grade NAT,** and with no router changes.
- **The home's address stays private.** Link names point at the relay.
- **The relay can't read the files,** since TLS ends at home.

Cons:

- **Another machine to rent, set up, and keep updated,** which most households won't do.
- **The hosting company sees who connects and when,** though not what they get.
- **More to build:** a relay mode, its setup, and its own updates.

### Option C: A tunnel company

Cloudflare Tunnel or Tailscale Funnel carries connections to the server with no open port.

Pros: works behind carrier-grade NAT, with nothing to rent, and is often free.

Cons:

- **Cloudflare Tunnel ends TLS at Cloudflare,** so Cloudflare can see every shared file. That breaks driver 2.
- **Tailscale Funnel passes TLS through,** but Tailscale sees who connects and when, and each family member's server needs a Tailscale account.
- **Accordion would depend on a company's free tier** that can change.

### Option D: Copy the file somewhere public

Sharing uploads an encrypted copy to a bucket, such as one of the backup locations, and the link points there.

Pros: the home server stays off the internet.

Cons:

- **The files leave the box,** to a company that serves them to anyone who asks.
- **Folders, previews, revoking, and download limits** all get much harder, since the bucket serves the files.
- **Every share costs storage and upload time** before the link works.

## Options: what serves link pages

### Option 1: The main HTTPS server, by name

The server's existing HTTPS listener also answers the links name, and routes requests by host name.

Pros: the least code. Cons: one routing mistake puts an admin page or an API on the internet, since every route sits in the same router. The main listener also counts private addresses as the home network, so a request that a router passes through from the internet can look local.

### Option 2: Its own listener and router, in the server

The server listens on a second port for links only. The router behind it is built from Drive's link handlers and nothing else, so other routes aren't there to reach.

Pros:

- **Other routes don't exist on that port,** which a test can check by listing them.
- **Its own rules:** every request counts as coming from the internet, whatever address it comes from. It has its own rate limits and headers.
- **Little extra code,** with the gateway's TLS server and Drive's code as they are.

Cons: a bug in the link handlers runs in the server process, which can read every file.

### Option 3: Its own sandboxed process

A separate process, shut in like the thumbnail workers ([decision 14](0014-thumbnails-and-sandboxed-workers.md)), serves link pages. It asks the server for each link's files over a narrow local API.

Pros: a compromised link server reaches only what the narrow API gives it.

Cons:

- **A long-running network process in the sandbox,** which decision 14's sandbox wasn't built for.
- **Every file is copied across a socket,** which slows big downloads on a Raspberry Pi.
- **The API still has to check tokens and rate-limit,** so the logic that matters stays in the server.

## Options: the links name

### Option I: The server's own name

Links use `https://ourhome.dedyn.io/s/...`. Cons: the server's name points at its home address, which nobody outside can reach, and changing that would break decision 21's "the same name everywhere".

### Option II: A random label, with a wildcard certificate

Links use a second random label of the server's name, such as `https://f8q2m.ourhome.dedyn.io/s/...`, with its own address record. A wildcard certificate for `*.ourhome.dedyn.io` covers it.

Pros:

- **A different origin from the web app,** so a shared file the browser opens can't read the household's cookies or call Accordion's API, and the web app's pages can't be framed by it.
- **Kept out of certificate logs,** which list only the wildcard. Scanners that watch those logs for new names to attack don't learn this one.
- **Can be changed** if links are abused, without touching the server's name or remote access.

Cons: anyone with a link learns the name, and with it the home's public address. Changing the name breaks every link.

### Option III: Remote access's label

Links use the name devices find home at. Pros: one record. Cons: everyone with a link learns where remote access answers, and changing one breaks the other.

## Comparison

| Driver | A: Forward a port | B: Rented relay | C: Tunnel company | D: Copy out |
|---|---|---|---|---|
| Nothing leaves the box | Yes | Connection times only | No, or connection times | No |
| Works behind carrier-grade NAT | No | Yes | Yes | Yes |
| Nothing to rent or sign up for | Yes | No | Usually an account | A bucket |
| Work for one developer | Small | Medium | Small, but fragile | Large |

| Driver | 1: Main server | 2: Own listener | 3: Own process |
|---|---|---|---|
| Nothing else exposed | One mistake away | By construction | By construction |
| A link-handler bug reaches | Everything | Everything | The narrow API |
| Big downloads on a Pi | Fast | Fast | Slower |
| Work for one developer | Small | Small | Large |

## Recommendation

**The router forwards a TCP port (option A), to a listener and router that serve link pages and nothing else (option 2), under a random label with a wildcard certificate (option II).** A rented relay (option B) is planned for households behind carrier-grade NAT, and the admin guide will describe Tailscale Funnel (option C) with what it shares with Tailscale. Copying files out (option D) is rejected.

Option A keeps the files and who opens them inside the home, with no account anywhere. Option 2 makes "link pages and nothing else" a fact about the code rather than a rule every route has to follow, for little more work than option 1. Option 3's protection is real but costs a new kind of sandbox and slower downloads, and the checks that matter would still run in the server. It's the first thing to revisit if link pages grow, such as with uploads.

### Turning it on

1. **An admin turns on link sharing** in Settings, under **Remote access**. It's off until then, and it needs a name. It doesn't need remote access to be on.
2. **Accordion asks the router** to forward TCP port 443 to the link listener, as for remote access, and renews it. If the router says no, the page shows how to forward it by hand. If port 443 is taken or blocked, the admin picks another port, and links carry it.
3. **Accordion gets the wildcard certificate** through the DNS challenge, and points the links label at the home's public address, as decision 21's record does.
4. **Behind carrier-grade NAT,** the page says links can't reach people outside, and why. It offers the relay once it exists.
5. **Turning it off** closes the port, removes the record, and makes every link stop working at once. Links come back if it's turned on again, unless they've expired.

### The link listener

- **Its own port,** 8443 by default, on every address. Only the router's forward reaches it from outside.
- **Its own router,** built from Drive's link pages, the files they serve, and a few static files. No other module or server route is compiled into it, and a test lists its routes.
- **Only the links name.** Requests for any other host name get no answer, as decision 5's host check does on the main server.
- **Every request counts as the internet,** whatever its address, so nothing on this port gets the home network's allowances.
- **Rate limits:** per address, on opening links, password tries, and downloads at once. A link's password is limited per link too, so many addresses can't share the guessing.
- **Headers:** no caching by anyone else (`Cache-Control: private`), no search engines (`X-Robots-Tag: noindex`, and a `robots.txt` that says the same), no referrer, a strict content security policy, and no framing.

### What a link opens

- **A plain page,** made on the server, with no sign-in and no web app: the item's name, size, who shared it (the owner's name, which the share dialog shows), and a **Download** button.
- **Previews only for formats that can't run code:** the thumbnails and previews Drive already makes, shown as pictures and PDFs, and videos and audio the browser plays. Everything else, including HTML and SVG files, only downloads, with `Content-Disposition: attachment` and `X-Content-Type-Options: nosniff`.
- **For a folder:** its files and subfolders, with thumbnails, each downloadable, and all of it as one ZIP file. Nothing outside the shared folder can be named in a link's address, as Drive's checks already ensure.
- **The current version** of each file. Moving a file out of a shared folder, trashing it, or deleting it takes it out of the link.
- **A password,** when the link has one, on a page of its own, before anything about the item shows.
- **Expired, used up, removed, and wrong links** all get the same page, which says the link doesn't work and to ask whoever sent it. It doesn't say which, so nobody learns that a link existed.

### Making and managing links

- **The share dialog** gains **Copy link**. Each link has its own settings, so the owner can make one per person and remove one without the others:
  - **Expires:** in 1 day, 7 days, 30 days (the default), 1 year, or never.
  - **Password:** optional.
  - **Download limit:** optional.
  - **A note,** such as "For the accountant", which only the owner sees.
- **The owner and editors of an item can link it,** as decision 16 lets editors share. Viewers can't.
- **The item's details** list its links, with when each was last opened and how many times. **Shared by me** shows linked items too.
- **Admins see every link** in Settings, and can remove any, or turn link sharing off.
- **The event log** records links made, changed, and removed, and each opening as a count and a time. It doesn't keep the addresses of people who open links: those stay in memory, for rate limits.
- **The owner hears about an opening** in their notifications, at most once a day for each link, so a forwarded link shows up.

### Security

- **The certificate's key** is the wildcard's, so it can serve any name under the server's. The server keeps it as it keeps the main one, readable only by the accordion user.
- **A shared file can't reach the web app,** since it opens on the links name, a different origin with no cookies.
- **Tokens are 128 random bits,** as decision 16 says, so guessing one is hopeless. Rate limits are for passwords and load, not tokens.
- **The household's upload speed is the limit** on downloads. The listener allows a few downloads at once for each address, so one person downloading a big folder doesn't stop everyone else's links.
- **Turning link sharing off** is the emergency stop: the port closes at once.

### At home

- **Links use the home's public address,** so at home they work on routers that loop traffic back, which many do. The share dialog suggests checking a link from a phone on mobile data.
- **The household's own devices on remote access** reach links the same way, since the links name isn't one the tunnel carries.

### Testing

- **A test lists the link router's routes** and fails on any that isn't a link page.
- **Tests open links on the link listener over real TLS:** files, folders, previews, ZIP downloads, passwords, expiry, download limits, and removed links, each of which gets the same page.
- **Tests try to escape a shared folder** by naming items outside it, and by asking for the main server's routes and host name on the link port.
- **Tests check that previews never run code:** an HTML file and an SVG file only download, with the headers that say so.
- **Port mapping** for TCP joins the stand-in router tests.
- **The upgrade test** turns link sharing on, makes a link, and opens it on the link listener from the same machine.

### Revisit this decision if

- Many households can't open a port, because of carrier-grade NAT or blocked ports. The relay (option B) moves up.
- Link pages grow beyond viewing and downloading, such as with uploads or editing. Option 3 becomes worth its cost.
- Scanning or abuse of link names is a real problem.

## Consequences

- **Drive** gains links: their table, the link pages, the link router, and the share dialog's **Copy link**.
- **Core** gains a way for Drive to serve an item to a link: the same folder walk as decision 16's checks, without a person.
- **The gateway** gains the link listener's TLS, TCP port mapping, and a second certificate for the wildcard name. Its ACME client and name checks handle only the server's own name today. Both certificates prove themselves with the same `_acme-challenge` record, so their renewals take turns.
- **The server** gains the link sharing switch and port, starts the link listener while it's on, and gives it Drive's link router.
- **The web app** gains the link settings in the share dialog, links in an item's details and in **Shared by me**, the switch in **Remote access**, and the admin's list of links.
- **The Network page** lists the TCP port mapping and the links record.
- **Server health** gains a **Link sharing** check: whether it's on, whether the router forwards the port, the certificate, and when a link was last opened.
- **Docker and NAS installs** will publish the link port as well.
- **The spec's optional public sharing endpoint** is this listener. Remote access stays the only way to everything else.

## Open questions

- **Upload links,** for collecting files from people without accounts, such as photos from wedding guests: quotas, file size limits, and what an owner reviews before the files join their Drive.
- **The relay:** how it's set up, how it's updated, and whether it's a mode of the `accordion` program or its own small program.
- **Docs and Notes by link,** read-only, once those modules exist.
- **What the link page looks like on a phone,** with big photo folders.
