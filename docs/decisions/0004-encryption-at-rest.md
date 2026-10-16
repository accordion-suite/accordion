# 4. How user data is encrypted at rest

- **Status:** Accepted
- **Date:** October 16, 2026

## Context

The spec lists three layers of encryption for data on the server:

| Layer | Spec says | Default |
|---|---|---|
| Whole volume | Full-disk encryption (LUKS) set up by the installer, unlocked at boot by passphrase or TPM | On |
| Per user | Per-user data keys wrapped by the user's password-derived key (Argon2id) | On |
| End-to-end vaults | Client-side encrypted folders and notes. The server stores ciphertext only | Opt-in |

Backups are encrypted separately, before they leave the box, and aren't affected by this decision.

The per-user layer means each user's data is encrypted with a key that only their password can unlock. Identity now exists in `accordion-core`, with admin password resets and, per [decision 3](0003-sessions-and-tokens.md), passkeys and long-lived sessions. The per-user layer changes what account creation, password changes, and sessions must do, so it has to be settled before more is built on them.

The spec's threat model frames the choice:

- **Trusted:** the server owner.
- **In scope:** network eavesdropping, stolen or discarded disks, compromised client devices, brute-force sign-ins, and malicious shared links.
- **Out of scope:** a malicious server owner, and physical attacks on a running, unlocked server.

Encryption in transit (TLS and WireGuard) is out of scope here.

## Decision drivers

1. **One developer.** Every key, wrapping scheme, and recovery path is maintained by one person, and mistakes in it lose data.
2. **The spec's threats.** Stolen or discarded disks and stolen backups must reveal nothing.
3. **Boring to run.** The server restarts unattended after updates and power cuts, and service recovers within 5 minutes of a reboot.
4. **Forgotten passwords.** Family members forget passwords. An admin reset mustn't cost them their data.
5. **Open formats.** Files stay as files on disk, readable without Accordion, and identical files are stored once.
6. **Background work.** Mail delivery, search indexing, previews, OCR, and backups run while users are signed out.
7. **Passkey-only accounts.** Decision 3 brings passkeys to every client, and some users will never set a password.
8. **Privacy professionals.** The spec promises them data that nobody but they can read.

## Options

### Option 1: Per-user keys unlocked by the user's credentials

This is the spec as written. Each user has a data key, stored wrapped by a key derived from their password with Argon2id. Signing in unwraps it, and the server keeps it in memory while the user has a session.

Pros:

- **Protects signed-out users if the whole box is stolen.** With TPM unlock, a stolen server boots and decrypts its disk on its own. Data of users who aren't signed in stays encrypted.
- **Limits what the server can read at any moment** to the data of signed-in users.

Cons:

- **Every restart locks everyone out.** Unwrapped keys exist only in memory. After an automatic update or a power cut, every phone and laptop stops syncing until its user types their password again. Avoiding that means storing a copy of the key wrapped by each session token and each app password, and keeping all those copies in step when keys change.
- **Background work stops.** Incoming mail for a signed-out user has to be encrypted on arrival with a public key, so each user also needs a key pair. Indexing, previews, and OCR wait until the user signs in. The shared search index would hold everyone's words in plain text, which defeats the purpose, so each user would need a separate, encrypted index.
- **A forgotten password loses data.** An admin reset can't unwrap the old key. Avoiding loss needs recovery keys, which non-technical users lose, or an admin-held escrow key, which brings back the access the scheme was meant to remove.
- **Passkey-only accounts have no password to derive a key from.** The WebAuthn PRF extension can derive one, but support varies across browsers, platforms, and authenticators.
- **It breaks open formats.** Files on disk become ciphertext, unreadable without Accordion and the user's password. Identical files owned by different users can't be stored once.
- **It doesn't stop a determined admin.** The owner has root and can change the server to capture passwords. The threat model trusts the owner anyway.
- **The most work of any option,** in the code where a bug is most likely to lose data.

### Option 2: Full-disk encryption only

Data on disk is protected by encrypting the whole volume. The appliance image sets up LUKS, unlocked at boot by the TPM so the box restarts unattended, or by a passphrase if the owner prefers. On other installs, the admin uses LUKS or the NAS's own encrypted storage. Users who need data that even the server can't read use end-to-end vaults.

Pros:

- **Covers the spec's at-rest threats.** A stolen or discarded disk reveals nothing, and backups are encrypted separately.
- **Nothing changes for users.** Restarts, password resets, passkey-only accounts, and background work behave as they do without encryption.
- **Keeps open formats.** Files stay plain files on the decrypted volume, readable with standard tools, and deduplicated across users.
- **The least work.** Accordion writes no encryption code for this layer. LUKS and the NAS platforms are mature and widely audited.
- **Clear promises.** Disk theft is covered by the volume, and "only I can read this" is covered by vaults, which keep their key on the user's device, where it belongs.

Cons:

- **A stolen box with TPM unlock boots and decrypts.** The thief gets a running server whose services all need a sign-in, and whose admin panel is reachable only from the LAN or VPN. It's protected like a locked phone, not like a powered-off disk. Owners who want more can choose passphrase unlock, at the cost of typing it after every reboot.
- **Accordion can't set up encryption on every install.** Docker, NAS, and .deb or .rpm installs run on storage the admin already has, so Accordion can only check it and warn.
- **The server can read every user's data,** as it can in Google Workspace and Nextcloud. That's consistent with the threat model, but it should be stated plainly in the privacy docs.

### Option 3: Per-user keys held by the server

Each user has a data key, wrapped by a server master key instead of by their password. The server can always unwrap it.

Pros:

- **Crypto-shredding.** Deleting a user's key makes their data unreadable everywhere, including in old backups, without waiting for those backups to expire.
- No sign-in is needed to unlock data, so restarts and background work behave as in option 2.

Cons:

- **No protection beyond option 2.** The master key lives on the same box, so anyone who can read the disk can read the master key. Against disk theft it adds nothing to LUKS.
- **It breaks open formats and cross-user deduplication,** as option 1 does.
- **Key management with little to show for it:** generation, rotation, and backup of the master key, which must never be lost.

## Comparison

| Driver | Option 1: user-unlocked keys | Option 2: full-disk only | Option 3: server-held keys |
|---|---|---|---|
| Work for one developer | Very high | None for this layer | Medium |
| Stolen or discarded disk | Covered | Covered | Covered |
| Whole box stolen, TPM unlock | Signed-out users covered | Services need sign-in, data readable to an attacker with root | Same as option 2 |
| Unattended restarts | Users must sign in again | Yes | Yes |
| Forgotten passwords | Data lost without recovery | No effect | No effect |
| Files readable without Accordion | No | Yes | No |
| Background work while signed out | Blocked or redesigned | Yes | Yes |
| Passkey-only accounts | Depend on PRF support | No effect | No effect |
| Data nobody else can read | Partly | Through vaults | No |

## Recommendation

**Option 2, full-disk encryption only,** with end-to-end vaults for data that nobody but the user should read.

Option 1's only gain over option 2 is protecting signed-out users when the whole box is stolen and unlocks itself with the TPM. Its costs hit every driver: restarts lock everyone out, forgotten passwords lose data, background work stops, and files stop being files. Option 3 has option 1's format costs without its protection.

Option 2 matches the threat model the spec already sets out. The owner is trusted, disks and backups are protected, and the privacy professional, the persona who needs more, gets vaults, which protect against the server itself.

To make option 2 hold up:

- **The appliance image always encrypts the data volume.** The setup wizard offers TPM unlock (the default, so the box restarts unattended) or passphrase unlock (for owners worried about the whole box being stolen), and explains the trade-off.
- **Other installs are checked.** The health page warns when Accordion can't confirm that its data is on an encrypted volume, with a link to the admin guide for the platform.
- **The privacy docs say plainly** that the server can read data outside vaults, and that vaults are the answer for data it mustn't.

### Revisit this decision if

- Accordion hosts users who don't trust the owner, such as a hosted service or several households on one server.
- Beta households need protection from the admin for data that vaults can't cover, such as mail.
- Crypto-shredding becomes a requirement, for example to remove a deleted user's data from backups before they expire.

## Consequences

- **Spec updates.** Remove the per-user row from the encryption table. Add the health page check for an unencrypted data volume. State in the privacy section that the server can read data outside vaults.
- **No change to identity.** Account creation, `set_password`, and admin resets stay as they are, and sessions don't carry keys. This settles decision 3's open question about per-user keys.
- **Keys derived from a password or passkey belong to end-to-end vaults,** on the client. Vaults get their own design when they're built.
- **A deleted user's data stays in backups until those snapshots expire** under the backup retention policy. The admin guide should say so.
- **The health page needs an encryption check,** which means detecting LUKS and the common NAS encryption schemes, on bare metal and from inside a container.
