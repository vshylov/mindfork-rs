#!/usr/bin/env bash
# Builds Linux packages (deb + rpm + archlinux) from a ready binary via nfpm.
# nfpm must be on PATH. Used both in CI (packaging.yml / release.yml) and locally.
#
# Usage: packaging/linux/build-packages.sh <path-to-binary> <version> <output-dir>
# Example: packaging/linux/build-packages.sh target/release/mindfork 0.9.0 dist
set -euo pipefail

BIN="${1:?path to the binary}"
export VERSION="${2:?version (e.g. 0.9.0)}"
OUT="${3:?output directory}"

# The repo root (the script lives in packaging/linux/).
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# The packages carry the licence texts of the locked dependency graph, and that
# file is generated per build rather than committed (about.toml) — so it is a
# precondition here rather than something quietly skipped. Both workflows that
# call this script produce it first; a local run needs the one command below.
if [ ! -s THIRD-PARTY-NOTICES.md ]; then
    echo "THIRD-PARTY-NOTICES.md is missing. Generate it for this exact graph:" >&2
    echo "    cargo install cargo-about --locked --features cli" >&2
    echo "    cargo about generate about.hbs -o THIRD-PARTY-NOTICES.md" >&2
    exit 1
fi

# The binary is staged at a fixed path, which packaging/nfpm.yaml refers to.
mkdir -p dist/stage "$OUT"
cp "$BIN" dist/stage/mindfork
chmod +x dist/stage/mindfork

# Artifact names — following format conventions (§8 docs/history/installers.md).
deb="mindfork-rs_${VERSION}-1_amd64.deb"
rpm="mindfork-rs-${VERSION}-1.x86_64.rpm"
arch="mindfork-rs-${VERSION}-1-x86_64.pkg.tar.zst"

nfpm package --config packaging/nfpm.yaml --packager deb       --target "$OUT/$deb"
nfpm package --config packaging/nfpm.yaml --packager rpm       --target "$OUT/$rpm"
nfpm package --config packaging/nfpm.yaml --packager archlinux --target "$OUT/$arch"

echo "Built in $OUT:"
ls -l "$OUT"/*.deb "$OUT"/*.rpm "$OUT"/*.pkg.tar.zst
