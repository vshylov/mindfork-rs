# The quit waits for the landing — a stop's own path decides, the state rule only past a cap

> **Status:** implemented (2026-09-08) — every fork at its recommendation
> (the user's decision, 2026-09-08); the regression run in §6.1; no stage-0
> probe (nothing about a model's behaviour is in question).
> The item the effect track recorded
> ([acted-by-effect.md](acted-by-effect.md) §7): at a quit, a silent task
> whose round of tools is running (`InTools`) keeps its window because the
> round's write, if any, has not reported yet — the conservative side of
> the rule, taken for a case that is a few hundred milliseconds wide. This
> track has the quit wait that long and decide exactly.

## 1. Why, precisely

Three tracks built one rule: a stopped or quit silent task gives its
window back unless a call of it **wrote** to the profile's memory. At a
stop the decision is exact — the loop lands `Cancelled { wrote }` and
`handle_bg_done` gives the window back or not. At a quit there is no
landing to read, so `quit_bg` reads the loop's state instead: `Idle` →
refund, `Wrote` → keep, and `InTools` → keep, because the round's tools
are still running and the write they may make has not been reported. That
last case is exactly the round the effect track was about — a first round
of reads (`get_self_model`, a recall, a note's neighbours) stopped by the
user closing the app while the recall's embedding is in flight — and the
state rule keeps its window although, a few hundred milliseconds later,
the loop would have landed `wrote: false`.

The observation that makes the fix small: **a cancelled loop lands on its
own, and fast.** The quit cancels the token; a loop waiting for the lane
returns at once (`acquire_silent` selects on the token first); one in a
stream ends at its next chunk; one in its tools finishes them — a DB read
is instant, an embedding is what an embedding costs — and its next lane
wait returns at once. The landing then arrives on `bg_done_rx`, the very
channel `run`'s `select!` polls, carrying the exact fact. Nothing about
the quit has to *decide*; it has to **listen a little longer** before the
exit flush, and let the stop's own path decide.

**Requirements.**

- **R1. One decision path.** A task's window at a quit is decided by the
  same code that decides it at a stop — `handle_bg_done` on the landing —
  whenever the landing arrives in time.
- **R2. A quit stays a quit.** The wait is bounded; a task that has not
  landed by the cap is decided by the state rule as today (`Idle` refund,
  else keep), never held open.
- **R3. No request after the quit.** A cancelled loop never opens a stream
  it was about to open: it lands, it does not ask the engine once more.
- **R4. Nothing else about the exit moves**: the background runs land from
  their mirrors as before, the roll's outcome is left where it is, the
  flush is the last thing.

## 2. What exists (inventory)

- **`run`** (`orchestrator/mod.rs`): a `select!` over the command channel
  and the task channels — `bg_done_rx` among them (`(kind, BgOutcome)` →
  `handle_bg_done`), `compact_rx` for the roll; `handle_command(Quit)`
  returns `true`, the loop `break`s, `flush_saves()` runs once, `run`
  returns. `bg_done_rx` is a local of `run`, dropped with it.
- **`quit_bg`** (`background.rs`): for every slot, cancel the token, take
  the `Refund`, give the window back if `acted == Idle`. The slot keeps
  `cancel = Some` — nobody reads it after a quit.
- **`handle_bg_done`**: takes the slot's `Refund`, gives the window back on
  `Cancelled { consumed: false }` (`give_back`), clears the slot, emits the
  indicator, the task list and `SelfModelChanged` — `let _ =` sends, harmless
  on a closing UI.
- **The loop after a cancel**: `stream_round` → `lane_reservation` →
  `acquire_silent` with `biased; cancel.cancelled()` first — returns at
  once when the token is already cancelled; where the engine has **no**
  session budget (`ctx.sessions == None`) the reservation is `None` and the
  stream is opened directly, so a loop cancelled during its tools would
  send one request the cancelled token then ends.
- **Tests**: `extra_tools` on `OrchestratorDeps` (re-registered by
  `rebuild_registry`) lets a test register a tool of its own;
  `spawn_loop_allowing` runs a loop with a chosen tool set and returns its
  `done_rx` and `Acted`; the quit-track tests drive a quit through `run`
  (`spawn_english`, the chat read from disk).

## 3. Design

### 3.1 The quit in three steps

`handle_command(Quit)` keeps its shape but `quit_bg` becomes
**`cancel_bg_all`**: every slot's token cancelled, the refunds left on the
slots. `run`, after the loop breaks and before `flush_saves`:

```
settle_silent_tasks(&mut bg_done_rx, QUIT_SETTLE).await   // step 2
refund_unlanded()                                          // step 3
flush_saves()
```

**Step 2** drains `bg_done_rx` while any slot is still active and the cap
has time left: each landing goes through `handle_bg_done` — the stop's
path, `consumed` from the loop — so a round of reads that finished lands
`wrote: false` and is refunded, a round that wrote lands `wrote: true` and
keeps, a loop cancelled in its stream or its wait lands within
milliseconds. **Step 3** is today's `quit_bg` minus the cancel: for every
slot still holding a `Refund` — a task whose tools outlasted the cap — the
state rule, `Idle` refunded, `InTools` and `Wrote` kept (R2).

`QUIT_SETTLE` = **2 s**, the whole wait, not per task (fork F2): a task
not in its tools lands in milliseconds, a read round in tens, an
embedding in hundreds on a GPU host; two seconds covers a CPU embedding
and is still a quit.

### 3.2 No request after the quit (R3)

`stream_round` checks the token before opening a stream, so a loop
cancelled during its tools lands without a request even where the engine
has no session budget. One line; where a budget exists the lane wait
already does the same.

### 3.3 What the user sees

Nothing at the quit but, at most, a pause under two seconds when a task
was mid-tools; on the next launch a reflection that had only been reading
when the app closed reflects on the same replies. The tasks screen, the
commands, the roll and the background runs are untouched.

## 4. Difficult spots

- **Landings that are not the quit's.** `bg_done_rx` may hold a landing
  that arrived just before the quit (a task that finished on its own);
  draining it through `handle_bg_done` is exactly what the loop would have
  done a tick later — `Done` resets the streak, a refund is dropped — and
  the flush after it writes the same state the loop would have.
- **The roll lands elsewhere.** `compact_rx` is not drained: a cancelled
  roll has no window and its notice has no reader.
- **The events at exit.** `handle_bg_done` sends the indicator, the task
  list and `SelfModelChanged` to a UI that is leaving; the sends are
  already `let _ =`, and the UI loop has stopped reading.
- **A slot with no refund.** A task spawned before the refund tracks
  (none in practice) or the roll: `refund_unlanded` skips it, as `quit_bg`
  did.
- **The cap in tests.** `settle_silent_tasks` takes the cap as a
  parameter; the tests pass a small one and a slow test tool whose sleep
  is above or below it.

## 5. Forks

- **F1. What the quit waits for.** (a) **The tasks' own landings on
  `bg_done_rx`, each through `handle_bg_done`** *(recommended — R1: the
  stop's path decides; the state rule is the fallback past the cap)*. (b)
  Poll the `Acted` state until it leaves `InTools`, then the state rule —
  a second decision path beside the landing's. (c) A `Notify` in `Acted`
  — the same second path, one wake-up cheaper.
- **F2. The cap.** (a) **2 s for the whole wait** *(recommended — covers a
  CPU embedding; a quit stays a quit)*. (b) Unbounded until every task
  lands. (c) Per task.
- **F3. Past the cap.** (a) **The state rule as today** *(recommended)*.
  (b) Keep every unlanded window.
- **F4. Which slots.** (a) **Every active silent task** *(recommended — the
  landing is the exact answer for all, and the idle ones land at once)*.
  (b) Only `InTools` ones, the rest by the state rule immediately.
- **F5. The request guard (R3).** (a) **A token check before the stream in
  `stream_round`** *(recommended — one line, covers the no-budget engines)*.
  (b) None — the lane wait covers the pooled engines, and a cloud request
  ended at once costs a connection.
- **F6. Staging.** (a) **One PR** *(recommended)*. (b) Two.

## 6. Tests and the live run

- `tests/reflection.rs` / `tests/silent.rs`, at the orchestrator level
  with a registered slow test tool (`extra_tools` + `rebuild_registry`) and
  `spawn_loop_allowing`: a loop mid-tools at the cancel whose tool is a
  slow **reader** under the cap → the settle drains its landing and the
  window comes back; a slow **writer** under the cap → kept; a slow reader
  **over** the cap → the state rule keeps it (`InTools`); a slot at `Idle`
  with no task in tools → landed and refunded within milliseconds; the
  settle returns as soon as every active slot has landed, not at the cap.
- `tests/silent.rs`, through `run`: the quit-track tests unchanged in
  their expectations (a quit mid-stream refunds, after a write keeps, after
  reads refunds) now pass through the landing path — pinned by asserting
  the exit takes under the cap.
- `tool_loop.rs`: a loop cancelled before its stream opens sends no
  request where the engine has no session budget.
- **Live: not required** — the exit path; the LAN regression trio run
  once as the habit.

### 6.1 The run (2026-09-08)

The unit suite: **2946 green, 146 `#[ignore]`** (+5, all in
`tests/silent.rs`: a quit mid-reads refunded within the cap and with no
request after the cancel, mid-write kept, past the cap decided by the
state, mid-stream landed at once, an unbudgeted cancelled loop sending no
request). The quit-track tests through `run` keep their expectations under
the new path. The regression on the LAN stack (Qwen 3.6 27B, four slots
over 16384): `stop_silent_task_e2e_live`, `silent_roll_e2e_live`,
`background_subagent_e2e_live` — **3/3 in 37.9 s**.

## 7. Not in this track

- **Draining `compact_rx` at the quit** (a cancelled roll has nothing to
  decide) — **done** as the next track
  ([quit-settle-roll-and-cap.md](quit-settle-roll-and-cap.md)), and not
  cosmetic after all: the roll takes a slot but lands on `compact_rx`, so
  this track's settle waited the whole cap during a roll.
- **A configurable cap** — done in the same track: `tools.quit_settle_secs`,
  no cap by default.

## 8. Documentation touch list (AGENTS.md §4)

- spec §17.6 (the quit: waits for the landing, the cap), §11.10.
- architecture §3 (`run`'s exit), §9/§11 (`cancel_bg_all`,
  `settle_silent_tasks`, `refund_unlanded`), §5 (the request guard).
- [acted-by-effect.md](acted-by-effect.md) §7,
  [quit-refunds-window.md](quit-refunds-window.md) §7.
- CHANGELOG (Changed: the quit entry extended), journal `engine.md`,
  CLAUDE.md's status line and count.
