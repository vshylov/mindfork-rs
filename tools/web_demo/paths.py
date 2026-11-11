"""Where the web demo's build keeps things - all of it under ``target/web-demo/``.

docs/research/web-demo.md §13.5. The tools take no path on their command lines:
what one step writes the next one reads from the same fixed place, so there is
no argument to get wrong and nothing a caller can point somewhere else.
"""

from __future__ import annotations

from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
WORK = ROOT / "target" / "web-demo"

# The pinned third-party pieces, as fetch.py leaves them.
VENDOR = WORK / "vendor"
# The 32-bit libraries and the packages they came out of (build_i686.sh).
SYSROOT = WORK / "sysroot"
DEBS = SYSROOT / "debs"
LIB_DIRS = [SYSROOT / "usr" / "lib32", SYSROOT / "usr" / "lib" / "i386-linux-gnu"]
# The application, built for the emulator's machine (build_i686.sh).
BINARY = WORK / "cargo" / "i686-unknown-linux-gnu" / "release" / "mindfork"
# The guest's initramfs (initramfs.py), then the saved machine (make_state.mjs).
INITRAMFS = WORK / "guest" / "initramfs.cpio.gz"
STATE = WORK / "state"
# The page with everything it loads, and the same tree as one release asset.
SITE = WORK / "site"
ASSET = WORK / "mindfork-webdemo.tar"

# The application's own notices: `cargo about generate` writes them here
# (about.toml); the file is derived, so a checkout may not have it.
NOTICES = ROOT / "THIRD-PARTY-NOTICES.md"
