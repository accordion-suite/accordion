# 10. Backups a compromised server can't delete

- **Status:** Accepted
- **Date:** October 26, 2026

## Context

[Decision 8](0008-backups.md) backs up each server with restic to a local disk, an SFTP server, or an S3-compatible bucket. To run unattended, the server keeps everything it needs to reach those targets: the recovery key, the bucket's access key, and the SSH key. Those credentials can delete backups as well as add them.

So anyone who takes over the server can delete its backups too. Ransomware does exactly that: it encrypts the files, then deletes or encrypts the backups, so there's nothing to restore. Decision 8 left this as an open question: should Accordion use append-only REST targets or S3 Object Lock, and should either be on by default?

A few facts about restic shape the answer:

- **restic's REST server has an append-only mode.** In it, the server refuses every delete except restic's lock files, so a client can add backups but never remove one. [rest-server](https://github.com/restic/rest-server) is restic's own server, and rclone can serve the same protocol with `rclone serve restic --append-only`.
- **Removing old backups needs full access.** restic's `forget` and `prune` commands have to delete files. On an append-only target, someone with direct access to the repository has to run them.
- **Removing old backups automatically isn't safe there.** The [restic docs](https://restic.readthedocs.io/en/latest/060_forget.html) warn that an attacker who can add snapshots can add fake ones, timed so that a `--keep-daily` or `--keep-weekly` rule removes the real ones. `--keep-within` helps, but a gap of a week or more still lets the attacker win.
- **restic has no built-in support for S3 Object Lock.** restic reuses the files it has already uploaded for as long as any backup needs them, often for years. A bucket's default lock covers each file for a fixed time after upload, so old files that current backups still need become deletable again. The workaround is to extend every lock on a schedule, which restic doesn't do.

## Decision drivers

1. **A server taken over by an attacker can't destroy its own backups.**
2. **A family can set it up,** following a guide, on hardware or services they're likely to have.
3. **Nothing that can't be checked.** Accordion should be able to confirm the protection is there, not assume it.
4. **Restoring stays the same,** through the setup wizard or by hand with `restic`.
5. **Little to build and maintain** for one developer.

## Options

### Option 1: A REST server in append-only mode

The admin runs rest-server, or `rclone serve restic`, in append-only mode on another machine, such as a NAS, a spare Raspberry Pi with a USB disk, or a relative's computer. Accordion gains a fourth kind of target, a REST server, and backs up to it as usual. Accordion doesn't remove old backups there, because the server won't let it.

Pros:

- **The protection is enforced by another machine,** which the attacker doesn't control. No setting on the Accordion server can turn it off.
- **It's what restic was designed for,** and rest-server comes from restic's own developers.
- **Accordion can check it.** When it tries to remove a test snapshot, an append-only server refuses with "403 Forbidden".
- **It's the base for the second Accordion box** that decision 8 plans. That box will serve the same protocol in append-only mode.

Cons:

- **Old backups pile up.** Nothing is removed until someone cleans up by hand at the other machine, and that cleanup needs the recovery key. Family data is mostly photos and documents that are added and rarely deleted, so the total grows slowly, but it does grow.
- **It needs another machine** that's on when backups run, and someone to set up rest-server on it.
- **HTTPS on a home network** usually means a self-signed certificate, which Accordion has to trust safely.

### Option 2: S3 Object Lock

The admin creates a bucket with Object Lock and a default retention period at a provider that supports it, such as Backblaze B2 or Amazon S3. No file can be deleted until its retention ends.

Pros:

- **No other machine to run.** Many families already pay for a bucket.
- **The provider enforces it.** In compliance mode, nobody can shorten a lock, not even the account owner.

Cons:

- **restic doesn't support it.** Locks run out on files that current backups still use, so the protection fades unless something extends every lock on a schedule. Accordion would be building and maintaining that, for every provider.
- **Uploads may need extra headers.** Amazon requires a checksum header on uploads to a bucket with default retention. Whether restic sends one, and how each provider behaves, would need testing.
- **Recovering is awkward.** After an attack, the repository looks damaged until someone restores the bucket to an earlier point in time with a separate tool.
- **Removed files keep costing money** until their locks run out.

### Option 3: Rotating offline disks

The household keeps two USB disks and swaps them, say, once a month. The one that's unplugged, ideally kept at another house, can't be reached by anything on the server.

Pros:

- **Nothing can reach an unplugged disk.** It's the strongest protection, and needs no other software.
- **It works today.** A missing disk already doesn't stop backups to other targets.

Cons:

- **It depends on people remembering.** A disk that isn't swapped for six months protects six-month-old data.
- **The newest backups aren't protected.** An attack between swaps can destroy the disk that's plugged in.

### Option 4: Snapshots on the backup machine

The admin backs up over SFTP to a NAS whose file system takes its own read-only snapshots, such as ZFS on TrueNAS or Btrfs on a Synology.

Pros:

- **No change to Accordion.**
- **Many NAS models can do it already.**

Cons:

- **Accordion can't check it.** It can't tell whether snapshots are on, how long they're kept, or whether the SSH account can delete them.
- **Restoring needs the NAS's own tools** to bring back an earlier snapshot first.

## Comparison

| Driver | Option 1: REST append-only | Option 2: Object Lock | Option 3: offline disks | Option 4: NAS snapshots |
|---|---|---|---|---|
| A compromised server can't delete backups | Yes | Only while locks last | Only the unplugged disk | Yes, if set up right |
| A family can set it up | With a guide | With a guide | Yes | With a NAS and a guide |
| Accordion can check it | Yes | Partly | Partly | No |
| Restoring stays the same | Yes | No, after an attack | Yes | No, after an attack |
| Work for one developer | Low | High | Very low | None |

## Recommendation

**Option 1: a REST server in append-only mode,** as a fourth kind of target. The admin guide also recommends option 3 to households that won't run another machine, and describes option 4 for those that already have a NAS that takes snapshots.

Option 1 is the only one where another machine enforces the protection, Accordion can confirm it, and restoring works the usual way. Its cost, backups that pile up, is real but slow, and it's the price of a target that can't be told to delete anything.

### The REST server target

The admin enters:

- **The address,** such as `https://nas.home.arpa:8000/accordion`.
- **The username and password** from the REST server's password file.

Accordion passes the username and password to restic in `RESTIC_REST_USERNAME` and `RESTIC_REST_PASSWORD`, never in the address, so they don't show up in process lists or logs.

**HTTPS only.** A REST server on a home network usually has a self-signed certificate. If the system doesn't trust the server's certificate, Accordion trusts the one it finds the first time it connects, the way the SFTP target trusts a host key. It saves that certificate and passes it to restic with `--cacert` from then on. A different certificate later stops backups to that target, with a message saying so, until the admin removes the target and adds it again. A certificate the system already trusts, such as one from Let's Encrypt, isn't pinned, because it's replaced every few months.

### Finding out whether it's append-only

When the target is added, Accordion makes a tiny test snapshot of an empty folder, then tries to remove it:

- **If the server refuses,** the target is append-only. Accordion never runs `forget` or `prune` there, and the backups page says: "This server keeps every backup, so a compromised server can't delete them."
- **If it works,** the target behaves like any other, with the usual retention from decision 8. The backups page says this target doesn't protect against a compromised server, and links to the guide.

The test snapshot has its own tag, so restore lists never show it. If a server that allowed removing snapshots later switches to append-only, the next `forget` or `prune` gets the same refusal, and Accordion treats the target as append-only from then on.

### Keeping it tidy

Accordion shows how much space each append-only target uses, and its newest and oldest backups. It never removes anything there. When space runs low, the admin guide offers two ways, both run on the other machine:

1. **Start a new repository** on a fresh disk or folder, and keep the old one until the new one has a few months of backups. This needs no restic commands and leaves nothing for an attacker to exploit.
2. **Clean up by hand,** with restic and the recovery key. The guide lists the snapshots first, so the admin can spot ones that don't belong, then uses `forget --keep-within` as the restic docs recommend, followed by `prune`.

### Restoring

Restoring from a REST server works like any other target, in the setup wizard or with `restic restore`. The wizard gains the REST server as a fourth kind of target, with the same certificate check.

### Revisit this decision if

- restic adds support for S3 Object Lock, such as extending locks on files that backups still use. Option 2 would then need no second machine.
- The second Accordion box arrives. It will serve this same protocol in append-only mode, and could also take over the cleanup, since it can check the snapshots before removing any.

## Consequences

- **Core gains a REST server target,** with HTTPS, a pinned certificate, the append-only test, and no `forget` or `prune` on append-only targets.
- **The backups page** adds the REST server to the kinds of target, says whether each target protects against a compromised server, and shows space used on append-only ones.
- **The setup wizard** can restore from a REST server.
- **CI tests it** with `rclone serve restic --append-only`, using the rclone that CI already installs for the bucket tests.
- **The admin guide** covers setting up rest-server on a NAS or a Raspberry Pi, rotating offline disks, NAS snapshots, and cleaning up an append-only target.
- **Decision 8's open question** about protection from a compromised box is answered here.

## Open questions

- **Should Accordion nudge households toward an append-only target?** For example, the health page could suggest one when every target can be deleted from the server.
- **Low space on the other machine.** rest-server doesn't report free space, so Accordion only finds out when a backup fails. The error should say what to do.
