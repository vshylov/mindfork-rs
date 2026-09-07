# The silent stream yields to the turn — preemption on the session budget

> **Status:** implemented (2026-09-07) — stage 0 measured in §3.1, stage 1's
> live runs in §3.2; every fork at its recommendation (the user's decision,
> 2026-09-07).
> The track the silent-lane design deferred as its fork F3(b)
> ([silent-tasks-budget.md](silent-tasks-budget.md) §4.4, §8): the app's own
> request open on the pool is *cancelled* to make room for the user's turn,
> and re-run afterwards, rather than the turn waiting for that one silent
> round. The lane, the pool rule at one session, the order of the queue and
> the tasks screen's *waiting* are the ground this stands on and are not
> re-derived here.

## 1. Why, precisely

The silent lane (2026-09-07) made every request the app opens against the
chat engine count under the session budget: one permit for the app's own
requests over the same pool sum as the turns, so a title, a reflection round,
a compaction roll or the impersonation preview never overfills the KV pool
beside a turn or a run. Its interplay rule is the simplest one that is
correct (fork F3(a)): **a turn that does not fit beside an open silent round
waits for that one round**, bounded by the round's reply cap, and the
reverse never happens.

The bound is real, and its size is the engine's. On the LAN stack (Qwen 3.6
27B on a 4090, four slots over 16384) the whole guarded probe — a roll of
9k tokens beside a run of 9k — ran in 16.5 s, so a turn arriving behind a
roll waits a few seconds. On the CPU build launched as the managed launcher
launches it at one session (Gemma 3 4B, four slots over 2048) the same roll
takes most of a minute, and a turn typed while it streams sits behind it for
that long with nothing on the screen but the status bar's "compacting…". The
lane design measured the collision it prevents and recorded the wait it
introduces as the price, deferring the alternative until the price was
known: it is known now, and the user chose to pay for the turn instead —
this track.

What preemption buys is the user's *turn* — the one request whose latency
someone is watching — never waiting for the app's own work. What it costs is
a wasted round (the cancelled silent stream's generated tokens, and its
prefill unless the server kept the slot's cache), the bookkeeping the lane
design listed as R3's (a task cancelled to make room must run later, and
the watermark it advanced, the cadence it consumed, the title it owed must
not be spent for nothing), and one more way for two streams to chase each
other (a silent task re-admitted between a turn's rounds and cancelled again
at the next).

**Requirements.**

- **R1.** An interactive stream — the user's turn, a sub-agent's round, a
  dialogue's line, a background run's round, a turn's own summary — that
  does not fit beside an open silent stream starts as soon as the engine has
  freed the silent stream's room, not when that stream would have ended.
- **R2.** No silent task is lost to a preemption: the cancelled request is
  made again by the same task, with the same input, after the interactive
  stream that displaced it; the watermark, the cadence counter and the owed
  title stay exactly as the lane design left them — advanced at spawn and
  never refunded, because the task still completes.
- **R3.** Bounded: a silent task is displaced at most a few times; after
  that it holds its stream and the turn waits one round, as today. No
  starvation of a silent task by a long agentic turn, and no unbounded waste
  of engine time.
- **R4.** The user's own silent request — the impersonation preview — is
  never cancelled by the app's machinery: the user is watching it, and a
  background run's round can wait for it as today.
- **R5.** Nothing changes with no pool known (the clouds, one reported
  slot): preemption is a property of the pool's room, and without a pool no
  reservation exists to displace — the property the budget has now kept
  three times.
- **R6.** Observable: the log names the displaced task and the stream it
  yielded to; the tasks screen shows the task *waiting* again, off the
  budget, with no new state.
- **R7.** Measured before designed (lessons §3): the wait the turn pays
  today and the latency the engine needs to free a cancelled stream's room
  are both read off the CPU build through the app's own paths — the second
  is the floor any preemption can reach, and if it is not far below the
  first there is nothing to build.

## 2. What exists (inventory)

### 2.1 The lane, its wait, and the token every stream already carries

`SessionBudget::acquire_in` (`shared/session_budget.rs`) is one function
for both lanes: a permit from the lane's semaphore (cancellably), then under
a pool a loop over `room.notified()` until `need` fits beside `in_flight` or
nothing is open. The silent lane's holder is recorded in `silent_streaming`
as its label from the moment it is admitted until its `Reservation` drops —
what the tasks screen's *waiting* reads. The interactive waiter knows
nothing about *who* holds the room it waits for: `in_flight` is one sum.

Every `chat_stream` call already takes a `CancellationToken`, and every
client ends a cancelled stream the same way: `RetryBackend` yields
`Finished(Cancelled)` when the token fires (`retry.rs:330`, and again at
`:723` for the wait between attempts), the HTTP request is dropped with the
future, and the server sees a closed connection. So the mechanism a
preemption needs — "end this stream now, from outside" — exists on every
path; what does not exist is a second party holding a token to a silent
stream, and a way for the stream's owner to tell *its own* cancellation
from someone else's.

### 2.2 Who holds the silent lane, and what a cancel does to each today

| holder | where | on `Finished(Cancelled)` today | its timeout |
|---|---|---|---|
| the title | `title.rs::spawn_title` | the collected text so far → `salvage_title_source` → most likely `ui.err.title_empty`; **not retried** (`maybe_auto_title` fired once) | 60 s, over the stream only (the wait sits before it) |
| the compaction roll | `compaction.rs::spawn_compact` | the fragment so far reported **as the summary** — `Finished(reason)` only tracks `Length`; the roll would fold the history under a truncated summary | 180 s, over the stream only |
| reflection · notes · self-model | `tool_loop.rs::run_rounds` | `read_round` returns the reason, `run_rounds` breaks on `!= ToolCalls` and lands `Ok(())` — the task "succeeded", the watermark (advanced at spawn) stays, `SelfModelChanged` is announced for a window never read | 120 / 180 / 180 s, over the **whole task including the wait** for the lane and for room |
| impersonation (shared) | `impersonation.rs::spawn_impersonation` | `Ok(Cancelled)` → the text is **discarded** (the `Esc` path) | 120 s, whole task |
| a `fetch_url` summary in a silent loop | `fetch.rs` through `ToolContext.silent_lane` | the tool's result is the error; the loop's round goes on with it | the tool's own |

Three of the five would today mistake a preemption for a completion: the
roll would store a fragment as the summary (the worst of them — a lossy
fold), the loops would land `Ok` and the title would give up. So a
preemption cannot be "cancel the token": each holder must be told it was
displaced and act on it. That is the retry of §4.4, and the row for the
roll is why §2.5's "carry the reason out" pattern matters.

### 2.3 The bookkeeping the lane design left at spawn (R2)

Reflection advances `chat.reflected_upto` at spawn (`reflection.rs:228`,
"shifts only on an actual spawn"); both consolidations reset their reply
counters at spawn (`consolidation.rs:113`, `self_consolidation.rs:147`);
the title fires once per point (`maybe_auto_title`); the roll's plan is
cut once (`plan_roll`) and its slot (`bg_running(Compaction)`) is taken
until the result lands. The lane design's fork F7 left all of this alone
because the wait cannot lose a run. A preemption *can* — if the displaced
task gives up. It does not have to: as long as the same task re-makes the
same request and completes, every spawn-time advance is still honest. The
retry inside the task (§4.4) is what keeps R2 without touching one of
these — the alternative, landing the task as *preempted* and refunding
each advance in the orchestrator, is fork F4(b) and four more places to
get wrong.

### 2.4 What the server does with a cancelled stream's room

The guarded arm of the lane probe is the fact that matters: the roll
streamed *after* the run's stream ended while the run's slot still held its
9k tokens of parked cache, and both completed — so a **parked** slot's
cells do not count against a new prompt; llama.cpp evicts an idle slot's
cache when a prompt needs the space (unified KV: the cells are one pool,
the slot's cache a hint for prefix reuse). A cancelled stream's slot is idle
the moment the server notices the closed connection, which it does at the
next token it tries to send — one token's time. So the room a preemption
frees is available within about one decode step; what the arm of §3
measures is whether the *whole* path (the client's abort, the server's
notice, the new prompt's admission) is as short in practice.

The retry's cost has the same source: the displaced silent request is
re-sent byte for byte, and if its slot's cache survived (four slots, at most
two conversations in play at the default), the prefill is a prefix-cache
hit — the wasted round is the generated tokens only. Stage 1 reads the
retry's time to first token off the same probe (§7).

### 2.5 How each holder reads a `Finished(Cancelled)` it did not ask for

`spawn_title` and `spawn_compact` own a `CancellationToken` they create
themselves and never share (`CancellationToken::new()` inside the task) —
nothing can cancel them but their own timeout. `run_rounds` gets the
kind's token from the slot registry (`begin_bg`), cancelled only by `Quit`
(`cancel_all_bg`). `spawn_impersonation`'s token is the orchestrator's
`imp_cancel` — `Esc`. So today `Finished(Cancelled)` on a silent path
means "the app is quitting" (or "the user pressed `Esc`" for
impersonation), and each holder rightly treats it as final. A second source
of cancellation must be *distinguishable*: the reservation hands the holder
a **child** token of its own (`CancellationToken::child_token`), the
preemption cancels the child, and "the child fired while the parent did
not" reads as *displaced* — §4.1.

### 2.6 What the tests can already say

`session_budget.rs` pins both lanes (the silent lane one wide, the lanes
sharing the pool both ways, a cancelled waiter leaving nothing).
`tests/silent.rs` drives the orchestrator's silent tasks over
`KeyedRecorder` with `open_at_arrival`/`max_in_flight`, and
`the_roll_waits_for_the_run_and_the_wake_turn_waits_for_the_roll` is
exactly the shape preemption inverts: today its assertion is that the wake
turn's stream opened *after* the roll's ended. `KeyedRecorder` ignores the
cancellation token except in `hang` mode — a scripted stream with a delay
runs to its end whatever the token says — so a preemption test needs the
recorder to end a delayed script with `Finished(Cancelled)` when its token
fires, and to count the arrivals of the same key (the retry is a second
arrival). `tests/live.rs` has the hybrid `ScriptedParent` (a scripted
parent, live children and rolls) and `delegator_chat`.

## 3. Measurements to make (before designing further)

**Go/no-go probe (stage 0).** Two arms on the CPU build, launched as the
managed launcher launches it at one session (no `-np`, `-c 2048`: four
unified slots), through the app's own paths — `preemption_smoke` in
`tests/live.rs`, a chat seeded with 55 % of the pool as archive by a
scripted turn:

1. **The wait today** (`preemption_wait_e2e_live`): `/compact` opens the
   roll on the live server (its digest 55 % of the pool plus its cap), and
   half a second later a one-word user turn is sent — its prompt is the
   same 55 % plus its cap, so the two do not fit together and the turn
   waits (the lane's rule). Recorded: the roll's duration, and the turn's
   time from *sent* to the roll's end (its wait) and to its first token.
2. **The floor** (`preemption_floor_e2e_live`): a live turn asked for a
   long story is cancelled by the app (`AppCommand::Cancel`) the moment its
   first token arrives; a one-word turn is sent the instant it reports
   `Finished`. Recorded: the cancel's own latency (cancel → `Finished`) and
   the second turn's time to first token — the whole path a preemption
   would take, minus nothing.

**GO** if arm 1's wait is at least ten seconds and arm 2's floor is under
a few seconds — the turn would start an order of magnitude sooner than it
does. **NO-GO** if the server holds a cancelled stream's room for long (the
floor near the wait), or if arm 1 shows the turn not waiting at all on the
launcher's line; then the design below is not built and the roadmap keeps
the item as "the wait is the price". The LAN stack runs the same two arms
for the GPU numbers, which decide nothing (the user's decision already
weighed them) but go in the table.

### 3.1 Stage 0 — measured (2026-09-07)

**GO.** Both arms are `preemption_smoke` in `tests/live.rs` over the
hybrid `ScriptedParent` with one addition — a request with no script left
goes to the live server (`live_after_scripts`), so the seed is scripted and
the measured turn is live. The CPU build (`llama-server` b10807, Gemma 3 4B
Q8_0) launched with the managed launcher's own line at one session (no
`-np`, `-c 2048`: `n_slots = 4, kv_unified = true`); the LAN stack (Qwen
3.6 27B Q4_K_M on a 4090, `-np 4 --kv-unified -c 16384`) for the GPU row.
The seed is two scripted exchanges — the archive (55 % of the pool) and a
four-paragraph tail longer than `tail_tokens`, since `plan_cut` folds the
exchanges ahead of the tail and a one-exchange chat has nothing to fold
(the first run refused with `ui.compact.nothing`).

| stack | the roll | the turn's wait (sent → the roll's end) | the turn's first token | the floor (cancel → next turn's first token) |
|---|---:|---:|---:|---:|
| CPU, 2048 | 46.4 s | **45.9 s** | 85.2 s | **0.79 s** |
| LAN, 16384 | 6.1 s | **5.6 s** | 12.1 s | **0.35 s** |

**Arm 1 — the wait today.** The roll's stream opened first (slot 1, LRU,
1491 tokens), the turn's request half a second behind it, and the turn
waited for the roll's end: 45.9 s on the CPU build, 5.6 s on the GPU. The
rest of the turn's time to first token is its own cold prefill (slot 0,
LRU, 1401 tokens, 39 s on the CPU) — the seed being scripted, the server
had never seen the chat's prefix; a real turn's prefix was prefilled by the
previous turn and hits the cache. So the wait is the whole of what
preemption can remove, and the prefill is not its business. One stream at
a time on the client (`most open` 1), as the lane promises.

**Arm 2 — the floor.** The cancel is honoured on both sides at once: the
client's `Finished(Cancelled)` 1 ms after the cancel, and on the server
`stop: cancel task` **1 ms** after the connection closed, the slot's
`release` **110 ms** after that. The first run sent the next turn in that
gap and paid for it: the server had not yet noticed the closed connection,
picked another slot by LRU (the busy one was the cancelled one) and
prefilled the whole prompt cold — 36 s on the CPU build for the same
prefix the cancelled slot still held. Sent 300 ms after `Finished`, the
next turn lands on the released slot by prefix similarity (`f_sim_best`
0.985) and its first token arrives in **0.79 s** — the path a preemption's
waiter takes, since it is admitted only after the displaced reservation
drops, which is after the client saw the stream end. The gap is therefore
a design fact, not a probe artefact: the waiter must not race the server's
release for the slot's cache (§5, and the 300 ms is not the answer — the
warm prefix is a bonus; the room is what the waiter needs, and the room is
free at the release).

Against the criterion: the wait is 45.9 s where the floor is 0.79 s on
the CPU build — sixty times — and 5.6 s against 0.35 s on the GPU. The
design below is built. What the probe also settled: the server frees a
cancelled stream's compute and its room within a decode step, so
§2.4's reading of the parked cache holds, and a displaced roll's retry
lands on a warm slot as long as no third conversation evicted it.

### 3.2 Stage 1 — the same arms after the change (2026-09-07)

**The turn no longer waits for the roll's round — it waits for the
server's current batch.** The same two arms, the CPU build and the LAN stack,
after the change; the arm gained the assertion that the turn's first token
comes before the roll's end.

| stack | the roll, both attempts | the turn's first token | before (§3.1) | the floor |
|---|---:|---:|---:|---:|
| CPU, 2048 | 112.3 s (cancelled 1.1 s in, made again after the turn) | **62.9 s** | 85.2 s | 0.83 s |
| LAN, 16384 | 14.3 s | **7.9 s** | 12.1 s | 0.37 s |

The turn streamed before the roll's end on both, the roll landed a summary
from its second attempt, and the LAN regression — `silent_roll_e2e_live`,
`admission_e2e_live`, `background_subagent_e2e_live` — was green with the
two arms, 5/5 in 80 s. But the turn's first token is not at the floor, and
the server's log says why, in two parts §2.4's reading did not foresee:

- **A displacement takes effect at the end of the server's current batch.**
  The floor arm cancelled a stream that was *decoding* — one token per
  step, the cancel seen within 1 ms. The roll was cancelled 1.1 s into its
  **prefill**, and llama.cpp's loop honours a cancel between batches: the
  roll's slot was released only when its prefill batch had run to the end
  (`n_tokens = 888`, 23 s later on the CPU build), and the turn's request,
  queued behind that batch, launched at the same instant. So the turn now
  waits the roll's prefill batch rather than its prefill and its whole
  reply: on the CPU build about 24 s of its 62.9 s (the rest is its own
  cold prefill, 39 s, the scripted seed's artefact of §3.1), against the
  lane's 45.9 s; on the GPU about 1.4 s (7.9 s less the same ~6.5 s
  prefill) against 5.6 s.
- **The retry prefilled cold.** The roll's second attempt was placed by
  LRU on a never-used slot, not on the cancelled one whose cache still held
  its prefix (the floor arm's next turn *did* land on the cancelled slot by
  prefix similarity — a decoding stream's cache survives, a stream cancelled
  mid-prefill apparently does not): 49 s on the CPU build, the roll's total
  112 s against 46 s; on the LAN 14.3 s against 6.1 s. And while the turn
  prefilled beside the cancelled slot's parked tokens the server logged
  `failed to find free space in the KV cache, retrying with smaller batch
  size`: a parked cache is evicted under pressure, at the price of retried
  batches — the pool's room is free at the release, as §2.4 said, but not
  for free.

What the design buys is therefore what the server's batch leaves: on the
GPU stack the turn's wait fell from 5.6 s to under two, on the CPU build
from 46 s to 24 s, and the roll pays its prefill twice. Both are the
server's granularity — a smaller micro-batch (`-ub`) would shorten the
batch a cancel waits for, and that is a launch-line decision recorded in
§8, not this track's.

## 4. Design

### 4.1 The yield token

`acquire_silent` gains one input, whether the stream **yields**
(`SilentRequest { label, yields }` — or a second constructor; the shape is
fork F2's), and its `Reservation` gains two things: a **stream token**, a
child of the holder's own token that the holder passes to `chat_stream`
instead of its own, and a **`displaced()`** reading — the child fired while
the parent did not. The budget keeps, beside the streaming label, the
child token and the reservation's size for the one silent stream open
(`silent_open: Mutex<Option<SilentOpen { label, token, tokens, yields }>>`,
cleared by the reservation's drop as the label is today).

Nothing changes for a holder that ignores the new reading: the child token
cancels with the parent, so `Quit` and the timeouts end the stream as
before.

### 4.2 Who displaces, and when it helps

Only an **interactive** waiter (`acquire`, `label == None`) ever displaces,
and only from inside the wait loop it already runs: when `need` does not fit
beside `in_flight`, the silent stream open is one that yields, its token has
not been cancelled yet, and **removing its reservation would let this
waiter in** — `in_flight - silent.tokens + need <= pool`, or the silent
stream is the only thing open — the waiter cancels the child token, logs
one `info` line (`lane`, the displaced label, `need`, `in_flight`, `pool`),
and goes on waiting for `room` exactly as today; the silent holder's
reservation drops when its stream ends, the notify wakes the waiter, and
it is admitted. The check that it *helps* is what keeps a preemption from
being a wasted round for nothing: above one session two interactive
streams can be open beside a silent one, and if the waiter would still not
fit without the silent one, displacing it changes nothing.

A silent waiter never displaces anything (R2 of the lane design: the silent
tasks yield, never the reverse), and without a pool there is no
reservation, no room and no displacement (R5).

**Priority for the one who displaced.** The displaced task re-acquires the
lane at once (§4.4), and its reservation is small enough to fit into the
room it just freed before the interactive waiter — which was woken by the
same notify — takes it: a second displacement, a second wasted round. So
the budget counts **displacing waiters** (`displacing: AtomicUsize`,
incremented when a waiter cancels a silent token, decremented when that
waiter is admitted or cancelled), and `acquire_silent`'s wait loop treats
"a displacing waiter is pending" as "no room" — the silent task takes the
lane's permit (it is the only silent request, and the queue order holds)
but does not take the room until the interactive stream that displaced it
is in. The interactive waiter cannot starve the silent task this way: it is
admitted at the very next notify, or cancelled.

### 4.3 Who yields

The **title**, the **compaction roll** and the three loops' rounds
(reflection, notes consolidation, self-model consolidation) yield: each is
the app's own work, re-runnable from the same input, and nobody is
watching it stream. **Impersonation** does not yield (R4): it is the user's
request in the lane only because it shares the engine, its preview is on
the screen, and the one interactive stream that can arrive during it — a
background run's round — waits for it as today (the user's turn cannot: the
input box is busy). A **`fetch_url` summary inside a silent loop** does not
yield either: it is one tool call inside a round whose reservation was
already dropped, its stream is short, and a displaced summary would hand
the loop a failed tool result rather than a retry — the loop's *next*
round, not the tool, is the unit that yields.

### 4.4 The retry lives in the task

The displaced holder makes the same request again: `spawn_title`,
`spawn_compact` and `run_rounds` each wrap their acquire-and-stream in a
loop — *acquire the lane (yielding), stream on the reservation's token, and
if the stream ended `Cancelled` while the reservation reads `displaced()`,
go round again* — with the same `ChatRequest` (a loop's messages are pushed
only after a round completes, so a displaced round re-sends what it sent).
The task's slot stays taken, its watermark and counters stay where spawn
put them (R2 by construction — §2.3), the tasks screen shows it *waiting*
again off the budget, and `handle_bg_done` sees one outcome at the end as
today. The round trip is inside the `tokio::spawn` the task already runs
in; nothing about the orchestrator's handlers or the result channels moves.

**The cap (R3).** A task yields at most **three** times
(`SILENT_YIELDS_MAX`, one constant in `session_budget.rs`); its fourth
acquire is made with `yields: false` and holds — the turn then waits one
round, the lane's rule. Three is the number that bounds the waste to a few
rounds and still covers the common case (one turn, one displacement, or a
short agentic turn whose two or three rounds each find the same silent
task re-admitted in the gap between them). A long agentic turn — 32 rounds
on the user's own setting — meets a held silent round at most once per
silent task.

### 4.5 The loops' clock stops while they wait

`spawn_silent_loop` wraps the whole `run_rounds` in the task's timeout,
waits included (§2.2's last column) — already a latent defect under the
lane (a reflection queued behind a long roll counts the queue against its
120 s), and under preemption a displaced round waits for a whole
interactive stream. The title's and the roll's timeouts start after their
wait; the loops' should too: `run_rounds` takes the deadline and applies
it round by round to the stream-and-tools part, the waits outside it, as a
time *budget* consumed only while streaming. The task's outcome and
`handle_bg_done` are unchanged.

### 4.6 What the user sees

Nothing new on the screen: a displaced task's label leaves
`silent_streaming` when its reservation drops and the row reads *waiting*
again on the next tick (the runtime already re-asks for the rows while a
silent task runs, spec §11.10). The log says what happened — the `info`
line at the displacement, and the existing "stream waits for room" line
from the displaced task's re-acquire. The status bar's indicators are per
kind and stay lit through the retry, as through a wait.

### 4.7 What does not change

The lane, its width, the queue's order and the pool rule at one session;
the interactive lane's pricing, floor and calibration; the "alone is
admitted" clause; the launch line; impersonation's gate and its `Esc`; the
cadences, the strike counter and the one-at-a-time gates (a retry is
inside one run of a task); the clouds and the one-slot server (R5).

## 5. Difficult spots

- **A displacement that arrives before the request is sent.** The silent
  holder is admitted, its label recorded, and the interactive waiter
  cancels the child token before `chat_stream` has been called: the client
  ends the stream at once with `Finished(Cancelled)` (the retry decorator
  checks the token before its first attempt), the holder reads
  `displaced()` and goes round — one empty request, no server work. Fine,
  and the test for it is the cheapest of §7's.
- **The gap between the client's end and the server's release** (§3.1's
  arm 2). The displaced holder's reservation drops when its stream yields
  `Finished(Cancelled)` — on the client, 1 ms after the token — and the
  waiter's request can reach the server before the server has released the
  displaced slot (its `cancel task` follows the closed connection by ~1 ms,
  the `release` by ~110 ms). Nothing is lost by it: the waiter needs the
  *room*, the room is freed at the release, and a prompt's prefill fills
  the pool batch by batch over seconds, not milliseconds. What the gap can
  cost is the slot's *cache*: a request that arrives before the release is
  placed by LRU on another slot and prefills cold where the released slot
  would have matched its prefix. The turn's own prefix lives on the slot the
  previous turn used, not on the displaced one, so for the turn it is moot;
  for the displaced task's retry (the same prompt, the same slot) the retry
  is made only after its own stream ended and it waited for the turn — long
  after the release. No delay is added anywhere for this.
- **The interactive waiter cancelled while it displaces.** It cancelled the
  silent token and then its own turn was cancelled (`Esc`): the silent
  stream was wasted, the displacing count must still be decremented (the
  guard's drop, not the success path), and the silent task's retry proceeds
  into the room now free. The counter lives on the waiter's stack with a
  drop guard.
- **Two interactive waiters at once** (above one session): both may find the
  same silent stream worth displacing; the token is cancelled once (the
  second sees it cancelled and only counts itself as displacing), both are
  admitted in the notify's order as today.
- **The roll's fragment.** `spawn_compact` today treats any `Finished` but
  `Length` as a complete summary (§2.2); with the reservation's token in
  play a `Cancelled` there is either a displacement (retry) or `Quit` (the
  timeout's wording, as the cancelled wait already reports). The
  distinction is one `if`, and the wording defect it closes — a fragment
  folded as a summary — exists today only for a stream the app's own quit
  ends, which nobody sees; it becomes reachable and must be right.
- **The title's retry against a chat that was renamed meanwhile.**
  `handle_title_result` already refuses a title for a chat the user renamed
  during the generation (the `origin` check); a retry lengthens the window
  and changes nothing about the rule.
- **A held silent round after the cap** (§4.4) is the lane's rule again:
  the turn waits, bounded by the cap. The log line for the wait names the
  lane and the label as today, so a wait that surprises has its reason in
  the log.
- **The memo's key** (lane §5): a rebuilt budget mid-round has two budgets
  overlapping for one round; a displacement across them is impossible (the
  waiter reads its own budget's `silent_open`), so the worst case is one
  un-displaced round, the status quo.

## 6. Forks

The user's decision (2026-09-07): every fork at its recommendation.

- **F1. Who displaces.** (a) **Every interactive waiter** — the lane
  decides: the turn's rounds, a sub-agent's, a dialogue's line, a
  background run's round, a turn's summary *(recommended — one rule in one
  function; a background run is the user's work on the interactive lane,
  and it is the very stream the lane probe's roll collided with)*. (b) The
  user's own turn and the wake turn only; background runs wait as today —
  a flag on `acquire` and a second rule to explain.
- **F2. Who yields.** (a) **The title, the roll and the loops' rounds;
  impersonation and the in-loop summary hold** *(recommended — §4.3, R4)*.
  (b) Every silent stream, impersonation included. (c) The roll and the
  loops only — the title is short and rarely in the way.
- **F3. When a displacement is made.** (a) **Only when it lets the waiter
  in** *(recommended — §4.2; never a wasted round for nothing)*. (b)
  Whenever the waiter does not fit — simpler, and above one session it
  cancels silent rounds that were not the problem.
- **F4. Where the retry lives.** (a) **Inside the task** — the same spawn
  re-acquires and re-sends; the spawn-time bookkeeping stays *(recommended
  — §4.4, §2.3)*. (b) The task lands *displaced* and the orchestrator
  refunds the watermark, the counters and the owed title, re-asking at the
  next landing — R2 through four handlers. (c) No retry — rejected, R2.
- **F5. The cap.** (a) **Three yields per task, then it holds**
  *(recommended — §4.4)*. (b) One — a second displacement makes the turn
  wait. (c) Unbounded — a long agentic turn can starve a silent task and
  waste a round per turn round.
- **F6. The loops' clock.** (a) **The timeout counts streaming and tools
  only; the waits are outside it** *(recommended — §4.5, and it closes the
  latent defect under the lane)*. (b) Unchanged: a displaced reflection
  can time out in its wait.
- **F7. What the screen says.** (a) **Nothing new — *waiting* again, off
  the budget** *(recommended — R6, the projection reads what is true)*.
  (b) A fourth word (*yielded*, or *waiting ·2*) — one more field on
  `AppTask` for a state that lasts a round.
- **F8. Staging.** (a) **The stage-0 probe with a go/no-go, then one PR**
  *(recommended — one type's change, five holders, one recorder change)*.
  (b) Two PRs: the budget and the holders, then the loops' clock.

## 7. Stage and the test plan

Stage 0 is §3's probe, on this branch with the design. Stage 1, one PR:

- `session_budget.rs`: a yielding silent reservation is displaced by an
  interactive waiter that would then fit, and not by one that would not;
  a non-yielding one never is; a silent waiter displaces nothing; the
  displaced holder's `displaced()` is true and its parent token untouched;
  a displacing waiter is admitted before the displaced task's retry takes
  the room; the waiter's cancellation while displacing leaves the counter
  at zero; no pool — no displacement.
- `tests/silent.rs` over a `KeyedRecorder` that ends a delayed script with
  `Finished(Cancelled)` when its token fires: the wake turn arriving behind
  the roll opens **at once** (`open_at_arrival` 0 for the turn, the roll's
  key arriving **twice**, its second request equal to its first); the same
  for a reflection round; the title displaced and re-made; impersonation
  not displaced by a background run's round (the run waits, as today); the
  fourth arrival of a task holds (the turn waits); a displaced roll never
  lands a fragment as its summary.
- `tool_loop.rs`: the clock — a wait longer than the timeout does not end
  the task, a stream longer does.
- Live, mandatory: §3's two arms on the CPU build after the change — the
  turn's time to first token in arm 1 at the floor of arm 2, the roll
  completing after the turn with its retry's time to first token recorded
  (the prefix-cache question of §2.4) — and the LAN regression
  (`silent_roll_e2e_live`, `admission_e2e_live`,
  `background_subagent_e2e_live`).

## 8. Not in this track (recorded so they are not re-derived)

- **Cancelling a silent task from the tasks screen** — **done** as its
  own track ([stop-silent-task.md](stop-silent-task.md), 2026-09-07): the
  slot's token, a third outcome, and the holders' readings this track
  taught them.
- **A smarter roll** that re-plans against the conversation as it stands at
  the retry — the plan is cut once; a retry folds what the first attempt
  would have.
- **Preempting a background run for the user's turn** — both are on the
  interactive lane; the run is the user's work, and at one session they
  already take turns round by round.
- **Impersonation on its own engine** — its own pool, no contention.
- **A smaller batch on the launch line** for the CPU build — **done** as
  its own track ([cpu-batch.md](cpu-batch.md), 2026-09-07): the knob is
  `-b`, not `-ub` (measured on five lines), and `-ngl 0` now launches with
  `-b 256 -ub 256` — the batch a cancel waits for 6.5 s instead of 23.

## 9. Documentation touch list (AGENTS.md §4)

- spec §6.3 (the silent lane yields to an interactive stream that would
  then fit, at most three times per task, the retry inside the task),
  §6.7 (the roll re-made after a displacement, never a fragment), §9.3.2
  (a run's round displaces a silent stream like a turn's), §11.8
  (impersonation holds), §11.10 (*waiting* again after a displacement),
  §17.6 (a reflection round re-made; its timeout over the stream only).
- architecture §5 (`SessionBudget`: the yield token, the displacing count,
  the cap), §8/§9 (the holders' retry loops, the loops' clock), §10
  (nothing new on the screen — stated).
- No ADR: the mechanism lives inside the budget and amends no contract
  between layers.
- journal: engine.md with the probe's numbers, the live runs before and
  after, and the retry's prefill; CHANGELOG `[Unreleased]` Changed (a turn
  no longer waits for the app's own request; the loops' timeout no longer
  counts their wait); roadmap (the item closes);
  silent-tasks-budget.md §8 and tasks-screen.md §8 point here; CLAUDE.md's
  status line when the track ships; lessons §3 if the probe teaches one.
