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
STRLIT = re.compile(r'"[^"]*[Ѐ-ӿ][^"]*"')
TEST_MARKER = re.compile(r"#\[cfg\(test\)\]|mod tests|#\[test\]|#\[tokio::test")

# Whole files that legitimately keep Cyrillic and are skipped entirely.
SKIP_EXT = (".png", ".ico", ".dic", ".aff")
SKIP_FILES = {"locales/ru.json"}
SKIP_PREFIX = ("dictionaries/", "docs/ui-design/")  # design mocks/uploads, not prose


def is_skipped(path: str) -> bool:
    if path in SKIP_FILES or path.startswith(SKIP_PREFIX):
        return True
    return path.endswith(SKIP_EXT)


def allowed_line(path: str, line: str, in_test: bool) -> bool:
    """True when this Cyrillic line is intentionally kept."""
    stripped = line.lstrip()
    if path.endswith(".rs"):
        # Comments always translate; a Cyrillic *string literal* inside a test
        # module is an assertion on the `ru` locale and stays.
        if stripped.startswith("//"):
            return False
        code = line.split("//", 1)[0]
        if CYR.search(code) and STRLIT.search(code):
            return in_test  # prod strings must be gone; test strings stay
        return False
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
        in_test = False
        hits: list[tuple[int, str]] = []
        for i, line in enumerate(data.split("\n"), 1):
            if path.endswith(".rs") and TEST_MARKER.search(line):
                in_test = True
            if not CYR.search(line):
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
