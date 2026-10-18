#!/usr/bin/env bash
# Pins Cargo.lock to the latest pushed commit of each Accordion repo the
# server depends on. Run it before committing Cargo.lock.
#
# Usage: scripts/pin-versions.sh
#
# Cargo.lock is the tested set of module versions (decision 1). Local builds
# use the checkouts in this workspace instead (see link-workspace.sh), and
# while they do, Cargo rewrites Cargo.lock without the commits it pins. CI
# builds with --locked, so a Cargo.lock committed in that state fails CI.
#
# Push the other repos first: this pins what's on GitHub, not what's local.

set -euo pipefail

suite_repo="$(cd "$(dirname "$0")/.." && pwd)"
workspace="$(dirname "$suite_repo")"
manifest="$suite_repo/Cargo.toml"
org_url="https://github.com/accordion-suite"

mapfile -t crates < <(grep -hoE "^[a-z0-9-]+ = \{ git = \"$org_url/" "$suite_repo"/crates/*/Cargo.toml \
    | sed -E 's/ = .*//' | sort -u)
mapfile -t repos < <(grep -hoE "git = \"$org_url/[a-z0-9-]+\"" "$suite_repo"/crates/*/Cargo.toml \
    | sed -E "s|.*$org_url/([a-z0-9-]+)\"|\1|" | sort -u)

# The pinned commits are what's on GitHub, so local work isn't in them.
for repo in "${repos[@]}"; do
    dir="$workspace/$repo"
    [[ -d "$dir/.git" ]] || continue
    if [[ -n "$(git -C "$dir" status --porcelain)" ]]; then
        echo "warning: $repo has uncommitted changes, which won't be pinned." >&2
    fi
    if git -C "$dir" rev-parse -q --verify '@{upstream}' > /dev/null 2>&1 \
        && [[ -n "$(git -C "$dir" rev-list '@{upstream}..HEAD')" ]]; then
        echo "warning: $repo has commits that aren't pushed, which won't be pinned." >&2
    fi
done

# Cargo reads .cargo/config.toml from the current folder upward, so running
# from outside the workspace leaves the local patches out. The first command
# restores the git sources that a patched build removed, and the second moves
# each one to the latest commit.
outside="$(mktemp -d)"
trap 'rm -rf "$outside"' EXIT
cd "$outside"
cargo metadata --quiet --format-version 1 --manifest-path "$manifest" > /dev/null
update_args=()
for crate in "${crates[@]}"; do
    update_args+=(-p "$crate")
done
cargo update --manifest-path "$manifest" "${update_args[@]}"

grep -A2 -E "^name = \"($(IFS='|'; echo "${crates[*]}"))\"" "$suite_repo/Cargo.lock" \
    | grep -oE "$org_url/[a-z0-9-]+#[0-9a-f]{7}" \
    | sort -u \
    | sed -E "s|$org_url/(.*)#|pinned   \1 at |"
