#!/usr/bin/env python3
"""Build the web demo: the page with everything it loads, and the release asset.

docs/research/web-demo.md §12-§13. Nothing here is deployed.

  tools/web_demo/build_i686.sh        # the 32-bit binary and its libraries
  python tools/web_demo/build.py      # the machine, its state, the page, the asset
  python -m http.server --directory target/web-demo/site --bind 127.0.0.1 8765

The steps: the pinned third-party pieces are fetched and checked (fetch.py);
the guest's initramfs is written with the binary in it (initramfs.py); the
guest is booted under Node and saved once the demo has drawn its first frame
(make_state.mjs); that state is restored in a fresh emulator, typed into and
resized (verify_state.mjs); the manifest of everything redistributed is read
out of the files themselves (manifest.py); and the page is put together in
``target/web-demo/site/`` - the same tree a release publishes as one asset,
``target/web-demo/mindfork-webdemo.tar``.

It needs ``node`` (20 or newer) and ``zstd`` on the PATH. It takes no path:
every step reads and writes the fixed places of paths.py.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
import tarfile

import initramfs
import manifest
from paths import ASSET, HERE, NOTICES, ROOT, SITE, STATE, VENDOR, WORK

STATE_NAME = "state.bin.zst"
PRIMER_NAME = "primer.bin"
MACHINE_NAME = "machine.json"
MANIFEST_NAME = "manifest.json"
PAGE_NAME = "index.html"
# What the page loads from ``vendor/``, and the site's own faces it draws with.
VENDOR_FILES = ["libv86.js", "v86.wasm", "seabios.bin", "vgabios.bin", "xterm.js", "xterm.css"]
FONTS = ["JetBrainsMono-Regular.woff2", "JetBrainsMono-Bold.woff2", "JetBrainsMono-Italic.woff2", "OFL.txt"]
# What a press of Start downloads: everything but the page itself and its faces.
ON_START = [*(f"vendor/{name}" for name in VENDOR_FILES), STATE_NAME, PRIMER_NAME, MACHINE_NAME]
# The part of it the emulator fetches itself, and reports the progress of.
BY_EMULATOR = ["vendor/v86.wasm", "vendor/seabios.bin", "vendor/vgabios.bin", STATE_NAME]


def run(command: list[str]) -> None:
    print("+", " ".join(command), flush=True)
    subprocess.run(command, check=True, cwd=ROOT)


def build_machine(verify: bool) -> None:
    """The guest, booted and saved; then restored and used, unless told not to."""
    for tool in ("node", "zstd"):
        if shutil.which(tool) is None:
            raise SystemExit(f"error: {tool} is not on the PATH")
    run([sys.executable, str(HERE / "fetch.py")])
    initramfs.write()
    run(["node", str(HERE / "make_state.mjs")])
    # `--ultra -22` is a megabyte smaller than `-19` on this state, and the
    # emulator's own decoder reads it.
    run(["zstd", "-q", "-f", "--ultra", "-22", str(STATE / "state.bin"), "-o", str(STATE / STATE_NAME)])
    if verify:
        run(["node", str(HERE / "verify_state.mjs")])


def assemble(texts: dict[str, bytes]) -> None:
    """Everything the page loads, laid out as the page asks for it."""
    if SITE.exists():
        shutil.rmtree(SITE)
    for directory in ("vendor", "fonts"):
        (SITE / directory).mkdir(parents=True)
    for name in VENDOR_FILES:
        shutil.copyfile(VENDOR / name, SITE / "vendor" / name)
    for name in FONTS:
        shutil.copyfile(ROOT / "site" / "static" / "fonts" / name, SITE / "fonts" / name)
    for name in ("demo.css", "demo.js"):
        shutil.copyfile(HERE / "page" / name, SITE / name)
    shutil.copyfile(HERE / MACHINE_NAME, SITE / MACHINE_NAME)
    for name in (STATE_NAME, PRIMER_NAME):
        shutil.copyfile(STATE / name, SITE / name)
    for name, data in texts.items():
        target = SITE / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)


def size_of(names: list[str]) -> int:
    return sum((SITE / name).stat().st_size for name in names)


def write_page(pieces: list[manifest.Piece]) -> None:
    """``page/index.html`` with this build's numbers and this build's pieces."""
    page = (HERE / "page" / PAGE_NAME).read_text(encoding="utf-8")
    machine = json.loads((HERE / MACHINE_NAME).read_text(encoding="utf-8"))
    on_start = size_of(ON_START)
    replacements = {
        "@VERSION@": manifest.cargo_field("version"),
        "@COLS@": str(machine["cols"]),
        "@ROWS@": str(machine["rows"]),
        "@DOWNLOAD_MB@": f"{on_start / 1e6:.0f}",
        "@DOWNLOAD_BYTES@": str(on_start),
        "@MACHINE_BYTES@": str(size_of(BY_EMULATOR)),
        "@PAGE_PIECES@": manifest.list_html(pieces, manifest.PAGE),
        "@MACHINE_PIECES@": manifest.list_html(pieces, manifest.MACHINE),
    }
    for mark, value in replacements.items():
        if mark not in page:
            raise SystemExit(f"error: page/{PAGE_NAME} has no {mark}")
        page = page.replace(mark, value)
    (SITE / PAGE_NAME).write_text(page, encoding="utf-8")


def write_manifest(pieces: list[manifest.Piece]) -> dict:
    """The manifest, beside the files it describes - every one but itself."""
    files = {}
    for path in sorted(SITE.rglob("*")):
        if path.is_file():
            data = path.read_bytes()
            files[path.relative_to(SITE).as_posix()] = {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
    document = manifest.document(pieces, files, ON_START, BY_EMULATOR)
    (SITE / MANIFEST_NAME).write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    return document


def pack() -> None:
    """The page's tree as one archive: the asset a release carries. Members in
    name order, owned by nobody in particular, dated by the commit."""
    stamp = subprocess.run(["git", "log", "-1", "--format=%ct"], capture_output=True, text=True, check=True, cwd=ROOT)

    def plain(member: tarfile.TarInfo) -> tarfile.TarInfo:
        member.uid = member.gid = 0
        member.uname = member.gname = ""
        member.mtime = int(stamp.stdout.strip())
        return member

    with tarfile.open(ASSET, "w") as tar:
        for path in sorted(SITE.rglob("*")):
            if path.is_file():
                tar.add(path, arcname=path.relative_to(SITE).as_posix(), filter=plain)


def report(document: dict) -> None:
    files = document["files"]
    print(f"\nA press of Start downloads {document['on_start_bytes'] / 1e6:.1f} MB, uncompressed in transit:")
    for name in sorted(ON_START, key=lambda name: -files[name]["bytes"]):
        print(f"  {files[name]['bytes']:>10}  {name}")
    print(f"\n{len(document['pieces'])} redistributed pieces in {MANIFEST_NAME}:")
    for piece in document["pieces"]:
        print(f"  {piece['name']} {piece['version']}".rstrip() + f" - {piece['licence']}")
    print(f"\nThe asset: {ASSET.relative_to(ROOT)}, {ASSET.stat().st_size / 1e6:.1f} MB")
    print(f"The page:  {SITE.relative_to(ROOT)}; serve it with")
    print(f"  python -m http.server --directory {SITE.relative_to(ROOT)} --bind 127.0.0.1 8765")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--page-only", action="store_true", help="put the page together again from the state already built")
    parser.add_argument("--skip-verify", action="store_true", help="do not restore the state and type into it")
    parser.add_argument("--release", action="store_true",
                        help="what a release publishes: refuse to build without the application's third-party notices")
    args = parser.parse_args()

    if args.release and not NOTICES.exists():
        raise SystemExit(f"error: no {NOTICES.name} - `cargo about generate about.hbs -o {NOTICES.name}` writes it")
    if not args.page_only:
        build_machine(verify=not args.skip_verify)
    elif not (STATE / STATE_NAME).exists() or not (STATE / PRIMER_NAME).exists():
        raise SystemExit("error: --page-only needs a state built by an earlier run")

    WORK.mkdir(parents=True, exist_ok=True)
    pieces = manifest.collect()
    assemble(manifest.texts(pieces))
    write_page(pieces)
    document = write_manifest(pieces)
    pack()
    report(document)
    return 0


if __name__ == "__main__":
    sys.exit(main())
