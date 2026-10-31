# 14. Thumbnails, and how workers are sandboxed

- **Status:** Accepted
- **Date:** October 31, 2026

## Context

Drive's grid shows an icon for each file. People expect the photo itself, as Google Drive shows it, and the Drive settings already have a **Thumbnails** switch waiting for them. Thumbnails are step 3 of the work before the first full release.

Making a thumbnail means decoding a file someone uploaded. Image decoders are mostly written in C, and they're a common target: a crafted image can crash a decoder, use all the memory, or run an attacker's code. [Decision 1](0001-where-core-services-run.md) says code that handles untrusted input runs in separate, sandboxed worker processes, but no worker exists yet. This is the first, so this record also sets how every later worker is sandboxed: video frames, Office previews, OCR, Takeout import, and the text extraction that search needs in step 4.

Today the server runs as the `accordion` user under a systemd unit that already limits it: it can't gain privileges, write outside its data folder, create namespaces, or make memory both writable and executable. A worker started by the server inherits those limits. But it also inherits the `accordion` user, which can read the whole data folder: the database, every file, the DNS token, and the private keys. The sandbox's main job is to take that access away.

Accordion installs on Debian 12 or later, Ubuntu 24.04 or later, and Raspberry Pi OS based on Debian 12, on x86_64 and ARM64. The smallest supported server is a Raspberry Pi with 4 GB of RAM.

## Decision drivers

1. **A compromised decoder reaches nothing.** It can't read the database or other people's files, write anywhere, or connect to the network.
2. **The formats families have.** JPEG and PNG, and HEIC, which iPhones save photos in. PDFs and videos soon after.
3. **Fast and small on a Raspberry Pi.** A 48-megapixel photo shouldn't take seconds or hundreds of MB to shrink.
4. **One developer.** The sandbox has to be understood, tested, and kept working by one person.
5. **The same on every install,** and testable with `cargo test` in CI, not only on a real install.
6. **Security fixes keep coming** for whatever decodes files, without Accordion releasing for each one.
7. **No new connections** ([decision 11](0011-the-network-page.md)).

## Options: what makes the thumbnails

### Option A: Rust decoders, in a sandboxed copy of the server

The server runs its own program in a thumbnail mode, which decodes with Rust crates such as `image`, `zune-jpeg`, and `png`.

Pros:

- **Memory-safe decoders,** so a crafted file is far less likely to run code. It can still use all the memory or panic, so it still needs the sandbox.
- **Nothing new to install,** and the package stays reproducible.

Cons:

- **No HEIC, AVIF, or PDF.** The Rust crates that read them wrap the same C libraries. iPhone photos would show icons.
- **Slow and large on big photos.** The crates decode the whole image, then shrink it: about 150 MB of memory for a 48-megapixel photo.
- **Accordion releases for each decoder fix.**

### Option B: libvips from the distribution

[libvips](https://www.libvips.org) is the image library behind many photo services, built for making thumbnails quickly in little memory. Its `vips` program reads a file from standard input and writes a thumbnail to standard output. The package depends on `libvips-tools`, which Debian 12 has at version 8.14 and Ubuntu 24.04 at 8.15.

Pros:

- **The formats families have:** JPEG, PNG, GIF, WebP, TIFF, HEIC, and AVIF, through libheif, and PDFs through poppler.
- **Fast and small.** For JPEG and HEIC, it decodes at a fraction of the size, so a 48-megapixel photo shrinks in about the time and memory of a small one.
- **Fixes come with system updates.** The distribution patches libvips and its decoders, and servers install the fixes with their other updates.
- **It can refuse risky formats.** With `VIPS_BLOCK_UNTRUSTED` set, available since 8.13, libvips won't use the decoders its developers don't trust with hostile files, such as ImageMagick's.

Cons:

- **C decoders.** A crafted file can run code inside the worker, so everything rests on the sandbox.
- **A bigger install.** `libvips-tools` brings in libheif, poppler, and more, tens of MB to measure.
- **Versions differ by distribution,** so the tests have to cover the oldest supported one.

### Option C: ImageMagick

ImageMagick reads the most formats of any library.

Pros:

- **Almost every format,** including camera RAW files.

Cons:

- **The largest attack surface,** with a long history of security problems from its many decoders. Its own documentation recommends a policy file to turn most of them off.
- **Slower and larger than libvips** for thumbnails.

## Options: how the worker is sandboxed

### Option 1: bubblewrap

[bubblewrap](https://github.com/containers/bubblewrap) runs a program in its own namespaces, with only the folders it's given.

Cons:

- **It doesn't work under Accordion's unit,** which turns namespaces off with `RestrictNamespaces=yes`. Turning them back on would give the whole server that ability.
- Ubuntu 24.04 limits namespaces for unprivileged users through AppArmor, so it needs an extra profile there.

### Option 2: A systemd service for each thumbnail

The package installs a socket and a service template. The server connects to the socket for each thumbnail, systemd starts a fresh instance with its own throwaway user (`DynamicUser=yes`), no network, and a memory limit, and the server streams the file to it.

Pros:

- **A different user,** with no access to the data folder at all.
- **Real memory and CPU limits** through the instance's control group.
- **systemd does the sandboxing.** It's declared in the unit, and widely used.

Cons:

- **Only where systemd runs the server.** Development, CI tests, and any future container image need a second way to run workers, so the sandbox that ships isn't the one the tests use.
- **A unit started for each file,** which adds some time for each thumbnail. It's acceptable, but adds up when the server catches up on thousands of photos.
- **More moving parts in the package:** a socket, a template, and their permissions, which the upgrade test has to cover.

### Option 3: The worker locks itself down before it starts

The server starts its own program as a small launcher, `accordion sandbox`, with the file open on standard input and a pipe on standard output. The launcher shuts itself in, then runs `vips`, which inherits every limit:

- **Landlock** lets the worker read only the system's program and library folders, such as `/usr`, and write nowhere. The data folder, the database, and other people's files are out of reach. The file to read is already open on standard input, so the worker needs no path into the data folder.
- **A seccomp filter** refuses system calls the worker has no use for: making network connections, tracing other processes, running other programs once `vips` starts, and changing its own limits.
- **Resource limits** cap its memory, CPU time, and the size of what it writes. The server also stops it after 30 seconds.
- **Nothing else from the server:** an empty environment except for libvips's settings, no other open files, and a new process group, so stopping it stops everything it started.

Landlock and seccomp need no privileges. They work for any user, inside the existing unit, and in tests.

Pros:

- **One way everywhere.** `cargo test` in CI runs the same sandbox that ships, and can check that it holds, such as that the worker can't open the database.
- **Starts in milliseconds.**
- **Nothing to install or configure.** It's code in the server, using the `landlock` and `seccompiler` crates.

Cons:

- **The same user as the server.** Landlock alone keeps the worker out of the data folder. If the kernel doesn't support Landlock, there's no sandbox worth having, so the server has to refuse to run workers.
- **Memory limits are rougher** than a control group's. A cap on address space also counts memory that's reserved but never used, so it needs some headroom.
- **A seccomp filter to keep correct.** A filter that's too tight breaks `vips` after a distribution update, and one that's too loose protects less. Refusing known-dangerous calls, rather than allowing only known-safe ones, keeps it simple.

## Comparison

| Driver | A: Rust | B: libvips | C: ImageMagick |
|---|---|---|---|
| HEIC and PDF | No | Yes | Yes |
| Speed and memory on a Pi | Slow on big photos | Fast and small | Slower |
| Risk inside the worker | Low | Medium | High |
| Decoder fixes | With Accordion releases | With system updates | With system updates |

| Driver | 1: bubblewrap | 2: systemd service | 3: Landlock and seccomp |
|---|---|---|---|
| Works in the existing unit | No | Yes, beside it | Yes |
| Different user from the server | Yes | Yes | No |
| Same sandbox in tests | Yes | No | Yes |
| Memory limits | Rough | Exact | Rough |
| Needs kernel support | Namespaces | systemd | Landlock |
| Work for one developer | Medium | Medium | Medium |

## Recommendation

**libvips (option B), in a worker that locks itself down (option 3).**

libvips is the only option that shows iPhone photos and PDFs, and it's the fastest and smallest on a Pi. Its decoders are C, so the sandbox matters more than for Rust decoders. Option 3 keeps the worker out of the data folder, off the network, and inside memory and time limits, and it's the only sandbox that works the same in tests as in the package. Option 2 is stronger, because the worker runs as a different user, but every thumbnail made in development and CI would skip it.

### The sandbox

- **The launcher is the server's own program,** run as `accordion sandbox <limits> -- <program> <arguments>`. It applies Landlock, the seccomp filter, and the limits to itself, then runs the program. Doing this in a separate program, rather than between `fork` and `exec` in the server, keeps it out of the server's threads.
- **Landlock is required.** If the kernel doesn't support it, the server makes no thumbnails, and **Server health** says why. Every other limit is always available.
- **The unit allows the sandbox's system calls.** Its filter refuses calls outside `@system-service`, so it gets an exception for the Landlock calls if they're not in that set.
- **Every later worker uses the same launcher,** with its own program, limits, and folders it can read. A worker that needs a scratch folder, such as LibreOffice, gets a new empty one that's deleted afterward.

### Thumbnails

- **One size,** 480 pixels on the longer side, saved as WebP. The grid shows it cropped to fit the card, and it's sharp on high-resolution screens.
- **Formats:** JPEG, PNG, GIF, WebP, TIFF, HEIC, and AVIF first. PDFs show their first page, if poppler works under `VIPS_BLOCK_UNTRUSTED`. Videos, through ffmpeg, and Office files, through LibreOffice, follow in the same sandbox, each with its own decision on size and limits.
- **libvips runs with `VIPS_BLOCK_UNTRUSTED=1`,** and the server checks a file's first bytes before starting the worker, so a file that claims to be a JPEG but isn't is never decoded.
- **Limits for each thumbnail:** 1 GB of address space, 20 seconds of CPU time, a 10 MB result, and 30 seconds in all. Files over 200 MB get no thumbnail.

### Where thumbnails are kept, and when they're made

- **Core owns thumbnails,** not Drive, because Photos, Mail's attachments, and Docs need them too. Core keeps a queue in its own table and a cache folder, and modules ask for thumbnails through core's API ([decision 1](0001-where-core-services-run.md)).
- **Thumbnails are named by the file's content hash** ([decision 6](0006-file-storage.md)), in `cache/thumbnails/` in the data folder. Copies and identical uploads share one thumbnail, and so do versions that didn't change. A thumbnail goes when the last file with its contents goes.
- **They're made in the background,** one at a time and at low priority, so uploads and browsing come first. An upload queues one. After the update that adds thumbnails, the server works through existing files, newest first.
- **A file that fails isn't tried again** until the worker changes, such as after an update. The failure is recorded with its contents' hash.
- **Backups leave the cache out.** Thumbnails can be made again, so a restore rebuilds them.
- **Drive serves them at its own address,** such as `/api/drive/items/<id>/thumbnail`, after checking the person can see the file. A content hash in the address lets browsers keep each thumbnail for a year. Lists say whether each file has a thumbnail, and the grid shows the icon until it does.

### Testing

- Tests run the real sandbox and check that a worker can't read the data folder, write a file, open a network connection, or outlast its time limit.
- A test file that expands to a huge image, made to use up memory, has to fail within its limits without affecting the server.
- CI installs `libvips-tools` on Ubuntu 24.04, and tests on the Raspberry Pi cover Debian 12's older libvips. The strace check from decision 11 confirms workers make no connections.
- The upgrade test checks that thumbnails appear for the files it uploads.

### Revisit this decision if

- Landlock turns out to be missing or turned off on a supported system, such as a Raspberry Pi OS kernel.
- A container image becomes a supported install, and its seccomp profile refuses the Landlock calls.
- A memory-safe library reads HEIC and PDF well enough to replace libvips.
- A security review finds that workers need a separate user, which option 2 gives.

## Consequences

- **A new package dependency,** `libvips-tools`, and the package's install size grows.
- **A `sandbox` command in the server,** with the `landlock` and `seccompiler` crates.
- **The unit's system call filter** gains the Landlock calls, if they're not already allowed.
- **Core gains a thumbnail service:** a migration for its queue and failures, the cache folder, and an API for modules.
- **Drive gains a thumbnail address,** and its lists say which files have one. The web app shows them in the grid when the **Thumbnails** setting is on.
- **Backups exclude** `cache/` in the data folder.
- **Server health** gains a **Thumbnails** check: whether the sandbox works, and how many files are waiting.

## Open questions

- **Landlock on each supported system.** Check that Debian 12's, Ubuntu 24.04's, and Raspberry Pi OS's kernels turn it on, and which version each has, on the Raspberry Pi and in CI.
- **PDFs under `VIPS_BLOCK_UNTRUSTED`.** If libvips counts poppler as untrusted, PDFs need poppler's own `pdftoppm` in the same sandbox instead.
- **libvips under `MemoryDenyWriteExecute`.** libvips speeds some work up by generating code as it runs, which the unit forbids. Check that it falls back cleanly, or turn that off with `VIPS_NOVECTOR`.
- **The install size** that `libvips-tools` adds on each system.
