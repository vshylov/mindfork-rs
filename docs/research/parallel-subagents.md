# Parallel sub-agents — several `call_subagent` runs in one round — research

> Status: **forks decided, stage 1 done** (2026-09-03). User's decision:
> F1–F10 at their recommended options. Stage 1 (`feat/parallel-sessions`,
> §7): `sessions` on the six engine sections and `tools.subagent_parallel`
> in config, the turn's session semaphore around one stream, `-np N
> --kv-unified` above one (measured on the local build: 3 slots over the
> whole 4096-token pool, where `-np 3` alone gives 1536 each),
> `EngineBackend::parallel_slots` with its `RetryBackend` delegation and the
> settings hint, the `sessions` row on the assistant's Model tab. The
> `subagent_parallel` row and the parallel group itself are stage 2. One
> amendment found while building: the `subagent_parallel` **setting row**
> ships with stage 2, not stage 1 — a knob that does nothing yet is the
> advertised no-op docs/lessons.md §4 warns about. Supersedes the same-day draft of
> PR #440, closed to restart the research from scratch with a sharper
> brief — that draft planned a probe; this one ran it. Measured this session on the LAN
> stack (`llama-server` b10791, `gemma-4-E2B-it-Q8_0`, `-np 4` — four slots
> of 4096 tokens) and, for the model-behaviour probe, on all four clouds:
> the numbers are in §3. Extends
> [subagent-chats.md](subagent-chats.md) / [ADR 0010](../decisions/0010-subagent-nested-turn.md)
> (the nested turn) and [subagent-live.md](../history/subagent-live.md) (the
> in-flight mirror); reconciles with
> [two-agent-dialogue.md](two-agent-dialogue.md) §2.5/§3.9 (the one-session
> VRAM contract, which turns out to rest on a server default that changed).
>
> The ask (2026-09-03): the main agent should be able to start **several
> sub-agents at once**, working in parallel the way Claude Code runs its
> sub-agents. Engine modes support concurrency differently, so **each mode
> has its own setting for the number of simultaneous sessions**; the default
> is **one session shared by the main agent and its sub-agents — exactly what
> exists today**. Possibly two separate settings: the number of simultaneous
> sub-agents and the number of sessions, since when sessions run short the
> sub-agents can **take turns**.

## 1. Requirements

R1. The model decides. As in Claude Code, "parallel" means the model emits
    **several `call_subagent` calls in one reply** and the harness runs them
    concurrently; there is no new tool and no new argument. A single call
    behaves exactly as today.
R2. Per-mode session budget: managed, external and each of the four clouds
    carry their own **`sessions`** — how many request streams the app may
    keep open against that engine at once. Default **1** everywhere.
R3. A second knob, **how many sub-agents run at once**, independent of
    sessions: with fewer sessions than sub-agents, the runs share the sessions
    in turn rather than failing or waiting for one another to finish.
R4. Defaults reproduce today's behaviour bit for bit: one stream at a time,
    sibling calls executed one after another, the same records, the same
    transcript surfaces.
R5. No extra VRAM by default. The dialogue track's constraint (research
    §3.9: one loaded model, one `llama-server` session at a time) was a
    VRAM constraint, not a sequencing preference — §2.4 shows that on the
    current server the number of slots and the size of the KV pool are
    independent, which is what makes R2 satisfiable without new memory.
R6. Everything a sub-agent already has stays: its transcript on the call's
    record, the list row while it runs, the read-only screen, search,
    titling, `chat://`, the confirmation of dangerous calls, cancellation with
    the turn, per-run budgets and timeouts.
R7. Nothing about a chat that never delegates changes on the wire or on
    disk. New settings fields are `#[serde(default)]`; no schema step.

## 2. What exists today (inventory)

### 2.1 The loop executes a round's calls one after another

`TurnLoop::tool_round` ([generation.rs](../../src/app/orchestrator/generation.rs))
walks `out.calls` in order and `await`s `execute_call` for each; a
`call_subagent` call becomes `run_subagent`, which builds a child `TurnLoop`
over `&mut *self.shared` and awaits `child.run()` under the run's timeout.
So two sibling delegations in one reply already *work* — sequentially: the
second child starts when the first has landed. Spec §6.3 says as much ("the
client-side loop can execute every call in a round") and adds that tool
descriptions are "designed for one logical call per round"; the measurement
in §3.4 says the models do not need convincing.

What makes the child sound is the `&mut` borrow: `TurnShared` is borrowed
mutably by the parent, re-lent to the child for the duration of the call,
and the parent is suspended meanwhile (ADR 0010 §2). Two children at once
need the shared part borrowed **immutably** by all of them, which is a
mechanical change (§4.3): only two fields are ever mutated through it —
`confirm_rx` and `allowed_for_turn`, both inside `confirm_call`.

### 2.2 The mirror holds one child

`InflightTurn` ([mod.rs](../../src/app/orchestrator/mod.rs)) keeps `child:
Option<SubagentRun>` plus `child_stream`, `child_partial` and
`child_line_role` — one running (or just-ended) run. `TurnProgress::Child*`
carries no run id: the orchestrator applies each step to *the* child.
`forward_child`, `switch_within_turn`, the list's `running` mark
(`ChildSummary::running`) and the status-bar chip (`AppEvent::SubagentProgress`,
one `Option<SubagentProgress>`) all assume one. Every one of these is a
"the child" → "the child with this id" change (§4.5).

### 2.3 Tokens, budgets, cancellation

A child's token counter continues the parent's (`token_base`), and the bar
shows one number; two children re-basing on the same parent would send the
bar two interleaved sequences (§4.4). Budgets are per loop already
(`max_tool_rounds`, `workspace.max_rounds`, `subagent_max_tokens`,
`subagent_run_timeout_secs`); each child has its own cancellation token
under the turn's; `Esc` cancels the turn's token and every child with it.
None of this changes.

### 2.4 The engine side: the server already has four slots

The managed launcher (`managed::build_args`) passes `-c`, `-ngl`, `--jinja`
and friends and **no `-np`**. llama.cpp's default for `-np` has been `-1 =
auto` since December 2025 (`common/arg.cpp`; the server resolves it in
`tools/server/server.cpp`: `n_parallel < 0 → n_parallel = 4, kv_unified =
true`, b7433 carries the help text). So every managed server this app
launches on a current build has **four slots over one unified KV pool** of
`-c` tokens. The journal recorded the observable half of this when
compaction was built: "with no `-np` flag the server reports four slots over
an *undivided* context, so dividing would be wrong by 4x" (engine journal,
`context_budget` S1) — `OpenAiClient::context_budget` reads `n_ctx` as given
for that reason.

Two shapes, then, both read in `src/llama-context.cpp`:

- **unified** (`--kv-unified`, the auto default): `n_ctx_seq = n_ctx` — every
  sequence may grow to the whole pool, and the sequences share it. The pool
  costs the same VRAM whatever `-np` says.
- **split** (explicit `-np N` without `-kvu`): `n_ctx_seq = n_ctx / N`, padded
  to 256 — each slot owns a fixed share. The LAN stack of this session is
  this shape: `-np 4` over 16384 → `/props` reports `n_ctx: 4096` per slot.

What binds R5: under the unified shape, N sessions cost **no extra KV
memory**; what they cost is *sharing*. And what that sharing risks, read in
`server-context.cpp` (the `llama_decode` retry branch): when the processing
sequences together outgrow the pool, the server first purges idle slots
(their state is parked in the RAM prompt cache, see §2.5), then halves the
batch, and at `n_batch == 1` it answers **"Context size has been exceeded."
to every processing slot** and clears their prompts. Not provoked here (the
stack is split); §4.7 and F7 are about keeping it unreachable.

Spec §3.4's line "one loaded model instance, **one generation context at a
time** (parallel generations aren't needed)" describes the server of 2025.
It goes with this track.

### 2.5 The prompt cache parks evicted contexts in RAM

`--cache-ram` defaults to **8192 MiB** and `--cache-idle-slots` to on
(`common/common.h`). Two paths use it: at slot selection, a slot about to
lose most of its context saves it to the RAM cache and a task whose prompt
matches a parked entry loads it back (`prompt_save`/`prompt_load` in
`get_available_slot`); and, under unified KV, idle slots are saved and
**cleared from the pool** whenever a new task starts, which is how a
suspended parent gives its room to the children and gets it back. The
[prompt-caching research](prompt-caching.md) §4 noted the mechanism; §3.2
measures its effect on interleaved conversations, which is the fact that
decides between the two semantics of R3.

### 2.6 What the clouds and external servers offer

- **Parallel tool calls.** llama.cpp's `parallel_tool_calls` defaults to the
  chat template's own capability (`server-common.cpp`:
  `json_value(body, "parallel_tool_calls", caps["supports_parallel_tool_calls"])`);
  Gemma 4's template reports `true` on `/props`. Our client does not send
  the field. OpenAI's default is on, Anthropic emits several `tool_use`
  blocks unless `disable_parallel_tool_use` is set, Gemini documents
  parallel function calling, xAI is Chat-Completions-shaped. §3.4 measures
  all five rather than trusting the documentation.
- **Concurrency limits.** The clouds ration by requests/tokens per minute
  and, on some tiers, by concurrency; a burst of N parallel streams meets
  `429`, which the `RetryBackend` decorator already retries with backoff
  and `Retry-After` **while the turn is uncommitted** — a fresh request's
  first bytes are exactly that ([cloud-retry-backoff.md](cloud-retry-backoff.md)).
  Nothing in the app knows a tier; the per-provider `sessions` (R2) is the
  user saying what theirs allows.
- **External servers.** A llama.cpp behind the URL answers `/props` with
  `total_slots`; vLLM, LM Studio, Ollama and gateways do not (Ollama's
  `OLLAMA_NUM_PARALLEL` is server-side configuration). The app can *show*
  a discovered count; it cannot know one otherwise.
- **The rented gate** (`tools/e2e_hf.py`) deploys HF's llama.cpp image
  with `nParallel` from `tools/hf_api.py --parallel`, **default 1**, and the
  endpoint record splits `ctxSize` between slots
  ([remote-e2e-hf.md](../history/remote-e2e-hf.md)). So a two-sibling smoke
  there serialises on the server — the `sessions = 1` interleaved arm for
  free — and the concurrent arm needs the LAN stack, or `--parallel 2` at
  the cost of half the context per slot.

### 2.7 Background tasks already share the server

Auto-title, impersonation, RAG indexing and the silent reflection loops
issue requests of their own, uncoordinated with the turn: today they land on
the server's other slots (managed) or its queue. They stay outside this
design (F9): the sessions budget governs the **turn's** streams, which is
where the new concurrency is.

## 3. Measurements (2026-09-03)

Instrument: a Python script over `/v1/chat/completions` and `/props` (kept
out of the tree, like the dialogue probe's spike), non-streaming so that
`timings` come back whole. Stack: `http://192.168.1.20:8000`, llama.cpp
**b10791**, `google_gemma-4-E2B-it-Q8_0`, `-np 4` split (4 × 4096),
`--cache-ram` at its default. The model is the small one the four slots
allow; nothing below is a claim about its quality — every reply in §3.1–§3.3
was thinking-only under the small cap, which does not touch a timing.

### 3.1 Four streams at once: 2.8× the throughput of four in a row

Four requests of ~30 prompt tokens, 200 reply tokens each:

| arm | wall for all four | per-stream speed |
|---|---:|---:|
| sequential | 4.42 s | 197–202 tok/s |
| concurrent (4 threads) | **1.58 s** | 141 tok/s each, 565 tok/s aggregate |

Batched decoding is memory-bound, so the gain is the usual one and grows
with model size; the number that matters for the design is that per-stream
speed drops by a third — a user watching one sub-agent's transcript sees it
slower while its siblings run.

### 3.2 Interleaving two conversations on one slot costs nothing measurable

Two conversations A and B with distinct ~975-token system prompts, three
rounds each, 40 reply tokens per round. `id_slot: 0` pins every request to
one slot — what a `-np 1` server would do to a round-robin of two runs.

| arm | request | `prompt_n` | `cache_n` | `prompt_ms` |
|---|---|---:|---:|---:|
| pinned, alternating A/B | A0, B0 (cold) | 975, 974 | 0, 0 | 85, 80 |
| pinned, alternating A/B | A1, B1 | 29, 29 | **970, 969** | 43, 40 |
| pinned, alternating A/B | A2, B2 | 29, 29 | **994, 993** | 40, 42 |
| free slot choice, alternating | A1, B1 | 24, 24 | 977, 976 | 39, 39 |
| pinned, A held to the end, then B | A1, B1 | 24, 24 | 977, 976 | 40, 39 |

The pinned-alternating arm is identical to the held arm: after B evicts A
from the slot, A's next request finds its 970 tokens again — restored from
the RAM prompt cache, not re-prefilled. So "sub-agents take turns on one
session" (R3) does **not** pay the re-prefill that [prompt-caching.md](prompt-caching.md)
§3.1 measured for a changed prefix (2.7 s at 4.7k tokens). Two caveats
stated rather than hidden: the restore is a memory copy whose cost grows
with the model's KV per token (invisible on an E2B, not on a 31B — not
measured), and the parked set is bounded by `--cache-ram` (8 GiB), beyond
which eviction is real again.

### 3.3 Past the slots, the server queues; nothing fails

Six concurrent requests (150 reply tokens) on four slots: four finished at
1.15 s, the other two at 2.13 s, every one `200`. A client that sends more
streams than the server has slots loses time, not requests — which is why
the app's session budget can be wrong in either direction without breaking
anything, and why it still has to exist: on a cloud the same overflow is a
`429`, and on any server it is throughput given away.

### 3.4 The models emit two delegations in one reply — go on all five

One `call_subagent` schema (the app's own description and parameters), a
user asking for "two independent second opinions, each from its own
sub-agent, both started now", counting tool calls in the reply. Variant
**a** is the description as shipped; variant **b** appends one sentence:
*"Independent tasks may be delegated in one reply: several call_subagent
calls in one message run concurrently, each as its own subagent."*

| backend | model | variant a | variant b |
|---|---|---:|---:|
| llama.cpp (LAN) | gemma-4-E2B-it | **3/5** two calls (2/5 one) | **5/5** two calls |
| OpenAI Responses | gpt-5.6 | 2 calls | 2 calls |
| Anthropic Messages | claude-sonnet-5 | 2 calls | 2 calls |
| Gemini native | gemini-3.1-pro-preview | 2 calls | 2 calls |
| xAI Chat Completions | grok-4.6 | 2 calls ("I'll spin up two … in parallel") | 2 calls |

One trial per cloud and variant (cents); five per variant locally. The
sentence moved the small local model from three in five to five in five,
which is the evidence for F5 (the description states the contract). Claude
passed no `name`, as Qwen did in the sub-agent track's live run — the
landing auto-title covers it. Qwen 3.6 was not on the stack this session;
its template's `supports_parallel_tool_calls` and its behaviour are the
first thing the live run of stage 2 checks.

### 3.5 The same probes on Gemma 4 31B (2026-09-04, RTX 4090, 4 split slots)

The gate model on the gate hardware — `gemma-4-31B_q4_0-it`, llama.cpp
b10791, `-np 4` over 16384 (4096 per slot), one RTX 4090 with 64 GB of host
RAM — answers the two questions §3.1–§3.2 left open:

| probe | E2B (§3.1–§3.3) | 31B |
|---|---:|---:|
| four 200-token streams, sequential → concurrent | 4.42 s → 1.58 s (2.8×) | 28.6 s → **20.8 s (1.38×)**; per stream 30 → **11 tok/s**, aggregate 44 |
| six requests on four slots | 4 at 1.15 s, 2 at 2.13 s | 4 at 16.9 s, 2 at 25.9 s — queued, none failed |
| interleaved on one pinned slot, second visit | `cache_n` 970, `prompt_ms` 43 | `cache_n` 970, **`prompt_ms` 1298** |
| held to the end, second visit | `prompt_ms` 40 | `prompt_ms` 1235 |
| cold 975-token prefill | 85 ms | **22.3 s** |

Three readings:

- **The RAM prompt cache restores a 31B context as cheaply as it holds
  one.** The interleaved arm's second visit costs 1.30 s against 1.24 s for
  the held arm — the restore is a memory copy, invisible next to the 29 new
  tokens' own prefill. So the semantics F3 chose (a session per stream,
  alive runs interleaving) hold on the large model too, as long as the
  parked set fits `--cache-ram`.
- **Concurrency buys much less on this card than on the small model.**
  Re-measured in the unified shape (`-np 4 --kv-unified -c 16384`, what
  stage 1 launches at `sessions = 4`; the same day, after the e2e set):

  | streams at once | wall for the batch | per stream | aggregate |
  |---:|---:|---:|---:|
  | 1 (four in a row) | 23.1 s | 38 tok/s | 38 tok/s |
  | 2 | 10.1 s for two (11.5 s in a row) | 22 tok/s | 43 tok/s |
  | 4 | 19.0 s for four | 12 tok/s | 48 tok/s |

  A 31B Q4_0 on one 4090 is bandwidth-bound already at one stream, so a
  second stream costs the first almost half its speed and the batch gains
  1.14×; four gain 1.22× and each transcript reads at 12 tok/s. On this
  hardware `sessions` above **2** buys nothing a user would feel; the
  measured gain of parallel sub-agents here is the *overlap of their tool
  work*, not of their decoding. The E2B's 2.8× is what a small model, or a
  card with headroom, gets.
- **The cold prefill is anomalous on the Gemma stack, in both shapes** —
  960 tokens in 18.6 s (~52 tok/s) unified, 975 in 22 s split; the e2e set
  took 72 min against the ~10 min the same set took on this hardware in
  August (b10322, `-np 1`). Not the shape and not the card: the Qwen run
  below, same build, same line minus the projector, prefills 1305 tokens in
  **590 ms** (~2200 tok/s). So it is the Gemma 4 path on b10791 or the
  `--mmproj` projector — one experiment on the stack tells which (the
  Gemma line without `-mm`, one ~960-token cold request), and it is not
  this track's.

**Qwen 3.6 27B (`Qwen3.6-27B-Q4_K_M`, the same day, the same unified
line without a projector):** the template reports
`supports_parallel_tool_calls: true` (the §8 question — no explicit
`parallel_tool_calls` needed), and the model emitted **two `call_subagent`
calls in 10/10 replies** (5 per description variant; it left `name` blank
in 2 of 10, the landing auto-title's case).

| streams at once | wall for the batch | per stream | gain over in-a-row |
|---:|---:|---:|---:|
| 1 (four in a row) | 20.1 s | 43 tok/s | — |
| 2 | 5.6 s for two | 39 tok/s | **1.78×** |
| 4 | 6.9 s for four | 33 tok/s | **2.9×** |

The RAM cache restore: 260 ms on the second visit against 590 ms cold. So
on the same card the 27B keeps 77% of its per-stream speed at four streams
and the batch gains 2.9× — the E2B's figure, not the Gemma 31B's — which
says the 31B's 1.22× was its prompt path, not the hardware's ceiling.
`sessions` of 4 is worth having on this machine for Qwen; the honest
setting is per model, and the field is per mode, which is the right
granularity to say so in the hint rather than to guess.

## 4. Design

### 4.1 The unit of parallelism is the sibling group of one round

A round's calls are split into the **ordinary calls** (registry tools,
control tools, refusals — executed in order, as today) and the **sub-agent
group**: every `call_subagent` call of the round that passes the disabled
gate. The ordinary calls run first, in the model's order; then the group
runs **concurrently**; then the round's records and tool messages are
assembled **in the model's order**, so the request history and the stored
message are exactly what a sequential execution would have produced. A
group of one is today's path.

Why not run the group at its position among the ordinary calls, or the
ordinary calls concurrently too: an ordinary call's effects land in the
loop's accumulators (`self.effects`, `self.ctx` via `sync_attachments` at
the round's end) under `&mut self`, and nothing a sub-agent does in the
same round can see them anyway — the child clones `ctx` when it starts, and
the mirror into the snapshot happens once per round (`tool_round`). Ordering
the two phases keeps the existing invariants and the existing code; running
registry tools concurrently is a separate track (§8).

The group runs as futures inside the generation task, not as spawned
tasks: `futures_util::stream::iter(children).buffer_unordered(n)` — the
crate is already a dependency — polls at most `n` children at once and
yields completions as they come. Each child's future is the existing
`run_subagent` body (its `Box::pin(child.run())` under the run's timeout),
so cancellation, the timeout, the outcome mapping and the result trailer
are unchanged. `ToolCallStarted` is sent for every member of the group
before it starts (cards open together, spec §11.3); `ToolCall` follows each
result as it lands, so a later card can close before an earlier one — the
feed keys cards by `call_id` and already tolerates that.

### 4.2 Two knobs

- **`sessions`** — on each engine section (`engine.managed`, `engine.external`,
  `engine.openai`/`gemini`/`claude`/`grok`), `u32`, default **1**: how many
  request streams the app keeps open against that engine at once. Realised
  as a `tokio::sync::Semaphore` on `TurnShared`, sized from the active
  mode's value when the turn starts; **`TurnLoop::stream` acquires a permit
  for the duration of `stream_round` and nothing else** — a round's tool
  execution, a child's web fetch, a popup waiting for the user hold no
  session. The parent's own stream takes a permit too, so a turn at the
  default is today's turn: one stream, then one child at a time.
- **`tools.subagent_parallel`** — `u32`, default **1**: the `n` of
  `buffer_unordered` — how many of a round's sub-agents are alive at once;
  the rest start as siblings finish. At 1 the group runs one child at a
  time, in the model's order, which is the current behaviour.

How they combine (R3): with `subagent_parallel = 3` and `sessions = 1`,
three children are alive and their **rounds** take turns on the one
session — the interleaving §3.2 measured as free on llama.cpp — while their
tool calls (fetches, Python, MCP) overlap. With `sessions = 3` their rounds
stream together. With `sessions = 8` on a four-slot server, the server
queues the surplus (§3.3). Which of the two semantics for "take turns" this
is, and why, is F3.

### 4.3 `TurnShared` is borrowed immutably by every loop of the turn

`TurnLoop.shared` becomes `&'a TurnShared`. The two fields the loop
mutates through it move behind one async-aware lock:

```rust
confirm: tokio::sync::Mutex<ConfirmState>,   // { rx, allowed_for_turn }
```

`confirm_call` takes the lock for the whole ask-and-wait, which gives the
one property the popup needs — **one question outstanding at a time**: a
second child that reaches a dangerous call waits for the first popup's
answer before asking its own. `wait_for_decision`'s "skip a reply for
another call" stays as it is (still possible with a stale reply), and the
`select!` on the loop's cancellation token inside the wait means `Esc` with
a popup open releases the lock: no child can wedge a sibling. "Allow for
the rest of this turn" keeps covering every loop of the turn, as ADR 0010
decided. The rest of `TurnShared` is already read-only (`Arc`s, senders,
copies).

### 4.4 Tokens: one turn total

`token_base + total_tokens` per loop is replaced for the status bar by an
`AtomicU64` **turn total** on `TurnShared` that every loop adds its deltas
to; the bar receives the total and grows monotonically whichever loop
produced the token. Each loop's own `total_tokens` stays for its record and
its `ChildTokens` progress (the transcript's own counter). `RoundSink.child:
Option<u64>` (the base) becomes `Option<ChildRoute { run: Uuid }>`.

### 4.5 Progress and the mirror, keyed by run id

Every `TurnProgress::Child*` variant gains `run: Uuid` (`ChildStarted`
already carries it inside the run). `InflightTurn` replaces its five
single-child fields with

```rust
children: Vec<InflightChild>,   // { run: SubagentRun, stream: Uuid, partial, line_role }
```

in start order, an entry kept until landing as today. `handle_progress`
applies a step to the entry with that id; `forward_child(run)` forwards
when *that* transcript is the open conversation; `switch_within_turn` treats
the parent and every running child as one set; `activate_focused`/`view()`
resolve the open transcript's `LiveTurn` from its entry; `emit_chat_list`
marks every running row. `handle_done` already collects every run that
landed on the turn's records and titles each — a round with three runs is
three records, as a turn with three sequential delegations is today.

The status-bar chip becomes a small set: `AppEvent::SubagentProgress`
carries the run id, the screen keeps the running runs' latest positions by
id and words the line — *"3 sub-agents · «Critic» round 2 · web_search"* —
in the interface language, clearing an entry on its `None`. Wording is the
screen's (spec §9.3.2); the loop reports facts.

### 4.6 The model-facing contract

`CallSubagent` is constructed with the effective `subagent_parallel`
(`build_registry` already rebuilds the registry from config on every
settings edit) and, when it is above 1, its description ends with the
sentence §3.4 measured, with the number: *"… several call_subagent calls in
one message run concurrently, up to N at a time."* At 1 the description is
byte-identical to today's. The result trailer, the `chat://` address, the
outcome line — unchanged per run. The parent's round budget spends **one
round per round**, whatever the group's size, as spec §9.3.2 already says of
one delegation per round.

### 4.7 The managed launch line

`engine.managed.sessions` above 1 adds `-np <sessions> --kv-unified` to
the launch line; at 1 the line is unchanged. Unified rather than split (F4):
it is the shape llama.cpp itself chooses for its auto default, the pool
stays `-c` (R5), the main conversation keeps the whole window when it is
alone, and each child's context comes out of the same pool only while it
streams. The cost is the hazard of §2.4 — the processing sequences must fit
the pool together. Two guards, one now and one later:

- now: the sub-agent has no history, its replies are capped by
  `subagent_max_tokens`, and the settings field's hint says what the pool
  is shared by (*"N sessions share the context of `-c`; a sub-agent's round
  fails when they do not fit together"*); the failure, if reached, is the
  engine error path the loop already has — the child lands `Failed` with the
  server's message, the parent is told, the turn goes on;
- later (§8): admission by budget — a child's round waits for a session
  while the in-flight prompt sizes (`usage.prompt_tokens`, which every
  round already records) plus the caps would exceed `context_budget`.

`EngineBackend::parallel_slots() -> Option<u32>` (llama.cpp's
`total_slots` from the `/props` fetch `context_budget` and `vision` already
make; `None` elsewhere; **delegated by `RetryBackend` with a test asserting
a value it could not have produced by falling through** — the hole
docs/lessons.md §9 records three times) feeds a hint next to the external
mode's field: *"server: 4 slots"*. It never sets the value: the count is
typed, like the model name once was not and now is sent.

### 4.8 Cloud sessions

Per provider, `sessions` is the user's statement of their tier; the retry
decorator absorbs a `429` when the statement is optimistic (§2.6). No
provider knowledge is baked in; a hint names where each provider documents
its limits.

### 4.9 Dialogues stay sequential

`run_dialogue` keeps its internal "at most one request in flight" contract
(ADR 0011 §3) — its executor is a scripted loop, and a participant line and
a director checkpoint are one request each; under the semaphore they behave
as before. A dialogue is **not** a member of the parallel group in v1 (F6):
it runs at its position among the ordinary calls. The group machinery does
not exclude it by construction — a dialogue keys its progress by run id too
— so admitting it later is a one-line change once its own mirror state is
per child (§4.5 makes it so).

## 5. Difficult spots (named explicitly)

- **`Send` across the group.** The children's futures run inside a
  `tokio::spawn`ed task, so `&TurnShared` must be `Sync`: it holds `Arc`s,
  unbounded senders, `Copy` data and the two new locks — all `Sync`. No
  `RefCell`, no `Cell` (a `&RefCell` would make the task future `!Send`).
- **The recursion.** `run → tool_round → execute_call → run_subagent → run`
  already needs one `Box::pin`; `buffer_unordered` over boxed futures adds
  no second one.
- **Results in order, events out of order.** The group's results are
  collected with their index and written into `records`/`tool_msgs` at the
  round's end in the model's order; the screen's `ToolCall` events arrive in
  completion order. The mirror's `LivePartial.tools` already matches by
  `call_id`; the feed's card must too — verify, do not assume.
- **A child's effects.** `SetSystemMessage`/`SetSamplingOverride` go to the
  run, `AddAttachment` to the parent — unchanged; with several children the
  parent's attachments arrive in completion order and are mirrored into the
  snapshot once, at the round's end, as today.
- **The chip and the counter under concurrency** (§4.4–§4.5) are the two
  places where "the child" was an assumption rather than a field; both
  change shape, neither changes meaning.
- **Tests and the scripted engine.** `MockBackend::sequence` hands out
  replies in call order; two concurrent children consume it in an order
  the scheduler decides. The tests need a **keyed** mock (a script chosen by
  the request's system message) and a **counting** mock (max streams in
  flight, with `cycling`'s delay to make overlap observable) — both small
  additions to `shared/api/mock.rs`.
- **The RAM cache is not the KV pool.** §3.2's free interleaving holds while
  the parked contexts fit `--cache-ram`; on a 31B at 16k the cache may hold
  two or three, and beyond that a pinned single slot re-prefills. The
  default `subagent_parallel = 1` makes this opt-in; the settings hint says
  it.
- **Confirmation and cancellation.** A popup answered while a sibling's
  round is streaming is fine (the lock covers the ask-and-wait, not the
  stream); a popup open when the run's timeout fires ends that child alone
  (its token), and the lock is released by the `select!`.

## 6. Forks

Recommendations are marked; nothing is decided until the user says so.

- **F1. The unit of parallelism.** (a) **Sibling `call_subagent` calls of
  one round, the model deciding** *(recommended — Claude Code's shape, no
  new tool, R1)*. (b) A `spawn`/`await` pair: children that outlive the
  round while the parent keeps writing, results delivered in a later round
  (Claude Code's background agents). (c) Any sibling calls of any tools. —
  (b) changes the turn's contract (a round ends when its results are in) and
  (c) needs a per-tool concurrency mark; both are §8.
- **F2. The two knobs.** (a) **`sessions` per engine section + one global
  `tools.subagent_parallel`, both default 1** *(recommended — the user's
  own suggestion)*. (b) `sessions` only, a child holding a session for its
  whole run. (c) `subagent_parallel` only, the server left to queue.
- **F3. What "take turns" means when sessions run short.** (a) **Per
  round: a session is held for one stream and released for the tool
  phase; alive children interleave** *(recommended — §3.2 measured the
  interleave free on llama.cpp, and it is where R3's separation buys
  something: tool latency overlaps)*. (b) Per run: a session is held from a
  child's first request to its last; siblings beyond the sessions wait
  whole. — (b) is cache-safest on a `-np 1` server with a full RAM cache and
  forfeits every overlap; it can be offered later as a mode of the same
  semaphore if a large-model measurement argues for it.
- **F4. The managed launch shape at `sessions > 1`.** (a) **`-np N
  --kv-unified`: the pool stays `-c`, shared** *(recommended — llama.cpp's
  own default shape, R5, the main chat keeps its window)*. (b) `-np N` split:
  each conversation gets `-c / N`, failures isolated (a clean 400), the main
  chat's window quartered. (c) `-np N -c N×c`: every slot the full window,
  KV memory ×N. — The user can reach (c) from (a) by raising `-c`; (b) is a
  regression for the main chat at any realistic `-c`.
- **F5. Where the model is told.** (a) **The tool description, with the
  number, only when `subagent_parallel > 1`** *(recommended — measured 3/5 →
  5/5 on the small model; the registry is rebuilt on every settings edit,
  so the tool can carry the number)*. (b) A system-prompt block. (c) Nothing
  — the clouds and the larger local models do it unprompted.
- **F6. Dialogues in the group.** (a) **Not in v1; `run_dialogue` runs at
  its position, sequentially inside** *(recommended)*. (b) In the group from
  the start.
- **F7. The unified-pool hazard.** (a) **Hint + the existing failure path
  now; admission by budget later** *(recommended)*. (b) Admission by budget
  in stage 2. (c) Split KV (F4b) to make the failure per request.
- **F8. Session discovery in external mode.** (a) **Typed value, default
  1; llama.cpp's `total_slots` shown as a hint** *(recommended — explicit,
  like the model name's precedent once it was made explicit)*. (b) Auto
  from `/props` when blank, like the model name.
- **F9. Background tasks and the semaphore.** (a) **Outside it** *(recommended
  — they coexist with the turn on the server today; the sessions budget is
  the turn's)*. (b) Inside an app-wide semaphore on `EngineManager`.
- **F10. Staging.** (a) **Two PRs: stage 1 engine and settings, stage 2 the
  loop and the mirror** *(recommended; §7)*. (b) One PR. (c) Three, with the
  UI (list, chip, switch) split out of stage 2.

## 7. Stages and the test plan

**Stage 1 — settings, the launch line, the semaphore** (`feat/parallel-sessions`).
`sessions` on the six engine sections and `tools.subagent_parallel`
(`#[serde(default)]`, defaults 1); `build_args` adds `-np N --kv-unified`
when `sessions > 1`; `EngineBackend::parallel_slots` + `RetryBackend`
delegation + the settings hints; the semaphore on `TurnShared` around
`stream_round`. No behaviour change at the defaults — the live gate's
existing set is the regression scope, plus: `build_args` at 1 is
byte-identical to today's, at 3 carries the two flags; `parallel_slots`
reads `total_slots` from the measured `/props` fixture and the decorator
delegates it; a counting mock under `sessions = 1` never sees two streams
even with `subagent_parallel = 3` (the loop of stage 2 is what would open
them — so this test moves to stage 2, and stage 1 asserts the permit is
taken and released around one stream).

**Stage 2 — the group, the immutable shared part, the mirror** (`feat/parallel-subagents`).
§4.1, §4.3–§4.6, §4.9. Unit tests, over the keyed and counting mocks:
two siblings land two records in the model's order with the results of
their own persona; `subagent_parallel = 1` runs them one after the other
(the counting mock sees one stream at a time, in order); `= 2` with
`sessions = 2` sees two; `= 2` with `sessions = 1` sees one at a time but
both children alive (their `ChildStarted` precede either's `ChildEnded`);
a dangerous call in each child asks one popup at a time and the second
waits; `Esc` with the popup open cancels both; one child's timeout lands it
`TimedOut` while its sibling completes; the turn total on the bar is
monotone across interleaved chunks; the mirror keeps two running entries,
forwards each transcript's stream under its own id, marks both rows
running, and lets the user switch parent → child A → child B → parent
without cancelling; the description carries the number at `> 1` and is
unchanged at 1. Live, on the LAN stack (`-np 4`) and both gate models:
`parallel_subagents_e2e_live` — two planted-token files, the parent told to
delegate both reads at once; both transcripts on the records, both tokens
in the reply, and with `sessions = 2` a wall time below the two runs' sum;
the same smoke at `sessions = 1` for the interleaved arm; one cloud arm
(Anthropic, the strictest about tool-result shape) to prove the ordered
records satisfy a strict provider.

## 8. Not in this track (recorded so they are not re-derived)

- **Background sub-agents** (F1b): a child that outlives its round, with
  the parent notified in a later round — needs a "task notification" round
  shape and a place for a result that arrives after the turn ends.
- **Concurrent ordinary tools** (F1c): a `Tool::concurrent()` mark for
  read-only tools and a group per round like the sub-agents'.
- **Admission by budget** (F7): the app-side guard for the unified pool.
- **A large-model measurement of the RAM cache** (§3.2's caveat): the
  restore cost and the parked-set bound on a 31B at 16k, on the stack when
  it next runs that model.
- **Qwen 3.6's template** (`supports_parallel_tool_calls`) — read on the
  first live run of stage 2, and sent explicitly as `parallel_tool_calls:
  true` if the template's default proves to be off.

## 9. Documentation touch list (AGENTS.md §4)

- spec §3.4 (the stale one-context sentence), §6.3 (the round's two
  phases), §9.3.2 (the group, the two knobs, the description sentence),
  §11.6 (the fields and their hints), §11.1 (the chip's set form).
- architecture §5 (the loop's group and the immutable `TurnShared`), §6
  (`parallel_slots`, the launch line), §10 (the mirror's `children`).
- install.md §3 (`sessions` and what `-np N --kv-unified` does to the pool).
- ADR: a new one for the parallel group over the shared part — or an
  addendum to ADR 0010, whose §2 (the `&mut` borrow) it amends; decide with
  F10.
- journal: engine.md (stage 1), tools.md (stage 2), ui-screens.md (the
  mirror and the chip); CHANGELOG `[Unreleased]` Added; README (settings);
  roadmap (the §8 items); CLAUDE.md's status line when the track ships.
