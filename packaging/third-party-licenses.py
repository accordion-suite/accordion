"""Writes the licenses of every Rust library built into the server, and of
every library in the web app.

Most of them are under licenses, such as MIT and Apache-2.0, that ask for
their notices to come with a program built from them, so the release
packages include this file.

Usage: python3 packaging/third-party-licenses.py [--web FOLDER] TARGET... > licenses.txt

Run it from the repo, after `cargo fetch --locked`. TARGET is a Rust target,
such as aarch64-unknown-linux-musl. FOLDER is a checkout of accordion-web
after `npm ci`. Only its packages that aren't build tools go into the web
app, so only theirs are listed. The output is the same on every machine for
the same Cargo.lock and package-lock.json, so it doesn't break reproducible
packages.
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


def web_packages(folder):
    """The web app's packages that the build bundles into the app, as
    (name, version, license, source, folder). Build tools, such as esbuild,
    are dev dependencies and never reach the browser."""
    with open(os.path.join(folder, "package-lock.json"), encoding="utf-8") as file:
        lock = json.load(file)
    for path, entry in sorted(lock["packages"].items()):
        if not path or entry.get("dev"):
            continue
        installed = os.path.join(folder, path)
        with open(os.path.join(installed, "package.json"), encoding="utf-8") as file:
            manifest = json.load(file)
        source = manifest.get("repository")
        if isinstance(source, dict):
            source = source.get("url")
        yield manifest["name"], manifest["version"], manifest.get("license"), source, installed


def license_files(folder, license_file=None):
    names = sorted(n for n in os.listdir(folder) if LICENSE_FILE.match(n))
    if license_file and os.path.basename(license_file) not in names:
        names.append(license_file)
    for name in names:
        path = os.path.join(folder, name)
        if os.path.isfile(path):
            with open(path, encoding="utf-8", errors="replace") as file:
                yield os.path.basename(name), file.read().strip()


def write(out, seen, name, version, license, source, files):
    out.write("\n" + "=" * 78 + "\n")
    out.write(f"{name} {version}\n")
    out.write(f"License: {license or 'see the files below'}\n")
    if source:
        out.write(f"Source: {source}\n")
    if not files:
        out.write("\nThis library ships no license file.\n")
    for file_name, text in files:
        digest = hashlib.sha256(text.encode()).hexdigest()
        if digest in seen:
            out.write(f"\n{file_name}: the same text as {seen[digest]}.\n")
            continue
        seen[digest] = f"{file_name} for {name} {version}"
        out.write(f"\n{file_name}:\n\n{text}\n")


def main():
    args = sys.argv[1:]
    web = None
    if args[:1] == ["--web"]:
        if len(args) < 2:
            sys.exit(__doc__)
        web, args = args[1], args[2:]
    targets = args
    if not targets:
        sys.exit(__doc__)
    packages = {}
    for target in targets:
        for package in built_in(metadata(target)):
            packages[(package["name"], package["version"])] = package

    out = sys.stdout
    out.write("Licenses of the libraries built into Accordion\n\n")
    out.write("Each library's name and version is followed by its license and the license\n")
    out.write("files it ships with. A text that appeared earlier isn't repeated.\n")
    seen = {}
    out.write("\n\nRust libraries in the server\n")
    for (name, version), package in sorted(packages.items()):
        folder = os.path.dirname(package["manifest_path"])
        files = list(license_files(folder, package.get("license_file")))
        write(out, seen, name, version, package.get("license"), package.get("repository"), files)
    if web:
        out.write("\n\nLibraries in the web app\n")
        for name, version, license, source, folder in web_packages(web):
            write(out, seen, name, version, license, source, list(license_files(folder)))


if __name__ == "__main__":
    main()
