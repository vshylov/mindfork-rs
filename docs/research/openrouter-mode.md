# Research: a mode of its own for OpenRouter

**Status: the track is complete — all four stages built (§11–§14), for the
release 0.13.0.** Measured against the service on **2026-09-29** with a paid account;
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

**Stage 1 is built** (2026-09-29): what it measured on the way and where it
departed from this table — the embedder's arm came with it — is §11.

**Stage 2 is built** (2026-09-29), go/no-go met: §12.

**Stage 3 is built** (2026-09-29), go/no-go met: §13 — with a defect of the
player's found on the way, which was every mode's.

**Stage 4 is built** (2026-09-29), go/no-go met: §14. The mode serves all
five slots.

**User's decision, 2026-09-29: every stage ships in one release — 0.13.0.** The
one step of the settings schema, 3 → 4, therefore covers the track: no released
build will have read a file of schema 4 and met a mode it does not know
(§13.4). Stage 4's `video.provider` is a new field, additive (F3), and would
have owed no step in any release. The site's texts and the one-sentence
description of the application, which name the gateway, go with 0.13.0, in a
pull request of their own.

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

## 11. Measured while stage 1 was built (2026-09-29)

What §4.1 stated as design, measured before the code was made to rest on it;
and where the stage departed from the plan of §6–§7. Same method as §10 for the
probes; with the live runs of the stage's smokes, about fifty cents on the
gateway's meter — most of it the orchestrator's turns, each of which sends the
app's whole prompt and tool set.

### 11.1 A muted turn, by kind of model **[live]**

One request each — a five-word title, `max_tokens: 400` — with nothing about
reasoning, with each spelling of "off", with the lowest effort the entry lists
and with one it does not. Reasoning tokens, and the gateway's own `cost` where
it says something:

| model | the entry's `reasoning` | nothing asked | `reasoning_effort: "none"` | `reasoning: {enabled: false}` | the lowest listed | one not listed |
|---|---|---|---|---|---|---|
| `google/gemini-3.5-flash` | mandatory, `high…minimal` | 277, $0.00256 | **400** | **400** | `minimal`: **0**, $0.0000675 | `xhigh`: 200, 230 |
| `x-ai/grok-4.6` | mandatory, `xhigh…low` | 422, $0.00282 | **400** | **400** | `low`: 193, $0.00144 | `minimal`: 200, 159 |
| `anthropic/claude-sonnet-5.5` | mandatory, `max…low` | 0 | **400** | **400** | `low`: 0 | `minimal`: 200, 0 |
| `deepseek/deepseek-r1` | mandatory, no list | 325 | **400** | **400** | — | — |
| `openai/gpt-5.5` | optional, `xhigh…none` | 15 | 200, 0 | 200, 0 | `low`: 0 | `minimal`: 200, 0 |
| `qwen/qwen3.6-27b` | optional, on by default | 301 | 200, 0 | 200, 0 | — | — |
| `anthropic/claude-haiku-4.5` | optional | 0 | 200, 0 | 200, 0 | — | — |
| `meta-llama/llama-3.3-70b-instruct` | none | 0 | 200 | 200 | — | — |

Every refusal is the same sentence: `400 "Reasoning is mandatory for this
endpoint and cannot be disabled."`

- **A model that must reason refuses both spellings of "off"** — 4 of 4 — and
  takes its lowest listed effort. On Gemini 3.5 Flash that is no reasoning at
  all and a thirty-eighth of the price; on Grok 4.6 half. Through `external` the
  same turn is refused, re-sent with nothing asked, and answered at the model's
  default depth: it works, and it pays for reasoning that a title, a compaction
  roll and an impersonated message never wanted.
- **An effort the entry does not list is accepted** — 4 of 4, `200`. So the list
  describes the model; it does not validate the request. Nothing in the client
  relies on that: it asks only for what is listed.
- **A mandatory model with no list** has nothing lower to be asked for. The
  request carries no reasoning field, and is answered.

### 11.2 One model's entry **[live]**

`GET /model/{author}/{slug}` answers the entry of §3.2 for one model: **2 148
bytes** in 0.1 s where the list is **754 782** in 0.5 s, with or without a key.
It resolves what the list does not carry — `anthropic/claude-haiku-4.5:nitro`
and `:floor` answer the base model's entry (both route a chat request, §3.2's
list has neither) — and answers `404 "Model not found: …"` for a slug that is
nobody's, a `:free` variant that does not exist included. The `~…-latest`
aliases are in the list already. The mode's client asks this route and never
the list: `external`, matching the list by id, knows nothing about a model
named with a variant.

### 11.3 An answer delivered as reasoning **[live]**

`deepseek/deepseek-r1`, routed to one provider in every run here: **2 runs of
4** ended `finish_reason: "stop"` with the whole reply — the worked answer and
its last line — in `reasoning` and no `content` delta at all; the other two
split it. Read chunk by chunk outside the app, so it is the stream as the
gateway sends it. `qwen/qwen3.6-27b` split 4 of 4 across four providers. The
app shows such a turn as thoughts and an empty reply, which is what arrived.
Not this track's to fix; it is why the smoke of the mode's thoughts runs on the
second model.

### 11.4 A user message that is an image and nothing else **[live]**

A tool's image is re-homed into a `user` message (§4.1), which has no text of
its own. Sent with the image alone, `google/gemini-3.5-flash` answered **8 of
8** with a fault: the model's scratch text at the head of the reply (`0The
background is green…`, `_thought` followed by its reasoning) or no reply at all
— `finish_reason: "stop"` on the first chunk, no usage. With one text part
before the image, 8 of 8 were clean. `anthropic/claude-haiku-4.5` and
`qwen/qwen3.6-27b` were clean 4 of 4 without it. The app never sends the bare
form — every image it builds carries its label — so this is a property the
label turned out to have, written down where the label is built; it was met
because the smoke's fixture had none.

### 11.5 Where stage 1 departed from the plan

| the plan | what was built, and why |
|---|---|
| §7: embeddings are stage 2 | **The embedder's arm came with stage 1.** `ServerMode` is one enum for the chat engine and the embedder, so a value the first takes is a value the second has to answer for. The arm is built like a cloud's — a model and a key, `Ready` at once, no key check. Stage 2 is what is left: the retry, the batch cap, and whether the embedder's key is checked. |
| F7: a row shows the name, the window and the price | A row shows the **id** — it is what is stored and what the gateway routes on — followed by the window, the price and `no tools`. The published name is searched by the filter and not drawn. |
| F7: one shape, four sources | Three: the account's list, the public list, the embedding models. The speech and the video lists belong to the stages that have a slot to show them in. |
| F5: the headers on every request | On every request, the model list's included — asked by another client than the engine's, and named by the same switch. |
| §9 A2: not this track's to fix | Fixed here: the mode would have met it on every change of model. With the facts asked again on every applied change a second defect surfaced — an answer of the engine that was replaced reached the screens, sent before its epoch was looked at — and was fixed with it. |

## 12. Measured while stage 2 was built (2026-09-29)

### 12.1 How many inputs one embeddings request takes **[live]**

More inputs sent until the endpoint refused; the texts short and all different,
so that an answer out of order would show.

| endpoint | takes | the refusal |
|---|---|---|
| Gemini, OpenAI-compatible (`gemini-embedding-001`) | **100** | `400 "BatchEmbedContentsRequest.requests: at most 100 requests can be in one batch"` |
| the gateway, `google/gemini-embedding-2` and its preview | **100** | the same sentence, wrapped |
| the gateway, the other 31 embedding models | 128 and more | — |
| the gateway, `baai/bge-m3` on DeepInfra (§4.3) | 1024 | `422 array_above_max_length` |
| OpenAI (`text-embedding-3-small`) | 2048 | `400 "array length must be 2048 or less"` |

Every answer came in the order of its inputs. Gemini leaves `index` out of the
first vector (a zero, omitted) and numbers the rest.

So `rag_add`, which embeds every chunk of its text in one request, could not
add a text of more than a hundred chunks on a Gemini embedder — on Google's own
endpoint, before any gateway. The cap is **64**: under the strictest count, and
at the default chunk size under the per-request token ceilings that are
published.

### 12.2 The go/no-go **[live]**

On the production supervisor, through the tools the model calls
(`gateway_live_embed`), with a local `bge-m3-Q8_0.gguf` (llama.cpp b11234, CPU):

- Five passages indexed on the local model. The embedder moved to the gateway's
  `baai/bge-m3` **by an edit made in the session**. The query *"Which city is
  the capital of France?"*, embedded by the gateway, finds the passage the local
  model indexed — L2 distance 0.717 against 1.164 for the runner-up. **No
  reindex is offered**: no notice, the same generation, the base not stale. The
  canary, local against the gateway: **0.999491**, the floor being 0.999.
- A passage added through the gateway is found by the gateway and, moved back,
  by the local model.
- **The control**: the same edit to `intfloat/multilingual-e5-large` — the same
  width, another model — is said once, retires the generation and marks the
  base stale. That is the guard armed by the edit, which is the half of this
  that did not work before (§9, A1).
- A text of **131 chunks** is added through `google/gemini-embedding-2` and
  through Google's own endpoint, and its one planted fact is found; the bare
  client, given the same 131 chunks in one request, is refused by both.
- A key the gateway refuses is the embedder's status before anything is
  embedded.

The retry has no live arm. A `429` cannot be asked for, and unpinned the
gateway's router falls back by itself (§4.3); what is tested is the decorator
against a scripted embedder and the supervisor's stack against a stub that
answers `429` and then the vectors.

### 12.3 Where stage 2 departed from the plan

| the plan | what was built, and why |
|---|---|
| F8: a retry for every cloud embedder | As planned — and **not** for `external`, where the chat retry does apply. An embedder is called several times in a turn by the memory tools; against a server of the user's own that is down, three attempts are three seconds of waiting per call on a server nobody started. |
| F8: requests split at a fixed number | For every embedder, the user's own servers included: one rule, and a `llama-server` has no count of its own to exceed. A part that answers a vector short is an error rather than a shorter answer — vectors are matched to their texts by position. |
| §11.5: whether the embedder's key is checked | It is, like the chat engine's: `Connecting` until `GET /key` answered. |
| §9 A1: not this track's to fix | Fixed here, as the go/no-go could not be met without it: "no reindex was offered" says nothing from a guard that was not there. The dress is an argument of the one function that installs an embedder. |
| — | **Found by the control arm**: the first search after a change of model ran against the stale index. `rag_search` looked whether the base was stale, *then* embedded the query — and the guard checks on the first request an embedder serves, which was that one. It looks again after the embedding. |

## 13. Measured while stage 3 was built (2026-09-29)

### 13.1 What the matrix of §4.4 left open **[live]**

- **The speech filter works on the account's list too.**
  `GET /models?output_modalities=speech` and
  `GET /models/user?output_modalities=speech` answer the same 21 entries, with
  a key and without one; every entry's `output_modalities` is `["speech"]`.
- **The route takes two formats and refuses the rest before any model is
  asked.** `response_format: "wav"` is a `400` from the gateway's own schema —
  *"Invalid option: expected one of "mp3"|"pcm""* — whichever model is named.
  So the `external` speech mode, which asks a server for `wav`, cannot speak
  through this gateway at all: there was no way to use it for speech before
  this stage.
- **A voice is required by most.** Seven of eight models tried without one
  answer `400 "An explicit voice is required for this TTS provider."`; Fish
  Audio, which lists none, speaks. A voice nobody has is `400 "Provider
  returned 400"` from most, `404` from xAI, `502` from MiniMax; Deepgram alone
  names the voices it does have.
- **A wrong key** is `401 "User not found."`, as on every other route; a model
  that is not a speech model is `400 "Model … does not exist"`, a chat model
  included.
- **The price has no unit.** Fifty characters spoken by
  `x-ai/grok-voice-tts-1.0` cost $0.00075 on the gateway's own record of the
  request (`GET /generation?id=`, which answers some ten seconds after the
  speech) — fifty times its `pricing.prompt` of `0.000015`, while the same
  record counts 13 prompt tokens. That price is **per character**. Gemini's
  entries price a prompt and a completion, per token. The catalogue says which
  in neither case.

### 13.2 An MP3 that was streamed, and a decoder that trusts its first frame **[live]**

The go/no-go's second half failed on its first run, in the decoder: `attempt
to subtract with overflow`, inside the MP3 demuxer. So every model that
answers an MP3 was asked for one — twelve of the 21 — and each answer decoded
twice, as the player decodes by default (gapless) and without the trim:

| model | gapless | untrimmed |
|---|---|---|
| `minimax/speech-2.8-hd`, `-turbo` | **a panic** | 4.21 s, 4.03 s |
| `hexgrad/kokoro-82m` | **2.35 s** | 4.78 s |
| `sesame/csm-1b` | 1.83 s | 1.90 s |
| `canopylabs/orpheus-3b-0.1-ft` | 4.61 s | 4.66 s |
| the other seven | the same | the same |

Gapless playback trims what the first frame's `Info` tag says the encoder
added, and counts the frames by the same tag. A speech server writes that tag
**before it knows how long the stream will be**: MiniMax's counts no frames at
all, and the demuxer subtracts the encoder's delay from zero — a panic in a
build that checks for overflow, a wrapped number in one that does not;
Kokoro's counts half of them, and the sentence stops half way. The other
models write no tag, or a true one.

The application's decoder is built without the trim. What that costs is the
encoder's own padding left in — 48 ms and 66 ms on the two clips whose tags
told the truth — which a sentence read aloud does not miss.

This is not the gateway's: any server that streams an MP3 can write such a
tag, and the `external` mode plays what it is given.

### 13.3 The go/no-go **[live]**

Through the mode's own client (`shared::tts::gateway_live_tests`), each answer
transcribed back by `openai/whisper-large-v3-turbo` through the gateway's
`/audio/transcriptions`, and the sentence looked for in what was heard:

| | model | what happened |
|---|---|---|
| raw samples only | `google/gemini-3.8-flash-lite-tts`, voice `Zephyr` | asked once; 24 kHz mono; *"Testing 1, 2, 3. The weather is fine today."* heard back, and the Russian sentence word for word. The control: asked for `mp3` alone, the model refuses |
| MP3 only | `minimax/speech-2.8-turbo`, voice `English_expressive_narrator` | refused, asked again, answered; the session remembers `mp3`. What is transcribed is **what the application's decoder made of the container** — 32 kHz, 4.28 s — and the sentence is there to its last word, in both languages |
| 44.1 kHz | `fish-audio/s1`, no voice | 44 100 Hz in the label and in the clip; heard back; through the sound card a clip of 3.44 s played in 3.53 s — at 24 kHz it would have taken 6.32 s |
| a refused key | — | `TTS: status 401 Unauthorized: User not found.` |
| the list | — | 21 models with a key and without, 16 of them with voices |

Through the application (`gateway_live_speech`): `/tts` over a chat, the
engines built from the settings, the playback queue and the sound card. The
MP3-only model in **two voices** — the assistant's and the user's, two engines
on one memo — 8.04 s for two sentences; the 44.1 kHz model with no voice set,
5.58 s; the raw-samples model, 6.72 s; a model that needs a voice and has none
is an error in the feed, in the gateway's sentence, and the speech ends.

**Not spoken**: the one model that answers raw samples in stereo (11–23 s a
sentence, §4.4). Its label is read by a unit test, and the player is timed
without a network: a second of audio at 24 kHz in stereo lasts 1.06 s through
the sound card, a second at 44.1 kHz mono 1.07 s.

In the terminal (`tools/console_probe.py --scenario gateway`): `/tts` lights
the status line's `speaking` and it goes out with nothing said about a
failure; the Speech tab shows the provider's rows and none for instructions;
the model row lists 21 models, their voices counted and no price beside them;
the voice row lists the five voices of the model named.

### 13.4 Where stage 3 departed from the plan

| the plan | what was built, and why |
|---|---|
| F9: ask for `pcm`, on a refusal ask once for `mp3`, remember it for the model | As planned, and **both ways**: what is remembered is what the gateway did, so a model that stops taking the remembered format costs one refused request and is remembered the new way. The memo is the session's, not the client's — a speech client is built per command, and a memo inside it would pay the refusal with every `/tts`. |
| F9: the rate and the channels from `Content-Type` | As planned. The label also decides **what the body is**: raw samples are samples though they begin as an MP3 frame does, and a container is one whatever was asked. A label of raw samples that names no rate is taken for 24 kHz, and logged. |
| F7: the voice row gets a list of its own | From the **same answer** as the model row's — a voice belongs to a model — so whichever row is opened first asks, and the other does not. A model that lists no voice, or a name typed by hand, leaves the voice row the editor it was. |
| F7: a row shows the name, the window and the price | A speech model's row shows how many voices it lists, and **no price**: the catalogue's number has no unit and the unit differs by model (§13.1). "Per 1M tokens" beside it would be our claim, and for most of them a wrong one. |
| F6: `GET /key` once per apply | Not for speech. Nothing is applied: the slot has no engine and no status, its client is built when `/tts` is typed. A refused key is said by the first request, in the gateway's sentence. |
| F12: a hint under an `external` address that is the gateway's | The speech slot's hint is its own. The other slots' says *"this section keeps working as it is"*, which here is false: `external` asks for `wav`, and the gateway refuses it (§13.1). |
| F3: the modes' values and `SETTINGS_SCHEMA` 3 → 4 | No step of this stage's: `tts.mode` is one of the four values F3 names, and no build with schema 4 has been released. A release that carried stage 1 without this stage would have owed a step of this stage's own; by the user's decision of 2026-09-29 every stage ships in one release, 0.13.0 (§7), so none is owed. |
| — | **Found by the live run**: the decoder (§13.2). Fixed for every mode that plays a container. |

## 14. Measured while stage 4 was built (2026-09-29)

### 14.1 The list of models that take video **[live]**

| request | entries |
|---|---|
| `GET /models?input_modalities=video`, no key | **85** — 72 once the `:batch` twins are left out |
| `GET /models/user?input_modalities=video`, with the key | **461** — the account's whole list |

The account's list **does not take this filter**; it takes the speech one
(§13.1), so the two were not to be assumed alike. The entries carry
`architecture.input_modalities`, and narrowed by it the account's list is 70
models — the public list's 72 less two the account's own settings leave out.
That is a list narrowed on the provider's claim, made by this client because
the provider's filter was not applied; it is not a claim of ours.

Of the 85, 28 are Google's. The family opens the list: that only Gemini takes a
YouTube **link** is this project's measurement (§4.5), which may order a list
and never narrow it (F7).

### 14.2 What an address costs when it is not read as a video **[live]**

The 67-second video, `google/gemini-3.5-flash-lite`, the lowest effort:

| the address | prompt tokens | of which video | cost | the answer |
|---|---|---|---|---|
| `youtube.com/watch?v=<id>` | 6 119 | 4 422 + 1 672 audio | $0.0019 | the first sentences of the video |
| `youtu.be/<id>` | 6 116 | the same | $0.0005 (5 301 cached) | about the video |
| `youtube.com/embed/<id>` | 6 116 | the same | $0.0019 | about the video |
| `…watch?v=<id>&t=20s` | **558 136** | **0** | **$0.167** | "NO VIDEO" |
| `…watch?v=<id>&feature=share` | **551 337** | **0** | **$0.165** | *"The video shows a NASA video detailing the plans and progress for building a moon base."* |
| `https://example.com/` | 0 | 0 | $0.000005 | "NO VIDEO" |

One more parameter in the address, and the link is not a video to the gateway:
what is read is the page behind it — half a million tokens of it — at **ninety
times the price**, and the answer may be a description all the same, made up
from what the page says about the video. §4.5 had met the zero and not the
bill.

Two things follow. The address the client is given has to be the video's id
and nothing else — which is what `youtube_watch` has always built, from the id
it reads out of whatever was pasted. And the rule of §4.5 holds as written:
what tells an answer about the video from an answer about the page is the
count of video tokens, and the text is not looked at.

### 14.3 The go/no-go **[live]**

Through the client (`shared::video::gateway_live_tests`):

| | what happened |
|---|---|
| the video of 2026-09-08 | watched in 4.4 s; *"This is the moment where we should all start believing again. Now, bound for the moon. America is returning to the moon to build a moon base."* — a line of what is said in it, and the client, which refuses an answer without video tokens, handed it over |
| a link the gateway does not read | `https://example.com/`: a `200`, and the client's error — *"the gateway answered without reading the video — no video tokens among the 0 prompt tokens it counted"* |
| a video that is not there | *"status 502 Bad Gateway: Provider returned error: The caller does not have permission"* — the gateway's words and the provider's |
| a refused key | *"status 401 Unauthorized: User not found."* |
| the list | 72 without a key, 70 with one; Gemini opens both |

Through the tool (`features::tools::gateway_live_tests`), as the agentic loop
calls it:

- **A link as a person pastes it** — `…watch?v=IwZVXmQdX1E&t=20s&feature=share`,
  the very parameters of §14.2 — is watched: the header YouTube gave (*NASA Moon
  Base: The First Six Months · NASA · 1:07*), a description, and a transcript of
  twelve lines that opens with the video's first words.
- **A segment, 0:20–0:40, named in words**: the result says the whole video was
  read and charged, and the transcript is of the part asked for — three lines,
  at 0:26, 0:29 and 0:38, counted from the video's start — without the video's
  first words in it.
- **The ceiling**: the 67-second video under a ceiling of one minute, thirty
  seconds of it asked for, is refused in half a second — *"1:07 … 1:00"* — and
  nothing is spent.

In the terminal (`tools/console_probe.py --scenario gateway`): the settings'
search finds the video group; it names `openrouter`, the model, the gateway's
key rows, and has no row for the resolution; the model row lists 70 models,
fifteen of the Gemini family first, each with its window and price and none
marked "no tools".

### 14.4 Where stage 4 departed from the plan

| the plan | what was built, and why |
|---|---|
| F10: `video.provider` and a section per provider | The gateway has a section, `video.openrouter`; Gemini's fields stay where a file has always held them, at the top of `video`. Moving them into a section of their own would have been a rename — a step of the schema and a migration — for the same effect: neither model is retyped on a switch. |
| F10: the lowest reasoning effort the catalogue lists | As planned, from the model's own entry (`GET /model/{slug}`), asked once per client. A model that lists none is sent nothing about reasoning; an outage of the catalogue is not an answer and is asked about again. |
| F10: an answer with no video tokens is an error | As planned, and first among what is read of a `200`: before the text, which is never looked at. Before it, the body's `error` — one refusal comes as a `200` (§4.5). |
| F10: the length gate reads the whole video's length | As planned — and a video **whose length could not be read** is refused, where the native provider's is clipped to the ceiling: the gateway takes no bound to clip by. With the ceiling at 0 it is watched. The refusal does not advise a segment, which through the gateway costs the same. |
| F7: `?input_modalities=video` | Without a key. With one, the account's whole list, narrowed here by what each entry says it takes: the account's list does not take the filter (§14.1). |
| F7: a row marks a model that lists no `tools` | Not behind the video row: a model that watches is asked to describe, not to act. |
| — | **Measured on the way**: what a link with a parameter costs (§14.2). Nothing of the application sent one; the smoke that pastes such a link is there so that nothing ever does. |
