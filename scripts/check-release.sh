#!/usr/bin/env bash
# Checks CI's release files against a build of the same commit on this
# computer, before the release is signed (decision 9).
#
# Usage: scripts/check-release.sh v0.1.0
#        scripts/check-release.sh <run ID>
#
# With a tag, it checks the draft release that CI created for the tag. With
# the ID of a trial run of the release workflow, it checks that run's files
# instead. Either way, it checks out the commit CI built, so run it in the
# clone you build releases in. It needs gh, signed in to GitHub.
#
# The packages can differ in their bytes when CI's dpkg is another version,
# so they're compared by the files they hold. install.sh must be identical.
# The checked files stay in target/release-check/ci for
# scripts/sign-release.sh.

set -euo pipefail

REPO=accordion-suite/accordion

die() {
    echo "$*" >&2
    exit 1
}

# Everything runs from main, on the last line, which bash reads in full
# before running it, because checking out another commit replaces this file.
main() {
    [[ $# -eq 1 ]] || die "Usage: scripts/check-release.sh v0.1.0, or the ID of a trial run"
    release="$1"
    cd "$(dirname "$0")/.."

    command -v gh > /dev/null \
        || die "This needs gh, GitHub's command-line tool. Install it with: sudo apt install gh. Then sign in with: gh auth login"
    gh auth status > /dev/null 2>&1 \
        || die "gh isn't signed in to GitHub. Sign in with: gh auth login. Then try again."
    [[ -z "$(git status --porcelain)" ]] \
        || die "This checkout has changes that aren't committed. Run this in a clean clone that you build releases in."

    rm -rf target/release-check
    mkdir -p target/release-check/ci
    ci=target/release-check/ci

    if [[ "$release" =~ ^v[0-9] ]]; then
        # Fails if a tag here has the same name but another commit.
        git fetch --quiet origin "refs/tags/$release:refs/tags/$release" \
            || die "Couldn't fetch the tag $release from GitHub, or a tag here with that name points somewhere else."
        commit="$(git rev-parse "$release^{commit}")"
        gh release download "$release" --repo "$REPO" --dir "$ci" \
            || die "Couldn't download the draft release for $release. Check that the release workflow finished for the tag."
    elif [[ "$release" =~ ^[0-9]+$ ]]; then
        commit="$(gh run view "$release" --repo "$REPO" --json headSha --jq .headSha)"
        git fetch --quiet origin
        gh run download "$release" --repo "$REPO" --name release-files --dir "$ci" \
            || die "Couldn't download the files from run $release. Check that it's a finished run of the release workflow."
    else
        die "$release isn't a tag, such as v0.1.0, or a run ID."
    fi

    echo "Checking out $commit, the commit CI built."
    git checkout --quiet --detach "$commit"

    (cd "$ci" && sha256sum --check --quiet SHA256SUMS) \
        || die "CI's files don't match CI's own SHA256SUMS. Download them again, or run the release workflow again."

    scripts/build-packages.sh target/release-check/local

    files="$(awk '{print $2}' "$ci/SHA256SUMS" | sort)"
    [[ "$files" == "$(awk '{print $2}' target/release-check/local/SHA256SUMS | sort)" ]] \
        || die "CI's SHA256SUMS lists different files from this build's."
    failed=
    for file in $files; do
        if cmp -s "$ci/$file" "target/release-check/local/$file"; then
            echo "$file: identical"
        elif [[ "$file" == *.deb ]] \
            && python3 -I scripts/compare-debs.py "$ci/$file" "target/release-check/local/$file"; then
            echo "$file: the same files, packed differently by another version of dpkg"
        else
            echo "$file: different"
            failed=1
        fi
    done
    [[ -z "$failed" ]] \
        || die "CI's release files don't match this build, so don't sign them. Find out why before releasing."

    echo "$release $(sha256sum < "$ci/SHA256SUMS")" > target/release-check/checked
    echo "CI's release files match this build. Sign them with: scripts/sign-release.sh $release"
}

main "$@"; exit
