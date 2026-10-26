#!/usr/bin/env python3
"""Writes a release's update.json, which servers read to find out what's in
it (decision 12).

Usage: packaging/update-json.py <version> <commit time> <notes file>

The commit time is in seconds since 1970, and its date in UTC becomes the
release date, so the same commit always gives the same file. The notes file,
packaging/release-notes/<version>.md, starts with two settings, then a blank
line, then the notes:

    Security: no
    Installs itself: yes

    Drive shows photos in a grid, and backups to a USB disk are faster.

"Security: yes" means the release fixes a security problem, so servers
install it without the usual three-day wait. "Installs itself: no" means it
needs something new from the distribution, such as another package, so it
has to be installed with the install command instead.
"""

import datetime
import json
import sys

# The server refuses longer notes.
NOTES_LIMIT = 4000
REPO = "https://github.com/accordion-suite/accordion"


def fail(message):
    sys.exit(message)


def setting(lines, name, path):
    prefix = f"{name}: "
    for line in lines:
        if line.startswith(prefix):
            value = line[len(prefix):].strip()
            if value in ("yes", "no"):
                return value == "yes"
            fail(f'{path} says "{line}". Set {name} to yes or no.')
    fail(f'{path} doesn\'t say "{name}: yes" or "{name}: no". Add it before the notes.')


def main():
    if len(sys.argv) != 4:
        fail(__doc__.strip())
    version, commit_time, path = sys.argv[1:]
    try:
        with open(path, encoding="utf-8") as notes_file:
            text = notes_file.read()
    except FileNotFoundError:
        fail(
            f"There are no release notes at {path}. Write them first, as packaging/update-json.py "
            "describes."
        )
    head, blank, notes = text.partition("\n\n")
    if not blank:
        fail(f"{path} needs a blank line between its settings and the notes.")
    lines = head.splitlines()
    notes = " ".join(notes.split())
    if not notes:
        fail(f"{path} has no notes after its settings.")
    if len(notes) > NOTES_LIMIT:
        fail(f"The notes in {path} are longer than {NOTES_LIMIT} characters. Shorten them.")
    published = datetime.datetime.fromtimestamp(int(commit_time), datetime.timezone.utc)
    manifest = {
        "version": version,
        "published": published.strftime("%Y-%m-%d"),
        "security": setting(lines, "Security", path),
        "installs_itself": setting(lines, "Installs itself", path),
        "notes": notes,
        "link": f"{REPO}/releases/tag/v{version}",
    }
    json.dump(manifest, sys.stdout, indent=2, sort_keys=True, ensure_ascii=False)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
