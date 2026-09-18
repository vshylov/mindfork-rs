# The model picker — stage 4b of the public-release track

Stage 4a fixed the ways a first run ends badly
([robustness-and-defaults.md](robustness-and-defaults.md)). It left one item
open, **D7**, because the user's choice turned a string edit into a feature:

> `ui.settings.desc.model_name` names `gpt-4o, gemini-2.5-pro, claude-opus-4-8,
> grok-4.5`. A hint that names a model is a hint that ages.

**Fork F4 of stage 4, the user's decision (2026-09-18, against the
recommendation):** not "update the names" but **"offer a choice from
`/v1/models`"** — the provider's own catalogue, which cannot go stale because it
is fetched, not written. This document measures what those catalogues actually
contain, decides what the picker does with them, and asks the forks.

## 1. What the user is up against today

A cloud provider is configured by typing the model's exact id into a text field.
Nothing validates it: a typo or a retired name is a `404`/`400` on the first
message, and the field's only guidance is four model names in a hint that was
written once. This is the last of the twelve blockers from the public-release
audit that touches the first ten minutes
([public-release-readiness.md](public-release-readiness.md) B10).

What the code already has:

- `OpenAiClient::listed_models()` (`shared/api/openai/client.rs:266`) fetches
  `GET {base}/models` and returns the ids — used only to discover the name a
  running server reports, never offered to the user.
- The settings screen is **told** facts it never asks for: `EngineSlots`,
  `EngineSamplingFields` ("the UI is told, it never asks", `app/events.rs:736`).
  There is no screen-initiated request for data anywhere except
  `ImportMcpServers`, which answers with a status string.
- The choice popup (`screens/settings/choice.rs`) lists fixed strings and
  commits by **cycling** to the chosen index — one `SaveConfig` per step. It
  refuses to open on an empty list, and it has no type-to-filter.

## 2. Measurements

All figures 2026-09-18, against the real endpoints with the owner's keys
(`tools`-free probe, `scratchpad/probe_catalogues.py`); the local arm against the
live stack (llama.cpp **b11009**).

### 2.1 OpenAI — 132 entries and no capability field

`GET https://api.openai.com/v1/models`, Bearer. **132** models, **1305 ms**.
An entry carries exactly `id`, `object`, `created`, `owned_by`, `shutdown_date`:

```json
{"id": "text-embedding-ada-002", "object": "model", "created": 1671217299,
 "owned_by": "openai-internal", "shutdown_date": null}
```

The list mixes every modality the account can reach: chat (`gpt-5.6-sol`,
`gpt-6-astra`, `o3`, `gpt-4o`), image (`gpt-image-2.5-flare`,
`chatgpt-image-latest`), video (`sora-2`), audio (`tts-1`, `whisper-1`,
`gpt-realtime-2.1`, `gpt-4o-transcribe`), embeddings (`text-embedding-3-small`),
moderation (`omni-moderation-latest`), and completions-era models
(`davinci-002`). **Nothing in the entry says which is which** — no
`capabilities`, no modality list, no object type beyond `"model"`.

One field is a real claim: **56 of the 132 carry a `shutdown_date`** — the day
the endpoint stops serving them (`gpt-4` and `gpt-4o-2024-05-13` → `2026-10-23`,
`babbage-002` → `2026-09-28`). That is the provider stating a model is on its way
out, which is exactly the staleness D7 is about, published by the only party that
knows.

### 2.2 Gemini — two catalogues, one of them with the answer

- Compat (`…/v1beta/openai/models`, Bearer): **58** entries, **208 ms**, only
  `id`, `object`, `owned_by`, `display_name`.
- Native (`…/v1beta/models`, `x-goog-api-key`): the same **58**, **189 ms**, with
  `supportedGenerationMethods`, `displayName`, `description`, `inputTokenLimit`,
  `outputTokenLimit`, `thinking`, `temperature`/`topP`/`topK`/`maxTemperature`.

```json
{"name": "models/gemini-2.5-flash", "displayName": "Gemini 2.5 Flash",
 "inputTokenLimit": 1048576, "outputTokenLimit": 65536,
 "supportedGenerationMethods": ["generateContent", "countTokens",
   "createCachedContent", "batchGenerateContent"], "thinking": true}
```

`supportedGenerationMethods` sorts the list the provider's own way: **41** publish
`generateContent` (chat), **3** publish `embedContent` (`asyncBatchEmbedContent`
alongside), and the remaining 14 publish only `bidiGenerateContent` (live),
`predictLongRunning` (video), `generateAnswer` or `bidiGenerateMusic`. It is not a
perfect chat filter — `gemini-2.5-flash-preview-tts` and the image models also
generate content — but it is the endpoint's claim rather than ours.

**The id carries a `models/` prefix in both catalogues.** The app's chat client
builds `{base}/models/{model}:streamGenerateContent`
(`shared/api/gemini/client.rs:102`), so what belongs in the field is
`gemini-2.5-pro` and a picked `models/gemini-2.5-pro` would produce
`…/models/models/gemini-2.5-pro:…`.

### 2.3 Anthropic — 11 entries, all of them chat

`GET https://api.anthropic.com/v1/models`, `x-api-key` + `anthropic-version`.
**11** models, **394 ms**, `has_more: false`. Every entry is a chat model, and
each carries `display_name`, `created_at`, `max_input_tokens`, `max_tokens` and a
`capabilities` tree (`image_input`, `pdf_input`, `thinking`, `effort`,
`structured_outputs`, `citations`, …). The list starts at `claude-fable-5-1` and
ends at `claude-sonnet-4-5-20250929`. Nothing has to be filtered here — and
`claude-opus-4-8`, the name our hint recommends, is **fifth** in it.

### 2.4 xAI — the right endpoint is not `/v1/models`

- `GET /v1/models`: **12** entries, **331 ms** — the 7 language models plus 5
  `grok-imagine-*` image and video models. Each carries `aliases`,
  `context_length` and a full price table.
- `GET /v1/language-models`: **7** entries, **300 ms**, under a `models` key
  (**not** `data`), each with `input_modalities` / `output_modalities` and
  `aliases`.

```json
{"id": "grok-4.6", "aliases": [], "input_modalities": ["text", "image"],
 "output_modalities": ["text"], "version": "...", "fingerprint": "..."}
```

The aliases matter: `grok-build-0.1` is what `grok-code-fast-1` resolves to, and
`grok-4.20-0309-reasoning` answers to fifteen names. A picker that lists ids
lists the canonical one.

### 2.5 A local OpenAI-compatible server

The live stack (`192.168.1.20:8000`, llama.cpp b11009, one model):

```json
{"object": "list",
 "data": [{"id": "D:\\LLM\\GGUF\\gemma-4-31B_q4_0-it.gguf", "aliases": [...],
           "created": 1789691057, "owned_by": "llamacpp", "meta": {...}}],
 "models": [{"name": "D:\\LLM\\GGUF\\gemma-4-31B_q4_0-it.gguf",
             "capabilities": ["completion", "multimodal"], ...}]}
```

Two lists in one body: the OpenAI `data` array and an Ollama-shaped `models`
array that **does** publish `capabilities`. The id is the `-m` path, backslashes
and drive letter included — the shape `model_id` already normalizes for display
(`openai/client.rs:2020`).

**Measured: a single-model `llama-server` ignores the model field entirely.** A
request naming the listed path answered `200`; so did one naming
`no-such-model-xyz`, and both replies report the loaded model. So on such a
server the picked value cannot break anything — while on a multi-model endpoint
(router mode, LM Studio, LiteLLM, a gateway) the id is precisely what selects the
model, so it must be written **verbatim**. Router mode's own catalogue was
measured in stage 4a: it lists this machine's Hugging Face cache.

The embedding server (`:8001`) answers the same endpoint with its own path.

### 2.6 What this costs

190–1300 ms per fetch, one request, no streaming. The slowest is OpenAI's, which
is also the longest list.

## 3. Decisions taken without a fork

- **N1. Managed mode is not in scope.** Its model row is a **GGUF path** on this
  machine (`engine.managed.model_path`), not a name a catalogue can offer. A file
  picker is a different feature.
- **N2. Gemini's `models/` prefix is stripped on write**, and the row shows
  `displayName`. §2.2 — anything else produces a doubled path segment.
- **N3. Every other provider's id is written verbatim**, including a llama.cpp
  path. §2.5 — on a multi-model endpoint the id is the selector, and on a
  single-model one nothing reads the field at all.
- **N4. A value the catalogue does not list is never touched.** A configured
  model that is missing from the list (a fine-tune, a name behind a proxy) stays
  in the field and is shown as the current value; the picker offers, it does not
  correct.
- **N5. Silence is never a claim.** No key, no network, an endpoint that does not
  serve the route, a body that does not parse — the row is exactly what it is
  today, a text field, with one line saying why the list is not there. Nothing is
  cleared and nothing is guessed.
- **N6. One fetch per provider per session**, kept while the settings screen is
  open and re-fetched on an explicit refresh — not on every keystroke, and never
  behind the user's back (see F4).
- **N7. The picker is its own overlay, not the existing choice popup.**
  `apply_choice` reaches an option by cycling to it, one config write per step
  (`choice.rs:106`), which is wrong for a list of 132; and a filter box on a
  six-item theme list is noise. The new overlay reuses `ListScroll` (the gate
  `tools/list_scroll_check.py` insists there is one implementation of a list's
  scroll state) and the existing popup rendering.
- **N8. D7's hint stops naming models.** With a list one key away, the
  description says what the field takes and how to get the list — it names no
  model and cannot age.

## 4. Forks

**All four decided by the user on 2026-09-18, each at the recommendation:**
F1 (a), F2 (b), F3 (b), F4 (a).

### F1. How the picker opens

- **(a) Enter on the model row opens the list**, whose first entry is "type a
  name by hand" (which opens today's editor). With no catalogue available, Enter
  opens the editor directly, as it does today. *Recommended.*
- (b) Enter keeps opening the editor; a second key (shown in the hint) opens the
  list.
- (c) A separate row under the field, "Choose from the provider's catalogue…".

(a) makes the discoverable key do the discoverable thing and degrades to typing
on every failure path; its cost is that a user who always types one name now
passes through a list to get to the editor.

### F2. What the list shows where the provider publishes no capability

OpenAI publishes none (§2.1: 132 entries, chat mixed with `whisper-1` and
`sora-2`); an external server publishes none in `data` (llama.cpp's Ollama-shaped
`models` array is not standard). Gemini, xAI and Anthropic do publish enough to
sort chat from the rest.

- (a) Everything the endpoint lists, in its own order, for every provider. No
  judgment of ours at all.
- **(b) The provider's own claim where it exists** — Gemini's
  `generateContent` / `embedContent`, xAI's `/v1/language-models`, Anthropic's
  all-chat list — **and everything, newest first, where it does not** (OpenAI,
  external), with the entries the endpoint marks as retiring (`shutdown_date`)
  shown as such. Type-to-filter in the popup handles the long lists.
  *Recommended.*
- (c) (b) plus our own name patterns for OpenAI (drop `tts-*`, `whisper-*`,
  `*-image-*`, `text-embedding-*`, `omni-moderation-*`, `sora-*`, `*-realtime*`,
  `*-transcribe*`). Shorter and friendlier — and it is our claim, which is the
  thing D7 is about: it will age, and it will hide a model OpenAI ships next
  month under a name we did not predict.

### F3. Which rows get it

`cloud_rows` is shared by the assistant and impersonation, so those two come
together; the embedder duplicates the block inline; TTS and video have their own.

- (a) The assistant's model row only.
- **(b) The assistant, impersonation and the embedder** — the three rows a first
  run has to fill, with the embedder's list filtered by `embedContent` where the
  provider publishes it. *Recommended.*
- (c) Every model row, TTS and video included. Those catalogues answer a
  different question (which model speaks, which one watches a video) and neither
  is on the first-run path.

### F4. When the catalogue is fetched

- **(a) Only when the user opens the picker** — one request, a "fetching…" line,
  then kept for the session. The app makes no network request the user did not
  ask for. *Recommended.*
- (b) In the background when the settings screen opens, so the list is instant.

(a) costs the 190–1300 ms of §2.6 the first time; (b) spends a key and a request
on every visit to the settings screen, including the ones that had nothing to do
with the model.

## 5. Live run

The stage touches provider protocols, so the live gate applies (AGENTS.md §3): an
`#[ignore]` smoke per catalogue — OpenAI, Gemini, Anthropic, xAI and the local
`llama-server` — asserting that the fetch returns a non-empty list, that the ids
are non-empty, and that the filter's outcome still contains the model each
provider is known to serve today. Plus a run of the TUI against a real provider
to pick a model and send one message with it.

## 6. Plan

1. `shared/api/catalogue.rs` — one `list_models(source)` over the four shapes
   measured in §2, returning `id` + what the endpoint published as a name +
   whether it claimed chat/embedding. Unit tests against the recorded bodies.
2. `AppCommand::ListModels { slot }` → orchestrator (it holds the applied config
   and the secrets, so no key travels on the channel) → `AppEvent::ModelCatalogue
   { slot, result }` → the settings screen. The first request the UI has ever
   made.
3. The picker overlay: filter box, `ListScroll`, "type a name by hand", the
   current value shown, Esc back.
4. The rows of F3 wired to it; D7's description rewritten (N8).
5. Documentation per AGENTS.md §4, and the live run of §5.

## 7. Outcome

**Done, 2026-09-18, live GO.**

| Fork | What it is now |
|---|---|
| F1(a) | `Enter` on a model row opens the provider's list, whose **first row is "type a name by hand"**; a slot whose provider already refused goes straight to the editor |
| F2(b) | the provider's own claim where it makes one (Gemini's `supportedGenerationMethods`, xAI's `/v1/language-models`, Anthropic's chat-only catalogue), everything it lists where it makes none, newest first, retiring entries marked |
| F3(b) | the assistant, impersonation and the embedder — `XModelName`, `IxModelName`, `EModelName` |
| F4(a) | one request per slot, on the keypress; kept for the visit, `Ctrl+R` asks again |

The code: `shared/api/catalogue.rs` (four shapes → one `CatalogModel`),
`app/orchestrator/catalogue.rs` (the slot → an address and a key),
`AppCommand::ListModels` / `AppEvent::ModelCatalogue`, and
`screens/settings/picker.rs` (the overlay, its filter, the "by hand" row).

**Gates**: fmt / clippy / test green — **3317 unit tests, 195 `#[ignore]`** (+23
unit tests, +6 live smokes).

**Live run — GO**, 2026-09-18, the owner's keys and the live stack: OpenAI 132
entries (56 retiring, every role unstated), Gemini 58 (41 chat / 3 embedding, no
`models/` left in any id), Anthropic 11 (all chat, all named, the embedder's list
empty), xAI 7 (all chat, no `imagine` among them), a `llama-server` 1 (its own
`-m` path, unstated) — and the whole path through a real orchestrator,
`ListModels` in and `ModelCatalogue` out with the server's own id
(`the_model_catalogue_reaches_the_ui_e2e_live`).

**What the measurement changed in the design after the forks were decided:**

- Gemini's catalogue **pages** — 50 of its 58 models without `pageSize`, and
  Anthropic's `limit` defaults to 20. An unpaged picker would have quietly hidden
  eight models, which is the failure this feature exists to prevent.
- The parser reads Gemini's **compat** body as well as the native one, so a slot
  pointed at `…/v1beta/openai` still gets a list (with every role unstated)
  rather than a parse failure.
- A single-model `llama-server` **ignores the model field entirely** (a request
  naming `no-such-model-xyz` answered `200` from the loaded model), so writing
  its `-m` path into the field can break nothing — while on a multi-model
  endpoint that same string is the selector.

**The cost of F2(b), visible and accepted:** sorted newest-first by the
endpoint's own `created`, OpenAI's list opens on this month's *image* models —
`gpt-live-1`, `gpt-image-2.5-*` — because OpenAI publishes nothing that would
tell them from a chat model. The filter line is what resolves it. The
alternative was our own name patterns, which would be our claim about someone
else's catalogue and would age exactly as D7's hint did.

**Not verified here, and left to the owner:** picking a model in the running TUI
and sending a message with it. The fetch, the filter, the write and the whole
command→event path are covered by tests and the live smokes; what is not is the
terminal itself, which no agent-run test can open.
