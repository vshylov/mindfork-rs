# Research: folding one block with the mouse, and copying the first or last messages

**Status:** research (2026-11-15). Four decisions were taken by the owner before it
was written (§4.1). **Forks F1–F5 decided by the owner the same day** (§4.2): F1–F4
as recommended, and F5 the other way, so the target is the marker and the pill, not
the whole header. That choice left an expanded tool card with nothing to click, which
one more fork (F5b) settled as recommended: the card gets a `▾ details` pill. **Stage 1
is built** (2026-11-15, `feat/copy-first-last`): `/copy first|last [N]` and one label
per reply (§3.6, F2; [docs/journal/ui-input.md](../journal/ui-input.md)). Stage 2, the
fold, is next.

The roadmap item "per-message collapse, select and copy" (docs/roadmap.md) was
deferred until the app held the mouse. That track is complete
([mouse-always-captured.md](mouse-always-captured.md)), and the owner narrowed what
is left of the item to two things:

- a click on a foldable block's header folds or unfolds **that block alone**, and the
  state is kept in the chat file;
- `/copy first N` and `/copy last N` copy part of the conversation, in the format of
  the whole conversation's copy.

## 1. Summary

- **The state is the two flags `Ctrl+T`/`Ctrl+O` keep today, plus a set of
  exceptions.** An exception is a block whose state differs from its kind's flag.
  A click adds a block to the set or takes it out. `Ctrl+T` (or `/thoughts`) flips its
  flag and empties the set of thoughts and the summary; `Ctrl+O` (or `/toolcalls`)
  does the same for tool calls. So "fold all" still means all, and the set stays
  short: it is empty after every whole-feed toggle (§3.1).
- **A block is named by what the chat file already holds** (§3.2):
  - the thoughts by the first message of their bubble;
  - a tool card by its message and the call's index in it, **not** the call's wire
    id (Gemini's ids are synthesized as `name-index` and repeat between rounds);
  - the compaction summary by its boundary message.
- **Stored as an additive field of `feed_view`**: no schema step, an old file reads
  as before, a chat with no exceptions writes no new key. A transcript shares its
  parent's set, as it shares the flags, because message ids are unique across both.
  A clone keeps the set, because a clone keeps the message ids (§3.3).
- **The click** (§3.4):
  - **Where:** the marker and the pill only (F5). An expanded tool card gets a
    `▾ details` pill to click (F5b).
  - **When:** on the release with no drag. A drag from a pill still selects.
  - **Repeats:** each click toggles once.
  - **Position:** the pill stays on its screen row.
  - **Exception:** the reply still being written takes no click, since it has no ids
    yet.
- **The render rebuilds only the clicked block**, not the cache. The flags stay in
  the cache key, and a block's exceptions enter its fingerprint (§3.5).
- **`/copy first N` / `/copy last N`** is a subcommand of the registry's `/copy` row
  (§3.6):
  - N counts messages as the feed draws them (F1);
  - N is optional and defaults to 1 (F4);
  - an excerpt is copied without the chat's title (F3);
  - the copy settings (thoughts, calls, results) apply as they do to the whole copy.
- **F2:** found on the way, the whole copy writes a separate `Assistant:` label for
  every round of a reply with tool calls, so one reply in the feed reads as two or
  three in the copy. Decided: one label per reply, in the whole copy and `/export`
  too.
- **Two pull requests:** the copy, then the fold (§5). Neither touches the engine, so
  no live run against a model is due. A pty probe and the owner's look in Windows
  Terminal check the fold.

## 2. What the code does today

### 2.1 The fold

**Stored state.** `entities::chat::FeedView { thoughts, tools }` is two `bool`s, both
`false` (collapsed) by default. It is stored per chat as `Chat.feed_view`, which is
skipped when it is the default ([docs/history/feed-collapse.md](../history/feed-collapse.md)).

**The keys.**
- `Ctrl+T` (typed: `/thoughts`) flips `thoughts`.
- `Ctrl+O` (typed: `/toolcalls`) flips `tools`.

The screen applies the toggle at once and returns `ChatIntent::SetFeedView(view)`,
which reaches the orchestrator as `AppCommand::SetFeedView`.
`handle_set_feed_view` stores it with the save debounce and leaves `modified_at`
alone. Activating the chat sends it back as `ChatActivated.feed_view`.

**A transcript** opens with its parent's `feed_view`, and its toggles are stored on
the parent (`parent_of`): it is part of the parent (spec §11.2).

**Rendering.** `MessageFeed` keeps the view in its `CacheKey`, so a toggle clears the
whole block cache and every block is rebuilt (`build_lines`). In
`build_message_block`:
- `push_thoughts` draws a collapsed pill `▸ thoughts · N lines [Ctrl+T]` or the
  expanded `▾ thoughts` header and the gutter lines;
- `push_tool` draws the card's `⚒ name(args)` header either way, ending in the
  `▸ details · [Ctrl+O]` pill when collapsed and something is hidden;
- `push_compaction_boundary` draws the divider, which folds with the thoughts
  (fork F8c of history-compression).

### 2.2 What names a block

**A feed item is not a message.** `FeedMessage::from_messages` merges the assistant
messages of an agentic round into one bubble (`merge_round`):
- the thoughts of the rounds are joined into the bubble's single thoughts block;
- the bubble's `message_ids` lists every message folded into it. This is the only
  link from a feed position back to the chat file, and `focus_message` resolves a
  jump through it.

A message with `new_bubble` starts a bubble of its own.

**Tool calls.** A message's calls are `ToolCallRecord`s with a wire `id`. That id
does not name a call within a chat: the Gemini client synthesizes it as
`"{name}-{index}"` (`shared/api/gemini/client.rs`), so the first `web_search` of
every round is `web_search-0`. What does name a call is its message's `Uuid` and its
index in that message's `tool_calls`. That index is fixed once the message is stored.

**The compaction boundary** is named by the message verbatim history resumes at
(`ChatActivated.compaction`'s `Uuid`). A new roll moves it.

**The reply being written.** The live bubble is pushed as literals and has no
`message_ids` until the turn ends and the chat is drawn from stored messages. A
card it shows carries `call_id` for `ToolCallStarted`/`ToolCall` to match, but no
stored message. Rounds already finished when a chat is opened mid-turn arrive as
messages (`seat_rounds`) and have ids.

### 2.3 The feed's click

`ChatScreen::handle_mouse_at` (`screens/chat/input.rs`) handles a press in four steps:

1. It remembers the press (`MousePress`).
2. A drag past the press cell selects.
3. A release with no drag is a **click**. It follows a `chat://` reference under it
   (`chat_link_at`, answered against the last drawn frame's `link_hits`).
4. Otherwise a click ends the selection unless it lands on the highlight.

`count_click` counts presses on one cell: a double press takes a word, a triple a
line, on a row's text. While the assistant answers the feed selects nothing, but a
reference still follows (research F6).

### 2.4 The copy

`F5` and `/copy` send `AppCommand::CopyChat(id)`. `handle_copy_chat` formats the
chat's (or the transcript's) messages with
`features::chat_export::format_conversation` and returns the text as
`AppEvent::CopyToClipboard`. `/export` (Markdown) uses the same function. The format:
- the title, if any;
- then one block per message, joined by blank lines:
  - a user message is `User:` and its text;
  - an assistant message is `Assistant:`, its text and, per `CopySettings`, its
    thoughts, calls and results;
- system and tool messages are skipped.

The function walks **stored messages**, and each round of a reply is one, so a reply
whose first round said "Let me check" before a call copies as two `Assistant:`
blocks. The feed draws it as one bubble. With the default settings (text only), a
round with no text produces no block.

`/copy` is a row of the command registry (`features/ui_command.rs`) with
`Arity::None`. `Arity::Subcommand(&[...])` is the registry's form for a closed set of
words carrying one token for the runner to read (`/subagents stop 2`,
`/tasks stop reflection`).

## 3. Design

### 3.1 The state: flags and exceptions

`FeedView` gains a third field, `flipped`: the set of blocks whose state is the
opposite of their kind's flag. A block is expanded when its kind's flag is on, or
when it is in `flipped`, but not both.

| Action | Effect |
|---|---|
| A click on a block's header | Adds the block to `flipped` or takes it out |
| `Ctrl+T`, `/thoughts` | Flips `thoughts` and removes every thoughts and summary key from `flipped` |
| `Ctrl+O`, `/toolcalls` | Flips `tools` and removes every tool key |

After a whole-feed toggle, every block of that kind is in the new state, which is
what the owner asked to keep (§4.1, D3). One corner is accepted, and the help
explains it by what it does: if every thoughts block was expanded by a click,
`Ctrl+T` changes nothing visible. The flag becomes "expanded" and the clicks are
forgotten.

The rule lives in one type (`FeedView::toggle_thoughts`, `toggle_tools`,
`toggle_block`, `expanded(key)`) and is tested there. The screen and the orchestrator
only carry the value.

### 3.2 Keys

```rust
enum FoldKey {
    /// The thoughts of the bubble whose first message this is.
    Thoughts(Uuid),
    /// The `call`-th entry of this message's `tool_calls`.
    Tool { message: Uuid, call: usize },
    /// The compaction summary at this boundary.
    Summary(Uuid),
}
```

Where each key comes from:
- **The thoughts key** is the bubble's `message_ids[0]`. A bubble's first message
  stays first:
  - `/continue` appends to the last message in place;
  - a regeneration or a take-back removes the whole reply, and the key with it.
- **The tool key** is set on `FeedToolCall` in `from_message` (a new
  `fold: Option<FoldKey>` field). `merge_round` keeps each call's own key.
- **The summary key** is the boundary's id. After a new roll the old key no longer
  matches, and the new summary starts from the flag. That is right: it is new text.
- **The live bubble's blocks** have no key, so they take no click.

### 3.3 Storage

The JSON:

```json
"feed_view": {
  "tools": true,
  "flipped": [
    {"thoughts": "6f1c…"},
    {"tool": {"message": "a03e…", "call": 2}},
    {"summary": "9b77…"}
  ]
}
```

It is additive under ADR 0006 §8 (F12): `#[serde(default)]`, skipped when empty, no
bump.

**What older versions do with it.** An older version reads the flags and drops
`flipped` on its next save. A downgrade forgets the clicks and nothing else, the same
as when `feed_view` itself was added. The set is a `BTreeSet`, so the file's order is
stable and a save that changed nothing writes the same bytes.

**Where it travels.** The set rides the existing route: `SetFeedView`,
`ChatActivated.feed_view`, `handle_set_feed_view`, with no new command. A click
returns `ChatIntent::SetFeedView` the way a key does. `FeedView` stops being `Copy`,
a mechanical change at the places that copy it.

**Pruning.** A key whose message is gone is pruned **when the chat is activated**,
against its messages and its transcripts' messages. That is the one place every path
passes, and it keeps the stored set from collecting the dead keys of take-backs. A
key the prune drops marks the chat dirty, like any other view change.

**Other chats that hold these messages.**
- A transcript's blocks are keyed by the transcript's own message ids and stored in
  the parent's set (`parent_of`), like its flags. The ids are v4 `Uuid`s, so the
  parent's and the transcripts' keys cannot collide.
- A clone (`handle_clone`) copies the messages with their ids, so it keeps the
  exceptions. `reid_children` renews transcript ids, not message ids.
- `/export json` and `mindfork import` carry no view state today and carry none now.

### 3.4 The click

**The target is the marker and the pill** (F5, F5b), as drawn:

| Block | Collapsed | Expanded |
|---|---|---|
| Thoughts | the pill `▸ thoughts · N lines [Ctrl+T]` | `▾ thoughts` |
| A tool card | its pill `▸ details · [Ctrl+O]` | a new pill `▾ details`, with no keycap (F5b) |
| The compaction divider | `▸` and its label, with the keycap | `▾` and its label |

The rule that fills the rest of the divider's row is not part of the target.

**The rest of a header is ordinary feed text.** A click there ends a selection, a
double click takes a word, and a `chat://` reference in a tool's arguments follows.
A pill's text comes from the locale, so a reference can never be part of a target.

**Where a pill is drawn.** A pill that does not fit on the header's last row moves
to the next one (`push_trailing`), and the target is wherever it is drawn.

**When.** The toggle comes on the release of a press with no drag. A drag that starts
on a pill selects, as a drag from a `chat://` address does (F8 of the mouse
research).

**Every click toggles.** A press on a pill does not take a word or a line, so a
double click expands and collapses again. A pill is a control, and treating the
second press of a double click differently would mean holding the first toggle back
for the double-click interval. That delay would be felt on every single click.

**The selection.** A click on a pill ends a selection like any click outside the
highlight. A pill inside the highlight still toggles: the rebuild of its block drops
what the selection held there, as every rewrap of a block does today.

**The pill stays on its row.** Every row above the pill is unchanged by the toggle,
so keeping the scroll offset keeps the pill in place. The one thing that would move
it is the feed following its tail: there the new rows would push the pill up. So a
fold click ends following. Where the content becomes shorter than the view, the
clamp moves the view, which cannot be helped.

**While the assistant answers.** The stored blocks above the reply take clicks, as
references do. The live bubble's take none (§3.2).

**Popups.** With a popup open over the chat (the link picker, the emoji picker, the
feed's search), the popup takes the mouse first, as today.

### 3.5 Render

**The cache key** keeps the two flags only. A whole-feed toggle still rebuilds
everything, and a click does not.

**A block's own state.** A block is built with the expanded state of each of its
foldable parts. That state is hashed into the block's fingerprint, so a click
rebuilds the one block that changed. Because the block was rebuilt,
`forget_selection_in` runs for it, as for any changed block.

**The hit map.**
- `push_thoughts`, `push_tool` and `push_compaction_boundary` mark their pill's
  spans.
- A block's rows are rows as drawn, so after the wrap the build knows each pill's
  row and columns in the block. It records them with the block's key, the way it
  records `links` and `facts` today.
- `render` maps the visible ones to terminal cells (`fold_hits`, beside
  `link_hits`).
- `MessageFeed::fold_at(col, row)` answers against the last drawn frame.

The test finds the pill it clicks by reading the drawn buffer back
(docs/lessons.md §5).

**The pills keep their keycaps** (`Ctrl+T`, `Ctrl+O`). They are still true: the key
folds every block of the kind. The click is in the help's mouse rows and the manual.

**The expanded tool card gains a pill** (F5b): `▾ details`, drawn by the collapsed
pill's code on the header's last row or the next. A card with nothing to fold (no
arguments and no result yet) gets none, as it gets no collapsed pill. The pill is
drawn however the card was expanded, so it reads the same after `Ctrl+O` or a click.

**What this changes in the pictures.** The pill changes the look of every expanded
card. The demo's captures expand tool calls (`features::demo::feed_view`), so stage 2
redraws the screenshot gate's pictures and the demo reel, in a commit of their own.
Nothing else in the feed's look changes.

### 3.6 `/copy first N` and `/copy last N`

**The grammar.** The registry row becomes
`Arity::Subcommand(&["first", "last"])`, and its help label becomes
`/copy [first|last [N]]`.

| Command | Copies |
|---|---|
| `/copy` | Everything, as today |
| `/copy last 3` | The last three messages |
| `/copy first 2` | The first two messages |
| `/copy last` | One message (F4) |

**Refused:** `0`, a token that is not a number, and anything after it. The parser
already reports an unknown word.

**Counting** (F1): a message is what the feed draws as one:
- the person's message;
- the assistant's reply with all its rounds (a reply that `new_bubble` split counts as
  two, as it is drawn as two).

A message that would copy empty under the current settings is not counted. For
example, an assistant bubble of tool calls only, when calls are not copied. So
`/copy last 1` always yields text if there is any. An N larger than the conversation
copies all of it, without an error.

**Where it runs.** The rule already exists below `widgets`.
`entities::chat::visible_row_count` is how the chat card counts "N msg": the bubbles
the feed draws, with tool messages drawing none, a `System` entry a note row, and
consecutive assistant rounds one bubble unless `new_bubble` splits them. The
`bubble_count_agrees_with_the_list_counter` test pins it to the feed's
`from_messages`. The copy reuses that rule rather than stating it a third time:
- it becomes a grouping, `visible_rows(messages) -> Vec<Range<usize>>` (built as
  `visible_rows` over messages and `visible_row_ranges` over `(role, new_bubble)`
  pairs, which `mindfork stats` reads);
- the count is that grouping's length;
- `features::chat_export` takes its ranges.

So N on the command and the number on the chat card count the same thing.
`AppCommand::CopyChat` gains a range (`CopyRange::{All, First(n), Last(n)}`).
`F5` and the chat list's copy send `All`. An excerpt comes back as
`AppEvent::CopyExcerpt { text, messages }`, so the confirmation can name the count.

**The format.**
- It is `format_conversation`'s, over the chosen messages.
- The chat's title is left out (F3): an excerpt is a quote, and the title describes
  the whole.
- The copy settings apply.
- In a transcript the command copies the transcript's messages, as `/copy` does.

**The confirmation** says what was taken: "Messages copied: 3" instead of
"Conversation copied". The count goes after a label because the bundles have no
plural rules (the number-neutral wording of docs/journal/i18n.md, guarded in
`shared/i18n.rs`).

## 4. Decisions and forks

### 4.1 Decided by the owner (2026-11-15)

- **D1 — no per-message copy by mouse.** The mouse selects and copies text, as it
  does now. A message as a unit is copied by command.
- **D2 — `/copy first N` and `/copy last N`**, in the format of the whole chat's
  copy.
- **D3 — `Ctrl+T`/`Ctrl+O` still fold every block of their kind.** One block folds
  by the mouse only, with no key for it: the operation is not frequent enough to
  earn one. The deferred `Space`/`Tab` row of spec §11.3's key table goes.
- **D4 — the state of a single block is stored** in the chat file, after the
  recommendation. Without it, the one case it matters for comes back on every
  opening: everything expanded and one huge result folded away.

### 4.2 Decided by the owner the same day

**F1 — what N counts.**
- **(a) Messages as the feed draws them: a reply with all its rounds is one.**
  *(Recommended.)* N matches what the person sees and counts on the screen, and
  what the chat card's "N msg" already counts (`visible_row_count`).
- (b) Exchanges: a question and its answer are one. `last 1` would then be the last
  question with its answer, but a conversation does not always alternate (a
  greeting, a directed dialogue, a follow-up the assistant wrote unasked).
- (c) Stored messages. This is the file's count, which no one can see: a reply with
  three rounds would be three.

**Owner's decision (2026-11-15): (a).**

**F2 — one `Assistant:` label per reply.**
- **(a) One label per reply as drawn, in the whole copy, the excerpt and the
  Markdown `/export` alike.** The rounds' texts are joined by a blank line, as the
  feed joins them, and each round's thoughts, calls and results go where the
  settings put them. *(Recommended.)* This changes the whole copy's output for every
  reply with tool calls, which today reads as several replies.
- (b) Leave the whole copy as it is, and join rounds only in excerpts. Two formats
  for one thing.
- (c) Leave both as they are. Then `/copy last 1` on such a reply prints two or three
  labels.

**Owner's decision (2026-11-15): (a).**

**F3 — the title in an excerpt.**
- **(a) No title.** *(Recommended.)* The excerpt is a quote, usually pasted into
  another conversation, where the chat's title is noise.
- (b) The title first, as in the whole copy.

**Owner's decision (2026-11-15): (a).**

**F4 — a bare `first` or `last`.**
- **(a) Means 1.** *(Recommended.)* `/copy last` is the commonest case: the last
  reply.
- (b) N is required.

**Owner's decision (2026-11-15): (a).**

**F5 — the click target.**
- **(a) The header's rows, at any column past the rail.** *(Recommended.)* A wide
  target, the way a list row takes a click anywhere on it.
- (b) The marker and the pill's text only: a smaller target that leaves the rest of
  the header to ordinary clicks.

**Owner's decision (2026-11-15): (b)**, against the recommendation.

**F5b — the target on an expanded tool card.** This fork was added after F5 (b): a
collapsed card ends on its pill, but an expanded one has none. Its `⚒` header has
been its marker since fork C3 of feed-collapse, while the expanded thoughts and the
divider show `▾`.
- **(a) A `▾ details` pill on the expanded card's header**, with no keycap, as the
  expanded thoughts have none. *(Recommended.)* All three kinds then fold the same
  way. It changes the look of expanded cards slightly.
- (b) The `⚒` icon. The look does not change, but the target is two cells and does
  not look clickable.
- (c) The icon and the tool's name, without the arguments. A larger target, but it
  does not look like a button either.

**Owner's decision (2026-11-15): (a).**

## 5. Stages, tests, the live check

**Stage 1 — `feat/copy-first-last`** (`/copy first|last [N]`, and F2).

Tests:
- the registry parses the subcommand: N, bare, `0`, a non-number, extra tokens;
- `entities::chat`: `visible_rows` over rounds, `new_bubble` and a `System` note,
  its length equal to the old count;
- `chat_export`:
  - first and last;
  - N past the end;
  - empty blocks not counted;
  - no title;
  - the settings applied;
- orchestrator: `CopyChat` with each range for a chat and a transcript, and the
  confirmation's count;
- F2: a reply with three rounds copies as one block, in `F5` and `/export`.

**Stage 2 — `feat/block-fold`** (§3.1–§3.5).

Tests:
- `FeedView`:
  - toggle and expanded;
  - a flag toggle empties only its kind's keys;
  - serde: old file, empty set skipped, stable order;
- the feed:
  - a click on each kind's pill, collapsed and expanded, found by reading the drawn
    buffer, toggles that block alone;
  - the other blocks are not rebuilt (their fingerprints and rows are unchanged);
  - the pill stays on its row;
  - following ends;
  - a drag from a pill selects;
  - a double click on a pill is two toggles;
  - a click on the header beside the pill is an ordinary click, and a reference
    there follows;
  - a pill moved to the next row by a narrow panel is still the target;
  - the expanded card's pill, and none on a card with nothing to fold;
  - the live bubble takes no click;
  - the monochrome layout;
- the screen: the click returns `SetFeedView` with the set;
- orchestrator:
  - stored;
  - a transcript's keys stored on its parent;
  - pruning on activation;
  - a clone keeps the set;
  - `modified_at` untouched.

**The live check.** Neither stage reaches the engine, memory, tools or a provider, so
no `#[ignore]` smoke against a model is due (AGENTS.md §3). The fold is checked by:
- a pty probe like the mouse track's: SGR clicks on the headers of a seeded chat,
  the screen read back with pyte;
- the owner's look in Windows Terminal (fold, unfold, restart, the state kept).

**Docs.**
- spec:
  - §11.3: the fold, the stored `feed_view.flipped`, the mouse rows, the key
    table's deferred row removed;
  - §11.2: the copy;
- the manual, the help's mouse row and the command row;
- the screenshot gate's pictures and the demo reel, redrawn for the expanded card's
  pill;
- CHANGELOG;
- the ui-feed journal for the fold and the ui-input journal for the command;
- the roadmap item closed.

## 6. Risks

- **A dead key that survives.** The prune runs on activation; a key removed while
  the chat is open lives until the next activation. It does no harm, because no
  block matches it.
- **A whole-feed toggle after many clicks** loses them. This is by design (D3), and
  the help says what `Ctrl+T` does.
- **The grouping drifting between the feed and the copy.** The copy takes the
  card's rule (§3.6), which the existing agreement test pins to the feed, so there
  is one statement of it below `widgets`, not a new one.

## 7. Not in this track

- A key for folding one block (D3).
- Copying one message by the mouse (D1).
- Folding a user's or the assistant's message body. Only the existing foldable
  blocks fold: thoughts, tool calls, the summary.
- `/copy` of a range in the middle (`/copy 3..5`). Nobody asked for it, and the
  registry's one-token subcommand does not carry it.
