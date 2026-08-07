# Research: conversation history compression (rolling summary)

> Research for roadmap §"Context and tokens" item #1 ("Most valuable next").
> Status: **forks decided by the user 2026-08-07** (§8) — F8(c), F9(b), F10 with
> a full off switch, the rest as recommended. Next step: stage 0 (the probe, §9).
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

Sourced from provider docs/issues; the llama.cpp case is to be **reproduced
against our live stack in stage 0** (the probe), since it is the primary
audience:

| Provider | Behavior at overflow |
|---|---|
| llama-server (default) | **HTTP 400** `the request exceeds the available context size, try increasing it`. Context shift is **disabled by default** (opt-in `--context-shift`). |
| llama-server + `--context-shift` | Silent front eviction of KV blocks — the model quietly loses the *system prompt and early history* mid-generation. Worse than the error. |
| OpenAI Responses | `truncation` defaults to `disabled` → **400** when the model's window is exceeded (`"auto"` would drop middle items; we don't send the field). |
| Anthropic | **400** `invalid_request_error`: `prompt is too long: N tokens > M maximum`. |
| Gemini | **400** `INVALID_ARGUMENT` on the token limit. |

Our client does not swallow error bodies (a long-standing rule), so the user
sees the 400 text — but as a generic `ui.err.generation_failed` wrapper
(`generation.rs:1134-1139`), with no hint of what to *do*. Today's recourse:
start a new chat, `Ctrl+E` away exchanges by hand, or raise `-c`.

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
(§9) is the direct test of exactly this.

### 6.4 The trigger
In the `handle_done` tail: `next_prompt_estimate = last exact prompt_tokens +
completion_tokens` (from `GenResult`, newly carried) with the byte-estimate
fallback; compact when it exceeds `threshold_pct` (default ~75%) of the
**resolved context budget** minus a reply reserve (effective `max_tokens`, else
a constant). Budget resolution:

| Mode | Budget source |
|---|---|
| managed | `managed.context_size` (already known) |
| external | `/props` discovery (best-effort, llama.cpp only) → explicit setting → feature inactive |
| cloud | explicit setting → feature inactive (motivation there is cost, not a ceiling) |

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

## 10. Stages sketch (after go)

- **Stage 1 — core** (`feat/history-compaction`): `Chat.compaction` +
  request splice + digest variant with tool activity + the roll task
  (title pattern) + `/compact` + the feed divider with the foldable summary
  block (F8c, follows `Ctrl+T`) + the master toggle (F10 — off must be fully
  inert) + clamping + i18n (`prompt.compact.*`) + unit tests + the
  planted-fact live smoke. Spec §6.2/§6.6 bullets, §9.x or §11.x section;
  architecture §5/§11.
- **Stage 2 — auto**: `GenResult` carries usage → `maybe_auto_compact` in the
  `handle_done` tail + budget resolution (`/props` discovery per F5) +
  settings UI ("Memory" → "Context") + the 400-error hint naming `/compact` +
  `BackgroundKind::Compaction` chip/failure streak.
- **Stage 3 — read-back tools** (committed by F9(b)): `history_read`/
  `history_search` over the compressed range, the `attachment_read`/
  `attachment_search` shape; the summary block's wording switches from "not
  retrievable" to naming the tools.
- **Groundwork (not committed)**: impersonation reusing the summary; F3(b)
  tool-result eliding; compact-into-new-chat (F1c); prompt-caching alignment
  (roadmap #2 lands its breakpoints around the now-stable prefix).

---

## 11. Groundwork noted along the way

- `estimate_prompt_tokens` ignores `req.tools` — with MCP schemas enabled the
  estimate undercounts; worth fixing when the trigger starts depending on it.
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
