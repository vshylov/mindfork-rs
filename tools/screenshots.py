#!/usr/bin/env python3
"""Render frame dumps into screenshot images (docs/history/demo-screenshots.md).

Why: screenshots of a real session would expose private data and rot as the
app evolves. Instead the app renders its screens headlessly from a demo
fixture and serializes each frame as JSON (`cargo test dump_demo_frames --
--ignored` writes `assets/screenshots/dumps/*.json`; a drift-gate test keeps
those dumps honest). This script renders them: **PNG** for the README (GitHub
cannot load fonts into an embedded SVG) and **SVG** for mindfork.io (crisp at
any zoom; the font arrives by `@font-face` reference, served by the site
itself — the demo-screenshots track's stage 4, built once the site had set
the sizes and themes it needs). Both place every glyph at its grid cell — the
dump's geometry is authoritative, no font-metrics guesswork.

The dump format (`src/shared/shot.rs::ShotFrame`): frame metadata (grid size,
canvas colors) plus rows of cells `{s, w?, fg?, bg?, m?}` where `w` is the
column span (default 1), absent `fg`/`bg` mean the frame's canvas colors, and
`m` is a string of modifier letters (B/D/I/U/R/H/S).

Fonts: JetBrains Mono (the brand font; OFL) probed from the system the same
way `assets/build-wordmarks.py` does, with symbol-font fallbacks for glyphs
outside its coverage (rounded borders and box drawing are in; `✦`/`⚒`-class
symbols usually are not). The TTFs are deliberately not vendored; pass
`--font-dir` if probing fails. The SVG writer uses the same faces for
geometry only (cell advance, ascent); the glyphs themselves are drawn by the
viewer from the site's webfonts, with the SVG `<style>` scoped under the
root id so an inlined copy cannot leak rules into the page.

Block elements (`█ ▌ ▄` and the rest of U+2580-U+259F) are not set from the
typeface: each is filled as the part of its cell its name says (`BLOCKS`).
Set as glyphs, the rows of a bar did not meet. In the raster the typeface
draws them to its own line height, a fraction of a pixel short of the cell the
grid rounds to; in the SVG every row was a `<text>` of its own, and a browser
antialiases each row's edge by itself, so the two half-covered edges that meet
between pixels let the canvas through. The SVG's backgrounds had the second
defect too, a `<rect>` per row. So the SVG fills both as geometry: everything
of one colour is one `<path>`, and boxes that meet along a whole side are
joined first - a bar down thirty rows is one box, with no edge inside it.

Third-party deps (the `build-wordmarks.py` precedent): Pillow for raster,
fontTools for cmap coverage — `pip install pillow fonttools`.

Usage:
    python tools/screenshots.py                 # all dumps -> PNG + SVG
    python tools/screenshots.py --format svg    # vectors only
    python tools/screenshots.py --only chat-dark-en --size 17
    python tools/screenshots.py --self-test     # the block elements' arms; needs no font
"""

from __future__ import annotations

import argparse
import glob
import json
import sys
import unicodedata
from pathlib import Path
from typing import NamedTuple

try:
    from PIL import Image, ImageDraw, ImageFont
except ImportError:  # pragma: no cover - environment guard
    sys.exit("Pillow is required: pip install pillow fonttools")
try:
    from fontTools.ttLib import TTFont
except ImportError:  # pragma: no cover - environment guard
    sys.exit("fontTools is required: pip install pillow fonttools")

REPO = Path(__file__).resolve().parent.parent
DUMPS = REPO / "assets" / "screenshots" / "dumps"
OUT = REPO / "assets" / "screenshots"

# Supersampling factor: glyphs are drawn at SS x the target size and the
# finished sheet is downscaled once with Lanczos — much crisper edges than
# drawing at 1:1.
SS = 2

# Default canvas padding, measured in character cells rather than pixels so it
# stays proportional at any `--size`. One cell (~10 px at 16 px) is the gutter a
# terminal leaves around its grid: the earlier flat 24 px read as a wide mat
# around the capture — obvious on GitHub and on mindfork.io, where the image is
# scaled up inside the site's window chrome and the mat grew with it.
PAD_CELLS = 1

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
    "C:/Windows/Fonts/YuGothM.ttc",  # Yu Gothic — fullwidth/CJK forms (e.g. U+FF0B)
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    # DejaVu Sans Mono — the search icon `⌕` (U+2315), which neither
    # JetBrains Mono nor DejaVu Sans has; same `fonts-dejavu-core` package.
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    # WenQuanYi Micro Hei — fullwidth forms (U+FF0B) in 5 MB, where Noto CJK
    # takes sixty: what the release job's runner installs for them
    # (docs/research/demo-reel.md §3.6).
    "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
]

# The Block Elements (U+2580-U+259F) as the parts of a cell they fill:
# `(left, top, right, bottom)` in eighths of the cell, the top at zero. The
# three shades (`░ ▒ ▓`) are patterns, not parts, and stay the typeface's.
EIGHTHS = 8
Box = tuple[int, int, int, int]
UPPER_LEFT, UPPER_RIGHT = (0, 0, 4, 4), (4, 0, 8, 4)
LOWER_LEFT, LOWER_RIGHT = (0, 4, 4, 8), (4, 4, 8, 8)
BLOCKS: dict[str, list[Box]] = {
    "▀": [(0, 0, 8, 4)],
    # `▁` to `█`: the lower eighth, then one more with every code point.
    **{chr(0x2580 + n): [(0, EIGHTHS - n, 8, 8)] for n in range(1, 9)},
    # `▉` to `▏`: the left seven eighths, then one fewer.
    **{chr(0x2590 - n): [(0, 0, n, 8)] for n in range(1, 8)},
    "▐": [(4, 0, 8, 8)],
    "▔": [(0, 0, 8, 1)],
    "▕": [(7, 0, 8, 8)],
    "▖": [LOWER_LEFT],
    "▗": [LOWER_RIGHT],
    "▘": [UPPER_LEFT],
    "▙": [UPPER_LEFT, LOWER_LEFT, LOWER_RIGHT],
    "▚": [UPPER_LEFT, LOWER_RIGHT],
    "▛": [UPPER_LEFT, UPPER_RIGHT, LOWER_LEFT],
    "▜": [UPPER_LEFT, UPPER_RIGHT, LOWER_RIGHT],
    "▝": [UPPER_RIGHT],
    "▞": [UPPER_RIGHT, LOWER_LEFT],
    "▟": [UPPER_RIGHT, LOWER_LEFT, LOWER_RIGHT],
}


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
        tt = TTFont(str(regular))
        self.primary_cmap = set(tt.getBestCmap())
        # Em-relative metrics straight from the font tables — the SVG grid
        # must use the *true* advance (0.6 em for JetBrains Mono), because
        # browsers lay glyphs out unhinted: Pillow's px-hinted `getlength`
        # (10.0 at 16 px vs the true 9.6) would make every long run land
        # short of its grid cells.
        upm = tt["head"].unitsPerEm
        hhea = tt["hhea"]
        zero_glyph = tt.getBestCmap()[ord("0")]
        self.em_advance = tt["hmtx"][zero_glyph][0] / upm
        self.em_ascent = hhea.ascent / upm
        self.em_height = (hhea.ascent - hhea.descent + hhea.lineGap) / upm
        self.fallbacks: list[tuple[ImageFont.FreeTypeFont, set[int]]] = []
        for cand in FALLBACKS:
            p = Path(cand)
            if p.is_file():
                # `.ttc` collections need an explicit face index (0 = the
                # family's primary face); plain `.ttf` must not get one.
                number = 0 if p.suffix.lower() == ".ttc" else -1
                self.fallbacks.append(
                    (
                        ImageFont.truetype(str(p), px, index=max(number, 0)),
                        set(TTFont(str(p), fontNumber=number).getBestCmap()),
                    )
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


def under_repo(path: Path, what: str) -> Path:
    """Canonicalize a CLI-supplied path and confine it to the repository.

    The tool reads dumps and writes images only under the repo by design.
    Resolve first, then check containment with `is_relative_to` — not a
    `startswith` prefix test, the partial-traversal pitfall — which is the
    validation `pythonsecurity:S8707` asks of agent-invokable CLIs.
    """
    resolved = path.resolve()
    if not resolved.is_relative_to(REPO):
        sys.exit(f"{what}: {path} resolves outside the repository ({REPO})")
    return resolved


class Metrics(NamedTuple):
    """Pixel geometry shared by every cell of a sheet."""

    cell_w: int
    cell_h: int
    ascent: int


def cell_colors(
    cell: dict, canvas_fg: tuple[int, int, int], canvas_bg: tuple[int, int, int]
) -> tuple[tuple[int, int, int], tuple[int, int, int]]:
    """Effective fg/bg after the R(everse) and D(im) modifiers."""
    fg = parse_hex(cell["fg"]) if "fg" in cell else canvas_fg
    bg = parse_hex(cell["bg"]) if "bg" in cell else canvas_bg
    mods = cell.get("m", "")
    if "R" in mods:
        fg, bg = bg, fg
    if "D" in mods:
        fg = blend(fg, bg, 0.45)
    return fg, bg


def block_parts(cell: dict) -> list[Box]:
    """The parts of its cell a block element fills: none for any other cell,
    and none for one that is hidden."""
    if cell.get("w", 1) != 1 or "H" in cell.get("m", ""):
        return []
    return BLOCKS.get(cell["s"], [])


def eighth(origin: int, size: int, n: int) -> int:
    """The pixel the `n`th eighth-line of a cell falls on. The part that ends on
    a line and the part that starts on it get the same pixel, so two parts that
    meet in a cell, and two cells that meet on the grid, leave nothing between."""
    return origin + (size * n + EIGHTHS // 2) // EIGHTHS


def part_box(part: Box, x0: int, y0: int, m: Metrics) -> Box:
    """A part of the cell at `(x0, y0)`, in the inclusive pixels Pillow fills."""
    left, top, right, bottom = part
    return (
        eighth(x0, m.cell_w, left),
        eighth(y0, m.cell_h, top),
        eighth(x0, m.cell_w, right) - 1,
        eighth(y0, m.cell_h, bottom) - 1,
    )


def draw_glyph(
    draw: ImageDraw.ImageDraw,
    faces: Faces,
    cell: dict,
    x0: int,
    y0: int,
    m: Metrics,
    fg: tuple[int, int, int],
) -> None:
    """A cell's text, set from a typeface on the row's baseline."""
    s = cell["s"]
    font, is_primary = faces.pick(s, cell.get("m", ""))
    if is_primary and cell.get("w", 1) == 1:
        draw.text((x0, y0 + m.ascent), s, font=font, fill=fg, anchor="ls")
    else:
        # Fallback faces are proportional and wide glyphs span two cells:
        # center in the box on the shared baseline.
        box_w = cell.get("w", 1) * m.cell_w
        draw.text((x0 + box_w / 2, y0 + m.ascent), s, font=font, fill=fg, anchor="ms")


def draw_cell(
    draw: ImageDraw.ImageDraw,
    faces: Faces,
    cell: dict,
    x0: int,
    y0: int,
    m: Metrics,
    canvas_fg: tuple[int, int, int],
    canvas_bg: tuple[int, int, int],
) -> None:
    """One cell: background, then a block element as its part of the cell or
    any other text as its glyph, then underline/strikethrough."""
    mods = cell.get("m", "")
    fg, bg = cell_colors(cell, canvas_fg, canvas_bg)
    box_w = cell.get("w", 1) * m.cell_w
    if bg != canvas_bg:
        draw.rectangle((x0, y0, x0 + box_w - 1, y0 + m.cell_h - 1), fill=bg)

    parts = block_parts(cell)
    for part in parts:
        draw.rectangle(part_box(part, x0, y0, m), fill=fg)
    if not parts and cell["s"].strip() and "H" not in mods:
        draw_glyph(draw, faces, cell, x0, y0, m, fg)
    if "U" in mods:
        uy = y0 + m.ascent + max(2, SS)
        draw.line((x0, uy, x0 + box_w - 1, uy), fill=fg, width=SS)
    if "S" in mods:
        sy = y0 + round(m.cell_h * 0.55)
        draw.line((x0, sy, x0 + box_w - 1, sy), fill=fg, width=SS)


def metrics(faces: Faces) -> Metrics:
    """The cell grid of the raster faces — one cell is the primary face's `0`."""
    mono = faces.primary[""]
    ascent, descent = mono.getmetrics()
    return Metrics(cell_w=round(mono.getlength("0")), cell_h=ascent + descent, ascent=ascent)


def draw_row(
    draw: ImageDraw.ImageDraw,
    faces: Faces,
    row: list[dict],
    x0: int,
    y0: int,
    m: Metrics,
    canvas_fg: tuple[int, int, int],
    canvas_bg: tuple[int, int, int],
) -> None:
    """One row of cells from `(x0, y0)`, advancing by each cell's width."""
    x = 0
    for cell in row:
        draw_cell(draw, faces, cell, x0 + x * m.cell_w, y0, m, canvas_fg, canvas_bg)
        x += cell.get("w", 1)


def render(dump: Path, out_dir: Path, faces: Faces, pad: int) -> Path:
    frame = json.loads(dump.read_text(encoding="utf-8"))
    canvas_bg = parse_hex(frame["canvas_bg"])
    canvas_fg = parse_hex(frame["canvas_fg"])

    m = metrics(faces)
    pad_px = pad * SS

    img = Image.new(
        "RGB",
        (frame["width"] * m.cell_w + 2 * pad_px, frame["height"] * m.cell_h + 2 * pad_px),
        canvas_bg,
    )
    draw = ImageDraw.Draw(img)
    for y, row in enumerate(frame["rows"]):
        draw_row(draw, faces, row, pad_px, pad_px + y * m.cell_h, m, canvas_fg, canvas_bg)

    img = img.resize((img.width // SS, img.height // SS), Image.LANCZOS)
    out = out_dir / f"{dump.stem}.png"
    img.save(out, optimize=True)
    return out


def svg_escape(s: str) -> str:
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def fmt(v: float) -> str:
    """Compact coordinate: two decimals, trailing zeros trimmed."""
    return f"{v:.2f}".rstrip("0").rstrip(".")


def hexs(rgb: tuple[int, int, int]) -> str:
    return "#{:02x}{:02x}{:02x}".format(*rgb)


def svg_label(stem: str) -> str:
    """Accessible label from a dump stem like `settings-model-dark-en`."""
    for theme in ("dark", "light"):
        marker = f"-{theme}-"
        if marker in stem:
            screen, _, _ = stem.partition(marker)
            return f"mindfork — the {screen.replace('-', ' ')} screen, {theme} theme"
    return f"mindfork — {stem}"


class SvgGrid(NamedTuple):
    """Sub-pixel geometry shared by every cell of one SVG sheet."""

    cell_w: float
    cell_h: float
    ascent: float
    pad: int


def frame_fills(
    rows: list[list[dict]], canvas_fg: tuple[int, int, int], canvas_bg: tuple[int, int, int]
) -> tuple[dict[tuple[int, int, int], list[Box]], dict[tuple[int, int, int], list[Box]]]:
    """What a frame fills without a typeface, as boxes by colour, in eighths of
    a cell from the frame's corner: its cells' backgrounds, and the parts its
    block elements fill."""
    backgrounds: dict[tuple[int, int, int], list[Box]] = {}
    blocks: dict[tuple[int, int, int], list[Box]] = {}
    for y, row in enumerate(rows):
        x = 0
        for cell in row:
            w = cell.get("w", 1)
            fg, bg = cell_colors(cell, canvas_fg, canvas_bg)
            left, top = x * EIGHTHS, y * EIGHTHS
            if bg != canvas_bg:
                backgrounds.setdefault(bg, []).append((left, top, left + w * EIGHTHS, top + EIGHTHS))
            for a, b, c, d in block_parts(cell):
                blocks.setdefault(fg, []).append((left + a, top + b, left + c, top + d))
            x += w
    return backgrounds, blocks


def joined(boxes: list[Box], down: bool = False) -> list[Box]:
    """Boxes that meet along a whole side, made one: side by side, or - `down` -
    one under the other. What is left has no edge where the two met."""

    def turned(box: Box) -> Box:
        return (box[1], box[0], box[3], box[2]) if down else box

    out: list[Box] = []
    for start, lo, end, hi in sorted(map(turned, boxes), key=lambda box: (box[1], box[3], box[0])):
        if out and (out[-1][1], out[-1][3], out[-1][2]) == (lo, hi, start):
            out[-1] = (out[-1][0], lo, end, hi)
        else:
            out.append((start, lo, end, hi))
    return [turned(box) for box in out]


def fill_path(boxes: list[Box], grid: SvgGrid, colour: tuple[int, int, int]) -> str:
    """Everything a frame fills in one colour, as one `<path>`: its boxes joined
    across and then down, each a subpath. A corner two boxes share is written
    from the same number, so it is the same point in both."""

    def across(n: int) -> str:
        return fmt(grid.pad + n * grid.cell_w / EIGHTHS)

    def down(n: int) -> str:
        return fmt(grid.pad + n * grid.cell_h / EIGHTHS)

    d = "".join(
        f"M{across(x0)} {down(y0)}H{across(x1)}V{down(y1)}H{across(x0)}Z"
        for x0, y0, x1, y1 in joined(joined(boxes), down=True)
    )
    return f'<path fill="{hexs(colour)}" d="{d}"/>'


def svg_fills(
    rows: list[list[dict]],
    grid: SvgGrid,
    canvas_fg: tuple[int, int, int],
    canvas_bg: tuple[int, int, int],
) -> list[str]:
    """Pass 1 - the frame's backgrounds, then its block elements over them."""
    backgrounds, blocks = frame_fills(rows, canvas_fg, canvas_bg)
    return [fill_path(boxes, grid, colour) for fills in (backgrounds, blocks) for colour, boxes in fills.items()]


class TextRun:
    """Pass-2 state: the pending same-style tspan run."""

    def __init__(self) -> None:
        self.chars: list[str] = []
        self.key: tuple | None = None
        self.x = 0
        self.cells = 0

    def start(self, x: int, key: tuple, s: str) -> None:
        self.x, self.key, self.chars, self.cells = x, key, [s], 1

    def extend(self, s: str) -> None:
        self.chars.append(s)
        self.cells += 1

    def take(self) -> tuple[tuple | None, str, int, int]:
        """Return (key, text, x, cells), resetting the run."""
        key, text, x, cells = self.key, "".join(self.chars), self.x, self.cells
        self.chars, self.key, self.cells = [], None, 0
        return key, text, x, cells


def flush_run(
    run: TextRun, spans: list[str], grid: SvgGrid, canvas_fg: tuple[int, int, int]
) -> None:
    """Emit the pending run as one <tspan>, if it holds visible text."""
    key, text, run_x, cells = run.take()
    if key is None or not text.strip():
        return
    fg, mods = key
    classes = " ".join(
        cls for flag, cls in (("B", "b"), ("I", "i"), ("U", "u"), ("S", "st")) if flag in mods
    )
    attrs = [f'x="{fmt(grid.pad + run_x * grid.cell_w)}"']
    # textLength pins the run's end to the grid even if a viewer
    # substitutes a font with a slightly different advance.
    if cells > 1:
        attrs.append(f'textLength="{fmt(cells * grid.cell_w)}"')
    if classes:
        attrs.append(f'class="{classes}"')
    if fg != canvas_fg:
        attrs.append(f'fill="{hexs(fg)}"')
    spans.append(f"<tspan {' '.join(attrs)}>{svg_escape(text)}</tspan>")


def fill_attr(fg: tuple[int, int, int], canvas_fg: tuple[int, int, int]) -> str:
    """` fill="#…"` when the fg differs from the canvas default, else empty."""
    return f' fill="{hexs(fg)}"' if fg != canvas_fg else ""


def cell_traits(
    cell: dict,
    faces: Faces,
    canvas_fg: tuple[int, int, int],
    canvas_bg: tuple[int, int, int],
) -> tuple[str, int, tuple[int, int, int], tuple, bool, bool]:
    """-> (s, w, fg, key, hidden, solo): what the pass-2 dispatch runs on."""
    w = cell.get("w", 1)
    mods = cell.get("m", "")
    fg, _ = cell_colors(cell, canvas_fg, canvas_bg)
    # A block element is filled as geometry (`svg_fills`): to the text it is
    # the space it stands in.
    s = " " if block_parts(cell) else cell["s"]
    hidden = "H" in mods or not s.strip() and "U" not in mods and "S" not in mods
    covered = all(ord(c) in faces.primary_cmap or ord(c) < 0x20 for c in s)
    solo = w > 1 or not covered
    key = (fg, "".join(m for m in "BIUS" if m in mods))
    return s, w, fg, key, hidden, solo


def svg_text_spans(
    row: list[dict],
    faces: Faces,
    grid: SvgGrid,
    canvas_fg: tuple[int, int, int],
    canvas_bg: tuple[int, int, int],
) -> list[str]:
    """Pass 2 — text runs. A run key is (fg, B, I, U, S); spaces join the
    current run only when the full key matches, else they end it. Cells the
    primary font does not cover, and wide (w=2) cells, become solo
    anchored-middle tspans centered in their box — the vector cousin of the
    raster path's centered fallback drawing.
    """
    spans: list[str] = []
    run = TextRun()
    x = 0
    for cell in row:
        s, w, fg, key, hidden, solo = cell_traits(cell, faces, canvas_fg, canvas_bg)
        # A cell extends the current run only on a full style-key match: for
        # a hidden cell a plain space qualifies, for a visible one any glyph
        # the primary face places at native advance (i.e. not solo).
        if key == run.key and (s == " " if hidden else not solo):
            run.extend(s)
        elif hidden:
            flush_run(run, spans, grid, canvas_fg)
        elif solo:
            flush_run(run, spans, grid, canvas_fg)
            center = grid.pad + (x + w / 2) * grid.cell_w
            spans.append(
                f'<tspan x="{fmt(center)}" text-anchor="middle"{fill_attr(fg, canvas_fg)}>'
                f"{svg_escape(s)}</tspan>"
            )
        else:
            flush_run(run, spans, grid, canvas_fg)
            run.start(x, key, s)
        x += w
    flush_run(run, spans, grid, canvas_fg)
    return spans


def render_svg(dump: Path, out_dir: Path, faces: Faces, pad: int, px: int, font_base: str) -> Path:
    """One dump -> one SVG: a <path> per colour for the backgrounds and the
    block elements, then one <text> per row with a <tspan> per same-style run.

    Runs keep the grid honest by construction: every run's `x` is set from
    the cell index, so advance drift can never accumulate across runs, and
    within a run every glyph is the primary monospace face at native advance.
    """
    frame = json.loads(dump.read_text(encoding="utf-8"))
    canvas_bg = parse_hex(frame["canvas_bg"])
    canvas_fg = parse_hex(frame["canvas_fg"])

    grid = SvgGrid(
        cell_w=px * faces.em_advance,  # the font's true advance, unhinted
        cell_h=px * faces.em_height,
        ascent=px * faces.em_ascent,
        pad=pad,
    )
    width = frame["width"] * grid.cell_w + 2 * pad
    height = frame["height"] * grid.cell_h + 2 * pad
    sid = dump.stem

    out: list[str] = []
    out.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" id="{sid}" '
        f'viewBox="0 0 {fmt(width)} {fmt(height)}" width="{fmt(width)}" height="{fmt(height)}" '
        f'role="img" aria-label="{svg_label(sid)}">'
    )
    # Scoped under #id: an inlined copy must not leak rules into the page.
    # The @font-face URLs are site-absolute — the site serves the woff2.
    out.append("<style>")
    for weight, style, filename in (
        (400, "normal", "JetBrainsMono-Regular.woff2"),
        (700, "normal", "JetBrainsMono-Bold.woff2"),
        (400, "italic", "JetBrainsMono-Italic.woff2"),
    ):
        out.append(
            "@font-face{font-family:'JetBrains Mono';"
            f"src:url('{font_base}{filename}') format('woff2');"
            f"font-weight:{weight};font-style:{style};font-display:swap}}"
        )
    out.extend([
        (
            f"#{sid} text{{font:{px}px 'JetBrains Mono',monospace;fill:{hexs(canvas_fg)}}}"
            f"#{sid} .b{{font-weight:700}}"
            f"#{sid} .i{{font-style:italic}}"
            f"#{sid} .u{{text-decoration:underline}}"
            f"#{sid} .st{{text-decoration:line-through}}"
            f"#{sid} .u.st{{text-decoration:underline line-through}}"
        ),
        "</style>",
        f'<rect width="100%" height="100%" fill="{hexs(canvas_bg)}"/>',
    ])

    out.extend(svg_fills(frame["rows"], grid, canvas_fg, canvas_bg))
    for y, row in enumerate(frame["rows"]):
        row_y = pad + y * grid.cell_h
        spans = svg_text_spans(row, faces, grid, canvas_fg, canvas_bg)
        if spans:
            out.append(
                f'<text xml:space="preserve" y="{fmt(row_y + grid.ascent)}">{"".join(spans)}</text>'
            )

    out.append("</svg>")
    path = out_dir / f"{dump.stem}.svg"
    path.write_text("\n".join(out), encoding="utf-8", newline="\n")
    return path


# --- the self-test ---------------------------------------------------------

# A cell of the size the default render draws at twice over: both sides odd,
# so a half and an eighth of it fall between pixels.
TEST_CELL = Metrics(cell_w=19, cell_h=43, ascent=33)
INK, PAPER, MARK = (200, 220, 240), (10, 20, 30), (90, 60, 30)
TEST_GRID = SvgGrid(cell_w=8, cell_h=16, ascent=12, pad=1)
AMOUNTS = {
    "HALF": 4,
    "ONE QUARTER": 2,
    "THREE QUARTERS": 6,
    "ONE EIGHTH": 1,
    "THREE EIGHTHS": 3,
    "FIVE EIGHTHS": 5,
    "SEVEN EIGHTHS": 7,
}
QUARTERS = {
    "UPPER LEFT": UPPER_LEFT,
    "UPPER RIGHT": UPPER_RIGHT,
    "LOWER LEFT": LOWER_LEFT,
    "LOWER RIGHT": LOWER_RIGHT,
}


class NoFaces:
    """Stands where the typefaces go, for cells that must not ask for one: it
    knows which characters the primary face has - ASCII - and draws none."""

    primary_cmap = frozenset(range(0x20, 0x7F))

    def pick(self, text: str, mods: str) -> None:
        raise AssertionError(f"{text!r} was set from a typeface")


def named_parts(ch: str) -> list[tuple[int, int, int, int]]:
    """What a Block Elements character fills by its Unicode name - the second
    opinion `BLOCKS` is held to."""
    name = unicodedata.name(ch)
    if name.endswith(" SHADE"):
        return []
    if name == "FULL BLOCK":
        return [(0, 0, 8, 8)]
    if name.startswith("QUADRANT "):
        return [QUARTERS[quarter] for quarter in name.removeprefix("QUADRANT ").split(" AND ")]
    side, _, amount = name.removesuffix(" BLOCK").partition(" ")
    n = AMOUNTS[amount]
    return [
        {
            "UPPER": (0, 0, 8, n),
            "LOWER": (0, EIGHTHS - n, 8, 8),
            "LEFT": (0, 0, n, 8),
            "RIGHT": (EIGHTHS - n, 0, 8, 8),
        }[side]
    ]


def drawn(rows: list[list[dict]]) -> Image.Image:
    """Rows of cells drawn on the test grid, with no typeface to draw from."""
    width = max(sum(cell.get("w", 1) for cell in row) for row in rows)
    img = Image.new("RGB", (width * TEST_CELL.cell_w, len(rows) * TEST_CELL.cell_h), PAPER)
    draw = ImageDraw.Draw(img)
    for y, row in enumerate(rows):
        draw_row(draw, NoFaces(), row, 0, y * TEST_CELL.cell_h, TEST_CELL, INK, PAPER)  # type: ignore[arg-type]
    return img


def inked(img: Image.Image) -> set[tuple[int, int]]:
    """The pixels of a test drawing that took the ink."""
    return {(x, y) for x in range(img.width) for y in range(img.height) if img.getpixel((x, y)) == INK}


def box(left: int, top: int, right: int, bottom: int) -> set[tuple[int, int]]:
    return {(x, y) for x in range(left, right) for y in range(top, bottom)}


def self_test() -> list[str]:
    problems = []

    def expect(what: str, got: object, want: object) -> None:
        if got != want:
            problems.append(f"{what}: {got!r}, not {want!r}")

    def ink_of(what: str, rows: list[list[dict]]) -> set[tuple[int, int]] | None:
        try:
            return inked(drawn(rows))
        except AssertionError as refusal:
            problems.append(f"{what}: {refusal}")
            return None

    def same(what: str, got: set[tuple[int, int]] | None, want: set[tuple[int, int]]) -> None:
        if got is not None and got != want:
            missing, stray = len(want - got), len(got - want)
            problems.append(f"{what}: {missing} of {len(want)} pixels are not drawn, {stray} are drawn outside")

    def ink(what: str, rows: list[list[dict]], want: set[tuple[int, int]]) -> None:
        same(what, ink_of(what, rows), want)

    block = [chr(cp) for cp in range(0x2580, 0x25A0)]
    for ch in block:
        expect(f"the parts of {ch} ({unicodedata.name(ch)})", sorted(BLOCKS.get(ch, [])), sorted(named_parts(ch)))
    expect("what else is drawn as parts", sorted(set(BLOCKS) - set(block)), [])

    w, h = TEST_CELL.cell_w, TEST_CELL.cell_h
    # The defect this guards: a bar is a column of cells, and the row of pixels
    # where two of them meet belongs to the bar like every other.
    ink("a column of full blocks", [[{"s": "█"}]] * 3, box(0, 0, w, 3 * h))
    ink("a column of left halves", [[{"s": "▌"}]] * 3, box(0, 0, 10, 3 * h))
    ink("a row of lower halves", [[{"s": "▄"}] * 3], box(0, 22, 3 * w, h))
    # Two parts that share a line share its pixel: between them they are the
    # cell, and no pixel is both's.
    for one, other in ("▌▐", "▀▄", "▙▝", "▚▞"):
        first, second = ink_of(one, [[{"s": one}]]), ink_of(other, [[{"s": other}]])
        if first is not None and second is not None:
            same(f"{one} and {other} together", first | second, box(0, 0, w, h))
            same(f"{one} and {other} overlapping", first & second, set())
    ink("a hidden block", [[{"s": "█", "m": "H"}]], set())
    ink("a reversed block", [[{"s": "█", "m": "R", "fg": hexs(PAPER), "bg": hexs(INK)}]], box(0, 0, w, h))

    # The SVG: what meets along a whole side is one box, so a bar and a
    # background have no edge between their rows for a viewer to antialias.
    def cells(*at: tuple[int, int]) -> list[Box]:
        return [(8 * x, 8 * y, 8 * x + 8, 8 * y + 8) for x, y in at]

    def both_ways(boxes: list[Box]) -> list[Box]:
        return joined(joined(boxes), down=True)

    expect("a column of cells, joined", both_ways(cells((0, 0), (0, 1), (0, 2))), [(0, 0, 8, 24)])
    expect("a square of cells, joined", both_ways(cells((0, 0), (1, 0), (0, 1), (1, 1))), [(0, 0, 16, 16)])
    expect("cells around a corner", both_ways(cells((0, 0), (1, 0), (0, 1))), [(0, 8, 8, 16), (0, 0, 16, 8)])
    expect("cells that touch at a corner", both_ways(cells((0, 0), (1, 1))), [(0, 0, 8, 8), (8, 8, 16, 16)])
    marked = hexs(MARK)
    frame = [
        [{"s": "▌", "bg": marked}, {"s": "a", "bg": marked}],
        [{"s": "▌", "bg": marked}, {"s": "b"}],
        [{"s": "▌", "m": "H"}, {"s": "█", "w": 2}],
    ]
    backgrounds, blocks = frame_fills(frame, INK, PAPER)
    expect("a frame's backgrounds", backgrounds, {MARK: [(0, 0, 8, 8), (8, 0, 16, 8), (0, 8, 8, 16)]})
    expect("a frame's block elements", blocks, {INK: [(0, 0, 4, 8), (0, 8, 4, 16)]})
    expect(
        "the fills of a frame",
        svg_fills(frame, TEST_GRID, INK, PAPER),
        [
            f'<path fill="{marked}" d="M1 17H9V33H1ZM1 1H17V17H1Z"/>',
            f'<path fill="{hexs(INK)}" d="M1 1H5V33H1Z"/>',
        ],
    )
    spans = svg_text_spans(frame[0], NoFaces(), TEST_GRID, INK, PAPER)  # type: ignore[arg-type]
    expect("a block element in the text", spans, ['<tspan x="9">a</tspan>'])
    return problems


def main() -> int:
    if sys.argv[1:] == ["--self-test"]:
        problems = self_test()
        for problem in problems:
            print(f"error: {problem}", file=sys.stderr)
        if not problems:
            print("clean: every block element is drawn as the part of its cell its name says")
        return 1 if problems else 0

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dumps", type=Path, default=DUMPS, help="dump directory or a single .json")
    parser.add_argument("--out", type=Path, default=OUT, help="output directory")
    parser.add_argument("--only", help="render only dumps whose stem contains this substring")
    parser.add_argument("--size", type=int, default=16, help="font size in px (default 16)")
    parser.add_argument(
        "--pad",
        type=int,
        help=f"canvas padding in px (default: {PAD_CELLS} character cell, ~10 px at size 16)",
    )
    parser.add_argument("--font-dir", type=Path, help="directory with JetBrainsMono-*.ttf")
    parser.add_argument(
        "--format",
        choices=("png", "svg", "both"),
        default="both",
        help="which renders to write (default both)",
    )
    parser.add_argument(
        "--svg-font-base",
        default="/fonts/",
        help="URL prefix for the SVG @font-face sources (default /fonts/, the site's own)",
    )
    args = parser.parse_args()

    dumps_root = under_repo(args.dumps, "--dumps")
    out_dir = under_repo(args.out, "--out")

    dumps = [dumps_root] if dumps_root.is_file() else sorted(dumps_root.glob("*.json"))
    if args.only:
        dumps = [d for d in dumps if args.only in d.stem]
    if not dumps:
        print(f"no dumps under {dumps_root} — run: cargo test dump_demo_frames -- --ignored")
        return 1

    out_dir.mkdir(parents=True, exist_ok=True)
    faces_png = Faces(args.font_dir, args.size * SS) if args.format in ("png", "both") else None
    faces_svg = Faces(args.font_dir, args.size) if args.format in ("svg", "both") else None
    # `em_advance` is size-independent, and --format always selects at least one
    # writer, so whichever face got loaded answers the padding question for both.
    em_advance = (faces_png or faces_svg).em_advance  # type: ignore[union-attr]
    pad = args.pad if args.pad is not None else round(PAD_CELLS * args.size * em_advance)
    for dump in dumps:
        if faces_png:
            out = render(dump, out_dir, faces_png, pad)
            print(f"{dump.name} -> {out.relative_to(REPO)}")
        if faces_svg:
            out = render_svg(dump, out_dir, faces_svg, pad, args.size, args.svg_font_base)
            print(f"{dump.name} -> {out.relative_to(REPO)}")
    missing = (faces_png.missing if faces_png else set()) | (
        faces_svg.missing if faces_svg else set()
    )
    if missing:
        # Name what did not render rather than shipping silent tofu.
        print(f"WARNING: no font covered: {' '.join(sorted(missing))}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
