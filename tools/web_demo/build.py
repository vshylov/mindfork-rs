#!/usr/bin/env python3
"""Build the web demo probe's page into ``target/web-demo/site/``.

docs/research/web-demo.md, stage 1: a local page for the owner's go/no-go.
Nothing here is deployed and nothing is part of a release.

  python tools/web_demo/build.py --binary <32-bit mindfork> --libs <dir> [--libs <dir>...]
  python -m http.server --directory target/web-demo/site 8765

The steps: the pinned third-party pieces are fetched and checked (fetch.py);
the guest is booted under Node with the binary in it and saved once the demo
has drawn its first frame (make_state.mjs); that state is restored in a fresh
emulator and typed into (verify_state.mjs); and the page is put together from
``page/``, the state, the emulator, the terminal and the site's own typeface.

It needs ``node`` (20 or newer; ``--node`` or $NODE names another) and the
``zstd`` command.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
WORK = ROOT / "target" / "web-demo"
VENDOR = WORK / "vendor"
STATE = WORK / "state"
SITE = WORK / "site"

# What the page loads from ``vendor/``.
VENDOR_FILES = ["libv86.js", "v86.wasm", "seabios.bin", "vgabios.bin", "xterm.js", "xterm.css", "LICENSE.v86", "LICENSE.xterm"]
FONTS = ["JetBrainsMono-Regular.woff2", "JetBrainsMono-Bold.woff2", "JetBrainsMono-Italic.woff2", "OFL.txt"]
# What a press of Start downloads: everything but the page itself and its faces.
ON_START = ["vendor/libv86.js", "vendor/v86.wasm", "vendor/seabios.bin", "vendor/vgabios.bin", "vendor/xterm.js",
            "vendor/xterm.css", "state.bin.zst", "primer.bin", "machine.json"]
# The part of it the emulator fetches itself, and reports the progress of.
BY_EMULATOR = ["vendor/v86.wasm", "vendor/seabios.bin", "vendor/vgabios.bin", "state.bin.zst"]


def run(command: list[str]) -> None:
    print("+", " ".join(str(part) for part in command), flush=True)
    subprocess.run(command, check=True, cwd=ROOT)


def app_version() -> str:
    match = re.search(r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(encoding="utf-8"), re.MULTILINE)
    if not match:
        raise SystemExit("error: no version in Cargo.toml")
    return match.group(1)


def pinned_versions() -> dict[str, str]:
    """The emulator's and the terminal's versions, as fetch.py pins them."""
    text = (HERE / "fetch.py").read_text(encoding="utf-8")
    v86 = re.search(r'"name": "v86 ([^"]+)"', text)
    xterm = re.search(r'"name": "@xterm/xterm ([^"]+)"', text)
    if not v86 or not xterm:
        raise SystemExit("error: fetch.py names no v86 or xterm version")
    return {"@V86_VERSION@": v86.group(1), "@XTERM_VERSION@": xterm.group(1)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--binary", type=Path, help="the 32-bit x86 release binary")
    parser.add_argument("--libs", action="append", type=Path,
                        help="a directory holding the binary's 32-bit shared libraries; repeatable")
    parser.add_argument("--page-only", action="store_true",
                        help="put the page together again from the state already built")
    parser.add_argument("--node", default=os.environ.get("NODE", "node"), help="the Node binary (default: node)")
    parser.add_argument("--skip-verify", action="store_true", help="do not restore the state and type into it")
    args = parser.parse_args()

    state = STATE / "state.bin.zst"
    if args.page_only:
        if not state.exists() or not (STATE / "primer.bin").exists():
            raise SystemExit("error: --page-only needs a state built by an earlier run")
    else:
        if args.binary is None or not args.libs:
            raise SystemExit("error: --binary and --libs are required (or --page-only)")
        for tool in (args.node, "zstd"):
            if shutil.which(tool) is None:
                raise SystemExit(f"error: {tool} is not on the PATH")
        run([sys.executable, str(HERE / "fetch.py")])
        run([args.node, str(HERE / "make_state.mjs"), str(STATE), str(args.binary), *[str(d) for d in args.libs]])
        # `--ultra -22` is a megabyte smaller than `-19` on this state, and the
        # emulator's own decoder reads it.
        run(["zstd", "-q", "-f", "--ultra", "-22", str(STATE / "state.bin"), "-o", str(state)])
        if not args.skip_verify:
            run([args.node, str(HERE / "verify_state.mjs"), str(state)])

    if SITE.exists():
        shutil.rmtree(SITE)
    (SITE / "vendor").mkdir(parents=True)
    (SITE / "fonts").mkdir()
    for name in VENDOR_FILES:
        shutil.copyfile(VENDOR / name, SITE / "vendor" / name)
    for name in FONTS:
        shutil.copyfile(ROOT / "site" / "static" / "fonts" / name, SITE / "fonts" / name)
    for name in ("demo.css", "demo.js"):
        shutil.copyfile(HERE / "page" / name, SITE / name)
    shutil.copyfile(HERE / "machine.json", SITE / "machine.json")
    shutil.copyfile(state, SITE / "state.bin.zst")
    shutil.copyfile(STATE / "primer.bin", SITE / "primer.bin")

    on_start = sum((SITE / name).stat().st_size for name in ON_START)
    page = (HERE / "page" / "index.html").read_text(encoding="utf-8")
    machine = json.loads((HERE / "machine.json").read_text(encoding="utf-8"))
    replacements = {
        "@VERSION@": app_version(),
        "@COLS@": str(machine["cols"]),
        "@ROWS@": str(machine["rows"]),
        "@DOWNLOAD_MB@": f"{on_start / 1e6:.0f}",
        "@DOWNLOAD_BYTES@": str(on_start),
        "@MACHINE_BYTES@": str(sum((SITE / name).stat().st_size for name in BY_EMULATOR)),
        **pinned_versions(),
    }
    for mark, value in replacements.items():
        if mark not in page:
            raise SystemExit(f"error: page/index.html has no {mark}")
        page = page.replace(mark, value)
    (SITE / "index.html").write_text(page, encoding="utf-8")

    sizes = {name: (SITE / name).stat().st_size for name in ON_START}
    (WORK / "sizes.json").write_text(json.dumps(sizes, indent=2) + "\n", encoding="utf-8")
    print(f"\nA press of Start downloads {on_start} B ({on_start / 1e6:.1f} MB), uncompressed in transit:")
    for name, size in sorted(sizes.items(), key=lambda item: -item[1]):
        print(f"  {size:>10}  {name}")
    print(f"\nThe page is in {SITE.relative_to(ROOT)}; serve it with")
    print(f"  python -m http.server --directory {SITE.relative_to(ROOT)} 8765")
    return 0


if __name__ == "__main__":
    sys.exit(main())
