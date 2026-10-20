#!/usr/bin/env bash
# Makes local builds of the server use the Accordion repos checked out in this
# workspace, instead of the versions pinned in Cargo.lock.
#
# Writes .cargo/config.toml at the workspace root with a [patch] entry for
# every Accordion crate the server depends on whose repo is checked out here.
# Cargo reads that file in every folder inside the workspace.
#
# Usage: scripts/link-workspace.sh
#
# clone-workspace.sh runs this for you. Run it yourself after adding a
# dependency on another Accordion crate.
#
# While the patches are in place, local builds rewrite Cargo.lock without the
# commits it pins. Run scripts/pin-versions.sh before committing Cargo.lock.
# This script also turns on a pre-commit hook in this repo that refuses a
# Cargo.lock without them.

set -euo pipefail

suite_repo="$(cd "$(dirname "$0")/.." && pwd)"
workspace="$(dirname "$suite_repo")"
config="$workspace/.cargo/config.toml"
org_url="https://github.com/accordion-suite"

# Lines like: accordion-core = { git = "https://github.com/accordion-suite/accordion-core", ... }
deps="$(grep -hE "^[a-z0-9-]+ = \{ git = \"$org_url/[a-z0-9-]+\"" "$suite_repo"/crates/*/Cargo.toml \
    | sed -E "s|^([a-z0-9-]+) = \{ git = \"$org_url/([a-z0-9-]+)\".*|\2 \1|" \
    | sort -u)"

output="# Written by accordion/scripts/link-workspace.sh. Run that script again
# rather than editing this file.
"
current_repo=""
while read -r repo crate; do
    [[ -n "$repo" ]] || continue
    path="$repo/crates/$crate"
    if [[ ! -f "$workspace/$path/Cargo.toml" ]]; then
        echo "skipped  $crate (no checkout at $path)"
        continue
    fi
    if [[ "$repo" != "$current_repo" ]]; then
        output+=$'\n'"[patch.\"$org_url/$repo\"]"$'\n'
        current_repo="$repo"
    fi
    output+="$crate = { path = \"$path\" }"$'\n'
    echo "linked   $crate -> $path"
done <<< "$deps"

mkdir -p "$(dirname "$config")"
printf '%s' "$output" > "$config"

git -C "$suite_repo" config core.hooksPath scripts/hooks
echo "turned on the pre-commit hook that checks Cargo.lock"
