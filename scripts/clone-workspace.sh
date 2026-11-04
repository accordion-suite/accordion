#!/usr/bin/env bash
# Clones every Accordion repo into the workspace (the folder containing this
# repo), or updates the ones already there. Repos that don't exist on GitHub
# yet are skipped.
#
# Usage: scripts/clone-workspace.sh
#
# Run it from bash. On Windows, use WSL.

set -uo pipefail

repos=(
    accordion
    accordion-core
    accordion-gateway
    accordion-drive
    accordion-docs
    accordion-mail
    accordion-notes
    accordion-calendar
    accordion-contacts
    accordion-photos
    accordion-sheets
    accordion-web
    accordion-desktop
    accordion-android
    accordion-ios
)

workspace="$(cd "$(dirname "$0")/../.." && pwd)"
failed=0

for repo in "${repos[@]}"; do
    dir="$workspace/$repo"
    url="https://github.com/accordion-suite/$repo.git"

    if [[ -d "$dir/.git" ]]; then
        # A repo made with new-repo.sh has no upstream until its first push.
        if ! git -C "$dir" rev-parse -q --verify '@{upstream}' > /dev/null 2>&1; then
            echo "skipped  $repo (not pushed yet)"
        elif git -C "$dir" pull -q --ff-only; then
            echo "updated  $repo"
        else
            echo "FAILED   $repo (pull failed; check for local changes)"
            failed=1
        fi
    elif git ls-remote -q "$url" > /dev/null 2>&1; then
        if git clone -q "$url" "$dir"; then
            echo "cloned   $repo"
        else
            echo "FAILED   $repo (clone failed)"
            failed=1
        fi
    else
        echo "skipped  $repo (not on GitHub yet)"
    fi
done

# Make local server builds use these checkouts.
"$workspace/accordion/scripts/link-workspace.sh" || failed=1

# Cargo never deletes old build files, and a new Rust version or a dependency
# update leaves a full set of them behind. Start the shared build folder over
# when Rust changes or the folder passes 40 GB.
cache="$workspace/build-cache"
# The accordion repo pins its own Rust version, which can differ from the one
# the other repos use.
rustc_now="$(rustc --version; cd "$workspace/accordion" && rustc --version)"
if [[ -d "$cache" ]]; then
    size_gb="$(( $(du -s --block-size=1G "$cache" | cut -f1) ))"
    if [[ "$(cat "$cache/.rustc-version" 2>/dev/null)" != "$rustc_now" || $size_gb -gt 40 ]]; then
        rm -rf "$cache"
        echo "emptied  build-cache (${size_gb} GB; the next build will take longer)"
    fi
fi
mkdir -p "$cache"
echo "$rustc_now" > "$cache/.rustc-version"

exit $failed
