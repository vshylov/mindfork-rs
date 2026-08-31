# Status hints, unified across screens — track design

**Status:** done. Forks decided by the user 2026-08-31 (all as recommended).
One PR, branch `feat/status-hints-unified`. The journal entries live in
[docs/journal/ui-screens.md](../journal/ui-screens.md).

The chat screen's status bar was reworked twice in the last week — the hints
became a right-aligned corner block that sheds rather than wraps (PR #423), and
the `Esc` label learned that a running turn outranks the back-stack (PR #424).
The other five screens were not part of either change, and the gap is now
visible in two ways at once: their hint rows are laid out by a **different**
grid (left-aligned, ragged right), and their hint **lists** are static where the
keys they name are not.

## 1. Problem

### 1.1 Two grids

There are two implementations of "lay hotkey hints out in columns":

| Renderer | Alignment | `danger` keycap | Used by |
|---|---|---|---|
| `status_bar::hotkey_lines` | right | no | settings |
| `Palette::hotkey_grid` | **left** | yes | chat list, search, self-model, changes |

Both compute the same cell widths and both pick "the most columns that fit";
they differ only in where the block lands and how an incomplete bottom row is
placed. The committed demo dumps (116 columns) show the result side by side:

```
chat       ● chat  ● emb    Ctrl+W  mouse: select   F1  help  …  Ctrl+Q  quit|
settings              Tab/↑↓  section   Enter  parameters  …    Ctrl+Q  quit|
chat list | ↑↓ PgUp/Dn Home/End  select   Enter  open  …  Ctrl+R  auto-name
           Ctrl+N  new                    Ctrl+D  clone  …  Esc  back
self-model| Enter  edit   Space  goal status   Del  delete   Ctrl+K  clear …
```

The chat bar's own history is the argument for one grid: right alignment was
adopted there precisely because a left-aligned block "merged and visually
competed" with the indicators on its left, and because an incomplete wrapped
row reads as a column when it lands under the columns above and as a floating
group when it does not (journal, *status bar — pill top-left, hotkey grid
right*). Nothing about that reasoning is chat-specific.

### 1.2 Hints that name keys which do nothing

The rule the project already states, in spec §11.2: **an advertised key that is
a no-op is worse than a missing hint.** The chat list follows it — `Del` and
`Ctrl+D` disappear on a sub-agent transcript row, `Ctrl+G` outside content mode,
`Ctrl+O` on a chat with no transcripts. No other screen does, and reading each
key handler against its footer turns up eight places where the footer is wrong:

| Screen | Hint | What the handler does |
|---|---|---|
| self-model | `Enter edit` on an observation | `begin_edit`: `RowAction::Insight(_) => return None` — "insights aren't edited, only deleted" |
| self-model | `Space goal status` off a goal | no-op on every row but `RowAction::Goal` |
| self-model | `Del delete` on summary/traits/interests/relationship | no-op — only goals and observations delete |
| self-model | *(missing)* `↑↓`, `Ctrl+Q` | both handled, neither advertised |
| changes | `R put this file back` on a deleted file | `is_revertable()` refuses `FileState::Gone`, so the confirmation never arms |
| changes | `↑↓ file` with the diff pane focused | the arrows scroll the diff there (`step` branches on `focus`) |
| changes | *(missing)* `Ctrl+Q` | handled — and it "punches through everything, including the confirmation" |
| settings | `←→ choose`, `Space toggle` | each is a no-op off `FieldKind::Choice` / `FieldKind::Toggle` |
| all but chat | *(missing)* `F1` | opens the help on every screen (`app/runtime/input.rs`), advertised only on the chat bar |

The user's report names the first row of that table: an observation on the `F3`
screen is selected, the footer says `Enter edit`, and `Enter` does nothing.

This is the same shape as the defect PR #424 fixed, described in
docs/lessons.md: *deriving a label proves it tracks the axis you chose — and
nothing about the axis you did not model*. Here the labels are not derived at
all — they are five constants in an array — so they track nothing.

## 2. Decision

### 2.1 One grid, right-aligned, in `shared/ui.rs`

One renderer, `shared::ui::render_hotkey_grid`, produces every hint row in the
application. It takes the layout the chat bar established — cells filled
row-by-row left-to-right, columns lined up vertically, an **incomplete bottom
row right-aligned under the columns above** — plus the two things only the chat
bar needs (a lead cluster on the top row for the status pill, and one
accent-highlighted cell for the mouse-mode light) and the one thing only the
screens need (the red `danger` keycap). `shared/ui.rs` is the home because it
already owns `screen_chrome`, the helper that renders these rows, and because
`shared::ui` may depend on `shared::theme` while the reverse is not true.

`Palette::hotkey_grid` and `status_bar::hotkey_lines` both go away.

**The screens wrap, they do not shed** (user's decision, 2026-08-31). The chat
bar's `HINT_ROWS_MAX = 2` cap and its `keep_order` shed list exist because the
hints share a row with a status pill that swells at every turn boundary; a
full-screen panel's footer has the whole width and no competitor, so it grows a
row instead of hiding a key. This is what the settings screen already does, and
it keeps the shed order — a genuinely awkward thing to define — confined to the
one surface that needs it.

### 2.2 Hint lists derived from the screen's own state

Each screen builds its footer from the same state its key handler reads, in the
same frame. A key that would be a no-op right now is **not shown** (user's
decision, 2026-08-31) — the chat-list rule, applied everywhere. Dimming was
rejected: it would add a third keycap style beside normal and danger, and it
contradicts a rule the code and the spec already state.

Per screen:

- **Self-model** (`F3`). The footer follows `selected_action()`:
  `Enter` is shown for the six editable rows and reworded to *add* on the
  "add a goal" row; `Space` only on a goal; `Del` only on a goal or an
  observation. `↑↓ select`, `F1 help` and `Ctrl+Q quit` join the row.
  `Ctrl+K clear` and `Esc close` are unconditional — they are about the whole
  model, not the selection.
- **Changes** (`F4`). `↑↓` is worded for the focused pane (*file* /
  *scroll the diff*); `R` is dropped on a file that cannot be put back;
  `F1` and `Ctrl+Q` join the row.
- **Settings** (`Ctrl+P`). Inside the fields pane, `←→ choose` shows only on a
  `Choice` field, `Space toggle` only on a `Toggle` field, and `Del reset` only
  where `reset_field` would do something. `F1 help` joins the row.
- **Search** (`Ctrl+G`) and the **chat list** (`Esc`). `Enter` is dropped when
  there is nothing selected to open; `F1 help` joins the row. Everything else
  on the chat list is already contextual.

`F1` sits immediately before `Esc` everywhere, which is its position in the chat
bar's own list.

### 2.3 What stays the same

- The `F1` dialog's per-screen sections keep listing **every** key, including
  the ones the footer hides for the current selection — that is what makes
  hiding safe, and it is why the chat bar sheds `F1` last.
- The keycap styling, the `GAP` of 3 columns, and the cell-width formula
  (`keycap + 1 + description`) are unchanged; only the block's position and the
  bottom row's placement change.

## 3. Stages

One PR. The grid change and the relevance change are not separable in practice
— every screen's footer is rewritten by the second, and the first only decides
where the result lands — and neither is a mechanical refactor whose diff could
be read on its own (AGENTS.md §2).

## 4. Verification

Pure UI: no live run required (AGENTS.md §3). The real render is exercised
headlessly through the screenshot pipeline, whose dumps are regenerated and
committed (`cargo test dump_demo_frames -- --ignored`, then
`python tools/screenshots.py`).

Unit tests, per screen: the grid's right edge is flush and a wrapped bottom row
lands under the columns above; each conditional hint appears on the rows where
its key works and is absent on the rows where it does not — asserted by moving
the selection, not by calling the builder with a hand-made state.
