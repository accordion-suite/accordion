#!/usr/bin/env bash
# Creates a new Accordion repo in the workspace, set up from templates/repo.
#
# Usage: scripts/new-repo.sh [--typescript] accordion-<name>
#
# Repos are Rust by default. --typescript sets up a TypeScript repo instead,
# such as the web app (decision 13), with templates/repo-typescript in place
# of the Rust settings.
#
# The repo is created next to this one (in accordion-suite/), committed, and
# connected to github.com/accordion-suite/<name>. Create the empty repo on
# GitHub first, then push.

set -euo pipefail

typescript=no
if [[ "${1:-}" == --typescript ]]; then
    typescript=yes
    shift
fi
name="${1:-}"
if [[ ! "$name" =~ ^accordion-[a-z0-9-]+$ ]]; then
    echo "Usage: $0 [--typescript] accordion-<name>   (for example, accordion-drive)" >&2
    exit 1
fi

suite_repo="$(cd "$(dirname "$0")/.." && pwd)"
workspace="$(dirname "$suite_repo")"
target="$workspace/$name"

if [[ -e "$target" ]]; then
    echo "$target already exists." >&2
    exit 1
fi

# accordion-drive -> Accordion Drive
suffix="${name#accordion-}"
case "$suffix" in
    ios) suffix_title="iOS" ;;
    *) suffix_title="$(tr '[:lower:]' '[:upper:]' <<< "${suffix:0:1}")${suffix:1}" ;;
esac
title="Accordion $suffix_title"

cp -r "$suite_repo/templates/repo" "$target"
if [[ "$typescript" == yes ]]; then
    rm "$target/deny.toml"
    cp -r "$suite_repo/templates/repo-typescript/." "$target/"
fi
cp "$suite_repo/.gitattributes" "$suite_repo/LICENSE" "$target/"
sed -i "s/{{TITLE}}/$title/" "$target/README.md"

cd "$target"
git init -q -b main
git add -A
git commit -q -m "Set up repo from the Accordion template"
git remote add origin "https://github.com/accordion-suite/$name.git"

cat <<EOF
Created $target

Next:
  1. Create an empty repo named $name at https://github.com/organizations/accordion-suite/repositories/new
     (don't add a README, license, or .gitignore).
  2. Push it:  git -C "$target" push -u origin main
  3. Describe the repo in README.md.

EOF
if [[ "$typescript" == yes ]]; then
    echo "CI starts working once the repo has a package.json with check, test, and build scripts, and a package-lock.json."
else
    echo "CI starts working once the repo has a Cargo workspace with a Cargo.lock."
fi
