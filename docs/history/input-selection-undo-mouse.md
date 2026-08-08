# Design plan: text selection, undo/redo, mouse in the input box (InputBox, items 8-10)

> **Status:** stages **A** (selection), **B** (clipboard + moving quit
> `Ctrl+C`→`Ctrl+Q`/`F10`), **C** (undo/redo), and **D** (mouse: click → cursor,
> drag → selection) — **all done** (see
> [docs/journal/ui-input.md](../journal/ui-input.md)). The track is complete.
> **Context:** follows the `widgets/input_box.rs` audit (custom multiline input,
> [ADR 0001](../decisions/0001-ui-crates-ratatui-030.md)). Audit items 1-7 are done
> (see [docs/journal/ui-input.md](../journal/ui-input.md), "InputBox refinements"
> + "wrap cache"). Here are the
> three remaining major gaps versus "big editors":
> **8** text selection (+ copy/cut), **9** undo/redo,
> **10** mouse in the field (click → cursor, drag → selection).
> **Method:** four stages — **A** selection → **B** clipboard → **C** undo/redo →
> **D** mouse. Each is self-contained and testable. Order: A precedes B and D
> (they build on the selection model); C is independent (can land at any point).

Cross-cutting DoD — §8. Deliberately out of scope — §9. Decision points requiring
a call **before** implementation — §2 (the main one — copy-key layout).

---

## 1. Motivation, scope, where the logic lives

`InputBox` is used in **five** places (all consumers of one widget):
the chat input box (`screens/chat`), chat rename (`widgets/chat_list`, `F2`,
single-line), the settings field editor (`screens/settings`, single-line + a large
multiline editor for the system message), the self-model editor (`screens/self_model`,
`F3`), and the settings search bar (`screens/settings/search`, `/`).

**Logic placement principle:**

- **Selection and undo/redo are state and operations of the widget itself**
  (`InputBox`). Then all five consumers get them for free through the already
  shared entry point `InputBox::on_key` (see audit item 6 — `on_key` already sees
  the full `KeyEvent`, including `Shift`/`Ctrl`). No consumer duplicates
  cursor/selection logic.
- **Clipboard is a UI-layer side-effect**, the widget doesn't touch it (FSD:
  `widgets → features → shared`, not `app`). Like writing the transcript via
  `F5` (`AppEvent::CopyToClipboard` → `runtime::deliver_clipboard` → `arboard`) and
  the mouse toggle (`ChatIntent::SetMouseCapture` → `execute!` in `dispatch`),
  copying a selection is driven **by the screen's intent, executed by runtime**.
  The widget only exposes `selected_text()` and can `delete_selection()`.

**Invariants that must not break:**

- **FSD**: `screens`/`widgets` don't import `app`; the screen returns `ChatIntent`,
  runtime executes the side-effect (architecture.md §2).
- **Wrap cache (item 7)**: selection **doesn't change row geometry** — `rows_cache`
  isn't invalidated when the selection moves/grows (the key is `(width, revision)`,
  and `revision` bumps only on a `lines` edit). So selection **doesn't call**
  `touch()`. Replacing/deleting a selection is an edit → `touch()` as usual.
- **`KeyOutcome` (item 6)**: extending selection (`Shift`+navigation) is `Moved`
  (doesn't wake spellcheck debounce / `SetDraft`). Typing/deleting over a selection
  is `Edited`. The `runtime` loop provides redraw on any selection change
  (`dirty` on any terminal event — architecture.md §4), not `KeyOutcome`.
- **Spellcheck underline sync (item 5)**: deleting a multiline selection is a
  structural edit (line merge) → reset `misspelled` for the affected lines (as in
  `join_misspelled_into_prev`), sync the `Vec` length.

---

## 2. Decision points — need a call before implementation

Marked **[R]** = requires the user's answer; recommendation in parens.

### [R] Fork 1 — selection-extension keys (recommendation: standard scheme)

| Action | Keys |
|---|---|
| Extend by character/visual row | `Shift`+`←/→/↑/↓` |
| Extend to start/end of row | `Shift`+`Home/End` |
| Extend by word | `Ctrl`+`Shift`+`←/→` |
| Extend to start/end of text | `Ctrl`+`Shift`+`Home/End` |
| Select all | `Ctrl`+`A` |

Arrows/`Home`/`End` are layout-independent; `Ctrl+A` is normalized via
`shared/keys::physical_char` (like other Ctrl shortcuts). **Legacy-terminal
caveat:** on a "bare" unix terminal without the kitty keyboard protocol,
`Shift`+arrow may arrive indistinguishable from a plain arrow (the modifier
isn't reported) — there, keyboard selection just doesn't extend (graceful
degradation; the mouse, stage D, keeps working). Same class of limitation as
`Shift+Enter` (see audit item 11, groundwork). Windows Terminal/conhost report
modifiers on arrows.

### Fork 2 — copy/cut keys (RESOLVED: Ctrl+C/Ctrl+X, quit → Ctrl+Q/F10)

**Decision (user, 2026-07-11):** `Ctrl+C` = **copy** (always, not
contextual), `Ctrl+X` = **cut**. The freed-up former quit (`Ctrl+C`)
**moves to `Ctrl+Q` + `F10`** (two alternatives — if a terminal swallows one,
the other works). The **status bar** at the bottom shows `Ctrl+Q quit`; the
**help overlay** (`F1`/`?`) shows both (`Ctrl+Q` / `F10`).

This is wider than the input box — moving quit touches **all four screens**
(chat, chat list, settings, self-model), where `Ctrl+C → *Intent::Quit`.
Handled as a separate preceding step **B.0** (§5.0).

**Compatibility caveats (as requested):**

- **`Ctrl+Q`.** Historically this is XON (resume) for XON/XOFF software flow
  control (`Ctrl+S`/`Ctrl+Q`). The app runs in **raw mode** (crossterm
  `enable_raw_mode` clears `IXON` in termios), so the terminal driver
  **doesn't intercept** `Ctrl+Q` — it reaches the app on both Linux and
  Windows. Residual (rare) risks: a multiplexer (`tmux`/`screen`) with its own
  flow control/binding on its own prefix, an emulator that re-enables flow
  control, or a DE shortcut on `Ctrl+Q` (most emulators bind quit to
  `Ctrl+Shift+Q`, so plain `Ctrl+Q` is usually free). Windows Terminal doesn't
  claim `Ctrl+Q` by default. Exactly for these rare cases we also keep **`F10`**
  as a second quit key.
- **`F10`.** On many Linux DEs it opens the terminal emulator's menu
  (GNOME Terminal/xterm), but is disabled in terminal settings almost
  everywhere. It's a mutual backstop for `Ctrl+Q` (if `F10` is intercepted,
  `Ctrl+Q` still works).
- Both keys are currently **free** on all screens (Ctrl parsing captures
  `c/p/n/r/e/u/k/g/b/t/w`, F-keys — `F1/F2/F3/F5`; neither `q` nor `F10` is
  taken).

Paste already works (bracketed paste / coalescing). In **single-line**
consumers (rename/search/settings fields) `Ctrl+C` wasn't quit anyway — there
it uniformly becomes "copy".

### [R] Fork 3 — undo/redo keys (recommendation: `Ctrl+Z` / `Ctrl+Y`)

`Ctrl+Z` (undo) and `Ctrl+Y` (redo) — both free on all screens, layout-independent
via `physical_char`. Redo alternative — `Ctrl+Shift+Z` (Mac habit), but
`Ctrl+Y` is simpler and doesn't conflict. **Caveat:** `Ctrl+Z` on a unix
terminal is normally SIGTSTP (suspend), but the app is in raw mode + without
job-control signals it arrives as a key event (crossterm), so we intercept it.
Verify on a live unix terminal (groundwork, doesn't block Windows).

### Fork 4 — undo granularity (decision: snapshots + coalescing)

The model — a **stack of snapshots** `(lines, cursor)` (not operation records —
simpler and provably correct; the input box is short, memory isn't a concern).
**Coalescing**: consecutive same-type edits merge into one undo unit. A new
snapshot is pushed before the edit if the edit "class" changed or a break
occurred:

- break by type: inserting characters ↔ deleting ↔ structural (newline/merge/
  paste/replace-selection);
- break by position: the cursor jumped away from where the last edit ended;
- forced snapshot: `insert_newline`, `insert_str` (paste), replacing a
  selection, `set_text`/`clear` (consumers, e.g. loading a draft — **must
  not** enter the user's undo: see §6.3).

Depth cap (e.g. `UNDO_CAP = 200` units) — evict the oldest. `Ctrl+K`
("clear/restore", the `cleared` buffer) becomes a **special case** of the
general undo (§6.4).

### Fork 5 — mouse requires capture (`Ctrl+W`), decision fixed by design

Mouse in the field only works **while mouse capture is on** (the `Ctrl+W`
toggle, `mouse_scroll`) — otherwise crossterm doesn't get mouse events (native
terminal selection). This is an already-accepted project decision (see
post-M9 "wheel scroll"): capture on → wheel/click go to the app, native
selection is available with `Shift` held; capture off → native mouse
selection, the app doesn't see the mouse. Click-in-field and drag-to-select
are **new consumers of the same capture**, no new toggle.

### [R] Fork 6 — clipboard scope (recommendation: chat-first)

Selection/undo — right away in all five consumers (widget level). **Copy/
cut to clipboard** — starting with only the **chat input box** (the main use
case; the side-effect plumbing is already there). Chat rename / settings
fields / self-model editor get selection + delete/replace, and clipboard
copy is groundwork (their consumers wire `Ctrl+C/X` later with the same
pattern). Alternative — all five at once (more plumbing, but uniform).
Recommendation — **chat-first**.

---

## 3. Selection model (`InputBox` widget)

New field:

```rust
/// Selection anchor (row, column) in character indices. `Some` — there's an
/// active selection [anchor, cursor] (normalized on use: start = the smaller
/// of the two positions). `None` — no selection. Cursor — existing (row, col).
anchor: Option<(usize, usize)>,
```

Methods:

- `has_selection() -> bool` — `anchor` is set and doesn't equal the cursor.
- `selection_span() -> Option<((usize,usize),(usize,usize))>` — normalized
  `(start, end)` (start ≤ end by (row, col)).
- `selected_text() -> Option<String>` — selection text (for copying).
- `set_anchor_if_none()` — before `Shift`-navigation: if `anchor == None`,
  set `anchor = (row, col)` (current cursor). Navigation then moves the
  cursor.
- `clear_selection()` — `anchor = None` (on plain navigation/edit without
  replacement).
- `delete_selection() -> bool` — deletes `[start, end)`, puts the cursor at
  `start`, `anchor = None`, `touch()`, syncs `misspelled` (see §4.3). Returns
  whether there was anything to delete.

**Interaction with existing code:**

- Plain navigation (`move_left/right/up/down/home/end/word_*/doc_*`) calls
  `clear_selection()` first (cursor without Shift collapses the selection).
  `Shift` variants call `set_anchor_if_none()` instead of `clear_selection()`,
  then the same movement.
- Editing (`insert_char/insert_str/insert_newline/backspace/delete/
  delete_word_*`) — at the start: if `has_selection()`, first
  `delete_selection()`, then the action (for `backspace`/`delete` with a
  selection — deleting the selection **is** the action, no extra
  character deletion). `replace_range` (spellcheck suggestion) ignores/
  clears the selection.
- `set_text/clear` — `anchor = None` (reset, like other state).

**Cache and `KeyOutcome`:** moving/extending the selection doesn't touch
`lines` → **doesn't** call `touch()` (the wrap cache stays intact).
`delete_selection` changes `lines` → `touch()`. `Shift`-navigation →
`KeyOutcome::Moved`; editing over a selection → `Edited`.

---

## 4. Stage A — selection (widget)

Branch `feat/input-selection`. Only `widgets/input_box.rs` (+ `shared/theme.rs`
for the highlight color) + tests.

### 4.1 Key handling (`on_key`)

Extend `on_key`:

- `Shift`+`←/→/↑/↓/Home/End`, `Ctrl+Shift`+`←/→/Home/End`:
  `set_anchor_if_none()` → the corresponding cursor move → `Moved`. (Currently
  `Shift` on arrows is ignored — the `KeyCode::Left =>` branches etc. catch
  them regardless of modifier; add branches with `SHIFT` **before** those.)
- `Ctrl+A` (phys. `a`): `anchor = (0,0)`, cursor to text end → `Moved`.
  (Currently `Ctrl+A` → `Ignored`; add a branch in the `on_key` Ctrl parsing.)
- Plain arrows/Home/End (no Shift): call `clear_selection()` first (already
  calling `move_*`, add the reset) → `Moved`.
- Editing with an active selection: `delete_selection()` before inserting a
  character/newline; `backspace`/`delete` with a selection =
  `delete_selection()` (and that's it) → `Edited`.

`Ctrl+A` via `physical_char` works with a Cyrillic layout too, and in all
consumers (their Ctrl parsing in `on_key` passes through unknown letters, see
audit item 6).

### 4.2 Rendering the selection highlight

In `render`/`render_single_line`: for each visual row, compute the
intersection of the selection `[start, end)` with the row's range (in
row-local coordinates, like `misspelled` via `clip_ranges`) and color the
**background** of those spans. Composes with spellcheck underline and command
highlighting:

- `styled_line(chars, misspelled_ranges, selection_range, palette)` — new
  parameter `selection_range: Option<(usize,usize)>`; selected characters get
  `Style::bg(palette.selection_bg)` on top of (not instead of) `fg`/underline.
  Span order — split by the union of selection and error boundaries.
- Command highlighting (`command=true`, whole text `warning`): selection
  still colors the background (the user can select the command) — keep
  `fg=warning` + bg.

**Color** — new field `Palette::selection_bg` (`shared/theme.rs`): in
dark/light/auto, a muted overlay (can reuse `keycap_bg`, like the selected
chat-list row, if a separate shade isn't needed; decide at implementation
time). Compat mode (`GlyphSet`): the background color works there too (it's a
color, not a glyph).

### 4.3 Syncing `misspelled` on `delete_selection`

A selection can span several logical lines. Deleting it merges them (like
`backspace` at a line boundary). Rule (reuse the item-5 approach):

- single line (`start.0 == end.0`): `edit_misspelled(row, start.1, end.1-start.1, 0)`
  (shift/reset ranges, like a plain deletion);
- multiple lines: remove `misspelled` entries for lines `[start.0+1 ..=
  end.0]`, reset the entry for line `start.0` (its content changes) — sync
  the `Vec` length with `lines`. The recheck (debounce) will rebuild it.

### 4.4 Stage A tests (pure, no terminal)

- `Shift+Right` sets the anchor and extends; repeat grows it; plain `Right`
  collapses.
- `Ctrl+A` selects all; `selected_text()` == the whole text.
- typing a character over a selection replaces it (`delete_selection` +
  `insert_char`).
- `Backspace`/`Delete` with a selection delete the whole selection (not one
  character).
- multiline selection: `selected_text()` via `\n`; `delete_selection` merges
  the lines and syncs `misspelled` (no panic, lengths match).
- rendering with a selection doesn't panic; selected buffer cells carry `bg`
  (TestBackend).
- `Shift`-navigation → `KeyOutcome::Moved`, typing over it → `Edited`;
  `revision` doesn't grow when extending a selection, grows on replacement
  (the item-7 cache stays intact).

---

## 5. Stage B — copy/cut to clipboard (chat-first)

Branch `feat/input-clipboard`. Builds on A. `screens/chat` + `app/runtime` +
the `ChatIntent` contract.

### 5.0 Moving quit to `Ctrl+Q`/`F10`, freeing up `Ctrl+C`

Preceding step (Fork 2). The former quit — `Ctrl+C` → `*Intent::Quit` — on
**all four** screens. Changes:

- `screens/chat/input.rs`: in the Ctrl parsing, replace the phys. `c` →
  `Quit` branch with: phys. `q` → `ChatIntent::Quit`; add `KeyCode::F(10)` →
  `Quit` in the code match. (The freed `Ctrl+C` picks up copying, §5.2.)
- `widgets/chat_list.rs` (`ChatListState::on_key`): replace phys. `c` →
  `Quit` with phys. `q`; add `F10` → `ChatListAction::Quit`. (The search bar/
  rename field have their own parsing; `Ctrl+C` there will go to copying if
  there's a selection.)
- `screens/settings/apply.rs`, `screens/self_model.rs`: same — change
  `Ctrl+C`→`Quit` to `Ctrl+Q` + `F10`.
- **Status bar** (`widgets/status_bar.rs`): the hotkey hint `Ctrl+C quit` →
  `Ctrl+Q quit` (one key in the narrow bar).
- **Help overlay** (`screens/chat/popups.rs::HELP_KEYS` and the other
  screens' help lines): show **both** — `Ctrl+Q` / `F10 — quit`.
- Layout independence: `q` via `physical_char` (like other Ctrl letters);
  `F10` is layout-independent.

Keys `q`/`F10` are free on all screens beforehand (verified: Ctrl parsing
doesn't catch `q`, F-keys are taken only by `F1/F2/F3/F5`).

### 5.1 Contract and side-effect plumbing

New intent (modeled on `SetMouseCapture` — executed in `runtime`, not passed
to the orchestrator; the data is already at the UI):

```rust
// screens/chat/mod.rs, enum ChatIntent
CopyToClipboard(String),  // write text to the system clipboard (runtime side-effect)
```

Plumbing — like `SetMouseCapture`, but needs the `arboard` slot. `dispatch`
currently doesn't get it; options:

- **Recommended:** handle `ChatIntent::CopyToClipboard` **in
  `process_input_batch`** (it already has `&mut clipboard`) — before/instead
  of `dispatch_any`, calling `write_clipboard(clipboard, &text)` and, on
  error, `screen.push_error(...)`. Success is silent (no feed note — copying
  in the editor shouldn't be noisy; a short `push_note` is possible if
  desired, decide at implementation time).
- Alternative: pass `&mut Option<arboard::Clipboard>` into
  `dispatch`/`dispatch_any` (wider signature). Less local.

Cut = copy the selection + `delete_selection()`. The screen builds
`CopyToClipboard(selected)` **and** calls `self.input.delete_selection()` +
`mark_input_changed()`.

### 5.2 Keys (Fork 2 — resolved: `Ctrl+C` copy, `Ctrl+X` cut)

In `screens/chat/input.rs::handle_key`, Ctrl parsing (after moving quit, §5.0
frees `Ctrl+C`):

- phys. `c`: if `self.input.has_selection()` → return
  `ChatIntent::CopyToClipboard(self.input.selected_text().unwrap())` and
  clear the selection (`clear_selection`); without a selection — no-op (not
  quit — that's `Ctrl+Q`/`F10`).
- phys. `x`: with a selection → the same copy intent + `delete_selection()`
  + `mark_input_changed()`; without a selection — no-op.

In single-line consumers (`chat_list` rename, `settings` editor/search,
`self_model` editor) — **groundwork** (Fork 6): their `handle_key` will wire
`Ctrl+C/X` with the same pattern later. In the first pass, selection and
`delete_selection` (via editing) already work there, but not clipboard
copying.

### 5.3 Stage B tests

- `chat/input`: with a selection, `Ctrl+C` → intent
  `CopyToClipboard(<selected>)`, selection cleared; without a selection →
  `Quit`.
- `Ctrl+X` with a selection → copy intent + the field text shortened by the
  selection.
- `runtime`: `process_input_batch`/dispatch with `CopyToClipboard` calls
  `write_clipboard` (via a mock slot; error → `push_error`, no panic).

---

## 6. Stage C — undo/redo (widget)

Branch `feat/input-undo`. Only `widgets/input_box.rs` + tests. Independent of
A/B/D.

### 6.1 State

```rust
/// Undo stack: snapshots (lines, cursor) BEFORE an edit. Coalescing — a new
/// snapshot is only pushed when the edit class changes / the position jumps
/// (see §6.2). Cap is UNDO_CAP.
undo: Vec<Snapshot>,
/// Redo stack: snapshots popped from `undo` on undo; cleared by any new edit.
redo: Vec<Snapshot>,
/// Class of the last edit and the position of its end — for coalescing.
last_edit_kind: Option<EditKind>,
```

`Snapshot { lines: Vec<Vec<char>>, row: usize, col: usize }`.
`EditKind { Insert, Delete, Structural }`.

### 6.2 Coalescing

Before a mutation — `record_undo(kind)`: if `last_edit_kind` matches `kind`,
the edit continues the previous position, and `kind != Structural` — the
snapshot is **not** pushed (it merges). Otherwise —
`undo.push(snapshot_before)`, `redo.clear()`, trim to `UNDO_CAP`. After the
mutation — `last_edit_kind = Some(kind)`, remember the end position.

- `insert_char` → `Insert`; a typing run = one unit, break on a space/newline
  (a matter of taste — could break by word instead, starting with "merged",
  decide at implementation time).
- `backspace`/`delete`/`delete_word_*` → `Delete`.
- `insert_newline`/`insert_str`/replacing a selection → `Structural` (always
  its own unit).

### 6.3 `undo()` / `redo()`

- `undo()`: if `undo` is non-empty — `redo.push(current_snapshot)`, restore
  `undo.pop()` (lines+cursor), `anchor=None`, `touch()`,
  `misspelled.clear()` (the recheck will rebuild it),
  `last_edit_kind=None`.
- `redo()`: mirror image.
- **Consumer boundaries:** `set_text`/`clear`, called **programmatically**
  (loading a draft on chat switch, `restore_input`, applying a suggestion) —
  **must not** enter the user's undo and **must** clear the history (foreign
  context). Decision: `set_text`/`clear` do
  `undo.clear(); redo.clear(); last_edit_kind=None`. User edits after that
  build their own history.

### 6.4 `Ctrl+K` → a special case of general undo (RESOLVED: replace)

**Decision (user, 2026-07-11):** replace `cleared`/toggle with general undo —
one model of undoing. `Ctrl+K` with a non-empty field =
`record_undo(Structural)` + clear the content (**not** the history — it
pushes a snapshot onto `undo`, doesn't clear the stack); then `Ctrl+Z`
restores the text. The `cleared` field and `Ctrl+K` toggle semantics
(pressing again used to restore) are **removed**. **Behavior change** — note
in spec §11.5 and the changelog (§10): pressing `Ctrl+K` again no longer
restores; restoring is `Ctrl+Z`. The `clear_or_restore_*` tests get replaced
with `Ctrl+K`→`Ctrl+Z`.

### 6.5 Stage C tests

- typing text → `undo` returns to empty in one unit (coalescing);
- typing "abc", a pause-break (structural edit), more typing → two undo
  steps;
- `insert_str` (paste) — a separate undo unit;
- `redo` after `undo`; a new edit clears `redo`;
- `set_text` (programmatic) clears the history — `undo` after it doesn't
  revive the previous user text;
- `UNDO_CAP` — old units get evicted;
- `Ctrl+K` → `Ctrl+Z` restores the cleared text.

---

## 7. Stage D — mouse in the field (click → cursor, drag → selection) — done

Branch `feat/input-mouse`. Builds on A (drag sets the selection).
`widgets/input_box.rs` + `screens/chat`. **Implemented:** field
`InputBox.last_area` (text area of the last render), private
`place_cursor_at(mx,my)` (inverts the wrap layout + snaps to a cluster
boundary), public `mouse_press`/`mouse_drag` (start/grow a selection);
`ChatScreen::handle_mouse` routes `Down/Drag(Left)` into them (with `Ctrl+W`
capture on, not during impersonation). Click/drag don't bump the wrap-cache
revision and don't wake the spellcheck debounce.

### 7.1 Remembering the render area

The widget knows `last_width`, but mapping a click also needs the
**position**. Add:

```rust
/// Inner text area of the last render (past the border and the `❯` column).
/// Needed for mapping a mouse click (screen → text position). `None` before
/// the first render.
last_area: Option<Rect>,
```

Set in `render`/`render_single_line` (`inner`). Precedent — `last_width`.

### 7.2 Mapping a click → (row, col)

`fn place_cursor_at(&mut self, mx: u16, my: u16) -> bool` — if `(mx,my)` is
inside `last_area`, convert it to a text position and place the cursor;
return whether it hit:

- **multiline:** `vrow = (my - area.y) as usize + self.scroll`;
  `vcol = (mx - area.x) as usize`. Take `rows_cached(last_width)` (the item-7
  cache!), find row `vrow` → `(li, start, end)`, within it — the character
  index whose cumulative width ≥ `vcol` (like `col_for_visual`, with a snap
  to a cluster boundary from item 1). Click below the last row → end of
  text; right of the row's end → end of the row.
- **single_line:** `col` from `hscroll + (mx - area.x)` via `col_at_width`
  (+ snap).

This is the **inverse** of `cursor_visual`; extract into a helper
`position_at_visual(vrow, vcol)`.

### 7.3 Handling mouse events

`ChatScreen::handle_mouse` already receives `MouseEvent` (with `Ctrl+W`
capture on). Add:

- `Down(Left)` inside the field area: `place_cursor_at` + start a selection
  (`anchor = cursor`, then `clear`/`set` — effectively `anchor =
  Some(pos)`, `cursor = pos`, an empty selection).
- `Drag(Left)`: `place_cursor_at` (the cursor moves, `anchor` is kept) → the
  selection grows.
- `Up(Left)`: finalize (nothing special; an empty selection is just placing
  the cursor).
- Wheel (`ScrollUp/Down`) — as now (the feed). Click **outside** the field
  (in the feed) — currently no-op; keep it (feed selection is a separate
  large track, "deferred beyond M3").
- Double click → select word (`word_left_col`/`word_right_col`) —
  **nice-to-have, groundwork** (crossterm doesn't give a double click
  directly — needs its own time-based detector; the loop has the time, but
  it adds complexity — deferred).

During impersonation/overlays — `handle_mouse` is already a no-op (there's a
guard).

### 7.4 Stage D tests

- `place_cursor_at` (pure, via a given `last_area` + `rows_cached`): a click
  in the middle of a line places the cursor there; a click right of the end
  → end of row; below → end of text; a click on a wide/emoji glyph snaps to
  the cluster boundary (item 1).
- `handle_mouse`: `Down`+`Drag` build a selection from point to point
  (`selected_text` matches); a click without a drag → an empty selection
  (cursor moved).
- a click outside `last_area` — the cursor doesn't move (returns `false`).

---

## 8. Cross-cutting DoD (each stage)

- `cargo fmt` / `cargo clippy --all-targets -- -D warnings` / `cargo test` —
  green.
- New tests per the stage lists; existing ones stay intact (except the
  deliberate changes in §10).
- FSD intact: `widgets`/`screens` don't import `app`; the clipboard
  side-effect lives in `runtime`, driven by intent.
- Wrap cache (item 7) doesn't regress: selection/cursor don't bump
  `revision` (extend the
  `navigation_preserves_revision_but_edit_bumps_it` test to Shift
  navigation).
- No live run needed (TUI widget, covered by unit tests on `TestBackend`);
  manually verify drag/double-click on a real terminal at finalization.

---

## 9. Deliberately out of scope (groundwork)

- **Feed message selection/copy** — a separate large track ("deferred
  beyond M3"); with mouse capture on, clicks in the feed stay no-op.
- **Double/triple click** (word/line) — needs its own time-based detector;
  deferred.
- **Rectangular (block) selection**, multiple cursors — out of scope.
- **Clipboard copy in non-chat consumers** (rename/settings/self-model) —
  Fork 6 groundwork (selection and deletion work there from stage A;
  `Ctrl+C/X` later with the same pattern).
- **`Shift`/`Ctrl+Z` on a "bare" unix terminal** without the kitty keyboard
  protocol — graceful degradation (modifiers on arrows may not arrive;
  `Ctrl+Z` may go to SIGTSTP). Windows is the main target. **Done (audit
  item 11):** on unix `runtime` enables the kitty keyboard protocol
  (`DISAMBIGUATE_ESCAPE_CODES`) when the terminal supports it →
  `Shift+Enter`/`Shift`+arrows are recognized; for terminals without the
  protocol, `Alt+Enter` is a fallback newline in all multiline fields.
- **Feed autoscroll while dragging to the field's edge** — out of scope.

---

## 10. Impact on invariants and documentation

- **spec §11.5** (input/editing): extend the key table with selection
  (`Shift`+navigation, `Ctrl+A`), copy/cut (`Ctrl+C`/`Ctrl+X`), undo/redo
  (`Ctrl+Z`/`Ctrl+Y`), mouse (click/drag with capture on). **Behavior
  changes:** (a) quit `Ctrl+C` → **`Ctrl+Q`/`F10`** (`Ctrl+C` now copies);
  (b) `Ctrl+K` is no longer toggle-restore — it clears, restore is
  `Ctrl+Z`.
- **spec §11.3** (mouse): click/drag in the input box — new consumers of
  `Ctrl+W` capture.
- **spec §11.7** (keys/quit) and **README** (key tables): quit `Ctrl+Q`/
  `F10` instead of `Ctrl+C` — update everywhere quit is listed.
- **Help overlay `F1`/`?`** (`HELP_KEYS` + the list/settings/self-model help
  lines): add selection/copy/undo/mouse; show quit as `Ctrl+Q` / `F10`.
- **Status bar** (`widgets/status_bar.rs`): quit hint `Ctrl+C` → `Ctrl+Q`.
- **architecture.md §4** (dirty): selection redraws on a terminal event
  (already the case); §9 — mention the selection/undo model in the widget.
- **[docs/journal/ui-input.md](../journal/ui-input.md)**: an entry per stage (like items 1-7); note the
  quit relocation (`Ctrl+Q`/`F10`) with the `Ctrl+Q`/`F10` compatibility
  caveat (§2 Fork 2).
- **`ChatIntent` contract**: `+CopyToClipboard(String)` (runtime
  side-effect, not the orchestrator) — document next to
  `SetMouseCapture`.

---

## 11. Order and estimate

Recommended branch order: **A → B → D** (B and D build on the A selection
model; independent of each other) and **C** at any point (independent). Each
stage is a separate PR with tests, like items 1-7. The biggest risk is the
selection-highlight rendering (composing with spellcheck/command styles,
stage A.2), mapping clicks with wide glyphs/scroll (stage D.2), and moving
quit in B.0 (a coordinated change across four screens + status bar/help); all
covered by unit tests on `TestBackend`. §2 decision points are resolved
(Fork 2/4/5/6.4 — resolved; Fork 1/3/6 — reasonable defaults); no blockers
before starting.
