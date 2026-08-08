# Research: a `grok` mode (xAI) as a fourth cloud provider

**Status:** **implemented** (2026-08-09). User's decision, 2026-08-09: **F1 = A**
(plain Chat Completions via the existing `OpenAiClient`) and **F2 = `grok`** for
both the mode label and the stored-key name; F3–F6 went with the
recommendations below. Outcome recorded in
[ADR 0004](../decisions/0004-engine-contract-multi-provider.md) and
[docs/journal/engine.md](../journal/engine.md).

Everything in §2 was verified
**live** against `https://api.x.ai/v1` with a real key (`GROK_API_KEY`), not
read off the docs — the docs are thin and the per-model parameter rules are not
in them at all.

Related: [ADR 0004](../decisions/0004-engine-contract-multi-provider.md) (the
engine contract and the flat provider taxonomy),
[openai-responses-client.md](openai-responses-client.md),
[gemini-native-client.md](gemini-native-client.md),
[api-key-storage.md](api-key-storage.md).

## 1. Problem

The app supports four inference sources behind `EngineBackend`: managed
`llama-server`, an arbitrary external OpenAI-compatible server, and the clouds
OpenAI (Responses), Gemini (native `generateContent`), Anthropic (Messages).
The request is to add **xAI Grok** as a first-class mode.

The question this doc answers is not "is it possible" — it is **which of xAI's
three protocol surfaces we bind to**, and **how little code it takes**, because
the answer turns out to be much smaller than it was for Gemini or Anthropic.

## 2. What we learned about the API (verified live, 2026-08-09)

### 2.1 Base URL, auth, and the readiness probe

- Base URL `https://api.x.ai/v1`, auth `Authorization: Bearer <key>` — the same
  shape `OpenAiClient::with_api_key` already sends.
- `GET /health` and `GET /v1/health` → **404**. `OpenAiClient::probe` treats
  404 as "alive, not loading" (only 503 means "still loading"), so the External
  path's background probe reports `Ready` correctly with no change. Cloud modes
  skip the probe entirely (immediate `Ready`), so this only matters for the
  no-code baseline in §3.

### 2.2 Three protocol surfaces, all three live

| Surface | Endpoint | Verified |
|---|---|---|
| OpenAI Chat Completions | `POST /v1/chat/completions` | streaming, tools, tool-result round-trip, `usage` |
| OpenAI Responses | `POST /v1/responses` (+ `/compact`, `GET`/`DELETE /{id}`) | streaming, tools, reasoning items with `encrypted_content` |
| Anthropic Messages | `POST /v1/messages` | non-streaming reply with `thinking` blocks |

The Anthropic-compatible surface returns `thinking` blocks whose `signature` is
an **empty string** — a detail that matters if we ever pointed `AnthropicClient`
at it, since our Anthropic path resends the signature on tool-use turns.

### 2.3 Reasoning is exposed on the plain Chat Completions path

This is the finding that shapes everything else. On `/v1/chat/completions`,
Grok streams its reasoning in **`delta.reasoning_content`** — byte-for-byte the
field `OpenAiClient` already parses into `ChatChunk::Thoughts` (it was built for
llama.cpp, which uses the same field). Non-streaming replies carry
`message.reasoning_content`, and `usage.completion_tokens_details.reasoning_tokens`
is populated — also already parsed.

Neither OpenAI nor Gemini nor Anthropic gives us reasoning on their
OpenAI-compatible path; each needed a native client. Grok does.

`reasoning_effort` (top level, Chat Completions):

| Value | grok-4.5 | grok-4.3 | grok-4.20-non-reasoning |
|---|---|---|---|
| `minimal` / `low` / `medium` / `high` / `xhigh` | 200 | 200 | **400** — the parameter itself is rejected |
| `none` | **400** "does not support `reasoning_effort` value `none`" | — | — |

So: reasoning cannot be switched off on a reasoning model, and the parameter
cannot be sent at all to a non-reasoning one. Our `ReasoningEffort` enum already
has exactly the accepted values plus `None` — the `None` variant is the trap.

### 2.4 Tool calling needs no signature dance

Standard OpenAI shape: `choices[].message.tool_calls[]` with
`id`/`function.name`/`function.arguments`, `finish_reason: "tool_calls"`.

**Verified:** replaying the assistant turn with `tool_calls` and **no**
`reasoning_content`, followed by a `role: "tool"` message, returns **200**. On
the Responses surface, replaying the `function_call` **without** the preceding
`reasoning` item also returns 200 (and replaying it *with* the item works too).

This is the opposite of every other cloud we support: Anthropic requires the
thinking block + signature in the same turn, OpenAI Responses requires the
reasoning item with `encrypted_content` before its own `function_call`, and
Gemini 3 requires a per-call `thoughtSignature` or it 400s. Grok requires
nothing. **No contract changes, no new persisted fields, no migration.**

### 2.5 Sampling: what is honored, what 400s, what is silently dropped

A status code alone can't tell "honored" from "ignored", because xAI ignores
unknown fields (`{"totally_bogus_field": 1}` → 200). Sending a **wrong type**
separates them: a field in xAI's request schema fails deserialization (422), an
unknown one is dropped. Both probes were run.

| Field | Result on `grok-4.5` |
|---|---|
| `temperature`, `top_p`, `seed` | **honored** (422 on a wrong type → in the schema) |
| `max_tokens`, `max_completion_tokens`, `logprobs`, `response_format` | in the schema |
| `reasoning_effort` | honored, see §2.3 |
| `presence_penalty`, `frequency_penalty` | **400** — "Model grok-4.5 does not support parameter presencePenalty" (also 400 on `grok-4.3` and `grok-4.20-non-reasoning`) |
| `stop` | **400** on `grok-4.5` and `grok-4.3`; 200 on `grok-4.20-non-reasoning` |
| `top_k`, `min_p`, `repetition_penalty` | **silently ignored** (200 on a wrong type → not in the schema) |
| `verbosity` (top level) | **silently ignored** — not a Chat Completions field |
| llama.cpp extensions (`dynatemp_*`, `samplers`, `mirostat`, `thinking`, `reasoning_budget`, `chat_template_kwargs`, DRY/XTC…) | silently ignored |

Two things fall out of this:

- **The `stop` invariant saves us.** The project deliberately never sends
  `stop` (anti-self-cutoff, CLAUDE.md "Key architectural decisions"). Had we
  sent it, Grok would 400 on its two best models.
- **The penalties are the only live hazard.** They are in
  `SETTABLE_SAMPLING_FIELDS`, so under the no-code baseline (§3) a user who sets
  one gets a hard 400 on every request until they clear it.

`stream_options: {include_usage: true}` is honored — the trailing usage-only
chunk arrives, so token accounting and compaction triggers work unchanged.
Prompt caching happens automatically (`usage.prompt_tokens_details.cached_tokens`
was non-zero on the second request of a conversation) with nothing to configure.

### 2.6 Models and context windows (`GET /v1/models`, live)

| Model | Context | Notes |
|---|---|---|
| `grok-4.5` (alias `grok-4.5-latest`) | 500k | xAI's recommended default; reasoning, cannot be disabled |
| `grok-4.3` (aliases `grok-latest`) | 1M | reasoning |
| `grok-4.20-0309-reasoning` (alias `grok-4.20`) | 1M | reasoning |
| `grok-4.20-0309-non-reasoning` | 1M | rejects `reasoning_effort` |
| `grok-4.20-multi-agent-0309` | 1M | `reasoning_effort` selects **agent count**, not depth; accepts `xhigh` |
| `grok-build-0.1` | 256k | |
| `grok-imagine-image`, `grok-voice-think-fast-*` | — | image / audio, out of scope |

Pricing is returned inline per model; `usage.cost_in_usd_ticks` comes back on
every reply — an xAI extension we ignore.

### 2.7 No embeddings

`GET /v1/embedding-models` → `{"models": []}`. `POST /v1/embeddings` exists but
rejects language models. **xAI ships no embedding model**, exactly like
Anthropic. RAG under a Grok engine must use a separate embedder (local
`llama-server --embeddings`, OpenAI, or Gemini) — the mechanism already exists
(ADR 0002), and the `Claude`-shaped handling in `apply_embed` is the precedent
to copy.

## 3. The baseline: it already works today, with no code at all

Because §2.3 and §2.4 hold, Grok is reachable **right now** through
`external` mode:

- URL `https://api.x.ai/v1`
- Model `grok-4.5`
- API key env `GROK_API_KEY`

That gives streaming, "thoughts", tool calling, token counting and the health
chip. Three things are wrong with leaving it there, and they are the actual
justification for a dedicated mode:

1. **Sampling.** External means `supported_sampling_fields(None)` — the whole
   llama.cpp set is offered in the UI and sent. Most of it is silently dropped
   (misleading), and `presence_penalty`/`frequency_penalty` produce a hard 400.
2. **Keys.** External resolves the key only from an env variable
   (`api_key_env`); the saved machine-bound key (ADR 0008) is provider-indexed
   and deliberately does not apply to an arbitrary URL. So a Grok user must set
   an OS env variable, unlike an OpenAI/Gemini/Claude user.
3. **Discoverability.** "external + a URL you have to know" is not a provider
   the user can find in the mode selector.

**This baseline is worth confirming by hand before any code is written** — it is
the cheapest possible end-to-end check of the whole path.

## 4. Architecture: where this lands

The flat taxonomy of ADR 0004 was designed for exactly this. Grok becomes a
fourth `CloudProvider` and a sixth `ServerMode`. The full change surface, from
a grep of every non-test site that mentions `Claude`:

### 4.1 `shared/config.rs`

- `ServerMode::Grok`, `ImpersonationMode::Grok`, `CloudProvider::Grok`.
- `CloudProvider::base_url` / `chat_base_url` → `https://api.x.ai/v1`
  (identical for both — no compat/native split, unlike Gemini).
- `CloudProvider::key()` → `"grok"` (a persisted string, see fork F2).
- A `grok: CloudSettings` field on `EngineSettings`,
  `ImpersonationEngineSettings`, `EmbedSettings` (`#[serde(default)]`, so old
  `settings.json` reads unchanged — no migration).
- `cloud_ref`/`cloud_mut` currently take one `&CloudSettings` per provider
  positionally; a fourth makes that six arguments. See fork F5.

### 4.2 `app/supervisor.rs`

- `cloud_chat_setup`: one match arm. Which client it constructs is fork F1.
- `apply_embed`: `ServerMode::Grok` joins the `Claude` arm (warn + 
  `unavailable_embed()`), per §2.7.
- `ServerMode::Grok`/`ImpersonationMode::Grok` join the existing cloud arms in
  `apply_chat`/`apply_impersonation`.

### 4.3 `entities/sampling.rs`

`supported_sampling_fields(Some(CloudProvider::Grok))` — derived from §2.5:

```rust
&["temperature", "top_p", "max_tokens", "seed", "thinking", "reasoning_effort"]
```

No `top_k`/`min_p` (silently dropped), no penalties (400), no `verbosity`
(OpenAI-Responses-specific). This is the single source of truth for both the
settings UI and the `get_sampling`/`set_sampling` tools, so getting it right
here fixes hazard #1 from §3 everywhere at once.

The `reasoning_effort: none` trap (§2.3) needs a decision — see fork F4.

### 4.4 UI and the rest

- `screens/settings/helpers.rs`: `mode_label`/`imp_mode_label` → `"grok"`,
  `cycle_mode`/`cycle_imp_mode` order, `SERVER_MODES: [_; 6]`,
  `IMP_MODES: [_; 7]`.
- `screens/settings/catalog.rs`: three match arms gain `| ServerMode::Grok`
  (assistant / impersonation / embeddings). No new field kinds — cloud rows are
  already generic over the provider.
- `app/orchestrator/mod.rs`: `CloudProvider::Grok` in the `secrets_present`
  candidate list, so the "key saved on this machine" status row works.
- `locales/{en,ru}.json`: two description strings (`ui.settings.desc.mode`,
  `ui.settings.desc.model_name`) enumerate providers and want `grok`
  mentioned. Mode labels themselves are literals, not locale keys.
- Docs: architecture §5–§6, ADR 0004 status list, `docs/install.md` §3,
  `docs/journal/engine.md`, CHANGELOG.

Nothing in the orchestrator, the agentic loop, the tools, the feed or storage
changes — which is the whole point of the trait seam.

## 5. Forks (need the user's decision before implementation)

### F1 — which protocol surface do we bind to? *(the one that matters)*

| | A. Chat Completions | B. Responses | C. Anthropic Messages |
|---|---|---|---|
| Client | reuse `OpenAiClient` as-is | reuse `ResponsesClient` as-is | reuse `AnthropicClient` |
| New code in `shared/api/` | **none** | none | none |
| Reasoning | `reasoning_content` deltas ✔ | `response.reasoning_summary_text.delta` ✔ | `thinking` blocks ✔ |
| Sampling available | `temperature`/`top_p`/`seed`/`max_tokens` | `max_tokens` + reasoning only (our `ResponsesClient` sends nothing else) | `max_tokens` + reasoning only |
| Tool round-trip | verified 200 | verified 200 | untested |
| Extras | — | `/responses/compact`, stored responses, `encrypted_content` | — |
| Risk | none observed | ties us to xAI's Responses fidelity; `verbosity` unverified | empty `signature` in thinking blocks vs our resend logic |

**Recommendation: A.** It is the only option that preserves
`temperature`/`top_p`/`seed` — real knobs this project exposes and users tune —
and it is the path already exercised by the no-code baseline. Both B and C would
cost us sampling for features we do not use (we keep our own history, so stored
responses and `/compact` buy nothing; our compaction is client-side by design).

B is a cheap follow-up later if xAI-only features (server-side Live/X search)
become interesting: `ResponsesClient` accepts xAI's endpoint verbatim today —
verified, same event names, same `encrypted_content` shape.

### F2 — mode name and stored key: `grok` or `xai`?

`CloudProvider::key()` is persisted in `settings.json` under `api_keys` and
"must not be renamed". The company is xAI, the models are Grok, and the env
variable the docs use is `XAI_API_KEY` (the user's is `GROK_API_KEY`).
**Recommendation:** `grok` for both the mode label and the key — it matches the
existing labels, which name models/vendors as users think of them
(`claude`, not `anthropic`).

### F3 — default model in a fresh config

Other providers ship no default (empty `model_name` → `Disconnected` with a
clear message). **Recommendation:** keep that, and put `grok-4.5` in the
`ui.settings.desc.model_name` hint and `docs/install.md`. Pinning a default in
code means shipping a model name that goes stale.

### F4 — `reasoning_effort: none` and the non-reasoning model

`ReasoningEffort::None` is a legal value in our UI, and Grok 400s on it. Two
options: (a) leave it — the user gets a clear provider error in the chip; (b)
filter `None` out of the Grok wire (send nothing instead). **Recommendation:
(b)** — silently omitting a value the provider cannot express is what the other
clients already do, and a 400 mid-conversation is a bad way to learn it. The
non-reasoning model (`grok-4.20-0309-non-reasoning`) rejects the parameter
entirely, but we cannot detect that from a model name reliably; leave it as
documented user error.

### F5 — the `cloud_ref`/`cloud_mut` positional-argument shape

A fourth provider makes them six-argument functions, and a fifth would make
them eight. **Recommendation:** fold them into a small
`impl CloudProvider { fn pick<'a>(self, s: &'a Engine…) }`-style accessor (or a
`[(CloudProvider, &CloudSettings); N]` array) as part of this change — it is a
five-minute refactor now and a chore later. Cheap, but it touches three structs,
so it should be a deliberate decision rather than a drive-by.

### F6 — do we also expose xAI's server-side search?

xAI offers server-side Live Search / X Search as a request parameter. It
overlaps with our own tool layer and would be the first server-side tool in the
project. **Recommendation: out of scope** — note it in the roadmap, decide
separately.

## 6. Scope estimate

Assuming F1=A:

| Piece | Size |
|---|---|
| `shared/config.rs` (enum variants, settings field, URLs, key) | ~60 lines |
| `app/supervisor.rs` (match arms + embed handling) | ~15 lines |
| `entities/sampling.rs` (supported fields + `none` filter) | ~15 lines |
| `screens/settings/{helpers,catalog}.rs` | ~20 lines |
| `app/orchestrator/mod.rs` (secrets candidate) | 1 line |
| locales ×2, docs (ADR 0004 status, architecture §5, install §3, journal, CHANGELOG) | prose |
| Unit tests mirroring the existing Claude/Gemini ones (config round-trip, supervisor `Ready`/`Disconnected`, sampling matrix, settings rows) | ~150 lines |
| `#[ignore]` live smoke against `api.x.ai` (stream + thoughts + tool round-trip) | ~80 lines |

**One PR, roughly a day** including the live run — an order of magnitude less
than the Gemini or Anthropic tracks, because there is no new wire format.

## 7. Live-run results (2026-08-09, `grok-4.5`, real key)

| Check | Result |
|---|---|
| `GET /v1/models` | 200, 7 models listed |
| Streaming chat, `reasoning_effort: low` | `delta.reasoning_content` then `delta.content` — the exact shape `OpenAiClient` parses |
| `stream_options.include_usage` | trailing usage chunk with `reasoning_tokens`, `cached_tokens` |
| Tool call | standard `tool_calls`, `finish_reason: "tool_calls"` |
| Tool result replay **without** reasoning echo | 200 |
| `/v1/responses` with our exact `ResponsesClient` payload | 200; `reasoning` item with `encrypted_content`, same SSE event names |
| `/v1/responses` tool replay, with and without the reasoning item | 200 both ways |
| `/v1/messages` (Anthropic shape) | 200, `thinking` block with empty `signature` |
| `GET /health` | 404 → our probe reads "ready" |
| `GET /v1/embedding-models` | `{"models":[]}` |

## 7b. What shipped (2026-08-09)

Built as researched, one PR. Two things worth pinning here because they were not
visible until the code existed:

- **`reasoning_effort: "none"` was a live bug, not a nicety** (F4). §2.3 recorded
  that xAI rejects the value; what the code showed is *who asks for it* — the
  orchestrator sets it on title generation, compaction **and** impersonation, so
  without the filter ordinary chat would have worked while those three failed
  with a `400`. `OpenAiClient::with_effort_none_omitted(true)` (set only for
  Grok) drops the field; a live smoke pins it.
- **F5 grew a test, not just a refactor.** `cloud_ref`/`cloud_mut` now take a
  `CloudProvider::ALL`-ordered array indexed by `CloudProvider::index`. A
  provider added to `ALL` without its index — or in the wrong slot — would hand
  back **another provider's** key and model name with no type error to catch it,
  so `provider_index_matches_its_slot_in_all` asserts the correspondence and
  writes through `cloud_mut` to check the neighbour is untouched.

Live smokes (`MINDFORK_GROK_KEY=… cargo test grok_smoke -- --ignored`), all
green against `api.x.ai` with `grok-4.5`: reasoning deltas arrive as
`ChatChunk::Thoughts`; a tool result replays with no signature; `effort=none`
completes. 1962 unit tests green, clippy/fmt clean.

Left as documented user error, per §8: a non-reasoning model
(`grok-4.20-0309-non-reasoning`) rejects `reasoning_effort` outright, and the
model name is not a reliable way to detect that.

## 8. Open questions

- **Does `top_k`/`min_p` ever apply?** No — they are not in xAI's request
  schema (wrong-type probe returns 200). Settled, recorded here so nobody
  re-derives it.
- **Is `text.verbosity` honored on `/v1/responses`?** Accepted (200), effect
  unverified. Only matters if F1=B.
- **Do the per-model parameter rules change per model family?** They already
  do (`stop` is rejected by 4.5/4.3 and accepted by 4.20-non-reasoning). Our
  matrix targets the reasoning models; a user on a non-reasoning model gets a
  provider error rather than a silent wrong answer, which is acceptable.
- **Rate limits / tier behavior** — not probed; the failure mode is an ordinary
  429 surfaced in the error text, as with the other clouds.
