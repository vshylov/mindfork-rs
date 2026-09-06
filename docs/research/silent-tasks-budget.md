# The silent tasks under the app-wide budget

**Status:** design; the forks in §6 are open for the user's decision. The
last item [background-subagents.md](background-subagents.md) §8 and
[admission-by-budget.md](admission-by-budget.md) §8 left for later, and the
one [tasks-screen.md](tasks-screen.md) §8 carried forward. The parent's fork
F9 ([parallel-subagents.md](parallel-subagents.md) §6) put these tasks
*outside* the budget on the argument that "they coexist with the turn on the
server today"; §2 below shows that coexisting on a **unified** pool is the
collective failure the admission track measured, and that the app's own
default launch shape puts every silent request on that pool.

Related: spec [§6.3](../../spec.md) (the session budget), [§6.7](../../spec.md)
(compaction), [§9.3.2](../../spec.md) (background runs), [§11.10](../../spec.md)
(the tasks screen), [§17.6](../../spec.md) (auto-reflection); architecture
§5 (`SessionBudget`), §9 (the silent loops).

---

## 1. Why, precisely

The session budget (`shared/session_budget.rs`) is **app-wide** since the
background-subagent track: one `Arc` per engine section, cloned into every
turn and every background run, so at `sessions = 1` a run and the next turn
take turns round by round, and under a shared KV pool a stream that would
not fit beside the open ones waits instead of provoking the server's
"Context size has been exceeded" — which ends *every* running conversation
at once. Everything the **turn machinery** opens is under it: the loop's own
stream, a sub-agent's round, a dialogue's line, a director checkpoint, a
`fetch_url` summary.

Everything else the app asks the engine for is not. Seven request paths
reach `backend.chat_stream` with no permit and no reservation
(§2.2): the automatic title, the three silent loops (reflection, notes
consolidation, self-model consolidation), the compaction roll, impersonation
in *shared* mode, and a `fetch_url` summary made from inside a silent loop.
They were left out twice, deliberately (parallel F9, background-subagents
§4.7), on the reasoning that they had always shared the server with the turn
and nothing had gone wrong.

What that reasoning missed is *where* they share it. At the default
`sessions = 1` the launcher writes no `-np` at all, and a `llama-server`
without `-np` runs **four slots over one unified pool** (its default since
December 2025 — the fact the parallel track itself rests on). The app keeps
its own streams to one at a time; a silent request lands on a second slot of
the same pool, beside whatever is streaming. And the one silent request
sized by the conversation — the compaction roll — fires **exactly when the
conversation is at its largest**: at 75 % of the window, with a digest of
everything but the last 2048 tokens. On the LAN stack's numbers (`-c
16384`): a 12k-token turn has just landed, the roll's request is ~10k of
digest plus a 2048 cap, and the next turn — the user's, or the wake turn the
app now starts by itself — is 12k plus 2048. Together they are far past
16k; the server halves its batch to one, then ends both slots. The roll
fails silently (a `debug` line), the turn fails in front of the user, and
nothing in the app knew the two were on one pool. The same shape, smaller,
for every silent loop: a reflection digest of the recent window plus its
2048 cap beside a long conversation.

Two more things §2 turns up. After a single landing the orchestrator can
start **five** silent requests at once (§2.4) — each gated only by its own
kind's "one at a time" — beside the wake turn. And a collision is invisible
on the silent side: the loop runner reads the server's in-stream error as
the end of a round and lands the task `Ok` (§2.5) — the streak resets, a
reflection announces `SelfModelChanged` for a window it never read, and
the only witness is one `warn` line in the log.

**Requirements.**

- **R1.** Every request the app opens against the chat engine counts under
  the same budget the turns stream under: a permit, and under a pool a
  reservation — no request can overfill the pool beside another.
- **R2.** The interactive path — the user's turn, its sub-agents, a wake
  turn — must not get slower at the defaults than it is today, or not by
  more than one bounded silent round; the silent tasks yield, never the
  reverse.
- **R3.** No silent task is lost to the budget: a task that waited, or was
  cancelled to make room, runs later — the watermark it advanced, the
  cadence it consumed, the title it owed are not spent for nothing.
- **R4.** At one session with no pool known, and on the clouds, nothing
  about the turn path changes byte for byte — the property the budget has
  kept twice.
- **R5.** Observable: the tasks screen (spec §11.10) and the log say when a
  silent task is waiting for room rather than running.
- **R6.** Measured before designed: the collision is reproduced through the
  app's own paths, not inferred from the probe of the parent track.

## 2. What exists (inventory)

Anchors at `3437dd6`.

### 2.1 The budget, and who holds it

`SessionBudget` (`shared/session_budget.rs:31`): `sessions` permits over an
optional pool; `price` (`:143`) — the calibrated prompt estimate floored by
the loop's last exact size, plus the reply cap, or the whole pool when there
is no cap; `acquire` (`:156`) — a permit, then a wait for room unless the
stream is alone. `Orchestrator::session_budget()`
(`background_runs.rs`, the memo keyed by mode, count and pool) hands the one
`Arc` to `start_generation` (`generation.rs:685`) and to every background
run (`:77`); `TurnShared.sessions` and `ToolContext.sessions` carry it into
the loops and into `fetch_url` (`features/tools/fetch.rs:457`). The
turn's own stream normally *has* a cap — `default_sampling.max_tokens` is
2048 (`config.rs:2040`) — so it reserves prompt + 2048, not the pool; the
"no cap → the pool" clause is the cleared-field case.

### 2.2 The seven requests outside it

| request | where | shape | cap | bound | trigger |
|---|---|---|---|---|---|
| auto-title | `title.rs:238` | one stream | 2048 | 60 s | the first reply; every landed run |
| reflection | `reflection.rs:263` → `tool_loop.rs:139` | a loop, ≤ 6 rounds, with tools | 2048 | 120 s | every N replies (`auto_reflect_every`) |
| notes consolidation | `consolidation.rs:152` → the same loop | ≤ 8 rounds | 2048 | 180 s | every N replies |
| self-model consolidation | `self_consolidation.rs:186` → the same loop | ≤ 8 rounds | 2048 | 180 s | every N replies |
| compaction roll | `compaction.rs:443` | one stream, a digest of the folded span | 2048 | 180 s | `/compact`; automatically at `threshold_pct` (75) of the window |
| impersonation, *shared* engine | `impersonation.rs:259` | one stream | 1024 | — | `Ctrl+U`, blocked while a turn runs (`:58`) but not while a background run streams |
| `fetch_url` summary inside a silent loop | `fetch.rs:457` with `ctx.sessions = None` (`mod.rs:1526`) | one stream | `SUMMARY_MAX_TOKENS` | the tool's | a silent loop that fetches a page |

Every one is `backend_if_ready` (`engines.rs:359`) → `chat_stream`, through
the retry decorator like a turn's, with a cancel token of its own and
nothing in between. `background_tool_ctx` says it in a comment: *"Outside
the session budget, as every background task is (fork F9)"*. Impersonation
on its **own** engine (`ImpersonationMode::Managed`/`External`) streams to a
different server with a different pool and at most one stream at a time —
not this track's.

The three silent loops share one runner, `spawn_silent_loop`
(`tool_loop.rs:79`): a fully owned `SilentLoop` — backend, registry,
`ToolContext`, request, allowed tools, token, `max_rounds`, timeout, kind,
`done_tx`. One insertion point covers three of the seven.

### 2.3 The pool at one session is `None`, and the server has four slots

`pool_for` (`orchestrator/pool.rs:20`) answers `None` whenever `sessions ≤
1`: "one permit; no two streams ever overlap". True of the streams the
budget knows. The launcher (`shared/api/managed.rs:89`) adds `-np N
--kv-unified` only above one session; at one it writes nothing, and the
server's own default is four slots over one unified `-c`. So at the
default the pool is real, unknown to the app, and shared by the turn and
every silent request. The external row has the same gap the other way: a
llama.cpp reporting four slots gets a pool only above one session.

### 2.4 The fan-out after a landing

`handle_done` (`generation.rs:947–967`) calls, in order: `maybe_auto_title`
(the first reply), `maybe_auto_title_run` per landed run, then reflection,
notes consolidation, self-model consolidation, and compaction last. Each
checks only `bg_running(kind)` — its own slot — so with the cadences aligned
five requests can open within the same tick, beside the wake turn a landed
background run may start (`maybe_wake`). Five silent streams on four slots
is the server's queue; four silent streams and a turn on one 16k pool is
§1's collision, several times over.

### 2.5 What each task does when it fails

`handle_bg_done` (`background.rs:51`) counts an `Err` into the kind's
streak and shows one error at three (`BACKGROUND_FAILURE_ALERT`) — but a
collision never reaches it as an `Err`: `read_round` (`tool_loop.rs:161`)
logs the in-stream `Error` chunk as a `warn`, takes the `Finished(Error)`
that follows as the round's end, and `run_rounds` returns `Ok(())` on a
round with no calls. The task lands as a success; reflection and self-model
consolidation then emit `SelfModelChanged`. A title ends the same way with
`ui.err.title_empty` in the log (an automatic one) or the list (a requested
one); the roll carries the failure out and reports it only when `/compact`
asked. Reflection advances `chat.reflected_upto` **at spawn**
(`reflection.rs:228`), so a window lost to anything is not re-read; the
consolidations reset their counters at spawn too (`consolidation.rs:114`);
a title is not retried; a deferred auto-compaction (server not ready) is
deliberately *not* a strike and re-triggers at the next landing
(`compaction.rs:226`). The silent loops have no retry of their own beyond
the decorator's transient policy.

### 2.6 What the tests can already say

`tests/parallel.rs` drives the budget with `KeyedRecorder` — scripts keyed
by system-message substring, an exact `usage` per script (`sized`), a
per-stream delay, `max_in_flight` and `open_at_arrival` — and pins
turn-taking under a pool (`siblings_that_do_not_fit_the_pool_take_turns`).
`tests/background.rs` pins a run and the turns taking turns at one session
(`max_in_flight == 1`). `tests/reflection.rs` drives `maybe_auto_reflect`
on a bare orchestrator. The silent loops have no concurrency test at all:
nothing today asserts how many streams a landing opens.

## 3. Measurements to make (before designing further)

**Go/no-go probe (stage 0).** Two arms on the CPU build, launched exactly
as the managed launcher would at `sessions = 1` — no `-np`, `-c 2048` — so
the server's own four-slot default is what is measured, not a flag:

1. **The roll beside a turn.** A chat sized so that auto-compaction fires at
   the landing (the turn's `usage` at ≥ 75 % of 2048) while a scripted
   background run keeps a second stream open; then a wake turn. Expected on
   `main`: the roll and the turn both end with the pool error, the roll's
   failure a `debug` line, the turn's an error in the feed.
2. **The fan-out.** Cadences set to 1: one landing opens title + reflection
   + both consolidations + the roll. Record `open_at_arrival` on the
   server side (`/slots`) and the outcomes.

**GO** if arm 1 reproduces through the app's paths; then the design below
is built. **NO-GO** — the server serialises or the pool is per slot at the
default — and the track shrinks to §2.4's fan-out (a silent lane of one)
plus the pool rule at one session, with the tests of §7 and no reservation
work. The instrument is `admission_control_e2e_live`'s shape
(`tests/live.rs`), not a new script: the CPU stack is the free gate
(install.md §7.3), and `MINDFORK_LLAMA_BIN` launches it as the app would.

**Also measured, not decided:** the cost of R2's wait — how long a reflection
round holds the pool on the LAN stack (the 27B at ~50 tok/s and a 2048 cap
is up to ~40 s worst case, but reflections rarely fill their cap); the
number decides F3.

## 4. Design

### 4.1 One budget, two lanes

`SessionBudget` gains a **silent lane**: a second permit count beside the
interactive one, over the **same** pool and the same `in_flight` sum.

```
SessionBudget {
    permits:        Semaphore(sessions),   // the interactive lane, as today
    silent_permits: Semaphore(1),          // the silent lane (F5)
    pool, in_flight, room, density         // shared by both lanes
}
acquire(need, cancel)         -> Reservation   // interactive, unchanged
acquire_silent(need, cancel)  -> Reservation   // a silent permit + the same room wait
```

A silent stream takes a silent permit, then waits for room under the same
rule as any stream (`in_flight + need ≤ pool`, or alone), and releases both
when it ends. Two consequences fall out with no further mechanism:

- the silent tasks **take turns among themselves** — §2.4's fan-out becomes
  a queue, the server sees one silent stream at a time, and a landing's five
  requests spread over the minute after it instead of the second;
- a silent stream **never overfills the pool** beside a turn, a run or a
  scene: it reserves its digest plus its cap like every other stream, and
  the interactive stream that arrives next reserves beside it or waits.

The interactive lane's semantics do not change: `sessions` permits, the
same pricing, the same "alone is admitted" clause. What changes for the
turn path is only that a silent reservation can now be among the open ones
it measures against (§4.4).

### 4.2 What a silent stream reserves

`price(estimate, 0, Some(cap))` — the calibrated estimate of the request
about to be sent, no floor (a silent loop keeps no `last_usage`; adding one
is cheap and §7 pins it for the loops' later rounds), plus the task's cap:
2048 for the loops and the roll, `TITLE_MAX_TOKENS` for a title, 1024 for
impersonation, `SUMMARY_MAX_TOKENS` for a summary. Every silent request has
a cap today, so the "no cap → the pool" clause is never reached from the
silent lane. The estimator's density is the budget's — the turn's exact
usage calibrates a roll's reservation as it calibrates a child's.

The insertion points are the seven rows of §2.2: `run_rounds` around
`chat_stream` (three tasks at once, through a `sessions` field on
`SilentLoop`), `spawn_title`, `spawn_compact`, `spawn_impersonation` in
shared mode, and `background_tool_ctx` handing the budget into
`ToolContext.sessions` — after which `fetch_url` needs no change, since it
already reserves whatever budget it is given; a `lane` flag on the context
tells it which permit to take.

### 4.3 The pool at one session

`pool_for` stops keying on `sessions`. The pool is known when the server
runs **more than one slot over one context**: managed — always, unless the
user's extra arguments say `-np 1` (then the reported `total_slots` is 1,
and the app should believe it: the rule becomes *managed, reported slots ≠
1 → context_budget*, with "not reported yet" reading as the launcher's
default of four); external — reported slots > 1, as today. The clouds stay
`None`. Consequence for the interactive lane at `sessions = 1`: its single
permit still serialises the turn path, so nothing there waits for room
that would not have waited for the permit — the pool only starts to matter
between the lanes, which is the point. R4 holds on the clouds and on an
external server that reports nothing.

### 4.4 The lanes' interplay (F3)

With a pool, a turn arriving while a silent stream is open reserves beside
it when it fits — a 12k turn beside a 2k title on a 16k pool — and waits
when it does not. The wait is bounded by one silent round (one stream:
≤ 2048 generated tokens, and a loop drops its reservation between rounds
like a turn's loop does). The reverse never happens: a silent stream
arriving while an interactive one is open waits or fits, and the
interactive stream's own next round is priced first only in the sense that
the silent lane cannot take more than one reservation at a time.

The alternative, **preemption** — cancel the open silent stream when an
interactive one needs its room — makes the turn wait for nothing, at the
price of a wasted round and of R3's bookkeeping (the reflection watermark
would have to move at *success*, the counters be refunded, the title
re-queued). It is the better behaviour on a slow engine and the more
invasive change; §3's timing measurement decides it, and it can be added
behind the same `acquire_silent` later without touching the callers.

### 4.5 The order of the queue

The silent lane is FIFO by the semaphore. The order requests are *made*
in `handle_done` therefore becomes the order they run: today title first,
compaction last "because it reads the freshest usage" — a reason about the
data it reads, not about when its stream should open. Under a queue the
roll should go **first after the title**: it is the one silent task that
protects the *next* turn (spec §6.7), and a reflection ahead of it can hold
the lane for a minute. Proposed: title, roll, reflection, notes, self-model.

### 4.6 What the user sees

The tasks screen (spec §11.10) already lists the four silent tasks as
running/idle off the slot registry. A task waiting for its permit or for
room is *running* by that registry (its slot is taken at spawn), which is
honest about the slot and silent about the wait; R5 asks for one word more.
The cheapest true source is the budget itself: `SessionBudget` records
whether the silent lane's holder is streaming or waiting (an atomic set by
`acquire_silent` around the wait), and the snapshot reads it into a third
state, *waiting* — the row reads "reflection · waiting for room". No new
event, no new field on the registry; `AppTask` gains a `waiting: bool`
the screen words (F10). A `tracing::info` line at the wait names the task
and the numbers, as the interactive wait's does.

### 4.7 What does not change

The interactive lane's pricing, floor, calibration and "alone" clause; the
launch line (no `-np` at one session — the server's own default is what is
guarded now, not changed); the retry decorator; impersonation on its own
engine; RAG and `/reindex` (the embedder is a different server); TTS (no
engine); the cadences, the strike counter and the one-at-a-time gates of
the silent tasks — the lane is *beneath* them, not instead of them.

## 5. Difficult spots

- **The turn's stream with the field cleared.** A profile whose
  `max_tokens` is empty makes the turn's own stream reserve the whole pool
  (§2.1's clause), and under §4.3 the pool is now known at one session: a
  silent stream can then never overlap that turn — strict alternation,
  round by round — rather than streaming beside it on a spare slot as
  today. Correct (the app cannot know how long an uncapped reply grows) and
  slower for that profile; the hint of the `max_tokens` field should say
  so, and the journal records it as the one deliberate slowdown.
- **A silent loop's later rounds.** A reflection's third round carries the
  first two rounds' tool results; the estimate covers them, the floor
  (`last_usage`) is what a turn's loop has and the silent loop lacks —
  add it (`read_round` already sees the `Usage` chunk it ignores today).
- **The roll's digest is not the request.** `plan_roll` builds the request
  in the orchestrator; the reservation must price *that* request, so the
  price is taken inside `spawn_compact` from the `ChatRequest` it was
  handed, not from the plan's cut.
- **Impersonation's own gate** (`imp_gen`, `!gen_state.is_idle()`) stays:
  the silent permit is for the server, the gate is for the input box.
  A `Ctrl+U` while a background run streams now waits for room like any
  request, and the preview shows nothing meanwhile — the existing
  "impersonating…" spinner is the indicator, and its start should mark the
  wait honestly (a note is the door not left open, docs/lessons.md §4).
- **The wake turn behind the fan-out.** A landed background run wakes the
  assistant *and* starts the fan-out in the same `handle_done`; the wake
  turn takes the interactive lane, the five silent requests queue on
  theirs, and under a pool the wake turn reserves beside whichever silent
  stream is open first. §4.5's order matters here: the title's stream is
  short, the roll's is not.
- **The memo's key.** `session_budget()` is memoized per (mode, sessions,
  pool); the pool now changes when `/props` first answers at one session,
  so the memo rebuilds then — a stream holding the old `Arc` keeps its
  reservation, the next takes the new one, as today on a `sessions` edit.
  Two budgets can therefore overlap for one round; both lanes are on the
  old one's pool for that round, which is the status quo's risk for one
  round and no worse.
- **The count on the bar.** "in background: n" counts runs; the silent
  indicators are per kind. Nothing here changes either; the tasks screen
  is where *waiting* lands (§4.6).

## 6. Forks

- **F1. Which requests join.** (a) **All seven of §2.2** *(recommended —
  R1 is "every request"; the paths are seven lines around seven
  `chat_stream` calls)*. (b) The five silent tasks only; impersonation and
  the in-loop summary stay outside (impersonation has a gate of its own,
  the summary is rare). (c) The roll and the loops only — the ones sized
  by the conversation; the title is small.
- **F2. The lane.** (a) **A second permit count on the same budget, the
  silent lane of §4.1** *(recommended — the queue and the reservation in
  one mechanism, one `Arc`, one pool sum)*. (b) The silent tasks take
  *interactive* permits: the simplest patch, and at one session every
  silent round stalls the user's next message and the fan-out serialises
  ahead of a wake turn — R2 fails at the default. (c) A reservation
  without a permit — room only: keeps today's concurrency on the spare
  slots and prevents the collision, but five silent streams still open at
  once (§2.4) and the server, not the app, queues them.
- **F3. The interplay under a pool.** (a) **The interactive stream waits
  for the open silent round** *(recommended for v1 — bounded by one round,
  no retry semantics; §3 measures the bound)*. (b) Preemption: the silent
  stream is cancelled to make room, with R3's bookkeeping (watermark at
  success, counters refunded, the title re-queued). (c) Both, behind a
  setting — rejected: a knob for a wait the user cannot observe.
- **F4. The pool at one session.** (a) **Known whenever the server runs
  more than one slot** *(recommended — §4.3; managed reads the reported
  slot count, defaulting to the launcher's four)*. (b) Keep `pool_for`'s
  rule and guard the silent lane by permits alone — the collision of §1
  stays possible at the default.
- **F5. The silent lane's width.** (a) **One** *(recommended — one silent
  stream at a time is what the fan-out needs, and it is the parked-set
  advice's shape: one more conversation to park, not five)*. (b)
  `sessions` — the same width as the interactive lane; more silent
  throughput above one session, and at one it is (a).
- **F6. The queue's order after a landing.** (a) **Title, roll,
  reflection, notes, self-model** *(recommended — §4.5)*. (b) Today's:
  title, reflection, notes, self-model, roll.
- **F7. The watermark on a lost reflection.** (a) **Unchanged in v1** — the
  wait cannot lose a run (nothing cancels it but the cadence's own
  timeout), so R3 holds without moving the watermark. (b) Move it to
  success now, ahead of F3(b) — a behaviour change to a feature this track
  is not about.
- **F8. What the tasks screen says.** (a) **A third state, *waiting*, read
  off the budget** *(recommended — §4.6; the screen was built to be a
  projection, and this is one more field of it)*. (b) Running/idle as
  today; the log line is the only witness.
- **F9. Staging.** (a) **A stage-0 probe with a go/no-go, then one PR**
  *(recommended — the code is seven insertion points, one type and one
  rule)*. (b) Two PRs: the lane and the pool rule, then the screen's word.

## 7. Stage and the test plan

**Stage 0 — the probe** (§3): the two live arms as `#[ignore]` smokes
beside `admission_control_e2e_live`, run on the CPU build without `-np`;
the outcome recorded in the journal as the go/no-go.

**Stage 1 — the lane** (`feat/silent-lane`). Unit, over the existing
fixtures:

- `session_budget.rs`: a silent permit is one; a second silent stream
  waits for the first; a silent reservation counts in `in_flight` and an
  interactive one waits behind it under a pool; an interactive stream that
  fits beside a silent one is admitted; a silent stream alone is admitted
  whatever its size; with no pool the silent lane is a plain semaphore of
  one and the interactive lane is unchanged bit for bit (the existing
  suite unchanged).
- `pool.rs`: managed at one session — the pool with four reported slots,
  with none reported, none with one reported; external unchanged; clouds
  none.
- `tests/reflection.rs` and a new `tests/silent.rs` over `KeyedRecorder`:
  a landing with every cadence at 1 opens the five requests **one at a
  time** (`max_in_flight == 1` on the silent keys) and all five complete;
  a silent loop's second round is priced with its first round's exact size
  (the recorder's `usage`); a roll beside a background run on a 4000-token
  pool takes its turn; a wake turn arriving over an open roll waits for it
  and completes (`open_at_arrival` of the wake key is 1); impersonation in
  shared mode takes the silent lane; a cancelled wait leaves no
  reservation and no strike (`bg_failures` unchanged).
- The tasks screen: `AppTask::waiting` words as "waiting for room", pinned
  on the rendered frame; the orchestrator's snapshot reads it off the
  budget.
- The order of `handle_done`'s fan-out (F6), pinned by the recorder's
  arrival order.

**Live**: the two probe arms re-run on the CPU build after the change —
arm 1 completes both the roll and the turn with the turn waiting
(`open_at_arrival` 1), arm 2 shows one silent stream at a time — and the
existing `admission_e2e_live` / `background_subagent_e2e_live` as the
regression on the LAN stack. A live run is mandatory: every path here is
an engine path (AGENTS.md §3).

## 8. Not in this track (recorded so they are not re-derived)

- **Preemption** (F3b), if §3's bound says the wait is too long on the
  27B: the same `acquire_silent`, plus the watermark-at-success change.
- **Cancelling a silent task from the tasks screen** — tasks-screen.md §8's
  item; a lane makes it cheaper (the waiting task has nothing to undo) but
  it is its own question.
- **Impersonation on its own engine under a budget of that engine's** —
  one stream at a time by its gate; no contention to guard.
- **The parked-set bound** (`--cache-ram`) — the silent lane adds at most
  one parked conversation, which the install note's advice already covers.
- **A retry that re-enters the budget** (admission §8) — unchanged.

## 9. Documentation touch list (AGENTS.md §4)

- spec §6.3 (the silent lane beside the interactive one), §6.7 (the roll
  reserves; the queue order), §9.3.2 (a run's neighbours now include the
  silent lane), §11.6 (the `sessions` hint: "the app's own background
  requests take one more stream at a time"), §11.8 (impersonation waits
  for room in shared mode), §11.10 (*waiting*), §17.6 (reflection under
  the lane).
- architecture §5 (`SessionBudget`'s two lanes, the seven callers), §9
  (the silent loops), §10 (the tasks screen's third state), the module
  map (`pool.rs`'s rule).
- install.md §3 (`sessions` at one: the server's four slots and what the
  app now does about them).
- locales `en`/`ru`: the `sessions` hint, the *waiting* word, the
  impersonation note.
- No ADR: the lane implements the item three tracks recorded and amends
  no contract between layers.
- journal: engine.md (the budget is the engine's) with the probe's
  numbers and the live runs; CHANGELOG `[Unreleased]` Fixed (the collision
  at the default) and Changed (one silent request at a time); roadmap (the
  item closes); parallel-subagents.md §6 F9, background-subagents.md §4.7,
  admission-by-budget.md §8 and tasks-screen.md §8 (each points here);
  CLAUDE.md's status line when the track ships.
