# Sub-agent chats, stage 2: the transcript while it runs

Design plan for PR 7 of the sub-agent track
([research](research/subagent-chats.md) §3.5 "stage 2", §7 stage 7;
[ADR 0010](decisions/0010-subagent-nested-turn.md)). Stages 1–6 are merged
(#356–#361). This document decides how a running sub-agent becomes visible
*before* its turn lands — the one thing stage 1 deliberately left out
(research §4 item 2). Forks in §5 need the user's confirmation before
implementation (AGENTS.md §1).

## 1. What exists, and what the gap is

A `call_subagent` run is a nested `TurnLoop` inside the parent's generation
task. Until the parent's turn lands (`handle_done`), the orchestrator knows
nothing of the run: the child's events are muted by its `RoundSink`, its
messages accumulate in the child loop's `messages`, and the record with the
`SubagentRun` is built only when the call returns. What the user sees
meanwhile: the parent's "generating" state, the token counter growing, and —
since PR 6 — the status-bar chip *"sub-agent «name» · round N · tool"*.

The gap: a delegation of ten rounds is minutes of a chip. Nothing to open, no
row in the list, and **any chat switch cancels the turn** (`switch_to` calls
`request_cancel` before it looks the target up — `chats.rs`), so there is no
way to look elsewhere and come back.

Facts the design rests on:

- **`done_tx` is the task → orchestrator channel**, one per orchestrator,
  carrying `GenResult` at the end of a turn. Anything sent on the same channel
  before the result is delivered before it — FIFO by construction.
- **The orchestrator is the sole owner of `Chat`**, and `view(id)` is the
  single resolver the list, the screen, titling, copy and export use to find
  a transcript (PR 4). Whatever holds a running transcript must be reachable
  through `view()` for all of those to work unchanged.
- **The screen has one feed**, keyed by the active chat, accepting streamed
  events only for `current_gen`. `activate_chat` rebuilds the feed from a
  message list and resets the generation state.
- **`file_round`** is the one place a loop's round becomes domain messages
  (`self.messages`), at depth 0 and 1 alike — the natural progress hook.

## 2. Goal

While a sub-agent runs:

1. its transcript is a **row of the list** under its parent, marked *running*;
2. it **opens** (read-only, as a landed one does) showing the rounds filed so
   far, and **grows** as further rounds file;
3. **switching between the parent and that child — either direction — does
   not cancel the turn**; any other switch cancels as today;
4. the parent's bubble shows a **running card** for the call until it returns
   (deferred to a follow-up PR — fork F7);
5. when the turn lands, everything the user looked at is exactly what stage 1
   would have shown: the landed run on the record, the parent's feed whole.

Out of scope: token-level streaming of the child's text into its feed (rounds
are the unit — fork F3), a second feed buffer in the screen, persisting the
in-flight state (a crash loses the turn as it does today).

## 3. Design

### 3.1 One channel, two kinds of message

`done_tx: UnboundedSender<GenResult>` becomes `UnboundedSender<GenMessage>`:

```rust
enum GenMessage {
    Progress { id: Uuid /* generation */, progress: TurnProgress },
    Done(GenResult),
}
enum TurnProgress {
    /// The parent's loop filed a round (depth 0): its assistant message and
    /// the tool messages, exactly what `file_round` pushed.
    RoundFiled(Vec<Message>),
    /// `run_subagent` is about to run the child: the run as it will land —
    /// id, persona, title, `name`, `created_at`, `User(message)` — with no
    /// rounds yet and no outcome.
    ChildStarted(Box<SubagentRun>),
    /// The child's loop filed a round (depth 1).
    ChildRoundFiled(Vec<Message>),
    /// The run returned: its outcome, `finished_at`, tokens — the landed run
    /// carries the same, this just lets the list show it early.
    ChildEnded { outcome: RunOutcome, finished_at: DateTime<Utc>, tokens: u64 },
}
```

Sent from `file_round` (by `depth`) and around the child in `run_subagent`.
The orchestrator's `select!` arm matches on the kind. **FIFO with `Done` is
what makes the table below safe to drop at landing**: every progress message
of a turn precedes its result.

### 3.2 The in-flight table

```rust
struct InflightTurn {
    gen: Uuid,
    chat: Uuid,
    /// The parent's rounds filed so far (not yet in `Chat.messages`).
    rounds: Vec<Message>,
    /// The running (or just-ended, not yet landed) sub-agent.
    child: Option<SubagentRun>,
    /// The parent was switched away from and back while the turn ran, so its
    /// feed lost the stream — re-activate it at landing (§3.5).
    parent_needs_refresh: bool,
}
```

One `Option<InflightTurn>` on the orchestrator (one turn at a time — the
`GenState` invariant). Created by `start_generation`, updated by the
progress arm, **dropped by `handle_done`** (after the landing below). The
table is the in-flight mirror of what `GenResult` will deliver; nothing is
ever recovered from it, so a mismatch is a bug to assert in tests, never a
path to handle.

### 3.3 The list and the resolver

- `ChildSummary` gains `running: bool` (additive, `skip_serializing_if`).
  `Chat::summary()` never sets it; `emit_chat_list` appends the in-flight
  child's card to its parent's `children` with `running: true`. The list
  widget draws the mark the way it draws an outcome (`ui.chatlist.run.running`
  — a muted *running* beside the count) and refuses nothing new: `Del`/
  `Ctrl+D` already refuse on any transcript row.
- `view(id)` gains a third arm: after the chats, the in-flight child —
  `ChatView::Child { parent, run }` with `run` borrowed from the table. Every
  consumer of `view()` then works on a running transcript unchanged: opening,
  `names_of`, copy/export/speech of the snapshot so far, `first_match_in_chat`
  (no index rows yet — answers `None`, a plain open), the `chat://` address
  book (the card's address becomes a link as soon as the list carries the
  child — earlier than stage 1's "at landing").
- `with_child_mut` gains the same arm, so **rename** (`F2`, `/rename`) of a
  running transcript edits the table; at landing `handle_done` copies
  `title`/`renamed_manually` from the table onto the landed run with the same
  id **before** titling (fork F5), so a hand-given title survives and the
  automatic one is skipped for it, exactly as for a landed run.

### 3.4 Opening and growing

Opening the running child is the ordinary `SwitchChat` → `activate_focused`:
`ChatActivated { child: Some(ChildView), messages: run.messages so far }`, the
read-only screen of PR 4, the `≡ transcript` chip. Two additions:

- **Growth.** On `ChildRoundFiled`, the table's run gets the messages, and if
  `active_id == child.id` the orchestrator sends
  `AppEvent::TranscriptGrew { id, messages }`; the screen appends them to its
  feed (`FeedMessage::from_messages` on the new slice, stitched onto the last
  bubble the way `from_messages` stitches rounds), guarded by
  `active_chat == id`, keeping scroll unless the user is at the bottom (the
  rule the streaming feed already follows). A round is the unit: the child's
  text does not stream token by token into its feed (fork F3).
- **The state chip.** The child view is not "generating" in the screen's
  sense (no `current_gen`), so it shows no spinner; the status-bar sub-agent
  chip is cleared by the screen's `activate_chat`... — it must **not** be:
  `set_subagent_progress` stays keyed on the *parent's* generation id, and
  the screen keeps the chip across the switch when the new chat is the
  running child or its parent (`ChatActivated` carries `live_turn: Option<Uuid>`
  — the running generation id, when the activated conversation is part of
  it — so the screen can keep `current_gen` and the chip rather than reset
  them). Inside the child view `Esc` keeps its meaning (back/close); cancelling
  the turn is the parent's `Esc`, and the refusal note on a send says so.

### 3.5 Switching without cancelling

`switch_to(id)` gains one rule before `request_cancel`: **if the running
turn's chat and the target are the parent and its in-flight child, in either
direction, the turn is not cancelled**. Concretely,
`inflight.as_ref().is_some_and(|t| t.chat == id || t.child.id == Some(id))`
and the current `active_id` is the other one.

Coming **back to the parent** mid-turn: `activate_focused(parent)` builds the
feed from `chat.messages` **plus `inflight.rounds`** (the rounds filed so far,
so the parent's earlier tool cards of this turn are there), and `ChatActivated`
carries `live_turn: Some(gen)` so the screen restores `generating` and
`current_gen` and accepts the rest of the stream. What the feed cannot have is
the text the parent streamed in its *current* round before the call (it lives
only in the task's `RoundOutput`): that round files when the call returns, and
the table marks `parent_needs_refresh = true`, so **at landing `handle_done`
re-activates the parent** (a full `ChatActivated` with the landed messages)
when it is the active chat — one refresh that closes every gap at once,
instead of a second stream of partial text (fork F4).

Every other switch — to a third chat, `Ctrl+N`, a clone — cancels as today.

### 3.6 Landing

`handle_done`, in order: copy a hand-given title from the table onto the
landed run (§3.3); push messages and apply effects as today; if
`parent_needs_refresh` and the parent is active, `activate_focused(parent)`;
if the active chat is the in-flight child, nothing — `view(id)` now resolves
it from the chat, and the next list snapshot shows it without the *running*
mark; drop the table; titling as in PR 6. A cancelled turn lands the partial
run (`Cancelled`) through the same path.

### 3.7 The running card (follow-up PR)

The parent's bubble gets the tool card only when `AppEvent::ToolCall` arrives
— after the call returns. A `ToolCallStarted { generation_id, call_id, name,
arguments }` event before a loop-executed call would let the feed draw the
card in a *running* state (title and address from the table, a muted
*running…* where the result goes), replaced by the real card on `ToolCall`
matched by `call_id`. Useful beyond sub-agents (a long `python_exec`), so it
is its own small PR after this one (fork F7).

## 4. Difficult spots

1. **Two copies of the child's messages** — the table's and the task's. They
   are built from the same `file_round` pushes, so they agree by construction;
   a test pins it (the landed run equals the table's last state plus the
   outcome).
2. **The chip and the child view.** The chip is keyed on the parent's
   generation; the child view has no generation of its own. `live_turn` on
   `ChatActivated` is the one new field that lets the screen keep both the
   chip and `current_gen` across the two allowed switches.
3. **`Esc` semantics in the child view.** `Esc` there is navigation (back to
   the list / the parent), never a cancel — the parent's `Esc` cancels. The
   status-bar `Esc` hint already words the target.
4. **The reconciliation pass and search** see nothing of a running child
   (nothing is on disk) — correct: it is indexed with the parent's next save
   after landing.
5. **`handle_done` ordering** — the title copy must precede `maybe_auto_title_run`,
   and the table must be dropped *after* the refresh reads `parent_needs_refresh`.

## 5. Forks

Recommendations marked; please confirm or veto before implementation.

- **F1. Where the progress travels.** (a) **On `done_tx`, as a second kind of
  message** — FIFO with the result for free *(recommended)*. (b) A separate
  channel with a sequence number. (c) Through `evt_tx` and back via a command
  (the UI would own orchestrator state — no).
- **F2. Where the in-flight child lives.** (a) **A side table on the
  orchestrator, reached through `view()`** *(recommended)*: every existing
  consumer works unchanged. (b) A provisional record pushed into
  `Chat.messages` and replaced at landing (a chat the save debounce could
  write half-landed — no).
- **F3. Granularity of growth.** (a) **Per round** — `ChildRoundFiled`, the
  screen appends messages *(recommended)*: one event kind, no second stream,
  no partial-text bookkeeping. (b) Token-level: the child's `Chunk`/`Thoughts`
  routed to the screen under a child generation id when the child is open —
  a second streaming bubble, `GenerationStarted` re-emission on every switch,
  and the partial text of the round in progress lost on each switch anyway.
- **F4. The parent's feed after a switch back.** (a) **Rebuild from
  `chat.messages + inflight.rounds`, and one full re-activation at landing**
  *(recommended)*. (b) Also carry the current round's partial text as
  progress (`RoundText`) — more events for a gap that lasts until the call
  returns and is closed at landing anyway.
- **F5. Renaming a running transcript.** (a) **Allowed; the table's title and
  `renamed_manually` are copied onto the landed run** *(recommended)*. (b)
  Refused with a note until it lands.
- **F6. Which switches do not cancel.** (a) **Parent ↔ its in-flight child
  only** *(recommended)*. (b) Any switch keeps the turn running in the
  background — a bigger change (a finished turn landing on a chat that is not
  open, notes in the wrong feed), and spec §4.4's one-turn model says no.
- **F7. The running card.** (a) **A follow-up PR** (`feat/tool-call-started`)
  *(recommended)* — it is a general tool-card feature. (b) In this PR.
- **F8. Staging.** (a) **One PR for §3.1–§3.6** *(recommended)*; the running
  card after it. (b) Split the table/list (7a) from the switch rule (7b).

## 6. Test plan

Orchestrator (`tests/subagent.rs`, the `ScriptRecorder` engine with a `hang`
script to hold the child mid-run): the list carries the running child with
`running: true` while the child hangs and without it after landing; opening
the running child activates it with the rounds so far and `TranscriptGrew`
follows the next filed round; `SwitchChat` parent → child → parent does not
cancel (the turn lands `Completed`, the parent's feed is re-activated whole);
a switch to a third chat still cancels; a rename during the run lands on the
record and the automatic title is skipped; the table is gone after landing;
`first_match_in_chat` on a running child answers `None`. Screen: `TranscriptGrew`
appends only for the active chat and stitches onto the last bubble; the
chip survives the two allowed switches and no other. Entities: `ChildSummary.running`
is additive. Live: `subagent_with_tools_e2e_live` re-run (the progress path
is on every delegation now) — GO/no-go of the PR.

## 7. Documentation touch list

spec §9.3.2 (the run while it runs), §11.2 (the *running* row), §11.3 (the
child view growing, `Esc`), §4.4 (the one exception to "a switch cancels");
architecture §5 (the channel, the table), §10 (the resolver's third arm, the
screen's `live_turn`); journal `ui-screens.md`; CHANGELOG Added; this plan →
`docs/history/` when the running-card PR closes the track, with
`docs/roadmap.md` updated.
