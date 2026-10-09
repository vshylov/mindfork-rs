#!/usr/bin/env python3
"""The FSD layer gate: a layer names only the layers below it.

docs/architecture.md §2 states the dependency rule and calls one consequence
the key FSD invariant in the code — `screens`/`widgets` never import `app`. It
held by convention and in prose, and by 2026-10-09 the chat and tasks screens
imported eleven event payload types from `app::events`, the first of them seven
weeks earlier; every one of them was written by copying the neighbour that had
it, the way the eight scroll states once spread (`list_scroll_check.py`). A
rule the documents call *key* and nothing checks is a rule that has drifted by
the time someone reads it. The types moved down and this gate keeps the rule
(docs/research/fsd-layer-gate.md).

The rule, as the code has it:

    main.rs -> app -> screens -> widgets -> features -> { entities <-> shared }

What is verified:

1. **A production line in layer L names `crate::M::` only when M is L or a
   layer below it.** `entities` and `shared` are the bottom pair and see each
   other: `shared/storage` and `shared/api` read and write the domain types,
   the domain types take their settings and locale from `shared/config` and
   `shared/i18n` (spec §4's deliberate adaptation). `main.rs` is the crate
   root, above every layer. Any path counts, not only `use` lines — one of
   the leaks was a type named in a signature.
2. **Comments are prose.** A `//` or `/* */` mention is skipped: a rustdoc
   link is resolved by rustdoc and by nothing else, and the tree links upward
   in prose already.
3. **Test code stands outside the layer it tests.** The client's tests assert
   its typed errors against `features::compaction::is_context_overflow` — the
   cross-layer contract itself — and the live tests drive the tools. Test
   code is recognized by **shape**, never by a file-name pattern or an
   allowlist: an item under `#[cfg(test)]` (a `mod … { }` block, a `fn`, a
   `use`, a `mod …;` declaration and the file it names, with whatever that
   file declares in turn), a file opening with `#![cfg(test)]`, and every file
   under a `tests/` directory. The extent of a block is found by counting
   braces on a copy of the source with its comments and string, raw-string
   and char literals blanked, so a `}` in a fixture cannot end the block
   early.
4. **The gate cannot go quiet on a missing subject** (`doc_index_check.py`'s
   rule): no Rust sources under `src/`, a layer directory of the table that is
   not there, a directory under `src/` the table does not name, or a scan that
   saw no cross-layer path at all — the matcher is broken or the tree moved —
   each fail rather than pass.

Deliberately *not* verified: `super::` chains (they cannot leave a layer), the
order of modules inside a layer, and what `app/events.rs` chooses to re-export
(its business — it re-exports the lower layers' payload types beside the
events that carry them). The invariant about values rather than shapes — the
orchestrator is the sole owner of `Chat` — stays with the tests that exercise
it.

Exit code is non-zero on any violation, so this doubles as a CI lint.

Usage:
    python tools/layer_check.py             # verdict + every violation
    python tools/layer_check.py --list      # also the files per layer and the test files set aside
    python tools/layer_check.py --root DIR  # another tree (a scratch copy with a planted violation)
    python tools/layer_check.py --self-test # every arm against fixture trees, in both directions
"""

from __future__ import annotations

import re
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterator

SRC_DIR = "src"
# Top to bottom. A file in layer L may name a layer at the same index or a
# higher one; the two at the bottom may also name each other.
LAYERS = ("app", "screens", "widgets", "features", "entities", "shared")
BOTTOM_PAIR = frozenset({"entities", "shared"})
TEST_DIR = "tests"
CRATE_ROOT = "(crate root)"

PATH_RE = re.compile(r"\bcrate::(app|screens|widgets|features|entities|shared)\b")
CFG_TEST = re.compile(r"^\s*#\[cfg\(test\)\](.*)$")
INNER_CFG_TEST = re.compile(r"^\s*#!\[cfg\(test\)\]")
ATTR = re.compile(r"^\s*#!?\[")
MOD_DECL = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([^\W\d]\w*)\s*;")
IDENT_CHAR = re.compile(r"\w")


def repo_root() -> Path:
    """The repository root, found from this script's own location.

    Not the working directory: the gate has to give the same answer whether it
    is run from the root, from `tools/`, or by a CI step that cd's elsewhere.
    """
    return Path(__file__).resolve().parents[1]


# ---------------------------------------------------------------------------
# Blanking comments and literals, so braces and paths are read from code only.
# Every blanked character becomes a space and newlines stay, so line numbers
# and column positions are those of the source.
# ---------------------------------------------------------------------------


def _blank(s: str) -> str:
    return "".join("\n" if ch == "\n" else " " for ch in s)


def _skip_line_comment(text: str, i: int) -> int:
    j = text.find("\n", i)
    return len(text) if j < 0 else j


def _skip_block_comment(text: str, i: int) -> int:
    """`/* */`, nestable as in Rust; an unterminated one runs to the end."""
    depth, j, n = 1, i + 2, len(text)
    while j < n and depth:
        if text.startswith("/*", j):
            depth, j = depth + 1, j + 2
        elif text.startswith("*/", j):
            depth, j = depth - 1, j + 2
        else:
            j += 1
    return j


def _skip_string(text: str, i: int) -> int:
    """A `"…"` or `b"…"` literal from its opening quote; escapes honoured."""
    j, n = text.index('"', i) + 1, len(text)
    while j < n and text[j] != '"':
        j += 2 if text[j] == "\\" else 1
    return min(j + 1, n)


def _raw_string_end(text: str, i: int) -> int | None:
    """The end of an `r"…"`, `r#"…"#` or `br"…"` literal at `i`, else None.

    `r#type` is a raw identifier, not a string: after the hashes there must be
    a quote.
    """
    j = i + (2 if text.startswith("br", i) else 1)
    hashes = 0
    while j < len(text) and text[j] == "#":
        hashes, j = hashes + 1, j + 1
    if j >= len(text) or text[j] != '"':
        return None
    close = '"' + "#" * hashes
    k = text.find(close, j + 1)
    return len(text) if k < 0 else k + len(close)


def _char_literal_end(text: str, i: int) -> int | None:
    """The end of a `'x'`, `'\\n'`, `'\\u{..}'` or `b'x'` literal at `i`, else None.

    `'a` with no closing quote is a lifetime or a label and is left alone.
    """
    j = i + (2 if text[i] == "b" else 1)
    n = len(text)
    if j >= n:
        return None
    if text[j] != "\\":
        return j + 2 if j + 1 < n and text[j + 1] == "'" and text[j] != "\n" else None
    # An escape: `'\''` closes right after it, the others at the next quote.
    k = j + 2 if j + 1 < n and text[j + 1] == "'" else text.find("'", j + 2)
    if k < 0 or k - j > 12 or "\n" in text[j:k]:
        return None
    return k + 1


def _prefixed(text: str, i: int) -> bool:
    """Is the `r`/`b` at `i` the start of a token (not the tail of `bar`)?"""
    return i == 0 or not IDENT_CHAR.match(text[i - 1])


def _skip_at(text: str, i: int) -> int | None:
    """The end of the comment or literal that starts at `i`, or None for code."""
    c = text[i]
    if c == "/" and text.startswith("//", i):
        return _skip_line_comment(text, i)
    if c == "/" and text.startswith("/*", i):
        return _skip_block_comment(text, i)
    if c == '"':
        return _skip_string(text, i)
    if c == "'":
        return _char_literal_end(text, i)
    if c not in "rb" or not _prefixed(text, i):
        return None
    if text.startswith('b"', i):
        return _skip_string(text, i)
    if text.startswith("b'", i):
        return _char_literal_end(text, i)
    return _raw_string_end(text, i)


def mask(text: str) -> str:
    """The source with comments and string/char literals blanked."""
    out: list[str] = []
    i, n = 0, len(text)
    while i < n:
        end = _skip_at(text, i)
        if end is None:
            out.append(text[i])
            i += 1
        else:
            out.append(_blank(text[i:end]))
            i = end
    return "".join(out)


# ---------------------------------------------------------------------------
# Test code by shape.
# ---------------------------------------------------------------------------


def _item_extent(lines: list[str], k: int) -> tuple[int, bool]:
    """The last line of the item starting at line `k`, and whether it opened a block.

    An item ends when its braces close, or — for one that never opens any,
    a `use`, a `const`, a `mod x;` — at its first `;`.
    """
    depth, opened = 0, False
    for e in range(k, len(lines)):
        for ch in lines[e]:
            if ch == "{":
                depth, opened = depth + 1, True
            elif ch == "}":
                depth -= 1
        if opened and depth <= 0:
            return e, True
        if not opened and ";" in lines[e]:
            return e, False
    return len(lines) - 1, opened


def _item_start(lines: list[str], i: int, same_line: str, test: set[int]) -> tuple[int, str]:
    """Where the item under the `#[cfg(test)]` at line `i` begins, and its first line's text.

    The item is on the attribute's own line when text follows it there;
    otherwise further attributes and blank (or comment-only) lines belong to
    it, and the item is the first line after them.
    """
    if same_line.strip():
        return i, same_line
    k = i + 1
    while k < len(lines) and (not lines[k].strip() or ATTR.match(lines[k])):
        test.add(k)
        k += 1
    return k, lines[k] if k < len(lines) else ""


def test_lines(lines: list[str]) -> tuple[set[int], list[str]]:
    """Which lines (0-based) of a masked file are test code, and which `mod x;` they declare."""
    test: set[int] = set()
    declared: list[str] = []
    i, n = 0, len(lines)
    while i < n:
        m = CFG_TEST.match(lines[i])
        if not m:
            i += 1
            continue
        test.add(i)
        k, first = _item_start(lines, i, m.group(1), test)
        if k >= n:
            break
        e, opened = _item_extent(lines, k)
        test.update(range(k, e + 1))
        decl = MOD_DECL.match(first)
        if decl and not opened:
            declared.append(decl.group(1))
        i = e + 1
    return test, declared


def resolve_mod(file: Path, name: str) -> Path | None:
    """The file a `mod name;` in `file` refers to, by the 2018 path rules."""
    base = file.parent if file.name in ("mod.rs", "main.rs", "lib.rs") else file.parent / file.stem
    for cand in (base / f"{name}.rs", base / name / "mod.rs"):
        if cand.is_file():
            return cand
    return None


# ---------------------------------------------------------------------------
# The check.
# ---------------------------------------------------------------------------


@dataclass
class Report:
    violations: list[tuple[str, str]] = field(default_factory=list)
    files_by_layer: dict[str, list[str]] = field(default_factory=dict)
    test_files: list[str] = field(default_factory=list)
    cross_layer_paths: int = 0
    scanned: int = 0


def layer_of(rel: Path) -> str | None:
    """`app`…`shared` for a file under one of the layer directories, `None` at the crate root."""
    return rel.parts[0] if len(rel.parts) > 1 else None


def allowed(src: str | None, dst: str) -> bool:
    if src is None:  # the crate root sees everything
        return True
    return LAYERS.index(dst) >= LAYERS.index(src) or {src, dst} <= BOTTOM_PAIR


class Tree:
    """One `src/` tree: its files, their masked lines, which are test code."""

    def __init__(self, src: Path) -> None:
        self.src = src
        self.files = sorted(p for p in src.rglob("*.rs") if p.is_file())
        self.masked: dict[Path, list[str]] = {
            p: mask(p.read_text(encoding="utf-8")).split("\n") for p in self.files
        }
        self.test_lines: dict[Path, set[int]] = {}
        self.test_files: set[Path] = set()
        declared: list[Path] = []
        for p, lines in self.masked.items():
            self.test_lines[p], names = test_lines(lines)
            declared.extend(f for name in names if (f := resolve_mod(p, name)))
            if TEST_DIR in p.relative_to(src).parts[:-1] or any(INNER_CFG_TEST.match(l) for l in lines):
                self.test_files.add(p)
        self._close_over(declared)

    def _close_over(self, pending: list[Path]) -> None:
        """A `#[cfg(test)] mod x;` names a file; whatever that file declares is
        test code too, so follow the declarations to a fixpoint."""
        while pending:
            f = pending.pop()
            if f in self.test_files:
                continue
            self.test_files.add(f)
            for line in self.masked.get(f, []):
                decl = MOD_DECL.match(line)
                if decl and (sub := resolve_mod(f, decl.group(1))):
                    pending.append(sub)

    def production_lines(self, p: Path) -> Iterator[tuple[int, str]]:
        if p in self.test_files:
            return
        skip = self.test_lines[p]
        for idx, line in enumerate(self.masked[p]):
            if idx not in skip:
                yield idx, line


def _missing_subject(src: Path) -> list[tuple[str, str]]:
    """The arms that fail rather than pass when there is nothing to check."""
    found: list[tuple[str, str]] = []
    for layer in LAYERS:
        if not (src / layer).is_dir():
            found.append((f"{SRC_DIR}/{layer}", "a layer of the table is not there — if it moved, LAYERS moves with it"))
    for child in sorted(src.iterdir()):
        if child.is_dir() and child.name not in LAYERS:
            found.append(
                (
                    f"{SRC_DIR}/{child.name}",
                    "a directory the layer table does not name — a new layer is added to "
                    "LAYERS, in its place in the order, before anything imports it",
                )
            )
    return found


def _scan_file(tree: Tree, p: Path, report: Report) -> None:
    rel = p.relative_to(tree.src)
    layer = layer_of(rel)
    if layer is not None and layer not in LAYERS:
        return  # already reported as a directory the table does not name
    report.files_by_layer.setdefault(layer or CRATE_ROOT, []).append(rel.as_posix())
    raw: list[str] | None = None
    for idx, line in tree.production_lines(p):
        for m in PATH_RE.finditer(line):
            dst = m.group(1)
            report.cross_layer_paths += dst != layer
            if allowed(layer, dst):
                continue
            raw = raw if raw is not None else p.read_text(encoding="utf-8").split("\n")
            report.violations.append(
                (
                    f"{SRC_DIR}/{rel.as_posix()}:{idx + 1}",
                    f"`{layer}` names `crate::{dst}` — a layer above it: {raw[idx].strip()}",
                )
            )


def check(root: Path) -> Report:
    report = Report()
    src = root / SRC_DIR
    if not src.is_dir() or not any(src.rglob("*.rs")):
        report.violations.append(
            (
                SRC_DIR,
                "no Rust sources found — this gate fails rather than passes on a "
                "missing subject, so it cannot go quiet the day the tree moves",
            )
        )
        return report
    report.violations.extend(_missing_subject(src))
    tree = Tree(src)
    report.scanned = len(tree.files)
    report.test_files = sorted(p.relative_to(root).as_posix() for p in tree.test_files)
    for p in tree.files:
        _scan_file(tree, p, report)
    if report.cross_layer_paths == 0:
        report.violations.append(
            (
                SRC_DIR,
                "no cross-layer path was seen at all — the matcher is broken or the "
                "tree moved; a gate that sees nothing must not pass",
            )
        )
    return report


def print_report(report: Report, list_mode: bool) -> int:
    if list_mode:
        for layer in (CRATE_ROOT, *LAYERS):
            files = report.files_by_layer.get(layer, [])
            print(f"{layer}: {len(files)} file(s)")
            for rel in files:
                print(f"       {rel}")
        print(f"\n{len(report.test_files)} test file(s) set aside:")
        for rel in report.test_files:
            print(f"       {rel}")
        print()

    if not report.violations:
        print(
            f"clean: {report.scanned} .rs files scanned, {len(report.test_files)} of them test "
            f"files; {report.cross_layer_paths} cross-layer path(s), every one downward"
        )
        return 0

    print(f"FSD layers: {len(report.violations)} violation(s)\n")
    for where, what in report.violations:
        print(f"  {where}\n       {what}")
    print(
        "\nDependencies point downward: main.rs -> app -> screens -> widgets -> features ->\n"
        "{ entities <-> shared }. A type a screen needs from the orchestrator is defined\n"
        "below both — `entities` for data, `features` for a scenario's own progress — and\n"
        "`app/events.rs` re-exports it beside the event that carries it. Test code is\n"
        "exempt: a `#[cfg(test)]` item or a file under `tests/`. See docs/architecture.md §2."
    )
    return 1


# ---------------------------------------------------------------------------
# Self-test: every arm against fixture trees, in both directions.
# ---------------------------------------------------------------------------

MAIN = "main.rs"
CHAT = "screens/chat.rs"
WIDGETS = "widgets/mod.rs"
FEATURES = "features/mod.rs"
ENTITIES = "entities/mod.rs"
SHARED = "shared/mod.rs"
EMPTY_FN = "pub fn x() {}\n"

# A legal tree: every layer names the one below it, the bottom pair names
# each other, and the crate root names `app`.
BASE = {
    MAIN: "mod app; mod screens; mod widgets; mod features; mod entities; mod shared;\n"
    "fn main() { crate::app::run(); }\n",
    "app/mod.rs": "pub fn run() { crate::screens::draw(); crate::shared::s(); }\n",
    "screens/mod.rs": "pub mod chat;\npub fn draw() { crate::widgets::w(); }\n",
    CHAT: "pub fn chat() { crate::features::f(); crate::screens::draw(); }\n",
    WIDGETS: "pub fn w() { crate::features::f(); }\n",
    FEATURES: "pub fn f() { crate::entities::e(); }\n",
    ENTITIES: "pub fn e() { crate::shared::s(); }\n",
    SHARED: "pub fn s() { let _ = crate::entities::e; }\n",
}

LEAK = "use crate::app::events::TaskList;\n"
TEST_MOD_OPEN = "#[cfg(test)]\nmod tests {\n"


def _case(name: str, files: dict[str, str | None], red: str | None):
    """A self-test arm: `files` overlay BASE (None deletes); `red` names the file the
    one violation must cite, or None when the tree must be clean. Exactly one, so
    a leak planted inside a test block beside one planted after it cannot pass as
    the latter."""
    return name, files, red


def _chat_with(tail: str) -> dict[str, str]:
    return {CHAT: BASE[CHAT] + tail}


CASES = [
    _case("a clean tree is clean", {}, None),
    _case("a screen importing app is red", {CHAT: LEAK + BASE[CHAT]}, CHAT),
    _case("the same line in a comment is prose", {CHAT: "// " + LEAK + BASE[CHAT]}, None),
    _case("the same line in a block comment is prose", {CHAT: "/* " + LEAK + " */\n" + BASE[CHAT]}, None),
    _case("inside #[cfg(test)] mod tests { } is test code", _chat_with(TEST_MOD_OPEN + "    " + LEAK + "}\n"), None),
    _case(
        "after the test module's closing brace is production again",
        _chat_with(TEST_MOD_OPEN + "    fn t() {}\n}\n" + LEAK),
        CHAT,
    ),
    _case(
        "a `}` inside a string literal does not end the test module",
        _chat_with(
            TEST_MOD_OPEN + '    const S: &str = "}";\n    const R: &str = r#"}"#;\n'
            "    const C: char = '}';\n    " + LEAK + "}\n" + LEAK
        ),
        CHAT,
    ),
    _case(
        "a `{` inside a string literal does not hold the test module open",
        _chat_with(TEST_MOD_OPEN + '    const S: &str = "{{";\n}\n' + LEAK),
        CHAT,
    ),
    _case("a #[cfg(test)] use line is test code", {CHAT: "#[cfg(test)]\n" + LEAK + BASE[CHAT]}, None),
    _case(
        "a #[cfg(test)] fn is test code, and the line after it is not",
        _chat_with("#[cfg(test)]\npub fn fixture() -> usize {\n    let _ = crate::app::events::TASK_LANDED_CAP;\n    1\n}\n" + LEAK),
        CHAT,
    ),
    _case(
        "attributes between #[cfg(test)] and the item belong to it",
        _chat_with("#[cfg(test)]\n#[allow(dead_code)]\n/// doc\nmod tests {\n    " + LEAK + "}\n"),
        None,
    ),
    _case(
        "a #[cfg(test)] mod x; file is test code",
        {CHAT: BASE[CHAT] + "#[cfg(test)]\nmod fixtures;\n", "screens/chat/fixtures.rs": LEAK},
        None,
    ),
    _case(
        "what a test file declares is test code too",
        {
            CHAT: BASE[CHAT] + "#[cfg(test)]\nmod fixtures;\n",
            "screens/chat/fixtures.rs": "mod more;\n",
            "screens/chat/fixtures/more.rs": LEAK,
        },
        None,
    ),
    _case(
        "a mod x; without #[cfg(test)] is production",
        {CHAT: BASE[CHAT] + "mod helpers;\n", "screens/chat/helpers.rs": LEAK},
        "screens/chat/helpers.rs",
    ),
    _case("a file under tests/ is test code", {"screens/chat/tests/draw.rs": LEAK}, None),
    _case("a file opening with #![cfg(test)] is test code", {"screens/probe.rs": "#![cfg(test)]\n" + LEAK}, None),
    _case(
        "a path in a signature counts, not only a use line",
        {WIDGETS: "pub fn w(_s: &crate::screens::chat::State) { crate::features::f(); }\n"},
        WIDGETS,
    ),
    _case("features naming widgets is red", {FEATURES: "pub fn f() { crate::widgets::w(); }\n"}, FEATURES),
    _case("shared naming features is red", {SHARED: "pub fn s() { crate::features::f(); }\n"}, SHARED),
    _case("entities naming features is red", {ENTITIES: "pub fn e() { crate::features::f(); }\n"}, ENTITIES),
    _case(
        "shared and entities see each other",
        {"shared/x.rs": "pub fn x() { crate::entities::e(); }\n", "entities/y.rs": "pub fn y() { crate::shared::s(); }\n"},
        None,
    ),
    _case("a layer naming itself is fine", {"screens/other.rs": "pub fn o() { crate::screens::draw(); }\n"}, None),
    _case("main.rs names anything", {MAIN: BASE[MAIN] + "fn x() { crate::shared::s(); }\n"}, None),
    _case("no src/ at all is red", dict.fromkeys(BASE), SRC_DIR),
    _case("a missing layer directory is red", {WIDGETS: None}, f"{SRC_DIR}/widgets"),
    _case("a directory the table does not name is red", {"extra/mod.rs": EMPTY_FN}, f"{SRC_DIR}/extra"),
    _case(
        "a scan that sees no cross-layer path is red",
        dict.fromkeys(BASE, EMPTY_FN) | {MAIN: "fn main() {}\n"},
        SRC_DIR,
    ),
]


def _write_tree(root: Path, files: dict[str, str | None]) -> None:
    for rel, content in files.items():
        p = root / SRC_DIR / rel
        if content is None:
            if p.is_file():
                p.unlink()
            while p.parent != root / SRC_DIR and not any(p.parent.iterdir()):
                p.parent.rmdir()
                p = p.parent
            continue
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content, encoding="utf-8")


def _run_case(name: str, files: dict[str, str | None], red: str | None) -> str | None:
    """The failure of one arm, or None when it behaved."""
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        _write_tree(root, BASE)
        _write_tree(root, files)
        cited = [where for where, _ in check(root).violations]
    if red is None:
        return f"{name}: expected clean, got {cited}" if cited else None
    if len(cited) == 1 and (cited[0].startswith(f"{SRC_DIR}/{red}") or cited[0] == red):
        return None
    return f"{name}: expected one violation citing {red}, got {cited}"


def self_test() -> int:
    failures = [f for f in (_run_case(*case) for case in CASES) if f]
    for f in failures:
        print(f"self-test FAILED: {f}")
    if failures:
        return 1
    print(f"self-test: {len(CASES)} arms OK, both directions")
    return 0


def main() -> int:
    args = sys.argv[1:]
    if "--self-test" in args:
        return self_test()
    root = repo_root()
    if "--root" in args:
        root = Path(args[args.index("--root") + 1]).resolve()
    return print_report(check(root), "--list" in args)


if __name__ == "__main__":
    # The report carries em-dashes and section signs. On Windows the console
    # defaults to cp1252, where printing them raises UnicodeEncodeError and kills
    # the run exactly when the report is needed — the trap `cyrillic_scan.py` hit.
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())
