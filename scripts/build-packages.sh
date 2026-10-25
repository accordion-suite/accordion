#!/usr/bin/env bash
# Builds the release packages, accordion_<version>_amd64.deb and
# accordion_<version>_arm64.deb, and their SHA256SUMS.
#
# Usage: scripts/build-packages.sh [output folder]
#
# The output folder defaults to target/packages. CI runs this for each
# release, and the developer runs it again before signing one, so the two
# builds can be compared (decision 9). For that, the same commit must give
# the same files on any machine:
#
# - Rust is the version in rust-toolchain.toml, and zig, which compiles the C
#   parts and links, is the version below.
# - Paths to the checkout and Cargo's folder are replaced in the program.
# - Every file in the packages gets the release commit's time.
#
# Build from a clone outside the workspace. Inside it, Cargo builds your
# local checkouts of the other repos instead of the versions Cargo.lock pins,
# and this script stops.

set -euo pipefail

ZIG_VERSION=0.14.1
ZIGBUILD_VERSION=0.23.4
RESTIC_VERSION=0.19.1
# From restic's signed SHA256SUMS for this version.
declare -A RESTIC_SHA256=(
    [amd64]=f415415624dcc452f2a02b8c33641791a8c6d6d3b65bbb3543fcf9a25151585c
    [arm64]=a5f64aaab53d51e311fa3829124c5b703f2d14cf187d8640b6be3b2b49376465
)
declare -A RUST_TARGET=(
    [amd64]=x86_64-unknown-linux-musl
    [arm64]=aarch64-unknown-linux-musl
)
# The issues page rather than a person's email, so problems get reported
# where they're tracked. Debian's archive would want an email address, but
# apt and dpkg accept this.
MAINTAINER="Accordion <https://github.com/accordion-suite/accordion/issues>"

die() {
    echo "$*" >&2
    exit 1
}

repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo"
out="$(mkdir -p "${1:-target/packages}" && cd "${1:-target/packages}" && pwd)"
umask 022

[[ "$(zig version 2>/dev/null)" == "$ZIG_VERSION" ]] \
    || die "Packages need zig $ZIG_VERSION, so they match other builds. Install it, then try again."
[[ "$(cargo-zigbuild -V 2>/dev/null)" == "cargo-zigbuild $ZIGBUILD_VERSION" ]] \
    || die "Packages need cargo-zigbuild $ZIGBUILD_VERSION. Install it with: cargo install --locked cargo-zigbuild --version $ZIGBUILD_VERSION"
[[ -z "$(git status --porcelain)" ]] \
    || die "This checkout has changes that aren't committed. Packages are built from a commit, so commit or stash them first."
rustup toolchain install --no-self-update >/dev/null
rustup target add "${RUST_TARGET[@]}" >/dev/null
# Crates from other repos should come from GitHub, not from a local path.
if cargo tree --locked -p accordion-server -e normal --prefix none \
    | grep '^accordion-[a-z-]* v[^ ]* (/' | grep -qv " ($repo/"; then
    die "This checkout builds your local copies of other Accordion repos. Build from a clone outside the workspace instead."
fi

version="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)"
SOURCE_DATE_EPOCH="$(git log -1 --format=%ct)"
export SOURCE_DATE_EPOCH
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
export RUSTFLAGS="--remap-path-prefix=$cargo_home=/cargo --remap-path-prefix=$repo=/build"
export CFLAGS="-ffile-prefix-map=$cargo_home=/cargo -ffile-prefix-map=$repo=/build"

targets=()
for target in "${RUST_TARGET[@]}"; do
    targets+=(--target "$target")
done
cargo zigbuild --locked --release -p accordion-server "${targets[@]}"

python3 -I packaging/third-party-licenses.py "${RUST_TARGET[@]}" > target/third-party-licenses.txt

for arch in amd64 arm64; do
    restic="restic_${RESTIC_VERSION}_linux_${arch}.bz2"
    mkdir -p target/restic
    if [[ ! -f "target/restic/$restic" ]]; then
        curl -fsSLo "target/restic/$restic.part" \
            "https://github.com/restic/restic/releases/download/v${RESTIC_VERSION}/$restic"
        mv "target/restic/$restic.part" "target/restic/$restic"
    fi
    echo "${RESTIC_SHA256[$arch]}  target/restic/$restic" | sha256sum --check --quiet \
        || die "The restic download isn't the file restic published. Delete target/restic and try again."

    root="target/package-$arch"
    rm -rf "$root"
    install -D -m 0755 "target/${RUST_TARGET[$arch]}/release/accordion" "$root/usr/bin/accordion"
    mkdir -p "$root/usr/lib/accordion"
    bunzip2 --stdout "target/restic/$restic" > "$root/usr/lib/accordion/restic"
    chmod 0755 "$root/usr/lib/accordion/restic"
    install -D -m 0644 packaging/systemd/accordion.service "$root/usr/lib/systemd/system/accordion.service"
    install -D -m 0644 packaging/systemd/accordion.sysusers "$root/usr/lib/sysusers.d/accordion.conf"
    install -D -m 0644 packaging/debian/copyright "$root/usr/share/doc/accordion/copyright"
    install -m 0644 LICENSE "$root/usr/share/doc/accordion/LICENSE"
    install -m 0644 target/third-party-licenses.txt "$root/usr/share/doc/accordion/third-party-licenses.txt"

    mkdir -p "$root/DEBIAN"
    for script in postinst prerm postrm; do
        install -m 0755 "packaging/debian/$script" "$root/DEBIAN/$script"
    done
    cat > "$root/DEBIAN/control" <<EOF
Package: accordion
Version: $version
Architecture: $arch
Maintainer: $MAINTAINER
Installed-Size: $(du -s -k --apparent-size "$root/usr" | cut -f 1)
Depends: systemd (>= 252), ca-certificates
Recommends: openssh-client
Section: net
Priority: optional
Homepage: https://github.com/accordion-suite/accordion
Description: Self-hosted Drive, Docs, Mail, and Notes
 Accordion replaces Google Workspace on hardware you own, with one sign-in,
 one backup, and zero telemetry.
EOF
    find "$root" -exec touch --no-dereference --date="@$SOURCE_DATE_EPOCH" {} +
    # One compression thread, because xz splits its output differently with
    # more, and machines have different numbers of cores.
    dpkg-deb --root-owner-group --threads-max=1 -Zxz \
        --build "$root" "$out/accordion_${version}_${arch}.deb"
done

(cd "$out" && sha256sum "accordion_${version}_amd64.deb" "accordion_${version}_arm64.deb" > SHA256SUMS)
echo "Built in $out:"
cat "$out/SHA256SUMS"
