# Admission by budget — the unified KV pool never overfilled by the app — research

> Status: **built** (2026-09-04, `feat/admission-by-budget`). User's
> decision, same day: **F1–F8 at their recommended options**. What was
> built, tested and run live is the journal entry
> ([docs/journal/tools.md](../journal/tools.md), *admission by budget*);
> §7's live smoke plants the text in the children's messages rather than in
> files, since the CPU build's Gemma 3 template drops tools, and sizes its
> arms from the pool `/props` reports, so the same three arms ran GO on the
> CPU build (2048) and on the LAN stack (Qwen 3.6 27B, `-np 4 --kv-unified
> -c 16384`: two 9k children take turns; the control arm loses one child;
> four quarter-pool children stream exactly two at a time). The item
> [parallel-subagents.md](parallel-subagents.md) §8 recorded as fork F7 —
> *"admission by budget: the app-side guard for the unified pool"* — and
> that track's §4.7 named as the *later* of its two guards. This document
> plans it: a turn's request streams may open at once only while what they
> will occupy of the server's one KV pool fits it together; a stream that
> would not fit **waits** for a session instead of taking one, and the
> failure §3 reproduces — the server ending *every* running conversation
> at once — becomes unreachable from the app's side. Measured this session
> on the local CPU build (`llama-server` b10807, Gemma 3 4B Q8_0, a pool of
> 2048 tokens over two unified slots), which is enough to observe *server*
> behaviour: the numbers are in §3. The parent track's stage-2 loop (the
> parallel group, the session semaphore, `TurnShared`) is the code this
> changes, and the sibling track [concurrent-tools.md](concurrent-tools.md)
> §4.5 (a tool's own request under the same semaphore) is the second
> caller it covers.
>
> The ask (2026-09-04): F7 first, out of the parent track's §8 — it is
> self-contained, it is measurable locally, and it closes a real failure
> that `sessions > 1` makes reachable.

## 1. Requirements

R1. **No collective failure from the app's own streams.** Under a unified
    pool, the streams the app keeps open at once never together exceed the
    pool — the server's "Context size has been exceeded." to every
    processing slot (§2.2, reproduced in §3.2–§3.3) is not provoked by
    anything the turn does.
R2. **Waiting, not failing.** A stream that would not fit next to the ones
    already open waits for room, the way it already waits for a session;
    it is cancellable (`Esc`), and the run's own timeout bounds it.
R3. **Never a deadlock, never a new refusal.** A stream that is alone is
    admitted whatever its size — the server, not the app, decides whether
    a single conversation fits its window (that is the path every chat has
    today, with the compaction trigger and the typed context error
    upstream of it).
R4. **Defaults reproduce today's behaviour bit for bit.** At `sessions =
    1` no two streams of a turn are ever open at once, so the guard can
    never wait; on the clouds there is no pool and no guard. No new
    settings field is required for the guard to work; any new field is
    `#[serde(default)]`.
R5. **The pool is what the app already knows.** A managed server's `-c`
    is the pool the app launched; an external llama.cpp's `n_ctx` on
    `/props` is what `context_budget` already reads for the compaction
    trigger. Nothing new is asked of the server.
R6. **The estimate errs on the side of waiting.** A reservation is a
    prediction; where the prediction is wrong it must be *too large* (a
    stream waits that could have run) rather than too small (the failure
    R1 forbids). §3.6 measures how wrong the app's estimator can be.
R7. **Nothing changes for a chat that never opens two streams** — on the
    wire, on disk, or in what the model is told.

## 2. What exists today (inventory)

### 2.1 The session semaphore, and what it does not know

Stage 1 of the parent track put a `tokio::sync::Semaphore` on
`TurnShared`, sized from the active engine section's `sessions`
(`config.engine.active_sessions()`, never below 1); `TurnLoop::stream`
takes a permit for the duration of `stream_round` and for nothing else
(`acquire_session`, cancellable), and `fetch_url`'s page summary takes a
permit of the same semaphore through `ToolContext::sessions`
(concurrent-tools §4.5). A permit is a *count*: it says how many streams
are open, not how much of the pool they occupy. Two permits and a 16k
pool admit two 9k conversations, and §2.2 is what the server then does.

The loop already holds the other half of the information: every round
records the server's exact `usage.prompt_tokens` (`TurnLoop::last_usage`,
recorded at the one place both call sites go through, so the round-limit
branch cannot forget it), `estimate_prompt_tokens(&request)` prices a
request before it is sent (the live indicator's number until the exact one
arrives), and the reply of every child and every summary is capped —
`subagent_max_tokens` min'ed with the effective `max_tokens`
(`SubagentLimits`), `SUMMARY_MAX_TOKENS` for the summary. Only the main
turn's own stream may carry no cap (`SamplingConfig::max_tokens` is an
`Option`), and it is the one stream that never overlaps another: the loop
streams, then runs its tools, then streams again.

### 2.2 The server's failure, read and then reproduced

`server-context.cpp` (b10807), the `decode` retry branch: when
`llama_decode` cannot find room for a batch, the server first purges one
*idle* slot with tokens still in it (`try_clear_idle_slots`, unified only —
the slot's state was saved to the RAM prompt cache when it went idle,
`[TAG_IDLE_SLOT_CLEAR]`), then halves the batch; at `n_batch == 1` it
answers **"Context size has been exceeded."** to *every* slot that is
processing, clears their prompts (`slot.prompt_clear()` — "it's complicated
to keep track of how much of the current batch has been processed"), and
throws out of `update_slots`. The comment above it is a `TODO: try to
terminate only the largest active slot/sequence and continue with the
rest` — the per-sequence failure does not exist upstream, so the guard is
the app's to build.

§3.2 and §3.3 reproduce both routes into that branch: a prompt that does
not fit next to a running one (the batch offset in the log is the new
prompt's), and two prompts that fit together but whose *replies* grow into
each other — the second is the insidious one, since both streams have been
delivering text for a minute when they die.

### 2.3 What the app does with that error today

On the wire it is an in-stream error object with `code: 500` and `type:
server_error` (§3.2) — not the typed `exceed_context_size_error` the
server sends for a single oversized request, which is a `400` before any
stream opens. What was *supposed* to happen, by the code's own comments:
`wire::parse_stream_error` reads it; `stream_error_transient` says
**transient** (500 is in `RETRYABLE_STATUSES`), so the `RetryBackend`
decorator retries it while the stream is uncommitted — no text and no tool
call delivered yet — up to `MAX_ATTEMPTS = 3` with a 1 s base delay
([cloud-retry-backoff.md](cloud-retry-backoff.md)); a committed stream
lands the child `Failed` with the server's message, the parent is told
through `tool.call_subagent.result.failed`, and the turn goes on.

**What actually happened, measured by §7's control arm (2026-09-04):
neither.** The client parsed each `data:` payload as a
`ChatCompletionChunk` first and asked `parse_stream_error` only when that
parse *failed* — and it never fails on `{"error":{…}}`, because every field
of the chunk has a `#[serde(default)]`: the envelope deserialized as a
chunk with no choices and no usage and was skipped in silence, the
connection then closed, and the stream ended "without a terminator", which
the client reads as a finished turn. Both children of the colliding round
landed **`Completed`** — one with a reply cut mid-word (`“KELVAR`), one with
an empty reply — and the parent read them as complete. No retry fired, no
`Failed` was recorded, nothing on screen said why. The unit tests of
`parse_stream_error` in `wire.rs` are green because they test the parser
alone, never the client's order of asking. **Fixed in this track**: the
client asks for the envelope *before* the chunk parse, with a
`sse_server` test of a text delta followed by llama.cpp's error object
(`Text`, `Error { transient: true }`, `Finished(Error)`, nothing after),
and the control arm re-run lands the collision as `Failed` — which is the
only reason the arm can assert anything.

The parent's own context survives either way — §3.4 measures it — because
under the unified shape a suspended parent is parked in the RAM prompt
cache, not in the pool.

### 2.4 What `/props` says, and what it does not

`default_generation_settings.n_ctx` is `n_ctx_slot()` (`server-context.cpp`):
`llama_n_ctx_seq`, capped by `--kv-unified-per-slot` and by the model's
training context. Under the unified shape `n_ctx_seq` is the whole pool;
under the split shape it is the pool divided by `-np`. `total_slots` is
`n_parallel`. **Nothing on `/props` says which shape the server is in** —
the field list (`get_res_props`) carries no `kv_unified`. So a server
reporting `n_ctx 2048, total_slots 2` is either a unified pool of 2048
shared by two slots or a split `-c 4096` with 2048 for each, and the app
cannot tell from the outside. It *can* tell for the server it launched:
`managed::build_args` writes `-np N --kv-unified` above one session and
nothing at one, so a managed server above one session is unified by
construction, and `managed.context_size` is its pool. F4 is about the
external mode.

### 2.5 The `--kv-unified-per-slot` cap (llama.cpp PR #24124, 2026-08-27)

A per-slot ceiling under the unified pool: `n_ctx_slot()` is min'ed with
it, an oversized request is refused per request (§3.5), and without `-c`
the pool is sized to `n_parallel × N`. What it is not: a guard against the
collective failure at any cap above `pool / n_parallel` — two slots capped
at 1500 over a pool of 2048 can still outgrow it together, and the
`decode` branch of §2.2 is what happens then. At `pool / n_parallel` it is
the split shape's economics (the main chat's window divided by N — fork
F4b of the parent track, rejected for the same reason) with the unified
shape's memory. §3.5 measures the arm where it is exactly that.

### 2.6 The estimator

`shared::tokens::estimate_text` is "four UTF-8 bytes per token", plus four
tokens of template overhead per message — an order-of-magnitude figure for
the live indicator, replaced by the exact `usage` when it arrives. It was
never asked to bound anything. §3.6 measures it against the exact count on
the probe's prompts.

## 3. Measurements (2026-09-04)

The local CPU build: `D:\LLM\llama.cpp\build\bin\Release\llama-server.exe`
b10807 (commit `163a40796`), `gemma-3-4b-it-q8_0.gguf`, launched exactly
as the managed launcher would at `sessions = 2` with `-c 2048`:

```
llama-server -m gemma-3-4b-it-q8_0.gguf -c 2048 -np 2 --kv-unified --jinja --slots -t 12
```

`/props`: `total_slots 2`, `n_ctx 2048` — the whole pool for each slot;
the log: `n_slots = 2, n_ctx_slot = 2048, kv_unified = 'true'`. The probe
(`tools/kv_pool_probe.py`) sends two
conversations A and B with distinct filler prompts (no shared prefix, so
the cache cannot help), `max_tokens 400`, `temperature 0`, in the plain
and the streaming shape, one after the other or at once. Prompt sizes are
the server's own `prompt_n`.

### 3.1 Control: one after the other

| request | prompt_n | reply | finish | wall |
|---|---:|---:|---|---:|
| A | 1244 | 400 | `length` | 74.8 s |
| B | 1244 | 400 | `length` | 79.2 s |

Each alone: 1644 of 2048. Together they would be 2488 before a reply
token — the pool cannot hold both prompts.

### 3.2 The prefill collision: two prompts that do not fit together

A and B at once, `220` filler words each (1244 tokens).

| shape | A | B | wall |
|---|---|---|---:|
| plain | HTTP **500** `server_error` "Context size has been exceeded." | the same | 22.9 s |
| streaming | HTTP 200, **4 chunks**, then the error object | HTTP 200, **0 chunks**, then the error object | 24.0 s |

The server log: slot 0 finished its prefill (1247 tokens) and began
decoding; slot 1's prefill reached batch offset 74 and found no room;
`try_clear_idle_slots` had nothing to purge (both slots processing); the
batch was halved 1024 → 1 in **100 ms**, then `Context size has been
exceeded. off = 74, n_batch = 1` and `send_error` to **both** tasks, `stop
processing: n_tokens = 1247` and `1240`, `update_slots: decode() failed`.
The slot that had been decoding perfectly well died with the one that
overflowed.

In the app's terms (§2.3): B is uncommitted and retried; A had committed
and lands `Failed`.

### 3.3 The growth collision: two prompts that fit, two replies that do not

A and B at once, `160` filler words each (884 tokens): 1768 together, in
the pool with 280 to spare; each reply capped at 400.

| shape | A | B | wall |
|---|---|---|---:|
| plain | HTTP 500, the same message | the same | 48.3 s |
| streaming | HTTP 200, **143 chunks**, then the error object | HTTP 200, **141 chunks**, then the error object | 69.1 s |

Both prompts prefilled; both decoded ~140 tokens; at slot 0 = 1025 and
slot 1 = 1024 (2049 > 2048) the next token had no cell, and both were
ended: `stop processing: n_tokens = 1025` / `1024`. Both streams had
committed, so neither is retried in the app — two `Failed` children after
a minute of visible progress. **This is the case the reservation's reply
half (§4.2) exists for: a guard that priced prompts alone would have
admitted this pair.**

### 3.4 The parked parent survives the collision

P (1244 tokens, 400 reply) runs first and returns; then A ∥ B collide as
in §3.2 (59.8 s to the double failure); then P sends its next turn.

| P's second request | prompt_n | cache_n | wall |
|---|---:|---:|---:|
| after the collision | **5** | **1239** | 44.9 s (74.8 s cold) |

The parent's context was parked in the RAM prompt cache when its slot went
idle and restored intact after the server had cleared both processing
slots — consistent with the parent track's §3.2/§3.5, and what makes R3's
"the turn goes on" true today. One more line in the log worth knowing: a
parked context being *restored* while the other slot is full fails to
find its cells (`state_read_meta: failed to find 1643 available cells`,
`prompt_load: failed to load prompt from cache`) and falls back to a full
prefill — not a failure, but the whole cold cost. A reservation that
prices the prompt covers the restore too (§4.7).

### 3.5 `--kv-unified-per-slot 1024` on the same pool

Relaunched with the cap at exactly `pool / n_parallel`; `/props` now
reports `n_ctx 1024` (the main chat's window, halved).

| arm | A | B |
|---|---|---|
| §3.2's pair (1244 each) | HTTP **400** `exceed_context_size_error` "request (1244 tokens) exceeds the available context size (1024 tokens)", **0.02 s** | the same, independently |
| §3.3's pair (884 each, 400 asked) | HTTP 200, `finish_reason: length` after **140** tokens (884 + 140 = 1024) | the same |

Per request and instant where the pool was collective and slow — but the
second row is a **silent cut**: each reply stopped at 260 tokens short of
its cap with nothing but `length` to say so, which for a sub-agent is a
truncated report the parent reads as complete. And the price is the split
shape's: every conversation, the main chat included, gets 1024 of the 2048
the machine holds. The cap is a server operator's tool for many
independent clients, not this app's guard (F7).

### 3.6 The estimator against the exact count

The probe's prompts through `estimate_prompt`'s rule (bytes / 4 + 4 per
message):

| prompt | estimate | exact (`prompt_n`) | exact / estimate |
|---|---:|---:|---:|
| 220 filler words | 567 | 1244 | **2.19** |
| 160 filler words | 414 | 884 | **2.14** |

The filler (`A17-sigma`-shaped words) is adversarial for a BPE tokenizer
and the ratio on prose is near 1, but that is the point: the estimator's
error is *content-dependent* and can be a factor of two in the direction
R6 forbids. A guard that trusted it raw would admit §3.2's pair. The turn
has a better instrument at hand — its own latest exact `usage` next to
the estimate of the same request, a ratio measured on *this* model and
*this* conversation's kind of text — and §4.3 uses it.

## 4. Design

### 4.1 The reservation, next to the permit

`TurnShared.sessions` becomes a small type of its own — call it the
**session budget** — wrapping what is there (the permit semaphore) with
what §2.1 showed missing: the pool size when known, and the sum of the
reservations of the streams currently open.

```
SessionBudget {
    permits:   Semaphore(sessions),       // as today
    pool:      Option<u64>,               // tokens; None = no guard
    in_flight: Mutex<u64> + Notify,       // sum of open reservations
}
acquire(need: u64) -> Reservation         // cancellable, like acquire_session
```

`acquire` takes a permit, then — when `pool` is `Some` — waits until
either **no stream is open** or `in_flight + need ≤ pool`, adds `need`,
and returns a guard that subtracts it and notifies the waiters when
dropped. The guard is dropped where the permit is dropped today: with the
stream, before the round's tools run. One call site per stream — the
loop's `stream` and the summary in `fetch.rs` — each pricing its own
request (§4.2).

The "no stream is open" clause is R3: a request larger than the pool is
admitted alone and fails or succeeds on the server, exactly as today, and
a waiter can only be waiting *behind* an open stream, which ends — so the
wait always ends. Taking the permit first is deliberate: a stream waiting
for room holds a session it is about to use, and a third loop behind it
queues on the count as it does now; nothing streams without both.

### 4.2 What a stream reserves

`need = prompt + reply`, both the largest honest figure:

- **prompt** — the calibrated estimate of the request about to be sent
  (§4.3), floored by this loop's own last exact size: a round's prompt is
  the previous round's prompt plus its reply plus the tool results, so
  `last_usage.prompt_tokens + last_usage.completion_tokens` is a lower
  bound the server has already vouched for. A child's first round has no
  last usage and rides the estimate alone.
- **reply** — `request.sampling.max_tokens` when set, which it always is
  for a child (`SubagentLimits::max_tokens`, min'ed into the request) and
  for the summary (`SUMMARY_MAX_TOKENS`). When it is `None` — only the
  main turn's own stream, which never overlaps another (§2.1) — the
  reservation is **the whole pool**: the stream is admitted alone, and
  nothing waits behind it that would not have waited anyway (F2).

### 4.3 Calibrating the estimator with the turn's own usage

Every round of every loop ends with the server's exact `prompt_tokens` for
a request the loop had estimated a moment earlier. Their ratio is the
tokenizer's density on this conversation's text, and `TurnShared` keeps
the latest one (an atomic — `f64` bits — written by any loop, read by
`acquire`). The main turn's first round always precedes the first child,
so the ratio exists before any reservation is priced; a summary's
reservation uses it too. The ratio is applied **floored at 1.0** (F3): the
estimator over-counting is a stream that waits when it could have run,
which R6 accepts; under-counting is the failure.

Why not send the exact count instead: llama.cpp's `/tokenize` would give
it, at a round trip per round and per child on the server the streams
are competing for, and the clouds have no such endpoint at all — the
guard has no cloud case, but the estimator's rule should not fork by
mode.

### 4.4 Where the pool comes from

| mode | pool | why |
|---|---|---|
| managed, `sessions > 1` | `managed.context_size` (`-c`) | the launcher wrote `-np N --kv-unified`: unified by construction (§2.4) |
| managed, `sessions = 1` | none | one permit; no two streams ever overlap |
| external, `parallel_slots > 1` | `context_budget` (`n_ctx` on `/props`) | the shape is unknowable from outside (§2.4); assuming *shared* is never wrong, only pessimistic on a split server (F4) |
| external, `parallel_slots` absent or 1 | none | no `/props` (vLLM, Ollama, LM Studio, gateways) — nothing the app could measure against; one slot — the server queues |
| the clouds | none | no pool |

An explicit `compaction.context_tokens` outranks the discovered figure, as
it already does for the compaction trigger — one number for "the window",
read by both consumers. The orchestrator computes the pool once per turn
where it builds the semaphore (`start_generation`), from `context_budget()`
and the mode; the loop never asks the server.

### 4.5 The two waiters

- **A child's round.** `TurnLoop::stream` prices `self.request` and
  reserves; a child whose sibling fills the pool waits with its permit,
  cancellable by `Esc` (`cancelled_round`, as for the permit today), and
  the run's `run_timeout` keeps ticking — a child that waits past its
  deadline lands `TimedOut`, which is what a child stuck behind a permit
  does today.
- **`fetch_url`'s summary.** Prices its own request (the page's capped
  text plus the instruction, and `SUMMARY_MAX_TOKENS`) and reserves under
  the same budget through `ToolContext`; concurrent fetches in one segment
  (concurrent-tools §4.5) therefore summarize as many at once as the pool
  holds, not as many as the permits allow.

### 4.6 What the user sees

Nothing new in v1 (F6): a waiting child looks exactly like a child waiting
for a session today — its card open, the chip counting it, its transcript
row listed. A `tracing::info` line names the wait and the numbers
(`need`, `in_flight`, `pool`), which is what a journal entry or a bug
report needs. The `sessions` field's hint changes one clause: *"a
sub-agent's round fails when the running conversations do not fit the
pool together"* becomes *"…waits for room when…"* (spec §11.6, install.md
§3, the locale strings).

### 4.7 The restore is a prompt

Under the unified shape a returning conversation's parked context is
restored into the pool before its new tokens are prefilled (§3.4's second
finding), so a child's second round needs its whole prompt's cells, not
just the new tail's. §4.2's prompt half is the whole request, so the
reservation covers the restore; the exact floor makes it the server's own
figure.

### 4.8 What does not change

The launch line (`-np N --kv-unified` above one session; no
`--kv-unified-per-slot`, F7), the parallel group's order and records, the
permit's lifetime, the retry decorator (the accidental recovery of §2.3
stays as the second line of defence for whatever the estimate misses),
the background tasks (title, reflection, indexing — outside the turn's
budget, parent F9), and every chat at `sessions = 1`, where `acquire`'s
wait clause can never be reached because `in_flight` is always zero when
the single permit is free.

## 5. Difficult spots (named explicitly)

- **Two counters, one truth.** The permit count and the token sum are two
  gates; the invariant is "a stream holds both or neither". The guard
  type owns both and hands out one `Reservation` whose drop releases both
  in the right order (the sum first, then the permit — a waiter woken by
  the sum must find the permit free too).
- **The `Notify` wake-up race.** `tokio::sync::Notify` drops a permit if
  no one is waiting when `notify_one` fires; the wait loop must re-check
  the sum after every wake and use `notify_waiters` (all waiters
  re-check) — several waiters of different sizes, and the smallest may be
  the one that fits.
- **Cancellation while waiting.** `select!` over the cancel token and the
  wait, as `acquire_session` does; a cancelled waiter must not leave a
  partial reservation — the sum is added only after the wait resolves.
- **The floor can exceed the estimate by a lot** when a child's tool
  results are huge (a 30 KB file read): the estimate's bytes/4 for the
  results plus the last exact prompt is the honest figure, and it is what
  §4.2 computes; the danger is forgetting the floor, not applying it.
- **`max_tokens` on the request vs. `SubagentLimits`.** The child's cap is
  written into `sampling.max_tokens` when the run starts (line 2520), so
  pricing the request sees it; the director's line in `run_dialogue`
  (`512.min(limits.max_tokens)`) rides the same field. Price the request,
  never the config.
- **Tests and the scripted engine.** The counting mock (parent §5) sees
  streams, not tokens; the tests need it to report a fixed
  `usage.prompt_tokens` per persona so the floor and the ratio are
  observable, and a `SessionBudget` unit test needs no engine at all.
- **The external pessimism** (F4a) is real on a split server with
  `sessions > 1` and prompts above half a slot: two 3k conversations on
  4k split slots run at once today and would take turns with the guard.
  The user who launched a split server can type `compaction.context_tokens
  = n_ctx × slots`… but that lies to the compaction trigger. The honest
  escape is F4b's typed field, kept for §8 until someone has the server.

## 6. Forks

Recommendations are marked; nothing is decided until the user says so.

- **F1. Where the guard lives.** (a) **Inside the session semaphore — one
  `acquire(need)` for every stream of the turn** *(recommended — the two
  callers already go through it, and "a stream holds both or neither" is
  one type's invariant)*. (b) In the sub-agent group only, the summary
  left out. (c) A separate gate ahead of the permit.
- **F2. The reply half when `max_tokens` is unset.** (a) **Reserve the
  whole pool — the stream is admitted alone** *(recommended — only the
  main turn's own stream, which overlaps nothing)*. (b) A fixed guess
  (1024). (c) Leave that stream out of the sum.
- **F3. The estimator's correction.** (a) **The ratio of the turn's latest
  exact usage to its estimate, floored at 1.0** *(recommended — §3.6:
  the error is content-dependent, and the turn measures it on its own
  text)*. (b) A fixed factor (×1.5). (c) The raw estimate.
- **F4. The pool in external mode.** (a) **`n_ctx` from `/props` when the
  server reports more than one slot; assumed shared** *(recommended —
  never wrong, pessimistic only on a split server; llama.cpp's own default
  shape is unified)*. (b) A typed `pool` field on the external section
  (`shared` / `per slot`). (c) No guard in external mode.
- **F5. A single stream larger than the pool.** (a) **Admitted alone; the
  server decides** *(recommended — R3, today's path)*. (b) Refused
  client-side with a typed error.
- **F6. What a waiting child shows.** (a) **Nothing new; a log line**
  *(recommended — indistinguishable from waiting for a session, which is
  what it is)*. (b) A "waiting for room" step on the chip and the
  transcript row.
- **F7. `--kv-unified-per-slot`.** (a) **Not in the launch line**
  *(recommended — §3.5: the split shape's window with a silent cut)*.
  (b) An optional managed field for operators who want per-request
  refusals.
- **F8. Staging.** (a) **One PR** *(recommended — a guard inside an
  existing type, two call sites, no UI beyond a hint's clause)*. (b) Two:
  the type, then the callers.

## 7. Stage and the test plan

**One stage** (`feat/admission-by-budget`): `SessionBudget` in the
orchestrator (or `shared` if the summary's caller makes it cleaner to
share), the pool computed in `start_generation` per §4.4, the ratio on
`TurnShared`, the two callers pricing their requests, the hint's clause,
the docs of §9.

Unit tests — the budget type alone, then the loop over the keyed and
counting mocks (parent §5) extended with a per-persona `prompt_tokens`:

- the type: two reservations that fit run together; a third that does not
  waits and is admitted when one drops; a reservation larger than the pool
  is admitted when alone and waits when not; a cancelled waiter leaves the
  sum unchanged; `pool = None` never waits; the permit count still bounds;
- the loop: `sessions = 2`, two children whose reservations exceed the
  pool — the counting mock sees **one** stream at a time, both children
  complete, records in the model's order; the same pair under a pool that
  holds both — two at once (the stage-2 test, with a pool); the floor: a
  child's second round reserves at least its first round's exact size;
  the ratio: a parent round whose exact usage is twice its estimate makes
  the child's reservation twice as large; `max_tokens = None` reserves
  the pool; `fetch_url`'s summary reserves; managed at `sessions = 1`
  computes no pool; external with `parallel_slots = 1` computes none,
  with 4 computes `context_budget`; `Esc` while waiting lands
  `cancelled`.

Live, on the local CPU build — the arms of §3 driven through the app:

- `admission_e2e_live` on the launcher's own line at `sessions = 2`,
  `-c 2048`, Gemma 3 4B: a parent whose first reply is **scripted** (two
  `call_subagent` calls — a hybrid backend, test-only, plays the scripted
  reply once and then delegates to the real server, so the small model's
  willingness to delegate is not the variable) and two children each
  told to read a planted file large enough that the two together exceed
  the pool with their caps. With the guard: both children `completed`,
  the counting decorator sees one stream at a time, wall ≈ the two runs'
  sum. Control arm, guard off (`pool = None` forced): the double failure
  of §3.2 or §3.3 as the children's outcomes — the test proves the guard
  did something, not that the server is polite.
- On the LAN stack when it runs Qwen 3.6 (`-np 4 --kv-unified -c
  16384`, `sessions = 4`): `parallel_subagents_e2e_live` unchanged as
  the regression, plus a variant whose children each carry a ~7k-token
  attachment, so four do not fit and the guard admits two — asserted
  through the stream counter and the outcomes, not the wall time.
- The full orchestrator e2e set on one gate model, since the loop's
  stream path changes for every turn.

## 8. Not in this track (recorded so they are not re-derived)

- **The external shape, typed** (F4b): a `pool` field on the external
  section for the split-server user F4a slows down; wait for that user.
- **A waiting label** (F6b) on the chip and the transcript row.
- **Admission for the background tasks** (auto-title, reflection, RAG
  indexing): they share the server outside the turn's budget (parent F9)
  and can still collide with a turn's streams under a unified pool; an
  app-wide budget on `EngineManager` would be the shape.
- **`--kv-unified-per-slot` as a managed option** (F7b).
- **The parked-set bound** of the RAM prompt cache — still the parent
  track's §8 item; §3.4 adds one data point (one 1244-token context
  survived two failing siblings), not the bound.
- **A retry that waits for room.** The decorator's recovery of an
  uncommitted stream (§2.3, live only since the envelope fix) retries on a
  timer; with the guard in place the retry could re-enter `acquire`
  instead — but by then the estimate was wrong once, and the honest fix is
  the estimate.
- **The other clients' in-stream envelopes.** The OpenAI-compatible client
  was the one whose error object parsed as an empty chunk; the Anthropic
  and Gemini clients have their own event shapes and were not re-measured
  here. A `sse_server`-style test per client, of an error object after a
  text delta, is the cheap way to know.

## 9. Documentation touch list (AGENTS.md §4)

- spec §3.4 (the pool sentence: "waits" where it says "fails"), §6.3 (a
  stream reserves before it opens), §9.3.2 (the group under the budget),
  §11.6 (the `sessions` hint's clause).
- architecture §5 (`SessionBudget` on `TurnShared`, the two callers), §8
  (the summary's reservation).
- install.md §3 (`sessions` and the pool: the failure is now a wait).
- locales `en`/`ru`: `ui.settings.desc.sessions`.
- No ADR: the guard implements a fork the parent track recorded (F7) and
  amends no contract between layers; ADR 0010's amendment already covers
  the group.
- journal: tools.md (the loop and the live runs, next to stage 2's entry);
  CHANGELOG `[Unreleased]` Changed; roadmap (the F7 item closes; §8's
  items above join the parent's list); parallel-subagents.md §8 (the F7
  bullet points here); CLAUDE.md's status line when the track ships.
