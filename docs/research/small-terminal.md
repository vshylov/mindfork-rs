# Research: a terminal window too small for the layout

**Status:** research, 2026-09-30. Measurements done; the §5 forks are **open** —
nothing here is implemented, and nothing is to be until they are answered.

Reported by the user with three screenshots of 0.13.0 in Windows Terminal (`ru`
locale): the chat at 57×5, the settings at 45×6, the chat list at 45×2 — "not
great, especially on the chat screen". The roadmap already carries the smallest
possible answer as an *on demand* item, a "terminal too small" message
([public-release-readiness.md](public-release-readiness.md) §2.4); this note is
about what the right answer is.

**Related:** spec §11.1, §11.3, §11.5–§11.7; architecture §10;
[status-hints-unified.md](../history/status-hints-unified.md) §2.1 (the footers'
"wrap, never shed" rule); [lessons.md](../lessons.md) §5.

## 1. What happens today

The chat, as the layout hands it out at 57 columns (English locale here and
below — the repository's documents are English; the `ru` rows are wider, which
only moves the thresholds):

```
57x8 — the smallest window in which every part is whole
╭ ◆ Gemma 4 on a 12 G…  gemma-4-12B-it-Q5_K_M · 16k ctx ╮
│▌                                                      █
╰───────────────────────────────────────────────────────╯
╭ input · Enter send · Shift+Enter newline ─────────────╮
│❯                                                      │
╰───────────────────────────────────────────────────────╯
● chat  ● emb       Ctrl+W  mouse: select    F1  help
                    Esc  chats               Ctrl+Q  quit

57x6 — the input has no text row; the prompt sits on its bottom border
╭ input · Enter send · Shift+Enter newline ─────────────╮
╰❯ ─────────────────────────────────────────────────────╯
● chat  ● emb       Ctrl+W  mouse: select    F1  help

57x5 — the reported frame: the prompt and the cursor are on the status bar
╭ ◆ Gemma 4 on a 12 G…  gemma-4-12B-it-Q5_K_M · 16k ctx ╮
│▌                                                      █
╰───────────────────────────────────────────────────────╯
╭ input · Enter send · Shift+Enter newline ─────────────╮
●❯ hat  ● emb       Ctrl+W  mouse: select    F1  help
```

Three mechanisms produce everything in the screenshots.

1. **The chat's rows are left to the solver.** `ChatScreen::render` asks for
   `[Min(3), Length(banner), Length(input), Length(status)]`
   (`screens/chat/render.rs`). Below 8 rows these cannot all hold; ratatui ranks
   `Min` above `Length` (`ratatui-core` 0.1.2, `layout/constraint.rs`), so the
   feed keeps its three rows to the end and the input and the status bar lose
   theirs in the solver's order — which is not an order anyone chose: the input
   loses its text row while the status bar still has two rows of key hints.
2. **The input box draws outside its rectangle.** `InputBox::render` takes
   `block.inner(area)` and draws the prompt into `Rect { height: 1, ..inner }`.
   When the box has fewer than three rows `inner` has height 0 and sits on the
   row *below* the box, so the forced one-row rectangle is the status bar's (or
   the box's own bottom border). The cursor is placed by the same arithmetic.
   ratatui clamps neither: `Frame::set_cursor_position` stores what it is given.
3. **Every other screen's footer wraps and never sheds**
   (`shared::ui::screen_chrome`: `[Min(3), Length(footer rows)]`, the user's
   decision of 2026-08-31). The panel is what gives way, down to its border.

## 2. Measurements

Taken on this machine, 2026-09-30, by a scratch probe (not committed): every
screen drawn through the real `app/runtime::compose_frame` into a `TestBackend`,
the buffer and the cursor read back. **The instrument was checked against the
report**: at 57×5 it reproduces the first screenshot cell for cell, the prompt
over the chat chip included. The sweep's panic trap was given a control panic
and caught it.

### 2.1 Nothing panics

21 runs of 6 262 frames each — every size from 0×0 to 100×30, shrinking and
then growing through one live screen object, the way a resize goes — 131 502
frames, **0 panics** (3.6 ms a frame in a debug build). The states: the chat
(idle in both locales, with a wrapped draft, generating, indexing banner,
impersonation preview, in-feed search, help dialog, emoji picker, tool
confirmation), the chat list, the settings (sections, a field, `Enter` on a
choice field, the text editor, the field search), the self-model, the message
search, the changes and the tasks screens. One run measured the plain chat a
second time: the key meant to open the destructive-action confirmation did not
open it. **Not reached**, then: that confirmation, the spelling suggestions,
the profile and `chat://` pickers, the settings' model picker.

So the defect class is "a wrong frame", not "a crash" — 0×0 included, which is
a size real hosts report (§2.6).

### 2.2 The chat by height

57 columns, idle, empty draft, no banner. "Rows" are what the solver gives.

| window rows | feed | input | status | on screen | cursor |
|---|---|---|---|---|---|
| 8 | 3 | 3 | 2 | everything; the feed's one row is the blank row under the last message | on its row |
| 7 | 3 | 2 | 2 | no input text row; the prompt drawn on the input's bottom border | on that border |
| 6 | 3 | 2 | 1 | the same | on that border |
| 5 | 3 | 1 | 1 | the input is its top border; the prompt drawn over the chat chip | on the status bar |
| 4 | 3 | 1 | 0 | no status bar; the prompt falls outside the frame | below the last row |
| 3–1 | all | 0 | 0 | the feed's border | below the last row |

- **The smallest whole height is 7–9**: 8 where the key hints take two rows
  (measured at 100 columns and below in `en`, at 120 and below in `ru`), 7
  where they fit one, one more while the indexing banner is up.
- At that size **two rows of eight are content** (one feed row, one input row),
  and the feed's one row is blank: the feed follows the tail, and the tail of a
  message block is its padding row.
- The cursor is outside the frame at every height ≤ 4, and at every width ≤ 3.

### 2.3 The chat by width

- **The model caption outranks the chat's title.** The title is cut to what the
  caption leaves: at 45 columns `Gemma…` beside a whole 33-column
  `gemma-4-12B-it-Q5_K_M · 16k ctx`; at 30 the title and its marker are gone and
  the caption itself is clipped from the left (`╭a-4-12B-it-Q5_K_M · 16k ctx ╮`).
- **The status pill is drawn unwrapped and clipped** at the window's edge with
  no mark. A generating `ru` pill with a token counter is 63 columns
  (`status_bar.rs` tests), so under 63 the part that falls off is the counter —
  the one that moves (lessons §5, the indexing banner's trap).
- The input box's title (44 columns in `en`, 48 in `ru`, corners included) is
  clipped below that; it is a hint, and harmless.

### 2.4 The footers

Rows the footer takes, by window width (`ru` / `en`), at a height where nothing
else constrains it:

| screen (hints) | 120 | 100 | 80 | 70 | 57 | 45 | 30 |
|---|---|---|---|---|---|---|---|
| chat list (13) | 4 / 3 | 5 / 4 | 7 / 5 | 7 / 7 | 13 / 7 | 13 / 13 | 13 / 13 |
| settings, on the sections | 2 / 1 | 2 / 2 | 2 / 2 | 3 / 3 | 4 / 3 | 4 / 4 | 7 / 7 |
| settings, on a field | 2 / 2 | 2 / 2 | 3 / 3 | 3 / 3 | 3 / 3 | 5 / 5 | 9 / 9 |
| message search | 1 / 1 | 2 / 2 | 2 / 2 | 3 / 3 | 5 / 5 | 5 / 5 | 5 / 5 |
| changes | 1 / 1 | 2 / 2 | 2 / 2 | 2 / 3 | 4 / 4 | 4 / 4 | 7 / 7 |
| self-model | 1 / 1 | 1 / 1 | 2 / 2 | 2 / 2 | 2 / 2 | 3 / 2 | 6 / 3 |
| tasks | 1 | 1 | 1 | 1 | 1 | 2 | 2 |

- This is not a tiny-window number. **At the classic 80×24 the `ru` chat list
  spends 7 rows of 24 on its legend**; at the reported 57 columns it spends 13,
  so a 57×14 window shows the list's title and one chat.
- The grid keeps its columns aligned, so one wide cell decides the column count
  for all thirteen: two columns need 59, and the window has 57.
- **A short window clips the footer from the bottom**, and every list ends with
  `F1`, `Esc`, `Ctrl+Q` — the three that matter most go first:

```
45x6 — the settings: one row of content, and no F1, Esc or quit below it
╭ ⚙  Settings ──────────────────────────────╮
│ ▸ Sections           │  Mode              █
╰───────────────────────────────────────────╯
       Tab/↑↓  section    Enter  parameters
       /  search          Ctrl+Z/Y  undo/redo
       F1  help           Esc  close
```

- The settings panel itself wants 46 columns (a 24-column menu, 20 for the
  fields, the border); below that the solver squeezes the menu to a few letters.

### 2.5 A modal's key legend

The keys that answer a popup are a `title_bottom` on its border, and a title
longer than the border is clipped without a mark. Widths the legends need,
corners included:

| popup | `en` | `ru` |
|---|---|---|
| tool confirmation (spec §9.8) | 57 | **69** |
| emoji picker | 43 | 44 |
| `chat://` picker | 38 | 42 |
| profile picker | 41 | 41 |
| spelling suggestions (popup is 40 wide) | 32 | 36 |
| destructive confirmation (popup is 56 wide) | 26 | 26 |

```
45x10 — the tool confirmation: the key that runs the call is not on screen
╭ Tool call ────────────────────────────────╮
│Run this call?                             │
│python_exec                                │
│print(1)                                   │
╰ allow for this turn · Esc — decline ──────╯
```

In `ru` that starts **below 69 columns** — the reported 57-column window is
already inside it. The popup's own comment sets the bar ("every disagreement is
a consent given to something the user could not see"), and a height that cannot
hold the body cuts the code being approved the same way. This one is the same
defect class as lessons §4, and it is not about tiny windows at all.

The help dialog at 57×5 is its tab strip and no content row.

### 2.6 What a terminal can be shrunk to

| host | floor, in cells |
|---|---|
| Windows Terminal | 2×2 per pane (`MINIMUM_VISIBLE_CELLS`, `TermControl.cpp`) — the window's own minimum is wider (the reported windows: 57×5, 45×6, 45×2) |
| alacritty (`MIN_COLUMNS`, `MIN_SCREEN_LINES`) | 2×1 |
| VTE (GNOME Terminal), as reported by the §3 survey | 2×1 |
| tmux pane, as reported by the §3 survey | 1×1 |
| a container's pty right after start, Emacs `eshell` | **0×0** with no error ([moby#43229](https://github.com/moby/moby/issues/43229), [crossterm#891](https://github.com/crossterm-rs/crossterm/issues/891)) |

The sizes that are *used*, not merely reachable: a half- or quarter-tile
(about 60–100 columns), a tmux split (any width, 8–15 rows), a phone's SSH
client with its keyboard up (about 45–60 × 8–15).

## 3. Prior art

Collected by a sub-agent from upstream sources on 2026-09-30; the rows marked ✓
were re-read by hand in the raw file the same day.

| application | below its size | threshold |
|---|---|---|
| btop ✓ | a placeholder — `Terminal size too small:` / `Width = W Height = H` / `Needed for current config:` / `Width = … Height = …`; quit and the box toggles stay live ([btop.cpp](https://github.com/aristocratos/btop/blob/main/src/btop.cpp)) | computed from the boxes shown; 80×24 with all four |
| lazygit ✓ | squashes side panels below 28 and 21 rows, then one view: `Not enough space to render panels` ([layout.go](https://github.com/jesseduffield/lazygit/blob/master/pkg/gui/layout.go)) | computed; height < max(9, panels + 4) or width < 10 |
| ncdu 1.x | a warning with `i` to ignore and `q` to quit | 60×17, fixed |
| nano ✓ | never refuses: drops the title bar under 3 rows and the help rows under 5; says `Too tiny` on the status line | — |
| vim, neovim | clamp their idea of the screen to 12×2 and draw as if it were larger | — |
| tmux | drops the status line when the rows do not exceed it | pane 1×1 |
| WeeChat, irssi | a bar that does not fit is not drawn; irssi clamps to 20×1 | — |
| Codex CLI ✓ | no placeholder; the footer collapses by width in a written order, and the cursor is clamped or hidden when its rectangle is empty ([footer.rs](https://github.com/openai/codex/blob/main/codex-rs/tui/src/bottom_pane/footer.rs)) | — |
| Crush | a compact layout below 120 columns or 30 rows; the permission dialog goes **full screen** at ≤ 77 columns or ≤ 20 rows | fixed |
| gemini-cli | one breakpoint, "narrow" under 80 columns | fixed |

Two patterns, split by what the application is. **Dashboards with rigid boxes
put up a placeholder** (btop, ncdu, lazygit at its floor). **Editors, chat
clients and multiplexers shed chrome and keep the text** — hints first, then
titles and status, the input and the content last; none of the chat clients
with a readable source replaces its whole UI. Wording, where there is a
placeholder: "terminal too small", the current size and the needed one, and a
key that still works.

ratatui has no recipe for a minimum size. Its layout documentation says what
§1 shows: when the constraints cannot all hold, "the solver can return an
arbitrary solution that is close to fulfilling the constraints. The specific
result is non-deterministic when this occurs"
([concepts/layout](https://ratatui.rs/concepts/layout/)).

## 4. What holds whichever way the forks go

1. **A widget draws inside its rectangle, and the cursor is inside the frame or
   hidden.** `InputBox` draws its prompt and places its cursor only when its
   inner area has a row. This alone removes the prompt on the status bar.
2. **The screen decides what gives way, not the solver.** The chat's rows come
   from arithmetic in one pure function of the window's size and the rows each
   part asks for, tested over every size — the successor of `input_height`,
   which exists because the solver already had to be overruled once (spec
   §11.5).
3. **The shape depends on the window's size alone** — never on the draft's
   rows, the pill's width or a banner. A layout that changes shape with its
   content changes exactly while the content moves (lessons §5); one that
   changes with the window changes when the user drags its edge, which is what
   they asked for.
4. **A modal's legend is never clipped.** When it does not fit the border it
   moves into the body and wraps, and a confirmation that cannot show its
   question, its subject and its keys at once is not answerable (F2). The tool
   confirmation first: 69 columns in `ru` today.
5. **The chat's title outranks the model caption** in the feed's header: the
   caption sheds its context size, then itself.
6. **The sweep becomes a gate**: every screen at every size draws without a
   panic, with the cursor inside the frame or hidden, and with each part the
   size calls for actually on screen. Cut to the sizes where something changes,
   and over a short feed, it is seconds.
7. **No setting.** The thresholds are constants; nobody configures the size at
   which a border disappears.

## 5. Forks

### F1. The chat screen in a window its layout does not fit

- **(a) A floor only.** Below the size where today's layout is whole (about
  30×9) a placeholder replaces the screen: "window too small", the size it is
  and the size it needs. One seam (`compose_frame`), about a hundred lines, and
  every wrong frame is gone. It also makes a 60×8 tmux pane or a phone with its
  keyboard up a screen that says "too small" — sizes at which a chat has room
  for four lines of text and a prompt.
- **(b) The chrome sheds in a fixed order; the floor sits far below
  (recommended).** One geometry, one rule — *the feed keeps four rows for as
  long as there is chrome left to give up*:

  | window rows | status bar | input | feed | feed rows, 1-row draft | today |
  |---|---|---|---|---|---|
  | ≥ 11 | up to 2 rows | bordered | bordered | rows − 7 (two rows of hints) | the same |
  | 10 | 1 row | bordered | bordered | 4 | 3 |
  | 8–9 | 1 row | a bare row | bordered | 4–5 | 1–2 |
  | 6–7 | 1 row | a bare row | bare | 4–5 | wrong frame |
  | 3–5 | none | a bare row | bare | 2–4 | wrong frame |
  | < 3, or < 20 columns | placeholder | | | | wrong frame |

  The reported 57×5 becomes four rows of the conversation over `❯` and the
  text being typed. Nothing changes at 11 rows and up, so the screenshots and
  their drift gate are untouched. What it costs: the input box and the feed
  each learn to draw without a border; what the input's title says moves or is
  given up (the idle hint — given up, `F1` has it; "generating" — the status
  bar says it; the in-feed search counter — into the row; read-only — the
  status chip says it); a draft taller than one row takes at most a third of a
  bare window; and a feed with few rows ends on the last line of text, not on
  the padding under it.
- **(c) The same ladder with no floor** — at one row, the prompt alone, the way
  vim and nano go. Against it: below three rows nothing is usable, and a line
  that says so is more honest than a lone prompt.

The order inside (b) is a judgement: the status row outlives both borders here
because one row carries the engine's state and the turn's, where a border's two
rows carry a title. The other defensible order gives up the feed's border
before the input's.

### F2. What the keys do under the placeholder

- **(a) Only quit works; the placeholder says which key (recommended).** `Enter`
  under a frame that cannot show the tool confirmation is a consent to a call
  nobody read, and a message typed blind is sent blind. Resizing is the way
  out, and it is one drag.
- **(b) Every key works.** btop's choice — but there the keys toggle boxes, not
  consents.

### F3. The footers of the other screens

- **(a) As today: wrap, never shed,** and rely on the floor (F4). At 57 columns
  the `ru` chat list keeps giving 13 rows to its legend.
- **(b) Bounded (recommended): a footer takes at most a third of the window,**
  and past that it sheds. The order needs no per-screen table, which is what
  the 2026-08-31 decision was avoiding: `F1` and `Esc` first — they are on every
  screen, and `F1` opens the full list of whatever was hidden, the reason the
  chat bar already sheds it last — then the screen's own hints in the order it
  lists them, while they fit. At 80×24 nothing changes (7 rows of 24 is inside
  a third); at 57×14 the `ru` chat list's legend takes 4 rows where it takes 11
  today.
- **(c) One row everywhere below some height.** Simpler, and it takes hints
  away from an 80×24 window that has room for them.

This reopens a recorded decision, for windows it was not made about.

### F4. A screen other than the chat, below what it needs

- **(a) Each screen declares its minimum; below it, the placeholder
  (recommended).** The settings cannot be whole under 46 columns; a list needs
  a title and a few rows. The frame's minimum is the front layer's — the help
  dialog's or a popup's when one is open — so a modal that cannot be shown
  whole is covered by F2. The tool confirmation takes the whole frame in a
  small window before it gives up (Crush does the same).
- **(b) A compact form of every screen** — the settings one pane at a time, and
  so on. A track of its own, for screens one visits for a minute; not proposed.

## 6. Plan, for the recommended answers

Three stages, each a PR; pure UI, so no live run is owed — the acceptance is
the gate of §4.6 and a person's look at a real terminal at the three reported
sizes.

1. **No wrong frame.** §4.1, §4.4, §4.5; the placeholder at `compose_frame`
   with per-screen minimums (F4a) and the keys under it (F2a) — the chat's
   minimum is still today's whole layout at this stage; the gate (§4.6). Fixes
   every frame in the report, and the 69-column legend for every window.
2. **The chat sheds.** §4.2–§4.3 and the ladder of F1(b); the chat's minimum
   drops to 20×3.
3. **The footers are bounded.** F3(b), in `shared::ui::hotkey_grid` and
   `screen_chrome` — one place, every screen.

Documents per stage: spec §11.1 (a new "Small windows" subsection; §11.5 for
the input, §11.7 for the footers), architecture §10, the journal
(`ui-feed.md`, `ui-screens.md`), CHANGELOG, and the roadmap's "terminal too
small" item, which stage 1 closes.
