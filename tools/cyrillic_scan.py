#!/usr/bin/env python3
"""Cyrillic-content guard for the RU -> EN source-language migration.

Reports every tracked file that still contains Russian text that should have
been translated, applying an allowlist for content that legitimately stays
Russian (the `ru` locale bundle, Hunspell dictionaries, in-test fixture and
assertion strings, intentional `[ru]` desktop/installer localization, binary
blobs).

Test code is allowlisted wholesale, because it legitimately holds Cyrillic
fixture data and ru-locale assertions and the scanner cannot tell those from
prose *by file*. It can, however, tell them **by position**: a print-macro
format string is the developer-facing test log, which AGENTS.md §3 says is
English. So that one position is carved back out of the test allowlist — see
`print_fmt_spans`. Values are exempt by construction: interpolate them
(`eprintln!("... {SOURCE:?}", ...)`) instead of spelling them into the label.

Test code is known **by shape**, with the layer gate's own code
(`layer_check.py`). A file under a `tests/` directory, named `*tests.rs`,
declared by a `#[cfg(test)] mod x;` or opening with `#![cfg(test)]` is test code
end to end; in any other file, test code is the one item a `#[cfg(test)]` stands
on, up to its closing brace or its `;`. The first marker in a file used to make
the rest of the file test code, which is how two production strings below a
`#[cfg(test)]` accessor stayed Russian through the whole migration.

Exit code is non-zero when non-allowlisted Cyrillic remains, so this doubles as
the migration acceptance gate and (post-migration) a CI lint that prevents
Russian from creeping back into comments/docs.

Usage:
    python tools/cyrillic_scan.py              # summary + exit code
    python tools/cyrillic_scan.py --list       # every offending line
    python tools/cyrillic_scan.py --self-test  # the test-code arms, against fixtures
"""
from __future__ import annotations
import subprocess, re, sys, os, collections, tempfile
from pathlib import Path

# The Rust shape — blanking literals and comments, an item's extent, which
# files a `#[cfg(test)] mod x;` makes test code — has one implementation, the
# layer gate's. Its directory is put on the path by hand, so the import holds
# however the script is started.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import layer_check  # noqa: E402

CYR = re.compile(r"[Ѐ-ӿ]")
SRC_DIR = "src"

# Print macros whose format string is developer-facing log output. Deliberately
# only these four:
#   * `assert!`/`panic!` are excluded because the *condition* sits in the same
#     macro call as the message (`assert!(r.contains("<ru string>"), "msg")`) —
#     flagging them would hit exactly the ru-locale assertion data that
#     legitimately stays;
#   * `write!`/`writeln!` are excluded because in tests they usually build an
#     expected-value buffer, which is fixture data, not a log.
PRINT_MACRO = re.compile(r"\b(?:e?println|e?print)!\s*\(")

# Physical-key data (always kept): a modifier + a single Cyrillic letter (the
# char crossterm reports under a Cyrillic layout, e.g. `Ctrl+и`), and single
# Cyrillic char literals / backtick spans (the JCUKEN key-mapping table in
# keys.rs). These carry no prose, so stripping them cannot mask an untranslated
# sentence (a Russian word has 2+ letters and survives the strip).
_KEY_MOD = re.compile(r"(?i)(Ctrl|Alt|Shift|Cmd|Win|Super|Meta)\+[Ѐ-ӿ]")
_KEY_LIT = re.compile(r"['`][Ѐ-ӿ]['`]")


def strip_key_data(s: str) -> str:
    return _KEY_LIT.sub("", _KEY_MOD.sub("", s))


def code_part(line: str) -> str:
    """The line up to a real `//` comment, ignoring `//` inside string literals.

    Splitting naively on the first `//` misreads protocol-relative URLs in test
    fixtures (`href="//example.com/..."`) as a comment start, which would flag
    the fixture text that follows.
    """
    in_str = esc = False
    for i, c in enumerate(line):
        if esc:
            esc = False
        elif c == "\\":
            esc = True
        elif c == '"':
            in_str = not in_str
        elif not in_str and c == "/" and line[i + 1 : i + 2] == "/":
            return line[:i]
    return line

def _find_macro(line: str, pos: int, limit: int) -> tuple[int | None, str | None]:
    """None-state step: advance to just past the next print macro's `(`."""
    m = PRINT_MACRO.search(line, pos, limit)
    return (m.end(), "seek") if m else (None, None)


def _open_quote(line: str, pos: int, limit: int) -> tuple[int | None, str | None, int]:
    """Seek-state step: skip whitespace, then enter the literal at a `"`.

    Running out of line means the format string opens on a later line (state
    stays "seek"); anything that is not a plain `"` literal (a raw string, a
    variable) makes the tracker bail out rather than guess.
    """
    while pos < limit and line[pos].isspace():
        pos += 1
    if pos >= limit:
        return None, "seek", 0
    if line[pos] == '"':
        return pos + 1, "in", pos + 1
    return pos, None, 0


def _close_literal(line, pos, start, spans) -> tuple[int | None, str | None]:
    """In-state step: record the span up to the unescaped closing quote."""
    j, esc = pos, False
    while j < len(line):
        c = line[j]
        if esc:
            esc = False
        elif c == "\\":
            esc = True
        elif c == '"':
            spans.append((start, j))
            return j + 1, None
        j += 1
    spans.append((start, len(line)))  # continues on the next line
    return None, "in"


def print_fmt_spans(line: str, state: str | None) -> tuple[list[tuple[int, int]], str | None]:
    """Character spans of `line` that sit inside a print-macro format string.

    A small cross-line state machine, because the format string routinely opens
    on the `eprintln!(` line and continues over several `\\`-continuation lines.
    `state` is threaded by the caller from line to line:

        None    outside any print macro
        "seek"  the macro's `(` was seen, looking for the opening quote
        "in"    inside the format string literal

    Only the **first** literal is tracked: once it closes we stop, so the
    macro's value arguments (which may legitimately mention ru-locale data,
    e.g. `eprintln!("gate: {}", r.contains("<ru string>"))`) are not covered.
    Each state's step lives in its own helper; a step returning `None` for the
    position ends the line, and the state it returns carries over to the next.
    """
    spans: list[tuple[int, int]] = []
    # Outside a string literal `//` starts a comment; inside one it is text.
    limit = len(line) if state == "in" else len(code_part(line))
    pos: int | None = 0
    start = 0  # a literal continued from the previous line starts at column 0
    while pos is not None:
        if state is None:
            pos, state = _find_macro(line, pos, limit)
        elif state == "seek":
            pos, state, start = _open_quote(line, pos, limit)
        else:
            pos, state = _close_literal(line, pos, start, spans)
    return spans, state


# Whole files that legitimately keep Cyrillic and are skipped entirely.
SKIP_EXT = (".png", ".ico", ".dic", ".aff", ".woff2")
SKIP_FILES = {
    "locales/ru.json",
    # The migration glossary is a RU -> EN mapping table: the Russian column is
    # its content. Archived with the migration design doc; kept as the term
    # reference for anyone writing English docs/comments after the switch.
    "docs/history/glossary-ru-en.md",
    # This detector necessarily spells out Cyrillic Unicode ranges.
    "tools/cyrillic_scan.py",
    # A fixed measurement fixture, not prose to translate: the embedding
    # calibration probes are deliberately bilingual, because the similarity
    # range they measure is the one the gates operate in on a bilingual corpus.
    # Editing it (translating a probe included) silently invalidates the
    # reference constants in shared/embed_calibration.rs and therefore every
    # threshold derived from them.
    "src/shared/embed_probes.json",
    # This doc is *about* Cyrillic Mermaid rendering: its repro inputs (table
    # rows and a fenced diagram) must stay Cyrillic to demonstrate a
    # byte-offset-vs-char-offset bug that ASCII would not trigger. Inline
    # markers cannot be used there without breaking the table/code fence.
    "docs/research/mermaid-ascii-rendering.md",
    # The Russian legal texts: translations of LICENSE, DISCLAIMER.md and
    # PRIVACY.md, shown on the app's F1 tabs and on the ru installer's wizard pages
    # (docs/history/legal-ru-translations.md). Russian *is* their content — the same
    # reason locales/ru.json is here. The RTFs generated from them for the
    # wizard need no entry: tools/wizard_rtf.py escapes every non-ASCII
    # character, so those files are pure ASCII.
    "docs/legal/LICENSE.ru.txt",
    "docs/legal/DISCLAIMER.ru.md",
    "docs/legal/PRIVACY.ru.md",
    # A golden chat file as a Russian-locale install wrote it before the first
    # chat-file migration step (ADR 0006): the step's fixture, pinned by tests
    # that assert on its Cyrillic titles and replies. Translating it would test a
    # file nobody ever had.
    "src/shared/storage/fixtures/chat_v1_call_subagent.json",
}

# Opt-in markers for content that must stay Cyrillic because the text *is* the
# subject (e.g. a parsing-bug repro that only triggers on multibyte input, or
# Russian homographs used to evaluate speech stress). Markers are HTML comments,
# so they are invisible in rendered Markdown and keep the intent next to the
# content instead of hidden in this script.
#
# Note for a print-macro format string that must keep Cyrillic: a same-line
# `// cyrillic-ok` works only on the line that *opens* the literal, since a
# continuation line sits inside the string and cannot carry a comment. There,
# either wrap the call in the block markers or — usually better — bind the
# value to a `const` and interpolate it, which keeps the label English.
OK_LINE = "cyrillic-ok"           # same-line marker
OK_START = "cyrillic-ok:start"    # begin an allowed block
OK_END = "cyrillic-ok:end"        # end an allowed block
SKIP_PREFIX = ("dictionaries/", "docs/ui-design/")  # design mocks/uploads, not prose


def is_skipped(path: str) -> bool:
    if path in SKIP_FILES or path.startswith(SKIP_PREFIX):
        return True
    return path.endswith(SKIP_EXT)


def allowed_line(path: str, line: str, in_test: bool, in_print_fmt: bool = False) -> bool:
    """True when this Cyrillic line is intentionally kept."""
    # Physical-key data (modifier+Cyrillic, single-char literals) always stays.
    if not CYR.search(strip_key_data(line)):
        return True
    stripped = line.lstrip()
    if path.endswith(".rs"):
        # Comments always translate, wherever they sit.
        if stripped.startswith("//"):
            return False
        code = code_part(line)
        if not CYR.search(code):
            return False  # Cyrillic only in a trailing comment -> translate
        # A print-macro format string is the developer-facing test log, which
        # the convention says is English (AGENTS.md §3) — so it translates even
        # inside tests, where the blanket allowance below would otherwise cover
        # it. This is the one thing the wholesale test allowlist got wrong: the
        # scanner cannot tell a label from a fixture by file, but it can by
        # position. Values stay data — interpolate them (`{SOURCE:?}`) instead
        # of spelling them into the label.
        if in_print_fmt:
            return False
        # Cyrillic in code position: inside tests that is fixture/assertion data
        # and stays (Rust has no Cyrillic identifiers here). This deliberately
        # covers multi-line string literals, whose opening quote sits on an
        # earlier line. Outside tests it is a production string -> Phase 5.
        return in_test
    if path.endswith(".desktop"):
        # Localized keys such as `Comment[ru]=...` are the ru UI translation.
        return bool(re.match(r"[A-Za-z]+\[ru\]\s*=", stripped))
    if path.endswith(".iss"):
        # `ru.Msg=...` CustomMessages and the Russian.isl reference are the ru
        # installer UI; comments (`;`) still translate.
        if stripped.startswith(";"):
            return False
        return stripped.startswith("ru.") or "Russian.isl" in line
    return False


def _line_flagged(path: str, line: str, in_test: bool, spans, ok: bool) -> bool:
    """True when this line carries Cyrillic that should be reported."""
    if not CYR.search(line) or ok:
        return False
    in_print_fmt = any(CYR.search(line[s:e]) for s, e in spans)
    return not allowed_line(path, line, in_test, in_print_fmt)


def rust_test_lines(data: str) -> set[int]:
    """The 0-based lines of a Rust file that belong to a `#[cfg(test)]` item."""
    test, _declared = layer_check.test_lines(layer_check.mask(data).split("\n"))
    return test


def declared_test_files(src: Path) -> set[str]:
    """Rust files that are test code end to end: under a `tests/` directory,
    opening with `#![cfg(test)]`, or declared by a `#[cfg(test)] mod x;` (and
    whatever those declare), as `src/...` paths."""
    if not src.is_dir():
        return set()
    return {p.as_posix() for p in layer_check.Tree(src).test_files}


def scan_file(path: str, data: str, test_file: bool = False) -> list[tuple[int, str]]:
    """Non-allowlisted Cyrillic lines of one file: [(line_no, text)].

    `test_file` — the file is test code end to end, by its declaration
    ([`declared_test_files`]). A dedicated test module (`foo/tests.rs`, or
    anything under `tests/`) is one by its name as well: its `#[cfg(test)]`
    marker sits in the parent `mod.rs`, not in the file itself.
    """
    is_rust = path.endswith(".rs")
    whole = test_file or path.endswith("tests.rs") or "/tests/" in path
    test_lines = rust_test_lines(data) if is_rust and not whole else set()
    ok_block = False
    fmt_state: str | None = None
    hits: list[tuple[int, str]] = []
    for i, line in enumerate(data.split("\n"), 1):
        # Advance the print-macro tracker on *every* line, before any of the
        # early exits below: a format string that opens on one line and
        # carries the Cyrillic on the next is exactly the case this catches.
        spans: list[tuple[int, int]] = []
        if is_rust:
            spans, fmt_state = print_fmt_spans(line, fmt_state)
        in_test = whole or (i - 1) in test_lines
        if OK_START in line:
            ok_block = True
        elif OK_END in line:
            ok_block = False
        if _line_flagged(path, line, in_test, spans, ok_block or OK_LINE in line):
            hits.append((i, line.strip()[:100]))
    return hits


def report(offenders, total: int, list_mode: bool) -> int:
    if not offenders:
        print("clean: no non-allowlisted Cyrillic remains")
        return 0
    print(f"remaining non-allowlisted Cyrillic: {total} lines in {len(offenders)} files\n")
    for path, hits in sorted(offenders.items(), key=lambda kv: -len(kv[1])):
        print(f"{len(hits):5}  {path}")
        if list_mode:
            for ln, text in hits:
                print(f"        {ln}: {text}")
    return 1


# ---------------------------------------------------------------------------
# Self-test: the test-code rule against fixtures, in both directions.
# ---------------------------------------------------------------------------

# A Russian word, spelt as escapes so the fixtures read the same in any editor;
# a fixture names it `{ru}`.
_RU = "привет"

# (name, path, source, the file is test code by declaration, the lines flagged)
SELF_TEST_CASES = [
    # message_feed.rs's shape: the strings below a test accessor are production.
    ("a production string below a test accessor", "src/feed.rs", """pub struct Feed;
#[cfg(test)]
impl Feed {
    pub fn probe(&self) -> &str { "{ru}" }
}
pub fn label() -> &'static str { "{ru}" }
""", False, [6]),
    ("a test module's fixture", "src/feed.rs", """pub fn x() -> u8 { 1 }
#[cfg(test)]
mod tests {
    #[test]
    fn t() { assert_eq!(super::x().to_string(), "{ru}"); }
}
""", False, []),
    ("a declared test module ends at its `;`", "src/api/mod.rs", """#[cfg(test)] mod probe_tests;
pub const LABEL: &str = "{ru}";
""", False, [2]),
    ("a brace in a string does not close the item", "src/feed.rs", """#[cfg(test)]
mod tests {
    const B: &str = "}";
    const T: &str = "{ru}";
}
const P: &str = "{ru}";
""", False, [6]),
    ("a multi-line literal in a test item", "src/feed.rs", """#[cfg(test)]
const T: &str = "first
{ru}";
const P: &str = "{ru}";
""", False, [4]),
    ("attributes stacked under the marker", "src/feed.rs", """#[cfg(test)]
#[allow(dead_code)]
fn helper() -> &'static str {
    "{ru}"
}
fn shown() -> &'static str {
    "{ru}"
}
""", False, [7]),
    ("a print-macro label inside a test", "src/feed.rs", """#[cfg(test)]
mod tests {
    fn t() { eprintln!("{ru} {}", 1); }
}
""", False, [3]),
    ("a comment inside a test", "src/feed.rs", """#[cfg(test)]
mod tests {
    // {ru}
}
""", False, [3]),
    ("a file declared test code", "src/api/probe.rs", 'const P: &str = "{ru}";\n', True, []),
    ("the same file undeclared", "src/api/probe.rs", 'const P: &str = "{ru}";\n', False, [1]),
    ("a test file by its name", "src/api/cache_probe_tests.rs", 'const P: &str = "{ru}";\n', False, []),
]


def _declaration_arm() -> str | None:
    """A `#[cfg(test)] mod x;` makes `x` test code, and what `x` declares too;
    a plain `mod y;` beside it does not."""
    with tempfile.TemporaryDirectory() as tmp:
        src = Path(tmp) / "src"
        (src / "probe").mkdir(parents=True)
        (src / "main.rs").write_text("#[cfg(test)]\nmod probe;\nmod shown;\n", encoding="utf-8")
        (src / "probe.rs").write_text("mod deeper;\n", encoding="utf-8")
        (src / "probe" / "deeper.rs").write_text("", encoding="utf-8")
        (src / "shown.rs").write_text("", encoding="utf-8")
        got = sorted(p.rsplit("/", 1)[-1] for p in declared_test_files(src))
    want = ["deeper.rs", "probe.rs"]
    return None if got == want else f"declared test files: {got}, expected {want}"


def self_test() -> int:
    failures = []
    for name, path, source, test_file, want in SELF_TEST_CASES:
        got = [ln for ln, _ in scan_file(path, source.replace("{ru}", _RU), test_file)]
        if got != want:
            failures.append(f"{name}: flagged lines {got}, expected {want}")
    if declaration := _declaration_arm():
        failures.append(declaration)
    for f in failures:
        print(f"self-test FAILED: {f}")
    if failures:
        return 1
    print(f"self-test: {len(SELF_TEST_CASES) + 1} arms OK, both directions")
    return 0


def main() -> int:
    if "--self-test" in sys.argv:
        return self_test()
    list_mode = "--list" in sys.argv
    # Tracked files **and** new ones not yet added (`--others`, honouring
    # .gitignore). A plain `git ls-files` sees only tracked files, so a brand-new
    # file's violations stayed invisible locally and first surfaced in CI, after
    # the commit — which is exactly how one slipped through. In CI there are no
    # untracked files, so this changes nothing there.
    files = [
        f
        for f in subprocess.check_output(
            ["git", "ls-files", "--cached", "--others", "--exclude-standard"]
        )
        .decode()
        .split("\n")
        if f
    ]
    test_files = declared_test_files(Path(SRC_DIR))
    offenders: dict[str, list[tuple[int, str]]] = collections.OrderedDict()
    total = 0
    for path in files:
        if is_skipped(path):
            continue
        try:
            data = open(path, encoding="utf-8", errors="replace").read()
        except OSError:
            continue
        if not CYR.search(data):
            continue
        hits = scan_file(path, data, path in test_files)
        if hits:
            offenders[path] = hits
            total += len(hits)
    return report(offenders, total, list_mode)


if __name__ == "__main__":
    # `--list` prints the offending lines, which are Cyrillic by definition. On
    # Windows the console defaults to cp1252, so printing them raised
    # UnicodeEncodeError and killed the run exactly when the report was needed.
    # `errors="replace"` keeps it working even on a console whose font/codepage
    # cannot render Cyrillic: line numbers and paths still point at the offenders.
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())
