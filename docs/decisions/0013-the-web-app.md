# 13. How the web app is built

- **Status:** Accepted
- **Date:** October 27, 2026

## Context

The spec describes the web app as "one app at the server's address," with an app switcher in the top bar, and one sign-in for every module. Its stack table picks a "TypeScript single-page app, installable PWA," for its "offline cache, one codebase for desktop browsers and phones." It also sets targets the web app has to meet:

- **Speed:** first load under 2 seconds on the home network, and under 4 seconds over the VPN on 4G. Moving between pages takes under 300 milliseconds.
- **Accessibility:** WCAG 2.2 AA, full keyboard navigation, and tested with screen readers.
- **Languages:** ready for translation from day one, with English and five more languages at 1.0.
- **Offline:** Docs and Notes fully editable offline. The Docs editor is built on ProseMirror and Yjs, which are JavaScript libraries.

The first full release waits on Drive's web pages, so family members can use their files without setting up a WebDAV client. They need a folder view, uploads with drag and drop that resume after a dropped connection (Drive's API already supports this), downloads, new folders, renaming and moving, the trash, and previews of images and PDFs.

[Decision 7](0007-module-interface.md) gave each module the path `/<id>/` for its pages, such as `/drive/`, and left how the web app is built, and where each module's pages live, to this decision. The workspace layout plans an `accordion-web` repo for it.

The server already has 10 pages: the setup wizard and its owner page, the welcome page, and the home, account, health, backups, people, updates, and network pages. Each is plain HTML with a plain JavaScript file, about 72 KB of JavaScript in all, built into the server program. There's no build step and no npm. A strict content security policy allows only the server's own scripts. These pages have worked well, and they're the starting point for what follows.

Three earlier decisions limit the choices:

- **Reproducible builds** ([decision 9](0009-releases-and-installing.md#reproducible-builds)). CI's packages and the developer's own build must hold the same files, byte for byte. Anything the web app's build produces has to come out the same on any machine.
- **Zero telemetry** ([decision 11](0011-the-network-page.md)). The server serves every file itself. There are no third-party fonts, CDNs, or analytics, and the browser never contacts anything else.
- **Few dependencies, each reviewed.** cargo-deny checks every Rust crate's license, advisories, and source. npm packages don't have that check yet, and npm packages can run code when they're installed, which is a common way for npm supply-chain attacks to spread.

## Decision drivers

1. **Family members can use it** on phones and computers, with a keyboard or a screen reader, in their own language.
2. **Fast on a Raspberry Pi,** over the home network and over 4G.
3. **Few dependencies, each one reviewable,** and a release that's still reproducible.
4. **Nothing loads from anywhere but the server.**
5. **Grows into Docs, Mail, and Notes,** including ProseMirror, Yjs, and offline editing, without starting over.
6. **Little to maintain** for one developer.

## Options

### Option 1: Keep going without a build step

Write the web app the way the current pages are written: plain HTML and JavaScript files, with no compiler, no framework, and no npm. Each module's crate holds its pages.

Pros:

- **Nothing new to install or trust.** Builds stay reproducible, because the files are served as written.
- **Already proven** by 10 pages, and it's the smallest download.

Cons:

- **No types.** A Drive page has far more state than a settings page: selections, an upload queue, drag and drop, and previews. Without types, renaming a field in the API breaks pages silently.
- **Everything is built by hand,** including components that every module shares, such as menus, dialogs, and the file list, along with their keyboard and screen reader behavior.
- **ProseMirror and Yjs still need bundling** when Docs arrives, so a build step comes anyway, later and under more pressure.

### Option 2: A TypeScript app with the usual tools

A TypeScript single-page app with React, built with Vite, the most common setup for new web apps.

Pros:

- **Matches the spec** and the tools most web developers know, which helps contributors.
- **The largest choice of libraries,** including ready-made accessible components.

Cons:

- **A large dependency tree.** A typical Vite and React project installs well over 100 npm packages, mostly build and lint tools. Each is code that runs on the developer's computer and in CI, and few would get a real review.
- **Frequent upkeep.** The build tools release major versions often, and one developer would spend real time on upgrades that don't change what families see.
- **Heavier pages.** React and React DOM add over 40 KB of compressed JavaScript before any of Accordion's own code.

### Option 3: A TypeScript app with small tools

A TypeScript single-page app built with esbuild, which compiles and bundles TypeScript in one program with no dependencies of its own. The pages use Preact, a 4 KB library with React's way of building components and no dependencies. The TypeScript compiler, also free of dependencies, checks the types.

Pros:

- **Matches the spec:** TypeScript, a single-page app, and an installable PWA.
- **Three tools, none with dependencies:** esbuild, TypeScript, and Preact. ProseMirror and Yjs add about 20 small packages when Docs arrives, all under the MIT license.
- **Reproducible:** esbuild gives the same output for the same input and version, so CI and the developer's build can match.
- **Small pages,** so the first load fits the spec's targets on a Pi.
- **Plain DOM.** Preact renders ordinary HTML elements, so native controls, ARIA, and screen readers work as they do on the current pages.

Cons:

- **Fewer ready-made parts** than React. Routing, the PWA's service worker, and accessible components are mostly Accordion's own code.
- **A smaller community** than React's, though Preact has been maintained since 2015 and most React knowledge carries over.

### Option 4: Pages rendered on the server

Rust renders each page as HTML, with a little JavaScript to update parts of a page, in the style of htmx.

Pros:

- **No JavaScript build,** and the pages share Rust types with the API.
- **Fast first loads.**

Cons:

- **Doesn't fit the spec's offline goals.** Offline Docs and Notes need the app running in the browser, so the heavy client code comes anyway.
- **Every action is a trip to the server,** which makes 300 ms page changes hard over 4G.
- **Two ways to show the same data,** because the desktop and mobile apps use the JSON API regardless.

### Option 5: Rust compiled to WebAssembly

A Rust web framework, such as Leptos or Dioxus, compiled to WebAssembly, so the server and the web app share one language.

Pros:

- **One language, and shared types** with the server.

Cons:

- **Large downloads.** Apps are often several hundred kilobytes of WebAssembly before they show anything, which strains the first-load target on 4G.
- **ProseMirror and Yjs are JavaScript,** so Docs needs a bridge to them, or a Rust editor that doesn't exist yet.
- **Younger frameworks,** with less proven accessibility, and long compile times for every change to a page.

## Comparison

| Driver | Option 1: no build | Option 2: React and Vite | Option 3: Preact and esbuild | Option 4: server-rendered | Option 5: WebAssembly |
|---|---|---|---|---|---|
| Matches the spec's TypeScript PWA | No | Yes | Yes | No | No |
| Dependencies to review | None | 100+ | 3, then about 20 with Docs | None in the browser | Many crates |
| Reproducible | Yes | Yes, with care | Yes | Yes | Yes |
| Fast on a Pi over 4G | Yes | Fair | Yes | First load only | Slow first load |
| Ready for Docs and offline | No | Yes | Yes | No | Partly |
| Work for one developer | High, for rich pages | Medium, mostly upkeep | Medium | Medium | High |

## Recommendation

**Option 3: a TypeScript app with esbuild and Preact,** in the `accordion-web` repo.

### Where it lives

`accordion-web` holds the app's shell (the top bar, the app switcher, sign-in, and shared components) and each module's pages, in a folder per module, such as `src/drive/`. Keeping them in one repo means one build, one lockfile, and shared components without publishing packages between repos.

The module rules carry over to the browser, so any module's pages could move later. A module's pages use only their own module's API, such as `/api/drive/`, and the server's shared endpoints, such as `/api/session`. They never call another module's API, and modules share code only through the shell's components.

### How the server serves it

The release build checks out the `accordion-web` commit that the suite pins, installs its packages, builds it, and puts the files in the package at `/usr/lib/accordion/web`. The server serves them from there. They're read-only to the service, like the server program itself. During development, the `ACCORDION_WEB_DIR` setting points the server at `accordion-web`'s build folder instead, so a change to a page needs a rebuild of the web app, not of the server.

The suite pins the web app's commit the same way it pins module versions: `scripts/pin-versions.sh` writes it to `packaging/web-commit`, and the lock check covers it.

The URLs follow decision 7:

| Path | What answers |
|---|---|
| `/<id>/…`, such as `/drive/` | The web app's page, when that module is on. The app shows the right page from the rest of the path. |
| `/app/…` | The app's scripts, styles, and icons. Their names include a hash of their contents, so browsers can keep them for a year. |
| `/manifest.webmanifest` | The PWA manifest, which lets phones and computers install the app. |

The server, not the module, answers these, because there's one app. A module that's off gets no page, and the app switcher leaves it out. The server compresses each file once, when the package is built, not on every request.

### Dependencies and the build

- **esbuild and the TypeScript compiler** are pinned by version. esbuild is downloaded and checked against its checksum, like zig and restic.
- **npm installs only libraries,** from a committed lockfile, with `npm ci --ignore-scripts`, so no package runs code when it's installed.
- **Each new package needs a reason** in the commit that adds it, and Dependabot proposes updates as it does for Rust.
- **Licenses** of the browser packages go into the package's third-party license file, with the Rust crates' licenses.
- **The content security policy stays strict:** scripts and styles from the server only, no inline scripts, and no `eval`.

### How it meets the spec's targets

- **Speed.** A size budget runs in CI. Drive's first load stays under 150 KB of compressed JavaScript and CSS, and the build fails above it. After the first visit, the service worker serves the app's files from the browser's cache.
- **Accessibility.** Pages use native elements, such as buttons and lists, before ARIA. Automated checks with axe run on every page in the browser tests, and screen reader tests with NVDA and VoiceOver happen before each full release.
- **Languages.** Every string the app shows comes from a message file, in English at first, with the browser's own `Intl` functions for plurals, dates, and sizes. Strings are never built from pieces, so translators see whole sentences.
- **No third parties.** Fonts are the device's own, as on the current pages, and so are icons, which are SVG files in the repo.
- **Tests.** Type checks and unit tests run on each push. Browser tests use Playwright against a real server, as the current pages' manual checks already do.

### The existing pages

The setup wizard and the welcome page stay in the server as they are. They run before there's an HTTPS name, and before the person has an account, so they shouldn't depend on the web app.

The admin pages (health, backups, people, updates, and network) and the account page move into the web app's settings later, one at a time, and the plain versions are removed as each one moves. The home page becomes the web app.

### What comes first

The shell, sign-in, and Drive's pages, with the scope from the context above. Sharing, versions, and previews of other file types follow in later releases.

### Revisit this decision if

- **Preact stops being maintained,** or falls far behind React's way of building components. Moving to React is mostly mechanical.
- **Docs needs a React-only library.** Preact runs most React libraries through its compatibility layer, but not all of them.
- **The mobile apps decide to show web pages** inside their native shells, which would change what the web app needs to support.

## Consequences

- **The `accordion-web` repo is created** with `scripts/new-repo.sh`. The template gains a variant for a TypeScript repo, with its own CI and Dependabot settings for npm.
- **The release build gains a web step,** and decision 9's reproducibility check covers the web app's files too. CI installs esbuild and Node.js at pinned versions.
- **The server serves the web app** from `/usr/lib/accordion/web`, or from `ACCORDION_WEB_DIR`, and answers `/<id>/` for each module that's on.
- **`pin-versions.sh` pins the web app's commit** in `packaging/web-commit`, beside `Cargo.lock`.
- **Drive's API is the web app's first user.** Gaps it finds, such as a way to list a folder in pages, go into `accordion-drive`.
- **The admin pages move into the web app over time,** and the setup wizard stays plain.

## Open questions

- **Offline data.** How Docs and Notes keep documents in the browser while offline, and how much storage they use, belong with the Docs and Notes decisions.
- **The service worker after an update.** A server update changes the app's files. The app has to notice and reload, without losing an upload or an unsaved change. That will be designed with the PWA.
- **Translation.** Where community translators work, such as a self-hosted Weblate, before the five languages the spec asks for at 1.0.
