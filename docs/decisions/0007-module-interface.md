# 7. How modules plug into the server

- **Status:** Accepted
- **Date:** October 21, 2026

## Context

[Decision 1](0001-where-core-services-run.md) builds core and every module into one server binary. Each module stays a library crate in its own repo and "exposes a function that registers its routes, jobs, and search content with core". Drive is the first module, so what that function looks like has to be settled now.

The spec asks four things of modules on the server:

- **A module that's off doesn't run.** It uses no RAM, it disappears from every menu, and its protocol endpoints, such as WebDAV, don't answer. Its data stays on disk.
- **The admin turns modules on and off** from a **Modules** page, which shows each module's status and data size.
- **Dependencies are explicit.** Docs and Notes require Drive, because their files live there.
- **Modules can be turned on per group,** so a small team can give Mail to staff but not guests.

Module code also needs to know who's signed in. The sign-in check, `Authenticated`, lives in the server crate today. A module can't depend on the server crate, because the server depends on every module.

Out of scope: how the web app is built and where each module's pages live, which the next decision covers. Search and background jobs don't exist yet. They join the interface when they do.

## Decision drivers

1. **Decision 1's rules.** Modules use core only through its public API, own their tables, and share no in-memory state except through core.
2. **Off means off.** A module that's off starts no tasks and answers no requests.
3. **Security checks happen once.** The host check, the cross-site check, sign-in, and the home-network check for admin pages apply to every module without each module repeating them.
4. **One developer.** Adding a module should touch as few places in the server as possible.
5. **The compiler checks it.** A module that doesn't fit the interface fails to build.
6. **A module can still move into its own process later,** as decision 1 keeps possible.

## Options

### Option 1: The server wires each module by hand

Each module crate exposes ordinary functions, such as `accordion_drive::routes()` and `accordion_drive::run()`, and the server calls each one by name.

Pros:

- **The least code.** There's no shared interface to design.
- **Each module can differ,** with whatever setup it needs.

Cons:

- **On and off, dependencies, startup, and shutdown are repeated** in the server for every module. Missing one is a quiet bug, such as a background task that starts while its module is off.
- **The Modules page needs code per module,** rather than reading a list.
- **Nothing makes modules consistent.** Each one could pick its own URLs and its own way to report errors.

### Option 2: A shared `Module` trait

A small crate defines a `Module` trait. Each module implements it, and the server keeps a list of modules and handles them all the same way.

Pros:

- **One code path** for on and off, dependencies, startup, shutdown, and routing.
- **The Modules page reads the list.** Each module reports its name, what it requires, and its data size through the trait.
- **Adding a module is one line** in the server's list.
- **URLs are consistent by construction,** because the server mounts each module's routes under paths named after the module.

Cons:

- **One more crate,** and an interface to keep in step with every module. A change to the trait means updating each module, but the compiler finds every place.
- **The trait has to stay general.** Anything only one module needs belongs in that module, not in the trait.

### Option 3: Plugins loaded at runtime

Modules are dynamic libraries or WebAssembly components that the server loads when it starts.

Pros:

- **Modules ship separately** from the server, and third parties could write them.

Cons:

- **Rust has no stable ABI** for dynamic libraries, and WebAssembly components can't share core's database and storage without an internal API, which is the cost decision 1 avoided.
- **Decision 1 ships modules as tested sets** in one binary. Loading them separately gives that up.

## Comparison

| Driver | Option 1: by hand | Option 2: trait | Option 3: plugins |
|---|---|---|---|
| Decision 1's rules | Yes | Yes | Needs an internal API |
| Off means off | Per module, by hand | Once, in the server | Yes |
| Security checks happen once | Yes | Yes | Yes |
| Places to change for a new module | Several | One | One, plus packaging |
| The compiler checks it | Yes | Yes | No |
| Can move into its own process | Yes | Yes | Yes |

## Recommendation

**Option 2: a shared `Module` trait, in a new crate called `accordion-module` in the `accordion-core` repo.**

Option 1 is less work for the first module and more for every one after it, and the work it repeats is the work that's easiest to get wrong, such as keeping a module that's off from running. Option 3 brings back the internal API that decision 1 avoided.

### The `accordion-module` crate

The crate holds what every module needs to plug into the server, and nothing else:

- **The `Module` trait.** A module reports its ID, such as `drive`, its name for the **Modules** page, and the IDs of modules it requires. It returns its routes, and runs its background work until the server stops it.
- **Core's services.** A module starts with the database, identity, and storage. It runs its own migrations in the database under its own ID, so it owns its tables (decision 2).
- **Sign-in checks.** `Authenticated` moves here from the server crate, and finds a session cookie or bearer token as it does now. A second check, for WebDAV and the other standard protocols, accepts a username and app password and never the account password, as [decision 3](0003-sessions-and-tokens.md) requires. Both find identity in the module's state, so a module doesn't depend on the server.
- **Error responses.** The server's `ApiError` moves here, so every module answers errors with the same JSON shape and the same plain messages.

Keeping this out of the main `accordion-core` crate keeps core free of web code. Core stays usable by a command-line tool or a sandboxed worker that never serves a request.

### URLs

The server mounts each module's routes under paths named after its ID, so no module can claim another's paths:

| Path | What it's for | How it signs in |
|---|---|---|
| `/api/<id>/…` | The module's JSON API, for the web app and the desktop and mobile apps | Session cookie or bearer token |
| `/dav/<id>/…` | WebDAV, CalDAV, or CardDAV, for standard clients | Username and app password |
| `/<id>/…` | The module's pages in the web app | Session cookie |

For Drive, that's `/api/drive/`, `/dav/drive/`, and `/drive/`. The server keeps every other path, such as `/`, `/api/session`, `/setup`, `/health`, and the admin pages. Calendar and Contacts will need `/.well-known/caldav` and `/.well-known/carddav`. The trait adds a way to claim those when they ship.

### Security checks

The server applies its checks around every module's routes: the host check that blocks DNS rebinding (decision 5), the cross-site check, and the home-network check for any admin pages a module adds. WebDAV clients don't send the headers the cross-site check looks for, so it lets them through. Browsers do send them, so a website can't use a browser's saved WebDAV password to change files.

### On and off

- **The server stores which modules are on,** in its own table. The setup wizard turns on Drive and Notes by default, as the spec says.
- **A module that's off isn't started.** The server never calls it, so it runs no tasks and holds no memory beyond its code. Requests to its paths get the same `404 Not Found` as any unknown path.
- **Turning a module on or off takes effect without a restart.** The server starts or stops the module's background work and swaps in routes with or without it. A restart would drop everyone's open connections and interrupt their uploads, for a change that affects one module.
- **Dependencies are checked by the server.** It won't turn off Drive while Docs or Notes is on, and it turns on what a module requires, after asking the admin.
- **Migrations run when a module first starts,** so a module that's never been on has no tables.
- **Turning a module on for some groups** is a check the server makes before passing a request to the module, once groups exist. Modules don't check it themselves.

### Revisit this decision if

- A module moves into its own process. The trait becomes the place to put a remote implementation.
- The trait keeps growing methods that only one module uses.
- Swapping routes at runtime causes bugs that a restart wouldn't.

## Consequences

- **A new `accordion-module` crate in `accordion-core`,** with the trait, core's services, both sign-in checks, and error responses. `auth.rs` and `error.rs` move there from the server crate.
- **App passwords in identity,** with the `identity_app_passwords` table that decision 3 lists, before Drive's WebDAV. The server adds pages to create and delete them.
- **The server keeps a list of modules,** a table of which ones are on, and, later, the **Modules** page.
- **A new `accordion-drive` repo,** created with `scripts/new-repo.sh`, as the first module.
- **The next decision covers the web app:** how it's built, and where each module's pages live.
