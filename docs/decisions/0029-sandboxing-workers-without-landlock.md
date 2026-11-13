# 29. Sandboxing workers without Landlock

- **Status:** Accepted
- **Date:** November 13, 2026

## Context

Workers decode files people upload: thumbnails, previews, and the text that search reads from PDFs and Office files. [Decision 14](0014-thumbnails-and-sandboxed-workers.md) runs each one through a launcher, `accordion sandbox`, which shuts itself in before it starts the worker program:

- **Landlock** keeps it out of the data folder and lets it write nowhere except a scratch folder.
- **A seccomp filter** refuses sockets, tracing, namespaces, and signals to other processes.
- **Resource limits** cap its memory and CPU time.

The worker runs as the server's own user, so Landlock is the only thing between a compromised decoder and the database, the private keys, and everyone's files. Decision 14 made Landlock required: without it, the server runs no workers, and **Server health** says why. It asked to revisit that "if Landlock turns out to be missing or turned off on a supported system, such as a Raspberry Pi OS kernel."

It is. The Raspberry Pi kernel, `6.18.50+rpt-rpi-2712`, is built with `CONFIG_SECURITY_LANDLOCK` not set, and its security modules are only `capability,apparmor`. Raspberry Pi OS and umbrelOS on a Pi both use it. So a Raspberry Pi, the smallest supported server and the developer's own, makes no thumbnails or previews, and search can't read inside files. The Umbrel app ([decision 27](0027-running-on-umbrelos.md)) is blocked on the same thing. Debian's and Ubuntu's own kernels have Landlock.

## Decision drivers

1. **A compromised worker reaches nothing,** as decision 14 requires. It can't read the database or other people's files, write anywhere, or connect to the network.
2. **Works on a Raspberry Pi,** with Raspberry Pi OS and with umbrelOS, and on every other supported install.
3. **The same in the .deb and the Umbrel container,** so there's one sandbox to understand and test.
4. **Nothing for the owner to install or set up** beyond Accordion.
5. **Tests run the sandbox that ships,** as much as they can.
6. **Protects the rest of the machine,** such as a household's Umbrel apps, when a worker uses a lot of memory.
7. **One developer.**

## Options

### Option A: No change

Landlock stays required. Households on a Raspberry Pi get no thumbnails, previews, or search inside files, or use other hardware, as decision 27's fallback says.

Pros:

- **No work.**

Cons:

- **Breaks driver 2** on the smallest supported server, which many households would start with.
- **The Umbrel app stays blocked.**

### Option B: Run workers without Landlock

Keep the seccomp filter and the limits, and drop Landlock when the kernel doesn't have it, with a warning in **Server health**.

Pros:

- **Almost no work.**

Cons:

- **Breaks driver 1.** A compromised decoder can read the whole data folder. seccomp stops it from connecting anywhere, but it can still copy what it reads into the thumbnail or text it returns, which the person who uploaded the crafted file may be able to see, such as through a shared folder.

### Option C: Namespaces in the launcher

When there's no Landlock, the launcher does what bubblewrap does: it creates its own user, mount, and network namespaces, and builds a file system that holds only `/usr` and the other folders the worker reads.

Pros:

- **Strong:** the worker can't see the data folder at all, and has no network.
- **Works for any user** where the kernel allows unprivileged user namespaces.

Cons:

- **Doesn't work in the Umbrel container.** Docker's default seccomp profile refuses `mount` and namespaces other than user namespaces to a container without `CAP_SYS_ADMIN`. So it breaks drivers 2 and 3.
- **The server's unit has to allow namespaces,** which decision 14 turned off with `RestrictNamespaces=yes`. User namespaces are a large part of the kernel to give to a server that faces the internet.
- **The most intricate code of these options:** mount setup, `pivot_root`, and user ID maps.
- **Not checked yet** whether the Pi kernel allows unprivileged user namespaces.

### Option D: An AppArmor profile for workers

The Pi kernel has AppArmor. The .deb installs a profile that lets workers read only `/usr` and a few other folders, and the launcher asks the kernel to apply it when it starts the worker program.

Pros:

- **The kernel enforces it,** on the Pi as it is.

Cons:

- **Doesn't work in the Umbrel container.** Loading a profile needs root on the host, which an Umbrel app doesn't have. So it breaks drivers 2 and 3.
- **A profile for each worker program,** kept in step with each distribution's file paths.
- **Not checked yet** whether Raspberry Pi OS Lite installs and turns on AppArmor's tools.

### Option E: A worker service, with its own user for each job

Workers move out of the server into a small second process, `accordion workers`, that can't see the data folder and has no network. The server sends it each job over a local socket, with the file already open and a pipe for the result. The worker service runs each job as a user ID of its own, then runs the launcher from decision 14 inside that, with seccomp, the limits, and Landlock when the kernel has it.

- **On the .deb,** the worker service is a second systemd service, `accordion-workers.service`. It has no network (`PrivateNetwork=yes`), the data folder is hidden from it (`InaccessiblePaths=`), and only the `accordion` user can connect to its socket.
- **On umbrelOS,** it's a second container from the same image, with no network (`network_mode: none`), no data folder mounted, and a read-only file system. The socket is in a small folder both containers share.

Pros:

- **Works on any kernel,** inside both installs, so it meets drivers 2 and 3.
- **Stronger than Landlock alone.** A compromised decoder runs as a user that owns nothing, in a process that has no network and can't even see the data folder. Decision 14 named a different user as the main advantage of its systemd option. Where the kernel has Landlock, it's a second layer.
- **Jobs can't see each other.** Thumbnails, text, and previews run at the same time, and each gets its own user ID. Under one shared user, a compromised job could open another job's file through `/proc`.
- **An exact memory limit for all workers together,** from systemd or Docker, so a big AVIF or a video can't push out the household's other Umbrel apps.
- **The server gets tighter.** It no longer runs the launcher, so its unit drops the `@sandbox` system calls decision 14 added.

Cons:

- **A second process to package:** a systemd service and socket in the .deb, and a second container in the Umbrel app, which the upgrade test has to cover.
- **The worker service starts with two privileges,** `CAP_SETUID` and `CAP_SETGID`, to switch to each job's user. It uses nothing else, and runs only the worker programs the server is allowed to ask for. Docker gives containers both by default, so the Umbrel container ends up with fewer privileges than a default container.
- **Tests can't switch users** without root. `cargo test` and the dev server run the worker service as the developer's own user, without the switch. One CI test runs it with `sudo` and checks that a job can't read the server's files.
- **A small protocol** between the server and the worker service, for the job, its limits, and the open file.

## Comparison

| Driver | A: No change | B: Without Landlock | C: Namespaces | D: AppArmor | E: Worker service |
|---|---|---|---|---|---|
| A compromised worker reaches nothing | Yes, where it runs | No | Yes | Yes | Yes |
| Works on a Pi, both systems | No | Yes | Raspberry Pi OS, if allowed | Raspberry Pi OS, if installed | Yes |
| Same in the .deb and the container | Yes | Yes | No | No | Yes |
| Nothing for the owner to set up | Yes | Yes | Yes | Yes | Yes |
| Tests run what ships | Yes | Yes | Yes | No | Partly, plus one CI test |
| Protects other apps' memory | Roughly | Roughly | Roughly | Roughly | Exactly |
| Work for one developer | None | Small | Large | Medium | Medium |

## Decision

**A worker service, with its own user for each job (option E).** It's the only option that works in both installs on a Pi, and it's stronger everywhere, not only where Landlock is missing.

### How it works

- **The worker service listens on a local socket.** On the .deb, it's `/run/accordion/workers.sock`, owned by the `accordion` group. In the Umbrel app, it's in a shared folder only the two containers mount.
- **Each job brings its input and output already open,** passed over the socket: the file on standard input, and pipes for the result and messages.
- **Only known programs run,** such as `vips`, `pdftoppm`, `ffmpeg`, and LibreOffice, each with the limits and folders the server sets for it today.
- **Each job runs as its own user ID,** from a range Accordion reserves, which isn't used again while a job runs. It needs no entry in `/etc/passwd`. Before the ID is used again, every process with it is stopped.
- **Inside, the launcher works as it does today:** seccomp, the limits, writable memory that can't be run, and Landlock when the kernel has it.
### How files reach a job

Many workers read a file by name, not from standard input: ffmpeg seeks through a video, and LibreOffice and LibRaw open the file they're given. A job's user can't reopen a file the server opened, because Linux checks permissions again on every open, even through `/proc/self/fd`. So:

- **Each job works in a folder of its own,** in `jobs/` in the data folder. That's the one part of the data folder the worker service can see, at the same path as the server, so paths in a job's arguments work on both sides.
- **The server makes the folder and lets only that job's user in,** with a POSIX ACL: an extra permission entry for one user, which a file's owner can add without privileges. A default entry gives the server the same access to what the job writes.
- **A file the job reads by name is a hard link** to the stored file, in the job's folder, which the job's user can read only while it runs. Nothing is copied, so a large video costs no time or space. The data folder itself stays closed to the job, so the link is its only way to the file.
- **What a job writes stays in its folder** until the server takes it, after every process the job started has stopped, so nothing can change a result after the server checks it. Then the worker service deletes what's left, as the job's user.

This needs a file system with POSIX ACLs for the data folder. ext4, XFS, and Btrfs have them, and so do Raspberry Pi OS and umbrelOS. **Server health** says when the data folder's file system doesn't.

### Limits and health

- **Memory for all workers together** is capped by systemd's `MemoryMax=` on the .deb and Docker's `mem_limit` in the Umbrel app. The cap has to fit a 48-megapixel AVIF's 2 GB.
- **Server health** says which layers are on: own users, no network, and whether the kernel has Landlock. When the worker service isn't running, thumbnails and search wait, and the check says why.

### Also

Ask Raspberry Pi to turn on Landlock in their kernel, through an issue in `raspberrypi/linux`. It helps other software that sandboxes itself, and adds the second layer on a Pi. Accordion doesn't wait for it.

### Revisit this decision if

- Umbrel stops allowing a second container without a network, or with `CAP_SETUID` and `CAP_SETGID`.
- A security review finds the worker service's two privileges a bigger risk than Landlock alone.

## Consequences

- **Decision 14 changes:** Landlock is no longer required, and the launcher runs inside the worker service, not the server. Its record gets an Updated line pointing here.
- **Decision 27's compose file** gains the worker container, and the Pi's sandbox test becomes a test of this.
- **The .deb** gains `accordion-workers.service` and its socket, and the server's unit drops `@sandbox`, `setpriority`, and `ioprio_set`.
- **Core's `accordion-sandbox` crate** gains the worker service and the server's side of its socket. Thumbnails, previews, and search's text use it instead of running the launcher.
- **Server health's Thumbnails check** describes the new layers.
- **The upgrade test** checks that thumbnails still appear after the update that adds the worker service.

## Open questions

- **Passing an open file between two containers** through a socket in a shared folder. It works between processes on one kernel, but hasn't been tried in Umbrel's setup.
- **The memory cap on a Pi 8 GB** that's shared with Umbrel apps, and on a Pi 4 GB.
- **Which user ID range** to reserve, so it can't clash with users the system or Docker already has.
- **Data folders on file systems without POSIX ACLs,** such as some NAS shares. They could copy files into the job's folder instead.
