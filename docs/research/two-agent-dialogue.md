# Two personas in dialogue, directed by the model (`run_dialogue`) — research

> Status: **forks confirmed; probe §5.1 — Gemma arm GO, cloud spot-checks
> GO** (2026-08-29, every go bar met, two design rules found on the way; the
> Qwen arm needs the stack rotated to Qwen 3.6 and is the one piece
> pending). User's decision (2026-08-28):
> F1–F10 at their recommended options, **F6 amended** — the director carries
> the **main agent's identity** (the chat's persona) and **knows the
> conversation with the user**, delivered as a brief built by the `/compact`
> summarizer's mechanism (§3.2, F6). The §5 probe follows on
> `spike/dialogue-probe`. This is the document the sub-agent track promised:
> [subagent-chats.md](subagent-chats.md) §3.14 sketched the feature and left
> two seams open for it on purpose (`RunKind`, `RunOutcome`), and the roadmap
> carries it as "the two-agent dialogue (research §3.14 — its own research
> document when wanted)".
>
> The ask (2026-08-28): the assistant can spin up **two sub-agents with custom
> system messages** that talk **to each other**, each seeing the other as the
> user. Custom system messages are mandatory — a model inhabits a role far
> better through its system message than through instructions inside a user
> turn. The **main agent is the dialogue's director and moderator**: it decides
> when the conversation is finished, and editing the dialogue's messages is on
> the table. The dialogue is written by the **same model the assistant runs
> on** (as `call_subagent` does). The feature is allowed to be expensive in
> time (local) and money (cloud), but must be **frugal with VRAM**: one
> `llama-server` session at a time — the three conversations involved (director
> and two personas) take turns, never overlap.

## 1. Requirements

R1. Two participants, each with a **caller-written system message** (`a`/`b`),
    each seeing the other's lines as **`user` turns** — the shape a chat model
    is trained on, which is why the roles land.
R2. The **director** is model-driven: it steers the dialogue while it runs and
    **decides when it is over** — a message cap is a backstop, not the
    mechanism.
R3. The director can **edit**: at minimum, reject or steer a bad line; the
    strongest form (rewriting a message outright) is considered here (F3).
R4. One model writes everything — the turn's engine backend, exactly as
    `call_subagent` uses it (`TurnShared.backend`,
    `src/app/orchestrator/generation.rs:1273`). No second model, no second
    server.
R5. **Sequential by construction**: at any moment at most one engine request is
    in flight. Three contexts (A, B, director) are *conversations*, not
    processes; they take turns on the same server (§3.9).
R6. The result is a **transcript the user can open** — the same experience a
    sub-agent transcript already gives: nested in the chat list, read-only,
    searchable, titled, addressable as `chat://`.
R7. Accepted costs: wall-clock on local models (prompt re-processing when
    contexts alternate), tokens/money on cloud. Not accepted: extra VRAM.

## 2. What already exists (inventory)

### 2.1 The seams left on purpose

[subagent-chats.md](subagent-chats.md) §3.14 pre-designed this feature against
the sub-agent track's seams, and the code kept them:

- `RunKind` (`src/entities/subagent.rs:22`) has one variant and a doc comment
  naming "the director-led dialogue of two personas" as the planned second
  kind. Everything downstream — list nesting, the read-only screen, deletion,
  search scoping, titling — keys off "this record has a run", **never off the
  kind** (`chat.rs:334`, `search.rs:285`, `title.rs:83`,
  `orchestrator/mod.rs:1015`).
- The effects rule was written with this feature in mind: *"environment
  effects to the parent, identity effects to the child — which is also what
  the dialogue feature will need (two identities, one environment)"*
  (subagent-chats.md §3.4).
- §3.14's sketch: `RunKind::Dialogue` on the same `SubagentRun`; the record of
  the director's call holds the whole dialogue — **one call, one transcript**;
  participant A's request is built by the **role swap** impersonation already
  does; a director tool `run_dialogue { a, b, opening, max_turns }`.

What §3.14 did **not** design is the moderation the user now asks for (R2/R3):
the sketch stops at `max_turns`. That is this document's main addition (§3.3,
F1).

### 2.2 The nested-turn machinery (ADR 0010) this reuses

| Need | Exists | Where |
|---|---|---|
| Loop-executed dispatch (a `Tool` cannot reach the engine loop or emit events — `ToolContext` has no sender) | the name ladder in `resolve_call_result` | `generation.rs:1908`, arm at `:1949` |
| Executing under the turn: cancellation lineage, timeout, token accounting, confirmation plumbing | `run_subagent` as the template | `generation.rs:1996` |
| Transcript persistence, inseparable from the exchange | `ToolCallRecord.subagent: Option<Box<SubagentRun>>` | `entities/message.rs:48`, `entities/subagent.rs:45` |
| List nesting, fold state, read-only viewer, refusals with a note | `Chat::children`, `ChildSummary`, `ChatView` | `chat.rs:334`, `:482`, `orchestrator/mod.rs:425` |
| Search: transcripts as conversations of their own | `sub_id` scope, `CACHE_SCHEMA` 2 | `storage/cache/mod.rs:63` |
| `chat_search`/`chat_read` reach transcripts | `ChatRef.parent` | `features/tools/chats.rs:76` |
| Title at landing, manual rename wins | `maybe_auto_title_run` | `orchestrator/title.rs:83` |
| Live view while the turn runs | `TurnProgress::Child*`, `InflightTurn.child*`, `view()`'s third arm | `generation.rs:50`, `orchestrator/mod.rs:435` |
| `chat://` address minted before the run starts | `chat_links`, id minted at `:2061` | `features/chat_links.rs` |
| Whole-run timeout, per-round reply cap | `tools.subagent_run_timeout_secs` (600 s), `tools.subagent_max_tokens` (4096) | `config.rs:875-879` |
| Parent `Esc` cancels, timeout cancels the child alone | `cancel.child_token()` | `generation.rs:2040`, `:2127` |

### 2.3 The role swap

`swap_role_message` (`src/app/orchestrator/impersonation.rs:233`): User →
assistant, Assistant → user; System/Tool and empty messages dropped. This is
exactly R1's mechanism. Two caveats it carries: it drops tool records (fine —
participants have no tools in v1, F4), and impersonation also **forces
reasoning off** for its background turn — the same mute the director's
checkpoint requests want (§3.3).

### 2.4 Provider facts that bind the design

- **Strict alternation** is a cloud-provider constraint handled in the wire
  layers (Anthropic merges adjacent same-role, `api/anthropic/wire.rs:9-11`;
  Gemini likewise, `api/gemini/wire.rs:217`). llama.cpp applies the model's
  own jinja template server-side (spec §2, `spec.md:176`) — and **Gemma's
  template rejects a conversation whose first non-system message is
  `assistant`**, which is exactly what the opener's own view would look like.
  §3.2's derivation rule (prologue + merge) closes this by construction; the
  probe (§5) verifies it on the real template.
- **Tool records under a role swap are a hazard**: B's `assistant(tool_calls)`
  round re-sent as `user` content is the orphaned-tool-call shape that 400s on
  Anthropic ([history-compression.md](history-compression.md) §
  "Protocol correctness"). v1 sidesteps it — participants have **no tools**
  (F4) — and the seam for later is stated there.
- **Assistant prefill is not needed.** Each participant writes a fresh
  message; nothing here depends on the `/continue` capability gates.
- **Thinking on verdict turns**: Qwen can spend a whole capped turn inside
  `reasoning_content` and return nothing (docs/lessons.md §9, measured 1-in-20
  → 0-in-20 with thinking muted). Director checkpoints mute thinking the same
  way the auto-title/impersonation background turns do. Participants keep the
  chat's thinking behaviour — their thoughts land in the transcript like any
  assistant message's.
- `reasoning_effort: "none"` is rejected by xAI (docs/lessons.md §3) — the
  mute must go through the existing per-provider path, not a new hardcode.

### 2.5 KV cache and the one-session constraint

[prompt-caching.md](prompt-caching.md) measured the cost of a changed prefix
(0% reuse, 2.7 s prefill at 4.7k tokens, linear in length) and recorded
llama-server's shape: **4 slots, LRU, `--cache-ram` parking** (§3.1, §"llama.cpp").
Three observations shape §3.9:

- All three dialogue contexts are **append-only** by design (each new message
  extends the tail; the system prompts never change mid-run — director notes
  append, F3), so each context's *own* prefix is stable and slot-cacheable.
- With 4 slots, the three dialogue contexts plus the parent's suspended
  conversation are exactly at capacity; whether they actually retain their
  slots is measurable in the probe via `timings.cache_n` — the instrument the
  roadmap already wants surfaced.
- Even at 0% reuse the design is *correct*, just slower — R7 accepts that.
  What R5 forbids is concurrency, and the executor is a plain sequential
  `await` loop inside the turn: at no point are two requests in flight.

## 3. Design

### 3.1 The tool

`run_dialogue` — a schema-only `Tool` (catalog, profile toggle, localized
description: axis A) plus a loop-executed arm beside `call_subagent`
(`resolve_call_result`), implemented as `TurnLoop::run_dialogue` next to
`run_subagent`. Depth-guarded exactly like `call_subagent`: withheld from
sub-agent runs, refused at `depth > 0`; participants get no tools at all (F4),
so there is no nesting by construction. Group: Awareness (next to
`call_subagent`); `danger: false`; `counts_toward_round_limit: true` (the
parent spends one round on the call).

Arguments (localized descriptions; `required` marked):

```jsonc
{
  "a": { "name": "Alice", "system_message": "…" },   // system_message required
  "b": { "name": "Bob",   "system_message": "…" },   // system_message required
  "opening": { "speaker": "a", "text": "…" },        // text required; speaker defaults "a"
  "scene": "…",              // optional shared setting, delivered to both (§3.2)
  "direction": "…",          // optional director brief: goals, tone, when to stop
  "max_messages": 16,        // optional; default 16, schema cap 64
  "moderate_every": 2        // optional; default 2, range 1..8
}
```

`name` is optional on both sides (the sub-agent live run showed Qwen omitting
optional names — F13 of the sub-agent track); fallbacks are localized labels.
The tool description teaches the model what the caller controls: personas'
system messages **are** the role instrument (R1), `direction` is how the
caller adds *explicit* directing intent — the director also receives the
chat's persona and a conversation brief automatically (§3.2, F6), so
`direction` is for emphasis, not the only channel — and the description says
plainly that a dialogue is a long, many-request operation.

### 3.2 The three contexts and the derivation rule

The canonical state is **one transcript**: an ordered list of messages, each
attributed to `a` or `b` (plus director interventions, §3.4). Every request is
**derived** from it fresh — derivation, not incremental mutation, is what makes
editing (F3) trivially correct.

- **Participant A's view**: `system = a.system_message` (+ appended director
  notes addressed to A); A's messages as `assistant`, B's as `user`.
- **Participant B's view**: mirror image (this is `swap_role_message`'s
  mapping, minus tool handling it doesn't need in v1).
- **Prologue**: a `user`-side opener is prepended where the first derived
  message would otherwise be `assistant` (always the case for whoever spoke
  first), and to both views when `scene` is set: the prologue's text is
  `scene`, else a short localized marker. Then **adjacent same-role messages
  are merged** (the same normalization the Anthropic/Gemini wires do). Result:
  every view is `system, user, assistant, user, …` — the strictest template
  (Gemma's) is satisfied by construction, on every provider. The prologue is
  derivation-only: it never appears in the stored transcript.
- **Director's view** (per the amended F6): `system` = **the parent turn's
  own persona** (`ctx.system_message` — the field a sub-agent's persona
  *replaces*, here kept), then a **conversation brief**, then a localized
  director appendix (`prompt.dialogue.*`, the axis-A pattern impersonation
  uses) + `direction`. The brief is what makes the director *the main agent
  directing*, not a hired stranger: it is built the way `/compact` builds a
  request's head — the chat's rolling summary when one exists
  (`Chat::compaction`, `entities/chat.rs:93`; the request shape is
  `compaction_view`, `chat.rs:296`) plus the unfolded tail within a token
  budget; a long **uncompacted** conversation is folded once at dialogue
  start by the same summarizer the `/compact` path uses (one extra request,
  §3.9). The self-model injection stays top-turn-only, as ADR 0010 F3
  decided for every nested run. The director's conversation is **persistent
  and append-only** across checkpoints: new dialogue lines arrive as `user`
  content (rendered `Name: text`), its own verdicts stay as its `assistant`
  tool-call turns with a short `tool` result ("noted"). Persistence gives
  the director memory of its own notes ("I already told A to wrap up") and
  keeps its context cache-friendly (§2.5).

### 3.3 The loop

```
transcript = [opening by args.opening.speaker]
loop:
  if messages_generated >= max_messages: outcome = RoundLimit; break
  if it is a checkpoint (every moderate_every messages): run director checkpoint
      -> may append notes / retry / rewrite / stop (outcome = Completed; break)
  speaker = the participant who did not write the last dialogue message
  build speaker's view; stream one generation (the participant's turn)
  append the message to the transcript (author = speaker)
```

- A **participant turn** is one streamed generation through the child sink
  (`RoundSink` with `child: Some(token_base)`), so tokens count into the
  parent's totals re-based, the status chip moves, and the live view (stage 2)
  streams it — the same path a sub-agent's round takes. Per-message reply cap:
  `tools.subagent_max_tokens` min'ed with the effective `max_tokens`, as the
  sub-agent does (`generation.rs:2031`).
- **The empty-line recovery** (a probe finding, §5.1): an instruction
  conflict in a participant's effective system — typically a director's note
  fighting the persona's own format rule — can send a thinking model into
  unbounded deliberation: the whole reply cap spent in `reasoning_content`,
  no text (measured on Gemma 4: 1536/1536 tokens of thoughts, four times).
  The executor re-asks that one generation once with thinking muted (the
  compliance-probe rule, lessons §9); a second empty reply fails the run
  honestly. Both generations count against `max_messages`.
- A **director checkpoint** is one request carrying the verdict tools' schemas
  (§3.4), thinking muted (§2.4). The executor interprets the returned calls
  itself — no registry dispatch; the precedent is the conversation-control
  pair, which the loop also interprets by name (spec §9.3.3). Several calls in
  one reply are applied in order (e.g. `dialogue_note` + `dialogue_continue`).
  **A reply with no tool call counts as `continue`** — the dialogue proceeds
  toward its cap rather than stalling; the fallback is recorded on the run and
  its rate is a probe metric (§5).
- **Counting**: every participant generation — including regenerations after
  `dialogue_retry` — counts against `max_messages`, so a director stuck in a
  retry loop terminates. Checkpoints do not count.
- The **first checkpoint runs before the first generated message only if
  `moderate_every == 1`**; the opening line is the caller's and needs no
  verdict.

### 3.4 The director's powers

Verdict tools (schemas exist only inside checkpoint requests; they are not in
the profile catalog):

| Tool | Args | Effect |
|---|---|---|
| `dialogue_continue` | — | proceed |
| `dialogue_stop` | `reason` (req.), `summary?` | end the dialogue; `outcome = Completed`; reason+summary go to the parent's result and land as a closing director note in the transcript |
| `dialogue_note` | `to: "a"\|"b"\|"both"`, `text` | appended to the target's system appendix for all subsequent turns (identity effect — the §3.4 rule of the sub-agent track); also stored in the transcript as a director intervention, so the user sees the steering |
| `dialogue_retry` | `note?` | drop the last dialogue message and regenerate it; `note` is applied to that regeneration as a one-shot system appendix |
| `dialogue_rewrite` | `text` | replace the last dialogue message's text with the director's words (costs no request); the replaced text is gone — the transcript keeps the final cut, the director's intervention row records that a rewrite happened |

Editing an **arbitrary** earlier message (`index` argument) is deferred (F3):
the conversation after it was generated against the original text, so a
mid-history edit silently invalidates what followed — the honest version of
that feature needs "…and regenerate everything after", which is a different,
much more expensive operation.

Director interventions (notes, retries, rewrites, the stop reason) are stored
in the transcript as `System`-role messages, rendered as `Note` rows in the
viewer (`FeedRole::Note` exists; the current projection drops `System`
messages — `message_feed.rs:144` — so the child-view builder maps them
explicitly). Derivation excludes them from participants' views except as the
system appendices described above.

### 3.5 Storage and schema

- `RunKind::Dialogue` — the second variant, at last. `SubagentRun` gains
  `participants: Vec<Participant>` (`#[serde(default)]`, empty for sub-agent
  runs), `Participant { name: Option<String>, system_message: String }`. The
  run's existing `system_message` holds the **resolved director brief** (the
  template + `direction`), which the viewer's system bubble shows alongside
  both personas; `name`/`sampling_override` stay `None`.
- **The transcript is role-encoded** (F2): participant **a**'s messages are
  stored `Assistant`, **b**'s are stored `User`. This is the same
  perspective-encoding impersonation already uses, and it buys the entire
  existing pipeline unchanged: the two-sided feed, message stitching, search
  snippets, `chat_read`, export. The viewer's per-chat character names —
  already resolved at activation for sub-agent transcripts (Assistant header =
  run name, User header = parent persona) — resolve from `participants`
  instead: Assistant header = a's name, User header = b's name. No
  `Message.author` field is needed for two participants; §3.14's
  `author` proposal is recorded as the growth path if more than two ever come.
- **`CHAT_SCHEMA` 2 → 3**, a no-op step ("chat files may carry dialogue
  runs"). The bump is not cosmetic: `RunKind` is a plain serde enum
  (`subagent.rs:20-24`), so an old binary reading `"kind": "dialogue"` fails
  the whole file's deserialization; the version guard
  (`storage/schema.rs:67`) is what turns that into the polite "data from a
  newer version" refusal ADR 0006 designed. `SETTINGS_SCHEMA` does not move —
  the one new knob is additive (§3.8).
- The run's messages keep the sub-agent invariant "never a record with a run
  of its own" (`subagent.rs:69-72`).

### 3.6 What the parent gets back

The tool result is compact and closes the door (docs/lessons.md §4): the
participants, how many messages, how it ended — the director's `reason` and
`summary` when it stopped itself, "the message cap" when it didn't — and the
transcript's `chat://` address, with the same "cite it when you mention the
conversation" teaching `call_subagent`'s result carries. The full text is
deliberately **not** returned: the parent's context is the expensive one, the
user reads the transcript in the viewer, and the parent model can `chat_read`
the address if it genuinely needs the words (child transcripts are already in
that tool's scope — subagent-chats.md F4).

Auto-title at landing (`maybe_auto_title_run`) works unchanged; the initial
title is "«A» ↔ «B»" from the participants' names/fallbacks.

### 3.7 Viewing, live

- **Landed**: list nesting under the parent (fold state, `▸ n` mark), the
  read-only screen with refusals-with-a-note, search as a conversation of its
  own, `/export` of the open transcript — all free (they key off "has a run").
- **Live (stage 2)**: `ChildStarted(run)` fires at dialogue start (the run
  carries `participants`, so the viewer labels sides immediately);
  `ChildRoundFiled` per landed message; the token-level partial
  (`ChildStep`/`child_partial`) gains one bit — **which side is streaming** —
  so the in-flight bubble renders on the correct side with the correct name.
  That is the only new event surface; everything else (`view()`'s third arm,
  `switch_within_turn`, `TranscriptGrew`) is already generic over the child.
  Stage 1 lands the whole run at `handle_done` exactly as `call_subagent`'s
  stage 1 did; a cancel lands the partial transcript with
  `outcome = Cancelled`.

### 3.8 Budgets, limits, outcomes

| Limit | Source | On hit |
|---|---|---|
| `max_messages` (arg, default 16, cap 64) | per call | `RunOutcome::RoundLimit`; result says the cap fired and names the argument |
| whole-run timeout | **new** `tools.dialogue_run_timeout_secs`, default 1800 s (additive `#[serde(default)]` — no settings migration) | `TimedOut`, partial transcript lands |
| per-message reply cap | `tools.subagent_max_tokens` (shared with sub-agents) | the message ends at the cap, dialogue continues |
| engine failure mid-run | — | `Failed`, partial lands (retry/backoff already wraps the backend below us) |
| `Esc` / chat switch | parent's cancel → child token | `Cancelled`, partial lands |
| director stop | `dialogue_stop` | `Completed` |

No new `RunOutcome` variants — the five existing ones map exactly, which is
what "room left on purpose" bought. The run timeout gets its own knob because
the honest default differs by an order of magnitude: a sub-agent run is a few
rounds (600 s), a 16-message dialogue on a local 27–31B model is ~25 sequential
requests at tens of seconds each.

### 3.9 Sequentiality and the cost model (R5, R7)

The executor is one `async` loop inside the turn: participant turn, maybe a
checkpoint, participant turn, … — every request `await`ed to completion before
the next is built. **At most one engine request is ever in flight**, on the
turn's one backend; a local `llama-server` therefore serves the whole feature
with one loaded model and zero extra VRAM. This is not an added guard but the
absence of any concurrency to guard against; the invariant is stated in the
tool's spec section and pinned by a unit test on the mock backend (max
in-flight = 1).

Requests per dialogue: `M + ceil(M / K) + R + 1 (+ 1)` — messages,
checkpoints, retries, the landing auto-title, plus at most one summarization
request when the director's brief has to fold a long uncompacted
conversation (§3.2). Defaults (16, 2): ~25 requests. Each context
is append-only, so on llama-server the three prefixes are individually
cacheable (§2.5); whether the 4-slot LRU actually holds all three alongside
the parent is a probe measurement, not an assumption. Worst case is full
re-prefill per request — linear in transcript length, accepted by R7 and said
plainly in the tool's description and spec section.

### 3.10 i18n

Axis A (agent language, `ctx.loc` / the run's locale): the tool description
and parameter descriptions, the verdict tools' schemas, the director appendix
(`prompt.dialogue.*`; the brief reuses the compaction summarizer's existing
localized prompt), participant fallback labels, every result string
(stopped/cap/timeout/cancelled). Axis B: nothing new — the viewer
reuses the child-view chrome; the system bubble shows the stored briefs.
Both bundles ship together as always; the no-dead-key and localization gates
cover the new families.

## 4. Forks

**F1 — where the moderator lives.** *(the central one)*
  a) **In-run scripted checkpoints (recommended).** The executor alternates
     participants and, every K messages, asks the director context for a
     verdict expressed as tool calls (§3.3). Deterministic control flow, a
     fixed budget (`M + M/K`), no round-budget entanglement, testable without
     a model; matches the ask's "three conversations taking turns" literally.
  b) In-run **agentic** director: a child `TurnLoop` whose tools are the
     verdict set, participants generated inside its tool executions. More
     freedom (the director could deliberate in prose between actions) at the
     cost of: its rounds burn `max_tool_rounds` (default 8 — a 16-message
     dialogue cannot fit), its prose needs a home in the transcript, and the
     known narrate-instead-of-call failure mode (lessons §9) sits in the
     control path instead of a fallback-able checkpoint.
  c) Parent-turn stepwise tools (`dialogue_next`/`dialogue_stop`/… called by
     the parent assistant across its own rounds). Strongest reading of "the
     main agent is the moderator" — and the worst fit: each step burns a
     parent round and re-processes the parent's full context (the largest
     prefix in the app) per dialogue message; every message lands in the
     parent's history twice; the transcript would span many call records,
     breaking "one call, one transcript" that every viewer/search/deletion
     seam keys off. The parent still *authors* the direction in (a) — via
     personas, `direction`, and reading the result.
  d) No moderator, `max_messages` only (§3.14's literal sketch) — fails R2.

**F2 — transcript encoding.**
  a) **Role-encoded two-sided (recommended):** a = `Assistant`, b = `User`
     (§3.5). Zero message-shape changes; feed, search, export, `chat_read`
     work today; names via the existing per-chat header resolution.
  b) All messages `Assistant` + new `Message.author` (§3.14's sketch): cleaner
     semantics, generalizes past two participants — and needs a feed
     projection keyed on author, search/snippet role handling, and an export
     story. Deferred as the growth path.

**F3 — the director's editing powers (R3).**
  a) Verdict-only (`continue`/`stop`).
  b) + steering: `dialogue_note`, `dialogue_retry(note)`.
  c) **(recommended)** = b + `dialogue_rewrite` of the **last** message — the
     cheapest true edit (no request), covering "that line broke character —
     fix it and move on".
  d) = c + editing any earlier message by index — deferred: it invalidates
     everything generated after it (§3.4).

**F4 — participants' tools.** **None in v1 (recommended).** The ask is
dialogue writing; tools would put `assistant(tool_calls)` rounds under a role
swap — the orphaned-call shape Anthropic 400s on — and multiply cost. The seam
if ever wanted: derive the *other* side's view from final texts only (a
speaker's tool rounds are its private kitchen), which dissolves the
alternation hazard; recorded here, not built.

**F5 — knobs.**
  a) **(recommended)** `max_messages` (default 16) and `moderate_every`
     (default 2) as call arguments; one new setting
     `tools.dialogue_run_timeout_secs` = 1800; reply cap shared with
     sub-agents.
  b) Reuse `subagent_run_timeout_secs` (600 s) — wrong by an order of
     magnitude for a local dialogue (§3.8).

**F6 — the director's identity.** **User's decision (2026-08-28), replacing
the recommendation:** the director **is the main agent** — it keeps the
parent chat's persona and it **knows the conversation with the user**,
delivered as a brief built by the `/compact` summarizer's mechanism (§3.2).
The originally recommended neutral built-in director prompt was rejected: the
director's stop/steer judgment should be informed by *why* the user wanted
this dialogue, which lives in the conversation, not only in `direction`.
This does not contradict ADR 0010's identity rule — the *participants* remain
foreign personas with their own system messages; it is the director that was
never foreign. The self-model injection stays top-turn-only (ADR 0010 F3,
the user's own earlier decision); flag it if the director should carry it
too.

**F7 — the opening.** **(recommended)** `opening.text` is required, authored
by the caller in the opener's voice (the calling model writes good openers,
and the first generated message then has a `user` turn to answer). Alternative
— generating the opening from the persona alone — adds the empty-conversation
template risk for no gain. `scene` stays optional for shared setting.

**F8 — schema.** **(recommended)** `RunKind::Dialogue` + `CHAT_SCHEMA` 2→3
no-op bump (§3.5) — the designed downgrade route. Alternative — encoding a
dialogue as `kind: "subagent"` plus extra fields so old binaries still open
the file (degraded, unlabeled) — trades a clean refusal for a silent
misreading; against ADR 0006's spirit.

**F9 — the profile toggle's default.** **(recommended)** enabled by default,
like `call_subagent` — the model only reaches for it when a dialogue is asked
for, and the description states the cost. Alternative: off by default (the
cross-chat-search precedent) if the user prefers explicit opt-in per profile.

**F10 — staging.**
  a) **(recommended)** Spike probe (§5) → PR 1: entities + schema step +
     executor + landed viewing + result + i18n + tests + live smoke → PR 2:
     live view (§3.7) + polish + docs sweep. Small enough that PR 1 and 2 may
     share a branch if the track lands quickly.
  b) Everything in one PR — against AGENTS.md §2's one-stage-per-PR.

An ADR (0011: the dialogue as a directed multi-context run on the sub-agent's
record) goes in with PR 1, amending 0010's map.

## 5. MVP probe (go/no-go, before any product code)

A spike (`spike/dialogue-probe`), the `continue-generation.md` §7 pattern: a
standalone `#[ignore]` test driving `OpenAiClient` directly with the §3.2
derivation and §3.3 checkpoint logic inlined — no product seams touched.

Fixture: two personas with a **naturally finite** task (e.g. a barista and a
customer resolving a wrong order; direction: "stop once they part amicably"),
`max_messages 16`, `moderate_every 2`; a second fixture with a mild conflict
to check steering (`dialogue_note` changes the next line's behaviour). The
director fixture is shaped per the amended F6 — a small parent persona + a
hand-written conversation brief + the director appendix — so the prompt shape
under test matches the product's. One deliberate divergence: the probe's
checkpoints are **stateless** (full script re-sent each time, earlier
directions restated in the prompt) — hand-building the persistent tool-call
history through the client would test the harness, not the model; the
persistent form only changes what the wire replays, and lands with PR 1.

Models: the live gate's pair — Gemma 4 31B and Qwen 3.6 27B (the same stack
the sub-agent smokes ran on), n = 5 per model per fixture. Cloud spot-check:
one run each on Anthropic and Gemini (keys exist) to exercise the wire
merging under the swap.

Measured, with go bars:

1. **Template acceptance**: 0 request rejections (the assistant-first
   prologue fix holds on Gemma's jinja).
2. **Role fidelity**: no message speaks both parts or answers as the wrong
   persona (read the raw transcripts — lessons §3: reconstruct a cell by
   hand); ≥ 9/10 clean per model.
3. **Termination**: the director stops before the cap in ≥ 4/5 runs of the
   finite fixture, at a point a reader agrees is an ending.
4. **Verdict compliance**: ≥ 95% of checkpoint replies carry a parseable
   verdict call (the prose-fallback rate); thinking-muted, per §2.4.
5. **Cost**: tokens, wall-clock, and `timings.cache_n` per request — the
   §3.9 slot question answered with numbers.

No-go contingencies, pre-committed: verdict flake → force the verdict via
`tool_choice`/grammar or reshape the checkpoint as a two-option closed
question; role bleed → name-prefix the user-side lines (`Bob: …`) and
re-measure; Gemma template refusal despite the prologue → merge the prologue
into the opening message instead. If none of the three rescues it on either
gate model, the feature stops here and this document records why.

### 5.1 Results — Gemma arm (2026-08-29): GO

Stack: `gemma-4-31B_q4_0-it` on the live `llama-server`
(`spike/dialogue-probe`, `src/shared/api/dialogue_probe.rs`; the probe wraps
its client in `RetryBackend` exactly as `live_backend()` does — the first run
died on a stale pooled connection, the back-to-back-load flake of lessons §9,
which is a transport fact and not a template rejection).

| fixture | stopped by director | fallbacks | bleed | steering | cost |
|---|---|---|---|---|---|
| finite (café), n=5 | **5/5**, at 6–8 msgs of 12, reasons correct | 0/17 | 0 | none needed | 42–59 s, ~3.5k+1.7k tok |
| steering (haggle), n=5 | **4/5** (one honest cap on near-incompatible personas) | 0/24 | 0 | 15 notes, followed | 99–152 s, ~8.4k+4.4k tok |
| editing (poet), n=2 | **2/2**, reasons verifiable (day+hour+spot) | 0/12 | 0 | 4 retries, 4 rewrites | ~140 s, ~6k+5.4k tok |

Go bars: template acceptance — 0 rejections in ~180 requests (the prologue +
merge derivation holds on Gemma's jinja); role fidelity — 0 heuristic hits
and the read transcripts are clean (no side-speaking, no narration, personas
held to the last line — Rita: "Seven. Got it. Now, if you're done, I've got
floors to mop."); termination — 11/12 director stops at points a reader
agrees are endings; verdict compliance — **53/53** checkpoint replies carried
a parseable call; five transport retries absorbed by the decorator.

**Cache slots (the §3.9 question): held.** Alternating the three contexts on
the default server, every context reuses its prefix from its second visit —
A `cache_n=146` of 161, B 145, D 106, and it persists across rounds. The
worst case (full re-prefill per alternation) did not occur; a dialogue costs
what its appended tails cost. Wall per request: participant 6.5–15.9 s avg,
director 1.6–3.1 s avg.

**Two design rules found live** (both folded into §3.3/§3.1):

1. **The all-thinking empty turn.** Gemma 4 emits `reasoning_content`, and an
   instruction conflict in a participant's effective system — first a
   self-contradictory fixture persona, then, structurally, a director's
   one-shot retry note fighting the persona's own format rule ("always three
   sentences" vs "one sentence for the day") — sent it into unbounded
   deliberation: 1536/1536 tokens of thoughts, empty text, four times. The
   muted re-ask (`reasoning_budget: 0`) recovered **4/4**; the executor
   adopts that rule, and the tool's description will tell the calling model
   that a persona should carry its own line-format clause and that a retry
   note argues *against* the persona — `dialogue_rewrite` is the strong edit.
2. **The escalation ladder is real.** Unprompted, the director used exactly
   the intended ladder: notes while steering preferences (the haggle — 15
   notes, obeyed), retry-with-note against a style, and — when the persona
   won anyway — rewrite, in the character's register ("Thursday. The back
   corner, by the window."). `retry` losing to a strong persona is not a
   defect; it is why `rewrite` exists.

**Cloud spot-check (same day): GO on both.** One finite run each through the
real cloud clients wrapped in the retry decorator — `claude-haiku-4-5`:
8 msgs, director stop, 0/4 fallbacks, 14 s wall, 7.5k+0.5k tokens;
`gemini-2.5-flash`: 10 msgs, director stop, 0/5 fallbacks, 14 s. The swap
survives both strict-alternation wires end-to-end, verdict tools parse on
both, and both directors also used a steering note unprompted. One style
slip, not a bleed: haiku's Mara emitted one `*nods…*` stage direction
against the persona's rules — line discipline is the caller's persona text,
not the mechanism, and the doc's §3.1 note (the tool description teaches the
caller to write a line-format clause) is where that lives. Verdict
compliance across every arm: **62/62**.

Pending: the Qwen 3.6 arm (the stack rotates on request) — the checkpoint
mute and the participant thinking budget are the things to watch there.

## 6. Test plan

Unit (mock backend, no server):
- Derivation: swap + prologue + merge produce strict alternation from every
  transcript shape (opening by a / by b, with/without `scene`, after notes,
  after a rewrite); director notes reach exactly their target's appendix;
  System interventions never leak into participant views.
- Loop: turn order alternates; checkpoint cadence; retry replaces and counts;
  rewrite costs no request; stop/cap/timeout/cancel/engine-failure map to the
  five outcomes; the no-call fallback continues; **max one in-flight request**
  asserted on the mock.
- Verdict parsing: each tool, several-in-one-reply ordering, malformed args.
- Storage: round-trip of a dialogue run; `CHAT_SCHEMA` 3 step is a no-op on
  v2 files and old binaries' refusal path is the version guard (mutation-test
  the gate both ways — lessons §2); sub-agent runs read back with empty
  `participants`.
- Viewer: labels from `participants`; Note rows for interventions; refusals
  unchanged. Render one real frame and look at it (lessons §2).
- Search/`chat_read`: a dialogue transcript is a conversation of its own.

Live `#[ignore]` (the mandatory gate, AGENTS.md §3):
- `dialogue_e2e_live`: a 6-message capped dialogue with a planted natural
  ending; asserts the tool was actually called (lessons §2), alternation held,
  the transcript landed with `Completed` or `RoundLimit`, the result cites the
  `chat://` address. Run on both gate models before the PR; the probe's
  fixtures graduate into this smoke.

## 7. Scope and documentation

Touch list (per AGENTS.md §4): spec — a roster row in §9.3 and a new
**§9.13** (the tool, the derivation rule, the sequentiality invariant, the
budgets); architecture — a `run_dialogue` bullet beside `call_subagent` in §8
and a line in §5's loop notes; `docs/journal/tools.md` entry per PR;
CHANGELOG (Added + Data for the schema bump); roadmap — the §3.14 pointer
retires; ADR 0011; README's tool table row. No new keys or commands — the
help overlay is untouched.

Size estimate: the executor + derivation is the only genuinely new logic
(~400–600 lines with its tests); everything else rides existing seams —
entities (+2 small types, +1 variant, +1 field), one schema step, one config
knob, catalog + i18n entries, viewer labels, and in PR 2 one progress-event
field. Comparable to a mid-sized post-M9 track; well under the sub-agent
track's 8 PRs because that track already built the hard parts.

## 8. Open questions

- Whether `moderate_every: 1` should also place a checkpoint **before** the
  first generated message (a director pre-brief). Leaning no — the personas
  and `direction` are the pre-brief.
- Whether the director's `summary` should also become the run's closing
  `System` note verbatim (recommended yes — the transcript then explains its
  own ending to a reader who never sees the parent chat).
- TTS groundwork noted in passing: a two-persona transcript is the natural
  consumer of per-speaker voices (`docs/research/tts.md` §13's multi-speaker
  note); nothing here blocks or builds it.
