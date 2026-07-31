#!/usr/bin/env python3
"""Fetch the vendored sublime-syntax grammars listed in syntaxes/SOURCES.md.

Every grammar is pinned to an exact upstream commit, so a fetch is
reproducible: running this reproduces the working copy byte for byte, and
`--check` says whether the working copy still matches the pins (it downloads,
compares and writes nothing).

Updating a grammar is therefore a deliberate act: bump the commit in
SOURCES.md, re-run this, and let the build fail if the new revision uses
sublime-syntax features syntect cannot load (see docs/history/vendored-syntaxes.md
§2.1).

Usage:
    python tools/fetch_syntaxes.py [--check] [--only Zig,TOML]

Stdlib only, like the other tools/ scripts.
"""

from __future__ import annotations

import argparse
import sys
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SYNTAXES = ROOT / "syntaxes"
LICENSES = SYNTAXES / "licenses"
MANIFEST = SYNTAXES / "SOURCES.md"


@dataclass
class Entry:
    grammar: str
    file: str
    repo: str  # "host:owner/name"
    path: str
    commit: str
    licence: str
    licence_path: str  # bare path, or "host:owner/name@commit:path"


def raw_url(repo: str, commit: str, path: str) -> str:
    """Raw-file URL for `host:owner/name` at `commit`. The path is quoted —
    some upstreams put spaces in a grammar's filename (`Vue Component`)."""
    host, _, slug = repo.partition(":")
    path = urllib.parse.quote(path)
    if host == "github":
        return f"https://raw.githubusercontent.com/{slug}/{commit}/{path}"
    if host == "codeberg":
        return f"https://codeberg.org/{slug}/raw/commit/{commit}/{path}"
    raise SystemExit(f"unknown host {host!r} in {repo!r} (expected github/codeberg)")


def licence_url(entry: Entry) -> str:
    """A licence may live in a different repository than the grammar — see the
    "Files taken from sharkdp/bat" section of the manifest."""
    spec = entry.licence_path
    if ":" in spec:
        repo, _, rest = spec.partition("@")
        commit, _, path = rest.partition(":")
        return raw_url(repo, commit, path)
    return raw_url(entry.repo, entry.commit, spec)


def parse_manifest() -> list[Entry]:
    """Reads the Markdown table. Line-based on purpose: the manifest has to
    stay readable on GitHub, and a hand-rolled parser keeps this script
    stdlib-only (same trade-off as tools/link_check.py)."""
    entries: list[Entry] = []
    for line in MANIFEST.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip("|").split("|")]
        if len(cells) != 7 or cells[0] in ("Grammar", "---"):
            continue
        if set(cells[0]) <= {"-", ":"}:  # the header separator row
            continue
        entries.append(Entry(*cells))
    if not entries:
        raise SystemExit(f"no grammar rows found in {MANIFEST}")
    return entries


def download(url: str) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": "mindfork-fetch-syntaxes"})
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            return resp.read()
    except urllib.error.HTTPError as e:
        raise SystemExit(f"{url}\n  HTTP {e.code} — is the pinned commit/path right?")
    except urllib.error.URLError as e:
        raise SystemExit(f"{url}\n  {e.reason}")


def sync(dest: Path, data: bytes, check: bool) -> str:
    """Writes `data` to `dest` (or, in check mode, compares). Returns a status
    word for the log."""
    # Upstreams are inconsistent about line endings; normalising means a
    # re-fetch on Windows does not rewrite every file.
    data = data.replace(b"\r\n", b"\n")
    existed = dest.exists()
    if existed and dest.read_bytes() == data:
        return "ok"
    if check:
        return "DRIFT" if existed else "MISSING"
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_bytes(data)
    return "updated" if existed else "added"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--check",
        action="store_true",
        help="compare the working copy against the pins without writing",
    )
    ap.add_argument("--only", help="comma-separated grammar names")
    args = ap.parse_args()

    wanted = {s.strip() for s in args.only.split(",")} if args.only else None
    entries = parse_manifest()
    problems = 0
    for e in entries:
        if wanted and e.grammar not in wanted:
            continue
        grammar = sync(SYNTAXES / e.file, download(raw_url(e.repo, e.commit, e.path)), args.check)
        licence = sync(LICENSES / f"{e.grammar}.txt", download(licence_url(e)), args.check)
        print(f"  {e.grammar:12} grammar: {grammar:8} licence: {licence}")
        problems += sum(s in ("DRIFT", "MISSING") for s in (grammar, licence))

    if problems:
        print(
            f"\n{problems} file(s) differ from the pins in {MANIFEST.name}.\n"
            "Run without --check to restore them, or update the pins on purpose.",
            file=sys.stderr,
        )
        return 1
    print(f"\n{len(entries)} grammar(s) {'verified' if args.check else 'in sync'}.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
