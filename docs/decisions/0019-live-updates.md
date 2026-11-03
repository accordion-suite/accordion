# 19. Live updates

- **Status:** Accepted
- **Date:** November 3, 2026

## Context

The server can't tell a client that something changed, so clients ask again on timers:

- **The bell** checks for new notifications every 60 seconds while the web app is open ([decision 18](0018-the-event-log.md)). A folder shared with you can take a minute to appear.
- **Thumbnails:** while a new file has no thumbnail, Drive's page loads its list again every 2–15 seconds, for up to three minutes ([decision 14](0014-thumbnails-and-sandboxed-workers.md)).
- **Folders don't update at all.** When Sam adds photos to a shared folder, someone with the folder open sees them only after they reload or open it again. The same goes for a second tab, or a file saved over WebDAV.

Planned work needs the same thing:

- **The desktop sync client** needs to know when another device changed something, so it can sync right away instead of checking every few minutes.
- **Mail** in the web app needs to show new mail as it arrives.
- **Docs** needs real-time co-editing with presence cursors. The spec plans Yjs over WebSocket for it.
- **Notes** shared between people need to show each other's edits.

Some pieces exist already:

- **The gateway** serves HTTP/2 and HTTP/1.1, and passes WebSocket upgrades through. When the server stops, it gives open connections 10 seconds to finish, then closes them.
- **[Decision 3](0003-sessions-and-tokens.md)** chose server-side sessions partly so that signing out could close a session's WebSockets and push connections at once, since one process holds them all.
- **[Decision 18](0018-the-event-log.md)** said the event log is what a push channel would send.

## Decision drivers

1. **Nothing leaks.** Nobody learns about an item through a live update that they couldn't already open, as with the event log.
2. **Nothing leaves the box.** No push service run by someone else, in line with the spec's zero-telemetry rules.
3. **Signing out ends it.** A device that's signed out stops hearing about changes at once.
4. **A missed update is never a wrong screen.** Connections drop, phones sleep, and servers restart for updates. Clients must end up showing the truth afterward.
5. **One channel for every module,** as decision 18 made one log.
6. **Small on a Raspberry Pi,** with every family member's devices and tabs connected.
7. **One developer.**

## Options: how updates reach clients

### Option A: Server-sent events

The client opens one long-lived `GET` request, and the server writes messages into the response as things happen. Browsers have this built in as `EventSource`.

Pros:

- **Ordinary HTTP.** It passes through the gateway, its host check, and the session cookie like any other request. Native apps read it with any HTTP client.
- **Browsers reconnect on their own,** after a dropped connection or a server restart.
- **Cheap over HTTP/2.** Every tab's stream shares one connection to the server.
- **Little to build.** Axum, which the server already uses, has it built in.

Cons:

- **One way only.** Clients still send changes as ordinary requests, which is how they work today.
- **Over HTTP/1.1, each stream holds one of the browser's six connections to the server.** Accordion always serves HTTPS with HTTP/2, so this only affects the development server and clients that can't use HTTP/2.
- **Text only,** which rules it out for Yjs's binary updates.

### Option B: WebSocket

The client upgrades a request into a two-way connection, and both sides send messages whenever they like.

Pros:

- **Two-way and binary,** which Docs' co-editing needs.
- **The gateway passes upgrades through already.**

Cons:

- **More to build in every client:** reconnecting, backing off, and checking the connection is still alive.
- **Its own path through HTTP/2.** Browsers open a separate connection for each WebSocket unless the server supports WebSockets over HTTP/2, which needs settings turned on in the gateway.
- **Needs a matching `Origin` check on the upgrade,** as decision 3 requires, since the same-origin rules that protect ordinary requests don't apply to WebSockets.
- **More than live updates need.** The server would talk and the client would only listen.

### Option C: Long polling

The client sends a request that the server holds open until something happens, answers, and the client sends the next one.

Pros:

- **Works through anything** that passes ordinary requests.

Cons:

- **A new request after every message,** and messages in between can be lost unless the server keeps a backlog for each client.
- **Everything option A does, built by hand.**

### Option D: Push services

Web Push in browsers, and Apple's and Google's push services for phone apps. The server sends each message to the browser's or phone maker's service, which delivers it.

Pros:

- **Reaches a device whose app is closed,** which nothing else can do on iPhones.

Cons:

- **Every message goes through a company's servers.** Even encrypted, they learn when and how often someone hears from the server. That breaks driver 2.
- **The server needs outbound connections** that would join the Network page, and each browser maker's service is different.

## Options: what's sent

### Option 1: Hints

Each message says only what to look at again, such as "the list of this folder changed" or "your notifications changed". The client loads it through the same addresses it uses now, which check access as they always do.

Pros:

- **Can't show more than the client could already load.** The addresses that check access stay the only way to see anything.
- **Missed messages don't matter.** After a reconnect, the client loads what's on screen again, and it's up to date.
- **Works for changes that aren't events,** such as a thumbnail being ready, or a notification read in another tab.
- **Small.** A hint is an ID or two.

Cons:

- **An extra request after each hint.** Hints that arrive close together are combined, so an upload of 212 files is one reload, not 212.

### Option 2: The changes themselves

Each message carries the change, such as the renamed item or the new notification, ready to show.

Pros:

- **No extra request.**

Cons:

- **Every message is checked and written for each reader,** with the names they're allowed to see, which duplicates every list and activity address.
- **Missed messages leave the screen wrong,** so the server has to keep a backlog for each client, or the client reloads after reconnecting anyway.
- **A second way to see data,** so a bug in it could leak what the addresses would hide.

## Comparison

| Driver | A: Server-sent events | B: WebSocket | C: Long polling | D: Push services |
|---|---|---|---|---|
| Nothing leaves the box | Yes | Yes | Yes | No |
| Signing out ends it | Yes | Yes | Yes | Needs each device unregistered |
| Recovers after a drop | Browsers reconnect | Built by hand | Built by hand | The service retries |
| Small on a Raspberry Pi | About 22 KB a stream | Similar | A request after each message | Small, sent elsewhere |
| Work for one developer | Small | Medium | Medium | Large |

| Driver | 1: Hints | 2: The changes |
|---|---|---|
| Nothing leaks | Access checked by the existing addresses | Checked again for each message |
| A missed update is never a wrong screen | Reload after reconnecting | Needs a backlog |
| Works for changes that aren't events | Yes | Only with a second message format |

## Measurements

On the development machine, with a small Axum server that sends each message to every open stream, over plain HTTP/1.1 without TLS. A Python script on the same machine opened the streams and timed each message until the last stream received it, so the times include the script's own overhead.

| Open streams | Server memory added | For each stream | Time to reach every stream |
|---|---|---|---|
| 10 | 0.5 MB | 51 KB | 1.2 ms |
| 100 | 2.3 MB | 24 KB | 2.3 ms |
| 1,000 | 21.3 MB | 22 KB | 17 ms |

A family of five with three devices each, and a couple of tabs open on some, has about 30 streams: under 2 MB. TLS adds buffers for each connection, but over HTTP/2 a browser's tabs share one. A Raspberry Pi 5 is slower, so these figures join the Raspberry Pi measurements already planned.

## Recommendation

**Send hints over server-sent events (options A and 1), through one stream for every module. Docs' co-editing gets its own WebSocket for each open document when Docs is built.**

### The stream

- **One address, `GET /api/live`,** answered with `text/event-stream`. The web app opens one stream in each tab when someone signs in. The desktop and phone apps open one while they're running, with their app password or token as for any other request.
- **Signing in is checked when the stream opens,** as for any request. The stream belongs to that session.
- **A `ping` event every 30 seconds** keeps the connection from looking idle to routers and the VPN, and lets the client notice a stream that has stopped. It's an event rather than a comment line, since browsers don't show comments to the page. If nothing arrives for 75 seconds, the client closes it and opens a new one.
- **On reconnecting, the client loads again what it's showing:** the open folder, the bell's count, and so on. This is what makes missed hints harmless.
- **Each stream says how long to wait before reconnecting,** between one and five seconds, chosen at random, so clients don't all return at the same moment after an update restarts the server.
- **When the server stops,** it ends every stream at once instead of waiting for the gateway's 10 seconds, so clients reconnect to the new server as soon as it's up.
- **A cap of 20 streams for each person,** so a broken client can't fill the server's memory. Opening one more closes their oldest.
- **The response turns off buffering and caching,** with `Cache-Control: no-cache` and `X-Accel-Buffering: no`, so a reverse proxy in front of Accordion passes messages on at once.

### What's sent

Each message has a kind, named after its module, and a little JSON:

| Kind | Data | The web app |
|---|---|---|
| `drive.items` | The folders whose lists changed | Loads the folder again if it's open, and its **Activity** tab |
| `drive.shared` | Nothing | Loads **Shared with me** again if it's open |
| `notifications` | Nothing | Loads the bell's count and list again |

- **Hints are combined for half a second.** Everything for one stream in that time goes as one message, with each folder named once.
- **Moves name both folders,** each to the people who can open it.
- **Sharing sends `drive.shared` to the person whose access changed,** including when it's removed, so their **Shared with me** updates.
- **A thumbnail being ready** sends `drive.items` for the folders holding files with those contents, which replaces the thumbnail timer.
- **Seeing, reading, removing, or clearing notifications** sends `notifications` to that person's streams, so the bell matches in every tab.
- **Later modules add their own kinds,** such as `mail.mailbox` for a mailbox with new mail, or `notes.note` for a shared note someone edited.

### Who hears what

- **Core works out who can open an item when it sends the hint,** from the item's owner and the people it, or a folder above it, is shared with. It's the same rule as the event log's, run once for each hint rather than once for each stream.
- **A hint goes only to streams of people who can open the item now.** Someone who lost access hears `drive.shared` and nothing more.
- **A hint names only IDs, never names or contents.** Even so, it goes only to people who could load what it points to.

### How it's built

- **Core keeps the list of open streams,** in memory, with each stream's person and session.
- **Core sends hints after a change commits,** never from inside its transaction, so a change that's rolled back sends nothing. Storage sends them for its own changes, and the event log sends `notifications` when it adds a notification.
- **Modules send their own hints through core's API,** with a new `live` field in `Services`, saying which item or which people they're for. Like the event log, a module sends its hint right after its own change.
- **Ending a session closes its streams,** whether by signing out, ending it from the device list, a password change, or deleting the account, as decision 3 planned.
- **Slow streams are dropped.** Each stream holds at most 64 waiting messages. A client that can't keep up is disconnected, and reconnects and reloads, so the server's memory stays bounded.
- **The server's `/api/live` handler** sits in the `accordion` repo with the other account addresses, so modules don't each open a stream.

### Timers

- **The bell's 60-second check and the thumbnail timer go.** The stream replaces both.
- **The 75-second silence check stays,** so a network that holds messages back still gets reloads now and then rather than never.
- **The web app still loads what's on screen when the page comes back into view,** since phones often close connections while a page is hidden.

### Co-editing in Docs

Yjs needs a two-way channel that carries binary updates for one document, many times a second. That doesn't fit a stream of hints, so Docs opens a WebSocket for each open document, under the same rules:

- The session is checked on the upgrade, with a matching `Origin` header.
- The document is checked with the same access rule as its file in Drive.
- Ending the session closes it, through the same list of open connections in core.

Docs' own decision covers its messages, and whether to turn on WebSockets over HTTP/2 in the gateway.

### Apps that are closed

The stream reaches only apps that are running. Notifications on a phone whose app is closed need Apple's or Google's push services, or on Android, a self-hosted option such as UnifiedPush. That's for the phone apps' decision, which must weigh what those services learn.

### Testing

- **Core's tests** check that:
  - each change to storage sends the right hint after it commits, and a failed change sends none
  - a hint reaches only streams of people who can open the item, including through a shared folder above it
  - ending a session closes its streams
  - a stream that falls behind is dropped, and the cap closes the oldest
- **The server's tests** check that `/api/live` needs signing in, sends hints, and ends when the session is signed out.
- **The web app's tests** cover combining hints, reloading after a reconnect, and the silence check.

### Revisit this decision if

- Clients that can't use HTTP/2 turn out to be common, and run out of connections. Tabs could then share one stream between them.
- Phones or the desktop app need hints while closed or asleep.
- Hints cause more reloads than a Raspberry Pi can serve, such as a folder of thousands of files reloaded after each upload. Hints could then name the items that changed, for clients to load one at a time.

## Consequences

- **Core** gains the list of open streams, sending hints after storage and event log changes, working out who can open an item, the `live` field in `Services`, and closing streams when a session ends.
- **Drive** sends hints for its own changes, such as descriptions, and for thumbnails.
- **The server** gains `/api/live`, and ends every stream when it stops.
- **The web app** opens the stream, reloads what's on screen for each hint and after each reconnect, and drops the bell's and thumbnails' timers.
- **The gateway** is unchanged. Docs may later turn on WebSockets over HTTP/2 in it.
- **Decision 18** said the push channel would send events from the log. It sends hints instead, some of them caused by events.

## Open questions

- **Sharing one stream between tabs,** if many tabs on HTTP/1.1 turn out to be a problem.
- **A count on the installed app's icon,** with the Badging API, which only works while the app is running.
- **How the desktop sync client uses the stream,** which the sync decision will settle: a hint could start a sync, with the sync protocol's own list of changes saying what to fetch.
