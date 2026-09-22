#!/bin/sh
# mindfork — the install script for Linux x86_64.
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
# Options (all optional):
#   --dir DIR        where to install                      (default: $HOME/mindfork)
#   --version vX.Y.Z which release                         (default: the latest)
#   --from DIR       install from files already on disk — the archive and
#                    sha256sums.txt — instead of downloading them
#   --no-deps        do not install the one system library the app needs
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
mindfork install script (Linux x86_64)

  install.sh [OPTIONS] [-- MINDFORK ARGS…]

  --dir DIR         where to install                 (default: $HOME/mindfork)
  --version vX.Y.Z  which release                    (default: the latest)
  --from DIR        install from files already on disk (the archive and
                    sha256sums.txt) instead of downloading them
  --no-deps         do not install the one system library the app needs
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

check_platform() {
    [ "$(uname -s)" = "Linux" ] ||
        die "this script installs the Linux build; on $(uname -s) see docs/install.md"
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
    have tar || die "tar is required"
    have sha256sum || die "sha256sum is required (coreutils)"
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

archive_name() { printf 'mindfork-rs-%s-x86_64-linux.tar.gz' "$1"; }

# The one archive a `--from` directory holds, as its tag.
tag_in_dir() {
    found=""
    for f in "$1"/mindfork-rs-v*-x86_64-linux.tar.gz; do
        [ -f "$f" ] || continue
        [ -z "$found" ] || die "--from $1 holds more than one archive; name the release with --version"
        found="$f"
    done
    [ -n "$found" ] || die "--from $1 holds no mindfork-rs-v…-x86_64-linux.tar.gz"
    found="${found##*/mindfork-rs-}"
    printf '%s' "${found%-x86_64-linux.tar.gz}"
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
    got="$(sha256sum "$1/$2" | awk '{ print $1 }')"
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

link_binary() { # install dir
    bin="/usr/local/bin/mindfork"
    if [ -d /usr/local/bin ] && as_root ln -sf "$1/mindfork" "$bin" 2>/dev/null; then
        say "  linked $bin"
    else
        say "  not linked into /usr/local/bin (no permission). Run it as:  $1/mindfork"
    fi
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
        printf '%s\n' "$version" >"$marker"
        [ -n "$from" ] || rm -rf "$dir/.download"
        say "  unpacked"
    fi

    ensure_starts "$dir" "$no_deps"
    [ "$no_link" = 1 ] || link_binary "$dir"
    say "  $("$dir/mindfork" --version)"

    if [ $# -gt 0 ]; then
        say "== mindfork $*"
        exec "$dir/mindfork" "$@"
    fi
    say "Done. Next: mindfork setup --help   (docs/install.md §3.3), or just: mindfork"
}

main "$@"
