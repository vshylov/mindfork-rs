#!/usr/bin/env python3
"""Fetch the third-party pieces of the web demo probe, each pinned by a hash.

docs/research/web-demo.md, stage 1. Nothing here is committed: the files land
in ``target/web-demo/vendor/`` and are checked every time, so a piece that
changed upstream stops the build instead of entering it.

  python tools/web_demo/fetch.py            # download what is missing, verify all
  python tools/web_demo/fetch.py --check    # verify only; download nothing

A saved machine state restores only on the exact emulator build that made it,
so the emulator is taken from its versioned npm package and not from the
repository's rolling ``latest`` release.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import io
import sys
import tarfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
VENDOR = ROOT / "target" / "web-demo" / "vendor"

# The commit of copy/v86 the BIOS images are read from.
V86_COMMIT = "315b2b0b758703ef0e6cc7b4088d6068f83f3c20"

# An npm package: its registry tarball, the registry's own integrity value
# (sha512, base64), and the members taken out of it.
PACKAGES = [
    {
        "name": "v86 0.5.473",
        "url": "https://registry.npmjs.org/v86/-/v86-0.5.473.tgz",
        "sha512": "mNHB0DXDmfxn0CrlqCmlgMR1W7aKuD+sHmSctLcTROOwpOvbE4qsAhmaE6Vy+riQfcE4PfA2xvW59/ZXsYnHNw==",
        "members": {
            "package/build/libv86.js": "libv86.js",
            "package/build/libv86.mjs": "libv86.mjs",
            "package/build/v86.wasm": "v86.wasm",
            "package/LICENSE": "LICENSE.v86",
        },
    },
    {
        "name": "@xterm/xterm 6.0.0",
        "url": "https://registry.npmjs.org/@xterm/xterm/-/xterm-6.0.0.tgz",
        "sha512": "TQwDdQGtwwDt+2cgKDLn0IRaSxYu1tSUjgKarSDkUM0ZNiSRXFpjxEsvc/Zgc5kq5omJ+V0a8/kIM2WD3sMOYg==",
        "members": {
            "package/lib/xterm.js": "xterm.js",
            "package/css/xterm.css": "xterm.css",
            "package/LICENSE": "LICENSE.xterm",
        },
    },
]

# A single file: where it comes from, its sha256, the name it gets.
FILES = [
    {
        "name": "seabios.bin",
        "url": f"https://raw.githubusercontent.com/copy/v86/{V86_COMMIT}/bios/seabios.bin",
        "sha256": "73e3f359102e3a9982c35fce98eb7cd08f18303ac7f1ba6ebfbe6cdc1c244d98",
    },
    {
        "name": "vgabios.bin",
        "url": f"https://raw.githubusercontent.com/copy/v86/{V86_COMMIT}/bios/vgabios.bin",
        "sha256": "a4bc0d80cc3ca028c73dafa8fee396b8d054ce87ebd8abfbd31b06b437607880",
    },
    {
        # The probe's guest: the Buildroot image v86's own site boots (Linux
        # 6.8.12, BusyBox, uClibc). Stage 2 replaces it with one assembled from
        # named distribution packages (docs/research/web-demo.md, F2).
        "name": "buildroot-bzimage68.bin",
        "url": "https://i.copy.sh/buildroot-bzimage68.bin",
        "sha256": "507a759c70ab7a490a233be454d0b5b88bc667956a410b531cb4edc091e2eb1c",
    },
]


def download(url: str) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": "web-demo-fetch"})
    with urllib.request.urlopen(request, timeout=120) as response:
        return response.read()


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha512_b64(data: bytes) -> str:
    return base64.b64encode(hashlib.sha512(data).digest()).decode()


def fetch_package(package: dict, check_only: bool) -> list[str]:
    """Returns the problems found; an empty list means the package is in place."""
    archive = VENDOR / "archives" / package["url"].rsplit("/", 1)[1]
    if not archive.exists():
        if check_only:
            return [f"{package['name']}: {archive.name} is not downloaded"]
        archive.parent.mkdir(parents=True, exist_ok=True)
        archive.write_bytes(download(package["url"]))
    data = archive.read_bytes()
    if sha512_b64(data) != package["sha512"]:
        return [f"{package['name']}: {archive.name} does not match its pinned sha512"]
    # Only the named members leave the archive, each under a name chosen here:
    # nothing in the tarball decides where a file is written.
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tar:
        for member, target in package["members"].items():
            source = tar.extractfile(member)
            if source is None:
                return [f"{package['name']}: no {member} in {archive.name}"]
            (VENDOR / target).write_bytes(source.read())
    return []


def fetch_file(item: dict, check_only: bool) -> list[str]:
    path = VENDOR / item["name"]
    if not path.exists():
        if check_only:
            return [f"{item['name']} is not downloaded"]
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(download(item["url"]))
    if sha256(path.read_bytes()) != item["sha256"]:
        return [f"{item['name']} does not match its pinned sha256"]
    return []


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true", help="verify only; download nothing")
    args = parser.parse_args()

    VENDOR.mkdir(parents=True, exist_ok=True)
    problems: list[str] = []
    for package in PACKAGES:
        problems += fetch_package(package, args.check)
    for item in FILES:
        problems += fetch_file(item, args.check)
    if problems:
        for problem in problems:
            print(f"error: {problem}", file=sys.stderr)
        return 1
    print(f"clean: {len(PACKAGES)} packages and {len(FILES)} files in {VENDOR.relative_to(ROOT)}, each matching its pin")
    return 0


if __name__ == "__main__":
    sys.exit(main())
