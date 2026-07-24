# Research: a full-fledged client for `gemini` mode (native generateContent)

**Status:** research (2026-07-10). A counterpart to
[docs/research/openai-responses-client.md](openai-responses-client.md) for Gemini.
Extends [ADR 0004](../decisions/0004-engine-contract-multi-provider.md) (which
explicitly marked native Gemini "out of scope — routed through OpenAI-compat").
Conclusion: a native client is **feasible and fits behind the `EngineBackend`
trait**, like Anthropic and OpenAI Responses, but Gemini has one structural
wrinkle (**thought signatures are per-part and mandatory for Gemini 3 during
tool-use**) that no other provider has; it determines whether the signature
needs to be persisted on the domain `Message`.

## 1. Problem

`ServerMode::Gemini` mode currently hits the **OpenAI-compatible Chat
Completions** endpoint (`…/v1beta/openai/chat/completions`) via the same
`OpenAiClient` with `WireDialect::Gemini`. The dialect is strict
(`restrict_to_strict`) — it **strips** reasoning signals (`thinking`,
`reasoning_effort`, `reasoning_budget`) and llama.cpp extensions, so:

- there are no "thoughts" (CoT) in `gemini` mode — `ChatChunk::Thoughts` never
  arrives;
- `reasoning_effort` isn't sent — reasoning depth can't be controlled;
- `supported_sampling_fields(Gemini)` = `temperature`/`top_p`/
  `frequency_penalty`/`presence_penalty`/`seed`/`max_tokens` (even `top_k`,
  which Gemini **accepts natively**, is stripped by the dialect — a limitation
  of the compat protocol, not of Gemini).

Claude (Phase 2) and OpenAI (Responses) have "thoughts" and `reasoning_effort`
— the asymmetry is visible to the user right in the "Sampling" section.

## 2. What we've learned about the API (verified against the docs, July 2026)

### 2.1 Gemini has TWO native APIs

- **`generateContent` / `streamGenerateContent`** — mature, stable, well
  documented REST (`…/v1beta/models/{model}:streamGenerateContent`). This is
  the counterpart to Chat Completions by "generation": `contents[]` +
  `generationConfig`. **Recommended target.**
- **Interactions API** — newer (an analogue of OpenAI Responses):
  `generation_config.thinking_level`/`thinking_summaries`, "thought steps" with
  `signature`+`summary`. Fresh, less documented, has a separate migration
  guide. **Not taken yet** — immaturity for the same outcome.

We go with `generateContent` (thoughts via `thinkingConfig.includeThoughts`,
streaming via `:streamGenerateContent?alt=sse`).

### 2.2 Reasoning lives in `generationConfig.thinkingConfig`

- **Thought summaries** (not raw CoT): `thinkingConfig.includeThoughts: true`
  → the response gets parts `{"text": "...", "thought": true}` (incrementally,
  during streaming). This maps directly to our `ChatChunk::Thoughts`.
- **Depth** depends on the model generation (a pitfall, see §3):
  - **Gemini 3.x** — `thinkingConfig.thinkingLevel`:
    `minimal`|`low`|`medium`|`high`;
  - **Gemini 2.5** — `thinkingConfig.thinkingBudget` (tokens): `0` disables it
    (except for 2.5 Pro — minimum 128, can't be disabled), `-1` = dynamic,
    otherwise a range (2.5 Flash `0–24576`, Pro `128–32768`).
- There's no separate `reasoning_effort` field in the **native** API — there's
  `thinkingLevel`/`thinkingBudget`. Our `ReasoningEffort`
  (`none/minimal/low/medium/high/xhigh`) maps to `thinkingLevel` (3.x) or
  `thinkingBudget` (2.5).
- **Thought tokens**: `usageMetadata.thoughtsTokenCount` (already included in
  output billing) — feed our `TokenUsage.reasoning_tokens`.

### 2.3 Thought signatures — per-part and mandatory for Gemini 3 (the key wrinkle)

This is what sets Gemini apart from Anthropic (one signature per turn) and
OpenAI (one reasoning element per turn):

- The `thoughtSignature` — an **encrypted opaque token bound to a specific
  part** (`functionCall` or `text`), not to the turn as a whole.
- **Gemini 3: mandatory.** If a `functionCall` part appears in `contents`
  without a `thoughtSignature`, the API returns `400`: *"Function call … in the
  N content block is missing a thought_signature."* Gemini 2.5 — optional (no
  400).
- **Parallel calls**: only the **first** `functionCall` part of a turn carries
  a signature. **Sequential** calls (in different rounds of a turn) each get
  their own.
- Rule: if you got a signature, return it **in the same part** when sending the
  history.

```jsonc
// model's response: the signature is a sibling of functionCall
{ "functionCall": { "name": "calc", "args": {"x": 1} },
  "thoughtSignature": "<opaque>" }
```

**Why this matters to us.** `message_to_api` (`orchestrator/request.rs`)
rebuilds the history from the persistent `Chat` **on every generation**: past
assistant turns with tool calls (`ApiMessage::assistant_tool_calls`) are resent.
For Anthropic this is safe (the server itself drops old thinking and doesn't
require a signature on historical `tool_use`); for **Gemini 3**, historical
`functionCall` parts without a signature → `400`. So for Gemini 3 the signature
most likely **needs to be persisted** on the domain `Message` (unlike
Anthropic/OpenAI, where it lives only in the turn's memory). This is the one
change outside the engine layer that makes Gemini harder than the two prior
providers.

### 2.4 Protocol format (native generateContent)

| | Chat Completions (our `OpenAiClient`) | Gemini generateContent |
|---|---|---|
| Endpoint | `/v1/chat/completions` | `…/v1beta/models/{model}:streamGenerateContent?alt=sse` |
| Auth | `Authorization: Bearer` | `x-goog-api-key: <key>` |
| System | `messages[0].role="system"` | top-level `systemInstruction:{parts:[{text}]}` |
| History | `messages[]` (roles system/user/assistant/tool) | `contents:[{role:"user"|"model", parts:[…]}]` — **only `user`/`model`** |
| Tool result | `role:"tool"`, `tool_call_id` | a `{functionResponse:{name, response:{…}}}` part inside a **`role:"user"`** content |
| Tool call | `assistant.tool_calls[]` (`id`+`arguments`-string) | a `{functionCall:{name, args:{…}}}` part (+ a sibling `thoughtSignature`); **args is an object**, there's **no** `id` |
| Tool schema | `{type:"function", function:{name,description,parameters}}` | `tools:[{functionDeclarations:[{name,description,parameters}]}]` (OpenAPI subset) |
| Token limit | `max_tokens` | `generationConfig.maxOutputTokens` |
| Reasoning | (none in compat) | `generationConfig.thinkingConfig.{thinkingLevel\|thinkingBudget, includeThoughts}` |
| Stop reason | `finish_reason` | `candidates[].finishReason` (`STOP`/`MAX_TOKENS`/`SAFETY`/…) |
| Usage | `prompt_tokens`/`completion_tokens` | `usageMetadata.{promptTokenCount, candidatesTokenCount, thoughtsTokenCount, totalTokenCount}` |

**SSE**: `:streamGenerateContent?alt=sse` gives `data: {…}` lines, where each
object is a partial `GenerateContentResponse`
(`candidates[0].content.parts[]` with deltas). Text and thought parts
(`thought:true`) stream incrementally; a `functionCall` part usually arrives
whole, `thoughtSignature` — on it. The client derives the stop reason from
`finishReason` + the presence of `functionCall` parts.

**No `id`/`call_id` for calls** — matching `functionCall`↔`functionResponse`
goes **by name and order** (unlike OpenAI/Anthropic with `tool_call_id`). Our
`ApiToolCall.id`/`ApiMessage.tool_call_id` aren't needed for the Gemini wire
(pairing is positional), but we keep storing our own `id` — the agentic loop is
transparent to this, wire just doesn't send it.

## 3. Pitfalls

1. **`thinkingLevel` (3.x) ≠ `thinkingBudget` (2.5).** The client doesn't know
   the model's generation without a config. Options: (a) infer it from the
   model name (`gemini-3*`→level, `gemini-2.5*`→budget); (b) default to sending
   `thinkingLevel` (targeting the 3.x flagship) + document it; (c) a config
   selection field. Targeting the current flagship, a sensible default is
   `thinkingLevel`; on a miss with 2.5, a clear API error follows.
2. **Thoughts can't be fully disabled on 3.x or 2.5 Pro.** `reasoning_budget==0`
   (impersonation/auto-title) on Gemini 3 → at best `thinkingLevel:"minimal"`,
   on 2.5 Pro → minimum 128 tokens. Same as OpenAI (`effort:none` isn't
   available for all models) — we just **ignore the thought text** downstream
   (impersonation/title already do this).
3. **`maxOutputTokens` includes thought tokens** — the same class of bug as
   "thoughts ate the budget." Needs a generous default or a warning;
   `Finished(Length)` on `finishReason:"MAX_TOKENS"`.
4. **Tool schema is an OpenAPI subset, Gemini is picky.** It doesn't accept
   part of JSON Schema (`$schema`, arbitrary `additionalProperties`, some
   formats). Our schemas are generated for OpenAI, so **sanitization** (stripping
   `$schema`/unsupported bits) is likely needed in `gemini/wire.rs`. Needs
   verification against the project's live tool schemas.
5. **`role:"model"`, not `"assistant"`; no `system`/`tool` roles.** system →
   top-level; a `tool` result → a `functionResponse` part inside `role:"user"`.
   Our `ApiRole` maps to wire like Anthropic does (there the `tool` role also
   collapses into user).
6. **Signatures (see §2.3).** Without persistence — risk of `400` on Gemini 3
   when replaying history with tool calls. Solution — persist the signature
   per-tool-call (see §4.2).
7. **Embeddings** stay on OpenAI-compat (`OpenAiClient`, `…/v1beta/openai/
   embeddings`) — same as OpenAI kept `/v1/embeddings`. `cloud_embed_setup` is
   untouched. Nuance: the base URL for native chat (`…/v1beta`) and compat
   embeddings (`…/v1beta/openai`) are **different** — split it out in the
   supervisor/config (see §4.4).
8. **Base URL.** `CloudProvider::Gemini.base_url()` is currently
   `…/v1beta/openai` (for compat). The native `GeminiClient` takes `…/v1beta`
   and builds the path `/models/{model}:streamGenerateContent`.

## 4. Architecture: where this lands

Native Gemini is a **different protocol**, so it's a new `EngineBackend`
implementation, exactly like `AnthropicClient` and `ResponsesClient`. Layers
above the engine (orchestrator, agentic loop, tools, UI) are untouched — **the
feature sits behind the trait** (the main takeaway, as in the OpenAI research).

Layout: `shared/api/gemini/{client.rs, wire.rs}` (alongside `openai/`,
`anthropic/`). `shared/api/mod.rs` re-exports `GeminiClient`.

### 4.1 Fork: native client (A) or extend the compat dialect (B)?

| Option | Pros | Cons |
|---|---|---|
| **A. Native `GeminiClient` (generateContent)** | "Full-fledged": thoughts, `reasoning_effort`, `top_k` back, `thoughtsTokenCount`, "proper" signatures; a stable documented protocol; symmetric with Claude/Responses | A separate wire layer (roles/parts/signatures/schema sanitization); persisting the signature for Gemini 3 |
| B. Extend `WireDialect::Gemini` (compat) | Cheap: stop stripping reasoning, send `reasoning_effort` + `extra_body.google.thinking_config` | Compat is **in beta**, under-documented on returning summaries/signatures; **unstable** (Gemini 3 Preview rejects `reasoning_effort:"medium"`); signatures via compat are murky → risk of `400` on tool-use with Gemini 3. Tools are central to us — the risk is unacceptable |

**Recommendation — A** (as with OpenAI: a full client behind the trait, not a
fragile layer over compat). B would work as a quick temporary probe for
"thoughts without tool-use," not as the goal.

### 4.2 Contract changes (the main difference from OpenAI/Anthropic)

Gemini's signature is **per-tool-call**, not one per turn, so the existing
`ThinkingRef`/`ThinkingBlock` (one per turn) **doesn't fit**. More natural:

- **`ApiToolCall.thought_signature: Option<String>`** and **`ToolCallDelta.
  thought_signature`** — the Gemini client sets it when parsing a
  `functionCall` part; other backends leave it `None` (like `thinking`).
  `ToolCallAccumulator` accumulates the signature together with the call.
- **`ToolCallRecord.thought_signature: Option<String>`** (domain
  `entities/message.rs`, `#[serde(default, skip_serializing_if=Option::is_none)]`
  → without migration) — **persisted** for replaying history on Gemini 3.
  `record_to_api`/the reverse mapping thread it through. Other providers don't
  read this field.
- The existing `ThinkingBlock`/`ThoughtsSignature(ThinkingRef)` **isn't needed**
  for Gemini (thoughts-as-text stream as parts with `thought:true`; the
  signature rides on the call). No need to touch it — Gemini simply doesn't use
  it.

**Alternative (simpler, but less correct):** Gemini 2.5 only (signatures
optional → don't persist, a branch like Anthropic). Drops the 3.x flagship —
not recommended.

### 4.3 Sampling changes

- `supported_sampling_fields(Some(Gemini))` → `["temperature", "top_p",
  "top_k", "max_tokens", "seed", "frequency_penalty", "presence_penalty",
  "thinking", "reasoning_effort"]`. Differences from the current compat set:
  **+`top_k`** (Gemini accepts it natively), **+`thinking`/
  `reasoning_effort`**. **No `verbosity`** (that's OpenAI-Responses-specific).
  This is the **only** change needed for the settings UI and `get/set_sampling`
  — everything else follows from it (as it did with Claude/OpenAI).
- Wire mapping: `thinking:Some(true)`+`includeThoughts:true`;
  `reasoning_effort`→`thinkingLevel` (3.x) / `thinkingBudget` (2.5);
  `reasoning_budget==Some(0)`→ minimum level/`thinkingBudget:0` (see pitfall 2);
  `max_tokens`→`maxOutputTokens`;
  `temperature`/`top_p`/`top_k`/`seed`/`frequency_penalty`/`presence_penalty`→
  `generationConfig.*`.

### 4.4 Supervisor and cleaning up `WireDialect`

- `cloud_chat_setup`: the `CloudProvider::Gemini` branch builds a
  `GeminiClient` instead of `OpenAiClient::with_dialect(WireDialect::Gemini)`.
- After this, **`WireDialect::Gemini` becomes dead** (its only consumer was
  Gemini cloud). And `WireDialect::OpenAi` is already removed (Responses).
  With a single `LlamaCpp` variant left → **`WireDialect` can be removed
  entirely** along with `is_strict`/`restrict_to_strict`; `openai/wire.rs`
  gets slimmer, `OpenAiClient` remains for External + embeddings. A nice
  symmetric cleanup (like removing the `OpenAi` dialect in Responses).
- `cloud_embed_setup(Gemini)` is **untouched** — embeddings go through compat
  (`…/v1beta/openai/embeddings`). Split the base URL: the chat client —
  `…/v1beta`, the embedder — `…/v1beta/openai` (either `GeminiClient` builds
  the path itself from `…/v1beta` while `base_url()` stays for the embedder;
  or two accessors).

## 5. What else native Gemini gives us (beyond "thoughts" and effort)

- **`top_k` back** (compat stripped it) — free from §4.3.
- **`thoughtsTokenCount`** — into the token counter (we already have
  `reasoning_tokens`).
- **`stopSequences`, `responseMimeType`/`responseSchema` (structured output),
  `candidateCount`, `safetySettings`** — groundwork, out of scope.
- **Multimodality** (images/audio in `parts`) — a large separate track, out of
  scope.
- **Google tools** (`googleSearch`, `codeExecution`) — server-side analogues of
  our `web_search`/`python_exec`; conflict with the client-side agentic loop
  (like OpenAI's). Out of scope, groundwork.

## 6. Scope estimate

By the Anthropic/Responses precedent (`wire.rs` ~500–580 lines +
`client.rs` ~400, with tests and smokes):

- **Phase A** (core, no signatures): `gemini/wire.rs` (building `contents`/
  `systemInstruction`/`tools.functionDeclarations`/
  `generationConfig.thinkingConfig`; schema sanitization; parsing SSE parts —
  text, `thought:true`, `functionCall`, `finishReason`, `usageMetadata`) +
  `gemini/client.rs` (`EngineBackend`, `x-goog-api-key`, deriving
  `FinishReason`, usage) + sampling/supervisor changes. Smokes: generation;
  `Thoughts` stream with `includeThoughts`; `reasoning_effort`/`thinkingLevel`
  is accepted; a tool call without signatures (Gemini 2.5 or a single round).
- **Phase B** (signatures for Gemini 3): `thought_signature` in `ApiToolCall`/
  `ToolCallDelta`/`ToolCallRecord` (+persistence), threading through the
  agentic loop, echoing the signature on `functionCall` parts in
  `build_contents`. Smoke: two rounds of tool-use on **Gemini 3** without a
  `400` (checks the presence of a signature in the body of the second request)
  + replaying the history on the next generation.
- **Phase C** (optional): precise schema sanitization for Gemini's pickiness;
  choosing `thinkingLevel` vs `thinkingBudget` in the UI/by model name.

Realistically: A+B is one PR, comparable in size to Claude/Responses, **plus
~30 lines to persist the signature** (Gemini's unique addition). Ripple outside
`shared/api` — the contract (per-call signature + persistence), sampling, the
supervisor, generation.rs.

**Risk profile**: medium. Layers above `EngineBackend` don't change; the
unique risk is Gemini 3 signatures (needs a live check, §7). Rollback = point
the supervisor branch back at `OpenAiClient` + the Gemini dialect (which would
require temporarily bringing back `WireDialect::Gemini` if it's already been
removed — account for this in the commit order).

## 7. Open questions (need a live key)

> **Live run (Gemini 3.1 Pro Preview, 2026-07-10): GO.** All 4 smokes green
> (`MINDFORK_GEMINI_MODEL=gemini-3.1-pro-preview`, ~17s): generation, the
> "thoughts" stream (`includeThoughts`), a single tool round, and
> `tool_use_round_trips_signature` — the last one with
> `thought_signature present: true` and a successful resend of the signature
> without a `400`. The signature mechanism works; persistence (Phase B) is the
> right and sufficient approach, kept.

1. **Scope of signature validation on Gemini 3.** ~~Does the API require a
   signature on **all** historical `functionCall` parts, or only on the most
   recent turn?~~ **Resolved by design choice:** we **always** resend the
   signature (we persist it), so the failure mode is unreachable regardless of
   server behavior. The live run confirmed: the signature arrives and is
   accepted on resend (`present: true`, round 2 with no `400`). Persistence
   stays as the safe path — no need to remove it.
2. **`thinkingLevel` vs `thinkingBudget` by model name.** Does 3.x accept
   `thinkingBudget` (and vice versa)? Is generation inference needed, or is one
   parameter universal.
3. **Tool schema sanitization.** Which exact JSON Schema keys of our tools
   Gemini rejects (`$schema`, `additionalProperties`, formats) — need to see
   this against the project's live schemas.
4. **`thoughtSignature` format for parallel calls** (only the first part) —
   confirm the accumulator correctly places the signature on the right call.
5. **`role` for `functionResponse`** — confirm it's `user` (not `function`);
   and that several consecutive `functionResponse` parts can be sent in one
   `user` part (like the Anthropic merge).
6. **Does the thought summary arrive without organization verification** (for
   OpenAI, summaries are held back until verification — check whether Google
   has an analogous gate; per the docs — no, but need to see it with a live
   key).

Smokes — modeled on Anthropic/OpenAI: `#[ignore]`, key from
`MINDFORK_GEMINI_KEY`, model from `MINDFORK_GEMINI_MODEL`.

## 8. Implementation plan (detailed)

Three phases + optional C. **A** is finished and merged separately (thoughts/
effort without tool-use signatures is already useful), **B** adds signatures
for Gemini 3, **C** is polish. Commit order and precise changes:

### Phase A — native client core (thoughts + reasoning, no signatures) — DONE

> **Status (2026-07-10):** implemented on branch `feat/gemini-native-client`.
> `shared/api/gemini/{wire,client,mod}.rs` (native `GeminiClient`); `gemini`
> mode switched from OpenAI-compat to native `generateContent`; `WireDialect`
> removed entirely (orphaned — `openai/wire.rs` slims down);
> `supported_sampling_fields(Gemini)` gains `top_k`/`thinking`/
> `reasoning_effort`, loses `verbosity`; `CloudProvider::chat_base_url()`
> (native `…/v1beta`, embeddings stay on compat `…/v1beta/openai`). Generation
> inference by model name (`gemini-3*`→`thinkingLevel`, otherwise
> `thinkingBudget`) was rolled into Phase A. **13 unit tests + 3 `#[ignore]`
> smokes** (`MINDFORK_GEMINI_KEY`); fmt/clippy/test green. Live run — left to
> the user (needs a key). Next — Phase B (Gemini 3 signatures).

1. **`shared/api/gemini/wire.rs`** (new; modeled on `openai/responses/wire.rs`):
   - `build_request(req, model, stream) -> GenRequest`:
     - `systemInstruction: {parts:[{text}]}` from `req.system` (skip if empty);
     - `contents: Vec<Value>` from `build_contents(req)` (see below);
     - `tools: [{functionDeclarations: [{name, description, parameters: sanitize(schema)}]}]`
       (skip if empty); `sanitize_schema` strips `$schema`/unsupported bits
       (Phase C refines this — for A, stripping `$schema` and
       `additionalProperties` from the root is enough);
     - `generationConfig`: `maxOutputTokens`←`max_tokens`, `temperature`/
       `topP`←`top_p`/`topK`←`top_k`/`seed`/`frequencyPenalty`/
       `presencePenalty` (all `skip_if None`), `thinkingConfig` (see below);
     - `thinkingConfig`: `includeThoughts:true` when
       `thinking==Some(true)&&!force_off`; depth — `mapping(reasoning_effort)`
       (defaulting to `thinkingLevel`; §3.1); `force_off`
       (`reasoning_budget==Some(0)`) → minimum level, `includeThoughts:false`.
   - `build_contents(req)`: role `User→"user"`, `Assistant→"model"`,
     `Tool→"user"` with a `functionResponse:{name, response:{...}}` part.
     Merges adjacent same-role messages (like Anthropic). `functionCall` part:
     `{functionCall:{name, args}}` — `args` converted from a string to an
     object (a non-object → `{}`, like Anthropic). System is skipped.
     The signature part `thoughtSignature` is **Phase B** (not set in A).
   - Serde types for SSE parsing: `GenResponse { candidates:[{content:{parts:[Part]},
     finishReason}], usageMetadata }`, `Part { text?, thought?:bool, functionCall?,
     thoughtSignature? }`, `usageMetadata { promptTokenCount, candidatesTokenCount,
     thoughtsTokenCount, totalTokenCount }`. Build/parse tests — as in
     `responses/wire.rs`.
2. **`shared/api/gemini/client.rs`** (new; modeled on `responses/client.rs`):
   - `GeminiClient { http, base_url, api_key, model }`; `chat_stream`:
     `POST {base}/models/{model}:streamGenerateContent?alt=sse`, header
     `x-goog-api-key` (not `bearer_auth`); the error body isn't swallowed; SSE
     via `eventsource()`.
   - Parsing stream parts → `ChatChunk`: `text`(no `thought`)→`Text`;
     `text`+`thought:true`→`Thoughts`; `functionCall`→`ToolCall(ToolCallDelta{index,
     id: name-based or empty, name, arguments: args.to_string()})` (for Gemini
     `args` is a whole object in a single chunk; `index` = the part's ordinal
     number); `finishReason` → `Finished` (`STOP`+calls happened→`ToolCalls`;
     `MAX_TOKENS`→`Length`; otherwise `Stop`); `usageMetadata`→
     `Usage(TokenUsage{prompt=promptTokenCount, completion=
     candidatesTokenCount, reasoning=thoughtsTokenCount})`. Cancellation —
     `select! cancel`.
     - **`id` nuance**: Gemini has no `call_id`. The client synthesizes a
       stable `id` (e.g. `format!("{name}-{index}")`) — needed only for our own
       internal `functionCall↔functionResponse` pairing; in the wire (Phase A
       `build_contents`), `id` isn't serialized (Gemini's pairing is
       positional).
   - `#[ignore]` smokes (key `MINDFORK_GEMINI_KEY`, model
     `MINDFORK_GEMINI_MODEL`, default `gemini-2.5-flash`): `simple_generation`;
     `thinking_streams_thoughts` (includeThoughts→Thoughts); a tool call, one
     round.
3. **`shared/api/gemini/mod.rs`** + re-export `GeminiClient` in
   `shared/api/mod.rs`.
4. **Sampling** (`entities/sampling.rs`): `supported_sampling_fields(Some(Gemini))` →
   `["temperature","top_p","top_k","max_tokens","seed","frequency_penalty",
   "presence_penalty","thinking","reasoning_effort"]`. Update the
   `supported_fields_mirror_wire_dialect` test (Gemini now has `top_k`+
   reasoning, no `verbosity`). Check the `retain_supported` test.
5. **Supervisor** (`app/supervisor.rs`): `cloud_chat_setup` branch
   `CloudProvider::Gemini` → `GeminiClient::new(base, key, model)` instead of
   `OpenAiClient::…with_dialect(Gemini)`. Base URL for chat is `…/v1beta` (not
   `…/openai`): either a new `CloudProvider::Gemini.chat_base_url()` accessor,
   or `GeminiClient` builds the path from the common `…/v1beta`.
   `cloud_embed_setup(Gemini)` **not touched** (embeddings via compat
   `…/v1beta/openai`). Impersonation (`cloud_chat_setup`) picks up Gemini
   automatically.
6. **Removing `WireDialect::Gemini`**: the dialect is orphaned → remove the
   variant, `is_strict`/`restrict_to_strict`, and (since a single `LlamaCpp`
   remains) if desired **all of `WireDialect`**. Be careful with the order: do
   this in the same PR **after** step 5, otherwise rollback is harder (see
   §6). Update `openai/wire.rs`, the re-export in `mod.rs`, the
   `gemini_dialect_keeps_temp_and_strips_extensions` test (remove/replace).
7. **UI** (`screens/settings/`): check that the "Sampling" section for Gemini
   now shows `top_k`/`thinking`/`reasoning_effort` and hides `verbosity` — it
   all follows from `supported_sampling_fields`, no UI code changes needed
   (as with Claude/OpenAI). Run the settings tests.
   → **Gate A**: `cargo fmt`/`clippy -D warnings`/`test` green; a live smoke
   (generation + thoughts + one tool round) on Gemini 2.5/3.

### Phase B — thought signatures (Gemini 3, tool-use round-trip) — DONE

> **Status (2026-07-10):** implemented on branch `feat/gemini-native-client`.
> `thought_signature: Option<String>` added to `ApiToolCall`/`ToolCallDelta`
> (contract) and `ToolCallRecord` (domain, **persisted**
> `#[serde(default, skip_serializing_if)]` → without migration). The
> accumulator collects the signature by index; the Gemini client puts it in
> from the `functionCall` part; wire `build_contents` resends it as a sibling
> of `functionCall`; `generation.rs` persists `call.thought_signature`,
> `record_to_api` threads it through on history replay. Turn-level
> `thinking_ref` (Anthropic/OpenAI) untouched. **+3 unit tests** (signature
> emitted in wire; threading through the accumulator; record serde round-trip)
> + `#[ignore]` smoke `tool_use_round_trips_signature` (Gemini 3). fmt/clippy/
> test green (**851 passed**). **Live run — GO** (Gemini 3.1 Pro Preview): the
> signature arrived (`present: true`), round 2 with the resend passed without
> a `400`; persistence kept (§7-1).

Difference between Gemini and Anthropic/OpenAI: the signature is
**per-tool-call**, not one per turn → we don't reuse the existing
`ThinkingRef`/`ThinkingBlock`, we add a field on the call instead.

1. **Contract** (`shared/api/contract.rs`):
   - `ApiToolCall.thought_signature: Option<String>` (update `ApiToolCall {…}`
     literals: `record_to_api`, tests, the accumulator);
   - `ToolCallDelta.thought_signature: Option<String>`;
   - `ToolCallAccumulator::push` collects `thought_signature` for the right
     call by `index` (when `Some`). Other backends don't set this field →
     `None`.
2. **Domain** (`entities/message.rs`): `ToolCallRecord.thought_signature:
   Option<String>` (`#[serde(default, skip_serializing_if=Option::is_none)]`
   → without migration). **Persisted** for replaying history on Gemini 3
   (see §2.3, §7-1).
3. **Gemini client** (`gemini/client.rs`): when parsing a `functionCall` part
   with `thoughtSignature` — put it into `ToolCallDelta.thought_signature` at
   the same `index`. (For parallel calls only the first gets a signature — the
   accumulator handles this fine, the rest stay `None`.)
4. **Gemini wire** (`gemini/wire.rs`, `build_contents`): for a `functionCall`
   part, set a sibling `thoughtSignature` from `ApiToolCall.thought_signature`
   (skip if `None`). Test: the signature rides on the right part.
5. **generation.rs**: when building `records`, copy `call.thought_signature`
   into `ToolCallRecord` (one line in the loop, line ~542).
   `assistant_tool_calls(out.text, out.calls)` already carries the signatures
   in `out.calls` (the accumulator) → wire resends them in the same turn.
   Turn-level `thinking_ref`/`.with_thinking` **untouched** (that's for
   Anthropic/OpenAI; Gemini doesn't use it).
6. **request.rs** (`record_to_api`): thread `thought_signature` from
   `ToolCallRecord` into `ApiToolCall` — so historical calls carry the
   signature on the **next** generation (closes the Gemini 3 `400` on replay).
   - **Text-part signatures** (a pure-reasoning turn without a call) are
     **not persisted** (our assistant-text replay doesn't carry them); per the
     docs the hard requirement applies only to `functionCall` parts. Noted as
     a known limitation.
   - `#[ignore]` smoke `tool_use_round_trips_signature` on **Gemini 3**: round
     1 gives a call+signature; round 2 resends the signature on `functionCall`
     + the result — no `400`. Plus a replay check (a third request with the
     history from step 2 — no `400`).
   → **Gate B**: same as A + a live tool round-trip on Gemini 3.

### Phase C — polish (optional) — partially done

> **Status (2026-07-10):** two targeted correctness fixes done; the rest
> deferred.
> - **Clamp force-off on Gemini 2.5 Pro** (done): 2.5 Pro can't disable
>   thoughts (`thinkingBudget` minimum 128), so `reasoning_budget==0`
>   (impersonation/auto-title) was sending `thinkingBudget:0` → `400`. Now we
>   send `128` for 2.5 Pro.
> - **Surfacing blocks** (done): `promptFeedback.blockReason` and blocking
>   `finishReason` values (SAFETY/RECITATION/…) → a note in the feed + `warn`,
>   instead of a silent empty `Stop`.
> - **Schema sanitization** (deferred): scanning the project's tool schemas
>   shows them clean (only `enum`, which Gemini supports; no
>   `$ref`/`oneOf`/`nullable`/…) — the risk is lower than expected; fix
>   surgically based on real `400`s from a live run of the full registry.
> - **Explicit UI choice of `thinkingLevel`/`thinkingBudget`** (deferred):
>   inference by model name works for current models (3.x → level, 2.5 →
>   budget); explicit selection — as needed.

- **Schema sanitization**: the exact set of keys Gemini rejects (`$schema`,
  nested `additionalProperties`, `date-time`/… formats) — based on the
  result of a live run against the project's actual tool schemas.
- **`thinkingLevel` vs `thinkingBudget`**: inference by model name
  (`gemini-3*`→level, `gemini-2.5*`→budget) or explicit selection — if a live
  run shows one parameter isn't universal.
- Update `CLAUDE.md` (the log), `ADR 0004` (native Gemini instead of "out of
  scope"), `architecture.md §9` as needed.

### Ripple outside `shared/api/gemini/` (summary)

`contract.rs` (+2 fields, the accumulator), `entities/message.rs` (+1
persisted field), `entities/sampling.rs` (the supported set),
`app/supervisor.rs` (branch + base URL), `openai/wire.rs`+`mod.rs` (removing
`WireDialect`), `orchestrator/request.rs` (+signature),
`orchestrator/generation.rs` (+1 line for records). UI/orchestrator/agentic
loop are structurally unchanged. ~40 lines + a new module ~900 lines with
tests.

### Fork requiring a live key (decide before Phase B)

Persisting the signature (steps B-2/B-6) — in case Gemini 3 validates the
signature on **all** historical `functionCall` parts. If a live run shows it's
enough to have a signature only on the **current** turn (like Anthropic),
persistence (B-2/B-6) can be dropped, leaving the signature only in turn
memory (simpler). **By default we design with persistence** (safer); confirm
with `MINDFORK_GEMINI_KEY` before finalizing B.

---

Sources: [thinking (generateContent)](https://ai.google.dev/gemini-api/docs/generate-content/thinking),
[thought signatures](https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures),
[generateContent reference](https://ai.google.dev/api/generate-content),
[OpenAI compatibility](https://ai.google.dev/gemini-api/docs/openai.md.txt),
[Interactions API thinking](https://ai.google.dev/gemini-api/docs/thinking).
