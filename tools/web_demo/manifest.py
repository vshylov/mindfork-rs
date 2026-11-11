#!/usr/bin/env python3
"""The web demo's manifest: every redistributed piece, as this build found it.

docs/research/web-demo.md §13.5. The demo hands a visitor other people's work -
an emulator, a terminal, a BIOS, a kernel, a shell, three C libraries - and each
comes with terms: its name and licence have to be said, its licence text has to
travel, and for the copyleft ones the exact source has to be within reach. A
list written by hand says what its author believed on the day; this one is read
out of the very files the build packs:

- the emulator and the terminal from their npm packages' own ``package.json``;
- the kernel's release from its boot header, the BIOS's from its image;
- the shell and its C library from the Alpine image's record of its packages,
  down to the commit of Alpine's build files each was made from;
- the application's C libraries from the distribution packages they were
  unpacked from (``sysroot/debs``): package, version, source package;
- the application from ``Cargo.toml`` and the commit being built.

What cannot be read is stated once, in ``TERMS`` below, and checked against
what can: a pin moved to a version these facts were not written for stops the
build.

  python tools/web_demo/manifest.py --self-test   # the readers, on small fixtures

build.py calls ``collect`` and ``texts``; nothing here takes a path.
"""

from __future__ import annotations

import html
import io
import json
import re
import struct
import subprocess
import sys
import tarfile
from dataclasses import asdict, dataclass, field
from functools import lru_cache
from pathlib import Path

import fetch
from initramfs import LIBS, library
from paths import DEBS, NOTICES, ROOT, SYSROOT, VENDOR

# The manifest's own shape. Raised when a field changes meaning, so that a page
# written for one shape does not read another.
FORMAT = 1
# Where the texts go in the asset, and the distribution's copies of the GNU
# licences, which its packages' copyright files refer to by these names.
TEXTS = "licences"
COMMON_LICENCES = Path("/usr/share/common-licenses")
# The commit of copy/v86 the BIOS images are read from (fetch.py pins the same).
V86_BIOS = "https://github.com/copy/v86/tree/315b2b0b758703ef0e6cc7b4088d6068f83f3c20/bios"
APORTS = "https://gitlab.alpinelinux.org/alpine/aports/-/tree"
PAGE = "page"
MACHINE = "machine"


@dataclass
class Piece:
    """One redistributed work. ``texts`` are paths in the asset; ``sources`` are
    (what, url) pairs - where the exact source of this build of it is."""

    name: str
    version: str
    part: str
    role: str
    licence: str
    homepage: str
    texts: list[str] = field(default_factory=list)
    sources: list[dict[str, str]] = field(default_factory=list)
    # The distribution package it was taken out of, where there is one.
    package: str = ""


def source(what: str, url: str) -> dict[str, str]:
    return {"what": what, "url": url}


def text_path(name: str) -> str:
    return f"{TEXTS}/{name}"


# What no file says about itself: the terms of the works whose packages do not
# record them in a form to read, and what each is in the demo.
TERMS = {
    "glibc": {
        "name": "GNU C Library",
        "licence": "LGPL-2.1-or-later",
        "common": ["LGPL-2.1"],
        "homepage": "https://sourceware.org/glibc/",
        "role": "the C library the program is linked against",
    },
    "gcc": {
        "name": "GCC runtime library (libgcc_s)",
        "licence": "GPL-3.0-or-later WITH GCC-exception-3.1",
        "common": ["GPL-3"],
        "homepage": "https://gcc.gnu.org/",
        "role": "the compiler's runtime library, which the program loads",
    },
    "alsa-lib": {
        "name": "ALSA library",
        "licence": "LGPL-2.1-or-later",
        "common": ["LGPL-2.1"],
        "homepage": "https://www.alsa-project.org/",
        "role": "the sound library the program is linked against; the machine has no sound",
    },
}


# --- the readers -----------------------------------------------------------


def kernel_release(image: bytes) -> str:
    """The release a Linux x86 boot image names itself: the boot protocol's
    ``kernel_version`` field, at 0x20e, points at the string."""
    offset = struct.unpack_from("<H", image, 0x20E)[0]
    start = 0x200 + offset
    text = image[start : start + 128].split(b"\0", 1)[0]
    return text.decode("ascii", "replace").split(" ", 1)[0]


def bios_release(image: bytes) -> str:
    """The SeaBIOS release an image was built from: its version string is
    ``rel-<release>-<commits since>-g<commit>``."""
    found = re.search(rb"rel-(\d+(?:\.\d+)+)", image)
    if not found:
        raise SystemExit("error: the BIOS image names no SeaBIOS release")
    return found.group(1).decode("ascii")


def alpine_packages(record: str) -> list[dict]:
    """Alpine's record of an image's packages (``lib/apk/db/installed``): one
    dict per package - its one-letter fields, and under ``files`` every path the
    package owns."""
    packages = []
    for block in record.split("\n\n"):
        fields: dict = {"files": set()}
        directory = ""
        for line in block.splitlines():
            key, _, value = line.partition(":")
            if key == "F":
                directory = value
            elif key == "R":
                fields["files"].add(f"{directory}/{value}")
            else:
                fields.setdefault(key, value)
        if "P" in fields:
            packages.append(fields)
    return packages


def alpine_owner(packages: list[dict], path: str) -> dict:
    for package in packages:
        if path in package["files"]:
            return package
    raise SystemExit(f"error: no package in Alpine's record owns /{path}")


def deb_fields(control: str) -> dict[str, str]:
    """A distribution package's name, version and source out of its control
    fields. ``Source`` is absent where it is the package's own name, and carries
    a version in brackets where the source's differs from the package's."""
    pairs = (line.split(": ", 1) for line in control.splitlines() if ": " in line)
    fields = {key: value for key, value in pairs}
    package, version = fields["Package"], fields["Version"]
    name, _, rest = fields.get("Source", package).partition(" ")
    return {"package": package, "version": version, "source": name, "source_version": rest.strip("()") or version}


def upstream_version(version: str) -> str:
    """``2.39-0ubuntu8.9`` is the distribution's ninth build of upstream's 2.39."""
    return version.rsplit("-", 1)[0].split(":", 1)[-1]


def deb_control(deb: Path) -> str:
    done = subprocess.run(["dpkg-deb", "-f", str(deb)], capture_output=True, text=True, check=True)
    return done.stdout


def deb_paths(deb: Path) -> set[str]:
    done = subprocess.run(["dpkg-deb", "--fsys-tarfile", str(deb)], capture_output=True, check=True)
    with tarfile.open(fileobj=io.BytesIO(done.stdout)) as tar:
        return {name.removeprefix("./") for name in tar.getnames()}


def deb_owning(library: Path) -> dict[str, str]:
    """The package in ``sysroot/debs`` a library of the sysroot was unpacked from."""
    inside = library.relative_to(SYSROOT).as_posix()
    for deb in sorted(DEBS.glob("*.deb")):
        if inside in deb_paths(deb):
            return deb_fields(deb_control(deb))
    raise SystemExit(f"error: no package in {DEBS} holds {inside}")


def cargo_field(name: str) -> str:
    found = re.search(rf'^{name} = "([^"]+)"', (ROOT / "Cargo.toml").read_text(encoding="utf-8"), re.MULTILINE)
    if not found:
        raise SystemExit(f"error: no {name} in Cargo.toml")
    return found.group(1)


def commit() -> str:
    done = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, check=True, cwd=ROOT)
    return done.stdout.strip()


# --- the pieces ------------------------------------------------------------


def repository_url(repository: dict[str, str] | str) -> str:
    """Where an npm package says its code lives, as a link: the field is a URL,
    an object holding one, or the ``github:owner/name`` shorthand."""
    url = repository if isinstance(repository, str) else repository["url"]
    url = url.removeprefix("git+").removesuffix(".git")
    return url.replace("github:", "https://github.com/", 1) if url.startswith("github:") else url


def npm_piece(stem: str, name: str, role: str) -> Piece:
    """A piece that is an npm package: its ``package.json`` says the rest, and
    its source is the very archive fetch.py pins."""
    record = f"{stem}.package.json"
    package = json.loads((VENDOR / record).read_text(encoding="utf-8"))
    pinned = next(entry for entry in fetch.PACKAGES if record in entry["members"].values())
    return Piece(
        name=name,
        version=package["version"],
        part=PAGE,
        role=role,
        licence=package["license"],
        homepage=repository_url(package["repository"]),
        texts=[text_path(f"{stem}.txt")],
        sources=[source(f"the npm package {pinned['name']}", pinned["url"])],
    )


def bios_piece() -> Piece:
    release = bios_release((VENDOR / "seabios.bin").read_bytes())
    return Piece(
        name="SeaBIOS",
        version=release,
        part=MACHINE,
        role="the machine's BIOS and VGA BIOS",
        licence="LGPL-3.0-only",
        homepage="https://www.seabios.org/",
        texts=[text_path("LGPL-3.txt"), text_path("GPL-3.txt")],
        sources=[
            source(f"seabios-{release}.tar.gz", f"https://www.seabios.org/downloads/seabios-{release}.tar.gz"),
            source("the configuration and script v86 builds it with", V86_BIOS),
        ],
    )


def kernel_piece(alpine: str) -> Piece:
    release = kernel_release((VENDOR / "vmlinuz-virt").read_bytes())
    # `6.18.60-0-virt`: upstream's version, Alpine's build of it, the flavour.
    version, build, flavour = release.split("-")
    series = version.split(".", 1)[0]
    return Piece(
        name="Linux",
        version=version,
        part=MACHINE,
        role="the machine's kernel",
        licence="GPL-2.0-only",
        homepage="https://kernel.org/",
        texts=[text_path("GPL-2.txt")],
        sources=[
            source(f"linux-{version}.tar.xz", f"https://cdn.kernel.org/pub/linux/kernel/v{series}.x/linux-{version}.tar.xz"),
            source("Alpine's configuration and patches for it", f"{APORTS}/v{alpine}/main/linux-lts"),
        ],
        package=f"Alpine Linux {alpine}, linux-{flavour} {version}-r{build}",
    )


def alpine_piece(packages: list[dict], alpine: str, path: str, role: str, texts: list[str]) -> Piece:
    """A piece out of Alpine's base image: the image's record says the rest."""
    package = alpine_owner(packages, path)
    name, version = package["P"], package["V"]
    return Piece(
        name={"busybox": "BusyBox"}.get(name, name),
        version=upstream_version(version),
        part=MACHINE,
        role=role,
        licence=package["L"],
        homepage=package["U"],
        texts=texts,
        sources=[source(f"Alpine's build files for {name} {version}", f"{APORTS}/{package['c']}/main/{package['o']}")],
        package=f"Alpine Linux {alpine}, {name} {version}",
    )


@lru_cache(maxsize=1)
def library_packages() -> dict[str, dict[str, str]]:
    """The packages the guest's libraries were unpacked from, by the family of
    their source package: ``gcc-14`` is ``gcc``, whichever compiler built it."""
    found: dict[str, dict[str, str]] = {}
    for lib in LIBS:
        deb = deb_owning(library(lib))
        family = re.sub(r"-\d+$", "", deb["source"])
        if family not in TERMS:
            raise SystemExit(f"error: no terms are written down for the source package {deb['source']} (manifest.py, TERMS)")
        found.setdefault(family, deb)
    return found


def library_pieces() -> list[Piece]:
    """The application's C libraries, one piece per source package: what the
    packages in ``sysroot/debs`` say, with ``TERMS`` for what they do not."""
    pieces = []
    for family, deb in library_packages().items():
        name, terms = deb["source"], TERMS[family]
        pieces.append(
            Piece(
                name=terms["name"],
                version=upstream_version(deb["version"]),
                part=MACHINE,
                role=terms["role"],
                licence=terms["licence"],
                homepage=terms["homepage"],
                texts=[text_path(f"{family}-copyright.txt")] + [text_path(f"{common}.txt") for common in terms["common"]],
                sources=[
                    source(
                        f"Ubuntu's source package {name} {deb['source_version']}",
                        f"https://launchpad.net/ubuntu/+source/{name}/{deb['source_version']}",
                    )
                ],
                package=f"Ubuntu, {deb['package']} {deb['version']}",
            )
        )
    return pieces


def application_piece() -> Piece:
    repository, head = cargo_field("repository"), commit()
    texts = [text_path("mindfork.txt"), text_path("mindfork-syntaxes.txt")]
    if NOTICES.exists():
        texts.insert(1, text_path("mindfork-third-party.md"))
    return Piece(
        name="mindfork",
        version=cargo_field("version"),
        part=MACHINE,
        role="the program itself, built for 32-bit x86",
        licence=cargo_field("license"),
        homepage=cargo_field("homepage"),
        texts=texts,
        sources=[source(f"the repository at {head[:12]}", f"{repository}/tree/{head}")],
    )


def collect() -> list[Piece]:
    """Every piece of the demo, the page's first and then the machine's."""
    alpine = (VENDOR / "alpine-release").read_text(encoding="utf-8").strip()
    packages = alpine_packages((VENDOR / "alpine-installed").read_text(encoding="utf-8"))
    musl = alpine_piece(packages, alpine, "lib/ld-musl-i386.so.1", "the C library the shell is linked against", [text_path("musl.txt")])
    # The notice is fetched from the release it belongs to (fetch.py); the
    # library is whatever the image carries. They have to be the same release.
    if musl.version != fetch.MUSL:
        raise SystemExit(f"error: the image carries musl {musl.version}; fetch.py pins the COPYRIGHT of {fetch.MUSL}")
    return [
        npm_piece("v86", "v86", "the x86 emulator the machine runs in"),
        npm_piece("xterm", "xterm.js", "the terminal the program draws in"),
        Piece(
            name="JetBrains Mono",
            version="",
            part=PAGE,
            role="the typeface",
            licence="OFL-1.1",
            homepage="https://www.jetbrains.com/lp/mono/",
            texts=["fonts/OFL.txt"],
            sources=[source("the typeface's repository", "https://github.com/JetBrains/JetBrainsMono")],
        ),
        application_piece(),
        *library_pieces(),
        kernel_piece(alpine),
        alpine_piece(packages, alpine, "bin/busybox", "the shell that starts the program", [text_path("GPL-2.txt")]),
        musl,
        bios_piece(),
    ]


# --- the texts -------------------------------------------------------------


def syntaxes() -> bytes:
    """The licences of the syntax grammars compiled into the program, as one
    text: a release archive carries them as a directory (``licenses/syntaxes``)."""
    parts = []
    for path in sorted((ROOT / "syntaxes" / "licenses").glob("*.txt")):
        parts.append(f"==== {path.stem} ====\n\n{path.read_text(encoding='utf-8').strip()}\n")
    return "\n\n".join(parts).encode()


def texts(pieces: list[Piece]) -> dict[str, bytes]:
    """Every text the pieces name, by its path in the asset: the licences as
    their authors, or the distribution that packaged them, ship them."""
    # A package's copyright file is the distribution's statement of whose the
    # work is and on what terms - the notices its licence requires to travel.
    doc = SYSROOT / "usr" / "share" / "doc"
    from_packages = {
        f"{family}-copyright.txt": (doc / deb["package"] / "copyright").read_bytes()
        for family, deb in library_packages().items()
    }
    known = {
        "v86.txt": (VENDOR / "LICENSE.v86").read_bytes(),
        "xterm.txt": (VENDOR / "LICENSE.xterm").read_bytes(),
        "musl.txt": (VENDOR / "COPYRIGHT.musl").read_bytes(),
        "mindfork.txt": (ROOT / "LICENSE").read_bytes(),
        "mindfork-syntaxes.txt": syntaxes(),
        **from_packages,
        **{f"{path.name}.txt": path.read_bytes() for path in sorted(COMMON_LICENCES.glob("*GPL-[23]*"))},
    }
    if NOTICES.exists():
        known["mindfork-third-party.md"] = NOTICES.read_bytes()
    named = {name for piece in pieces for name in piece.texts if name.startswith(f"{TEXTS}/")}
    missing = sorted(name for name in named if name.removeprefix(f"{TEXTS}/") not in known)
    if missing:
        raise SystemExit(f"error: no text for {', '.join(missing)}")
    return {name: known[name.removeprefix(f"{TEXTS}/")] for name in sorted(named)}


# --- the page's list -------------------------------------------------------


def link(url: str, label: str) -> str:
    return f'<a href="{html.escape(url, quote=True)}">{html.escape(label)}</a>'


def piece_html(piece: Piece) -> str:
    """One piece as a list item: what it is, its terms with their texts, and
    where its source is. A text's link is its file name and a source's is what
    it is, so no two links of the list read the same and lead elsewhere."""
    title = link(piece.homepage, piece.name) + (f" {html.escape(piece.version)}" if piece.version else "")
    terms = ", ".join(link(text, text.rsplit("/", 1)[-1]) for text in piece.texts)
    parts = [f"{title}, {html.escape(piece.role)}. {html.escape(piece.licence)}: {terms}."]
    if piece.package:
        parts.append(f"Unmodified, from {html.escape(piece.package)}.")
    if piece.sources:
        parts.append("Source: " + "; ".join(link(item["url"], item["what"]) for item in piece.sources) + ".")
    return "<li>" + " ".join(parts) + "</li>"


def list_html(pieces: list[Piece], part: str) -> str:
    return "\n".join("      " + piece_html(piece) for piece in pieces if piece.part == part)


def document(pieces: list[Piece], files: dict[str, dict], on_start: list[str], by_emulator: list[str]) -> dict:
    """The manifest itself: what the asset is of, what is in it, and whose."""
    return {
        "format": FORMAT,
        "application": cargo_field("version"),
        "commit": commit(),
        "on_start": on_start,
        "on_start_bytes": sum(files[name]["bytes"] for name in on_start),
        "by_emulator_bytes": sum(files[name]["bytes"] for name in by_emulator),
        "files": files,
        "pieces": [asdict(piece) for piece in pieces],
    }


# --- the self-test ---------------------------------------------------------

RECORD = """C:Q1abc=
P:busybox
V:1.37.0-r31
U:https://busybox.net/
L:GPL-2.0-only
o:busybox
c:c3ef5d10
F:bin
R:busybox
R:sh

P:musl
V:1.2.6-r3
L:MIT
U:https://musl.libc.org/
o:musl
c:8f79d393
F:lib
R:ld-musl-i386.so.1
"""


def self_test() -> list[str]:
    """What each reader makes of a small input; the problems found."""
    problems = []

    def expect(what: str, got: object, want: object) -> None:
        if got != want:
            problems.append(f"{what}: {got!r}, not {want!r}")

    image = bytearray(0x400)
    struct.pack_into("<H", image, 0x20E, 0x100)
    image[0x300:0x320] = b"6.18.60-0-virt (builder) #1 SMP\0"
    expect("the kernel's release", kernel_release(bytes(image)), "6.18.60-0-virt")
    expect("the BIOS's release", bios_release(b"\0\0rel-1.16.2-0-gea1b7a0\0"), "1.16.2")

    packages = alpine_packages(RECORD)
    expect("the packages of a record", [package["P"] for package in packages], ["busybox", "musl"])
    expect("the owner of the shell", alpine_owner(packages, "bin/sh")["V"], "1.37.0-r31")
    expect("the owner of the loader", alpine_owner(packages, "lib/ld-musl-i386.so.1")["c"], "8f79d393")

    own = deb_fields("Package: libasound2t64\nVersion: 1.2.11-1ubuntu0.3\nSource: alsa-lib\n")
    expect("a package and its source", (own["source"], own["source_version"]), ("alsa-lib", "1.2.11-1ubuntu0.3"))
    other = deb_fields("Package: libfoo\nVersion: 2:1.0-3\nSource: foo (1.0-2)\nDescription: a: b\n")
    expect("a source at another version", (other["source"], other["source_version"]), ("foo", "1.0-2"))
    expect("a package that is its own source", deb_fields("Package: glibc\nVersion: 2.39-1\n")["source"], "glibc")
    expect(
        "where an npm package's code lives",
        [repository_url(r) for r in ("github:copy/v86", {"type": "git", "url": "git+https://github.com/a/b.git"}, "https://c.example/d")],
        ["https://github.com/copy/v86", "https://github.com/a/b", "https://c.example/d"],
    )
    expect("upstream's version", [upstream_version(v) for v in ("2.39-0ubuntu8.9", "2:1.0-3", "1.37.0-r31")], ["2.39", "1.0", "1.37.0"])

    piece = Piece(
        name="A <b>",
        version="1",
        part=MACHINE,
        role="the thing",
        licence="MIT",
        homepage="https://a.example/?x=1&y=2",
        texts=[text_path("a.txt")],
        sources=[source("its source", "https://a.example/src")],
        package="Somewhere, a 1-r0",
    )
    expect(
        "a piece on the page",
        piece_html(piece),
        '<li><a href="https://a.example/?x=1&amp;y=2">A &lt;b&gt;</a> 1, the thing. MIT: '
        '<a href="licences/a.txt">a.txt</a>. Unmodified, from Somewhere, a 1-r0. '
        'Source: <a href="https://a.example/src">its source</a>.</li>',
    )
    expect("the pieces of one part", list_html([piece], PAGE), "")
    return problems


def main() -> int:
    if sys.argv[1:] != ["--self-test"]:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    problems = self_test()
    for problem in problems:
        print(f"error: {problem}", file=sys.stderr)
    if problems:
        return 1
    print("clean: the manifest's readers give what they should on every fixture")
    return 0


if __name__ == "__main__":
    sys.exit(main())
