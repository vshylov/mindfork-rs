#!/usr/bin/env bash
# Собирает Linux-пакеты (deb + rpm + archlinux) из готового бинарника через nfpm.
# nfpm должен быть в PATH. Используется и в CI (packaging.yml / release.yml), и локально.
#
# Использование: packaging/linux/build-packages.sh <путь-к-бинарнику> <версия> <каталог-вывода>
# Пример:        packaging/linux/build-packages.sh target/release/mindfork-rs 0.9.0 dist
set -euo pipefail

BIN="${1:?путь к бинарнику}"
export VERSION="${2:?версия (напр. 0.9.0)}"
OUT="${3:?каталог вывода}"

# Корень репозитория (скрипт лежит в packaging/linux/).
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# Бинарь стейджится в фиксированный путь, на который ссылается packaging/nfpm.yaml.
mkdir -p dist/stage "$OUT"
cp "$BIN" dist/stage/mindfork-rs
chmod +x dist/stage/mindfork-rs

# Имена артефактов — по конвенциям форматов (§8 docs/history/installers.md).
deb="mindfork-rs_${VERSION}-1_amd64.deb"
rpm="mindfork-rs-${VERSION}-1.x86_64.rpm"
arch="mindfork-rs-${VERSION}-1-x86_64.pkg.tar.zst"

nfpm package --config packaging/nfpm.yaml --packager deb       --target "$OUT/$deb"
nfpm package --config packaging/nfpm.yaml --packager rpm       --target "$OUT/$rpm"
nfpm package --config packaging/nfpm.yaml --packager archlinux --target "$OUT/$arch"

echo "Собрано в $OUT:"
ls -l "$OUT"/*.deb "$OUT"/*.rpm "$OUT"/*.pkg.tar.zst
