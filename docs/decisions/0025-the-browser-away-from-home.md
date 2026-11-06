# 25. The browser away from home

- **Status:** Accepted
- **Date:** November 6, 2026

## Context

Accordion's editors run only in the browser. Drive's pages do today, and Docs, Sheets, and Notes will too. There's no plan for downloadable versions of them, so a browser that can't reach the server shuts a person out of everything but file sync.

Away from home, a browser can reach the server only through the WireGuard app ([decision 21](0021-remote-access.md)). Accordion's own apps don't need it, because they carry their own tunnel ([decision 23](0023-sync.md)), but that tunnel carries only the app's requests. Decision 23 left the browser for later.

That's fine for someone who sets up their devices at home and then takes them on a train. It doesn't work for the household the spec has in mind: an admin who runs the server and family members who live elsewhere and aren't technical. For them, today's path is:

1. Visit the admin's home, or have a device that's already signed in, because every setup comes from **Settings > Apps and devices** on a signed-in device.
2. Install the WireGuard app and Accordion's app, and set up each one.
3. Remember to turn the WireGuard tunnel on whenever they want the web app, on Windows and Android.

Two problems hide in there:

- **The first setup.** Someone who has never signed in has nowhere to get a setup from. The welcome link an admin sends a new person only works where the server is reachable.
- **The second app.** The browser needs the WireGuard app even on a computer that already has Accordion's app and its tunnel.

## Decision drivers

1. **One install, one code.** A family member installs Accordion, pastes what the admin sent, and both their files and the web app work, from their own home.
2. **Nothing new on the internet.** Sign-in, the API, and WebDAV stay off the public address, as decisions 21 and 22 chose. Only shared-link pages are public.
3. **The same name everywhere.** The browser uses the server's real name, so the certificate, passkeys, and bookmarks work unchanged.
4. **The admin never holds a way into someone's account.** Setting up a device for someone mustn't let the admin sign in as them.
5. **No system VPN.** The reasons in decision 23 still hold: phones and computers allow one at a time, and work VPNs clash with it.
6. **One developer.** Reuse the tunnel, the setup codes, and the browser sign-in that already exist.

## Options: how the browser reaches the server

### Option A: The app routes the browser through its tunnel

Accordion's app runs a small proxy on the computer, on `127.0.0.1`. It tells Windows, through the standard proxy setup script setting (a PAC file), to send the server's name to that proxy and everything else direct. Edge, Chrome, and Firefox (which uses the system setting by default) all follow it.

The proxy accepts only `CONNECT` requests for the names of servers the app has accounts on, port 443. It carries each one the way the app carries its own requests: directly at home, through the account's tunnel away. The browser's TLS session runs from the browser to the server's gateway, so the proxy and the tunnel only ever see encrypted bytes.

The browser doesn't look the name up itself. It hands the name to the proxy. That matters at another home: the server's home address, such as `192.168.1.20`, may belong to someone else's printer there.

Pros: one app covers files and the browser, with no WireGuard app. Nothing new is public. The real name, certificate, and passkeys keep working. It reuses the tunnel and the way the app chooses between home and away.

Cons: it works only on computers with the app installed, not a borrowed PC or a Chromebook. Any program on the computer can reach the server through the proxy, as any program can through the WireGuard app today. A computer that already has a proxy setting, such as a work laptop, needs care (see below). macOS needs the same work when its app is built.

### Option B: The app opens the web app in its own window

The app shows the web app in a WebView2 window, the Edge engine built into Windows, with its requests sent through the tunnel.

Pros: it looks like a downloaded app. No system setting changes.

Cons: the person's own browser still can't reach the server, so links in emails and bookmarks don't work. Passkeys, downloads, printing, and tabs all behave differently in an embedded window. It's more to build and maintain than option A, and it needs doing again on macOS.

### Option C: The app sets up a system VPN

The app adds WireGuardNT, so every program reaches the server.

Pros: the browser and everything else work, with no proxy.

Cons: decision 23 already turned this down. Installing the driver needs admin rights, it takes the computer's one VPN, and it clashes with other networks that use the same private addresses.

### Option D: The web app on the public address

The links-only listener from decision 22 serves the whole web app. Anyone signs in from any device.

Pros: it works on every device, with nothing installed. Most of it is built: the public name, the certificate, the router port, and a public listener exist already.

Cons: sign-in and the whole API face the internet, which decisions 21 and 22 avoided on purpose. Every bug in any module becomes reachable from anywhere. It would need two-step sign-in or a passkey for every sign-in from outside, tight limits on attempts, and an admin switch that's off by default.

### Option E: Keep the WireGuard app

Leave the browser to the WireGuard app, and only fix the first setup.

Pros: nothing to build for the browser.

Cons: two installs and a tunnel to remember, which family members who aren't technical won't manage.

## Comparison

| Driver | A: Proxy in the app | B: App window | C: System VPN | D: Public web app | E: WireGuard app |
|---|---|---|---|---|---|
| One install, one code | Yes | Yes | Yes | Nothing to install | No |
| Nothing new on the internet | Yes | Yes | Yes | No | Yes |
| The same name everywhere | Yes | Yes | Yes | Yes | Yes |
| No system VPN | Yes | Yes | No | Yes | No |
| Works on any device | No | No | No | Yes | Where WireGuard installs |
| Work to build | Small to medium | Medium | Medium | Small, plus hardening | None |

## Options: the first setup

- **The admin sets up a device for someone (chosen).** Described below.
- **A longer-lived setup code made by the person.** It still needs the person to have signed in somewhere first, so it doesn't help a new person.
- **The welcome link reachable from the internet.** That puts a page on the public address that sets passwords, which driver 2 rules out.

## Recommendation

**Option A, with setups an admin makes for someone else.** Option D stays possible as a separate decision, behind an admin switch, for households that want any-device access and accept the risk.

### Setting up someone else's device

- **On the People page, an admin selects Set up a computer next to a person.** It shows a link to the installer and a setup to copy, with a QR code too. The admin sends both to the person however they like, such as by email or chat.
- **For someone new, it replaces the welcome link.** The setup carries the welcome token, so the person sets their password from their own home.
- **The setup holds no session.** It carries a temporary tunnel key, as decision 23's setups do, that reaches only the sign-in page, the welcome page, and the address that approves an app. The person signs in with their own password or passkey. The admin learns nothing that lets them in, which meets driver 4.
- **It's for one person.** Approving the app works only for the account the admin named. Signing in as someone else through it fails, and the key is removed.
- **It's good once, for 7 days.** Seven days covers an email that sits unread over a weekend. Once the app has its own key, the temporary one is removed.
- **The admin log records each setup made, and the person sees it in their device list.** Either of them can cancel it before it's used.

### What the person does

1. Opens the installer link and installs Accordion.
2. Pastes the setup. The app opens their browser, through its proxy, at the welcome page or the sign-in page.
3. Sets their password, or signs in, then approves the app as they would at home.

From then on, their files are in File Explorer and the server's name works in their browser, at their home or anywhere else.

### The proxy

- **It listens on `127.0.0.1` only,** on a port picked at random when the app starts.
- **It carries only `CONNECT` to port 443 of the app's servers.** Anything else gets an error, so it can't be used to reach other sites or other ports.
- **It serves the setup script itself.** The script names each server and sends everything else `DIRECT`.
- **The app sets the script for the signed-in Windows user only,** in the per-user Internet Settings, so it needs no admin rights. It removes the setting when its last account is removed, and when it's uninstalled.
- **If the computer already has a proxy or a setup script, the app leaves it alone.** It says that this computer's browser needs the WireGuard app away from home, and why. Merging with a work proxy's own script is left for later.
- **Before its own key is registered, the proxy uses the setup's temporary key,** so the browser reaches only the sign-in, welcome, and approval pages.
- **The tray menu gets Open Accordion,** which opens the server in the default browser.

## Consequences

- **The desktop app** gains the proxy, the setup script setting, setups that start with a browser sign-in, and **Open Accordion**.
- **Identity** gains setups made for someone else: who for, who by, the welcome token if there is one, the temporary key, and expiry.
- **The server's temporary-key check** (`remote::limit_temporary_keys`) gains the sign-in, welcome, and approval addresses for these keys.
- **The web app** gains **Set up a computer** on the People page, and shows pending setups in **Apps and devices**.
- **The user guide** for family members becomes: install, paste, sign in. The WireGuard app moves to the admin guide, for phones until the phone app exists and for computers without Accordion's app.
- **macOS** gets the same proxy through its system proxy settings when its app is built. Linux desktops vary too much, so the Linux client leaves the browser to the WireGuard app.
- **The phone app** carries its own tunnel and shows the web app in its tabs, as the spec's app shapes describe, so this decision doesn't change phones.

## Open questions

- **When the app isn't running,** the setup script can't load. Check that Edge, Chrome, and Firefox fall back to connecting directly, rather than failing every site. If one doesn't, the app keeps a tiny helper running, or removes the setting when it quits.
- **Browsers cache the setup script.** Check how quickly a new server account reaches an open browser, and whether the app has to tell Windows the setting changed.
- **The Network page and Remote access page** should say when remote access can't work, such as behind carrier-grade NAT, before an admin sends anyone a setup.
- **Whether the mapped network drive (WebDAV) follows the setup script** on Windows. It isn't needed with the app's own files in File Explorer, but some households may still use it.
