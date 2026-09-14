# Research: `external` mode against OpenRouter

**Status:** design complete; **stage 1 implemented** — F1 (thoughts under either
field name) and the documentation half of F3/F4, see §7; merged as
[#551](https://github.com/vshylov/mindfork-rs/pull/551).

**F2 was reopened by measurement and is now fixed** (§5, §8.1, §9): the "change
nothing" conclusion had been drawn from reading, and the live run answers the
silent turns' request with a `400`. The user chose (a) — recover from the refusal
and remember it — on 2026-09-14, and stage 2 implements exactly that and nothing
else. F3(b) and F4(c) are **confirmed buildable** by the same run (M5) and stay a
separate track; F5, F6 and the rest stay proposals.

**Measured, on the second pass — by the author, not by this session.** What was
written here came from reading: the session had no network route to
`openrouter.ai` (the environment's egress proxy answers `403` to the `CONNECT`)
and no account. The author then ran it on **`deepseek/deepseek-r1` through
OpenRouter, 2026-09-14** — §8 carries the results, and **M1 is a GO**: the
thoughts this review was written about arrived. So claims are tagged:

- **[live]** — measured against the service on that run;
- **[code]** — read in this repository;
- **[crate]** — read in a dependency's own source (`~/.cargo/registry`);
- **[docs]** — OpenRouter's published protocol, or a corroborated third-party
  report of it, and **still unmeasured**.

A **[docs]** claim is exactly what [lessons.md](../lessons.md) §3 says not to
build on without measuring, which is why stage 1 changed only what is safe under
*either* answer — and why the three that are still `[docs]` (F2's cost, F3(b),
F4(c)) remain proposals rather than code.

**Related:** [docs/install.md](../install.md) §3 (the `external` mode) and §3.2 (keys),
spec §3.4 (the engine's lifecycle and settings), §6.5 (parsing "thoughts"),
§6.7 (history compression — the context window), §8 (sampling),
[ADR 0004](../decisions/0004-engine-contract-multi-provider.md) (the engine
contract), [docs/journal/engine.md](../journal/engine.md),
[external-model-name.md](external-model-name.md) (why the `model` field is sent
and why a many-model catalogue is not guessed at),
[cloud-retry-backoff.md](cloud-retry-backoff.md) (why `external` is retried like
a cloud), [grok-xai-provider.md](grok-xai-provider.md) (the last provider added
on this same client).

## 1. The question, and the short answer

> How compatible is `external` mode with the OpenRouter service?

**The core loop works, and always did**: streaming, tool calls, images, the
token counter, retries and an authenticated endpoint all speak the same wire
this client already speaks. `external` has been OpenRouter's documented home
since [external-api-key.md](../history/external-api-key.md) — it is named in
install.md §3, in the settings hint, and in the `model_name` field's reason
for existing.

What was **not** true is the part nobody had looked at: of the three channels a
reply arrives on — text, tool calls, **thoughts** — the third one was lost
entirely on this route, and lost in the quietest possible way. Beyond that, two
things degrade silently (the context window, and the sampling fields the gateway
drops), and the rest is tuning and defaults.

| | verdict |
|---|---|
| chat, streaming, tools, images, usage, retries, auth | works unchanged (§4.1) |
| **thoughts (CoT)** | **was broken** — fixed in stage 1 and measured **[live]** (§5 F1, §7, §8.1) |
| automatic compaction | inactive until the user types a window — confirmed **[live]** (§5 F3, §8.1) |
| **the silent turns** (title, compaction roll, impersonation) | **`400` on a model that always reasons** — measured **[live]** (§5 F2, §8.1) |
| sampling beyond the OpenAI set | silently dropped, while the UI offers it — confirmed field by field **[live]** (§5 F4, §8.1) |
| reasoning across tool rounds, tool-result images, `/continue` | provider-dependent; noted, not fixed (§5 F5, F6) |

## 2. What `external` mode puts on the wire today **[code]**

One client serves this mode: `OpenAiClient`
([src/shared/api/openai/client.rs](../../src/shared/api/openai/client.rs)),
built by `external_chat_setup`
([src/app/supervisor.rs:410](../../src/app/supervisor.rs)) from
`ExternalSettings` ([src/shared/config.rs:444](../../src/shared/config.rs)):
URL, optional model name, optional key (stored encrypted or named by an env
variable), `sessions`, `concurrent_calls`.

### 2.1 The request

`build_chat_request` ([wire.rs:257](../../src/shared/api/openai/wire.rs)) builds
one body for every OpenAI-compatible server. Two properties matter here:

- **the default body is plain OpenAI.** Every `SamplingConfig` field is
  `Option` and is serialized only when set, so an untouched install sends
  `model` + `messages` + `stream` + `stream_options` (+ `tools`/`tool_choice`).
  The llama.cpp extensions appear only when the user sets them in Settings →
  Sampling;
- **`stop` is never sent** (spec §7, the anti-self-cutoff invariant).

`model` is sent when the field is filled in and omitted when it is empty —
which is what a gateway needs, and the reason the field reaches the wire at all
([external-model-name.md](external-model-name.md) §2.3).

Three requests are made **besides** chat:

| call | what for | on a gateway |
|---|---|---|
| `GET {root}/health` | readiness (`probe`, [client.rs:104](../../src/shared/api/openai/client.rs)); `503` = loading, anything else = ready | harmless: a `404` reads as ready |
| `GET {root}/props` | llama.cpp's own description — context window, `total_slots`, `modalities.vision` ([client.rs:130](../../src/shared/api/openai/client.rs)) | absent → "cannot say" everywhere |
| `GET {base}/v1/models` | the model's name when the field is blank ([client.rs:153](../../src/shared/api/openai/client.rs)) | answers, with hundreds of entries → deliberately not guessed at |

`{root}` is the base URL with `/v1` stripped, so
`https://openrouter.ai/api/v1` probes `https://openrouter.ai/api/health`.

The client is wrapped in `retry::RetryBackend` for this mode
([supervisor.rs:437](../../src/app/supervisor.rs)) — decided in
[cloud-retry-backoff.md](cloud-retry-backoff.md) §2 with this exact case named:
"as often a proxy or a gateway (LiteLLM, OpenRouter) as a local
`llama-server`".

### 2.2 The response

`chat_stream` ([client.rs:180](../../src/shared/api/openai/client.rs)) reads the
SSE and yields `ChatChunk`s. What it looks at, in order: an `{"error":{…}}`
envelope inside an open `200` stream; `usage` (+ llama.cpp's `timings`);
`delta.reasoning_content` → `Thoughts`; `delta.content` → the `<think>` splitter
→ `Text`; `delta.tool_calls`; `finish_reason`.

## 3. What OpenRouter is, on the wire **[docs]**

- base `https://openrouter.ai/api/v1`, `Authorization: Bearer sk-or-…`;
  `HTTP-Referer`/`X-Title` are optional attribution headers;
- **`model` is required** and carries a `vendor/model` slug;
- **unsupported parameters are ignored, not rejected** — the documented rule is
  that a parameter the chosen model does not support is dropped and the rest
  forwarded (`provider.require_parameters: true` opts into the stricter
  behaviour of excluding such endpoints instead). **Measured, that rule has an
  exception, and it is the one that mattered here** (§8.1, M4): a *recognised*
  parameter asking for something the endpoint cannot do is a `400`, not a
  shrug — `reasoning_effort: "none"` against a model that always reasons is
  answered "Reasoning is mandatory for this endpoint and cannot be disabled".
  Read "ignored" as being about parameters the endpoint has no use for, never
  as a guarantee that a request cannot be refused **[live]**;
- **reasoning** is requested as `reasoning: { effort: … }`, with the flat
  OpenAI `reasoning_effort` supported as the legacy spelling and converted to
  it. Reports say sending **both** shapes in one request is rejected. The
  catalogue's `supported_parameters` lists `reasoning` and `include_reasoning`
  for `deepseek/deepseek-r1` and **not** `reasoning_effort` **[live]**;
- **reasoning comes back as `delta.reasoning`** (plus a `reasoning_details`
  array carrying the provider's own signed/encrypted blocks), not as
  `delta.reasoning_content`;
- **usage is always included** now, in the last SSE message;
  `stream_options.include_usage` and `usage: {include: true}` are deprecated
  no-ops, and the block carries `completion_tokens_details.reasoning_tokens`
  and a `cost`;
- the stream carries **SSE comments** (`: OPENROUTER PROCESSING`) as keep-alive
  while a provider is slow to start;
- `/api/v1/models` lists the whole catalogue, with `context_length` and
  `supported_parameters` **per model**;
- there is an OpenAI-compatible `POST /api/v1/embeddings`;
- errors use the ordinary `{"error":{"message","code"}}` envelope, including
  `402` for exhausted credits.

## 4. Where the two meet

### 4.1 What works unchanged

- **The keep-alive comments are inert** **[crate]**, and this is worth stating
  because it is the classic way a hand-rolled SSE loop breaks on OpenRouter
  (`JSON.parse(": OPENROUTER PROCESSING")`). `eventsource-stream` 0.2.3 parses
  the WHATWG grammar: a comment line is matched and discarded
  (`RawEventLine::Comment(_) => {}`), and `dispatch()` returns `None` for an
  event whose data buffer is empty — so a keep-alive produces no event, no
  chunk and not even a parse warning in the log. Pinned by a unit test
  (§7).
- **Usage** — the shape the client reads is the shape OpenRouter sends, and the
  deprecated `stream_options` it is asked with is a no-op rather than an error.
  So the exact `prompt_tokens` the compaction trigger and the status bar need
  (spec §6.7, "which token number") arrives. `timings` does not — that is
  llama.cpp's own, and only the slow-prefill note depends on it.
- **The model name** is sent from settings, and a catalogue of hundreds is
  deliberately left unguessed ([client.rs:393](../../src/shared/api/openai/client.rs)) —
  so the caption is empty unless the user typed the name, which on a gateway
  they must have done anyway for the request to work at all.
- **Auth** — one Bearer key, stored machine-encrypted or named by an env
  variable, carried by the chat request *and* by the probe.
- **Errors and retries** — `RETRYABLE_STATUSES`
  ([error.rs:51](../../src/shared/api/error.rs)) covers `429`/`5xx` with
  `Retry-After`; `402` (out of credits) is correctly **not** retried, and its
  body text reaches the user because the client does not swallow error bodies.
  A mid-stream `{"error":…}` envelope is recognised before the chunk parse.
- **Images** — base64 `data:` URLs in `image_url` parts, the same bytes every
  other provider gets.
- **Embeddings** — the `Embedder` half of this client already sends `model`, so
  the "Embeddings" tab can point at the same gateway. Untested here, and the
  endpoint's existence is a **[docs]** claim.

### 4.2 What degrades silently

- **No `/props` → no context window** → `Orchestrator::context_budget`
  ([compaction.rs:259](../../src/app/orchestrator/compaction.rs)) has no source
  and the automatic compaction trigger stays inactive (spec §6.7). `/compact`
  still works. The relief already exists — Settings → Memory → Context, "the
  context window in tokens" (`compaction.context_tokens`) — but nothing tells
  the user they now have to fill it in. **This is the one degradation that
  costs a conversation**: it ends in the provider's "context length exceeded"
  instead of a roll.
- **No `/props` → no `modalities`** → `vision()` is `Unknown`, which is the
  optimistic path by design: images are staged and a one-time note says the
  server could not say ([images.rs:260](../../src/app/orchestrator/images.rs)).
  Correct as is.
- **No `/props` → no `total_slots`** → the "Parallel sessions" hint is blank
  and `pool_for` ([pool.rs:25](../../src/app/orchestrator/pool.rs)) guards
  nothing, which is right: there is no shared KV pool to overfill on a gateway.
- **The sampling screen offers what the gateway will drop.**
  `supported_sampling_fields(None)`
  ([sampling.rs:247](../../src/entities/sampling.rs)) returns *every* field for
  `external`, because `external` means llama.cpp there. Through OpenRouter,
  `temperature`, `top_p`, `top_k`, `min_p`, `max_tokens`, `seed`,
  `frequency_penalty` and `presence_penalty` survive; `dynatemp_*`,
  `typical_p`, `top_n_sigma`, `adaptive_*`, `mirostat*`, `dry_*`, `xtc_*` and
  `samplers` do not. `repeat_penalty` deserves its own line: the gateway's
  field is spelled `repetition_penalty`, so the knob is not merely unsupported
  — it looks supported and does nothing. The same list is what `set_sampling`
  advertises to the model, and what the message's metadata snapshot records as
  "applied".
- **`finish_reason`** — `from_wire` ([contract.rs:247](../../src/shared/api/contract.rs))
  maps everything it does not know to `Stop`, so a reply cut by a content
  filter or by the gateway's own `error` reason reads as a complete one.
  Pre-existing and not specific to OpenRouter.

### 4.3 What was outright broken

**Thoughts.** The client read `delta.reasoning_content` only; OpenRouter sends
`delta.reasoning`. An unknown field deserializes away in silence, so the
thoughts of every reasoning model on this route were dropped without a log line
— and the `<think>` fallback (spec §6.5) cannot rescue it, because the gateway
has already lifted the reasoning *out* of `content`. The failure is invisible by
construction: a model that thinks and a model that does not produce the same
feed. Fixed in stage 1 (F1).

## 5. Forks

### F1. The thoughts field — **recommendation (a), implemented**

- **(a) read `reasoning_content`, fall back to `reasoning`.** One extra field
  on `Delta` and one accessor that takes both, so a server sending both names
  (reported for some gateways, which mirror one trace under two keys) cannot
  have its thoughts doubled. `reasoning_content` keeps precedence because it is
  what llama.cpp, vLLM, DeepSeek and xAI emit natively — the local stack's
  behaviour is unchanged byte for byte.
- (b) `#[serde(alias = "reasoning")]` on the existing field. Rejected: an alias
  cannot express precedence, and serde's behaviour when *both* keys are present
  is last-one-wins by document order — i.e. decided by the server's JSON
  ordering rather than by us.
- (c) also consume `reasoning_details`. Rejected **for this stage**: it is the
  echo-back half (F5), it needs a live reasoning model to shape, and the plain
  string already carries the text the feed shows.

### F2. `reasoning_effort: "none"` on the silent turns — **REOPENED: measured, and it is a `400`**

Three turns force `ReasoningEffort::None` — the title
([title.rs:156](../../src/app/orchestrator/title.rs)), the compaction roll
([compaction.rs:323](../../src/app/orchestrator/compaction.rs)) and
impersonation ([impersonation.rs:226](../../src/app/orchestrator/impersonation.rs))
— and `with_effort_none_omitted` is set for Grok only, which rejects the
*value* (grok-xai-provider.md §2.3). The question was whether a gateway needs
the same treatment, or the nested `reasoning: {effort}` shape.

**The first answer, from reading, was "change nothing":** OpenRouter accepts the
flat `reasoning_effort` as the legacy spelling and converts it, sending both
shapes is reported to be rejected, `none` is among the effort values it accepts
**[docs]** — and `external` is *also* every local `llama-server`, which reads
`"none"` as "do not think". What remained looked like a **cost** risk at worst:
a provider that cannot be muted would silently drop the parameter and the silent
turns would pay for reasoning tokens.

**Measured (M4, §8.1), that is wrong on both counts.** The exact body
`title.rs` builds, sent to `deepseek/deepseek-r1`, is answered:

```
HTTP 400
{"error":{"message":"Reasoning is mandatory for this endpoint and cannot be
 disabled.","code":400,"metadata":{"provider_name":null}}}
```

So the parameter is **not** ignored — it is read, and refused — and the cost
risk is really a **functional** one: on a model that always reasons, the title,
the compaction roll and impersonation all fail with a `400` while ordinary chat
keeps working. That is byte for byte the failure
[`with_effort_none_omitted`](../../src/shared/api/openai/client.rs) was built
for on xAI ("those three background turns would fail with a `400` while ordinary
chat kept working — a confusing failure to diagnose",
[grok-xai-provider.md](grok-xai-provider.md) §2.3), arriving by a different
route. And the nested shape is no escape: the endpoint does not refuse a
*spelling*, it refuses the *request to disable reasoning*, so
`reasoning: {enabled: false}` would be refused the same way.

The control (M4a) narrows it to one field: a minimal request carrying
`reasoning_effort: "none"` and nothing else of ours is refused identically, so
the llama.cpp-only fields the silent turns also send are not involved.

The fix therefore is not "which spelling" but "when to ask at all", and it has
three shapes — **the fork is open, and this is what it needs a decision on**:

- **(a) recover from the refusal.** The engine layer catches a `400` whose
  message says reasoning cannot be disabled and retries once without the
  reasoning-disable fields, memoising the answer per backend (the
  `TurnLoop::vision` precedent) so at most one turn per session pays the extra
  round trip. Works for every gateway, every model and every future wording
  drift, needs no configuration, and cannot regress a local `llama-server`
  because that server never sends the refusal.
- **(b) stop sending `reasoning_effort` on `external`.** The cheapest patch, and
  less lossy than it looks: `title.rs`'s own comment says the field that
  *actually* suppresses "thoughts" on llama.cpp is `reasoning_budget: 0` (plus
  `chat_template_kwargs.enable_thinking: false`), with `reasoning_effort` the
  third of three. Still a measurable risk — a llama.cpp built-in format that
  reads only `reasoning_effort` would start thinking on the silent turns — so it
  needs a local run before it ships, and it leaves the gateway paying for
  reasoning it cannot refuse.
- **(c) ask the catalogue.** `supported_parameters` for this model lists
  `reasoning` and `include_reasoning` and **not** `reasoning_effort` (§8.1), so
  the same fetch F3(b) and F4(c) want would also say "do not send this field
  here". Principled, and one request serves three features — but it is a
  heuristic: the list says what is *supported*, never that reasoning is
  *mandatory*, so it wants (a) underneath it anyway.

**Recommendation: (a), with (c) when the catalogue fetch lands** — (a) is the
one that cannot be wrong, and it is the only one that also covers LiteLLM,
vLLM-behind-a-proxy and whatever refuses next.

**User's decision, 2026-09-14: (a), and F2 alone** — F3(b)/F4(c) stay a separate
track. Implemented; see §9.

### F3. The context window — **recommendation: documentation now, (b) as a proposal**

- **(a) documentation only.** The setting exists; install.md §3 gains the
  sentence that on a gateway it is the user's job. Cheap, immediate, and
  correct for every gateway, not just this one.
- **(b) read it from the catalogue.** `GET /v1/models` already runs in this
  mode; OpenRouter's entries carry `context_length`, and vLLM's carry
  `max_model_len`. `context_budget()` could return the entry whose `id` equals
  the configured model. This is a real improvement — it would light the
  automatic trigger on the exact number the provider enforces — but it is a new
  wire-shape assumption per gateway, it needs a live check against at least two
  of them, and it belongs behind its own measurement. **Proposed as stage 2.**
- (c) a default window for `external`. Rejected outright: spec §6.7 is explicit
  that a provider which cannot say leaves the trigger inactive rather than
  acting on a guess, and a guessed window is exactly how a conversation dies at
  75% of a number nobody enforces.

### F4. The sampling fields a gateway drops — **recommendation: documentation now, (c) as a proposal**

- **(a) documentation only** — say which fields survive a gateway. Stage 1.
- (b) a fifth "provider" for gateways in `supported_sampling_fields`. Rejected:
  `external` is one mode with two populations (a local `llama-server` and a
  gateway) and nothing in the settings tells them apart, so this would need a
  user-visible flag whose only job is to grey out fields.
- (c) ask the endpoint. OpenRouter's catalogue entry carries
  `supported_parameters` for the configured model — the same fetch F3(b) makes.
  A capability answer from the server beats a table in our source (the
  `vision`/`/props` precedent, spec §3.4). **Proposed as stage 2, together with
  F3(b) since it is one request.**

### F5. `reasoning_details` across tool rounds — **recommendation: note, do not build blind**

`ApiMessage.thinking` exists and carries reasoning blocks back to Anthropic and
to OpenAI Responses; the OpenAI-compatible wire ignores it, so through a gateway
a thinking model's blocks are not echoed on the next round. For most models this
costs nothing; for Anthropic-family models reached *through* the gateway it can
cost continuity, and reports describe signature rejections when the blocks are
reconstructed inexactly. Building this without a live reasoning model would be
guessing at a shape whose whole point is byte-exactness. **Proposed as stage 3,
after M2/M5 in §8.**

### F6. `/continue` and tool-result images — **recommendation: note only**

`/continue` sends `continue_final_message` + `add_generation_prompt: false`
(vLLM's opt-in, ignored by llama.cpp builds that predate it) and a trailing
assistant message. A gateway drops the two unknown fields, and whether a
trailing assistant turn continues or restarts is then the routed provider's
decision — the same per-provider split spec §6.5/§6.4 already records
(Anthropic continues up to 4.5, OpenAI never, Grok restarts). Tool-result
images ride a `role:"tool"` message whose content is a parts array — measured
good on llama.cpp and on grok-4.5, outside the OpenAI spec's letter, and
therefore provider-dependent here too. Both are worth knowing and neither is
worth a blind change: `ServerMode::supports_continuation` cannot answer a
question whose answer lives one hop downstream.

## 6. What this does **not** cover

No OpenRouter-specific request knobs are exposed and none are proposed here:
`provider` routing and `require_parameters`, the `models` fallback array,
`transforms`, and the `HTTP-Referer`/`X-Title` attribution headers. `usage.cost`
is read and discarded — the app has no spend surface for `external`. Each is a
feature of its own, and each would make `external` less of a neutral
OpenAI-compatible mode than it is.

## 7. Stage 1 — what was implemented

1. **`wire::Delta::reasoning`** and **`Delta::thoughts()`**
   ([wire.rs:481](../../src/shared/api/openai/wire.rs)) — `reasoning_content`
   first, `reasoning` second, both taken so one trace cannot be read twice;
   `chat_stream` calls the accessor instead of reading the field directly. A
   local `llama-server` sees no change: its field is still the first one read,
   and the request is untouched.
2. **Three unit tests**
   ([client.rs](../../src/shared/api/openai/client.rs)): a gateway's
   `reasoning` becomes `Thoughts`; `reasoning_content` wins when a server sends
   both (one `Thoughts`, not two); and a `: OPENROUTER PROCESSING` keep-alive
   comment adds no chunk, no error and no terminator — the last one through a
   raw-body SSE stub, since the existing helper can only write `data:` lines.
3. **One `#[ignore]` smoke** — `a_gateway_streams_thoughts_under_its_own_field_name`,
   declared by `MINDFORK_LIVE_GATEWAY_MODEL` on the
   `MINDFORK_LIVE_TEXT_ONLY` pattern: the run states that the named model
   reasons, and the smoke then **fails** rather than skips if no thoughts
   arrive (lessons §9).
4. **Documentation** — install.md §3 gains the gateway paragraph (context
   window, the sampling subset, the required model name, the key), spec §6.5
   gains the second field name, plus CHANGELOG and the journal entry.

## 8. The live run this still owes

Stage 1's unit tests prove the parse; only a live gateway proves the claim. The
session could not run it — twice over, and both refusals are the environment's
rather than the code's: the organization's egress policy answers `403` to a
`CONNECT` for `openrouter.ai:443` (recorded by the proxy's own status endpoint,
which the session is told not to route around), and an `OPENROUTER_API_KEY`
added to the environment after this container started does not reach it.

**The live set could not have targeted a gateway at all, either** — found while
preparing these commands, and fixed with them. `live_client` names a stack by a
pair of variables (URL + key) and sent **no** `model`, so every `#[ignore]`
smoke in the repository — this client's, the orchestrator's e2e set, the
embedder's — would have answered `400 "model name is missing"` against
OpenRouter, with only the new gateway smoke passing because it sets its own. It
now derives a third variable from the same convention
(`MINDFORK_ENGINE_URL` → `MINDFORK_ENGINE_MODEL`,
`MINDFORK_EMBED_URL_ALT` → `MINDFORK_EMBED_MODEL_ALT`) and sends it when set;
unset, the request is byte-identical to before. Test-only code — nothing in the
application reads these.

So the run is one paste, from a machine that can reach the service
(PowerShell; `$env:OPENROUTER_API_KEY` is where the key already is):

```powershell
$env:MINDFORK_ENGINE_URL         = "https://openrouter.ai/api/v1"
$env:MINDFORK_ENGINE_KEY         = $env:OPENROUTER_API_KEY
$env:MINDFORK_ENGINE_MODEL       = "<vendor/model>"   # any model on the account
$env:MINDFORK_LIVE_GATEWAY_MODEL = "<vendor/model>"   # …and it must reason

cargo test a_gateway_streams_thoughts_under_its_own_field_name -- --ignored --nocapture
cargo test tool_call_is_emitted_and_parsed                     -- --ignored --nocapture
cargo test simple_generation                                   -- --ignored --nocapture
```

**Named smokes, not `cargo test -- --ignored`** — the correction the first
attempt earned. The set is ~235 smokes, 121 of them multi-round end-to-end
conversations built for a local stack where a token is free and a 20k-token
ballast costs nothing; against a metered gateway that is hours of wall clock and
a real bill, most of it spent on behaviour that has nothing to do with this
review. Three smokes answer M1 and M2 in under a minute.

### 8.1 What the run measured — `deepseek/deepseek-r1`, 2026-09-14

| | outcome |
|---|---|
| **M1 — GO** | `a_gateway_streams_thoughts_under_its_own_field_name` **green** in 49.5 s: `finish=Some(Stop)`, a full reasoning trace in `thoughts` (the model's own working of 17×23, several hundred words) *and* a non-empty answer. The gateway's `delta.reasoning` reaches `ChatChunk::Thoughts`, which is the whole claim of F1 — and the defect it fixed, since the same run on the old parse would have printed `thoughts=` empty. |
| **M2 — partly** | `tool_call_is_emitted_and_parsed` **green** in 4.3 s, so native `tool_calls` do come back through the gateway. No `400` mentioning reasoning blocks anywhere in the run, i.e. nothing yet forces F5 — weak evidence, since R1's reasoning is plain text rather than Anthropic's signed blocks. See the failure mode below, which is the more interesting half. |
| **F3 — confirmed** | `auto_compaction_fires_without_the_command_live` **failed exactly as predicted**: "nothing folded. Last exact prompt: Some(22567) tokens". That smoke sets `context_tokens: None` deliberately, so the window can only come from the engine — and a gateway has no `/props` to give it. Not a defect; §4.2 measured. |
| **usage — works** | The same failure printed the counter it was measuring against: `exact prompt: Some(4875)`, `Some(5158)`, `Some(22255)`, `Some(22567)`, each flagged exact. OpenRouter's `usage` parses, and a 22.5k-token prompt streams through the gateway without trouble. |
| **M4 — a `400`, not a bill** | The exact body `title.rs` builds, sent to `deepseek/deepseek-r1`, is answered `HTTP 400`: `{"error":{"message":"Reasoning is mandatory for this endpoint and cannot be disabled.","code":400}}`. The parameter is read and **refused**, not ignored — so the title, the compaction roll and impersonation fail on a model that always reasons, while ordinary chat keeps working. F2 reopens as a functional defect; its fork is above. |
| **M5 — both stage-2 proposals are buildable** | `deepseek/deepseek-r1` carries `context_length: 64000` **and** `supported_parameters: frequency_penalty, include_reasoning, max_tokens, presence_penalty, reasoning, repetition_penalty, response_format, seed, stop, temperature, tool_choice, tools, top_k, top_p`. The window is there for F3(b); the list is there for F4(c) — and it confirms §4.2 field by field: **none** of `min_p`, `typical_p`, `top_n_sigma`, dynatemp, adaptive, mirostat, DRY, XTC or `samplers` appears, while the penalty it does take is spelled `repetition_penalty` where we send `repeat_penalty`. Note also what is **absent**: `reasoning_effort`. |
| **M4a — the field is isolated** | The control: a *minimal* request — `model`, one user message, `stream` — plus `reasoning_effort: "none"` and nothing else, answered by the same `400`. So the refusal is that field's alone; the llama.cpp-only fields the silent turns also carry (`thinking`, `reasoning_budget`, `chat_template_kwargs`) are absent from this body and cannot be the cause. F2's fix has one target. |
| **M3** | not run. |

**The failure mode worth knowing about: a provider that does not parse its own
model's tool template.** In the first (whole-set) run, `attachment_read_e2e_live`
ended with DeepSeek's chat template in the **reply text** —
`function<|tool_sep|>attachment_read … <|tool_call_end|><|tool_calls_end|>` —
after the same turn had issued six correct native tool calls. So this is neither
"R1 cannot call tools" (M2 is green) nor our wire: on a gateway the template →
`tool_calls` parse belongs to the **routed provider**, and when it misses, the
model's raw special tokens arrive as ordinary content, the loop sees no call, and
the turn ends on junk. Nothing to fix here — parsing every vendor's template is
the rabbit hole ADR 0004 exists to avoid — but it is what a "the model went mad"
report from a gateway user will actually be, and OpenRouter's `provider` routing
controls (§6) are the lever we do not expose.

### 8.2 What is still owed

| | what to run | what would falsify the design |
|---|---|---|
| **M3** | the app itself: a chat, a `python_exec` chart, a `/file` round trip | tool-result images refused → F6 hardens into a real limit |

The outcome is recorded in [docs/journal/engine.md](../journal/engine.md), per
AGENTS.md §3.

## 9. Stage 2 — what was implemented for F2(a)

One behaviour, in the client that owns this wire
([client.rs](../../src/shared/api/openai/client.rs)):

- `send_chat` is the single attempt (build the body, send it cancellably, check
  the status) that `chat_stream` now calls — once normally, twice when the
  refusal arrives;
- `should_stop_asking` recognises the refusal, and each of its three conditions
  rules out a way of being wrong: the turn must actually have asked
  (`reasoning_effort == None`-effort) and the field must actually have gone out;
  the status must be `400` — a `503` carrying the same words is the retry
  decorator's business (spec §6.8), and dropping a sampling field to answer an
  outage would file it as a capability; and the message must name reasoning
  **and** its disabling. The message match is a pair of substrings rather than
  the sentence, because providers reword — and a false positive costs exactly one
  round trip, since the second attempt meets the same error and it surfaces
  unchanged;
- `reasoning_off_refused` is an `AtomicBool` on the client, so the answer is
  learned once per server rather than rediscovered per silent turn. It feeds the
  same `omit_effort_none` switch xAI is configured with — the fix reuses the
  mechanism rather than adding a second one — and is never cleared: a model that
  must reason does not change its mind mid-session, and a server swapped behind
  the URL gets a new client from `supervisor::apply`.

**Tests**: three unit tests — the recovery (and that the *second* body no longer
carries the field), the memo (a later turn is not refused again), and one
table-driven arm for the three cases that must **not** recover. Each negative
script ends with a reply that would succeed, so a wrong retry turns the turn
green and the assertion catches it; counting requests at the stub was the first
version and it was worthless, because a second request against a spent script is
refused by the OS and still arrives as "an error" — the count-based arm survived
removing the status check. All five mutations (never recover; do not remember;
drop the status check; drop the "did it ask" check; loosen the message match) are
caught now.

**Live**: `a_muted_turn_survives_an_endpoint_that_must_reason`, declared by
`MINDFORK_LIVE_MANDATORY_REASONING_MODEL` — it sends `title.rs`'s own request
shape and fails rather than skips if the turn does not complete, which before
this change was the `400` itself.
