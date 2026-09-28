# Research: a mode of its own for OpenRouter

**Status: research complete, all forks decided — the track is open, stage 1 in
progress.** Measured against the service on **2026-09-29** with a paid account;
every number below is from that day. **User's decision, 2026-09-29: every
recommendation of §6 is taken as written** — a provider of its own, in all five
slots, over the four stages of §7.

**The question:**

> Is a separate `openrouter` mode possible and worth having, next to `external`?
> And can the same mode serve embeddings, impersonation, speech and watching a
> YouTube video?

**Related:** [openrouter-external.md](openrouter-external.md) (the review of
`external` against this gateway, and what it left out — §6 there),
[gateway-capabilities.md](../history/gateway-capabilities.md),
[gateway-images-and-continue.md](../history/gateway-images-and-continue.md),
[gateway-thinking-switch.md](../history/gateway-thinking-switch.md),
[gateway-vision-catalogue.md](gateway-vision-catalogue.md),
[grok-xai-provider.md](grok-xai-provider.md) (the last provider added, and the
wrong-type method used in §3.5), [model-picker.md](model-picker.md),
[tts.md](tts.md), [youtube-integration.md](youtube-integration.md),
[embedding-model-change-reindex.md](embedding-model-change-reindex.md),
[ADR 0004](../decisions/0004-engine-contract-multi-provider.md),
[ADR 0006](../decisions/0006-data-schema-versioning.md),
[ADR 0008](../decisions/0008-api-key-storage.md),
[ADR 0009](../decisions/0009-tts-speech-synthesis.md), spec §3.4, §6.4, §8, §9.9, §11.6,
§11.8, §11.9, §12.2, [lessons.md](../lessons.md) §3 and §8.

Claims are tagged the way the earlier review tagged them:

- **[live]** — measured against the service on the day above;
- **[code]** — read in this repository;
- **[api]** — read in OpenRouter's own OpenAPI document (`/openapi.json`, 97
  paths on that day);
- **[docs]** — OpenRouter's prose documentation, **unmeasured**.

One **[docs]** claim turned out false when measured (§4.5), which is
[lessons.md](../lessons.md) §3 once more.

## 1. The short answer

**Possible — in all five slots — and worth it.** The gateway has an endpoint for
each of them, every one was exercised live, and two of the five cannot be reached
through `external` at all.

| slot | through the gateway | through `external` today | what a mode of its own takes |
|---|---|---|---|
| chat | yes — measured in the earlier review | works, by inference (§2.3) | a provider arm, a dialect, the picker |
| impersonation | yes — the same request | works, with a second URL and key to type | the same arm |
| embeddings | **yes** — 33 models, the OpenAI shape, `baai/bge-m3` among them (§4.3) | works, with a third URL and key | the same arm + a retry |
| speech | **yes** — `POST /audio/speech`, 21 models (§4.4) | **cannot**: asks for `wav`, which the gateway refuses | a client of its own |
| YouTube video | **yes** — Gemini behind the gateway takes the link (§4.5) | **cannot**: the slot is Gemini-only | a second `VideoUnderstanding` |

The case for the mode is not that `external` fails at chat — it does not — but:

1. **switching costs three fields per slot** (§2.1), where a provider costs one
   key and a mode row;
2. **speech and video have no road at all** for a user whose only account is
   this one (§2.2);
3. **`external` recognises a gateway by inference**, and the inference has
   holes (§2.3); a mode knows;
4. **a handful of defects** of "OpenRouter through `external`" have no honest
   fix inside a neutral mode (§2.4).

What it costs is a provider's worth of code in each layer, plus two clients —
four stages, §7.

## 2. Why `external` is not the answer

### 2.1 What switching costs today **[code]**

`external` is **one section per slot** — `url`, `model_name`, a key of its own
([config.rs:482](../../src/shared/config.rs), `ExternalSlot` in
[secrets.rs](../../src/shared/secrets.rs)) — and four slots have one: chat,
impersonation, embeddings, speech. A user with a local `llama-server` **and** an
OpenRouter account who wants to move the chat from one to the other retypes the
URL, the model and the key; to move the three slots that can work against this
gateway (§2.2), nine fields. Nothing is kept for the way back, because the
section has one set of values.

A cloud provider is the opposite on every count: its section is kept per
provider ("so switching providers doesn't lose the other's values",
[config.rs:519](../../src/shared/config.rs)), and its key is **one** secret
shared by every slot of that provider
([`SecretSlot`, config.rs:703](../../src/shared/config.rs), ADR 0008 §3). That is
the shape the request describes: `external` stays the local server's, and the
gateway becomes a mode one cycles to.

### 2.2 What `external` cannot do against this gateway

- **Speech.** `TtsMode::External` asks for `response_format: "wav"`
  ([tts/openai.rs:81](../../src/shared/tts/openai.rs)) **[code]**, and the
  gateway's schema takes two values: `400 … expected one of "mp3"|"pcm"`
  **[live]**. The OpenAI mode with its base URL overridden does ask for `pcm`,
  but it reads the key from the `openai` slot and plays every answer at a
  hard-coded 24 kHz ([tts/openai.rs:20](../../src/shared/tts/openai.rs)) — while
  four of the gateway's models answer at 44.1 kHz (§4.4), which would play at
  0.54× speed. Read from the code and the measurement; not run.
- **Video.** The slot has no provider selector: `resolve_config` takes the
  stored **Gemini** key and the native Gemini base URL
  ([video/mod.rs:99](../../src/shared/video/mod.rs)) **[code]**.

### 2.3 What `external` infers, and a mode would know **[code]**

The code never looks at the host. An endpoint is a gateway when `GET
{url}/models` holds an entry whose `id` **equals** the configured model and that
entry carries metadata
([client.rs:225](../../src/shared/api/openai/client.rs),
`Orchestrator::endpoint_catalogued`,
[compaction.rs:340](../../src/app/orchestrator/compaction.rs)). Five behaviours
hang on that one lookup, each by a slightly different condition: the `/continue`
gate, re-homing a tool's images, the thinking switch, vision, the narrowed
sampling list. So:

- a slug the catalogue does not list **verbatim** turns all five off in silence;
- an empty model name does too — no lookup is made;
- the probe still asks `/health` every 60 s
  ([supervisor.rs:39](../../src/app/supervisor.rs)) and `/props` on every
  `vision()` and window question: both are `404` on this host **[live]**, read
  as "ready" and "cannot say".

### 2.4 Defects of "OpenRouter through `external`", measured here

| | finding | evidence |
|---|---|---|
| D1 | **`repeat_penalty` is offered and does nothing.** The offer is translated (`catalogue_aliases`, [sampling.rs:262](../../src/entities/sampling.rs)) and the wire is not — decision G3(ii) of the capabilities track, "the wire keeps sending everything". | **[live]** a wrong-typed `repeat_penalty` is a `200`, a wrong-typed `repetition_penalty` a `400` naming the field (§3.5) |
| D2 | **The picker lists 73 models that cannot chat.** Every `:batch` slug is in `/models`, and asked to chat answers `404 … cannot be used with the chat/completions endpoint`. | **[live]** two slugs tried |
| D3 | **The picker shows an id and nothing else** — 460 rows with no window, price or modality, although the same response carries all of them. | **[code]** the OpenAI shape reads `id`, `created`, `shutdown_date` ([catalogue.rs:230](../../src/shared/api/catalogue.rs)) |
| D4 | **Two probes that can only fail**: `/health` every minute, `/props` on every question about the window or vision (§2.3). | **[live]** `404` on both, under `/api` and under `/api/v1` |

D1 has no fix inside `external`: that mode is also every local `llama-server`,
which reads the other spelling.

## 3. What the gateway is, on the wire

### 3.1 Endpoints **[api]**, each exercised **[live]** unless marked

| endpoint | for |
|---|---|
| `POST /chat/completions` | chat, impersonation, **video input** |
| `POST /embeddings`, `GET /embeddings/models` | embeddings |
| `POST /audio/speech` | speech |
| `GET /models` (29 query parameters), `GET /models/user` | the catalogue; the second is narrowed by the account's own privacy and provider settings (457 of 460 here) |
| `GET /models/{author}/{slug}/endpoints` | the providers behind one model, with price, quantization and uptime |
| `GET /key`, `GET /credits` | the key's limit and usage; the account's balance |
| `POST /audio/transcriptions` | speech-to-text — used here only to verify the speech (§4.4) |
| `POST /responses`, `POST /messages` | the OpenAI Responses and Anthropic Messages shapes — **[api]**, not exercised (fork F4) |
| `POST /rerank`, `/images`, `/videos` | not this track (§8) |

### 3.2 The catalogue **[live]**

`GET /models` needs no key and answered 755 KB in 0.14 s: **460** entries from
64 vendors — 371 plain, **73 `:batch`**, 16 `:free` — 18 of them `~…-latest`
aliases. Per entry, and none of it read by the picker today:

- `name`, `context_length` (never absent), `top_provider.max_completion_tokens`,
  `pricing` per token, `expiration_date` (24 entries);
- `architecture.input_modalities` — `image` on 292, `video` on 85, `audio` on 44;
- `supported_parameters` — `tools` missing on 68;
- `reasoning: { mandatory, default_enabled, supported_efforts, default_effort }`
  — `mandatory: true` on 114, and **26 different lists** of
  `supported_efforts` (Gemini 3.5 Flash: `high…minimal`, no `none`; GPT-5.5:
  `xhigh…none`; Claude Sonnet 5.5: `max…low`);
- `supported_voices` on the speech models (§4.4).

The filters work: `supported_parameters=tools`, `input_modalities=video`,
`output_modalities=speech|embeddings`, `q=`, `sort=`, `providers=`.

`max_tokens` above a model's ceiling is **clamped, not refused** — the app's
default of 16384 was answered `200` by four models whose ceiling is 4000–8192
(42 models have one below 16384).

### 3.3 The key **[live]**

`GET /key` answers `limit`, `limit_remaining`, `usage` (and per day, week,
month), `is_free_tier`, `expires_at`. A well-formed wrong key is `401 "User not
found."`, no key `401 "No cookie auth credentials found"` — on `/key` and on
chat alike. It is the one request that says whether a key is good without
spending anything.

### 3.4 A chat stream **[live]**

Every chunk carries **`provider`** — the company that served it (`"Amazon
Bedrock"` for `anthropic/claude-haiku-4.5`). The last chunk carries `usage` with
**`cost`** in dollars and `prompt_tokens_details` (`cached_tokens`,
`video_tokens`, `audio_tokens`). `X-Generation-Id` is a response header. Neither
`provider` nor `cost` is parsed today ([wire.rs](../../src/shared/api/openai/wire.rs)
holds the three token counts).

Implicit caching reaches through the gateway by itself: the same 19 k-token
request repeated showed `cached_tokens: 17722` and a fifth of the cost. Nothing
here proposes caching of our own — that track is deferred
([prompt-caching.md](prompt-caching.md)).

### 3.5 Which request fields the gateway reads **[live]**

The method of [grok-xai-provider.md](grok-xai-provider.md) §2.5: send the field
with a value of the wrong type. A refusal naming the field proves the schema
reads it. A `200` proves less — dropped, or read leniently — so the second list
is "no evidence of being read", which for the llama.cpp extensions agrees with
the catalogue publishing none of them.

- **refused, so read:** `temperature`, `top_p`, `top_k`, `min_p`, `top_a`,
  **`repetition_penalty`**, `presence_penalty`, `frequency_penalty`,
  `reasoning_effort`, `reasoning`, `include_reasoning`, `stream_options`,
  `provider`, `usage`, `plugins`, `user`, `session_id`, `tools`;
- **`200`:** **`repeat_penalty`**, `repeat_last_n`, `typical_p`, `top_n_sigma`,
  `dynatemp_*`, `mirostat*`, `dry_*`, `xtc_*`, `adaptive_*`, `samplers`,
  `thinking`, `reasoning_budget`, `chat_template_kwargs`,
  `continue_final_message`, `add_generation_prompt` — and, leniently, `seed`,
  `max_tokens`, `stop`.

A behavioural check of the penalty (greedy decoding, fixed seed, a penalty of
1.8) was tried and **proved nothing**: on a pinned provider the baseline itself
came back as three different texts in three runs. Recorded so the next reader
does not repeat it.

### 3.6 Errors **[live]**

The ordinary envelope, with wording worth passing through unchanged: `400
"<slug> is not a valid model ID"`, `400 "No models provided"`, `404 "No allowed
providers are available for the selected model …"` (with the list of providers
that do serve it). An upstream refusal arrives **wrapped**: `502 "Provider
returned error"` with `metadata.raw` and `metadata.provider_error_code` — a
video that cannot be read is a `502` carrying Google's `403`. `429` was met on
a `:free` variant and on two of the smaller providers. The gateway's own `524`
(edge timeout, **[api]**) is not in `RETRYABLE_STATUSES`
([error.rs:51](../../src/shared/api/error.rs)).

## 4. Slot by slot

### 4.1 Chat

Everything the earlier review measured holds, and the mode changes how it is
arrived at rather than what is sent:

- the client is `OpenAiClient` behind `RetryBackend`, built by
  `cloud_chat_setup` ([supervisor.rs:590](../../src/app/supervisor.rs)) like
  Grok's — a model and a key are required, the status is `Ready` without a probe;
- "is this a gateway" stops being a question: the `/continue` gate, the image
  re-homing and the narrowed sampling list are the mode's, with the catalogue
  supplying the per-model facts;
- a **dialect** on the client — the precedent is `with_effort_none_omitted` —
  spells `repetition_penalty`, leaves the llama.cpp-only fields out, and asks for
  reasoning as the catalogue describes the model (the lowest listed effort on a
  muted turn, `enabled: false` only where `mandatory` is `false`; the `400`
  recovery of the earlier review stays underneath).

**A requirement found by reading, not a fork.** The catalogue is re-asked at
start-up and on a status flip only
([settings.rs:184](../../src/app/orchestrator/settings.rs) calls `apply_chat`
without `refresh_engine_facts`; `context_budget` then answers from the memo,
[compaction.rs:294](../../src/app/orchestrator/compaction.rs)). A mode that is
`Ready` at once never flips — so picking another model inside it would keep the
previous model's window and sampling list. The mode has to re-ask on every
apply. The same gap exists today for a switch from `external` to a cloud
(§9, A2).

### 4.2 Impersonation

Nothing of its own: `apply_impersonation` calls the same three builders as chat
([supervisor.rs:214](../../src/app/supervisor.rs)) **[code]**, so the provider
arm serves both. What the mode adds is that the
impersonation model can differ from the assistant's **on the same key** — today
a second URL and a second key (`ExternalSlot::Impersonation`).

### 4.3 Embeddings **[live]**

`POST /embeddings` with `{model, input: [..]}` — exactly what the `Embedder`
half of `OpenAiClient` sends — answers the OpenAI shape, vectors in input order,
with `provider` and `usage.cost` beside them.

| model | dim | norm | served by |
|---|---|---|---|
| `baai/bge-m3` | 1024 | 1.0000 | DeepInfra (fp32), Parasail |
| `qwen/qwen3-embedding-8b` | 4096 | 1.0000 | Nebius, DeepInfra, SiliconFlow (fp8) |
| `openai/text-embedding-3-small` | 1536 | 1.0001–1.0004 | OpenAI, Azure |
| `google/gemini-embedding-001` | 3072 | 1.0000 | Google |

Unit norms matter: the RAG and attachment indexes are `vec0` tables declared
without a metric ([db/mod.rs:537](../../src/shared/storage/db/mod.rs)), so they
rank by L2 on the vectors as they arrive.

**One model, several providers — how far apart?** Each provider pinned, five
texts, pairwise cosine:

| pair | cosine |
|---|---|
| `bge-m3`: DeepInfra vs Parasail | 0.999999 |
| `bge-m3`: one provider, asked twice | 0.999999 (not bit-identical) |
| `qwen3-embedding-8b`: three providers, pairwise | 0.99989 – 0.99995 |
| `text-embedding-3-small`: OpenAI vs Azure | 0.99978 – 1.00000 |

So the router moving a model between providers does not move the vector space:
every pair is above `CANARY_MATCH` (0.999,
[embed_identity.rs:39](../../src/shared/embed_identity.rs)), and no provider has
to be pinned for an index to stay valid.

**The local model against the gateway's.** `bge-m3-Q8_0.gguf` on a local
llama.cpp (b11234, CPU) against `baai/bge-m3`, on the app's own canary text:

| pair | cosine | the guard would say |
|---|---|---|
| local Q8_0 vs DeepInfra | 0.999490 | same model |
| local Q8_0 vs Parasail | 0.999487 | same model |
| control: local `bge-m3` vs `intfloat/multilingual-e5-large` | 0.439705 | changed |

A user can therefore move the embeddings slot between the local `bge-m3` and the
gateway's **without a reindex** — which is the switching the request is about.
The margin is 0.0005: a coarser quantization may fall under it, and then the
guard offers a reindex, which is its safe side.

**Limits and failures:**

- a batch is capped **per provider**: DeepInfra refuses above 1024 inputs (`422
  array_above_max_length`), Parasail took 2048. The app's batches are 16 and 32
  — except `rag_add`, which sends every chunk of a text in one request;
- an input over the model's window is a `400`, not a silent truncation;
- **`429` is ordinary**: a pinned Parasail answered "The engine is currently
  overloaded" on 4 of 10 requests. Unpinned, the router falls back by itself
  (18 of 18 valid requests were answered), but the embedder has **no retry** —
  `RetryBackend` wraps `EngineBackend` only **[code]**;
- `/models?output_modalities=embeddings` lists 37 — the 33 of
  `/embeddings/models` plus four `:batch` slugs. The picker wants the second.

**`input_type`.** The gateway has a field for query-versus-document, which the
app knows as `EmbedRole` and can express only as a text prefix. Measured on
eight models, the cosine between the two roles of one text:

| honours it | ignores it (cosine 1.0000) |
|---|---|
| `voyageai/voyage-4-lite` — 0.62; `google/gemini-embedding-001` — 0.93 | `bge-m3`, `text-embedding-3-small`, `qwen3-embedding-8b`, `multilingual-e5-large`, `mistral-embed`, `pplx-embed` |

Note `multilingual-e5-large` on the right: the e5 prefixes stay necessary.

### 4.4 Speech **[live]**

`POST /audio/speech` takes `{model, input, voice, response_format, speed}` and
returns the audio as the body. The matrix — every speech model in the catalogue,
both formats, a sentence in English and one in Russian, each answer transcribed
back by `openai/whisper-large-v3-turbo` through the gateway's own
`/audio/transcriptions`:

| model | voices listed | `pcm` | `mp3` | Russian heard back |
|---|---|---|---|---|
| `google/gemini-3.8-flash-tts`, `-lite-tts`, `3.1-flash-tts-preview` | 30 | 24 kHz | **`400`** | yes |
| `minimax/speech-2.8-hd`, `-turbo` | 45 | **`400`** | 32 kHz | yes |
| `fish-audio/s1`, `s2-pro`, `s2.1-pro`, `s2.1-pro-free:free` | none | **44.1 kHz** | 44.1 kHz | yes |
| `bytedance-seed/seed-audio-1-0` | none | 24 kHz, **stereo** | 44.1 kHz | yes (11–23 s per sentence) |
| `microsoft/mai-voice-2`, `-flash` | 4 | 24 kHz | 24 kHz | yes |
| `qwen/qwen-audio-3.0-tts-flash`, `-plus` | 2 | 24 kHz | 24 kHz | yes |
| `x-ai/grok-voice-tts-1.0` | 5 | 24 kHz | 24 kHz | yes |
| `hexgrad/kokoro-82m` | 54 | 24 kHz | 24 kHz | no |
| `deepgram/aura-2`, `flux-tts:free` | 90, 36 | 24 kHz | 24 kHz | no |
| `canopylabs/orpheus-3b-0.1-ft`, `sesame/csm-1b`, `mistralai/voxtral-mini-tts-2603` | 7, 7, 30 | 24 kHz | 24 kHz (voxtral 22.05) | no |

What follows from it:

- **no format is taken by every model** — `pcm` by 19 of 21, `mp3` by 18, and
  the default (no field) is `pcm`. The refusal names the field, so one retry in
  the other format settles a model;
- **the rate and the channel count are in `Content-Type`**
  (`audio/pcm;rate=44100;channels=1`) and cannot be assumed;
- **a voice is required** by some (`400 "An explicit voice is required for this
  TTS provider"`) and the catalogue publishes the list for 16 of 21;
- **15 of 21 speak Russian**; the six that do not were tried with the first
  voice they list, which is an English one;
- **`speed` changed nothing** on Gemini (3.8 s, 4.3 s and 3.8 s of audio for
  1.0, 0.5 and 2.0) or on Grok (3.6 s, 3.4 s, 3.7 s) — the schema says as much:
  "only used by models that support it";
- **`instructions` is not a field here** — it, and any unknown key, is dropped
  with a `200`;
- **4000 characters in one request is accepted** (Gemini: 248 s of audio in
  34.8 s; Grok: 242 s in 35.1 s);
- **not every model streams.** For 1500 characters the first byte came after
  0.4–0.5 s from Grok, fish-audio and MiniMax, and after **28.2 s** from Gemini,
  which sends the whole clip at once;
- **OpenAI's speech models are not in the catalogue**: `openai/gpt-4o-mini-tts`
  is `400 … does not exist`.

A probe's own mistake, kept because a client could make it too: raw PCM may
begin with `0xFFFF` — a sample of −1 — which reads as an MP3 frame sync. The
label decides; sniffing the body does not.

### 4.5 Watching a YouTube video **[live]**

The link goes into `chat/completions` as a content part — `{"type":
"video_url", "video_url": {"url": …}}`. Of the 85 models that list video input,
Gemini is the family that takes a YouTube **link**: the two others tried went
to download it as a file and refused — `qwen/qwen3.6-flash` with a `400
"Missing Content-Length of multimodal url"`, `google/gemma-4-31b-it` with
`Invalid image URL: content_type='text/html'`. Two videos were used: the one the
unit tests name, and a 67-second one **published 2026-09-08**, which no model
can describe from memory.

**It works, and costs what the native request costs:**

| request, the 67 s video, `gemini-3.5-flash-lite` | prompt tokens | of which video |
|---|---|---|
| native Gemini, `MEDIA_RESOLUTION_LOW` — what the app sends | 6147 | 6094 |
| through the gateway, Google AI Studio | 6147 | 6094 |
| through the gateway, Google Vertex | 6146 | 4422 + 1672 audio |
| blind control — the link as text, no video part | 77 | 0 → "NO VIDEO" |

The transcripts of the three routes agree almost word for word: they differ in
where a line is split, in one company's name on one route and in a word's
ending. On the app's default model, `google/gemini-3.5-flash`, the same video
is 6147 tokens and $0.0127.

**The documentation is wrong about Vertex.** It says a YouTube link works on AI
Studio only **[docs]**; both routes took it **[live]**, and the router filters
the endpoints by itself — a refusal's `routing_funnel` lists a step "Filter by
Input Video Support". No provider has to be pinned.

**What the gateway does not carry:**

| | measured |
|---|---|
| **segment bounds** | four spellings on the 213 s video, none honoured: as fields of `video_url` and as a sibling `video_metadata` the whole video was read (19 426 tokens each time); **in the URL (`&start=40&end=80`, `&t=40s`) the video was not read at all** — `video_tokens: 0`, a `200`, and a description made up from the link |
| **media resolution** | three spellings, none honoured |
| `processing: "agentic"` | the transcript is right and the prompt is 53 tokens: the video is handled somewhere the usage does not show, and the bill moves to 1070 completion tokens |

Natively a 20-second segment is 1874 tokens against 6147 for the whole. Through
the gateway a segment can only be **named in the prompt**, which was tried and
answers for the right part — at the whole video's price.

**The one rule this slot needs.** Twice a request came back `200` with a
plausible description of a video that was never read. What told them apart was
`usage.prompt_tokens_details.video_tokens`: zero. An answer without video tokens
is a refusal, whatever the text says.

**Resolution, measured natively** — an adjacent finding (§9, A3): on
`gemini-3.5-flash` and its `-lite` the token count is the same for `LOW`,
`MEDIUM`, `HIGH` and unset (6094). On `gemini-2.5-flash` `LOW` is 6908 against
19 772 — and through the gateway 2.5 costs the 19 772.

**Errors.** A video that does not exist is a `502` wrapping Google's `403
PERMISSION_DENIED` (§3.6). A model with no video input is a `404 "No endpoints
found that support input video"`. And one refusal arrived as **a `200` whose
body is the error envelope** — no `choices`, an `error` with its own `code:
400` (Gemma, above) — so a non-streaming client reads the body before it
believes the status.

## 5. What adding a provider costs **[code]**

**The precedent.** Grok was one PR (#285): 20 files, +1137/−140, of which the
source was nine files, +599/−111, and the rest documentation. It needed no
client, no schema change and no new locale keys. Features built since have
each had to add its arm — 21 later commits touch a Grok line — so today's
footprint is larger than the PR was: `grok` appears on 195 lines in 23 files.

**Where the compiler helps, and where it does not.** Most sites are exhaustive
matches, which refuse to build until the new variant is handled. These are not —
a forgotten one still builds and leaves the provider out in silence:

| site | what a miss costs |
|---|---|
| `CloudProvider::ALL`, `ServerMode::ALL` ([config.rs:56](../../src/shared/config.rs), `:162`) | `from_key` cannot read the build's own `llm_history` rows |
| `named_key_env_vars` ([config.rs:562](../../src/shared/config.rs)) | the key's variable is **not removed** from the environment of model-driven children — a safe-defaults leak |
| `SERVER_MODES`, `IMP_MODES`, `cycle_mode`, `cycle_imp_mode` (settings helpers) | the mode cannot be selected |
| the `_ => CatalogueShape::OpenAi` arm (orchestrator catalogue) | the picker falls back to the id-only list |
| `CLOUD_MODES` and three tests that already omit Grok | coverage that looks complete |

**What this provider needs that Grok did not:** a catalogue shape with four
per-slot sources (§3.2), the dialect (§4.1), a speech client (§4.4), a video
client and the slot's first provider selector (§4.5), a retry around the
embedder (§4.3), and `available_sampling_fields` letting one cloud provider be
narrowed by its catalogue — today every cloud ignores the endpoint's list
([sampling.rs:301](../../src/entities/sampling.rs)).

**Documents that enumerate providers** and would each change: README, install.md
§3, manual.md, spec §6.4, §8, §9.9, §11.6, §11.9, §12.2, architecture §5,
ADR 0004's status list, the site's index — and **PRIVACY.md**, which names every
host the app talks to and says which clouds are probed; it is regenerated into
the installer's page and the site's (`tools/wizard_rtf.py`,
`tools/site_legal_pages.py`).

## 6. Forks

**User's decision, 2026-09-29: all twelve at their recommendation.** F1, F2, F3
and F5 were the user's call and were answered as recommended; the others were
recorded with the recommendation they would be taken at, and stand. What was
adopted, in one place:

| fork | adopted |
|---|---|
| F1 | (a) a provider of its own |
| F2 | all five slots, four stages |
| F3 | (b) the variants and `SETTINGS_SCHEMA` 3 → 4 |
| F4 | (a) Chat Completions for every model, a dialect on `OpenAiClient` |
| F5 | (b) attribution on by default, a setting turns it off, PRIVACY.md says what is sent |
| F6 | (a) `GET /key` once per apply |
| F7 | the provider's claims: four sources, `:batch` left out, the row shows window and price |
| F8 | the mode, a retry, a batch cap; `input_type` on demand |
| F9 | one client, the format negotiated |
| F10 | a selector, the evidence rule, the segment in words |
| F11 | parse now, show later |
| F12 | (a) a hint |

### F1. How to meet the need — **adopted (a)**

- **(a) a provider of its own**: a fifth `CloudProvider`, selectable in every
  slot, one key.
- (b) **named connections inside `external`** — a list of URL/model/key sets to
  switch between. Solves the switching for chat and for any gateway, not only
  this one. Does not give speech or video, does not share a key across slots,
  and turns the section from an object into a list — a breaking settings change.
- (c) leave it: `external` and a paragraph in install.md.

(b) and (a) do not exclude each other; (b) answers a different question — two
local servers — and nobody has asked it.

### F2. Which slots, and in what order — **adopted: all five, four stages**

Chat with impersonation, then embeddings, then speech, then video (§7). The
alternative is stage 1 alone and the rest on demand — but speech and video are
the two slots this gateway's users have no other road to (§2.2).

### F3. A new value of an existing enum — **adopted (b)**

`engine.mode`, `embed.mode`, `impersonation_engine.mode` and `tts.mode` are
strict enums, and [lessons.md](../lessons.md) §8 is about exactly this: a new
*value* fails the parse of the whole file in any binary that does not know it.

- (a) **variants and no bump** — what Grok did, before the rule. An older 0.12.0
  refuses to start ("unknown variant") once a slot is set to the new mode;
  **0.11.2 and earlier reset the settings in silence and write the defaults over
  the file, stored keys included**.
- **(b) variants and `SETTINGS_SCHEMA` 3 → 4**, with a step that changes
  nothing. Every older binary that has the downgrade guard (ADR 0006 §4) then
  refuses with "created by a newer version" before it reads a value, whether
  or not the mode was ever selected; the pre-migration backup is the way back.
  Chats (`metadata.mode`) and `data.db` (`llm_history.mode`) carry the same
  value and need no bump of their own: an older binary never gets past
  `settings.json`.
- (c) read the mode enums leniently from now on. Rejected: a mode that falls
  back to the default is an engine silently switched — the defect the typed
  gate was built against ([settings-typed-parse.md](settings-typed-parse.md)).

The video slot's selector is a **new field** (`video.provider`), which is
additive either way.

### F4. The wire — **adopted (a)**

- **(a) Chat Completions for every model**, `OpenAiClient` with a dialect.
- (b) the model's native shape through the gateway's `/messages` and
  `/responses`. Rejected: three clients kept in step for one provider, and the
  only thing the native shapes carry that this one does not — reasoning blocks
  echoed across tool rounds — was measured to change nothing
  ([openrouter-external.md](openrouter-external.md) F5).

### F5. Attribution headers — **adopted (b)**

`HTTP-Referer` (the app's URL) and `X-OpenRouter-Title` make the gateway list
the application in its public rankings and on each model's page **[docs]**; both
were accepted **[live]**. They name the application, never the user — but the
engine clients name themselves to no provider today (`engine_client` sets no
`User-Agent`, [http.rs:29](../../src/shared/api/http.rs)), and PRIVACY.md
promises that "no request is ever made about you, your machine or your usage".

- (a) always.
- **(b) on by default, a setting turns it off**, and PRIVACY.md says what the
  two headers hold.
- (c) off by default. In practice (d) with a setting nobody finds.
- (d) never — the literal reading of the promise, at the price of the one free
  channel through which this gateway's users find applications.

### F6. Checking the key — **adopted (a)**

- **(a) `GET /key` once, when the engine is applied**: a refused key becomes the
  slot's status instead of the first message's error, and the settings hint can
  show what the key has left. One request per apply, never periodic — and a new
  line in PRIVACY.md's list of probes.
- (b) no check, like the other clouds.

### F7. The picker — **adopted: the provider's claims, all of them**

One shape, four sources: `/models/user` for chat and impersonation when a key is
stored (the account's own filters) and `/models` otherwise; `/embeddings/models`;
`?output_modalities=speech`; `?input_modalities=video`. `:batch` slugs are left
out — the endpoint itself refuses them (D2). A row shows the name, the window
and the price, and marks a model that lists no `tools`. For the video slot the
list is every model that claims video, Gemini first: that only Gemini takes a
YouTube link is **our** measurement, and a list narrows on the provider's claim,
never on ours ([model-picker.md](model-picker.md) §3). The voice row gets a list
of its own from `supported_voices`.

### F8. Embeddings — **adopted: the mode, a retry, a batch cap**

- a retry around the embedder, for every cloud embedder rather than this one;
- requests split at a fixed number of inputs — `rag_add` is the one unbounded
  caller;
- `input_type` as a further `EmbedConvention` — **on demand**: two of eight
  models honour it, and the prefixes the app already has cover the rest.

### F9. Speech — **adopted: one client, the format negotiated**

`OpenAiTts` gains a third constructor rather than a sibling type: ask for `pcm`,
on a `400` naming `response_format` ask once for `mp3` and remember it for the
model; take the rate and the channels from `Content-Type`. No default model — a
default that names a model ages ([model-picker.md](model-picker.md) §1). `speed`
is sent as today and the hint says a model may ignore it; the `instructions` row
is not shown. The per-request ceiling stays 2000 characters, as for native
Gemini.

### F10. Video — **adopted: a selector, the evidence rule, the segment in words**

`video.provider` (`gemini` by default) and a section per provider, so that the
model is not retyped on a switch. The client asks for the lowest reasoning
effort the catalogue lists and sends no `processing`. **An answer with no video
tokens is an error.** A segment is named in the prompt, the length gate reads
the whole video's length, and the result says the whole video was read. The
resolution row is hidden in this mode.

### F11. The gateway's own knobs — **adopted: parse now, show later**

`provider` and `usage.cost` are two optional fields on the message's metadata —
additive — and stage 1 can record them at no design cost. Showing them, and the
routing controls (`provider.order/only/ignore/sort`, `data_collection`, `zdr`),
is a stage of its own, **on demand**; of those, `provider.only` has the measured
use ([openrouter-external.md](openrouter-external.md) §8.1).

### F12. A user already on `external` — **adopted (a)**

- **(a) a hint**: a settings row whose `external` URL is on `openrouter.ai` says
  there is a mode for it.
- (b) move the values over at the first start. Rejected: settings that rearrange
  themselves.
- (c) nothing.

`external` keeps working against the gateway exactly as it does: nothing of the
earlier three tracks is removed.

## 7. The track

| stage | what | go/no-go, live |
|---|---|---|
| **1 — MVP** | the provider in chat and impersonation: config, supervisor, settings rows, the dialect, the picker's shape, the catalogue re-asked on every apply, F3, F5, F6 | the named smokes of the earlier review through the **mode**, on an Anthropic, a Google and an open-weight reasoning model; a switch `external` (local llama.cpp) → `openrouter` → back inside one session keeps both sections and changes the window and the sampling list each way; a wrong key is a status |
| **2** | embeddings: the mode, the retry, the batch cap | RAG ingest and search through `baai/bge-m3`; an index built on the local Q8_0 answers a query embedded by the gateway, with no reindex offered |
| **3** | speech: the mode, the client, the voice list | a sentence spoken by a `pcm`-only and by an `mp3`-only model and **transcribed back** — the assertion is the text; a 44.1 kHz model plays at its own rate |
| **4** | video: the selector and the client | the 2026-09-08 video: `video_tokens > 0` and a line of the transcript; a URL the gateway does not read is an error, not a description |
| 5 | the knobs of F11 | **on demand** |

Stage 1 is about the size of the Grok PR plus the picker's shape and the
dialect; stages 3 and 4 are a client each; stage 2 is the smallest.

## 8. What this does not cover

Speech-to-text (24 models — it would be voice input, a feature nobody has
asked for), reranking (7), image and video **generation**, the Batch API behind
the `:batch` slugs, the gateway's OAuth key exchange, workspaces and guardrails,
BYOK, `plugins` (its web search and PDF parsing duplicate tools the app has),
prompt caching of our own ([prompt-caching.md](prompt-caching.md) — deferred),
and the six `openrouter/*` router slugs, which were not tried.

## 9. Adjacent findings — not this track's to fix

Found while reading for this research. A1 and A2 are read, **not run**.

| | finding |
|---|---|
| **A1** | **The embedding guard and the prefixer are lost on the first in-session change of the embedding settings.** `apply_embed_settings` — the only place that wraps the embedder in `EmbedGuard { PrefixedEmbedder { … } }` — runs once, at start-up ([mod.rs:267](../../src/app/orchestrator/mod.rs)); a settings edit goes through `flush_restarts` → `engines.apply_embed`, which installs the bare client ([engines.rs:236](../../src/app/orchestrator/engines.rs)), and so does a managed relaunch. Until the restart: no canary check, no `query:`/`passage:` prefixes. It bears on this track — a switch of the embedder is the moment the guard exists for. |
| **A2** | **The engine's facts survive a switch that flips no status** (§4.1): from `external` to a cloud, the previous engine's window keeps deciding when compaction fires. |
| **A3** | **`video.media_resolution` does nothing on the default video model** (§4.5): identical token counts on `gemini-3.5-flash` at every value. It still matters on the 2.5 generation. |

## 10. How this was measured

Python 3.10 and `urllib` against `https://openrouter.ai/api/v1`, the key taken
from `OPENROUTER_API_KEY` and never printed; the native Gemini arms with
`GEMINI_API_KEY`; the local embedder was
`llama-server -m bge-m3-Q8_0.gguf --embeddings -c 8192` (b11234, CPU). Some
450 requests, **$0.60** on the gateway's own meter. Nothing in the repository
was run or changed — no smoke exists yet for a mode that does not.

The discipline that mattered, each time:

- **a control arm for anything a model could answer from memory** — a video
  published after every model's cutoff, and a blind request beside it;
- **the token count, not the answer, as the evidence** that a video was read;
- **the audio transcribed back**, so that "`200`, 180 KB" is not mistaken for
  speech;
- **a repeatability check before a behavioural comparison** — which is how the
  penalty probe of §3.5 was found to prove nothing.
