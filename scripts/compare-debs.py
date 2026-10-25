"""Checks that two Debian packages hold the same files.

Usage: python3 -I scripts/compare-debs.py FIRST.deb SECOND.deb

Different versions of dpkg can pack the same files into slightly different
bytes, so the check before signing a release (decision 9) compares what's
inside each package: the control files and scripts, and every installed
file's path, owner, permissions, time, and contents. It prints any
difference and exits with 1 if there is one.
"""

import hashlib
import io
import sys
import tarfile


def ar_members(path):
    """The members of a .deb, which is an ar archive, in order."""
    with open(path, "rb") as file:
        data = file.read()
    if not data.startswith(b"!<arch>\n"):
        sys.exit(f"{path} isn't a Debian package.")
    members, offset = [], 8
    while offset < len(data):
        header = data[offset : offset + 60]
        name = header[:16].decode().strip().rstrip("/")
        size = int(header[48:58].decode().strip())
        offset += 60
        members.append((name, data[offset : offset + size]))
        # Members start on even offsets.
        offset += size + size % 2
    return members


def tar_entries(blob):
    """Each file in a control or data archive, as the fields that matter."""
    entries = {}
    with tarfile.open(fileobj=io.BytesIO(blob), mode="r:*") as archive:
        for member in archive:
            content = archive.extractfile(member).read() if member.isfile() else b""
            entries[member.name.removeprefix("./").rstrip("/") or "."] = {
                "type": member.type,
                "mode": oct(member.mode),
                "owner": f"{member.uid}:{member.gid} ({member.uname}:{member.gname})",
                "time": member.mtime,
                "size": member.size,
                "link": member.linkname,
                "sha256": hashlib.sha256(content).hexdigest(),
            }
    return entries


def contents(path):
    found = {}
    for name, blob in ar_members(path):
        # The compression can differ between dpkg versions, so compare the
        # archive by what it holds, not its name's extension.
        kind = name.split(".")[0]
        if kind in ("control", "data"):
            found[kind] = tar_entries(blob)
        else:
            found[name] = blob
    return found


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    first, second = sys.argv[1:]
    a, b = contents(first), contents(second)
    differences = []
    for part in sorted(a.keys() | b.keys()):
        if part not in a or part not in b:
            differences.append(f"{part}: only in {first if part in a else second}")
        elif part not in ("control", "data"):
            if a[part] != b[part]:
                differences.append(f"{part}: different")
        else:
            for name in sorted(a[part].keys() | b[part].keys()):
                one, other = a[part].get(name), b[part].get(name)
                if one is None or other is None:
                    differences.append(f"{part} {name}: only in {first if one else second}")
                    continue
                for field in one:
                    if one[field] != other[field]:
                        differences.append(f"{part} {name}: {field} {one[field]} and {other[field]}")
    for difference in differences:
        print(difference)
    sys.exit(1 if differences else 0)


if __name__ == "__main__":
    main()
