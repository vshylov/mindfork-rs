# Parallel subagents: several `call_subagent` runs in flight at once — research

> **Status: research, pre-decision (2026-09-03).** The forks in §6 need the
> user's confirmation before any product code; the stage-0 probe in §7 is the
> go/no-go that decides whether the local-engine half is worth building.
> Precedents this builds on: [subagent-chats.md](subagent-chats.md) and
> [ADR 0010](../decisions/0010-subagent-nested-turn.md) (the nested turn),
> [../history/subagent-live.md](../history/subagent-live.md) (the in-flight
> mirror and the live transcript), [two-agent-dialogue.md](two-agent-dialogue.md)
> and [ADR 0011](../decisions/0011-dialogue-directed-run.md) (the sequential
> contract this must *not* break), [history-compression.md](history-compression.md)
> §9a (the measured facts about `llama-server` slots),
> [prompt-caching.md](prompt-caching.md) §4 (slot eviction).

## 1. What is asked

The main agent should be able to delegate **several tasks at once** — three
searches, a critic and an implementer, one subagent per file — and have the
subagents **run at the same time**, the way Claude Code fans out its `Agent`
tool. Today `call_subagent` (spec §9.3.2) is a nested turn that the parent
`await`s to completion; a round with three delegations runs them one after
another, and a model that emits three calls in one reply gets three sequential
runs whose wall time adds up.

The question has two halves, and they have different answers:

- **Mechanism** — can the loop, the record, the in-flight mirror and the UI
  hold N concurrent children? Yes; §5 is the design, and it is smaller than
  the live-transcript stage was.
- **Engine** — does running N requests at once actually go faster, and what
  does it cost in VRAM? On the clouds, unconditionally yes and nothing. On a
  local `llama-server` the answer is "measurably yes, within one context
  budget" — but it rests on server facts the project has measured only in
  passing (§4), so it is the probe's job, not a design assumption.

## 2. The reference: how Claude Code does it

From [code.claude.com/docs/en/sub-agents](https://code.claude.com/docs/en/sub-agents)
(read 2026-09-03):

- **The trigger is the model emitting several `Agent` tool calls in one
  message**; the harness runs them concurrently and returns each one's result
  as its tool result. There is no "batch" tool — parallelism is a property of
  the round, not of the call.
- **Foreground** subagents block the conversation until they complete;
  *"permission prompts are passed through to you as they come up"*.
- **Background** subagents run while the conversation continues; their result
  *"reaches Claude as a completion notification in a later turn"*; a
  permission prompt from one is surfaced in the main session *"and names the
  subagent that is asking"*. Background runs get a smaller built-in tool set.
- Each subagent starts with a **fresh context** (a *fork* is the exception);
  nesting is allowed to a depth of three; the concurrent limit is 20; a
  finished subagent can be **resumed** by id (`SendMessage`).

Mapped onto this project: the foreground shape is exactly one round of the
agentic loop with its `call_subagent` calls executed concurrently instead of
in sequence — the transcripts, the records and the landing are already there.
The background shape (a child that outlives the round and reports later)
contradicts "the run lands with the turn" (ADR 0010 §5) and needs a turn that
stays open waiting on events; it is named as a possible later stage (§6 F1)
and deliberately not designed here. Nesting stays refused (ADR 0010 §3), and
resuming a landed run is a separate feature the record already makes possible
(the transcript is stored whole).

## 3. Inventory — what exists, and what stands in the way

### 3.1 The round already carries several calls; it executes them one by one

Spec §6.3 has said since M3 that *"the client can execute every call of a
round"*, and the wire does: the OpenAI accumulator builds calls by index
(`shared/api/contract.rs:356–391`), Anthropic's several `tool_use` blocks
land in one assistant turn and their results merge into one `user` turn
(`shared/api/anthropic/wire.rs:636–660`), Gemini's parallel function calls
map the same way, and every provider gets **all the round's tool messages
back in one request** (`generation.rs:1895`, `:1989`). The loop itself is a
plain sequential `await` over the round:

```rust
// src/app/orchestrator/generation.rs:1901
for call in &out.calls {
    self.execute_call(call, rewrite, &mut records, &mut tool_msgs).await;
}
```

No `join`, no spawn, anywhere on the call path. Filing is per round: one
assistant message carrying every record, then the tool messages, through
`file_round` (`generation.rs:1806`). So "run the round's subagent calls
concurrently" changes nothing about what is filed or sent back — only *when*
the results become available.

### 3.2 The child borrows the turn's shared part `&mut`

ADR 0010 §2: the child `TurnLoop` borrows `TurnShared` from its parent for the
duration of the call, *sound because the parent is suspended inside
`execute_call`*. `TurnShared` (`generation.rs:1349–1381`) holds:

| Field | Nature | For N children |
|---|---|---|
| `backend: Arc<dyn EngineBackend>`, `registry: Arc<ToolRegistry>` | shared, immutable | clone the `Arc` |
| `evt_tx`, `done_tx` (unbounded senders) | cloneable | clone |
| `id: Uuid` (the turn's generation id), the limits, engine mode/model name | plain data | copy |
| `allowed_for_turn: HashSet<ToolId>` | **`&mut`** — "approve for the rest of this turn" | one lock |
| `confirm_rx: UnboundedReceiver<(String, ToolDecision)>` | **`&mut`, single receiver** | one broker (§5.2) |

Two fields are the whole aliasing problem. Everything else is already the
shape a concurrent child needs. The two are taken `&mut` in exactly one place
each (`generation.rs:2055–2056`).

### 3.3 The progress channel and the mirror know one child

`GenMessage::Progress` variants `ChildStarted(Box<SubagentRun>)`,
`ChildRoundFiled`, `ChildStep`, `ChildTokens`, `ChildLineStarted`,
`ChildTranscript`, `ChildEnded` (`generation.rs:51–90`) carry **no run id** —
there has only ever been one child. The orchestrator's `InflightTurn`
(`orchestrator/mod.rs:438–463`) has singular `child`, `child_stream`,
`child_partial`, `child_line_role`; `ChildStarted` *overwrites* `child`
(`mod.rs:1111`); `forward_child` (`mod.rs:1252`) forwards one stream to the
screen when that child's transcript is open; `emit_chat_list`
(`mod.rs:1580–1594`) pushes one *running* card under the parent; the status
chip is one `SubagentProgress { name, round, tool, kind }` (`events.rs:448`).
All of it generalizes to a list keyed by the run id, which is **already
minted before the run starts** (`generation.rs:2181`) precisely so the live
row can exist — the id is there; the messages just do not carry it.

### 3.4 Cancellation, budgets, the one-turn invariant — unchanged

Each child has its own token from `cancel.child_token()` (`generation.rs:2160`)
and its own `tokio::time::timeout(limits.run_timeout, …)` (`:2245`); `Esc` on
the parent cancels the tree. `max_tool_rounds` and `workspace.max_rounds`
apply per loop, and a round of the parent counts once however many calls it
holds (`generation.rs:1859–1868`). `GenState` keeps **one turn at a time
application-wide** (`gen_state.rs:46`) — parallel children live *inside* one
turn, so nothing about that machine moves.

### 3.5 The engine contract is concurrency-safe; the server is the question

`EngineBackend: Send + Sync` with `&self` methods (`contract.rs:400`);
`OpenAiClient` holds a `reqwest::Client`, which is a pool; `RetryBackend` is
stateless per call (`retry.rs:271–293`). Two `chat_stream` calls on one
backend are type-safe and work at the HTTP level today. What the *server*
does with two of them is §4.

### 3.6 What the project has written down about concurrency

- spec §3.4: *"One loaded model instance, one generation context at a time
  (the target hardware is a consumer PC; parallel generations aren't
  needed)"* — a **design assumption from M0**, not a measured constraint, and
  already false in practice: the automatic title races the reply
  (`tests/title.rs:166–170`, *"whichever of the two concurrent requests lands
  first"*), impersonation and RAG indexing are concurrent sub-states of the
  orchestrator (`gen_state.rs:8–10`), and the embedder is a second server.
- ADR 0011 / two-agent-dialogue.md §3.9: *"at most one request in flight
  ever, which is the VRAM contract"* — this one is **per feature** and stays:
  a dialogue's three contexts are large and append-only, and the probe measured
  them holding their cache slots. Parallel subagents do not change how a
  dialogue runs (§5.6).
- Two process-wide gates serialize on purpose and **refuse** rather than
  queue: the Python sandbox (`shared/sandbox.rs:117`, `Semaphore::new(1)`,
  *"in the normal agentic loop, calls are already sequential anyway — this is
  defense in depth"*) and the code workspace's command runner
  (`features/tools/code.rs:974`, `try_acquire` → the "busy" refusal). Both
  were written on the sequential premise; with siblings they become a real
  path (§5.5, fork F4).
- SQLite sits behind a `Mutex<Connection>` (serialized, fine); the MCP client
  routes replies by request id through a pending map (`shared/mcp.rs:101`),
  so concurrent calls to one server are safe.

## 4. The engine half: what concurrency costs on `llama-server`

This is the part that decides whether the feature is a cloud-only latency win
or a local one too. Facts first, then what has to be measured.

**Measured, in this repository** (history-compression.md §9a M2, on b9769 and
re-confirmed on b9867): with **no `-np` flag** the server sets
`n_parallel = 4, kv_unified = true` and does *not* divide `n_ctx`; with an
explicit `--parallel 2 -c 1024` it reports `n_ctx: 512` per slot and rejects
a 696-token prompt. The managed launcher passes no `-np` (`managed.rs:68–141`
— the full list is `--host --port -ngl -c -m --mmproj --jinja
--reasoning-format --embeddings -ub -b --no-mmap --flash-attn` and the
speculative flags), so **a managed server already has four slots over one
undivided, shared KV buffer**; so does the `docker/` chat container.

**Documented upstream** (`tools/server/README.md`, read 2026-09-03):
`-np, --parallel N` — *"number of server slots (default: -1, -1 = auto)"*;
`-kvu, --kv-unified` — *"use single unified KV buffer shared across all
sequences (default: enabled if number of slots is auto)"*; `-cb,
--cont-batching` — *"(default: enabled)"*; `--cache-ram` default 8192 MiB.
`GET /slots` reports `is_processing` and `n_ctx` per slot; `/props` reports
`total_slots` (measured M1).

What follows from the two together, and what does not:

- **Weights are loaded once; the KV buffer is allocated once at `-c`.** With
  unified KV, N concurrent sequences *share* that buffer: the extra VRAM of a
  second in-flight request is the per-batch compute scratch, not a second
  cache. This is the fact that makes the feature plausible locally at all —
  and it is inferred from the flag's description, not measured here. **P2**.
- **Decode is memory-bandwidth-bound**, so batching a few sequences per step
  costs far less than N× per token: aggregate tokens/s rises, per-sequence
  tokens/s falls somewhat. How much, on the user's GPU with a 27–31B Q4 model,
  is **P2**.
- **The budget is shared.** Parent's suspended prefix (kept in its slot's
  cache, not in flight) plus every running child's context must fit the one
  `n_ctx`. A child is small by construction — system + one message + its
  rounds — so three children next to a 10k parent fit a 16k window, but a
  child that reads a long attachment does not. A *single* over-long request is
  refused as HTTP 400 `exceed_context_size_error` before streaming (M3/M4);
  what the server does when the **sum** of concurrent sequences overruns a
  unified buffer is not documented and not measured — **P3**.
- **Slots are also the cache.** Four slots, LRU (prompt-caching.md §4). The
  parent's prefix survives the delegation only if a slot keeps it, so the
  cap on concurrent children should leave one slot free: `total_slots − 1`,
  three on the default server. `timings.cache_n` on the parent's next request
  is the instrument (**P2**).
- **A one-slot server** (`-np 1` — the user's own gate stack, and the rented
  HF endpoint at `nParallel: 1`) cannot run two requests at once. Whether a
  second concurrent request **queues** (the feature quietly degrades to
  today's sequential behaviour) or is **refused** (the feature needs an
  explicit sequential fallback) is **P3**. `total_slots` from `/props` lets
  the app know in advance on llama.cpp; on any other server only the setting
  applies.
- **The rented gate splits the context**: HF's `nParallel` *"splits ctxSize
  between llama.cpp slots"* (`tools/hf_api.py:343`), so a two-slot gate is a
  two-times-8k gate. Whether the endpoint can be told `--kv-unified` (llama.cpp
  reads `LLAMA_ARG_*` environment variables, and the payload already sets
  `LLAMA_ARG_JINJA`) is a stage-1 question for the CI half — the parallel
  smoke may have to run on the user's stack with `-np 4`, or on the `docker/`
  CPU stack, rather than on the rented one. Fork **F8**.

**On the clouds** none of this applies: N streams are N independent requests,
the cost in tokens is identical to running them in sequence, and the only new
effect is a higher chance of a 429 under a per-minute rate limit, which the
`RetryBackend` already handles per request (cloud-retry-backoff.md). The
parallel win is pure wall time: three 40-second delegations become one.

## 5. Design

### 5.1 The trigger: several calls in one round (F1 → a)

No new tool, no schema change. When a round's `out.calls` holds two or more
`call_subagent` calls, the loop runs **those** concurrently and every other
call of the round sequentially, then files the round exactly as today — one
assistant message with every record **in the order the model emitted the
calls**, the tool messages in the same order. The model triggers parallelism
the only way it can on every provider at once: by asking for several things in
one reply. `call_subagent`'s description gains one sentence telling it so
(*independent tasks may be delegated in one reply as several calls; they run
at the same time*) — the description is the prompt (lessons §4), and whether
the gate models actually do it is **P1**.

Ordering rule, stated once: within a round the subagent calls form one
**wave** that runs first; the remaining calls run after it, in emitted order.
A round is a batch of independent work by the spec's own contract ("one
logical call per round" is what the *descriptions* are written for; the loop
promised to execute every call), and the model has no way to express "A, then
B" inside one reply anyway. Effects are applied in emitted order at the join.

Not generalized to other tools (F6): `code_write` then `code_run` in one
round is order-dependent, and nothing else is both isolated and long enough to
pay for it. `run_dialogue` is excluded from the wave and runs alone after it —
its sequential contract is per feature and stays (ADR 0011 §3).

### 5.2 `TurnShared` becomes shareable — the whole refactor

Make the two `&mut` fields lockable and hand each child a **clone** of the
shared part instead of a borrow:

- `allowed_for_turn: Arc<std::sync::Mutex<HashSet<ToolId>>>` — held for a
  lookup or an insert, never across an `await`.
- `confirm_rx` behind a **confirmation broker**: `Arc<tokio::sync::Mutex<
  UnboundedReceiver<…>>>`, locked by whichever loop is asking **for the whole
  round trip** — send `ToolConfirmRequest`, await the decision, release. A
  second child that needs confirmation while the first is waiting queues on
  the lock and asks next; the user sees one popup at a time, and the popup
  **names the run that is asking** (Claude Code's rule; the payload gains the
  run's `name`). Decision routing by the turn's generation id is unchanged —
  every loop of the turn shares it (ADR 0010 §2) — and with exactly one
  outstanding request per turn the decision cannot reach the wrong asker.
- The rest is `Arc`/`Clone`/`Copy` already.

`TurnLoop.shared` changes type from `&mut TurnShared` to an owned handle;
`run_subagent` builds the child with `self.shared.clone()`; the dialogue's
loops the same. This is a mechanical refactor with **no behaviour change** and
ships as its own PR (AGENTS.md §2: refactor and behaviour do not mix). ADR
0010's rule that there is *one* loop type keeps holding — N children are N
instances of it, and the sequential single child is the N = 1 case of the same
code path, not a preserved second path.

### 5.3 Running the wave

In `tool_round`: partition the calls; for the wave, build each child exactly
as `run_subagent` does today (own request, cloned `ToolContext`, own child
token, own timeout, id minted up front, `ChildStarted` sent) and run them with
`futures_util::future::join_all` over pinned, **borrowed** futures — no
`tokio::spawn`, no `'static` bound, the children are I/O-bound streams and
concurrency on one task is all they need (the `futures-util` crate is already
a dependency). Each child's result is a `CallResult` with its run attached;
after the join the records are placed into the round in emitted order and the
tool messages follow.

**Cap and waves.** `tools.subagent_parallel_max` (default **3**; `1` is
exactly today's behaviour) bounds one wave; on llama.cpp the effective cap is
`min(setting, total_slots − 1)` read from `/props` at turn start (one slot for
the parent's prefix, §4), and `total_slots ≤ 1` means sequential. A round
asking for more than the cap runs in successive waves — the model asked for
five, it gets five, three then two; nothing is refused and the result needs
no note (F2).

**Cancellation** — the parent's token cancels every child's; a cancel
mid-wave lands every partial run as `Cancelled` with the turn, as one child
does today. A child's timeout is its own and does not touch its siblings; the
round's wall time is the slowest child's.

**Tokens.** `ChildTokens` is today re-based on the parent's totals *per
child*; with siblings each child reports **its own run's** completion tokens
under its run id, and the orchestrator sums the running children onto the
parent's figure for the status bar. The landed runs carry their own totals as
now.

### 5.4 Progress, the mirror, the list, the chip

- Every `TurnProgress::Child*` variant gains `run: Uuid`. `ChildStarted`
  already carries the run (with its id); the rest carry the id alone.
- `InflightTurn.child` and its three companions become
  `children: Vec<InflightChild { run, stream, partial, line_role }>` in
  emitted order, looked up by id. `ChildStarted` pushes, never overwrites.
- `emit_chat_list` pushes a *running* card for **each** running child under
  the parent; `activate_focused`'s "is this the live child" and
  `switch_to`'s "parent ↔ child does not cancel" (subagent-live.md §3.5)
  test membership in the list; `forward_child` forwards the stream of
  **whichever** child's transcript is open. The read-only screen, rename of a
  running transcript, `chat://` resolution — all keyed by run id already —
  need nothing.
- The status chip: one aggregate wording, worded by the screen (axis B):
  *"subagents: 3 running · «critic» round 2 · web_search"* — the count, then
  the most recently active child's detail (F5). `SubagentProgress` gains
  `running: usize`; `progress: None` still clears at the wave's end.
- The parent's bubble already opens one running card per call at
  `ToolCallStarted` (`generation.rs:1958`), so a wave shows N *running…*
  cards at once with no feed change.
- Landing: `handle_done` copies a hand-given title onto the landed run *with
  the same id* — already per id — for each child.

### 5.5 Contention on the shared environment (F4)

Siblings share the parent's environment on purpose (ADR 0010 §3): the same
attachments, the same project root and change journal, the same notes and
RAG. Three consequences, one of them needing code:

- **The two process gates.** A sibling's `python_run`/`code_run` while
  another holds the permit gets the *busy* refusal today — correct for a
  second *turn* (impossible) and wrong for a sibling, which was told it may
  use the tool. Both gates were built for load, not exclusion, so the fix is
  **queue, do not refuse**: `acquire()` under the caller's cancellation token
  instead of `try_acquire()`. The refusal text stays for the impossible case
  and its tests (lessons §2: the gate's behaviour has a test of its own; it
  stays and gains a "second caller waits and then runs" sibling).
- **Two siblings editing one file.** Last write wins in the journal, `F4`
  shows the composite diff, and the parent's model sees both results. No
  lock; the tool descriptions already say the project is shared. Documented.
- **Attachments a sibling produced** (`AddAttachment` from `fetch_url` /
  `youtube_watch`) are mirrored into the parent's snapshot once per round
  (`sync_attachments`, `generation.rs:1927`); a sibling running *in the same
  wave* does not see them until the parent's next round. Same rule as today
  across rounds; documented, not changed.

### 5.6 What stays sequential

`run_dialogue` — alone, after the wave (§5.1). The parent's own stream — a
loop has one request in flight of its own, as before. Compaction,
auto-title, impersonation — untouched. The one-turn invariant — untouched.

### 5.7 Settings, spec, docs

- `tools.subagent_parallel_max` (u8, default 3, min 1) with its description
  naming the local budget rule; settings screen row next to the subagent
  timeout.
- spec §3.4's sentence becomes: *one loaded model, one **turn** at a time;
  inside a turn, up to N concurrent requests bounded by the server's slots
  (§9.3.2)*. spec §6.3's pseudo-loop gains the wave. spec §9.3.2 gains a
  paragraph; §9.13 states that the dialogue is excluded from the wave.
- A new ADR (0012) records the decision once stage 1 lands; architecture §5
  (`TurnShared` as a handle) and §10 (the mirror as a list).

## 6. Forks

Recommendation first in each; confirmation requested before stage 1.

- **F1. The trigger.** (a) **Several `call_subagent` calls in one round run
  concurrently** — no new tool; the Claude Code shape. (b) A new
  `call_subagents { tasks: [...] }` array tool — explicit, works on a model
  that never emits two calls in one reply, one more tool in the catalog and
  a second result shape to teach. (c) Background: `spawn_subagent` returns an
  id, the result arrives later — a turn that stays open on events, a child
  that outlives its round; a later stage at most. → **(a)**, with (b) held in
  reserve *only* if P1 shows a gate family that will not batch.
- **F2. Over the cap.** (a) **Waves** — the extra calls run in the next
  wave. (b) Refuse the extras with a note. (c) No cap. → **(a)**; default 3,
  min'ed with `total_slots − 1` on llama.cpp.
- **F3. Confirmation with several askers.** (a) **Serialize** through one
  broker; the popup names the asking run. (b) A queue UI showing every
  pending request. → **(a)**; (b) has no precedent in the app's popups.
- **F4. The sandbox and command gates.** (a) **Queue** (`acquire` under
  cancellation). (b) Keep refusing, with a retry hint in the text. → **(a)**;
  a refusal the model must retry is the "door left open" shape lessons §4
  warns about.
- **F5. The chip.** (a) **One aggregate chip** — count plus the most recent
  child's detail. (b) One chip per child, cycling. → **(a)**.
- **F6. Which tools run concurrently.** (a) **`call_subagent` only.** (b) A
  read-only whitelist too (`web_search`, `fetch_url`, `attachment_search`).
  (c) Every call of a round. → **(a)** now; (b) is a cheap follow-up on the
  same wave machinery once the ordering rule has lived a while.
- **F7. The dialogue.** (a) **Excluded from the wave, runs alone after it.**
  (b) Allowed alongside subagents. → **(a)** — ADR 0011's contract stays
  literally true.
- **F8. Where the parallel smoke runs.** (a) The user's stack at `-np 4`
  (or the default auto slots). (b) The `docker/` CPU stack (slow, free,
  server behaviour only). (c) The rented gate, if `LLAMA_ARG_KV_UNIFIED` /
  `LLAMA_ARG_N_PARALLEL` reach the endpoint. → **(a) for the go/no-go, (b)
  for P3's overflow measurements**; (c) is a stage-1 CI question.

## 7. Stage 0 — the probe (go/no-go, before any product code)

The dialogue track's pattern: a spike branch, a scripted probe, measurements
recorded in this document, then the decision. Four questions:

- **P1 — do the models batch?** With the one added description sentence, a
  user message asking for three independent delegations (three unrelated
  searches; a critic and a summarizer; two personas) — does the reply carry
  two or more `call_subagent` calls? Five runs each on Gemma 4 31B and Qwen
  3.6 27B, and one run each on Claude, GPT and Gemini. **GO** at ≥ 4/5 per
  gate family. A family at ≤ 2/5 makes F1(b) the route for that family.
- **P2 — does it go faster locally, and does the parent keep its cache?**
  On the user's stack at the default slots: three child-sized requests
  (~1–2k prompt, 500-token replies) issued concurrently vs the same three in
  sequence — wall time, per-stream and aggregate tokens/s from `timings`;
  then a request on the parent's prefix and its `cache_n`. **GO** at wall
  time ≤ 0.6× sequential and `cache_n` ≈ the parent's prompt. A NO-GO here
  does not kill the feature — it sets the local default cap to 1 and makes
  the feature a cloud win, which is still worth stage 1.
- **P3 — the two server edges.** (i) `-np 1`: a second concurrent request —
  queued or refused, and with what body. (ii) Default slots, unified KV, a
  tiny `-c`: concurrent sequences whose sum overruns the buffer — error text,
  a wait, or a silent truncation. Both are server behaviour, measurable on
  the `docker/` CPU stack with a 4B model. The answers write the fallback
  rule (`total_slots ≤ 1 → sequential`) and the cap's justification.
- **P4 — the clouds under three streams.** One run per provider with three
  concurrent children: any 429, and that `RetryBackend` absorbs it without a
  tool effect firing twice (the invariant cloud-retry-backoff.md pinned).

Instrument: a `#[ignore]` probe test on the orchestrator's live set that
issues the requests through `OpenAiClient` directly (P2, P3) and through a
real turn (P1, P4), printing the measurements — the shape `dialogue-probe`
used. No product code moves until the numbers are in §7's results section.

## 8. Stages

1. **Stage 0** — the probe above (`spike/parallel-subagents-probe`), results
   into this document, decision on F1–F8.
2. **Stage 1a** (`refactor/turn-shared-handle`) — `TurnShared` as a cloneable
   handle with the two locked fields; no behaviour change; every existing
   test green as the proof.
3. **Stage 1b** (`feat/parallel-subagents`) — the wave in `tool_round`, run
   ids on the progress variants, the mirror as a list, the list and screen
   membership rules, the aggregate chip, the token sum, the two gates
   queueing, the setting, the description sentence, the spec/ADR/journal
   updates. Live: a `parallel_subagents_e2e_live` smoke asserting ≥ 2
   children overlapped in time (their `ChildStarted` timestamps precede the
   first `ChildEnded`) and that the round filed every record in emitted
   order; run per F8.
4. **Later, separately** — F6(b) if wanted; the background shape (F1 c) as
   its own research if a real use appears.

## 9. Difficult spots, named

1. **The confirmation broker is a lock held across an `await`.** That is its
   point, and it is the one place the design serializes on purpose; the
   child holding it while the user reads a popup blocks only its siblings'
   *confirmations*, not their streams. A cancel while waiting must release it —
   the `select!` with the child token already surrounds the wait.
2. **Determinism of the filed round.** Records in emitted order regardless of
   completion order — asserted by a test on the mock backend with staggered
   completion.
3. **Cost visibility.** N children multiply tokens per wall-second, not per
   task; the counter keeps counting every one of them, and the settings
   description says so.
4. **The `-np 1` external server.** If P3 says "refused", the fallback must
   be explicit and tested; if "queued", the feature silently equals today's
   behaviour there, and the smoke must assert *overlap* (not just success) so
   a sequential gate cannot pass it for the wrong reason (lessons §2).
5. **The two gates' existing tests** say "a second caller is refused"; they
   change meaning to "a second caller waits", and the busy refusal keeps one
   test through a zero-wait acquire.
6. **Spec §3.4** is quoted by the docs as a principle; revising it is a
   documentation change with a reason, recorded in the ADR, not a silent
   edit.

## 10. What this does not do

- No nesting (ADR 0010 §3), no background children, no resuming a landed run.
- No change to the dialogue's execution.
- No lock on the shared project; siblings that edit the same file are the
  caller's composition problem, and the diff screen shows the result.
- No change on the wire: a round with three `call_subagent` calls is sent and
  answered exactly as today; only the time between the two requests shrinks.
