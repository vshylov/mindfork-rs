#!/usr/bin/env python3
"""Write the web demo guest's initramfs: everything the machine runs.

docs/research/web-demo.md, stage 2. The guest is a kernel and this archive -
no distribution is installed in it. The archive holds a static shell, the
application with the libraries it loads, a certificate bundle, and an ``/init``
that starts ``mindfork demo`` on the machine's terminal and nothing else.

  python tools/web_demo/initramfs.py --out <file.cpio.gz> --binary <mindfork> \\
      --busybox <busybox.static> --ca-bundle <ca-certificates.crt> \\
      --libs <dir> [--libs <dir>...] [--tty ttyS0]

The archive is a gzip-compressed ``newc`` cpio, written here byte by byte: the
one device node an initramfs needs can then be recorded without root, and the
same inputs give the same archive.
"""

from __future__ import annotations

import argparse
import gzip
import io
import json
import stat
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
# What the binary is linked against, and the loader that starts it.
LIBS = ["ld-linux.so.2", "libc.so.6", "libm.so.6", "libgcc_s.so.1", "libasound.so.2"]
HOME = "/opt/mindfork"
# Printed on the terminal the moment before the application takes it: what
# follows the mark is the application's own output, from its first byte.
START_MARK = "@@demo-starts@@"

INIT = """#!/bin/busybox sh
# The web demo's whole userland: mount what a program expects, give the
# terminal its size, and run the application on it. docs/research/web-demo.md.
bb=/bin/busybox
$bb mount -t devtmpfs dev /dev
$bb mount -t proc proc /proc
$bb mount -t sysfs sys /sys
export HOME=/root PATH=/bin TERM=xterm-256color COLORTERM=truecolor
tty=/dev/{tty}
# A session of its own, and the terminal opened inside it: a session leader
# with no terminal takes the first one it opens as its controlling terminal,
# so a change of the terminal's size reaches the application as a signal.
# The size is set on the descriptors the application inherits - a terminal
# nobody holds open forgets its size, and the application then finds a
# window of nothing by nothing and draws nothing.
$bb setsid $bb sh -c "exec <$tty >$tty 2>&1; $bb stty rows {rows} cols {cols}; $bb printf '\\n{mark}\\n'; exec {home}/ld-linux.so.2 --library-path {home} {home}/mindfork demo"
# The application has quit. Nothing else runs here; the page starts a new
# machine. Process 1 must not exit.
exec $bb sleep 2147483647
"""


class Cpio:
    """A ``newc`` archive: entries are appended, ``finish`` gives the bytes."""

    def __init__(self) -> None:
        self.out = io.BytesIO()
        self.inode = 0

    def _entry(self, name: str, mode: int, data: bytes = b"", rdev: tuple[int, int] = (0, 0)) -> None:
        self.inode += 1
        encoded = name.encode() + b"\0"
        fields = [self.inode, mode, 0, 0, 2 if stat.S_ISDIR(mode) else 1, 0, len(data), 0, 0, rdev[0], rdev[1], len(encoded), 0]
        self.out.write(b"070701" + b"".join(b"%08X" % field for field in fields))
        self.out.write(encoded)
        self._pad()
        self.out.write(data)
        self._pad()

    def _pad(self) -> None:
        self.out.write(b"\0" * (-self.out.tell() % 4))

    def directory(self, name: str) -> None:
        self._entry(name, stat.S_IFDIR | 0o755)

    def file(self, name: str, data: bytes, executable: bool = False) -> None:
        self._entry(name, stat.S_IFREG | (0o755 if executable else 0o644), data)

    def char_device(self, name: str, major: int, minor: int) -> None:
        self._entry(name, stat.S_IFCHR | 0o600, rdev=(major, minor))

    def finish(self) -> bytes:
        self.inode = -1  # the trailer's inode is 0
        self._entry("TRAILER!!!", 0)
        self.out.write(b"\0" * (-self.out.tell() % 512))
        return self.out.getvalue()


def build(binary: Path, busybox: Path, ca_bundle: Path, lib_dirs: list[Path], tty: str) -> bytes:
    machine = json.loads((HERE / "machine.json").read_text(encoding="utf-8"))
    archive = Cpio()
    for directory in ("bin", "dev", "proc", "sys", "tmp", "root", "etc", "etc/ssl", "etc/ssl/certs", "opt", HOME.lstrip("/")):
        archive.directory(directory)
    # The kernel opens /dev/console for process 1 before /init can mount /dev.
    archive.char_device("dev/console", 5, 1)
    init = INIT.format(tty=tty, rows=machine["rows"], cols=machine["cols"], mark=START_MARK, home=HOME)
    archive.file("init", init.encode(), executable=True)
    archive.file("bin/busybox", busybox.read_bytes(), executable=True)
    archive.file("etc/ssl/certs/ca-certificates.crt", ca_bundle.read_bytes())
    for lib in LIBS:
        found = next((d / lib for d in lib_dirs if (d / lib).exists()), None)
        if found is None:
            raise SystemExit(f"error: no {lib} in {', '.join(str(d) for d in lib_dirs)}")
        archive.file(f"{HOME.lstrip('/')}/{lib}", found.read_bytes(), executable=True)
    archive.file(f"{HOME.lstrip('/')}/mindfork", binary.read_bytes(), executable=True)
    # mtime=0: the same inputs give the same bytes.
    packed = io.BytesIO()
    with gzip.GzipFile(fileobj=packed, mode="wb", compresslevel=6, mtime=0) as stream:
        stream.write(archive.finish())
    return packed.getvalue()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--binary", required=True, type=Path, help="the 32-bit x86 release binary")
    parser.add_argument("--busybox", required=True, type=Path, help="a statically linked 32-bit BusyBox")
    parser.add_argument("--ca-bundle", required=True, type=Path, help="a PEM bundle of CA certificates")
    parser.add_argument("--libs", required=True, action="append", type=Path,
                        help="a directory holding the binary's 32-bit shared libraries; repeatable")
    parser.add_argument("--tty", default="ttyS0", help="the guest's terminal device (default: ttyS0)")
    args = parser.parse_args()

    data = build(args.binary, args.busybox, args.ca_bundle, args.libs, args.tty)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_bytes(data)
    print(f"{args.out}: {len(data)} B")
    return 0


if __name__ == "__main__":
    sys.exit(main())
