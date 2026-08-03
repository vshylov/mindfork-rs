# Collapsing tool calls in the feed, per chat

**Status:** in progress. Forks **C1–C3 — user's decision, 2026-08-03**.

Asked for directly: tool calls should collapse the way "thoughts" already do,
they should be **collapsed by default**, and the collapsed/expanded state should
be remembered **per chat** — for thoughts as well as for tool calls.

## 1. What exists today

- "Thoughts" (CoT) collapse via `Ctrl+T`. The state is one **global** `bool` on
  the widget (`MessageFeed::show_thoughts`, `toggle_thoughts`) and is part of
  `CacheKey`, so toggling it rebuilds every block. Collapsed by default.
- Tool calls **never** collapse: `push_tool` always draws the header
  `⚒ name(args)` plus every argument and result block the presenter
  (`features/tools/present.rs`) produced.
- Per-chat UI state already has a playbook — `Chat.draft` (spec §11.7):
  a field on `Chat` (`#[serde(default)]`, no migration), `AppCommand::SetDraft`,
  a field in `AppEvent::ChatActivated`, written with the save debounce and
  **without touching `modified_at`** (a view toggle must not bump the chat up
  the list).

## 2. Forks

### C1 — where the per-chat state lives → **(a) in the chat file**

- **(a) `Chat.feed_view`, the `draft` playbook.** Survives a restart. Costs a
  contract change: a new `AppCommand`, a field in `ChatActivated`, a handler in
  `orchestrator/chats.rs`.
- (b) A `HashMap<Uuid, FeedView>` inside `ChatScreen`. ~10 lines, no contract
  change, but everything resets to the default on restart.

**User's decision (2026-08-03): (a).** Recommended: the state describes a chat,
and `draft` already proves the path.

### C2 — the hotkey for tool calls → **(a) `Ctrl+O`**

`Ctrl+T` is thoughts. Free in the chat screen *and* in `InputBox`'s own
`on_key`: `o`, `l`, `d`. Ruled out by the terminal itself: `Ctrl+I` = Tab,
`Ctrl+H` = Backspace, `Ctrl+M`/`Ctrl+J` = Enter.

- **(a) `Ctrl+O`** — no terminal meaning of its own; mnemonic "output".
- (b) `Ctrl+L` — free since the chat list moved to `Esc`, but shells train
  "clear screen" into it.
- (c) `Ctrl+D` — free here, but reads as EOF on unix.

**User's decision (2026-08-03): (a).**

### C3 — what a collapsed tool call looks like → **(a) header only**

Not a fork the user was asked about; recorded because it is the one design
choice inside the rendering.

- **(a) The card's header line stays** (`⚒ name(args)` — *what* was called is
  the informative half) and only the argument/result blocks are hidden; the
  header gains the collapsed pill's marker and keycap, mirroring
  `push_thoughts`: `⚒ name(args) ▸ details [Ctrl+O]`.
- (b) Hide the card entirely — loses the fact that a tool ran at all.
- (c) A separate summary line ("N lines hidden") — a second line per call,
  noisier than the thing it replaces.

Chosen **(a)**: one line per call either way, and the collapsed state still
answers "what did it do", which is the question the feed is scanned for.
The pill is shown **only when collapsed** (there is something to reveal), same
as the thoughts pill; when expanded the `⚒` header is the marker. A call with
no arguments *and* no result gets no pill — there is nothing to hide.

### C4 — what an expanded card looks like → **the layout the user specified**

Added after the first live look (**user's decision, 2026-08-03**): collapsed
stays exactly as it is; expanded shows **the tool's name in the header, the
arguments enumerated below, then a gap row, then the result**. The gap keeps the
card's `│` gutter (a second round of feedback): a blank row there reads as the
end of the card rather than as a break inside it.

The reason it needed saying: the header is a *title*. `truncate_header` flattens
whitespace and cuts at `HEADER_MAX_CHARS = 100`, and `scalar_str` drops anything
that is not a scalar — so a long query ends in `…` and an array/object argument
never appears at all, in either mode. Collapsed that is the right summary;
expanded it was a lie.

Implemented as `ArgDetail::{Compact, Full}` on the presenter — the two are
**different presentations, not one with a flag threaded through it**
(`compact_args`/`full_args`), because which field is code, which is large, and
what may be folded into a header is knowledge that already lives in
`features/tools/present.rs`; the widget stays generic.

`Full`: header suffix `None`; one `key: value` line per field, untruncated;
arrays/objects as compact JSON; a value that cannot share a line with its key
(code, a large or multiline string) under a `key:` label as its own block, so
code keeps its highlighting *and* is still named. Field order is
`serde_json::Map`'s — alphabetical, since the wire format does not preserve the
model's own order for us.

A first attempt added the missing detail **conditionally** (only when the header
truncated or dropped something) and kept the compact header. It was rejected on
sight: the layout then depended on how long the values happened to be, so two
neighbouring calls could look structurally different for no reason the reader
could see. Unconditional is both simpler and more predictable.

The confirmation popup (§9.8) keeps `Compact`: a decision prompt, not a viewer.

## 3. Shape

- `entities/chat.rs`: `FeedView { thoughts: bool, tools: bool }`
  (`Copy + Eq + Default`, both `false` = collapsed) + `Chat.feed_view`
  (`#[serde(default, skip_serializing_if = "FeedView::is_default")]` → old chat
  files read without migration, and a chat nobody expanded writes no new key).
  A named type rather than two bools: it travels through a command, an event and
  a cache key, where a bare `(bool, bool)` is exactly what gets swapped.
- `widgets/message_feed.rs`: `show_thoughts` → `view: FeedView`; `toggle_tools`,
  `set_view`, `view()`; `CacheKey.show_thoughts` → `view` (collapsing tools
  reshapes blocks just like collapsing thoughts).
- `screens/chat`: `Ctrl+T`/`Ctrl+O` toggle **and return
  `ChatIntent::SetFeedView(FeedView)`** — a discrete keypress, so it needs none
  of the `draft_dirty`/`take_dirty_draft` machinery, which exists only because
  typing is continuous. `activate_chat` takes the view and applies it.
- `app`: `AppCommand::SetFeedView`, `ChatActivated.feed_view`,
  `handle_set_feed_view` (mark dirty, leave `modified_at` alone).

## 4. Live run

Not required (AGENTS.md §3): rendering, key handling and a per-chat field — no
engine, memory, tool or provider path is touched. Covered by `TestBackend` and
orchestrator integration tests.
