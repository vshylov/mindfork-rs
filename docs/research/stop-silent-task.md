# Stopping a silent task from the tasks screen

> **Status:** implemented (2026-09-07) — every fork at its recommendation
> (the user's decision, 2026-09-07); the live run in §6.1. The item the tasks-screen
> design left out ([tasks-screen.md](tasks-screen.md) §8: "they have cancel
> tokens, but a user-facing stop for them is its own question") and the
> silent-preemption track pointed back at ([silent-preemption.md](silent-preemption.md)
> §8). Small enough for no stage-0 probe: nothing about a model's behaviour
> is in question — the token exists, the screen exists, and what has to be
> designed is what a cancelled task *is* to the orchestrator.

## 1. Why, precisely

The tasks screen (`F7`, spec §11.10) lists the app's own four silent tasks —
reflection, notes consolidation, self-model consolidation, history
compaction — as *running*, *waiting* or *idle*, and stops a running
background run with `F6`. On a task row `F6` does nothing and the footer
does not offer it. The tasks are meant to be invisible, and mostly are; but
the two tracks since gave the user reasons to want a hand on them:

- **They now take turns with the turn on the pool.** A task that holds its
  stream (past its three yields, or the roll on a slow engine) makes the
  next message wait a round; a user watching the tasks screen see a row
  *running* for a minute on a CPU-only server has nothing to do but wait.
- **They are visible now**, and a thing that is visibly running and cannot
  be stopped is the kind of surface docs/lessons.md §4 warns about: the
  screen says *running* and offers no verb.
- **The token is already there.** Every task's slot holds a
  `CancellationToken` (`BgSlot.cancel`, cancelled by `Quit` through
  `cancel_all_bg`), every stream ends `Finished(Cancelled)` when it fires,
  and since the preemption track the holders already tell a cancellation
  of their own token from a displacement. A stop is the same token,
  cancelled for one kind, by a key.

What is *not* there is the outcome. A task whose own token fires today lands
as whatever its path made of `Finished(Cancelled)`: the loops break out of
`run_rounds` and report **`Ok`** — the streak resets, reflection announces
`SelfModelChanged` for a window it did not finish — and the roll reports the
**timeout's** wording. Only `Quit` cancels today, and nobody reads an outcome
during a quit; a user's stop is read at once, on the screen and in the chat.

**Requirements.**

- **R1.** `F6` on a task row that is running or waiting stops that task; on
  an idle row it does nothing and is not advertised (spec §11.2's rule).
- **R2.** A stopped task lands as **cancelled**: the slot clears, the
  indicator goes out, the row reads *idle* — and neither the failure streak
  nor the success path counts it (no alert, no reset).
- **R3.** Nothing the task already wrote is undone, and nothing it owed is
  refunded: the watermark it advanced and the counter it reset at spawn stay
  where spawn put them — a stop skips this window, as a failure does; the
  next cadence runs over the next.
- **R4.** The one task the user can *ask* for — `/compact` — answers its
  stop in the chat, as it answers its failure; an automatic roll stops
  quietly and is planned again at the next landing if the conversation is
  still over the threshold.
- **R5.** A stop while the task is *waiting* (for the lane, for room, or
  for its retry after a displacement) ends the wait and opens no stream.
- **R6.** No confirmation, no command, no new state on the screen: one key
  on one row, and the projection tells the rest.

## 2. What exists (inventory)

### 2.1 The screen and its route

`screens/tasks.rs`: the rows are `Row::Run(i)` then `Row::App(i)`;
`selected_stoppable()` answers a `Uuid` for a running **background** run
only (a child of the turn is ended by cancelling the turn; a landed run has
nothing to stop), `handle_key` maps `F6` to `TasksIntent::StopRun(id)`,
and `hints()` offers `F6` only when `selected_stoppable()` is `Some` — the
footer is built from the same predicate the key dispatches on (the
tasks-screen research's F7, spec §11.2). `runtime/dispatch.rs::dispatch_tasks`
sends `AppCommand::StopSubagentRun { id }`; the orchestrator's
`handle_stop_subagent_run` cancels the seat's token and the run lands on
its own path, as `Cancelled`, with its notification.

### 2.2 The slot registry and the outcome channel

`orchestrator/background.rs`: `BgSlot { cancel: Option<CancellationToken>,
failures: u32 }` per `BackgroundKind`; `begin_bg` takes the slot and emits
`BackgroundTask { active: true }` and the task list; `handle_bg_done(kind,
Result<(), String>)` clears it, counts an `Err` into the streak (one alert
at three), resets the streak on `Ok`, emits `BackgroundTask { active: false }`,
the task list, and `SelfModelChanged` on an `Ok` of reflection or
self-model consolidation. `cancel_all_bg` (the `Quit` branch) cancels every
active slot. The channel is `bg_done_tx: (BackgroundKind, Result<(), String>)`;
the roll reaches the same handler through `handle_compact_result`, which
turns a manual roll's failure into an error in the feed and an `Ok` for the
streak, and an automatic one's into an `Err`.

### 2.3 What each holder does when its own token fires

| holder | its token | on `Finished(Cancelled)` from its own token today |
|---|---|---|
| the three loops (`run_rounds`) | the slot's, through `ToolContext.cancel` | a wait cancelled → `Streamed::Cancelled` → `RoundsEnd::Done` → **`Ok(())`**; a stream cancelled → the round's reason is `Cancelled`, not a displacement → the loop breaks → **`Ok(())`** |
| the roll (`spawn_compact`) | the slot's (`begin_bg(Compaction, cancel)`) | a wait cancelled → `Err(ui.err.compact_timeout)`; a stream cancelled and not displaced → `Err(ui.err.compact_timeout)` — the preemption track's "a fragment is not a summary", worded as the timeout because only `Quit` could reach it |
| the title | its own private token | not on the screen; not this track's |
| impersonation | `imp_cancel` (`Esc`) | its own path; not this track's |

So the mechanism is there and the *reading* is wrong for a stop the user
will see: the loops would call a stopped task a success, the roll would call
it a timeout.

### 2.4 The bookkeeping (R3)

Reflection advances `chat.reflected_upto` at spawn; the consolidations reset
their counters at spawn; the roll's plan is cut at spawn and re-cut at the
next landing if the automatic trigger fires again. The lane design (F7) and
the preemption design (§2.3) both kept every one of these at spawn and
argued why: a task that fails or is displaced-and-retried leaves them honest.
A stop is the user's decision to skip; nothing needs refunding, and the
alternative — re-reading the window at the next cadence — would make the
stopped task come back sooner, which is the opposite of what a stop asks.
*(Revisited 2026-09-08 — [stop-refunds-window.md](stop-refunds-window.md):
a stop postpones; the window is given back when no round of the task's
tools had run, and kept once one has.)*

### 2.5 What the tests can already say

`screens/tasks.rs` has key tests over `TaskList::fixture` (`F6` on a landed
run is `None`, on a running background run `StopRun`); `runtime` tests
cover `dispatch_tasks`; `tests/silent.rs` spawns a loop by hand
(`spawn_loop`) with its own token and reads `done_rx`; `tests/background.rs`
pins `handle_stop_subagent_run`. Nothing today asserts what a cancelled
silent task lands as, because nothing but `Quit` cancels one.

## 3. Design

### 3.1 The key and the intent

`selected_stoppable()` answers a `Stoppable` — `Run(Uuid)` for a running
background run, `Task(BackgroundKind)` for an app task that is running or
waiting — and `F6` maps it to `TasksIntent::StopRun(id)` or the new
`TasksIntent::StopTask(kind)`. The footer offers `F6` for either, worded
**stop** (`ui.tasks.hk.stop`, today "stop run": one word, since the row under
the cursor says what is stopped); the `F1` row reads "stop the selected run
or task". An idle task row keeps `F6` silent and unadvertised (R1).

### 3.2 The command and the cancel

`dispatch_tasks` sends `AppCommand::StopBackgroundTask { kind }`; the
orchestrator's `handle_stop_background_task(kind)` cancels the slot's token
if the slot is active (`cancel_bg(kind)`, the one-kind sibling of
`cancel_all_bg`) and does nothing otherwise — the screen may be a snapshot
behind. The task ends on its own path, at once: a wait returns `None`, a
stream yields `Finished(Cancelled)` on the next chunk (the engine honours
the closed connection within a batch — silent-preemption §3.2), and the
outcome reaches `handle_bg_done` as it always has. No confirmation: `F6`
on a run has none, and a silent task is cheaper to lose.

### 3.3 The outcome: a third kind

The channel carries `BgOutcome { Done, Cancelled, Failed(String) }` instead
of `Result<(), String>`. `handle_bg_done` on `Cancelled`: the slot clears,
the indicator goes out, the task list is re-sent, the streak is **untouched**
(neither reset nor counted), and `SelfModelChanged` is still sent for
reflection and self-model consolidation — a stopped run may have written
through its tools before the stop, and the event only makes an open `F3`
re-ask. No alert, no notice (R2, R6).

The holders report it: `run_rounds` returns `RoundsEnd::Cancelled` when its
wait was cancelled (`Streamed::Cancelled`) or when a round's stream ended
`Cancelled` and the reservation does not read displaced — the parent token
fired; `spawn_silent_loop` maps it to `BgOutcome::Cancelled`. The roll's
`CompactResult.text` becomes `Result<String, CompactEnd>` with
`CompactEnd::{Failed(String), Cancelled}`: the cancelled wait and the
cancelled-not-displaced stream report `Cancelled`; `handle_compact_result`
turns a **manual** roll's `Cancelled` into a notice in the feed
(`ui.compact.cancelled`: "Compression stopped; nothing was folded.") and the
outcome `Cancelled`, an **automatic** roll's into the outcome alone (R4).
`Quit`'s `cancel_all_bg` reaches the same paths and nobody reads the
outcome, as today.

### 3.4 What the user sees

The row under the cursor goes from *running* or *waiting* to *idle* on the
next snapshot — `handle_bg_done` re-sends the list, within the stream's next
chunk. The status bar's per-kind indicator goes out with it. A manual
`/compact` gets its one notice. Nothing else: no "stopped" state, no
timestamp — the screen was built to be a projection of what is, and a
stopped task *is* idle.

### 3.5 What does not change

The title (private token, not on the screen), impersonation (`Esc`), the
runs' stop, the cadences and their spawn-time bookkeeping, the streak's
meaning (consecutive *failures*), the lane and the preemption rules — a
stopped task's reservation drops like any other, and a displacing waiter
it was yielding to is admitted as before.

## 4. Difficult spots

- **A stop that races the landing.** The user presses `F6` as the task's
  last chunk arrives: the token is cancelled after the stream ended; the
  loop lands `Done`, the roll lands its summary. Correct — the work was
  done — and the slot is idle either way. The reverse race (the outcome
  lands `Cancelled` a chunk after a `Done` would have) is what "cancelled"
  means.
- **A stop during a displacement's retry.** The task is waiting with its
  yields counted; its own token fires; `acquire_silent` returns `None` →
  `Streamed::Cancelled` → `Cancelled`. The `Displacing` guard of the waiter
  that displaced it is unaffected: it holds its own count and is admitted
  on the next notify, as when the task ends any other way.
- **A stop between a round's tools.** The loop's tools run outside the
  reservation, with `ToolContext.cancel` — the same token — in hand: a
  tool that checks it stops early, one that does not finishes its write,
  and the loop then makes the next round's reservation with a cancelled
  token → `None` → `Cancelled`. Nothing written is undone (R3).
- **A stop of the roll after `/compact`, then `/compact` again.** The slot
  is free once the outcome lands; a second command before that is refused
  with the existing `ui.compact.busy`. The stop's notice and the refusal
  can both show if the user is fast; both are true.
- **The manual roll's failure streak.** Today a manual roll's failure is an
  `Ok` for the streak (S6 of the compaction journal: the user was told at
  once). A manual roll's stop is `Cancelled` for the streak — untouched —
  which is the same effect by a truer name.
- **`Result<(), String>` is read in three places.** The channel's sender in
  `spawn_silent_loop`, the roll's `handle_compact_result`, and the handler;
  the tests that assert on `bg_failures` after an `Err` keep their meaning
  under `Failed`.

## 5. Forks

- **F1. The key.** (a) **`F6` on the task row** *(recommended — the screen's
  one stop key, the footer already the row's own)*. (b) A key of its own
  (`Delete`, `X`) — a second stop verb for the same screen.
- **F2. A command.** (a) **None in v1** *(recommended — the screen is the
  surface; a `/tasks stop <kind>` needs localized kind names and a parser
  for a thing four rows away)*. (b) `/tasks stop reflection|notes|self|compact`.
- **F3. The feedback.** (a) **The row's state and, for a manual `/compact`,
  one notice** *(recommended — R6; a stop is not a failure and the feed is
  not a log)*. (b) A notice per stop, naming the task.
- **F4. The bookkeeping on a stop.** (a) **Kept at spawn** — the window is
  skipped, the next cadence runs over the next *(recommended — §2.4)*. (b)
  Refunded: the watermark moved back, the counters restored — the task
  comes back at the next landing.
- **F5. An automatic roll after a stop.** (a) **Planned again at the next
  landing if still over the threshold** *(recommended — the protection
  stays; a stop is per attempt)*. (b) Suppressed until the conversation
  grows past the level it was stopped at — a watermark for a thing the user
  can always type `/compact` for.
- **F6. `SelfModelChanged` on a cancelled reflection.** (a) **Sent**
  *(recommended — a partial run may have written; the event costs a
  re-ask of an open screen)*. (b) Not sent: the screen may show a stale
  snapshot until its next open.
- **F7. Staging.** (a) **One PR** *(recommended — one intent, one command,
  one enum, four holders' readings)*. (b) Two: the outcome first, the key
  after.

## 6. Tests and the live run

- `screens/tasks.rs`: `F6` on a running task row → `StopTask(kind)`, on a
  waiting one the same, on an idle one `None`; the footer offers `F6` for
  those and not for idle; the help row.
- `runtime`: `StopTask` → `StopBackgroundTask { kind }`.
- Orchestrator (`tests/silent.rs` and `tests/background.rs`): a loop
  spawned by hand and cancelled mid-stream lands `Cancelled` — the recorder
  saw one request, `done_rx` says `Cancelled`; cancelled while waiting for
  room — no request, `Cancelled`; cancelled during a displacement's retry —
  `Cancelled`, the displacing waiter admitted; `handle_bg_done(Cancelled)`
  leaves `bg_failures` where it was, clears the slot, emits
  `BackgroundTask { active: false }`, the task list, and `SelfModelChanged`
  for reflection; `handle_stop_background_task` on an idle kind does
  nothing; a manual `/compact` stopped → the notice and no error, the chat's
  compaction unchanged; an automatic roll stopped → quiet, the streak
  untouched, the next landing plans again.
- Live (the paths are engine paths): `stop_silent_task_e2e_live` — on the
  LAN stack, `/compact` on a seeded chat, `StopBackgroundTask { Compaction }`
  a second later: the notice, the slot idle, one live stream that ended
  early; then `/compact` again completes. Plus `silent_roll_e2e_live` and
  `background_subagent_e2e_live` as the regression. The screen itself needs
  a real terminal; its logic is under unit test.

### 6.1 The live run (2026-09-07)

On the LAN stack (Qwen 3.6 27B, four slots over 16384)
`stop_silent_task_e2e_live`: `/compact` on a chat seeded with 55 % of the
pool, the stop command 1.0 s later — the notice arrived **0.00 s** after
the stop (the wait for the lane was still on: the roll had not opened its
stream, and a cancelled wait returns at once), nothing was folded, at most
one live stream open; the next `/compact` completed in 5.8 s. With
`silent_roll_e2e_live` and `background_subagent_e2e_live` as the
regression: 3/3 in 57 s. The unit suite: 2903 green, 144 `#[ignore]`.

## 7. Not in this track (recorded so they are not re-derived)

- **Stopping the title** — no row, a private token, and a title is seconds.
- **A `/tasks stop` command** (F2b) — **done** as its own track
  ([tasks-stop-command.md](tasks-stop-command.md), 2026-09-08): both halves
  of the reading above turned out light — the names were the screen's, the
  parser already carried a token behind a word.
- **Refunding the window on a stop** (F4b), if users turn out to stop tasks
  to *postpone* them rather than to skip them — **done** as its own track
  ([stop-refunds-window.md](stop-refunds-window.md), 2026-09-08): the
  window comes back when the task was stopped before a round of its tools
  ran, and stays advanced once one has — R3 above is superseded to that
  extent.
- **A smaller batch** for the CPU build — **done** as its own track
  ([cpu-batch.md](cpu-batch.md), 2026-09-07): `-b`, not `-ub`; a stop's
  slot frees in 6.5 s rather than 23 on a CPU-only host.

## 8. Documentation touch list (AGENTS.md §4)

- spec §11.10 (`F6` on a task row; *idle* after a stop), §6.7 (a stopped
  roll: the manual notice, the automatic re-plan), §17.6 (a stopped
  reflection skips its window), §11.7 (no new command — stated once).
- architecture §10 (the screen's `Stoppable`), §9/§11 (`BgOutcome`, the
  holders' readings), §5 (`RoundsEnd::Cancelled`).
- locales `en`/`ru`: `ui.tasks.hk.stop` reworded, `ui.help.tk_stop`,
  `ui.compact.cancelled`.
- journal ui-screens.md (the screen's home) with a pointer from engine.md's
  index line if the entry touches the loop; CHANGELOG `[Unreleased]` Added;
  roadmap; tasks-screen.md §8 and silent-preemption.md §8 point here;
  CLAUDE.md's status line when the track ships.
