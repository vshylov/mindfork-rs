# Concurrent ordinary tools — a round's read-only calls run at once — research

> Status: **decided, implementation starting** (2026-09-04). User's
> decision, same day: F1a, F2a, **F3 — `4` on the four clouds, `1` on
> managed and external** (so the knob is a field of each engine section,
> as `sessions` is — §4.4 rewritten to that shape), F4a, F6a, F7a, F8a, F9a;
> F5 was not answered and is built at its recommended (a), pending the
> user's word. The item [parallel-subagents.md](parallel-subagents.md) §8
> recorded as F1c and left for later: the round's *ordinary* tool calls —
> the reads the model issues in one reply — run concurrently, the way that
> track made the round's `call_subagent` calls run. The model-behaviour
> probe (§3) ran before any design: on the LAN stack (Gemma 4 31B,
> llama.cpp b10807, the unified four-slot line stage 1 launches) and on all
> four clouds, every model answered a task of two independent reads with
> two `fs_read` calls in one reply and a task of three pages with three
> `fetch_url` calls, 26/26 — **go**. Extends the sibling's design (§4.1 there: the round's three phases,
> `resolve_round` → `run_group` → the records), which is the shape this
> track reuses, and [ADR 0010](../decisions/0010-subagent-nested-turn.md)'s
> amendment, whose point 4 names "a per-tool concurrency mark" as this
> track. The precedent for a per-tool mark is `Tool::danger()`
> ([tool-confirmation.md](../history/tool-confirmation.md), spec §9.8).
>
> The ask (2026-09-04, from the §8 review): of the five items the parallel
> sub-agent track left, this one was judged the most valuable — the loop's
> structure after stage 2 is exactly what it needs, and the win is real
> wherever a call waits on a network.

## 1. Requirements

R1. The model decides. "Concurrent" means the model emits **several calls in
    one reply** and the harness runs the ones that may run together, together;
    there is no new tool, no new argument, and nothing new is told to the
    model — §3 shows it does this unprompted (unlike the sub-agent case,
    where a sentence in the description was measured to matter).
R2. **Results identical to a sequential round, by construction.** The request
    history, the round's records, the stored message and every call's result
    text are what executing the calls one after another in the model's order
    would have produced. Not "identical in practice": identical because the
    rule that picks what runs together cannot admit two calls whose results
    depend on their order (§4.2).
R3. **Only a tool whose author says so joins a group.** A per-tool mark,
    default off. A marked tool has no effect a sibling call of the same round
    could observe, changes nothing outside the application, holds no
    exclusive resource (a process, a sidecar, a socket to a stateful peer),
    and is never dangerous (spec §9.8) — so no confirmation popup can ever be
    part of a group.
R4. A knob bounds how many run at once; `1` is the sequential round.
R5. Everything a tool call has stays: its card (opened when the group starts,
    closed when its own result lands), the disabled/control/rewrite gates,
    cancellation with the turn, the round budget and the workspace exemption
    (spec §9.12), its effects, its images.
R6. An engine request a tool makes on its own — `fetch_url`'s page summary —
    obeys the engine's **`sessions`** budget like every stream of the turn
    (spec §11.6). Today it does not, and today nobody notices, because only
    one such request is ever in flight.
R7. Nothing on the wire or on disk changes for a chat whose rounds never
    carry two marked calls. The new settings field is `#[serde(default)]`;
    no schema step; no migration.

## 2. What exists today (inventory)

### 2.1 The round after stage 2 of the sibling track

`TurnLoop::tool_round` counts the round and hands its calls to
`execute_round` ([generation.rs](../../src/app/orchestrator/generation.rs)),
which runs three phases: `resolve_round` walks the calls **in the model's
order** — every ordinary call through `resolve_call` (the card opens, then
`resolve_call_result`: the disabled gate, the control gate, the rewrite gate,
the confirmation round trip, and the invocation under a `select!` with the
turn's cancellation token), every `call_subagent` only announced and prepared
as a `ChildSpec`; `run_group` runs the children as futures inside the
generation task, `buffer_unordered(tools.subagent_parallel)`, each card
closing as its run lands; then `record_call` writes the tool messages and the
records in the model's order, skipping the card close for what the group
already closed (`announced[i]`).

What an ordinary invocation actually borrows: `&self.shared.registry` (an
`Arc`), `&self.ctx` (the turn's immutable snapshot), `&self.cancel`. The
only mutation is `self.effects.extend(outcome.effects)` after the result —
and `report_progress`, which is `&self`. So a set of invocations can be
built over immutable borrows and their effects applied afterwards; the
sibling's `run_group(&self, …)` is already that shape, and its `announced`
path is already the way a card closed early is not closed twice. None of the
loop's ordering invariants (the assistant message pushed before the round
runs, `sync_attachments` once at the round's end, the cancellation check
after filing) is touched by running some calls together.

### 2.2 What a tool may touch

`ToolContext` ([features/tools/mod.rs](../../src/features/tools/mod.rs)) is
a per-turn snapshot: `Arc`s (storage, engine, embedder, attachments, the
other-chats scope, the history view), copies of config, a `&'static` locale,
the cancellation token. No `RefCell`, no `Cell` — the futures a group builds
over `&ToolContext` are `Send`, which the generation task requires (the
sibling's §5 first spot). `Storage` synchronises internally: concurrent
reads of `data.db`/`cache.db` serialise on its connection and are correct,
which also means the database-bound tools gain nothing from a group — the
gain is where a call waits on the file system or the network.

Three classes of tools need naming before any mark is given:

- **Tools that reach the engine inside `invoke`.** Exactly one: `fetch_url`,
  whose default `summarize` makes a single-turn request per page
  (`summarize_text`: no history, no tools, 768 reply tokens, a 90-second
  timeout). It runs **outside the session semaphore**, because the semaphore
  lives on `TurnShared` in `app` and a tool sees only `ToolContext` in
  `features` (FSD). Sequentially that is invisible: the parent holds no
  permit during its tool phase, so the summary is the turn's only stream.
  Three fetches at once are three summaries at once — on a `-np 1` external
  server they queue (the sibling's §3.3: nothing fails), on a unified-pool
  managed server they share the pool with nothing else processing (the idle
  parent is parked in the RAM cache), and on a cloud they are three requests
  against a `sessions` the user set to one. R6 is about this.
- **Tools that write inside a read.** `note_recall` backfills missing note
  vectors before it searches (`ensure_note_vectors`: embed, then upsert —
  idempotent, but two concurrent calls would embed the same notes twice);
  `web_search` re-ranks with the embedder (a read). `attachment_search`
  reads the index and refuses when nothing is indexed; it builds nothing.
- **Tools that hold a resource.** `python_exec` (the `wasmer` sidecar),
  `code_build`/`code_run`/`code_test` (a process tree, with the kill-tree
  discipline of docs/lessons.md §6 — *cancellation drops a future and runs
  no cleanup of yours*), every MCP tool (third-party code; the client's
  transport does allow several requests in flight — a pending map keyed by
  request id under a writer mutex — but `McpTool::danger()` is `true` by
  decision and the server's `readOnlyHint` is untrusted input, mcp.rs).

### 2.3 The candidates

Read-only, no exclusive resource, never dangerous — the set a first mark
covers, and what each waits on:

| tool | waits on | note |
|---|---|---|
| `fs_read`, `fs_list` | the file system | the commonest pair in one reply (§3) |
| `code_read`, `code_grep`, `code_list` | the file system | `code_grep` walks in-process; no subprocess |
| `attachment_read`, `attachment_search` | `cache.db` | serialised by storage; no gain, no harm |
| `chat_search`, `chat_read` | `cache.db`, chat files | the same |
| `history_read`, `history_search` | memory, `cache.db` | the same |
| `get_self_model`, `get_sampling`, `get_llm_name`, `get_llm_history` | `data.db`, none | trivially safe; marked for consistency of the rule, not for gain |
| `fetch_url` | the network, then the engine (§2.2) | the largest win: seconds per page |

Left out of the first mark, each for a named reason: every writer
(`fs_write`, `code_edit`, `code_write`, the note writers, the self-model
updaters, `add_insight`, `reflect`, `consolidate_notes`, `set_sampling`);
the command tools and `python_exec` (a resource, §2.2); the MCP tools (§2.2;
§4.9 for later); `note_recall` (writes inside a read); `youtube_watch`
(network-heavy per call and an `AddAttachment` effect — nothing wrong with
it, but its gain is one video per reply in practice, so it waits for a
second pass); `web_search` (the throttling question, §2.6 and F4); the two
loop-executed tools and the control pair, which never reach the registry.

### 2.4 Cards, chip, records

The feed keys a tool card by `call_id`: `ToolCallStarted` opens it marked
*running…*, `ToolCall` with the same id completes it, and since the sibling
track a later card may close before an earlier one (spec §11.3). The parent
loop reports no per-tool chip; a child loop's `report_progress(Some(name))`
names the tool it is entering, which the status bar words as *"«Critic» ·
round 2 · web_search"*. The records and the tool messages are written by
`record_call` from the `results` slots in the model's order, whatever order
the slots were filled in.

### 2.5 The gates and the budget

The round is counted **before** the calls run, from their names
(`counts_toward_round_limit`), so a group changes nothing about budgets.
The disabled, control and rewrite gates decide per call from the name and
the round's state; the confirmation gate asks per call, under the one lock
the sibling introduced. A marked tool is never dangerous (R3), so a group
never contains a call the gate would stop; a call the other gates refuse
resolves on the spot, as today.

### 2.6 The web tools under load

docs/lessons.md §9 recorded what several web smokes from one address
provoke: DuckDuckGo answers a challenge page — as `HTTP 200`/`202`, not an
error — and `web_search` (the chain DuckDuckGo lite → html → Mojeek →
Ecosia, or Tavily when keyed and chosen) has a classifier for exactly that.
Three searches at once from one address is that situation on purpose. Tavily
is built for concurrent use and rate-limits by plan. Nothing comparable is
known about `fetch_url`: three pages from three hosts are three unrelated
connections, and a page that throttles its own readers throttles a
sequential reader too.

### 2.7 The models already emit several calls

The sibling's §3.4–§3.5 measured two `call_subagent` calls in one reply on
five models; §3 below measures the same for ordinary tools, where no
description sentence is needed. On llama.cpp the reply can carry several
calls only when the chat template's capability says so
(`chat_template_caps.supports_parallel_tool_calls` in `/props`, the
request's default since the server reads it from the template): both gate
models report `true`. A model whose template does not simply never produces
a group — nothing to do, nothing to guard.

## 3. Measurements (2026-09-04)

Instrument: a Python script over `/v1/chat/completions`, non-streaming, two
tool schemas taken verbatim from the app's own descriptions (`fs_read`,
`fetch_url`), no system prompt beyond "a helpful assistant with tools",
provider-default sampling. Two prompts: *read the two files A and B with
fs_read and say what each contains*; *compare the landing pages of three
sites: fetch each with fetch_url*. Counting tool calls in the first reply.

| backend | model | two reads | three fetches | reply time |
|---|---|---:|---:|---:|
| llama.cpp (LAN, b10807, `-np 4 --kv-unified -c 16384`) | gemma-4-31B-it Q4_0 | **5/5** two `fs_read` | **5/5** three `fetch_url` | 3–7 s |
| OpenAI Chat Completions | gpt-5.6 | **2/2** | **2/2** | 1–3 s |
| Gemini (OpenAI-compatible endpoint) | gemini-3.1-pro-preview | **2/2** | **2/2** | 3–6 s |
| xAI Chat Completions | grok-4.6 | **2/2** | **2/2** | 2–6 s |
| Anthropic Messages | claude-sonnet-5 | **2/2** | **2/2** | 2–3 s |

Twenty-six replies, every one with exactly as many calls as independent
tasks — never one call, never a call too many. Two notes from the run, neither about the
product: gpt-5.6 refuses function tools together with reasoning on the Chat
Completions endpoint (*"use /v1/responses or set reasoning_effort to
'none'"*) — the app talks to OpenAI through Responses, so only the probe
had to pass `reasoning_effort: none`; and Gemma 4 31B, thinking model that
it is, answered with calls and no visible text, as the app expects.

**Not measured, and why.** The wall-time gain is arithmetic — the longest
call instead of the sum — so the stage's live smoke proves overlap by the
order of card events rather than by a stopwatch (§7). Three concurrent
DuckDuckGo searches from one address is the measurement F4 needs before
`web_search` is admitted; it belongs to that follow-up, not to this
decision.

## 4. Design

### 4.1 The mark: `Tool::concurrent()`

```rust
/// Whether a call to this tool may run **at the same time as its neighbours**
/// in a round (spec §6.3). `true` is a claim by the tool's author: the call
/// has no effect a sibling call of the same round could observe, changes
/// nothing outside the application, holds no exclusive resource, and is
/// never `danger()`. Default `false`: a new tool runs alone until someone
/// says otherwise, which is the right default for the same reason
/// `danger()` defaults the other way — a wrong `false` costs a user some
/// seconds, a wrong `true` could interleave a read with the write it was
/// meant to follow.
fn concurrent(&self) -> bool { false }
```

On the trait beside `danger()`, `counts_toward_round_limit()` and
`enabled_by_default()` — the `Tool` impl is where a tool's other contracts
live, and the catalog (`ToolInfo`) gains the bit so tests can assert the
documented set without instantiating the loop. Two invariants pinned by
unit tests over `standard_registry`: no tool is both `danger()` and
`concurrent()`; the marked set is exactly §2.3's table (a snapshot test, so
a tool cannot be marked by accident and the doc cannot drift).

### 4.2 The unit of parallelism is the *segment*

A **segment** is a maximal run of *consecutive* calls in the model's order
that would all be invoked concurrently: each one allowed by the profile,
marked, not a control call, in a round not being discarded, not a
`call_subagent` (which has its own group). Any other call — a writer, a
disabled or unknown name, a control call, a group call — ends the segment
and resolves at its own position exactly as today. A segment of one *is*
today's path (one future, awaited).

Why not "every marked call of the round, as one group" — the shape the
sub-agent group has: that group runs **after** all ordinary calls because a
child clones its context when it starts and can observe none of the round's
effects anyway. An ordinary read can. Take a round of `code_read(f)`,
`code_write(f)`, `code_read(f)`: run the reads first and the second read
returns the old bytes; run them last and the first read returns the new
ones; only the model's order returns what the model asked for. The segment
rule cannot reorder a read across a write, so R2 holds by construction —
the only assumption is the one R3 states, that marked calls do not observe
each other, and that is a property of the tools, not of the round.

Why not dependency analysis on arguments (two `fs_read`s of different
paths may reorder across an `fs_write` of a third): correct, and worth
nothing — a round that mixes reads and writes is the code-workspace walk
(read → change → check), where each step depends on the last and nothing
runs together anyway.

### 4.3 Execution

`resolve_round` grows a scanner: instead of one call at a time, it takes the
next segment. A segment of one goes through `resolve_call` unchanged. A
longer one goes through `run_segment(&self, calls) -> Vec<(usize,
CallOutcome)>`: every member's card opens first (`announce_call` — cards
open together, as the sub-agent group's do); the invocations run as futures
inside the generation task, `futures_util::stream::iter(..)
.buffer_unordered(width)`, each wrapped in the same `select!` with the
turn's cancellation token that `resolve_call_result` uses today, so `Esc`
resolves every member with the "tool cancelled" text and the round files
what it has; each result closes its card as it lands (`announce_result`,
`announced[i] = true`). After the segment, in the **model's order** (F6):
the effects go to `self.effects`, the results into their `results[i]` slots.
`record_call` and everything after it are untouched.

Nothing spawns: the futures borrow `&self.ctx` and `&self.shared` for the
segment's duration, so no `'static`, no `Arc` cloning, and the cancellation
token reaches them unchanged — the sibling's reason for the same choice.

### 4.4 The knob

**`concurrent_calls: u32` on each engine section** (`engine.managed`,
`engine.external`, `engine.openai`/`gemini`/`claude`/`grok`),
`#[serde(default)]` — the shape `sessions` has, and the user's decision on
F3: the default is **4 on the four clouds and 1 on managed and external**.
A local server is the machine the user is sitting at, and its one context
pool is what every stream of the turn shares; a cloud is someone else's
fleet. So a local setup keeps today's sequential round bit for bit unless
the user raises the number, and a cloud setup gets the overlap out of the
box. The value is the `width` of `buffer_unordered` — how many of a
segment's calls are alive at once; the rest start as siblings finish. Read
into `TurnShared` at the turn's start from the active mode's section
(`engine.active_concurrent_calls()`, beside `active_sessions()`), like
`subagent.parallel`. A settings row *"Parallel tool calls"* on each
engine section's Model tab beside "Sessions", whose hint names what the
number covers (*"reads and page fetches the model issues in one reply run
at once; writers, commands and plugins always run one after another"*) —
derived from the marked set, so the hint cannot advertise what the rule
does not do (docs/lessons.md §4).

### 4.5 The summary takes a session (R6)

The turn's semaphore becomes `Arc<tokio::sync::Semaphore>`, held by
`TurnShared` as today and handed to the turn's `ToolContext` as
`sessions: Option<Arc<Semaphore>>` — `None` for background tasks, which the
sibling's F9 keeps outside the budget. `fetch_url::summarize_text` acquires a
permit around its stream and nothing else. At the defaults nothing
observable changes: the parent holds no permit during its tool phase, so
the one summary a sequential round makes takes the free permit and gives it
back. With `sessions = 1` and three fetches in a segment, the three
downloads overlap and the three summaries stream one after another. With a
sub-agent fetching while its sibling streams, the summary now waits its
turn, which is what `sessions` promised. FSD is respected: `features` holds
a tokio primitive, not an `app` type.

No deadlock is possible: a permit is held for a stream and released before
any tool runs (the sibling's §4.2 rule), the summary holds one only while
its own stream runs, and no holder ever waits for a tool — so a summary
waiting for a permit is waiting for a stream, and streams end.

### 4.6 The sub-agent group is unchanged

Ordinary phase — now with segments — then the group, then the records. A
child observes a writer's effect of the same round today (the file is on
disk when it starts) and still does; a child never observes a read.

### 4.7 The chip

Only a child loop reports per-tool progress. For a segment it reports once,
with the members' names joined (*"fs_read, fs_read, code_read"*) — a fact
in the interface's hands; the screen words it as it words one name. No new
event shape.

### 4.8 The model-facing contract

Nothing is added to any description (R1, §3). Spec §6.3's sentence —
*"tool descriptions are still designed for 'one logical call per round'
for predictability — with one exception the model is told about"* — is
rewritten to what is true: a round may carry several calls; the ones that
may run together do; the rest run in the model's order.

### 4.9 MCP, later

An MCP tool could join a segment on its server's `readOnlyHint`, but the
annotation is untrusted by decision (mcp.rs, the plugins track's F3) and
may never relax a safety property. If it ever does, it is through a
per-server opt-in the user types, the way `mcp_images` is a user's switch
and not a server's claim. Not in v1.

## 5. Difficult spots (named explicitly)

- **The `&mut self` walk.** `resolve_round` is `&mut self` because
  `resolve_call` is (effects, the confirmation state). A segment runs over
  `&self` and hands its effects back — `run_group(&self, …)` is the
  precedent; the scanner must not hold a borrow of `calls` across the
  `&mut` application. Straightforward, but the place a lifetime error will
  appear first.
- **Effects and the order they land.** A segment's effects are applied in
  the model's order after the whole segment, not in completion order (F6):
  an `AddAttachment` from the second call of three must be mirrored after
  the first's, so that a sequential round and a concurrent one leave the
  same `Chat`. Today only `youtube_watch` produces one, and it is not marked
  yet; the rule is stated now so that marking it later changes nothing.
- **Cancellation is a drop.** A member's future is dropped on `Esc`, and no
  cleanup of it runs (docs/lessons.md §6). Every tool in §2.3 holds nothing
  that needs cleanup — no process, no temp file, no half-written state —
  which is a condition of the mark, stated in its doc comment, not a happy
  accident.
- **Throttling multiplies.** §2.6. `web_search` stays out until measured
  (F4); `fetch_url` is admitted because three hosts are three unrelated
  connections.
- **Tests and nondeterminism.** Completion order is the scheduler's;
  assertions are on what is stable — each result recorded against its own
  arguments (docs/lessons.md §2: on a degrading path, assert *which*
  result, never `is_ok()`), the records in the model's order, the maximum
  in-flight count seen by a counting tool, and the event order "every
  `ToolCallStarted` of a segment precedes any `ToolCall` of it". A counting
  tool with a delay makes overlap observable, the way the sibling's
  `KeyedRecorder` does for streams; it is registered on the test
  orchestrator's registry (`ToolRegistry::register`), no static state (a
  process-wide gate makes sibling tests fail each other — lessons §2).
  Every wait bounded.
- **Older smokes.** A live smoke that asserts the *order* of card events
  across two reads in one reply will see a different order once they
  overlap (lessons §9: a new capability invalidates an older smoke's
  assertion while improving its outcome). Read the harness's collectors
  before the run, not after the red.
- **Sonar.** `resolve_round` gains a scanner; the round was already split
  three ways for cognitive complexity (the sibling's stage 2). The scanner
  is its own function (`next_segment`), and `run_segment` mirrors
  `run_group` rather than growing inside `resolve_call`.
- **The summary's permit and the mock engine.** A test that opens two
  summaries at `sessions = 1` needs an engine that counts streams — the
  sibling's counting mock already does, and lives in the orchestrator's
  test module rather than in `features`; the `fetch_url` unit test asserts
  the permit is taken and released around the stream with a semaphore of
  one, the way `acquire_session`'s own tests do.

## 6. Forks

Recommendations are marked; nothing is decided until the user says so.

- **F1. The unit of parallelism.** (a) **Maximal segments of consecutive
  marked calls, in the model's order** *(recommended — R2 by construction,
  §4.2)*. (b) Every marked call of the round as one group, before or after
  the rest (the sub-agent group's shape — wrong for a read that follows a
  write, either way). (c) Dependency analysis on arguments — correct and
  worth nothing (§4.2).
- **F2. Where the mark lives.** (a) **`Tool::concurrent()` on the trait,
  default `false`, the catalog carrying the bit** *(recommended — the
  `danger()` precedent; the author of the tool makes the claim)*. (b) A
  list of names in the loop. (c) A user-edited list in config — a
  correctness property is not a preference.
- **F3. The default width.** (a) **`4`** *(recommended — the results are
  identical by construction, the risky tools are excluded or guarded, no
  memory is spent, and a knob most users never find would leave the feature
  unused; `1` stays reachable and is what the hint calls "one after
  another")*. (b) `1`, as `subagent_parallel` defaults — that default had a
  VRAM reason (the sibling's R5) which does not exist here; raising it
  later would be the one-line follow-up the sibling's knobs took. The whole
  e2e set runs at the chosen default on both gate models before the PR
  either way. **User's decision (2026-09-04): (a) for the four clouds,
  (b) for managed and external** — which makes the knob a per-section
  field like `sessions` (§4.4), and means the live gate, which runs on a
  local stack, exercises the segments only through the smoke that raises
  the number.
- **F4. `web_search` in the set.** (a) **Out in v1; admitted after a
  measured probe of three concurrent searches on the keyless chain** *(recommended —
  §2.6)*. (b) In from the start. (c) In only while Tavily is the provider
  — the registry is rebuilt on every settings edit, so a mark that reads
  the provider is consistent; the first follow-up if (a).
- **F5. The page summary and `sessions`.** (a) **A permit through
  `ToolContext.sessions`, taken around the summary's stream** *(recommended
  — R6, §4.5; small)*. (b) Leave it outside — the server queues, the cloud
  retries a `429`, the unified pool takes three summaries at once. (c)
  Leave `fetch_url` unmarked — forfeits the largest win for the sake of a
  request the tool makes on its own. *Not answered in the user's decision
  of 2026-09-04; built at (a), to be confirmed.*
- **F6. The order effects land.** (a) **The model's order, after the
  segment** *(recommended — same `Chat` as a sequential round)*. (b)
  Completion order.
- **F7. Telling the model.** (a) **Nothing** *(recommended — §3: 20/20
  unprompted; the spec sentence is rewritten, not the descriptions)*. (b) A
  sentence in the marked tools' descriptions.
- **F8. The decision record.** (a) **A new ADR 0012 — the round's segments
  and the mark** *(recommended — it changes the tool contract of spec §9.2
  and the loop contract of §6.3, which are ADR 0004's, and ADR 0010's
  amendment points here by name)*. (b) A second amendment to ADR 0010. (c)
  None — the research doc suffices.
- **F9. Staging.** (a) **One PR: the mark, the segment, the knob and row,
  the permit, the tests, the live smoke** *(recommended — the sibling's
  stage 2 was one PR of the same size, and the two halves cannot be
  observed apart)*. (b) Two: the mark and the segment at width 1 (no
  behaviour change), then the default and the row.

## 7. Stage and the test plan

**One PR — `feat/concurrent-tools`** (F9a). `Tool::concurrent()` with the
marks of §2.3 and the two catalog invariants; `next_segment` and
`run_segment` in the loop; `concurrent_calls` on the six engine sections
and its settings row;
`ToolContext.sessions` and the permit in `summarize_text`; the ADR; the
docs of §9.

Unit tests (the orchestrator's scripted engine plus a counting tool —
in-flight now, in-flight max, a per-call delay, the result echoing its own
argument): the scanner — a writer, a disabled name, a control call and a
`call_subagent` each end a segment, a lone marked call is a segment of one,
a round of marked calls is one segment; a segment of three at width 3 sees
three in flight, at width 1 sees one and finishes in the model's order, at
width 2 sees two; the records and the tool messages carry each result
against its own arguments in the model's order whatever the completion
order; the `ToolCallStarted` events of a segment all precede its first
`ToolCall`; `Esc` mid-segment resolves every member "cancelled" and the
turn lands `Cancelled` with the round filed; a segment's effects are
applied in the model's order (a marked test tool emitting an effect); the
summary takes and releases a permit (a semaphore of one, two summaries in a
row) and two `fetch_url` summaries at `sessions = 1` open one stream at a
time on the counting engine; no registered tool is both dangerous and
concurrent; the marked set equals the documented one; the settings row's
three (renders, edits, hint).

Live (the LAN stack, both gate models; AGENTS.md §3): a new
`concurrent_tools_e2e_live` — two planted-token files, the parent told to
read both with `fs_read` in one reply (the §3 prompt); both tokens in the
reply, two `fs_read` records on one assistant message, and in the event
log both cards opened before either closed — the overlap proof, as the
sibling's smoke reads it off the runs' timestamps. Then **the full e2e set
at the chosen default** on both models: at F3a every multi-call round of
every smoke takes the new path, which is the point of running the set and
not the one smoke. One cloud arm (Anthropic, the strictest about
tool-result shape) to prove the ordered records satisfy a strict provider
— the sibling proved it for the group, and a segment produces the same
history.

## 8. Not in this track (recorded so they are not re-derived)

- **`web_search` in the set** (F4): after the concurrent-search probe on the
  keyless chain, or per provider (F4c).
- **MCP tools on `readOnlyHint`** (§4.9): a per-server opt-in, never the
  server's word alone.
- **`youtube_watch`, `note_recall`** (§2.3): the first once its effect
  ordering is exercised by a marked tool with an effect, the second once
  the vector backfill is moved out of the read path or made a single
  flight.
- **Ordinary calls and sub-agents in one group**: the phases stay ordered
  (§4.6); a child observing a sibling read's result is not a case anyone
  has asked for.
- **Concurrency across rounds** — the sibling's F1b (background
  sub-agents), unchanged.
- **A per-tool width** (three fetches, one search): one number until a
  measurement says two.

## 9. Documentation touch list (AGENTS.md §4)

- spec §6.3 (the loop: segments; the "one logical call per round"
  sentence), §9.2 (the trait: `concurrent()`), §9.3 (the roster: which are
  marked — a column or a sentence per family), §9.8 (a marked tool is never
  dangerous), §11.3 (cards of a segment open together), §11.6 (the field
  and its hint), §9.3.1 (`fetch_url`'s summary under `sessions`).
- architecture §5 (the round's phases with segments; `run_segment`), §8
  (the trait method, the catalog bit, `ToolContext.sessions`).
- ADR 0012 (F8a), linked from CLAUDE.md's ADR list and architecture §5.
- journal: tools.md (the track, the live runs); CHANGELOG `[Unreleased]`
  Added; README (the settings row); roadmap (close the F1c item; add §8's
  follow-ups); CLAUDE.md's status line when the track ships; lessons.md if
  a trap surfaces.
