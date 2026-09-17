#!/usr/bin/env bash
# Install the pinned nfpm and verify it, for the two workflows that build the
# Linux packages: release.yml (the real artifacts) and packaging.yml (the gate
# that validates them on a pull request). One script for the same reason
# tools/install_inno.ps1 is one script — the gate and the release must not drift
# onto different builders (docs/research/inno-setup-7.md §4).
#
# What it replaces, and why (docs/research/release-pipeline.md §2.4): both
# workflows used to add
#
#     deb [trusted=yes] https://repo.goreleaser.com/apt/ /
#
# and `apt-get install nfpm`. `[trusted=yes]` tells apt to accept that repository
# **without checking its signature**, and no version is named — so the tool that
# assembles the .deb/.rpm/.pkg.tar.zst a stranger installs was whatever that host
# served that day, fetched over a connection nothing authenticated. Upstream
# publishes a release tarball on GitHub whose digest is in the release metadata,
# which is all this needs.
#
# Bumping nfpm is a deliberate edit of the three lines below: the version, and
# the hash, which is the asset's digest as the GitHub API reports it
# (`gh api repos/goreleaser/nfpm/releases/latest --jq '.assets[] | .name + " " + .digest'`).
#
# Idempotent: an already-installed pinned version is left alone, so the script is
# usable on a developer machine to get exactly the builder the releases use.
#
# Usage: tools/install_nfpm.sh [install-dir]   (default /usr/local/bin)
set -euo pipefail

VERSION="2.47.0"
SHA256="0660ca602b2d2d2ae4781a06c692b3eeb9d437ffea05b831d76e41f4a3188783"
URL="https://github.com/goreleaser/nfpm/releases/download/v${VERSION}/nfpm_${VERSION}_Linux_x86_64.tar.gz"

DEST="${1:-/usr/local/bin}"

# `nfpm --version` prints an ASCII-art banner and then a block of fields; the one
# that carries the version is `GitVersion:    2.47.0` (measured, not assumed —
# the first spelling of this line looked for a `version:` that nfpm never prints,
# and the script then rejected the copy it had just installed).
installed_version() {
    command -v nfpm >/dev/null 2>&1 || return 0
    nfpm --version 2>/dev/null | sed -n 's/^GitVersion: *v\{0,1\}\([0-9][^ ]*\).*/\1/p' | head -1
}

if [ "$(installed_version)" = "$VERSION" ]; then
    echo "nfpm $VERSION is already installed."
    exit 0
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading nfpm $VERSION..."
# Bounded like every other network step in these workflows (docs/lessons.md §10):
# a dead connection fails and retries inside the step's own budget.
curl -fsSL --retry 3 --connect-timeout 30 --max-time 300 -o "$tmp/nfpm.tar.gz" "$URL"

actual="$(sha256sum "$tmp/nfpm.tar.gz" | cut -d' ' -f1)"
if [ "$actual" != "$SHA256" ]; then
    echo "nfpm $VERSION SHA-256 mismatch: expected $SHA256, got $actual" >&2
    exit 1
fi
echo "SHA-256 verified. Installing into $DEST..."

tar -xzf "$tmp/nfpm.tar.gz" -C "$tmp" nfpm
# `sudo` where it is needed and not where it is not: the release and packaging
# jobs run as a user with passwordless sudo, a container smoke runs as root.
if [ -w "$DEST" ]; then
    install -m 755 "$tmp/nfpm" "$DEST/nfpm"
else
    sudo install -m 755 "$tmp/nfpm" "$DEST/nfpm"
fi

# The version is asserted after the install for the reason install_inno.ps1
# states: a silent fallback to another copy already on PATH would keep building
# packages with a tool the log claims was replaced.
found="$(installed_version)"
if [ "$found" != "$VERSION" ]; then
    echo "expected nfpm $VERSION on PATH, found '${found:-nothing}'" >&2
    exit 1
fi
echo "nfpm $found at $(command -v nfpm)"
