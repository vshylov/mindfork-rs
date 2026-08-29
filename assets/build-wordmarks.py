#!/usr/bin/env python3
"""Generator for mindfork wordmarks: text → outlines (a self-contained SVG).

Why: an SVG with `<text>` depends on a font present on the viewer's machine. GitHub, someone
else's browser, and Linux viewers don't have the needed monospace font — the
typeface gets substituted, and the wordmark looks broken. So the glyphs are converted to `<path>`:
the file becomes self-contained and renders the same everywhere.

Font: **JetBrains Mono ExtraBold** (SIL Open Font License 1.1) — determined from the
reference `wordmark-example.png` by fitting metrics (see `assets/README.md §Font`).
The OFL permits using the font to create artwork and distributing the
resulting outlines; the font file itself isn't checked into the repo — it's only needed for
regeneration.

The lockup's geometry (the icon-to-text proportions) was measured from the reference — the constants
`ASC_RATIO`/`GAP_RATIO`/`BASE_RATIO`/`TRACKING` below.

Run:
    python assets/build-wordmarks.py [--font <path to JetBrainsMono-ExtraBold.ttf>]

Without `--font` the font is looked up in standard locations (system fonts, JetBrains
IDE bundles). Overwrites `assets/mindfork-wordmark*.svg`.
"""

from __future__ import annotations

import argparse
import glob
import os
import sys

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

# --- Brand palette (see assets/README.md) ---------------------------------
ORANGE = "#c25a27"  # accent: "fork" and the glyph's stem
GRAY = "#5c6370"  # the glyph's branches
PLATE = "#09090b"  # the icon's backdrop
LIGHT_TEXT = "#e4e4e7"  # "mind" on dark
DARK_TEXT = "#18181b"  # "mind" on light
MUTED = "#71717a"  # the tagline

# --- Icon glyph: 5 rectangles on a 16x16 grid ---------------------------
# The single source of truth is mindfork-icon-transparent.svg; this is a duplicate for
# generation. The widgets/logo.rs test cross-checks the table in the code against the same SVG.
GLYPH = [
    (7, 2, 2, 12, ORANGE),
    (11, 2, 2, 5, GRAY),
    (9, 5, 2, 2, GRAY),
    (3, 7, 2, 5, GRAY),
    (5, 10, 2, 2, GRAY),
]

# --- Lockup geometry, measured from wordmark-example.png --------------------
# The icon in the reference: 388x388 px; the text baseline, ascender height, and
# gap are recomputed as fractions of the icon size, so they scale to any S.
ASC_RATIO = 0.5227  # ascender height ('d','f','k') / icon size
GAP_RATIO = 0.3608  # the gap "icon's right edge → text ink's left edge" / S
BASE_RATIO = 0.7418  # baseline below the icon's top / S
TRACKING = -35  # letter tracking, font units (~ -0.035em)

ASCENDER = 730  # the height of 'd'/'f'/'k' in font units
UPM = 1000
NAT_ADV = 600  # the monospace font's natural advance
ADV = NAT_ADV + TRACKING  # the actual advance with tracking
LSB_M = 38  # 'm''s left side bearing — where the ink starts

FONT_CANDIDATES = [
    "C:/Windows/Fonts/JetBrainsMono-ExtraBold.ttf",
    "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-ExtraBold.ttf",
    "/usr/local/share/fonts/JetBrainsMono-ExtraBold.ttf",
    os.path.expanduser("~/.fonts/JetBrainsMono-ExtraBold.ttf"),
    "C:/Program Files/JetBrains/*/jbr/lib/fonts/JetBrainsMono-ExtraBold.ttf",
]


def find_font(explicit: str | None) -> str:
    if explicit:
        if not os.path.isfile(explicit):
            sys.exit(f"font not found: {explicit}")
        return explicit
    for pat in FONT_CANDIDATES:
        for hit in sorted(glob.glob(pat), reverse=True):
            if os.path.isfile(hit):
                return hit
    sys.exit(
        "JetBrainsMono-ExtraBold.ttf not found.\n"
        "Download: https://github.com/JetBrains/JetBrainsMono/releases (OFL 1.1)\n"
        "or pass a path: --font <path>"
    )


def glyph_paths(font: TTFont, text: str, start_index: int = 0) -> str:
    """The substring's outlines as one `d`, in font units, accounting for tracking.

    `start_index` — the substring's position within the whole word (so "fork" lands in its
    place, rather than starting from zero).
    """
    gs = font.getGlyphSet()
    cmap = font.getBestCmap()
    pen = SVGPathPen(gs, ntos=lambda v: f"{v:g}")
    for i, ch in enumerate(text):
        dx = (start_index + i) * ADV
        gs[cmap[ord(ch)]].draw(TransformPen(pen, (1, 0, 0, 1, dx, 0)))
    return pen.getCommands()


def ink_width(text: str = "mindfork") -> float:
    """The word's ink width in font units (for layout and the viewBox)."""
    # 'm'.xMin=38 … 'k'.xMax=580 (measured against the font; see assets/README.md)
    return (len(text) - 1) * ADV + 580 - LSB_M


def icon(size: float, x: float, y: float, plate: bool, mono: bool) -> str:
    """The icon: an optional rounded backdrop + a glyph on a 16x16 grid."""
    s = size / 16.0
    out = [f'  <g shape-rendering="crispEdges" transform="translate({x:g},{y:g})">']
    if plate:
        out.append(
            f'    <rect width="{size:g}" height="{size:g}" '
            f'rx="{size * 0.1875:g}" fill="{PLATE}"/>'
        )
    out.append(f'    <g transform="scale({s:g})">')
    for gx, gy, gw, gh, color in GLYPH:
        fill = "currentColor" if mono else color
        out.append(
            f'      <rect x="{gx}" y="{gy}" width="{gw}" height="{gh}" fill="{fill}"/>'
        )
    out.append("    </g>")
    out.append("  </g>")
    return "\n".join(out)


def wordmark_group(
    font: TTFont,
    size: float,
    ink_x: float,
    baseline: float,
    split: bool,
    text_fill: str,
) -> str:
    """The word "mindfork" as outlines.

    `split=True` — two paths: "mind" in color `text_fill` and "fork" in the accent; otherwise
    one word in `currentColor` (the single-color variant). The outlines stay in font
    units, while the group's transform sets the scale and flips the Y axis — this way `d`
    stays readable and editable by hand.
    """
    scale = size / UPM
    # A pen at zero gives ink starting at LSB_M — shift it so the ink lands on ink_x.
    tx = ink_x - LSB_M * scale
    out = [
        f'  <g transform="translate({tx:g},{baseline:g}) scale({scale:g},{-scale:g})">'
    ]
    if split:
        out.append(f'    <path fill="{text_fill}" d="{glyph_paths(font, "mind", 0)}"/>')
        out.append(f'    <path fill="{ORANGE}" d="{glyph_paths(font, "fork", 4)}"/>')
    else:
        out.append(f'    <path fill="currentColor" d="{glyph_paths(font, "mindfork")}"/>')
    out.append("  </g>")
    return "\n".join(out)


def svg(width: float, height: float, body: str, extra: str = "") -> str:
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width:g} {height:g}"'
        f' width="{width:g}" height="{height:g}" role="img" aria-label="mindfork"{extra}>\n'
        f"  <title>mindfork</title>\n{body}\n</svg>\n"
    )


def horizontal(font: TTFont, s: float, text_fill: str | None, mono: bool) -> str:
    """Horizontal lockup: the icon on the left, "mindfork" on the right."""
    size = ASC_RATIO * s / (ASCENDER / UPM)
    scale = size / UPM
    ink_x = s + GAP_RATIO * s
    baseline = BASE_RATIO * s
    w = ink_x + ink_width() * scale
    body = icon(s, 0, 0, plate=not mono, mono=mono)
    body += "\n" + wordmark_group(
        font, size, ink_x, baseline, split=not mono, text_fill=text_fill or LIGHT_TEXT
    )
    extra = ' fill="currentColor" color="#e4e4e7"' if mono else ""
    return svg(round(w, 1), s, body, extra)


# Vertical lockup: unlike the horizontal one, there's no reference — the proportions
# are derived. The word's width = 2x the icon size (otherwise the 8-letter monospace word
# stretches to 3.2x and the lockup becomes bottom-heavy); the vertical gap is taken as the
# same fraction of the ascender height as the horizontal one (0.3608/0.5227).
STACK_TEXT_RATIO = 2.0
GAP_PER_ASC = GAP_RATIO / ASC_RATIO


def stacked(font: TTFont, s: float) -> str:
    """Vertical lockup: the icon on top, "mindfork" centered below it."""
    w_text = STACK_TEXT_RATIO * s
    size = w_text / ink_width() * UPM
    scale = size / UPM
    asc = ASCENDER * scale
    w = max(w_text, s)
    baseline = s + GAP_PER_ASC * asc + asc
    body = icon(s, (w - s) / 2, 0, plate=True, mono=False)
    body += "\n" + wordmark_group(
        font, size, (w - w_text) / 2, baseline, split=True, text_fill=LIGHT_TEXT
    )
    # bottom margin for the descender overhang of 'o'/'d' (-10 units), otherwise the viewBox would clip them
    return svg(round(w, 1), round(baseline + 10 * scale, 1), body)


def tagline(font: TTFont, tag_font: TTFont, s: float, text: str) -> str:
    """Horizontal lockup + a tagline under the word, justified to its width."""
    size = ASC_RATIO * s / (ASCENDER / UPM)
    scale = size / UPM
    ink_x = s + GAP_RATIO * s
    baseline = BASE_RATIO * s
    w_text = ink_width() * scale
    w = ink_x + w_text

    # Tagline: fit the size and tracking so the line justifies exactly to the
    # wordmark's width (a classic technique — a tagline "under the word", edge to edge).
    gs = tag_font.getGlyphSet()
    cmap = tag_font.getBestCmap()
    n = len(text)
    tag_size = size * 0.26
    tsc = tag_size / UPM
    # width = (n-1)*(600+track) + xMax_last - xMin_first, solve for track
    first = cmap[ord(text[0])]
    last = cmap[ord(text[-1])]
    from fontTools.pens.boundsPen import BoundsPen

    bp = BoundsPen(gs)
    gs[first].draw(bp)
    xmin = bp.bounds[0]
    bp = BoundsPen(gs)
    gs[last].draw(bp)
    xmax = bp.bounds[2]
    track = (w_text / tsc - (xmax - xmin)) / (n - 1) - NAT_ADV

    pen = SVGPathPen(gs, ntos=lambda v: f"{v:g}")
    for i, ch in enumerate(text):
        gs[cmap[ord(ch)]].draw(
            TransformPen(pen, (1, 0, 0, 1, i * (NAT_ADV + track), 0))
        )
    tag_base = baseline + size * 0.42
    tx = ink_x - xmin * tsc

    body = icon(s, 0, 0, plate=True, mono=False)
    body += "\n" + wordmark_group(font, size, ink_x, baseline, split=True, text_fill=LIGHT_TEXT)
    body += (
        f'\n  <g transform="translate({tx:g},{tag_base:g}) scale({tsc:g},{-tsc:g})">\n'
        f'    <path fill="{MUTED}" d="{pen.getCommands()}"/>\n  </g>'
    )
    return svg(round(w, 1), round(tag_base + size * 0.1, 1), body)


def main() -> None:
    # The Windows console defaults to cp1252/cp866 — Russian output crashes it.
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, OSError):
            pass

    ap = argparse.ArgumentParser(description="Build mindfork wordmarks (text → outlines)")
    ap.add_argument("--font", help="path to JetBrainsMono-ExtraBold.ttf")
    ap.add_argument("--tag-font", help="path to JetBrainsMono-Bold.ttf (tagline)")
    args = ap.parse_args()

    path = find_font(args.font)
    tag_path = args.tag_font or path.replace("ExtraBold", "Bold")
    if not os.path.isfile(tag_path):
        tag_path = path
    font, tag_font = TTFont(path), TTFont(tag_path)
    print(f"font:    {path}")
    print(f"tagline: {tag_path}")

    here = os.path.dirname(os.path.abspath(__file__))
    files = {
        "mindfork-wordmark.svg": horizontal(font, 48, LIGHT_TEXT, mono=False),
        "mindfork-wordmark-dark.svg": horizontal(font, 48, LIGHT_TEXT, mono=False),
        "mindfork-wordmark-light.svg": horizontal(font, 48, DARK_TEXT, mono=False),
        "mindfork-wordmark-mono.svg": horizontal(font, 48, None, mono=True),
        "mindfork-wordmark-stacked.svg": stacked(font, 64),
        "mindfork-wordmark-tagline.svg": tagline(
            font, tag_font, 52, "TUI · RUST · LOCAL LLM"
        ),
    }
    for name, content in files.items():
        with open(os.path.join(here, name), "w", encoding="utf-8", newline="\n") as fh:
            fh.write(content)
        print(f"  ✓ {name}  ({len(content)} bytes)")


if __name__ == "__main__":
    main()
