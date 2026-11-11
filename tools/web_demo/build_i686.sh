#!/usr/bin/env bash
# Build the 32-bit x86 release binary the web demo runs
# (docs/research/web-demo.md §13.5). The emulator in the visitor's tab is a
# 32-bit machine, so this is the one place the application is built for i686.
#
#   tools/web_demo/build_i686.sh
#
# On Ubuntu 24.04, without root - a developer's machine and the release's
# runner take the same road. Everything lands under target/web-demo/:
#
#   sysroot/   the 32-bit libraries the binary links against and the guest
#              then loads, unpacked from the distribution's own packages, which
#              stay beside them in sysroot/debs/ - the manifest reads each
#              library's package, version and copyright file from there;
#   cargo/     the build: cargo/i686-unknown-linux-gnu/release/mindfork.
#
# The other way to build for i686 - `gcc-multilib` and `libasound2-dev:i386`
# installed by root - was never run and is not here: it needs a second
# architecture enabled in apt, and it would leave the release building what no
# one can build at their desk.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
target=i686-unknown-linux-gnu
sysroot="$root/target/web-demo/sysroot"

# ALSA's 32-bit build is not among the packages apt knows here - that takes the
# i386 architecture, which takes root - so its two packages are fetched by
# name and checked against these hashes. The other packages come through
# `apt-get download`, which verifies them against the archive's signed index.
alsa_debs=(
    "libasound2t64_1.2.11-1ubuntu0.3_i386.deb a10771c96026b43a9bbbef2db5429fb6e4799e44195a68a8d6138604537913a7"
    "libasound2-dev_1.2.11-1ubuntu0.3_i386.deb 01f9c499ed37f9511f4794eef822d6a6a5edf6af90a3aea5b77adcb1ed60fbb8"
)

# Where they are looked for, in order. Launchpad first: it keeps every
# published build for good, so the pin cannot go stale there. Then mirrors of
# the archive on other people's networks, which carry what the archive
# publishes today - the pinned build until Ubuntu supersedes it. They are here
# because of the release of 0.26.0 (2026-11-11): Canonical's network was down,
# Launchpad answered neither by name nor by address, and the release went to
# its draft without the demo. Whichever source answers, the file is the one
# the hash names or it is not used.
alsa_sources=(
    "https://launchpad.net/ubuntu/+archive/primary/+files"
    "https://mirrors.edge.kernel.org/ubuntu/pool/main/a/alsa-lib"
    "https://mirrors.mit.edu/ubuntu/pool/main/a/alsa-lib"
)

# One pinned package into the current directory, from the first source that
# has it. A source is given a minute and one try: a host that is down does not
# refuse, it hangs - on that release day curl sat on Launchpad for 13 minutes
# 43 seconds before it gave up - and the next source is the retry. One that
# answers with something else (a moved page, a newer build under the old name)
# is passed over like one that is down.
fetch_pinned() { # file name, its sha256
    local name="$1" sha256="$2" source
    for source in "${alsa_sources[@]}"; do
        # HTTPS at every hop: Launchpad answers with a redirect to its file
        # store, and a mirror may answer with one to another.
        if curl -sfL --proto '=https' --proto-redir '=https' --tlsv1.2 \
            --connect-timeout 15 --max-time 60 -o "$name" "$source/$name" &&
            echo "$sha256  $name" | sha256sum --check --quiet - >/dev/null 2>&1; then
            echo "$name: from ${source#https://}"
            return 0
        fi
        echo "$name: not from ${source#https://}" >&2
        rm -f -- "$name"
    done
    echo "error: $name is at none of the sources" >&2
    return 1
}

make_sysroot() {
    # A sysroot that was not finished is started again: a package left from an
    # earlier attempt would be unpacked over the newer one.
    rm -rf -- "$sysroot"
    mkdir -p "$sysroot/debs"
    (
        cd "$sysroot/debs"
        # Ubuntu ships the 32-bit C library and compiler runtime as amd64
        # packages, so apt fetches them with no second architecture enabled.
        local gcc_major
        gcc_major="$(gcc -dumpversion | cut -d. -f1)"
        apt-get download libc6-dev-i386 libc6-i386 "lib32gcc-${gcc_major}-dev" lib32gcc-s1
        # The compiler runtime's copyright file is in the package its
        # documentation directory is a link to - `gcc-N-base`, at its version.
        local runtime base
        runtime="$(echo lib32gcc-s1_*.deb)"
        base="$(dpkg-deb -f "$runtime" Depends | grep -o 'gcc-[0-9]*-base' | head -n 1)"
        apt-get download "${base}=$(dpkg-deb -f "$runtime" Version)"
        local pin deb sum
        for pin in "${alsa_debs[@]}"; do
            read -r deb sum <<<"$pin"
            fetch_pinned "$deb" "$sum"
        done
        for deb in *.deb; do dpkg-deb -x "$deb" ..; done
    )
    # The C library's linker script names /lib32 and /lib, which a merged-/usr
    # system has as links that no package carries.
    ln -s usr/lib32 "$sysroot/lib32"
    [[ -e "$sysroot/lib" ]] || ln -s usr/lib "$sysroot/lib"
    ln -sf ../lib32/ld-linux.so.2 "$sysroot/usr/lib/ld-linux.so.2"
    touch "$sysroot/.complete"
    echo "sysroot: $sysroot"
    return 0
}

if [[ ! -e "$sysroot/.complete" ]]; then
    make_sysroot
fi

rustup target add "$target" >/dev/null
gcc_dir="$(echo "$sysroot"/usr/lib/gcc/x86_64-linux-gnu/*/32)"
export CARGO_TARGET_DIR="$root/target/web-demo/cargo"
export PKG_CONFIG_ALLOW_CROSS=1
export PKG_CONFIG_LIBDIR="$sysroot/usr/lib/i386-linux-gnu/pkgconfig"
export PKG_CONFIG_SYSROOT_DIR="$sysroot"
# For the C the crates compile (onig, sqlite, aws-lc): the sysroot's headers
# first, then the shared ones a multiarch system keeps under the amd64 name -
# `gcc -m32` does not look there by itself. `TARGET_CFLAGS` is the name the
# `cc` crate reads for the target and never for a build script's own host code.
export TARGET_CFLAGS="-m32 -isystem $sysroot/usr/include -isystem /usr/include/x86_64-linux-gnu"
export CARGO_TARGET_I686_UNKNOWN_LINUX_GNU_LINKER=gcc
export CARGO_TARGET_I686_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=--sysroot=$sysroot -C link-arg=-B$gcc_dir -C link-arg=-L$gcc_dir -C link-arg=-L$sysroot/usr/lib32 -C link-arg=-L$sysroot/usr/lib/i386-linux-gnu"

cd "$root"
cargo build --release --locked --target "$target"
ls -l "$CARGO_TARGET_DIR/$target/release/mindfork"
