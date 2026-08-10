#!/usr/bin/env python3
"""Render frame dumps into screenshot images (docs/demo-screenshots.md).

Why: screenshots of a real session would expose private data and rot as the
app evolves. Instead the app renders its screens headlessly from a demo
fixture and serializes each frame as JSON (`cargo test dump_demo_frames --
--ignored` writes `artwork/screenshots/dumps/*.json`; a drift-gate test keeps
those dumps honest). This script is the raster half: it places every glyph at
its grid cell — the dump's geometry is authoritative, no font-metrics
guesswork — and writes PNGs for the README and the site.

The dump format (`src/shared/shot.rs::ShotFrame`): frame metadata (grid size,
canvas colors) plus rows of cells `{s, w?, fg?, bg?, m?}` where `w` is the
column span (default 1), absent `fg`/`bg` mean the frame's canvas colors, and
`m` is a string of modifier letters (B/D/I/U/R/H/S).

Fonts: JetBrains Mono (the brand font; OFL) probed from the system the same
way `artwork/build-wordmarks.py` does, with symbol-font fallbacks for glyphs
outside its coverage (rounded borders and box drawing are in; `✦`/`⚒`-class
symbols usually are not). The TTFs are deliberately not vendored; pass
`--font-dir` if probing fails.

Third-party deps (the `build-wordmarks.py` precedent): Pillow for raster,
fontTools for cmap coverage — `pip install pillow fonttools`.

Usage:
    python tools/screenshots.py                 # all dumps -> artwork/screenshots/
    python tools/screenshots.py --only chat-dark-en --size 17
"""

from __future__ import annotations

import argparse
import glob
import json
import sys
from pathlib import Path

try:
    from PIL import Image, ImageDraw, ImageFont
except ImportError:  # pragma: no cover - environment guard
    sys.exit("Pillow is required: pip install pillow fonttools")
try:
    from fontTools.ttLib import TTFont
except ImportError:  # pragma: no cover - environment guard
    sys.exit("fontTools is required: pip install pillow fonttools")

REPO = Path(__file__).resolve().parent.parent
DUMPS = REPO / "artwork" / "screenshots" / "dumps"
OUT = REPO / "artwork" / "screenshots"

# Supersampling factor: glyphs are drawn at SS x the target size and the
# finished sheet is downscaled once with Lanczos — much crisper edges than
# drawing at 1:1.
SS = 2

# Probed per variant; the first hit wins. Same roots as build-wordmarks.py,
# plus the per-user Windows font directory.
FONT_ROOTS = [
    "C:/Windows/Fonts",
    str(Path.home() / "AppData/Local/Microsoft/Windows/Fonts"),
    "/usr/share/fonts/truetype/jetbrains-mono",
    "/usr/local/share/fonts",
    str(Path.home() / ".fonts"),
    "C:/Program Files/JetBrains/*/jbr/lib/fonts",
]
VARIANTS = {
    "": "JetBrainsMono-Regular.ttf",
    "B": "JetBrainsMono-Bold.ttf",
    "I": "JetBrainsMono-Italic.ttf",
    "BI": "JetBrainsMono-BoldItalic.ttf",
}
# Symbol fallbacks for glyphs JetBrains Mono lacks; proportional fonts are
# fine here — fallback glyphs are centered in their cell box.
FALLBACKS = [
    "C:/Windows/Fonts/seguisym.ttf",  # Segoe UI Symbol
    "C:/Windows/Fonts/seguiemj.ttf",  # Segoe UI Emoji (monochrome outlines)
    "C:/Windows/Fonts/segoeui.ttf",  # Segoe UI — broad text coverage (sub/superscripts)
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
]


def find_variant(font_dir: Path | None, filename: str) -> Path | None:
    if font_dir:
        cand = font_dir / filename
        return cand if cand.is_file() else None
    for root in FONT_ROOTS:
        for hit in sorted(glob.glob(f"{root}/{filename}"), reverse=True):
            return Path(hit)
    return None


class Faces:
    """Loaded font faces plus cmap coverage, one size."""

    def __init__(self, font_dir: Path | None, px: int) -> None:
        regular = find_variant(font_dir, VARIANTS[""])
        if regular is None:
            sys.exit(
                "JetBrains Mono not found. Download: "
                "https://github.com/JetBrains/JetBrainsMono/releases "
                "then pass --font-dir DIR (or install the family)."
            )
        self.primary: dict[str, ImageFont.FreeTypeFont] = {}
        for key, filename in VARIANTS.items():
            path = find_variant(font_dir, filename) or regular
            self.primary[key] = ImageFont.truetype(str(path), px)
        self.primary_cmap = set(TTFont(str(regular)).getBestCmap())
        self.fallbacks: list[tuple[ImageFont.FreeTypeFont, set[int]]] = []
        for cand in FALLBACKS:
            p = Path(cand)
            if p.is_file():
                self.fallbacks.append(
                    (ImageFont.truetype(str(p), px), set(TTFont(str(p)).getBestCmap()))
                )
        self.missing: set[str] = set()

    def pick(self, text: str, mods: str) -> tuple[ImageFont.FreeTypeFont, bool]:
        """Face for `text` -> (font, is_primary)."""
        cps = [ord(c) for c in text]
        if all(cp in self.primary_cmap or cp < 0x20 for cp in cps):
            key = "".join(m for m in "BI" if m in mods)
            return self.primary[key], True
        for font, cmap in self.fallbacks:
            if all(cp in cmap for cp in cps):
                return font, False
        self.missing.add(text)
        return self.primary[""], True


def parse_hex(value: str) -> tuple[int, int, int]:
    return tuple(int(value[i : i + 2], 16) for i in (1, 3, 5))  # type: ignore[return-value]


def blend(a: tuple[int, int, int], b: tuple[int, int, int], t: float) -> tuple[int, int, int]:
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))  # type: ignore[return-value]


def render(dump: Path, out_dir: Path, faces: Faces, size: int, pad: int) -> Path:
    frame = json.loads(dump.read_text(encoding="utf-8"))
    cols, rows_n = frame["width"], frame["height"]
    canvas_bg = parse_hex(frame["canvas_bg"])
    canvas_fg = parse_hex(frame["canvas_fg"])

    mono = faces.primary[""]
    cell_w = round(mono.getlength("0"))
    ascent, descent = mono.getmetrics()
    cell_h = ascent + descent
    pad_px = pad * SS

    img = Image.new("RGB", (cols * cell_w + 2 * pad_px, rows_n * cell_h + 2 * pad_px), canvas_bg)
    draw = ImageDraw.Draw(img)

    for y, row in enumerate(frame["rows"]):
        x = 0
        for cell in row:
            w = cell.get("w", 1)
            fg = parse_hex(cell["fg"]) if "fg" in cell else canvas_fg
            bg = parse_hex(cell["bg"]) if "bg" in cell else canvas_bg
            mods = cell.get("m", "")
            if "R" in mods:
                fg, bg = bg, fg
            if "D" in mods:
                fg = blend(fg, bg, 0.45)

            x0, y0 = pad_px + x * cell_w, pad_px + y * cell_h
            box_w = w * cell_w
            if bg != canvas_bg:
                draw.rectangle((x0, y0, x0 + box_w - 1, y0 + cell_h - 1), fill=bg)

            s = cell["s"]
            if s.strip() and "H" not in mods:
                font, is_primary = faces.pick(s, mods)
                if is_primary and w == 1:
                    draw.text((x0, y0 + ascent), s, font=font, fill=fg, anchor="ls")
                else:
                    # Fallback faces are proportional and wide glyphs span two
                    # cells: center in the box on the shared baseline.
                    draw.text(
                        (x0 + box_w / 2, y0 + ascent),
                        s,
                        font=font,
                        fill=fg,
                        anchor="ms",
                    )
            if "U" in mods:
                uy = y0 + ascent + max(2, SS)
                draw.line((x0, uy, x0 + box_w - 1, uy), fill=fg, width=SS)
            if "S" in mods:
                sy = y0 + round(cell_h * 0.55)
                draw.line((x0, sy, x0 + box_w - 1, sy), fill=fg, width=SS)
            x += w

    img = img.resize((img.width // SS, img.height // SS), Image.LANCZOS)
    out = out_dir / f"{dump.stem}.png"
    img.save(out, optimize=True)
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dumps", type=Path, default=DUMPS, help="dump directory or a single .json")
    parser.add_argument("--out", type=Path, default=OUT, help="output directory")
    parser.add_argument("--only", help="render only dumps whose stem contains this substring")
    parser.add_argument("--size", type=int, default=16, help="font size in px (default 16)")
    parser.add_argument("--pad", type=int, default=24, help="canvas padding in px (default 24)")
    parser.add_argument("--font-dir", type=Path, help="directory with JetBrainsMono-*.ttf")
    args = parser.parse_args()

    dumps = [args.dumps] if args.dumps.is_file() else sorted(args.dumps.glob("*.json"))
    if args.only:
        dumps = [d for d in dumps if args.only in d.stem]
    if not dumps:
        print(f"no dumps under {args.dumps} — run: cargo test dump_demo_frames -- --ignored")
        return 1

    args.out.mkdir(parents=True, exist_ok=True)
    faces = Faces(args.font_dir, args.size * SS)
    for dump in dumps:
        out = render(dump, args.out, faces, args.size, args.pad)
        print(f"{dump.name} -> {out.relative_to(REPO)}")
    if faces.missing:
        # Name what did not render rather than shipping silent tofu.
        glyphs = " ".join(sorted(faces.missing))
        print(f"WARNING: no font covered: {glyphs}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
