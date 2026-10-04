#!/bin/sh
# mindfork — the install script for Linux x86_64 and macOS on Apple Silicon.
#
#   curl -fsSL https://github.com/vshylov/mindfork-rs/releases/latest/download/install.sh | sh
#   … | sh -s -- --dir /workspace/mindfork -- setup --llama cuda-12 --model /models/chat.gguf --verify
#
# It puts the **portable** build into one directory — the binary, and beside it
# the `data/` the app keeps everything in — and then, optionally, hands the rest
# of the command line to `mindfork` itself. That is the whole job: everything
# else a fresh machine needs (llama.cpp, the Python sandbox, the settings) is
# `mindfork setup`, which is tested Rust and not shell (docs/install.md §3.3).
#
# Written for the machine that is **new every time** — a rented GPU box, a
# container whose own disk is cleared at every stop — and therefore idempotent:
# the same line again downloads nothing that is already there and only puts back
# what the container lost (docs/research/cloud-provisioning.md §4.4).
#
# On a Mac the same, minus what a Mac does not need: no glibc to check, no
# system library to install (CoreAudio and the rest are the OS's), no OpenMP
# for llama.cpp's macOS build (docs/research/macos.md §4.7). Downloaded with
# `curl`, nothing it installs is quarantined (§5.1).
#
# Options (all optional):
#   --dir DIR        where to install                      (default: $HOME/mindfork)
#   --version vX.Y.Z which release                         (default: the latest)
#   --from DIR       install from files already on disk — the archive and
#                    sha256sums.txt — instead of downloading them
#   --no-deps        Linux: do not install the system libraries the app needs
#                    (ALSA), and llama.cpp when the hand-over installs it (OpenMP)
#   --no-link        do not link the binary into /usr/local/bin
#   -h, --help
#   -- ARGS…         run `mindfork ARGS…` when the install is done
#
# POSIX sh, no bashisms: a minimal image has `dash` and nothing else. The whole
# script is one function called on its last line, so a download of it that was
# cut short runs nothing at all.

set -eu

REPO="vshylov/mindfork-rs"
MIN_GLIBC_MINOR=35 # the release is built on ubuntu-22.04: glibc 2.35

say() { printf '%s\n' "$*"; }
warn() { printf 'install.sh: %s\n' "$*" >&2; }
die() {
    warn "$*"
    exit 1
}

# Spelled out rather than read back from `$0`: piped into `sh`, there is no file.
usage() {
    cat <<'EOF'
mindfork install script (Linux x86_64, macOS on Apple Silicon)

  install.sh [OPTIONS] [-- MINDFORK ARGS…]

  --dir DIR         where to install                 (default: $HOME/mindfork)
  --version vX.Y.Z  which release                    (default: the latest)
  --from DIR        install from files already on disk (the archive and
                    sha256sums.txt) instead of downloading them
  --no-deps         Linux: do not install the system libraries the app needs
                    (ALSA), and llama.cpp when the hand-over installs it (OpenMP)
  --no-link         do not link the binary into /usr/local/bin
  -h, --help        this text
  -- ARGS…          run `mindfork ARGS…` when the install is done, e.g.
                    -- setup --llama cuda-12 --model /models/chat.gguf --verify

Running it again is safe: what is already in place is kept, and only what is
missing is put back. Your data (DIR/data) is never touched.
EOF
}

have() { command -v "$1" >/dev/null 2>&1; }

# ------------------------------------------------------------------ the platform

# Set by `check_platform`: which build, and the archive name's tail for it.
OS=""
SUFFIX=""

check_platform() {
    case "$(uname -s)" in
    Linux) check_linux ;;
    Darwin) check_macos ;;
    *) die "there is a prebuilt build for Linux x86_64 and for macOS on Apple Silicon; on $(uname -s) see docs/install.md" ;;
    esac
    have tar || die "tar is required"
}

# `uname -m` says x86_64 in a shell Rosetta translates, on Apple Silicon all the
# same — so the machine is asked, not the shell. An Intel Mac has no build: the
# sandbox's Wasmer has none for it, and llama.cpp's has no Metal
# (docs/research/macos.md §1, fork F1).
check_macos() {
    OS=macos
    SUFFIX=aarch64-macos
    if [ "$(uname -m)" != arm64 ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null || true)" != 1 ]; then
        die "the macOS build is for Apple Silicon; an Intel Mac has none — build from source with cargo (docs/install.md §1)"
    fi
    # macOS 26 has `sha256sum`; every macOS has `shasum`.
    have sha256sum || have shasum || die "shasum is required"
}

check_linux() {
    OS=linux
    SUFFIX=x86_64-linux
    case "$(uname -m)" in
    x86_64 | amd64) ;;
    *) die "a prebuilt archive exists for x86_64 only; on $(uname -m) build from source (docs/install.md §1)" ;;
    esac
    # `getconf` answers "glibc 2.39"; its absence, or another answer, is musl or
    # something older than the binary can load — and the loader's own message
    # for that is a wall of symbol versions.
    libc="$(getconf GNU_LIBC_VERSION 2>/dev/null || true)"
    case "$libc" in
    "glibc 2."*) ;;
    *) die "the prebuilt binary needs glibc 2.${MIN_GLIBC_MINOR} or newer, and this system does not report a glibc at all (musl?) — build from source (docs/install.md §1)" ;;
    esac
    minor="${libc#glibc 2.}"
    minor="${minor%%[!0-9]*}"
    [ "${minor:-0}" -ge "$MIN_GLIBC_MINOR" ] ||
        die "the prebuilt binary needs glibc 2.${MIN_GLIBC_MINOR} or newer; this system has ${libc} — build from source (docs/install.md §1)"
    have sha256sum || die "sha256sum is required (coreutils)"
}

sha256_of() { # file
    if have sha256sum; then
        sha256sum "$1"
    else
        shasum -a 256 "$1"
    fi | awk '{ print $1 }'
}

# ------------------------------------------------------------------ the network

# HTTPS only, and a failed status is a failure — not a page saved as a file.
fetch() { # url, destination
    if have curl; then
        curl --proto '=https' --tlsv1.2 -fsSL --retry 5 --retry-delay 2 -o "$2" "$1"
    elif have wget; then
        wget -q --https-only --tries=5 -O "$2" "$1"
    else
        die "curl or wget is required to download"
    fi
}

# The tag of the latest release, read off the **redirect** of /releases/latest.
# Not the API: that is 60 requests an hour per address, and a datacenter's
# address is shared with every other machine behind it.
latest_tag() {
    url="https://github.com/${REPO}/releases/latest"
    if have curl; then
        final="$(curl --proto '=https' --tlsv1.2 -fsSIL -o /dev/null -w '%{url_effective}' "$url")"
    elif have wget; then
        final="$(wget -q --https-only --max-redirect=5 --spider -S "$url" 2>&1 |
            awk 'tolower($1) == "location:" { u = $2 } END { print u }')"
    else
        die "curl or wget is required to download"
    fi
    final="$(printf '%s' "$final" | tr -d '\r')"
    case "$final" in
    */releases/tag/*) printf '%s' "${final##*/releases/tag/}" ;;
    *) die "could not read the latest release from ${url} (got: ${final:-nothing}); name one with --version" ;;
    esac
}

# A tag goes into a URL and a file name, so it is checked, not trusted.
check_tag() {
    case "$1" in
    v[0-9]*.[0-9]*.[0-9]*) ;;
    *) die "'$1' is not a release tag (expected vX.Y.Z)" ;;
    esac
    case "$1" in
    *[!A-Za-z0-9.+-]*) die "'$1' is not a release tag (expected vX.Y.Z)" ;;
    esac
}

# ------------------------------------------------------------------ the install

archive_name() { printf 'mindfork-rs-%s-%s.tar.gz' "$1" "$SUFFIX"; }

# The one archive a `--from` directory holds, as its tag.
tag_in_dir() {
    found=""
    for f in "$1"/mindfork-rs-v*-"$SUFFIX".tar.gz; do
        [ -f "$f" ] || continue
        [ -z "$found" ] || die "--from $1 holds more than one archive; name the release with --version"
        found="$f"
    done
    [ -n "$found" ] || die "--from $1 holds no mindfork-rs-v…-${SUFFIX}.tar.gz"
    found="${found##*/mindfork-rs-}"
    printf '%s' "${found%-"$SUFFIX".tar.gz}"
}

# Refuses unless `sha256sums.txt` names the archive and the digests agree. The
# release writes its lines as `<hash>  ./<name>`; `sha256sum` elsewhere writes
# `<hash>  <name>` or `<hash> *<name>` — all three are read.
#
# What this proves: the bytes are the ones the release published — not
# truncated, not corrupted. Both files come from the same place, so it is not
# proof of *who* published them; `gh attestation verify` is (docs/install.md §1).
verify() { # directory, archive name
    [ -f "$1/sha256sums.txt" ] || die "sha256sums.txt is missing from $1"
    want="$(awk -v f="$2" '{ n = $2; sub(/^\*/, "", n); sub(/^\.\//, "", n); if (n == f) print $1 }' "$1/sha256sums.txt")"
    [ -n "$want" ] || die "sha256sums.txt does not list $2 — refusing to install it"
    got="$(sha256_of "$1/$2")"
    [ "$want" = "$got" ] ||
        die "checksum mismatch for $2 (expected $want, got $got) — refusing to install it"
    say "  sha256 ok"
}

# Unpacked beside the target and moved in: `tar` writing over a binary that is
# running is "Text file busy", while a rename is not — so an upgrade works
# under a running app, which keeps the old file until it exits. The archive
# holds no user data (its `data/` is the bundled dictionaries only), so
# settings, chats and everything `mindfork setup` downloaded stay as they were.
#
# `--no-same-owner`: the archive records the CI runner's uid/gid (1001), and
# GNU tar run as root *restores* recorded owners by default — which in a
# container that runs as root but without CAP_CHOWN (a RunPod pod, measured
# 2026-09-22: 51 "Cannot change ownership to uid 1001" lines and a failed
# install) is "Operation not permitted". The files are ours to own; nothing
# about the archive's owner is worth keeping. For a non-root user this is
# already tar's default, so nothing changes there.
unpack() { # archive path, install dir
    stage="$2/.staging"
    rm -rf "$stage"
    mkdir -p "$stage"
    tar --no-same-owner -xzf "$1" -C "$stage"
    [ -f "$stage/mindfork" ] || die "the archive holds no 'mindfork' binary"
    chmod 0755 "$stage/mindfork"
    mv -f "$stage/mindfork" "$2/mindfork"
    cp -Rf "$stage/." "$2/"
    rm -rf "$stage"
}

# A Mac's `tar -xf FILE` gives every file it unpacks the archive's quarantine
# (measured, docs/research/macos.md §5.1), and an archive a browser saved has
# one — so a `--from` of it installs a binary macOS may refuse to start. `curl`
# sets none, so a download by this script never gets here. The mark is the
# user's to clear, not ours: it is what macOS asks a person about.
quarantine_note() { # install dir
    [ "$OS" = macos ] || return 0
    xattr -p com.apple.quarantine "$1/mindfork" >/dev/null 2>&1 || return 0
    warn "the archive came through a browser, and macOS quarantined what it unpacked. If macOS refuses to start mindfork, clear the mark:"
    warn "    xattr -dr com.apple.quarantine $1"
}

# ------------------------------------------------------------------ what a bare image lacks

# The binary is asked, not the package database: `--version` either runs or
# names the library it could not load. The only one the app needs beyond glibc
# is ALSA's (speech playback), and a minimal image — every container, most
# rented boxes — does not carry it.
starts() { "$1/mindfork" --version >/dev/null 2>&1; }

as_root() {
    if [ "$(id -u)" = 0 ]; then
        "$@"
    elif have sudo && sudo -n true 2>/dev/null; then
        sudo "$@"
    else
        return 1
    fi
}

install_alsa() {
    if have apt-get; then
        # `libasound2t64` first: after the time_t64 transition `libasound2` is
        # only a virtual name on Ubuntu 24.04+, and apt satisfies it with an OSS
        # shim that lacks symbols the app needs (packaging/nfpm.yaml).
        as_root apt-get update -qq &&
            { as_root apt-get install -y -qq --no-install-recommends libasound2t64 ||
                as_root apt-get install -y -qq --no-install-recommends libasound2; }
    elif have dnf; then
        as_root dnf install -y -q alsa-lib
    elif have pacman; then
        as_root pacman -Sy --noconfirm --needed alsa-lib
    elif have zypper; then
        as_root zypper --non-interactive install alsa
    else
        return 1
    fi
}

alsa_hint() {
    if have apt-get; then
        say "    sudo apt-get install -y libasound2t64    # or libasound2 on older releases"
    elif have dnf; then
        say "    sudo dnf install -y alsa-lib"
    elif have pacman; then
        say "    sudo pacman -S alsa-lib"
    else
        say "    install ALSA's runtime library (libasound.so.2) with your package manager"
    fi
}

ensure_starts() { # install dir, no_deps
    if starts "$1"; then
        return 0
    fi
    why="$("$1/mindfork" --version 2>&1 || true)"
    # A Mac has no library to install for it: what does not start is said.
    [ "$OS" = linux ] || die "the installed binary does not start: ${why:-no output}"
    case "$why" in
    *libasound*) ;;
    *) die "the installed binary does not start: ${why:-no output}" ;;
    esac
    if [ "$2" = 1 ]; then
        warn "the app cannot start yet: libasound.so.2 is missing, and --no-deps was given. Install it with:"
        alsa_hint >&2
        exit 3
    fi
    say "== libasound.so.2 is missing (a bare image does not carry it) — installing"
    if install_alsa >/dev/null 2>&1 && starts "$1"; then
        say "  installed"
        return 0
    fi
    warn "could not install it (not root, no sudo, or no network). The app is unpacked but cannot start until you run:"
    alsa_hint >&2
    exit 3
}

# llama.cpp's Linux builds are built with OpenMP, so its server needs
# `libgomp.so.1` — and a bare image lacks that too (measured 2026-10-02 on
# `ubuntu:24.04`: `error while loading shared libraries: libgomp.so.1`; RunPod's
# images carry it). The binary that needs it is not here yet: `setup --llama`
# downloads it after this script hands over. So the linker's cache is asked
# instead of a binary, and only when the hand-over is going to install llama.cpp.
wants_llama() { # the hand-over's arguments
    [ "${1:-}" = llama ] && [ "${2:-}" = setup ] && return 0
    for a in "$@"; do
        case "$a" in
        --llama | --llama=*) return 0 ;;
        esac
    done
    return 1
}

# `ldconfig` is in /sbin, which a non-root PATH does not name.
has_openmp() {
    for ldc in ldconfig /sbin/ldconfig /usr/sbin/ldconfig; do
        if command -v "$ldc" >/dev/null 2>&1; then
            "$ldc" -p 2>/dev/null | grep -q 'libgomp\.so\.1 '
            return
        fi
    done
    return 1
}

install_openmp() {
    if have apt-get; then
        as_root apt-get update -qq &&
            as_root apt-get install -y -qq --no-install-recommends libgomp1
    elif have dnf; then
        as_root dnf install -y -q libgomp
    elif have pacman; then
        as_root pacman -Sy --noconfirm --needed gcc-libs
    elif have zypper; then
        as_root zypper --non-interactive install libgomp1
    else
        return 1
    fi
}

openmp_hint() {
    if have apt-get; then
        say "    sudo apt-get install -y libgomp1"
    elif have dnf; then
        say "    sudo dnf install -y libgomp"
    elif have pacman; then
        say "    sudo pacman -S gcc-libs"
    else
        say "    install OpenMP's runtime library (libgomp.so.1) with your package manager"
    fi
}

# Unlike ALSA's, a missing OpenMP does not stop the hand-over: the app runs
# without it, and `llama setup` refuses the build it cannot start, saying the
# same thing.
ensure_openmp() { # no_deps, the hand-over's arguments...
    # llama.cpp's macOS build has no OpenMP (it uses Accelerate).
    [ "$OS" = linux ] || return 0
    no_deps_openmp="$1"
    shift
    wants_llama "$@" || return 0
    has_openmp && return 0
    if [ "$no_deps_openmp" = 1 ]; then
        warn "llama.cpp will not start: libgomp.so.1 is missing, and --no-deps was given. Install it with:"
        openmp_hint >&2
        return 0
    fi
    say "== libgomp.so.1 is missing (llama.cpp needs it, a bare image does not carry it) — installing"
    if install_openmp >/dev/null 2>&1 && has_openmp; then
        say "  installed"
        return 0
    fi
    warn "could not install it (not root, no sudo, or no network). llama.cpp will not start until you run:"
    openmp_hint >&2
}

# Into /usr/local/bin — and on a Mac, failing that, into Homebrew's
# /opt/homebrew/bin, which a Mac with Homebrew has on its PATH and lets its user
# write: Apple Silicon has no /usr/local/bin of its own. The app follows the link
# back to its directory, so its data stays beside the binary.
link_binary() { # install dir
    dirs="/usr/local/bin"
    [ "$OS" = macos ] && dirs="$dirs /opt/homebrew/bin"
    for d in $dirs; do
        [ -d "$d" ] || continue
        if ln -sf "$1/mindfork" "$d/mindfork" 2>/dev/null || as_root ln -sf "$1/mindfork" "$d/mindfork" 2>/dev/null; then
            say "  linked $d/mindfork"
            return 0
        fi
    done
    say "  not linked into $(printf '%s' "$dirs" | sed 's/ / or /') (no permission). Run it as:  $1/mindfork"
}

# ------------------------------------------------------------------ main

main() {
    dir="${HOME:-.}/mindfork"
    version=""
    from=""
    no_deps=0
    no_link=0
    while [ $# -gt 0 ]; do
        case "$1" in
        --dir) [ $# -ge 2 ] || die "--dir takes a directory"; dir="$2"; shift 2 ;;
        --dir=*) dir="${1#--dir=}"; shift ;;
        --version) [ $# -ge 2 ] || die "--version takes a tag"; version="$2"; shift 2 ;;
        --version=*) version="${1#--version=}"; shift ;;
        --from) [ $# -ge 2 ] || die "--from takes a directory"; from="$2"; shift 2 ;;
        --from=*) from="${1#--from=}"; shift ;;
        --no-deps) no_deps=1; shift ;;
        --no-link) no_link=1; shift ;;
        -h | --help) usage; exit 0 ;;
        --) shift; break ;;
        *) die "unknown option '$1' (arguments for mindfork go after '--'; see --help)" ;;
        esac
    done
    [ -n "$dir" ] || die "--dir takes a directory"

    check_platform
    if [ -n "$from" ]; then
        [ -d "$from" ] || die "--from $from is not a directory"
        [ -n "$version" ] || version="$(tag_in_dir "$from")"
    elif [ -z "$version" ]; then
        version="$(latest_tag)"
    fi
    check_tag "$version"
    archive="$(archive_name "$version")"

    mkdir -p "$dir"
    dir="$(cd "$dir" && pwd)"
    marker="$dir/.mindfork-version"
    say "== mindfork ${version} -> ${dir}"
    if [ -x "$dir/mindfork" ] && [ "$(cat "$marker" 2>/dev/null || true)" = "$version" ]; then
        say "  already installed"
    else
        if [ -n "$from" ]; then
            src="$(cd "$from" && pwd)"
        else
            src="$dir/.download"
            mkdir -p "$src"
            base="https://github.com/${REPO}/releases/download/${version}"
            say "  downloading ${archive}"
            fetch "${base}/${archive}" "$src/$archive"
            fetch "${base}/sha256sums.txt" "$src/sha256sums.txt"
        fi
        [ -f "$src/$archive" ] || die "$src holds no $archive"
        verify "$src" "$archive"
        unpack "$src/$archive" "$dir"
        quarantine_note "$dir"
        printf '%s\n' "$version" >"$marker"
        [ -n "$from" ] || rm -rf "$dir/.download"
        say "  unpacked"
    fi

    ensure_starts "$dir" "$no_deps"
    ensure_openmp "$no_deps" "$@"
    [ "$no_link" = 1 ] || link_binary "$dir"
    say "  $("$dir/mindfork" --version)"

    if [ $# -gt 0 ]; then
        say "== mindfork $*"
        exec "$dir/mindfork" "$@"
    fi
    say "Done. Next: mindfork setup --help   (docs/install.md §3.3), or just: mindfork"
}

main "$@"
