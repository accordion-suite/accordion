# 3. How sign-in sessions and tokens work

- **Status:** Accepted
- **Date:** October 15, 2026

## Context

Identity in `accordion-core` can create accounts and check passwords. Next it needs to remember a sign-in: give the client something to send with each request, check it, and end it when the user signs out. Every kind of client needs this:

- **The web app,** in a browser at the server's address.
- **The desktop and mobile apps,** which sync in the background and can be offline for weeks.
- **Standard clients** such as Thunderbird, iOS Mail, DAVx5, and WebDAV mounts. IMAP, SMTP, WebDAV, CalDAV, and CardDAV support only a username and password.
- **Third-party apps** that sign in through Accordion's built-in OpenID Connect (OIDC) provider.
- **The mail engine,** a separate process ([decision 1](0001-where-core-services-run.md)) that checks IMAP and SMTP sign-ins.

Decision 1 lists token verification as a library, "so any component can check a sign-in without asking another process", and `accordion-auth` was created for it. That was written while separate processes per module were still an option. With one server process, the only component outside the server that checks sign-ins is the mail engine. Sandboxed workers get their input from the server and never see a user's credentials.

Some parts are fixed whatever the choice:

- **OIDC ID tokens are signed JWTs.** The standard requires them, and OIDC Core requires providers to support RS256 signatures.
- **Standard clients use a password,** because their protocols allow nothing else. Which password is covered below.
- **Passkeys and TOTP** (from the spec) apply to every client that can show a sign-in page.

Out of scope: sign-in throttling and lockout, how the OIDC provider is built, and per-user encryption keys. Each needs its own design.

## Decision drivers

1. **One developer.** Every flow, key, and client-side token handler is maintained by one person.
2. **Revocation.** The spec puts compromised devices in scope. Signing a device out, from the device list or by an admin, must cut it off within seconds, including connections that are already open.
3. **Offline clients.** Desktop and mobile apps must resume after weeks offline without a new sign-in.
4. **Family members rarely sign in.** On their own devices they stay signed in, and they never see a sign-in expire in the middle of a task.
5. **Passkeys and TOTP everywhere** a sign-in page can be shown, including the first-party apps.
6. **One server process and one SQLite writer** ([decision 2](0002-metadata-database.md)). Checking a sign-in mustn't write to the database on every request.
7. **Raspberry Pi performance.** WebDAV clients send many requests in a burst, and each one carries credentials.

## Options

### Option 1: Server-side sessions with opaque tokens

Signing in creates a row in a sessions table and gives the client a random token. The server stores only a hash of it. Each request looks up the hash, and signing out deletes the row.

Pros:

- **Revocation is immediate.** Deleting the row ends the session on its next request. Because one process holds every open connection, it can also close the session's WebSockets and push connections at once.
- **Little to build.** There are no signing keys to generate, store, or rotate, no refresh flow in every client, and no clock differences to handle.
- **Offline clients resume as they are.** A token stays valid while its session exists, so a phone that comes back after a month sends the same token.
- **The device list comes for free.** The sessions table is the list of signed-in devices that users and admins see.
- **A leaked database doesn't leak sign-ins.** Only hashes are stored.
- **Checks are cheap.** A check is one indexed read on an in-process SQLite connection, a few microseconds (an estimate to confirm).

Cons:

- **Only the server can check a token.** Anything outside it has to ask the server. Today that's only the mail engine.
- **Moving a module into its own process,** which decision 1 keeps possible, would mean that module asks core about each request, or core starts issuing signed tokens at that point.
- **Every request reads the database.** Reads never wait on the writer, but it's a cost that signed tokens don't have.
- **Recording when each session was last used** would be a write per request, so those updates need throttling.

### Option 2: Short-lived signed tokens with refresh tokens

The server issues signed access tokens (JWT or PASETO) that expire after 5–15 minutes, plus a long-lived refresh token stored on the server. Any component with the public key can check an access token without asking the server. Clients trade the refresh token for a new access token before the old one expires. This is the usual OAuth 2.0 design.

Pros:

- **Any process can check a sign-in on its own,** which is what `accordion-auth` was created for. A module that moves into its own process later needs nothing new.
- **No database read per request** for access tokens.
- **Shared machinery.** The OIDC provider needs signing keys and JWTs for ID tokens anyway.

Cons:

- **Revocation waits for expiry.** A signed-out device keeps working until its access token runs out. Making it immediate means checking a revocation list on every request, which brings back the database read. Open WebSockets need tracking separately either way.
- **Refresh logic in every client.** The web app, desktop app, both mobile apps, and the sync engine all need it, including when several requests run at once. Rotating refresh tokens, the recommended practice, misfires when two requests refresh at the same moment or a response is lost on a flaky connection, and the user is signed out.
- **Key management.** Signing keys have to be generated, stored, rotated, and backed up, and clocks on different devices disagree.
- **Browsers still need a safe place for the refresh token.** In a cookie it's a server-side session in all but name. In JavaScript storage, any cross-site scripting bug can steal it.
- **The main benefit doesn't apply yet.** With one server process, no other process needs to check tokens on its own.

### Not considered: long-lived signed tokens

Signed tokens that last weeks would need a revocation list checked on every request to meet driver 2. That's option 1 with signing added.

## Comparison

| Driver | Option 1: server-side sessions | Option 2: signed tokens with refresh |
|---|---|---|
| Work for one developer | Low | High: keys, rotation, and refresh in every client |
| Revocation | Immediate, including open connections | After the access token expires, 5–15 minutes |
| Offline clients | Resume with the same token | Refresh on reconnect, and rotation can sign them out |
| Family members stay signed in | Yes | Yes, if refresh works in every client |
| Database work | One read per request | A write per refresh |
| Checks outside the server | Must ask the server | On their own |
| Fit with one server process | Ideal | Main benefit unused |

## Recommendation

**Option 1, server-side sessions with opaque tokens,** for everything Accordion issues to its own clients. Signed JWTs are used only where OIDC requires them, for ID tokens given to third-party apps.

What option 2 adds is checking a sign-in without asking the server. With one server process, nothing needs that. The mail engine is the only outside checker, and asking the server is a small part of the cost of a network sign-in. Option 1 meets the revocation driver fully and is far less work.

It's also easy to reverse. If a module moves into its own process, core can issue short-lived signed tokens for that module alone, backed by the same sessions.

### Credentials

| Client | Credential | How it signs in | Ends |
|---|---|---|---|
| Web app | Session token in a cookie | Sign-in page: passkey, or password plus TOTP if it's turned on | After 30 days without use, or on sign-out |
| Desktop and mobile apps | Session token, sent as a bearer token | The same sign-in page in the system browser, through OAuth 2.0 with PKCE | After 180 days without use, or on sign-out |
| Standard clients | App password | Username and app password | When the user or an admin deletes it |
| Third-party apps | OIDC ID token, plus opaque access and refresh tokens | The sign-in page, through the OIDC provider | Access token after 1 hour, refresh token when the user disconnects the app |

The lifetimes are starting values, to confirm with the beta households.

### Design details

- **Tokens.** Each token is 256 random bits from the OS, encoded as base64url, with a short prefix that names its kind, such as `acs_` for sessions. The prefix makes a leaked token recognizable in logs and to secret scanners. The database stores a SHA-256 hash. A fast hash is safe here because tokens are random, unlike passwords, and it keeps checks cheap.
- **Browser cookies.** The cookie is named `__Host-accordion-session` and set with `Secure`, `HttpOnly`, and `SameSite=Lax`. The `__Host-` prefix stops other sites and subdomains from setting it. Requests that change state, and WebSocket upgrades, must carry a matching `Origin` header, which blocks cross-site requests.
- **Native apps sign in through the system browser** ([RFC 8252](https://www.rfc-editor.org/rfc/rfc8252)). On iOS, an app can use passkeys only for domains listed in the app when it's built, and every Accordion server has its own domain. Signing in through the system browser makes passkeys work in every app, and there's only one sign-in page to build. Each app is an OAuth public client of the built-in provider, and the token endpoint returns a session token, which the app keeps in the OS keychain.
- **Standard clients use app passwords only.** IMAP, SMTP, WebDAV, CalDAV, and CardDAV never accept the account password. These protocols send the password with every request and can't ask for a passkey or a TOTP code, so the account password would bypass two-step sign-in. Each app password:
  - Is 16 random lowercase letters (about 75 bits), so it can be typed on a phone keyboard without symbols.
  - Belongs to one device, and the device list shows when it was last used.
  - Works only for mail (IMAP and SMTP), or only for files, calendars, and contacts (WebDAV, CalDAV, and CardDAV).
  - Is stored as a SHA-256 hash, so a burst of WebDAV requests doesn't run Argon2 on every request.
- **Re-authentication for sensitive actions.** Changing a password, adding or removing a passkey, creating an app password, and admin actions need a sign-in from the last 15 minutes. Otherwise Accordion asks for a passkey or password again. Each session records when and how the user signed in.
- **Revocation.** The device list shows each session and app password with its device name and when it was last used. Signing out a device deletes its row and closes its open connections. Changing the password signs out every other session. **Sign out everywhere** also deletes app passwords. Admins can do the same for any user.
- **Throttled writes.** A session's last-used time is updated at most once an hour, so checking a session stays a read.
- **The mail engine asks the server.** It checks app passwords, and session tokens from the mobile app's JMAP connection, through an endpoint on a local Unix socket. The endpoint follows OAuth token introspection ([RFC 7662](https://www.rfc-editor.org/rfc/rfc7662)) where it fits, because both candidate engines can check tokens that way. How the engine checks app passwords is part of the mail engine choice.
- **Audit log.** Every sign-in, sign-out, and new app password goes to the user's audit log, as the spec requires.

### Revisit this decision if

- A module moves into its own process.
- The gateway or another separate process needs to check sign-ins on its own.
- Profiling on the reference hardware shows session checks as a noticeable cost.

## Consequences

- **`accordion-auth` is removed.** Session checks live in identity in `accordion-core`. This replaces decision 1's point that token verification is a library, and the rest of decision 1 stands. Remove the crate and its row in the `accordion-core` README.
- **New identity tables:** `identity_sessions` and `identity_app_passwords`.
- **The OIDC provider's authorization and token endpoints ship with the desktop app in alpha,** because the first-party apps sign in through them. That's earlier than third-party sign-in alone would need.
- **The server keeps a list of open connections for each session,** so revocation can close them.
- **Standard clients need an app password,** which is one more step for family members. Setup helpers can carry it for them, such as an Apple configuration profile for iOS and macOS Mail. These are planned with Mail.
- **Spec update when accepted:** state that standard clients use app passwords only, never the account password.

## Open questions

- **The setup wizard runs before TLS exists.** The spec puts it at `http://accordion.local`, where a `Secure` cookie can't be set and anyone on the network could reach the wizard first. The wizard needs its own protection, such as a one-time setup code, and a move to HTTPS before the owner's first session. This belongs with the installer and gateway design.
- **Per-user encryption keys.** If a user's password unlocks their data key, the server needs that key while the user is signed in, so sessions would have to carry it. That's a separate decision, and it may add to this one.
