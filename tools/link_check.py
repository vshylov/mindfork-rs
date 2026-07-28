#!/usr/bin/env python3
"""Relative-link guard for the Markdown documentation.

Reports every tracked `*.md` file with a relative link whose target does not
exist. Nothing else catches this: a stale link is valid Markdown, so `cargo`,
`clippy` and `cyrillic_scan` all pass and the breakage only shows when someone
clicks it on GitHub.

The failure has one dominant cause, and it is procedural rather than careless.
AGENTS.md §4 says a finished track's design plan moves to `docs/history/` — the
file gains a directory level, and every relative link inside it silently breaks
(`../src/foo.rs` starts resolving to `docs/src/foo.rs`). One archived plan had
23 such links. The same move also strands links *to* the plan, which are easy to
remember, and links *inside* it, which are not.

Deliberately narrow, so that a green run means something:

* **Only relative links.** `http(s)`/`mailto`/bare `#anchor` are somebody else's
  problem — checking the network would make the gate slow and flaky.
* **Only the file part.** `file.md#section` is verified as far as `file.md`;
  anchors are not resolved, because heading slugs are a rendering detail and
  chasing them would produce false failures on every heading edit.
* **Code is skipped** — fenced blocks and inline spans. `[text](url)` inside
  backticks is an *example* of a link, not a link, and this is the whole reason
  no hardcoded exception list is needed.

Exit code is non-zero when a target is missing, so this doubles as a CI lint.

Usage:
    python tools/link_check.py            # summary + exit code
    python tools/link_check.py --list     # every offending link with its line
"""

from __future__ import annotations

import collections
import re
import subprocess
import sys
import urllib.parse
from pathlib import Path

# `](target)` — the inline-link form. Reference-style definitions (`[id]: path`)
# are not used anywhere in this repo; add them here if that changes.
LINK = re.compile(r"\]\(\s*([^)\s]+)")
FENCE = re.compile(r"^\s*(```|~~~)")
# One or more backticks, the shortest run that closes with the same count.
CODE_SPAN = re.compile(r"(`+)[^`]*?\1")
SKIP_SCHEME = ("http://", "https://", "mailto:", "data:", "//")


def tracked_markdown():
    out = subprocess.run(
        ["git", "ls-files", "*.md"], capture_output=True, text=True, check=True
    ).stdout
    return [Path(line) for line in out.splitlines() if line]


def strip_code(lines):
    """Blank out fenced blocks and inline spans, keeping line numbers intact."""
    out, fence = [], None
    for line in lines:
        m = FENCE.match(line)
        if fence:
            # Inside a fence: everything is code until the matching marker.
            out.append("")
            if m and m.group(1)[0] == fence[0]:
                fence = None
            continue
        if m:
            fence = m.group(1)
            out.append("")
            continue
        out.append(CODE_SPAN.sub("", line))
    return out


def broken_links(path: Path):
    """[(line_no, target)] for every relative link that does not resolve."""
    text = path.read_text(encoding="utf-8", errors="replace")
    bad = []
    for lineno, line in enumerate(strip_code(text.split("\n")), 1):
        for m in LINK.finditer(line):
            target = m.group(1).strip()
            if not target or target.startswith("#") or target.lower().startswith(SKIP_SCHEME):
                continue
            # Anchors and query strings are not part of the path on disk.
            file_part = target.split("#", 1)[0].split("?", 1)[0]
            if not file_part:
                continue
            resolved = urllib.parse.unquote(file_part)
            if not (path.parent / resolved).exists():
                bad.append((lineno, target))
    return bad


def main() -> int:
    show_all = "--list" in sys.argv
    findings = collections.OrderedDict()
    for path in tracked_markdown():
        if not path.exists():  # listed but deleted in the working tree
            continue
        bad = broken_links(path)
        if bad:
            findings[path] = bad

    if not findings:
        print("clean: every relative link in the Markdown docs resolves")
        return 0

    total = sum(len(v) for v in findings.values())
    print(f"broken relative links: {total} in {len(findings)} file(s)\n")
    for path, bad in findings.items():
        print(f"  {len(bad):>3}  {path.as_posix()}")
        if show_all:
            for lineno, target in bad:
                print(f"       {path.as_posix()}:{lineno}  ->  {target}")
    if not show_all:
        print("\nRun with --list to see each link.")
    print(
        "\nA plan moved into docs/history/ gains a directory level: its own\n"
        "relative links need one more `../`. See AGENTS.md §4."
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
