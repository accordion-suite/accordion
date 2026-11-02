# 16. Sharing files and folders

- **Status:** Accepted
- **Date:** November 2, 2026

## Context

Each person sees only their own files. Sharing is the largest gap between Drive and Google Drive, and the spec lists shared folders as a must-have for family members. The spec asks for:

- Sharing with people on the server, or by link with a password, an expiry date, and a download limit.
- Shared drives owned by a group, for small teams.
- The same sharing for Docs, since every document is a file in Drive, and sharing a note in Notes.
- A guest role for people who only open links, and an audit log that records shares.
- Malicious shared links are in the threat model.

Today, core's storage gives every item one owner, and Drive checks that the signed-in person owns an item before every action. Anyone else gets "not found", so people can't learn what exists. Search, Recent, Starred, thumbnails, ZIP downloads, and WebDAV all assume one owner.

The server is reachable only on the home network and over the VPN ([decision 5](0005-server-name-and-certificates.md)), so a link only works for someone who can reach it. The spec's optional public sharing endpoint isn't designed yet. [Decision 6](0006-file-storage.md) gives core the folder tree and Drive the share links.

## Decision drivers

1. **Obvious to family members.** Sharing a folder with a parent should work the way it does in Google Drive.
2. **Nothing leaks.** One missed check mustn't show someone a file nobody gave them.
3. **One model for every module.** Docs, Notes, Photos albums, and search follow the same rules.
4. **Predictable quotas,** as decision 6 promises.
5. **Beyond the web app.** Shared files reach File Explorer, WebDAV apps, and later the phone and desktop apps.
6. **One developer.** Small enough to build and test thoroughly.

## Options: what can be shared

### Option A: Any file or folder, with chosen people

As in Google Drive. A share on a folder covers everything inside it, including what's added later. Each person finds what others shared with them in **Shared with me**.

Pros:

- **Familiar** to anyone who has used Google Drive.
- **Files and folders alike,** such as one photo for a grandparent or a whole school folder for a child.
- **Docs and Notes fit as they are,** since their documents are files.
- **Links fit the same model:** a link is a share with whoever holds it.

Cons:

- **Access depends on where an item is.** Moving a file into or out of a shared folder changes who can see it, and every check walks up the folder tree.
- **Shared items live outside My Drive,** so apps that only show folders, such as File Explorer, need somewhere to put them.

### Option B: Shared drives only

Group-owned spaces, such as a "Family" drive that everyone in the household can edit.

Pros:

- **The simplest permissions:** membership of a drive.
- **Natural for small teams,** who the spec says need them.

Cons:

- **No sharing a single file or folder,** which is what families do most.
- **Needs groups first,** which identity doesn't have.

### Option C: Shares that appear inside the recipient's Drive

As in Nextcloud. A shared folder shows up as a folder in the recipient's own Drive, which they can rename and move.

Pros:

- **Shows up everywhere at once,** in WebDAV and sync clients, with no separate place.

Cons:

- **Two names for one folder,** the owner's and the recipient's, which confuses both.
- **Unclear endings.** What the recipient sees when the owner deletes, moves, or unshares the folder needs explaining.
- **The most code** in core's folder tree.

## Options: who owns what an editor adds

### Option 1: The folder's owner

Everything in a folder tree belongs to the tree's owner, whoever added it.

Pros:

- **Trash, quotas, versions, and the plain view stay as they are,** with one owner for each tree.
- **Removing an editor is clean.** Their uploads stay with the folder's owner.

Cons:

- **An editor uses the owner's storage.** A child with edit access can fill a parent's quota.

### Option 2: Whoever added it

As in Google Drive.

Pros:

- **Each person's quota covers what they upload.**

Cons:

- **Folders with several owners.** Whose trash a deleted file goes to, and whose plain view shows it, each need rules.
- **Orphaned files.** When the folder's owner removes an editor, the editor's files stay in a folder they can no longer open. Google Drive needs special handling for this.

## Comparison

| Driver | A: Any item | B: Shared drives | C: In the recipient's Drive |
|---|---|---|---|
| Obvious to families | Yes | Partly | Partly |
| Single files | Yes | No | Yes |
| One model for every module | Yes | Yes | Yes, with a second name for each share |
| Shows up in File Explorer | With a second address | Yes | Yes |
| Work for one developer | Medium | Medium, after groups | Large |

| Driver | 1: The folder's owner | 2: Whoever added it |
|---|---|---|
| Predictable quotas | The owner pays | The uploader pays |
| Trash and plain view | Unchanged | New rules |
| Removing an editor | Clean | Orphaned files |

## Recommendation

**Share any file or folder with chosen people (option A). Everything in a folder tree belongs to its owner (option 1). Core enforces access for every module.**

Shared drives come later, as their own decision, once identity has groups. The rules below let a group receive a share the same way a person does.

### Access

- **Two levels:**
  - **Viewer:** can open, download, and copy to their own Drive.
  - **Editor:** can also add, rename, replace, and move items inside the shared folder, restore versions, and move items to the trash.

  A commenter level comes with comments in Docs.
- **A share on a folder covers everything inside it,** including what's added later. Someone's access to an item is the highest level any share gives them, on the item or a folder above it.
- **Only the owner shares and changes access,** at first. Google Drive lets editors share too, but families rarely need it, and one person in charge is easier to explain. The owner stays the owner; giving a file to someone else comes later.
- **Removing someone works on their next request,** in the web app and over WebDAV.
- **Editors move items only inside the owner's tree.** Moving something out of a shared folder, which only the owner can do, ends access through that folder. To keep their own copy, people use **Make a copy**, which counts against their own quota.
- **What an editor moves to the trash goes to the owner's trash.** Only the owner can empty it or delete forever.
- **Anything not shared with someone still answers "not found",** so nobody learns what exists.

### Where it's enforced

- **Core's storage owns the shares and the check.** It answers what a person may do with an item, and lists what's shared with them. Drive replaces its ownership check with a call to core, and Docs, Notes, and Photos use the same call ([decision 1](0001-where-core-services-run.md)).
- **One table in core** records each share: the item, the person, the level, who shared it, and when. The check is one database query that walks up the folder tree, which is a few levels deep in family use.
- **Every way in uses the same check:** search, Recent, Starred, thumbnails, versions, previews, ZIP downloads, and WebDAV. Step 4's search inside files filters its results by it too.

### What people see

- **Share,** in each item's menu, opens a dialog like Google Drive's:
  - Choose people on the server by name, and **Viewer** or **Editor**.
  - See who has access, and change or remove each person.
- **Shared with me,** in the sidebar, lists what others shared with you, newest first, with who shared it. Opening a shared folder shows what's inside it, with a path that starts at the shared folder.
- **Lists mark what's shared,** with an icon on items that others can see.
- **Viewers see only what they can do.** Rename, Move, and Move to trash don't appear for them.
- **Finding people to share with.** People's names are visible only to admins today. A new address lists the names and usernames of active accounts to anyone signed in. The **People** page in Settings stays for admins only.
- **No notification at first.** There's no Mail module to send one, so new shares appear at the top of **Shared with me**.

### WebDAV and apps

- **A second WebDAV address, `/dav/shared/`,** holds what's shared with you, with the same access. Viewers can only read. File Explorer maps it as a second drive.
- **Shortcuts in My Drive come later.** Google Drive's **Add shortcut to Drive** puts a shared folder inside My Drive, so sync clients see it with everything else. It belongs in a decision with the desktop client.

### Quotas

- **Everything in a tree counts against its owner's quota,** including what editors add.
- **An upload that would go over the owner's quota fails,** with a message saying the folder's owner is out of space, not the person uploading.

### Links, designed now and built later

- **Drive owns links,** as decision 6 says. A link gives view-only access to a file or folder. It can have a password, an expiry date, and a download limit, as the spec asks.
- **A link's address carries a random 128-bit token,** stored only as a hash, as passwords are. Password guesses are rate-limited, as sign-ins are. The admin can turn links off for the whole server.
- **People who open a link get no account.** The guest role stays for later.
- **Links reach only people who can reach the server:** the home network and the VPN. Reaching anyone else needs the public sharing endpoint, the first thing Accordion would put on the internet. It needs its own decision, and links are built with it.

### The audit log

- **Core records each share, change, and removal,** with who did it and when. The spec's audit log can then show them. The log itself is separate work.

### Testing

- **One test covers every way in.** It tries every Drive address and WebDAV method as the owner, a viewer, an editor, and someone with no access, and expects the right answer for each. It fails when a new address isn't in its list, so a new feature can't skip the check.
- **Core's tests cover the rules:**
  - access inherited from folders
  - moving out of a shared folder
  - removing someone
  - an editor's trash going to the owner
  - quotas
- **The web app's tests** cover the **Share** dialog and **Shared with me**.

### Revisit this decision if

- Families ask for editors to share with others.
- Shared drives arrive, and groups need to receive shares.
- Editors filling owners' quotas becomes a common complaint.

## Consequences

- **Core** gains the shares table and its migration, the access check, and lists of what's shared with each person. Search, Recent, and Starred include shared items.
- **The server** gains an address that lists people to share with.
- **Drive** switches every address from its ownership check to core's access check. Item lists say who owns each item and what the reader may do. WebDAV gains `/dav/shared/`.
- **The web app** gains the **Share** dialog, **Shared with me**, the shared icon, and menus that match each person's access.
- **Unchanged:** thumbnails, which sit behind Drive's check, and the plain view on disk, which shows each tree under its owner.

## Open questions

- **Notifications.** Whether a new share should show a count on **Shared with me** until Mail can send a message.
- **Who sees whom.** Everyone on a family server can see everyone's name. A small team may want groups to limit that.
- **Shortcuts** for WebDAV and sync clients, with the desktop client.
- **The public sharing endpoint,** which links wait for.
