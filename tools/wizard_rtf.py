#!/usr/bin/env python3
"""Render `DISCLAIMER.md` into the RTF the Windows installer shows on its
"Information" page (`packaging/windows/mindfork.iss`, `InfoBeforeFile`).

Why a generated file at all. Inno Setup's license and info pages take plain
text or RTF and nothing else — the disclaimer is Markdown, and pointed at
directly it would show the user `# Disclaimer`, `**bold**` and
`[accompanying file](LICENSE)` verbatim. Hand-maintaining a second copy of a
legal notice is the other obvious answer, and it is worse: the copy the user
accepts at install time would drift from the one the repository, the release
archives and the app's `F1` tab all show, and nothing would notice. So the
wizard's copy is *derived*, committed next to the `.iss` (the installer
compiles on a machine with no Python), and `--check` runs in CI's `lint` job so
the two can never disagree.

`LICENSE` needs none of this: it is plain ASCII text and the wizard's license
page renders it as-is — which is also the point of keeping that file free of
Markdown (see `credits::license_file_carries_nothing_but_the_mit_text`).

The Markdown subset is the one `DISCLAIMER.md` actually uses: ATX headings,
paragraphs (hard-wrapped in the source, unwrapped here so the wizard's memo can
wrap them to its own width), `-` bullet lists with indented continuation lines,
`---` rules, and inline `**bold**`, `*italic*`, `` `code` `` and
`[links](target)` — a link keeps its text and drops the target, which in a
wizard page cannot be clicked anyway. Anything outside that subset is passed
through as literal text rather than silently dropped, so a new construct shows
up in the output instead of disappearing from a legal notice.

Every non-ASCII character becomes a `\\uNNNN?` escape, so the output is pure
ASCII: no encoding negotiation with the compiler, and no Cyrillic for
`cyrillic_scan.py` to trip over should the source ever gain any.

The text is deliberately **not** translated for the `ru` wizard, matching the
app itself (`shared/credits.rs`): the legal texts are English, only the page
chrome around them is localized (by Inno's own `Russian.isl`).

Usage:
    python tools/wizard_rtf.py            # regenerate the RTF
    python tools/wizard_rtf.py --check    # verify it matches DISCLAIMER.md (CI)
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

# The one source → output pair. A list, because the license page may one day
# want the same treatment, and because a single place to look beats a constant
# named after what it happens to hold today.
PAIRS = [("DISCLAIMER.md", "packaging/windows/disclaimer.rtf")]

# Half-point font sizes: body 9pt, `##` 11pt, `#` 13pt — the wizard's own
# controls are 8pt, so the body stays a touch larger than its surroundings
# without turning a seven-section notice into a scrolling marathon.
BODY_FS, H2_FS, H1_FS = 18, 22, 26

HEADER = (
    r"{\rtf1\ansi\ansicpg1252\deff0\uc1"
    "\n"
    r"{\fonttbl{\f0\fswiss\fcharset0 Segoe UI;}{\f1\fmodern\fcharset0 Consolas;}}"
    "\n"
    r"{\*\comment Generated from DISCLAIMER.md by tools/wizard_rtf.py - do not edit.}"
    "\n"
    rf"\viewkind4\f0\fs{BODY_FS}"
    "\n"
)

# Inline spans, longest-delimiter first so `**bold**` is not read as two
# italics. Code is matched before everything else: a backticked span is
# literal, and `*` inside one is an asterisk, not emphasis.
INLINE = re.compile(
    r"`(?P<code>[^`]+)`"
    r"|\*\*(?P<bold>.+?)\*\*"
    r"|\*(?P<italic>[^*]+)\*"
    r"|\[(?P<link>[^\]]+)\]\([^)]*\)"
)


def escape(text: str) -> str:
    """Plain text → RTF: the three literals that need a backslash, and a
    `\\uNNNN?` escape for everything above ASCII (`\\uc1` in the header is what
    makes the trailing `?` the one-byte fallback a reader may skip)."""
    out = []
    for ch in text:
        if ch in "\\{}":
            out.append("\\" + ch)
        elif ord(ch) < 128:
            out.append(ch)
        else:
            # Astral characters would need a surrogate pair; the notice has
            # none, and a legal text quietly losing a character is not a
            # failure mode worth accepting silently.
            if ord(ch) > 0xFFFF:
                raise ValueError(f"non-BMP character {ch!r} needs a surrogate pair")
            out.append(f"\\u{ord(ch)}?")
    return "".join(out)


def inline(text: str) -> str:
    """Inline Markdown → RTF runs. Control words carry a trailing space as
    their delimiter, which the reader consumes — so the spacing of the source
    text survives untouched."""
    out, pos = [], 0
    for m in INLINE.finditer(text):
        out.append(escape(text[pos : m.start()]))
        if m.group("code") is not None:
            out.append(rf"{{\f1 {escape(m.group('code'))}}}")
        elif m.group("bold") is not None:
            out.append(rf"{{\b {inline(m.group('bold'))}}}")
        elif m.group("italic") is not None:
            out.append(rf"{{\i {inline(m.group('italic'))}}}")
        else:  # a link: keep the text, drop the unclickable target
            out.append(inline(m.group("link")))
        pos = m.end()
    out.append(escape(text[pos:]))
    return "".join(out)


def paragraph(lines: list[str]) -> str:
    """A hard-wrapped source paragraph, unwrapped into one RTF paragraph."""
    return rf"\pard\sa120 {inline(' '.join(lines))}\par" + "\n"


def bullet(lines: list[str]) -> str:
    """A list item: hanging indent, so a wrapped line sits under its own text
    rather than under the marker — the same treatment the app's Markdown
    renderer gives these four-row items (ADR 0003)."""
    return rf"\pard\fi-280\li280\sa80 \bullet\tab {inline(' '.join(lines))}\par" + "\n"


def heading(level: int, text: str) -> str:
    size = H1_FS if level == 1 else H2_FS
    space_before = 0 if level == 1 else 200
    return (
        rf"\pard\sb{space_before}\sa100\b\fs{size} {inline(text)}"
        rf"\b0\fs{BODY_FS}\par" + "\n"
    )


def rule() -> str:
    """`---` → an empty paragraph carrying a bottom border."""
    return r"\pard\sb120\sa120\brdrb\brdrs\brdrw10\brsp40\par" + "\n"


def render(markdown: str) -> str:
    """The Markdown subset above → a complete RTF document."""
    body: list[str] = []
    pending: list[str] = []  # the paragraph or list item being accumulated
    pending_is_bullet = False

    def flush() -> None:
        nonlocal pending, pending_is_bullet
        if pending:
            body.append(bullet(pending) if pending_is_bullet else paragraph(pending))
        pending, pending_is_bullet = [], False

    for raw in markdown.splitlines():
        line = raw.rstrip()
        stripped = line.strip()
        if not stripped:
            flush()
        elif m := re.match(r"(#{1,6})\s+(.*)", stripped):
            flush()
            body.append(heading(len(m.group(1)), m.group(2)))
        elif re.fullmatch(r"-{3,}", stripped):
            flush()
            body.append(rule())
        elif m := re.match(r"[-*]\s+(.*)", stripped):
            flush()
            pending, pending_is_bullet = [m.group(1)], True
        else:
            # A continuation line — of the wrapped paragraph or of the wrapped
            # list item, whichever is open.
            pending.append(stripped)
    flush()
    return HEADER + "".join(body) + "}\n"


def repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


def main() -> int:
    check = "--check" in sys.argv
    root = repo_root()
    stale = []
    for src_rel, out_rel in PAIRS:
        wanted = render((root / src_rel).read_text(encoding="utf-8"))
        out = root / out_rel
        # Compared as text, not bytes: the line endings are the checkout's
        # business (`.gitattributes`), not this gate's.
        current = out.read_text(encoding="utf-8") if out.is_file() else None
        if current == wanted:
            print(f"OK: {out_rel} matches {src_rel}")
            continue
        if check:
            stale.append((src_rel, out_rel))
            continue
        out.write_text(wanted, encoding="utf-8", newline="\n")
        print(f"written: {out_rel} ({len(wanted)} chars from {src_rel})")
    if stale:
        for src_rel, out_rel in stale:
            print(f"STALE: {out_rel} does not match {src_rel}", file=sys.stderr)
        print(
            "\nThe Windows installer shows the disclaimer from the generated RTF, so it\n"
            "would present a different text than the repository, the release archives\n"
            "and the app's F1 tab. Regenerate and commit:\n"
            "    python tools/wizard_rtf.py",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    # The disclaimer carries em-dashes; on Windows the console defaults to
    # cp1252, where printing one raises UnicodeEncodeError and kills the run
    # exactly when the report is needed (the trap `doc_index_check.py` hit).
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())
