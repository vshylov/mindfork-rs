#!/usr/bin/env python3
"""Fetch the third-party pieces of the web demo, each pinned by a hash.

docs/research/web-demo.md §12-§13. Nothing here is committed: the files land
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
import struct
import sys
import tarfile
import urllib.request

from paths import ROOT, VENDOR

# The commit of copy/v86 the BIOS images are read from.
V86_COMMIT = "315b2b0b758703ef0e6cc7b4088d6068f83f3c20"
ALPINE = "https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/x86"
# The musl release Alpine's image carries; its COPYRIGHT is fetched below.
MUSL = "1.2.6"

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
            # What the package says of itself: the manifest's version and licence.
            "package/package.json": "v86.package.json",
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
            "package/package.json": "xterm.package.json",
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
    # The guest (docs/research/web-demo.md §13.1, F6): Alpine's release images.
    # A branch keeps only the newest build of a package, so a package pinned by
    # hash stops resolving within weeks; a release's images are kept for good,
    # and each has its sha256 published beside it.
    {
        "name": "alpine-virt-3.24.4-x86.iso",
        "url": f"{ALPINE}/alpine-virt-3.24.4-x86.iso",
        "sha256": "cccf118b601128c81007b98260b9778be077c586c1fd12e04023cb975cc7f4a1",
        # The kernel: Linux 6.18.60, Alpine's `linux-virt`.
        "iso_members": {("BOOT", "VMLINUZ_VIRT.;1"): "vmlinuz-virt"},
    },
    {
        "name": "alpine-minirootfs-3.24.4-x86.tar.gz",
        "url": f"{ALPINE}/alpine-minirootfs-3.24.4-x86.tar.gz",
        "sha256": "df793865bf166c10acaad3a4d5d40498f9ee3f89fa405d39c7f540464fb3b0c1",
        # The shell /init runs in, the C library it is linked against, and the
        # image's own record of its packages - each one's version, licence and
        # the commit of Alpine's build files it was made from (manifest.py).
        "tar_members": {
            "./bin/busybox": "busybox",
            "./lib/ld-musl-i386.so.1": "ld-musl-i386.so.1",
            "./lib/apk/db/installed": "alpine-installed",
            "./etc/alpine-release": "alpine-release",
        },
    },
    # musl's copyright notice, which its licence requires to travel with the
    # library; Alpine's image carries the library without it. The version is the
    # one the image's record names - manifest.py refuses a mismatch.
    {
        "name": f"musl-{MUSL}.tar.gz",
        "url": f"https://musl.libc.org/releases/musl-{MUSL}.tar.gz",
        "sha256": "d585fd3b613c66151fc3249e8ed44f77020cb5e6c1e635a616d3f9f82460512a",
        "tar_members": {f"musl-{MUSL}/COPYRIGHT": "COPYRIGHT.musl"},
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


def iso_file(image: bytes, directory: str, name: str) -> bytes | None:
    """One file out of an ISO 9660 image, by the names of its primary volume
    descriptor: `name` in the root's `directory`. Enough of the format for
    that and no more - the image's hash is what vouches for the bytes."""
    sector = 2048

    def records(extent: int, size: int):
        data = image[extent * sector : extent * sector + size]
        at = 0
        while at < len(data):
            length = data[at]
            if length == 0:  # the rest of this sector is padding
                at = (at // sector + 1) * sector
                continue
            record = data[at : at + length]
            where, how_long = struct.unpack_from("<I", record, 2)[0], struct.unpack_from("<I", record, 10)[0]
            yield record[33 : 33 + record[32]].decode("ascii", "replace"), where, how_long
            at += length

    primary = image[16 * sector : 17 * sector]
    if primary[0] != 1 or primary[1:6] != b"CD001":
        return None
    root = primary[156:190]
    listing = records(struct.unpack_from("<I", root, 2)[0], struct.unpack_from("<I", root, 10)[0])
    inside = next(((where, size) for entry, where, size in listing if entry == directory), None)
    if inside is None:
        return None
    found = next(((where, size) for entry, where, size in records(*inside) if entry == name), None)
    if found is None:
        return None
    return image[found[0] * sector : found[0] * sector + found[1]]


def fetch_file(item: dict, check_only: bool) -> list[str]:
    path = VENDOR / item["name"]
    if not path.exists():
        if check_only:
            return [f"{item['name']} is not downloaded"]
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(download(item["url"]))
    data = path.read_bytes()
    if sha256(data) != item["sha256"]:
        return [f"{item['name']} does not match its pinned sha256"]
    # What is taken out of an image is named here, as with the packages.
    for (directory, name), target in item.get("iso_members", {}).items():
        member = iso_file(data, directory, name)
        if member is None:
            return [f"{item['name']}: no {directory}/{name} in the image"]
        (VENDOR / target).write_bytes(member)
    if "tar_members" in item:
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tar:
            for member, target in item["tar_members"].items():
                source = tar.extractfile(member)
                if source is None:
                    return [f"{item['name']}: no {member} in the archive"]
                (VENDOR / target).write_bytes(source.read())
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
