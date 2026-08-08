#!/usr/bin/env python3
"""Structure guard for the split engineering documentation.

The ~995 KB `CLAUDE.md` used to be a small guide followed by a 243-entry
engineering journal, and the whole thing was auto-loaded into every agent
session — roughly 237K tokens spent before any work began. The journal now lives
in `docs/journal/*.md`, split by subsystem, `CLAUDE.md` is a router that links to
those files, and the recurring traps sit in `docs/lessons.md`.

That structure rots silently. An entry appended without its index bullet, a
journal file nobody links to, a router that creeps back towards a megabyte —
none of it breaks a build, none of it fails a test, and each one is invisible
until someone goes looking for a document that turns out to be unreachable. This
is the same procedural gap that `link_check.py` and `cyrillic_scan.py` close, so
it gets the same answer: a check in CI's `lint` job.

What is verified:

1. **Index vs entries.** Each journal file has a `## Entries (N)` section listing
   every entry as a `- <title>` bullet, followed by the entries as `### <title>`
   headings. The bullet list and the heading list must be equal **as ordered
   sequences**, and `N` must be the count. This is what catches an entry appended
   without its index row — the likeliest failure by far.
2. **No duplicate entry titles**, within a file or across files. A duplicate
   means an entry was copied rather than moved, and the two copies will drift.
3. **Reachability.** Every `docs/journal/*.md` is referenced from `CLAUDE.md`,
   and every `docs/journal/...` path in `CLAUDE.md` resolves. A journal file
   nobody links to is a file nobody reads.
4. **`docs/lessons.md` exists and is referenced from `CLAUDE.md`.**
5. **`CLAUDE.md` stays under `CLAUDE_MD_MAX_BYTES`.**

Two deliberate choices:

* **A missing or empty `docs/journal/` is a failure, not a pass.** A check that
  goes quiet when its subject disappears is worse than no check, because it
  reports confidence it no longer has.
* **A reference is any occurrence of the path**, whether a Markdown link target
  or an inline code span, because how the router chooses to format its map is
  not this gate's business. Fenced code blocks are skipped, though — a path in a
  worked example is an illustration, not a reference (the same reasoning
  `link_check.py` applies).

Exit code is non-zero on any violation, so this doubles as a CI lint.

Usage:
    python tools/doc_index_check.py            # verdict + every violation
    python tools/doc_index_check.py --list     # also the full entry inventory
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

# `CLAUDE.md` is auto-loaded in full at the start of every agent session, so its
# size is a tax every session pays before any work begins — which is exactly how
# the 946 KB journal that used to live here went unnoticed for so long. The
# ceiling is a router's worth of prose with room to grow, an order of magnitude
# below the point where anyone would feel the cost again.
CLAUDE_MD_MAX_BYTES = 30_000

CLAUDE_MD = "CLAUDE.md"
JOURNAL_DIR = "docs/journal"
LESSONS = "docs/lessons.md"

ENTRIES_HEADER = re.compile(r"^##\s+Entries\s*\((\d+)\)\s*$")
ENTRY_HEADING = re.compile(r"^###\s+(.+?)\s*$")
INDEX_BULLET = re.compile(r"^-\s+(.+?)\s*$")
FENCE = re.compile(r"^\s*(```|~~~)")

# A concrete file, so a glob (`docs/journal/*.md`) in the prose is not read as a
# reference to a file named `*.md`.
JOURNAL_REF = re.compile(r"docs/journal/([A-Za-z0-9._-]+\.md)")
LESSONS_REF = re.compile(re.escape(LESSONS))


def repo_root() -> Path:
    """The repository root, found from this script's own location.

    Not the working directory: the gate has to give the same answer whether it
    is run from the root, from `tools/`, or by a CI step that cd's elsewhere.
    """
    return Path(__file__).resolve().parents[1]


def outside_fences(text: str):
    """(line_no, line) for every line that is not inside a fenced code block."""
    fence = None
    for lineno, line in enumerate(text.split("\n"), 1):
        m = FENCE.match(line)
        if fence:
            # Inside a fence: everything is code until the matching marker.
            if m and m.group(1)[0] == fence[0]:
                fence = None
            continue
        if m:
            fence = m.group(1)
            continue
        yield lineno, line


def parse_journal(text: str):
    """(declared, header_line, index, headings) for one journal file.

    `declared` is the N of `## Entries (N)` (None when the header is missing);
    `index` and `headings` are [(line_no, title)] in document order.

    The index region needs no explicit end marker: bullets are collected only
    after the header and only until the first `### ` heading, so a `- ` line in
    an entry body is content rather than an index row.
    """
    declared = header_line = None
    index: list[tuple[int, str]] = []
    headings: list[tuple[int, str]] = []
    for lineno, line in outside_fences(text):
        m = ENTRIES_HEADER.match(line)
        if m:
            declared, header_line = int(m.group(1)), lineno
            continue
        m = ENTRY_HEADING.match(line)
        if m:
            headings.append((lineno, m.group(1)))
            continue
        if declared is not None and not headings:
            m = INDEX_BULLET.match(line)
            if m:
                index.append((lineno, m.group(1)))
    return declared, header_line, index, headings


def check_index(rel: str, parsed, out: list[tuple[str, str]]) -> None:
    """Check 1: the index and the entries of one file agree."""
    declared, header_line, index, headings = parsed
    if declared is None:
        out.append(
            (
                rel,
                "no `## Entries (N)` header — every journal file carries one, "
                "listing its entries as `- <title>` bullets",
            )
        )
        return
    if declared != len(headings):
        out.append(
            (
                f"{rel}:{header_line}",
                f"header says `## Entries ({declared})` but the file holds "
                f"{len(headings)} `### ` entries",
            )
        )

    idx_titles = [t for _, t in index]
    head_titles = [t for _, t in headings]
    if idx_titles == head_titles:
        return

    idx_set, head_set = set(idx_titles), set(head_titles)
    for lineno, title in headings:
        if title not in idx_set:
            out.append(
                (
                    f"{rel}:{lineno}",
                    f"entry missing from the `## Entries` index: {title!r}",
                )
            )
    for lineno, title in index:
        if title not in head_set:
            out.append(
                (
                    f"{rel}:{lineno}",
                    f"index lists an entry with no `### ` heading: {title!r}",
                )
            )
    if idx_set == head_set:
        # Same titles, different order — the loops above find nothing, so the
        # divergence has to be named explicitly or the report would be empty.
        for pos, (a, b) in enumerate(zip(idx_titles, head_titles), 1):
            if a != b:
                out.append(
                    (
                        f"{rel}:{index[pos - 1][0]}",
                        f"index order diverges at position {pos}: the index has "
                        f"{a!r}, the entries have {b!r}",
                    )
                )
                break


def check_duplicates(per_file, out: list[tuple[str, str]]) -> None:
    """Check 2: no entry title appears twice, in one file or across files."""
    seen: dict[str, list[str]] = {}
    for rel, (_, _, _, headings) in per_file.items():
        for lineno, title in headings:
            seen.setdefault(title, []).append(f"{rel}:{lineno}")
    for title, places in seen.items():
        if len(places) > 1:
            out.append(
                (
                    places[0],
                    f"duplicate entry title {title!r}, also at "
                    + ", ".join(places[1:])
                    + " — an entry was copied rather than moved, and the copies "
                    "will drift",
                )
            )


def references(text: str, pattern: re.Pattern) -> dict[str, int]:
    """{matched text: first line it appears on}, outside fenced code blocks."""
    found: dict[str, int] = {}
    for lineno, line in outside_fences(text):
        for m in pattern.finditer(line):
            found.setdefault(m.group(0) if not m.groups() else m.group(1), lineno)
    return found


def check_claude_md(root: Path, journal: list[Path], out: list[tuple[str, str]]) -> None:
    """Checks 3-5: reachability, lessons.md, and the byte ceiling."""
    path = root / CLAUDE_MD
    if not path.exists():
        out.append((CLAUDE_MD, "missing"))
        return

    size = path.stat().st_size
    if size > CLAUDE_MD_MAX_BYTES:
        out.append(
            (
                CLAUDE_MD,
                f"{size:,} bytes, over the {CLAUDE_MD_MAX_BYTES:,}-byte ceiling. "
                "It is auto-loaded in full into every agent session, so this is a "
                "per-session tax. Whatever grew probably belongs in a "
                f"{JOURNAL_DIR}/ file or in {LESSONS}, not in the router",
            )
        )

    text = path.read_text(encoding="utf-8")

    linked = references(text, JOURNAL_REF)
    on_disk = {p.name for p in journal}
    for name in sorted(on_disk - set(linked)):
        out.append(
            (
                f"{JOURNAL_DIR}/{name}",
                f"not referenced from {CLAUDE_MD} — a journal file nobody links "
                "to is a file nobody reads; add it to the document map",
            )
        )
    for name, lineno in sorted(linked.items()):
        if name not in on_disk:
            out.append(
                (
                    f"{CLAUDE_MD}:{lineno}",
                    f"references {JOURNAL_DIR}/{name}, which does not exist",
                )
            )

    if not (root / LESSONS).exists():
        out.append((LESSONS, "missing — it holds the traps that recur across areas"))
    elif not references(text, LESSONS_REF):
        out.append((CLAUDE_MD, f"does not reference {LESSONS}"))


def collect(root: Path, out: list[tuple[str, str]]):
    """Parse every journal file, reporting a missing or empty directory."""
    journal = sorted((root / JOURNAL_DIR).glob("*.md"))
    if not journal:
        out.append(
            (
                f"{JOURNAL_DIR}/",
                "no `*.md` files — the journal is missing or empty. This gate "
                "fails rather than passes here: a check that goes quiet when its "
                "subject disappears reports confidence it no longer has",
            )
        )
    per_file = {}
    for path in journal:
        rel = path.relative_to(root).as_posix()
        per_file[rel] = parse_journal(path.read_text(encoding="utf-8"))
    return journal, per_file


def report(per_file, violations, list_mode: bool) -> int:
    if list_mode:
        for rel, (_, _, _, headings) in per_file.items():
            print(f"{rel}  ({len(headings)} entries)")
            for lineno, title in headings:
                print(f"       {lineno}: {title}")
        print()

    if not violations:
        entries = sum(len(h) for _, _, _, h in per_file.values())
        print(
            f"clean: {entries} entries across {len(per_file)} journal files, "
            f"each indexed and reachable from {CLAUDE_MD}"
        )
        return 0

    print(f"documentation structure: {len(violations)} violation(s)\n")
    for where, what in violations:
        print(f"  {where}\n       {what}")
    # Deliberately points at the router rather than at the design doc: that doc
    # moves to docs/history/ when the track finishes (AGENTS.md §4), and a stale
    # path in *this* gate's own message would be the exact rot it exists to stop.
    print(
        f"\nThe layout is described in {CLAUDE_MD}: each {JOURNAL_DIR}/*.md carries a\n"
        "`## Entries (N)` index matching its `### ` entries, and is linked from the\n"
        f"document map there, alongside {LESSONS}."
    )
    return 1


def main() -> int:
    list_mode = "--list" in sys.argv
    root = repo_root()
    violations: list[tuple[str, str]] = []
    journal, per_file = collect(root, violations)
    for rel, parsed in per_file.items():
        check_index(rel, parsed, violations)
    check_duplicates(per_file, violations)
    check_claude_md(root, journal, violations)
    return report(per_file, violations, list_mode)


if __name__ == "__main__":
    # Entry titles carry em-dashes and arrows. On Windows the console defaults to
    # cp1252, where printing them raises UnicodeEncodeError and kills the run
    # exactly when the report is needed — the same trap `cyrillic_scan.py` hit.
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())
