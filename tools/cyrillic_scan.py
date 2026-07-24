#!/usr/bin/env python3
"""Cyrillic-content guard for the RU -> EN source-language migration.

Reports every tracked file that still contains Russian text that should have
been translated, applying an allowlist for content that legitimately stays
Russian (the `ru` locale bundle, Hunspell dictionaries, in-test assertion
strings, intentional `[ru]` desktop/installer localization, binary blobs).

Exit code is non-zero when non-allowlisted Cyrillic remains, so this doubles as
the migration acceptance gate and (post-migration) a CI lint that prevents
Russian from creeping back into comments/docs.

Usage:
    python tools/cyrillic_scan.py            # summary + exit code
    python tools/cyrillic_scan.py --list     # every offending line
"""
from __future__ import annotations
import subprocess, re, sys, os, collections

CYR = re.compile(r"[Ѐ-ӿ]")
TEST_MARKER = re.compile(r"#\[cfg\(test\)\]|mod tests|#\[test\]|#\[tokio::test")

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

# Whole files that legitimately keep Cyrillic and are skipped entirely.
SKIP_EXT = (".png", ".ico", ".dic", ".aff")
SKIP_FILES = {
    "locales/ru.json",
    # The migration glossary is a RU -> EN mapping table: the Russian column is
    # its content. Removed (or demoted to a terminology note) once this lands.
    "tools/glossary-ru-en.md",
    # This detector necessarily spells out Cyrillic Unicode ranges.
    "tools/cyrillic_scan.py",
    # This doc is *about* Cyrillic Mermaid rendering: its repro inputs (table
    # rows and a fenced diagram) must stay Cyrillic to demonstrate a
    # byte-offset-vs-char-offset bug that ASCII would not trigger. Inline
    # markers cannot be used there without breaking the table/code fence.
    "docs/research/mermaid-ascii-rendering.md",
}

# Opt-in markers for content that must stay Cyrillic because the text *is* the
# subject (e.g. a parsing-bug repro that only triggers on multibyte input, or
# Russian homographs used to evaluate speech stress). Markers are HTML comments,
# so they are invisible in rendered Markdown and keep the intent next to the
# content instead of hidden in this script.
OK_LINE = "cyrillic-ok"           # same-line marker
OK_START = "cyrillic-ok:start"    # begin an allowed block
OK_END = "cyrillic-ok:end"        # end an allowed block
SKIP_PREFIX = ("dictionaries/", "docs/ui-design/")  # design mocks/uploads, not prose


def is_skipped(path: str) -> bool:
    if path in SKIP_FILES or path.startswith(SKIP_PREFIX):
        return True
    return path.endswith(SKIP_EXT)


def allowed_line(path: str, line: str, in_test: bool) -> bool:
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


def main() -> int:
    list_mode = "--list" in sys.argv
    files = [f for f in subprocess.check_output(["git", "ls-files"]).decode().split("\n") if f]
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
        # A dedicated test module (`foo/tests.rs`, or anything under `tests/`)
        # is test code end to end: its `#[cfg(test)]` marker sits in the parent
        # `mod.rs`, not in the file itself.
        in_test = path.endswith("tests.rs") or "/tests/" in path
        ok_block = False
        hits: list[tuple[int, str]] = []
        for i, line in enumerate(data.split("\n"), 1):
            if path.endswith(".rs") and TEST_MARKER.search(line):
                in_test = True
            if OK_START in line:
                ok_block = True
            elif OK_END in line:
                ok_block = False
            if not CYR.search(line):
                continue
            if ok_block or OK_LINE in line:
                continue
            if allowed_line(path, line, in_test):
                continue
            hits.append((i, line.strip()[:100]))
        if hits:
            offenders[path] = hits
            total += len(hits)

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


if __name__ == "__main__":
    sys.exit(main())
