# ADR 0004 — Engine contract and multi-provider inference (no crate split)

**Status:** accepted (2026-06-26). Defines the boundaries of `shared/api`
before adding cloud inference providers (Claude/Anthropic, OpenAI, Gemini).
Builds on [ADR 0002](0002-embeddings-dedicated-server.md) (the
`EngineBackend`/`Embedder` split).

**Context:** a new requirement appeared — inference not only via a local
OpenAI-compatible server (llama.cpp `llama-server`), but also via the leading
cloud APIs: **platform.claude.com** (Anthropic), **platform.openai.com**
(OpenAI), and **aistudio.google.com** (Gemini). Native Gemini support is
**not required yet** — Gemini is accessed via its OpenAI-compatible endpoint
(`generativelanguage.googleapis.com/v1beta/openai/`), i.e. the same path as
OpenAI.

Analysis showed: the key seam — the `EngineBackend` trait
(`shared/api/backend.rs`) — already isolates the transport, and everything
above it (orchestrator, client-side agentic loop, tools, UI, three-tier
sampling) is **provider-independent**. These layers don't need to change. But
the `shared/api` layer itself is uneven and leaky: it mixes the generic
contract, llama.cpp-specific client details, and child-process management.

A second question arose: should the engine be split into separate crates for
isolation.

## Decision

### 1. No new crates

`mindfork-rs` remains a **single binary crate**. A crate split is
*packaging*, not isolation; it only pays off given a concrete trigger (an
external consumer, publishing a library, build times, compiler-enforced
boundary checks), none of which currently apply. A split would bring a
workspace, extra `Cargo.toml` files, a bloated `pub` surface (what was
`pub(crate)` would have to be opened up — in places this *weakens*
encapsulation) and version coordination — for near-zero payoff. FSD
boundaries are held by module discipline, as before.

**Isolation and crate splitting are different jobs.** We do the former; the
latter remains a cheap, reversible future option: if the boundaries are drawn
correctly now with modules and a trait, extracting crates later (when/if a
trigger appears) becomes nearly mechanical.

### 2. Improve `shared/api` isolation — together with the feature, not separately

Before adding providers, `shared/api` is reorganized **by the nature of
responsibility**, not "everything in one pile". Target layout:

```
shared/api/
├─ contract/      EngineBackend/Embedder traits + generic ChatRequest/ChatChunk/ApiMessage/ToolSchema
├─ openai/        wire + client for the OpenAI family (OpenAI itself, Gemini-compat, llama.cpp llama-server)
├─ anthropic/     AnthropicClient + its wire (/v1/messages)
├─ managed/       server.rs — launching the child llama-server (this is NOT a "client")
└─ thoughts.rs    shared streaming "thoughts" parser
```

These are the same steps needed for multi-provider support, so the isolation
comes **together with the feature**, not as a separate "for tidiness"
refactor.

### 3. Decouple sampling from the generic contract

The main leak: `ChatRequest` carries `entities::sampling::SamplingConfig`,
full of llama.cpp extensions (`dynatemp_*`, `dry_*`, `top_n_sigma`,
`mirostat`, `samplers`, …). For OpenAI this is a direct source of `400`
(a strict server rejects unknown fields), for Anthropic — dead weight.

Decision: **each client is responsible for its own wire** and filters what
it knows how to send. The generic contract keeps carrying `SamplingConfig`
as **user intent**, but the client treats it as "best effort": the OpenAI
client sends the commonly-supported set (`temperature`/`top_p`/`top_k`/
`max_tokens`/`seed`/penalties) and extensions only for a llama.cpp target;
the Anthropic client — only `temperature`/`top_p`/`top_k`, the rest is
ignored. This way the UI and `set_sampling` don't change, and
incompatibility never reaches the network. (Alternative — carving out an
"extension bag" for engine-specific fields — left for later, if filtering
turns out not to be enough.)

### 4. Cross-cutting gaps common to all clouds (Phase 0)

Independent of which provider is chosen, done once:

- **`model` field in the request.** Currently `wire.rs` doesn't send it
  (`llama-server` uses the loaded model). All clouds require the model name
  in the body — need to thread `model_name` from config into `ChatRequest`.
- **Authentication.** `OpenAiClient` doesn't handle keys. Add an API key
  (Bearer for OpenAI/Gemini-compat, `x-api-key` + `anthropic-version` for
  Anthropic).
- **Secrets not on disk.** The key is read from an **env variable**, not
  written into `settings.json` (it would land there as plain text). Config
  stores only the *name* of the env variable / a "key from environment" flag.
- **Notion of provider.** `ServerMode { Managed | External }` is extended
  with a cloud variant (External + key + protocol), or a `protocol`/
  `provider` field is introduced alongside `url`. `probe()` against
  `/health` for clouds will return `404` → the code already treats this as
  "alive, ready" (`client.rs`), so the readiness gate works without changes.

### 5. Embeddings for RAG under a cloud engine

The `Embedder` trait is already separated ([ADR 0002](0002-embeddings-dedicated-server.md)),
so the embedding source is chosen independently of the chat engine.
Important: **Anthropic has no embeddings API** — with an Anthropic engine,
RAG uses a separate embedder (local `llama-server --embeddings`, OpenAI
`/v1/embeddings`, or `UnavailableEmbedder`). The mechanism already exists, no
extra architectural work required.

## Implementation plan

1. **Phase 0 (foundation):** the `model` field across the contract; auth +
   key from env; provider in config; provider-dependent sampling filtering;
   reorganizing `shared/api` into submodules (item 2). Unblocks OpenAI and
   Gemini-compat.
2. **Phase 1:** OpenAI + Gemini (via the OpenAI-compat endpoint) — mostly
   config/UI wiring and tests (the client already speaks Chat Completions).
3. **Phase 2:** `AnthropicClient` + `anthropic` wire as a separate
   `EngineBackend` implementation (`/v1/messages`: `tool_use`/`tool_result`
   blocks, event-based SSE `content_block_delta`/`thinking_delta`,
   `max_tokens` required). Cross-check models/fields via the `claude-api`
   skill.
4. **Native Gemini — out of scope** (accessed via OpenAI-compat).

### Implementation status

- **Phase 0** — done: the `model` field (injected by the backend), Bearer
  key from env, `WireDialect` with provider-dependent filtering, provider in
  config, mode-driven UI.
- **Phase 1** — done together with Phase 0: OpenAI and Gemini (OpenAI-compat)
  work live.
- **Phase 2** — done: `AnthropicClient` + `anthropic` wire in the
  `shared/api/anthropic/` module (an `EngineBackend` implementation);
  `Claude` wired through config/supervisor/settings.
- **CoT (extended thinking) for Claude** — done on top of Phase 2:
  `wire::build_request` sends `thinking:{type:"adaptive", display:"summarized"}`
  (+ `output_config.effort`) when `thinking` is enabled; `budget_tokens`/
  `reasoning_budget` are not sent (4.x models reject them). Parsing
  `signature_delta` → `ChatChunk::ThoughtsSignature`. **With tool-use**,
  Anthropic requires returning the thinking block with its signature in the
  assistant turn of the same round — the agentic loop attaches
  `ApiMessage.thinking` (text+signature) to the turn with the calls,
  `build_messages` puts `AntBlock::Thinking` first. The signature only lives
  in the turn's memory (auto-discarded by the server between turns — not
  persisted). `supported_sampling_fields(Claude)` extended with `thinking`/
  `reasoning_effort`. Verified with live `#[ignore]` smokes against the
  Anthropic API (Phase A: "thoughts"+signature; Phase B: signature round-trip
  with tool-use without `400`). Known groundwork left — `redacted_thinking`.
- **OpenAI Responses API — done** (2026-07-10, docs/research/openai-responses-client.md):
  the `openai` mode was switched from Chat Completions to Responses
  (`POST /v1/responses`) — a new `EngineBackend` implementation
  (`shared/api/openai/responses/`, `ResponsesClient`). Gives reasoning
  summaries (`reasoning.summary:"auto"` → `ChatChunk::Thoughts`), depth
  (`reasoning.effort`, `ReasoningEffort` extended with `Minimal`/`XHigh`),
  verbosity (`text.verbosity`, new field `SamplingConfig.verbosity`).
  Tool-use round-trip — the reasoning element (`id`+`encrypted_content`) is
  resent before its own `function_call` (analogous to Anthropic's thinking
  signature): `ThoughtsSignature(String)` → `ThoughtsSignature(ThinkingRef{id,
  signature})`, `ThinkingBlock.id`. `store:false`,
  `include:["reasoning.encrypted_content"]`, `strict:false`. The
  `WireDialect::OpenAi` dialect is removed (Gemini stays on Chat Completions).
  `supported_sampling_fields(OpenAi)` = `max_tokens`+reasoning+verbosity.
  The "proxy with a key" pattern (formerly `openai`+url-override) is closed
  by `ExternalSettings.api_key_env`. Native Gemini via Responses — future
  work.
- **Native Gemini — Phase A done** (2026-07-10, docs/research/gemini-native-client.md):
  the `gemini` mode was switched from OpenAI-compat Chat Completions to
  **native `generateContent`/`streamGenerateContent`** — a new `EngineBackend`
  implementation (`shared/api/gemini/`, `GeminiClient`). Gives "thoughts"
  summaries (`thinkingConfig.includeThoughts` → `ChatChunk::Thoughts`),
  reasoning depth (`thinkingLevel` for Gemini 3.x / `thinkingBudget` for 2.5,
  inferred from the model name), `thoughtsTokenCount`.
  `supported_sampling_fields(Gemini)` += `top_k`/`thinking`/
  `reasoning_effort` (− `verbosity`). system → top-level
  `systemInstruction`, `user`/`model` roles, a tool result →
  `functionResponse` in a user turn, a call → `functionCall` (args as an
  object, no `call_id` — the id is synthesized). The `WireDialect` dialect
  is **removed entirely** (Gemini was its last consumer; `OpenAiClient`
  sends sampling as-is). Gemini embeddings remain on the OpenAI-compatible
  endpoint (`OpenAiClient`, `…/v1beta/openai/embeddings`), as with Anthropic
  RAG.
- **Native Gemini — Phase B done** (2026-07-10): `thoughtSignature` "thought
  signatures" **per tool call** (a unique difference from Anthropic/OpenAI,
  where the signature is one per turn) for Gemini 3 with tool-use.
  `ApiToolCall`/`ToolCallDelta` (contract) and `ToolCallRecord` (domain,
  **persisted**, without migration) gained `thought_signature`; the
  accumulator collects it by index, the Gemini client takes it from the
  `functionCall` part, wire resends it alongside `functionCall`,
  `generation.rs`/`record_to_api` persist and thread it through on history
  replay (otherwise Gemini 3 returns `400`). The round-level `ThinkingRef`
  (Anthropic/OpenAI) is unaffected. The "persist vs. signature-only-for-the-
  current-turn" fork is to be confirmed against a live key.
- **`shared/api` layout (§2) — done** (after Phase 2, for symmetry with
  `anthropic/`): `backend.rs` → `contract.rs` (provider-agnostic contract);
  `client.rs`+`wire.rs` → `openai/` (with private `wire`, re-exporting
  `OpenAiClient`/`WireDialect`); `server.rs` → `managed.rs`; `anthropic/` was
  already there. The public surface is unchanged (re-exported from
  `shared/api/mod.rs`); external references `shared::api::backend::*` →
  `::contract::*`. Moves done via `git mv` (history preserved). Tests/clippy/
  fmt green.

## Settings UX (`screens/settings.rs`)

Settings already has three "engine" configs each with its own mode selector:
the assistant chat engine and the impersonation engine (section "Model/
Server", subsections Assistant/Impersonation) and embeddings (section
"Tools"). Adding clouds **must not** proliferate fields — quite the
opposite, the screen becomes less cluttered.

### Field visibility by mode (key technique)

Currently `model_fields` shows **all** fields regardless of mode (in
External, useless `Binary`/`-ngl`/`--jinja`/… are visible). Switching to
**mode-driven visibility**: mode is always the first field, below it — only
fields relevant to it. Then a cloud mode is 2–3 fields against 8 for managed,
and every mode looks simpler even though there are more capabilities overall.

| Mode | Visible fields |
|---|---|
| Managed (local `llama-server`) | binary, model (`-m`), ngl, ctx, jinja, no_mmap, host, port |
| External (own OpenAI URL) | url, model (opt.), API key (env variable name, opt. — for a proxy/gateway with auth) |
| OpenAI / Claude / Gemini (cloud) | model, API key (env variable name), base URL (opt., override) |
| Shared (impersonation only) | — (reuses the assistant's engine) |

### Mode taxonomy — flat

`Mode` is extended with providers as equal variants of one `←/→` cycle:
`Managed | External | OpenAI | Claude | Gemini` (impersonation adds
`Shared`). Chosen flat (rather than "Cloud" + a sub-choice of provider): one
selector, no nesting, a single mental question "where does inference come
from". The downside — a cycle of 5–6 items — is offset by mode-driven
visibility (fields under each mode are short) and a `field_description` hint
below the selector.

### Secrets — env variable name, not the key

Cloud modes show the "API key" field as the **name of an env variable**
(e.g. `ANTHROPIC_API_KEY`), not the secret itself. Only the name is written
to `settings.json`; the key is read from the environment at startup/request
time (see item 4). If the variable isn't set — a warning appears under the
section (the same mechanism as field hints).

### Embeddings and impersonation

- **Embeddings**: cloud embeddings exist only for OpenAI/Gemini (Anthropic
  has none). To avoid introducing a separate "provider" for RAG, cloud
  embeddings are folded into an extended External: `url + API key(env) +
  model`. No new selector is added.
- **Impersonation**: `Shared` works with a cloud unchanged (reuses the
  assistant's engine, no extra fields); other modes are the same as for the
  assistant.

### Config shape (Phase 0)

`EngineSettings`/`ImpersonationEngineSettings` gain fields for the cloud:
`model_name: Option<String>`, `api_key_env: Option<String>`, an optional
`base_url` override; `ServerMode` gains cloud variants. All under
`#[serde(default)]` — old `settings.json` files are read without migration
(project invariant).

## Consequences

- **Plus:** multi-provider support is achieved by adding trait
  implementations; layers above `EngineBackend` are untouched. `shared/api`
  isolation improves naturally, as part of the feature. Crate splitting
  remains a cheap option.
- **Plus:** decoupling sampling eliminates a class of `400` errors on strict
  servers.
- **Minus:** the generic `ChatRequest` still formally knows about
  `SamplingConfig` (full decoupling via an extension bag is deferred) —
  acceptable, since the "best effort" interpretation is localized to the
  clients.
- **Minus:** Anthropic requires a separate wire layer (a different protocol)
  — this is a deliberate, medium-sized piece of work, isolated behind the
  trait.
- **Risk:** part of the sampling UI is meaningless for cloud models; it's
  worth hiding it per selected provider or silently not sending it (resolved
  in the UI in Phase 1/2).
