#!/usr/bin/env bash
# Pushes every Accordion repo in this workspace in the order the pins need,
# then pins the suite to what it pushed and pushes the suite.
#
# Usage: scripts/ship.sh ["<commit message>"]
#
# The order is accordion-core, accordion-gateway, the modules, and
# accordion-web, then this repo. Commit your work in each repo first: the
# script stops if one has uncommitted changes, is on another branch, or is
# behind GitHub.
#
# With a message, the suite's uncommitted changes go into one commit with the
# new pins, such as "Add Settings, and pin core and web". Without one, the
# suite has to be committed already, and the pins get a commit of their own
# if they changed.
#
# Cargo.lock is committed right after scripts/pin-versions.sh, with no cargo
# command in between, because a local build strips the pins.

set -euo pipefail

die() {
    echo "ship: $*" >&2
    exit 1
}

say() {
    echo "ship: $*"
}

suite="$(cd "$(dirname "$0")/.." && pwd)"
workspace="$(dirname "$suite")"
org_url="https://github.com/accordion-suite"
message="${1:-}"
[[ $# -le 1 ]] || die "Usage: scripts/ship.sh [\"<commit message>\"]. Put the message in quotes."

# The repos the suite pins, from its crates' git dependencies, in the order
# they depend on each other: core first, the modules after it, and the web
# app, which the suite pins in packaging/web-commit, last.
mapfile -t pinned < <(grep -hoE "git = \"$org_url/[a-z0-9-]+\"" "$suite"/crates/*/Cargo.toml \
    | sed -E "s|.*$org_url/([a-z0-9-]+)\"|\1|" | sort -u)
order=(accordion-core accordion-gateway)
for repo in "${pinned[@]}"; do
    [[ " ${order[*]} " == *" $repo "* ]] || order+=("$repo")
done
order+=(accordion-web)

# Checks a repo before anything is pushed, so a problem in a later repo
# doesn't leave the earlier ones pushed without their pin.
check() {
    local dir="$1" name="$2" allow_changes="$3" branch
    branch="$(git -C "$dir" symbolic-ref --short -q HEAD || true)"
    [[ "$branch" == main ]] || die "$name is on ${branch:-a detached HEAD}, not main. Switch to main, then try again."
    if [[ -z "$allow_changes" && -n "$(git -C "$dir" status --porcelain)" ]]; then
        die "$name has uncommitted changes. Commit them, or stash them, then try again."
    fi
    git -C "$dir" fetch -q origin main || die "Couldn't fetch $name from GitHub. Check your connection, then try again."
    [[ -z "$(git -C "$dir" rev-list HEAD..origin/main)" ]] \
        || die "$name is behind GitHub. Pull, check that it still builds, then try again."
}

ahead() {
    git -C "$1" rev-list --count origin/main..HEAD
}

for repo in "${order[@]}"; do
    [[ -d "$workspace/$repo/.git" ]] || die "$repo isn't in $workspace. Run scripts/clone-workspace.sh, then try again."
    check "$workspace/$repo" "$repo" ""
done
check "$suite" accordion 1
if [[ -z "$message" && -n "$(git -C "$suite" status --porcelain)" ]]; then
    die "accordion has uncommitted changes. Commit them, or give a commit message to commit them with the pins."
fi

for repo in "${order[@]}"; do
    count="$(ahead "$workspace/$repo")"
    if [[ "$count" == 0 ]]; then
        say "$repo is up to date."
        continue
    fi
    git -C "$workspace/$repo" push -q origin main || die "Couldn't push $repo. Git's message above says why."
    say "pushed $repo ($count commit$([[ "$count" == 1 ]] || echo s))."
done

"$suite/scripts/pin-versions.sh"

cd "$suite"
pins_changed=
git diff --quiet -- Cargo.lock packaging/web-commit || pins_changed=1
if [[ -n "$message" ]]; then
    git add -A
    git diff --cached --quiet && die "accordion has nothing to commit, so the message isn't used. Run scripts/ship.sh without it."
    git status --short
    git commit -q -m "$message"
    say "committed \"$message\"."
elif [[ -n "$pins_changed" ]]; then
    # Names what moved, such as "Pin core, Drive, and web". Modules are
    # products, so their names are capitalized.
    names=()
    while read -r repo; do
        case "$repo" in
            core | gateway) names+=("$repo") ;;
            *) names+=("${repo^}") ;;
        esac
    done < <(git diff -U0 -- Cargo.lock \
        | grep -oE "^\+source = \"git\+$org_url/accordion-[a-z0-9-]+" \
        | sed -E 's|.*/accordion-||' | sort -u)
    git diff --quiet -- packaging/web-commit || names+=(web)
    case "${#names[@]}" in
        0) list="the latest pushed commits" ;;
        1) list="${names[0]}" ;;
        2) list="${names[0]} and ${names[1]}" ;;
        *) list="$(printf '%s, ' "${names[@]:0:${#names[@]}-1}")and ${names[-1]}" ;;
    esac
    git add Cargo.lock packaging/web-commit
    git commit -q -m "Pin $list"
    say "committed \"Pin $list\"."
fi

count="$(ahead "$suite")"
if [[ "$count" == 0 ]]; then
    say "accordion is up to date, so there's nothing to ship."
    exit 0
fi
git push -q origin main || die "Couldn't push accordion. Git's message above says why."
say "pushed accordion ($count commit$([[ "$count" == 1 ]] || echo s)). CI and the upgrade test run on it now."
