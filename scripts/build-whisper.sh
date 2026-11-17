#!/usr/bin/env bash
# Builds whisper.cpp's command-line program for transcribing voice memos
# (decision 33), for the package's /usr/lib/accordion:
#
# - whisper, for processors with AVX2, FMA, F16C, and BMI2 (Intel since
#   2013, AMD since 2015), or ARM's dot product and half-precision
#   instructions (Raspberry Pi 5 and other Cortex-A76 and later).
# - whisper-baseline, for older processors, such as Celerons in small NAS
#   boxes and the Raspberry Pi 4. It's slower, but it runs.
#
# The server picks one when it starts, by asking the processor.
#
# Usage: scripts/build-whisper.sh <amd64|arm64> <output folder>
#
# Each is one static program, built with zig against musl, like the server,
# so it needs no libraries and runs in the sandbox, which reads only the
# system's folders. scripts/build-packages.sh runs this, and the same
# commit gives the same programs on any machine: whisper.cpp is the commit
# below, CMake is the release below, checked against Kitware's hashes, and
# zig is the version build-packages.sh checks.

set -euo pipefail

WHISPER_VERSION=1.9.5
# The commit the v1.9.5 tag points to.
WHISPER_COMMIT=d1be6fde11ac6e0407606b4e42fe72d34add8037
WHISPER_REPO=https://github.com/ggml-org/whisper.cpp
CMAKE_VERSION=4.4.5
# From Kitware's cmake-4.4.5-SHA-256.txt.
declare -A CMAKE_SHA256=(
    [x86_64]=ed85135c5a5135b45e50423cb406dea04ca3338da0b8ef3c9ede963559680be0
    [aarch64]=301d5063ab259577c1b57ce5490b1aab9ea5c56dc4c822f73499553dab94925f
)
declare -A ZIG_TARGET=(
    [amd64]=x86_64-linux-musl
    [arm64]=aarch64-linux-musl
)
declare -A PROCESSOR=(
    [amd64]=x86_64
    [arm64]=aarch64
)
# The instructions each program may use. Without GGML_NATIVE, nothing
# depends on the computer that builds it. On x86, ggml's options choose
# them. On ARM, zig's processor features do, since zig doesn't take the
# -march that ggml's ARM option passes, and ggml follows what the compiler
# says it may use.
declare -A FAST=(
    [amd64]="-DGGML_SSE42=ON -DGGML_AVX=ON -DGGML_AVX2=ON -DGGML_FMA=ON -DGGML_F16C=ON -DGGML_BMI2=ON"
    [arm64]=""
)
declare -A FAST_CPU=(
    [amd64]=""
    [arm64]="-mcpu=generic+v8_2a+dotprod+fullfp16"
)
declare -A BASELINE=(
    [amd64]="-DGGML_SSE42=OFF -DGGML_AVX=OFF -DGGML_AVX2=OFF -DGGML_FMA=OFF -DGGML_F16C=OFF -DGGML_BMI2=OFF"
    [arm64]=""
)
declare -A BASELINE_CPU=(
    [amd64]=""
    [arm64]="-mcpu=generic"
)

die() {
    echo "$*" >&2
    exit 1
}

arch="${1:-}"
[[ -n "${ZIG_TARGET[$arch]:-}" ]] || die "Usage: $0 <amd64|arm64> <output folder>"
[[ -n "${2:-}" ]] || die "Usage: $0 <amd64|arm64> <output folder>"
repo="$(cd "$(dirname "$0")/.." && pwd)"
out="$(mkdir -p "$2" && cd "$2" && pwd)"
cd "$repo"
command -v zig >/dev/null || die "Building whisper.cpp needs zig. Install the version build-packages.sh names, then try again."

# CMake, for the computer that builds.
host="$(uname -m)"
[[ -n "${CMAKE_SHA256[$host]:-}" ]] \
    || die "whisper.cpp is built on x86_64 or ARM64 computers, and this one is $host."
cmake_archive="cmake-${CMAKE_VERSION}-linux-${host}.tar.gz"
mkdir -p target/cmake
if [[ ! -f "target/cmake/$cmake_archive" ]]; then
    curl -fsSLo "target/cmake/$cmake_archive.part" \
        "https://github.com/Kitware/CMake/releases/download/v${CMAKE_VERSION}/$cmake_archive"
    mv "target/cmake/$cmake_archive.part" "target/cmake/$cmake_archive"
fi
echo "${CMAKE_SHA256[$host]}  target/cmake/$cmake_archive" | sha256sum --check --quiet \
    || die "The CMake download isn't the file Kitware published. Delete target/cmake and try again."
cmake_dir="target/cmake/cmake-${CMAKE_VERSION}-linux-${host}"
[[ -x "$cmake_dir/bin/cmake" ]] || tar -xzf "target/cmake/$cmake_archive" -C target/cmake
cmake="$repo/$cmake_dir/bin/cmake"

# whisper.cpp at its commit.
src=target/whisper-src
if [[ "$(git -C "$src" rev-parse HEAD 2>/dev/null)" != "$WHISPER_COMMIT" ]]; then
    rm -rf "$src"
    git init -q "$src"
    git -C "$src" fetch -q --depth 1 "$WHISPER_REPO" "$WHISPER_COMMIT" \
        || die "Couldn't fetch whisper.cpp at $WHISPER_COMMIT from GitHub. Check the connection, then try again."
    git -C "$src" -c advice.detachedHead=false checkout -q FETCH_HEAD
fi

build_one() {
    local name="$1" flags="$2" cpu="$3"
    local build="$repo/target/whisper-build-$arch-$name"
    # zig as the compiler, through small programs CMake can call by name.
    local tools="$repo/target/whisper-tools-$arch-$name"
    mkdir -p "$tools"
    for tool in cc c++; do
        printf '#!/bin/sh\nexec zig %s -target %s %s "$@"\n' "$tool" "${ZIG_TARGET[$arch]}" "$cpu" > "$tools/$tool"
        chmod 0755 "$tools/$tool"
    done
    for tool in ar ranlib; do
        printf '#!/bin/sh\nexec zig %s "$@"\n' "$tool" > "$tools/$tool"
        chmod 0755 "$tools/$tool"
    done
    rm -rf "$build"
    # shellcheck disable=SC2086 # The flags are separate words.
    "$cmake" -S "$src" -B "$build" -G "Unix Makefiles" \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_SYSTEM_NAME=Linux \
        -DCMAKE_SYSTEM_PROCESSOR="${PROCESSOR[$arch]}" \
        -DCMAKE_C_COMPILER="$tools/cc" \
        -DCMAKE_CXX_COMPILER="$tools/c++" \
        -DCMAKE_AR="$tools/ar" \
        -DCMAKE_RANLIB="$tools/ranlib" \
        -DCMAKE_C_FLAGS="-ffile-prefix-map=$repo/$src=/whisper.cpp" \
        -DCMAKE_CXX_FLAGS="-ffile-prefix-map=$repo/$src=/whisper.cpp" \
        -DCMAKE_EXE_LINKER_FLAGS="-static -s" \
        -DBUILD_SHARED_LIBS=OFF \
        -DGGML_NATIVE=OFF \
        -DGGML_OPENMP=OFF \
        -DGGML_CCACHE=OFF \
        -DWHISPER_BUILD_TESTS=OFF \
        -DWHISPER_BUILD_SERVER=OFF \
        -DWHISPER_BUILD_EXAMPLES=ON \
        -DWHISPER_SDL2=OFF \
        -DWHISPER_CURL=OFF \
        $flags >"$build.log" 2>&1 \
        || die "CMake couldn't set up whisper.cpp's build. See $build.log."
    "$cmake" --build "$build" --target whisper-cli -j "$(nproc)" >>"$build.log" 2>&1 \
        || die "whisper.cpp didn't build. See $build.log."
    install -m 0755 "$build/bin/whisper-cli" "$out/$name"
}

build_one whisper "${FAST[$arch]}" "${FAST_CPU[$arch]}"
build_one whisper-baseline "${BASELINE[$arch]}" "${BASELINE_CPU[$arch]}"
install -m 0644 "$src/LICENSE" "$out/whisper-LICENSE"
echo "Built whisper.cpp $WHISPER_VERSION for $arch in $out"
