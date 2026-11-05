# 24. Signing the apps

- **Status:** Accepted
- **Date:** November 5, 2026

## Context

Decision 23 builds a desktop app for Windows and macOS, and its build order puts the Windows app next. Both systems check who made an app before they run it:

- **Windows** checks an Authenticode signature. An unsigned installer gets SmartScreen's "Windows protected your PC" warning, which hides the **Run anyway** button behind **More info**. On a new Windows 11 computer with Smart App Control turned on, an unsigned app doesn't run at all.
- **macOS** checks for a Developer ID signature and Apple's notarization. Without both, Gatekeeper refuses to open the app. The File Provider extension that puts Accordion in Finder (decision 23) has to be signed too.

Family members are the people installing the app, so these warnings matter. A warning that tells them to ignore a warning teaches them to ignore warnings.

Some pieces exist already:

- **The release key** ([decision 9](0009-releases-and-installing.md)). An Ed25519 key, kept offline on the developer's computer, signs `SHA256SUMS` for every release. The install script and the server check it, so nothing installs unless the developer signed it, even if GitHub is compromised.
- **Updates** ([decision 12](0012-updates.md)) only install releases the release key signed. Decision 23 says the desktop app updates itself the same way.
- **Releases are built by CI** and checked against a build on the developer's computer (`scripts/check-release.sh`) before the developer signs them.

The release key doesn't help with Windows or macOS. Neither system knows it, so they still warn. Each needs a signature that chains to a certificate it trusts, and those come with conditions: money, a verified identity, and sometimes where the build happens.

Decision 23 left this open, and asked for a decision before the Windows app. This decision covers how the Windows and macOS apps are signed, and how that fits the release key. The phone apps' decision covers their stores. Linux has no system-wide app signature, so the Linux client stays signed by the release key only.

## Decision drivers

1. **No warnings for family members,** or as few as Windows allows.
2. **The release key stays in charge.** Whatever signs for Windows or macOS must not be able to push an update to installed apps.
3. **The developer's private details stay private** where possible. A certificate in a person's name shows that name, and sometimes their city, to everyone who installs.
4. **Little money,** since Accordion has no income.
5. **One developer.** Little to set up, and little to do for each release.
6. **No lock-in** that would force a change later, such as a license condition.

## Options for Windows

### Option A: SignPath Foundation

SignPath Foundation is a nonprofit that signs open-source projects' Windows releases for free, with a certificate in its own name. Each release is built by CI from the public repo, sent to SignPath from the workflow, and signed after someone on the project approves it.

Pros:

- **Free.**
- **The publisher shows as "SignPath Foundation,"** not the developer's name or city.
- **The signing key isn't on the developer's computer or in CI.** It's in SignPath's hardware, and each signature needs the developer's approval, with multifactor sign-in.
- **Proves the build came from the public source,** which fits the spec's openness.

Cons:

- **Accordion has to be accepted.** The terms ask for an OSI-approved license, no proprietary parts, active maintenance, and software already released in the form they'll sign. So the first Windows alpha would go out unsigned.
- **No commercial dual licensing,** of any part. A contributor agreement that let Accordion sell other licenses later would end it.
- **A code signing policy page,** linked from the home page, naming the project's roles (authors, reviewers, and approvers) and stating its privacy policy.
- **The build must come from CI,** on GitHub's own runners, so the installer can't be built on the developer's computer.
- **SmartScreen reputation still has to build up.** No certificate skips it since 2024, and it's not clear whether a shared certificate helps.

### Option B: Azure Artifact Signing

Microsoft's own signing service, formerly Trusted Signing. Microsoft checks the developer's identity, keeps the key in its hardware, and signs from CI or from the developer's computer.

Pros:

- **About $9.99 a month,** with no hardware to buy.
- **Signs from anywhere,** including the developer's computer, so it fits today's release steps.
- **Microsoft's recommendation** for apps outside the Microsoft Store.

Cons:

- **Individual developers must be in the United States or Canada,** and Accordion's developer isn't. Elsewhere, only registered organizations can use it, so it's out unless Accordion becomes one.
- **The certificate shows the developer's legal name,** with their city, region, and country.
- **A monthly bill to an Azure account,** which starts when the account is made, not at the first release.
- **Reputation builds over time,** as with any certificate.

### Option C: A code signing certificate from a certificate authority

A certificate from a company such as Sectigo, DigiCert, or Certum, in the developer's name. Since June 2023, the key must sit in a hardware token or the authority's cloud hardware.

Pros:

- **Available worldwide.**
- **Certum's "Open Source Developer" certificate** is the cheapest, from €69 with a smart card and reader, for people who publish open-source software.
- **The key stays with the developer,** on a token, like the release key.

Cons:

- **$129–$300 a year** for most authorities' standard certificates, and certificates last at most 459 days since February 2026, so it's renewed every year.
- **The certificate shows the developer's name.**
- **A hardware token,** which CI can't use. Signing stays a manual step on the developer's computer, with the token plugged in.
- **Reputation builds over time.**

### Option D: The Microsoft Store

The Store re-signs MSIX packages for free after it checks them, and Store installs show no SmartScreen warnings.

Pros:

- **Free,** and no warnings at all.
- **Package identity,** which some of Windows' shell features need.

Cons:

- **The Store updates the app, not Accordion.** Updates wouldn't be checked against the release key, and each one waits for the Store's review, which slows urgent fixes.
- **MSIX only.** Tauri builds MSI and EXE installers, and a Store listing for those still needs a certificate from option B or C.
- **Not everyone can use the Store,** such as on Windows LTSC or in companies that turn it off.

### Option E: No signature

Pros: nothing to do. Cons: the strongest SmartScreen warning, and Smart App Control blocks the app. Only fit for testers.

## Options for macOS

There's one real option: the **Apple Developer Program**, 99 USD a year. It gives the Developer ID certificate that signs the app and its File Provider extension, and lets Apple notarize them for no extra fee. The certificate shows the developer's legal name. The same membership covers the iPhone app later.

How it's signed can still be chosen:

- **On the developer's computer, with `rcodesign`.** It's an open-source Rust tool that signs and notarizes from Linux. The Developer ID key stays offline, like the release key, and notarization uploads the signed app to Apple with an App Store Connect API key.
- **In CI, on GitHub's macOS runners,** with the key in CI's secrets. That's what decision 9 kept the release key out of.

## Comparison

| Driver | A: SignPath Foundation | B: Azure Artifact Signing | C: Certificate | D: Microsoft Store |
|---|---|---|---|---|
| Warnings | Until reputation builds | Until reputation builds | Until reputation builds | None |
| Release key stays in charge | Yes | Yes | Yes | No, the Store updates |
| Name shown | SignPath Foundation | The developer's, with city | The developer's | The developer's chosen name |
| Cost a year | Free | About $120 | €69–$300 | Free |
| Who can use it | Accepted open-source projects | Individuals in the US or Canada, so not Accordion's developer | Anyone | Anyone |
| Where it signs | CI, after approval | CI or the developer's computer | The developer's computer | The Store |
| Lock-in | No dual licensing | A monthly bill | Yearly renewal | MSIX and the Store's review |

## Costs

| What | When | Cost |
|---|---|---|
| Windows, with SignPath Foundation | From the first signed Windows release | Free |
| macOS, with the Apple Developer Program | From the macOS app (step 5 of decision 23) | 99 USD a year |
| Windows fallback, Certum's Open Source Developer certificate | Only if SignPath says no | From €69 the first year |
| Windows fallback, a standard certificate from another authority | Only if SignPath says no and Certum can't issue one | $129–$300 a year |

## Recommendation

**SignPath Foundation for Windows (option A), and the Apple Developer Program for macOS, signed on the developer's computer with `rcodesign`.** The release key keeps signing `SHA256SUMS`, which now includes the signed installers, so updates still need the developer's own key. The Microsoft Store can come later, as an extra way to install, not instead.

SignPath is free, keeps the developer's name off every install, and doesn't put a signing key on the developer's computer or in CI. Its conditions match how Accordion is already built: AGPL, public, and built by CI. Its one real cost is the license condition, and Accordion is already AGPL, with no other license on offer.

### Windows

- **The first Windows alpha ships unsigned,** for testers only, with steps for SmartScreen's **More info** and **Run anyway**. SignPath asks for software already released in the form it signs.
- **Then Accordion applies to SignPath Foundation,** with the Windows app's repo, its CI workflow, and the policy page below.
- **The release workflow builds the installer** on GitHub's runners and sends it to SignPath. The developer approves the request in SignPath, with multifactor sign-in, and the workflow puts the signed installer in the draft release.
- **`scripts/check-release.sh` compares the installer without its signature,** since Authenticode signatures are part of the file. `osslsigncode` removes one on Linux.
- **The developer then signs `SHA256SUMS` with the release key,** as today. That one file now covers the server packages and the signed desktop installers.
- **If SignPath says no,** Accordion uses Certum's Open Source Developer certificate, or a standard certificate from another authority if Certum can't issue one. Either way, the developer signs on their computer with the hardware token, before signing `SHA256SUMS`. Azure Artifact Signing isn't a fallback, since it takes individual developers only in the United States and Canada.

### The code signing policy page

- **In the `accordion` repo,** linked from the README as **Code signing policy**, as SignPath requires.
- **It names the roles:** the developer is the author, reviewer, and approver, until there are more people.
- **It states the privacy policy:** the apps send nothing to anyone but the person's own Accordion servers, which is already a rule of the spec.

### macOS

- **The developer joins the Apple Developer Program when the macOS app starts,** not before, since the fee is yearly.
- **The Developer ID key is made on the developer's computer** and kept offline, encrypted, like the release key.
- **`rcodesign` signs the app and its File Provider extension, then notarizes and staples them,** from Linux. The App Store Connect API key for notarizing stays offline too.
- **The signed app goes into `SHA256SUMS`** before the release key signs it.

### What each signature is for

| Signature | Checked by | What it stops |
|---|---|---|
| Release key, on `SHA256SUMS` | The install script, the server, and the apps' updates | Anyone but the developer releasing or updating Accordion |
| SignPath Foundation, on the Windows installer and app | Windows | SmartScreen warnings, and Smart App Control blocking the app |
| Developer ID and notarization, on the macOS app | macOS | Gatekeeper refusing to open the app |

A stolen Windows or macOS signature could make a fake download look trusted, but it can't update an installed app, since updates check the release key.

### Revisit this decision if

- SignPath Foundation turns Accordion down, or changes its terms.
- Accordion wants commercial dual licensing, which SignPath doesn't allow.
- SmartScreen keeps warning for months after the first signed releases.
- Azure Artifact Signing opens to individual developers in more countries.

## Consequences

- **The `accordion` repo** gains the code signing policy page, linked from its README.
- **The desktop app's release workflow** sends the Windows installer to SignPath and waits for the developer's approval.
- **`scripts/check-release.sh`** compares Windows installers without their signatures, and macOS apps without theirs.
- **`SHA256SUMS`** covers the desktop installers, so the release key signs them too.
- **The developer** approves each Windows release in SignPath, and signs and notarizes each macOS release with `rcodesign`, before signing `SHA256SUMS` as today.
- **The Apple Developer Program** costs 99 USD a year from the macOS app on.
- **Decision 23's open question about signing** is answered.

## Open questions

- **Whether SignPath Foundation accepts Accordion,** and how long it takes.
- **The contributor agreement and the client SDK's license,** which are still open. A license that lets Accordion sell other licenses would rule out SignPath.
- **A Microsoft Store listing,** later, for people who install apps that way. It would need MSIX packaging, and a way for Store updates to keep up with the release key's.
