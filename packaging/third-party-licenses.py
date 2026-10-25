"""Writes the licenses of every Rust library built into the server.

Most of them are under licenses, such as MIT and Apache-2.0, that ask for
their notices to come with a program built from them, so the release
packages include this file.

Usage: python3 packaging/third-party-licenses.py TARGET... > licenses.txt

Run it from the repo, after `cargo fetch --locked`. TARGET is a Rust target,
such as aarch64-unknown-linux-musl. The output is the same on every machine
for the same Cargo.lock, so it doesn't break reproducible packages.
"""

import hashlib
import json
import os
import re
import subprocess
import sys

ROOT = "accordion-server"
LICENSE_FILE = re.compile(r"^(licen[cs]e|copying|copyright|notice|unlicense)([-_.].*)?$", re.I)


def metadata(target):
    output = subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", target],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return json.loads(output)


def built_in(meta):
    """The packages compiled into the server: its normal dependencies, and
    theirs, leaving out build scripts' and macros' own dependencies, which
    run while building but aren't part of the program."""
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = next(p["id"] for p in meta["packages"] if p["name"] == ROOT)
    found, todo = set(), [root]
    while todo:
        node = nodes[todo.pop()]
        for dep in node["deps"]:
            if not any(kind["kind"] is None for kind in dep["dep_kinds"]):
                continue
            package = packages[dep["pkg"]]
            if any("proc-macro" in t["kind"] for t in package["targets"]):
                continue
            if dep["pkg"] not in found:
                found.add(dep["pkg"])
                todo.append(dep["pkg"])
    # Accordion's own crates are under Accordion's license.
    return [packages[i] for i in found if not packages[i]["name"].startswith("accordion")]


def license_files(package):
    folder = os.path.dirname(package["manifest_path"])
    names = sorted(n for n in os.listdir(folder) if LICENSE_FILE.match(n))
    if package.get("license_file") and os.path.basename(package["license_file"]) not in names:
        names.append(package["license_file"])
    for name in names:
        path = os.path.join(folder, name)
        if os.path.isfile(path):
            with open(path, encoding="utf-8", errors="replace") as file:
                yield os.path.basename(name), file.read().strip()


def main():
    targets = sys.argv[1:]
    if not targets:
        sys.exit(__doc__)
    packages = {}
    for target in targets:
        for package in built_in(metadata(target)):
            packages[(package["name"], package["version"])] = package

    out = sys.stdout
    out.write("Licenses of the Rust libraries built into Accordion\n\n")
    out.write("Each library's name and version is followed by its license and the license\n")
    out.write("files it ships with. A text that appeared earlier isn't repeated.\n")
    seen = {}
    for (name, version), package in sorted(packages.items()):
        out.write("\n" + "=" * 78 + "\n")
        out.write(f"{name} {version}\n")
        out.write(f"License: {package.get('license') or 'see the files below'}\n")
        if package.get("repository"):
            out.write(f"Source: {package['repository']}\n")
        files = list(license_files(package))
        if not files:
            out.write("\nThis library ships no license file.\n")
        for file_name, text in files:
            digest = hashlib.sha256(text.encode()).hexdigest()
            if digest in seen:
                out.write(f"\n{file_name}: the same text as {seen[digest]}.\n")
                continue
            seen[digest] = f"{file_name} for {name} {version}"
            out.write(f"\n{file_name}:\n\n{text}\n")


if __name__ == "__main__":
    main()
