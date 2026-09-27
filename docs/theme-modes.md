# Colour modes — system, monochrome, full — and user themes

Track design plan. Asked for by the user on 2026-09-27: three ways of drawing
the interface — the one the app has today, one with no colours and no text
attributes, and one where the app paints its own background and text colour,
tuned for contrast — plus themes a user can write for the last one.

§6 records the forks and the user's decisions; §7 is the stage list. **Stage 1
(the framework and the full mode) and stage 2 (the monochrome mode and
`NO_COLOR`, §9) are done; stage 3 (user themes) is what is left.**

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
| `full` (stage 1) | the theme's **canvas** | the theme's | as today |
| `mono` (stage 2) | the terminal's | the terminal's | none — reverse video for a selection and a search match |

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
places where that is not so; they are stage 2's work list, and §9.3 names each
with what it got.

### 3.3 Terminals

- **`NO_COLOR`** (no-color.org) covers colour only, and says a program's own
  configuration overrides the variable.
- **Asking the terminal to change its own background (OSC 11) is not a base to
  build on.** Windows Terminal learned to *reset* the colours (OSC 110/111/112)
  only in April 2025 (microsoft/terminal PR 18767); an app that died between
  the set and the reset would leave the user's terminal repainted.
- **ratatui 0.30.2 never writes the trailing cell of a wide glyph** — it
  resets it to the default style and leaves it out of the diff (ratatui issue
  2652; fixed upstream on 2026-09-03, unreleased as of 2026-09-27). What
  colour that cell has is therefore the terminal's decision. §8 has what the
  one console this could be measured on decides.

## 4. Design

### 4.1 Configuration

```jsonc
"interface": {
  "theme": "auto",          // unchanged: the system mode's foreground palette
  "theme_mode": "full",     // new, optional: "system" | "full" | "mono"
  "full_theme": "dark"      // new: the full mode's theme, by name
}
```

- **`interface.theme` is not touched.** Its enum gains no value. An older binary
  reading a file with a value it does not know loses the *whole* configuration
  (§2), so everything new is a new field, which an older binary ignores.
- **`theme_mode` is optional and is written only when chosen.** Absent means
  "whatever the environment's default is" — `system`, or `mono` under
  `NO_COLOR` (§9.2). The settings row stores the default as absence, so
  resetting the row really does go back to following the environment.
- **`theme_mode` parses leniently**: a value this binary does not know reads as
  absent, with a line in the log. Stage 2 added `mono`; a stage-1 binary
  meeting it falls back to `system`, not to a default configuration.
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

- **Colour mode** — `system` / `full` / `monochrome`.
- **Theme** — in the system mode the row is today's (`auto` / `dark` /
  `light`); in the full mode it lists the full themes. Two fields behind one
  label, so each mode remembers its own choice. The monochrome mode has no
  colours to pick, and no row.

Both apply live, like the theme does today: the settings screen rebuilds its
palette per frame from the working copy, and the saved configuration comes
back to every other screen as a `Settings` event.

### 4.6 When the canvas changes

On a change of canvas — the first frame in the full mode, a theme or mode
switch, a terminal resize — the terminal is erased **with the new canvas as the
current background** before the frame is drawn, and the frame is repainted in
full. A cell that no frame ever writes keeps the colour the last erase gave
it, and there is one behind every wide glyph (§3.3). A terminal that gives
that cell the glyph's colours itself makes the erase cosmetic — the one
measured does (§8) — and on one that does not, the erase is what makes the
colour it keeps the canvas. `terminal.clear()` flickers and the project avoids
it for routine repaints (lessons §5); these events are rare, and an erase is
the only way to set the colour of a cell that is never written.

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

1. **The framework and the full mode** — *done* (PR 634). §4: the fields, the
   two settings rows, the pass, the built-in themes with their canvas, the
   floors as a test, the erase on a canvas change. Go/no-go is a **live check
   in real terminals** (§8): the console host was measured; Windows Terminal
   and one Linux terminal are a look.
2. **Monochrome** — *done*, §9. The pass gains its second job (drop colours
   and attributes), the 22 places of §3.2 get a glyph or, for the two fork A
   names, reverse video; `NO_COLOR`.
3. **User themes** — `data/themes/<name>.json`, the registry, the row listing
   them, `mindfork themes export`, the directory in the backup's list, the
   contrast fit and its report, a chapter in the manual.

## 8. Risks and the live check

**The cell behind a wide glyph.** ratatui resets a wide glyph's trailing cell
to the default style and leaves it out of the diff, so its colour is whatever
the terminal makes of it. In the full mode a terminal that left it alone would
show its own background behind every emoji — unless the screen was erased with
the canvas first, which is what §4.6 does.

**Measured, 2026-09-27**, with `tools/console_probe.py` — the app in a hidden
console of its own, driven by injected keys, the screen buffer read back
through the console API. Host: the inbox console host of Windows 11 Pro, build
26200. The light theme, because a cell's colours come back as the nearest
legacy colour: its canvas reads as white, the console's own background as
black.

| Step | Cells not painted | Wide glyphs | Trailing cells off their glyph's background |
|---|---|---|---|
| launched (`mindfork demo`, system mode) | all 3600 on the console's own | — | — |
| Settings → Colour mode `full`, Theme `light` | 0 of 3600 | — | — |
| the chat | 0 | — | — |
| the emoji picker over it | 0 | 44 | 0 |
| its selection moved | 0 | 44 | 0 |
| the picker closed | 0 | — | — |
| the help dialog | 0 | — | — |
| Colour mode back to `system` | all 3600 on the console's own | — | — |
| **first frame** of a run started in `full`/`light`, 3 CJK and 2 emoji in the input box | 0 | 5 | 0 |
| the same first frame, **the erase removed** (control) | 0 | 5 | 0 |

So on this host the full mode paints to the edges through every transition,
and the system mode gets the console's background back. **The control arm is
the finding**: with the erase taken out, the cells behind wide glyphs in a
first frame carry the canvas all the same — this console host gives a wide
glyph's trailing cell the glyph's colours when it prints the glyph. The erase
changes nothing here. It stays as the guard for a terminal that does not do
that — Windows 10's console host is the candidate, and was not available to
measure — at the cost of one erase per change of canvas.

What the probe cannot see: the selection backdrop against the canvas and the
dark canvas against the console's black (each pair reads as one legacy
colour), a resize (a hidden console does not resize — the decision is
unit-tested, the path is not exercised), and anything a person judges by eye.

**What the app does not paint.** The margin a terminal keeps around its grid
stays the terminal's colour. The cursor is the terminal's too, and Windows
Terminal is known to lose it on backgrounds an application sets
(microsoft/terminal issue 3647).

**The syntect theme cache leaks one theme per distinct palette.** Its comment
says three exist. With the modes there are five; with user themes, as many as
the user switches between in one run — a few hundred bytes each, bounded by the
theme files on disk.

The checklist for stage 1's go/no-go, per terminal — what is left for a look
in Windows Terminal and a Linux terminal, and for the eye on any of them:

1. Settings → Interface → Colour mode → `full`: the whole window takes the
   canvas at once, no stripe of the old background anywhere in the grid.
2. Theme → `light`, then back to `dark`: the same, in both directions.
3. A chat with emoji and CJK in the feed; scroll it: no half-cell of another
   colour behind a wide glyph.
4. Open and close a popup (`F1`, the emoji picker): nothing of it remains.
5. Resize the window: the new area is canvas-coloured.
6. The input box: the cursor is visible on both themes.
7. Colour mode → `system`: the terminal's own background is back.

## 9. Stage 2 — the monochrome mode

Done on branch `feat/theme-modes-mono`. Forks A and C (§6) are this stage's.

### 9.1 The pass, and the one thing it keeps

```rust
// shared/ui.rs
pub fn strip_styles(buf: &mut Buffer, palette: &Palette);   // the monochrome mode
pub fn finish_frame(buf: &mut Buffer, palette: &Palette);   // paint_canvas, then strip_styles
```

`finish_frame` is what `compose_frame` ends with now. `strip_styles` returns at
once unless the palette is the monochrome one (`Palette::mono`, a flag next to
`compat`); in that mode every cell's foreground, background and underline
colour become the terminal's default and its attributes are dropped.

A pass for the reasons of §4.2, only more so: attributes never went through the
palette at all (57 places set one directly, §2), so a palette of "no colours"
would have left every one of them on the screen.

**Reverse video is asked for by name.** Fork A keeps it for a text selection
and a search match. The pass cannot tell those from the reverse video a widget
sets by hand — five popups mark their selected row with `reversed()`, an
unhighlighted code block is a reversed rectangle — so "keep `REVERSED`" would
have kept all of them, and made every such widget, and the next one written,
responsible for knowing the mode. Instead a widget draws a selection or a
match **on `MONO_MARK`** (`Palette::selection`, `Palette::search_match`), a
background no palette uses — an indexed colour, where the palettes are named
ANSI or RGB — and the pass turns a cell with that background into a reversed
one. Everything else is stripped, reverse video included. What survives is
what was asked for.

**The palette is one palette**: the dark theme's colours with the flag set,
whatever `interface.theme` and `interface.full_theme` say. Nothing of it
reaches the terminal, and one value is one entry in each cache a palette keys.

### 9.2 `NO_COLOR`

- `ThemeMode::for_environment(no_color)` — monochrome when the variable is set
  to anything but an empty string, the system mode otherwise
  ([no-color.org](https://no-color.org): the value itself means nothing).
- `config::set_environment_mode`, called once from `main.rs::launch_tui`,
  records it process-wide — the shape `theme::set_detected_background` has, and
  for its reason: one fact about the process, read from render paths.
- `InterfaceSettings::mode()` is the chosen mode, or that one;
  `set_mode()` stores a mode equal to it as **absence**. So with the variable
  set, `system` is a choice and is written, and cycling back to `monochrome` —
  or `Del` — returns the file to following the environment.
- A test binary never sets the process-wide value, so the tests do not depend
  on the environment they run in; they take `mode_under`/`set_mode_under`, the
  same seam `Palette::auto_with` is for the detected background.

**The variable was not being ignored** — found by the live check (§9.6), and
the reason the roadmap's "`NO_COLOR` is ignored" was wrong. crossterm honours
it on its own: with the variable set it writes no colour, and in place of every
colour change it writes `ESC[;m` — a reset of every attribute. ratatui's backend
sets a cell's attributes *before* its colours, so under `NO_COLOR` the app had
been drawing with no colour and without whichever attribute shared a cell with
a colour change, and with nothing in their place.

So the colour mode decides what crossterm may write, per frame:
`force_color_output(!palette.mono)` (`app/runtime`, `set_colour_output`). In
the monochrome mode every cell's colours are the default and nothing is
written either way; in a mode the user **chose** over the variable, colours are
written although it is set. Without this, "a chosen mode wins" was true in
`settings.json` and false on the screen.

### 9.3 The 22 places, and what each got

Everything below is the monochrome mode's only: `palette.mono` is read, and
every other mode draws what it drew. The committed dumps did not change.

| # | Where | What only styling said | In the monochrome mode |
|---|---|---|---|
| 1 | `widgets/profile_list.rs` | the selected profile (reverse video) | `› ` before it — `ui::mark_selected`, the list's own `highlight_symbol` |
| 2 | `widgets/chat_link_picker.rs` | the selected reference (reverse video) | the same; the marker's columns come out of the title |
| 3 | `widgets/emoji_picker.rs` | the selected emoji (a backdrop) | `[😀]`, in the cell's four columns |
| 4 | `screens/settings/render.rs` | the selected result of the field search (reverse video) | `› ` before it |
| 5 | `widgets/help_dialog.rs` | the active tab (a backdrop, bold) | `[Tab]` — `Palette::tab_label` |
| 6 | `screens/settings/helpers.rs` | the active subsection tab — in Sampling the only thing saying whose parameters these are | `[Tab]` |
| 7 | `widgets/input_box.rs` | a text selection (a backdrop) | **reverse video** (fork A) |
| 8 | `widgets/input_box.rs` | a misspelled word (red underline) | not marked (fork A); `Ctrl+G` still offers corrections |
| 9 | `widgets/message_feed.rs` | a search match in the feed (the accent) | **reverse video** (fork A) |
| 10 | `widgets/message_feed.rs` | the message a jump landed on (the rail in the accent) | the rail `█` |
| 11 | `screens/search.rs` | the matched words of a snippet (accent, bold) | **reverse video** (fork A) |
| 12–14 | `shared/markdown/writer.rs` | bold, italic, strikethrough | `**…**`, `*…*`, `~~…~~` — the markers the parser took off |
| 15 | `shared/markdown/writer.rs` | inline code (a chip) | `` `…` `` |
| 16 | `widgets/chat_list.rs` | the open chat (a green dot, a bold title) | `●`, every other chat `○` |
| 17 | `widgets/chat_list.rs` | a chat listed only as a match's parent (muted) | `(title)`, the parentheses out of the title's columns |
| 18 | `screens/changes.rs` | the shown file while the diff pane has the focus (bold) | `›` before it |
| 19 | `widgets/message_feed.rs` | a `chat://` address that opens a chat (accent, underline) | `<chat://…>` |
| 20 | `screens/chat/render.rs` | the input is a command and will be run (the warning colour) | the word *command* in the input box's title |
| 21 | `widgets/input_box.rs` | the placeholder is not typed text (dim) | not drawn |
| 22 | `widgets/message_feed.rs`, `shared/markdown/writer.rs` | the wrapped rows of a thought, of the compaction summary, of a quote (muted italic) | the `│ ` / `> ` on **every** row — `wrap::wrap_hanging` |

### 9.4 Beyond the list

Three things the inventory had classed as "weakly duplicated" — a subtle cue
remains — and one it had not reached:

- **Keycaps.** A key and its description were told apart by the number of
  spaces between them. `[Enter]` instead of the pill, the same width
  (`Palette::keycap`), so no hint grid moves.
- **Whose message a row belongs to**, once the role header has scrolled away,
  was the rail's colour. The rail says it by shape: `▌` the user's, `║` the
  assistant's, `│` a system message or a note.
- **An unhighlighted code block** was a reversed rectangle, padded to be one.
  It is left to its fences: reverse video is for the two marks, and there is
  no background to square off.

Every glyph is WGL4 (the compatibility set's repertoire), so the mode combines
with `interface.terminal_compat`.

### 9.5 Left open

- **A search for a word that a marker splits finds nothing.** The feed's
  search matches the rendered text (spec §11.3), and in this mode the rendered
  text holds the markers: `mar**ker**` no longer contains `marker`. The index
  still finds the message; what is lost is the mark inside it.
- **The hanging gutter is the monochrome mode's only.** Every mode could wrap a
  thought or a quote under its `│ ` / `> `; it would change the wrap width of
  those rows everywhere, and the committed dumps with it — a decision for the
  eye, not for this stage.
- **The current match** of an in-feed search is not told from the other
  matches in any mode (the counter and the scroll position say which).
- What a terminal draws itself is not the app's: the cursor, and the terminal's
  own text selection in the feed.

### 9.6 Measured

`tools/console_probe.py`, scenarios `mono` and `no-color` — the app in a hidden
console, the screen buffer read back through the console API (§8 for the
method). Host: the inbox console host of Windows 11 Pro, build 26200;
`mindfork demo`, 3600 cells. A cell's attributes come back as the console
keeps them: its two colours, reverse video and underline as flags. "Bare"
below is *every cell carries the console's default attributes* (`0x07`).

| Step | Colour pairs on screen | Cells not bare | In reverse video |
|---|---|---|---|
| launched, system mode (the control) | 4 | 575 | — |
| Settings → Colour mode → `monochrome` | 1 | 0 | none |
| the chat | 1 | 0 | none |
| two characters selected in the input box | 1 | 0 | those two, `me` |
| the selection dropped | 1 | 0 | none |
| the help dialog | 1 | 0 | none |
| the emoji picker (44 wide glyphs) | 1 | 0 | none |
| Colour mode back to `system` | 6 | 1405 | — |
| **`NO_COLOR=1`**, first frame | 1 | 0 | none |
| `NO_COLOR=1`, Settings: the row reads `monochrome` | 1 | 0 | none |
| `NO_COLOR=1`, `system` chosen in Settings | 6 | 1416 | — |
| `NO_COLOR=` (empty), first frame | 4 | 575 | — |
| `NO_COLOR` unset, first frame | 4 | 575 | — |

**The row that failed first** is `NO_COLOR=1`, `system` chosen: before
`set_colour_output` it read 2 colour pairs, not 6 — the finding of §9.2.

**The control for that finding** — the build before this stage, the same
settings screen:

| | Colour pairs | Bold cells |
|---|---|---|
| `NO_COLOR` unset | 6 | 19 |
| `NO_COLOR=1` | 1, and the bright half of it | 10 |

No colour, and nine of the nineteen bold cells gone with it: the active
section's title, whose cell changes colour, lost its bold; the pane's title,
whose cell does not, kept it.

The stage 1 scenarios (`full-mode`, `first-frame`) were run again on this
build and read as in §8.

What the probe cannot see here: whether a terminal *renders* reverse video
legibly — the flag is measured, the look is not — and any host but this one.

### 9.7 Tests

The pass, the palette's helpers, the environment's default and every one of
the places of §9.3–§9.4 have a test that reads what is drawn; two tests hold
whole frames to the contract — every captured screen (`demo_shots`) and every
composed frame, the help dialog over it included, carry no colour and no
attribute in the monochrome mode.

Checked by mutation: 65 mutants — each of the mode's decisions broken one at a
time, in both directions where there are two ("not in the monochrome mode",
"in every mode") — every one killed. Two survived the first run and were
killed by tests written for them: a marker that did not open its own line
(reachable only where no paragraph opens it — next to a block formula, in a
tight list item after a code block), and the quoted lines wrapped first to
last (visible only with two quoted paragraphs that both wrap).
