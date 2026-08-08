# Research: conversation history compression (rolling summary)

> Research for roadmap §"Context and tokens" item #1 ("Most valuable next").
> Status: forks decided by the user 2026-08-07 (§8) — F8(c), F9(b), F10 with a
> full off switch, the rest as recommended. **The track is complete**: stage 0
> (the probe, GO — §9a), stage 1 (core), stage 2 (the automatic trigger —
> sub-decisions §10.1, and what it uncovered — §9b), stage 3 (the read-back
> tools — sub-decisions §10.2).
> Prepared 2026-08-07, branch `docs/history-compression-research`.
> Closely related roadmap item: #2 **prompt caching** — the two must compose (§3.3).

The ask: the whole conversation is sent to the engine on every request; local
models with an 8k window hit a hard ceiling. Research auto-summarizing old
messages past a token threshold ("rolling summary"), next to the existing token
counter.

---

## 1. The problem today (baseline)

### 1.1 The request carries everything, and nothing guards the ceiling

`build_request` sends **all** of `chat.messages`, unconditionally
([request.rs:71-77](../../src/app/orchestrator/request.rs)): no windowing, no
cap, no overflow check anywhere in the codebase. The only history truncations
that exist are user-initiated (`Ctrl+R` regenerate, `Ctrl+E` delete exchange —
`generation.rs:114,154`). Character-budget truncation exists only in
`truncate_middle` (`rename_chat.rs:48`), used solely for the title digest —
never on the path to a real turn.

Nothing checks `prompt_tokens + max_tokens ≤ context_size`. The token counter
in the status bar *shows* the approach to the ceiling but nothing acts on it.

### 1.2 What actually happens at the ceiling, per provider

The llama.cpp row was **measured in stage 0** (§9a) — against the live stack
*and* against the C++ source; the cloud rows are from provider docs:

| Provider | Behavior at overflow |
|---|---|
| llama-server (default) | **HTTP 400** *before* any SSE, body `{"error":{"code":400,"type":"exceed_context_size_error","message":"request (N tokens) exceeds the available context size (M tokens), try increasing it","n_prompt_tokens":N,"n_ctx":M}}`. Measured. Context shift is **disabled by default**. |
| llama-server + `--context-shift` | **Does not rescue an oversized prompt** — same 400 (measured). It only applies *during generation*: with `n_keep=0` the discard window starts at position 1, so the **system prompt goes first**, along with ~half the window. |
| OpenAI Responses | `truncation` defaults to `disabled` → **400** when the model's window is exceeded (`"auto"` would drop middle items; we don't send the field). |
| Anthropic | **400** `invalid_request_error`: `prompt is too long: N tokens > M maximum`. |
| Gemini | **400** `INVALID_ARGUMENT` on the token limit. |

Our client does not swallow error bodies (a long-standing rule): `client.rs:110-122`
puts the first 500 characters of the body into the error text, so the user
**already sees the real reason** — but as raw JSON inside a generic
`ui.err.generation_failed` wrapper (`generation.rs:1134-1139`), with no hint of
what to *do*. Today's recourse: start a new chat, `Ctrl+E` away exchanges by
hand, or raise `-c`.

**A quieter failure worth naming**, since compression does not fix it and it is
easy to confuse with one: when the *prompt* fits but prompt+generation reaches
`n_ctx`, llama-server does not error — it stops generating and reports
`finish_reason: "length"`, which on the OpenAI path is **indistinguishable from
hitting `max_tokens`** (the native `truncated` flag is not exposed there).
Measured, §9a M6.

### 1.3 The hidden bulk: tool messages

`Tool`-role messages **are persisted** into `chat.messages` and replayed into
every subsequent request (`message_to_api`, `request.rs:27-30`) — while being
**hidden from the feed** (`FeedMessage::from_message` returns `None` for them;
the feed renders results from `ToolCallRecord.result` on the assistant message
instead). A `fetch_url` page, a `python_exec` dump, an MCP result (clipped at
20k chars) all live in history forever, at full token cost on every turn, with
no UI visibility of that cost. Tool-heavy chats hit the ceiling far sooner than
their visible text suggests. This is the single biggest compression target.

### 1.4 Who hits this

- **Managed / external llama-server** — the primary pain: `-c` 8k–16k
  (`DEFAULT_CONTEXT_SIZE = 8192`, the dev stack runs 16384). A ceiling.
- **Cloud** — 200k–1M windows; the ceiling is distant, but every turn re-sends
  the whole history, so the motivation there is **cost**, not breakage.

---

## 2. Constraints the design must respect (the project's own invariants)

1. **The feed shows everything; nothing is destroyed.** Soft delete is the
   house philosophy (spec §12.3), and `Chat.deleted` exists precisely so even
   removed content survives. Compression must therefore change **what the
   request carries**, never what `chat.messages` holds or what the feed shows.
   This single decision dissolves most of the hard problems: search, export
   (`F5`), TTS, the reflection watermark, content indexing — all read
   `chat.messages` and are untouched.
2. **The orchestrator is the sole owner of `Chat`** — summarization is a
   background task that returns its result through an internal channel and the
   orchestrator applies it (the `title_tx` pattern).
3. **Prefix cache (spec §6.6).** "History is append-only" (§6.2) is the
   load-bearing assumption. A compaction rewrites the start of the prompt →
   one **deliberate full re-prefill** on the next turn — the same class as
   `set_system_message` ("the only tool that invalidates the chat's prefix
   cache (justified)", spec §9.5). The ongoing win is a *shorter* prefix. The
   invariant becomes "append-only **between compactions**"; §6.2 and §6.6 need
   a bullet. This also composes with roadmap #2 (provider prompt caching):
   compactions are rare threshold events, not per-turn churn — a cache
   breakpoint survives between them.
4. **Protocol correctness on rewritten history.** An orphaned
   `assistant_tool_calls` without its tool result 400s on Anthropic (strict
   alternation) and violates the contract's ordering; Gemini 3 requires
   `thought_signature` replay on historical calls. Both hazards are avoided
   *entirely* by (a) cutting only at **user-message boundaries** (an exchange
   never splits) and (b) feeding the summarizer a **text digest**, never a
   message array (the title digest precedent) — so no rewritten message array
   exists anywhere.
5. **Complementary to the memory tracks, not a substitute.** Notes /
   self-model hold durable, cross-chat facts (and already move them out of the
   conversation); the summary holds **chat-local conversation state** — what
   was asked, decided, produced *in this chat*. RAG holds the user's corpus.
   None of the three covers "the middle of this conversation, verbatim-ish".
6. **i18n**: the summary and its block header are read by the *model* →
   **axis A** (profile language, `profile_locale`), key convention
   `prompt.compact.system` + an all-langs test. Error strings → axis B.

---

## 3. The capability ladder

### (a) Do nothing / raise `-c`
Not free: KV memory scales with context, and 8k *is* the honest capacity of
many local setups. The error remains unexplained. Baseline, not an answer.

### (b) Hard truncation (sliding window, no model call)
Drop or stop sending messages older than a token window. Cheap, deterministic,
no engine call. But it is **silent amnesia**: the model loses the beginning of
the conversation with nothing carrying the gist forward — the user asked "what
did we decide at the start?" and the model confabulates. This is what LM
Studio/Ollama do, and it is the behavior users complain about. Rejected as the
*mechanism*, but its shape (a protected recent tail) survives inside (c).

### (c) Summarize-and-retain (rolling summary) — **recommended**
Past a threshold, a background task summarizes the oldest part of the
conversation; the request then carries `system + [summary block] +
messages[upto..]`. Messages are retained (constraint 2.1); the summary is
re-rolled forward as the chat grows (`new = summarize(old_summary + next
chunk)`). Bounded input by construction — which matters, because **the
summarizer has the same context limit as the chat**: "re-summarize everything
from scratch" does not fit, by definition of the situation that triggered it.
Rolling is not just cheaper — it is the only shape that always fits.

### (d) Compact-into-new-chat (manual)
"Start a new chat seeded with a summary." Simple, but it forks the
conversation identity: history, search, the feed all split across two chats.
Useful as a *manual escape hatch* later; not the mechanism. Claude Code offers
both (c)-style `/compact` and this.

### (e) Retrieval over old history
Embed old exchanges into a chat-scoped index (the infrastructure exists —
attachments already have exactly this, vec0 partitioned by `chat_id`) and
inject top-k relevant fragments per turn. Rejected as the primary mechanism
for the same reason the file-attachments research rejected RAG-instead-of-
inline (§3 there): **retrieval ≠ guaranteed context** — the model doesn't
know what it wasn't shown, and top-k per turn also rewrites the prompt every
turn (kills the prefix cache). But as a **complement** to (c) — read-back
tools over the compressed range, the `attachment_read`/`attachment_search`
shape — it turns compression from lossy into *paged*: the summary carries the
gist, the tools reach the verbatim text on demand. See F9.

---

## 4. Prior art (one line each)

- **Claude Code `/compact`** — client-side (c): auto at a context threshold +
  manual command with optional instructions; structured summary replaces
  history. The closest relative to what fits here.
- **OpenAI Responses `truncation:"auto"`** — server-side middle-drop; we run
  `store:false` and don't send it. Server-side (b), with (b)'s amnesia.
- **Anthropic / Gemini** — no server-side compaction; clients do it. Anthropic
  ships prompt caching (roadmap #2's other half).
- **LM Studio / Ollama / llama.cpp `--context-shift`** — variants of (b);
  Ollama's silent truncation is a running complaint.
- **MemGPT-style paging** — (e) taken to an OS metaphor; research, not a fit
  for a TUI chat's complexity budget.

---

## 5. What the codebase already gives us (mechanics map)

Verified against source 2026-08-07; file:line references.

### 5.1 The request path
- `build_request` (`request.rs:58-77`): `system = chat.system_message` →
  `inject_attachments` (sync, in `build_request`); `inject_self_model` is
  appended **inside the spawned task** (`generation.rs:613-622`) because
  relevance selection needs async embedding. Final system layout:
  `persona → attachments block → self-model block(s)`.
- `GenSpawn` (`generation.rs:430-467`) carries no `Chat` reference and no
  context size — the task never touches `Chat`.
- Within a turn the loop appends `assistant_tool_calls` + tool results to the
  request and re-sends the whole thing per round; domain messages (assistant
  **and tool**) accumulate in `GenResult.messages` and are pushed to
  `chat.messages` in `handle_done` (`generation.rs:369-371`).
- The `handle_done` tail (`generation.rs:399-401`) is where
  `maybe_auto_reflect` / `maybe_auto_consolidate` / `maybe_auto_self_consolidate`
  already hang — the natural slot for `maybe_auto_compact`.

### 5.2 Token accounting — the trigger's inputs
- Exact `prompt_tokens` arrives per round via `ChatChunk::Usage`
  (`stream_round`, `generation.rs:1112-1126`) and is emitted to the UI — but
  it lives **only** in `ChatScreen.gen_context`, wiped on chat switch and on
  each generation. **The orchestrator has no token count today**, and
  `GenResult` doesn't carry usage. The trigger needs `GenResult` to gain the
  last round's `prompt_tokens` (+ `completion_tokens`: the next turn's prompt
  ≈ prompt + completion + the new user message + injection deltas).
- The estimate fallback `estimate_prompt_tokens` (`generation.rs:1163-1172`,
  bytes/4 via `shared/tokens.rs`) has a known blind spot: it does **not**
  count tool schemas (`req.tools`) — significant with MCP servers enabled.
  The exact figure from `usage` includes everything; prefer it.

### 5.3 Context-budget knowledge
- **Managed**: `config.engine.managed.context_size` (`config.rs:221`, default
  8192), passed as `-c`.
- **External / cloud**: no context field anywhere, no provider→window table.
- llama.cpp fills the external gap: **`GET /props`** returns
  `default_generation_settings.n_ctx` (sourced from the server README; the
  per-slot vs global nuance and availability on our stack to be verified in
  stage 0). Third-party OpenAI-compatible servers won't have it → an explicit
  setting stays the fallback. For the cloud, model names are free-form — we
  don't guess windows; an explicit setting or "inactive".

### 5.4 The precedents to copy
- **Watermark**: `Chat.reflected_upto`/`reflected_at`
  (`chat.rs:56-68`) + clamping on read (`reflect_window`,
  `reflection.rs:76-81`) + "advance only after every gate passes" ordering
  (`maybe_auto_reflect`, `reflection.rs:144-291`) — the exact shape for
  `compacted_upto`.
- **Background single-turn task returning text**: `title.rs` — its own typed
  channel (`TitleResult`/`title_tx`), timeout + cancel, **`reasoning_budget:
  Some(0)`** to mute baked-in thinking (`title.rs:68-87`), and
  `salvage_title_source` (recover output from thoughts when `content` came
  back empty) — the same failure mode applies to a summarizer.
  `SilentLoop` (`tool_loop.rs`) is the *wrong* fit: it is a tool-calling loop
  whose `done_tx` carries only `Result<(), String>` — no way to return text.
- **Digest builder**: `build_conversation_digest` (`rename_chat.rs:27-44`) —
  roles labeled via `digest.role.*`, char budget, `truncate_middle`. It drops
  `Tool` messages and thoughts entirely; the summarizer input needs a variant
  that includes tool activity compactly (name + clipped args/result) — that
  is where the §1.3 bulk actually gets compressed.
- **One-at-a-time + failure streak**: `BackgroundKind` slots
  (`background.rs`) — a `Compaction` variant gives the gate, the quiet
  status-bar chip and the 3-strike error alert for free.
- **Additive `Chat` fields**: the §2-documented serde pattern (ADR 0006 F12) —
  no migration.

### 5.5 A trap confirmed: System-role messages don't exist
Nothing ever stores a `MessageRole::System` in `chat.messages`, and both
`message_to_api` and the feed **drop** the role defensively. A summary
implemented as a System message inside `messages` would silently vanish from
both the request and the UI. The summary must be a **separate `Chat` field
spliced in `build_request`** (or a marked User/Assistant message — rejected:
role semantics muddy, needs a new display flag, pollutes every consumer that
iterates messages).

---

## 6. Proposed shape (sketch, not a commitment)

### 6.1 Data
```rust
/// On Chat — additive, no migration (ADR 0006 F12):
#[serde(default, skip_serializing_if = "Option::is_none")]
pub compaction: Option<Compaction>,

pub struct Compaction {
    pub summary: String,        // the rolling summary text (model-generated)
    pub upto: usize,            // messages[..upto] are covered by the summary
    pub boundary_id: Uuid,      // id of messages[upto] at write time — staleness guard
    pub compacted_at: DateTime<Utc>,
    pub rolls: u32,             // how many times the summary was rolled forward
}
```
`chat.messages` is never touched. `upto` always points at a **user** message
(an exchange never splits — §2.4). Clamped on read like `reflect_window`; if a
history truncation ever cuts *below* `upto` (practically unreachable — `Ctrl+E`
works at the tail), clamp and re-roll on the next trigger.

### 6.2 Request
`build_request` becomes: `messages: chat.messages[upto..]` + a summary block
appended to `system` right after the persona (before attachments):

```
[persona]
\n\n [summary block: header + summary text]   ← changes only on compaction
\n\n [attachments block]
\n\n [self-model blocks]                      ← appended in-task, as today
```

The header (axis A, `prompt.compact.*`) frames the block as **data, not
instructions** (the attachments precedent) and — the thrice-learned house
lesson — **states what is and isn't reachable**: stage 1 wording says plainly
that older messages were summarized and their verbatim text is not
retrievable; the stage-3 read-back tools (F9b) change that sentence to name
them. A block that describes a situation without saying what's possible sends
the model improvising (`fs_read`, `web_search`, six `python_exec` rounds — the
journal has three case studies).

### 6.3 The roll
Trigger (auto or `/compact`) → pick the cut: the largest user-message boundary
that still leaves a **tail** of ≥ `tail_budget` tokens verbatim → build the
summarizer input = previous summary (if any) + a **digest** of
`messages[upto..cut]` (roles labeled, tool calls as compact lines, thoughts
excluded) sized to an input budget derived from the known context → one
single-turn request (title pattern: no history, no tools, `reasoning_budget:
Some(0)`, `max_tokens = summary_budget`, timeout, salvage) → result returns
via a typed channel → the orchestrator verifies `boundary_id` still matches
(the chat may have been truncated meanwhile), writes `Compaction`,
`mark_dirty` (no `modified_at` bump — the reflection precedent). If the window
exceeds one input budget, the task **loops rolls** until caught up — each roll
bounded, which is what makes this work on any window size.

The summarization prompt asks for a structured summary that **preserves exact
identifiers, numbers, names, and decisions verbatim** — the planted-fact probe
(§9) is the direct test of exactly this, and it passed (§9a).

Two requirements the probe turned from guesses into measured facts (§9a):
the prompt must state a **length limit in words** (a bare `max_tokens` cap
truncates mid-sentence instead of making the model prioritize), and the roll
template must instruct it to **drop what later parts superseded** — otherwise
the summary grows with every roll, which defeats the point. `max_tokens` stays
as a safety net well above the stated limit, and `finish_reason == "length"` is
treated as "this summary was cut", not as success.

### 6.4 The trigger
In the `handle_done` tail: `next_prompt_estimate = last exact prompt_tokens +
completion_tokens` (from `GenResult`, newly carried) with the byte-estimate
fallback; compact when it exceeds `threshold_pct` (default ~75%) of the
**resolved context budget** minus a reply reserve (effective `max_tokens`, else
a constant). Budget resolution:

| Mode | Budget source |
|---|---|
| managed | `managed.context_size` (already known) |
| external | `/props` → `default_generation_settings.n_ctx` (measured §9a M1; **read it directly, never divide by `total_slots`**) → the `n_ctx` carried in a 400 body → explicit setting → feature inactive |
| cloud | explicit setting → feature inactive (motivation there is cost, not a ceiling) |

**The 400 body is itself a discovery channel** (§9a M3): it carries `n_ctx` and
`n_prompt_tokens` as numbers, so even a server with no `/props` teaches us its
budget the first time we overrun it — "learn it from the failure". That makes
the fallback chain end in something better than "inactive" for any llama.cpp
server.

**Which token number the trigger uses matters more than expected.** The exact
`usage.prompt_tokens` is the primary input; the `bytes/4` estimate is a coarse
fallback only, because its error **changes sign by content type** (§9a M9): it
overestimates Russian prose by 68% and *underestimates* code by 20% and JSON
tool results by 7% — i.e. it is unsafe exactly on the tool-heavy chats that
overflow first (§1.3). A margin on the estimate is therefore not optional.

Compacting at ~75% rather than at the wall leaves headroom for the user to
keep typing while the background roll runs — the same reason reflection
doesn't block the turn. If the user still outruns it and the server 400s, the
error hint gains a pointer at `/compact` (today it is a bare
`generation_failed`).

### 6.5 UI
- **Feed divider + collapsible summary** at the boundary (F8c): a muted line
  at index `upto` — "older messages are summarized for the model" — carrying
  the summary itself as a foldable block that **expands together with the CoT
  ("thoughts") blocks**: it reads the existing per-chat `FeedView.thoughts`
  flag (`Ctrl+T`), so no new hotkey and no new `FeedView` field. Collapsed by
  default, like every foldable block. The feed already maps message ids
  (`FeedMessage::message_ids`); no new plumbing class.
- The token counter visibly drops after a compaction — no extra chip needed;
  the roll itself can show the quiet `BackgroundKind` chip like reflection.
- `/compact` joins `HELP_COMMANDS`, the input-box command highlight, spec
  §11.7 / README tables.
- Settings: "Memory" section, a new "Context" group — enabled, budget/
  threshold, summary budget, tail budget.

### 6.6 What deliberately does NOT change
Search (FTS + in-feed), export `F5`, TTS, clone, import, backup, the
reflection watermark, `Chat.deleted` — all read `chat.messages`, which
compression never edits. Impersonation builds its own request over full
history and is **out of scope** for stage 1 (groundwork: reuse the same
summary).

---

## 7. Cost and quality, honestly

- **One-time**: each compaction is a full re-prefill next turn (§2.3) plus one
  background generation over ~digest+summary tokens (tens of seconds on a
  local stack — background, invisible unless watched).
- **Ongoing win**: every turn after it carries `summary + tail` instead of the
  full history — on an 8k window that is the difference between working and
  400.
- **Quality**: a summary is lossy; rolling (summary-of-summary over many
  rolls) degrades further. Mitigations: identifiers-verbatim prompt, the
  planted-fact probe as the go/no-go, the protected tail, and the memory
  tracks carrying durable facts independently. F9's read-back tools are the
  real answer for "I need the verbatim text back".
- **A local model summarizing itself**: Gemma 4 31B is a capable summarizer,
  but this is exactly what stage 0 must *measure*, not assume.

---

## 8. Forks — **decided by the user 2026-08-07**

F8, F9 and F10 were decided with refinements (spelled out below); everything
else — as recommended.

- **F1 — mechanism.** **Decision: (a).**
  - (a) **Rolling summary, messages retained; request-side only** —
    *recommended* (§3c: the only shape that always fits; every invariant
    survives untouched).
  - (b) Hard truncation window (no model call) — silent amnesia, rejected §3b.
  - (c) Compact-into-new-chat — an escape hatch, not a mechanism; groundwork.

- **F2 — where the summary sits in the request.** **Decision: (a).**
  - (a) **A block in `system`, after the persona** — *recommended*
    (System-role messages are dropped by every consumer — §5.5; system is
    already the home of injected blocks, handled by every provider wire).
  - (b) A synthetic first user message — muddy role semantics, alternation
    hazards, a new display flag.

- **F3 — one mechanism or two.** **Decision: (a).**
  - (a) **Summarization only** — *recommended*; the digest already compresses
    tool bulk (§1.3) on its way into the summary.
  - (b) A cheaper first rung — eliding old tool-result bodies before any
    summarization (no model call, feed unaffected since it renders
    `ToolCallRecord.result`). Real, but a second mechanism with its own
    corners; note as groundwork.

- **F4 — trigger.** **Decision: (a).**
  - (a) **Auto at threshold + manual `/compact`** — *recommended* (auto is
    what saves the 8k user; manual is the escape hatch and the test lever).
  - (b) Manual only. (c) Auto only.

- **F5 — context-budget source for external servers.** **Decision: (a).**
  - (a) **`/props` auto-discovery (llama.cpp), explicit setting as fallback**
    — *recommended*: the primary audience runs external llama-server (the dev
    stack itself does), and discovery removes the "changed `-c`, forgot the
    setting" footgun. Verified in stage 0.
  - (b) Explicit setting only — simpler, one more thing to keep in sync.
  - Cloud is an explicit setting either way, else the feature is inactive.

- **F6 — the summarizer engine.** **Decision: (a).**
  - (a) **The chat engine itself** — *recommended*: works fully local (the 8k
    audience *is* the local audience), no new provider, no new key. The ADR
    0009 out-of-band pattern (a separate cloud slot) buys nothing here.
  - (b) A separately configurable engine — config surface for a niche want;
    groundwork at most.

- **F7 — task shape.** **Decision: (a).**
  - (a) **Single-turn digest→summary, `title.rs` pattern** (own typed
    channel, timeout, salvage, `reasoning_budget=0`) — *recommended*.
  - (b) A `SilentLoop` tool-based loop — wrong fit: it cannot return text
    (§5.4), and a summarizer needs no tools.

- **F8 — visibility in the feed.** **Decision: (c), refined** — the divider
  carries the summary as a foldable block, and it **expands together with the
  CoT ("thoughts") blocks**: the same `Ctrl+T` / per-chat `FeedView.thoughts`
  state, no separate toggle and no new `FeedView` field (§6.5). Collapsed by
  default, like every foldable block.
  - (a) A muted divider at the boundary only.
  - (b) Nothing (token counter only).
  - (c) Divider + expandable summary view.

- **F9 — is the compressed range reachable?** **Decision: (b)** — the
  read-back tools are **in scope for the track**, as their own stage after the
  core lands (§10, stage 3). Until they land, the stage-1 block wording
  honestly says the verbatim text is not retrievable; once they land, it names
  them (§6.2).
  - (a) Summary only, with the block honestly saying verbatim text is not
    retrievable — the stage-1 boundary while (b) is being built.
  - (b) **`history_read`/`history_search` tools** over the compressed range —
    the `attachment_read`/`attachment_search` shape (pagination over a digest
    of old exchanges; the chat-scoped vec0 index infrastructure already
    exists). Turns compression from lossy into paged; the block then names the
    tools.

- **F10 — default state.** **Decision: (a), refined** — active where a budget
  is known and **on by default**, but the feature must be **fully switchable
  off** in settings. Recorded interpretation (to be confirmed on reading):
  a master toggle "History compression", default **on**; when **off**, the
  feature is inert — no auto trigger **and no splice**, `build_request` sends
  the full history exactly as today, and `/compact` refuses with a localized
  pointer at the setting. The stored `Compaction` **survives the flip** (the
  messages were never touched, so off→on→off is lossless in both directions).
  - (a) On by default where a budget is known (managed; external once
    `/props` answers), inactive where it isn't — what it prevents (a hard
    400 / silent shift) is strictly worse than what it does, and the divider
    keeps it visible. Auto-title is the "on by default" precedent; the
    off-by-default precedents (python, control tools, confirm) are risk
    gates, which this is not.
  - (b) Off by default, opt-in — condemns the default-config 8k user to the
    400 until they find the setting.

---

## 9. Stage 0 — the probe (go/no-go before any wiring)

> **Done, 2026-08-07 — verdict GO.** Results and what they changed: **§9a**.

The house pattern: measure before building. All against the live stack
(Gemma 4 31B q4 + the usual `llama-server`):

1. **Reproduce the baseline**: drive a chat past `-c` → confirm the 400 text
   and what the user actually sees. (Also pins the error-hint wording for
   §6.4.)
2. **`/props`**: confirm `default_generation_settings.n_ctx` on our server,
   and whether it is per-slot (`--parallel` divides `-c`).
3. **The planted-fact test** (the ZARYA pattern, the project's signature):
   build a conversation whose early part contains a planted identifier and a
   planted *decision*, exceed the threshold, roll the summary, then ask.
   - **Go criterion**: the model answers both from the summary alone —
     including after **two consecutive rolls** (summary-of-summary) — and the
     summary respects its token budget.
   - **No-go**: identifiers reliably lost → the summary alone is not honest
     enough as an interim state; re-scope — the F9(b) read-back tools move
     from stage 3 into the MVP, and/or the summarizer prompt/budget is
     reworked.
4. **Latency**: one roll's wall time on the live stack (informational — it's
   background — but it calibrates the threshold headroom).

---

## 9a. Stage 0 — measured results (2026-08-07): **GO**

Two stacks. **Live** — the project's usual gate stack: external `llama-server`
build `b9867-152d337fa`, `gemma-4-31B_q4_0-it.gguf`, `-c 16384`, `total_slots 1`.
**Local** — a CPU-only build `b9769-c926ad098` with `gemma-3-4b-it-q8_0.gguf` at
`-c 1024`, used for the server-behaviour measurements where a tiny context makes
overflow trivial to reach (that behaviour is a property of the server, not the
weights — and the load-bearing ones were then re-confirmed on the live stack).
The C++ answers come from a real checkout of the same project at `c926ad098`.

### Server behaviour

- **M1 — `/props` works and is enough.** Live: `default_generation_settings.n_ctx
  = 16384`, `total_slots = 1`, `build_info = b9867-152d337fa`, plus `model_path`
  and the chat template's capabilities. **F5's discovery path is confirmed.**
- **M2 — never divide by `total_slots`.** Locally, `--parallel 2 -c 1024` reports
  `n_ctx: 512` — the per-slot figure — and a 696-token prompt (fits 1024, not
  512) is genuinely rejected, so it is the real limit and not a display artefact.
  But the *source* shows the trap in the other direction: with **no** `-np` flag
  the server sets `n_parallel = 4, kv_unified = true`
  (`tools/server/server.cpp:109-114`), so `total_slots` is 4 while `n_ctx` is
  **not** divided. Dividing would then be wrong by 4×. Reading the field as given
  is correct in every configuration.
- **M3/M4 — the overflow error arrives as HTTP 400 *before* any SSE**, and this
  is deliberate: the handler blocks on the first task result and only switches
  the response into streaming mode if it is not an error
  (`server-context.cpp:4161-4174`, "in streaming mode, the first error must be
  treated as non-stream response … to match the OAI API behavior"). Measured on
  both stacks: `stream: true` and `stream: false` return **byte-identical**
  responses, `content-type: application/json`, no `data:` line at all. Live, with
  a 32 706-token prompt against 16 384: rejected in **0.9 s**, before prefill.
  The body is machine-readable — `type: "exceed_context_size_error"` plus numeric
  `n_prompt_tokens` and `n_ctx` — so nothing has to be parsed out of prose.
- **M5 — `usage` on a normal stream** arrives in a final chunk with an empty
  `choices` array: `{completion_tokens, prompt_tokens, total_tokens,
  prompt_tokens_details:{cached_tokens}}`. **`cached_tokens` is a gift for stage
  1**: it reports how much of the prompt was served from the slot's prefix cache,
  which is a direct, cheap way to *verify* the prefix-cache claims of §2.3 rather
  than assert them.
- **M6 — `--context-shift` does not rescue an oversized prompt.** Measured: the
  same 400. It applies only *during generation* — with `-c 1024` and a 725-token
  prompt asking for 500 tokens, the flag let generation run to completion by
  discarding half the window (`n_keep = 1, n_discard = 511` in the log), while
  without it generation stopped at the wall and reported
  `finish_reason: "length"` at exactly `total_tokens = 1024`. Two consequences:
  the eviction throws away the **system prompt first** (`n_keep` defaults to 0,
  so the discard window starts at position 1), and the API says nothing —
  `truncated` exists only on the native `/completion` path, never on
  `/v1/chat/completions` (`server-task.cpp:377` vs `:398`).
- **M7/M8 — an exact token count is available**: `POST /v1/chat/completions/input_tokens`
  takes a full chat body, applies the template and counts with the same flags as
  the real path (`add_special = true`), returning `{"input_tokens": N}`. Verified
  live. `/tokenize` also exists (with `with_pieces`) but counts *raw text*, so it
  would miss the template and BOS. The build string is readable from `/props`
  (`build_info`) and rides every chunk as `system_fingerprint`.

### M9 — the estimate's error changes sign

Measured against the live tokenizer via `input_tokens`, comparing our
`shared/tokens.rs` heuristic (`bytes/4`):

| sample | estimate / actual |
|---|---|
| Russian prose | **1.68** (over) |
| English prose | 1.35 (over) |
| Rust code | **0.80** (under) |
| JSON tool result | **0.93** (under) |

Safe on prose, **unsafe on code and tool output** — the exact content §1.3
identifies as the invisible bulk. This is why §6.4 makes `usage.prompt_tokens`
the primary input. It is also a pre-existing inaccuracy in the `~` figure the
status bar shows today (§11 groundwork).

### The planted-fact probe (the go/no-go)

A 38-message synthetic design conversation (~22 000 chars) with two facts planted
in the first three exchanges — an identifier (`ZARYA-8823`) and a decision **with
its reason** (SQLite over PostgreSQL, *because the product must ship as a single
binary*) — and an assertion that no later message mentions either, so the probe
measures what it claims to. Split early 40% / middle 30% / verbatim tail, two
consecutive rolls (`summarize(early)`, then `summarize(summary₁ + middle)`), then
both questions asked against **summary₂ + the tail only**. A control column asks
the same questions over the full uncompressed conversation — without it, "the
model can't answer" is indistinguishable from "the summary lost it".

**Validity was verified before trusting the result**: across the six requests
the probe makes, the two *compressed* questions contain **zero** occurrences of
`ZARYA`, `SQLite`, `Postgres` or `single binary` outside the summary — so a pass
cannot come from anywhere else. The fixture's own planting assertion earns its
keep (it failed the first draft), and one confound was caught and removed: the
verbatim tail originally ended on an assistant recap listing "settled" decisions
*without* storage, which a model trusting the tail over the summary could have
answered from — a false NO-GO blamed on compression.

Result on the live 31B: **both questions PASS from the compressed context, and
match the control.** Q1 returned `ZARYA-8823` verbatim; Q2 returned SQLite *and*
the single-binary reason — i.e. a decision survived two rounds of
summary-of-summary with its rationale intact, which was the real question. Each
roll cost **~10.6 s** (~1 950 prompt tokens in). The thinking-mute
(`reasoning_budget: 0` + `chat_template_kwargs.enable_thinking: false`) held: no
roll came back with empty `content`, the failure mode `title.rs` exists to
survive.

### The probe's most useful finding: a `max_tokens` cap is not a length limit

At `--summary-tokens 400` both rolls returned **exactly 400** completion tokens
and both summaries were **cut mid-sentence** ("Decision pending on whether to
reject"). The facts survived only because the model front-loads decisions; the
trailing "open questions" section was simply lost. Re-run at **700**, the shape
of the problem showed itself: roll 1 finished naturally at **450** tokens, roll 2
again hit the ceiling at **700**. So the summary was never bounded — at 400 it
was *being truncated*, and given room it **grows with each roll**, which is
exactly the failure mode rolling exists to avoid.

Two causes, both fixable in the prompt rather than the mechanism, and both are
stage-1 requirements rather than nice-to-haves:

1. **The budget must be a stated length, not just a `max_tokens` cliff.** A cap
   silently truncates; an instruction makes the model *prioritize*.
2. **My own roll template invited the growth** — it said the new summary
   "replaces the previous one, so nothing important may be dropped", which
   instructs the model never to shed anything. It must instead say to compress or
   drop what later parts superseded, under the same limit.

Also worth carrying into stage 1: the roll should treat
`finish_reason == "length"` as a signal (the summary was cut, not finished)
rather than accepting the text silently — the same "don't hide a truncation"
rule §1.2 applies to the conversation itself.

**The fix was validated, not just proposed.** A third run added a stated
`Hard limit: at most 250 words` to the system prompt (with "staying under it
matters more than covering everything — keep decisions, identifiers and
constraints, drop procedural detail") and replaced the roll template's
"nothing important may be dropped" with "compress or drop what later parts
superseded, so the summary does not grow as the conversation does":

| variant (`max_tokens` 700 unless noted) | roll 1 | roll 2 | complete? | bounded? | facts |
|---|---|---|---|---|---|
| no length instruction, cap 400 | 400 | 400 | **no** — both cut mid-sentence | only by force | PASS |
| no length instruction, cap 700 | 450 | **700** | no — roll 2 cut | **no, grows** | PASS |
| **250-word instruction, cap 700** | **346** | **399** | **yes** | **yes** (+15%) | PASS |

The capped run's summary₂ is 227 words and ends on a complete "Open Questions"
section — precisely the part truncation had been eating. So the recommended
prompt shape is: `max_tokens` as a **safety net well above** the stated limit,
with the real budget expressed in words inside the prompt.

### Latency and residual risk

A roll costs **~10-12 s** on the reference stack (31B q4 on GPU) for ~2 000
prompt tokens in — background work, invisible unless watched, and comfortably
inside the ~75% threshold's headroom (§6.4).

**Untested, and honestly the audience most in need of the feature**: the probe
ran on 31B. The 8k-window user is typically running a 4B–12B model, and
summarization quality is exactly where model size tells. Stage 1's `#[ignore]`
smoke should therefore be run on a small model too; if a 4B cannot hold a
decision's rationale across two rolls, that strengthens the case for F9(b)'s
read-back tools rather than invalidating the mechanism.

- **Stage 1 — core** (`feat/history-compaction`): `Chat.compaction` +
  request splice + digest variant with tool activity + the roll task
  (title pattern) + `/compact` + the feed divider with the foldable summary
  block (F8c, follows `Ctrl+T`) + the master toggle (F10 — off must be fully
  inert) + clamping + i18n (`prompt.compact.*`) + unit tests + the
  planted-fact live smoke. Spec §6.2/§6.6 bullets, §9.x or §11.x section;
  architecture §5/§11.
- **Stage 2 — auto** (**done**, sub-decisions §10.1): `GenResult` carries usage →
  `maybe_auto_compact` in the `handle_done` tail + budget resolution
  (`EngineBackend::context_budget` → `/props`, per F5) + settings UI ("Memory" →
  "Context") + the overflow hint naming `/compact` (or the setting, when
  compression is off) + the `BackgroundKind::Compaction` failure streak on the
  automatic path only. **What it cost that was not planned for**: the client
  never emitted `ChatChunk::Usage` at all on the llama.cpp path — §9b.
- **Stage 3 — read-back tools** (**done**, committed by F9(b), sub-decisions §10.2):
  `history_read`/`history_search` over the compressed range, the
  `attachment_read`/`attachment_search` shape; the summary block's wording
  switches from "not retrievable" to naming the tools.
- **Groundwork (not committed)**: impersonation reusing the summary; F3(b)
  tool-result eliding; compact-into-new-chat (F1c); prompt-caching alignment
  (roadmap #2 lands its breakpoints around the now-stable prefix).

---

## 9b. What stage 2 found: the exact `usage` never reached us

Measured 2026-08-08, while the stage-2 live smoke refused to fire.

M5 (§9a) established that llama-server reports `usage` in a final chunk with an
empty `choices` array. It does — and **our client threw it away every single
time**. The order on the wire, re-measured directly:

```
data: {"choices":[{"finish_reason":"length","index":0,"delta":{}}], …}
data: {"choices":[],"usage":{"prompt_tokens":20,"completion_tokens":16, …}}
data: [DONE]
```

`OpenAiClient::chat_stream` yielded `Finished` and **`break`ed** the moment a
chunk carried `finish_reason`, so the usage chunk that follows it was never
read. Two consequences, one of them long-standing:

- The status-bar counter's exact figure never arrived on llama.cpp: the `~`
  estimate was, in practice, the only number the user ever saw — contrary to
  what spec §11.1 claimed. The same for reasoning tokens.
- Stage 2's trigger reads the exact figure **and nothing else** (S2), so it had
  nothing to fire on. The smoke reported "nothing folded" with no exact prompt
  ever observed, which is what led here.

Fixed by holding the reason until the stream's own terminator (`[DONE]` or the
body ending) instead of finishing on it. Worth recording as a method note: the
first two attempts at diagnosing this were **instrumentation bugs of the test,
not findings** — `run_turn_capture` drains events up to `Finished`, so it had
already consumed the `TokenUsage` events the diagnostic was looking for. Only
isolating the question to the client itself (stream one request, print the
chunks) answered it.

The other clients are unaffected, checked rather than assumed: Anthropic carries
usage in `message_delta`, OpenAI Responses in `response.completed`, Gemini in
the same part as `finishReason` — all alongside the finish signal, not after it.

---

## 10.1. Stage 2 — sub-decisions (recorded before implementation)

Written after reading the stage-1 code; the track-level forks (§8) already
decided *what* stage 2 is (F4a auto+manual, F5a `/props` discovery), these are
the *how* questions it turned out to contain. **Decided by the user
2026-08-08** — S1(a), S2(a), S3(a); the rest as recommended.

- **S1 — how the context budget is discovered.** **Decision: (a).**
  - (a) **A new `EngineBackend::context_budget()` with a `None` default**,
    implemented by `OpenAiClient` (GET `/props` →
    `default_generation_settings.n_ctx`, read as given — never divided by
    `total_slots`, M2) and left at the default for Anthropic/Gemini/Responses.
    The orchestrator resolves it once per applied engine into a cache and stays
    provider-agnostic. *Recommended*: the trait **is** the engine contract
    (ADR 0004), "what window does this engine have" is engine knowledge, and
    every existing backend compiles unchanged.
  - (b) Resolve it in the supervisor's readiness probe (it already holds the
    concrete `OpenAiClient`) — but that widens the `ServerSupervisor` trait for
    all three servers and couples a budget to a *status* channel that exists to
    carry something else.
  - (c) Explicit setting only — contradicts F5a, and re-introduces the
    "changed `-c`, forgot the setting" footgun.
  - Resolution order either way: explicit `compaction.context_tokens` →
    `managed.context_size` when the mode is managed (that number *is* the `-c`
    the child was launched with, and needs no network) → discovery → **inactive**.

- **S2 — which token number the trigger reads.** **Decision: (a).**
  - (a) **Exact `usage.prompt_tokens` only**; where a provider reports no
    usage, auto-compaction simply never fires (`/compact` still works).
    *Recommended* because of M9: the `bytes/4` estimate's error **changes
    sign** — it overestimates Russian prose by 68% but *underestimates* code by
    20% and JSON tool results by 7%, i.e. it is unsafe precisely on the
    tool-heavy chats that overflow first (§1.3). Every provider we speak to
    reports usage (we send `stream_options.include_usage`), so this is a
    theoretical rather than a practical exclusion.
  - (b) Fall back to the estimate with a margin — more coverage, but the margin
    would have to be sized against the *worst* content type and would then fire
    early on prose.

- **S3 — learning the budget from the 400 body.** **Decision: (a).**
  - (a) **Defer as groundwork; ship the hint only.** *Recommended*, on a
    measured redundancy: the body shape that carries `n_ctx`
    (`exceed_context_size_error`) **is** llama.cpp's, and llama.cpp answers
    `/props` — so the discovery path already covers every server the learning
    path could. What the failure genuinely needs is the *hint* (S4), and the
    user is not left stuck without it: `/compact` needs no budget at all.
  - (b) Implement now — `overflow` threaded through `RoundOutput` →
    `GenResult`, plus relaxing `handle_done`'s early return (an overflow turn
    produces no messages). Real plumbing for a case (a) already covers.

- **S4 — the overflow hint, and it must not create a dead end.** The generic
  `ui.err.generation_failed` (raw JSON in a wrapper) becomes a message that
  says what to do — but **which** advice depends on the switch: with
  compression on it names `/compact`, with it off it names the setting.
  Pointing at a command that would refuse is exactly the defect class the
  journal has closed three times (the by-reference attachment block, the
  `youtube_watch` unconfigured path, the `python_exec` sandbox). Detection is a
  pure function over the error text (best-effort, a small documented list of
  provider markers), so it is testable without a server.

- **S5 — the threshold's shape.** One percentage,
  `compaction.threshold_pct` (default **75**), against the resolved budget; the
  reply reserve is **folded into the remaining 25%** rather than subtracted
  separately. The trigger compares `last_prompt_tokens + last_completion_tokens`
  (the next turn's prompt, near enough) against `budget × pct / 100`. A
  separate reserve would be a second knob describing the same headroom.

- **S6 — failure semantics differ by origin.** The **auto** path passes the
  real result into `handle_bg_done` (the 3-strike alert the slot machinery
  exists for), while the **manual** path keeps stage 1's behaviour: report the
  failure straight to the user and close the slot as a success, since alerting
  twice for a command just typed is noise. `CompactResult` therefore carries
  its origin — the stage-1 code comment asks for exactly this.

- **S7 — which chat.** The one whose turn just finished (`res.chat_id`), not
  "the active one": they are the same today (one generation at a time), and
  reading the turn's own id is what stays correct if that ever stops being true.

- **S8 — cloud.** Explicit `context_tokens` or inactive. No provider→model→window
  table: model names are free-form and windows move under us (§11), and the
  motivation there is cost rather than a ceiling.

---

## 10.2. Stage 3 — sub-decisions (recorded before implementation)

Written after reading the stage-1/2 code and the two precedents this stage
copies (`attachment_read`/`attachment_search`, and the `cache.db` full-text
index). F9(b) already decided *what* stage 3 is — read-back tools over the
compressed range — these are the *how* questions it turned out to contain.
**Decided by the user 2026-08-08** — all as recommended.

The one finding that reshaped the stage before any code: **the full-text index
this project already maintains covers exactly the content the read-back tools
need.** `cache.db` (SQLite FTS5, trigram) indexes every message of every chat
and, checked rather than assumed, `search::indexed_messages` filters only on
*empty text* — so `Tool`-role messages are indexed too, at full length. That is
§1.3's invisible bulk, already searchable, already kept in step by the post-save
hook, and reachable per chat through `CacheDb::matching_messages_in_chat`.

- **S9 — how a tool sees the compressed range.** **Decision: (a).**
  - (a) **A turn snapshot on `ToolContext`** — `Arc<[Message]>` of
    `messages[..upto]`, built where `attachments` already is (`TurnInfo`).
    *Recommended*: the orchestrator stays the sole owner of `Chat`, there is no
    I/O on the tool path, and the snapshot is consistent with the request the
    model was actually shown — a roll that lands mid-turn changes the chat but
    must not change what this turn's tools describe.
  - (b) The tool loads the chat from storage by `chat_id` — bypasses the
    ownership invariant and can read a stale on-disk copy (saves are debounced
    800 ms).

- **S10 — what a "page" is.** **Decision: (a).**
  - (a) **Token-budgeted pages over a rendered transcript**, reusing
    `entities::attachment::paginate` (cuts on a line boundary, never splits a
    character). *Recommended*: it is the `attachment_read` guarantee verbatim —
    the model walks `1..M` and **knows** it has read everything, which is the one
    thing retrieval cannot promise.
  - (b) One page = one exchange. Semantically tidy, but exchange sizes vary
    wildly — the largest single message in the dev corpus is 38 782 characters,
    so a "page" could exceed the window the whole feature exists to protect.
  - (c) Message-index ranges — no `M` to walk, so the guarantee is lost.

- **S11 — what `history_search` searches with.** **Decision: (a).**
  - (a) **The existing `cache.db` FTS5 index**, scoped to the chat and filtered
    to the compressed range. *Recommended*, and the decisive argument is the
    audience: an embedding server is a **separate** server (ADR 0002) and is
    routinely unconfigured, while the 8k local user is exactly who this track
    exists for — a search that needs an embedder would be absent precisely where
    it is needed. It also costs no new table, no indexing task, no
    generation-stamping and no `/reindex` integration. Trigram additionally does
    substring matching, so `8823` finds `ZARYA-8823` — identifiers are what a
    summary loses first.
  - (b) A new chat-scoped vec0 semantic index, mirroring `attachment_search`.
    Better recall for paraphrase, but the *summary* is already the semantic view
    of that range; what it loses is verbatim detail, and verbatim detail is what
    lexical search is best at. The two are complementary the other way round
    from how it first looks. Groundwork, if a live run shows lexical misses.
  - (c) Both — twice the surface for a gap not yet observed.

- **S12 — are the tools always offered?** **Decision: (a).**
  - (a) **Registered only when this chat actually has a compressed range** — a
    special case in `effective_tool_ids`, exactly like the one `get/set_sampling`
    already has for a provider with no settable fields. *Recommended*: two tool
    schemas cost prompt on every turn, and this feature's whole audience is
    people fighting a ceiling. It also earns an invariant — **the block and the
    tool set are driven by the same `compaction_view`**, so the block can name
    the tools without ever promising one that is absent (S15). The tool set
    changes at the moment of a compaction, which already re-prefills the prompt,
    so the prefix cache pays nothing extra.
  - (b) Always registered, answering "nothing has been compressed" — simpler,
    but it spends context on every turn of every chat to describe a situation
    that does not exist.

- **S13 — what `history_read` covers.** **Decision: (a).**
  - (a) **The compressed range only.** *Recommended*: it is precisely what the
    model cannot see; the verbatim tail is already in the prompt, so a page spent
    on it would be a page wasted.
  - (b) The whole conversation — a simpler sentence to write in the description,
    at the cost of inviting calls that return what the model is already holding.

- **S14 — the page size.** **Decision: (a).**
  - (a) **A new `compaction.page_tokens`**, settings → "Memory" → "Context".
    *Recommended*: an 8k user and a 200k user need materially different pages,
    and this feature's users are the ones with the least room to spare.
  - (b) A constant — one knob fewer, but wrong at both ends of the range.
  - (c) Reuse `attachments.page_tokens` — the same meaning, but a setting named
    after attachments silently governing history reading is the kind of coupling
    that surprises whoever changes it.

- **S15 — the block's wording.** F9(b) commits it: the block stops saying the
  verbatim text is unreachable and **names the two tools**, in both bundles.
  *Recorded first as a single wording* on the argument that S12 gates block and
  tools on the same `compaction_view` — and **corrected during implementation**:
  that holds for the chat-level gate, but a *profile* can switch the two tools
  off, and then the block would name tools the model does not have. So there is a
  with-tools/without-tools pair after all (`compaction.block.tools` /
  `…no_tools`), chosen from the turn's real tool set — exactly the split
  `prompt.attachments.end_excerpt` vs `…_search` makes for an attachment that
  may or may not be indexed. The invariant the single wording was reaching for
  survives in the form that matters: the block never names an absent tool.

- **S16 — what a search hit carries.** The **page number**, plus a snippet and
  the speaker. That is what makes the pair compose the way the attachment pair
  does: search says *where* to look, `history_read` guarantees *everything* can
  be read. A hit without a page number would leave the model with a fragment and
  no way to widen it.

Three notes that are not forks:

- **One renderer, parameterized by the tool-result clip.** The transcript the
  reader pages through and the digest the summarizer saw are built by the same
  function — so what was summarized is what can be re-read — but the digest
  clips a tool result to `TOOL_RESULT_CLIP` (200 chars) while the reader must
  not clip at all, or paging back to a `fetch_url` result would return the same
  200 characters the summary already had.
- **FTS query escaping stays in its one home.** `features::chat_search::to_fts_query`
  exists precisely because `shared/storage` may not depend on `features`, so
  `CacheDb` takes an already-escaped query; the tool calls it rather than growing
  a second escaper to drift from the first. Raw input cannot reach `MATCH` —
  measured when that home was built, `C++`, `cost-benefit` and `50%` are all SQL
  errors on ordinary text.
- **Both "nothing here" answers must close the door**, the lesson this journal
  has now recorded four times (the by-reference attachment block, the
  `youtube_watch` unconfigured path, the `python_exec` sandbox, the stage-2
  overflow hint): "nothing has been compressed in this conversation — all of it
  is already in front of you" is an answer; a bare empty result is an invitation
  to improvise.

### What the stage-3 live run cost, and what it taught

The smoke asks the one question unit tests cannot: **will a real model, told by
the block that the tools exist, reach for one instead of guessing?** Its validity
rests on the seed being genuinely un-summarizable, and the fixture took **four
attempts** — each failing its own precondition rather than the feature, and each
worth recording:

1. **Fifteen item→code pairs survived a 250-word summary whole.** Fifteen pairs
   is about sixty words. Worse, the model filed the list with `note_save` first,
   so it could have answered from memory without touching the history *and* the
   note's result travelled into the digest. The smoke now enables **only** the
   two read-back tools — the same move `spawn_orch_live_no_embed` makes for
   attachments: remove the alternative rather than hope it is not taken.
2. **Sixty entries, but the target was the only *named* one** among "item number
   N" — so the summary compressed the rest into ranges and kept exactly the entry
   that had to be lost. Every entry must be equally plausible and equally
   nameable.
3. **Sixty entries as adjective × noun were grouped by adjective**, and all sixty
   codes still fitted. Any structure is compressible; the fixture went to **200
   entries against a 120-word limit**, which is arithmetic rather than a hope
   about the model's judgement.
4. **The control question was itself the confound** — the sharpest of the four.
   Asked about the target and landing *inside* the folded range, it made that one
   entry the most salient thing in the range, so the summary kept precisely what
   the test needed dropped — twice in a row, which is what showed it was not
   chance. The control now asks about a **different** entry, keeping its purpose
   (proving the model can answer this kind of question at all) without steering
   the summarizer. Note the mirror image in stage 1's smoke, where the control
   *helped* because that test wants the fact kept: the same device is an aid or a
   confound depending on which direction the assertion runs.

**Result — GO** (Gemma 4 31B q4_0, external `llama-server`): the summary dropped
199 of 200 entries, keeping one example — the control's; the model then called
`history_search` twice and answered with the target's code, which existed nowhere
but in messages no longer being sent.

---

## 11. Groundwork noted along the way

- `estimate_prompt_tokens` ignores `req.tools` — with MCP schemas enabled the
  estimate undercounts; worth fixing when the trigger starts depending on it.
- **The `~` token figure in the status bar is materially wrong today** (§9a M9):
  measured against the real tokenizer it overestimates Russian prose by 68% and
  underestimates code by 20%. Independent of this feature — it is what the user
  reads on every turn — and cheap to improve (per-script byte ratios, or the
  exact count once a turn has reported `usage`).
- **`usage.prompt_tokens_details.cached_tokens`** is reported by llama-server and
  ignored by us; it measures prefix-cache reuse directly, which would let stage 1
  *verify* the §2.3 trade-off instead of reasoning about it.
- The exact `prompt_tokens` dies in the UI today (`ChatScreen.gen_context`);
  carrying it through `GenResult` (stage 2) also opens the door to persisting
  a per-chat "last known size" for restart continuity.
- Impersonation (`Ctrl+U`) builds its own full-history request and will hit
  the same ceiling; it can reuse the same summary with role-swap care.
- A per-provider context-window table was considered and rejected: model names
  are free-form, windows change under our feet; explicit setting + discovery
  only.

## 12. Sources

- llama.cpp server README — `/props` (`default_generation_settings.n_ctx`),
  `--context-shift` off by default:
  <https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md>
- The 400 overflow error as seen by downstream clients:
  <https://github.com/continuedev/continue/issues/9797>,
  <https://github.com/gpustack/gpustack/issues/852>
- Context shift discussion (model support caveats):
  <https://github.com/ggml-org/llama.cpp/discussions/14170>
- OpenAI Responses `truncation` (default `disabled` → 400):
  <https://platform.openai.com/docs/guides/conversation-state>,
  <https://community.openai.com/t/documentation-issues-responses-endpoint-storage-storage-persistence-truncation/1268533>
- Anthropic `prompt is too long` 400:
  <https://portkey.ai/error-library/input-length-error-10153>
- In-repo: spec §6.2/§6.6/§9.5/§9.7, `docs/roadmap.md` §"Context and tokens",
  ADR 0006 (additive fields), `docs/file-attachments.md` §3 (retrieval ≠
  guaranteed context), CLAUDE.md journal (title/reflection/attachment
  precedents).
