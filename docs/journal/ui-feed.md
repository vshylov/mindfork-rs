# Journal — UI: feed, rendering and the terminal

The message feed and everything drawn into it: the markdown renderer, syntax highlighting, Mermaid, tool cards, the status bar, themes and compatibility mode, plus the terminal-level fights (wide glyphs, redraw, synchronized output).

**Reference documents for this area:** architecture.md §10, spec.md §11.3–§11.4

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)). They record what was done,
why, what was measured and what was rejected — the reasoning behind the code, not
its current shape. For the current shape read the reference documents named above;
for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (30)

- Post-M9: mouse-wheel feed scrolling (done)
- Post-M9: own markdown renderer (tables + LaTeX + theme) (done)
- Post-M9: repaint-on-change (cursor-blink fix) (done)
- Post-M9: inline tool blocks in the feed + call-text wrapping (done)
- Post-M9: a token counter in the status bar during generation (done)
- Post-M9: TUI redesign (palette, role rails, pills, "keycaps") (done)
- Post-M9: status bar — pill top-left, hotkey grid right (done)
- Post-M9: role-rail color independent of line style (done)
- Post-M9: dim background under popups (done)
- Post-M9: configurable conversation-copy composition (`F5`) (done)
- Post-M9: scrolling with VS16 emoji without flicker (done)
- Post-M9: compact status chips for all servers in the status bar (done)
- Post-M9: scrollbars (feed, input, settings, help, chat list) (done)
- Post-M9: compatibility mode for old terminals (done)
- Post-M9: improved tool-call view (code highlighting, console, compact header) (done)
- Post-M9: markdown-render refinements (LaTeX + writer + feed cache) (done)
- Post-M9: synchronized output (DEC 2026) — fixing the "jumping cursor" (done)
- Post-M9: horizontal row separators for Markdown tables (done)
- Post-M9: rendering Mermaid diagrams in the feed (done)
- Post-M9: Mermaid — source until the closing fence (fix for streaming flicker) (done)
- Post-M9: emoji popup — a "hanging" selection ghost after closing (done)
- Post-M9: ratatui-core/crossterm update 0.1.1 → 0.1.2 (done)
- Post-M9: a full redraw on screen switch and in the input box with VS16 (done)
- Post-M9: in-feed text search (`Ctrl+F`) (done)
- Post-M9: raw HTML blocks render their text (and S3(c) measured, then rejected) (done)
- Post-M9: an unhighlighted code block is drawn as a rectangle (done)
- Post-M9: Zig code blocks are highlighted (done)
- Post-M9: vendored syntax grammars for 19 languages (done)
- Post-M9: collapsible tool calls, and the collapse state per chat (done)
- Post-M9: navigable `chat://` references in the feed (done)

### Post-M9: mouse-wheel feed scrolling (done)
- **The mouse wheel scrolls the feed** on par with `PageUp/PageDown`. `ratatui::init()`
  doesn't enable mouse capture, so crossterm wasn't delivering wheel events — that's the
  crux of the fix. `runtime.rs` handles `Event::Mouse`, `ChatScreen::handle_mouse` maps
  `ScrollUp/ScrollDown` → `MessageFeed::scroll_up/scroll_down` (`WHEEL_SCROLL=3`
  lines); a no-op when an overlay/popup/help is open.
- **Mouse capture is a toggle, `Ctrl+W`** (**off** by default). Reason: the wheel and
  native text selection share the same terminal mouse-reporting mechanism —
  "wheel only, leave selection alone" can't be split out technically. Off → native
  mouse selection works; on → the wheel scrolls the feed, selection stays
  available with `Shift` held (Windows Terminal and most terminals support that).
  The toggle: `ChatIntent::SetMouseCapture(bool)` → `runtime::dispatch` sends
  `Enable/DisableMouseCapture` (a purely terminal side effect; the screen doesn't know
  about the terminal, per FSD). Capture is **always** released on exit and in the
  panic hook (the terminal doesn't stay in mouse mode after a panic). `Ctrl+M` is
  unusable for the toggle — the terminal reports it as `Enter`; `Ctrl+W` was chosen
  instead (W=wheel), layout-independent (`shared/keys`).
- The current mode is shown in the **status bar** (`mouse: scroll/select (Ctrl+W)`);
  added to the help overlay (`F1`/`?`). In "scroll" mode **the word "scroll"**
  is highlighted with the `accent` color (like markdown headings in the feed) —
  `Palette::hint_highlight_value` colors the value after the colon (the label with `:`
  stays dim; split by `:`, not by space — correct for
  multi-word values under localization); in "select" mode the whole description is
  dim.

### Post-M9: own markdown renderer (tables + LaTeX + theme) (done)
- **`shared/markdown.rs` was rewritten** from `tui-markdown` to an own walker over
  `pulldown-cmark` 0.13 events → [ADR 0003](../../docs/decisions/0003-own-markdown-renderer.md).
  Reason: `tui-markdown` didn't support tables and math and ignored the theme.
  Signature: `render(input, width, palette) -> Text<'static>` (`message_feed`
  passes the panel width and palette). The `tui-markdown` dependency is removed;
  direct `pulldown-cmark`, `syntect`, `ansi-to-tui` are added.
- **Theme**: colors (headings/links/list markers) come from `Palette` (previously the
  feed ignored dark/light). Code-block highlighting — `syntect` + `ansi-to-tui`,
  **the syntect theme is built from `Palette`** (`build_code_theme`): scopes → roles
  (keyword→accent, string→success, number→warning, function→user, type→assistant),
  text/comments — gray based on background lightness (the `Palette.dark` flag: Auto/Dark→dark,
  Light→light); named ANSI colors are converted to RGB (Campbell). Themes are cached by
  palette (`Box::leak` — there are only so many palettes, `HighlightLines<'static>`). It used
  to be hardcoded to `base16-ocean.dark`, disconnected from the theme (closes an ADR 0003
  gap).
- **Delimiter-scoped LaTeX** (modeled on the .NET `LaTeXConverter`): `normalize_delimiters`
  converts `\(…\)`→`$…$`, `\[…\]`→`$$…$$` (skipping code spans/blocks), a parser with
  `ENABLE_MATH` yields `InlineMath`/`DisplayMath`, and only their content goes through
  `latex_to_unicode` — the parser strips the dollar signs (no "$→$"), no false positives
  in prose/code. **Behavior change:** "bare" commands outside `$…$` (`\alpha`, `x^2`)
  are no longer converted. The converter is extended: `\frac{a}{b}`→`a/b`, `\sqrt{x}`→
  `√(x)`, `\pmod{n}`→`(mod n)`, text/font wrappers and accents
  (`\text/\mathrm/\mathbb/\vec/\hat/\overline/…`) → their content, operator-name
  functions (`\log/\sin/\cos/\lim/\max/\gcd/…`) → the word without `\` (the main
  gap fixed — `\log` in "O(n \log n)"), size modifiers for delimiters
  (`\left/\right/\big/\Big/…`) are stripped, spacing commands, `\{ \}` protection, dropping
  grouping braces, dozens of symbols.
- **Tables** (`ENABLE_TABLES`): cells accumulate in `TableBuilder`; on close —
  a box-drawing layout fit to the panel width. Column widths are "water-fill"
  (`fit_columns`): narrow ones keep their natural width, the rest split the remainder
  evenly with a readable minimum floor (`MIN_COL`/`MAX_MIN`). Cell content is word-wrapped
  (reuses `shared::wrap`), row height = the max rows among
  cells, alignment from markup, header in bold. If the minimums don't fit —
  natural width + horizontal clipping with "…". A table is guaranteed to be ≤ the width
  → a second wrap pass in `message_feed` is safe. Horizontal scroll instead of clipping — future work.

### Post-M9: repaint-on-change (cursor-blink fix) (done)
- **The `app/runtime.rs` loop only paints on change** (a `dirty` flag), not every tick.
  Previously `terminal.draw` was called unconditionally every iteration (~20/s,
  `TICK=50ms` period): whenever the input box was focused, every frame called
  `frame.set_cursor_position`, and `ratatui` after `draw` always sends "show +
  move cursor" even with an empty buffer diff. Windows Terminal resets the cursor's
  blink phase on every move → the cursor blinked more often and unevenly (CPU stayed
  ~0% since the diff was empty). Now `dirty` is raised on: an applied orchestrator
  event, a terminal event (input/mouse/**resize** — previously hidden by the
  unconditional repaint, now handled explicitly), a dictionary reload,
  a spellcheck-highlight recompute. There are no timer-driven animations in rendering, so
  idle ticks don't need to repaint.
- **The debounced spellcheck recheck was moved into the loop.** `maybe_recheck_spelling`
  (`screens/chat.rs`) — a deferred, timer-based action (300ms debounce): previously it
  was only invoked by `render`, and `render` ran every tick. With repaint-on-change,
  once the last keypress's events are gone → highlighting never appeared (visible
  only if dictionaries exist). Fix: the loop body still runs every tick anyway
  (the `poll` timeout), so it's the loop that calls `maybe_recheck_spelling` (now `pub`,
  returns `bool` — whether it recomputed), raising `dirty` **only** when the highlighting
  actually changed. So the loop, not render ticks, ensures the debounce wakeup, and there
  are no extra frames (with cursor repositioning) while idle.

### Post-M9: inline tool blocks in the feed + call-text wrapping (done)
- **Tool blocks are drawn at the call site, not in a "header"** (`widgets/message_feed.rs`):
  `FeedToolCall` got a `text_offset` field (a byte offset into `FeedMessage::
  text` — how much of the reply text had been generated BEFORE the call). `build_lines`
  for the assistant (`push_assistant_body`) splits the text by calls' `text_offset` into
  markdown fragments and inserts the tool block between them: `text-before-the-call → 🔧 block →
  text-after`. Previously `push_tools` ran BEFORE the body — all calls hung above the reply.
- **Call arguments and results are wrapped, not truncated**: `truncate`/
  `.take(6)` removed; `push_tool` via a new `push_wrapped` (gutter prefixes `  🔧 `/`  │ `,
  wrapping to visual width via `shared::wrap`, continuation alignment) lays out long
  `arguments`/`result` across several rows in full. The feed already does a second
  `wrap::wrap_line` pass, but lines are already ≤ the width → a no-op.
- **Agentic-loop round stitching** (`FeedMessage::from_messages`): on chat reload,
  consecutive round assistant messages (with the in-between tool
  messages in history dropped) merge into one "Assistant:" block with inline calls;
  each round's `text_offset` is shifted by the accumulated length.
  `screens/chat.rs::push_tool_call` sets `text_offset = last.text.len()`;
  `activate_chat` builds the feed via `from_messages`.
- **The live stream matches a reload byte-for-byte**: round texts in live mode used
  to be concatenated with no separator, while `from_messages` inserts `\n\n`
  (thoughts — `\n`). Flags `pending_text_sep`/`pending_thoughts_sep` on `ChatScreen`
  are set when a tool is called and consumed by the very first text/thoughts chunk of
  the next round, inserting the same separator (if the accumulated text is non-empty).
  Reset in `begin_generation`. Covered by a test
  `live_stream_with_tool_matches_reload` (live == `from_messages`).

### Post-M9: a token counter in the status bar during generation (done)
- **A combined counter** (`tokens: 1290`): the whole conversation's (prompt) tokens
  **plus** the current/last reply's tokens, as one number. Visible **right at
  the start** (`~1234`, reply still 0), not "counting up from 1"; during
  generation the number grows live (bright, accent), afterward — dim (the turn's
  final result) until the next generation.
- **The conversation is an `~` estimate, then an exact number**. The server only
  sends the exact `prompt_tokens` at the end of a turn (with `usage`), so until
  then a client-side **estimate** is shown
  (`shared/tokens.rs::estimate_prompt`, a "UTF-8 bytes / 4" heuristic: Latin
  ≈4 chars/token, Cyrillic ≈2 — close to Gemma/Qwen BPE) marked with a `~`.
  `start_generation` emits the estimate right after `GenerationStarted`
  (`TokenUsage{completion:0, context:Some(est), context_exact:false}`); when
  `usage` arrives it's replaced with the exact `prompt_tokens` (`context_exact:true`, the
  `~` is removed).
- **The reply has two sources**: (1) a live approximation from the count of streamed
  deltas (`Text`/`Thoughts`) — with llama-server one delta ≈ one token, works with any
  server; (2) the exact `completion_tokens` from `usage`. The request asks for usage via
  a new `stream_options.include_usage=true` field (`wire.rs`, only while streaming);
  `ChatCompletionChunk.usage` → `Usage{prompt_tokens,completion_tokens}` → the client
  emits `ChatChunk::Usage(TokenUsage)` (a new enum variant) **before** parsing
  `choices` (a usage chunk has empty `choices`) and before `Finished`.
- **Flow**: `stream_round` (`orchestrator/generation.rs`) sends
  `AppEvent::TokenUsage{generation_id, completion, context:None, ..}` on every delta with
  an accumulated count `base_tokens + streamed` (doesn't touch the conversation —
  `context:None` keeps the estimate); on `ChatChunk::Usage` — the exact `completion`+`context`.
  The reply counter **accumulates across agentic-loop rounds** (`total_tokens` +=
  `RoundOutput.tokens`, where `tokens` = usage if present, otherwise the delta count).
  `runtime.rs` → `ChatScreen::set_token_usage` (gated by `generation_id`; the context is
  updated only when `Some`); fields `gen_tokens`/`gen_context`/`gen_context_exact` are
  reset in `begin_generation`. `status_bar::render` draws `tokens: [~]<conversation+reply>`
  as a single number (`~` while the conversation figure is an estimate); hidden when
  `tokens==0 && context==None`.
- An external/strict OpenAI server will either support `stream_options` (exact count)
  or ignore it (leaving the conversation estimate and the live reply approximation) —
  graceful degradation.

### Post-M9: TUI redesign (palette, role rails, pills, "keycaps") (done)
- **Goal**: dramatically refresh the interface per a finished mockup
  (`docs/redesign/TUI Redesign.dc.html`). Style: calm **rounded** borders,
  colored message **role rails**, status "pills", a quiet hotkey line with
  "keycap"-styled labels. Translating the mockup into ratatui — not pixel-perfect, but
  per its own "RATATUI IMPLEMENTATION" note (Block + rounded borders, `▌` rails,
  `Style::fg`, reversed keycap spans — all in 256 colors).
- **`shared/theme.rs` — the palette extended** with structural roles on top of the previous
  ones (`user/assistant/tool/success/warning/error/accent`): `*_soft` (light variants
  of role headers), `text`, `muted`, `border`/`border_focus`, `keycap_fg`/
  `keycap_bg`. Exact shades of the **dark** theme — taken from the mockup (oklch → sRGB, computed
  during development). **Auto** (default) — named ANSI (adapts to the
  terminal) + neutral grays for structure; **Light** — darkened. Helpers:
  `panel(title, focused)` (a rounded `Block` with a title), `keycap(label)`,
  `hint(key, desc)`, `pill(label, color)`, `border_style(focused)`, `muted_style`.
  (Unused `warning_style`/`error_style` removed.) The palette is still `Hash`
  (the syntect-theme cache key in `markdown.rs`).
- **`widgets/message_feed.rs`**: every message gets a **colored gutter rail**
  `▌` by role (content is built at width `width − 2`, wrapped, the rail is prepended
  to each visual row → `prepend_rail`); role headers in caps with an icon
  (`✦ ASSISTANT` / `❯ YOU`, `role_header`); "thoughts" collapse into a **pill**
  (`▸ thoughts · N para. · [Ctrl+T]`); a tool card — `⚒  name(args)` (two spaces
  after the emoji — it's 2 columns wide) + the result on the `└` gutter. Feed title: on the left
  `◆ <chat>`, on the right meta `model · Nk ctx` (`ChatScreen::model_meta` from the settings
  snapshot).
- **`widgets/input_box.rs`**: a rounded border + a focus color, a `❯` prompt
  column on the left (width `PROMPT_W`; the inner input area is shifted right, the test helper
  `render_at` accounts for this).
- **`widgets/status_bar.rs`**: server status shown as a **pill** (`● server: …` colored by
  status), separators `│`, hotkeys via `keycap`+`hint`. Substrings `ready`/
  `generating`/`tokens:`/`mouse:` kept (tests).
- **`screens/settings.rs`**: the main panel and hints — via `panel`/`keycap`;
  the section menu with a rail on the active section; field values colored by type
  (`render_field_line` takes the palette: a toggle — green/muted, a choice —
  blue, a dash — border color).
- **`widgets/chat_list.rs`**: a fullscreen rounded panel "▤ Chats" (+count on the
  right); **a search line inside a border** with a "`/` key" on the right; list rows — a dot
  (green for the active one), title and count, **right-aligned**
  (truncation with `…`, `truncate_to_width`), selection — a soft backdrop + a green rail on
  the selected row; a **hotkey status line at the bottom** laid out as a **neat grid**
  (`status_lines`: max columns to fit the width, columns line up vertically;
  `Del` — a red "key"). **Rename (F2)** — a separate input field with a
  **real cursor** (`render_rename` + `set_cursor_position`, horizontal
  scroll; `Mode::Rename` holds a `buffer: Vec<char>` + `cursor`, supports
  `←/→`/`Home`/`End`/`Backspace`/`Delete`/mid-string insert), the search line is hidden while
  editing. `profile_list.rs`/`impersonation_preview.rs`, the help and
  spellcheck overlays — also on rounded panels.
- **Width-2 emoji glyphs** (`⚒`/`⚙`/`⌨`) `unicode-width` counts as 1 → the following
  space got overwritten; after them we add **two** spaces (otherwise they merge with the text).
- The default theme stayed **Auto** (the redesign's structure is visible everywhere; the mockup's exact
  shades — the **dark** theme in settings). Unit tests updated to match the new
  look (finding tool blocks by name, rail-aware empty lines, `↑/↓` navigation accounting for the
  prompt column). Gate green.

### Post-M9: status bar — pill top-left, hotkey grid right (done)
- **Symptom**: on wrap, hotkeys went into a separate grid **under** the status line, and
  "`Ctrl+W  mouse: selection`" landed right under "`● server: ready`" — the two left
  columns merged and visually competed.
- **Fix** (`widgets/status_bar.rs`, `lines`/`grid_layout`/`right_grid`): the status
  pill (server + generation + token counter) stays on the left on the **top** line,
  and hotkeys are laid out as a **neat grid, right-aligned**, and **share
  the top line** with the pill. When everything fits — one line: pill on the left, hotkeys
  on the right (with a visible gap between them). When it doesn't fit — hotkeys wrap DOWN
  as a grid **whose columns line up vertically** (as in the chat list window), and the
  partial (wrapped) row is right-aligned — its keys land **exactly under
  the columns of the row above** (e.g. `Ctrl+C exit` exactly under `Ctrl+P settings`, padded
  out to the column width), so the left/middle part of the window is empty and doesn't draw attention.
- **Layout** (`grid_layout`): cells are filled row-by-row, left-to-right/top-to-
  bottom, but the incomplete bottom row takes the **rightmost** columns (`empty_lead`
  empty columns at the start); column widths — the max over the cells actually placed in
  that column (including the wrapped one), the total block width + leading indent =
  the full width → the block hugs the right. **Choosing the number of columns**: the max number of
  columns (→ the fewest rows) at which the pill can share the top line with the
  block (`state_w + GAP + block_width(cols) ≤ width`). A narrow fallback (even one column doesn't fit next to
  the pill): the pill on its own top line, the grid right-aligned below it.
  `height()`/`render()` and their call sites in `screens/chat.rs` unchanged (same signature).
  The old inline path and the left-side grid (`palette.hotkey_grid`) in the status bar are no longer
  used (the palette's grid helper remains for the chat-list overlay). Tests: a single
  line with edges flush; wrapping with the pill on the top line and `Ctrl+C` exactly under the
  `Ctrl+P` column (rendered in `TestBackend`, comparing positions in characters).

### Post-M9: role-rail color independent of line style (done)
- **Symptom**: a message's vertical role rail (`▌`) on the left looked a different
  color (dimmed) next to divider lines `───` and horizontal table
  borders.
- **Cause**: `markdown` sets `.add_modifier(Modifier::DIM)` on these lines at the
  **line level** (`line.style`, see `shared/markdown.rs` `rule`/`border_line`), and
  `message_feed::prepend_rail` copied `line.style` onto the whole output line, including
  the rail span. The rail span only overrode `fg` (not modifiers), so the
  line-level `DIM` leaked onto it too.
- **Fix** (`widgets/message_feed.rs::prepend_rail`): the line-level style is now
  **folded into the content spans** (`line.style.patch(span.style)`), while `out.style`
  is reset to the default. Content looks identical to before (same resulting
  span styles), but the rail is now its own span with its single `fg` and
  no longer inherits any line-level modifiers (a general fix — not only for
  the current `DIM`). Regression test `rail_is_not_dimmed_next_to_table_borders`.

### Post-M9: dim background under popups (done)
- **Symptom**: popups over the screen only cleared their own area (`Clear`), and the background
  behind them stayed at full brightness and visually blended with the popup, hurting
  readability.
- **Fix**: a helper `shared/ui.rs::dim_background(frame)` applies `Modifier::DIM`
  to **all** cells of the screen buffer; called **before** `Clear`+rendering the popup.
  `Clear` then resets the popup's cells to the default (undimmed) style — so
  only the background gets dimmed, while the popup itself stays bright. The `DIM` effect is
  terminal-dependent (Windows Terminal supports it). Lives in `shared` (FSD: `screens → shared`) so it can
  be reused across screens.
- **Where applied**: the help overlay (`F1`/`?`) and the spellcheck-suggestion popup (`Ctrl+G`)
  in `screens/chat.rs`; the large multi-line editor for a profile's system message/greeting
  in `screens/settings.rs` (only `editor.multiline` — compact single-line
  field-edit strips are edited in place and don't dim the background). Fullscreen
  overlays (profile picker, chat list, settings) don't dim the background — they already
  occupy the whole screen.

### Post-M9: configurable conversation-copy composition (`F5`) (done)
- **Copying a chat's conversation to the clipboard (`F5`) became configurable**: a new
  section `AppConfig.copy: CopySettings` (`copy_thoughts`/`copy_tool_calls`/`copy_tool_results`,
  `#[serde(default)]` → old `settings.json` without migration; all flags off
  **by default** — the previous "message text only" behavior). By choice,
  "thoughts" (CoT, before the text), tool-call parameters (name + arguments)
  and their results are added to the assistant block.
- **`features/chat_export.rs::format_conversation` takes `&CopySettings`**:
  the optional blocks are drawn from `Message.tool_calls` (name/`arguments`/`result`) —
  separate tool messages are still skipped (avoiding duplication). Tool block:
  a `[Tool: name]` header (printed when either of the two flags is on) +
  `Arguments: {json}` / `Result: …`. "Thoughts" — a `[Thoughts]` block. An assistant
  message with no text but with tool calls is included if the corresponding option
  is enabled (otherwise skipped, as before). Pure logic — the orchestrator
  (`chats.rs::handle_copy_chat`) passes `&self.config.copy`.
- **UI**: three toggles in the "Interface" section of settings (`FieldId::ICopyThoughts`/
  `ICopyToolCalls`/`ICopyToolResults`) with description tooltips (`field_description`).
- **Tests**: chat_export (by default text only; "thoughts" before the text; parameters/
  results under a shared header; a message made only of tool calls); config (defaults
  off). **631 tests green**, clippy/fmt clean.

### Post-M9: scrolling with VS16 emoji without flicker (done)
- **A full feed redraw when scrolling with "drifting" VS16 emoji (`🕸️`/`🗂️`)
  no longer flickers.** Previously `app/runtime.rs` called `terminal.clear()`, which sends
  a screen-clear escape `ESC[2J` — the screen blanks for a moment (flicker). The point of the redraw is to
  wipe "hanging" artifacts: conhost/Command Prompt draw a VS16 cluster wider
  than ratatui's model, content drifts, and the per-cell diff doesn't reach the drifted character.
  Every cell needs to be explicitly rewritten, including spaces in empty spots.
- **A dead end (important)**: "lightening" `clear()` down to a plain back-buffer reset
  (`swap_buffers`) is NOT ALLOWED — then the diff compares "empty → frame" and **skips
  space cells** (they equal the empty back buffer): empty spots don't get
  redrawn, the old content/artifact stays visible (visually, a "mush" of
  overlaid text). The diff only emits cells that DIFFER.
- **Fix**: make the back buffer differ from **any** real cell. We fill
  the current buffer with a sentinel character `"\0"` (`current_buffer_mut().content`; such
  a character never occurs in real content) and move it into the back buffer via
  `swap_buffers()` — **without** flushing to the screen (flush isn't called). Then the next `draw`
  diffs "`\0` → the real frame": every cell differs (spaces too) → ratatui
  rewrites the whole screen cell-by-cell, **without** `ESC[2J` (no flicker) and with spaces in
  empty spots. The internal swap inside `draw` restores the invariant "back buffer =
  screen"; the `"\0"` itself never reaches the screen. The trigger is unchanged (`take_feed_scrolled`:
  VS16 is present and a scroll happened). The intra-line drift caused by a wide emoji is inherent to the
  terminal (the same as with the old `clear()`), not a regression. Real conhost behavior
  isn't caught by unit tests (TestBackend) — verified on a live terminal.

### Post-M9: compact status chips for all servers in the status bar (done)
- **The status bar showed one server** (chat) as a single pill `● server: ready`,
  whereas there are now several servers: chat, **embeddings**, **impersonation**. Now —
  a cluster of compact **chips** (glyph + label), one per active server.
- **Data model**: a new snapshot `ServerStatuses { chat, embed, impersonation }`
  (`shared/server.rs`); the event `AppEvent::ServerStatus` now carries it instead of a single
  `ServerStatus`. The orchestrator emits the snapshot on **any** status change:
  the chat background probe (`status_rx`), the impersonation probe (`imp_status_rx` — previously its
  status never reached the UI at all), and settings changes (`apply_chat/embed/impersonation_
  settings` via the `emit_server_status` helper). `EngineManager` got
  `embed_status` + a `statuses()` method; `apply_chat` no longer returns a status
  (it's read via `statuses()`).
- **The embeddings status wasn't tracked before** (`apply_embed` is lazy, no probe):
  `EmbedSetup` got a `status` field — `Ready` for a real embedder,
  `NotConfigured` for `UnavailableEmbedder`. There's no embeddings probe → the chip is binary
  (configured/hidden); a real probe is groundwork.
- **Render** (`widgets/status_bar.rs`): a glyph encodes the status — `●` ready (success),
  `◐` connecting (warning), `✕` no connection (error) / not configured (warning). **Chat
  is always shown** and, on disconnect, carries the reason (`✕ chat: no connection: <why>`) — it
  blocks generation; **embeddings/impersonation** — chips `emb`/`imp` shown **only
  when configured** (`NotConfigured`, including impersonation in `shared` mode → the chip hidden),
  without the reason text (compact). Glyphs are 1 column wide (no emoji) — the layout of the
  status line/hotkey grid doesn't "shift". The `Palette::pill` helper was removed (its role now
  belongs to the local `chat_chip`/`secondary_chip`/`chip` with different glyphs).
- **Tests**: chips are visible only for configured servers (impersonation in `shared` mode/
  embeddings off → hidden); glyph color follows status; earlier layout/token/
  mouse tests were ported to `ServerStatuses` (the grid-wrap width was tuned for the shorter
  chip). **671 tests green**, clippy/fmt clean.

### Post-M9: scrollbars (feed, input, settings, help, chat list) (done)
- **Shared helper `shared/ui.rs::render_scrollbar`**: a vertical scrollbar
  (ratatui `Scrollbar`) in the right-hand column of the given area; **a no-op
  when the content fits** (`total ≤ viewport`) — it stays quiet on short
  content. Drawn **over the panel's right border line**
  (`area.inner(Margin::new(0, 1))` — corners intact), doesn't take width away
  from the content → line wrap/scroll math is unchanged. The track uses the
  border color (the `focused` parameter says which color the border under the
  bar was drawn in: normal/focused), the thumb `█` uses the `text` color
  (readable on both). **A ratatui subtlety**: `ScrollbarState::content_length`
  is the number of **scroll positions** (`total − viewport + 1`), not rows;
  with `content_length = total` the thumb wouldn't reach the bottom at full
  scroll (see the comment in the helper).
- **Chat feed** (`widgets/message_feed.rs`): the bar sits on the right border
  when content overflows, position — `self.scroll` (clamping unchanged).
- **Input box** (`widgets/input_box.rs`, multiline mode): the bar appears when
  `vrows > visible_rows` (the field has hit its height cap and is scrolling);
  single-line mode is unaffected (it scrolls horizontally there).
- **Settings screen** (`screens/settings.rs::render_fields`): the bar sits over
  the screen's right border (column `list_area.right()` — the border line:
  `fields_area` reaches right up to the inner panel), below the section title
  line; position — the actual `ListState::offset()` after the list is
  rendered.
- **Help popup** (`F1`/`?`, `screens/chat.rs`): the key list now **scrolls** on
  a short terminal — `↑↓`/`PgUp`/`PgDn` scroll it (`ChatScreen.help_scroll`,
  clamped in `render_help` — the popup's height is only known there); any
  other key closes it (as before). `List` was replaced with a `Paragraph`
  using `scroll`; the bar sits on the popup's right border; a caption at the
  bottom when content overflows — "↑↓ scroll · Esc — close".
- **Chat list** (`widgets/chat_list.rs`): the bar sits over the right border of
  the "▤ Chats" panel on the list rows (the search line and hotkey grid are
  unaffected); position — the list's offset after rendering.
- **Tests**: the helper (the bar only appears on overflow; the thumb sits at
  the top initially and at the bottom on a full scroll; a zero-size area is a
  no-op); one test per feed/input/settings/chat list (the "█" thumb in the
  border column appears only on overflow); help (arrows scroll and don't
  close, reopening resets the scroll, an "overscroll" is clamped on render,
  the thumb appears on a short terminal). **744 tests green**, clippy/fmt
  clean.

### Post-M9: compatibility mode for old terminals (done)
- **Toggle `interface.terminal_compat`** (settings "Interface" section,
  `FieldId::ICompat`, **off** by default; `#[serde(default)]` → old
  `settings.json` files without migration): older emulators (Windows 10
  conhost, etc.) render emoji and rare Unicode characters as "tofu" boxes and
  ignore `DIM` — the mode switches the UI over to a safe glyph set. See
  spec §11.6.
- **`GlyphSet`** (`shared/theme.rs`): all decorative UI glyphs are gathered
  into one struct with two statics — `UNICODE_GLYPHS` (the previous redesign
  look) and `COMPAT_GLYPHS`. **The compat set targets WGL4** (Windows fonts'
  base repertoire: Consolas/Lucida Console) plus ASCII: `✦→*`, `❯→>` (role
  headers, the input prompt), `⚒→#` (the tool card; the prefix/continuation
  count-width matches in both sets), `▸/▾→►/▼` (the "thoughts" pill,
  "Sections"), `◆→♦` (titles), `▤→≡` (chat list), `⚙/⌨→#` (settings/help
  panels), `✓/✗/⚠→√/×/!` (operation statuses, `F3` goals), `◐/✕→○/×` (server
  chips; **the ready glyph `●` is not replaced** — it's already in WGL4),
  `⟳→»` (generation), `✻→*` (background task), `⌕/▏→?/│` (the search bar),
  `➕→+` (the spellcheck popup), the Braille spinner → ASCII `|/-\` (the RAG
  banner, impersonation), rounded borders (`BorderType::Rounded`, arc segments
  `╭╮╰╯`) → straight ones. WGL4-safe glyphs (`▌` rails, `│`/`└` gutters,
  markdown table box-drawing, the `█` scrollbar, `…`, arrows, `‹›`, `·`, `☺`)
  are deliberately left alone. Emoji in message **content** (and the `Ctrl+B`
  popup) aren't replaced — that's data, not chrome.
- **Wired through the palette** (minimal ripple): `Palette` gained a
  `compat: bool` field (+ builder `with_compat`, method `glyphs() ->
  &'static GlyphSet`) — the palette is already threaded through every render
  function, like the `dark` flag. `panel()` picks the border type from the
  set. Widgets/screens read glyphs from `palette.glyphs()`; spinner
  duplication (consts in `chat.rs`/`impersonation_preview.rs`) is gone —
  frames now live in `GlyphSet.spinner`. Rebuilding the palette with the flag
  — `ChatScreen::set_settings`, `SettingsScreen::palette()` (a helper),
  `runtime::apply_event` (for an open chat list/`F3`) — applied on the fly
  from the `Settings` event.
- **`dim_background(frame, palette)`** (`shared/ui.rs`): conhost doesn't
  support SGR `DIM`, so in compat mode the background under popups is dimmed
  **by color** — the fg of every cell → `palette.muted` + `BOLD` removed (in
  a 16-color mapping it would give a "bright" variant and undo the dimming);
  the normal mode keeps the previous `DIM`.
- **Tests**: theme (sets switch via the flag; the compat set has no glyphs
  needing replacement, the spinner is ASCII; tool-prefix count-widths match,
  the input prompt is 2 columns); ui (compat dimming colors the fg instead of
  DIM); message_feed (compat feed without emoji: `* ASSISTANT`/`> YOU`/
  `# note_save`/`► thoughts`); status_bar (compat chips `○/×`, `» generating`,
  `* reflection`, `●` stays); settings (the toggle in the "Interface" section,
  saving + the working-copy palette, the field description); config (default
  off, roundtrip). **751 tests green** (+7), clippy/fmt clean.
- **Groundwork**: auto-detecting an old terminal on first launch (a heuristic
  over `WT_SESSION`/`TERM_PROGRAM` on Windows) — currently manual toggling
  only.

### Post-M9: improved tool-call view (code highlighting, console, compact header) (done)
- **Tool cards in the feed used to show raw JSON**: the header `⚒ name({"code":"...\n..."})`
  (multi-line Python code collapsed into a JSON string with `\n` escapes), the result —
  as flat `muted` text. Now arguments/results render meaningfully, with
  code highlighting and colored console output for `python_exec`.
- **A clean presenter** `features/tools/present.rs` (no ratatui, testable):
  `present(name, arguments, result) → ToolPresentation` with a `header_suffix` + `ToolBlock`
  blocks (`Code{lang,text}`/`Console{stdout,stderr,exit}`/`Markdown`/`Plain`).
  Knowledge about tools lives in the tools layer; the `message_feed` widget stays generic and
  just renders the blocks. `arguments` is parsed from a JSON string; on failure — graceful
  degradation to the previous inline view. Rules: `python_exec` → code as a `python`
  block + a console; `fs_write`/`fs_read` → content highlighted by the extension of
  `path`; a large text field (multiline/>100 chars) → a separate block, short scalars → the header
  (`name(value)` / `name(k=v, …)`, truncated to 100 chars); prose tools
  (`web_search`/`fetch_url`/`rag_search`/`note_recall`) → a markdown result. Tool
  names are string literals (a stable wire protocol).
- **A new helper `shared::markdown::highlight_code(code, lang, palette)`** — syntect
  highlighting of a code block **without the enclosing ` ``` `** and without wrapping, using the
  same theme-consistent palette (`build_code_theme`) as fenced blocks; an unrecognized
  language (`resolve_syntax` missed) → lines colored `text` (not reversed — more readable
  for non-strictly-code arguments); a trailing empty line is stripped.
- **Card rendering** (`message_feed::push_tool`/`push_block`/`push_console`/
  `push_gutter_lines`): `Code` — via `highlight_code` on a `│ ` gutter; `Console` —
  stdout colored `text`, **stderr — `error`**, the exit code — `warning`, sections on
  `└ `/`│ `; `Markdown` — via `markdown::render`; `Plain` — as before. The `│`/`└`
  gutters are WGL4-safe (compatibility mode); highlighting degrades to plain
  text. `parse_console` parses `python::format_output`'s output by label
  lines (`stdout:`/`stderr:`/`exit code:`), tolerant of empty lines inside
  sections; a non-our-format output ("(empty output, success)", launch errors) → `Plain`.
- **Fix for the rail indent after a card**: when a tool call is the last element of a
  message (the result = the end of a turn, typical for `python_exec`), the following
  indent used to get only the **railless** inter-message separator, and the colored `▌`
  rail was cut off at the result; for tools followed by more assistant text, the
  indent was railed. Now every card is followed by a railed empty line
  (`ensure_blank_line` after `push_tool`); `build_lines` no longer adds the railless
  separator if the body already ends with an empty line (avoiding a double gap).
  The indent after a card is now the same everywhere (as the last element / followed
  by text / between two calls).
- **Invariants**: FSD (`widgets → features → shared`), `FeedToolCall` unchanged,
  live streaming and reload go through the same presenter. **Tests**: the presenter (17 —
  python/fs/generic/console/truncation/invalid JSON), `highlight_code` (no fences +
  RGB; fallback with no language), card rendering (RGB code highlighting, stderr colored as
  error), the rail indent (`tool_last_in_message_keeps_railed_trailing_blank`). **827 tests
  green** (+19), 26 `#[ignore]`, clippy/fmt clean. Docs: spec §11.3–11.4,
  architecture.md §8.

### Post-M9: markdown-render refinements (LaTeX + writer + feed cache) (done)
- Six focused stages per the plan
  [docs/history/markdown-refinements.md](../../docs/history/markdown-refinements.md) (branch
  `feat/markdown-refinements`, one commit per stage): event-walker defects, gaps in the
  unicode approximation of LaTeX on real LLM output, false positives of the
  math extension, and no render cache in the feed. Only `shared/markdown/`
  (writer/latex/code/mod) + `widgets/message_feed.rs`; the external surface
  (`render`/`render_with`/`highlight_code`) unchanged. See [ADR 0003](../../docs/decisions/0003-own-markdown-renderer.md),
  spec §11.4.
- **Stage 1 — writer defects** (`writer.rs`): `$$…$$` no longer double-skips before a
  formula (the first line is placed into the already-open empty paragraph line via
  `push_span`); DisplayMath inside a table cell stays **inside the cell** (lines joined with "; "),
  no longer leaks above the table; an autolink `<url>`/email doesn't duplicate the URL (`LinkType` —
  the link is no longer remembered); a syntect/ansi highlighting error → a flat line, not
  lost content (a shared `highlight_line_or_plain`); `<br>` → a line break (inside a
  cell — a space); a code block's info string (` ```rust,no_run `) resolves the syntax by the first
  token; `---` stretches to the panel width; images print alt text + the URL (a separate
  `image` field).
- **Stage 2 — degrading unknowns gracefully + symbol tables** (`latex.rs`): unrecognized
  brace commands **keep their braces** (`LBRACE`/`RBRACE` sentinels) — `\binom{n}{k}`
  stays readable, `\boxed{x}` stays as-is instead of collapsing into `\binomnk`; fraction
  variants `\dfrac`/`\tfrac`/`\cfrac` → aliases of `\frac`; `\binom{n}{k}→C(n, k)`; `\sqrt[3]{x}→∛(x)`,
  `\sqrt[n]{x}→ⁿ√(x)`; `\left.`/`\right.` — swallow the "invisible" delimiter dot;
  `\overset`/`\underset`/`\stackrel` → the base (second) argument; +text wrappers
  (`texttt`/`emph`/`mbox`/`overbrace`/…); ~50 new symbols (⟨⟩ ⌊⌋ ≅ ⊢ ∖ ↪ variant-
  Greek letters ⋃ †), `\backslash→\`; `normalize_delimiters` — `~~~` fences and an
  unclosed short `` ` `` (per CommonMark it's literal — formulas are normalized
  afterward; an unclosed `` ``` `` — verbatim to the end); a recursion-depth ceiling
  (stack protection).
- **Stage 3 — letter subscripts/superscripts, `\mathbb`, a bracket fallback** (`latex.rs`):
  letter super-/subscripts (`x_i→xᵢ`, `a_n→aₙ`, `x^T→xᵀ`, `\sum_{i=1}^{n}→∑ᵢ₌₁ⁿ` — the
  full available set of Unicode modifiers); an unmappable group keeps its
  grouping (`x^{q+}→x^(q+)`, previously lost the braces); `\mathbb{R}→ℝ`, `\mathcal{L}→ℒ`,
  `\mathfrak{C}→ℭ` (BMP only — supplementary-plane isn't used, terminals render it unevenly).
- **Stage 4 — environments and formula mode** (`latex.rs` + `writer.rs`) — the most
  valuable one for quality (`\begin{aligned}…\end{aligned}` — the main source of "mush"):
  `MathMode` (Inline/Display) + two wrappers `latex_to_unicode`/`_display` (DisplayMath calls
  display); `strip_environments` drops `\begin{…}`/`\end{…}` (+ array/tabular colspecs),
  `\\` → a line break (Display) / "; " (Inline), `&` (alignment) is removed,
  `\label{…}`/`\hline`/`\notag`/… is removed, `\&` → a literal `&`; real source
  line breaks (without `\\`) don't break the formula; `collapse_spaces` (dropping `&`/`\hline`
  doesn't leave double spaces); inline output has no `\n` (a ratatui span). **Behavior
  change**: `\\` is now a line separator, not a literal `\`.
- **Stage 5 — a price heuristic** (`writer.rs`): `$5-$10`/`$5/$7` (pulldown returns
  `InlineMath("5-"/"5/")`) are no longer swallowed by the math extension — content made
  only of digits/signs and ending in a separator (`-`/`–`/`/`) is printed as the literal
  `$…$`; legitimate `$3.14$`/`$2+2$`/`$x^2$`/`$n$` are unaffected.
- **Stage 6 — a feed render cache** (`message_feed.rs`) — the biggest systemic CPU win,
  orthogonal to the renderer: `build_lines` is called on every dirty frame
  (streaming — up to ~20/s, every scroll step) and used to re-run markdown+syntect over
  **the whole** history. A `CachedBlock{fingerprint,lines}` cache keyed by message position; the
  `CacheKey{width,palette,show_thoughts}` key (a change → resets the whole cache);
  the fingerprint (an std hash of all `FeedMessage` fields) is compared per block —
  a streaming/changed one is recomputed, the rest come from the cache; truncating the history
  (`Ctrl+E`/regen) drops the tail; `build_message_block` — a pure function computing one
  message's contribution (the rail + a trailing separator). Golden equivalence between a
  warm cache and a fresh render, checked by tests.
- Three existing tests were deliberately changed (plan §9): `\\` semantics
  (`escaped_backslash_and_brace`), `x^{ab}→xᵃᵇ`, `\mathbb{R}→ℝ`. **888 unit tests
  green** (+~35), 33 `#[ignore]`, clippy `-D warnings`/fmt clean. No live run
  needed (a pure render module with no engine; the feed is an interactive TUI, covered
  by golden tests).

### Post-M9: synchronized output (DEC 2026) — fixing the "jumping cursor" (done)
- **Symptom**: while a response was streaming, the cursor would jump between the input
  box and the token counter in the status bar; while RAG indexing ran in the
  background — between the input box and the banner spinner (at the frame rate,
  ~20/s). Dirty repaint (the earlier cursor-blink fix) doesn't cure this: during
  streaming/animation frames legitimately come often.
- **Cause** (traced through ratatui 0.30.1 / crossterm 0.29 source): the terminal's
  hardware cursor = the write position. ratatui writes the frame diff with the cursor
  **visible** (`apply_buffer_with_cursor`: first `flush()`es the diff, only then
  `show_cursor` +
  `set_cursor_position`), and for `CrosstermBackend` both cursor methods are
  `execute!` (an immediate flush), i.e. the tail of the frame **by construction**
  goes to the terminal as separate writes; a large diff is also chopped up by stdout's
  small buffer (`LineWriter`, ~1 KiB). Windows Terminal renders asynchronously (ConPTY) and
  would show an intermediate state: the cursor at the last cell written by the
  diff. The diff is written top-to-bottom by row, and the status bar is at the bottom of the screen →
  during generation the "last cell" is the token counter; in an RAG animation frame
  only the spinner changes. The observation matches the mechanics exactly.
- **Fix** (`app/runtime/mod.rs::run_loop`): every frame is now wrapped in
  synchronized output — `execute!(BeginSynchronizedUpdate)` (CSI `?2026h`)
  before `terminal.draw`, `execute!(EndSynchronizedUpdate)` (CSI `?2026l`) after.
  The terminal buffers everything in between and applies the frame **atomically** —
  intermediate states (the cursor on the counter/spinner) are physically never shown. A draw
  error is now propagated **after** the mode is lifted (the match arms no longer carry
  `?` — a shared `drawn?` after the ESU), so the terminal doesn't stay in buffering mode;
  `?2026l` is also emitted in the panic hook and on exit (a panic inside `draw` can
  happen between h/l — otherwise the frame would remain frozen until the terminal's timeout;
  DECRST of an unset mode is a
  no-op). Bonus: the full-feed repaint during VS16 scrolling (the `\0` sentinel) also
  became atomic on terminals that support this.
- **Compatibility**: Windows Terminal ≥ 1.18 (2023), kitty/alacritty/wezterm/foot/
  iTerm2/Ghostty/konsole/tmux 3.4+ — support it; conhost (compat mode)
  ignores the unknown private mode — graceful degradation (the jump remains, as
  before, no worse). On Windows the commands are declared ANSI-safe, crossterm's winapi
  fallback — a no-op. **Residual behavior**: ratatui unconditionally sends `show`+`MoveTo` every
  frame, and WT resets the blink phase on every move → during active streaming the
  cursor in the input box looks "solid" (doesn't blink). This is the previous
  behavior minus the jumps; unfixable without bypassing ratatui's draw cycle.
- **No tests, deliberately**: BSU/ESU go straight to real stdout, bypassing
  `TestBackend`; the effect itself is terminal-emulator behavior. Verification — a live run on WT
  (a long streaming response + `/rag add` of a large folder) and a look at conhost
  ("no worse than before"). **935 unit tests green** (count unchanged), 33
  `#[ignore]`, clippy `-D warnings`/fmt clean. Docs: spec §4.4.1, architecture §4.

### Post-M9: horizontal row separators for Markdown tables (done)
- **Tables in the feed gained optional horizontal separators between the
  body rows** (`├───┼───┤`, a "grid" look — easier to track a row across a
  wide table). Setting `interface.table_row_separators`
  (a container `#[serde(default)]` → old `settings.json`s need no migration),
  **off by default** (a compact look — a separator only below the
  header); enabling it gives a "grid" look. A toggle "Table row separators" —
  in the "Appearance" group of the "Interface" section (`FieldId::ITableSeparators`,
  with a description hint; in `field_spec` — a plain Toggle).
- **Wired via `markdown::RenderOpts`** (a precedent — `RenderOpts` on
  `InputBox`): the renderer gained a behavior-flag struct
  `{ soft_break_as_newline, table_row_separators }` — `render_with(input, width,
  palette, opts)` takes it instead of the previous positional soft-break
  bool; a 3-arg `render` facade (default flags) remains for the module's tests
  (`#[allow(dead_code)]` with a comment — the feed's production paths pass the flags
  explicitly). `Writer` carries the flag as a field (like `soft_break_as_newline`);
  `render_table` gained a `row_separators` parameter and inserts a
  `border_line(Mid)` **between** body rows (still `└─┴─┘` at the
  bottom after the last one; clipping a narrow table also clips the separators, the "≤ panel
  width" invariant holds).
- **The feed**: a field `MessageFeed.table_row_separators` (default `false` — mirroring
  the config default) + a setter `set_table_row_separators`; the flag joined the render
  **cache key** (`CacheKey`) — toggling the setting resets the cache and
  redraws the tables. Wired through all three markdown paths of the feed: assistant
  response fragments, user messages (alongside `soft_break_as_newline`),
  markdown blocks inside tool cards. `ChatScreen::set_settings` passes the setting from
  the snapshot (like the palette/confirm flag) — applies on the fly, no restart needed.
- **Tests**: table (a single `├…┤` under the header by default; with the flag —
  separators between rows and not after the last one; a single-row table
  looks the same as without the flag; the width invariant with the flag at 24–80 columns, including clipping);
  message_feed (the setting works + a value change invalidates the cache); settings
  (the toggle is in the "Interface" section, toggling it saves the config, a description
  is present); config (default on, round-trip with it off). Docs: spec §11.4.
  **947 unit tests green** (+6), 33 `#[ignore]`, clippy `-D warnings`/fmt
  clean.

### Post-M9: rendering Mermaid diagrams in the feed (done)
- **` ```mermaid `-blocks in the feed render as text graphics** (crate
  `mermaid-text` ≥ 0.56.1; two net-new dependencies — `mermaid-text` + `ascii-dag`)
  instead of printing the source. Getting to the feature was a three-step move: a **probe**
  (2026-07-14, verdict NO-GO on 0.56.0 — a panic on Cyrillic sequence diagrams + silent corruption of
  flowchart labels, byte offsets treated as character ones;
  [docs/research/mermaid-ascii-rendering.md](../../docs/research/mermaid-ascii-rendering.md)) →
  **an upstream fix, ours** (a bug report
  [leboiko/markdown-reader#29](https://github.com/leboiko/markdown-reader/issues/29) +
  a ready-made [PR #30](https://github.com/leboiko/markdown-reader/pull/30) with three fixes and
  regression tests; the maintainer did a full audit off our "broader note", closed
  3 more spots of the same class, and shipped 0.56.1) → **a re-probe** (0 panics on
  a 31-case corpus, the corruption gone) → implementation per the validated plan §4.
- **Key rule — a hard fallback instead of clipping** (clipping with "…" is fine for tables, a
  cut-off diagram is unreadable): only the whitelist renders
  (**flowchart/sequence** per `detect`; pie/gantt/class/state are ugly as text) and only
  what fits the panel width entirely (**our post-check**: the crate's `max_width` is a soft
  hint, sequence diagrams ignore it; the feed's "line ≤ width" invariant is hard, otherwise
  a second wrap pass would break the panel borders). Any failure (garbage/a truncated stream/a type outside the
  whitelist/overflow) → the source, as a code block, **byte-for-byte identical to the disabled-
  toggle render** (the golden test `mermaid_fallback_matches_disabled_render` compares
  whole `Line`s, styles included). Worst case = the previous behavior.
- **Implementation** (modeled on `TableBuilder`): `Writer.mermaid: Option<(info, src)>`
  — `start_codeblock`, when the flag is on and `lang=="mermaid"`, buffers the block (the fence
  isn't printed), `text()` accumulates the source verbatim (resilient to it being split across
  events), `end_codeblock` decides: `mermaid::render_mermaid_block` (a new submodule
  `shared/markdown/mermaid.rs`) or `emit_fenced_source` (the fallback, matching the old
  unhighlighted look: reverse video + DIM fences with the full info string ` ```mermaid title=x `).
  The compat palette (`palette.compat`) → `render_ascii_with_width` (ASCII glyphs, for conhost).
  Streaming: an unfinished block doesn't parse → the source; once finished, the feed's cache
  recomputes the message and swaps in the diagram. **We deliberately don't catch upstream
  panics** (`catch_unwind`): the app's panic hook restores the terminal on any
  panic, including a caught one — "catch and continue" would leave the TUI broken;
  we rely on the 0.56.1 audit + the narrow whitelist (recorded in the module doc).
- **Wiring**: `RenderOpts.render_mermaid` (Default=false — tests/tool cards see no
  behavior change); the feed threads through **the whole `RenderOpts`** instead of the bare
  `table_row_separators: bool` via `build_message_block`/`push_body`/
  `push_assistant_body`/`push_markdown_fragment`/`push_tool`/`push_block` (a future
  flag won't touch signatures again; `soft_break_as_newline` gets added to `push_body` for
  the user just as before). `MessageFeed.render_mermaid` + `set_render_mermaid` +
  a field in `CacheKey` (toggling the setting invalidates the cache); `ChatScreen::set_settings`
  threads it from the snapshot. The config `interface.render_mermaid` (`#[serde(default)]`
  on the container → old `settings.json` files without migration), **on by default** —
  the fallback makes enabling it safe. A "Mermaid diagrams" toggle in the "Appearance"
  group of the "Interface" section (`FieldId::IMermaid`, described in both locale
  bundles — the i18n gates covered it automatically).
- **Tests**: the mermaid module (a sequence/Cyrillic flowchart with no corruption; fallbacks —
  a narrow width/outside the whitelist/garbage/a truncated stream; ASCII in the compat palette; the width budget at
  60/90/120); writer (a diagram instead of the source + a 4-scenario golden fallback +
  the flag off + Cyrillic); the feed (the toggle + cache invalidation); settings
  (the toggle persists the config + the description); config (default-on). **1090 unit tests
  green** (+13), 48 `#[ignore]`, clippy `-D warnings`/fmt clean; the new crates' licenses
  (MIT; MIT OR Apache-2.0) are in the `deny.toml` allowlist. **No live engine run
  required** (a pure render module without the engine/memory — a precedent from
  markdown-refinements); the crate's behavior on real LLM diagrams was verified by
  the probe (a 31-case corpus, including 17 real ones from architecture.md).
- **Groundwork**: extending the whitelist (state/class/er — once their text render becomes
  readable). `max_width` as a hard budget — **closed**: our upstream feature request #32
  was implemented in 0.57.0 (`RenderOptions::max_width_strict` → `Error::TooWide`);
  we upgraded to 0.57.0 but did NOT adopt the strict mode — our post-check via
  `display_width` is more precise (accounts for CJK/emoji, matches the wrap in `message_feed`),
  and `render_with_width` already tightens the gaps to the width.

### Post-M9: Mermaid — source until the closing fence (fix for streaming flicker) (done)
- **Symptom**: during a streamed response a ```mermaid block "flickered" — as
  chunks arrived, the feed alternately rendered a stub as a diagram and reverted
  to the source. **Cause**: CommonMark (pulldown-cmark) stretches an unclosed
  fence to the end of the document, so from the parser's events an in-progress
  streamed block is indistinguishable from a complete one, and a syntactically
  valid stub (`flowchart LR\n A --> B` with no tail) successfully rendered as a
  partial diagram. The claim in spec §11.4 that "an incomplete block doesn't
  parse → source" relied on the stub failing to parse — but it often does
  parse. Branch `fix/mermaid-stream-fallback`.
- **Fix** (`shared/markdown/`): fence closure is checked **against the
  source**. `render_with` switched to `Parser::into_offset_iter()`;
  `Writer::run(src, iter)` calls a new `fenced_block_is_closed(src, range)`
  before every `Start(CodeBlock)` (only when mermaid rendering is enabled) — a
  `Start(Tag)` range covers the whole element: a block followed by more text
  in the document is closed by construction; for a block running to EOF, the
  range's last line must be a closing fence (the same fence character
  `` ` ``/`~`, no shorter than the opener; blockquote prefix/indent trimmed).
  The result is a `Writer.codeblock_closed` field; the mermaid branch of
  `start_codeblock` buffers the block only when the fence is closed, otherwise
  — the regular source path (byte-for-byte identical to the toggle-off render,
  the prior golden invariant). The check is deliberately simple: a false
  "closed" on an edge case would only trigger an attempted render with the
  standard fallback.
- **No plumbing through the screen/events is needed**: the feed's cache already
  recomputes a streaming message by fingerprint — once the closing fence
  arrives, the block is recomputed and the source is swapped for the diagram;
  exactly **one "source → diagram" transition per block** (closure is a
  property of the block, not of the end of the message: a closed diagram
  renders even while the message's tail is still streaming). Bonus: a block
  whose model forgot the closing fence forever stays source (consistent with
  the hard-fallback philosophy).
- **Tests**: writer (`mermaid_unclosed_fence_streams_as_source` — golden: 6
  truncation shapes, including a valid stub, a tilde fence, and "closer shorter
  than the opener", byte-for-byte equal to the disabled render;
  `mermaid_closed_fence_renders_even_while_tail_streams`); feed
  (`streamed_mermaid_stays_source_until_fence_closes` — simulating streaming
  across two chunks through the cache). **1128 unit tests green** (+3), 50
  `#[ignore]`, clippy `-D warnings`/fmt clean. No live run needed (a pure
  render module without an engine — precedent mermaid-render/
  markdown-refinements); behavior covered by golden tests. Docs: spec §11.4,
  CHANGELOG (Fixed).

### Post-M9: emoji popup — a "hanging" selection ghost after closing (done)
- **Symptom** (branch `fix/emoji-picker-residue`): after closing the emoji
  popup (`Ctrl+B`), a ~1-column-wide colored rectangle (background
  `keycap_bg`) lingered at the previously selected cell, surviving
  subsequent redraws.
- **Cause found empirically** (a probe against real `Buffer::diff` in
  ratatui 0.30.1, not a guess): a wide emoji occupies **two** cells — its
  own (the glyph) and a **trailing** one that ratatui resets to the default
  (`" "`, the default style). When the popup closes, the frame "popup →
  empty" produces updates for cells `x` (the glyph) and `x+2` (the space to
  the right), but **not for `x+1`**: in both buffers it's the default
  space, so the diff considers it unchanged and doesn't send it to the
  terminal. conhost/Command Prompt itself doesn't clear a wide glyph's
  second half when the first half is overwritten → a piece of it stays on
  screen. It's visible only where the cell had a **background** — hence
  exactly the selected grid cell "glowed", not the whole popup.
- **Fix** — reused a mechanism already present in the project for a
  flicker-free full redraw (the buffer sentinel `"\0"` + `swap_buffers`,
  set up for "drifting" VS16 emoji when scrolling the feed): a probe
  confirmed that with the sentinel, diff emits **every** cell, including
  the trailing one. `ChatScreen` gained a flag `full_redraw` +
  `request_full_redraw()`; `handle_emoji_key` requests a redraw for **any**
  action in the popup — not just closing, but also shifting the selection
  via arrows too (the highlight likewise drifts off the previous cell,
  leaving the same artifact). Opening the popup requires no redraw (the
  glyph goes nowhere).
- **Generalizing the flag without losing the original source**: the
  `app/runtime` loop switched from `take_feed_scrolled()` to
  `take_full_redraw()`, which unconditionally pulls in **both** sources
  (not via `||` — `take_feed_scrolled` must reset the feed widget's
  internal state regardless of the second request); the previous VS16 path
  is additionally pinned by an assertion in an existing test.
- **Tests**: screen behavior (`emoji_picker_actions_request_full_redraw` —
  closing via `Enter`/`Esc` and shifting the selection request a redraw,
  the flag is consumed exactly once, opening — no request;
  **mutation-tested**: removing `request_full_redraw()` fails the test) +
  **a root-cause canary**
  (`wide_glyph_trailing_cell_is_not_repainted_by_plain_diff` in
  `widgets/emoji_picker`): pins that a plain diff skips the trailing cell
  while the sentinel repaints it; the test failing would mean upstream
  fixed this on its own and the workaround could be reconsidered. **1169
  unit tests green** (+2), 53 `#[ignore]`, clippy `-D warnings`/fmt clean.
  No live engine run **needed** (a pure UI fix, no engine/memory involved);
  visual verification on conhost is left to the user.
- **A related case closed at the user's request: the spellcheck suggestion
  popup (`Ctrl+G`).** First **verified by a probe**, rather than "just in
  case": the hypothesis was that there was no defect since `List` applies
  highlighting via `buf.set_style(row_area, …)` **after** rendering the
  content — seemingly covering the trailing cell too. The probe showed the
  opposite: the glyph `➕` = `mod=ITALIC | REVERSED`, while its **trailing
  cell** is `mod=NONE`, and it isn't repainted on close. So the class is
  the same as with emoji (the highlight doesn't reach the trailing cell),
  and a fix was needed. Added `request_full_redraw()` to
  `handle_suggest_key` — **more precise than for emoji**: only when an
  action actually changes something (`Esc`/`Enter`/`↑`/`↓`), other keys
  (`_ => return`) don't request a redraw. Test
  `suggest_popup_actions_request_full_redraw` (shifting the selection /
  both closing paths request it, a no-op key doesn't; **mutation-tested**).
  `➕` isn't VS16 (U+2795), so the row shift seen with `❤️` doesn't occur
  here.
- **A second iteration, based on a user report (same branch): VS16
  clusters removed from the popup list.** A full redraw uncovered an
  adjacent defect: after moving the selection through the grid, `🔥` would
  disappear and the frame "broke" in two spots (rows 3 and 4 — exactly
  where `✌️` and `❤️` sat). **Mechanics** (confirmed by a probe against
  real `Buffer::diff` + reading the backend, not by guesswork): for a VS16
  cluster, `ratatui` **deliberately** emits the glyph's trailing cell (a
  workaround for terminals that don't clear the second half), and
  `CrosstermBackend::draw` tracks `last_pos` **by cell number, without
  accounting for glyph width** (`x == p.x + 1` → no `MoveTo`) — the
  trailing cell prints one column to the right, and the rest of the row
  drifts: the following wide emoji gets its right half overwritten
  (conhost blanks the whole glyph → `🔥` disappeared), and the frame gets
  pushed outward. Without the sentinel the trailing cell wasn't emitted at
  all (the glyph didn't change: space → space), which is why the defect
  only surfaced after the first iteration. **Fix at the source**:
  `✌️`→`🤞`, `❤️`→`💖` (supplementary-plane characters, honest 2 columns, no
  trailing cells produced); the rest of the set untouched. Widths were
  checked programmatically (`unicode-width` 0.2 counts a VS16 cluster as 2
  — the grid model was consistent, the bug is specifically in terminal
  output).
- **Two gates for the invariant** (both **mutation-tested** by
  reintroducing `❤️` into the list): `emoji_list_is_width2_without_vs16` —
  a direct one (width 2, no U+FE0F, a message suggesting a replacement) and
  `sentinel_repaint_never_writes_into_second_half_of_wide_glyph` —
  behavioral and terminal-agnostic: during a full redraw, no diff update
  targets the second half of a wide glyph. **1174 unit tests green** (+7
  over `main`), 53 `#[ignore]`.
- **Third iteration: the same defect found and fixed in the feed — at the
  sentinel level.** Checking a deferred risk (at the user's request): the
  feed's long-standing scrolling path uses the same technique and is
  triggered **specifically when VS16 is present**. A probe against a real
  feed frame (40 lines with `🗂️`): during a **normal** scroll — **0**
  updates into the second half of a wide glyph, **via the sentinel — 6**,
  exactly matching the number of VS16 cells in the frame. So the row shift
  in the feed was being triggered by the sentinel itself.
- **A fix at the mechanism, not the content** (unlike the emoji list): the
  sentinel was factored out into a named `shared/ui.rs::prime_full_redraw`
  and redesigned — instead of the symbol `"\0"`, a cell gets **a space +
  the `HIDDEN` marker modifier**. The space matches the content of a wide
  glyph's trailing cell (which `ratatui` resets to default), so the
  condition for emitting the trailing cell
  (`prev.symbol() != next.symbol()`) no longer triggers — no shift; while
  the modifier difference preserves full redraw coverage. `HIDDEN` is
  unused elsewhere in the UI (the palette works with
  `DIM`/`BOLD`/`ITALIC`/`UNDERLINED`/`REVERSED`) — pinned by test
  `sentinel_modifier_is_unused_by_ui`. Measurement: **1194 out of 1200**
  cells repainted, the uncovered ones are exactly 6 trailing halves (they're
  already covered by the glyph itself — no need to write there).
- **The gate moved to the mechanism**:
  `prime_full_redraw_repaints_all_but_wide_glyph_tails` (`shared/ui.rs`) on
  a synthetic frame with VS16 checks both properties — (1) no update
  targets the second half of a wide glyph, (2) no gaps in the repaint (the
  only uncovered cells are trailing ones). **Mutation-tested**: reverting
  the sentinel to `"\0"` fails it with a precise message. Emoji popup tests
  switched over to the real helper instead of their own copy of the
  sentinel. **1177 unit tests green** (+10 over `main`), 53 `#[ignore]`.
- **Fourth iteration: the full-redraw trigger extended from scrolling to a
  change in feed content.** A detailed conhost run by the user (a chat
  about the mechanics of emoji — VS16, ZWJ families, surrogate pairs)
  showed: artifacts show up on **streaming output** and on **adding a
  note** (`F5` "Conversation copied…", with the feed scrolling down at the
  time), while **scrolling fixes them**. The cause is systemic: only
  scrolling was requesting a full redraw (`take_feed_scrolled`), while
  `scroll_to_bottom` didn't raise the flag at all — a content change had
  no trigger.
- **How "change" is defined (the outcome of the second attempt)**: the flag
  is set by the feed's **own mutators** (`ChatScreen::mark_feed_changed` in
  `activate_chat`/`push_*`/`continue_`/`rewrite_assistant`/`Ctrl+T`). The
  first attempt defined a change **based on render** (a miss in the feed's
  block cache) — it worked, but was one frame late: an artifact would
  still **flash** ("defects appear for a split second and vanish" per the
  user's report). A bad frame can't be hidden behind synchronized output —
  on conhost, where the problem lives, mode 2026 is ignored. So we
  determine it **before** the render: `take_full_redraw()` in the loop sees
  the flag already raised, and the bad frame never gets drawn. The
  render-time plumbing (`MessageFeed.content_changed`) and the "catch-up"
  frame in the loop were removed.
- **The risk flag is cached** (`ChatScreen.feed_has_risky`) and only
  accumulates: edits always touch the last block, so `mark_feed_changed`
  checks it, while `activate_chat` (a full feed replacement) recomputes
  from scratch — otherwise scanning the whole feed on every streamed chunk
  would be O(the entire chat's text). Over-estimating is safe (an extra
  redraw isn't visible, a missed one leaves an artifact).
- **The risk of this approach is closed by a gate**
  `every_feed_mutator_marks_content_change`: it runs **all** feed mutators
  (+`Ctrl+T`) and requires a redraw request — a forgotten call would
  reintroduce the flash. **Mutation-tested** (removing the call in
  `push_chunk` fails the test with the mutator's name in the message).
- **The detector was expanded** from VS16 to a risk group
  (`feed::is_risky_glyph`): VS16, **ZWJ** (`👨‍👩‍👧` — on the screenshots
  it's precisely these that produce the worst mess), skin-tone modifiers,
  supplementary-plane pictographs (`😀`), width-2 BMP emoji (`✅`/`⭐`).
  **CJK is deliberately excluded** — terminals render ideographs
  consistently, no point triggering a full redraw for them on every chunk.
  Cost: streaming text with emoji makes every frame a full redraw (plain
  text — unchanged from before).
- **Not closed: "the background inverts on half a glyph"** (the report's
  third symptom). The mechanics differ and **lie upstream**: `ratatui`
  resets a wide glyph's trailing cell to the **default style** (confirmed
  by a probe: the glyph cell has `bg=Red`, the trailing cell has
  `bg=Reset`) and omits it from the diff, while conhost never sets the
  attribute of the second half itself — half the glyph keeps the old
  background. A full redraw doesn't fix this (the cell simply isn't
  emitted). It'd need to be fixed either in `ratatui` (having the trailing
  cell inherit the glyph's style) or in `ratatui-crossterm` (tracking
  `last_pos` with glyph width in mind — then emitting the trailing cell
  would stop shifting the row and become safe). A candidate for an
  upstream PR; there's precedent (mermaid-text #29/#30).
- **Consequence**: replacing `❤️`/`✌️` → `💖`/`🤞` in the popup is now
  **not strictly required** (the shift is fixed by the sentinel), but stays
  as defense in depth — VS16 clusters in a fixed-width grid remain a
  fragile class, and the list gate pins this.

### Post-M9: ratatui-core/crossterm update 0.1.1 → 0.1.2 (done)
- **A dependency update along with a review of the workarounds** from the
  previous PR (#189, emoji artifacts on conhost); branch
  `chore/ratatui-0.1.2`. Forks confirmed by the user on 2026-07-20 (narrow
  the redraw down to closing; keep the VS16 emoji replacement).
- **The 0.1.2 release's scope is exactly one functional change** (checked
  against the crates' source, not the release notes): `ratatui-core/src/
  buffer/diff.rs`; `backend.rs` — doc changes only (mentions of Termina),
  while **`ratatui-crossterm` is byte-for-byte identical to 0.1.1**. Hence
  the narrow regression surface.
- **What exactly 0.1.2 fixes** (ratatui#2585, restored `invalidated` in
  diff): a wide glyph's trailing cell is now emitted **when**
  `previous_width > cell_width` **AND** "the style is visible on an empty
  cell" (`bg != Reset` or `REVERSED`/`UNDERLINED`/`SLOW_BLINK`/
  `RAPID_BLINK`/`CROSSED_OUT`). I.e. it closes exactly **our** observed
  defect — the selection-backdrop ghost when **closing** the popup.
- **What's NOT fixed** (verified by probes against `Buffer::diff`, not
  assumption):
  1. **Unstyled trailing cells** — in the emoji popup's grid, 43 of 44
     glyphs go without emitting a trailing cell (`styled emitted=1
     missed=0 | unstyled emitted=0 missed=43`).
  2. **Shifting the selection** — the glyph stays wide, the condition
     `previous_width > cell_width` doesn't hold, no trailing cell is sent
     (confirmed both on the emoji popup and on `➕ add to dictionary`:
     `ITALIC|REVERSED` → `tail_close=true tail_move=false`).
  3. **The backend's `last_pos` ignoring glyph width** (ratatui#2651, the
     row shift) — the source wasn't touched at all.
- **A key finding that changed the plan** (the task assumed the popups'
  redraw could be removed entirely): for **shifting the selection**, a
  full redraw provably **accomplishes nothing** — the sentinel must skip a
  wide glyph's trailing cell (otherwise the backend would print half of it
  without `MoveTo` and shift the row, per item 3), so it was a no-op there
  from the start; the backdrop is actually cleared by the glyph itself
  being reprinted via a plain diff. So the redraw request was **narrowed
  to closing** the popups (emoji and spelling), not removed and not kept
  for every action. It stays for item 1 (unstyled trailing cells), which
  upstream doesn't cover.
- **The canary was rewritten to match the new boundary** (the old
  `wide_glyph_trailing_cell_is_not_repainted_by_plain_diff` was failing —
  by design, since it was set up for "upstream fixed it itself"):
  `upstream_repaints_styled_tail_on_close_but_not_on_selection_move` pins
  **both** sides — (A) upstream sends a styled trailing cell on close
  (fails if upstream regresses), (B) it doesn't on a shift (fails once
  upstream also fixes that case → then the workaround should be removed),
  plus that the sentinel recovers the trailing cell on close and
  **doesn't** touch it on a shift.
- **Left unchanged** (with justification): `prime_full_redraw` and its
  "space + `HIDDEN`" design — the invariant "never write into the second
  half of a wide glyph" rests on the unfixed #2651 and became **more**
  important, not less; `mark_feed_changed` + `is_risky_glyph` — the feed's
  text is unstyled (`bg = Reset`), the 0.1.2 fix doesn't apply to it;
  replacing VS16 emoji (`💖`/`🤞`) and the
  `emoji_list_is_width2_without_vs16` gate — defense in depth, while #2651
  remains open.
- Popup tests were rewritten under the narrowed contract and
  **mutation-tested in both directions** (bringing back the redraw on
  shift → the shift assertion fails; removing it from close → the close
  assertion fails). **1177 unit tests green** (count unchanged), 53
  `#[ignore]`, clippy `-D warnings`/fmt clean. Docs: architecture.md §4
  (the boundary with upstream + issue numbers), CHANGELOG (`[Unreleased]`
  wording aligned to the actual mechanics — the mention of "moving across
  the grid" removed).
- **No live engine run needed** (rendering, no engine/memory involved). **A
  manual regression pass on conhost was run by the user**: popups
  (emoji/spelling), Mermaid, and the feed stream — clean, narrowing the
  redraw introduced no regressions. A separate defect of the same class
  was found (below).

### Post-M9: a full redraw on screen switch and in the input box with VS16 (done)
- **Symptom** (a manual conhost regression pass after the ratatui update):
  in a feed with `❤️`, switching to the chat list / `F3` and **back** would
  add an extra space after the emoji; it went away on scroll. Same branch
  `chore/ratatui-0.1.2`.
- **Cause — the VS16 branch of diff + the unfixed ratatui#2651**
  (reproduced by a probe against `Buffer::diff`, all three observations
  matching the report): for a VS16 cluster, ratatui emits the trailing
  cell **when its symbol has changed**. Within a single screen, feed edits
  don't touch the trailing cell (`same_screen=false` — which is why the
  bug wasn't visible during normal use), but on **returning from another
  screen** it held a foreign symbol (`switch=true`) → the trailing cell is
  sent to the terminal, the backend prints it with no `MoveTo` (position
  is tracked by cell number, without glyph width) → the rest of the row
  drifts right. Scrolling "fixed" it because it requests a full redraw,
  while the sentinel **doesn't** send the trailing cell (`sentinel=false`)
  — exactly the property for which it's built as "space + `HIDDEN`".
- **Fix — a full redraw on SCREEN SWITCH** (`app/runtime/mod.rs`),
  centralized: `prime_full_redraw` was hoisted above the `match`, out of
  the `ActiveScreen::Chat` arm; the condition is `requested || switched`,
  where `switched` = a change in `std::mem::discriminant(&active)` **as
  observed after rendering** (switching "there and back" between frames
  changes nothing visually). A one-place fix instead of 8+
  `*active = ActiveScreen::…` sites; covers any direction and any screen
  (VS16 can appear both in a chat title in the list, and in the `F3`
  narrative). The chat's flag is only pulled while the chat is active
  (otherwise it would be lost).
- **The same class was found and closed in the input box** (via probing,
  not reported by the user): an edit **to the left of** `❤️`, immediately
  followed by a non-space character, moves the glyph onto a foreign cell
  → the trailing cell gets emitted → the row shifts. `mark_input_changed`
  now requests a full redraw when the field contains a risk-group glyph;
  the check is streaming — a new `InputBox::any_char(pred)` (the caller
  supplies the predicate, the widget doesn't know about upper layers —
  FSD), avoiding building `text()` on every edit. For ordinary text — a
  no-op.
- **Tests**: `shared::ui::screen_switch_emits_vs16_tail_without_full_redraw`
  pins the mechanics (a screen switch sends the trailing cell; an edit
  within a screen — doesn't; the sentinel — doesn't) and explains why a
  screen switch must go through a full redraw;
  `input_with_risky_glyph_requests_full_redraw` — the field's behavior
  (mutation-tested). **1179 unit tests green** (+2), 53 `#[ignore]`,
  clippy `-D warnings`/fmt clean.
- **What remains upstream**: half the background of wide glyphs (the
  trailing cell carries the default style, and conhost never sets its
  attribute) — not fixable locally, the cell is marked `skip` and doesn't
  reach the diff. Same root as ratatui#2651; would be fixed either by
  having the trailing cell inherit the glyph's style, or by tracking
  `last_pos` with width in mind (then trailing-cell emission would stop
  shifting the row and become safe).

### Post-M9: in-feed text search (`Ctrl+F`) (done)
- Closes the roadmap's last search item ([plan](../../docs/history/in-feed-search.md),
  forks **F1–F6 decided by the user 2026-07-29**, all as recommended). Two
  stages: **3a** moved the highlight out of the block cache, **3b** built the
  mode. Done by hand rather than delegated — five consecutive agent runs died on
  transient API 529s.
- **The investigation invalidated the roadmap's own wording, before any code.**
  Both `docs/roadmap.md` and stage 2's fork S5 specified this as "`/`-search".
  `/` is **unimplementable** here: the chat's input box is always focused, and
  typing `/` into an empty box is exactly the gesture that starts a command
  (`/rag`, `/file`, `/tts`, `/reindex`). Gating on empty input does not rescue it
  — that *is* the command-entry gesture. The settings screen can use `/` only
  because it has a top-level state with no focused editor, which
  `keys::is_slash_key` documents as its precondition; the chat never has one. So
  `Ctrl+F`, and both documents were corrected rather than left standing.
- **Stage 3a — the query left `CacheKey`, and that was the bulk of the work.**
  Highlighting every match instead of one is a one-line change; the cost was that
  the query was part of the cache key, and a key mismatch clears every block. For
  an incremental field that meant re-running markdown + syntect + LaTeX + mermaid
  + table layout over the whole chat **on every keystroke** — up to 70 blocks and
  260 K characters. Measured after moving it out: a query change costs **17 ms**,
  a normal warm frame, against **39 ms** for a rebuild. One rebuild per jump
  remains and should — the marker changes the rail colour, which is baked in.
  - The header exclusion had to be rebuilt: it worked by slicing an index into the
    *unwrapped* body, meaningless post-cache. `CachedBlock` now records how many
    **output** rows the header took, counted while the block is built — normally
    one, but a long custom role name can wrap it, so counting output rows is the
    only stable answer. Mutation-tested.
  - **I predicted a bug that does not exist.** Excluding the rail span from the
    match text looked necessary; the mutation **survived**, because
    `highlight_line` derives its offsets from the same concatenation it matches
    over, so including the rail shifts both consistently and the output is
    byte-identical. The parameter was dropped — an untestable precaution is worse
    than none — and the reasoning lives at the call site.
  - Five stage-2 tests moved from asserting on `feed.cache[..]` to asserting on the
    lines handed to the renderer. Not a behaviour change: the cache is query-free
    by design now, and reaching into it was reading an implementation detail. One
    asserted the exact property this stage inverts and was rewritten.
- **Stage 3b — next/prev goes to the matched line, not the message.** The feed
  could only scroll to a block's first row, and stage 2's fork S6 had refused to
  *store* an intra-block offset because a rewrap changes a block's height. Here it
  is **derived every frame** instead, riding the same wrap-accumulating loop the
  jump uses, so S6's objection does not apply. Not a nicety: the largest real
  message is **38,782 characters**, so message-granular stepping would leave the
  viewport unmoved — the test asserts the scroll row moves, not the match index.
- **The counter is asserted equal to the number of highlighted occurrences**, so
  the two cannot drift. It exists because a common word matches **200–300 times in
  a single chat** (measured) — for calibration, the cross-chat screen caps at 200
  hits across *all* chats. Matching runs over what is drawn (fork F2), which is
  what makes that equality possible; the honest costs are that it also covers
  thoughts and tool cards, which the index does not, and misses text the renderer
  reshaped.
- **Three things that fail silently, all tested.** Fast typing arrives as one
  coalesced paste and would have landed in the message being written, so the field
  has its own paste target. `activate_chat` renumbers the feed under any open
  search (`Ctrl+E`, `Ctrl+R`, a rewrite round, a cross-chat jump all funnel
  through it), so the search closes there. And the field stands in for the input
  box rather than taking a layout row, because a fifth constraint would shrink the
  feed and rewrap the chat on open *and* close.
- **Two process lessons, both mine.** A `python` patch script that asserts on one
  pair mid-batch and dies **writes nothing**, silently losing the whole batch — it
  cost a lost early return in `handle_key`, found by a test rather than by eye. And
  **`cargo clippy … | tail` discards the exit code** (the pipeline reports `tail`'s),
  so a `&&` chain sails past a failing gate; that is how a commit landed with
  clippy red. Verified by exit code afterwards and the commit rewritten.
- **1615 unit tests green** (+11), 69 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean. **No live run required** (AGENTS.md §3): TUI
  rendering and pure matching, no engine, memory or provider protocol.
- **Still open**: highlighting a match the renderer transformed (needs
  `highlight_ranges` threaded through the renderer — stage 2's fork S3(c));
  `cache.db` holding chat-list summaries to remove the ~94 ms startup parse.

### Post-M9: raw HTML blocks render their text (and S3(c) measured, then rejected) (done)

- **Started as the deferred fork S3(c)** — highlight a match the renderer
  transformed (docs/history/in-feed-search.md, stage 2 §1.4) — and the
  measurement redirected it. Branch `docs/feed-highlight-transformed`.
- **Measured exhaustively rather than by guessing queries**: for every message in
  the dev corpus, every alphanumeric run of ≥3 characters in the *source* (what
  the index can match) checked against the *rendered* output (what the feed can
  highlight). Result: **88 of 186 805 words (0.047%)**, across **20 messages of
  1213**. The words settle it — `rightarrow` (the LaTeX command name, not the `→`
  actually on screen), `e0f2fe` (a hex colour in a mermaid `style` line), `graph
  lr`, `500px`, `td`. Nobody searches for those, and the prose around them
  highlights fine. So S3(c) — reworking `writer`/`latex`/`table`/`code` and the
  mermaid path to carry source ranges — was **rejected, not deferred**, and
  recorded as such in the plan (§5) and the roadmap. Along the way a hunch of mine
  was checked and killed: a soft wrap does **not** break a match, because
  `find_matches` matches each query token independently and a token never contains
  whitespace.
- **What the measurement found instead**: 68 of the 88 came from one message and a
  *different* defect. Block-level raw HTML was dropped by the renderer — prose
  included — so it was **invisible in the feed**, not merely unhighlightable.
  Verified directly rather than inferred: `<table><tr><td>visible prose
  here</td></tr></table>` renders to `""`, while inline `<strong>bold</strong>`
  renders fine, because pulldown-cmark delivers inline HTML's inner text as `Text`
  events and a block's as nothing at all. The old code comment ("text between tags
  arrives as `Text`") was right for inline and wrong for blocks.
- **The fix** (new `shared/markdown/html.rs`, the `latex.rs` precedent): the block
  is accumulated whole — a tag can straddle two `Html` chunks — and converted once
  at `TagEnd::HtmlBlock` by a small state machine. Tags stripped;
  `<script>`/`<style>` **content dropped** (otherwise "strip the tags" would dump
  CSS into the conversation — the one way the fix could be worse than the bug);
  entities decoded, unknown ones left visible; whitespace collapsed; block elements
  end the line and `<td>`/`<th>` separate words, so a row reads as one line;
  `<img>` prints alt + URL **exactly as `Writer::end_image` does** for a markdown
  image, so the same picture reads the same either way. Not the markup verbatim (a
  30-row table would become a wall of tags) and not a rebuilt table (a far larger
  feature that would still need this fallback). Not an HTML parser either — no tree
  is built, so malformed markup degrades into text rather than an error.
- **Not reusing `web::extract_readable`**, which does the same job for RAG and
  `fetch_url`: it lives in `features`, which `shared` may not depend on (FSD), and
  pulling `scraper` into the renderer to strip tags is heavy for the job.
- **A pre-existing behaviour deliberately preserved**: a lone `<br>` is an HTML
  *block*, so it now goes through the buffer — `is_break_only` keeps it producing
  the blank line it always did. The buffering arm sits **before** `is_br` on
  purpose: a `<br>` line inside a larger block belongs in the buffer, in order,
  not pushed out ahead of it.
- **A test of mine that was wrong, and was fixed rather than the code**: it
  asserted HTML-block lines fit the panel width. The writer does not wrap
  paragraphs — the feed does — so the assertion was inventing a promise. Replaced
  with the invariant that actually matters: no span carries a raw newline (the
  source is full of them, and one surviving would break the feed's row math).
- **Correcting my own estimate**: I expected this to close 77% of the highlight
  gap. Re-measured after the fix, it closes **26%** — 88 → **65** words. The 23
  recovered are prose (`milestones`, `methodically`, `matters`); what remains from
  that message is attribute names and values (`frameborder`, `background`,
  `500px`), which the converter drops **by design** — they are markup, and showing
  them would be the wrong fix.
- **Verified on the real message**, not only on fixtures: the corpus's one
  HTML-carrying message (1574 characters previously swallowed) now renders its
  table as prose rows, decodes `&amp;` into "What & Why", prints the image as
  "Clarity - With Specflow (clarity.svg)", and leaks no `<td`, `background-color`
  or `500px`.
- **1632 unit tests green** (+17), 69 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean. **A live run is not required** (AGENTS.md
  §3): this is the markdown renderer — no engine, memory, tool or provider
  protocol is touched. Docs: spec §11.4 (raw HTML) and §11.3.1 (the highlight
  boundary, now with the measured number), architecture §3 (the module map, which
  was also missing `mermaid.rs`), CHANGELOG, roadmap, and the plan's §5.

### Post-M9: an unhighlighted code block is drawn as a rectangle (done)

- **Reported from a screenshot**: a ` ```text ` block's background followed the
  ragged right edge of every line, so the block read as a stack of bars of
  differing length rather than one panel. Asked for explicitly: the rectangle
  should be **sized to the text, not stretched across the panel** — the rule
  tables already follow. Branch `fix/code-block-rectangle` (a simple task by
  AGENTS.md §1: one module, no cross-layer contract, no new dependency — no
  design doc).
- **Which blocks this is about, and why it is not all of them.** A code block's
  background is `code_style()` = `REVERSED`, pushed onto the line-style stack —
  and **only on the unhighlighted path** (no language, or one syntect doesn't
  know, which is what ` ```text ` is). A **highlighted** block has no background
  at all: the pipeline `as_24_bit_terminal_escaped(.., false)` carries only
  foreground color (ADR 0003). So there is nothing to square off there, and
  padding it would be invisible weight — deliberately left alone, pinned by
  `highlighted_block_is_left_ragged` so the scope decision is a test rather than
  a comment. The visual asymmetry between the two kinds of block predates this
  and is a separate question.
- **The width is known only at the end**, so the block is squared off at
  `end_codeblock`: `Writer.code_start` records the index of the opening fence in
  `lines` (set on the unhighlighted branch only), and `code::pad_code_block`
  takes the rows from there, measures them and pads each with a raw space span.
  Raw on purpose — the background comes from the **line** style, so the padding
  picks it up on its own.
- **One blank column on the right** (`CODE_RIGHT_PAD`, added after the first
  live look at the result — flush text against the background's hard edge reads
  as abrupt). Deliberately asymmetric: a matching column on the left would shift
  the code out of alignment with the fence markers and the surrounding prose.
  Rows are wrapped to `width - CODE_RIGHT_PAD` rather than to `width`, so the
  column survives the one case where the cap would eat it — a block whose text
  fills the panel, i.e. exactly where the edge is tightest. The price is that
  such a line wraps one column earlier; the alternative (keep the column only
  when it happens to fit) would give wide blocks a different look from narrow
  ones, which is the raggedness this whole change is about.
- **Rows are wrapped by the renderer, not left to the feed.** A code line longer
  than the panel would otherwise be split later and its tail would stay ragged
  inside an otherwise rectangular block. Wrapping here makes the feed's re-wrap a
  no-op (every row ≤ `width`) — exactly the invariant tables already rely on. The
  block's width is then `min(widest row, panel)`: after a word-wrap the widest
  row is often narrower than the panel (a 180-column line at panel 20 wraps to
  17), and that is correct — the rectangle follows the content.
- **`trim_row_trailing_ws` is reused from the table code** for a reason that bit
  tables first: `wrap_ranges` "spills" a word-boundary space past the row's edge,
  which would inflate the measured width and defeat the padding. Trailing spaces
  are meaningless in a code block (leading ones — indentation — are not).
- **The fences' `DIM` moved from the line onto its span** (`fence_line`). It had
  to: the padding takes the *line* style, so a line-level `DIM` would make the
  rectangle's top and bottom edges a different shade from its body. Visually
  identical for the fence text itself (the line style is folded into the spans on
  render). Mutation-tested — restoring the line-level `DIM` fails
  `rectangle_padding_is_not_dimmed` and nothing else.
- **A blank line inside the block becomes a full row of the rectangle** rather
  than a gap in it — it falls out of the same padding, and is pinned separately
  because it is the case a reader notices first.
- **Tests**: the rectangle (all rows one width, that width the block's own and
  below the panel, and the rows really carry the background); a blank line filled;
  the blank right column (at a comfortable panel and at one exactly as wide as the
  block's longest line, plus "exactly one column, not a margin"); a long line
  wrapped into the rectangle at four panel widths; the padding not dimmed; the
  highlighted block left ragged. The existing "content not glued to the fence"
  test now compares trimmed text — its point is the *content*, not the trailing
  background. **Mutation-tested**: removing the `pad_code_block` call fails four
  of the new tests **and** the golden `mermaid_fallback_matches_disabled_render`
  (the mermaid fallback pads through `emit_fenced_source`, so the two paths would
  diverge); `CODE_RIGHT_PAD = 0` fails the column test alone — while
  `highlighted_block_is_left_ragged` stays green through both, as a scope pin
  should. **1681 unit tests green** (+6), 70 `#[ignore]`, clippy
  `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean.
- **A live run isn't required** (AGENTS.md §3): this is the markdown renderer —
  no engine, memory, tool or provider path is touched (the precedent set by
  markdown-refinements and the mermaid work). The result was nonetheless checked
  against the **actual block from the report** (a rendered dump: 11 rows, all 44
  columns wide, every one carrying `REVERSED`).

### Post-M9: Zig code blocks are highlighted (done)

- **Reported from a screenshot**: a ` ```zig ` block rendered as flat text on the
  reverse-video rectangle while the ` ```rust ` block above it was coloured.
  Branch `fix/zig-code-highlighting` (a simple task by AGENTS.md §1: one table
  entry, no cross-layer contract, no new dependency — no design doc).
- **Cause, confirmed by probing the bundle rather than by reading the alias
  table**: `SyntaxSet::load_defaults_newlines` carries **75** syntaxes — the
  Sublime Text defaults — and Zig is not among them. So `resolve_syntax("zig")`
  returns `None` and `start_codeblock` takes the *unhighlighted* branch, which is
  exactly the reverse-video rectangle the previous entry squared off. Nothing was
  broken by that change: an unrecognized language has always landed there.
- **The same probe answered the wider question the report implies**: of ~60
  labels models commonly emit, **36 do not resolve** — `toml`, `dockerfile`,
  `powershell`, `swift`, `scss`, `graphql`, `terraform`, `asm`, `julia`,
  `solidity` and the rest. Zig is one instance of a standing limit, not a
  regression, and the doc comment now states the bundle's size and names Zig
  among the gaps.
- **Which grammar to borrow was measured, not guessed** (the table already has
  the pattern — `typescript → js`, `kotlin → java`): a representative Zig snippet
  was highlighted through the C, C++, Go, Java, JS and Rust grammars and the
  spans compared. **Rust wins**: it colours `const`/`pub`/`fn`, the call name,
  the numeric type names (`u8`/`usize` — spelled as in Rust), numbers, strings
  with `\n` escapes, `//` comments and the operators, missing only
  `try`/`defer`/`var`. Go is the runner-up and the interesting one — it is alone
  in catching `var`/`defer`, but loses `pub`/`fn`/the types **and** paints
  `while` with the function colour, i.e. it is actively misleading where Rust is
  merely silent. C++ catches `try` and little else.
- **Tests**: `zig`/`Zig` added to `language_aliases_resolve_to_syntax`, plus
  `zig_block_is_highlighted` — a behavioural test asserting the *symptom*, that
  the block carries RGB foreground colours and **no** `REVERSED` line style, so
  it pins the path taken rather than the table lookup. **Mutation-tested**:
  removing the alias fails it with the rendered rectangle in the message.
  **1682 unit tests green** (+1), 70 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean.
- **A live run isn't required** (AGENTS.md §3): this is the markdown renderer —
  no engine, memory, tool or provider path is touched (the precedent set by
  markdown-refinements and the previous code-block entry).
- **Groundwork**: real grammars for the missing languages. syntect can load
  `.sublime-syntax` YAML at runtime (the `yaml-load` feature is on by default),
  so vendoring a few definitions and extending the set once in the `LazyLock`
  would give true Zig/TOML/Dockerfile highlighting — an asset to license and
  maintain, and a separate track from this one-line fix.

### Post-M9: vendored syntax grammars for 19 languages (done)

- **Asked for right after the Zig fix**, which had closed the symptom with an
  approximation and named the real fix as groundwork. Plan with forks F1–F5 —
  [docs/history/vendored-syntaxes.md](../../docs/history/vendored-syntaxes.md)
  (**user's decision, 2026-07-31**, all four questions as recommended: the
  curated set, vendored files with a manifest, a build-time dump, no user
  overlay yet). Branch `feat/vendored-syntaxes`, stacked on the Zig fix (they
  collide by construction — this deletes the alias that fix added).
- **Everything load-bearing was probed against real grammars before any
  code**, and two of the findings decided the design:
  - **syntect loads only `.sublime-syntax`** — there is no `.tmLanguage`
    syntax loader (`plist-load` is for *themes*), which disqualifies several
    obvious upstreams (`PowerShell/EditorSyntax`,
    `Microsoft/TypeScript-Sublime-Plugin`, `wmertens/sublime-nix`).
  - **`extends:` is unsupported**, so every grammar must be self-contained.
    That is what rules out cherry-picking from the current `sublimehq/Packages`
    — its `TypeScript`/`TSX` extend `JavaScript` and fail to load, as does
    `alexlouden`'s `HCL`. Measured, not assumed: each produced its own error.
- **Cost, measured in release** — the number that chose F3: assembling the set
  at runtime is **130 ms** (24 unlink + 105 build) on the first code block,
  while a prebuilt **uncompressed** dump loads in **0.55 ms** — indistinguishable
  from the 0.57 ms the app already paid for syntect's defaults. The compressed
  form is 4.1 ms for 45 KiB less, so uncompressed wins; it is the same trade
  syntect makes for its own assets.
- **`build.rs` assembles the dump, and a grammar that fails to load fails the
  build.** That is not a slogan — it fired on the first run: the **Swift**
  grammar was rejected over a subroutine call (`\g<1>`). The cause was my own
  optimisation: I had given the build-dependency `regex-fancy`, reasoning that
  a dump stores regex *source* so the engine cannot matter. It does — loading a
  grammar **compiles** its regexes, so the build is also what validates them,
  and fancy-regex rejects what oniguruma accepts. The build-dep now uses
  `regex-onig`, the runtime's engine; validating with a different one would
  reject working grammars and could accept broken ones.
- **The set: 19 grammars** (Zig, TypeScript, TOML, Dockerfile, PowerShell,
  Swift, Kotlin, SCSS, Sass, GraphQL, Terraform, Elixir, Solidity, Julia, Nix,
  Dart, Protobuf, CMake, nginx) in `syntaxes/`, each pinned in `SOURCES.md` to
  an upstream repository, commit and licence, with the licence text vendored
  next to it. `tools/fetch_syntaxes.py` re-fetches from those pins;
  `--check` reports drift and is **not** in CI (it needs network — the
  grammars are vendored precisely so the build does not).
  `sharkdp/bat`'s `.gitmodules` was the shortlist source (a curated,
  syntect-verified list), and three files come from bat's own converted copies
  where no self-contained upstream exists — with the **original** author's
  licence recorded, not bat's.
- **The alias table had to be pruned, and that is a rule, not a cleanup**:
  `resolve_syntax` tries the canonical token first, so while `zig → rs`
  remained, the vendored Zig grammar was **never reached** — measured, `zig`
  still highlighted as Rust after the grammar was added. `typescript → js` and
  `kotlin → java` went the same way. What is left maps only what no grammar
  answers (`docker`, `pwsh`, `hcl`, `proto3`, `jsx`, `tsx`).
- **Binary size: +181 KiB**, measured on the release binary (20 397 056 →
  20 582 400). Worth recording because I had claimed the opposite in a code
  comment — that dropping syntect's `default-syntaxes` would make the binary
  *smaller* — and the measurement refuted it: under LTO the linker already
  drops the unused dumps. The feature is still off (it is genuinely dead
  weight), but the comment now states the measured number.
- **Attribution**: the "Components" tab (`F1`) gained a grammars section, built
  by **parsing the manifest** (`include_str!`) rather than duplicating it into
  a static list — drift is then impossible by construction, which beats a gate
  test that merely detects it. The gate that remains checks what a parser
  cannot: that every vendored file has a manifest row, and every row a licence
  text on disk.
- **Tests**: `vendored_grammars_resolve_by_their_own_label` (27 labels, the
  point being that a vendored grammar needs no alias);
  `dump_carries_the_vendored_grammars` (the dump is the bundled 75 **plus**
  every file — a guard against the build silently degrading to defaults);
  `grammar_manifest_matches_the_vendored_files`; the Components tab scrolled to
  its last page. **All three gates were mutation-tested** — re-adding `zig → rs`
  fails the first, deleting a grammar file fails the other two.
  **1686 unit tests green** (+4), 70 `#[ignore]`, clippy `-D warnings`/fmt/
  `cyrillic_scan`/`link_check` clean.
- **A live run isn't required** (AGENTS.md §3): the markdown renderer and a
  build step — no engine, memory, tool or provider path is touched. What
  replaces it here is that the build itself validates every grammar with the
  runtime's own regex engine, and the resolution tests assert the exact grammar
  each label reaches.
- **A process lesson worth recording**: `git checkout <file>` to undo a
  scripted measurement mutation reverted **all** uncommitted work in those two
  files, not just the mutation — Cargo.toml's feature trim and code.rs's whole
  change had to be redone from context. Back up the file, or apply the mutation
  as a patch you can reverse.
- **Follow-up, same branch: Vue, Svelte, Nim, V, JSONC** (asked for right after
  the track landed). Three vendored — the set is now **22** — and the other two
  answered by measurement rather than by adding files:
  - **V could not be vendored, and that is a licensing fact, not a preference**:
    the only `.sublime-syntax` for V that exists
    (`elliotchance/vlang-sublime`) has **no licence file and no statement in its
    README**, i.e. all rights reserved. §5 of the plan says such a candidate is
    dropped, so the label maps to **Go** — measured the best of go/rust/c on real
    V code (it shares `:=`, `import`, `struct`, the primitive type names,
    single-quoted strings, `//`). Rust catches `pub`/`fn`/`mut` but reads `'` as
    a lifetime and **mangles V's default string form** — worse than three plain
    keywords. Recorded in `syntaxes/SOURCES.md` under "not vendored".
  - **JSONC needed no grammar at all**: the bundled JSON grammar already
    highlights `//` and `/* */` as comments (measured), so `jsonc`/`json5` → `json`
    is exact rather than approximate. Worth checking before adding a file.
  - **Svelte repeated the SCSS lesson** — `master` fails to load on
    sublime-syntax v2 features, bat's pinned commit works. Vue's grammar lives
    on the `new` branch under a filename with a space (`Vue Component`), which
    is why `fetch_syntaxes.py` now quotes the path.
- **The follow-up produced the gate the original stages lacked**:
  `every_syntax_can_highlight_without_panicking`. Vue and Svelte embed other
  languages by scope, and an unresolved reference is **invisible at load time** —
  it surfaces only while parsing, where a panic would kill the app (we
  deliberately do not `catch_unwind`, see the mermaid module). Highlighting a
  mixed markup/script/style snippet through **every** syntax in the set closes
  that gap; all 100 pass, and Vue/Svelte/Nim were additionally eyeballed —
  embedded JS and CSS inside `<script>`/`<style>` really do highlight.
  **1687 unit tests green** (+1), gates clean.

### Post-M9: collapsible tool calls, and the collapse state per chat (done)

- **Asked for directly**: tool calls should fold away the way "thoughts" already
  do, be **collapsed by default**, and the collapsed/expanded state should be
  remembered **per chat** — for both kinds of block. Plan with forks C1–C3 —
  [docs/feed-collapse.md](../../docs/feed-collapse.md) (**user's decision, 2026-08-03**,
  both questions as recommended). Behaviour — spec §11.3. Branch
  `feat/feed-collapse`.
- **Reading the code decided the shape and made it small.** Per-chat UI state
  already has a playbook — `Chat.draft` (spec §11.7): a field on `Chat`
  (`#[serde(default)]`, **no migration**), an `AppCommand`, a field in
  `ChatActivated`, written with the save debounce and **without touching
  `modified_at`**. Everything here follows it, so the per-chat half needed no new
  mechanism, only a second traveller on an existing road.
- **One place it deliberately departs from that playbook**: the toggle returns
  `ChatIntent::SetFeedView` **straight from `handle_key`** instead of raising a
  dirty flag the loop picks up. `draft_dirty`/`take_dirty_draft` exists because
  typing is continuous and must not send a command per keystroke; a `Ctrl+O` press
  is discrete, so the flag machinery would be ceremony.
- **`FeedView { thoughts, tools }`, a named type rather than two bools** (C1): it
  travels through a command, an event, a chat field and the feed's render-cache
  key — a bare `(bool, bool)` is exactly the pair that gets swapped by accident.
  `Copy + Eq + Hash`, both `false` = collapsed, and
  `skip_serializing_if = "FeedView::is_default"` so a chat nobody expanded
  anything in writes no new key at all.
- **`Ctrl+O` for tools** (C2, `Ctrl+T` being taken). The candidates were narrowed
  by what the terminal itself claims — `Ctrl+I` is Tab, `Ctrl+H` Backspace,
  `Ctrl+M`/`Ctrl+J` Enter — leaving `o`/`l`/`d` free in both the chat screen and
  `InputBox::on_key`. `Ctrl+L` carries "clear screen" from shells and `Ctrl+D`
  reads as EOF on unix, so `Ctrl+O` ("output"), which has no prior meaning.
  Layout-independent through `keys::hotkey_char`, as every other Ctrl shortcut.
- **What a collapsed call looks like** (C3): the card's **header stays**
  (`⚒ name(args)` — *what* ran is the informative half) and only the argument and
  result blocks fold away; the header then carries the same pill the collapsed
  thoughts block uses (marker, label, keycap), appended to its **last row** rather
  than pushed as a line of its own, so a call is one line collapsed and one line
  of header expanded. Overflowing the width is safe — `build_message_block` wraps
  every body line afterwards. A call with **nothing to hide** (no arguments, no
  result yet) gets no pill, mirroring `push_thoughts` on empty thoughts: promising
  something behind a pill that hides nothing is the small lie this project keeps
  finding and closing.
- **A test of mine was wrong about the code, and the code was right.**
  `tool_card_is_collapsed_by_default` first asserted that a short argument
  disappears — it doesn't: the presenter (`features/tools/present.rs`) puts a
  short scalar into the **header suffix** (`note_save(секрет)`) and only <!-- cyrillic-ok -->
  multi-line/long values into a block. That is the collapsed card's one-line
  summary working as designed, so the assertion moved to a multi-line
  `python_exec` argument, which is genuinely a block.
- **Tests**: collapsed by default (header kept, arguments and result gone, pill
  present, **exactly one row per call**), the toggle revealing them and hiding
  them again (and dropping the pill when expanded — the `⚒` header is the marker
  then), no pill when there is nothing to reveal, the pill localized across every
  built-in locale; the two keys reporting the whole new view and staying
  independent (plus `Ctrl+щ`, the physical `O`); activation applying the chat's
  stored state; and at the orchestrator level the write rules (flagged for saving,
  `modified_at` untouched, an identical state a no-op) plus a full round trip
  through the real `run` loop — expand in chat A, a new chat B opens collapsed,
  switching back restores A, and the choice is still on disk after a restart. The
  existing `cache_matches_fresh_render` gained the tools flag, so the cache is
  checked against all four combinations, and the entity has its own serde test for
  the no-migration claim, and one pinning both formerly-hardcoded labels across
  locales, plus nine for the expanded layout. **1833 unit tests green**
  (+19), 76
  `#[ignore]`, clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check`/i18n gates
  clean.
- **All seven load-bearing behaviours were mutation-tested** — never collapsing,
  dropping the "nothing to hide" guard, taking the state out of the cache key,
  bumping `modified_at`, activation ignoring the stored state, the key not
  reporting it back, and dropping `skip_serializing_if` — each fails its own test
  and only that one. (One mutation landed on the *first* `if !expanded` in the
  file, which is `push_thoughts` — so the thoughts tests got audited for free.)
- **A live run isn't required** (AGENTS.md §3): rendering, key handling and a
  per-chat field — no engine, memory, tool or provider path is touched. Two
  things were nonetheless checked against reality rather than reasoned about: the
  collapsed and expanded forms were **dumped side by side and read** before the
  tests were written (which is what showed the pill sits on the header rather
  than needing a line of its own), and a throwaway probe ran **all 182 chat files
  of the real dev data root** through the new type — every one loads as collapsed
  and writes back **without** a `feed_view` key, i.e. the additive claim holds on
  actual user data, not just on a fixture.
- **The `git checkout <file>` trap bit for a third time** (already recorded twice
  in this journal): reverting a mutation that way discarded the *whole*
  uncommitted change in `input.rs`, not just the mutated line, and the suite
  stayed red until it was re-applied by hand. Back the file up first — `cp` — or
  commit before mutating.
- **Two unlocalized strings found next door and fixed on the user's say-so**
  (they were spotted here and first recorded as groundwork; the user asked for
  them in the same change): the **expanded** thoughts label
  (`format!("{} мысли", …)`, while the collapsed pill correctly used <!-- cyrillic-ok -->
  `ui.feed.thoughts`) and the console exit-code label in `push_console`
  (`"код возврата: {code}"`). Both showed Russian under an `en` interface. <!-- cyrillic-ok -->
  - **The exit-code label got its own key rather than reusing
    `python.console.exit`**: that one is **axis A** — the label the *model* reads
    inside the tool result, written in the profile's language — while the feed
    re-renders it for the *human* after `present::parse_console` has stripped it,
    which is **axis B**. Same text today, different axis, so a Russian interface
    reading an English-profile chat now says "код возврата: 3" rather than <!-- cyrillic-ok -->
    inheriting the agent's language. New key `ui.feed.exit_code`; `push_block`/
    `push_console` gained the locale to reach it.
  - **Why the gate missed them, checked rather than assumed**: `cyrillic_scan.py`
    sets `in_test` on the first `#[cfg(test)]` it sees and **never unsets it**,
    and `message_feed.rs` has `#[cfg(test)]` **test accessors** at line ~564 — so
    every production line below them was scanned as test code, where Cyrillic in
    code position is legitimate fixture data. The strings are fixed; the
    scanner's blind spot is recorded in the roadmap (an attribute on a single
    item shouldn't mean "the rest of the file is tests").
  - Pinned by `expanded_thoughts_and_exit_code_are_localized`, which renders an
    **English-profile tool result under a Russian interface** and vice versa, and
    asserts no Cyrillic leaks into an `en` feed. Mutation-tested: restoring
    either literal fails it.
- **The collapsed pill reads like the thoughts pill** (asked for after the first
  live look): `▸ детали · Ctrl+O` against `▸ мысли · 9 стр. · Ctrl+T` — the <!-- cyrillic-ok -->
  separator is explicit here, since the thoughts pill gets its own from inside
  `ui.feed.thoughts_lines` and this pill has no count to show. The first
  mutation run **survived** — nothing asserted the separator — so
  `tool_card_is_collapsed_by_default` gained the assertion, which then failed
  under the same mutation.
- **An expanded card is a different presentation, not a longer one** (fork C4,
  the layout specified by the user after the same live look; collapsed
  unchanged): the tool's **name alone** in the header, every argument enumerated
  below one per line, a gap row, then the result. The gap **keeps the card's `│`
  gutter** (a second round of feedback): drawn blank it read as the end of the
  card rather than as a break inside it. `│` is WGL4, so compatibility mode
  needs no substitution — it is already the card's gutter.
  - **Why it was needed**: the header is a *title* — `truncate_header` flattens
    whitespace and cuts at `HEADER_MAX_CHARS = 100`, and `scalar_str` drops
    anything that is not a scalar. So a long query ended in `…` and an
    **array/object argument never appeared at all, in either mode**. That second
    half was the more interesting find: `set_sampling`'s `samplers`/
    `dry_sequence_breakers` and any MCP tool with structured arguments were
    simply invisible.
  - **`ArgDetail::{Compact, Full}` on the presenter**, split into
    `compact_args`/`full_args` rather than one function with a flag threaded
    through it — they are genuinely different presentations. It belongs there and
    not in the widget: which field is code, which is large, what may be folded
    into a header is knowledge `features/tools/present.rs` already owns, and the
    widget stays generic (spec §11.3).
  - **A value that cannot share a line with its key** — code, a large or
    multiline string — goes under a `key:` label as its own block, so
    `python_exec`'s code keeps its highlighting *and* is still named. Field order
    is `serde_json::Map`'s (alphabetical): the wire format does not preserve the
    model's own order for us, and stability is what matters for something read
    repeatedly. The doc comment says so, after checking `Cargo.toml` for
    `preserve_order` rather than assuming arrival order.
  - **My first attempt was conditional** — keep the compact header, and add the
    missing detail only when it truncated or dropped something. The user replaced
    it with the layout above, and was right: the conditional version made a
    card's structure depend on how long its values happened to be, so two
    neighbouring calls could look different for no visible reason.
  - The confirmation popup (§9.8) keeps `Compact` — a decision prompt, not a
    viewer, which is the decision the journal already recorded for it.
  - The existing presenter tests describe the compact form, so they got a
    **shadowing `present`** in `mod tests` (the `calc`/`datetime` precedent) —
    zero call-site churn, and `Full` has its own seven.
  - **A probe over realistic calls came before the tests**, and it corrected two
    fixtures I would otherwise have written wrong: a truncation needs **two**
    medium values whose join overflows (any single scalar over the ceiling is
    `is_big` and becomes a block instead, so a lone long value never reaches the
    truncation), and one widget test anchored on "the first row containing `1`",
    which the argument listing now matches before the result does.
  - Five mutations, each failing only its own test: `Full` falling back to the
    compact header, the gap row removed, the gap appearing with no arguments, a
    code argument losing its label, and the gap drawn blank instead of guttered.
  - One more sloppy anchor caught by its own failure: the gap test first looked
    for "the row after the last argument" by name, but the listing is
    **alphabetical**, so the name it picked was the *first* argument. Anchored on
    the result row instead.

### Post-M9: navigable `chat://` references in the feed (done)
- **The idea arrived from a live run, not from a design.** With the cross-chat
  pair enabled (spec §9.11), grok-4.6 started citing conversations as
  `chat://<short-id>` — a scheme nothing in the repository mints or mentions,
  invented out of the bracketed address `chat_search` printed. The roadmap item
  that followed asked only for the second half ("recognize the reference and
  resolve it"); the code survey moved its centre of gravity, and the design
  ([chat-uri-links.md](../research/chat-uri-links.md)) is in two halves because
  of it. Forks F1/F3/F4/F7 decided by the user 2026-08-14, F2/F5/F6 carried
  their recommendation.
- **What the survey found, and why the first half exists.** Three gaps, none of
  them where the roadmap looked:
  - Nothing taught the scheme, so the feature rested on one model's habit. A
    different model writes `[a1b2c3d4]`, "chat 3", or the title — and the
    renderer would light up nothing at all.
  - A **bare** `chat://…` is not even styled: the parser runs without GFM
    autolinks (`ENABLE_STRIKETHROUGH|TASKLISTS|MATH|TABLES`), so only the
    markdown form `[Title](chat://id)` got colour, and only on the URL suffix
    `end_link` appends. There is no `Link` span kind and no source→screen offset
    map — the latter structurally unobtainable, as the feed's own doc comment
    records.
  - A click in the feed is a no-op, and mouse capture (`Ctrl+W`) is **off by
    default** because turning it on costs native terminal selection. So the
    "clicking it goes nowhere" in the roadmap was understating it: clicking
    anything in the feed goes nowhere, and a click could not be the way in.
  - Plus a latent defect the first half had to fix on its way past:
    `chat_read("chat://a1b2c3d4")` failed **its own scheme** — `resolve` stripped
    only `-` before the hex test, so the address fell through the id rung into
    title matching and answered "unknown". Teaching the model to write `chat://`
    without this would have made every round trip fail.
- **One address, one producer.** `features/chat_links.rs` holds the scheme:
  `uri`/`short_id` mint it, `hex_needle` reads it back (scheme stripped,
  dashes dropped, 4–32 hex — the floor is half a short id, below which
  "cafe"-shaped words shadow titles), `resolve_prefix` resolves it, `find_refs`
  scans rendered text. `chat_search`/`chat_read` now print `chat://a1b2c3d4`
  instead of `[a1b2c3d4]` (fork F7) and their descriptions ask the model to cite
  that form **when it mentions a conversation to the user** — "as needed", not
  as decoration. Teaching costs nothing while the pair is off: a disabled tool
  is not advertised at all.
- **Detection runs in the block builder, before the wrap** (fork F3). The
  obvious home was the post-render pass the in-feed search highlight uses, and
  it has a hole this feature would fall into: an address is 15 columns
  (`chat://` + 8 hex), a narrow panel splits it across rows, and that pass
  matches per line. Building it earlier also puts the result in the cache, which
  is correct here and wrong for search: whether a reference is a link is a
  property of the content and the address book, not of a query being typed. So
  the address book's fingerprint joins `CacheKey` — the `role_names` precedent —
  while the search query stays deliberately out of it.
- **Only what resolves is drawn as a link.** The address book is the current
  profile's non-hidden chats, the open one included, derived from the chat-list
  snapshot the screen already keeps (`ChatSummary` gained `profile_id`, fork
  F5) rather than from an event of its own — so the profile boundary cannot
  drift from the list the user is looking at. An unknown id, or another
  profile's, stays plain text. That single rule answers the roadmap's open
  question ("what should a reference to another profile's chat do") and keeps
  the UI from ever offering a door onto nothing (lessons §4).
- **`Ctrl+L`, not a click** (fork F4). The picker lists the conversations the
  chat links to — read from the block cache, so it is exactly what is drawn —
  newest block first, deduplicated, title + date, modelled on the profile
  picker. The mouse is stage 2. The two degenerate cases are told apart: no
  references at all says what a reference *looks like* rather than opening an
  empty list, and a reference back to the open conversation says so rather than
  doing nothing. Following one is `AppCommand::SwitchChat` — an address names a
  conversation, not a message — so the search back-stack needs no new variant
  (fork F6 recorded the alternative if a live run says otherwise).
- **A shared seam rather than a second copy**: `highlight_line`'s span
  re-splitting became `restyle_ranges(line, text, ranges, patch)`, and the link
  pass supplies its own patch (accent + `UNDERLINED`). One mechanism, two
  callers — the shape the Sonar duplication gate has punished three times in
  this repo when it was not done up front (lessons §2).
- **Tests**: 11 on the scheme (both forms, case, trailing punctuation and the
  markdown form's closing paren, a full uuid, non-ASCII neighbours, another
  scheme left alone, unresolvable → nothing), 6 on the feed (styling, the
  narrow-panel case that motivates F3, dedup/order, cache invalidation, plain
  text for an unknown id), 4 on the picker, 5 on the screen (the `Ctrl+L`
  flows, the profile boundary, the picker closing on a chat switch), 2 on the
  tools (round-tripping the scheme, every printed address carrying it). Suite
  **2181 → 2209**.
- **Stage 2 — the click** (same branch, at the user's call). `MessageFeed`
  still stores no `Rect`: the click map is **derived in `render`, from the rows
  about to be drawn, for the viewport only**, in absolute terminal cells. That
  is the one point where the second wrap, the scroll and the panel's origin have
  all been applied, so nothing stored can go stale — and it sidesteps the
  `InputBox::last_area` shape the design had pencilled in, which would have had
  to re-derive all three at click time. Cost is bounded by the screen (tens of
  rows), not the conversation; a test pins the map at absolute column 3 (border
  + rail), which is exactly the off-by-two that would otherwise ship unnoticed.
  `handle_mouse` now returns an `Option<ChatIntent>` — it had returned `()`
  since the mouse existed, because nothing in the feed had ever been actionable
  — and the reference is tried before the input box, which cannot compete: they
  own disjoint areas and `mouse_press` already answers `false` outside its own.
  One degradation is stated rather than hidden: an address the wrap split
  across two rows is styled but not clickable, and `Ctrl+L` stays the route that
  always works. **No live run** (AGENTS.md §3): pure UI, no engine, memory or
  tool path touched. +7 tests, suite **2209 → 2216**.
- **Smoke — GO** (gemma-4-31B q4_0 + bge-m3 via llama-server, the user's live
  stack). "The model uses the format" is a behavioural claim no unit test can
  settle, so the §9.11 go/no-go was extended: the answer must now also carry a
  `chat://` address that `find_refs` resolves to the seeded conversation
  (`narrow_profile_to` returns the bootstrap chat id, so the assertion is exact
  rather than "not the current one"), and the prompt asks *which* conversation
  the fact came from instead of "answer with the code alone", which suppressed
  the citation by construction. The model called `chat_search` once and answered
  with the seeded code plus the conversation named as `chat://cd1d3e13` — the
  address form, unprompted beyond the tool descriptions, on a **local** model
  rather than the one that invented the scheme. That is the evidence the first half needed: the habit was
  transferable, and teaching it is what makes it so. Full orchestrator e2e
  regression (the change touches every turn's tool descriptions and the feed's
  build path) — **34/34 in 827 s**, no repeats needed.
