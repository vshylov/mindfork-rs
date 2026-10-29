# Image generation by cloud models

Status: **research; forks decided by the owner on 2026-10-29** (§7). The
decisions:
- **As recommended:** F1 (a), F2 (a), F3 (a), F4 (a), F5 (a), F6 (a), F8 (a),
  F9 (a).
- **F7, amended:** the switch off by default and nothing drawn in follow-ups or
  background runs, as recommended — but the ceiling per turn is **a setting**, and
  `0` removes it. **Turns from Telegram must draw**: they do, since the tool is not
  one of the dangerous families a bridged turn loses (ADR 0013 D3 (a)).
- **Clarified for F9:** the picture goes to the phone as a photo, and the phone is
  **stage 2**, straight after the tool (§8).

**Stage 0, the probe, ran on 2026-10-29: GO** (§11). It measured the providers
($0.97 in all) and the tool on Gemma 4 and Qwen 3.8, and handed three forks back to
the owner, **decided the same day as recommended**:
- **F3 re-opened → (a′):** the default is OpenAI's `gpt-image-2.5-flare` at
  `medium` — measured at a third of Gemini's price.
- **F10 (a):** after a refusal the tool stops drawing for the rest of the turn.
- **F11 (a):** a tool's picture is re-homed into a user message on every
  OpenAI-compatible endpoint, behind a label with no words of any language — a PR
  of its own before stage 1, since it is every tool's (Gemma 4 answers in JSON
  after a picture inside a tool result).

**F11 is built** (§11.5: Gemma 4's JSON replies 17 of 21 → 0 of 21). **Stage 1
is built** (§12): the slot, the three clients, `image_generate` with editing, the
`generated` chat file (chats 6 → 7), the ceiling and the stop after a refusal —
live **GO** on every provider and through the loop on Gemma 4 and Qwen 3.8. **Stage 2 is built** (§13): a picture drawn from Telegram goes back to
the phone as a photo after the words — live **GO** through the bridge on Gemma 4
and Qwen 3.8. Next is stage 3. Provider facts were read from the providers' own documentation
on 2026-10-29 (§10) and then measured (§11.1).

## 1. Why

The owner's framing (2026-10-29): add image generation by the models of cloud
providers. Local generation is out of scope, because consumer hardware cannot
hold a Stable Diffusion model and a language model at the same time.

That constraint shapes the design more than any provider does:

- **The chat model is often local.** A Gemma or a Qwen on the person's own GPU,
  or on a rented pod, has no image output at all, and neither have Claude and
  DeepSeek (§3.5). If pictures can only come from a chat model that draws, most
  of the audience never sees one.
- **The answer the project already has for this shape is a slot.** Speech
  ([ADR 0009](../decisions/0009-tts-speech-synthesis.md)) and video
  (`youtube_watch`, spec §9.9) are both capabilities that most chat engines lack,
  and both are served by a provider configured **apart from the chat engine**,
  reading the key already stored for that provider
  ([ADR 0008](../decisions/0008-api-key-storage.md)). A Claude user hears replies
  through OpenAI; a local Gemma watches a video through Gemini. Image generation
  is the same shape a third time.
- **The model decides when to draw.** In a conversation the request is rarely a
  command — "show me what that would look like", a companion asked for a
  picture. A tool the chat model calls, with a prompt the chat model writes, is
  how that reaches a provider; the person typing the prompt themselves is the
  second, simpler road (§5).

## 2. What the providers give — summary

Read on 2026-10-29 from each provider's documentation; free catalogue calls only
(Gemini's `models.list`, OpenRouter's public lists) — **no picture was generated
for this document**. Prices are for one square picture of about a megapixel.

| | OpenAI | Gemini | xAI | OpenRouter |
|---|---|---|---|---|
| **Models now** | `gpt-image-2.5-sunburst`, `-flare` (2026-09-08); `gpt-image-2` | `gemini-nano-banana-2.1` (GA 2026-10-06), `-3.1-flash-lite-image`, `-3-pro-image`; **Imagen shut down 2026-08-17** | `grok-imagine-image-2.0`, `grok-imagine-image` | 61 image models of 13 vendors — all of the three before, plus FLUX, Seedream, Qwen-Image, Recraft, … |
| **Endpoint** | `POST /v1/images/generations`, `/edits` | `generateContent` with `responseModalities: ["IMAGE"]` — the transport the Gemini client and its speech already use | `POST /v1/images/generations`, `/edits` | `POST /api/v1/images` |
| **Shape** | OpenAI's | Gemini's own | OpenAI's, another dialect | OpenAI's, another dialect |
| **Edit** | yes, ≤ 16 pictures | yes, picture parts in the request | yes, ≤ 5, JSON only | per model (`input_references`) |
| **Picture comes as** | base64 only; png/jpeg/webp, transparency | base64 `inlineData` | a temporary URL by default, base64 on request; JPEG only | base64 + `media_type` |
| **One picture costs** | `gpt-image-2`: $0.006 low · $0.053 medium · $0.211 high; 2.5 priced by tokens ($30 per 1M out) | $0.034 (2.1, 1K) · $0.050 (2K) · $0.113 (4K); Pro $0.134 | $0.04 (2.0) · $0.02 (1.0) | the vendor's price; failures not billed |
| **Says the cost** | tokens | tokens | `cost_in_usd_ticks` | `usage.cost`, USD |
| **Before the first picture** | an organisation ID check may be required | billing on: **no free tier** for image output | — | credits |
| **A refusal** | `400`, `code: moderation_blocked`, stage + categories | `blockReason` / `finishReason: IMAGE_SAFETY`, …; or plain text and no picture | undocumented — and charged | undocumented for this endpoint |
| **Can the chat model itself draw?** | via the Responses tool `image_generation`, on `gpt-5.2`+ | **no** — image models take no function calls | via its Responses tool, on `grok-4.7` | via the beta server tool `openrouter:image_generation`, which hands the model a temporary URL |

Anthropic and DeepSeek generate no pictures at all (§3.5).

Two conclusions shape the rest:

- **Two wires cover all four.** OpenAI, xAI and the gateway speak one request
  shape (`model`, `prompt`, a size or an aspect, base64 back) with different
  dialects; Gemini is `generateContent`, which the app already speaks.
- **No image model can be the conversation.** Google's image models take no
  function calls, and OpenAI's are not chat models at all; where a chat model draws
  (OpenAI's and xAI's Responses tools, the gateway's server tool), it is a hosted
  tool of one provider's chat. The slot (§5 A) works whatever the person talks to.

## 3. Per provider

### 3.1 OpenAI

- **Models.** `gpt-image-2.5-sunburst` (the most capable, "for workflows where
  editing precision matters most") and `gpt-image-2.5-flare` (the fastest), both
  released 2026-09-08; `gpt-image-2` (2026-04-21) is still served and the guide
  calls it "earlier". `gpt-image-1` was shut down on 2026-10-23, DALL·E on
  2026-05-12; `gpt-image-1.5`, `gpt-image-1-mini` and `chatgpt-image-latest` shut
  down on 2026-12-01. An image model is never the top-level `model` of a chat or
  Responses request — its pages list both as "Not supported".
- **`POST /v1/images/generations`.** `model`, `prompt` (≤ 32 000 characters),
  `size` — `1024x1024`, `1536x1024`, `1024x1536`, `auto`, or on the 2 and 2.5
  models any `WIDTHxHEIGHT` divisible by 16 with an aspect between 1:3 and 3:1, up
  to 3840×2160 ("above 2560x1440 are experimental"); `quality` `low`/`medium`/
  `high`/`auto`, plus `xhigh`/`max` on 2.5; `background` `transparent` (png/webp
  only); `output_format` png/jpeg/webp ("jpeg is faster"); `n` 1–10;
  `moderation` `auto`/`low`; `stream` with `partial_images` 0–3 (each partial
  costs 100 more output tokens). The answer is **base64 only** — `url` and
  `response_format` are unsupported for GPT image models:

  ```json
  {"created": 1713833628, "data": [{"b64_json": "..."}],
   "usage": {"input_tokens": 50, "output_tokens": 50, "total_tokens": 100,
             "input_tokens_details": {"text_tokens": 10, "image_tokens": 40}}}
  ```

  The reference calls `usage` "for `gpt-image-1` only" while its own 2.5 example
  carries it — present but to be treated as optional.
- **`POST /v1/images/edits`.** JSON (`images: [{image_url | file_id}]`, up to 16,
  a data URL accepted) or multipart; an optional `mask` (prompt-guided, "may not
  follow its exact shape"); the default model is `gpt-image-2.5-sunburst`.
- **The Responses tool.** `tools: [{"type": "image_generation", "model": …,
  "quality": …, "size": …}]`, `gpt-image-2` when no model is named; offered by the
  mainline models from `gpt-5.2` to `gpt-6.1-terra`, listed beside `function`
  tools; forced by `tool_choice: {"type": "image_generation"}`. The output item
  `image_generation_call` carries `result` (base64) and `revised_prompt`.
  Multi-turn editing goes by `previous_response_id` or by sending the earlier item
  back — and the app sends `store: false`, so it would have to re-send the whole
  item, picture included (documented, no example; a probe question). **Chat
  Completions has no image output at all.**
- **Price** (per 1M tokens): image output $30, image input $8, text input $5, on
  2.5 and 2 alike; Batch half. Per picture, published for `gpt-image-2` only, at
  1024×1024: **$0.006 low, $0.053 medium, $0.211 high**; the 2.5 models publish
  token rates and a calculator, and warn that equal rates are not equal cost.
  "Complex prompts may take up to 2 minutes."
- **Gating.** "You may need to complete the API Organization Verification" (an ID
  check) before GPT image models answer; forum reports show a `403` "organization
  must be verified" otherwise. 20 images a minute on the lowest paid tier. This is
  the one provider where a key that chats can still be refused a picture — the
  error has to be said in OpenAI's words, not as a generic failure.
- **Refusal.** HTTP 400 with a stable discriminator:

  ```json
  {"error": {"type": "image_generation_user_error", "code": "moderation_blocked",
             "moderation_details": {"moderation_stage": "input", "categories": ["harassment"]}}}
  ```

  `moderation_stage` is `input`, `output` or `unknown`; the guide says not to retry
  without changing the prompt.
- **Provenance.** OpenAI's provenance check reads C2PA credentials and SynthID;
  that every API picture carries them is implied, not stated.

### 3.2 Google Gemini

- **Models.** `gemini-nano-banana-2.1` ("Nano Banana 2.1"), GA since 2026-10-06
  and the recommended replacement for everything older; `gemini-3.1-flash-lite-image`,
  the cheapest, 1K only, "not optimized for … multi-turn sequential editing";
  `gemini-3-pro-image`, the premium one. `gemini-3.1-flash-image` was deprecated on
  2026-10-06 with no shutdown date; `gemini-2.5-flash-image` shuts down on
  2027-03-15. **Imagen is gone**: every `imagen-4.0-*` model shut down on
  2026-08-17, and the live API answers `404` — there is no `:predict` route to
  design for.
- **The documentation moved; the endpoint did not.** Google's image guide now
  shows only the new Interactions API (`POST /v1beta/interactions`), while
  `generateContent` "remains fully supported" and every image model still lists
  it. The app's native client speaks `generateContent`, and its speech already
  asks it for a non-text modality (`responseModalities: ["AUDIO"]`, ADR 0009) — a
  picture is the same request with `["IMAGE"]` (or `["TEXT", "IMAGE"]`) and
  `imageConfig: {aspectRatio, imageSize}`. Fourteen aspect ratios, `1:1`, `3:2`,
  `2:3`, `16:9` and `9:16` among them; `imageSize` `512`, `1K` (the default), `2K`,
  `4K`. No count: one picture per request.
- **The answer.** The picture is a part, `inlineData {mimeType, data}`, beside any
  text; `usageMetadata.candidatesTokensDetails` counts the `IMAGE` tokens.
  Thinking cannot be turned off on the Gemini 3 image models and is billed. A
  thought signature may sit on any part, the picture's included, and multi-turn
  editing wants the model's content sent back unchanged — which matters to shape C
  and not to a tool that sends the picture to edit as an input of a fresh request.
- **No function calling.** Every image model's card says "Function calling: Not
  supported"; the one tool they take is Google Search grounding. **A Gemini image
  model cannot be the app's chat model** — it would lose every tool.
- **Price, one picture.** Nano Banana 2.1: **$0.034 at 1K**, $0.050 at 2K, $0.113 at
  4K; Pro $0.134 up to 2K, $0.24 at 4K; Lite $0.034, 1K only. **No free tier for
  any image model** — a key on the free tier chats and cannot draw, and the
  refusal has to be said in Google's words.
- **Refusal.** A blocked prompt is `promptFeedback.blockReason`; a blocked picture
  is `finishReason` `IMAGE_SAFETY`, `IMAGE_PROHIBITED_CONTENT`, `IMAGE_OTHER`,
  `NO_IMAGE` or `IMAGE_RECITATION`; and a model that declines answers in **plain
  text with `STOP` and no picture** — the case a tool has to catch by the absence
  of an image rather than by a code.
- **Provenance.** Every picture carries a SynthID watermark; no visible mark is
  mentioned.
- **Discovery.** `models.list` carries no output-modality field: an image model is
  recognisable by its id alone.

### 3.3 xAI

- **Models.** `grok-imagine-image-2.0`, the recommended one, **$0.04 a picture**;
  `grok-imagine-image` (1.0) $0.02; `grok-imagine-image-quality` ($0.05) retires
  on 2026-11-02 and is then served by 2.0 at low quality. `grok-2-image-1212`
  appears nowhere in the current documentation.
- **`POST /v1/images/generations` and `/v1/images/edits`.** `aspect_ratio` from
  fifteen values (`1:1`, `3:2`, `2:3`, `16:9`, `9:16`, `21:9`, … and `auto`) —
  **no pixel size**; `resolution` `1k`/`2k`; `quality` `low`/`medium`/`auto` on 2.0
  only; `n` 1–10; `response_format` `url` (the default, a temporary link with no
  lifetime stated) or `b64_json`. The picture is **JPEG**: no transparency, no
  format choice, no streaming. Edits are JSON only — "the OpenAI SDK's
  `images.edit()` method is not supported" — take up to five pictures, named in
  the prompt as `<IMAGE_0>`, `<IMAGE_1>`, and bill the input picture as well as
  the output. `usage` carries `cost_in_usd_ticks`: the one direct provider that
  states the price of the call in the answer.
- **The Responses tool.** `{"type": "image_generation"}` with `grok-4.7`, fixed to
  `grok-imagine-image-2.0`, combinable with client functions; no size or quality.
  Chat Completions is deprecated and does not have it.
- **Refusal.** The REST shape is **not documented**. A refused prompt is still
  charged, and the Responses API adds a $0.05 fee for a violation caught before
  generation. The probe has to find the shape.

### 3.4 OpenRouter

The gateway offers a picture three ways, and its guide now documents the first:

1. **The Image API, `POST /api/v1/images`.** `model`, `prompt`; `resolution`
   (`512` … `4K`), `aspect_ratio`, `n`, `quality`, `output_format`
   (png/jpeg/webp/svg), `seed`, `input_references` (pictures to edit, http(s) or
   data URLs), the usual `provider` routing. The answer:

   ```json
   {"created": 1748372400,
    "data": [{"b64_json": "<base64>", "media_type": "image/png"}],
    "usage": {"prompt_tokens": 0, "completion_tokens": 4175, "total_tokens": 4175, "cost": 0.04}}
   ```

   Billing is **all-or-nothing**: a failed generation is a `502` and is not
   charged. Streaming exists for the OpenAI models only. No refusal body is
   documented for this route.
2. **Chat completions with `modalities: ["image", "text"]`** and `image_config` —
   pictures in `message.images[].image_url.url` as data URLs. Still in the
   reference and live, but gone from the guide, and the stream schema has no field
   for a picture.
3. **The server tool `openrouter:image_generation`** (beta) — any tool-calling
   chat model on the gateway may call it beside its own functions; the gateway
   draws with `openai/gpt-5-image` unless told otherwise and hands the model
   `{"status": "ok", "imageUrl": "…"}`, a **temporary** URL the model then writes
   into its text.

**The catalogue says what an image model is.** `GET /models?output_modalities=image`
lists 63 models; `GET /images/models` lists 61 with typed parameters (the
resolutions and aspects each takes, its `n` range, how many `input_references`),
and `/images/models/{id}/endpoints` gives the real billing unit — per token, per
picture or per megapixel. The families today: Google's Nano Banana line, OpenAI's
`gpt-image-1/2/2.5` and `gpt-5-image`, Black Forest Labs' FLUX.2 and FLUX-3,
ByteDance's Seedream, xAI's Grok Imagine, Qwen-Image, Recraft (SVG included),
Microsoft's MAI, Krea, Tencent, Meta. **One key reaches every vendor in §3**, and
the answer states what the picture cost.

### 3.5 Anthropic and DeepSeek

Neither generates pictures. Anthropic's vision FAQ: Claude "is an image
understanding model only" and cannot generate images. DeepSeek's models produce
text; its vision is image input, on `deepseek-flash` only. A person whose chat
engine is either is the slot's first beneficiary (§1).

## 4. What mindfork already has (the code as of 0.22.0)

Almost every piece a generated image needs already exists, built for other
producers. What is missing is the client for each provider's image endpoint, the
tool, and a settings section.

1. **The slot pattern.** `TtsSettings` and `VideoSettings` in
   `shared/config.rs`: a provider selector (`TtsMode`, `VideoProvider`), a section
   per provider so a switch loses nothing, the key read from the shared stored
   key by `cloud_provider()`, an env-variable fallback, an optional base URL. The
   clients are stateless and built from a config snapshot per call
   (`shared/tts`, `shared/video`, `shared/stt`). The gateway's sections have **no
   default model** — a default that names a model ages
   ([model-picker.md](model-picker.md) §1) — and the cloud ones do.
2. **A tool can return pictures.** `ToolOutcome.images: Vec<ToolImage>`
   (`features/tools/mod.rs`) carries base64 + MIME + the result line that names
   the file. The orchestrator prepares each one on the path `/image attach` takes
   (png/jpeg, downscaled to `images.downscale_px` = 1568), caps them at
   `MAX_TOOL_RESULT_IMAGES` = 4 a result, asks once a turn whether the engine has
   vision, names what survives `tool-image-N` against the chat, and writes the
   directive notes when an image is withheld (spec §9.10 — "you cannot see it; this
   is not an error" was measured necessary on Gemma 4, Qwen 3.6 and gpt-oss).
   The per-provider placement is done too: Gemini refuses a multimodal
   `functionResponse` and gets the image as user parts after it; the gateway gets
   the images of a run of tool results in one user message.
3. **`python_exec` is the template to copy.** An image its code saves is stored
   **as written** in the chat's folder and a prepared copy is offered to the model
   under its own switch, `tools.python_images` (on by default)
   ([sandbox-file-exchange.md](../history/sandbox-file-exchange.md) F5, §11 S8).
   A generated image wants exactly that: the original kept, a smaller copy shown.
4. **The chat's files.** `data/files/<chat-id>/`, listed by `Chat.files:
   Vec<ChatFile>` with an origin (`sandbox`, `recovered`, `attached`), a SHA-256
   so the same bytes are not stored twice, addressed by `#N` or name. `/file open`
   hands an image to the desktop's viewer (`shared/os_open.rs`, whose allowlist
   opens documents and images directly), `/file folder` opens the folder, backups
   carry `files/`, and the owner decided **no quota** (F10 of the same plan).
5. **Pictures to edit are already addressable.** A user's image lives on its
   message (`MessageImage`, base64 in the chat file, downscaled); a file of the
   chat has a handle `python_exec` already accepts in its `files` argument. An
   edit request needs nothing new to name its input.
6. **Catalogues.** The model picker (`shared/api/catalogue.rs`) has a slot per
   settings row (`ModelSlot::{Speech, Transcription, Video}`, …) and already sees
   image models: the gateway's `openai/gpt-image-2` is classified
   `ModelRole::Other` and kept out of the chat list; OpenAI's own list carries
   `gpt-image-2.5-*` and `chatgpt-image-latest` ([model-picker.md](model-picker.md)).
   An image slot is one more `ModelSlot` and one more role.
7. **The feed draws no pixels.** A tool card shows a chip per image it returned
   (`ToolCallRecord.images`, a count); rendering pictures is a roadmap idea
   ("Rendering images in the feed"). `ratatui-image` 11.1.0 (2026-09-17) fits the
   ratatui 0.30 and `image` 0.25 the app has, and draws by the kitty, iTerm2 and
   Sixel protocols with a half-block fallback; 12.0.0 is at rc.3 (2026-10-18).
   The audience's terminals are often the ones that draw none of it — tmux without
   `allow-passthrough`, JupyterLab's terminal, the Windows console host
   ([companion-platform.md](companion-platform.md) C12).
8. **The messenger bridge receives pictures and sends none**
   ([ADR 0013](../decisions/0013-messenger-bridge.md), stage B). A phone is where a
   picture is actually seen; a generated image is the obvious first thing it would
   send back.
9. **Money.** Nothing asks before a paid call: `tools.confirm_dangerous` is about
   effects outside the application (spec §9.8), and `youtube_watch` — the paid
   precedent — is not "dangerous". What bounds a turn is `max_tool_rounds` (8). The
   companion platform's rule 5 stands for any turn nobody asked for: it spends
   money only within a cap the person set ([companion-platform.md](companion-platform.md) §7).
10. **Stored types.** A new section with an enum of its own (`image_gen.provider`)
    is skipped whole by an older reader and needs no settings step. A **new value**
    of `FileOrigin` (`generated`) is a chat-schema step, 6 → 7: an older binary
    skips a chat that holds it, in silence
    (`shared/storage/schema/stored_enums_tests.rs`, docs/lessons.md §8).

## 5. Three shapes, and the one proposed

**A. A tool over an image slot (`image_generate`).** The chat model calls it with
a prompt; the tool calls the configured provider out of band, as `youtube_watch`
does; the original lands in the chat's files and a prepared copy goes back in the
result, so a model with vision sees what it drew and can try again. Works with
every chat engine, local ones included — which is the owner's case.

**B. A command (`/image generate <prompt>`).** The person types the prompt; no
model is involved, so it works on an engine without tool calls and costs exactly
one image. Same slot, same client, same storage. The result is staged for the next
message, as `/image paste` stages a clipboard picture — the person decides whether
the model sees it.

**C. Native image output from the chat model.** Where the chat engine itself can
draw: OpenAI's and xAI's hosted `image_generation` tools in their Responses APIs,
the gateway's server tool, its `modalities: ["image", "text"]`. A Gemini image
model as the assistant is not among them — it takes no function calls (§3.2). C
needs the engine contract (`EngineBackend`) to carry an image chunk, assistant
messages to hold pictures and replay them, and each provider's history rules —
OpenAI's item re-sent whole under `store: false`, the gateway's temporary URL
fetched before it dies — for a capability that exists only when the chat engine is
one of three clouds. Not proposed; recorded so it is not rediscovered.

**Proposed: A and B over one slot, A first.** A carries the value — the model
deciding when a picture helps — and B is a few dozen lines on top of the same
client once A exists.

## 6. Design

### 6.1 The slot — `image_gen` in the settings

A section of its own, shaped like `tts` and `video`, all `#[serde(default)]`:

```jsonc
"image_gen": {
  "enabled": false,      // F7: nothing is drawn, or paid for, until the person says so
  "provider": "openai",  // F2/F3 (F3 re-decided after the probe: (a′))
  "gemini":     { "model_name": "gemini-nano-banana-2.1", "image_size": "1K", "api_key_env": null, "url": null },
  "openai":     { "model_name": "gpt-image-2.5-flare",    "quality": "medium", "api_key_env": null, "url": null },
  "openrouter": { "model_name": null,                     "resolution": "1K", "api_key_env": null, "url": null },
  "xai":        { "model_name": "grok-imagine-image-2.0", "resolution": "1k", "api_key_env": null, "url": null },
  "show_to_model": true, // F6: the prepared copy goes back in the result
  "max_per_turn": 4      // F7: a turn's ceiling, said to the model when reached; 0 — none
}
```

The key is the provider's stored key (ADR 0008) — an OpenAI key entered for chat
or speech draws too. Like the speech and video slots, this one is not asked about
its key in advance: it has no status, and a refused key, an unverified
organisation or a free-tier Gemini key is the first picture's answer, in the
provider's words. The settings screen gets a section beside *Speech* and *Video*.
The model row opens the picker (a new `ModelSlot::Image`) for the gateway, whose
catalogue says what an image model is; OpenAI's and Gemini's lists do not, and
guessing by name is what the project has been burned by, so their rows stay typed
fields with a default — the video slot's arrangement.

### 6.2 The clients — two wires

Under `shared/image_gen/`, stateless and built per call from a snapshot, as speech
and video are:

- **The OpenAI shape, three dialects.** One client for `…/images/generations`,
  `…/images/edits` and the gateway's `/images`, differing where §3 says they do:
  OpenAI takes a pixel `size` (the aspect mapped to `1024x1024`, `1536x1024`,
  `1024x1536`, `1536x864`, `864x1536`) and answers base64 only; xAI takes
  `aspect_ratio` and `resolution`, is asked for `b64_json` so no temporary URL has
  to be fetched, and names its edit inputs `<IMAGE_0>` in the prompt; its refusal
  is an error *string* beside a usage that was charged (§11.1); the gateway takes
  `aspect_ratio`, `resolution` and `input_references` — each only when the model's
  catalogue entry lists it (FLUX.2, Recraft and MAI take no `resolution`) — and
  reports `media_type` and `usage.cost`.
- **Gemini's `generateContent`**, on the transport the Gemini client and its speech
  share: `responseModalities: ["IMAGE"]` (measured enough on its own), `imageConfig`,
  the pictures to edit as `inlineData` parts of the one user turn. The answer's
  first image part is taken — a JPEG, measured — and everything else dropped,
  including a thought signature of up to three million characters on that very
  part; a reply with no image part is a refusal, its text kept as the reason.

An edit is always a **fresh request** with the pictures as inputs — never a replay
of an image model's own conversation — so no thought signature and no stored
response id is ever needed.

### 6.3 The tool — `image_generate`

Named like `python_exec` and `youtube_watch`. Group *External world*; offered only
while `image_gen.enabled` is on (a new `ToolGate`), and then on the profile's
toggle like any tool.

Parameters the model sees:

- `prompt` (required) — the whole picture in words. The description asks for a
  full description rather than a title, in any language.
- `aspect` — one of `1:1`, `3:2`, `2:3`, `16:9`, `9:16`; `1:1` when absent. Each
  client maps it to what its provider takes (§3): a size for OpenAI, an aspect
  ratio for Gemini, the gateway and xAI.
- `images` — pictures to change or to draw from, by the handles the chat already
  has: a chat file (`#3`, `cat.png`), a message image or a `tool-image-N`. Absent
  — a new picture (F4). The description says it is for pictures the person wants
  changed — the probe saw a model pass its own failed attempt as the reference for
  the retry (§11.2).

What the model does **not** choose: quality, resolution, format and count. They
are the price, and the price is the person's (one image per call; a second
picture is a second call, which the turn's ceiling counts).

The result, in the scaffold language (axis A):

```
Saved as #7 image-3.png (1536×1024, 2.1 MB) in the chat's files; the user can open it with /file open #7.
The provider rewrote the prompt as: "…"            ← when it says so (OpenAI's revised_prompt)
```

followed by the loop's own line about whether the picture is shown (spec §9.10:
"shown below", or "not shown" and why). A refusal is a result, not an error, and
says so in the directive form measured for the withheld-image notes: *"The
provider refused to draw this: «…». This is not a connection error. Tell the user
what was refused and ask what to change; do not describe a picture."* The probe
saw "the same prompt will be refused again" read as leave to rephrase (§11.2), so
**after a refusal the tool draws nothing more in that turn** (F10): a later call
gets "drawing is stopped for this turn after a refusal" without reaching the
provider, and xAI's refusals, which are charged, stay at one a turn.

### 6.4 Where the picture lives

- **The original** — as the provider sent it, in the chat's folder, listed as a
  `ChatFile` with origin `generated` (F5). `/file open #7` shows it in the
  desktop's viewer; backups carry it; `/file remove` deletes our copy. The name is
  `image-N.<ext>`, the first free `N` in the chat — a slug of the prompt would be a
  transliteration problem in half the prompts this audience writes.
- **The copy for the model** — `ToolOutcome.images`, prepared by the loop on the
  existing path when `show_to_model` is on and the engine has vision. **One trap
  found while reading the loop:** `prepare_tool_images` drops a tool image whose
  *raw* bytes exceed `images.max_bytes` (10 MB) before it is ever downscaled. A
  4K PNG is past that. The tool hands the loop the original only when it is under
  the cap and a downscaled PNG otherwise, so the picture the model is promised is
  the picture it gets.
- **The card** — the tool card's chip names the file (`▣ #7 image-3.png —
  1536×1024`), so the handle `/file open` takes is on screen.

### 6.5 The command — `/image generate <prompt>`

The person's own road (shape B), on the same client: the picture is stored as a
chat file **and staged** for the next message, exactly as `/image paste` stages a
clipboard image — the chip says what is waiting, `/image remove` unstages it, and
sending it is what shows the model. No tool, no model, no round. A refusal is a
feed note in the interface language (axis B).

### 6.6 Money

Image generation is the first tool whose single call can cost more than the turn
around it. Three things bound it, none of them a popup by default (F7):

1. **The switch** — off until the person turns it on.
2. **A ceiling per turn** (`max_per_turn`, 4 by default) — a setting, `0` for none,
   the convention `video.max_minutes` already follows. The call past it is refused
   with a sentence the model can relay; `max_tool_rounds` still bounds the turn.
3. **No drawing in a turn nobody is waiting for** — the tool is not offered to a
   woken follow-up or a background sub-agent, by the companion platform's rule 5.
   **A turn the person starts from Telegram is not one of them**: it keeps every
   tool of the profile but the four dangerous families (ADR 0013 D3 (a)), and
   `image_generate` is not dangerous — so the phone draws, and gets the picture
   (§8, stage 2).

What the provider says the call cost — `usage.cost` from the gateway,
`cost_in_usd_ticks` from xAI — goes into the card and the log; OpenAI and Gemini
state tokens only, and the card shows those. The app keeps no price list of its
own: prices moved within the month this document was written (§3), and a stale
price shown is worse than none.

### 6.7 Seeing it

The terminal is the weakest place to look at a picture, so v1 does not try: the
chip and `/file open` work in every terminal, and the bridge (§8, stage 2) sends
the picture to the phone where a turn came from the phone. Drawing pixels in the
feed stays the roadmap's own track (F8), which this one makes worth doing.

## 7. Forks

Each fork lists its options with a recommendation; the owner's choice follows
each, dated.

- **F1. The shape.** (a) **A tool and a command over one slot, the tool first**
  (§5 A + B) *(recommended)* — the tool is what a conversation needs, the command
  is cheap once the client exists. (b) The tool only. (c) The command only — no
  model in the loop, no probe needed, but the companion and "show me" cases never
  happen. (d) Native output from the chat model (§5 C).
  **Owner's decision (2026-10-29): (a).**
- **F2. Which providers, in what order.** (a) **Stage 1: OpenRouter, OpenAI and
  Gemini; xAI later** *(recommended)*. The gateway alone reaches 61 models
  of 13 vendors, xAI's and FLUX among them, on a route that states the cost and
  does not bill a failure; OpenAI and Gemini are the direct keys this audience
  most often already holds, and they are exactly the speech slot's clouds. xAI's
  own endpoint is the OpenAI wire in one more dialect, and its refusal is
  undocumented and charged — it waits for the probe to say what it looks like.
  (b) All four in stage 1. (c) The gateway only: one client and every vendor, but
  a person whose only key is OpenAI's or Google's gets nothing.
  **Owner's decision (2026-10-29): (a)** — xAI in stage 3, once F9 put the phone
  second.
- **F3. The default provider and models.** (a) **`gemini`, with
  `gemini-nano-banana-2.1` at 1K** *(recommended)* — the cheapest of the direct
  providers' recommended models at their default quality ($0.034, against xAI's
  $0.04 and `gpt-image-2`'s $0.053 at medium), GA since 2026-10-06, made for
  editing, and no identity check in front of it; OpenAI's section defaults to
  `gpt-image-2.5-flare` at `medium`, xAI's to `grok-imagine-image-2.0`, the
  gateway's to no model (its list is one keypress away, as in every slot).
  (b) `openai` — the speech slot's default, but a key that chats may be refused
  pictures until the organisation is verified, which a first try should not meet.
  (c) No default provider: the switch stays off until one is chosen.
  **Owner's decision (2026-10-29): (a).** **Re-opened by the probe the same day**
  (§11.1): measured, OpenAI's `gpt-image-2.5-flare` at `medium` costs $0.013 a
  picture and Gemini's $0.039 with the thinking it cannot turn off — the premise of
  (a) was wrong. Options again: (a′) **`openai`, `gpt-image-2.5-flare` at
  `medium`** *(now recommended)* — a third of the price, the exact size asked for,
  PNG, a refusal with a stable code; the risk is an account that must pass the
  organisation check first, said in OpenAI's words on the first picture.
  (b′) Gemini as decided — no check in front of it, but JPEG, three times the price,
  and a violent prompt softened rather than refused. (c′) No default.
  **Owner's decision (2026-10-29, after the probe): (a′).**
- **F4. Editing in the first version.** (a) **Yes** — the `images` argument, by
  the handles the chat already has *(recommended)*: every provider in §2 takes
  input pictures (on the gateway, the models whose catalogue entry allows
  `input_references`; the tool refuses by name on one that takes none), and "make
  the sky darker" is the second thing anyone asks after the first picture.
  (b) Generation only; editing a stage later.
  **Owner's decision (2026-10-29): (a).**
- **F5. What the original is in the chat's files.** (a) **A new origin,
  `generated`** — a chat-schema step 6 → 7, so an older binary refuses the chat as
  a newer version's instead of skipping it in silence *(recommended)*. (b) Reuse
  `sandbox` — no schema step, but `/file list` would say the code wrote it.
  (c) No file at all, only the prepared copy on the tool message — the original
  resolution is lost the moment it arrives.
  **Owner's decision (2026-10-29): (a).**
- **F6. Does the model see what it drew.** (a) **Yes, by default**
  (`image_gen.show_to_model` on), through the existing path and its notes
  *(recommended)* — a model with vision can check the picture against the request
  and try again; a picture in the history costs its tokens on every later turn,
  which is why it is a switch. (b) Off by default.
  **Owner's decision (2026-10-29): (a).**
- **F7. Money.** (a) **The switch off by default, a ceiling per turn (4), and no
  drawing in follow-ups and background runs** *(recommended)*. (b) As (a), plus a
  confirmation popup before every call (the tool marked `danger`) — safe, and
  tiring by the third picture; the person can already stop a turn with `Esc`.
  (c) A popup only, no ceiling.
  **Owner's decision (2026-10-29): (a), amended** — the ceiling per turn is a
  setting (`image_gen.max_per_turn`, 4 by default) and `0` removes it; follow-ups
  and background runs do not draw; a turn started from Telegram must draw, which
  the bridge's tool set already allows (§6.6).
- **F8. How the person sees the picture.** (a) **The chip that names the file,
  `/file open`, and the phone** (§6.7) *(recommended)*. (b) As (a), plus a small
  half-block thumbnail in the card — our own ~50 lines over the decoded pixels
  (the ADR 0003 precedent), drawn on any 24-bit terminal including the Windows
  console host, but nothing in `mono` and a smear at the sizes a card allows.
  (c) Real pixels through `ratatui-image` now — the roadmap's track, with its own
  questions (scrolling a protocol image, `finish_frame`'s passes, tmux).
  **Owner's decision (2026-10-29): (a).**
- **F9. The bridge sends the picture back.** (a) **Yes, as a stage of its own** —
  a turn that came from the phone and drew answers with the picture as a photo
  *(recommended)*. (b) Not in this track.
  **Owner's decision (2026-10-29): (a)** — as a photo (the Bot API's `sendPhoto`;
  Telegram recompresses it, and the original stays in the chat's files), and as
  **stage 2**, before the rest of the providers and the command. The alternatives
  weighed and not taken: the photo plus the original as a file, the file alone;
  the phone inside stage 1, or last.
- **F10. After a refusal, in the same turn** (found by the probe, §11.2 — both
  models redrew a refused request with softened prompts, up to seven times, without
  asking). (a) **The tool stops drawing for the rest of the turn**: a call after a
  refusal is refused by the tool itself — "drawing is stopped for this turn after a
  refusal; tell the user what was refused and ask what to change" *(recommended)* —
  deterministic, free, and the person chooses the softened version, which is the
  person's picture. (b) Wording only: the refusal note asks the model not to call
  again and to ask first — cheaper to build, and the probe shows the current
  wording read as permission to rephrase; it would need its own measurement.
  (c) Keep the retries within the turn's ceiling — fewer round trips for the
  person, up to four charged refusals a turn on xAI.
  **Owner's decision (2026-10-29): (a).**
- **F11. Where a tool's picture goes on llama.cpp** (found by the probe, §11.3 —
  Gemma 4 wraps its reply in JSON after a picture inside a tool result, 28 of 30
  English requests). (a) **Re-home it on every OpenAI-compatible endpoint, as a
  gateway already gets it, behind a label that names the tool and carries no words
  of any language** (`image_generate → tool-image-3.png`) *(recommended)* —
  measured to end the JSON (0 of 29) with Qwen's calls and checks intact; the
  label's form measured on both models before it ships, since the Russian label
  moved English replies into Russian (Gemma 11 of 15, Qwen 3 of 9). It changes the
  wire for every producer — `python_exec`, MCP — so it is **a PR of its own before
  stage 1**, with the image smokes extended to assert that the reply is prose.
  (b) Re-home for Gemma only — telling the model by its name, which the project
  has been burned by. (c) Leave it: pictures from any tool keep breaking Gemma 4's
  English replies.
  **Owner's decision (2026-10-29): (a).**

## 8. Stages and the probe

**Stage 0 — the probe** (a `spike/` branch, frozen to a tag afterwards as the
project does with probes). Two questions, each with a go/no-go:

1. **Do the providers do what §3 says?** The same five prompts to each provider
   the owner has a key for, at the cheapest and the default quality: time to the
   picture, its size and format, what the provider counted, and the shape of a
   refusal — **one** prompt the providers' policies refuse, per provider and no
   more (xAI charges a refused prompt, and repeated violations are what an
   account gets flagged for), and one edit of the first picture. About a dollar
   in all. *GO:* every provider in F2 returns a picture for four of five and a
   refusal that can be told from a transport error.
2. **Does a local model use the tool well?** The tool offered to Gemma 4 31B and
   Qwen 3.8 on the reference stack, with a stub provider that returns a fixed
   picture and, on cue, a refusal. Ten conversations that ask for a picture, ten
   that do not (one of them about pictures), five that end in a refusal, five with
   vision on that ask the model whether the picture matches the request.
   *GO:* one call with a faithful prompt in ≥ 9/10 per model, ≤ 1/10 calls
   nobody asked for, 5/5 refusals relayed without a picture described, and the
   match judged right in ≥ 4/5. A wording that fails is reworded and measured
   again, as the withheld-image notes were (spec §9.10).

**Before stage 1 — F11 (a), done** (§11.5). Tool pictures re-homed on llama.cpp
behind a language-free label: a wire change for every producer, its own PR,
measured on Gemma 4 and Qwen 3.8 with a control build.

**Stage 1 — the slot and the tool** (built, §12). `image_gen` settings and their section,
the clients for OpenRouter, OpenAI and Gemini (F2), `image_generate` with editing
(F4), the original as a `generated` chat file (the 6 → 7 step, F5) and the copy
for the model (F6), the chip, the configurable ceiling (F7). Unit tests against
recorded responses; an `#[ignore]` smoke per provider and one through the agentic
loop on a live chat model.

**Stage 2 — the phone** (F9; built, §13): a turn from Telegram that drew
answers with the picture as a photo (`sendPhoto`) beside its text. The Bot API
takes a photo of at most 10 MB, so a larger original goes as a prepared copy; the
original stays in the chat's files either way. Live: a turn sent from the owner's
phone that draws.

**Stage 3 — xAI, the picker and the command.** xAI's dialect with the refusal
shape the probe found, `ModelSlot::Image` with the gateway's catalogue roles,
`/image generate`.

**Later, its own track** — pixels in the feed (F8 c).

## 9. Not in this track

- **Local generation** — the owner's exclusion (§1). An `external` provider for a
  server of the person's own (an OpenAI-compatible `/images/generations` in front
  of ComfyUI or a rented pod) is the door it would come through later; it is not
  proposed now.
- **Shape C** (§5).
- **Pixels in the feed** — the roadmap's own track; this one makes it more useful,
  and does not need it (F8).
- **Video generation** — the same slot shape, different endpoints, different
  prices; not asked for.

## 10. Sources

Read on 2026-10-29. OpenAI's and xAI's pages carry no date; the dates in §3 come
from their changelogs, deprecation and migration notices. Google's pages show
"Last updated" between 2026-09-17 and 2026-10-28.

**OpenAI**
- [Image generation guide](https://developers.openai.com/api/docs/guides/image-generation) — models, sizes, quality, moderation, per-picture costs, the organisation check
- [Images API: generate](https://developers.openai.com/api/reference/resources/images/methods/generate) · [edit](https://developers.openai.com/api/reference/resources/images/methods/edit) · [streaming events](https://developers.openai.com/api/reference/resources/images/generation-streaming-events)
- [Image generation tool (Responses)](https://developers.openai.com/api/docs/guides/tools-image-generation) · [Responses: create](https://developers.openai.com/api/reference/resources/responses/methods/create)
- [Chat Completions: create](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create) — no image output
- [Models](https://developers.openai.com/api/docs/models) · [Pricing](https://developers.openai.com/api/docs/pricing) · [Changelog](https://developers.openai.com/api/docs/changelog) · [Deprecations](https://developers.openai.com/api/docs/deprecations) · [Content provenance](https://developers.openai.com/api/docs/guides/content-provenance)

**Google**
- [Image generation guide](https://ai.google.dev/gemini-api/docs/image-generation) · [`generateContent` reference](https://ai.google.dev/api/generate-content) · [Migrating to the Interactions API](https://ai.google.dev/gemini-api/docs/migrate-to-interactions)
- [Models](https://ai.google.dev/gemini-api/docs/models) · [Pricing](https://ai.google.dev/gemini-api/docs/pricing) · [Deprecations](https://ai.google.dev/gemini-api/docs/deprecations) · [Changelog](https://ai.google.dev/gemini-api/docs/changelog) · [Imagen (shut down)](https://ai.google.dev/gemini-api/docs/imagen)

**xAI**
- [Image generation](https://docs.x.ai/developers/model-capabilities/images/generation) · [Image editing](https://docs.x.ai/developers/model-capabilities/images/editing) · [REST reference: images](https://docs.x.ai/developers/rest-api-reference/inference/images)
- [Image generation tool](https://docs.x.ai/developers/tools/image-generation) · [Models](https://docs.x.ai/developers/models) · [Pricing](https://docs.x.ai/developers/pricing) · [The 2026-11-02 retirement](https://docs.x.ai/developers/migration/imagine-image-quality-nov-2)

**OpenRouter**
- [Image generation guide](https://openrouter.ai/docs/guides/overview/multimodal/image-generation.md) · [Image API reference](https://openrouter.ai/docs/api/api-reference/images/generate-an-image.md) · [Chat completion reference](https://openrouter.ai/docs/api/api-reference/chat/create-a-chat-completion.md)
- [The image-generation server tool](https://openrouter.ai/docs/guides/features/server-tools/image-generation.md) · [Changelog](https://openrouter.ai/docs/changelog.md)
- The catalogues, read live: `GET /api/v1/models?output_modalities=image`, `GET /api/v1/images/models`

**Anthropic, DeepSeek**
- [Claude vision](https://platform.claude.com/docs/en/build-with-claude/vision) — "an image understanding model only"
- [DeepSeek models and pricing](https://api-docs.deepseek.com/quick_start/pricing)

**Terminal pictures**
- [ratatui-image on crates.io](https://crates.io/crates/ratatui-image) — 11.1.0 (2026-09-17), 12.0.0-rc.3 (2026-10-18)

## 11. Stage 0 — the probe (2026-10-29)

The tag `probe/image-generation-stage0` (the unmerged branch
`spike/image-generation-probe`, frozen); the provider script is
`tools/image_gen_probe.py`, the tool-use harness
`src/app/orchestrator/tests/image_probe.rs` (§11.4).

### 11.1 The providers

The five prompts of §8 (a photograph 1:1, lettering 2:3, a Russian prompt 3:2, a
wide scene 16:9, a tall one 9:16), one edit of the first picture ("make it
night… keep the fox exactly as it is") and one prompt of fictional graphic
violence, which the policies refuse — once per provider. Each provider's slot
default: Gemini `gemini-nano-banana-2.1` at `1K`, OpenAI `gpt-image-2.5-flare` at
`medium`, xAI `grok-imagine-image-2.0` at `1k`, and on the gateway
`black-forest-labs/flux-3-image` at `1K` — a third vendor beside the two direct
ones. **About $0.97 in all.**

| | Gemini | OpenAI | xAI | OpenRouter (FLUX-3) |
|---|---|---|---|---|
| **Pictures** | 5/5, edit ✓ | 5/5, edit ✓ | 5/5, edit ✓ | 5/5, edit ✓ |
| **Seconds** | 10–14 | 10–13 | 8–19 | 16–29 |
| **Format** | **JPEG**, 0.4–1.1 MB | PNG, 1.3–3.1 MB | JPEG, 0.1–0.6 MB | PNG, 1.0–2.1 MB |
| **2:3 · 16:9** | 848×1264 · 1376×768 | 1024×1536 · 1536×864 (as asked) | 832×1248 · 1280×720 | 832×1248 · 1360×768 |
| **What it counted** | 1120 image tokens a picture, any aspect, plus 600–860 thinking tokens | 439 image tokens (1:1), 343 (3:2), 280 (16:9); usage present | `cost_in_usd_ticks` | `usage.cost` |
| **One picture costs** | **$0.039** — $0.034 of picture and ~$0.005 of thinking | **$0.008–0.013** | $0.04; edit $0.07 | $0.048, edit the same |
| **The refused prompt** | **drawn** — "a cinematic … action-thriller photograph from a film scene" | `400` in 1.1 s, `moderation_blocked`, stage `input`, category `other` | `400` after **15.5 s**: `{"error": "Generated image rejected by content moderation."}` — and **charged $0.04** | `400` in 1.9 s: "Black Forest Labs blocked this request: it was flagged for graphic violence.", `metadata.provider_name` |

All four drew `MINDFORK` letter for letter.

What this changes:

1. **OpenAI is the cheapest, not Gemini.** `gpt-image-2.5-flare` at `medium` spent
   439 output tokens on a 1024×1024 picture — **$0.013** at $30 per million, against
   the $0.053 OpenAI publishes for `gpt-image-2` at the same quality. Gemini's real
   price is its list price plus thinking that cannot be turned off: $0.039. F3 was
   decided on "Gemini is the cheapest" (§7), so **F3 goes back to the owner**. The
   owner's key needed no organisation check; whether that is the key's history or
   OpenAI's rule, one account cannot say.
2. **Gemini answers in JPEG**, and the picture part carries a thought signature of
   0.9–3.2 million characters. A client that kept the response would keep megabytes
   of signature; the tool takes the image bytes and drops the rest (§6.2).
3. **Gemini softens instead of refusing.** The violent prompt came back as a film
   still with a cheerful sentence. A refusal on Gemini is the rarer path, so its
   handling (§3.2) is tested by a stub, not by provoking one.
4. **xAI refuses after drawing, and bills the refusal.** The shape is now known — an
   error *string*, not an object, beside the usage — and the price of a refused
   picture belongs in the result the model reads, so a model does not retry a
   refused prompt at $0.04 a time.
5. **The gateway names the vendor in the refusal**, and a model whose catalogue
   entry has no `resolution` must not be sent one: FLUX.2, Recraft and MAI list none
   (`GET /images/models`). The client sends only what the entry lists.
6. **Gemini's `["IMAGE"]` alone works** (the sixth call), so the client need not ask
   for text it throws away — and with `["TEXT", "IMAGE"]` its sentence is not always
   clean (one came back with a fragment pasted mid-sentence).

### 11.2 The local models

The real agentic loop (`spawn_orch_tools`) against `llama-server` b11715 on the
RTX 4090, the gate's own weights — Gemma 4 31B `q4_0` and Qwen 3.8 27B `Q4_K_M`
(ggml-org), each with its vision projector, `-c 32768 --jinja`. The default
profile's tools **plus** a stub `image_generate` with the description and result
lines of §6.3, so a call nobody asked for competes with every tool the model really
has. The stub hands back the matching OpenAI picture from §11.1 — or, on cue, the
wrong one, or a refusal. Thirty conversations a model, each in a new chat: ten that
ask for a picture (five English, five Russian; four name a shape), ten that do not
(two of them about pictures: PNG against JPEG, "describe a Japanese garden"), five
that end in a refusal, five that ask the model to check the result (three get the
wrong picture first).

| | Gemma 4 31B | Qwen 3.8 27B | GO |
|---|---|---|---|
| **Asked: one call, a faithful prompt** | 10/10 | 10/10 | ≥ 9/10 |
| **…the shape asked for** | 4/4 | 4/4 | — |
| **Not asked: calls** | 0/10 | 0/10 | ≤ 1/10 |
| **Refused: relayed, no picture described** | 5/5 | 5/5 | 5/5 |
| **Checked: judged right** | 5/5 (3 wrong pictures caught and redrawn) | 5/5 (the same) | ≥ 4/5 |
| **Minutes for 30** | 12 | 8 | — |

**GO on both models.** The prompts are full descriptions, not titles — a median of
81 words for Gemma, 64 for Qwen — in English for English requests and mostly for
Russian ones too (Qwen once wrote a Russian prompt in Russian). A refusal was said
in each model's own words with an alternative offered, and no reply described a
picture that was never drawn. Both name the file and `/file open #N` on their own when
the result line offers it.

Three findings, none of them in the GO criteria, each with a design consequence:

1. **After a refusal, both models try again — with a softened prompt, and without
   asking.** Gemma made 13 calls in the five refused conversations (one, 7), Qwen 16
   (one, 6); Qwen once sent an empty prompt. The note's "the same prompt will be
   refused again" is read as "change the prompt", which it literally permits. On
   OpenAI and the gateway an input-stage refusal is free and takes a second; on xAI
   each one is a charged picture and 15 s. A ceiling of 4 bounds it but does not
   stop it — see F10.
2. **A redraw may carry the wrong picture forward.** Qwen redrew a mismatched
   picture by passing it in `images` as the reference — on a real provider the fox
   would have leaked into the city. The description should say `images` is for
   pictures the user wants changed, not for retries.
3. **Gemma wraps its reply in JSON after a picture** — `{"action": "text",
   "action_input": "Here is your red fox…"}` — in 5 of its 10 asked conversations,
   all five English ones; one such blob even held a second `image_generate` call as
   text, never executed. Qwen never did. §11.3 measures whether it is the picture
   inside the tool result that does it.

### 11.3 Gemma's JSON, measured

The five English requests of §11.2 again, each in up to three variants, the
variants interleaved so a drift of the server falls on all of them alike: the
picture **inside the tool result** (what llama.cpp is sent today), **no picture**
(the result says it is saved and not shown), and the picture **re-homed** into a
user message after the result — what a gateway already gets
(`wire::rehome_tool_images`), forced on llama.cpp by a spike-only switch.

| Gemma 4 31B | inside the result | no picture | re-homed |
|---|---|---|---|
| Russian profile, 3 rounds × 5 | **14/15 JSON** | 0/15 | 0/15 |
| English profile, 2 rounds × 5 English requests | **9/10 JSON** | — | 0/10 |
| English profile, 2 rounds × 2 Russian requests | 0/4 | — | 0/4 |

1. **The picture inside the tool result is what does it** — on Gemma 4, after an
   English request: 28 of 30 such conversations across the runs, against 0 of 9
   Russian ones and 0 of 10 on Qwen. Every turn still made exactly one call; only
   the reply's shape broke.
2. **It is not image generation's alone.** Every producer that puts a picture in a
   tool result on llama.cpp — `python_exec`'s charts, an MCP screenshot — hands
   Gemma 4 the same message. The gate's smokes for them assert on the files and on
   what the model was told, not on the reply's shape, so nothing has caught it.
3. **Re-homing ends it** (0 of 29), and the model still sees the picture. Two
   things come with it. Under a Russian profile the re-homed picture's label is
   Russian (`prompt.images.label`), and Gemma answered English requests in Russian
   11 times of 15; under an English profile, 0 of 14 — the label's language, not
   the move. And twice in those 14 Gemma answered as if the person had sent the
   picture ("I'm glad you like the image!", "That is a very clean, bold start!"):
   the label names a file, not the tool that made it.

4. **Qwen loses nothing by it** — re-homed, under the Russian profile: 10 of 10
   asked conversations with one call, 5 of 5 checks right (the three wrong pictures
   caught and redrawn), no JSON. But the Russian label moved it too: 3 of 9 English
   requests answered in Russian, where with the picture inside the result it never
   switched. The label's wording has to carry no language — F11.

### 11.4 Verdict

**GO.** The providers do what §3 says, at the prices §11.1 measured: every one drew
five of five and the edit; three refuse in a shape the tool can tell from a broken
connection, and the fourth, Gemini, did not refuse at all — it softened the prompt
and drew. Both gate models call the tool when asked and only then, relay a refusal
without inventing a picture, and catch a wrong picture. Spent: $0.97 at the
providers; the local runs cost nothing.

What the probe added before stage 1, each decided by the owner the same day (§7):

- **F3 re-opened → (a′)** — OpenAI is the default, at a third of Gemini's price.
- **F10 (a)** — a refusal stops drawing for the rest of the turn.
- **F11 (a)** — a tool's picture is re-homed on llama.cpp too, behind a
  language-free label, in a PR of its own. It reaches past this track:
  `python_exec`'s charts and MCP screenshots meet Gemma 4 the same way.

The probe's code is frozen as the tag `probe/image-generation-stage0`:
`git show probe/image-generation-stage0:tools/image_gen_probe.py` (the providers,
and `score` for the rows), `…:src/app/orchestrator/tests/image_probe.rs` (the stub
and the thirty conversations), and the one-line spike switch in
`src/shared/api/openai/client.rs` (`PROBE_REHOME`) the control arm used.

### 11.5 F11, built (2026-10-29)

`fix/tool-images-re-homed`: `OpenAiClient::shaped_for_endpoint` re-homes a tool's
pictures on every endpoint, with no catalogue asked first, and a tool's picture is
labelled `tool → name:` (`request::tool_image_label`) — in the live round and on
history replay alike; a user's own image keeps its numbered, localized label.

Measured by the new smoke `a_tools_picture_leaves_the_reply_prose_live`: a drawing
tool beside the default profile's tools, a Russian profile, five English and two
Russian requests, each in a new chat; the build before the change (control) and the
build with it, run alternately on the same server.

| | JSON replies, control | JSON replies, fixed | English answered in Russian, control → fixed |
|---|---|---|---|
| Gemma 4 31B, 3 rounds each | **17 of 21** | **0 of 21** | 0 of 15 → 0 of 15 |
| Qwen 3.8 27B, 2 and 3 rounds | 0 of 14 | 0 of 21 | 2 of 10 → 3 of 15 |

Every turn drew, in both builds. The language-free label leaves Gemma's replies in
the request's language (the localized one had moved 11 of 15), and Qwen at its own
rate under a Russian profile — the same with the picture inside the result — so the
smoke counts a switch and does not fail on it; the label's form is held by unit
tests.

What the JSON needs is a reply with little to say. `sandbox_outputs_e2e_live` asks
Gemma 4 to say in a sentence what its chart shows, and that reply was prose in both
builds (2 of 2 each) — as the probe's "check the picture" conversations were
(§11.2). The envelope comes when the reply is "here it is". The existing image
smokes — `sandbox_outputs_e2e_live`, `tool_result_image_is_seen_live`,
`a_withheld_tool_image_is_not_described_live` — pass on Qwen 3.8 with the change.

## 12. Stage 1, built (2026-10-29)

`feat/image-generation-stage1`. What §6 designed, and where it moved:

- **The slot** — `image_gen` in `settings.json` (`shared/config.rs`,
  `ImageGenSettings`): `enabled` off, `provider` `openai`, a section per provider
  (`model_name`, `quality`, `api_key_env`, `url`), `show_to_model` on,
  `max_per_turn` 4. §6.1's sketch had `image_size` for Gemini and `resolution` for
  the gateway; they are one field, `quality` — *what the provider's price is set
  by* — read as OpenAI's `quality`, Gemini's `imageSize` and the gateway's
  `resolution`. The settings take no step: a section an older reader skips whole.
  The settings screen has a group of its own in *Tools*, after *Video*: the switch
  first, then who draws and that provider's rows; the quality row cycles through
  the chosen provider's values. The gateway's model row is a typed field for now —
  the picker (`ModelSlot::Image`) is stage 3's.
- **The clients** — `shared/image_gen/` (`openai.rs`, `gemini.rs`,
  `gateway.rs`). A refusal is `Drawing::Refused`, an answer; every other failure is
  an `Err` in the provider's words. The gateway's client asks `GET /images/models`
  once and sends `aspect_ratio`, `resolution` and `input_references` only where
  the model's entry lists them; a model that takes no pictures to change is
  refused by name before anything is sent. Its refusal has no code, so a `400`
  whose upstream message speaks of blocking, flagging, moderation or policy is
  read as one — the probe's FLUX refusal word for word.
- **The tool** — `features/tools/image_gen.rs`, gated by `ToolGate::ImageGen`.
  The ceiling and the stop after a refusal live in `ToolContext::draws`, fresh
  with every turn's context and shared by a sub-agent's clone, so a group child
  counts against its turn's ceiling. A woken turn (a follow-up, a background run's
  notice) is not offered the tool, nor is a background sub-agent; a turn from the
  phone is. The result names the file by the `#N` `/file list` shows — the probe's
  measured line — and the stored name is `image-N.<ext>`, the first `N` no file of
  the chat holds under any extension. A picture over `images.max_bytes` is handed
  to the loop as a prepared copy (§6.4's trap). `images` resolves `#N`, a stored
  file's name and a conversation image's name, `tool-image-2` without its
  extension included — the form the probe's models wrote.
- **The card** — the collapsed tool card names the saved file
  (`image_generate(image-3.png)`) instead of the start of the prompt; a new field
  on the call's record was not needed.
- **Chats 6 → 7** — the stamp for `FileOrigin::Generated`, with a golden v6 file
  (`chat_v6_sandbox_file.json`).

### 12.1 Live

**The providers** (`shared/image_gen/live_tests.rs`): each drew a 16:9 picture and
changed it, the edit carrying the first picture.

| | OpenAI `gpt-image-2.5-flare` medium | Gemini `gemini-nano-banana-2.1` 1K | OpenRouter `flux-3-image` 1K |
|---|---|---|---|
| **Drew** | 1536×864 PNG, 2.5 MB, 11.7 s | 1376×768 JPEG, 1.0 MB, 12.2 s | 1360×768 PNG, 2.1 MB, 24.0 s |
| **Counted** | 28 in / 280 out tokens | 23 in / 1981 out (picture + thinking) | $0.048 |
| **Changed it** | 13.4 s, 1335 in / 280 out | 14.2 s, 1143 in / 1853 out | 33.8 s, $0.048 |

**Through the loop** (`image_generate_draws_and_changes_through_the_loop_live`):
the default profile's tools, a live chat model on `llama-server` (RTX 4090,
`-c 32768 --jinja`, the vision projector loaded), OpenAI drawing. "Draw me a small
red fox sitting in fresh snow, in watercolor", then "Now make it night, with a full
moon — keep the fox exactly as it is", in one chat.

| | Gemma 4 31B `q4_0` | Qwen 3.8 27B `Q4_K_M` |
|---|---|---|
| **Draw** | one call, an 80-word description | one call, a 75-word description |
| **Change** | one call, `images: ["#1"]` | one call, `images: ["#1"]` |
| **Replies** | prose (2 of 2) | prose (2 of 2), each naming `/file open #N` |
| **Files** | `image-1.png`, `image-2.png`, both `generated` | the same |

**GO.** Gemma ran twice (the first run's only failure was the smoke counting a
chat's `.bak` beside it). Spent on the live runs: about $0.30. No refusal was
provoked live — the probe recorded each provider's (§11.1), and the stop after
one is pinned by the tool's tests.

## 13. Stage 2, built (2026-10-29)

`feat/image-generation-stage2`. F9 as the owner decided it: a turn from the phone
that drew answers with the picture as a photo.

- **What goes, and when.** The seat's reply is read off the turn before it lands
  (`SeatReply`), and the pictures with it: the turn's `AddChatFile` effects of
  origin `generated`, in the order drawn — a chart `python_exec` wrote is not one,
  and a bridged turn cannot run code anyway. The words go first, whole, as every
  reply's do; then each picture, read off the disk in a task of its own so a 4K
  original does not hold the orchestrator, goes to the bot's sender as
  `Outbound::Photo` — the original's bytes and the file's name. However the turn
  ended: a picture the provider drew was paid for and is kept, so a stopped or
  failed turn still sends it. A turn at the terminal in the bridged chat sends
  nothing to the phone, as its words do not.
- **What Telegram takes** (`features/bridge/telegram.rs`, `photo`). `sendPhoto`
  takes a photo of at most 10 MB whose width and height add up to 10 000 px at
  most, one side at most twenty times the other. The original goes as it is when
  its bytes say PNG or JPEG and it is within the first two; anything else — a
  4K PNG over 10 MB from the gateway, a WebP — goes as a **JPEG copy at most
  2560 px on its longer side** (`image_prepare::jpeg_copy`): Telegram recompresses
  every photo to JPEG anyway, and the `prepare` the model's copy goes through keeps
  a PNG a PNG, which at 2560 px can still be over the limit. The third rule no
  scaling mends; Telegram's refusal of it is said like any other. The type is read
  from the bytes, so `Outbound::Photo` carries none.
- **What the phone sees.** *sending a photo…* (`upload_photo`) while it goes up,
  then the photo with the file's name under it — `image-3.png`, the name a change
  can be asked for by. A photo that does not go — a refusal, the network still
  down after the voice note's retries (shared now, `retrying`), a file that cannot
  be read — is `ChannelFailure::PictureUnsent`: one line to the phone naming the
  file and the reason (its words went without it), and the terminal told.

### 13.1 Live

`a_picture_drawn_from_the_phone_reaches_it_live` (`tests/bridge_live.rs`): the
bridge against the tests' own Bot API (`TelegramFake`, which learned `sendPhoto`),
the default profile's tools plus `image_generate`, a live chat model on
`llama-server` (RTX 4090, `-c 32768 --jinja`, the vision projector loaded), OpenAI
drawing. From the phone: "Could you draw me a small watercolor of your lighthouse
at dusk?", then "Now make it night, with the lamp lit and the sea dark."

| | Gemma 4 31B `q4_0` | Qwen 3.8 27B `Q4_K_M` |
|---|---|---|
| **Turn 1** | 32.4 s; one call, 1024×1024 PNG, 2.4 MB | 27.5 s; one call, 1024×1024 PNG, 2.5 MB |
| **Turn 2** | 31.2 s; a change, `images: ["#1"]` | 24.6 s; a change, `images: ["tool-image-1.png"]` |
| **Replies** | prose, in character (2 of 2) | prose, in character (2 of 2) |
| **Photos** | after the words, the chat's file byte for byte | the same |

**GO.** The two models named the picture to change by different handles — `#1` and
the conversation image's name — both resolved, as stage 1 built. About $0.10 spent.

**Telegram itself — checked by the owner** the same day, from the phone, as the
bridge's own stages were: a self-portrait asked for in Russian came back as the
reply's words, then the photo with `image-1.png` under it.
