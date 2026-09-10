# Journal — UI: the input box and keyboard

The `InputBox` widget and everything typed into it: editing and navigation, selection/undo/mouse, clipboard and paste, spellcheck, emoji, and layout-independent hotkeys.

**Reference documents for this area:** architecture.md §10, spec.md §11.5

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)). They record what was done,
why, what was measured and what was rejected — the reasoning behind the code, not
its current shape. For the current shape read the reference documents named above;
for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (30)

- Post-M9: fast multiline clipboard paste (done)
- Post-M9: `↑/↓` navigation by visual row of a wrapped line (done)
- Post-M9: clearing the input box with undo (`Ctrl+K`) (done)
- Post-M9: manual chat rename in a single-line `InputBox` (done)
- Post-M9: emoji paste from clipboard (cluster width + clipboard recovery) (done)
- Post-M9: emoji picker popup (`Ctrl+B`) (done)
- Post-M9: InputBox refinements (clusters/spellcheck/navigation) (done)
- Post-M9: InputBox — a wrap cache + streaming cluster-boundary search (item 7) (done)
- Post-M9: InputBox — text selection (track "selection/undo/mouse", stage A of items 8–10) (done)
- Post-M9: InputBox — copy/cut + moving Quit (track "selection/undo/mouse", stage B of items 8–10) (done)
- Post-M9: InputBox — undo/redo (track "selection/undo/mouse", stage C of items 8–10) (done)
- Post-M9: InputBox — mouse in the field (track "selection/undo/mouse", stage D of items 8–10) (done)
- Post-M9: line breaks on unix terminals — the kitty protocol + Alt+Enter (audit item 11) (done)
- Post-M9: InputBox — API hygiene (audit items 12–15) (done)
- Post-M9: spellcheck popup — selection style now matches the rest of the lists (done)
- Post-M9: universal layout-independent hotkeys — stage 1, Windows (done)
- Post-M9: universal layout-independent hotkeys — stage 3, upstream crossterm (submitted)
- Post-M9: spellcheck skips URLs and email addresses (done)
- Post-M9: `Home`/`End` as a ladder of stops (done)
- Post-M9: pasting an image from the clipboard (done)
- Post-M9: `/exit` and `/quit` — a typed route out (done)
- Post-M9: command-only control — stage 1 (typed routes for the chords) (done)
- Post-M9: command-only control — stage 2 (`/profile`, `/self clear`) (done)
- Post-M9: OSC 52 — copying to the client's clipboard (done)
- Post-M9: `/project` is highlighted while it is typed (done)
- Post-M9: OSC 52 — the note that did not close the door (done)
- Post-M9: the draft flush lags the intent — a spent command resurfaces in the box (done)
- Post-M9: the restored message and the draft are separated by a blank line (done)
- Post-M9: the line-break hint names the chord the terminal can deliver (done)
- Post-M9: a stress mark is part of the word (done)

### Post-M9: fast multiline clipboard paste (done)
- **Symptom**: a large clipboard paste lagged in Windows Terminal, and a line break
  inside it was treated as `Enter` (= send the message). **Cause**: the terminal sent
  the paste **character by character** as regular keypresses, so the `run_loop`
  loop did a `terminal.draw` per character (slow), and `\n`/`\r` arrived as
  `KeyCode::Enter` → send.
- **A key nuance — bracketed paste does NOT work on Windows**: `Event::Paste` in
  crossterm 0.29 is emitted **only by the unix parser** (`src/event/sys/unix/parse.rs`);
  on Windows input is read via the Console API (`ReadConsoleInput`), and bracketed
  paste mode (`ESC[?2004h`) is useless — paste events arrive as regular `KeyEvent`s
  (interleaved with `KeyEventKind::Release`, at that). So a bare `EnableBracketedPaste`
  isn't enough — batching is needed in the loop.
- **Fix — batching events in `run_loop`** (`app/runtime.rs`): in one tick we drain
  **all** available events (`event::poll(Duration::ZERO)` in a loop), then
  `process_input_batch` coalesces consecutive "text" keypresses into a paste. This
  fixes both problems: one repaint per batch (not per character), and `Enter`
  **inside a run** → a line break, not a send.
  - `collect_press` drops "release"/repeat key events (the app ignores them anyway,
    and they would break up the character run on Windows).
  - `paste_char`: a text key without Ctrl/Alt → a character (`Enter→'\r'`, `Tab→'\t'`,
    `Char(c)→c`); everything else (arrows, shortcuts) breaks the run.
  - `chunk_batch` (pure, testable): a run of text keys of length **≥2** →
    `Chunk::Paste(String)`; a run of 1 (regular typing / a single `Enter`) — a regular
    event, so `Enter`-sends aren't broken. A human physically can't type 2+
    keys within one zero-timeout drain — so ≥2 reliably means a paste.
  - On unix `EnableBracketedPaste` is kept (under `#[cfg(unix)]`) — there a clean
    `Event::Paste` arrives, which the same `process_input_batch` handles as a `Chunk`.
- **Collecting the paste "tail" across console-chunk boundaries** (`PASTE_BURST`/`PASTE_GAP`):
  a large paste (thousands of characters) arrives in the Windows console buffer in
  **several chunks**, and the loop drains them over different iterations. At a chunk
  boundary the character run broke → if a chunk happened to end exactly on a lone
  `Enter`, it slipped through as a send (a bug: ~6900 characters went in as a paste, then
  one `Enter` fired as a send). Fix: if a single zero-timeout drain gathered a burst
  (`batch.len() ≥ PASTE_BURST=2`), we chase the paste's tail via
  `while event::poll(PASTE_GAP=20ms)` — while events keep arriving less than 20ms
  apart, we treat them as one paste (real human typing has pauses >>20ms). This way the
  whole paste is collected into ONE batch with no internal boundaries, and no more
  lone `Enter`s appear on the seams.
- **`InputBox::insert_str(&str)`** (`widgets/input_box.rs`): insertion at the cursor
  position in one pass (the line tail is cut off, the text is split on `\n` into
  logical lines, the tail is glued to the last one, the cursor lands at the end of what
  was inserted). `normalize_paste`: `\r\n`/`\r` → `\n` (so CRLF from the clipboard — whether
  it's `Enter`+`\n` or `\r\n` — collapses into one line break), `\t` → 4 spaces. No
  per-character loop → a large paste doesn't lag.
- **Routing**: a paste (coalesced or unix `Paste`) → on the settings screen goes to the
  active field editor (`SettingsScreen::handle_paste` → `editor.input.insert_str`,
  useful for the model path), otherwise — `ChatScreen::handle_paste` → the chat's input
  box. In the chat it respects modality (help/suggestions popup/overlays with single-line
  fields → no-op) and **never sends** a message, even with line breaks inside;
  `mark_input_changed` kicks off the spellcheck debounce. Added to the help overlay
  (`F1`/`?`, `Ctrl+V`).

### Post-M9: `↑/↓` navigation by visual row of a wrapped line (done)
- **The `↑/↓` arrows in the input box now move by visual row**, not by logical
  lines: if a long line wraps across several rows, `↑/↓` move the
  cursor between its parts (previously they'd jump across a whole logical line to the
  neighboring one). The column is preserved where possible. **Root cause of the bug**:
  `move_up`/`move_down` (`widgets/input_box.rs`) worked in terms of `self.row` (a
  logical line), while wrapping into visual rows (`wrap::wrap_ranges`) is only computed
  at render time against the width `view_w` — `on_key` has no width available.
- **Fix**: the widget caches the width of the last render (`InputBox.last_width`,
  set in `render`); `move_up`/`move_down` build visual rows
  (`visual_rows`) at that width, take the cursor's visual position (`cursor_visual`)
  and move it to the neighboring row via `col_for_visual` (the nearest column by
  width in columns, stepping one character back on a soft wrap — otherwise a
  position `== end` would land at the start of the next row, `is_soft`). Before the
  first render (`last_width == 0`) — a fallback to the logical transition (`move_up_logical`/
  `move_down_logical`). `↑` on the top visual row / `↓` on the bottom one — a no-op.
  Rendering in the loop happens before key handling, so the width is always current.
- **`Home`/`End` — by visual row**: `Home` → the start of the current visual row,
  `End` → its end (`move_home`/`move_end`). On a soft wrap, `End` lands on the
  last position of the row (via the same `col_for_visual`/`is_soft`), not sliding into
  the start of the next one. Before the first render — the whole logical line (fallback).
- **Goal column**: a run of `↑/↓` keeps the original column (`InputBox.goal_col`,
  in columns) when passing through short rows — like in large editors.
  Remembered on the first vertical move, reset by **any** other cursor
  move/edit (`move_left`/`move_right`/`move_home`/`move_end`, `insert_char`/
  `insert_newline`/`insert_str`/`backspace`/`delete`, `replace_range`/`set_text`/
  `clear` — reset inside those methods themselves, to also cover paths that bypass
  `on_key`: pasting, suggestions, draft loading). See spec §11.5.

### Post-M9: clearing the input box with undo (`Ctrl+K`) (done)
- **`Ctrl+K` deletes all text in the input box**; pressing it again **restores what
  was deleted**, provided nothing was typed after the deletion (a toggle). The logic
  lives in the `InputBox` widget itself (`widgets/input_box.rs::clear_or_restore`): a
  `cleared: Option<String>` buffer holds the deleted text; a non-empty field →
  remember and clear it; an empty field with a live buffer → restore it (cursor at
  the end). **Any text input** (`insert_char`/`insert_newline`/`insert_str`/`replace_range`/
  `set_text`, and also an explicit `clear()` on send) invalidates the buffer to
  `None`, so it can only be restored right after the deletion; cursor movement and
  no-op backspace/delete on an empty field don't touch the buffer (restore survives
  them).
- **Wiring**: `screens/chat.rs::handle_key` — a `'k'` branch in the Ctrl match
  (layout-independent via `shared/keys`, physical K = `Ctrl+л`) calls
  `input.clear_or_restore()` + `mark_input_changed()` (draft persistence +
  a spellcheck recheck). No orchestrator command needed (purely local
  input state). Added to the help overlay (`F1`/`?`) and the keybinding table in spec
  §11.5.

### Post-M9: manual chat rename in a single-line `InputBox` (done)
- **The rename field (`F2` in the chat list) switched from a hand-rolled
  character-by-character buffer to a single-line `InputBox`** (`set_single_line`, like
  editable settings fields). This gave it, "for free": **spellcheck**
  (underlining errors), **word-wise navigation/deletion** (`Ctrl+←/→`,
  `Ctrl+Backspace/Delete`), `Ctrl+Home/End`, **clear/restore `Ctrl+K`**, clipboard
  paste, and horizontal scrolling of a long name. Previously the `Vec<char>` buffer
  only supported character-by-character movement/editing.
- **`widgets/chat_list.rs`**: `Mode::Rename` now holds a `Box<InputBox>` (+ a
  `spell_dirty` flag) instead of `Vec<char>`/`cursor`; `on_key_rename` handles only
  `Esc`/`Enter`/`Ctrl+K` (layout-independent, `shared/keys`), everything else is handled by
  `InputBox` itself. The manual `render_rename` function was removed — the field renders via
  `input.render(...)` (its own rounded border + `❯` + a real cursor). New
  methods `handle_paste` and `recheck_rename_spelling(&SpellChecker) -> bool`. `render`
  became `&mut self` (`InputBox::render` requires `&mut`).
- **The spellchecker isn't duplicated**: the owner is the chat screen (`ChatScreen::spellchecker()
  -> Option<&SpellChecker>`); the loop in `app/runtime.rs`, when the list is open, lends
  it to the list screen (`ChatListScreen::recheck_spelling`) — the `screen`/`active`
  fields are separate, so an immutable borrow of the checker coexists with a mutable borrow of the list.
  No debounce (a name is short — recomputing on the `spell_dirty` flag is cheap). Clipboard
  paste for the list is routed into the rename field
  (`process_input_batch`). **A suggestion popup (`Ctrl+G`) for rename was NOT
  added** — only underlining (by agreement).

### Post-M9: emoji paste from clipboard (cluster width + clipboard recovery) (done)
- **Two independent bugs with emoji in the input box**, both only inside the application (in a bare
  terminal emoji paste fine and the cursor is correct — the terminal manages the caret itself):
  - **(B) Cursor "drifts" on BMP-emoji clusters** (`❤️` = `❤` U+2764 +
    a selector U+FE0F; `👍🏽` = an emoji + a skin-tone modifier). Cause: width was computed
    character-by-character via `unicode-width`, which gives `❤`=1, `U+FE0F`=0 (total 1), while
    the terminal draws the cluster at width **2** → the cursor landed in the middle of the emoji, text
    after it — one column to the left. `👍🏽` was measured as 4 instead of 2.
  - **(A) Supplementary-plane emoji (`😊` U+1F60A, `🥹`) don't paste at all.**
    Cause — a **crossterm 0.29 limitation on Windows**: clipboard paste arrives as
    ordinary console key events, and such emoji are encoded as a UTF-16 surrogate pair;
    console key-down/key-up records break the pair assembly in crossterm
    (`event/sys/windows/parse.rs`: the surrogate is handled without accounting for `key_down`), and
    the character is lost **before** our layer. BMP characters (letters, `❤`, `U+FE0F`) pass through.
- **Fix (B1) — width accounting for the emoji cluster** (`shared/wrap.rs`): a new
  `width_at(chars, i)` — a causal (only looking at the previous character) width for character
  `i`: a `U+FE0F` selector brings the preceding "text" character up to 2 columns
  (`2 − prev_width`); a skin-tone modifier (`U+1F3FB..U+1F3FF`) gives 0 (the base
  is already 2). `display_width` now sums `width_at`, and incremental consumers
  (`wrap_ranges`; `col_at_width`/`col_for_visual`/single-line rendering in `input_box`;
  truncation with "…" in `chat_list`/`markdown`) were switched from `char_width` to `width_at` —
  otherwise wrapping/cursor would diverge from `display_width` at a cluster boundary. `char_width`
  is kept as the context-independent base. Improves both the feed (`message_feed`) and tables.
- **Fix (B2) — cursor/deletion by grapheme cluster** (`shared/wrap.rs`
  `prev_boundary`/`next_boundary` via `unicode-segmentation`, UAX #29; `input_box`
  `move_left`/`move_right`/`backspace`/`delete`): the cursor and deletion moved by Unicode
  scalars, and `❤️`/`👍🏽` are **two** scalars (base + variant selector/modifier). Symptoms:
  `←`/`Backspace` through `👍🏽` required two presses (after the first, `👍` remained);
  through `❤️` the cursor first jumped to the middle; `Delete` at the start of a line left
  an "orphaned" variant selector/modifier → phantom glyphs and broken spellcheck
  underlines. Now movement/deletion take a whole cluster: `prev_boundary`/
  `next_boundary` give the boundary left/right of a position. A lone scalar emoji (`😊`)
  — a normal ±1 boundary (one press, as before).
- **Fix (A) — reconciling paste against the clipboard** (`app/runtime.rs`, Windows only):
  `Chunk::Paste` (reconstructed from key events) is checked against the clipboard via a pure
  `paste_projection_matches`: from the clipboard content, code points > U+FFFF are dropped (exactly the ones
  the console loses), both sides are normalized on `\r\n`/`\r`/`\t` — if they match, it's the
  same paste and the **full** clipboard text (with emoji) is used; otherwise (the clipboard is stale/it's not the
  same paste/unavailable) — the reconstruction (without emoji, but with no risk of pasting someone else's data).
  Safe in any outcome: if emoji are already present in the reconstruction — the projection won't
  match and the reconstruction stays; an empty reconstruction never matches. `read_clipboard_
  text` lazily creates an `arboard::Clipboard` (the same slot as `F5` copy).
  `reconcile_paste` on non-Windows is the identity function (there a correct `Event::Paste` arrives).
- **Residual limitation**: a paste consisting **entirely** of supplementary-plane emoji
  (with no BMP character at all) leaves no key events at all → neither `Chunk::Paste`
  nor another reconciliation trigger fires → it won't paste (nothing to latch onto). Emoji **inside text**
  (the common case, "hi 😊") are recovered. A full fix would require
  patching the console-reading layer (patching/forking crossterm or a custom `InputRecord` reader).

### Post-M9: emoji picker popup (`Ctrl+B`) (done)
- **`Ctrl+B` opens a popup grid of popular emoji** in the chat window; the chosen one
  is inserted into the input box **at the cursor**. The widget `widgets/emoji_picker.rs`
  (`EmojiPickerState` + `EmojiPickerAction`) is FSD-self-contained (`screens →
  widgets`): it only holds the selection index, and responds to key presses with an action
  (`None`/`Cancel`/`Pick`). The `EMOJIS` list is fixed (44 emoji, 4 rows of
  `COLS=11`); the grid width was chosen so that the popup's bottom hint fits
  in full. Navigation `←↑↓→` over the grid (clamped), `Enter` — insert, `Esc` —
  close. The selected cell uses a dark `palette.keycap_bg` background (like the selected
  chat-list row; a reversed style gave a light background that washed out the colored glyph).
- **Insertion** via `InputBox::insert_str` — safe for multi-scalar emoji
  (`❤️`, `👍🏽`; cursor/deletion by grapheme clusters is already correct, see above).
- **Remembers the last choice**: `ChatScreen.emoji_last` holds the index; opening goes
  through `EmojiPickerState::with_selected(idx)` (clamped), closing (both `Enter` and
  `Esc`) saves the current selection back. While the popup is open the input box loses
  focus, `handle_paste`/`handle_mouse` are no-ops (like the spellcheck popup); it's drawn
  on top with a dimmed background (`dim_background`). `Ctrl+B` is layout-independent
  (`shared/keys`, physical B = `Ctrl+и`), added to the help overlay (`F1`/`?`), the
  README/spec §11.5 keybinding tables. Pure widget tests (navigation/clamping/`with_selected`/
  render-without-panic) and screen plumbing (insertion at the cursor, remembering the choice,
  cancellation).

### Post-M9: InputBox refinements (clusters/spellcheck/navigation) (done)
- Six targeted fixes to the `widgets/input_box.rs` widget (+ the shared `shared/wrap.rs`) from
  an audit — correctness with no user-behavior change; branch
  `feat/input-box-refinements`. See spec §11.5–11.6, ADR 0001.
- **(1) Cursor snaps to a cluster boundary at `↑/↓`/`End`** (`col_for_visual`): the
  target visual column could land **between an emoji base and its variation
  selector** (`❤️` = ❤ + U+FE0F, widths 1+1) — the cursor sat in the middle of the
  cluster, and the next `insert`/`backspace` would split it (an orphaned selector — the
  same class of bug already fixed for `←/→`). Now `col_for_visual` snaps down to a
  cluster boundary (a new `wrap::snap_boundary` — the largest boundary `≤ col`); for ASCII the
  snap is a no-op (boundaries are everywhere).
- **(2) A hard break of a long word doesn't cut a cluster** (`wrap::wrap_ranges`): when
  wrapping a word longer than the width, the break point could cut `❤️` or a **flag**
  (a pair of regional indicators) across rows (the second scalar "drifted" to the start
  of the next row). A new `hard_break` shifts the break to a cluster boundary while
  guaranteeing progress (≥1 cluster, otherwise `wrap_ranges` would loop). **Fast path**:
  the snap only kicks in when the character at the break is a VS16 or a regional
  indicator (otherwise `return i` with no boundary scanning) — wrapping long ASCII/Cyrillic
  "words" stays O(n), not O(n²). A shared module → improves `message_feed` for free too.
- **(3) A hard invariant for single-line mode** (`set_single_line`): the contract
  "call before `set_text`" was only documented; enabling it on already-multiline
  content had `render_single_line` take `lines[0]`, and `col` could point past its
  length → **a panic on the slice**. Now the setter, at `on=true`, collapses the lines
  into one via a space and clamps the cursor (all current callers are disciplined —
  this is a defensive path).
- **(4) Aligning `hscroll` to a character boundary** (`render_single_line`): with a wide
  glyph (CJK/emoji) on the left, the slice could start "in the middle" of a character, and
  `hscroll` (in columns) wouldn't match the real width of the hidden prefix — **the cursor
  was drawn a column to the right** of its actual position. After `col_at_width` we round
  `hscroll` to the `display_width` of that prefix (guaranteeing `≤ cursor_vw` → the cursor
  is visible).
- **(5) Syncing spellcheck underlines with edits** (`edit_misspelled`): error ranges
  (`misspelled`, row/character coordinates) used to be updated only externally with a
  ~300ms debounce, so structural edits (a line break, joining lines, deleting a word)
  drew stale ranges on the new text (half of a neighboring word underlined, ranges from
  "someone else's" line after a join). Now the mutators sync
  `misspelled` in place: `edit_misspelled(row, at, removed, inserted)` — ranges
  to the left aren't touched, ranges to the right shift by the delta, ranges that
  intersect the edit are reset (underlines stay put between recheck passes). Joining/
  wrapping lines resets the affected rows and syncs the `Vec`'s length; `set_text`/`insert_str`
  (paste) reset everything — the recheck rebuilds it. This also closed a latent bug:
  `set_text` previously **didn't** reset `misspelled` (stale underlines from the prior chat).
- **(6) `on_key` distinguishes an edit from a move** (`KeyOutcome { Edited, Moved, Ignored }`
  instead of `bool`): the chat screen marked the input "dirty" (spellcheck debounce +
  a disk `SetDraft`) on **any** handled key, including arrows/Home/End — pure
  navigation needlessly woke the recheck and sent a draft command. Now `mark_input_changed`
  is only called on `.edited()` (chat/input, chat_list rename); redrawing on
  cursor movement didn't suffer (the `runtime` loop marks `dirty` on any terminal
  event regardless of intent). A companion method `handled()` — `#[allow(dead_code)]`
  (needed by tests, paired with `edited()`). Three call sites that ignored the result
  (settings apply/search, self_model) are untouched.
- **Tests**: wrap (`snap_boundary`; a hard break keeps ❤️/a flag together); input_box
  (the `col_for_visual`/`↑` snap past the middle of a cluster; `set_single_line` on
  multiline content — joining without a panic; `hscroll` at a character boundary with CJK;
  `edit_misspelled` — right/left shift, reset on an edit inside the word, sync on
  wrapping/joining, cleared on `set_text`/paste; `on_key` returns Edited/Moved/Ignored + the helpers).
  **903 unit tests green** (+15), 33 `#[ignore]`, clippy `-D warnings`/fmt clean.
  A pure widget refinement (no engine).
- **Groundwork** (from the same audit, not included): text selection (Shift+arrows/Ctrl+A +
  copy), undo/redo (generalizing the `Ctrl+K` buffer), mouse click in the field (cursor
  position), `Shift+Enter` on a "bare" unix terminal (kitty keyboard protocol),
  a view-model for the 6-arg `render`, deduplicating `Ctrl+K` across five screens. The widths of ZWJ
  families and flags remain a deliberate boundary (cursor/navigation by clusters are
  correct — only the width diverges; terminals render them differently).

### Post-M9: InputBox — a wrap cache + streaming cluster-boundary search (item 7) (done)
- Eliminating repeated O(n) recomputes and extra allocations per frame/keystroke (item 7
  of the same audit). Behavior unchanged — a micro-optimization; branch
  `feat/input-box-refinements`.
- **(7a) A visual-row cache** (`widgets/input_box.rs`): `visual_rows` (wrapping,
  O(n) over characters) was built 2–3 times per frame — the field height via `content_rows`
  (the screen), `render` itself, `↑/↓`/`Home`/`End` navigation. Now a cache `rows_cache`
  `(width, revision, rows)`: a `revision` counter is bumped by the `touch()` helper in every
  mutator of `lines` (navigation doesn't call it), `rows_cached(width)` only recomputes the
  wrap on a width/content change. Wrapping is now computed **once** per frame (was 2), and holding
  an arrow key computes it **zero** times (the cache survives navigation). The three
  consumers that go on to mutate `self` (render, `move_*`) take a `.to_vec()`
  copy (a cheap memcpy of the result vs. an O(n) wrap); `content_rows` became
  `&mut self` and returns `.len()` from the cache (the caller in `chat/render.rs` switched from
  `match &self.impersonation` to `if let` — the `else` branch needs `&mut self.input`,
  the fields don't overlap). Type aliases `VisualRow`/`RowCache` (otherwise clippy's
  `type_complexity` fires on the nested tuple).
- **(7b) Streaming cluster-boundary search** (`shared/wrap.rs`): `prev_boundary`/
  `next_boundary`/`snap_boundary` used to build **the entire** boundary `Vec` via
  `cluster_boundaries` just to take one neighbor (called on every `←/→`/Backspace).
  Rewritten as a streaming pass over graphemes with early exit (the first boundary `≥/> col` or
  advancing until `≤ col`) — one fewer heap allocation and no tail scan; the semantics unchanged
  (existing tests `snap_boundary_lands_on_cluster_start`/
  `grapheme_boundaries_group_emoji_clusters` stay green). `cluster_boundaries` removed.
- **(7c) A cheap guard for `input_is_command`** (`screens/chat/input.rs`):
  checked every frame and allocated the entire `text()` + parsed it. A command always
  starts with `/` (the first non-whitespace character), so first — a new
  `InputBox::first_non_whitespace()` (a streaming pass over characters, no allocation), and only when
  it returns `Some('/')` is `text()` built and `rag_command::parse` called. For regular input
  (letters/Cyrillic) the full text is no longer built.
- **Tests**: cache invalidation across **all** mutators (`row_cache_invalidates_on_every_
  mutator` — compares `rows_cached` against a fresh `visual_rows`, catches a forgotten
  `touch()`); the invariant "navigation doesn't bump the revision, an edit does";
  `first_non_whitespace`. **906 unit tests green** (+3), 33 `#[ignore]`, clippy
  `-D warnings`/fmt clean. A pure widget optimization (no engine).

### Post-M9: InputBox — text selection (track "selection/undo/mouse", stage A of items 8–10) (done)
- The first of four stages in the "selection/undo/mouse" track
  ([docs/history/input-selection-undo-mouse.md](../../docs/history/input-selection-undo-mouse.md), stage A);
  branch `feat/input-selection`. **Widget only** `widgets/input_box.rs` (+ tests) —
  selection is now available to **all five** `InputBox` consumers (chat, chat
  rename, settings fields, the self-model editor, search) via the shared `on_key`. Copying/
  cutting to the clipboard is stage B (a consumer side effect); here the widget only
  tracks selection and exposes `selected_text()`.
- **The selection model** — an `anchor: Option<(usize,usize)>` field (the anchor; the cursor —
  the existing `row/col`; the selection `[anchor, cursor]`, normalized in
  `selection_span` lexicographically). Methods: `has_selection`/`selected_text` (pub,
  for stage B — `#[allow(dead_code)]` for now), `set_anchor_if_none`/`clear_selection`/
  `select_all`, `delete_selection() -> bool` (joins multi-line content + syncs
  `misspelled`, item 5), `row_selection` (a row-local range for highlighting).
- **Keys** (`on_key`): `Shift`+navigation grows the selection (sets the anchor before
  moving), plain navigation clears it; `Ctrl+Shift`+`←/→` — by word; `Ctrl+A`
  (layout-independent via `physical_char`) — select all. Navigation was factored into a
  table `navigation(code, ctrl) -> Option<fn(&mut InputBox)>`, so the anchor logic
  applies uniformly to every direction with no duplicated branches.
- **Replacing/deleting a selection — inside the mutators themselves** (not `on_key`): `insert_char`/
  `insert_newline`/`insert_str` begin with `delete_selection()`, `backspace`/`delete`/
  `delete_word_*` begin with `if delete_selection() { return }`. So **paste**,
  `Shift+Enter`, and the emoji popup (`Ctrl+B`) also replace a selection, not just
  typing via `on_key`. `replace_range` (spellcheck suggestion) doesn't touch the
  selection (it targets a specific range).
- **Clearing the selection on an edit — centralized in `touch()`**: any edit to `lines`
  bumps the revision (the item-7 cache) **and** clears `anchor` (after an edit the anchor
  would point to stale coordinates). Navigation doesn't call `touch()` — it manages
  `anchor` itself. `delete_selection` clears `anchor` before `touch` (a double reset is harmless).
- **Invariants preserved**: extending the selection is `KeyOutcome::Moved` (doesn't wake
  the spellcheck debounce/`SetDraft`) and **doesn't bump the revision** (the item-7 wrap
  cache survives selection — the geometry doesn't change); replacing a selection is `Edited`.
  Highlighting — `styled_line` was rewritten (a per-character composition: spellcheck
  underline + **the selection background** `palette.keycap_bg` + command coloring); the
  background works in compat mode too (a color, not a glyph) — theme.rs untouched (`keycap_bg`
  is reused).
- **Tests**: `Shift`+arrow grows/plain clears; `Ctrl+A`/`Ctrl+Shift+→`;
  replacing the selection by typing; `Backspace`/`Delete` delete the whole selection; a multi-line
  delete joins + syncs `misspelled`; `Shift` navigation is `Moved`+revision
  unchanged, an edit is `Edited`+revision grows; the render carries the `keycap_bg`
  background (TestBackend); paste replaces the selection. **915 unit tests green** (+9), 33 `#[ignore]`, clippy
  `-D warnings`/fmt clean. No live run needed (a TUI widget, covered by
  TestBackend); spec §11.5/the help overlay/the status bar will be updated in stage B (copy +
  moving Quit from `Ctrl+C`→`Ctrl+Q`/`F10` — that's when the feature becomes user-complete).

### Post-M9: InputBox — copy/cut + moving Quit (track "selection/undo/mouse", stage B of items 8–10) (done)
- The second stage of the "selection/undo/mouse" track
  ([docs/history/input-selection-undo-mouse.md](../../docs/history/input-selection-undo-mouse.md), stage B);
  branch `feat/input-clipboard`. Builds on stage A (selection). Decision points resolved
  by the user: `Ctrl+C` copies / `Ctrl+X` cuts; **Quit moves from
  `Ctrl+C` to `Ctrl+Q` + `F10`** (`Ctrl+C` freed up); `Ctrl+K` is replaced by shared
  undo — groundwork for stage C.
- **Moving Quit — across all four screens** (a coordinated edit, step B.0):
  `Ctrl+C → *Intent::Quit` lived in the chat screen (the main match plus the
  impersonation and confirmation-popup branches), the chat list (`widgets/chat_list`), settings
  (`settings/apply`), and the self-model screen (`self_model`). Everywhere it was replaced with `Ctrl+Q` (physical
  `q` via `physical_char`, layout-independent) **plus** `F10` (a second option — in case a
  terminal/DE intercepts `Ctrl+Q`). Hints updated: the status bar's `Ctrl+Q quit`, the help
  overlay and the list/settings help lines — `Ctrl+Q`/`F10`.
- **`Ctrl+Q` compatibility** (verified by reasoning): historically XON (resume) for
  flow-control XON/XOFF, but the application runs in **raw mode** (crossterm's `enable_raw_mode`
  clears `IXON`), so the terminal driver doesn't intercept it — it reaches the
  application (Linux/Windows). `F10` guards against rare interceptions (a multiplexer/DE);
  `F10` opens the emulator's menu in some Linux DEs, but that's dismissible — the keys
  cover for each other.
- **Copy/cut** (`screens/chat/input.rs`): `Ctrl+C` with a selection →
  `ChatIntent::CopyToClipboard(selected)` + clearing the selection (`clear_selection`); with no
  selection — a no-op (not quit). `Ctrl+X` → the same intent + `delete_selection()` +
  `mark_input_changed()`. The widget exposed `has_selection`/`selected_text`/`clear_selection`
  (made `pub`).
- **Side-effect wiring** (`app/runtime`): a new intent
  `ChatIntent::CopyToClipboard(String)` (modeled on `SetMouseCapture` — executed by
  `runtime`, not the orchestrator: the text is already at the UI). Intercepted **in
  `process_input_batch`** (which already has the `arboard` slot; `dispatch` lacks it and `screen`
  is immutable there) → `write_clipboard`; success is silent, a failure (headless Linux with no X11) —
  a `push_error` note in the feed. In `dispatch` — a defensive arm (never reached).
- **Coverage — chat-first** (Fork 6): selection + delete/replace work in all five
  consumers (stage A), but clipboard `Ctrl+C`/`Ctrl+X` are wired only in the chat's
  input box for now. In the list/settings/self-model screens `Ctrl+C` is freed up (a no-op) —
  wiring copy there is groundwork.
- **Docs**: spec §11.5 (explaining selection/copy/moving Quit) and §11.7
  (the key table: `Shift`+navigation/`Ctrl+A`/`Ctrl+C`/`Ctrl+X`; Quit `Ctrl+Q`/`F10`),
  README (the key table) updated; `Ctrl+K` unchanged for now (undo — stage C).
- **Tests**: chat (`Ctrl+C` copies the selection → `CopyToClipboard`, clears the selection,
  text unchanged; `Ctrl+X` cuts → the text shrinks; `Ctrl+C` with no selection — a no-op;
  `Ctrl+Q`/`F10` quit; impersonation/the confirmation popup — quit via `Ctrl+Q`/`F10`);
  chat list/settings/self-model (`Ctrl+Q`/`F10` → Quit, `Ctrl+C` is no longer quit);
  layout (`Ctrl+й` physical Q → quit). The former `ctrl_c_quits*` tests were renamed/
  rewritten. **916 unit tests green** (+1 net; 6 rewritten), 33 `#[ignore]`,
  clippy `-D warnings`/fmt clean. A live run is the track's final manual step.

### Post-M9: InputBox — undo/redo (track "selection/undo/mouse", stage C of items 8–10) (done)
- The third stage of the "selection/undo/mouse" track
  ([docs/history/input-selection-undo-mouse.md](../../docs/history/input-selection-undo-mouse.md), stage C);
  branch `feat/input-undo`. **Widget only** `widgets/input_box.rs` + consumers
  (replacing `Ctrl+K`). A shared undo model across **all five** `InputBox` consumers.
- **Model — a stack of snapshots** `(lines, cursor)` with coalescing (not operational
  records — simpler and provably correct; the field is short). Fields `undo`/`redo: Vec<Snapshot>`,
  `last_edit_kind: Option<EditKind>` (`Insert`/`Delete`/`Structural`), a ceiling
  `UNDO_CAP=200`. `record_undo(kind)` at the start of every mutator writes a snapshot **before**
  the edit; coalescing: consecutive edits of the same class (except `Structural`) merge
  into a single unit; **a typing run breaks on whitespace** (word-granular — `insert_char` on
  whitespace clears `last_edit_kind`); navigation/selection (the `on_key` nav branch,
  `select_all`) break coalescing → the edit after them is a new unit; any edit
  clears `redo`.
- **`undo()`/`redo()`** (`Ctrl+Z`/`Ctrl+Y` in `on_key`, layout-independent):
  restore a snapshot (a shared `restore` — lines+cursor, clearing selection/highlighting,
  invalidating the item-7 cache), pushing the current state onto the opposite stack. They only
  return `Edited` when something actually changed (otherwise `Moved` — a no-op on an empty stack).
- **Consumer boundaries**: `set_text`/`clear` (a programmatic replacement — loading
  someone else's draft, sending, `restore_input`) **clear the history** (`Ctrl+Z` shouldn't
  resurrect someone else's context). `clear_undoable` (`Ctrl+K`) — writes a snapshot and
  clears the content, **without** touching the history → `Ctrl+Z` brings it back.
- **`Ctrl+K` → a special case of undo** (a user decision): the previous toggle semantics
  (`cleared`/`clear_or_restore`) were **removed**; `Ctrl+K` clears, `Ctrl+Z` restores.
  The consumers (chat/rename/settings/search/self-model) were switched from
  `clear_or_restore()` to `clear_undoable()`; `Ctrl+Z`/`Ctrl+Y` reach `on_key`
  through their `_` branch.
- **Replacing/deleting a selection — a single undo unit**: `delete_selection` was split into a
  public one (`Ctrl+X` cut — writes a snapshot + deletes) and a private `remove_selection`
  (no snapshot) — mutators (`insert_char`/`backspace`/…) write **their own** snapshot and
  call `remove_selection`, so "type/delete over a selection" undoes as a whole. A no-op
  at a boundary (`Backspace` at the start / `Delete` at the end with no selection) — no unit recorded.
- **Invariants preserved**: undo/redo go through `touch()` (invalidating the item-7 cache +
  clearing the selection); with tool delegation (`delete_word_left`→`backspace`), the double
  `record_undo(Delete)` **coalesces** — no extra snapshot.
- **Docs**: spec §11.5/§11.7, README, the help overlay (`Ctrl+Z`/`Ctrl+Y`, `Ctrl+K`).
- **Tests**: a typing run = a single unit + redo; a word-granular break on whitespace; navigation
  breaks coalescing; `insert_str` — its own unit; an edit clears redo; `set_text`
  clears the history; `Ctrl+K`→`Ctrl+Z`; a no-op undo/redo → `Moved`; `UNDO_CAP` evicts;
  undoing a selection replacement. The previous 6 `cleared`/`clear_or_restore` tests were replaced;
  consumer tests (chat_list rename, settings editor) — via `Ctrl+K`→`Ctrl+Z`.
  **920 unit tests green** (+4 net), 33 `#[ignore]`, clippy `-D warnings`/fmt
  clean. A live run — the track's final manual step (stage D — mouse — remains).

### Post-M9: InputBox — mouse in the field (track "selection/undo/mouse", stage D of items 8–10) (done)
- The final stage of the "selection/undo/mouse" track
  ([docs/history/input-selection-undo-mouse.md](../../docs/history/input-selection-undo-mouse.md), stage D);
  branch `feat/input-mouse` (off `feat/input-undo`). Builds on stage A (dragging grows the
  selection). Left-clicking in the **chat's** input box places the cursor, dragging —
  selects. **Only with mouse capture (`Ctrl+W`)** (otherwise crossterm gets no mouse
  events — native terminal selection takes over); there's no separate toggle, it's the
  same capture used for the wheel.
- **Widget** `widgets/input_box.rs`: a new field `last_area: Option<Rect>` (the inner
  text area of the last render, after the border and the `❯` column) — set in `render`
  before the single/multiline branch (the same `inner` feeds `render_single_line`). A private
  `place_cursor_at(mx,my) -> bool` — **the inverse of wrap layout** (screen
  coordinates → a text position): multiline `vrow = (my−area.y)+scroll`, `vcol =
  mx−area.x`, takes `rows_cached(last_width)` (the item-7 cache!), inside a row the
  logical column comes from `col_for_visual` (which already handles "past the end → the
  end of the row" + a soft-wrap rollback + **snapping to a cluster boundary**, item 1);
  a click below the last row → the end of the text; single-line —
  `col_at_width(hscroll+vcol)`+snap. Public
  `mouse_press` (places the cursor + `anchor = cursor` → starts an empty selection) and
  `mouse_drag` (moves the cursor, `anchor` stays → the selection grows).
- **Invariants preserved**: a click/drag only moves the cursor/selection → `place_cursor_at`
  **doesn't** call `touch()` (the item-7 wrap-cache revision doesn't grow — the geometry doesn't change)
  and **doesn't** wake `mark_input_changed` (no `SetDraft`/spellcheck debounce); it resets
  `last_edit_kind` (a click breaks undo coalescing, like navigation). Redraw is provided by the
  `runtime` loop (`dirty` on any terminal event).
- **Screen** `screens/chat/input.rs::handle_mouse`: alongside the previous `ScrollUp/ScrollDown`,
  added `Down(MouseButton::Left)` → `input.mouse_press` and `Drag(Left)` →
  `input.mouse_drag`, **gated on `impersonation.is_none()`** (during impersonation the
  field is hidden, `last_area` is stale). The previous overlay/help/popup guard is preserved;
  a click outside the input area (into the feed) is a no-op (feed selection — "deferred beyond M3").
  `MouseButton` was added to the `chat/mod.rs` import.
- **Coverage — chat-first** (like stage B): mouse-in-field was wired only for the main
  chat input box (`runtime` only forwards `Event::Mouse` when `active.is_chat()`); other
  `InputBox` consumers (rename/settings/self-model) don't get mouse forwarding — groundwork.
- **Groundwork** (not in stage D): double/triple click (word/line — crossterm doesn't give it
  directly, would need its own time-based detector), autoscroll on dragging to an edge, mouse-in-field
  in non-chat consumers, feed selection.
- **Docs**: spec §11.3 (click/drag — new consumers of `Ctrl+W` capture) and §11.5, README,
  the help overlay (`F1`/`?`), the design plan (all A–D done).
- **Tests**: the widget (`place_cursor_at` — mapping a click to a position, past the end → the end of the
  row, below → the end of the text, snapping to the ❤️ cluster, outside the area/before a render → a no-op;
  `mouse_press`+`mouse_drag` build a selection, a click with no drag is empty, a drag through
  soft wrap follows logical coordinates); the screen (`handle_mouse`: `Down`+`Drag` in
  the field build a selection via `last_area_for_test`; a click into the feed doesn't move the cursor).
  **931 unit tests green** (+11), 33 `#[ignore]`, clippy `-D warnings`/fmt clean.
  A live run (click/drag on a real terminal) — the final manual step for the whole
  track (A–D). **The "selection/undo/mouse" track (stages A–D) is complete.**

### Post-M9: line breaks on unix terminals — the kitty protocol + Alt+Enter (audit item 11) (done)
- Closed **audit item 11** of InputBox (groundwork from docs/history/input-selection-undo-mouse.md §9):
  on a "bare" unix terminal, the legacy encoding sends the **same** CR for both
  `Shift+Enter` and `Enter`, so line breaks in the input box weren't available there at all
  (Windows unaffected — the Console API reports modifiers). A runtime-layer issue, not the widget.
- **Enabling the kitty keyboard protocol** (`app/runtime/mod.rs`, `#[cfg(unix)]` next to
  bracketed paste): if the terminal supports it (`crossterm::terminal::
  supports_keyboard_enhancement()`), we push `PushKeyboardEnhancementFlags(
  DISAMBIGUATE_ESCAPE_CODES)` — the terminal starts reporting modifiers for special keys,
  and `Shift+Enter` becomes **distinguishable** from `Enter` (and `Shift`+arrows — from bare
  arrows, which as a bonus enables keyboard-driven selection from stage A). We pop it
  (`PopKeyboardEnhancementFlags`) on exit and in the panic hook (harmless on an empty stack).
  **The `disambiguate` level was chosen deliberately** — it does NOT affect regular typing or
  a lone `Shift`+character (text arrives as-is): `?` (Shift+/) still comes in as a plain
  `Char('?')`+`NONE`, emoji input, and layout-independent parsing of Ctrl shortcuts
  (`shared::keys::physical_char`, Cyrillic) don't regress (unlike
  `REPORT_ALL_KEYS`/`REPORT_EVENT_TYPES`, which would introduce release events and escaping for
  every key). On Windows the block under `#[cfg(unix)]` doesn't compile (imports are also
  cfg-gated — no unused warning).
- **`Alt+Enter` — a fallback line break** for terminals **without** the protocol: `Alt+Enter`
  arrives as `Enter`+`ALT` via the ESC meta prefix and is recognized even on legacy
  terminals (unlike the indistinguishable `Shift+Enter`). Adopted in **all three**
  multiline fields: chat (`screens/chat/input.rs`), a profile's system message/greeting
  (`screens/settings/apply.rs`, only `editor.multiline`), the self-model editor
  (`screens/self_model.rs`). The match was widened from `(Enter, SHIFT)` to
  `(Enter, m) if m.intersects(SHIFT | ALT)` — a bare `Enter` still sends/
  commits. On Windows `Alt+Enter` is often intercepted by the emulator (full screen) — but
  `Shift+Enter` still works there, so the overlap is harmless.
- **Docs**: spec §11.5 (the protocol + `Alt+Enter`) and §11.7 (the key table), README,
  the help overlay (`F1`/`?`: "Shift+Enter / Alt+Enter — line break"),
  docs/history/input-selection-undo-mouse.md §9 (groundwork closed).
- **Tests**: chat (`shift_and_alt_enter_insert_newline_not_send` — both insert a break,
  a bare Enter still sends the whole multiline input), settings
  (`alt_enter_also_inserts_newline_in_multiline_editor`), self-model
  (`alt_enter_inserts_newline_in_editor`). The wrap cache/selection unaffected.
  **934 unit tests green** (+3), 33 `#[ignore]`, clippy `-D warnings`/fmt clean.
  A live run of the kitty protocol on a real unix terminal (not reproducible on
  Windows) — the final manual step.

### Post-M9: InputBox — API hygiene (audit items 12–15) (done)
- The final part of the InputBox audit: four "API hygiene" items — a clean refactor/
  documentation with no user-behavior change. Branch `feat/input-box-hygiene`.
- **(12) `RenderOpts` instead of six positional arguments + a configurable placeholder**
  (`widgets/input_box.rs`, a precedent — `StatusModel`, SOLID stage 4a): `render(frame,
  area, opts: RenderOpts, palette)` — `RenderOpts { title, focused, command, placeholder }`
  (the palette is separate, like `StatusModel`). The placeholder for an empty, unfocused field
  used to be **hardcoded** ("type a message…") right into the generic widget — semantically
  foreign for settings/rename/search fields (they were only saved by always being
  `focused`, so the placeholder never rendered). A constructor `RenderOpts::focused(title)` —
  the typical case of modal fields (no placeholder/command highlighting); the chat screen passes
  the full literal with computed `focused`/`command` and its own placeholder. All five
  callers (chat/rename/settings editor/search/self-model) updated.
- **(13) `Ctrl+K` no longer duplicated across five screens**: clearing the field (`clear_undoable`,
  returned by `Ctrl+Z`) is already handled by `InputBox` itself in `on_key` (added in stage C
  undo). Removed **five** duplicate branches in consumers (chat `input.rs`, chat_list's
  `on_key_rename`, settings `apply.rs`/`search.rs`, the self-model editor) — `Ctrl+K`
  now falls through to their shared `_` path in `on_key`, where the needed side effects already exist
  (`mark_input_changed`/`spell_dirty` via `.edited()`; `editor.error = None`;
  `search_filter()`). Byte-for-byte behavior; a now-unused `let
  ctrl` in `on_key_rename` was also removed. Only one `Ctrl+K` branch remains — clearing the
  entire self-model with confirmation (`self_model.rs`'s non-editor path) — a different
  semantics, not clearing a field.
- **(14) The two definitions of "word" documented as a known divergence** (`word_left_col`):
  word navigation/deletion (`Ctrl+←/→`, `Ctrl+Backspace/Delete`) go by the
  whitespace/non-whitespace class (punctuation is part of the word), while spellcheck
  segments by its own rules (`features/spellcheck/segment.rs`, punctuation is a separate
  class). A deliberate simplification (as in large editors); no need to reconcile.
- **(15) The ZWJ/flag-width boundary documented** (a module doc in `shared/wrap.rs`):
  `width_at` handles VS16 and the skin-tone modifier, but ZWJ families (`👨‍👩‍👧`) and flags
  are measured by scalars — there's no "correct" answer (terminals render them
  differently). Cursor/navigation go by grapheme clusters (UAX #29) — only the width diverges.
- **Tests**: `placeholder_is_configurable_on_unfocused_empty_field` (the placeholder is drawn
  from `RenderOpts`, not hardcoded); the previous consumer tests for `Ctrl+K`→`Ctrl+Z` (chat_list
  rename, settings editor) stay green through the delegated path. **935 unit tests green**
  (+1), 33 `#[ignore]`, clippy `-D warnings`/fmt clean. A pure hygiene pass (no engine).
  **The InputBox audit (items 1–15) is complete.**

### Post-M9: spellcheck popup — selection style now matches the rest of the lists (done)
- **Symptom** (branch `fix/suggest-popup-selection-style`): the spellcheck
  suggestion popup (`Ctrl+G`) highlighted the selected item by **inverting
  the whole line** (`highlight_style(Style::new().reversed())`) and with no
  rail, while every other list in the app had long since settled on a
  different style: **a soft backdrop** `bg(palette.keycap_bg)` + **a green
  rail `▌`** (`palette.success`) on the selected line — the chat list
  (`widgets/chat_list.rs`, `item_line`), the settings screen's section
  menu and fields (`screens/settings/render.rs` + `render_field_line`),
  the "self-model" screen (`screens/self_model.rs`). The popup stood out
  from the rest.
- **Why this style, not reverse video** (the reasoning was already
  recorded for the settings section menu and carried over into the new
  code): `reversed()` swaps `fg↔bg` **per span independently** — the rail
  `▌` (a left half-block) ends up smeared over ~1.5 columns, and different
  spans of the line get different backgrounds. A uniform backdrop
  `keycap_bg` + a rail on top of it read cleanly.
- **The fix** (`screens/chat/popups.rs::render_suggest`): items are built
  via `.enumerate()`, the selected one gets a rail prefix `▌ ` in
  `success` color, the others get a matching-width indent (the text
  doesn't "jump" as the selection moves); `highlight_style` →
  `Style::new().bg(palette.keycap_bg)`. The selection index
  (`popup.selected.min(len-1)`) is computed once and reused for the
  `ListState`. `▌` is WGL4 — conhost compatibility mode still needs no
  substitution (like the feed's role rails and the `█` scrollbar).
- **A comment was tightened along the way** in `handle_suggest_key`: the
  full redraw fallback when closing the popup used to be justified by the
  selected line carrying `REVERSED`. The `keycap_bg` backdrop is, for
  ratatui 0.1.2, just as "visible on an empty cell" a style
  (`bg != Reset`, see architecture §4), so the mechanics are unchanged,
  but the wording now matches the fact.
- **Tests**: `suggest_popup_selection_matches_other_lists` (rendered in
  `TestBackend`: the rail is present and green, the backdrop is
  `keycap_bg`, no cell of the selected line is reversed).
  **Mutation-tested** — reverting to `.reversed()` fails it. **1180 unit
  tests green** (+1), 53 `#[ignore]`, clippy `-D warnings`/fmt clean. **No
  live run needed** (pure UI without engine or memory involved, covered
  by `TestBackend`).

### Post-M9: universal layout-independent hotkeys — stage 1, Windows (done)
- **Problem**: `Ctrl+<letter>` shortcuts were layout-independent for exactly
  **one** layout — `shared/keys.rs::physical_char` was a hardcoded table for the
  standard Russian JCUKEN. Under Greek/Hebrew/Georgian/Bulgarian/Armenian/Thai/
  Turkish/… the character passed through unchanged, the match against `'q'`/`'l'`
  failed, and every shortcut was dead. Research —
  [docs/research/layout-independent-hotkeys.md](../../docs/research/layout-independent-hotkeys.md),
  decision points R1–R5 accepted by the user per the recommendations 2026-07-24;
  branch `feat/universal-hotkeys-win`.
- **Key insight (from crossterm's own source)**: on Windows the character we
  receive is not raw input — for `Ctrl+<letter>` the console delivers a C0 code,
  and crossterm *computes* the character itself via
  `ToUnicodeEx(vk, active layout)` (`event/sys/windows/parse.rs::get_char_for_key`),
  discarding the VK/scan code. So the character can be **inverted with the mirror
  API**: `VkKeyScanExW(ch, hkl)` → VK → `MapVirtualKeyExW(vk, MAPVK_VK_TO_VSC, hkl)`
  → **scan code** → one static "Set 1 scan code → QWERTY" table (~47 entries,
  a hardware standard, one table for all languages forever). That's the whole
  mechanism: **no per-language data**, covers every installed layout including
  ones that don't exist yet. The static tables approach was rejected as the
  mechanism (§4A of the research): one script ≠ one layout (Bulgarian BDS vs
  Phonetic, Armenian East/West, Arabic 101/102 — a char-keyed table can't know
  the user's national variant; the OS can).
- **Tier ladder** (`hotkey_char(&KeyEvent) -> Option<char>`, the new single entry
  point): (0) **ASCII short-circuits** — never remapped by position, so on
  AZERTY/QWERTZ/Dvorak a shortcut still belongs to the key *labeled* with that
  letter (previous behavior, and what users expect); (1) Windows, **active
  layout** (`GetForegroundWindow`→`GetWindowThreadProcessId`→`GetKeyboardLayout`,
  mirroring crossterm) — the *exact* inverse, authoritative, also handles
  non-standard geometries like Russian Typewriter; (2) the static **JCUKEN**
  table; (3) Windows, **installed layouts** (`GetKeyboardLayoutList`) for
  characters the table doesn't know; (4) pass-through.
- **Why the table sits BETWEEN the two Windows tiers** (deviation from the
  research sketch, which put all WinAPI first): under conhost the foreground
  window's layout can't be queried (crossterm documents this) → tier 1 returns
  `None`, and probing installed layouts becomes a **guess** — with both Russian
  and Serbian installed the same letter sits on different keys, so the guess
  could regress today's Russian-on-conhost users. Keeping the known-good table
  ahead of the guess makes the change **strictly non-regressive**.
- **Call-site sweep**: 9 sites in 6 files (`screens/chat/{input,popups}.rs`,
  `screens/settings/apply.rs`, `screens/self_model.rs`, `widgets/chat_list.rs`,
  `widgets/input_box.rs`) moved from `keys::physical_char(c)` (inside a manual
  `if let KeyCode::Char(c)`) to `keys::hotkey_char(&key)`. The `KeyEvent`-shaped
  signature is deliberate: the future unix tier (below) arrives as a **field on
  the event**, so that upgrade is a one-file change instead of a second sweep.
  `physical_char` became a `#[cfg(test)]` pure core (table + lowercase);
  `is_slash_key` unchanged (both `/` and `.` are ASCII → not remapped, R4 defer).
- **Dependencies**: `windows-sys` (already direct — Job Object, DPAPI) gained
  `Win32_UI_Input_KeyboardAndMouse` + `Win32_UI_WindowsAndMessaging`. No new
  crates. Code lives behind `#[cfg(windows)] mod win_layout` in `shared/keys.rs`
  (precedent — `shared/secrets.rs::dpapi`, `shared/sandbox.rs`).
- **Unix is a separate mechanism, not a gap in this one**: the kitty keyboard
  protocol already carries the answer — the **base layout key** ("the key
  corresponding to the physical key in the standard PC-101 layout") of the
  `REPORT_ALTERNATE_KEYS` (0b100) enhancement — but **crossterm 0.29 parses only
  the shifted alternate and drops it** ([crossterm#968](https://github.com/crossterm-rs/crossterm/issues/968),
  open, no PR), so pushing the flag today gains nothing. Plan: contribute
  upstream (precedent — mermaid-text #29/#30 → 0.56.1), then prefer
  `base_layout_code`; recorded in docs/roadmap.md. Note the irony found during
  research: on kitty-protocol terminals our existing `DISAMBIGUATE` push
  *replaces* the terminal's own legacy fallback with faithful layout reporting —
  the **VTE family** (GNOME Terminal & co., which hasn't shipped the protocol)
  falls back to the Latin group itself and works for every language with no
  effort on our side.
- **Tests**: pure (table lookups, `hotkey_char` reads character keys only, ASCII
  pass-through, unknown characters); the **scan-code table** gate (letter rows +
  digits/punctuation + non-character keys like Esc/Tab/Space → `None`); and two
  OS-path tests that are machine-independent by construction — Latin letters
  resolve through installed layouts to *distinct* ASCII keys (true on QWERTY,
  AZERTY, QWERTZ alike), and a **self-calibrating cross-check**: if the standard
  Russian layout is installed (probed via a sentinel), the OS lookup must agree
  with the static table on all 26 characters, otherwise the test announces a skip
  (CI has no Russian layout; a Typewriter-only machine would legitimately
  disagree). **1273 unit tests green** (+6), 58 `#[ignore]`, clippy
  `-D warnings`/fmt/`cyrillic_scan` clean.
- **Live run — GO** (this machine, real Windows layouts, temporary diagnostic
  test removed afterward): the full OS pipeline resolved `д`→`l`, `й`→`q`,
  `ф`→`a`, `я`→`z` through **both** the active and the installed-layout tiers,
  agreeing with the static table; the self-calibrating cross-check ran (not
  skipped) over all 26 characters. **Bonus found in the run**: the resolver also
  fixes characters the table never had *within Russian* — `ю`→`.`, `ж`→`;`,
  `э`→`'`, `б`→`,`, `х`→`[` sit on punctuation keys, outside the 26 letter
  positions, so those `Ctrl` combos were dead before. Greek/Hebrew/Turkish
  characters returned `None` (those layouts aren't installed here) and correctly
  fell through to pass-through. A full interactive TUI check under a switched
  layout needs a real terminal — left to the user.

### Post-M9: universal layout-independent hotkeys — stage 3, upstream crossterm (submitted)
- **Stage 3** of the track (research
  [docs/research/layout-independent-hotkeys.md](../../docs/research/layout-independent-hotkeys.md) §4 C,
  fork R2): the unix half of layout independence isn't ours to write — the kitty
  keyboard protocol already carries the **base layout key** ("the key
  corresponding to the physical key in the standard PC-101 key layout"), but
  crossterm 0.29 parses only the shifted alternate and drops it
  ([#968](https://github.com/crossterm-rs/crossterm/issues/968), open since Feb
  2025, **zero comments**, no competing PR). So the work was an upstream
  contribution — precedent: mermaid-text #29/#30 → 0.56.1.
- **Patch** (submitted as
  [crossterm#1074](https://github.com/crossterm-rs/crossterm/pull/1074),
  +137/−15): the `CSI u` parser reads **both** alternates positionally — which
  also fixes a latent bug, since any alternate may be empty
  (`CSI 1076::108;5u`) and the old `codepoints.next()`, reached only under
  SHIFT, would then hand out the wrong slot; the base layout key is exposed as
  `KeyEvent::base_layout_code: Option<KeyCode>` (mirroring the existing
  enhancement-gated `kind`/`state` fields) plus a `with_base_layout_code`
  builder.
- **The one design decision**: the field takes **no part in
  `PartialEq`/`Hash`**. It describes the same key press rather than identifying
  it, and including it would silently break the ubiquitous
  `event == KeyEvent::new(KeyCode::Char('c'), CONTROL)` comparison for every
  application the moment it enables `REPORT_ALTERNATE_KEYS` — making the feature
  unusable. crossterm's manual impls destructure `KeyEvent` exhaustively, so
  this is an explicit `base_layout_code: _`. (Derived `PartialOrd`/`Ord` do
  include it, inconsistent with `Eq` — but they already disagree via
  `normalize_case`; pre-existing wart left alone, offered to the maintainers.)
- **Verification without a local unix toolchain**: no WSL at first, so the patch
  was checked by `cargo check --target x86_64-unknown-linux-gnu` locally +
  a throwaway workflow in our own repo (GitHub blocks Actions on forks until
  enabled by hand) that clones the patched fork and runs its suite on ubuntu.
  A local "just un-cfg the unix parser on Windows" attempt was **abandoned** —
  it cascaded into un-gating `InternalEvent` variants and then broke exhaustive
  matches in the Windows code, i.e. each hack lowered the fidelity of what was
  being tested. After the user installed WSL (**Ubuntu 24.04**, chosen to match
  `ubuntu-latest` in both CIs), everything was re-run natively: **119 crossterm
  tests green**, fmt/clippy clean, and — the load-bearing detail — **all
  pre-existing tests passed unchanged**, which is what makes "equality is
  untouched" concrete rather than asserted.
- **End-to-end spike caught a design error** (§4 C.2): our tree with
  `[patch.crates-io] crossterm = { path = … }` + the tier-0 branch, run on
  Linux. The base layout key must **not** be honoured for an ASCII character —
  on AZERTY the key labelled `A` sits at the US `Q` position, so it arrives as
  `Char('a')` with base layout key `q`, and taking it unconditionally would turn
  `Ctrl+A` (select all) into `Ctrl+Q` (**quit**) for every AZERTY user. Tier 0
  therefore belongs **after** the ASCII short-circuit — the same rule that
  protects Latin layouts in the Windows tier. Spike results: Greek `λ`, Hebrew
  `ק`, Cyrillic `д` resolve through the protocol to `l`/`e`/`l`; AZERTY `a`/`q`
  stays `a`; without the enhancement the table still answers. Our full suite on
  Linux: **1271 passed** (the delta from 1273 on Windows is exactly the
  `#[cfg(windows)]` tests), clippy `-D warnings` clean. The spike is
  deliberately **not committed** (it cannot build without the patched
  crossterm) — the research doc is its durable form.
- **Our side stays put until an upstream release** (bump → push
  `REPORT_ALTERNATE_KEYS` → add the tier-0 branch; no call-site changes thanks
  to the `KeyEvent`-shaped `hotkey_char` from stage 1). Timing caveat recorded
  in the roadmap: crossterm merges PRs regularly, but **the last crates.io
  release was 0.29 in April 2025** — the release, not the review, is the long
  pole. No unit-test count change in this repo (docs only).

### Post-M9: spellcheck skips URLs and email addresses (done)

- **Asked for directly**: the input box's spellcheck shouldn't touch URLs — and,
  once that landed, email addresses too. It was underlining a pasted link word by
  word — `github`, `vshylov`, `mindfork`, `blob` — i.e. the loudest noise lands
  on exactly the text a user *pastes* rather than types, and none of it is
  correctable. Branch `fix/spellcheck-skip-urls` (a simple task by AGENTS.md §1:
  one module, no cross-layer contract, no new dependency — no design doc).
- **The fix goes in segmentation, and that's the whole reason it is small.**
  `segment::words` has exactly two callers, both in `SpellChecker`
  (`misspellings` for the underlines, `misspelled_word_at` for the `Ctrl+G`
  popup), so skipping a link there covers both with no second rule to keep in
  sync — and the `chat_list` rename field, which shares the checker, comes
  along for free. `words` skips a link span whole rather than filtering
  afterwards, so no word can even start inside one; spans are
  whitespace-delimited and words never contain whitespace, so a word is always
  entirely inside or entirely outside one, and the character offsets after a
  link still address the original line (pinned by a test — those offsets are
  what draws the underline).
- **Four shapes, deliberately a heuristic and not a parser**: an explicit
  scheme (matched *anywhere* in the token, so `(https://x)` counts without
  trimming games), a `www.` prefix, a bare domain — the last one because
  `github.com/foo` is what people actually paste — and an email address.
  Surrounding punctuation is trimmed, so a trailing sentence period, `«…»` or
  `<user@example.com>` doesn't hide the link.
- **The load-bearing detail is the restriction on bare domains: lowercase
  ASCII.** Without it, a missing space after a period reads as a domain —
  `end.Next`, and its far more common Cyrillic equivalent — and the check would
  switch itself off for a genuine typo, silently. Requiring lowercase ASCII
  labels rules out both (the Cyrillic case by script, the English one by the
  capital that follows a period), while `example.com` and `sub.example.co.uk`
  still match. A digits-only last label (`3.14`) is not a TLD, so version
  numbers are unaffected. Filenames like `main.rs` do read as domains — that
  is a bonus rather than a cost: a filename isn't prose either.
- **The `@` inverts that restriction, which is why emails got their own
  branch rather than being folded into the domain rule.** An address is
  unambiguous by shape — there is no run-on sentence containing an `@` — so the
  host may be written in any case (`Vladimir.Shylov@Outlook.COM`), where a bare
  domain may not; the two cases are carried by a `DomainCase` parameter rather
  than by two copies of the label check. The local part keeps its own case and
  allows the characters people actually use (`._%+-'`), an optional `mailto:` is
  stripped, and the split is at the **last** `@`. Conversely, a bare `@` is not
  enough: `@username` has no domain, so a mention stays checkable.
- **Scope**: file paths are *not* covered — widening further would be a
  different feature. Neither are internationalized (non-ASCII) domains; the
  lowercase-ASCII rule is what buys the run-on-sentence guard.
- **Tests**: the four shapes skipped and the prose around them still checked;
  punctuation around a link; the run-on-sentence guard in both scripts plus a
  dotted abbreviation and `3.14`; a mention and a domain-less `a@b` staying
  checkable; offsets surviving a skipped link; and, in `check.rs`, that a URL is
  neither underlined nor offered suggestions. **Mutation-tested**: forcing
  `is_link_token` to `false` fails exactly the four URL tests, and forcing
  `is_email` to `false` fails exactly the email one — while the two negative
  guards stay green in both runs, as scope pins should. **1756 unit tests
  green** (+7), 73 `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan`/
  `link_check` clean.
- **A live run isn't required** (AGENTS.md §3): pure text segmentation inside
  one feature module — no engine, memory, tool or provider path is touched.
- Docs: spec §11.5 (the rule and its boundary; the same bullet also corrected a
  stale claim that segmentation uses `unicode-segmentation` — it has been our
  own scanner since M3).

### Post-M9: `Home`/`End` as a ladder of stops (done)

- **Asked for directly**: `End` moved to the end of the row a wrapped line was
  broken into, and there was no way short of `Ctrl+End` — which leaves for the
  end of the *whole text* — to reach the end of the line itself; same for
  `Home`. Then, on the same branch, **`Home` was asked to stop at the first
  non-whitespace character first** ("smart home"). Branch
  `feat/input-home-end-toggle` (a simple task by AGENTS.md §1: one widget, no
  cross-layer contract, no new dependency — no design doc).
- **The steps are told apart by the cursor's position, not by counting
  presses.** `move_home`/`move_end` compute the ladder of stops and hand out
  **the next one after where the cursor already is** (the first stop if it is at
  none). Stateless, so nothing has to be reset by the many other things that
  move the cursor (typing, paste, undo, a mouse click, `activate_chat`) — a
  press counter would need clearing at every one of them, and forgetting one is
  a silent bug. It also does the useful thing when the cursor reached a stop
  **by typing** rather than by `Home`, which is the common case at the end of a
  line; that is the rule large editors use.
- **The `Home` ladder, and why the third stop is conditional**
  (`home_stops`): the row's first non-blank → the row's start → **the line's**
  first non-blank → the line's start, deduplicated by value. The line-level text
  stop is only included when it lies **before** the row's start, i.e. on the way
  left from a later row of a wrapped line: on the line's first row it is already
  the row's own text stop, and in the pathological case of indentation wider
  than the field it would make `Home` jump *forward*. With the guard every case
  came out sane, checked by probing the real stop lists rather than by reasoning
  alone — an unindented line collapses to the two steps it had before, an
  indented wrapped line gets three from its bottom row.
- **The other boundaries coincide where you would expect them to**, so no case
  needs a special branch: on a line's last visual row the row end *is* the line
  end, so `End`'s second step is simply a no-op there. `End` deliberately gets
  **no** trailing-whitespace stop (nobody indents the right margin), and
  `single_line` mode keeps `Home` at column 0 — a settings path that starts with
  an accidental space is easier to fix from there than from its text.
- **A whitespace-only row has nothing to skip to**, so `first_non_blank` yields
  the range's start and the stop collapses instead of throwing the cursor to the
  far end of the blanks.
- **The line-level steps stop at their own line** — they use
  `self.lines[self.row]`, never the visual-row table, so they cannot run past a
  real `\n` into a neighbouring line (pinned by its own test, since a wrapped
  line's row table spans the whole text).
- **A pre-existing invariant kept the change small**: `col_for_visual` already
  rolls back off a soft wrap (`is_soft`), so the first `End` never yields a
  position that renders at the start of the *next* row — which is what makes
  "already at a stop" a well-defined comparison.
- **Tests**: the full ladder in both directions (row → line → a further press
  staying put), the "stays on its own logical line" guard, the indented line
  (including `Home` pressed from *inside* the indentation), the indented wrapped
  line's three-step ladder, and the all-blank row; the existing
  `home_end_act_on_visual_row` is unchanged and still green — an unindented
  line's first press behaves exactly as before. **Mutation-tested**: dropping
  the row-text stop, the line-text stop, the blank-row fallback or either
  toggle fails exactly its own test(s) and nothing else. **1792 unit tests
  green** (+5), 75 `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan`/
  `link_check` clean.
- **A live run isn't required** (AGENTS.md §3): cursor movement inside one
  widget — no engine, memory, tool or provider path is touched.

### Post-M9: pasting an image from the clipboard (done)

**What.** `/image paste` and `Ctrl+V` stage the image sitting on the system clipboard, so
a screenshot never has to be saved to a file first. The last deferred item of the
multimodality track (spec §9.10).

**The finding that shaped it: `Ctrl+V` alone cannot work.** The app has never had a
`Ctrl+V` handler — pasting works because the *terminal* injects the clipboard text, which
unix delivers as a bracketed-paste event and Windows as a run of ordinary key events that
`chunk_batch` reassembles. An image produces **no text**, so the terminal injects nothing
and the app sees nothing at all. Windows Terminal compounds it by binding `Ctrl+V` to its
own paste and never forwarding the key. This is the same shape as the known
supplementary-plane limitation recorded in this file: a paste that yields no key events
cannot be observed. Hence two routes, and the **command** is the reliable one:
`/image paste` works in every terminal and is what the help overlay names; `Ctrl+V` is a
convenience where the key is forwarded at all.

**Key decisions.**

- **`Ctrl+V` with no image on the clipboard pastes text.** The help overlay has always
  advertised `Ctrl+V — paste`, and until now that was a statement about the terminal
  rather than about the app. On a terminal that forwards the key it was simply false.
  Making the handler fall back to text makes the promise true where it can be, and costs
  nothing where the terminal handles it first. A typed `/image paste` deliberately does
  **not** fall back: the user asked for an image, so an absent one is reported, naming
  `/image attach` as the route that does not involve the clipboard.
- **The clipboard is read in `runtime`**, not in the orchestrator — the same arrangement
  `Ctrl+C` uses, since only `runtime` holds the `arboard` client. The orchestrator is
  handed pixels (`AppCommand::ImagePaste(Box<ClipboardImage>)`) rather than a request to
  go and look, which also keeps it testable without a clipboard at all: the integration
  tests feed RGBA directly.
- **Un-encoded across the channel, encoded on the blocking pool.** arboard hands over raw
  RGBA (it normalizes `CF_DIB` / `image/png` / `NSImage` itself, so there is no container
  to sniff). Encoding a screenshot costs tens of milliseconds and the clipboard is read on
  the **input thread**, so the png is produced where every other image already is.
- **Always png**, unlike a file, where a photographic source becomes jpeg. The dominant
  clipboard image in a terminal is a screenshot and jpeg artifacts on small text are the
  expensive failure; the size that would argue for jpeg is already bounded by the same
  downscale every image gets.
- **A synthetic name and a colliding-proof source.** No file exists, so a pasted image
  takes the first free `clipboard*.png` and a `clipboard:<uuid>` source. Dedupe is by
  source, so a constant one would have made a second screenshot silently replace the
  first — the failure that would cost the user the thing they had just copied.
- **`arboard`'s `image-data` feature is now on.** The comment that turned it off cited the
  `image` crate it pulls; that crate has been a direct dependency since stage 1 of this
  track, so the original cost argument had expired.

**Tests.** Unit: `prepare_rgba` (raw RGBA to png, opaque input still png, downscale,
and every mismatched-buffer case refused rather than reinterpreted at a guessed stride —
including a size whose byte count overflows); the parser (`/image paste`, and a gate that
the usage line names every subcommand in both locales); the chat screen (both routes
produce an intent and differ only in the text fallback; the command is highlighted while
typing); the orchestrator (a paste stages and travels with the message, two pastes get
distinct names, a malformed buffer is refused with *its own* message — asserted through
the locale key, since the fixture runs in `ru`).

**Smoke — GO** (2026-08-13, Windows 11): `clipboard_image_round_trip`, `#[ignore]` because
it clobbers the developer's real clipboard, puts a 64x32 image on the system clipboard,
reads it back and encodes it — size preserved, colours intact (asserted as ">100 distinct
colours" rather than byte equality, since a platform may composite or reorder channels),
3045 bytes of png out. That is the check the roadmap asked for when it deferred this: only
a real clipboard can show the feature is actually wired on the platform.

**A live model run is not required** (AGENTS.md §3): this changes where an image's pixels
come from, not what is sent — the request path, the wire formats and the staging are the
ones stage 1 and 2 already covered live.

### Post-M9: `/exit` and `/quit` — a typed route out (done)

- **Why.** Quitting has had two keys since the selection/undo track moved it off
  `Ctrl+C`: `Ctrl+Q`, with `F10` as the second option "in case the terminal/DE
  intercepts `Ctrl+Q`" (spec §11.7). That pairing assumes a host claims at most one
  of them. VS Code's integrated terminal claims **both** — `Ctrl+Q` is an editor
  chord, `F10` is the debugger's "step over" — and there the app has no advertised
  exit at all, since the help overlay teaches exactly those two keys. A slash
  command is ordinary typed text and reaches the app whatever the host binds, so it
  is the one route that cannot be taken away. Same argument, already made once in
  this project: `/image paste` exists next to `Ctrl+V` because Windows Terminal
  keeps that key (spec §9.10).
- **Two spellings, not one.** `/exit` and `/quit` are what every REPL, shell and
  database client answers to; someone hunting for the way out types whichever they
  already know rather than opening the help they are trying to leave. Kept as one
  `ALIASES` table so the parser, the help label and the tests cannot drift — the
  test that pins the label iterates the table rather than repeating the strings.
- **It quits mid-answer, like the keys.** The check sits after the command chain
  but *before* `handle_enter`'s `generating` gate, which is what holds ordinary
  messages back. The test carries its control arm (`/exit please` during generation
  → `None`), so the assertion is about the command rather than about a gate that
  might simply not be there.
- **The trap this actually had: the command would come back as the draft.** Every
  keystroke sends `SetDraft`, so by the time `Enter` arrives the orchestrator has
  already been told the box holds `/exit`, and `AppCommand::Quit` writes it to the
  chat — the next launch would greet the user with the command they used to leave.
  Clearing the box is not enough on its own: `run_loop` reads the draft at the
  **top** of the iteration, and the tick that quits breaks out at the bottom, so the
  clearing edit is never picked up. One flush after the loop (before the caller's
  `AppCommand::Quit`, which the channel keeps ordered behind it) closes it. Found by
  reading the loop rather than by running it — the symptom only shows up on the
  *next* launch, which is the kind a live pass tends to walk past.
- **It is highlighted while being typed**, like every other command:
  `input_is_command` gained the parser, so the box turns the whole line the
  command colour and stops spellchecking it (spec §11.5). Two things fall out of
  reusing the parser rather than matching a prefix: a *malformed* `/exit now` is
  still highlighted — it is a command, not prose, and the highlight going away
  mid-typo would be a lie — while `/exiting` and "how do I quit vim?" stay plain
  text, which is exactly what happens to them on `Enter`. What the box shows and
  what `Enter` does come from the same function, so they cannot disagree.
- **A stray argument leaves a note, not silence.** `/exit now` reports instead of
  going out to the model, as `/reindex` and `/compact` do — and here the silence
  would read as "the app refused to quit". The message names the bare command *and*
  the two keys, so a mistyped quit still ends with a route (lessons §4); a gate test
  asserts both are present in every bundled locale. The message quotes the spelling
  the user typed, never the other one, which would read as a different command
  having been recognized.
- **Sizing the help row.** The commands tab is a fixed-width strip (76 columns) with
  no wrap, so a long description is silently clipped — and the first wording was
  89 columns in `ru`, where the label already costs 15. Measured both locales
  against the existing rows and shortened to fit inside the table's current
  envelope. (The hotkeys tab already overflows — `Ctrl+F` is 89 columns in `en` —
  which is a pre-existing clip, not this change's.)

**Tests** (+11). The parser: every alias bare, padded and upper-cased, driven off
`ALIASES`; trailing arguments rejected for both; the error names the typed spelling
and the argument and not the other spelling; neighbouring commands, longer words
that merely start with one (`/exits`, `/quitter`) and plain prose (`how do I quit
vim?`) all fall through as messages; the per-locale gate. The chat screen: both
spellings produce `Quit` and hand back an **empty** draft; quitting during
generation with its control arm; a mistyped quit notes and does not send; text that
only resembles the command is sent; and the highlight — both spellings, a malformed
one, `/exiting` and prose, all driven off `ALIASES`. The help overlay: the row is
present, last, and renders with its localized description, with the label checked
against `ALIASES`.

**A live model run is not required** (AGENTS.md §3): this is input-box parsing and
UI routing — nothing on the engine, memory or tool paths. The one thing unit tests
cannot reach is the flush itself, which lives in `run_loop` (no TTY under test):
the screen-level test asserts the empty draft is handed back, and that it *reaches*
the orchestrator wants a real terminal — type `/exit` with text in the box, relaunch,
and the box should be empty.

### Post-M9: command-only control — stage 1 (typed routes for the chords) (done)

- **Why.** `/exit` existed because VS Code's integrated terminal claims *both*
  advertised quit keys. That argument was never specific to quitting: measured
  against VS Code's own `DEFAULT_COMMANDS_TO_SKIP_SHELL` (read from
  `src/vs/workbench/contrib/terminal/common/terminal.ts`, `main` as of
  2026-08-14) the host also takes `Ctrl+P`, `Ctrl+E`, `Ctrl+F`, `Ctrl+K`, `F1`,
  `F3`, `F5` and `F10`, so the settings screen, in-feed search, take-back,
  copy-conversation, the self-model screen and the help dialog were **all
  unreachable there**. In a browser tab (JupyterLab) the loss is different and
  worse: `Ctrl+N`/`Ctrl+T` never arrive, and `Ctrl+W` **closes the tab the
  session runs in**. The design doc is
  [docs/history/command-only-control.md](../history/command-only-control.md);
  all seven forks were decided by the user on 2026-08-14 (the recommendations).
- **The invariant that made the scope small.** "No hotkeys" cannot mean "no
  keys" — typing is keys. What every host forwards is the printable characters,
  `Enter`, `Esc`, `Backspace`/`Delete`, `Tab`, the arrows, `Home`/`End`,
  `PageUp`/`PageDown` and `Shift`+arrows; what dies is the chord class. Since
  every `Esc` chain already terminates at the chat screen, and the chat screen is
  where the input box lives, "safe keys navigate, commands act" covers the whole
  application with no third mechanism — which is why fork F1 rejected a
  leader-key command mode on every screen as redundant.
- **A command is its key, by construction.** Every arm of `run_ui_command` calls
  the *same* handler the chord uses (`handle_ctrl_shortcut`, promoted to
  `pub(super)` for this, or the chord's own intent) rather than re-deriving the
  action. That is what makes the confirmation popup, the `generating` gate and
  the per-chat feed-collapse state apply identically to `/regen` and `Ctrl+R`
  without a second code path to keep in step. The test asserts it the same way:
  two identical screens, one typed and one pressed, must return the same intent.
- **What a command adds is a voice.** A chord that does nothing costs a
  keypress; a typed command that vanishes reads as the app refusing (lessons §4).
  So each precondition answers: nothing to cancel for `/stop`, a turn in flight
  for `/regen`·`/takeback`·`/impersonate`, no chat for `/copy`·`/clone`·`/rename`,
  no settings snapshot yet for `/settings` — each naming a route that works.
  **A test found the one place I had written the silence anyway**: `/rename`
  began `let id = self.active_chat?`, and `?` on a `None` swallowed the refusal.
  The defect class this track exists to remove, reintroduced inside it.
- **`Esc`'s overload split in two.** `Esc` means cancel-the-turn *or* go-back
  depending on state; `/stop` is always the first and `/chats` always the second
  (fork F6). Cheap — two registry rows — and it removes the one place where the
  safe-key route was ambiguous rather than absent.
- **One registry, not nineteen sibling modules.** Each command is an exact word
  plus at most one free-text argument, so nineteen copies of `exit_command.rs`
  would have been precisely the sliding self-duplication the gate keeps catching
  (lessons §2, five recurrences). Commands with real syntax — subcommands, flags,
  `#N` targets — keep their own modules; **the design doc had promised separate
  parsers for the five argument-taking commands and that was wrong**: measured
  against `file_command`, none of them has any syntax to explain.
- **The table itself is a duplication hazard, and was measured against the
  lesson rather than hoped about.** Nineteen five-line `Spec { … }` literals is
  ~95 identically shaped lines — the shape that scored 16.1% on 50 rows. A
  `const fn row(…)` collapses each to one line; `rustfmt` then exploded them
  back to seven lines each, so the table carries `#[rustfmt::skip]` (the
  precedent is `widgets/logo.rs`). ~19 lines instead of ~95, and it reads as a
  table. The Sonar snippet analyzer could not pre-check this — it has no Rust
  language — so the number came from the PR analysis: **0.0% duplication on new
  code** against the 3% bar, with every new-code rating A and no new issues.
  Worth recording as the counter-case to the lesson's four failures: the shape
  that scored 16.1% at 50 struct-literal rows scores nothing at 19 one-line
  ones, so collapsing the row is a sufficient fix on its own.
- **Bare `/rename` prefills instead of opening a popup** — a deviation from the
  design doc, recorded here. The box is where a command is typed and holds
  nothing else at that moment, so handing back `/rename <current title>` for
  editing needs no new modal, no new key routing and no new render path, and it
  teaches the syntax by example. It is also the one command whose *answer* is
  text in the box, which its test carves out of the "every command clears the
  box" rule.
- **The help tab is derived, not restated.** `popups::command_rows()` composes
  the parser-owning commands, then the registry's rows, then the way out — so a
  command cannot exist without a help row. `/exit` moved into a one-row tail
  const so the registry can be inserted in front of it without the quit row
  moving off the end. Consequence worth noting: the tab is now taller than the
  dialog, so `commands_tab_text()` reads it in **two passes** (unscrolled, then
  scrolled past the end) — a one-pass helper would have silently stopped
  asserting about everything below the fold.
- **Not commands, deliberately: text editing.** A command is typed *in* the box,
  so it cannot act on the box's contents — `/undo` would destroy the object it
  operates on. The safe keys cover editing in every host. Profile CRUD
  (`Ctrl+N`/`Ctrl+D` in the settings screen) and clearing the self-model
  (`Ctrl+K` twice) stay chord-only; fork F5 put them in stage 2 as
  `/profile new|delete` and `/self clear`, since a Mac client keyboard has no
  `Insert` to offer as an in-screen alternate.

**Tests** (+23, 2222 → 2245 green). The parser, driven off the registry: every
alias bare, padded and upper-cased; the registry's own hygiene (no word claimed
twice, every label shows every spelling it accepts, the label's brackets agree
with the arity); a bare word rejecting arguments and naming the typed spelling
only; a required argument asked for with the help label as its usage line; the
argument being the whole remainder, spaces included; neighbouring commands,
near-words and prose falling through; the per-locale error gate. The screen: the
command-versus-chord agreement table (fixed ids in the harness, or every intent
carrying one would differ); every command clearing the box and handing back an
empty draft; highlighting for every alias and malformed variant; each blocked
command naming itself and `/stop`; `/stop` versus `/chats` mid-turn with `Esc` as
the control; the confirmation popup for both destructive commands; `/search` and
bare `/search`; `/find` seeding the query; `/rename` both ways and its length
ceiling; `/new` exact, prefix, ambiguous and missing; text that only resembles a
command still being sent; no command being a silent no-op; commands not firing
through a modal. Mutation-tested in three places — a delegation replaced by
`None`, the box not cleared, the required-argument check disabled — each caught
(the third by three tests).

**A live model run is not required** (AGENTS.md §3): this is input parsing and
UI routing, with nothing on the engine, memory or tool paths — every new intent
maps onto an `AppCommand` the chat list already sent. What unit tests cannot
reach is the claim that motivated the track, since it is about *hosts*: the
acceptance pass is manual, in a real VS Code integrated terminal and a real
JupyterLab terminal, exercising each tier-1 command and operating every modal
screen with safe keys only.

### Post-M9: command-only control — stage 2 (`/profile`, `/self clear`) (done)

- **What was left.** Stage 1 covered the chat screen; two actions still existed
  only as chords *inside* another screen — profile CRUD (`Ctrl+N`/`Ctrl+D` in
  the settings screen's "Profiles" section) and clearing the self-model
  (`Ctrl+K` twice). Fork F5 chose commands over in-screen alternates because a
  physical-key alternate such as `Insert` is missing from a Mac client keyboard,
  and a browser terminal is reached from whatever machine the user is sitting
  at. Stage 1 verified live in VS Code's integrated terminal by the user before
  this stage started.
- **Where the design doc was wrong, found by reading the code.** It said
  deletion cascades were "already confirmed today". They are not: `Ctrl+D` in
  the settings screen issues `DeleteProfile` outright. So the confirmation is
  **new**, and it is a deliberate break from "a command is its key" — the one in
  the whole track. The justification is asymmetry of information, not caution
  for its own sake: the screen shows the row it is about to delete, while
  a shortened `/profile delete <name>` resolves a *prefix* and can land on a
  profile the user did not picture. The popup names the profile and how many conversations go
  with it, which is exactly what the screen's selection was doing.
  (lessons §3 — "treat a recorded rationale as a claim with a date on it".)
- **Where the confirmation lives, and a dead method that proved it.** I first
  routed the new actions through `trigger_destructive` and gave `ConfirmAction`
  an `always_confirms()` predicate. Then the commands turned out to set
  `self.confirm` directly — the predicate was never called. Rather than reroute,
  the *difference* was the answer: these two differ from the destructive keys in
  **both** respects (they always ask, and generation does not gate them, since
  the keys they mirror are not gated either), so two call sites are honest where
  one funnel with two booleans would not be. The method came out; the type's doc
  comment carries the distinction.
- **`/profile` earns a module, `/self` does not.** The registry's own rule from
  stage 1 is that a command with real syntax gets a parser file. `/profile` has
  a subcommand *plus* a free-text name — the first typed route to cross that
  line — so `features/profile_command.rs`. `/self clear` is a single word from a
  closed set, which became `Arity::Subcommand(&["clear"])` in the registry: the
  parser normalizes the word and reports an unknown one *itself*, because a
  command with two places to explain itself grows two wordings.
- **The outcome is reported by the owner, not by the command.** `/profile new`
  in a chat has no profile list on screen to serve as the answer, and a command
  that appears to do nothing reads as a refusal (lessons §4). The first draft
  pushed a local note — which would have lied on the two paths where
  `handle_create_profile` fails (an empty name, a storage error). Emitting
  `AppEvent::Notice` from the orchestrator instead means the claim is only made
  when the write succeeded, and it fixes the *key* route too: `Ctrl+N` in the
  settings screen now also says what happened.
- **Refusals that come before the question.** `/profile delete` checks the
  last-profile rule itself instead of asking a question whose "yes" the
  orchestrator would then decline; `/self clear` refuses with no chat open,
  since the model belongs to the active profile and the orchestrator would
  silently drop the edit (the same `?`-shaped silence stage 1's lesson is about,
  avoided here by looking for it).
- **One resolver for two commands.** `/new <profile>` and `/profile delete
  <name>` both name a profile, so the exact-then-unambiguous-prefix rule and its
  candidate-listing note are now a single `resolve_profile`, parameterized only
  by the *route* it suggests (a whole bundle key, never a built one — lessons
  §7). Budgeting for that seam at design time is the duplication lesson applied
  before the gate rather than after it.
- **`tf` caught the wording split.** Moving the route into a `{route}`
  placeholder while the templates still spelled `/new` into the sentence failed
  immediately — `tf` asserts on arguments the template does not use. The
  assertion is what made a two-locale edit safe.

**Tests** (+13, 2245 → 2258 green). The parser: every subcommand in any case and
padding; a name keeping its spaces and losing its quotes; missing and unknown
subcommands reported with the offender quoted; near-words falling through; the
per-locale gate. The screen: `/profile list` naming the profiles and marking the
open chat's; `/profile new` matching the settings screen's default name; delete
always confirming with the profile and chat count in the question, and `Esc`
answering no; the last profile refused before the question; the shared resolver
used by both callers with each keeping its own suggested route; `/self clear`
confirming and reaching the same edit, bare `/self` still opening the screen, an
unknown word reported, and no chat open refused; highlighting for the whole
family. Mutation-tested in two places — deleting without asking, and dropping
the active-chat guard — both caught.

**A live model run is not required** (AGENTS.md §3): UI routing and two
orchestrator commands that already existed, with no engine, memory or tool path
touched. The acceptance pass is the same manual one as stage 1, plus: create a
profile from a chat and see the notice, delete it and see the question naming
its conversations.

### Post-M9: OSC 52 — copying to the client's clipboard (done)

- **Why.** `arboard` writes the clipboard of the machine the *process* runs on.
  Over SSH that is the wrong machine; on a headless box there is no clipboard at
  all and the constructor fails, so `/copy` there produced nothing but an error.
  OSC 52 hands the text to the terminal the user is actually sitting at, over the
  same pipe the drawing takes. Design doc:
  [docs/history/osc52-clipboard.md](../history/osc52-clipboard.md), all forks
  decided by the user on 2026-08-15 (the recommendations).
- **The roadmap item's premise was wrong, and the research is what found it.**
  I had written that item myself, three days earlier, with **JupyterLab** as the
  motivating case. Measured against the sources: JupyterLab's terminal package
  depends on `addon-fit`, `addon-search`, `addon-web-links` and `addon-webgl`,
  and its `widget.ts` registers OSC 8 and nothing else — xterm.js ignores OSC 52
  unless the host loads `@xterm/addon-clipboard`, so **the escape is dropped**.
  The mirror image: VS Code *does* load that addon (`xtermTerminal.ts`, both
  directions wired to its clipboard service) but usually runs the pty locally,
  where `arboard` was already right. The beneficiary list inverted — what
  survives is plain SSH and VS Code's remote modes — and the feature is still
  worth having, because on headless SSH the alternative is not "the wrong
  clipboard" but "no copy at all". Recorded as a second recurrence of the
  read-the-code-before-designing lesson.
- **A fallback, not a replacement** (fork F1a). The local write happens first and
  always: it costs nothing when it works, and its failure is one of the two
  signals that the terminal's clipboard is the one that matters. The other signal
  is `SSH_TTY`/`SSH_CONNECTION`. Terminal support itself is **not** detectable —
  the only query is the OSC 52 *read* form, which is the leak vector terminals
  disable and which we refuse to use — so `always` exists for the remote sessions
  the environment does not advertise (containers, web terminals) and `off` for a
  terminal that renders an unknown OSC as text.
- **The protocol acknowledges nothing, so neither does the note.** Three outcomes
  get three wordings (fork F4): the plain local copy keeps the old sentence; a
  sent sequence says *sent*, and says the terminal does not confirm it, so a user
  whose terminal silently ignored it knows what happened; over the ceiling
  nothing is sent and the note says where the text did go. Getting this wrong in
  the friendly direction — one cheerful "copied" — would be the project's
  recurring defect in its purest form.
- **The ceiling is 74 994 bytes and the copy is refused past it** (fork F3). The
  100 000-byte sequence limit less the header and base64. Real terminals are
  worse (kitty rejected over 6 138 bytes, tabby fails near 1 KB), but truncating
  everyone to the worst of them would be worse than falling back to the local
  clipboard — and a silently halved conversation *looks complete*, which is the
  failure mode worth engineering against. The corpus makes this concrete: single
  messages of 38 KB exist, so a whole conversation passes the ceiling routinely.
- **tmux yes, screen no** (fork F5). tmux needs DCS passthrough with every inner
  escape doubled — a wrapper that forgets the doubling ends the passthrough early
  and sprays base64 across the screen, which is why the test asserts the exact
  bytes. `screen` needs a different wrapper *and* 768-byte chunking; unsupported,
  and it degrades to exactly today's behaviour.
- **A surviving mutation that was right, and one that was my own fault.** The
  first "mutation" I wrote was `if false { … } else if tmux { … }` — which changes
  nothing, so its survival proved nothing (lessons §2: read a surviving mutation
  twice). The real one, dropping the escape doubling, was caught. But a genuine
  survivor followed: removing the ceiling check from `copy_text` changed real
  behaviour — an oversize copy would report the cheerful wording — and **every
  test passed**, because `copy_text` needs a real clipboard and a real stdout and
  so had no test at all. Extracting the decision into a pure `plan_terminal`
  (skip / too large / send) put the rule under a table test and killed the
  mutation. The general shape: when a function mixes a decision with two side
  effects, the decision is what the tests can reach.
- **The settings screenshots had to be regenerated**, since the "Interface"
  section grew a row. Per lessons §1 the font was verified first by re-rendering
  an *unchanged* frame and checking it came back byte-identical — it did, so the
  four settings images differ only by the new row. (Also learned: the script
  refuses an `--out` outside the repository, which is the path-confinement rule
  from the SonarQube lesson doing its job.)

**Tests** (+10, 2258 → 2268 green). The sequence's exact bytes for ASCII,
multi-byte UTF-8 and empty; the tmux wrapper with its doubled escapes; the
ceiling at the boundary and counted in bytes rather than characters; `write_to`
reporting what it did and writing *nothing* when it refuses; the emit-decision
matrix over mode × remote × local-failure; the default being the quiet one; the
plan matrix including "a skip never becomes a size report"; the three note
wordings, each distinguishable, plus the per-locale gate; the settings field
cycling auto → always → off and back. Mutation-tested in three places, one of
which exposed the untested seam described above.

**A live model run is not required** (AGENTS.md §3) — no engine, memory or tool
path is touched. What unit tests cannot reach is the claim itself, since it is
about terminals: the acceptance pass needs **two machines** — SSH from a laptop
into a headless box, `/copy` there, paste locally — and is worth repeating under
tmux and once in VS Code Remote-SSH.

### Post-M9: `/project` is highlighted while it is typed (done)

**What.** `/project …` was the one command family the input box did not colour:
typing `/project attach .` looked like an ordinary message all the way to
`Enter`, which then attached the project. It was also spellchecked as prose —
the same predicate gates both — so a path or a slot name could pick up red
underlines under a line that was never going to be sent anywhere.

**Why it happened.** There are two lists of the command parsers, and they are
kept by hand. One is the dispatch chain in `ChatScreen::submit` (`try_rag_command`
→ `try_reindex_command` → … → `try_exit_command`); the other is the pure
predicate `input_is_command`, which the renderer asks every frame for the
`warning` style and which `maybe_recheck_spelling` asks to skip the dictionary.
The code-workspace track added `/project` to the chain and to nothing else, and
because the chain is what makes the command *work*, every test and every live
run of that track passed. Nothing reads the two lists together, so the drift was
invisible from inside the code — it is visible only on screen, in the colour of a
line nobody asserted about.

**The fix** is one arm, placed at the same position in the predicate that
`try_project_command` occupies in the chain — between `/file` and `/image` — so
the two read as the same list rather than as two sets that happen to agree. The
doc comment now says out loud that they are the same parsers in the same order,
and names this defect, because the next command added will face exactly the same
fork.

**What was deliberately not done: unifying the two lists.** The obvious repair is
to make the chain the single source of truth and derive the predicate from it.
The chain cannot serve as one: each `try_*` is `&mut self`, clears the box,
pushes a note on a syntax error and returns an intent — asking it "is this a
command?" would run all of that on every keystroke, on every frame. The other
direction (a registry row per family, the way `ui_command::COMMANDS` already
works) is a real refactor of eleven parsers, three of which are in the chain
precisely *because* they take a subcommand plus an argument and did not fit a
registry row (`/profile`, `/export`, `/project` itself). That is a track, not a
bug fix; what this change buys instead is a test that fails when the lists drift
again, which is the property the refactor would have provided.

**Tests** (+1, 2437 → 2438 green). One table over every command family the chain
dispatches — `/rag`, `/reindex`, `/compact`, `/file`, `/project` (attach, status
and a `<slot>-cmd` line), `/image`, `/tts`, the registry, `/profile`, `/export`,
`/exit` — asserting for each that it is highlighted *and* that it never leaves as
a message; a malformed member of a family (`/project`, `/project nonsense`,
`/rag`) is still a command and not prose, since the box and `Enter` read it
through the same parser; and the near-words (`/projects`, `/project-attach`,
`how do I attach a project?`) stay plain text. The test was run against the
unfixed predicate first and fails there on `/project attach .` — a guard nobody
has watched fail is only a guard by assertion.

**A live model run is not required** (AGENTS.md §3): no engine, memory or tool
path is touched, and the change is a pure predicate over text the user typed.

### Post-M9: OSC 52 — the note that did not close the door (done)

- **Found live, in the host the feature was researched against.** A `/copy` in
  JupyterLab's terminal produced the honest note — *"sent to your terminal's
  clipboard (OSC 52). The terminal does not confirm it — if nothing pasted, your
  terminal may not support the sequence."* — and nothing was on the clipboard,
  exactly as the research said would happen there (xterm.js without
  `@xterm/addon-clipboard`, journal entry above). The behaviour was correct; the
  **message** was not, because it stopped at describing the dead end.
- **The fix is one clause, and the lesson is the fifth instance of §4.** Every
  message where the clipboard may not have delivered — the sent-to-terminal note
  and both too-large notes — now names `/export`, which writes the conversation
  to a file and depends on no terminal at all. JupyterLab is named outright: it
  is the one host known to drop the escape, and being told which terminal you
  are fighting is worth a clause. What makes this instance sharper than the
  earlier four is that the note was *carefully hedged and entirely true* — "may
  not support" is accurate, and useless. Hedging is not a route.
- **The irony worth recording**: `/export` was built one track later precisely
  for this host, and the note that most needed to point at it was written before
  it existed and never revisited. When a later feature answers an earlier
  feature's dead end, the earlier feature's messages are part of the change.

**Tests** (+1). A gate over every bundled locale: each of the three outcomes
where the clipboard may not have delivered names `/export`, with the ordinary
local copy as the control — it worked, so a route there would be noise on every
`F5`. The existing wording test's phrase assertion moved with the text.

**A live model run is not required** (AGENTS.md §3) — locale text and one test.
The live evidence is the screenshot that started this: the terminal that dropped
the escape now gets told what to do instead.

### Post-M9: the draft flush lags the intent — a spent command resurfaces in the box (done)

- **Found live, in the JupyterLab pass that opened the stage-3 track**
  (docs/history/commands-stage3.md): after a `/takeback` the box read
  `Please tell me about OpenAI./takeback` — the restored message with the spent
  command glued on. Deterministic under default settings, and one mechanism
  covered four commands. Every keystroke marks the draft dirty; the loop
  flushes it as `SetDraft` at the **top of the next iteration**, while a key's
  intent is dispatched at the **bottom of this one** (`handle_input_tick`). So
  the channel order was always `SetDraft("/takeback")` → the intent →
  `SetDraft("")`, and every handler reading `chat.draft` between the last two
  saw the spent command: `handle_delete_last` recorded it as the recovery
  draft and re-loaded it into the box via `activate()`'s `ChatActivated`
  (then `RestoreInput` prepended the deleted message — the screenshot);
  `handle_regenerate` re-loaded it the same way; `handle_clone` copied it
  into the clone; `handle_new_chat` left it as the old chat's saved draft
  forever, the late `SetDraft("")` landing on the newly active chat.
- **Why the popup hid it**: with `confirm_destructive_keys` on, the
  confirmation round trip gives the flush time to land — which is why the
  defect survived the stage-1 tests, whose confirmation case was the one that
  passed. The setting is off by default.
- **The fix is one seam**: `runtime`'s key and mouse paths flush the pending
  draft **before** dispatching the intent the screen returned
  (`flush_draft` in `input.rs`). The orchestrator then applies `SetDraft("")`
  first, and every reader above sees the truth; the same seam repairs the
  chord-path edge (keystrokes and `Ctrl+E` in one batch recorded a stale
  recovery draft) and puts a chat-switch draft on the chat it belongs to.
  Rejected alternative — clearing `chat.draft` inside `handle_delete_last` —
  fixes one reader and leaves `/regen`, `/clone` and `/new` wrong.
- **One adjacent nit**: `RestoreInput` prepended with no separator
  (`{text}{existing}`), so on the chord path the restored message fused with
  half-typed text mid-word. One space is inserted when neither boundary has
  its own; the existing trailing-space fixture stays byte-identical.

**Tests** (+2, and one extended). A runtime test drives `/takeback` through
`process_input_batch` key by key and pins the **order** — the empty `SetDraft`
must immediately precede `DeleteLastExchange` (re-ordering the flush after the
dispatch goes red); the `restore_input` test gains the no-boundary-whitespace
case. The registry-wide "clears the box and hands an empty draft back" test
already covered the screen half.

**A live model run is not required** (AGENTS.md §3) — pure UI/runtime routing.
The live evidence is the screenshot that started it.

### Post-M9: the restored message and the draft are separated by a blank line (done)

- **What.** Deleting the last exchange (`Ctrl+E` / `/takeback`) returns the
  user's message to the input box, prepending it to whatever is already
  typed. The glue was **one space** (the nit fixed in the entry above), which
  is right for a word boundary and wrong for what this actually is: two
  separate utterances. Typing a next question, then taking the previous
  exchange back, produced one run-on paragraph. The separator is now a **blank
  line** (`{restored}\n\n{draft}`) whenever the draft is non-empty.
- **Exactly one blank line, whatever the halves bring.** The restored text is
  `trim_end`-ed and the draft's leading newlines are dropped, so a message that
  ended in a newline or a draft that started with one still yields a single
  empty line between them — the joint is normalized rather than concatenated.
  A restored message that is blank (whitespace only) leaves the draft
  byte-identical; an empty box is still simply filled, untrimmed.
- **Where it lives.** `ChatScreen::restore_input`
  ([src/screens/chat/feed.rs](../../src/screens/chat/feed.rs)) — the one seam
  every `AppEvent::RestoreInput` goes through (delete-last, and the takeback
  paths that share it), so the orchestrator needed no change. spec §6.4 records
  the separator.

**Tests** (one extended). The `restore_input` fixture now pins the blank line on
both the trailing-space and the no-boundary-whitespace cases, and gains the
whitespace-only restored message that must leave the draft alone. 2548 unit
tests green.

**A live model run is not required** (AGENTS.md §3) — pure UI.

### Post-M9: the line-break hint names the chord the terminal can deliver (done)

- **Symptom** (reported from KDE Konsole): `Shift+Enter` in the input box does
  **nothing** — not a line break, and not the send that a "bare" terminal would
  give. **Cause**, in three parts, none of them in our key handler:
  1. Konsole's default keytab maps `Shift+Return` to `\EOM` — SS3 `M`, the
     keypad Enter (`data/keyboard-layouts/default.keytab`, `key Return+Shift`);
     unchanged in every release, and it is the keytab a new profile gets.
  2. crossterm 0.29's SS3 arm knows `A B C D H F` and `P–S` only; anything else
     returns `Err`, and `Parser::advance`'s `Err` branch does
     `self.buffer.clear()` — so the three bytes are **deleted**, and the app is
     handed no event whatsoever. That is why the symptom is silence rather than
     a stray `Esc` or a send.
  3. The kitty protocol, which would have made the keytab irrelevant, is not
     available: no released Konsole answers `CSI ? u`. `src/KittyKeyMap.h`,
     `handleKittyKeyboardQuery` and the profile property `KittyKeyboardEnabled`
     (default `true`) exist only on `master` — absent from every tag through
     `v26.04.0` — so `supports_keyboard_enhancement()` is correctly `false` and
     `runtime` pushes nothing. Konsole's kitty *graphics* and *notifications*,
     which it has had for years, are different protocols; that is what the
     wrong `konsole(22.12+)` line in
     [layout-independent-hotkeys.md](../research/layout-independent-hotkeys.md)
     §3.3 conflated (corrected there, with the evidence).
- **Measured, not assumed.** Four byte strings fed through a pty into
  crossterm 0.29: `ESC O M` → *no event*; `\r` → `Enter`; `ESC \r` → `Enter`+`ALT`
  (this is what Konsole sends for `Alt+Enter`: `Vt102Emulation::sendKeyEvent`
  prepends `\033` when no keytab entry claims the modifier); `ESC [ 13;2u` →
  `Enter`+`SHIFT`. So the app's existing `Alt+Enter` fallback (audit item 11)
  already works there — and a user who prefers the chord can point Konsole's
  keytab at `\E[13;2u`, which crossterm reads as `Shift+Enter`.
- **What changed.** Nothing in what the app *accepts* — both chords were, and
  remain, handled everywhere a line break is (chat, the settings editor, the
  self-model editor). What changed is what it **claims**: the input box's idle
  footer and the multiline editors' footer now interpolate
  `shared::keys::newline_chord()`, which answers `Shift+Enter` where a modified
  `Enter` is reportable and `Alt+Enter` where it is not. `app/runtime` sets the
  flag once, right where it decides whether to push `DISAMBIGUATE_ESCAPE_CODES`
  (and unconditionally `true` on Windows, where the Console API reports
  modifiers itself). Unset — tests, and any path with no terminal behind it —
  reads as `Shift+Enter`, which is why the committed demo dumps do not drift.
- **Why a global rather than a threaded parameter.** Same shape, and the same
  reason, as `theme::detected_background`: one immutable fact about the terminal
  the process is attached to, read from render paths that run every frame. The
  choice itself is a pure `chord_for(bool)` so it can be tested without touching
  the `OnceLock` — a test that `set`s it would re-label the UI for the whole
  test binary.
- **Deliberately left alone**: the in-feed search counter
  (`ui.chat.search.counter`) still names `Shift+Enter`, because it names `↑`
  beside it — the alternative is already on screen, and it works everywhere.

**Tests** (+2, `shared/keys`): the chord follows the flag in both directions,
and the default with the flag unset is the historical `Shift+Enter` (the
assertion that keeps the screenshot dumps and every other test stable). 2717
unit tests green on Linux — below CLAUDE.md's headline count, which is measured
on Windows where the `#[cfg(windows)]` tests also run.

**A live model run is not required** (AGENTS.md §3) — pure UI wording plus a
startup flag. The evidence that matters here is terminal-side, and both
directions were run: the app driven on a real pty that stays **silent** to
`CSI ? u` (Konsole's situation) draws `Alt+Enter newline` in the footer, and
the same binary on a pty that **answers** `\E[?1u\E[?62;c` draws
`Shift+Enter newline`. Plus the pty probe of crossterm's parser and Konsole's
own keytab, recorded above.

### Post-M9: a stress mark is part of the word (done)

<!-- cyrillic-ok:start — the entry is about checking Russian spelling: every
     Cyrillic run below is a word put through the dictionary or a segmenter's
     output. The prose is English. -->

- **Symptom** (reported with a screenshot, branch `fix/spellcheck-stress-marks`):
  `И́стинно так` typed into the input box, and `стинно` underlined while `И` is
  not. **Cause — one predicate.** `segment::words` grows a word while
  `chars[i].is_alphabetic()`, and `U+0301 COMBINING ACUTE ACCENT` — the sign
  every Russian source uses for stress, what `Alt+0769` types and what a paste
  from Wikipedia/Wiktionary carries — is category `Mn`, for which
  `char::is_alphabetic` is **false**. The word was cut in two at its stress and
  the halves looked up separately.
- **The check was wrong in both directions**, measured against the shipped
  `ru_RU`/`en_US` pair before touching anything:
  `чуде́сный ве́чер` → words `чуде` `сный` `ве` `чер`, three underlines under
  correct text; `О́зеро Байка́л` → a lone underline under `л`; and the quiet
  half — `за́мок` passed as `за` + `мок`, `хорошо́` as `хорошо` + nothing, so an
  error **disappeared** whenever both halves happened to be words. On
  `харашо́ напи́сано` the one genuine typo was underlined among two false ones.
  Not "noisy on stressed text": off for it.
- **Two predicates and a fallback**, no new dependency and no setting.
  `segment::is_mark` — the Combining Diacritical Marks block `U+0300–U+036F`
  and only it — makes a mark in-word (a word still *starts* at a letter, so an
  orphaned mark begins nothing); `segment::strip_marks` returns the word without
  its marks, or `None` when it has none, so an ordinary word allocates nothing;
  `check_word` looks the word up **as typed first**, then stripped.
- **Why the fallback is safe, and why the order.** The three bundled `.dic`
  files hold **zero** combining marks (counted), so a stripped lookup can only
  accept what the marked one rejected — never the reverse. Their accented
  entries are *precomposed*: `en_GB`'s `café` is `U+00E9`, one character, which
  `strip_marks` does not touch — and the as-typed-first order is what keeps that
  entry answering for its own word instead of `en_US`'s `cafe`. The mark is
  ignored, not the spelling under it: `харашо́` → `харашо` → still flagged, as
  the whole word, once.
- **`е`/`ё` needed nothing, and that is a measurement, not an assumption.** The
  `ru_RU` list carries 7347 stems spelled with `ё`; each one's `ё`→`е` spelling
  was put through the dictionary and **0 of 7347** were rejected. Meanwhile the
  reverse is *not* ignored — `афёра`, `опёка`, `гренадёр` are correctly flagged.
  So the data already does the one thing that helps and still catches the
  hypercorrections; a `ё`→`е` fallback of our own would only have broken the
  second half (user's decision, 2026-09-11, fork F5). One `ё` case *was* broken
  and this fixes it for free: `ё` typed decomposed (`е` + `U+0308`, as pasted
  from a macOS-authored source) was cut in two exactly like a stress, and the
  strip lands it on the `е` spelling, which by the count above is always
  accepted.
- **The rest of the input box already agreed.** `wrap::prev_boundary`/
  `next_boundary` move by grapheme cluster and `width_at` gives a mark 0
  columns, so `←`/`Backspace` already treated `И́` as one unit and the cursor
  already could not sit between a letter and its mark. The segmenter was the
  only layer that disagreed — nothing in rendering, wrapping or the popup's
  `replace_range` needed a line: a range that grew by a zero-width character
  still underlines and still replaces the right text.
- **Suggestions and the personal dictionary** follow from the same fact that no
  entry carries a mark: `suggest` asks about the unmarked word (a marked
  spelling has nothing to match against) and the replacement lands unstressed;
  `add_to_personal` stores the unmarked form, so adding `И́стинно` covers
  `Исти́нно` and every other placement, and the file stays a word list a person
  can edit (forks F3/F4).
- **Deliberately not done**: the spacing acute as a stress sign (measured:
  `мо´локо` splits today, `моˊлоко` with `U+02CA` stays whole and is rejected —
  both are rare conventions, and `´` is a quote elsewhere); precomposed Cyrillic
  grave (`всѐ` `U+0450`, `ѝли` `U+045D` — single characters the strip cannot
  reach); a Latin vowel inside a Cyrillic word (`мóлоко` — a homoglyph
  question); validating **where** the stress falls (`мол́око` is as correct as
  `мо́локо` — that needs an accentuated dictionary); and `ё`-restoration.

**Tests** (+11): `segment` — a marked word stays whole (`И́стинно так`,
`по-мо́ему`, a trailing `хорошо́`, a decomposed `ё` and a decomposed `café`), its
range covers the mark, an orphaned mark starts no word, `strip_marks` returns
`None` when there is nothing to strip and leaves a precomposed `café` alone;
`check` — the mark neither fails a word nor rescues a typo, a stressed word is
underlined whole or not at all, suggestions equal those for the unmarked word, a
personal word is stored stripped and every other placement is then correct;
`dict` — the same against the **shipped** `ru_RU`, beside
`the_bundled_dictionaries_load_and_answer` and for its reason (the rule rests on
a property of the data, so a dictionary swapped for a different upstream must
not be able to take it away quietly); `input_box` — a misspelling range keeps
the mark inside its underlined span, never orphaning it into one of its own;
`screens/chat` — the reported line typed into the chat input draws no underline
while `харашо́` still does. **3043 unit tests green, 156 `#[ignore]`**, clippy
`-D warnings`/fmt clean.

**A live model run is not required** (AGENTS.md §3) — spellcheck touches no
engine, no memory and no tool. The evidence is the measurement above, taken
against the real dictionaries with `spellbook` 0.4.2, plus the screen-level test
that reproduces the reported line end to end.

Design and every measurement:
[docs/research/spellcheck-stress-marks.md](../research/spellcheck-stress-marks.md).

<!-- cyrillic-ok:end -->
