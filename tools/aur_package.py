#!/usr/bin/env python3
"""Render the AUR package `mindfork-rs-bin` for a release.

The AUR holds a PKGBUILD per package, rewritten for every version
(docs/research/package-managers.md). Its source of truth is
`packaging/aur/PKGBUILD.in` in this repository; what changes from release to
release is two values — the version, and the SHA-256 of the release's own Arch
package — and this tool fills them in:

    version   the tag without its `v`; a prerelease tag is refused, since the
              AUR is not where a rehearsal goes and `pkgver` takes no `-`
    sha256    read from the release's `sha256sums.txt`, which writes its lines
              as `<hash>  ./<name>` — the same file `install.sh` and Scoop
              trust, so the package claims nothing the release did not

It writes `PKGBUILD` and the packaging licence (`LICENSE`, 0BSD — the AUR asks
for one) into the output directory. `.SRCINFO` is makepkg's to write, on Arch
(`packaging/aur/check.sh`), not this tool's: a hand-made one could disagree
with the PKGBUILD in ways only makepkg would notice.

Usage:
    python tools/aur_package.py --version v0.14.1 --sums sha256sums.txt --out aur
    python tools/aur_package.py --self-test
"""

from __future__ import annotations

import argparse
import re
import shutil
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
TEMPLATE = REPO / "packaging" / "aur" / "PKGBUILD.in"
LICENSE = REPO / "packaging" / "aur" / "LICENSE"
PLACEHOLDERS = ("@VERSION@", "@SHA256@")

VERSION = re.compile(r"v?(\d+\.\d+\.\d+)")
SUMS_LINE = re.compile(r"([0-9a-fA-F]{64})\s+\*?(?:\./)?(\S+)")


class Refusal(Exception):
    """Something the package must not be rendered from."""


def version_of(tag: str) -> str:
    """`v0.14.1` or `0.14.1` → `0.14.1`; anything else is refused."""
    match = VERSION.fullmatch(tag.strip())
    if not match:
        raise Refusal(f"{tag!r} is not a release version (vX.Y.Z; no prerelease suffix)")
    return match.group(1)


def asset_name(version: str) -> str:
    """The release's Arch package — nfpm's name for it (packaging/nfpm.yaml)."""
    return f"mindfork-rs-{version}-1-x86_64.pkg.tar.zst"


def sha256_of(sums: str, name: str) -> str:
    """The digest `sums` gives `name` — exactly one line must name it."""
    found = [
        match.group(1).lower()
        for line in sums.splitlines()
        if (match := SUMS_LINE.fullmatch(line.strip())) and match.group(2) == name
    ]
    if len(found) != 1:
        raise Refusal(f"sha256sums.txt names {name} {len(found)} times, not once")
    return found[0]


def render(template: str, version: str, sha256: str) -> str:
    for placeholder in PLACEHOLDERS:
        if template.count(placeholder) != 1:
            raise Refusal(f"the template holds {placeholder} {template.count(placeholder)} times")
    return template.replace("@VERSION@", version).replace("@SHA256@", sha256)


def write(tag: str, sums_path: Path, out: Path) -> Path:
    version = version_of(tag)
    sha256 = sha256_of(sums_path.read_text(encoding="utf-8"), asset_name(version))
    out.mkdir(parents=True, exist_ok=True)
    pkgbuild = out / "PKGBUILD"
    pkgbuild.write_text(
        render(TEMPLATE.read_text(encoding="utf-8"), version, sha256), encoding="utf-8", newline="\n"
    )
    shutil.copyfile(LICENSE, out / "LICENSE")
    return pkgbuild


# ---------- self-test ----------

SUMS = """\
b7da088bbee0e0f516d7e137e0c222feee97eafbb1424213f63b10dd715e051a  ./install.sh
97dbead3b811031451796a8876f530e74ee79a8b5a7564c89d0c1edb412f95b7  ./mindfork-rs-0.14.1-1-x86_64.pkg.tar.zst
381572bdefac8c13cdc2341cef3b7ef8cece9807b80799ae64e65454a3bbe4e7  ./mindfork-rs-0.14.1-1.x86_64.rpm
"""
SHA = "97dbead3b811031451796a8876f530e74ee79a8b5a7564c89d0c1edb412f95b7"


def _refused(fn, *args) -> bool:
    try:
        fn(*args)
    except Refusal:
        return True
    return False


def self_test() -> int:
    failures: list[str] = []
    name = asset_name("0.14.1")

    for tag, want in (("v0.14.1", "0.14.1"), ("0.14.1", "0.14.1"), (" v1.2.3\n", "1.2.3")):
        if version_of(tag) != want:
            failures.append(f"version_of({tag!r}) is {version_of(tag)!r}")
    for tag in ("v0.15.0-rc1", "0.15", "latest", "v0.14.1.2", ""):
        if not _refused(version_of, tag):
            failures.append(f"version_of({tag!r}) was accepted")

    # The release's own shape, a bare name, and sha256sum's binary-mode star.
    for sums in (SUMS, SUMS.replace("./", ""), SUMS.replace("  ./", " *")):
        if sha256_of(sums, name) != SHA:
            failures.append(f"sha256_of missed the package in:\n{sums}")
    if sha256_of(SUMS.upper(), name.upper()) != SHA:
        failures.append("an upper-case digest is not normalised")
    # A name that only ends like the package, or begins like it, is not it.
    if not _refused(sha256_of, SUMS, "x86_64.pkg.tar.zst"):
        failures.append("a suffix of the name was accepted")
    if not _refused(sha256_of, SUMS, asset_name("0.14.2")):
        failures.append("another version's package was accepted")
    if not _refused(sha256_of, SUMS + SUMS.splitlines()[1] + "\n", name):
        failures.append("a package named twice was accepted")

    template = TEMPLATE.read_text(encoding="utf-8")
    rendered = render(template, "0.14.1", SHA)
    for want in ("pkgver=0.14.1\n", f"sha256sums_x86_64=('{SHA}')\n", "pkgname=mindfork-rs-bin\n"):
        if want not in rendered:
            failures.append(f"the rendered PKGBUILD lacks {want!r}")
    if not _refused(render, template.replace("@SHA256@", "x"), "0.14.1", SHA):
        failures.append("a template without its digest placeholder was rendered")
    if not _refused(render, template + "\n# @VERSION@", "0.14.1", SHA):
        failures.append("a template with a placeholder twice was rendered")
    # The source URL the template builds must be the asset sha256sums names.
    if "mindfork-rs-${pkgver}-1-${CARCH}.pkg.tar.zst" not in template:
        failures.append("the template's source is not the release's Arch package")

    with tempfile.TemporaryDirectory() as tmp:
        sums_path = Path(tmp) / "sha256sums.txt"
        sums_path.write_text(SUMS, encoding="utf-8")
        out = Path(tmp) / "aur"
        write("v0.14.1", sums_path, out)
        if sorted(p.name for p in out.iterdir()) != ["LICENSE", "PKGBUILD"]:
            failures.append(f"write left {sorted(p.name for p in out.iterdir())}")

    for failure in failures:
        print(f"FAIL: {failure}")
    print(f"aur_package --self-test: {len(failures)} failure(s)")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--version", help="the release tag, vX.Y.Z")
    parser.add_argument("--sums", type=Path, help="the release's sha256sums.txt")
    parser.add_argument("--out", type=Path, help="the directory to write PKGBUILD and LICENSE into")
    parser.add_argument("--self-test", action="store_true", help="run the tool's own checks")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    if not (args.version and args.sums and args.out):
        parser.error("--version, --sums and --out are required")
    try:
        pkgbuild = write(args.version, args.sums, args.out)
    except Refusal as refusal:
        print(f"refused: {refusal}", file=sys.stderr)
        return 1
    print(f"wrote {pkgbuild}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
