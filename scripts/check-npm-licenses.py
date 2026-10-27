#!/usr/bin/env python3
"""Checks the npm packages in a repo's package-lock.json, as deny.toml checks
Rust crates (decision 13).

Usage: scripts/check-npm-licenses.py <repo folder>

Run it after npm ci. It fails on a package whose license can't be combined
with AGPL-3.0, or that doesn't say its license, and on a package from anywhere
but npm's registry or without a checksum.

Licenses come from each installed package's package.json, because not every
version of npm records them in the lockfile. Packages for other systems, such
as esbuild's build for macOS, don't install on Linux, so only their source
and checksum are checked. deny.toml skips other systems the same way.
"""

import json
import os
import re
import sys

# The same list as deny.toml. Check compatibility before you add one.
ALLOWED = {
    "AGPL-3.0-only",
    "AGPL-3.0-or-later",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "MIT",
    "MPL-2.0",
    "Unicode-3.0",
    "Zlib",
}
REGISTRY = "https://registry.npmjs.org/"


def allowed(expression):
    """Whether an SPDX expression, such as "(MIT OR Apache-2.0)", allows
    use under one of the allowed licenses. Every license joined by AND has
    to be allowed. Any one choice joined by OR is enough."""
    expression = expression.strip()
    if expression.startswith("(") and expression.endswith(")"):
        expression = expression[1:-1]
    if "(" in expression:
        # Nested expressions are rare. Name the package so someone checks it.
        return False
    return any(
        all(term.strip() in ALLOWED for term in re.split(r"\s+AND\s+", choice))
        for choice in re.split(r"\s+OR\s+", expression)
    )


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    repo = sys.argv[1]
    with open(os.path.join(repo, "package-lock.json")) as file:
        lock = json.load(file)
    if lock.get("lockfileVersion", 0) < 2:
        sys.exit("package-lock.json is an old lockfile. Run npm install with npm 7 or later to update it.")
    problems = []
    count = 0
    other_systems = 0
    for path, package in lock.get("packages", {}).items():
        # The repo itself, and folders linked into it.
        if not path or package.get("link"):
            continue
        count += 1
        name = path.rsplit("node_modules/", 1)[-1]
        license_ = package.get("license")
        manifest = os.path.join(repo, path, "package.json")
        installed = os.path.exists(manifest)
        if license_ is None and installed:
            with open(manifest) as file:
                license_ = json.load(file).get("license")
        if license_ is None and package.get("optional") and not installed:
            other_systems += 1
        elif not isinstance(license_, str):
            problems.append(f"{name} doesn't say its license. Check it by hand.")
        elif not allowed(license_):
            problems.append(f"{name} is under {license_}, which isn't on the allowed list.")
        resolved = package.get("resolved", "")
        if not resolved.startswith(REGISTRY):
            problems.append(f"{name} comes from {resolved or 'an unknown place'}, not npm's registry.")
        if not package.get("integrity"):
            problems.append(f"{name} has no checksum in the lockfile.")
    if problems:
        print("\n".join(problems))
        sys.exit(f"{len(problems)} problems with npm packages. Packages need an allowed license, npm's registry, and a checksum.")
    print(
        f"All {count} npm packages come from npm's registry and have checksums. The "
        f"{count - other_systems} that install here have allowed licenses, and {other_systems} "
        "for other systems were skipped."
    )


if __name__ == "__main__":
    main()
