#!/usr/bin/env python3
"""Draw the social preview card — `assets/og-card.png`, 1200x630.

A link to mindfork.io posted anywhere social used to render as a 256x256 app
icon with the site-wide description, because `og:image` pointed at the favicon
and `twitter:card` was `summary` (docs/research/public-documents.md §2.3). A
card is the one asset that cannot be generated at build time from the site's own
CSS, so it is drawn here and committed.

**Not gated**, unlike the screenshots and the legal pages: the card is drawn with
JetBrains Mono probed from the system (the same roots `tools/screenshots.py`
uses), and the CI runner has no such font — a `--check` there would fail on a
machine difference rather than on drift. Regenerate it deliberately when the
wording or the palette changes:

    python tools/og_card.py
    python tools/site_sync_assets.py     # mirror it into site/static/brand/

Usage:
    python tools/og_card.py [--out PATH]
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

try:
    from PIL import Image, ImageDraw, ImageFont
except ImportError:  # pragma: no cover - the same message screenshots.py gives
    print("Pillow is required: python -m pip install pillow", file=sys.stderr)
    raise

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(Path(__file__).resolve().parent))

from screenshots import find_variant  # noqa: E402

# The card's size is what every platform crops to 1.91:1 without letterboxing.
SIZE = (1200, 630)

# The site's dark palette (site/sass/_palette.scss), because a preview card is
# seen next to the site and should look like it.
BG = (13, 14, 17)
PANEL = (22, 24, 28)
TEXT = (232, 230, 227)
MUTED = (150, 148, 144)
ACCENT = (194, 90, 39)

TITLE = "mindfork"
PROMPT = "$ mindfork"
TAGLINE = "An AI chat that lives in your terminal"
LINES = [
    "Local models via llama.cpp — or OpenAI, Anthropic,",
    "Gemini and Grok. Memory, notes, RAG and tools.",
]
FOOT = "MIT  ·  Windows + Linux  ·  mindfork.io"


def font(px: int, bold: bool = False) -> ImageFont.FreeTypeFont:
    """JetBrains Mono at `px`, probed the way the screenshot renderer probes it.

    `find_variant(None, …)` searches every root in `FONT_ROOTS`, the JetBrains
    IDE bundles included — which is where the family usually is on a machine
    that never installed it system-wide.
    """
    name = "JetBrainsMono-Bold.ttf" if bold else "JetBrainsMono-Regular.ttf"
    hit = find_variant(None, name)
    if hit is None:
        raise SystemExit(
            "JetBrains Mono not found. Download it from "
            "https://github.com/JetBrains/JetBrainsMono/releases and install it, "
            "or drop the .ttf files into one of tools/screenshots.py's FONT_ROOTS."
        )
    return ImageFont.truetype(str(hit), px)


def draw_card() -> Image.Image:
    img = Image.new("RGB", SIZE, BG)
    d = ImageDraw.Draw(img)

    # A terminal window, because that is what the product is.
    d.rounded_rectangle((60, 60, 1140, 570), radius=18, fill=PANEL)
    d.rounded_rectangle((60, 60, 1140, 570), radius=18, outline=(44, 46, 52), width=2)
    for i, colour in enumerate(((90, 90, 96), (90, 90, 96), (90, 90, 96))):
        d.ellipse((92 + i * 26, 92, 104 + i * 26, 104), fill=colour)
    d.line((60, 124, 1140, 124), fill=(44, 46, 52), width=2)

    d.text((100, 170), PROMPT, font=font(34), fill=ACCENT)
    d.text((100, 232), TITLE, font=font(96, bold=True), fill=TEXT)
    d.text((100, 348), TAGLINE, font=font(38, bold=True), fill=TEXT)
    y = 412
    for line in LINES:
        d.text((100, y), line, font=font(28), fill=MUTED)
        y += 44
    d.text((100, 510), FOOT, font=font(24), fill=ACCENT)
    return img


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--out", default=str(ROOT / "assets" / "og-card.png"))
    args = ap.parse_args()
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    draw_card().save(out, format="PNG", optimize=True)
    print(f"{out} — {out.stat().st_size / 1024:.1f} KiB, {SIZE[0]}x{SIZE[1]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
