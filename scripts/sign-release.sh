#!/usr/bin/env bash
# Signs the release files that scripts/check-release.sh checked, with the
# release key (decision 9).
#
# Usage: scripts/sign-release.sh v0.1.0
#        scripts/sign-release.sh <run ID>
#
# It signs CI's SHA256SUMS and checks the signature against
# packaging/release-key.pem. For a tag, it then uploads SHA256SUMS.sig to the
# draft release. For a trial run, it leaves the signed files here for testing.
#
# The release key's private half is read from
# ~/.config/accordion/release-signing-key.pem, or from the file in
# ACCORDION_SIGNING_KEY. openssl asks for its passphrase.

set -euo pipefail

REPO=accordion-suite/accordion

die() {
    echo "$*" >&2
    exit 1
}

[[ $# -eq 1 ]] || die "Usage: scripts/sign-release.sh v0.1.0, or the ID of a trial run"
release="$1"
cd "$(dirname "$0")/.."
ci=target/release-check/ci
key="${ACCORDION_SIGNING_KEY:-$HOME/.config/accordion/release-signing-key.pem}"

# Only sign what was checked, and not changed since.
[[ -f target/release-check/checked \
    && "$(cat target/release-check/checked)" == "$release $(sha256sum < "$ci/SHA256SUMS")" ]] \
    || die "These files haven't been checked against a local build. Run scripts/check-release.sh $release first."
[[ -f "$key" ]] \
    || die "There's no release key at $key. Set ACCORDION_SIGNING_KEY to the file that holds it."

openssl pkeyutl -sign -inkey "$key" -rawin -in "$ci/SHA256SUMS" -out "$ci/SHA256SUMS.sig" \
    || die "openssl couldn't sign SHA256SUMS. Check the passphrase, then try again."
openssl pkeyutl -verify -pubin -inkey packaging/release-key.pem -rawin \
    -in "$ci/SHA256SUMS" -sigfile "$ci/SHA256SUMS.sig" > /dev/null 2>&1 \
    || die "The signature doesn't match packaging/release-key.pem, so $key isn't the release key."

if [[ "$release" =~ ^v[0-9] ]]; then
    gh release upload "$release" "$ci/SHA256SUMS.sig" --repo "$REPO" --clobber
    echo "Signed $release and uploaded SHA256SUMS.sig to the draft release."
    echo "Add release notes on GitHub, then publish the release there or with:"
    echo "  gh release edit $release --repo $REPO --draft=false"
else
    folder="$(pwd)/$ci"
    echo "Signed the trial run's files in $folder."
    echo "To try installing them, copy the folder to a test machine and run install.sh from it:"
    echo "  sudo env ACCORDION_DOWNLOAD_URL=file://<folder> sh <folder>/install.sh"
fi
