#!/usr/bin/env python3
"""Single-source guard for a list's scroll position.

ratatui moves a list's offset only as far as it must to bring the selection into
view. Build the `ListState` inside `render` and that rule runs from zero on every
draw, which means "scroll until the selection is the *last* visible row": moving
the selection **down** looks like a window correctly following it, moving **up**
scrolls the list on every press and the selection never walks to the top row.

The defect is invisible in a diff (four ordinary-looking lines), invisible to the
type system, and invisible to every test that only asserts on which item is
selected. It reached eight lists in this repository — the chat list, the settings
screen's section menu, field pane, field search and Choice popup, the profile and
reference pickers and the spellcheck suggestions — because each was written by
copying the neighbour that already had it, and it was reported by a user, twice.
The rule now has exactly one implementation, `ListScroll` in `src/shared/ui.rs`,
and this gate is what keeps it the only one: a rule that must hold in eight
places is a function, not a convention.

What is verified:

1. **`ListState`/`TableState` is named only in `src/shared/ui.rs`.** `TableState`
   is in the list although no table exists yet: it carries the same
   offset-plus-selection semantics, so a table added tomorrow would reproduce the
   defect exactly. Mentions inside `//` comments are fine — the journal entries
   and doc comments that explain the trap have to be able to name it.
2. **`src/shared/ui.rs` still defines `ListScroll`.** A gate whose subject has
   been deleted or renamed must fail rather than go quiet: silence would report
   confidence it no longer has (the same choice `doc_index_check.py` makes).
3. **`src/` holds Rust files at all** — for the same reason.

Deliberately *not* verified: whether a caller passes the right `view_h` or a
`len` matching its items. That is a value, not a shape, and only the caller can
know it; the tests next to each list cover it.

Exit code is non-zero on any violation, so this doubles as a CI lint.

Usage:
    python tools/list_scroll_check.py           # verdict + every violation
    python tools/list_scroll_check.py --list    # also the lists on the helper
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

SRC_DIR = "src"
# Where the rule lives, and the name it lives under.
HOME = "src/shared/ui.rs"
HELPER = "ListScroll"

# Word-exact (`\b` at both edges), so our own `ChatListState`/`ProfileListState`
# (widget state, not ratatui's) do not match: between the `t` of `Chat` and the
# `L` of `List` there is no word boundary, while `widgets::ListState` has one.
GUARDED = re.compile(r"\b(ListState|TableState)\b")
HELPER_DEF = re.compile(rf"^\s*pub struct {HELPER}\b")
HELPER_USE = re.compile(rf"\b{HELPER}\b")


def repo_root() -> Path:
    """The repository root, found from this script's own location.

    Not the working directory: the gate has to give the same answer whether it
    is run from the root, from `tools/`, or by a CI step that cd's elsewhere.
    """
    return Path(__file__).resolve().parents[1]


def in_comment(line: str, pos: int) -> bool:
    """Is the match at `pos` inside a `//` comment?

    Line comments only. Block comments (`/* */`) are not used in this codebase
    for anything but the occasional rustdoc example, and a token hidden in one
    would still be prose rather than a `ListState` someone can build.
    """
    return "//" in line[:pos]


def scan(path: Path, rel: str) -> tuple[list[tuple[int, str, str]], bool]:
    """Violations in one file, and whether it uses the helper.

    A violation is (line number, the guarded name, the line's text).
    """
    found: list[tuple[int, str, str]] = []
    uses_helper = False
    for lineno, line in enumerate(path.read_text(encoding="utf-8").split("\n"), 1):
        m = HELPER_USE.search(line)
        if m and not in_comment(line, m.start()):
            uses_helper = True
        if rel == HOME:
            continue
        for m in GUARDED.finditer(line):
            if not in_comment(line, m.start()):
                found.append((lineno, m.group(1), line.strip()))
    return found, uses_helper


def report(scanned: int, callers: list[str], violations, list_mode: bool) -> int:
    if list_mode:
        print(f"{len(callers)} file(s) hold a {HELPER}:")
        for rel in callers:
            print(f"       {rel}")
        print()

    if not violations:
        print(
            f"clean: {scanned} .rs files scanned, a ListState is built only in "
            f"{HOME}; {len(callers)} other file(s) name a {HELPER}"
        )
        return 0

    print(f"list scroll state: {len(violations)} violation(s)\n")
    for where, what in violations:
        print(f"  {where}\n       {what}")
    print(
        "\nA list's scroll offset is state, not decoration: built inside `render` it is\n"
        "recomputed from zero every frame, which pins the selection to an edge row and\n"
        f"scrolls the list on every press in one direction. Use `{HELPER}` from {HOME}\n"
        "— keep one on the widget and call its `render`. See docs/lessons.md §5."
    )
    return 1


def main() -> int:
    list_mode = "--list" in sys.argv
    root = repo_root()
    files = sorted((root / SRC_DIR).rglob("*.rs"))
    violations: list[tuple[str, str]] = []
    callers: list[str] = []

    if not files:
        violations.append(
            (
                SRC_DIR,
                "no Rust sources found — this gate fails rather than passes on a "
                "missing subject, so it cannot go quiet the day the tree moves",
            )
        )
        return report(0, callers, violations, list_mode)

    home = root / HOME
    if not home.is_file() or not any(
        HELPER_DEF.match(line) for line in home.read_text(encoding="utf-8").split("\n")
    ):
        violations.append(
            (
                HOME,
                f"`pub struct {HELPER}` is not defined here — if the helper moved, "
                "this gate's `HOME` moves with it; if it was deleted, every list "
                "just lost the rule it enforces",
            )
        )

    for path in files:
        rel = path.relative_to(root).as_posix()
        found, uses_helper = scan(path, rel)
        if uses_helper and rel != HOME:
            callers.append(rel)
        for lineno, name, text in found:
            violations.append(
                (
                    f"{rel}:{lineno}",
                    f"`{name}` outside {HOME}: {text}",
                )
            )

    return report(len(files), callers, violations, list_mode)


if __name__ == "__main__":
    # The report carries em-dashes and section signs. On Windows the console
    # defaults to cp1252, where printing them raises UnicodeEncodeError and kills
    # the run exactly when the report is needed — the trap `cyrillic_scan.py` hit.
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())
