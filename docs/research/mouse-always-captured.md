# Research: mouse capture always on, and a text selection of the app's own

**Status:** research and a pty probe (2026-11-14). **Forks decided by the owner
the same day** (§6), in two rounds: F1 with the legacy terminal mode, F2, F4,
F5, F7, F8, F9 and F10 as recommended; F3 and F6 the other way — the right
button does nothing, `Esc` keeps its meaning, the feed takes no selection while
the assistant answers, and a copy never ends a selection. **Stage 1 is built**
(2026-11-14): the input path (§5.2, `fix/mouse-input-path`; §3's probe re-run) and
the second half (§5.1, `feat/mouse-capture-setting`) — the *Mouse* setting,
`terminal` by default, the legacy mode's veto, button-and-drag capture following
the screen, the link on the release, the copy chip and `Ctrl+C` keeping the
field's selection ([docs/journal/ui-input.md](../journal/ui-input.md)). **Stage 2's
first half is built** (2026-11-15, `feat/feed-selection`): §5.3–§5.6 — the row
facts, the feed's selection and its grammar, the highlight, the text, the multi-click
and the autoscroll, the box's clamped drag and its multi-click. Its second half, §5.7
— the toggle removed and the default flipped to `app` — is next; stage 2b (the
prefixes) after it.

The ask (the owner, 2026-11-14): look into dropping the mouse's two modes —
*select* and *scroll* — altogether. Keep the terminal's mouse capture on all the
time, and give the feed and the input fields a text selection of the app's own.
The wheel is needed often, and the terminal's own selection does not do the job
anyway: it takes the interface's elements — the rail, the panel's borders, the
scrollbar — together with the text inside them.

Related: spec §11.3 (scrolling, `chat://` clicks, the input box's mouse), §11.5,
§11.7, §4.4.1 (the change-driven loop);
[docs/history/input-selection-undo-mouse.md](../history/input-selection-undo-mouse.md)
(stage D, and its Fork 5, which tied the field's mouse to `Ctrl+W`);
[docs/journal/ui-feed.md](../journal/ui-feed.md), "mouse-wheel feed scrolling"
(why capture became a toggle); [chat-uri-links.md](chat-uri-links.md) (the
feed's click map); [docs/history/osc52-clipboard.md](../history/osc52-clipboard.md)
(copying over SSH); the roadmap's *Per-message collapse, select and copy*.

## 1. Summary

- **Feasible, and more than half of it is already built.** The input box has had
  a text-anchored selection driven by the mouse since stage D of the selection
  track; the feed rebuilds a click map in absolute cells every frame (the
  `chat://` links); a copy already goes to the local clipboard and, over SSH, to
  the terminal's own over OSC 52; a finished frame already passes through
  post-render passes, and the monochrome mode already keeps a selection marker.
  What is missing is the feed's half: a selection anchored to the feed's rows, a
  highlight drawn outside the block cache, and a copy that knows which cells are
  chrome and which rows are a soft wrap. Both facts are known at the moment a row
  is made (§2.4) — the cache just does not keep them.
- **What it buys beyond the wheel.** A copy without the rail, the borders and the
  scrollbar; a paragraph copied as one line instead of broken at the panel's
  width; a selection that **scrolls** — the terminal's own cannot reach past the
  visible screen of an app on the alternate screen, so today a long answer cannot
  be selected whole at all; and `Ctrl+W` goes away, the key a browser tab closes
  the tab on (spec §11.7).
- **What it costs: the terminal's own mouse inside the window** (§4.1–§4.2).
  Its selection needs a modifier — `Shift` in Windows Terminal (over SSH too), VS
  Code on Windows and Linux, GNOME Terminal, Konsole, kitty, Alacritty, WezTerm;
  `Option` in iTerm2, `Fn` in Terminal.app — and **the classic Windows console
  (conhost) has none**, nor does VS Code on macOS by default: there the app's own
  selection and a setting are what is left. The right button becomes the app's
  (`Shift`+right-click is still Windows Terminal's paste). `Ctrl`+click on a link
  keeps working in Windows Terminal.
- **Others have done it** (§4.5) — tmux, Zellij, Textual, Codex (ratatui, the
  nearest neighbour) and Claude Code's full-screen mode all capture and select
  for themselves. Their trackers say what not to ship: a copy with the wrap's
  newlines and the padding in it, a selection that cannot scroll, a "copied" that
  was only attempted, an autoscroll that loses text.
- **Found on the way, measured (§3): two defects that hide behind the toggle's
  default today** and would reach every user once capture is permanent. The
  reader that gathers a Windows paste holds a stream of mouse reports as one
  batch: when reports arrive two to a read — a slow frame or an SSH link is
  enough — a drag or a fast wheel draws **no frame until the mouse rests**, and a
  key typed meanwhile is drawn **~0.65 s late**. And every plain movement of the
  pointer is a full frame. Both are a few lines in the input path (§5.2). A third,
  older one: a copy over OSC 52 reports itself with a feed note, and a note that
  lands while a reply streams splits the reply (docs/lessons.md §5).
- **The design (§5).** Capture in button-and-drag mode only, no motion reports;
  one selection on the screen at a time, owned by the feed or by a field; the
  feed's anchored to (block, row, column), drawn over the frame the way the click
  map is read off it, and not offered while the assistant answers. `Ctrl+C`
  copies and never ends a selection, the outcome on a status-bar chip rather than
  in the feed; a double click takes a word, a triple a line; a drag past the edge
  scrolls; the right button and `Esc` stay out of it. `Ctrl+W`, its status-bar light and
  `/mouse` are removed; a setting on the settings screen, and the legacy terminal
  mode by itself, keep the terminal's mouse where it is needed.
- **Stages (§7):** the input path first (it fixes today's scroll mode too), then
  the feed's selection with the default flipped — the bulk of the work, in
  `widgets/message_feed.rs` and the chat screen — then the other screens' fields
  and the wheel in lists. The go/no-go for the terminal side costs no code: the
  owner's `mindfork keys` session in Windows Terminal over SSH (§7, P0).

## 2. What the code does today

### 2.1 The toggle

- Capture is **off at start** (`app/runtime/mod.rs:413`). `Ctrl+W` or `/mouse`
  flips `ChatScreen::mouse_scroll` (`screens/chat/input.rs:228`) and returns
  `ChatIntent::SetMouseCapture`, which `dispatch` executes as crossterm's
  `EnableMouseCapture` / `DisableMouseCapture` (`app/runtime/dispatch.rs:996`).
  The status bar names the mode (`mouse: select` / `mouse: scroll`), draws the
  description in the accent while scrolling and pins the hint first in that mode
  (`widgets/status_bar.rs`). Exit and the panic hook lift capture.
- crossterm 0.29's `EnableMouseCapture` on unix writes `?1000h ?1002h ?1003h
  ?1015h ?1006h` — **any-motion tracking (1003) included**, so every movement of
  the pointer is a report (`crossterm/src/event.rs:325`). On Windows it sets the
  console input mode to `ENABLE_MOUSE_INPUT | ENABLE_WINDOW_INPUT |
  ENABLE_EXTENDED_FLAGS` (QuickEdit off, `event/sys/windows.rs:14`), and the
  console reports every movement as a `MOUSE_MOVED` record, which crossterm turns
  into `Moved` or `Drag` (`event/sys/windows/parse.rs:334`).

### 2.2 Who reads the mouse

- **Only the chat.** `process_input_batch` hands mouse events to the chat screen
  and drops them on every other screen (`app/runtime/input.rs:254`); the help
  overlay swallows them, being modal.
- `ChatScreen::handle_mouse` (`screens/chat/input.rs:697`): the wheel scrolls the
  feed by `WHEEL_SCROLL` = 3 rows; a left **press** on a `chat://` address follows
  it, anywhere else it goes to the input box's `mouse_press`; a left drag goes to
  `mouse_drag`. A release, the right and the middle button do nothing. Any open
  overlay (in-feed search, spelling suggestions, emoji, links, local servers, the
  profile picker, a confirmation) makes it a no-op.
- The loop marks the screen dirty on **any** terminal event
  (`app/runtime/mod.rs:1172`): every report, a plain movement included, is a
  frame.

### 2.3 The input box

- `InputBox` keeps a text-anchored selection (an anchor plus the cursor):
  keyboard extension, `Ctrl+A`, copy and cut, and since stage D the mouse —
  `mouse_press` puts the cursor and the anchor at the pointer, `mouse_drag` moves
  the cursor (`widgets/input_box.rs:861`), both through `place_cursor_at`, which
  inverts the wrap layout over `last_area` and the wrap cache (:824).
- **Gaps.** A drag that leaves the text area does nothing (`place_cursor_at`
  answers `false` outside it), so a selection stops growing at the field's edge
  and the field never scrolls under a drag; there is no double or triple click;
  and only the chat's box is reached at all — rename, the settings editors, the
  self-model editor and the search bars never see a mouse event.

### 2.4 The feed

- **Rows.** A message's block is built at the panel's width minus the rail's two
  columns: markdown becomes logical lines, each wrapped by `wrap::wrap_line` and
  given the rail (`widgets/message_feed.rs:1421`); blocks are cached per message.
  `render` concatenates every block's rows each frame (:1187), wraps them again
  (identity by construction) and draws one `Paragraph` with a scroll offset
  (:1041).
- **What a copy must leave out is known exactly where the rows are made.** The
  rail is the first span of a railed row; the separator under a message and the
  compaction divider are railless; every row past the first of one logical line
  is a soft wrap. And `wrap_ranges` keeps the whitespace at a break at the end of
  its row (`shared/wrap.rs`), so concatenating a line's soft rows rebuilds it
  verbatim — the join needs no guessing about spaces. None of this survives into
  the cache: a cached row is a bare `Line`.
- **Rows wrapped before the feed's wrap** carry a prefix on every row and no mark
  of which rows continue a line: a quote's `> ` (`wrap_hanging` in the markdown
  writer), the `│ ` gutter of the thoughts in the monochrome mode, the indent of a
  tool card's arguments and result (`push_gutter_lines`, `push_wrapped`).
- **The click map** (`link_hits`) is derived from the rows about to be drawn, in
  absolute terminal cells, every frame (:1038) — the one point where the wrap, the
  scroll and the panel's origin have all been applied. A selection needs exactly
  that: the frame the user pointed at, not a stored assumption about it.

### 2.5 Copy

- `ChatIntent::CopyToClipboard` reaches `copy_text` (`app/runtime/clipboard.rs`):
  `arboard` locally, then OSC 52 per `interface.osc52` (auto: when the session
  looks remote or the local copy failed). A plain local copy is silent; an OSC 52
  send, a text too large for it and a failure each push a **feed note**
  (`app/runtime/input.rs:344`).
- `F5` / `/copy` copy the whole conversation (its source text). There is no way
  to copy part of a message today except the terminal's own selection.

## 3. Measured (2026-11-14): the input reader under a stream of mouse reports

**Method.** `mindfork demo` (a throwaway root and the scripted engine — no user
data touched) in a pty, 120×36, `TERM=xterm-256color`; the debug build of
2026-11-12, whose input batching and mouse paths are those of `main` at 1df3d5d8
(the two commits since change neither — an `Alt+V` key and one line of
`dispatch`). `Ctrl+W` turns capture on, then a script
writes SGR mouse reports at a fixed interval and timestamps every frame the app
starts (each frame is bracketed by synchronized output, `ESC[?2026h`). The probe
lives in the session's scratchpad, not the repository.

| Stream | Reports per read | Interval | Frames while it ran | After the last report |
|---|---|---|---|---|
| left drag, 100 reports | 1 | 8 ms | 100 | — |
| plain motion (no button), 100 | 1 | 8 ms | 100 | — |
| wheel, 60 | 1 | 10 ms | 60 | — |
| wheel, 10 | 1 | 60 ms | 10 | — |
| left drag, 100 | **2** | 16 ms | **0** | one frame, 20 ms later |
| plain motion, 100 | **2** | 16 ms | **0** | one frame, 20 ms later |
| wheel, 60 | **2** | 20 ms | **0** | one frame, 20 ms later |
| plain motion, 100, and `a` typed at 160 ms | **2** | 16 ms | **0** — `a` drawn ~0.65 s after it was typed | |
| `a` alone (control) | — | — | drawn at once | |

**Cause.** `read_batch_from` (`app/runtime/mod.rs:1215`) takes a drain of two
events or more (`PASTE_BURST` = 2, `app/runtime/input.rs:9`) for a Windows paste
— which arrives as a burst of key events — and keeps reading while events come
less than `PASTE_GAP` = 20 ms apart (:17). It counts **every** event, mouse
reports included. One report per read never trips it: each wakes the loop alone.
Two in one read do, and then a stream at the pointer's rate, far under 20 ms
apart, keeps the chase going until the pointer rests: one batch, one frame, at
the end. Two reports reach one read whenever a frame takes longer than the gap
between two reports (a long chat, a debug build), or the transport hands them
over together — packets bunch up on a real network link, and an SSH session is
the owner's daily setup. The key's delay is the same batch: it waits inside
it.

**And the frames themselves.** The plain-motion stream drew 100 frames for 100
reports that changed nothing on screen. `?1003h` is what asks for those reports
on unix; the Windows console sends them regardless. A frame repositions the
cursor, and the loop is change-driven precisely because Windows Terminal
restarts the cursor's blink on every move (spec §4.4.1).

Today both hide behind the toggle's default (capture off); permanent capture
would make them everyone's. They are fixed in the input path, §5.2, and the fix
is worth landing first because it repairs today's scroll mode as well.

## 4. What the terminals do

Surveyed from the sources at their heads of 2026-11-13/14 — Windows Terminal
(`microsoft/terminal` f6de9e0), xterm.js, kitty, WezTerm, Alacritty, VTE,
Konsole, tmux, Zellij, Codex, JediTerm — plus their documentation and trackers.
The claims a fork rests on (Windows Terminal's, conhost's, tmux's, Codex's) were
read again in the code by hand.

### 4.1 The terminal's own selection while the app holds the mouse

| Terminal | Hold | Notes |
|---|---|---|
| Windows Terminal | `Shift` | Hard-coded ([`_canSendVTMouseInput`](https://github.com/microsoft/terminal/blob/f6de9e0618015def1c9dda5b82f9d3abf03eb799/src/cascadia/TerminalControl/ControlInteractivity.cpp#L748)); the same for a native app behind ConPTY and over SSH. |
| conhost, the classic console window | **nothing** | `Shift` skips only the VT mouse path; crossterm reads the mouse through the console API, whose records reach the app whatever is held ([`windowio.cpp`](https://github.com/microsoft/terminal/blob/f6de9e0618015def1c9dda5b82f9d3abf03eb799/src/interactivity/win32/windowio.cpp#L567)). The window menu's *Edit → Mark* still selects. |
| VS Code's terminal | `Shift` on Windows and Linux | On macOS `Option`, and only with `terminal.integrated.macOptionClickForcesSelection` on — off by default ([xterm.js](https://github.com/xtermjs/xterm.js/blob/f70764dd3da05ebda0390605c6f9adbbea3f0e90/src/browser/services/SelectionService.ts#L437)). |
| GNOME Terminal (VTE), Konsole, kitty, Alacritty, WezTerm, JetBrains | `Shift` | WezTerm: `bypass_mouse_reporting_modifiers`. |
| iTerm2 | `Option` | |
| Terminal.app | `Fn` | Users' reports; Apple documents `⌘R`, which turns mouse reporting off for the window. |
| tmux | the outer terminal's | tmux hands the drags to the app; the outer terminal's selection takes its whole screen — across panes and the status line. |

### 4.2 What else changes, in Windows Terminal

- **`Ctrl`+click on a link still opens it.** The terminal looks for a hyperlink
  before it sends a mouse report ([GH#9396](https://github.com/microsoft/terminal/blob/f6de9e0618015def1c9dda5b82f9d3abf03eb799/src/cascadia/TerminalControl/ControlInteractivity.cpp#L295)),
  for an OSC 8 link and for a URL it detected, and the app never sees that click.
- **The right button is the app's; `Shift`+right-click stays the terminal's**: it
  copies the terminal's own selection — **joined into one line**, `singleLine`
  follows `Shift` ([L376](https://github.com/microsoft/terminal/blob/f6de9e0618015def1c9dda5b82f9d3abf03eb799/src/cascadia/TerminalControl/ControlInteractivity.cpp#L376))
  — or pastes when there is none. A multi-line copy of the terminal's own
  selection takes `Ctrl+Shift+C`, `Enter` or `copyOnSelect`.
- **Its copy key, with no selection of its own, passes through to the app**, so
  the app's `Ctrl+C` arrives. Codex leaves copy-on-select off under Windows
  Terminal for exactly that reason ([`local_settings.rs`](https://github.com/openai/codex/blob/65a5410f72dac1f4b40862c34ef0a8b71b12834a/codex-rs/tui/src/local_settings.rs#L266)).
- **One wheel report per notch** (120 units of delta to a line); VS Code the
  same. `WHEEL_SCROLL` = 3 rows per report holds.

### 4.3 A wheel without capture: alternate scroll (`?1007`)

On the alternate screen with capture off, many terminals turn the wheel into
↑/↓ key presses — Windows Terminal **by default**, one arrow per notch (its input
modes start with `AlternateScroll`, [`terminalInput.hpp`](https://github.com/microsoft/terminal/blob/f6de9e0618015def1c9dda5b82f9d3abf03eb799/src/terminal/input/terminalInput.hpp#L182)).
Today's select mode receives exactly that: the arrows move the cursor of a
multi-line draft and step the in-feed search's matches, and otherwise do
nothing (`InputBox::move_up`/`move_down` stop at the edge rows). xterm.js,
kitty and WezTerm always do it; VTE, Konsole and Alacritty by default; iTerm2
only after the app sends `?1007h`; conhost not for a console-API client; tmux
3.6+ with `mouse on` not at all.

It is not the answer: an arrow from the wheel cannot be told from the key, so
the feed could only scroll this way by taking ↑/↓ from the input box at its edge
rows — and the second half of the ask, the chrome in a copy, stays where it is.
Set aside.

### 4.4 OSC 52 — the copy over SSH

| Terminal | Writes | Reads |
|---|---|---|
| Windows Terminal | yes (`compatibility.allowOSC52`, on), while focused | dropped |
| kitty, WezTerm, Alacritty | yes | kitty asks; Alacritty off by default; WezTerm no |
| VS Code | since 1.91; multi-byte text reported corrupted in 1.123–1.124, fixed in 1.125 | answered |
| iTerm2 | **off by default** (*Applications in terminal may access clipboard*) | asks |
| Konsole | write-only, since 24.12 (unverified) | no |
| GNOME Terminal (VTE), Terminal.app | **no** | no |
| tmux | an app's sequence only with `set-clipboard on` — the default `external` refuses it; a DCS-wrapped one only with `allow-passthrough on` (default off) | `get-clipboard` (3.7) |

Two consequences here:

- The copy path already says *sent to your terminal*, never *copied*, for an
  OSC 52 send — the false toast opencode is reported for (#59664) cannot happen,
  and the chip of §5.6 keeps that wording.
- **Inside tmux with its defaults the app's OSC 52 goes nowhere**:
  `osc52::sequence` wraps it in DCS passthrough, which tmux drops unless
  `allow-passthrough` is on ([`options-table.c`](https://github.com/tmux/tmux/blob/75fac4a77bb40dc655f8b217071c224c0c819c7f/options-table.c#L1395)),
  and `set-clipboard external` refuses an unwrapped one. `tmux load-buffer -w -`
  reaches the outer clipboard under the defaults (tmux 3.2+; Codex uses it). The
  OSC 52 path's gap rather than this track's — but a routine copy from the feed
  meets it more often (§8).

### 4.5 Prior art

- **tmux** (`mouse on`): a selection of one pane's cells, soft wraps rejoined,
  copied on release; a double click takes a word, a triple a line; a drag on the
  pane's top or bottom row scrolls a line every 50 ms.
- **Zellij**: a pane's cells including its scrollback, wrapped rows rejoined; copy
  on select by default, over OSC 52; `Shift` for the terminal's own.
- **Textual** (2.0.0, February 2025, [PR #5409](https://github.com/Textualize/textual/pull/5409)):
  the widgets' underlying text, across widgets; `Ctrl+C`, over OSC 52 only; a
  double click takes a widget, a triple its container; autoscroll at the edges.
- **Codex CLI**, ratatui — the nearest neighbour. In July 2025 it left the
  alternate screen for the terminal's normal scrollback to give the terminal its
  selection back ([PR #1672](https://github.com/openai/codex/pull/1672)); since
  December 2025 it has a full-screen transcript of its own again
  ([PR #7601](https://github.com/openai/codex/pull/7601)): a selection anchored to
  the content with the gutter left out, logical lines across the visual wrap, a
  double and a triple click (400 ms, the same cell,
  [`text_selection.rs`](https://github.com/openai/codex/blob/65a5410f72dac1f4b40862c34ef0a8b71b12834a/codex-rs/tui/src/text_selection.rs)),
  copy-on-select chosen per terminal, the right button copying — and a copy that
  rebuilds balanced markdown or HTML from source positions its renderer keeps
  (`transcript_view/text.rs`, `markdown_copy_tests.rs`). That last part is the
  high end, and a track of its own here (F4).
- **Claude Code**'s full-screen mode (2026,
  [docs](https://code.claude.com/docs/en/fullscreen.md)): a selection across the
  conversation, copied on release; a double and a triple click, `Shift`+arrows;
  the local clipboard tools, the tmux buffer or OSC 52; environment variables to
  opt out of the mouse.
- **Neovim, Helix**: the mouse on by default, an explicit yank, `Shift` for the
  terminal's own, OSC 52 providers. **lazygit** captures and selects nothing —
  "we simply don't support selections".

**What went wrong for them**, from their trackers (each issue read):

| Issue | What the reporter describes |
|---|---|
| [opencode #47165](https://github.com/anomalyco/opencode/issues/47165) | the terminal's own copy carries the wrap's newlines and the gutter's padding; with capture, over SSH in Terminal.app, "copied" and nothing on the clipboard |
| [opencode #61053](https://github.com/anomalyco/opencode/issues/61053) | a drag does not scroll, so a selection is one screen at most |
| [opencode #51056](https://github.com/anomalyco/opencode/issues/51056) | a drag with copy-on-select freezes the interface; manual copy avoids it |
| [opencode #59664](https://github.com/anomalyco/opencode/issues/59664) | a "Copied" toast for an OSC 52 that was only attempted |
| [Claude Code #83961](https://github.com/anthropics/claude-code/issues/83961) | an autoscrolled drag drops runs of ~26 columns from the copy |
| [Claude Code #61936](https://github.com/anthropics/claude-code/issues/61936) | GNOME Terminal's right-click menu and drag selection lost to capture |
| [Claude Code #85712](https://github.com/anthropics/claude-code/issues/85712) | the wheel a no-op on native Windows, several terminals; cause not found, closed as stale |
| [Zellij #2853](https://github.com/zellij-org/zellij/issues/2853) | the terminal's own copy of trailing spaces and hard breaks at the pane's width |

Each is a test or a live-check line of §7.

### 4.6 How many reports

- Terminals report a movement only into a new cell (Windows Terminal's
  `sameCoord`, [`mouseInput.cpp`](https://github.com/microsoft/terminal/blob/f6de9e0618015def1c9dda5b82f9d3abf03eb799/src/terminal/input/mouseInput.cpp#L336);
  xterm.js; Alacritty) — tens a second in ordinary use. conhost merges
  consecutive unread `MOUSE_MOVED` records ([`inputBuffer.cpp`](https://github.com/microsoft/terminal/blob/f6de9e0618015def1c9dda5b82f9d3abf03eb799/src/host/inputBuffer.cpp#L756)).
- No console-API mode gives buttons and drags alone: `ENABLE_MOUSE_INPUT`
  includes movement, and under ConPTY it makes the console ask Windows Terminal
  for `?1003;1006h` ([`VtIo.cpp`](https://github.com/microsoft/terminal/blob/f6de9e0618015def1c9dda5b82f9d3abf03eb799/src/host/VtIo.cpp#L718)). On native Windows the
  app filters movement itself (§5.2).
- `?1002h` alone reports drags in Windows Terminal — a movement with a button
  held, into a new cell — and since 1.22 ConPTY passes an app's VT output through
  unchanged, so over SSH too.
- Inside tmux, capture works whatever its `mouse` option says: tmux mirrors the
  active pane's mouse mode to the outer terminal and forwards the reports
  ([`server-client.c`](https://github.com/tmux/tmux/blob/75fac4a77bb40dc655f8b217071c224c0c819c7f/server-client.c#L2373)).
  Codex stays off when tmux's mouse is off, as a courtesy to that choice.

## 5. Design

### 5.1 The terminal's modes

- **On from the start** — unless the setting says `terminal` or the legacy
  terminal mode (`interface.terminal_compat`) is on (F1) — as **button and drag
  reports in SGR only**: `?1000h ?1002h ?1006h`. Not crossterm's
  `EnableMouseCapture`, which adds `?1003h` (every movement) and the legacy
  `?1015h`. A command of our own in `shared`, built and written the way
  `shared/osc52.rs` and `ui::set_background` are, so a test can assert its exact
  bytes. Off with crossterm's `DisableMouseCapture` — it lifts all five, harmless
  for those never set — at exit and in the panic hook, as today.
- **Windows**: crossterm's console mode stays the way to ask the console for mouse
  records. The console reports movement regardless; the input path drops it
  (§5.2).
- **Capture follows the screen in front** while the track is in progress (F7): on
  where the screen answers the mouse (the chat from stage 1), off where it does
  not yet (the list, settings, the self-model viewer until stage 3), so no screen
  loses the terminal's selection before it has one of its own.
- `mindfork keys` keeps its own `Ctrl+W`: it is the diagnostic that shows what a
  terminal sends, with capture and without.

### 5.2 The input path

- `collect_press` drops `MouseEventKind::Moved` (a movement with no button held):
  nothing in the app answers a hover, and the Windows console sends them anyway.
- The paste detector **counts key presses only**: a drain is a paste candidate
  when it holds two key presses or more, and the chase measures its 20 ms gap
  between keys, so mouse reports arriving meanwhile are collected but never keep
  it alive. A Windows paste is keys; a stream of reports is not one.
- With movement dropped, one report is one frame — what a drag needs to be drawn
  as it happens. The dirty rule itself stays.
- Tests on `read_batch_from`'s injectable source: two reports per read for 800 ms
  yield a batch per read (the regression of §3); a key between reports is in the
  first batch; a movement is dropped; a burst of keys is still one batch.

### 5.3 One selection on the screen

- At most one selection exists, owned by the feed or by a field; starting one
  ends the other, and `Ctrl+C` copies whichever exists.
- **What ends a selection** (F6) is what ends one in a text field of a GUI: a
  move of the cursor or typed text — in the feed's case the input box's cursor,
  the arrows, a click in the field — and, for the feed, a click in it outside the
  selection. Not `Esc`, which keeps its meanings, and **never a copy**, in the
  feed or in a field (F2; today a field's `Ctrl+C` ends its selection — that
  changes).
- **No feed selection while the assistant answers** (F6): a selection standing
  when a reply starts is dropped, and until the reply ends a drag in the feed
  selects nothing. The feed scrolls itself then, and markdown re-forms the text
  as it is written — both would move what a selection points at. A click on a
  `chat://` address still follows it, the wheel still scrolls, and the input
  box's selection works as ever.
- **The mouse grammar** (the left button):
  - *press* — remembers the point and the panel under it (the feed's rows, a
    field's text, or neither); nothing changes yet;
  - *drag* — a new selection from the press to the pointer, replacing the
    current one, **confined to the panel the press was in** (the property tmux and
    Zellij keep with panes): past the panel's edge the pointer is clamped to it,
    and the feed scrolls (§5.4);
  - *release without a drag* (a click) — follows the `chat://` address under it
    (F8: on the release, so that a drag which starts on an address selects it);
    otherwise a click in the feed outside the selection ends it, one inside
    keeps it, and in a field the click places the cursor. *Built (the owner's
    live check, 2026-11-15):* "inside" is the highlight, not the span between
    the selection's ends in reading order — that span holds the empty cells
    beside every row it crosses, where a click kept a long selection;
  - a second or third press on the same cell within 500 ms — the word, then the
    line (F5; Codex counts 400 ms).
- **The wheel** scrolls the panel under the pointer (the feed; later a field
  taller than its box, and the lists of stage 3); during a drag the selection
  follows.
- **The right and the middle button do nothing** (F3): in a terminal in a browser
  tab the right one opens the browser's context menu, and an action of the app's
  on top of it would be a second answer to one click.

### 5.4 The feed's selection

- **Coordinates:** (block, row within the block, column). A block is one
  message's cached rows, so the rows of earlier blocks do not move when a message
  or a note is appended below — those keep the selection.
- **What drops it** (F9): every `CacheKey` change, since each one invalidates
  every row — a resize, the theme, `Ctrl+T`/`Ctrl+O`, the language, role names,
  the jump marker, the address book, the compaction boundary, the feed's render
  settings; a regenerate or take-back that removes a selected block; and a reply
  starting (F6).
- **Row facts kept beside each cached block's rows**: the rail's width and whether
  the row is a soft wrap; made in `build_message_block`'s wrap loop, for the
  separator and for
  the compaction divider, carried through `render`'s second wrap (identity today;
  a piece it ever splits off is marked soft). The prefix of a pre-wrapped row
  (§2.4) is the same kind of fact, made by `push_gutter_lines`, `push_wrapped`
  and the quote's `wrap_hanging` — stage 2b.
- **The highlight** is painted over the frame after the `Paragraph`: the visible
  part of the selection gets `Palette::selection`, the field's own style — the
  selection backdrop, or in the monochrome mode the mark `strip_styles` keeps as
  reverse video. It is outside the cache like the search highlight, so a
  selection costs no re-render. It runs from the content's first column — past
  the rail — to each row's last glyph; a row's trailing padding stays
  unpainted.
- **The text:** for each selected row, the cells past the rail, cut to the
  selection's columns (a wide glyph is in when either of its cells is), trailing
  whitespace trimmed where a logical line ends (a code block's padding), joined to
  the next row by nothing when that one is a soft wrap and by `\n` otherwise. What
  is copied is **what is drawn**: rendered markdown — no `**`, a code block
  without its fences, a table in box-drawing characters. The source of a whole
  conversation stays `F5`/`/copy` (F4).
- **Autoscroll:** a drag held above or below the feed's rows scrolls it a row per
  tick (50 ms), the selection following the edge row; the loop's tick drives it,
  as it drives the spinner. A reply never runs meanwhile (§5.3), so the only
  scrolling is the drag's own.
- **The search highlight** keeps working underneath: the selection is painted
  last, and stepping between matches only scrolls, so the selection stays. A jump
  from the search screen moves the marker — a `CacheKey` change — and drops it
  (F9).

### 5.5 The fields

- **The chat's box** (stage 2): a drag past the text area is clamped to its edge
  — the selection reaches the first or the last row, and a draft taller than the
  box scrolls; a double and a triple click use the box's own word and line
  boundaries.
- **The other `InputBox` consumers** (stage 3): the runtime hands mouse events to
  whichever screen is in front, and each screen routes them to its focused field —
  rename, the settings editors, the self-model editor, the search bars.

### 5.6 Copy

- `Ctrl+C` copies the selection (F2) and **leaves it in place**, in the feed and
  in every field — a GUI field's behaviour; the field's `Ctrl+C` stops ending its
  selection. `Ctrl+X` still removes what it cut. There is no copy on release and
  none by the right button (F3).
- What is copied is what is drawn (F4); a copy in markdown is not wanted at all —
  the conversation's source is `F5` / `/copy`, and a partial markup copy raises
  questions with no good answer (a bold word selected by half, part of a
  diagram).
- **The outcome goes to a chip in the status bar**, for a few seconds, never to
  the feed (F10): copied here, sent to your terminal (OSC 52), too large or failed
  in the error colour. The input box's copy moves there as well — the verdicts
  `CopyReport::message` gives today, in the few words a chip has room for
  (`CopyReport::chip`; the "too big" dead ends still name `/export`), on a
  surface that neither splits a reply nor scrolls the feed. *After stage 1, the
  owner's decision (2026-11-15):* a send over OSC 52 reads `copied (OSC 52)` —
  over SSH the unconfirmed-send line came up on every `Ctrl+C`, in a terminal
  that had taken the text each time.

### 5.7 What goes away

- `Ctrl+W` (`screens/chat/input.rs:228`), `ChatIntent::SetMouseCapture`,
  `ChatScreen::mouse_scroll`, the status bar's `mouse:` cell with its accent and
  its pin (`widgets/status_bar.rs`), `/mouse` (`features/ui_command.rs:226`), the
  help rows `ui.help.mouse_toggle` / `ui.help.mouse_action` and their locale keys;
  the manual's paragraph on the toggle; the rows of spec §11.3, §11.5, §11.7. The
  footer gains the space of its widest hint.
- **What stays:** the terminal's own selection with its modifier — `Shift` in
  most terminals, `Option` in iTerm2, `Fn` in Terminal.app (§4.1) — named in the
  help overlay and the manual, with Windows Terminal's two catches: `Shift`+right
  click pastes, and copies the terminal's own selection as one line
  (`Ctrl+Shift+C` keeps its lines). And a setting on the settings screen,
  *Interface* — `app` | `terminal`, `app` by default (F1) — where `terminal` is
  today's select mode without a way to leave it: for VS Code on macOS, which has
  no modifier by default, a screen reader, or a terminal whose mouse reporting
  misbehaves. **The legacy terminal mode turns capture off by itself**: conhost,
  the terminal it exists for, has no modifier around capture at all (§4.1).
  Capture is on when the setting says `app` and the legacy mode is off.

## 6. Forks

**Decided by the owner, 2026-11-14.** Each fork keeps its options; the decision
follows them, with the owner's reason where it went another way than the
recommendation.

**F1 — what replaces the toggle.** (a) capture always on, no switch anywhere;
(b) always on, a setting `app` | `terminal`, `Ctrl+W` / its light / `/mouse`
removed; (c) as (b), with `Ctrl+W` / `/mouse` kept for the session.
*Decided: (b), and the legacy terminal mode turns capture off.* The setting
lives on the settings screen; the status-bar light and `/mouse` go with
`Ctrl+W`.

**F2 — when the copy happens.** (a) on `Ctrl+C`; (b) on release, automatically;
(c) (a) with (b) as an opt-in. *Decided: (a), `Ctrl+C`* — the field's key, and
the one Windows Terminal passes through when it has no selection of its own
(§4.2). *Second round:* a copy never ends a selection, anywhere — the behaviour
of a text field in a GUI; the input box's `Ctrl+C`, which ends its selection
today, changes with it.

**F3 — the right and the middle button.** (a) the right copies a selection;
(b) both do nothing; (c) the right copies or pastes. *Decided: (b), against the
recommendation:* the app is also used from a terminal in a browser tab, where
the right button opens the browser's context menu.

**F4 — what a copy holds.** (a) what is drawn, without the chrome; (b) the
markdown source for whole messages; (c) markdown rebuilt for any selection from
source positions (Codex's high end, §4.5). *Decided: (a); markdown is not wanted
at all,* not just later: the conversation's source is already `F5` / `/copy`,
and a partial copy in markdown has no good answer for a bold word selected by
half or for part of a Mermaid diagram.

**F5 — double and triple click.** *Decided: (a)* — a word, then the logical
line, in the feed and in the fields.

**F6 — `Esc` while there is a selection.** (a) `Esc` ends it first; (b) `Esc`
unchanged. *Decided: (b), against the recommendation.* A selection ends by a
click on a part of the text that is not selected, or by a move of the cursor or
typed text; a copy does not end it. *Second round, the owner's addition:* when
the assistant starts answering, a feed selection is dropped, and **no feed
selection can be made until the answer ends** — the feed scrolls itself then,
and markdown re-forms the text as it is written, both sources of defects.

**F7 — capture while the track is in progress.** *Decided: (b)* — on where the
screen in front answers the mouse, off elsewhere until stage 3.

**F8 — a click on a `chat://` address.** *Decided: (a)* — on the release, with no
drag in between.

**F9 — a feed selection across changes to the feed.** (a) dropped by every
rewrap; (b) anchored to the content, surviving one. *Decided: (a).* The first
round kept a selection across a resize, which needs (b)'s anchor and its
re-finding after the rewrap — with the limit that an end inside a table or a
diagram, laid out anew at the new width, cannot be found again. *Second round:*
not worth the code for now; a resize drops the selection like `Ctrl+T`/`Ctrl+O`
and the theme.

**F10 — where a copy's outcome is said.** *Decided: (a), a chip in the status
bar* — with one more reason of the owner's: a note in the feed scrolls the feed.
A better place may come later, the frame of an element on screen for instance.

## 7. Stages, tests, the live check

- **P0 — the terminal side, no code (go/no-go).** The owner, in Windows Terminal
  over SSH and then natively: `mindfork keys`, `Ctrl+W` (capture on), then the
  wheel, a left drag, a double click, the right button, `Shift`+drag, `Shift`+the
  right button, `Ctrl`+click on a URL; the lines it prints are what the app would
  receive. **GO** when a drag arrives as `Drag(Left)` with positions, the wheel as
  `ScrollUp`/`ScrollDown`, and `Shift`+drag prints nothing and selects in the
  terminal. The survey (§4) predicts all three for Windows Terminal; this
  confirms them where the owner works, and conhost's "nothing" (§4.1) the same
  way.
- **Stage 1 — the input path and the modes.** §5.2 alone first
  (`fix/mouse-input-path`): it repairs today's scroll mode and needs no fork.
  Then §5.1 (`feat/mouse-capture-setting`): the setting on the settings screen —
  defaulting to `terminal` for now, so a release between stages changes nothing —
  the legacy terminal mode's veto, the button-and-drag capture, capture following
  the screen (F7), the link on the release (F8), the copy chip (F10), the field's
  `Ctrl+C` keeping its selection (F2). Tests: §5.2's; the mode command's bytes;
  capture on only for `app` with the legacy mode off, and only where the screen
  answers the mouse; a click and a drag told apart on an address; a field's
  selection intact after `Ctrl+C`.
- **Stage 2 — the feed's selection** (`feat/feed-selection`). §5.3–§5.6: the row
  facts, the selection and its grammar, the highlight, the text, the
  multi-click, the autoscroll; the box's clamped drag and its multi-click; then
  §5.7 — the toggle removed, the default flipped to `app`. Tests on
  `TestBackend`: a drag over a wrapped paragraph copies one line; over a code
  block — no padding and no rail; over a table; a wide glyph at either end; the
  highlight's cells in both colour modes; a selection dropped by a resize,
  `Ctrl+T`/`Ctrl+O` and a theme change, kept when a message is appended below;
  dropped when a reply starts, and no selection made while it streams — a drag
  selects nothing, a link click still follows, the wheel still scrolls; ended by
  a click outside it, by a cursor move and by typing, kept by a click inside it,
  by `Esc` and by a copy; the right button doing nothing; an address followed on
  the release and selected by a drag; which selection `Ctrl+C` takes. And the
  failures of §4.5 as tests: a selection grown by autoscroll copies every row it
  covers
  (Claude Code #83961); a drag past the edge scrolls (opencode #61053); a copy
  runs once per gesture, never per report (opencode #51056); an OSC 52 send is
  told as sent, not copied (opencode #59664). **No live run in the AGENTS.md §3
  sense** (no engine, memory or tool path), but the TUI needs a person: the owner
  in Windows Terminal over SSH and natively — the native wheel is where Claude
  Code's full-screen mode got nothing (#85712) — conhost, VS Code's terminal and
  the web demo: the wheel, a selection pasted into Notepad (Cyrillic and emoji
  included — multi-byte text is where OSC 52 has broken before, §4.4), `Shift`
  for the terminal's own, typing while the mouse moves.
- **Stage 2b — the prefixes** (may fold into 2): a quote's `> `, the thoughts'
  gutter and a tool card's indent left out of a copy, their rows joined where they
  continue a line — the facts reported by the builders of §2.4.
- **Stage 3 — the other screens** (`feat/mouse-everywhere`): mouse events reach
  every screen; the fields of §5.5 get the click, the drag and the multi-click;
  the wheel scrolls the chat list, settings, help, tasks, notes and the
  self-model viewer; capture goes on everywhere. For read-only text (the
  self-model viewer, help, *About*) the cheap form is a selection over the
  frame's cells of one panel, the visible part only — if it is wanted at all.

## 8. Risks and open questions

- **A terminal with no way around capture** would lose its own selection inside
  the app: conhost always, VS Code on macOS by default (§4.1). For conhost the
  legacy terminal mode turns capture off (F1) — so a conhost user without that
  mode on gets the app's selection and no terminal one, and the setting's hint
  says which switch to reach for; VS Code on macOS has the setting. Until stage 3,
  F7 keeps the other screens' selection the terminal's.
- **A terminal whose reports misbehave** (wrong coordinates, no SGR): the same
  setting; crossterm parses the normal, SGR and rxvt encodings.
- **The highlight over a wide glyph** may be painted on one half where a terminal
  does not paint a wide glyph's trailing cell itself — the known limitation of
  spec §11.3; cosmetic.
- **A frame per drag report.** Every frame clones every cached row
  (`build_lines`, :1187); a long chat in a debug build can take longer than the
  gap between two reports. After §5.2 the reports that pile up meanwhile are one
  batch and one frame — late by a frame, never stalled.
- **Muscle memory:** inside the window the terminal's right-click paste and menu
  (GNOME Terminal's, Claude Code #61936) and its own multi-click are gone (§4);
  the help overlay and the manual name the modifier that brings them back.
- **A copy over SSH reaches only as far as OSC 52 does** (§4.4): nowhere from
  GNOME Terminal or Terminal.app, nowhere from inside tmux with its defaults (the
  app's DCS-wrapped sequence needs `allow-passthrough on`), nowhere from iTerm2
  until its clipboard switch is on. Those are the OSC 52 path's gaps and exist
  today; a routine copy from the feed meets them more often.
  `tmux load-buffer -w -` is the known route under tmux's defaults — a fix of its
  own, worth taking with stage 2.

## 9. Not in this track

- A keyboard copy mode — tmux's: move a cursor over the feed with the keys,
  select, yank — the route without a mouse; a later idea.
- A rectangular selection, a selection across panels, drag-and-drop, a context
  menu, hover effects (no `?1003h`, on purpose).
- A copy as markdown — not wanted (F4), not merely deferred.
- Per-message actions by click — copy one message's source, fold one block. The
  roadmap's *per-message collapse, select and copy* keeps those two; this track
  gives it the selection.
