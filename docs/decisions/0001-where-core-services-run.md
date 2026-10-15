# 1. Where core services run

- **Status:** Accepted
- **Date:** October 15, 2026

## Context

Every Accordion module (Drive, Docs, Mail, and Notes, then Calendar, Contacts, Photos, and Sheets) depends on shared core services: identity, search, sync, jobs, and storage. The spec describes "one server process per module behind a single gateway", but doesn't say how modules reach the shared services.

Modules live in separate repos with their own versions, and a suite release pins a set of versions that were tested together.

Not all of core is affected by this decision. Some parts are libraries in every design:

- **The sync engine.** The desktop and mobile apps run the same Rust code for offline merging.
- **Shared types and protocol definitions.**
- **Token verification,** so any component can check a sign-in without asking another process.

This decision is about the parts that hold state: identity (accounts, passkeys, and the OpenID Connect provider), search (the Tantivy index), jobs (the work queue), and the metadata database with its migrations.

Two facts constrain the design:

- **Tantivy allows one writer per index.** Only one process can update the search index.
- **Identity needs one owner.** Sign-in pages, passkey registration, and OpenID Connect endpoints have to live in one place.

## Decision drivers

1. **One developer.** Every internal API, process, and deployment piece is maintained by one person.
2. **Raspberry Pi hardware.** The idle stack must stay under 1.5 GB of RAM with all 1.0 modules on, and the minimum tier has 4 GB.
3. **A module that's off doesn't run** and uses no RAM.
4. **One search index and one sign-in** across all modules.
5. **Contained damage.** A bug in one module, especially in code that handles untrusted input, shouldn't expose everything.
6. **Separate versions, tested sets.** Modules are versioned on their own but ship as tested sets.
7. **Safe updates.** Updates are staged and roll back automatically on failure.
8. **Per-module resource use.** The admin **Modules** page shows RAM and disk use per module.

## Options

### Option 1: Shared library, one process per module

Core is a set of crates compiled into every module's binary. Each module process opens the database, search index, and file store itself.

Pros:

- Simple to start. There's no internal API to design.
- Matches the spec's "one binary per module".
- Modules are isolated in separate processes, and the OS reports RAM per module.

Cons:

- **Search can't be shared.** Only one process can write to a Tantivy index, so each module needs its own index, and searching everything means querying each one.
- **Identity still needs a single owner,** so it ends up as a separate service anyway. For identity, this option turns into option 2.
- **Shared database migrations have no owner.** Several processes race to run them at startup.
- Every core change means rebuilding and re-releasing every module.
- Mismatches between modules, such as different expectations about a shared table, only show up at runtime.

### Option 2: Shared services, one process per module

Core runs as its own process, or a few. Each module runs as a separate process and calls core over an internal API, such as HTTP or gRPC over Unix sockets. Modules include a small client library to make those calls.

Pros:

- **One owner for each piece of state:** one search index, one identity service, and one place that runs migrations.
- **Strong isolation.** Each module can run as its own system user, with its own database role and file paths. A bug in Mail can't read Drive's files directly.
- Modules restart, update, and turn on or off independently.
- The OS reports RAM and CPU per module, so the **Modules** page is straightforward.
- Modules don't have to be written in Rust, and an existing project could become a module.

Cons:

- **An internal API to maintain.** It has to be designed, documented, versioned, and kept backward compatible across module versions.
- **Distributed plumbing:** authentication between services, finding each other, health checks, and startup order.
- **Harder debugging.** Failures span processes, so you need tracing across them.
- **More memory.** Each extra Rust process adds roughly 20–50 MB at idle (an estimate to confirm). That's affordable on a Pi, but not free.
- Every search and permission check crosses a process boundary. Over a Unix socket the delay is small, but it adds up.
- Mismatches between versions show up at runtime or in integration tests, not at build time.

### Option 3: One server process, modules as crates

Core and every module compile into one server binary, built in the `accordion` repo. Modules stay in their own repos with their own versions and are pulled in as Rust crates, and the suite build pins which versions are used. A module that's off isn't started: its routes answer only when it's on, and its background tasks and caches don't run.

Untrusted input still runs in separate, sandboxed processes: file previews, OCR, transcription, Takeout import, and the mail protocol engine that handles SMTP and IMAP.

Pros:

- **Least work.** There's no internal API, service authentication, or discovery. Modules call core through ordinary Rust functions.
- **The compiler checks compatibility.** A module that doesn't match core's API fails to build, long before a release.
- **One process** owns the search index, identity, and migrations.
- **Lowest memory use,** and the simplest deployment, update, and rollback: one binary.
- **Easiest debugging.** One process, one log, one debugger.
- **The tested set is the build itself.** The server crate's `Cargo.toml` and `Cargo.lock` replace `release.toml`.

Cons:

- **No isolation between modules.** A vulnerability in one module's code can reach all the data the server can. Rust's memory safety and sandboxed workers for untrusted input reduce this risk, but isolation is weaker than in option 2.
- **One failure stops everything.** Tokio contains a panic to the request that caused it, but running out of memory, a deadlock, or an abort takes down every module. systemd restarts the server within seconds.
- **No per-module RAM figures from the OS.** The **Modules** page would show the server's total memory, plus each module's cache and data sizes.
- **Shared dependency versions.** All crates must agree on versions of shared dependencies such as Tokio and Axum, so one module can't upgrade them alone.
- **Rust only.** Every module must be a Rust crate.
- **Differs from the spec,** which says "one server process per module", "one binary per module", and "under 200 MB per module".

## Comparison

| Driver | Option 1: shared library | Option 2: shared services | Option 3: one server process |
|---|---|---|---|
| Work for one developer | Medium, growing | High | Low |
| RAM on a Pi | Medium | Highest | Lowest |
| Modules that are off use no RAM | Yes | Yes | Yes, apart from the binary's size |
| One search index and one sign-in | No | Yes | Yes |
| Isolation between modules | Separate processes | Separate processes, users, and database roles | Only for sandboxed workers |
| Version mismatches caught | At runtime | At runtime or in integration tests | At build time |
| Updates and rollback | Many binaries | Many binaries and API compatibility | One binary |
| RAM per module on the **Modules** page | Yes | Yes | Estimates only |

## Recommendation

**Option 3,** with sandboxed processes for untrusted input, and modules kept behind a clear interface so any of them can move into its own process later.

Option 1 fails driver 4 and turns into option 2 for identity and search, so the real choice is between options 2 and 3.

The main thing option 2 adds is isolation between modules. On a home server, most of the risk comes from untrusted input: email from the internet, uploaded files, and Takeout archives. Option 3 still isolates all of that. In exchange, option 2 costs an internal API, versioning across it, and running a distributed system, which is ongoing work for one developer.

Option 3 is also easier to reverse. To move a module into its own process later, add a remote implementation behind the same interface. Going from option 2 back to option 3 throws the API work away.

To keep that path open:

- Modules use core only through the public API of core's crates, never by reading core's database tables.
- Each module owns its own tables and migrations.
- Modules don't share in-memory state with each other except through core.

### Revisit this decision if

- Third-party modules, or modules in other languages, become a goal.
- A security review, such as the external audit planned before 1.0, finds that modules need isolating from each other.
- One module's resource use harms the others in practice.

## Consequences

- **Spec updates.** Change the architecture section and stack table from "one binary per module" to one server binary with sandboxed workers. Make the footprint target a total instead of a per-module figure, and show the server's total RAM on the **Modules** page.
- **A server crate in `accordion`.** It depends on core and each module by Git tag and builds the binary. Its `Cargo.toml` and `Cargo.lock` replace `release.toml`.
- **Local builds use local checkouts.** Add a `.cargo/config.toml` at the workspace root, with `[patch]` entries that point at the sibling repos.
- **Module repos become libraries.** Each one exposes a function that registers its routes, jobs, and search content with core.
- **The mail protocol engine stays a separate process** in every option, whether that's Stalwart or Postfix with Dovecot. The Mail module runs inside the server and handles the web client, settings, and DNS setup, and the engine uses Accordion's identity service for sign-in.
