#!/usr/bin/env bash
# Build the 32-bit x86 release binary the web demo runs
# (docs/research/web-demo.md, stage 1). The emulator in the visitor's tab is a
# 32-bit machine, so this is the one place the application is built for i686.
#
#   tools/web_demo/build_i686.sh --make-sysroot <dir>   # once, on Ubuntu, without root
#   SYSROOT=<dir> tools/web_demo/build_i686.sh           # build against it
#   tools/web_demo/build_i686.sh                         # build with multilib installed
#
# The binary is target/web-demo/cargo/i686-unknown-linux-gnu/release/mindfork;
# the libraries it loads are in <dir>/usr/lib32 and <dir>/usr/lib/i386-linux-gnu
# (or, with multilib, the same two directories under /).
#
# Tried here: the sysroot path, on Ubuntu 24.04 with gcc 13. The multilib path
# (`gcc-multilib` and `libasound2-dev:i386` installed by root, as a CI runner
# would) is the same build without the sysroot flags and has not been run.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
target=i686-unknown-linux-gnu

make_sysroot() {
    local dir="$1"
    mkdir -p "$dir/debs"
    (
        cd "$dir/debs"
        # Ubuntu ships the 32-bit C library and compiler runtime as amd64
        # packages, so `apt-get download` fetches them with no i386
        # architecture enabled - which enabling would need root for.
        local gcc_major
        gcc_major="$(gcc -dumpversion | cut -d. -f1)"
        apt-get download libc6-dev-i386 libc6-i386 "lib32gcc-${gcc_major}-dev" lib32gcc-s1
        # ALSA has no such package: its i386 build is fetched by name, at the
        # version of the amd64 one installed here, so headers and library agree.
        local alsa
        alsa="$(dpkg-query -W -f='${Version}' libasound2t64)"
        for package in libasound2t64 libasound2-dev; do
            local deb="${package}_${alsa}_i386.deb"
            [ -s "$deb" ] && continue
            curl -sfL -o "$deb" "http://archive.ubuntu.com/ubuntu/pool/main/a/alsa-lib/$deb" ||
                curl -sfL -o "$deb" "http://security.ubuntu.com/ubuntu/pool/main/a/alsa-lib/$deb"
        done
        for deb in *.deb; do dpkg-deb -x "$deb" ..; done
    )
    # The C library's linker script names /lib32 and /lib, which a merged-/usr
    # system has as links that no package carries.
    [ -e "$dir/lib32" ] || ln -s usr/lib32 "$dir/lib32"
    [ -e "$dir/lib" ] || ln -s usr/lib "$dir/lib"
    ln -sf ../lib32/ld-linux.so.2 "$dir/usr/lib/ld-linux.so.2"
    echo "sysroot: $dir"
}

if [ "${1:-}" = "--make-sysroot" ]; then
    [ -n "${2:-}" ] || { echo "usage: $0 --make-sysroot <dir>" >&2; exit 2; }
    mkdir -p "$2"
    make_sysroot "$(cd "$2" && pwd)"
    exit 0
fi

rustup target add "$target" >/dev/null
export CARGO_TARGET_DIR="$root/target/web-demo/cargo"
export PKG_CONFIG_ALLOW_CROSS=1

if [ -n "${SYSROOT:-}" ]; then
    sysroot="$(cd "$SYSROOT" && pwd)"
    gcc_dir="$(echo "$sysroot"/usr/lib/gcc/x86_64-linux-gnu/*/32)"
    export PKG_CONFIG_LIBDIR="$sysroot/usr/lib/i386-linux-gnu/pkgconfig"
    export PKG_CONFIG_SYSROOT_DIR="$sysroot"
    # The sysroot's headers first, then the shared ones a multiarch system
    # keeps under the amd64 name - `gcc -m32` does not look there by itself.
    export CFLAGS_i686_unknown_linux_gnu="-m32 -isystem $sysroot/usr/include -isystem /usr/include/x86_64-linux-gnu"
    export CARGO_TARGET_I686_UNKNOWN_LINUX_GNU_LINKER=gcc
    export CARGO_TARGET_I686_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=--sysroot=$sysroot -C link-arg=-B$gcc_dir -C link-arg=-L$gcc_dir -C link-arg=-L$sysroot/usr/lib32 -C link-arg=-L$sysroot/usr/lib/i386-linux-gnu"
else
    export PKG_CONFIG_LIBDIR=/usr/lib/i386-linux-gnu/pkgconfig
fi

cd "$root"
cargo build --release --target "$target"
ls -l "$CARGO_TARGET_DIR/$target/release/mindfork"
