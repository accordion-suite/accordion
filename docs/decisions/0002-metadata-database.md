# 2. Which database stores metadata

- **Status:** Accepted
- **Date:** October 15, 2026

## Context

Accordion keeps user content in open formats outside any database: files on disk, mail in the mail store, and notes as Markdown files. The database holds metadata: accounts, sessions, passkeys, shares, permissions, file and folder records, version history, the job queue, the audit log, and each module's own tables.

The spec's stack table says "PostgreSQL; SQLite on 1–3 user installs", so it plans to support both. Identity, jobs, and storage in `accordion-core` all need a database, so this has to be settled before they're built.

[Decision 1](0001-where-core-services-run.md) changes the picture. Accordion is now one server process, so only one process ever opens the database. Sandboxed workers don't touch it: they get their input and return their output through the server. That's the case SQLite is designed for.

The search index (Tantivy) and the mail engine's own storage are out of scope here.

## Decision drivers

1. **One developer.** Every database supported means more SQL to test, more migrations, and a bigger CI matrix.
2. **Boring to run.** The household admin must never repair a database, run a manual upgrade, or tune settings.
3. **Raspberry Pi hardware.** The idle stack must stay under 1.5 GB of RAM with all 1.0 modules on, and the minimum tier has 4 GB.
4. **Four install methods:** appliance image, Docker Compose, NAS app stores, and .deb or .rpm packages. Each one has to install, update, and roll back the database.
5. **Safe updates.** Updates take a snapshot, update, run a health check, and roll back on failure. Schema migrations are part of that.
6. **No data loss on power cut.** The spec requires write-ahead logging and an fsync on commit.
7. **Backups.** Nightly, encrypted, and restorable to a fresh box in under an hour for 500 GB.
8. **Scale.** Up to 25 users on the small team tier, measured against 10 active users on the family tier.

## Options

### Option 1: PostgreSQL only

Every install runs a PostgreSQL server next to Accordion.

Pros:

- **Many writers at once.** Concurrent writes don't wait on a single lock, so heavy write load stays smooth.
- **Full ALTER TABLE support,** so schema migrations are simpler to write.
- **A rich feature set:** more data types, better query planning for complex queries, and row-level locking.
- **Room to grow** if Accordion ever needs a database on a separate machine, though multi-site clustering is a non-goal.

Cons:

- **A second service to install, run, and monitor** on every install method. Docker Compose needs a second container, and the .deb and .rpm packages depend on the distribution's PostgreSQL.
- **Major version upgrades are manual work.** PostgreSQL releases a major version every year, and moving the data directory to a new one needs `pg_upgrade` or a full dump and restore. This is a well-known pain point for self-hosted apps, and it conflicts with driver 2.
- **The version varies by install method.** Each Linux distribution ships a different PostgreSQL version, so the tested set from decision 1 no longer includes the database.
- **More memory.** A PostgreSQL server tuned for a small machine uses roughly 50–150 MB at idle (an estimate to confirm). That's affordable, but it comes out of the 1.5 GB budget.
- **Harder snapshots and rollback.** A consistent snapshot needs `pg_dump` or a base backup, and rolling back a failed migration means restoring one.
- **Network-level security.** The database listens on a socket and has its own users and passwords to set up and protect.

### Option 2: SQLite only

The server opens a SQLite database file in write-ahead log (WAL) mode. SQLite is compiled into the binary, so there's nothing else to install.

Pros:

- **Nothing to run.** No second service, no ports, and no database users. Installing, updating, and removing Accordion covers the database too.
- **Upgrades ship with the binary.** The SQLite version is part of the build, so it's in the tested set and never drifts. There are no major version upgrades to manage.
- **Simple snapshots and rollback.** Before a migration, the server copies the database with SQLite's online backup API. Rolling back means putting the copy back.
- **Simple backups.** The backup job takes the same consistent copy and adds it to the encrypted repository.
- **Lowest memory use.** The page cache is a setting the server controls, typically a few MB to tens of MB.
- **Fast reads.** Queries are function calls inside the process, with no network round trip, so many small queries cost very little.
- **Proven for this kind of app.** Self-hosted apps such as Vaultwarden and Home Assistant use SQLite by default.

Cons:

- **One writer at a time.** Readers never block, but write transactions run one after another. Each write is short, so this is fine at 25 users, but long write transactions block every other write. Imports and other bulk jobs must commit in small batches.
- **Write speed depends on the disk.** With an fsync on every commit, as driver 6 requires, a fast SSD handles hundreds to thousands of small write transactions per second. A slow USB drive handles far fewer (estimates to confirm in benchmarking on the reference hardware). Grouping related writes into one transaction keeps this in check.
- **Limited ALTER TABLE.** Some schema changes need the table rebuilt: create a new table, copy the rows, and swap the two. Migrations are more work to write.
- **It needs a local file system.** SQLite can corrupt a database on network shares (NFS and SMB) and on some NAS storage layers. Installers must keep the database on local app storage, and the server should refuse to start on a network file system.
- **No path to a separate database machine.** That's acceptable while multi-site clustering is a non-goal.

### Option 3: Both, chosen by install size

PostgreSQL for larger installs and SQLite for 1–3 users, as the spec describes now.

Pros:

- Small installs get SQLite's simplicity, and larger ones get PostgreSQL's write concurrency.

Cons:

- **Two of everything.** Every query, migration, and test runs against both databases. The two SQL dialects differ in data types, date handling, and JSON functions, so code is limited to what both support, or it branches.
- **Bugs that appear on only one.** The less-used database gets less real-world testing, and its bugs surface late.
- **A migration tool.** A household that grows past three users needs a way to move from SQLite to PostgreSQL, which is one more thing to build and test.
- **It has both options' operations costs.** Larger installs still face PostgreSQL's upgrades and second service.
- **The 1–3 user line doesn't match the hardware tiers.** The family tier allows up to 10 users on a Pi 5, so most families would run PostgreSQL on the hardware where its memory and operations costs hurt most.

### Not considered: an embedded key-value store

Stores such as RocksDB or redb are fast and embedded, but they have no SQL, no schema, and no query planner. Every query, index, and migration would be hand-written, which is more work than SQLite for no gain at this scale.

## Comparison

| Driver | Option 1: PostgreSQL | Option 2: SQLite | Option 3: both |
|---|---|---|---|
| Work for one developer | Medium | Low | High |
| Boring to run | Second service and manual major upgrades | Nothing extra to run | Both sets of costs |
| RAM on a Pi | Roughly 50–150 MB more | Lowest | Depends on the install |
| Install methods | Each one bundles or depends on PostgreSQL | Built into the binary | Both |
| Snapshot and rollback | `pg_dump` and restore | Copy the file | Both |
| Database version in the tested set | No | Yes | Only for SQLite |
| Write concurrency | High | One writer at a time | Depends on the install |
| Fit with one server process | Good | Ideal | Good |

## Recommendation

**Option 2, SQLite only,** in WAL mode with `synchronous = FULL` so every commit is flushed to disk.

The main thing PostgreSQL adds is concurrent writes, and Accordion doesn't need them. With one server process, at most 25 users, and user content stored outside the database, write load is low. Every other driver favors SQLite: nothing extra to run, no upgrades to manage, the simplest backups and rollback, and the lowest memory use. Supporting both databases costs the most work of all and keeps PostgreSQL's operations costs anyway.

To make SQLite work well:

- **Keep write transactions short.** Bulk jobs, such as Takeout imports, commit in batches.
- **Group related writes** into one transaction instead of committing each row.
- **Send all writes through one writer** in the server, so tasks queue in the server instead of waiting on SQLite's lock.
- **Start with one database file.** Because each module owns its tables (decision 1), a module's tables can move to their own file later if that helps write throughput or lets the module move into its own process.

### Revisit this decision if

- Benchmarks on the reference hardware show write latency that users notice.
- Accordion needs to support well over 25 users on one server.
- Running the database on a separate machine, or across several servers, becomes a goal.

## Consequences

- **Spec updates.** Change the stack table's metadata row and the architecture diagram from "PostgreSQL or SQLite" to SQLite.
- **No database service in any install method.** The Docker Compose file, NAS packages, and .deb and .rpm packages contain only Accordion.
- **Storage location checks.** Installers put the database on local app storage, and the server refuses to start if the database is on a network file system.
- **Backups and updates use SQLite's online backup API** for consistent copies.
- **Migrations follow SQLite's rules,** including the table rebuild pattern for changes ALTER TABLE can't make.
- **A follow-up choice of Rust library,** for example `rusqlite` with a dedicated writer thread, or `sqlx`. It doesn't need its own decision record unless it turns out to be close.
