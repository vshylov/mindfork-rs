#!/usr/bin/env bash
# Builds Linux packages (deb + rpm + archlinux) from a ready binary via nfpm.
# nfpm must be on PATH. Used both in CI (packaging.yml / release.yml) and locally.
#
# Usage: packaging/linux/build-packages.sh <path-to-binary> <version> <output-dir>
# Example: packaging/linux/build-packages.sh target/release/mindfork-rs 0.9.0 dist
set -euo pipefail

BIN="${1:?путь к бинарнику}"
export VERSION="${2:?версия (напр. 0.9.0)}"
OUT="${3:?каталог вывода}"

# The repo root (the script lives in packaging/linux/).
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# The binary is staged at a fixed path, which packaging/nfpm.yaml refers to.
mkdir -p dist/stage "$OUT"
cp "$BIN" dist/stage/mindfork-rs
chmod +x dist/stage/mindfork-rs

# Artifact names — following format conventions (§8 docs/history/installers.md).
deb="mindfork-rs_${VERSION}-1_amd64.deb"
rpm="mindfork-rs-${VERSION}-1.x86_64.rpm"
arch="mindfork-rs-${VERSION}-1-x86_64.pkg.tar.zst"

nfpm package --config packaging/nfpm.yaml --packager deb       --target "$OUT/$deb"
nfpm package --config packaging/nfpm.yaml --packager rpm       --target "$OUT/$rpm"
nfpm package --config packaging/nfpm.yaml --packager archlinux --target "$OUT/$arch"

echo "Собрано в $OUT:"
ls -l "$OUT"/*.deb "$OUT"/*.rpm "$OUT"/*.pkg.tar.zst
