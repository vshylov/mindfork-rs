# mindfork brand identity

Logo and wordmark source files. Integration plan for the app, packaging, and
documentation — [docs/branding.md](../docs/branding.md).

## Files

| File | Purpose |
|---|---|
| `mindfork-icon.svg` | icon with a rounded dark backplate — primary |
| `mindfork-icon-transparent.svg` | glyph with no backplate — for light backgrounds, TUI, b/w |
| `mindfork-icon-{16,32,48,64,128,256}.png` | raster for Linux icon themes (hicolor) |
| `mindfork.ico` | Windows: `.exe` and installer icon (6 sizes, 16-256) |
| `mindfork-wordmark.svg` | horizontal lockup — primary (= `-dark`) |
| `mindfork-wordmark-dark.svg` | for a dark background ("mind" is light) |
| `mindfork-wordmark-light.svg` | for a light background ("mind" is dark) |
| `mindfork-wordmark-mono.svg` | single-color, `currentColor` — inherits the text color |
| `mindfork-wordmark-stacked.svg` | vertical lockup: icon on top, word below |
| `mindfork-wordmark-tagline.svg` | horizontal + tagline `TUI · RUST · LOCAL LLM` |
| `wordmark-example.png` | **reference** used to reconstruct the font and proportions |
| `build-wordmarks.py` | wordmark generator (text → curves) |

## Palette

| Role | HEX | Where |
|---|---|---|
| Accent | `#c25a27` | "fork," the glyph's trunk |
| Branches | `#5c6370` | the glyph's branches |
| Backplate | `#09090b` | the icon's rounded square |
| Text on dark | `#e4e4e7` | "mind" in `-dark` |
| Text on light | `#18181b` | "mind" in `-light` |
| Tagline | `#71717a` | the caption under the word |

The brand palette **intentionally does not match** the interface palette
(`src/shared/theme.rs`): there `accent` means "activity" (generation, the token
counter), and recoloring it to the brand orange isn't possible — it would break
the semantics. The logo is always drawn in its own colors (except `-mono`). See
decision point D3 in [docs/branding.md](../docs/branding.md).

## Glyph

The icon is pixel art on a **16x16** grid made of five rectangles (the ink
occupies x = 3...13, y = 2...14):

| x | y | w | h | color |
|---|---|---|---|---|
| 7 | 2 | 2 | 12 | `#c25a27` |
| 11 | 2 | 2 | 5 | `#5c6370` |
| 9 | 5 | 2 | 2 | `#5c6370` |
| 3 | 7 | 2 | 5 | `#5c6370` |
| 5 | 10 | 2 | 2 | `#5c6370` |

The pixel nature isn't an accident — it lets the logo be drawn **directly in the
terminal** with half-block characters (`▀`, 16 columns x 8 rows), see
`widgets/logo.rs` and §5 of [docs/branding.md](../docs/branding.md). The single
source of truth for the geometry is `mindfork-icon-transparent.svg`; the table in
the code is checked against it by a test.

Backplate corner radius is `0.1875 x size` (7.5 at 40, 12 at 64).

## Font

**JetBrains Mono ExtraBold**, [SIL Open Font License 1.1](https://github.com/JetBrains/JetBrainsMono/blob/master/OFL.txt).

The font was identified from the `wordmark-example.png` reference by fitting
metrics, not by eye: the reconstructed point size (277.7 px) and tracking
(-0.035 em) reproduce the reference **to pixel accuracy** — the word's total
width is 1249 px in both the reference and the rebuild, and the "icon → text"
gap is 213 px in both. A confirming detail: the `o`/`d` descender overshoot
(-10 font units) predicts the bottom edge of those letters at exactly the
measured 348 px.

Lockup metrics (fractions of the icon size `S`), measured from the reference:

| Value | Fraction of `S` |
|---|---|
| Ascender height (`d`,`f`,`k`) | `0.5227` |
| Gap "icon's right edge → text ink" | `0.3608` |
| Baseline below the icon's top | `0.7418` |
| Tracking | `-35` font units (`-0.035 em`) |

The vertical (`-stacked`) lockup has no reference — its proportions were
derived: word width = `2 x S` (otherwise the 8-letter monospace word runs out
to `3.2 x S` and the composition becomes bottom-heavy), the vertical gap uses
the same fraction of the ascender height as the horizontal one.

### Why the text was converted to curves

The first version of the wordmarks contained `<text class="wm">` **with no
`<style>` and no `font-family`** — the classes were never defined anywhere, and
the files rendered in a default serif font. Even with `font-family` this
wouldn't be fixed: the viewer (GitHub, someone else's browser, a Linux
previewer) doesn't have the needed monospace font, and the typeface gets
substituted. So the glyphs were converted to `<path>` — the SVG is
self-contained.

OFL permits using the font to create artwork and distributing the resulting
curves. We do **not** put the font file itself in the repository — it's only
needed for regeneration.

## Regenerating the wordmarks

```
pip install fonttools
python assets/build-wordmarks.py
```

The script looks for `JetBrainsMono-ExtraBold.ttf` in the system fonts and in
JetBrains IDE bundles; otherwise pass a path via `--font`. Download from:
[JetBrains/JetBrainsMono/releases](https://github.com/JetBrains/JetBrainsMono/releases).

Edits to the palette, tracking, and lockup proportions go **into the script's
constants**, not into the SVGs by hand — otherwise the variants would drift
apart from each other.

## Usage rules

- **Clear space** around the lockup — no less than the height of the `f` glyph
  (≈ `0.5 x S`).
- Do **not** reposition or rescale the icon and the word separately — the
  proportions are set by the lockup; scale the file as a whole.
- Do **not** recolor the word arbitrarily: on a colored background use `-mono`
  with `currentColor`, not a hand-picked color.
- Minimum size for the horizontal lockup — **~100 px wide**; below that, use
  the icon without the word.
- "mind" and "fork" are **one word with no space**, separated only by color.
