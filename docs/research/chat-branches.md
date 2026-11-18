# Research: branches inside a chat — a fork at a chosen message

**Status:** research (2026-11-18). Five decisions were taken by the owner before it
was written (§5.1). **Forks F1–F5 decided by the owner the same day** (§5.2): F2, F3
and F5 as recommended; F1 as recommended, with a stage of its own added, in which a
tool that reads a chat's line says the chat has branches and a separate tool reads
them (§3.8, stage 4 in §6); and F4 the other way — a click on a number writes the
number alone, and only into a command that takes one (§3.7). **Stage 1 is built**
(2026-11-18, `feat/message-numbers`): the numbers on the messages' rows (§3.5;
[docs/journal/ui-feed.md](../journal/ui-feed.md)). It changed two things in this
document: where a number is made (§3.5), and what a command does with one (§3.6).
**Stage 2 is built** (2026-11-18, `feat/chat-branches`): the tree, `/fork`, `/branch`,
the variants mark and the mouse ([ADR 0015](../decisions/0015-chat-branches.md),
[docs/journal/storage.md](../journal/storage.md)). Live **GO** on Gemma 4 and Gemini.
What it changed here is marked "as built" in §3.1, §3.6 and §3.10: a tail keeps no
draft; the note after a fork is an ordinary note; and a take-back of what a branch
continues from had to park it already, ahead of stage 3.

The roadmap holds two neighbouring ideas (docs/roadmap.md): "regeneration with
variations" and "editing any message, not just the last, with history branching".
Today `Ctrl+E`/`/takeback` edits the last exchange, `Ctrl+R`/`/regen` redoes the
last reply, and `/clone` copies a whole chat. There is no way to go back to a
message in the middle and continue from it, and what a take-back or a regeneration
removes cannot be returned to.

The owner asked for three things to be thought through:

- the fork is **a tree inside the chat**, not a copy of the chat;
- the message is chosen **without the mouse**: messages get numbers, drawn at the
  right end of the header's row, and a command forks by number;
- what the assistant's tools see of the tree.

## 1. Summary

- **The chat stays a line with tails kept beside it** (§3.1). `chat.messages` is
  the conversation in front of the person, as it is today. A new field,
  `chat.branches`, holds every tail that left that line, each with the id of the
  last message it shares with the line it left. A fork cuts the line and keeps the
  tail; a switch swaps two tails.
- **Nothing that reads `chat.messages` changes.** The request, compression,
  reflection, the feed, search, export and the tools go on reading a linear
  conversation. That is also the answer to what the assistant sees: the line in
  front of the person, by construction (§3.8). A later stage lets a tool that
  reads a line say that the chat has branches, and adds a tool that reads them.
- **A fork is a take-back that keeps what it took** (§3.3). `Ctrl+E` and `Ctrl+R`
  already cut the line's end and store it in `chat.deleted`. A fork does the same
  cut anywhere, stores the tail with its anchor, and can put it back. Everything a
  take-back already tells the rest of the app, a fork tells it too (§3.4).
- **A message's number is its place on the line**, counted by the rule the chat
  card and `/copy last N` already share (§3.5). It is drawn muted at the right end
  of the header's row, always. A shared prefix is never renumbered, and the
  variants of one place carry one number.
- **Two commands** (§3.6): `/fork N` forks at message N, `/branch` lists the
  chat's branches and switches between them. After a fork the next thing in the
  conversation is the person's message.
- **The mouse** (§3.7): `‹` and `›` on a message with variants switch between
  them, and a click on a number writes that number into a command that takes one.
- **A fork forks the conversation, not the world** (§3.9). Notes, the self-model,
  the chat's files and the attached project stay as they are. The model is not told
  about a fork.
- **One schema step**, chats 8 → 9, a stamp (§3.2, F3): an older build would drop
  the branches at its first save.
- **Four stages** (§6): the numbers; the tree with `/fork` and `/branch`;
  `Ctrl+R` and `Ctrl+E` kept as branches; the branches told to the assistant's
  tools, with a reader.

## 2. What the code does today

### 2.1 The chat is a line

`Chat.messages: Vec<Message>` (`entities/chat.rs`, spec §5.1) is the conversation,
in order. It is read directly, as a list, across the code. A grep for `.messages`
outside the test directories finds about 250 lines — a request and a sub-agent run
have a field of that name too, so not all are the chat's — and
`app/orchestrator/generation.rs` alone has 51.

### 2.2 What already cuts the line

Three operations remove the line's end, and all three keep it:

| Operation | Where | What leaves |
|---|---|---|
| `Ctrl+R`, `/regen` | `handle_regenerate` | everything after the last user-side row: `messages.split_off(idx + 1)` |
| `Ctrl+E`, `/takeback` | `handle_delete_last` | the last user-side row and everything after it: `messages.split_off(idx)`; the text returns to the input box |
| `rewrite_current_message` | the turn's landing | the reply the assistant replaced |

Each calls `Chat::record_deleted`, which puts a `DeletedExchange` — the messages,
the input box's draft, the time, a `DeletedCause` — at the front of `chat.deleted`.
That list is "for manual recovery" (spec §11.7): no screen shows it and nothing
restores from it. A record does not say where on the line it was cut from.

Reflection reads the causes as a behavioural signal (`behavior_markers`): how many
replies were regenerated or taken back since it last ran.

### 2.3 What hangs off a place on the line

| What | How it names its place | What a cut at the end does to it today |
|---|---|---|
| The rolling summary (`Chat.compaction`, spec §6.7) | `upto` as a fast path, `boundary_id` as the truth | `compaction_view` finds the boundary by id; a boundary that is gone makes the summary ignored, never misplaced |
| The reflection watermark (`Chat.reflected_upto`) | an index | `reflect_window` clamps it to the list's length |
| One block's fold (`FeedView.flipped`) | message ids | `prune` drops the flips whose message is gone |
| Sub-agent and dialogue transcripts | they live on the tool-call record | they leave with the exchange; `Chat::children()` walks the live messages only, so they also leave the chat list, the tasks screen and the `chat://` address book |
| A background run still out | its record's id | `cancel_orphaned_background_runs` cancels it; it lands on the archived copy (`child_mut_including_deleted`) and announces nothing |
| A pending follow-up | the time it was planned | `cancel_orphaned_followups` cancels whatever was planned at or after the last remaining row |
| The full-text index (`cache.db`) | `(message_id, text_hash)` per chat | the next save's diff deletes the rows that left |

The archive is walked for structure in three places: `Chat::reid_children` (a
clone), `Chat::child_mut_including_deleted` (a landing), and the chat-file
migration steps in `shared/storage/chat_steps.rs`, which treat "the messages and
the `deleted` archive alike".

### 2.4 What counts a message

`entities::chat::visible_rows` gives the bubbles the feed draws over a message
list, each as the range of stored messages it shows: a tool result belongs to the
bubble that called it, the rounds of one reply are one bubble unless a round is a
follow-up bubble (`Message::new_bubble`), and a `System` entry or a task
notification is a note row of its own. The chat card's count and `/copy first|last
N` use this rule (docs/research/block-fold-and-partial-copy.md §3.6), and the test
`bubble_count_agrees_with_the_list_counter` ties it to the feed's own projection
(`FeedMessage::from_messages`).

### 2.5 What the assistant's tools reach

- `history_read` and `history_search` (`features/tools/history.rs`) reach "only
  this conversation's own older messages, which the user is looking at in the feed
  the whole time" — the part a summary stands in for.
- `chat_search` and `chat_read` (`features/tools/chats.rs`, off by default) reach
  the profile's other chats through a snapshot made per turn. "Whatever is absent
  from the snapshot does not exist for either tool."
- No tool reaches `chat.deleted`.

The boundary in all three is the same: a tool reads what the person can see in a
feed.

### 2.6 Prior art

The survey in [cloud-chat-import.md](cloud-chat-import.md) §4 covers how other
clients hold branches:

- ChatGPT and DeepSeek store a tree of nodes, each naming its parent, with a
  pointer to the current leaf (DeepSeek has no pointer).
- Tree-native clients (LibreChat, Cherry Studio) keep every branch. LibreChat has
  a run of defects in exactly that structure: branch corruption and endless loops
  (#7003, #9491, #11123, #12313).
- Linear clients either walk the current path or flatten every node into one
  chain, which interleaves a regeneration with the answer it replaced.
- panchat keeps every branch plus an `active_path`.

That research decided to import "only the branch last seen" because mindfork is
linear (its F4). The model below can hold every branch of such an export; taking
them is that track's question, not this one's.

Claude Code's `/rewind` is the precedent for a fork at the person's own message:
the conversation returns to the point before a message the person picks, to be
written again.

## 3. Design

### 3.1 The model: the line, and the tails beside it

```rust
pub struct Chat {
    pub messages: Vec<Message>,   // the line in front of the person — as today
    pub branches: Vec<Branch>,    // every tail that is off it
    // …
}

pub struct Branch {
    /// The last message this tail shares with the line it left.
    /// `None` — the tail starts the chat.
    pub after: Option<Uuid>,
    /// The tail. Never empty.
    pub messages: Vec<Message>,
    pub parked_at: DateTime<Utc>,
    /// What took the tail off the line.
    pub cause: ParkCause,         // Fork | Switch | Regenerate | Takeback
    /// The summary and the reflection watermark in force when it was parked.
    pub compaction: Option<Compaction>,
    pub reflected_upto: Option<usize>,
}
```

- **The tree is implied by the anchors.** Message ids are unique within a chat, so
  an anchor names one place: on the line, or inside another parked tail. Tails with
  the same anchor, and the line's own continuation past that message, are the
  **variants** of one place.
- **A variant is named by its first message.** No branch id is stored. The variants
  of a place are ordered by their first message's timestamp, so "variant 2 of 3"
  means the same before and after any switch.
- **A parked tail is the line as it was left.** Returning to it restores the
  conversation the person walked away from, to its last message, with no "last
  active child" bookkeeping.
- **A tail keeps no draft** (as built). The first draft of this section gave it one,
  on `DeletedExchange`'s rule. A fork is typed as a command, so what the box held at
  that moment is the command; and a switch does not replace the box's text. The
  person's text stays where they typed it.
- **The line cannot be corrupted by the tree.** It is a plain list. A tail whose
  anchor is missing — a hand-edited file — is still a readable list of messages: it
  is kept, logged, listed as detached and never dropped. A walk from a tail to the
  line is iterative with a visited set (the rule of cloud-chat-import.md §7).

### 3.2 Storage and the schema step

- `branches` is a new field with `#[serde(default)]` and
  `skip_serializing_if = "Vec::is_empty"`: a chat with no branch writes the bytes
  it writes today.
- **The chat schema goes 8 → 9, as a stamp** (F3). Additive fields normally need no
  step (ADR 0006 §8). This one does, because of what an older build does with it:
  it reads the line, ignores the unknown key, and writes the file back without the
  branches at its first save — and a typed draft is enough to save a chat
  (`handle_set_draft`). The
  stamp makes that build refuse with "created by a newer version" instead. The
  step touches nothing else, like `settings_to_v4`.
- **Every structural walk learns the new container.** `reid_children` and the
  landing of a background run go over the parked tails as they go over `deleted`.
  A migration step after 9 has three message lists to visit, so the list of them
  becomes one helper in `chat_steps.rs` that every later step calls, with a test
  that a fixture holding all three is fully visited — the "rule kept where the data
  is stored" trap of docs/lessons.md §8.
- `Chat::is_pristine` counts a chat with branches as not pristine.
- `chat.deleted` stays as it is: the anchorless archive of old records. Stage 3
  decides what still goes into it (§3.10).

### 3.3 The operations

Both are pure functions on `Chat`, testable without the orchestrator.

**Fork at a cut.** A cut is a boundary between two bubbles of the line
(`visible_rows` gives the ranges).

1. `tail = messages.split_off(cut)`. An empty tail is not a fork: the command says
   so (§3.6).
2. The tail is parked: `after` is the id of the message before the cut,
   `cause = Fork`, with a copy of the summary and the watermark in force.
3. The watermark is lowered to the cut (§3.4).

**Switch to a variant.**

1. The path from the variant's tail to the line is resolved through the anchors.
   A tail anchored inside another parked tail needs that one on the line first, so
   the steps below run once per tail on the path, outermost first.
2. The line is cut after the anchor. A non-empty remainder is parked
   (`cause = Switch`).
3. The incoming tail's messages are appended, its summary and watermark are
   installed, and its record is removed.

A switch that brings a nested tail in splits the tail around it: of `[p1, p2, p3]`
with a variant anchored after `p2`, `[p3]` is parked after `p2`. The stored shape
follows the history of switches; the tree is the same.

**The invariants**, each a test:

- fork then switch back restores `messages` byte for byte;
- every message id appears once across the line and the tails;
- every anchor names a message on the line or in a tail, or the tail is listed as
  detached;
- no tail is empty.

Both operations are refused while a reply is being generated and while a seat turn
from the messenger bridge holds the chat (`seat_holds_open_chat`) — the conditions
`/regen` and `/takeback` check — and on a sub-agent transcript, which is read-only
by construction (`Orchestrator::view`). Each refusal says why
(docs/lessons.md §4). Neither needs the engine to be ready: nothing is generated.

### 3.4 What a fork and a switch do to what hangs off the line

To everything outside the chat file, both look like the truncation it already
handles, followed — for a switch — by an append.

| What | At a fork or a switch |
|---|---|
| The rolling summary | A parked tail keeps a copy of the summary in force; a switch installs the incoming tail's copy. `compaction_view` still checks the boundary by id, so a summary reaching past a cut is ignored on the new line and the full prefix is sent until the next compaction — a longer prompt, never a summary of what the line no longer holds |
| The reflection watermark | Lowered to the cut. A parked tail keeps its own and gets it back. **The clamp of §2.3 is not enough for a deep fork:** with the watermark at 40 and a fork at 10, `reflect_window` clamps to the length, so the new line's messages up to index 40 would never be reflected. After a take-back the same gap is at most one exchange wide, which is why it has not shown |
| A silent task in flight (a reflection, a compaction) | It lands by index. Its landing checks that the line still holds the message it measured to, and drops its result when it does not |
| Folds | The live set `prune` checks includes the parked tails, so a block's fold survives a switch |
| Transcripts | They travel with their tail. `children()` stays the line's: a parked transcript leaves the chat list, the tasks screen and the address book, as an archived one does. `reid_children` and the landing walk the tails |
| A background run still out | Cancelled, as a take-back cancels it (F5); it lands on the parked copy |
| A pending follow-up | Cancelled by the existing rule (F5) |
| The full-text index | The next save's diff removes the rows that left and adds the rows that came. A parked tail is not indexed |
| The speech in progress | Stopped, as `/regen` stops it |
| `modified_at` | Touched |

### 3.5 Numbers

- **What is counted:** the bubbles of the line, by `visible_rows`. The last
  bubble's number is the count on the chat card, and N in `/copy last N` counts the
  same things. One rule, already under a test.
- **A number is a depth,** so it is stable: a fork, a switch or a regeneration
  changes numbers only past its cut, and the variants of one place carry one
  number. A command names a message of the line in front of the person, so a
  number is never ambiguous where it is used.
- **Where it is made** (as built): the feed counts its own items each frame
  (`numbers` over `FeedMessage::is_numbered`) — the person's and the assistant's
  bubbles and a note with a stored message behind it; the screen's own notes and a
  transcript's persona take none. `FeedMessage::from_messages` merges rounds
  exactly where `visible_rows` does, so over a chat opened from the list the
  numbers are `visible_rows`' ranks; a test holds the two together at every prefix.
  The first draft of this section put a field on `FeedMessage`, filled by the
  projection. Five places build a feed item outside it — the echo, the reply being
  written, a follow-up bubble, a bubble re-opened after a note, the landed turn's
  re-keying, which projects the turn's rows alone — and each would have had to know
  the count.
- **What a live feed shows.** The echo and the reply being written take the next
  numbers at once. A bubble the screen drew and the chat did not keep takes one
  too, until the chat is opened again: the echo of a send that was refused, a reply
  a note split in two that a reload stitches into one. Rare, and the reason a
  command does not send a number (§3.6).
- **Where it is drawn:** at the end of the header's row, one column short of the
  panel's edge (the owner's decision, during stage 1), muted and unbolded like the
  model's name (spec §11.3):

  ```
  ▌ ✦ ASSISTANT  gemini-4-hydrogen                                       #10
  ```

  A note row that has a stored message — a task notification, a director's line —
  has no header, so its number ends its own first row; a first row the text fills
  is wrapped narrower. Without it the numbers on screen would skip.
- **Width:** the role's name is owed its place, then the number, then the model's
  name, which is cut with its "…" or, with fewer than four columns left, dropped. A
  row too narrow for the role and the number drops the number; `/fork` bare still
  reaches every message (§3.6).
- **Selection and copy:** the number is chrome. `RowFact` (`widgets/feed_selection.rs`)
  named only leading chrome (`lead`); it gained the trailing columns (`tail`), so a
  selection is never painted on a number and a copy never takes it. The in-feed
  search does not light or count it either.
- **The render cache:** the number is a property of a block's place, not of its
  message, so it stays out of the fingerprint and is checked beside it.
- **Always on** (the owner's decision): no setting. Every picture of the feed
  changes once — the committed screen dumps and the generated demo.
- A sub-agent transcript is numbered the same way; `/fork` is refused there.

### 3.6 The commands

Both are rows of the registry in `features/ui_command.rs`, on the help overlay's
"Commands" tab.

**A command resolves its number on the screen** (found in stage 1, §3.5): the
screen looks up the bubble that shows `#N` and sends the id of the message it
carries, not the number. What the person sees numbered is then what the command
acts on, whatever a live feed's count has drifted to; a bubble with no stored
message yet is answered with a note. `/copy first|last N` is not of this kind: it
counts from an end of the stored conversation and names no message.

**`/fork N`** — the conversation continues from message N.

| N is | The cut | The input box |
|---|---|---|
| a reply of the assistant | after it | as it was |
| the person's own message (F2) | before it | the message's text, to edit and send |
| a note row (a notification, a director's line) | before it | as it was — what `/takeback` does with a notification today |

The rule behind the table: **after a fork, the next thing in the conversation is
the person's message.** On the person's own message that is `/takeback` at any
depth: the message is written again, and what followed it is kept as a branch. It
is not an edit in place. `/edit N`, if it is added, is a different command: it
changes a message's text where it stands, removes nothing under it and makes no
branch (§8).

- `/fork` with no number opens a list of the line's messages — number, role, first
  line — to pick one with the arrows and `Enter`. It is the route when the window
  is too narrow to draw the numbers.
- `/fork N` where nothing follows N says that the conversation already continues
  from there.
- `0`, a number past the end and a token that is not a number are answered with
  the valid range.
- After a fork a note under the last message says what was kept and how to return:
  the count of messages in the parked continuation and the `/branch N K` that puts
  it back. It is derived from the chat's state — the line ends on a message that
  tails are anchored to — so it is drawn again when the chat is reopened. As built
  it is an ordinary note of the screen: it stays above the next message for the
  session, like any other, and is not there at the next opening.
- **A message sent in the session is forked at like any other** (found by the
  stage's pty run): its echo carries the stored message's id from its first frame.
  Without it the commonest fork — at the message just sent — was refused until the
  chat was opened again.

**`/branch`** — the chat's branches.

- Bare, it opens a list of the parked tails, each with the number and variant it
  would have on the line, the first line of its first message, its message count
  and its time; nested tails are indented under the tail that holds their anchor,
  detached ones are marked. `Enter` switches.
- `/branch N K` switches message N to its variant K — the typed route, with no
  list.

### 3.7 The variants mark, and the mouse

- The first bubble of a continuation that has other variants carries a mark before
  its number: `‹2/3› #11`.
- With the mouse captured, `‹` and `›` take a click and switch to the neighbouring
  variant. They are the only route that needs no command; they are drawn in the
  stage that makes them work.
- **A click on a number writes that number, and nothing else, into a command that
  takes one** (F4, the owner's decision). When the input box holds a command whose
  argument is a message's number — `/fork`, `/branch` — a click on `#10` sets that
  argument to `10`: written where there is none, replacing the one that is there.
  Nothing runs until `Enter`. With anything else in the box — nothing, a message
  being typed, another command — a number takes no click, and the press is the
  feed's own.
- **Which commands those are is a column of the registry**
  (`features/ui_command.rs`): a row says that its first argument is a message's
  number. A command added later joins by its row, not by a list kept at the click.
- **Proposed with the decision, not part of it: a number looks like a target while
  it is one.** The rule of the captured mouse is that what takes a click looks like
  it, and nothing else does. So while such a command is in the box the numbers are
  drawn in the accent colour — by a paint pass over the visible rows, on the
  trailing columns `RowFact` names (§3.5), not by a rebuild of the render cache,
  which a keystroke must not cost.

### 3.8 What the assistant sees

**The line in front of the person, and nothing else.** No tool reaches a parked
tail: `history_read`/`history_search` are built over the line's compressed part,
the snapshot behind `chat_search`/`chat_read` holds the other chats' lines, the
index holds no parked row, and the request is built from `chat.messages`. None of
that is a filter added for branches; it is what those paths already read.

Why this, and not the tree:

- A fork says "continue as if what followed had not been said". A search hit from
  the parked tail brings back exactly what was cut, and the model finds replies of
  its own that its history does not hold.
- It is the boundary the tools already have (§2.5): what a tool can read is what
  the person can see in a feed. The `deleted` archive is outside it today for the
  same reason.
- Nothing about a fork enters the prompt (the owner's decision), so the request's
  prefix up to the cut is the one already sent and a provider's prompt cache still
  holds up to there. The exception is a fork that leaves a summary behind (§3.4):
  the verbatim prefix replaces the summary block, once.

**A stage of its own makes the branches known to the tools** (F1, the owner's
decision; stage 4 in §6). Two things, together:

- a tool that reads a chat's line says that the chat has branches;
- a separate tool reads them — "compare this with what you answered in the other
  branch" is its use.

The line stays what every existing tool reads and searches. A parked tail is
reached only through the reader, by a call that names it. D5 stands beside this: a
fork still puts nothing into the prompt.

### 3.9 A fork does not rewind the world

Notes, the self-model, planned follow-ups already delivered, the chat's files
(`data/files/<chat-id>/`), attachments, a story, and every file the assistant
changed in the attached project live outside `chat.messages`. A fork leaves them as
they are: a note saved in the parked tail is still in memory, an edited file is
still edited. This is what a regeneration does today on a smaller scale — the
reply that saved a note is replaced, the note stays.

- The changes screen (`F4`) still shows and reverts what the assistant did to the
  project, whichever branch did it.
- The manual says it in one sentence where it describes `/fork`.

### 3.10 `Ctrl+R` and `Ctrl+E` as branches (stage 3)

Decided by the owner: after the tree is in, a regeneration and a take-back park
their tail as a branch instead of archiving it.

**One part of this could not wait for the stage** (found by stage 2's pty run). A
take-back or a regeneration after a fork can remove the message a parked tail
continues from; in the archive that message is gone, and the tail has no place to
return to. So stage 2 already parks the removed messages **where a branch continues
from them** (`Chat::park_if_anchored`), with the cause below, and archives as before
everywhere else. Stage 3 drops the condition.

- `Ctrl+R` parks the old reply (`cause = Regenerate`); the new reply is variant 2
  of that place, and `‹1/2›` returns to the old one. That is the roadmap's
  "regeneration with variations" as far as keeping the variants goes; generating
  several at once is not part of it.
- `Ctrl+E` parks the exchange (`cause = Takeback`). It is `/fork N` on the last
  message of the person.
- `rewrite_current_message` keeps writing to `chat.deleted`: the replaced text is
  the assistant's own draft, not a continuation the person may return to.
- **Reflection's signal** moves with the cause: `behavior_markers` counts the tails
  parked by a regeneration or a take-back since it last ran, plus the old
  `deleted` records. A reply the person switched back to is on the line and is not
  counted — going back to a reply is not a sign it did not land.
- `chat.deleted` stops growing except by rewrites; its old records stay, anchorless.

## 4. Rejected

- **A pool of nodes with parent pointers** (ChatGPT's `mapping`). The same tree,
  stored so that `messages` stops being the conversation: every reader of §2.1
  changes, every chat file is rewritten by a real migration, and a damaged pointer
  damages the line the person is reading (§2.6).
- **A copy of the chat per fork.** The shared prefix is stored twice, matched twice
  by search and reflected on twice; the chat's files and its project belong to one
  of the copies. "A branch as a chat of its own" stays available as `/clone`, then
  `/fork N` in the copy.
- **Anchors on `chat.deleted`'s records** instead of a new field. One container
  less to walk, but `deleted` means "gone" to its readers — `children()`, the tasks
  screen, the address book — and holding the person's live branches under that name
  makes the file say the opposite of what it holds. New causes would be new values
  of `DeletedCause`, which an older build fails to parse (docs/lessons.md §8), so
  it saves no schema step either.
- **A message cursor in the feed** — a mode in which the arrows move a mark from
  bubble to bubble. A new modal state of the chat screen, for what a number and a
  command do without one.

## 5. Decisions and forks

### 5.1 Decided by the owner (2026-11-18)

- **D1.** The fork is a tree inside the chat, not a copy of it.
- **D2.** Messages are numbered, the number stands at the right end of the
  header's row, and a command forks by number, so the feature does not depend on
  the mouse.
- **D3.** The numbers are always shown; there is no setting.
- **D4.** `Ctrl+R` and `Ctrl+E` become branches, in a stage of their own after the
  tree (§3.10).
- **D5.** The model is told nothing about a fork.

### 5.2 Decided by the owner the same day

- **F1 — what the assistant's tools reach.**
  - (a) **The line in front of the person only.**
  - (b) (a), and later an opt-in reader of a named variant, if a need appears.
  - (c) The whole tree, through the existing search tools.
  - Recommended: (a) now, (b) when asked for (§3.8).
  - **Owner's decision 2026-11-18: (a) — and (b) not on demand but as a stage of
    this track:** a tool that reads the line says that the chat has branches, and a
    separate tool reads them (stage 4 in §6).
- **F2 — `/fork N` on the person's own message.**
  - (a) **The cut is before it and its text goes to the input box.**
  - (b) The cut is after it and a new reply is generated at once.
  - (c) Two commands: `/fork N` for (b), `/edit N` for (a).
  - Recommended: (a). One rule covers every kind of message (§3.6), and sending
    the text unchanged gives (b).
  - **Owner's decision 2026-11-18: (a).** And (c) misnamed its second command:
    `/edit N`, if it is added, edits a message in place — the messages under it are
    not removed and no branch is made; it is for a typo (§8).
- **F3 — the storage.**
  - (a) **`branches` beside `messages`, and the schema stamp 8 → 9.**
  - (b) `branches` with no stamp, as other additive fields are. An older build
    then drops the branches in silence.
  - (c) Anchors on `chat.deleted`'s records (§4).
  - Recommended: (a).
  - **Owner's decision 2026-11-18: (a)** — there is no doing without the schema
    step.
- **F4 — the mouse on a number.**
  - (a) A click writes `/fork N` into the input box; nothing runs until `Enter`.
  - (b) A number takes no click; the mouse switches variants (`‹`, `›`) and nothing
    else.
  - Recommended: (a).
  - **Owner's decision 2026-11-18: neither.** A click writes the message's number
    alone, and only when the input box holds a command that takes one (§3.7).
- **F5 — a background run or a pending follow-up in a tail being parked.**
  - (a) **Cancelled, as a take-back cancels them today.**
  - (b) The run goes on and lands in the parked tail without a notification; the
    follow-up waits until the tail is back on the line.
  - Recommended: (a). A notification has to land on a line, and the one it
    belongs to is not in front of anyone.
  - **Owner's decision 2026-11-18: (a).**

## 6. Stages, tests, the live check

Each stage is a branch and a pull request of its own.

1. **The numbers** (`feat/message-numbers`) — **built 2026-11-18.** §3.5 and
   nothing of the tree: the number on the header and on a note row, the width rule,
   the trailing chrome, the cache. Tests: the numbers of a feed equal
   `visible_rows`' ranks; a copy of a selection across a header holds no number;
   the small-window gate (spec §11.1.1). The chat's screen dumps were regenerated.
   Pure UI: no live run against a model; a pty run of `mindfork demo`, and the
   owner's look in Windows Terminal.
2. **The tree** (`feat/chat-branches`) — **built 2026-11-18**, live **GO** on
   Gemma 4 (llama.cpp) and on Gemini through the gateway, the control red.
   §3.1–§3.4 and §3.6–§3.7: `Branch`, the stamp with its fixture, the two
   operations, `/fork`, `/branch`, the mark, the hooks of §3.4.
   - Unit tests: the invariants of §3.3; nested forks; the summary and the
     watermark across a fork before and after the boundary; a silent task landing
     after a fork; a clone of a chat with branches; a file with branches is "a
     newer version" to a reader at schema 8 — a round trip alone cannot fail on a
     version change (docs/lessons.md §8).
   - Wire tests: a line that ends between two bubbles of one turn — after a
     follow-up bubble's control call — is a new kind of line end; one request per
     client.
   - **Live, go/no-go:** on a forked line, a turn answers from the kept prefix and
     shows no knowledge of the parked tail; `history_search` and `chat_search` do
     not return a parked message; the token counter's cache share holds on the
     first turn after a fork; a fork before the summary's boundary sends the full
     prefix and compacts again. Gemma 4 locally and one cloud wire.
3. **`Ctrl+R` and `Ctrl+E` as branches** (`feat/regen-as-branch`). §3.10. Tests:
   the two keys park with their causes; `behavior_markers` over tails and old
   records; `rewrite_current_message` still archives. Live: a reflection after a
   regeneration reads the signal.
4. **The branches told to the tools, and a reader** (`feat/branch-reader`). F1's
   second half (§3.8). Its design is an addendum to this document, with forks of
   its own, before any code. What it has to settle:
   - which reads say that a chat has branches — `chat_read` over another chat's
     line, `history_read` over the open chat's compressed part — and whether a
     search hit says it too;
   - what is said: the number of branches, and the reader's name only on a turn
     that offers the reader — the rule by which the summary block names
     `history_read` (S12, spec §9.4);
   - the reader: a variant addressed the way `/branch N K` addresses it, paged
     like `chat_read`; whether it reaches the open chat's own parked tails or
     other chats' only; whether it is on by default;
   - whether a search reaches a parked tail at all.

   Live, go/no-go: a model told of branches by a read reads one when asked to
   compare, and leaves them alone when not asked.

The documentation of each stage follows AGENTS.md §4: spec §5.1, §9.11, §11.3,
§11.7 and §12.2, architecture §7, §8 and §10, the manual and README's command
tables, the help overlay, CHANGELOG (Added; **Data** for the stamp), and the
journal (ui-feed.md for stage 1, storage.md for stages 2 and 3, tools.md for
stage 4).

## 7. Risks

- **The chat file grows.** A parked tail keeps its messages whole, images
  included. `deleted` has the same property today and no bound either.
- **Two containers hold archived messages** until stage 3 narrows `deleted` to
  rewrites. The helper of §3.2 is what keeps a later migration from visiting only
  one.
- **A landing by index after the line moved** (§3.4) is the kind of defect that
  shows only with a background task in flight. It gets a test per task, not a
  shared guard added afterwards.
- **The mark and the note of §3.6 are derived state.** Both are computed from the
  anchors at activation; neither is stored, so neither can disagree with the file.
- **The person's own search does not see a parked tail** in this track. A word said
  only in a parked branch is not found by the chat list's search or by `Ctrl+F`
  until that branch is on the line.

## 8. Not in this track

- Search by the person across parked branches, with a hit that opens the chat on
  that branch.
- Removing a branch. Nothing is removed; a hidden flag on a tail, on the soft-delete
  convention, when asked for.
- A mark of branches on the chat card, and parked transcripts in the chat list.
- Generating several variants of a reply at once, or with different sampling.
- `/copy N..M`, `chat://` addresses of a message by number.
- Importing every branch of a ChatGPT, Claude or DeepSeek export
  (cloud-chat-import.md F4).
- **`/edit N`** — changing a message's text where it stands: nothing under it is
  removed and no branch is made; for a typo (the owner's definition, 2026-11-18).
  A command of its own if it is added, not a spelling of `/fork N`. It would use
  stage 1's numbers and the registry column of §3.7.
- `/fork` from the messenger bridge.
- Restoring a forked message's images to the input box: `/fork N` returns the text,
  as `/takeback` does.
