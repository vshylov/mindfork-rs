#!/usr/bin/env python3
"""Render a demo reel into an animated GIF and WebP (docs/research/demo-reel.md).

Why: an animation recorded by hand would be stale within a week — the
application changes every few days. The reel is made the way the screenshots
are: the app plays a scripted turn headlessly and keeps every frame
(`cargo test dump_demo_reel -- --ignored` writes `target/reel/*.json`; tests
next to the script hold it to the code), and this tool draws the frames with
the screenshots' own cell renderer (`tools/screenshots.py`).

A reel is `{name, frames}`; a frame is one of `shot`'s dumps plus `ms` (how
long it stays up), `cursor` (`[x, y]` or null) and `beat` (which part of the
scenario it belongs to — not drawn).

What it writes, beside each reel unless `--out` says otherwise:
- **GIF** — for the README; it plays everywhere. One palette for the whole
  reel and no dithering, so a colour does not flicker between frames.
- **WebP**, lossless — for the site; smaller for the same pixels.

Speed: a streamed frame changes a few rows, so each distinct row is drawn once
and pasted wherever it recurs.

Fonts: JetBrains Mono, written back out as TTF from the woff2 faces the site
ships (`site/static/fonts/`, the full family) into `target/reel/fonts/` — so
this machine and a CI runner draw with the same faces, and nothing has to be
installed for the primary face. `--font-dir` overrides. The symbol fallbacks
are screenshots.py's (`FALLBACKS`): system fonts, which CI installs.

Third-party deps: `tools/media-requirements.txt` — Pillow, fontTools, and
Brotli for the woff2 faces; CI installs it with `--require-hashes`.

Usage:
    python tools/demo_reel.py                    # every reel -> GIF + WebP
    python tools/demo_reel.py --format gif --font-dir DIR
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

# `screenshots` is the sibling module: this directory goes on the path to import it.
sys.path.insert(0, str(Path(__file__).resolve().parent))

from screenshots import (  # noqa: E402
    PAD_CELLS,
    REPO,
    SS,
    Faces,
    Image,
    ImageDraw,
    Metrics,
    TTFont,
    draw_row,
    metrics,
    parse_hex,
    under_repo,
)

REELS = REPO / "target" / "reel"
SITE_FONTS = REPO / "site" / "static" / "fonts"
FONT_CACHE = REELS / "fonts"

# The terminal's cursor, drawn as the bar Windows Terminal and most emulators
# default to: this many supersampled pixels wide, in the text colour.
CURSOR_W = 2 * SS

# Frames sampled for the GIF's one palette: enough to meet every colour the
# reel uses (the beats differ — the feed, the tool card, the self-model) while
# keeping the quantizer's input small.
PALETTE_SAMPLES = 12


def site_faces() -> Path:
    """JetBrains Mono as TTF, written out of the site's woff2 faces once (and
    again when a face changes), into a directory `Faces` can read."""
    faces = sorted(SITE_FONTS.glob("JetBrainsMono-*.woff2"))
    if not faces:
        sys.exit(f"no JetBrains Mono faces under {SITE_FONTS.relative_to(REPO)}")
    FONT_CACHE.mkdir(parents=True, exist_ok=True)
    for woff2 in faces:
        ttf = FONT_CACHE / f"{woff2.stem}.ttf"
        if ttf.is_file() and ttf.stat().st_mtime >= woff2.stat().st_mtime:
            continue
        try:
            font = TTFont(str(woff2), recalcTimestamp=False)
            font.flavor = None
            font.save(str(ttf))
        except ImportError:  # pragma: no cover - environment guard
            sys.exit("Brotli is required to read the woff2 faces: pip install brotli")
    return FONT_CACHE


class Sheet:
    """Draws a reel's frames on one grid, each distinct row once."""

    def __init__(self, faces: Faces, pad: int) -> None:
        self.faces = faces
        self.m: Metrics = metrics(faces)
        self.pad_px = pad * SS
        self.rows: dict[tuple[str, str, str], Image.Image] = {}

    def row(self, row: list[dict], width_px: int, fg: str, bg: str) -> Image.Image:
        key = (fg, bg, json.dumps(row, ensure_ascii=False, sort_keys=True))
        tile = self.rows.get(key)
        if tile is None:
            canvas_fg, canvas_bg = parse_hex(fg), parse_hex(bg)
            tile = Image.new("RGB", (width_px, self.m.cell_h), canvas_bg)
            draw_row(
                ImageDraw.Draw(tile), self.faces, row, self.pad_px, 0, self.m, canvas_fg, canvas_bg
            )
            self.rows[key] = tile
        return tile

    def frame(self, frame: dict) -> Image.Image:
        m, pad = self.m, self.pad_px
        width_px = frame["width"] * m.cell_w + 2 * pad
        height_px = frame["height"] * m.cell_h + 2 * pad
        img = Image.new("RGB", (width_px, height_px), parse_hex(frame["canvas_bg"]))
        for y, row in enumerate(frame["rows"]):
            tile = self.row(row, width_px, frame["canvas_fg"], frame["canvas_bg"])
            img.paste(tile, (0, pad + y * m.cell_h))
        cursor = frame.get("cursor")
        if cursor:
            x0 = pad + cursor[0] * m.cell_w
            y0 = pad + cursor[1] * m.cell_h
            ImageDraw.Draw(img).rectangle(
                (x0, y0, x0 + CURSOR_W - 1, y0 + m.cell_h - 1), fill=parse_hex(frame["canvas_fg"])
            )
        return img.resize((width_px // SS, height_px // SS), Image.LANCZOS)


def palette_for(images: list[Image.Image]) -> Image.Image:
    """One 256-colour palette for every frame, quantized from a sample of them
    stacked into one image."""
    step = max(1, len(images) // PALETTE_SAMPLES)
    sample = images[::step] + [images[-1]]
    w, h = sample[0].size
    sheet = Image.new("RGB", (w, h * len(sample)))
    for i, img in enumerate(sample):
        sheet.paste(img, (0, i * h))
    return sheet.quantize(colors=256, method=Image.Quantize.MEDIANCUT)


def write_gif(images: list[Image.Image], durations: list[int], out: Path) -> None:
    palette = palette_for(images)
    frames = [img.quantize(palette=palette, dither=Image.Dither.NONE) for img in images]
    frames[0].save(
        out,
        save_all=True,
        append_images=frames[1:],
        duration=durations,
        loop=0,
        optimize=False,
    )


def write_webp(images: list[Image.Image], durations: list[int], out: Path) -> None:
    images[0].save(
        out,
        save_all=True,
        append_images=images[1:],
        duration=durations,
        loop=0,
        lossless=True,
        # The slowest, smallest encoding: measured on the first reel, 955 KB
        # against 1 344 KB at the default effort, in seconds either way.
        method=6,
        minimize_size=True,
    )


def render_reel(path: Path, out_dir: Path, sheet: Sheet, formats: list[str]) -> list[Path]:
    reel = json.loads(path.read_text(encoding="utf-8"))
    frames = reel["frames"]
    images = [sheet.frame(f) for f in frames]
    durations = [f["ms"] for f in frames]
    written = []
    for fmt in formats:
        out = out_dir / f"{reel['name']}.{fmt}"
        (write_gif if fmt == "gif" else write_webp)(images, durations, out)
        written.append(out)
    return written


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reels", type=Path, default=REELS, help="reel directory or a single .json")
    parser.add_argument("--out", type=Path, help="output directory (default: beside each reel)")
    parser.add_argument("--size", type=int, default=16, help="font size in px (default 16)")
    parser.add_argument(
        "--pad",
        type=int,
        help=f"canvas padding in px (default: {PAD_CELLS} character cell, ~10 px at size 16)",
    )
    parser.add_argument("--font-dir", type=Path, help="directory with JetBrainsMono-*.ttf")
    parser.add_argument(
        "--format",
        choices=("gif", "webp", "both"),
        default="both",
        help="which animations to write (default both)",
    )
    args = parser.parse_args()

    root = under_repo(args.reels, "--reels")
    reels = [root] if root.is_file() else sorted(root.glob("*.json"))
    if not reels:
        print(f"no reels under {root} — run: cargo test dump_demo_reel -- --ignored")
        return 1
    out_dir = under_repo(args.out, "--out") if args.out else None
    formats = ["gif", "webp"] if args.format == "both" else [args.format]

    font_dir = under_repo(args.font_dir, "--font-dir") if args.font_dir else site_faces()
    faces = Faces(font_dir, args.size * SS)
    pad = args.pad if args.pad is not None else round(PAD_CELLS * args.size * faces.em_advance)
    sheet = Sheet(faces, pad)
    for reel in reels:
        target = out_dir or reel.parent
        target.mkdir(parents=True, exist_ok=True)
        for out in render_reel(reel, target, sheet, formats):
            size_kb = out.stat().st_size / 1024
            print(f"{reel.name} -> {out.relative_to(REPO)} ({size_kb:.0f} KB)")
    if faces.missing:
        # Name what did not render rather than shipping silent tofu.
        print(f"WARNING: no font covered: {' '.join(sorted(faces.missing))}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
