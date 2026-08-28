# Research: the model's name in `external` mode

**Status:** design complete; **user's decision, 2026-08-28: all six forks go with
the recommendations below** (F1 = send the configured name on the wire, F2 =
`/v1/models` when it lists exactly one, then `/props`, F3 = runtime state in the
orchestrator with settings always winning, F4 = normalize only what looks like a
file, F5 = F1 for chat + impersonation + embeddings and discovery for chat only,
F6 = moot under F2(a)). Implemented — see §9.

Related: [docs/journal/ui-feed.md](../journal/ui-feed.md) ("Post-M9: the model's
name on the assistant's header"), spec §11.3 (the feed's headers), spec §11.6
(the engine settings), spec §6.1 (the engine contract),
[ADR 0004](../decisions/0004-engine-contract-multi-provider.md),
[docs/install.md](../install.md) §3 and §3.1,
[embedding-model-change-reindex.md](embedding-model-change-reindex.md) §4 (D3 —
the same question asked of the embedder),
[embedding-input-prefixes.md](embedding-input-prefixes.md) §6.3 (why a name is
not trusted as a *capability* signal).

## 1. The question, and the short answer

> Can `external` mode show the model's name on the chat screen and record it in
> the message's metadata, if the external service is able to report one?

**Yes**, and the display half needs no new plumbing at all. Everything
downstream of `EngineSettings::active_model_name()` — the feed title's
right-hand caption, the per-message metadata snapshot, the live streaming
bubble's header — is mode-agnostic and already works in `external` mode the
moment that function returns `Some`. Today it returns `None` unless the user
typed a name into the "Model (opt.)" field by hand.

So the work is three questions, not one: **where the name comes from**, **who
owns it once it arrives**, and **what shape it is displayed in**. And a fourth
that has to be settled first, because it decides the answer to the first one: a
defect found while reading the code (§2.3) — in `external` mode the configured
model name is **never sent to the server**.

## 2. What the code actually does today

Read first, because it moved the design (lessons §3).

### 2.1 One function feeds every surface

`EngineSettings::active_model_name()`
([src/shared/config.rs:435](../../src/shared/config.rs)) is the single source:

| mode | answer |
|---|---|
| `managed` | the GGUF's base name via `gguf::display_name` — no directory, no `.gguf`, no `-00001-of-00003` tail |
| `external` | `external.model_name`, verbatim, `None` when unset or empty |
| the four clouds | the provider's configured `model_name` |

Three call sites consume it:

- **the feed title's caption** — `ChatScreen::model_meta()`
  ([src/screens/chat/render.rs:16](../../src/screens/chat/render.rs)); empty
  string when the answer is `None`;
- **the turn**, resolved **once** and used twice
  ([generation.rs:677](../../src/app/orchestrator/generation.rs)) — into
  `AppEvent::GenerationStarted { model }` (the live bubble's header) and into
  `GenSpawn.model_name`, which `finalize_message`
  ([generation.rs:2732](../../src/app/orchestrator/generation.rs)) writes into
  `MessageMetadata.model`. The single read is deliberate and documented: the
  header must not be able to name a different model than the stored message
  will claim;
- **the `/continue` capability gate** — `ServerMode::supports_continuation`,
  which for `External` answers `true` regardless of the name. A discovered name
  therefore cannot change generation behaviour; this whole feature is display
  and metadata.

`interface.show_model_name` (off by default) gates only the per-message pill.
The title caption is unconditional.

### 2.2 Why `external` is blank in practice

Nothing fills `external.model_name` unless a human types it:

- `apply_engine_env` ([src/main.rs:774](../../src/main.rs)) — `MINDFORK_ENGINE_URL`
  sets the mode and the URL and **no name**. Every live-smoke run and every
  `cargo run` against a stack therefore has a nameless feed;
- the docker lab seeds it explicitly
  ([docker/lab/settings.seed.json](../../docker/lab/settings.seed.json)) —
  `"model_name": "__CHAT_MODEL__"` next to `"show_model_name": true`. That
  substitution exists precisely because the toggle would otherwise show nothing;
- the settings row is labelled "Model (opt.)" and described as required for the
  cloud, which reads — correctly, today — as "optional decoration here".

### 2.3 The defect: the configured name is never sent

`external_chat_setup` ([src/app/supervisor.rs:327](../../src/app/supervisor.rs))
builds `OpenAiClient::new(url).with_api_key(key)` and stops. It is not that it
declines to pass the model — **`settings.external.model_name` is not among its
parameters at all**, so the value cannot reach the wire. `with_model` has two
production call sites in the whole tree, both on the cloud path
(supervisor.rs:506 and :543).

`ExternalSettings::model_name` is documented as "Model name for a multi-model
server (optional)", and `OpenAiClient::model` as "cloud embeddings/a multi-model
proxy require it". The intent was there from the multi-provider commit
(`8d0d2e8`); the `external` wiring was never added.

What this costs, measured against llama.cpp's own router mode: `proxy_post`
reads `model` out of the body, and `router_validate_model` answers
`400 "model name is missing from the request"` when it is empty
(`tools/server/server-models.cpp:1550-1554`, `:1621-1628`). The same is true of
LiteLLM and OpenRouter — and install.md §3.1 recommends exactly those as
`external` targets. So today the documented gateway setups cannot work, and the
one configuration that does work (a single-model `llama-server`, which ignores
the field) is the one where the name is least needed on the wire.

The same omission is in `apply_embed`'s external arm
([supervisor.rs:210](../../src/app/supervisor.rs)); TTS
([src/shared/tts/mod.rs:157](../../src/shared/tts/mod.rs)) does pass its model.

This matters to the question asked because it decides §7 F2: if the field is
sent, then on a multi-model endpoint the configured name **is** the true name,
and discovery is only ever needed for the blank single-model case.

## 3. What a server will actually tell us (measured)

Stack: local `llama-server` **b9769 (c926ad098)**, CPU,
`-m D:/LLM/GGUF/gemma-3-4b-it-q8_0.gguf -c 512 -ngl 0 --jinja`, port 8099. Two
runs — without and with `--alias gemma-3-4b-it --api-key <key>`.

### 3.1 Three independent channels, and they agree

| channel | no `--alias` | `--alias gemma-3-4b-it` | key required |
|---|---|---|---|
| `GET /v1/models` → `data[0].id` | `D:/LLM/GGUF/gemma-3-4b-it-q8_0.gguf` | `gemma-3-4b-it` | **no** — 200 without one |
| `GET /props` → `model_alias` | same full path | `gemma-3-4b-it` | **yes** — 401 without one |
| `GET /props` → `model_path` | same full path | full path | yes |
| SSE chunk `model` (chat completions) | same full path | `gemma-3-4b-it` | yes |

Three further measured facts:

- **`/v1/models` is a public endpoint** on an authenticated server —
  `server.cpp:193-194` marks it so in a comment, and the run confirms it: 200
  with no `Authorization` header while `/props` returns
  `{"error":{"message":"Invalid API Key",…}}` with 401. An endpoint-first
  design should therefore ask `/v1/models` first, not `/props`.
- **The reply's `model` is not an echo.** A request carrying
  `"model":"totally-made-up"` came back with `"model":"gemma-3-4b-it"` — on this
  build the server substitutes its own name unconditionally
  (`task.params.oaicompat_model = meta->model_name`,
  `server-context.cpp:4110`).
- Every chunk also carries `system_fingerprint: "b9769-c926ad098"` — the build,
  not the model. Out of scope here, but it is the honest answer to "which
  llama.cpp is this".

Without `--alias`, llama.cpp reports the model as **the path exactly as typed on
`-m`**, drive letter and all (`params.model.get_name()` falls through to the
path, `server.cpp:118-121`). That single fact decides §5.

### 3.2 The echo trap on older servers

llama.cpp PR **#17668** ("server: remove default `gpt-3.5-turbo` model name",
commit `5d6bd842e`, merged **2025-12-02**, ≈b7231) changed this. Before it:

```cpp
std::string model_name = params_base.model_alias.empty()
    ? DEFAULT_OAICOMPAT_MODEL      // "gpt-3.5-turbo"
    : params_base.model_alias;
params.oaicompat_model = json_value(data, "model", model_name);
```

So an **older** `llama-server`, started without `--alias`, asked without a
`model` field — which is precisely what mindfork sends today (§2.3) — answers
`"model":"gpt-3.5-turbo"`. A flat lie, and a plausible-looking one.

On that same old server `/props` (`model_alias`/`model_path`) and `/v1/models`
(`id` = alias or path) both reported the truth. **The endpoints were always
honest; only the completion echo was not.** That asymmetry is the argument for
asking an endpoint rather than reading the reply — and, if the reply is read
anyway, for ignoring the literal `gpt-3.5-turbo`.

### 3.3 Other server families — expected, not measured

Not verified on this pass; each is a standard `GET /v1/models`:

| server | `/v1/models` typically lists | note |
|---|---|---|
| vLLM | one entry — `--served-model-name` or the HF repo id | the single-entry rule fits |
| LM Studio | every loaded/downloaded model | often several |
| Ollama | every installed model | several; `model` is required in the request |
| LiteLLM / OpenRouter | the whole catalogue | dozens to hundreds |

### 3.4 The rule that falls out

**One entry → that is the model. Several → only the configured name can say,
and on such an endpoint one must be configured anyway** (it is the routing key —
§2.3). A list with several entries is therefore not a puzzle to solve with
heuristics; it is a case where the answer is already in settings, or where the
setup is broken for a reason discovery cannot fix.

## 4. Where a discovered name would plug in

| # | shape | before the first turn? | multi-model correct? | cost |
|---|---|---|---|---|
| A | **ask the engine once per connection**, cache it, fall back to it when the config is blank | yes | n/a — needs the config there | one trait method + one `ContextDiscovery`-shaped cache |
| B | parse `model` out of the SSE chunks, emit a new `ChatChunk::Model` | no | yes, per message | a new chunk variant, threaded through the round accumulator |
| C | A as the backbone, B as a metadata-only correction | yes | yes | both |

**A has a precedent that fits exactly.** `EngineBackend::context_budget()` and
`EngineBackend::vision()` ([contract.rs:419, :439](../../src/shared/api/contract.rs))
are both "ask the server about itself", both default to `None`/`Unknown` —
"cannot say", never a guess — and both are answered by `OpenAiClient` out of
llama.cpp's `/props`. The orchestrator caches the first of them in
`ContextDiscovery` ([compaction.rs:95](../../src/app/orchestrator/compaction.rs)):
an `epoch` bumped on settings change and on a readiness flip, a `pending` flag
so the question is asked once, an `answered` flag so "cannot say" is not
re-asked forever. A third such method — `model_id()` — costs one more field on
that pattern and nothing else.

**B's problem is the one the header feature already solved.** The live bubble is
drawn from `AppEvent::GenerationStarted` before a single token arrives; a name
that lands mid-stream would **rename the header under the reader**, which is
exactly why the model is resolved once at turn start today
(journal/ui-feed.md). B can honestly feed only the *stored* metadata — and then
a bubble whose stored name differs from its live one changes on the next
activation. C is the most correct and the most moving parts; it is worth doing
only if the multi-model case is not solved by fixing §2.3.

## 5. What shape it is displayed in

`D:/LLM/GGUF/gemma-3-4b-it-q8_0.gguf` is what an un-aliased `llama-server`
answers, and it is not something to put on a chat header.
`shared::gguf::display_name` ([src/shared/gguf.rs:95](../../src/shared/gguf.rs))
already produces `gemma-3-4b-it-q8_0` from it — it is what `managed` mode uses
for the same reason.

But it splits on `/` and `\` unconditionally, so applied blindly it would also
turn `meta-llama/Llama-3-8B` into `Llama-3-8B` and
`anthropic/claude-opus-4.5` into `claude-opus-4.5` — org-qualified ids that a
vLLM or gateway endpoint legitimately reports. **Normalize only what looks like
a file** (a `.gguf` suffix, or a path separator together with one) and pass
everything else through untouched. See F4.

## 6. Scope

The chat engine is the question asked. Three neighbours share the §2.3 defect
and should be decided together (F5), not silently:

- **impersonation-external** — the same `external_chat_setup`, so the same
  missing `model`;
- **embed-external** — `apply_embed` builds its client without `with_model`
  either. Discovery is a different matter there: embeddings are deliberately
  lazy (ADR 0002) and already have a stronger identity mechanism than a name —
  the canary fingerprint of
  [embedding-model-change-reindex.md](embedding-model-change-reindex.md), whose
  §4 explicitly rates the reported name as "display metadata, not the trigger";
- **TTS-external** — already passes its model; nothing to do.

## 7. Forks for the user

**F1 — Should `external` mode send the configured model name on the wire?**
- (a) **Recommended:** yes. Pass `external.model_name` into
  `external_chat_setup` and on to `.with_model(...)`, exactly as the cloud path
  does. It makes the gateway setups install.md §3.1 already recommends actually
  work, costs nothing on a single-model `llama-server` (measured: an unknown
  `model` is ignored), and turns the field into the routing key it is documented
  to be. An empty field sends nothing — byte-identical to today.
- (b) Leave it display-only and add a second field for the routing id. Two
  fields for one string; rejected unless someone wants the decorative name to
  differ from the real one.

**F2 — Where does a discovered name come from?**
- (a) **Recommended:** `GET /v1/models` when it lists **exactly one** entry;
  otherwise llama.cpp's `/props` (`model_alias`, falling back to `model_path`).
  Public endpoint first — it answers without a key (§3.1) — llama.cpp-specific
  second.
- (b) `/props` first. A llama.cpp bias, and it 401s on an authenticated server.
- (c) The reply's own `model` field (shape B of §4).
- (d) (a) plus (c) as a metadata-only correction (shape C).

**F3 — Who owns the discovered name?**
- (a) **Recommended:** runtime state in the orchestrator, mirroring
  `ContextDiscovery` — epoch, `pending`, `answered`, `known`, invalidated on a
  settings change and on a readiness flip. `active_model_name()` stays a pure
  config function; the fallback is applied where the turn and the caption read
  it. Nothing is written to `settings.json`, and a value the user typed always
  wins.
- (b) Write the discovered name back into `config.engine.external.model_name`.
  Rejected: it silently edits the user's settings, and under F1(a) a discovered
  name would then be **sent** as a routing key — a behaviour change nobody
  asked for.

**F4 — Normalization.**
- (a) **Recommended:** `gguf::display_name` only when the answer looks like a
  file; otherwise verbatim (§5).
- (b) Always normalize. Simpler, lossy on `org/model` ids.
- (c) Never. Puts a Windows path on the chat header.

**F5 — How far does F1 reach?** Chat only, or chat + impersonation + embeddings
(one line each, same defect)? **Recommended: all three for F1; discovery (F2/F3)
for the chat engine only** — the embedder's identity question is already
answered better by its canary (§6).

**F6 — The stale-echo guard.** Only if F2 lands on (c)/(d): ignore the literal
`gpt-3.5-turbo` (§3.2). Under (a) the endpoints are consulted first and the
question does not arise.

## 8. What it would take to build

**Unit** — the `/v1/models` URL builder next to the existing `/props` and
`/health` ones; the single-entry rule, and silence on a list of several; the
path-vs-id normalization split; the discovery cache's epoch dropping a stale
answer (the `ContextDiscovery` tests are the template); the caption and the
metadata reading the fallback; a request with a configured name carrying
`model`, and one without it staying byte-identical to today.

**Live — mandatory** (AGENTS.md §3: this touches the engine and a provider
protocol). `#[ignore]` smokes against `MINDFORK_ENGINE_URL`: a name discovered
with none configured; the discovered name matching what the reply claims; the
`--api-key` server proving the `/v1/models` 200 vs `/props` 401 asymmetry (the
local authenticated target of the live-stack notes is enough); and, for F1, a
two-model `llama-server --router` or a LiteLLM container — the only way to prove
the gateway case actually starts working.

**Docs on landing** — spec §11.3 and §11.6, install.md §3/§3.1 (the "Model
(opt.)" field stops being optional decoration), architecture §5–§6, an entry in
journal/engine.md (the wire and the discovery) and one in journal/ui-feed.md if
the caption's behaviour changes, CHANGELOG (user-visible), and the roadmap's
"Provider bridges" item, part of which F1 closes.

---

## 9. What was built, and what the live gate caught

Landed as one branch, `feat/external-model-name`, in the shape §7 records.

**The wire (F1).** `external_chat_setup` gained a `model_name` parameter and
passes it to `OpenAiClient::with_model`; `apply_embed`'s external arm does the
same. Three lines, and the field stops being decoration. An empty field still
sends no `model` key, which is what keeps a bare `llama-server` request
byte-identical to the one that shipped before.

**The question (F2).** `EngineBackend::model_id()` — a third method of the same
shape as `context_budget`/`vision`, default `None`. `OpenAiClient` answers it
from `GET /v1/models` when the catalogue holds exactly one entry, then from
`/props` (`model_alias`, falling back to `model_path`).

**The state (F3).** `orchestrator/model_name.rs` — `ModelDiscovery`, the
`ContextDiscovery` shape with two deliberate differences: the question is asked
**eagerly** (when the engine is applied and when readiness flips, because a
caption is on screen before the first message, unlike a compaction budget), and
only **when the configuration cannot name a model**. The answer reaches the
screen as `AppEvent::EngineModel(Option<String>)` — the *discovered* half only,
so the screen's preference for the configuration stays the single rule — and the
turn reads `Orchestrator::effective_model_name()`, which is still one read per
turn, so the live bubble's header and the stored metadata cannot disagree.

**The shape (F4).** `gguf::display_id` — `display_name` when the answer ends in
`.gguf`, verbatim otherwise.

### What only the live run could have caught

**`RetryBackend` did not delegate `model_id`.** Every stub test passed; the
first live run returned `None`. `external` mode wraps its client in that
decorator, and `external` is the *only* mode that asks — so the one missing
delegation was the entire feature for every real user of it, and it was
invisible to a test that talked to a client directly. This is the third
occurrence of that exact trap on this decorator (`context_budget`, `vision`,
now `model_id`), and both earlier ones left a comment saying so; the comments
did not prevent the third. What did catch it was AGENTS.md §3's "engine work
runs live before the PR" — see lessons §9.

**A turn started in the same tick as the bootstrap has no name.** Discovery is a
network round trip; the first live end-to-end smoke sent its message before the
answer landed and recorded `None`. This is correct behaviour, not a defect — a
human takes seconds to type, and blocking a turn on a round trip to satisfy a
caption would be the wrong trade — but it is why the smoke waits for
`AppEvent::EngineModel` before sending, and it is worth knowing that the very
first turn after launch can still record no name on a slow link.

### Measured, on the live stack

`llama-server` b10659, Gemma 4 31B, `MINDFORK_ENGINE_URL=http://…:8000/v1`, the
"Model (opt.)" field blank:

- `model_id()` → `gemma-4-31B_q4_0-it`, normalized from the reported
  `D:\LLM\GGUF\gemma-4-31B_q4_0-it.gguf`;
- the streaming bubble's header and the stored `MessageMetadata.model` both →
  `gemma-4-31B_q4_0-it`, read back through a fresh activation.

**Not covered live:** the multi-model gateway half of F1. Proving it needs a
`llama-server --router` or a LiteLLM container with two models; the unit test
asserts the request body carries the configured name, which is the half the app
controls, and llama.cpp's `router_validate_model` is the half that was measured
by reading it. Worth a real run the first time anyone points this mode at a
gateway.
