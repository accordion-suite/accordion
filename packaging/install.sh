#!/bin/sh
# Installs Accordion on Debian, Ubuntu, or Raspberry Pi OS (decision 9):
#
#   curl -fsSL https://github.com/accordion-suite/accordion/releases/latest/download/install.sh | sudo sh
#
# It downloads this release's package for the machine, checks the release
# signature and the package's checksum, and installs the package with apt.
# scripts/build-packages.sh fills in the version and the release key, so only
# the copy in a release works.
#
# Everything runs from main, on the last line, so a download that stops
# partway runs nothing.

set -eu

ACCORDION_VERSION=@VERSION@
# The public half of the release key: the line between its PEM file's
# BEGIN and END lines.
RELEASE_KEY=@RELEASE_KEY@
# Another place to download the release from, for testing. The signature
# check still applies.
DOWNLOAD_URL="${ACCORDION_DOWNLOAD_URL:-https://github.com/accordion-suite/accordion/releases/download/v$ACCORDION_VERSION}"
ISSUES=https://github.com/accordion-suite/accordion/issues

say() {
    printf '%s\n' "$*"
}

fail() {
    printf '%s\n' "$*" >&2
    exit 1
}

check_system() {
    [ "$(id -u)" = 0 ] \
        || fail "The installer needs to run as root. Run it again with sudo in front of sh."

    [ -r /etc/os-release ] \
        || fail "The installer couldn't tell which system this is, because /etc/os-release is missing."
    # Each in a subshell, because os-release sets VERSION and other names
    # that would clash with this script's.
    os_id=$(. /etc/os-release && printf '%s' "${ID:-}")
    os_version=$(. /etc/os-release && printf '%s' "${VERSION_ID:-}")
    os_name=$(. /etc/os-release && printf '%s' "${PRETTY_NAME:-}")
    major=${os_version%%.*}
    case "$os_id:$major" in
        debian:1[2-9] | debian:[2-9][0-9] | raspbian:1[2-9] | raspbian:[2-9][0-9]) ;;
        ubuntu:2[4-9] | ubuntu:[3-9][0-9]) ;;
        *) fail "Accordion runs on Debian 12 or later, Ubuntu 24.04 or later, and Raspberry Pi OS based on Debian 12 or later. This machine runs ${os_name:-a system the installer doesn't know}." ;;
    esac

    command -v dpkg > /dev/null && command -v apt-get > /dev/null \
        || fail "The installer needs apt and dpkg, which $os_name should have. Install them, then try again."
    arch=$(dpkg --print-architecture)
    case "$arch" in
        amd64 | arm64) ;;
        armhf | armel | i386) fail "Accordion runs on 64-bit systems, and this one is 32-bit ($arch). On a Raspberry Pi, install Raspberry Pi OS (64-bit), then try again." ;;
        *) fail "Accordion runs on x86_64 and ARM64 machines, and this one is $arch." ;;
    esac

    [ -d /run/systemd/system ] \
        || fail "Accordion runs as a systemd service, and systemd isn't running on this machine. On WSL, turn it on with systemd=true under [boot] in /etc/wsl.conf, restart WSL, then try again."
}

download() {
    curl -fsSL --retry 3 -o "$dir/$1" "$DOWNLOAD_URL/$1" \
        || fail "The installer couldn't download $1 from $DOWNLOAD_URL. Check that this machine is online, then try again."
    chmod 0644 "$dir/$1"
}

verify() {
    if ! command -v openssl > /dev/null; then
        say "Installing openssl, which checks the download's signature."
        apt-get install -y -qq openssl > /dev/null \
            || fail "The installer couldn't install openssl. Install it with: sudo apt install openssl. Then try again."
    fi
    printf '%s\n%s\n%s\n' '-----BEGIN PUBLIC KEY-----' "$RELEASE_KEY" '-----END PUBLIC KEY-----' \
        > "$dir/release-key.pem"
    openssl pkeyutl -verify -pubin -inkey "$dir/release-key.pem" -rawin \
        -in "$dir/SHA256SUMS" -sigfile "$dir/SHA256SUMS.sig" > /dev/null 2>&1 \
        || fail "The download doesn't match Accordion's release signature, so the installer stopped without installing anything. The download might be damaged, or someone might have changed it. Try again later, and if it happens again, report it at $ISSUES."
    (cd "$dir" && awk -v file="$deb" '$2 == file' SHA256SUMS | sha256sum --check --status) \
        || fail "The package doesn't match its checksum in the signed SHA256SUMS, so the installer stopped without installing anything. The download might be damaged. Try again, and if it happens again, report it at $ISSUES."
}

# Shows where to open the setup wizard, from the line Accordion logs when it
# starts.
show_wizard() {
    since=$1
    tries=0
    while [ "$tries" -lt 30 ]; do
        log=$(journalctl -u accordion.service -o cat --since "$since" 2> /dev/null || true)
        wizard=$(printf '%s\n' "$log" | sed -n "s/.*\(Accordion isn't set up yet\.\)/\1/p" | tail -n 1)
        if [ -n "$wizard" ]; then
            say ""
            say "$wizard" | fold -s -w 78
            say ""
            say "To show the setup code again later, run: sudo journalctl -u accordion"
            return
        fi
        name=$(printf '%s\n' "$log" | sed -n 's/.*serving \(https:\/\/[^ ]*\) on .*/\1/p' | tail -n 1)
        if [ -n "$name" ]; then
            say "Accordion is running at $name, with the data it already had."
            return
        fi
        if systemctl is-failed --quiet accordion.service; then
            say "Accordion is installed, but it stopped right after it started. Its last messages:" >&2
            journalctl -u accordion.service -o cat -n 5 --no-pager >&2 || true
            fail "See the rest with: sudo journalctl -u accordion"
        fi
        tries=$((tries + 1))
        sleep 1
    done
    say "Accordion is installed and starting. To find the setup wizard's address and code, run: sudo journalctl -u accordion"
}

main() {
    check_system
    deb="accordion_${ACCORDION_VERSION}_${arch}.deb"
    say "Installing Accordion $ACCORDION_VERSION on $os_name ($arch)."

    dir=$(mktemp -d)
    trap 'rm -rf "$dir"' EXIT
    trap 'exit 1' INT TERM
    # apt reads the package as its own unprivileged user.
    chmod 0755 "$dir"

    download SHA256SUMS
    download SHA256SUMS.sig
    download "$deb"
    verify

    installed=$(dpkg-query -W -f '${Status} ${Version}' accordion 2> /dev/null || true)
    case "$installed" in
        "install ok installed "*) previous=${installed##* } ;;
        *) previous= ;;
    esac
    since=$(date '+%Y-%m-%d %H:%M:%S')
    say "Updating the list of system packages."
    apt-get update -qq \
        || fail "apt couldn't update its list of packages. Check that this machine is online, then try again."
    say "Installing the package."
    DEBIAN_FRONTEND=noninteractive apt-get install -y -q "$dir/$deb" \
        || fail "apt couldn't install Accordion. Its messages above say why."

    case "$previous" in
        "") show_wizard "$since" ;;
        "$ACCORDION_VERSION") say "Accordion $ACCORDION_VERSION was already installed." ;;
        *) say "Accordion is updated from $previous to $ACCORDION_VERSION." ;;
    esac
}

main
