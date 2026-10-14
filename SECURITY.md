# Security policy

Accordion holds people's email, files, and notes, so security reports get top priority. Thank you for helping keep Accordion users safe.

## Reporting a vulnerability

Please don't report security vulnerabilities in public issues, discussions, or pull requests.

Report them privately through GitHub instead:

1. Go to the **Security** tab of this repository.
2. Select **Report a vulnerability**.
3. Describe the issue, including the steps to reproduce it, the affected component and version, and what an attacker could do with it.

Report vulnerabilities in any Accordion repository here, even if the issue is in a module such as `accordion-mail` or `accordion-core`.

## What to expect

Accordion is maintained by one person, so these timelines are realistic rather than instant:

- **Within 7 days:** you get a reply confirming the report was received.
- **Within 14 days:** you get an assessment of whether the issue is confirmed and how severe it is.
- **While a fix is in progress:** you get updates at least every 14 days.

When a fix is ready, it ships in a suite release, and a security advisory is published on GitHub. You'll be credited in the advisory unless you'd rather stay anonymous.

## Disclosure

Accordion follows coordinated disclosure. Please give the project 90 days from your report to release a fix before you share details publicly. If a fix needs more time, the maintainer will agree on a new date with you. If the issue is already being exploited, the fix and advisory will go out as soon as possible.

## Supported versions

Accordion hasn't had a release yet. Once it has, security fixes will go into the latest suite release. This section will list the supported versions then.

## Scope

In scope:

- Any code in Accordion repositories, including the installer, gateway, core services, modules, and apps
- Default configurations that expose data or allow access they shouldn't
- Data sent off the server without the admin's knowledge, since Accordion promises zero telemetry

Out of scope:

- Attacks that need the server owner to act maliciously
- Physical attacks on a running, unlocked server
- Vulnerabilities in third-party software that Accordion uses, unless Accordion uses it in an unsafe way. Please report those to the upstream project.
- Denial-of-service attacks that need large amounts of traffic

## Safe harbor

If you research security issues in good faith, follow this policy, and only test against servers you own or have permission to test, Accordion won't take legal action against you or ask anyone else to. Please don't access, change, or delete other people's data while you test.
