#!/usr/bin/env bash
# Tests that a server's data survives updates, by hand and through the update
# helper, and that a release that doesn't start rolls back (decision 12).
#
# Usage: scripts/upgrade-test.sh build <folder>
#        sudo scripts/upgrade-test.sh run <folder>
#
# build makes three test releases in <folder>, for this machine's
# architecture, with scripts/build-packages.sh and a throwaway signing key:
#
# - old: the newest release tag that has the update helper, or this commit
#   if no release has it yet.
# - new: this commit.
# - broken: the new package, with a server that stops as it starts.
#
# Each gets a version above the one before, so a commit can be tested before
# its version changes for a release. The builds use the last commit, not
# changes that aren't committed.
#
# run then takes a server through what an admin's server goes through:
#
# 1. Installs the newest published release from GitHub with its install
#    command, and adds an account, files, and a backup. If the old release is
#    the newest published one, it installs the old test release instead.
# 2. Installs the old release with its install command, the way servers
#    update by hand, and checks the data.
# 3. Updates to the new release from the Updates page's API, through the
#    update helper, and checks the data.
# 4. Tries to update to the broken release, and checks that the helper rolls
#    back to the new release with the data.
#
# run installs Accordion for the whole machine, then deletes it and its data.
# So it stops if Accordion is already installed, and is meant for a CI runner
# or a throwaway virtual machine. It downloads the published release from
# GitHub, and the server downloads updates from a web server on 127.0.0.1.
# CI runs both steps in .github/workflows/upgrade.yml.

set -euo pipefail

REPO_URL=https://github.com/accordion-suite/accordion
# Where the test web server serves the release that the server updates to.
UPDATE_PORT=8765
# For backups. The service can write under /mnt, but not elsewhere.
BACKUP_FOLDER=/mnt/accordion-upgrade-test

die() {
    echo "$*" >&2
    exit 1
}

say() {
    echo "upgrade test: $*"
}

repo="$(cd "$(dirname "$0")/.." && pwd)"

# The next patch version after <version>, such as 0.2.1 after 0.2.0.
bump() {
    local release="${1%%-*}"
    echo "${release%.*}.$((${release##*.} + 1))"
}

# Whether version $1 is newer than version $2.
newer() {
    dpkg --compare-versions "$1" gt "$2"
}

version_at() {
    git -C "$repo" show "$1:Cargo.toml" | sed -n 's/^version = "\(.*\)"$/\1/p' | head -n 1
}

# Builds <ref> as <version> into $out/release-<version>, and signs it.
build_release() {
    local ref="$1" version="$2"
    say "building $ref as $version."
    # Each build replaces the source but keeps target/, so later builds
    # only recompile Accordion's own crates.
    find "${src:?}" -mindepth 1 -maxdepth 1 ! -name target ! -name .git -exec rm -rf {} +
    git -C "$repo" archive "$ref" | tar -x -C "$src"
    sed -i "0,/^version = \".*\"/s//version = \"$version\"/" "$src/Cargo.toml"
    printf 'Security: no\nInstalls itself: yes\n\nTest release %s, built from %s.\n' \
        "$version" "$ref" > "$src/packaging/release-notes/$version.md"
    (
        cd "$src"
        cargo update --workspace --quiet
        git add -A
        git -c user.name="Upgrade test" -c user.email=upgrade-test@localhost \
            commit -q --allow-empty -m "Test release $version"
    )
    ACCORDION_RELEASE_KEY="$out/keys/test.pub.pem" ACCORDION_ARCHES="$arch" \
        "$src/scripts/build-packages.sh" "$out/release-$version"
    sign "$out/release-$version"
}

sign() {
    openssl pkeyutl -sign -inkey "$out/keys/test.pem" -rawin \
        -in "$1/SHA256SUMS" -out "$1/SHA256SUMS.sig"
}

# The new release's package, with a server that always stops as it starts.
build_broken() {
    local root="$out/broken-root" release="$out/release-$broken"
    say "building $broken, which doesn't start."
    rm -rf "${root:?}" "${release:?}"
    mkdir -p "$release"
    dpkg-deb -R "$out/release-$new/accordion_${new}_$arch.deb" "$root"
    printf '#!/bin/sh\necho "This test version of Accordion always stops as it starts." >&2\nexit 1\n' \
        > "$root/usr/bin/accordion"
    sed -i "s/^Version: .*/Version: $broken/" "$root/DEBIAN/control"
    dpkg-deb --root-owner-group -Zxz --build "$root" "$release/accordion_${broken}_$arch.deb" > /dev/null
    rm -rf "${root:?}"
    printf 'Security: no\nInstalls itself: yes\n\nTest release %s, which never starts.\n' \
        "$broken" > "$out/notes-$broken.md"
    python3 -I "$repo/packaging/update-json.py" "$broken" "$(date +%s)" "$out/notes-$broken.md" \
        > "$release/update.json"
    (cd "$release" && sha256sum "accordion_${broken}_$arch.deb" update.json > SHA256SUMS)
    sign "$release"
}

build() {
    [[ "$(id -u)" != 0 ]] || die "Run build as yourself, not as root, so it uses your Rust tools."
    if [[ -n "$(git -C "$repo" status --porcelain)" ]]; then
        say "this checkout has changes that aren't committed. The test builds the last commit without them."
    fi
    mkdir -p "$out/keys"
    src="$out/src"
    if [[ ! -d "$src/.git" ]]; then
        mkdir -p "$src"
        git -C "$src" init -q
    fi
    arch="$(dpkg --print-architecture)"

    # The newest release servers have, which a tag being built for release
    # isn't yet.
    published="$(git -C "$repo" tag --list 'v*' --no-contains HEAD --sort=-v:refname | head -n 1)"
    old_ref=HEAD
    if [[ -n "$published" ]] && git -C "$repo" cat-file -e "$published:crates/accordion-update/Cargo.toml" 2> /dev/null; then
        old_ref="$published"
    fi
    old="$(version_at "$old_ref")"
    if [[ "$old_ref" == HEAD && -n "$published" ]] && ! newer "$old" "${published#v}"; then
        old="$(bump "${published#v}")"
    fi
    new="$(version_at HEAD)"
    newer "$new" "$old" || new="$(bump "$old")"
    broken="$(bump "$new")"

    rm -f "$out/keys/test.pem" "$out/keys/test.pub.pem"
    openssl genpkey -algorithm ed25519 -out "$out/keys/test.pem"
    openssl pkey -in "$out/keys/test.pem" -pubout -out "$out/keys/test.pub.pem"

    rm -rf "${out:?}"/release-*
    build_release "$old_ref" "$old"
    build_release HEAD "$new"
    build_broken

    cat > "$out/versions" <<EOF
arch=$arch
published=$published
old_ref=$old_ref
old=$old
new=$new
broken=$broken
EOF
    say "built in $out:"
    cat "$out/versions"
}

installed_version() {
    dpkg-query -W -f '${Version}' accordion
}

# Points the test web server's release folder at release-<version>.
serve() {
    ln -sfn "../release-$1" "$out/serve/release"
}

cleanup() {
    local status=$?
    if [[ "$status" != 0 ]]; then
        echo "The test failed. The server's and the update helper's last messages:" >&2
        journalctl -u accordion.service -u accordion-update.service -o short-iso -n 80 --no-pager >&2 || true
    fi
    [[ -z "${web_server:-}" ]] || kill "$web_server" 2> /dev/null || true
    say "removing the test install."
    systemctl stop accordion-update.path accordion-update.service accordion.service 2> /dev/null || true
    DEBIAN_FRONTEND=noninteractive apt-get purge -y -q accordion > /dev/null 2>&1 || true
    rm -rf /etc/systemd/system/accordion.service.d /var/lib/accordion /var/lib/accordion-update \
        /mnt/accordion-upgrade-test
    rm -rf "${out:?}/serve" "${out:?}/state.json"
    systemctl daemon-reload
    [[ "$status" != 0 ]] || say "passed."
    exit "$status"
}

run() {
    [[ "$(id -u)" == 0 ]] || die "run installs Accordion, so it needs root. Run it again with sudo in front."
    [[ -d /run/systemd/system ]] || die "run needs systemd, which isn't running on this machine."
    [[ -f "$out/versions" ]] || die "There are no test releases in $out. Run scripts/upgrade-test.sh build $out first."
    if dpkg-query -W accordion > /dev/null 2>&1 || [[ -e /var/lib/accordion || -e /var/lib/accordion-update ]]; then
        die "Accordion is already on this machine. This test deletes Accordion and its data when it's done, so run it on a machine without Accordion, such as a throwaway virtual machine."
    fi
    local arch published old_ref old new broken
    # shellcheck source=/dev/null
    source "$out/versions"
    [[ "$arch" == "$(dpkg --print-architecture)" ]] \
        || die "The test releases are for $arch, but this machine is $(dpkg --print-architecture). Build them on this machine."
    local state="$out/state.json"
    trap cleanup EXIT

    mkdir -p "$out/serve"
    serve "$new"
    python3 -I -m http.server --bind 127.0.0.1 --directory "$out/serve" "$UPDATE_PORT" \
        > /dev/null 2>&1 &
    web_server=$!

    # The server's settings for the test. It's set before the owner is
    # created, so the server never reaches for GitHub, and doesn't announce
    # accordion.local on the runner's network.
    mkdir -p /etc/systemd/system/accordion.service.d
    printf '[Service]\nEnvironment=ACCORDION_UPDATE_URL=http://127.0.0.1:%s/release\nEnvironment=ACCORDION_LOCAL_NAME=off\n' \
        "$UPDATE_PORT" > /etc/systemd/system/accordion.service.d/upgrade-test.conf
    systemctl daemon-reload

    local first="$old"
    if [[ -n "$published" && "$old_ref" != "$published" ]]; then
        first="${published#v}"
        say "1. installing $published from GitHub."
        curl -fsSL --retry 3 -o "$out/published-install.sh" "$REPO_URL/releases/download/$published/install.sh"
        sh "$out/published-install.sh"
    else
        say "1. installing $old."
        ACCORDION_DOWNLOAD_URL="file://$out/release-$old" sh "$out/release-$old/install.sh"
    fi
    [[ "$(installed_version)" == "$first" ]] || die "$first didn't install."

    # The owner is the account a household's admin makes in the setup
    # wizard. With an owner and no name, the server serves plain HTTP on
    # 127.0.0.1:8080.
    printf 'correct horse\ncorrect horse\n' \
        | runuser -u accordion -- env ACCORDION_DATA_DIR=/var/lib/accordion \
            /usr/bin/accordion create-owner alex "Alex Kim"
    install -d -o accordion -g accordion -m 0700 "$BACKUP_FOLDER"
    systemctl restart accordion.service
    python3 -I "$repo/scripts/upgrade-test.py" add "$state"

    if [[ "$first" != "$old" ]]; then
        say "2. installing $old by hand, with its install command."
        ACCORDION_DOWNLOAD_URL="file://$out/release-$old" sh "$out/release-$old/install.sh"
        [[ "$(installed_version)" == "$old" ]] || die "$old didn't install."
        python3 -I "$repo/scripts/upgrade-test.py" check "$state" "$old"
    fi

    say "3. updating to $new through the update helper."
    python3 -I "$repo/scripts/upgrade-test.py" update "$state" "$new" installed "$new"
    [[ "$(installed_version)" == "$new" ]] || die "dpkg says $(installed_version) is installed, not $new."
    python3 -I "$repo/scripts/upgrade-test.py" check "$state" "$new"

    say "4. updating to $broken, which doesn't start, so it rolls back."
    serve "$broken"
    python3 -I "$repo/scripts/upgrade-test.py" update "$state" "$broken" rolled-back "$new"
    [[ "$(installed_version)" == "$new" ]] || die "dpkg says $(installed_version) is installed, not $new."
    python3 -I "$repo/scripts/upgrade-test.py" check "$state" "$new"
}

[[ $# -eq 2 ]] || die "Usage: scripts/upgrade-test.sh build <folder>, then sudo scripts/upgrade-test.sh run <folder>"
mkdir -p "$2"
out="$(cd "$2" && pwd)"
case "$1" in
    build) build ;;
    run) run ;;
    *) die "Usage: scripts/upgrade-test.sh build <folder>, then sudo scripts/upgrade-test.sh run <folder>" ;;
esac
