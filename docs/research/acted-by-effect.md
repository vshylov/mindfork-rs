# "Acted on" by effect — a silent task's window is consumed by a write, not by a round

> **Status:** implemented (2026-09-08) — every fork at its recommendation
> (the user's decision, 2026-09-08); the regression run in §6.1; no stage-0
> probe (nothing about a model's behaviour is in question).
> The item the refund track recorded and the quit track kept
> ([stop-refunds-window.md](stop-refunds-window.md) §7,
> [quit-refunds-window.md](quit-refunds-window.md) §7): a stopped or
> quit silent task gets its window back unless it had **acted on** it,
> and "acted on" has meant *a round of its tools was about to run* — the
> cheap criterion, taken first. A reflection whose first round only reads
> its self-model keeps its window under that rule although nothing was
> written; this track replaces the round with the effect.

## 1. Why, precisely

The refund rule rests on one argument (stop-refunds-window §1): a window
the task has acted on must not be read twice, because reflection's digest
is over the window only so that its observations are written once. "Acted
on" was then approximated by "a round of tools ran" — honest in the
direction of the rule (never refund a written window) but coarse the
other way. Reflection's tool set is half readers: `get_self_model`,
`note_recall`, `note_neighbors` are how it looks before it writes, and a
first round that only looks is the common shape of a run that is stopped
seconds after it starts — the user sees the chip, the model is still
reading. Under the round criterion that stop keeps the window; the
replies are skipped for a run that changed nothing.

The exact criterion is the effect: **did a call of this task change the
profile's stored memory** — the self-model or a note. That fact exists in
exactly one place, the writer tool at the moment it writes, and the loop
already receives every tool's outcome. What has to be designed is how the
fact travels (§3.1), how the quit reads it when it arrives *after* the
tools rather than before (§3.2), and what the landing reports (§3.3).

**Requirements.**

- **R1. The same rule, the exact fact.** A window comes back unless a
  call of the task wrote to the profile's memory; a round of reads
  consumes nothing. Stop and quit agree.
- **R2. Never refund a written window.** Every ambiguity — a tool that
  failed midway, a quit while a writer is running — resolves to *kept*.
- **R3. The fact is the tool's.** The writer says it wrote, on the outcome
  it returns; nobody keeps a list of writers by name beside the tools.
- **R4. One source.** The landing's `consumed` and the quit's reading are
  the same accumulated fact, not two counts.

## 2. What exists (inventory)

- **The contract** (`features/tools/mod.rs`): `ToolOutcome { result,
  effects: Vec<ChatEffect>, images }` — `effects` are *chat* effects
  (system message, sampling override, an attachment); nothing on the
  outcome says whether stored memory changed. Three constructors
  (`text`, `with_effects`, the `with_images` builder); no literal
  constructions outside the module.
- **The marks.** `Tool::concurrent()` (ADR 0012) is a *read-only* claim —
  but for a different purpose, and it is not the reader set the loops
  need: `note_recall` is deliberately unmarked ("it writes vectors inside a
  read"), `note_neighbors` is unmarked at all. Reusing it would keep a
  window that a `note_recall` merely searched.
- **The writers in the loops' sets.** Reflection: `update_self_model`,
  `update_user_model`, `add_insight`, `note_revise`, `note_supersede`,
  `note_merge`, `note_link` (readers: `get_self_model`, `note_recall`,
  `note_neighbors`); notes consolidation: the four note writers; self
  consolidation: the two model writers and the four note writers. Every
  writer has a refusal path that writes nothing (a missing id, nothing to
  change — `selfmodel.result.nothing`) and a success path that does.
  Outside the loops' sets, `note_save` and `note_cite_source` also write
  memory.
- **The loop** (`tool_loop.rs`): `run_tools` → `invoke_allowed` → the
  registry's `invoke`, keeping `o.result` and dropping the rest; `round`
  incremented and `acted` stored *before* the tools, the token checked
  right after the store (the quit track's guard);
  `RoundsEnd::Cancelled { rounds }` → `BgOutcome::Cancelled { consumed:
  rounds > 0 }`; `Refund { window, acted: Arc<AtomicBool> }` on the slot,
  read by `quit_bg`.

## 3. Design

### 3.1 The fact travels on the outcome

`ToolOutcome` gains `wrote: bool` — *this call changed the profile's stored
memory* — `false` from every constructor, set by a builder `.wrote()` on
the success path of each writer (fork F5: all nine memory writers, so the
contract is whole for any later caller). `invoke_allowed` returns the
result **and** the fact: `Ok(o) → o.wrote`, `Err(_) → true` (fork F3: a
tool that failed may have written before it failed, and the loop cannot
know how far it got — R2), a name outside `allowed` → `false` (nothing
ran). `run_tools` ORs the calls' facts into the round's.

### 3.2 The quit reads a state, not a bit

With the fact arriving after the tools, a bit set before them is no longer
enough for the quit: the guard of quit-refunds-window §3.3 stores before
the tools so that a refund can never race a write, and that store must
stay — but it now means *tools are running*, not *acted*. The slot's flag
becomes a three-valued state:

```
Idle     — no tools running, nothing written so far   → a quit refunds
InTools  — a round's tools are running                → a quit keeps (R2)
Wrote    — a call wrote                               → kept for good
```

The loop stores `InTools` where it stored `true` (the guard unchanged),
then after the round's tools `Wrote` if the round wrote, else back to
`Idle` unless it is already `Wrote`. `quit_bg` refunds `Idle` only. A quit
during a reading round keeps the window — a run of reads is a few hundred
milliseconds, the state returns to `Idle` when they end, and the
conservative side is the rule's.

### 3.3 The landing reports the same fact

`RoundsEnd::Cancelled { rounds }` becomes `Cancelled { wrote: bool }` — the
loop's accumulated fact — and `spawn_silent_loop` maps it onto
`BgOutcome::Cancelled { consumed: wrote }` (fork F4; the quit track's F4a
kept the count while the count and the flag were one fact — they no
longer are, and the count has no reader). `handle_bg_done` and
`give_back` are untouched.

### 3.4 What the user sees

A reflection stopped, or the app quit, while it was still *reading* —
`get_self_model`, a recall, the neighbours of an observation — comes back
at the next landing over the same replies; one that has written keeps its
place. Nothing else changes: the roll, the streak, the notes.

## 4. Difficult spots

- **A writer that refuses.** `note_revise` on a missing id, `update_user_model`
  with nothing to change: the success path is the one that reports, so a
  refusal consumes nothing — exactly the exactness the track is for, and
  the reason a static "this tool writes" claim (F1b) is the coarser answer.
- **A tool's `Err`.** Conservative by rule (F3a); in these tools an `Err`
  is an argument shape the schema should have refused, and rare.
- **`note_recall` writes vectors.** A cache, not memory: the recall does
  not report, and ADR 0012's reason for leaving it unmarked (a sibling call
  could observe the write) is about concurrency, not about the window.
- **The report and the write are one site.** Each writer's `.wrote()`
  sits on the line that returns after the storage call succeeded; a test
  per writer pins the success path to `true` and a refusal to `false`, so
  a new writer cannot report by accident and an old one cannot stop.
- **The main loop ignores the field.** The turn's agentic loop applies
  `effects` and shows `result`; `wrote` is nobody's business there, and it
  costs one bool on a struct that already carries a `Vec` of images.

## 5. Forks

- **F1. Where the fact comes from.** (a) **The tool's own report on its
  outcome** (`ToolOutcome.wrote`) *(recommended — R3: the effect itself,
  set where the write happens; a refusal reports nothing)*. (b) A static
  per-tool claim (`Tool::writes_memory()`), a round with any writer call
  counting — the `concurrent()` shape; simpler, coarser, and the quit's
  guard keeps its old meaning. (c) Reuse `concurrent()` — wrong for
  `note_recall` and `note_neighbors`.
- **F2. The quit's reading.** (a) **Three states — `Idle`/`InTools`/`Wrote`
  — and a quit refunds `Idle` only** *(recommended — R2; the guard's store
  stays where it is)*. (b) Refund unless `Wrote` — a quit during a
  writing tool would refund a window being written.
- **F3. A tool's `Err`.** (a) **Counts as a write** *(recommended — R2)*.
  (b) Does not.
- **F4. The landing.** (a) **`Cancelled { wrote }`, `consumed = wrote`**
  *(recommended — R4; the round count has no reader left)*. (b) Keep
  `rounds` beside it.
- **F5. Which tools report.** (a) **Every tool that changes the profile's
  stored memory** — the seven in the loops' sets plus `note_save` and
  `note_cite_source` *(recommended — the contract is whole)*. (b) The
  seven only.
- **F6. Staging.** (a) **One PR** *(recommended)*. (b) Two.

## 6. Tests and the live run

- The tools (`self_model.rs`, `notes/tests.rs`): each writer's success
  path returns `wrote`, its refusal path does not; `get_self_model`,
  `note_recall`, `note_neighbors` never do.
- `tests/silent.rs`: a loop whose first round calls only a reader lands
  `consumed: false` and leaves the state `Idle`; one whose round calls a
  writer with valid arguments lands `consumed: true` and leaves `Wrote`; a
  disallowed name is neither; the state reads `InTools` while a round's
  tools run (a slow tool, or the recorder's delay before the next stream).
- `tests/reflection.rs`: `quit_bg` refunds an `Idle` slot, keeps `InTools`
  and `Wrote`.
- The refund and quit tracks' tests, re-read under the new fact (their
  `consumed: false` stops were never in a round; the `one_call()` script
  calls `get_self_model`, a reader — those tests change their expectation
  to *refunded*, which is the point).
- **Live: not required** — the loops' engine paths are the same; the LAN
  regression trio run once as the habit.

### 6.1 The run (2026-09-08)

The unit suite: **2941 green, 146 `#[ignore]`** (+4 net: the two previous
tracks' three round-based tests in `tests/silent.rs` replaced by four
effect-based ones — a round of reads, a write, a disallowed call, the two
quits through the orchestrator's own loop with the chat read from disk —
plus one in `tests/reflection.rs` for a quit mid-tools, one in
`self_model.rs` and one in `notes/tests.rs` for the writers' reports). The
`InTools` state is pinned only through the quit's reading of it: the
recorder cannot hold a round's tools open. The regression on the LAN
stack (Qwen 3.6 27B, four slots over 16384): `stop_silent_task_e2e_live`,
`silent_roll_e2e_live`, `background_subagent_e2e_live` — **3/3 in 61.4 s**.

## 7. Not in this track

- **A quit that waits for a running round's reads** to settle to `Idle`
  before deciding — a few hundred milliseconds at the door for a rare
  case — **done** as its own track
  ([quit-waits-for-the-landing.md](quit-waits-for-the-landing.md)): the
  quit listens for the cancelled task's own landing, and the stop's path
  decides.
- **Reporting on the main loop's side** (a turn's own writes) — no consumer.

## 8. Documentation touch list (AGENTS.md §4)

- spec §9.2 (the outcome's `wrote`), §17.6 and §11.10 (the criterion
  reworded: written, not run).
- architecture §5 (`RoundsEnd::Cancelled { wrote }`, the state), §8
  (`ToolOutcome.wrote`, the writers), §9/§11 (`Refund`'s state, `quit_bg`).
- [stop-refunds-window.md](stop-refunds-window.md) §7 and
  [quit-refunds-window.md](quit-refunds-window.md) §7 (done here).
- CHANGELOG (Changed: the entry reworded), journal `engine.md` and
  `tools.md`, CLAUDE.md's status line and count.
