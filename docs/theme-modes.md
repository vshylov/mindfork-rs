# Colour modes — system, monochrome, full — and user themes

Track design plan. Asked for by the user on 2026-09-27: three ways of drawing
the interface — the one the app has today, one with no colours and no text
attributes, and one where the app paints its own background and text colour,
tuned for contrast — plus themes a user can write for the last one.

§6 records the forks and the user's decisions; §7 is the stage list. **Stage 1
(the framework and the full mode) is what the first PR implements.**

## 1. What and why

Today the app draws *foregrounds* and leaves the background to the terminal.
`interface.theme` picks one of three foreground palettes (`auto`, `dark`,
`light` — spec §11.6), and whatever the terminal's own background is, that is
what the text sits on. Three things follow that a setting cannot fix:

- **The contrast is not the app's to guarantee.** `dark` was tuned against
  `#0f1115` and `light` against `#fafafc` — the canvases the README's
  screenshots are rendered on — but a terminal whose background is `#300a24` or
  translucent gets the same foregrounds, and nobody measured those.
- **There is no way to turn styling off.** A terminal that renders attributes
  badly, an e-ink panel, a screen reader's review cursor — all get colours and
  attributes regardless. `NO_COLOR` is ignored (roadmap, "on demand").
- **There is no way to bring a palette of one's own** (roadmap, "A theme from a
  color configuration — idea").

The track adds a **colour mode** above the theme:

| Mode | Background | Foreground | Attributes |
|---|---|---|---|
| `system` (default) | the terminal's | `interface.theme`, as today | as today |
| `mono` (stage 2) | the terminal's | the terminal's | none |
| `full` (stage 1) | the theme's **canvas** | the theme's | as today |

## 2. What the code looks like today

Read-only survey, 2026-09-27.

- **Every role colour goes through `Palette`** (`src/shared/theme.rs`). Outside
  it: the logo's two brand colours (`widgets/logo.rs`), and code highlighting,
  which `build_code_theme` derives from the same palette
  (`shared/markdown/code.rs`).
- **Attributes do not go through anything.** Widgets set bold, dim, italic,
  underline and reverse directly — 57 places in production code.
- **A palette is built in three places** — `screens/chat/mod.rs`
  (`set_settings`), `screens/settings/render.rs` (per frame, from the working
  copy) and `app/runtime/dispatch.rs` (the broadcast to overlay screens) — and
  **a frame is drawn in one**: `app/runtime/mod.rs::draw_frame`.
- **Nothing paints a background under the text.** The only backgrounds are
  `keycap_bg` (the selection backdrop, the keycaps, inline code), the reverse
  video of an unhighlighted code block and of five popup lists. In the
  committed screenshot dumps that is 0.8–12.9 % of a frame's cells.
- **A value `settings.json` holds that the binary cannot parse resets the whole
  configuration** — `main.rs` reads it with `load_config().unwrap_or_default()`,
  and the start-up migration gate only parses the file as untyped JSON. This
  decides the shape of the new fields (§4.1).
- **User-supplied files have a precedent**: external locales
  (`data/locales/<code>.json`, `shared/i18n.rs`) — scanned once at start-up into
  a process-wide registry, a broken file logged and skipped, the value type kept
  `Copy` by interning, a template written by `mindfork locales export`. The
  settings row that lists them (`ILanguage`) is an ordinary `Choice`.

## 3. What was measured

### 3.1 Contrast of the shipped palettes

WCAG 2.x contrast ratio, each role against the canvas its palette was tuned for
and against the selection backdrop (`keycap_bg`) — any row can be the selected
one, so text has to be legible on both.

| Role | `dark`: canvas / selection | `light`: canvas / selection |
|---|---|---|
| `text` | 11.74 / 9.66 | 15.65 / 12.34 |
| `muted` | 5.00 / **4.12** | 4.52 / **3.57** |
| `error` | 5.68 / 4.67 | 6.85 / 5.40 |
| `assistant`, `success` | 8.58 / 7.06 | 4.93 / **3.89** |
| `tool`, `warning` | 9.31 / 7.66 | 4.67 / **3.68** |
| `tool_soft` | 11.87 / 9.76 | 5.12 / **4.04** |
| `keycap_fg` | 5.43 / **4.47** | 8.03 / 6.33 |
| code comments | 4.78 / **3.94** | 4.89 / **3.86** |
| `border_focus` | 4.07 | 4.03 |
| `border` | 1.66 | 1.73 |
| `keycap_bg` against the canvas | 1.22 | 1.27 |

Bold marks a value under 4.5:1, the WCAG AA floor for text. Body text is
comfortable in both; what falls short is secondary text **on a selected row**,
and in `light` two role colours as well.

The smallest move along OKLab lightness (hue and chroma kept) that clears 4.5:1
on both grounds:

| Palette | Role | Was | Becomes | Canvas / selection |
|---|---|---|---|---|
| `dark` | `muted` | `#7e848b` | `#858b92` | 5.49 / 4.52 |
| `dark` | `keycap_fg` | `#848a92` | `#858b93` | 5.50 / 4.53 |
| `dark` | code comments | `#808080` | `#8a8a8a` | 5.47 / 4.50 |
| `light` | `assistant`, `success` | `#008000` | `#007400` | 5.76 / 4.54 |
| `light` | `tool`, `warning` | `#a06400` | `#905500` | 5.78 / 4.56 |
| `light` | `tool_soft` | `#965f00` | `#8e5700` | 5.73 / 4.52 |
| `light` | `muted` | `#6e747c` | `#5f646c` | 5.71 / 4.51 |
| `light` | code comments | `#6e6e6e` | `#636363` | 5.76 / 4.55 |

Three roles of sixteen in `dark`, seven in `light`; the largest move is 0.054 of
OKLab lightness. `light`'s `user` is the named ANSI blue — a terminal's choice,
which a palette that owns its background has to replace with an RGB value.

**There is no single "optimal" contrast to tune for.** Body text on background
in themes people choose *because* they are easy on the eyes:

| Theme | Ratio |
|---|---|
| Solarized Light / Dark | 4.13 / 4.75 |
| One Dark | 6.57 |
| Catppuccin Latte / Mocha | 7.06 / 11.34 |
| Tokyo Night | 8.10 |
| Nord | 9.25 |
| Gruvbox Light / Dark | 10.22 / 10.75 |
| Dracula | 13.36 |
| mindfork `dark` / `light` | 11.74 / 15.65 |

A threefold spread among well-liked palettes says the upper end is taste. So
the app holds **floors** (a test) and leaves the rest to themes (stage 3).

### 3.2 What a frame carries that only styling says

The committed dumps, printed with every style dropped, stay readable — the
redesign duplicated most state in glyphs (`▌` on the selected row, `[x]`,
`‹ value ›`, `● ready`). An inventory of production rendering code found **22**
places where that is not so; they are stage 2's work list (§7).

### 3.3 Terminals

- **`NO_COLOR`** (no-color.org) covers colour only, and says a program's own
  configuration overrides the variable.
- **Asking the terminal to change its own background (OSC 11) is not a base to
  build on.** Windows Terminal learned to *reset* the colours (OSC 110/111/112)
  only in April 2025 (microsoft/terminal PR 18767); an app that died between
  the set and the reset would leave the user's terminal repainted.
- **ratatui 0.30.2 does not repaint the trailing cell of a wide glyph**, and
  conhost leaves that cell's old background in place (ratatui issue 2652; fixed
  upstream on 2026-09-03, unreleased as of 2026-09-27). See §8.

## 4. Design

### 4.1 Configuration

```jsonc
"interface": {
  "theme": "auto",          // unchanged: the system mode's foreground palette
  "theme_mode": "full",     // new, optional: "system" | "full" (| "mono", stage 2)
  "full_theme": "dark"      // new: the full mode's theme, by name
}
```

- **`interface.theme` is not touched.** Its enum gains no value. An older binary
  reading a file with a value it does not know loses the *whole* configuration
  (§2), so everything new is a new field, which an older binary ignores.
- **`theme_mode` is optional and is written only when chosen.** Absent means
  "whatever the environment's default is" — `system` today, and the hook stage
  2 hangs `NO_COLOR` on. The settings row stores the default as absence, so
  resetting the row really does go back to following the environment.
- **`theme_mode` parses leniently**: a value this binary does not know reads as
  absent, with a line in the log. Stage 2 adds `mono`; a stage-1 binary meeting
  it must fall back to `system`, not to a default configuration.
- **`full_theme` is a name**, not an enum: `dark` and `light` are built in, a
  user theme (stage 3) is its file's stem. An unknown name falls back to `dark`
  with a line in the log and is kept in the file.
- No schema step: both fields are additive (`#[serde(default)]`, ADR 0006 F12).

### 4.2 One pass over the finished frame

```rust
// shared/ui.rs
pub fn paint_canvas(buf: &mut Buffer, palette: &Palette);
```

Called once, as the last thing `draw_frame`'s closure does — after the screen
and after the help overlay. For every cell: a background that is still `Reset`
becomes the palette's canvas, a foreground that is still `Reset` becomes its
text colour. In the system mode the palette has no canvas and the pass returns
at once — the frame is byte-for-byte what it is today.

Why a pass rather than a base style under every widget:

- **It cannot be forgotten.** A popup's `Clear` resets its cells to the
  terminal's default; a new widget would have to remember to restore the
  canvas. The pass runs after all of them.
- **It reaches what bypasses the palette** — the logo, highlighted code,
  whatever a future widget hard-codes.
- **The precedent exists**: `dim_background` is the same walk over the same
  buffer.

The canvas is a field of `Palette` (`canvas: Color`, `Reset` = the terminal's
own), so everything that already receives a palette receives it, and the feed
and syntect caches, keyed by `Palette`, invalidate themselves when it changes.

### 4.3 One constructor

`Palette::for_interface(&InterfaceSettings)` replaces the three
`Palette::for_theme(theme).with_compat(flag)` call sites: mode, theme, full
theme and the compatibility flag are resolved in one place. `for_theme` stays
for the system mode's three palettes and for the tests.

### 4.4 Built-in themes

`dark` and `light`, retuned per §3.1 and **shared by both modes** (fork B): the
full mode's `dark` is the system mode's `dark` plus its canvas. The canvases are
the ones the palettes were tuned against — until now test-only constants of
the screenshot renderer, promoted to production as `CANVAS_DARK`/`CANVAS_LIGHT`.

The floors are a test over the built-in palettes, not a comment:

| What | Floor | Against |
|---|---|---|
| `text` | 7:1 | canvas and selection backdrop |
| every other text role, code greys | 4.5:1 | canvas and selection backdrop |
| `border_focus` | 3:1 | canvas |

The plain `border` and the selection backdrop itself are deliberately outside
the floors: a backdrop with 3:1 against the canvas would leave no room for
4.5:1 text on top of it, and the selected row is marked by the `▌` rail.

### 4.5 Settings

*Interface → Appearance* gains one row and re-purposes one:

- **Colour mode** — `system` / `full` (`mono` joins in stage 2).
- **Theme** — in the system mode the row is today's (`auto` / `dark` /
  `light`); in the full mode it lists the full themes. Two fields behind one
  label, so each mode remembers its own choice.

Both apply live, like the theme does today: the settings screen rebuilds its
palette per frame from the working copy, and the saved configuration comes
back to every other screen as a `Settings` event.

### 4.6 When the canvas changes

On a change of canvas — the first frame in the full mode, a theme or mode
switch, a terminal resize — the terminal is erased **with the new canvas as the
current background** before the frame is drawn, and the frame is repainted in
full. On terminals that paint both halves of a wide glyph this is cosmetic; on
conhost it is what gives the halves ratatui never repaints (§3.3) the right
colour to keep. `terminal.clear()` flickers and the project avoids it for
routine repaints (lessons §5); these events are rare, and an erase is the only
way to set the colour of a cell that is never written.

## 5. Out of scope

- **The terminal's own padding and its cursor colour.** Neither is a cell. Both
  are candidates for OSC 11/12 with a reset on exit, *after* the live check says
  whether they need it (§8).
- **A softer light canvas.** `#fafafc` is nearly white; `#f5f2ea` would keep
  text at 14.58:1. A taste question, better decided looking at rendered
  previews, and a one-line change once user themes exist.
- **Syntax colours of their own.** Highlighting keeps deriving from the role
  colours.

## 6. Forks — settled

**User's decisions, 2026-09-27: all four as recommended.**

**A. How strict is monochrome?** *Decided: no colours and no attributes, with
reverse video kept for a text selection and a search match only.* Those have no
glyph to fall back on; reverse video is not a colour and every terminal renders
it. Spelling marks are not drawn in this mode — the suggestions for the word
under the cursor still open.

**B. Retune the shipped `dark`/`light`, or keep them and add separate full
palettes?** *Decided: retune, and share them between the modes.* One source of
truth, moves too small to read as a different colour (§3.1). The price is the
regenerated screenshots.

**C. `NO_COLOR`.** *Decided: when the variable is set and no mode was chosen,
start in monochrome.* A mode chosen in settings wins, as the convention says.

**D. A user theme that names only some roles.** *Decided: the missing roles are
taken from the built-in theme matching the background and moved along lightness
until they clear the floors.* A colour the file names is never altered; a low
contrast there is reported, not corrected.

## 7. Stages

Each is its own branch and PR.

1. **The framework and the full mode** — §4: the fields, the two settings rows,
   the pass, the built-in themes with their canvas, the floors as a test, the
   erase on a canvas change. Go/no-go is a **live check in real terminals**
   (§8): Windows Terminal, conhost, one Linux terminal.
2. **Monochrome** — the pass gains its second job (drop colours and
   attributes), the 22 places of §3.2 get a glyph or, for the two fork A names,
   reverse video; `NO_COLOR`.
3. **User themes** — `data/themes/<name>.json`, the registry, the row listing
   them, `mindfork themes export`, the directory in the backup's list, the
   contrast fit and its report, a chapter in the manual.

## 8. Risks and the live check

**Wide glyphs on conhost.** ratatui resets a wide glyph's trailing cell to the
default style and leaves it out of the diff; conhost does not repaint that cell
with the glyph, so it keeps the background it had. Today that shows on a
selected row. In the full mode "the background it had" is the terminal's own,
everywhere — unless the screen was erased with the canvas first, which is what
§4.6 does. **That the erase is enough on conhost is a hypothesis until someone
looks at it**; the app cannot be started without a terminal, so the agent that
wrote this cannot.

**What the app does not paint.** The margin a terminal keeps around its grid
stays the terminal's colour. The cursor is the terminal's too, and Windows
Terminal is known to lose it on backgrounds an application sets
(microsoft/terminal issue 3647).

**The syntect theme cache leaks one theme per distinct palette.** Its comment
says three exist. With the modes there are five; with user themes, as many as
the user switches between in one run — a few hundred bytes each, bounded by the
theme files on disk.

The checklist for stage 1's go/no-go, per terminal:

1. Settings → Interface → Colour mode → `full`: the whole window takes the
   canvas at once, no stripe of the old background anywhere in the grid.
2. Theme → `light`, then back to `dark`: the same, in both directions.
3. A chat with emoji and CJK in the feed; scroll it: no half-cell of another
   colour behind a wide glyph.
4. Open and close a popup (`F1`, the emoji picker): nothing of it remains.
5. Resize the window: the new area is canvas-coloured.
6. The input box: the cursor is visible on both themes.
7. Colour mode → `system`: the terminal's own background is back.
