# Research: a full-fledged `openai` mode client (Responses API)

**Status:** implemented (2026-07-10). Originally research; **variant A** was
chosen (mode `openai` switched from Chat Completions to the Responses API),
adding "thoughts" (reasoning summaries), `reasoning_effort` (extended to
`minimal`/`xhigh`), `verbosity`; the "keyed proxy" case closed via
`ExternalSettings.api_key_env`. Summary in CLAUDE.md ("OpenAI → Responses
API"). Extends
[ADR 0004](../decisions/0004-engine-contract-multi-provider.md) (Phases 0–2).

## 1. Problem

Mode `ServerMode::OpenAi` today talks to **Chat Completions**
(`/v1/chat/completions`) via the same `OpenAiClient` as llama.cpp/Gemini,
differing only by `WireDialect::OpenAi`. The dialect **strips** all reasoning
signals (`restrict_to_strict`: `thinking`, `reasoning_effort`,
`reasoning_budget`, `chat_template_kwargs`), so:

- there are no "thoughts" (CoT) at all in `openai` mode — `ChatChunk::Thoughts`
  never arrives;
- `reasoning_effort` isn't sent, reasoning depth can't be controlled;
- `supported_sampling_fields(OpenAi)` = `frequency_penalty`/`presence_penalty`/
  `seed`/`max_tokens` — four fields, of which only one is actually meaningful
  for gpt-5.x.

Meanwhile Claude (Phase 2) has "thoughts" and `reasoning_effort` — the
asymmetry is visible to the user right in the "Sampling" section.

## 2. What we learned about the API (checked against docs, July 2026)

### 2.1 Reasoning lives in the Responses API, not Chat Completions

- **Raw CoT is never returned** — in any API. Only **reasoning summaries** are
  available. Same as Anthropic's `display:"summarized"`: the UI shows a
  summary, not raw tokens. ([reasoning guide])
- Summaries arrive **only via the Responses API** (`POST /v1/responses`) and
  only with explicit opt-in `reasoning.summary` = `"auto"` | `"concise"` |
  `"detailed"` (off by default). Chat Completions has no summaries; the docs
  themselves call Chat Completions a legacy path for reasoning models.
  ([reasoning guide])
- `reasoning.effort`: `none` | `minimal` | `low` | `medium` | `high` | `xhigh`
  (gpt-5.6 also has `max`), the set **depends on the model**; gpt-5.5 defaults
  to `medium`. Our `ReasoningEffort` only knows `none/low/medium/high`.
- Reasoning models don't accept `temperature`/`top_p` (we already knew this —
  commit `f032df8`); `frequency_penalty`/`presence_penalty` don't exist at all
  as parameters in the Responses API.

### 2.2 Reasoning items and tool-use (key nuance, mirrors Anthropic Phase B)

For stateless operation (`store: false`, our case — we keep our own chat
history) we need to:

- request `include: ["reasoning.encrypted_content"]` on **every** request;
- take the `{"type":"reasoning","id":"rs_…","encrypted_content":"gAAA…"}` item
  from the response and **return it in the input of the next round right
  before** its `function_call` item;
- skipping reasoning items isn't an error (the API just won't see them), but
  gives a **~3%** quality drop on benchmarks (OpenAI measured on SWE-bench).
  ([cookbook])

This is **the same mechanism** already implemented for Anthropic:
`ChatChunk::ThoughtsSignature` → `RoundOutput.thoughts_signature` →
`ApiMessage::with_thinking(ThinkingBlock)` → wire puts the block first in the
assistant turn with calls. There's exactly one difference: OpenAI needs an
`id` for the item, in addition to `encrypted_content`.

### 2.3 Protocol format

| | Chat Completions | Responses |
|---|---|---|
| System message | `messages[0].role="system"` | top-level `instructions` |
| History | `messages[]` | `input[]` — a mix of messages and **items** |
| Tool result | `role:"tool"`, `tool_call_id` | `{"type":"function_call_output","call_id","output"}` |
| Tool call | `assistant.tool_calls[]` | `{"type":"function_call","call_id","name","arguments"}` |
| Tool schema | `{type:"function", function:{name,…}}` | `{type:"function", name, description, parameters, strict}` (flat) |
| Token limit | `max_completion_tokens` | `max_output_tokens` |
| Stop reason | `finish_reason` | `status` + presence of `function_call` in `output` |
| Usage | `prompt_tokens`/`completion_tokens` | `input_tokens`/`output_tokens` (+`output_tokens_details.reasoning_tokens`, `input_tokens_details.cached_tokens`) |

**SSE**: `event: <name>` + `data: {...}`, and **`data` itself has a `type`
field** equal to the event name. So parsing is exactly like Anthropic:
`#[serde(tag = "type")]` over an enum. Event order for one turn:

```
response.created → response.in_progress
response.output_item.added        (item.type = "reasoning")
response.reasoning_summary_text.delta ×N        → ChatChunk::Thoughts
response.output_item.done         (reasoning: id + encrypted_content)
response.output_item.added        (item.type = "function_call": id, call_id, name)
response.function_call_arguments.delta ×N       → ChatChunk::ToolCall(arguments)
response.output_item.done         (function_call)
response.output_text.delta ×N                   → ChatChunk::Text
response.completed                (response.usage, response.status)
```

Plus `response.incomplete` (`incomplete_details.reason == "max_output_tokens"`),
`response.failed`, `error`.

## 3. Pitfalls (things to trip on unless planned for up front)

1. **`max_output_tokens` includes reasoning tokens.** This is exactly the same
   class of bug as "thoughts ate the whole budget" with Gemma/chat
   auto-titling: with a stingy limit, the model spends everything on
   reasoning, `output_text` comes back empty, and status is
   `incomplete/max_output_tokens`. Our `SamplingConfig.max_tokens` maps here —
   we need either a generous default or a warning in the field hint.
   Separately: `Finished(Length)` must be emitted on `response.incomplete`.
2. **`store` defaults to `true`** — OpenAI would store responses on its side.
   We send `store: false` explicitly (we have our own history, plus privacy).
3. **`strict` for function-tools in Responses defaults to "try strict mode"**
   (unlike Chat Completions, which is non-strict by default). Our schemas
   don't satisfy strict-mode requirements (`additionalProperties: false` +
   all fields in `required`) — the API promises a soft fallback, but we
   shouldn't rely on it: send `strict: false` explicitly.
4. **`response.completed` carries no `finish_reason`.** `FinishReason::ToolCalls`
   is derived by the client from the fact that a `function_call` item appeared
   in the stream.
5. **The summary may not arrive** — at low `effort` or on a trivial prompt the
   model doesn't reason. Exactly like Claude's adaptive thinking (already
   documented in CLAUDE.md). Tests must allow for this.
6. **Gemini and External stay on Chat Completions.** Gemini's OpenAI-compatible
   endpoint has no `/responses`; third-party servers even less so. `OpenAiClient`
   isn't going anywhere.
7. **Embeddings** (`/v1/embeddings`) stay on `OpenAiClient` — Responses doesn't
   have them. `cloud_embed_setup` untouched.
8. `CloudProvider::OpenAi.base_url()` is already `https://api.openai.com/v1` →
   client URL is `{base}/responses` (unlike Anthropic, where the client itself
   appends `/v1/messages`).

## 4. Architecture: where this lands

Responses is **a different protocol from the same vendor**, meaning it's a new
`EngineBackend` implementation, exactly like `AnthropicClient` (ADR 0004,
Phase 2). Layers above the engine (orchestrator, agentic loop, tools, UI)
aren't touched — this is the research's main conclusion: **the feature fits
entirely behind the trait**.

Layout: `shared/api/openai/responses/{client.rs, wire.rs}` (the vendor
grouping `openai/` is kept, split by protocol inside). Alternative — a sibling
module `shared/api/responses/` next to `anthropic/`; the choice is cosmetic,
the former is preferred (ADR §2 groups `openai/` by family).

### 4.1 Fork: replace the transport or add a mode?

| Variant | Pros | Cons |
|---|---|---|
| **A. `ServerMode::OpenAi` = Responses** (replacement) | The flat mode taxonomy doesn't grow; `CloudProvider::OpenAi` stays the key for `supported_sampling_fields`; zero config changes | Breaks anyone who used the `openai` mode + `url` override as a "keyed proxy" (such a proxy speaks Chat Completions) |
| B. New mode `openai-responses` | Breaks nothing | Two "OpenAI" entries in the selector; duplicated cloud fields; `CloudProvider` grows |
| C. Toggle `CloudSettings.openai_api = chat\|responses` | Flexible | `supported_sampling_fields(provider)` stops depending only on the provider → the signature leaks into settings UI, `get/set_sampling`, `retain_supported`, message metadata. Notable ripple for a rare case |

**Recommendation — A**, and close the "keyed proxy" case cheaply:
`ExternalSettings.api_key_env: Option<String>` (`#[serde(default)]`, no
migration) — right now External has no key field at all, and that's an
independent gap.

### 4.2 Contract changes (minimal)

- `ChatChunk::ThoughtsSignature(String)` → `ThoughtsSignature(ThinkingRef)`,
  where `ThinkingRef { id: Option<String>, signature: String }`. Anthropic puts
  `id: None`, OpenAI — `Some("rs_…")` + `signature = encrypted_content`. All
  places that ignore the variant (`subagent`, `fetch`, `title`,
  `impersonation`, `tool_loop`, `openai/client`) match `ThoughtsSignature(_)` —
  they won't need touching.
- `ThinkingBlock` += `id: Option<String>`.
- `RoundOutput.thoughts_signature: Option<ThinkingRef>` (`generation.rs`, ~5 lines).

### 4.3 Sampling changes

- `ReasoningEffort` += `Minimal`, `XHigh`, `Max` (serde lowercase; old
  `settings.json` still reads fine). Mapping `ReasoningEffort::None` →
  `reasoning.effort:"none"` (rather than "don't send the field") — this gives
  impersonation and chat auto-titling an honest way to turn off reasoning,
  mirroring `reasoning_budget=0`. Treat `reasoning_budget == Some(0)` as
  `effort:"none"`.
- `supported_sampling_fields(Some(OpenAi))` → `["max_tokens", "thinking",
  "reasoning_effort"]` (later `+"verbosity"`). This is the **only** change
  needed by settings UI, the `get/set_sampling` tools, and the
  `Message.metadata` snapshot — everything else derives from it automatically
  (as it did with Claude).
- `thinking: Some(true)` → `reasoning.summary: "auto"`; otherwise omit the
  field.

### 4.4 Supervisor

`cloud_chat_setup`: the `CloudProvider::OpenAi` branch builds `ResponsesClient`
instead of `OpenAiClient::with_dialect(WireDialect::OpenAi)`. `WireDialect::OpenAi`
becomes dead in the process (its only consumer was the OpenAI cloud) → remove
it along with `max_completion_tokens` and the OpenAI branch of
`restrict_to_strict`; `LlamaCpp` and `Gemini` remain. Nice side effect:
`wire.rs` slims down.

## 5. What else Responses gives us (beyond "thoughts" and effort)

Besides what's claimed, "full-fledged" can also include:

- **`text.verbosity`** (`low`/`medium`/`high`) — response length, independent
  of temperature. New field `SamplingConfig.verbosity` + one `SamplingParam`
  in the UI. Cheap.
- **Finer-grained usage**: `reasoning_tokens` (how much went into "thoughts")
  and `cached_tokens`. We already have a token counter in the status bar — this
  fits in naturally.
- **`prompt_cache_key`**: a stable per-chat key → cheaper and faster prompt
  caching. Our system prompt is large (self-model, maintenance protocol), so
  the win is real, but it's partly eaten by the fact that self-model injection
  already busts the prefix cache (accepted trade-off, architecture §9.3).
- **`service_tier`** (`flex`/`priority`) — price/latency.
- **OpenAI's built-in tools** (`web_search`, `code_interpreter`, `file_search`)
  — server-side analogues of our `web_search`/`python_exec`. A large separate
  track: conflicts with the client-side agentic loop (the server executes
  them, we execute via the orchestrator), breaks feed tool-card uniformity.
  **Out of scope**, but worth noting as groundwork.
- `previous_response_id` / conversations — **not needed**: we have our own
  history, edits, and turn regeneration; server-side state conflicts with
  that.

## 6. Scope estimate

Modeled on Anthropic (`wire.rs` 566 lines + `client.rs` 404, tests and smokes
included):

- **Phase A** (core, no tool-use round-trip): `responses/wire.rs` (build
  `input` from `ApiMessage`, flat tools, `store:false`, `include`,
  `reasoning`, `max_output_tokens`; parse SSE events) + `responses/client.rs`
  (`EngineBackend`, `FinishReason` derivation, usage) + sampling/supervisor
  changes. Live smokes: plain generation; `Thoughts` stream at
  `thinking=true`; `effort` accepted.
- **Phase B** (tool-use): `ThinkingRef` with `id`, echoing the reasoning item
  before `function_call` in `build_input`, `generation.rs` change. Live smoke:
  two rounds with a tool (no echo — degradation, with echo — correct; there
  shouldn't be API errors either way, so the smoke checks for the
  **presence** of the item in the body of the second request).
- **Phase C** (optional): `verbosity`, `reasoning_tokens` in the status bar,
  `prompt_cache_key`.

Realistically: A+B — one PR, comparable to Claude Phase A/B. Ripple outside
`shared/api` — about 40 lines (contract, sampling, supervisor, generation).

**Low risk profile**: layers above `EngineBackend` don't change; on failure the
rollback is: point the supervisor branch back to `OpenAiClient`.

## 7a. Live run results (gpt-5.5, 2026-07-10)

Generation and tool-calling work. **Reasoning summaries didn't come through**
— a server-side gate, not a bug: OpenAI only returns `reasoning.summary` for
**verified organizations**
(`platform.openai.com/settings/organization/general`); for unverified ones the
summary is empty (or a `400` on the mere presence of `reasoning.summary`).
Client changes as a result: `summary:"auto"` → `"detailed"` (more reliable on
some models) and parsing `response.reasoning_text.delta` alongside
`response.reasoning_summary_text.delta`. Since the response came back without
a `400`, reasoning did happen (paid for in reasoning tokens), just the summary
text is withheld pending verification. Raw CoT is never returned (summary
only).

## 7. Open questions (need a live key)

1. The exact `effort` set for the current default model (does it accept
   `xhigh`/`max`) — need to see a `400` on an unsupported value with our own
   eyes to decide: hide the values in the UI, or surface the error text to
   the user.
2. Whether the reasoning item's `id` is required for the echo (or is
   `encrypted_content` enough), and exactly what the API responds when a
   reasoning item isn't followed by a `function_call`.
3. Compatibility of `store:false` + `include:["reasoning.encrypted_content"]`
   with non-reasoning models (gpt-4.1, etc.) — shouldn't break, but verify.
4. Whether `reasoning.context: "all_turns"` is needed when manually replaying
   history (per the docs it only matters when accessing past response items;
   we don't have those between turns).

Smokes — modeled on Anthropic: `#[ignore]`, key from `MINDFORK_OPENAI_KEY`,
model from `MINDFORK_OPENAI_MODEL`.

---

Sources: [reasoning guide](https://developers.openai.com/api/docs/guides/reasoning),
[cookbook: reasoning items](https://developers.openai.com/cookbook/examples/responses_api/reasoning_items),
[streaming events](https://developers.openai.com/api/reference/resources/responses/streaming-events),
[function calling](https://developers.openai.com/api/docs/guides/function-calling).
