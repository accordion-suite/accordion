# Decision records

Each file in this folder records one significant decision about how Accordion is built: the context, the options considered, and why one was chosen. Decisions aren't deleted when they change. A new record replaces the old one, and the old one's status points to it.

| Number | Decision | Status |
|---|---|---|
| [1](0001-where-core-services-run.md) | Where core services run | Accepted |
| [2](0002-metadata-database.md) | Which database stores metadata | Accepted |
| [3](0003-sessions-and-tokens.md) | How sign-in sessions and tokens work | Accepted |
| [4](0004-encryption-at-rest.md) | How user data is encrypted at rest | Accepted |
| [5](0005-server-name-and-certificates.md) | How the server gets its name and certificate | Accepted |
| [6](0006-file-storage.md) | How file contents are stored on disk | Accepted |
| [7](0007-module-interface.md) | How modules plug into the server | Accepted |
| [8](0008-backups.md) | How backups work | Accepted |
| [9](0009-releases-and-installing.md) | How Accordion is released and installed | Accepted |
| [10](0010-backups-a-compromised-server-cant-delete.md) | Backups a compromised server can't delete | Accepted |

To add a decision, copy the structure of an existing record, give it the next number, and add it to this table.
