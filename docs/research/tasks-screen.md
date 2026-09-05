# A tasks screen — everything the app is doing, on one surface

**Status:** design **accepted**; every fork at its recommendation (the
user's decisions, 2026-09-06 — F1–F7 answered directly, F8 and F9 adopted
uncontested). The second of the two items
[background-subagents.md](background-subagents.md) §8 left for later (the
first, background dialogues, shipped 2026-09-06). Its own note there is the
starting position — *"the list's rows under each parent are the v1 surface"* —
and this document is the case for a v2.

Related: spec [§9.3.2](../../spec.md) (background runs), [§9.13](../../spec.md)
(background dialogues), [§11.2](../../spec.md) (the chat list),
[§11.1](../../spec.md) (the status bar), [ADR 0010](../decisions/0010-subagent-nested-turn.md),
[ADR 0011](../decisions/0011-dialogue-directed-run.md).

---

## 1. Why, precisely

Three surfaces report background work today, and each answers a different
question badly when there is more than one run out:

- **The chat list** nests a run under its parent chat, marked *running*
  (spec §11.2). It answers "what is this chat doing", and only for a chat you
  have scrolled to — two runs under two chats never appear together, and the
  row says nothing about *where* a run has got to.
- **The status bar** says *"in background: 2"* — a count with no names
  (`AppEvent::BackgroundRuns { out }`, a `u32`).
- **The transcript** shows one run in full, if you open it.

So the question a user actually asks — *what is the machine doing for me right
now, and where did the last thing land* — has no surface. That is the gap.
Everything else on the screen (the silent tasks, the finished runs) is there
because it answers the same question and costs almost nothing once the surface
exists.

**Requirements.**

- **R1.** One screen lists every background run: the ones out now, with
  progress, and the ones that have landed, with their outcome.
- **R2.** From a row, the two things a person wants: open it (the transcript)
  and stop it (a running one) — through the routes that already exist, never
  a second implementation of either.
- **R3.** Live: a run that files a round, or ends, changes the screen without
  a keypress; a run that has been going four minutes says so.
- **R4.** No new source of truth. A row is a projection of the orchestrator's
  seats and of the records already on the chats — nothing is stored for the
  screen's sake.
- **R5.** The rule of spec §11.2 holds here as everywhere: the footer names
  the keys that work **on the selected row**, and nothing else.
- **R6.** Cheap. No index, no walk the app does not already do, no timer that
  runs when the screen is closed.

## 2. What exists (inventory)

Anchors at `e38102a`.

### 2.1 Adding a screen is mostly bookkeeping

Two shapes exist. The chat list is built **synchronously** from the snapshot
the chat screen already holds (`runtime/dispatch.rs:704`); the changes screen
(`F4`) is a **round trip** — the key sends `AppCommand::OpenChanges`
(`events.rs:204`) and the screen is constructed when `AppEvent::WorkspaceChanges`
arrives (`dispatch.rs:185`, `:461`). A tasks screen wanting live data is the
second shape.

The screen itself is a pure projection with no `app` dependency
(`screens/changes.rs:79` — palette, locale, selection; `handle_key` `:145`,
`hints()` `:242`, `render` `:267`), drawn inside the shared
`shared::ui::screen_chrome` (`shared/ui.rs:410`).

What a new screen must touch, all of it compiler- or gate-enforced:
`screens/mod.rs`; the new `screens/tasks.rs`; `runtime/mod.rs` six sites
(`ActiveScreen` `:51`, `is_chat` `:73`, `set_theme` `:83`, `handle_paste`
`:109`, `help_context` `:243`, the draw match `:688`); `runtime/input.rs:255`;
`runtime/dispatch.rs` (`AnyIntent` `:535`, `dispatch_any` `:546`, a
`dispatch_tasks` beside `dispatch_changes` `:483`, an `apply_event` arm);
`events.rs` (the command, the event, and the exhaustive
`works_on_the_open_chat` match `:297`); the orchestrator's command arm and
emitter; `help_dialog.rs`'s `HelpContext` `:86` and the fixed-size
`HELP_SECTIONS` array (`runtime/mod.rs:229`, `[…; 7]` → 8); the gate tests
(`runtime/tests.rs:1506` enumerates every context by hand); both locale
bundles; README, spec §11.7, CHANGELOG, the journal.

### 2.2 The seats already know almost everything a row needs

`BackgroundRun` (`background_runs.rs:43`) holds the run's own generation id,
the run id, the **parent chat**, its cancellation token and the mirror
(`InflightChild`, `mod.rs:546`) — which is the whole `SubagentRun`: kind
(sub-agent or dialogue), title, persona name, `created_at`, the rounds filed
so far, tokens, outcome, and the round in progress with its open tool calls
(`partial.tools`).

What the orchestrator does **not** have is the run's *position*: the round
number and the tool it is inside. That report
(`AppEvent::SubagentProgress`, `events.rs:461`) is sent straight from the
generation task to the UI (`generation.rs:1804`), and the chat screen drops
any whose generation is neither the turn's nor the open transcript's
(`screens/chat/feed.rs:490`) — so a background run's position is visible today
**only** while its own transcript is open.

### 2.3 Finished runs are already free

A landed run lives on the call's record (`Chat.messages[].tool_calls[].subagent`),
and `Chat::children()` (`entities/chat.rs:344`) flattens it. `self.chats` holds
**every visible chat, of every profile, entirely in memory**
(`orchestrator/mod.rs:745`, `:650`), and `emit_chat_list` already walks all of
them through `Chat::summary()` on every filed round, rename and background
step (`mod.rs:1766`). So enumerating every run of every chat costs exactly one
chat-list emit — there is no index and none is needed (there is no runs table
in SQLite: `storage/db/mod.rs:264`).

That reverses the assumption this track started with. Listing the finished
runs is not the expensive half; it is free.

### 2.4 The silent tasks are four of seven

`BackgroundKind` (`events.rs:824`) covers reflection, note consolidation,
self-model consolidation and compaction, each with a slot
(`orchestrator/background.rs:20`, a cancel token and a failure streak) and a
localized label (`:107`). The other three background things do not use it:
auto-titling is fire-and-forget over `title_tx` with no flag at all, TTS has
its own `TtsActive`, and RAG indexing and `/reindex` share one slot and report
through `RagProgress`.

**Last outcome and last run time are stored nowhere**: `handle_bg_done`
(`background.rs:51`) consumes the result and keeps only the streak.

### 2.5 Free keys

`F7`, `F8`, `F9`, `F11`, `F12` are unbound; `Ctrl+S` is the only letter chord
free on every screen. `F6` is now the background-run stop key on a transcript,
which is the key this screen should reuse for the same act (R2).

## 3. No probe

Nothing here is model behaviour: the screen shows what the app is already
doing, and no request changes. AGENTS.md §1's go/no-go probe does not apply,
and a live run is not required for the screen itself — but the orchestrator
gains a progress step (§4.3), so the background e2e smokes are re-run once
before the PR.

## 4. Design

### 4.1 The screen

A full-screen surface, `F7` and `/tasks`, in two sections:

```
◆ Tasks
  RUNS
  ▌ ⟳ Critic              Space talk    round 3 · fs_read    4:12   1.2k
    ⟳ Mara ↔ Jonas        Plans         line 5 · director    1:07   0.7k
    ✓ Reviewer            Plans         completed           12:31   3.4k
    ✗ Fact-checker        Recipes       cancelled           09:58   0.2k
  THE APP'S OWN WORK
    ⟳ reflection                        running
      consolidation                     idle
```

A **run row**: the kind's glyph, the run's title, its parent chat, its
position (running) or outcome (landed), the elapsed time or the finish time,
and its tokens. A **task row**: the label and running/idle, nothing more —
that is all the current state can honestly say (§2.4).

Selection moves with `↑↓`/`PageUp`/`PageDown`/`Home`/`End`; `Enter` opens the
selected run's transcript through the existing read-only route; `F6` stops a
running one, through `ChatIntent::StopSubagentRun` — the same intent the
transcript's key and `/subagents stop` produce; `P` opens the parent chat;
`Esc` goes back; `F1` is the runtime's. Each of those is offered only where it
does something (R5): no `Enter` on a task row, no `F6` on a landed run.

### 4.2 What a row is (F1)

Every background run of every chat the list shows — running first (newest
first), then landed (newest first), capped at the most recent **50** landed
with a counted "…and n more" line. Cross-profile, like the chat list itself
(spec §11.2), because the finished ones are and the list's precedent is the
one to follow.

### 4.3 Where the data comes from (F3, F4)

A purpose-built snapshot: `AppEvent::TaskList(Vec<TaskRow>)`, emitted by the
orchestrator whenever a seat is added, steps or ends, and on request
(`AppCommand::RequestTasks`, the `F4` round trip). One producer, one consumer,
no assembly in the screen.

For the position (round, tool) the orchestrator has to learn what today only
the UI sees: the run's loop already sends `TurnProgress` steps to the
orchestrator over its own channel, so the report becomes one more step
(`ChildProgress { run, round, tool }`) stored on the seat. The alternative —
having the runtime keep a map fed by the existing `SubagentProgress` — leaves
the data in the layer that draws it, dies when the screen closes and needs the
chat screen's generation guard re-thought.

### 4.4 Live (F5)

Elapsed is rendered from `created_at`, so nothing is stored. The repaint tick
that today runs only on the chat screen (`runtime/mod.rs:598`,
`spinner_frame_needed` gated on `active.is_chat()`) extends to the tasks
screen **while a run is out** — one second, and nothing when the list is all
landed.

### 4.5 What it is not

Not a second control surface: every action is an existing route. Not a log —
a landed run's row is a pointer to its transcript, which is where the words
are. Not storage: closing the app forgets the ordering, not the runs.

## 5. Difficult spots

- **The exhaustive matches are the work.** Six sites in `runtime/mod.rs`
  alone, plus two "screen-replacing" matches written exhaustively on purpose
  so a late event cannot steal a screen the user is reading
  (`dispatch.rs:395`, `:461`). A tasks screen that opens a transcript also
  wants a third `Back` variant (`runtime/mod.rs:144`) so `Esc` returns to the
  task list rather than to the chat list, and a case in the single clearing
  funnel (`dispatch.rs:341`).
- **A landed run whose exchange was taken back** lives in `Chat.deleted`
  (`entities/chat.rs:369`) and is not in `children()`. It should not be on the
  screen either — it is gone from the conversation — and the walk that builds
  the rows must use `children()`, not the including-deleted reader.
- **The count on the bar and the rows must agree.** Both derive from the
  seats; the bar counts `outcome.is_none()` (`background_runs.rs:419`). The
  screen's "running" section is the same predicate, and a test should pin
  that, or the two surfaces drift the first time one of them is edited.
- **Cost of the emit.** `TaskList` on every step of every run is more traffic
  than `BackgroundRuns { out }`; the rows are cheap but not free (a walk of
  all chats for the landed half). The emit therefore carries the **live** half
  on a step and rebuilds the landed half only when a run lands or the screen
  asks — the same split `emit_chat_list` already lives with.

## 6. Forks

**Decided by the user, 2026-09-06**: the recommendation in every one —
F1(a), F2(a), F3(a), F4(a), F5(a), F6(a), F7(a), F8(a), F9(a).

- **F1** *(decided: (a))*. **What the screen lists.** (a) **Live runs and landed runs, capped at
  50 landed** *(recommended — §2.3: the landed half is free, and "where did
  it land" is half the question)*. (b) Live runs only — the smallest thing
  that closes the gap in §1. (c) Live plus only this session's landed runs —
  needs retention the app does not have today.
- **F2** *(decided: (a))*. **The silent tasks.** (a) **A second section, running/idle only**
  *(recommended — the labels exist, no new state, and "why is the machine
  busy" is the same question)*. (b) Also last-run time and last outcome — two
  new fields on `BgSlot` and a wider snapshot. (c) Runs only; the silent tasks
  stay on the status bar.
- **F3** *(decided: (a))*. **Where the position comes from.** (a) **A `TurnProgress::ChildProgress`
  step the orchestrator stores on the seat** *(recommended — one owner, and
  the data survives the screen being closed)*. (b) A runtime map fed by the
  existing `SubagentProgress` events.
- **F4** *(decided: (a))*. **The transport.** (a) **A purpose-built `TaskList` snapshot plus a
  request command** *(recommended, the `F4` shape)*. (b) Reuse `ChatList` +
  `BackgroundRuns` and assemble in the screen.
- **F5** *(decided: (a))*. **Elapsed.** (a) **Rendered from `created_at`, with the repaint tick
  extended to this screen while a run is out** *(recommended)*. (b) Show the
  start time and no elapsed, so no tick is needed.
- **F6** *(decided: (a))*. **The entry point.** (a) **`F7` and `/tasks`** *(recommended — `F7` is
  free everywhere and unclaimed by both measured hosts, and the typed route
  is the rule for anything a terminal might eat)*. (b) `/tasks` only.
  (c) `Ctrl+S` and `/tasks`.
- **F7** *(decided: (a))*. **The row's actions.** (a) **`Enter` opens the transcript, `F6` stops,
  `P` opens the parent chat** *(recommended — `F6` is already the stop key on
  a transcript)*. (b) `Enter` and `F6` only; the parent is one more `Esc`
  away. (c) Add `Del` to hide a landed run from the screen — rejected: the
  screen is a projection, not a store.
- **F8** *(decided: (a), uncontested)*. **Order.** (a) **Running first, then landed, both newest first**
  *(recommended)*. (b) Grouped by chat, like the list's nesting.
- **F9** *(decided: (a), uncontested)*. **Staging.** (a) **One PR** *(recommended — the screen is inert without
  its rows)*. (b) Two: the runs section, then the silent tasks.

## 7. Test plan

Unit, over the existing fixtures:

- the snapshot: a run out appears as a running row with its parent chat and
  position; it ends and becomes a landed row with its outcome; a run whose
  exchange was taken back is on neither (it is in the deleted archive);
- the two surfaces agree: the bar's count equals the screen's running rows,
  asserted through one shared predicate rather than two literals;
- the screen: `Enter` yields the transcript's open intent for a run row and
  nothing for a task row; `F6` yields `StopSubagentRun` for a running row and
  nothing for a landed one; `P` yields the parent chat; the footer offers
  exactly the keys the selected row answers (the §11.2 rule, asserted on the
  rendered frame);
- the runtime: `F7` and `/tasks` reach the same intent; `Esc` from a
  transcript opened here returns to the task list, not to the chat list;
- the help: the new section is registered and every footer hint of the screen
  has a row in it (the per-screen gate the other five screens carry).

Live: not required (§3); the background e2e smokes are re-run once because the
orchestrator gains a progress step.

## 8. Not in this track

- **Cancelling a silent task** from the screen (reflection, consolidation):
  they have cancel tokens, but a user-facing stop for them is its own
  question — they are supposed to be invisible.
- **History beyond the cap**: a searchable log of every run ever. The chat
  list and `chat_search` already reach every transcript.
- **The silent tasks under the app-wide budget**
  ([admission-by-budget.md](admission-by-budget.md) §8), which stays open.
- **Notifications across profiles** (background-subagents.md §8).
