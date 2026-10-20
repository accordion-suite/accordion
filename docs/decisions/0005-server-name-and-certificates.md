# 5. How the server gets its name and certificate

- **Status:** Accepted
- **Date:** October 18, 2026

## Context

Every Accordion client reaches the server by name, over TLS. The spec says certificates are automatic, from "ACME or built-in local CA", and that the appliance boots into a setup wizard at `http://accordion.local`. It doesn't say where the name comes from, which kind of certificate is the default, or how the wizard is protected before either exists.

Three things already built depend on the answer:

- **The session cookie** is named `__Host-accordion-session` and marked `Secure` ([decision 3](0003-sessions-and-tokens.md)). Browsers refuse it over plain HTTP.
- **Passkeys,** which decision 3 brings to every client, work only over HTTPS. Each passkey is tied to the domain it was created on.
- **The cross-origin check** in `accordion-server` compares a request's `Origin` with its `Host` header. That works when the browser talks to the server directly, but not behind a reverse proxy, and it accepts any host name.

Decision 3 left an open question that belongs here: the wizard runs before TLS exists, so anyone on the network could reach it first, and a `Secure` cookie can't be set.

Remote access over WireGuard is a separate decision, but its needs constrain this one, so they're listed as drivers.

## Decision drivers

1. **One developer.** Every piece of certificate and DNS code is maintained by one person.
2. **Browsers need HTTPS.** Secure cookies, passkeys, the web app's offline cache (service workers), and the Web Crypto API that vaults will use all need a secure context.
3. **Passkeys are tied to the name.** If the name changes, every passkey stops working. The name has to be settled before the owner's first sign-in.
4. **No open router ports by default.** This is goal 5 in the spec.
5. **Family members can't install certificates.** No browser warnings, and no profiles to install on phones.
6. **Accordion's developers aren't trusted.** The threat model says so, and every outbound connection is listed and can be turned off.
7. **One name everywhere.** The same name works on the home network and over the VPN, in browsers, apps, and standard clients.
8. **Setup takes under 15 minutes,** as the spec promises.
9. **Nobody else can claim a new server.** Before the owner's account exists, someone else on the network, or a website the owner visits, mustn't be able to take over the wizard.

## Options

### Option 1: Built-in local certificate authority

The server creates its own certificate authority (CA) and issues itself a certificate for `accordion.local` or a name the owner picks. Each device that uses Accordion installs the CA's root certificate.

Pros:

- **No outbound connections and no third party.** Works on a network with no internet at all.
- **No cost and no account** with anyone.
- **The native apps can trust it automatically.** The enrollment QR code can carry the root's fingerprint.

Cons:

- **Every browser on every device needs the root installed.** On iOS, that's a profile download followed by turning on full trust in **Settings > General > About > Certificate Trust Settings**. Android and desktop browsers each have their own steps. That's too much for family members.
- **Guests see a warning.** Anyone opening a link share on a device without the root sees a full-page security error.
- **A root on every device is a risk of its own.** If the server is compromised, the CA key can impersonate any website to every device that trusts it. Name constraints can limit a root to one domain, but not every platform enforces them.
- **`.local` names don't travel.** They rely on multicast DNS (mDNS), which doesn't cross a WireGuard tunnel and isn't available on every network or every Android device.
- **Standard clients** such as iOS Mail and DAVx5 each need the root trusted separately, or the user has to accept a certificate warning.

### Option 2: A public name with a Let's Encrypt certificate

The owner gives the server a name under a domain they control: one they own (which Mail needs anyway), or a free subdomain from a DNS host with an API, such as deSEC. The server gets a certificate from Let's Encrypt using the ACME DNS challenge. It proves it controls the name by adding a DNS record through the provider's API. The name's DNS record points at the server's address on the home network, so nothing is reachable from the internet.

Pros:

- **Trusted everywhere.** Browsers, phones, and standard clients accept it with nothing to install, including for guests.
- **No open ports.** The DNS challenge needs no inbound connection, unlike ACME's HTTP and TLS challenges.
- **One certificate for everything.** The mail engine can use it for IMAP and SMTP too.
- **The owner controls the name.** Accordion's developers have no part in it, and it outlives the project.
- **Standard and well supported.** ACME is an IETF standard ([RFC 8555](https://www.rfc-editor.org/rfc/rfc8555)), and Rust has maintained ACME clients.

Cons:

- **The owner needs a domain or a DNS account,** plus an API token that the server stores so it can renew.
- **Every DNS provider has its own API,** so Accordion can support only a short list directly.
- **Two outbound connections:** the certificate authority and the DNS provider. Both are already the kind the spec allows, and both appear on the **Network** page.
- **The name is public.** Every certificate is published in Certificate Transparency logs, and the DNS record shows the server's home network address. Neither lets anyone connect, but the name shouldn't identify the household.
- **Some routers block it.** DNS rebinding protection on some routers refuses public names that point at private addresses. The owner then adds an exception on the router.
- **Renewal must keep working.** Certificates last 90 days today, and the industry is shortening them. If renewal fails for weeks, clients stop connecting.

### Option 3: A name service run by Accordion's developers

Each server automatically gets a name under a domain the project owns, as Plex does with `plex.direct` and Home Assistant does with its cloud service. The project's DNS servers answer the certificate challenges for every server.

Pros:

- **The least setup.** No domain, no account, and no token.
- **Trusted everywhere,** as in option 2.

Cons:

- **Accordion's developers control every server's name.** Whoever runs the DNS can get a valid certificate for any Accordion server and intercept its traffic. The threat model rules that out.
- **A service to run forever.** One developer would host, pay for, and secure it. If it ever stops, every server loses its name, and every user loses their passkeys.
- **Every server depends on it,** which conflicts with the zero-telemetry principle even if the service logs nothing.

## Comparison

| Driver | Option 1: local CA | Option 2: public name, Let's Encrypt | Option 3: project name service |
|---|---|---|---|
| Work for one developer | Medium, plus a device guide per platform | Medium | High, and it never ends |
| Works in browsers without setup | No | Yes | Yes |
| Guests opening link shares | Warning | Yes | Yes |
| Stable name for passkeys | Yes | Yes, while the owner keeps the domain | Only while the service runs |
| No open ports | Yes | Yes | Yes |
| Independent of Accordion's developers | Yes | Yes | No |
| Works with no internet | Yes | No, renewal needs it | No |
| Same name over the VPN | No, for `.local` | Yes | Yes |
| Extra setup for the owner | Install the root on every device | One DNS token | None |

## Recommendation

**Option 2, a public name with a Let's Encrypt certificate,** as the default. **Option 1** stays as a fallback for households that keep the server off the internet, built after option 2.

Option 2 is the only option that works in every browser and client without touching each device, while keeping Accordion's developers out of the trust chain. Its main cost is a one-time DNS step for the household admin, which the wizard and admin guide can walk through. Option 1 can't be the default because family members and guests would face certificate warnings. Option 3 fails the threat model.

The fallback matters because the spec lets the owner turn off every outbound connection. With the certificate authority turned off, renewal stops, so those households need certificates that don't depend on it.

### The name

- **One host name per server,** such as `accordion.example.com` or `ourhome.dedyn.io`. The web app, the native apps, WebDAV, and later mail clients all use it.
- **It's the passkey relying party ID.** Passkeys are created for this exact name, not its parent domain, so other sites on the owner's domain can't use them.
- **The wizard suggests a name that doesn't identify the household,** because certificate logs are public.
- **Changing the name is rare and deliberate.** The admin panel warns that every passkey must be registered again. Users sign in with their password, or with a one-time sign-in link from an admin, and add a new passkey.

### Certificates

- **ACME DNS challenge with Let's Encrypt,** never the HTTP or TLS challenge, because those need an open port.
- **Keys stay on the server.** The private key is generated there and never leaves.
- **Renewal is automatic,** well before expiry. The health page already shows certificate expiry. It also warns when renewal has failed more than once.
- **DNS providers, to start:** deSEC (free subdomains, run by a nonprofit), Cloudflare, and RFC 2136 (the standard update protocol, for owners who run their own DNS). Owners with any other provider point `_acme-challenge` at a deSEC name with one CNAME record, so the server only needs a token for that name.
- **The token is stored on the encrypted data volume** and scoped to the one name where the provider allows it.
- **The server keeps its DNS record up to date** when its home network address changes. This is the spec's dynamic DNS helper. The admin guide also recommends a fixed address reservation on the router.

### Where TLS ends

- **The gateway is a library in the server binary,** like the modules ([decision 1](0001-where-core-services-run.md)). It terminates TLS for every HTTP request.
- **The mail engine gets the same certificate.** The gateway writes it to a file only the engine can read, and the engine reloads it after each renewal.
- **Behind the gateway, traffic stays on the machine,** over loopback or Unix sockets.
- **Installs behind an existing reverse proxy** (common with Docker and NAS setups) can turn on a setting where Accordion serves plain HTTP to that proxy, and trusts forwarded headers only from its address. The admin still enters the public name, and the proxy handles the certificate. Browsers still see HTTPS, so cookies and passkeys work.

### Checking the host name

- **The server answers only requests for its own name.** This blocks DNS rebinding, where a website the owner visits points its own name at the server's address to reach it from the owner's browser.
- **The cross-origin check compares `Origin` with the configured name,** not with the `Host` header.
- **Port 80 only redirects** to the HTTPS name. `http://accordion.local` keeps working as a way to find the server, and redirects too.

### The setup wizard

1. **The wizard starts at `http://accordion.local`,** as the spec says. Until it's claimed, it answers only requests for `accordion.local` or the server's address, and only from the local network.
2. **Claiming it needs a setup code,** 10 random lowercase letters, shown only where someone with access to the machine can see it: the terminal for `.deb` and `.rpm` installs, the container logs for Docker and NAS installs, an attached screen on the appliance, or the `accordion setup-code` command.
3. **A headless appliance has nowhere to show the code,** so it accepts a claim without one for 30 minutes after it starts. If that time passes, restarting the box opens it again. Restarting needs physical access, so it proves the person owns the box. The host name check keeps websites out during that time.
4. **The wizard asks for the name and DNS token,** then gets the certificate.
5. **The wizard moves to `https://<name>/setup`** with a single-use handoff token, valid for five minutes. The owner creates their account there, with a passkey or a password.

The owner's password and first passkey never touch plain HTTP, and the passkey is created on the final name. The setup code and DNS token do cross the home network unencrypted. Home Wi-Fi encrypts traffic against outsiders, and the token is scoped where possible, so that's an accepted risk for a 15-minute window.

Households that choose not to use an internet name get the local CA in the same step, once it's built.

### Revisit this decision if

- Beta households often get stuck on the DNS step.
- Browsers start treating local network names as secure without a public certificate.
- Router DNS rebinding protection blocks a large share of beta households.

## Consequences

- **Decision 3's open question is settled.** The wizard is protected by a setup code or a restart window, and it moves to HTTPS before the owner's first session.
- **Spec updates.** Change the encryption table to "Let's Encrypt over the DNS challenge, or a built-in local CA when the server is kept off the internet". Add the setup code and the move to HTTPS to the appliance install step. Add the certificate authority and DNS provider to the **Network** page's list of outbound connections.
- **Create `accordion-gateway`** with `scripts/new-repo.sh`, as a library. It starts with TLS, ACME, and the host name check. WireGuard follows with the remote access decision.
- **Server changes.** The server reads its name from configuration, listens on ports 443 and 80, and checks `Origin` against that name. The systemd unit grants `CAP_NET_BIND_SERVICE` so it can use those ports without running as root.
- **Passkeys can be built** once the name exists, with the name as the relying party ID.
- **Admin guide pages** for each supported DNS provider, for the CNAME setup, and for router DNS rebinding exceptions. Troubleshooting should also explain that a device that looked up the name before its record existed can keep getting "not found" for as long as the zone's negative caching time, which is 30 minutes at Cloudflare. That happened in the Raspberry Pi test.
- **The setup time target needs testing** with the DNS step included.
- **The local CA is planned,** after option 2, for households that keep the server off the internet.

## Open questions

- **Remote access needs an inbound port.** The spec promises no open router ports, but a WireGuard server at home needs one inbound UDP port, unless the router opens it automatically (UPnP or NAT-PMP) or traffic goes through a relay. The remote access decision also has to make the name resolve over the VPN, where the home network's address range can clash with the network the device is on.
- **Households without a domain.** deSEC's scoped tokens work: a token limited to one `_acme-challenge` record got a Let's Encrypt staging certificate on October 19, 2026. But deSEC has suspended new `dedyn.io` subdomains, with no date to restart, so examples above like `ourhome.dedyn.io` aren't available to new households. deSEC still hosts domains registered elsewhere. Options to compare:
  - **Buy a domain.** Many cost about $10 a year. The household then hosts its DNS at deSEC or Cloudflare. Mail needs a domain anyway, so this may be the simplest default.
  - **Duck DNS.** Free subdomains with an API that sets one TXT record per account, which is enough for one name. The token controls every subdomain on the account, and the service is run by two people.
  - **dynv6.** Free subdomains with a REST API. One report from 2023 says it refused `_acme-challenge` names, so test it before relying on it.
- **More DNS providers.** Check which providers beta households use before adding more.
- **A server on two networks.** The name points at one address, the one on the server's route to the internet. A live test on October 20, 2026, used a Raspberry Pi running umbrelOS that was on the home router's network (192.168.1.57) and was also a Wi-Fi access point with its own network (10.42.0.1).
  - **What happened:** devices on the access point's network normally reach 192.168.1.57 through the Pi, because it's their gateway. A PC running Proton VPN couldn't, most likely because the VPN's **Allow LAN connections** setting only exempts the network the PC is on. Pointing the name at 10.42.0.1 with `ACCORDION_ADDRESS` fixed that PC, and broke the name for devices on the router's network, which can't reach 10.42.0.1.
  - **Where else it applies:** a server that joins two networks with their own routers, such as a main network and a separate one for smart home devices. Also Accordion's own VPN, whose clients reach the server at a VPN address.
  - **Options to compare:**
    - **Keep one address,** chosen automatically or with `ACCORDION_ADDRESS`, and document the limits. No new code.
    - **Publish an address for each network,** as several address records. Clients choose one and try another if it fails. How quickly that happens varies by browser and platform, and can be as long as a connection timeout, so it needs testing on each. It also publishes more of the home network's layout.
    - **Answer DNS on the server** with the address for the network each question comes from (split-horizon DNS). It only works for devices that use the server for DNS. That suits VPN clients, whose WireGuard settings can name it, but home networks would need a router change.
  - **Recommendation:** keep one address for now, and settle this in the remote access decision, because VPN clients need the same answer. Meanwhile, the wizard's log should list its address on every network, not only the main one.
