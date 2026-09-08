# A quit gives the window back too — the fact the loop keeps in the open

> **Status:** proposed (2026-09-08) — the forks in §5 await the user's
> decision; no stage-0 probe (nothing about a model's behaviour is in
> question). The item the refund track recorded and did not take
> ([stop-refunds-window.md](stop-refunds-window.md) §7, its fork F6b): a
> stop gives a silent task's window back when no round of its tools ran,
> but a **quit** is not a stop — `cancel_all_bg` ends the tasks, nothing
> lands, and the spawn-time advance stays, so a restart mid-reflection
> skips the window as every stop used to. The reason was not a decision
> about quits but a missing fact: the loop's "a round of tools ran" is
> read at the landing, and at a quit there is no landing.

## 1. Why, precisely

The refund track ([stop-refunds-window.md](stop-refunds-window.md)) put
the rule in one place: a stopped task's window comes back unless the task
had **acted on** it — `run_rounds` returns `RoundsEnd::Cancelled { rounds }`,
`spawn_silent_loop` maps it onto `BgOutcome::Cancelled { consumed }`, and
`handle_bg_done` gives the window back on `consumed: false`. The quit path
never sees that outcome: `AppCommand::Quit` cancels every slot's token,
shuts the MCP host down and returns `true`; the orchestrator's loop ends
and `flush_saves` writes what is dirty. The reflection loop reads its
token at its next chunk and lands into a channel nobody reads. Its
watermark, advanced at spawn and long since flushed, is what the next
launch starts from — the window it was reading when the user quit is
skipped.

That is the stop track's defect in the one place the refund track could
not reach, and the case is not rare: reflection runs for tens of seconds
after a reply, which is exactly when a user closes the laptop.

**Requirements.**

- **R1. The same rule.** A quit refunds exactly what a stop would have:
  the window of a task that had not yet acted on it, and nothing of one
  that had. One criterion, one function (`give_back`).
- **R2. The fact without the landing.** Whether a round of the task's
  tools ran must be readable by the orchestrator at any moment, not only
  from the outcome the loop sends last.
- **R3. Written before the exit.** The refund is a chat edit like any and
  reaches the exit flush the way the spawn-time advance did.
- **R4. No waiting at the door.** A quit stays what it is — cancel and
  leave; it does not wait for a task's tools to finish so it can read an
  outcome.

## 2. What exists (inventory)

- **The quit** (`orchestrator/mod.rs`, the `Quit` arm): cancels the turn,
  RAG, impersonation and TTS tokens, `stop_all_background_runs()` (every
  background run lands *cancelled* from its mirror — that family has its
  fact on the orchestrator's side), `cancel_all_bg()` (every slot's token),
  `mcp.shutdown()`, `return true`. `run` then calls `flush_saves()` once —
  so anything `mark_dirty` names before the arm returns is written.
- **The loop's fact**: `run_rounds` increments `round` right before
  `run_tools` — *a round's tools are about to run* — and returns it in
  `RoundsEnd::Cancelled { rounds }`; nothing outside the loop can read
  `round` while the loop runs.
- **The slot** (`BgSlot { cancel, failures, window }`): `begin_bg(kind,
  cancel, window)` is called by the spawn tails **after**
  `spawn_silent_loop` — the tails build the token and the context first,
  spawn, then take the slot.
- **The refund** (`give_back(kind, window)`): reflection's watermark and
  stamp restored and the chat marked dirty; a counter added back. The
  counters are in memory and lost at a quit anyway.
- **Tests**: `tests/reflection.rs` drives `begin_bg` and `handle_bg_done`
  by hand; `tests/silent.rs::spawn_loop` builds a `SilentLoop`; the
  background-run tests send `AppCommand::Quit` through `run` and `load`
  the chat from disk afterwards.

## 3. Design

### 3.1 The fact in the open

The spawn tail creates `acted: Arc<AtomicBool>` and hands a clone to the
loop (`SilentLoop.acted` → `run_rounds`), which stores `true` at the very
line that increments `round` — the one place "a round's tools are about
to run" is decided — and the original to the slot, beside the window:

```rust
pub(super) struct Refund { pub window: Window, pub acted: Arc<AtomicBool> }
// BgSlot.refund: Option<Refund>;  begin_bg(kind, cancel, refund: Option<Refund>)
```

The flag and `rounds > 0` are the same fact from the same line; the
landing keeps reading the outcome (`consumed`), the quit reads the flag.
The roll passes `None`, as it does today.

### 3.2 The quit

`cancel_all_bg(&self)` becomes `quit_bg(&mut self)`: for every slot, cancel
the token, then take its `refund` and — if `!acted` — `give_back(kind,
window)`. The arm calls it where `cancel_all_bg` was; `flush_saves` after
the loop writes the restored watermark (R3). A counter's refund at a quit
lands in a map that is about to be dropped: harmless, and one path (fork
F2).

### 3.3 The race at the tools' door

The orchestrator cancels the token and then reads the flag; the loop, on
another worker, may be between the end of a `ToolCalls` stream and the
line that sets the flag. Two orders are possible, and one needs a guard:

- the loop sets the flag before the orchestrator reads it → *kept*;
  correct whether or not the tools then run;
- the orchestrator reads `false` and refunds, then the loop sets the flag
  and runs its tools → a write into a refunded window.

The guard is a check of the token **before** `run_tools`: a loop whose
token is already cancelled returns `Cancelled { rounds }` without starting
its tools. With it, the second order cannot write: the token was cancelled
before the read, so the loop's check — which comes after its own store —
sees it and stops. The guard is also a small improvement in its own
right: a stop that lands during the last chunks of a `ToolCalls` stream no
longer starts a round of tools nobody will read (fork F3).

### 3.4 What does not change

The landing path (`handle_bg_done`, `consumed` from the outcome); the
roll; the background runs' quit; what a quit does to everything else; the
tasks screen and the commands.

## 4. Difficult spots

- **Two readers of one fact.** The outcome's `consumed` and the slot's
  `acted` are set at the same line and read at different moments — the
  landing and the quit. Deriving `consumed` from the flag instead would
  make one source in the code as well as in fact, at the cost of `run_rounds`
  returning a count it no longer uses; §5 F4 has the choice.
- **`SeqCst` for a bool set once.** The store and the load are on
  different tasks and the guard's correctness rests on their order
  relative to the token; `SeqCst` on both, and the token's own atomics,
  make the argument in §3.3 hold without a proof about `Acquire`/`Release`
  pairs.
- **The exit flush after a refund for a chat never flushed.** A reflection
  spawned and quit within the save queue's debounce: the advance and the
  refund are both dirty edits of one chat, the flush writes the final
  state — the pre-spawn values — once.

## 5. Forks

- **F1. How the fact reaches the quit.** (a) **A flag on the slot the
  loop sets where it counts the round** *(recommended — R2, R4; the loop's
  own fact, read without waiting)*. (b) Refund blind — a read window
  written twice at the next launch, the defect the criterion exists for.
  (c) Wait at the quit for every task to land and read its outcome — the
  door held for a round of tools, and the outcome channel is read by a
  loop that has already returned.
- **F2. Which windows at a quit.** (a) **Every active one, through
  `give_back`** *(recommended — one path; a counter's refund is moot and
  harmless)*. (b) Reflection's only.
- **F3. The guard before the tools.** (a) **A cancelled loop never starts
  its tools** *(recommended — closes §3.3, and a stop during the last
  chunks of a stream starts nothing)*. (b) No guard — the race is
  microseconds wide and its cost is one duplicate.
- **F4. One source in the code.** (a) **Keep `RoundsEnd::Cancelled {
  rounds }` for the landing and the flag for the quit** *(recommended —
  the last track's shape stands; both are set at one line)*. (b) Derive
  `consumed` from the flag too and drop `rounds`.
- **F5. Staging.** (a) **One PR** *(recommended)*. (b) Two.

## 6. Tests and the live run

- `tests/silent.rs`: a loop whose first round calls a tool has `acted`
  true once its second stream opens, a loop stopped mid-first-stream has
  it false; a loop whose token is cancelled as its `ToolCalls` stream ends
  starts no tools (the recorder sees one request; the outcome is
  `Cancelled`).
- `tests/reflection.rs`: a reflection spawned through `maybe_auto_reflect`
  and quit (`quit_bg`) with `acted` false → watermark and stamp back, the
  chat dirty, and after `flush_saves` the chat on disk reads `None`; with
  `acted` true → kept; the roll's slot at a quit → nothing.
- The `Quit` arm: through the background-run harness if the profile can
  enable reflection there, else through `quit_bg` directly — the arm is a
  call.
- **Live: not required** — the loop gains a store on one line and a check
  before its tools; the LAN regression trio is run once as the habit.

## 7. Not in this track

- **Effects-based "acted on"** (the refund track's §7) — unchanged.
- **A quit that waits for the tools** (F1c).

## 8. Documentation touch list (AGENTS.md §4)

- spec §17.6 (a quit mid-reflection), §11.10 (the stop sentence gains the
  quit).
- architecture §5 (`SilentLoop.acted`, the guard), §9/§11 (`Refund`,
  `quit_bg`).
- [stop-refunds-window.md](stop-refunds-window.md) §5 F6 / §7 (F6b — done
  here).
- CHANGELOG (Changed: the previous entry extended), journal `engine.md`,
  CLAUDE.md's status line and count.
