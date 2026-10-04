#!/usr/bin/env bash
# Builds a rendered AUR package the way an AUR helper would, installs it, runs
# it, lints it, and writes its .SRCINFO beside the PKGBUILD — so what is pushed
# to the AUR has been built and run first (docs/research/package-managers.md).
#
# Runs as root inside archlinux:base-devel; makepkg itself refuses root, so the
# build is a throwaway user's, in a copy of DIR (a bind mount from another
# system may not take a chown):
#
#   docker run --rm -v "$PWD:/w" -w /w archlinux:base-devel \
#     bash packaging/aur/check.sh aur 0.14.1
set -euo pipefail

dir="$1"
version="$2"
pkgname=mindfork-rs-bin

pacman -Syu --noconfirm --needed namcap >/dev/null
useradd -m builder
work=/home/builder/pkg
cp -r "$dir" "$work"
chown -R builder "$work"

# The PKGBUILD's own dependencies, installed as root — makepkg -s would need
# sudo for the builder.
deps="$(su builder -c "cd '$work' && source PKGBUILD && echo \"\${depends[*]}\"")"
# shellcheck disable=SC2086 # the list is meant to split into packages
pacman -S --noconfirm --needed $deps >/dev/null

su builder -c "cd '$work' && makepkg --cleanbuild --noconfirm && makepkg --printsrcinfo > .SRCINFO"
pkg="$work/$pkgname-$version-1-x86_64.pkg.tar.zst"
test -f "$pkg"

# The linter's findings are printed for the log; an error among them fails.
lint="$(namcap "$work/PKGBUILD" "$pkg")"
printf '%s\n' "$lint"
if printf '%s\n' "$lint" | grep -q ' E: '; then
  echo "namcap reports an error"
  exit 1
fi

pacman -U --noconfirm "$pkg" >/dev/null
# The layout the release's own package has, under this package's name.
test "$(readlink -f /usr/bin/mindfork)" = /usr/lib/mindfork-rs/mindfork
grep -q '"mode": *"system"' /usr/lib/mindfork-rs/defaults.json
ls /usr/lib/mindfork-rs/data/dictionaries/*.dic >/dev/null
test -f "/usr/share/licenses/$pkgname/LICENSE"
# It runs, and it is the release it claims to be — as a user, not root.
got="$(su builder -c 'mindfork --version')"
echo "installed: $got"
test "$got" = "mindfork $version"

cp "$work/.SRCINFO" "$dir/.SRCINFO"
echo "AUR CHECK OK: $pkgname $version"
