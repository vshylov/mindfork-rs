# Research: where the feed stands after a chat is activated

**Status:** research (2026-11-19). Three decisions were taken by the owner before it
was written (§4.1). **Forks F2 and F3 decided by the owner the same day**, both as
recommended, after the look at stages 1 and 2 that also let F1 stand (§4.2, §4.3). **Stage 1 is
built** (2026-11-19, `feat/feed-landing`): the three reported cases, finished
transcripts and `/greeting` from the start
([docs/journal/ui-feed.md](../journal/ui-feed.md)). Its run through a pty found one
thing the design had missed (§3.7). **Stage 2 is built** (2026-11-19,
`feat/feed-refresh`): what arrives by itself is a refresh, the input box is loaded
only when the chat changes, and a woken turn does not scroll a reader away (§3.6).

The owner reported three cases of one complaint: the feed goes to its last row when
the person was looking somewhere else.

- Switching a message's variant (`/branch N K`, a click on `‹2/3›`) sends the feed to
  the bottom, though the person reads from the message whose variant changed.
- Following a `chat://` reference opens the other conversation at its end, though the
  person wants its beginning.
- Going back from a followed reference opens the first conversation at its end, and
  the reference that was followed is usually out of sight.

## 1. Summary

- **One cause.** Every opening and every rebuild of the feed ends in
  `ChatScreen::activate_chat`. It has two outcomes: a message to stand on, which only
  a search hit sends, or `scroll_to_bottom()`. Nothing else can say where the feed
  should stand, so everything else gets the bottom (§2.1).
- **The fix is a value that says where**: `FeedLanding`, sent with every activation in
  place of today's optional focus (§3.1). Six landings cover every route (§3.2).
- **A place in a feed** is a message and a row inside its block. It is read from the
  frame last drawn, and it survives the feed being rebuilt (§3.4). The way back from a
  reference keeps one (§3.5).
- **A variant switch does not move the feed when the message's header is on the
  screen.** The arrows of `‹2/3›` stay under the pointer, so the variants can be
  clicked through. A header that is off the screen is put on the first row (§3.3).
- **Found on the way, and the larger defect:** content that arrives in the open chat
  by itself is delivered by the same activation. A background run's result, a
  follow-up, rows from the messenger and a silent reply each send a reader to the
  bottom, close the in-feed search, drop the selection, close the open list, and
  reload the input box, which moves its cursor to the end and empties its undo
  history (§2.2). Stage 2 makes that delivery a refresh that touches nothing of the
  person's (§3.6).
- **Three pull requests** (§5). None touches the engine, so no run against a live
  model is due. A probe of the built binary through a pty and the owner's look in
  Windows Terminal check them.

## 2. What the code does today

### 2.1 One funnel, two outcomes

`Orchestrator::activate_focused` (`src/app/orchestrator/mod.rs`) builds
`AppEvent::ChatActivated` for every route. The runtime hands it to
`ChatScreen::activate_chat` (`src/screens/chat/feed.rs`), which rebuilds the feed
from the chat's messages and then does one of two things:

```rust
match &focus {
    Some(f) if self.feed_view.focus_message(&self.feed, f.message, Some(&f.query)) => {}
    _ => self.feed_view.scroll_to_bottom(),
}
```

`focus` is `Some` only for a search hit (`AppCommand::OpenChatAt`,
`OpenChatAtFirstMatch`, and a hit in a parked branch). The three reported cases send
`None`:

| Case | Route | Why the bottom |
|---|---|---|
| a variant switch | `handle_switch_branch` → `after_line_moved` → `activate(chat)` | the orchestrator has the variant's first message in hand and does not pass it |
| a followed reference | `ChatIntent::OpenChatLink` → `AppCommand::SwitchChat` | "an ordinary chat switch", by the words of spec §11.3 |
| the way back | `Back::Link { origin, chat }` → `AppCommand::SwitchChat(origin)` | the stash holds the chat's id alone; its comment says a chat "is reopened from storage in full, so there is no screen state a re-activation would lose" |

None of the three was a decision. [chat-branches.md](chat-branches.md) does not say
where the feed goes after a switch, and the reference's design decided that an
address names a conversation and not a message, which is about the address.

### 2.2 The same activation delivers what arrives by itself

Thirteen places call `activate`. Six of them are not the person opening anything:
the open chat got new rows, and the feed is rebuilt to show them.

| Call | What arrived |
|---|---|
| `deliver_notification` (`background_runs.rs`) | a background run's result |
| `deliver_due_followups` (`followups.rs`) | a planned follow-up |
| `push_bridged` (`bridge.rs`) | rows from the messenger |
| `land_seat_reply` (`generation.rs`) | a messenger's turn landed |
| `handle_done`, twice (`generation.rs`) | a woken turn chose silence, and its bubble goes |

`activate_chat` cannot tell these from an opening, so each of them:

1. scrolls to the bottom, whatever the person was reading;
2. closes the in-feed search (`Ctrl+F`) and forgets its query;
3. drops the feed's selection and the marker of a jump;
4. closes the references list and the branches list;
5. reloads the input box from the chat's stored draft. `InputBox::set_text` puts the
   cursor at the end and empties the undo history. The stored draft is the box's own
   text sent a tick earlier, so a keystroke that has not been sent yet can be
   overwritten. This last part is read from the code and was not reproduced.

Spec §11.3 already has the rule this breaks: content arriving on its own scrolls only
if the view was following the tail. It was written for a tool card and a note
([chat-search-stage2.md](../history/chat-search-stage2.md) §4), which are appended.
What is delivered by a rebuild was never brought under it.

A woken turn has the same defect one step later. `begin_generation` scrolls to the
bottom "because you pressed send", and a turn the assistant starts on a notification
goes through it too: `AppEvent::GenerationStarted` does not say who asked.

### 2.3 What the feed widget has

`MessageFeed` keeps `scroll` (a row), `follow`, and three things a jump sets
(`pending_focus`, `anchor`, `marker`). A jump becomes a row inside `render`, because
a row needs the block cache at the panel's width
([chat-search-stage2.md](../history/chat-search-stage2.md) §1.1). `DrawnRows` records
the last frame: the scroll, and where each block's rows begin. That record is enough
to say which message is on the first row and how far into it.

Missing: a way to the first row, a way to a row inside a block, and "leave it where
it is if it is on the screen".

### 2.4 Three more of the same kind

- **A rewrap after a manual scroll loses the place.** `anchor` is set by a jump and
  dropped by the first manual scroll, on purpose (fork S6: a resize must not pull a
  reader back to the hit). After that `scroll` is a bare row number, and a resize,
  `Ctrl+T`, `Ctrl+O` or a theme change rewraps every block under it.
- **A search hit lands on the message's first row**, not on the match. In a message
  taller than the window the highlighted word is below the fold.
- **A rebuild drops the feed's notes.** They are not messages, so a feed rebuilt from
  the chat has none. A notification landing removes the error note above it.

## 3. Design

### 3.1 The value

`entities::feed_landing` (below both layers that use it, re-exported by
`app/events.rs`, like `live_turn`):

```rust
/// A row of a chat's feed that can be found again after the feed is rebuilt.
pub struct FeedPlace {
    /// The first message of the bubble.
    pub message: Uuid,
    /// The row inside that bubble's block, from its first.
    pub row: usize,
}

/// Where the feed stands once a chat is activated.
pub enum FeedLanding {
    /// The end of the conversation, following it.
    Tail,
    /// The first row.
    Top,
    /// On a message, marked, the query highlighted inside it. A search hit.
    Jump { message: Uuid, query: String },
    /// On a message whose content has just changed: not moved when its first
    /// row is on the screen, put on the first row otherwise.
    Reveal { message: Uuid },
    /// Where the person was. The way back.
    Place(FeedPlace),
    /// Nothing of the person's is touched (stage 2).
    Hold,
}
```

`AppEvent::ChatActivated::focus: Option<FeedFocus>` becomes `landing: FeedLanding`.
`FeedFocus` goes: `Jump` is the same two fields. A landing that cannot be honoured (a
message the feed does not show) falls back to `Tail`, as a jump does today.

### 3.2 Who sends what

| Route | Landing |
|---|---|
| the chat list, `Ctrl+N`, a clone, the next chat after a delete, the last chat at startup | `Tail` |
| a search hit, the first match from the chat list, a hit in a parked branch | `Jump` |
| a followed `chat://` reference: a click, the references list | `Top` |
| `Esc` back from a followed reference | `Place`, saved when it was followed; `Tail` where the screen had no row to name |
| a transcript that is not running, by any route that asks for `Tail` (the owner's decision, §4.1) | `Top` |
| a variant switch: `/branch N K`, a click on the mark, the branches list | `Reveal` of the variant's first message |
| `/fork N`, `Ctrl+E`, `Ctrl+R` | `Tail`: the cut is the new end |
| `/greeting` | `Top`: the chat is one message, and it is read from its start |
| what arrives by itself (§2.2) | `Hold` (stage 2; `Tail` until then, as today) |

A running transcript keeps `Tail`: it is watched as it grows.

Two commands carry a landing from the screen's side. `AppCommand::SwitchChat(id)`
stays the ordinary switch. A new `AppCommand::OpenChat { chat, landing }` is what a
followed reference and its way back send.

### 3.3 A variant switch

The mark is clicked where it is drawn, and a second click usually follows the first:
the person is comparing variants. So the header keeps its screen row whenever it has
one.

The screen reads its place before the feed is rebuilt (§3.4) and gives it to the
widget with the message to reveal. In `render`, with the new rows known:

1. the view is put back on the held place;
2. if the revealed message's first row is inside that view, nothing more happens;
3. otherwise the message's first row becomes the view's first row.

The feed stops following its tail in both cases, as it does after a click on a block's
pill ([block-fold-and-partial-copy.md](block-fold-and-partial-copy.md) §3.4), and
resumes when the view is at the last row. A held place that is gone (the view's first
row was inside the variant that left) counts as "otherwise".

**The feed does not scroll past its end**, and two things follow, both seen in the pty
run. A variant shorter than the one it replaces, switched to at the feed's last rows,
leaves fewer rows than the view holds below the header: the header moves down by the
difference, once, and every switch after that leaves it where it is. And a header in
the feed's last screenful cannot be the view's first row: "put on the first row" there
is the last screenful, with the header on it. Keeping the header's row in both cases
would take a view that can stand below the feed's end, which nothing else in the
widget allows; not built.

The note the switch writes ("variant 2 of 3 at #11") is appended at the feed's end and
no longer drags the view there. The mark on the header says the same.

### 3.4 A place

Read from the last drawn frame: the block holding the view's first row, and the row
inside it. The screen turns the block into the first message id of its bubble.

- A block with no message behind it (a note, a transcript's persona, a reply still
  being written) is not a place. The next bubble below that has an id stands in, at
  its row 0. If there is none, the view is in the feed's last rows, and the place is
  the tail.
- **Restored once.** Fork S6 refused to *store* a row inside a block, because a rewrap
  changes the block's height. A place is applied by one `render` and then forgotten:
  from there the view is tied to the message, as after a jump. A row past the block's
  end (the window was resized while away) is clamped to its last row.
- Not written to disk. It is session state, like the way back that holds it.

### 3.5 The way back

`Back::Link { origin, chat }` gains `place: Option<FeedPlace>`, read from the screen
when the reference is followed. `Esc` sends `OpenChat { chat: origin, landing }` with
that place. A feed that was following its tail is given its place too, not `Tail`: the
last screenful restored is the tail again when nothing arrived meanwhile, and the
reference is still on the screen when something did. `Tail` is for a screen with no
row to name.

What does not come back is the feed's notes (§2.4): the rows above a note return
exactly, and the rows below one stand that note's height higher.

Everything else about the stash is unchanged: one step deep, consumed on use, dropped
by leaving for another chat or by working in the one drilled into (spec §11.3).

### 3.6 A refresh (stage 2)

`Hold` on the chat that is already open rebuilds the feed and nothing else:

- **The view.** Following the tail: it still does. Not following: the place read
  before the rebuild is restored after it, so the rows on the screen do not move.
- **The in-feed search, the selection, the jump's marker, the open lists.** Kept. The
  rows above a delivery do not change, the search's matches are derived every frame,
  and a selection is tied to its blocks.
- **The input box.** Not touched. Stage 2 makes this true of every activation of the
  chat that is already open, whatever its landing: the box is where the draft is
  written, so the stored copy has nothing to add to it. The box is loaded from a
  chat's draft only when the chat changes.
- **A turn nobody asked for.** `AppEvent::GenerationStarted` gains `woken: bool`,
  the orchestrator's own word for a turn it started on a notice. Its bubble scrolls
  into view only if the feed was following. A regenerated woken reply is woken too,
  and still shown: the rebuild before it asks for the tail.

As built, three things the list above did not say:

- **The token counter** keeps the last turn's figures through a refresh. An
  activation clears it because it belongs to the chat that was open; a refresh is the
  same chat's.
- **A jump's mark is found again by message id** (`MessageFeed::marked`, `remark`).
  It is a feed index, and the rebuild drops the feed's notes. In practice no note
  stands above a mark, since a jump is itself a rebuild; the lookup costs nothing and
  does not depend on that.
- **A landing no frame has drawn yet is left alone.** A refresh can arrive between an
  activation that asked for a landing and its frame; restoring the old view's place
  then would cancel what was asked.

`Hold` for a chat that is not the open one is `Tail`: there is nothing to hold.

### 3.7 A landing ends the following when it is asked for (found by the pty run)

A landing is asked for by an activation and applied by the next frame. The
orchestrator sends a note right behind two activations: a switch says which variant is
on the line, and a search hit in a parked branch names the branch. Both events are
applied before a frame is drawn. A note scrolls a feed that is following its tail to
its end (`scroll_to_bottom_if_following`), and that drops a pending request.

So the first build of stage 1 passed every test and did nothing in the app: a click on
`‹` at the tail was a reveal, then a note, then the tail. The screen's tests applied
one event and drew. The same sequence was already wrong on `main` for a hit in a
parked branch, which jumps and then says where the match is: opened from a feed that
was following, it landed at the tail.

`MessageFeed::land` now ends the following as it takes the request, for a jump, a
place and a reveal alike. A test applies the activation, the note and then the frame,
for every landing.

## 4. Decisions

### 4.1 Taken by the owner before this was written (2026-11-19)

| | Decision |
|---|---|
| **D1** scope | The three reported cases **and** what arrives by itself (§2.2), in one track |
| **D2** transcripts | A finished sub-agent transcript opens **from its start** |
| **D3** the chat list | An ordinary switch opens **at the end**, as today. No position is remembered per chat |

### 4.2 Proposed with the assessment, built as proposed

**F1. Where a variant switch leaves the feed.** (a) the message always on the first
row; (b) not moved when its header is on the screen, on the first row otherwise.
**(b)**, §3.3. Proposed to the owner with the assessment; D1–D3 answered the questions
asked and left this as proposed. **The owner's look in Windows Terminal (2026-11-19)
confirmed it:** everything works as it should.

### 4.3 Decided by the owner, 2026-11-19 — stage 3

**F2: (b)**, the view keeps what is on the screen through a rewrap. **F3: (b)**, a
search hit lands with its match in view. The options as they stood:

**F2. The view keeps its message through a rewrap after a manual scroll** (§2.4).
With a place readable from the last frame, a rewrap can restore it. This changes what
S6 decided for a jump only in the direction S6 wanted: the view is tied to what is on
the screen now, not to the hit.
(a) leave as is; (b) restore the place on every rewrap. Recommended: **(b)**.

**F3. A search hit lands with its match on the screen** (§2.4).
(a) the message's first row, as today; (b) the first row when the match fits under
it, otherwise the match a few rows below the view's top. Recommended: **(b)**.

## 5. Stages

| Stage | Branch | What |
|---|---|---|
| 1 | `feat/feed-landing` | `FeedLanding` and `FeedPlace`; the widget's first row, a row inside a block and a reveal; the three reported cases; finished transcripts and `/greeting` from the start |
| 2 | `feat/feed-refresh` | `Hold` for what arrives by itself; the input box loaded only when the chat changes; a woken turn that does not scroll a reader away |
| 3 | `feat/feed-rewrap` | F2 and F3, as the owner decides |

Each stage: unit tests at the widget (rows), the screen (what a frame shows after an
activation), the runtime (the stash) and the orchestrator (the landing each route
sends). A pure interface change: **no run against a live model is due.** The built
binary is driven through a pty on a chat with variants and references, and the owner
looks at it in Windows Terminal.

## 6. Out of scope

- **A position remembered per chat** for the ordinary switch (D3).
- **A back-stack deeper than one step** (docs/roadmap.md): a chain A → B → C still
  steps back to B and no further.
- **Notes that survive a rebuild** (§2.4). A note would need a stored position between
  two messages. Recorded here; nothing has asked for it.
- **An indicator of new rows below a reader.** The status chip already shows a running
  turn, and the scrollbar shows the feed grew.
