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
# name, from Launchpad, which keeps every published build for good, and
# checked against these hashes. The other packages come through
# `apt-get download`, which verifies them against the archive's signed index.
alsa_debs=(
    "libasound2t64_1.2.11-1ubuntu0.3_i386.deb a10771c96026b43a9bbbef2db5429fb6e4799e44195a68a8d6138604537913a7"
    "libasound2-dev_1.2.11-1ubuntu0.3_i386.deb 01f9c499ed37f9511f4794eef822d6a6a5edf6af90a3aea5b77adcb1ed60fbb8"
)

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
            curl -sfL --retry 3 -o "$deb" "https://launchpad.net/ubuntu/+archive/primary/+files/$deb"
            echo "$sum  $deb" | sha256sum --check --quiet -
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
