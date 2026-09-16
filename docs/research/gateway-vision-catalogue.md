# Research: whether a gateway's model takes images, from its catalogue

**Status:** measured (§2); forks decided by the user on 2026-09-16, both at the
recommendation — **V1(a)**, the listing read in both directions, and **V2(b)**, a
chat whose history already carries images is a stage of its own. Stage 1 (V1)
implemented (§6).

The last buildable item the OpenRouter review left behind (roadmap, "Provider
bridges"): on a gateway `vision()` is always `Unknown`, because it reads llama.cpp's
`/props` and nothing else — so an image meant for a text-only model is sent anyway.
OpenRouter documents `architecture.input_modalities` on the same catalogue entry the
context window and the sampling list already come from
([gateway-capabilities.md](../history/gateway-capabilities.md)). The roadmap said to
measure first what a text-only route does with an image (lessons §3). This is that
measurement.

Claims are tagged **[live]** (measured on OpenRouter, 2026-09-16) or **[code]** (read
in this repository).

**Related:** spec §9.10 (images in a message), §3.4 (the engine's lifecycle),
[openrouter-external.md](openrouter-external.md) §4.2 (where "no `/props` → no
`modalities`" was first written down, as correct at the time),
[gateway-images-and-continue.md](../history/gateway-images-and-continue.md) §1.2
(what image-capable routes do with a tool's image),
[multimodal-images.md](multimodal-images.md) (fork F4, the optimistic `Unknown`),
[docs/journal/engine.md](../journal/engine.md).

## 1. What the answer already drives **[code]**

`VisionSupport` has three values, and two consumers already act on `Unsupported`:

- **`/image attach`** refuses before decoding, with `ui.err.image_no_vision` — "switch
  to a vision-capable model or provider" (`images.rs`). On `Unknown` it attaches and
  says once that the engine could not say.
- **A tool's images** (`python_exec` charts, MCP screenshots) are withheld from the
  model, and the result says how many it did not see (`record_call`,
  `loop.images_no_vision`). On `Unknown` they are sent.

A stored message's images are **replayed as history on every later turn**
(`request.rs::message_to_api`, spec §9.10: an image is message-scoped). Nothing strips
them, whatever the engine answers.

`OpenAiClient::vision` reads `/props` → `modalities.vision`. A gateway serves no
`/props`, so the answer is `Unknown` there, always.

## 2. What was measured **[live]**

Instrument: a Python probe with the account's key. A 64×64 solid-orange PNG as a
base64 `data:` URL; the question "What single colour fills the attached image? Answer
with one word."; a blind arm without the image beside every seeing arm.

### 2.1 The catalogue

`GET /api/v1/models`: **444** models, **every one** carrying
`architecture.input_modalities` (`text` 444, `image` 273, `file` 169, `video` 78,
`audio` 44), none empty. The older `architecture.modality` string (`text+image->text`)
agrees with the list about images on **all 444**.

`GET /api/v1/models/{id}/endpoints` repeats the model's `architecture` and gives each
route pricing, limits and `supported_parameters` — **no modality field per route**. So
the model's list is the only claim the gateway publishes.

### 2.2 A text-only model given an image

Six models the catalogue lists as `["text"]`, four shapes each:

| shape | `deepseek-r1` | `gpt-oss-120b` | `llama-3.3-70b` | `qwen3-235b-2507` | `deepseek-chat-v3.1` | `mistral-nemo` |
|---|---|---|---|---|---|---|
| image in the new user message | 404 | 404 | 404 | 404 | 404 | 404 |
| image in an **earlier** message, a text-only follow-up | 404 | 404 | 404 | 404 | 404 | 404 |
| a tool result carrying the image as parts | 404 | 404 | 404 | 404 | 404 | 404 |
| the re-homed shape (tool text-only + a user message with the image) | 404 | 404 | 404 | 404 | 404 | 404 |

Every one is the same body, with the router's own trace:
`{"message": "No endpoints found that support image input", "code": 404, "metadata":
{"routing_funnel": [{"step": "Initial Endpoints", "endpoint_count": N}, …],
"failed_routing_step": "Filter by Image Support"}}`. The refusal happens **in the
gateway**, before any provider sees the request, and it happened on 24/24.

The blind arms, the same question with no image, answered `Gray`, `Black`, `black`,
`Blue` — and R1 spent its cap reasoning. So a route that *dropped* the image instead
of refusing would have been believed. None did.

### 2.3 Image-listing controls

`google/gemma-4-31b-it` (Chutes) and `anthropic/claude-haiku-4.5` (Amazon Bedrock),
the same image and question: `Orange`, `Orange`. What image-capable routes do with a
tool's image specifically was measured on 29 route-model pairs in
[gateway-images-and-continue.md](../history/gateway-images-and-continue.md) §1.2, and
fixed by re-homing it (H1).

### 2.4 What this means for the app today

- **A chat on a text-only model breaks for good on its first image.** The attach
  succeeds with the "could not say" note, the send is a `404`, and since the image is
  history from then on, **every later turn in that chat** is the same `404` (§2.2, row
  two) until the message that carries it is gone.
- **A tool's image does the same, without the user attaching anything.** By rows
  three and four, a `python_exec` chart on `deepseek/deepseek-r1` turns the next
  round into a `404` in either shape the client can send, and the stored tool message
  keeps the chat broken. With `Unsupported` the image is
  withheld and said, and the turn goes on (§1).
- **The absence of `image` in the list is a certain "no"**: the gateway's own router
  applies that very filter (§2.2, 24/24). A listed `image` is weaker — it passes the
  gateway, and some routes still fail it for a tool image (fixed by H1).

## 3. Decided, with the reason

- **`/props` first, the catalogue second** — the rule the context window already
  follows: a running llama.cpp describes this process, a catalogue the model in the
  abstract. A local `llama-server` answers `/props`, and its `/v1/models` has no
  `architecture`, so it is untouched either way.
- **Silence is never a claim.** No catalogue, no entry for the configured model, an
  entry without `architecture.input_modalities`, or a list that is not a list of
  strings all leave `Unknown`, exactly as now. The key is read leniently (raw JSON,
  the `reasoning` precedent), so an odd spelling cannot fail the entry that carries the
  window and the sampling list.
- **Nothing new downstream.** Both consumers of `Unsupported` exist, with their notes
  and their tests; this changes the answer they get on a gateway, not what they do with it.
- **The memoised catalogue is reused** (`catalogue_entry`, H1.1): no new request.

## 4. Forks

### V1. What a listed `image` means — **recommendation (a)**

- **(a) both directions.** `image` listed → `Supported`; a list without it →
  `Unsupported`. The attach note "this engine does not report whether it takes images"
  stops appearing on a model the catalogue vouches for — it would be untrue there.
- (b) only the "no". A listed `image` stays `Unknown` and keeps the caveat, on the
  grounds that the listing is weaker (§2.4). But the caveat is about the *engine not
  saying*, which is no longer true, and the routes that fail despite the listing
  failed a *tool* image, which H1 already fixed.

### V2. A chat whose history already carries images, on a model that takes none — **recommendation (b)**

§2.2 row two, reachable through V1 too: a chat that got an image while on a vision
model, then switched to a text-only one, is refused on every turn. V1 keeps new images
out, but it does not unstick a chat that already has one.

- (a) **In this track**: when the engine says `Unsupported`, the request carries each
  history image as a short text marker instead, and the model is told it cannot see
  them. This touches request building in **every** mode, including a local
  `llama-server` without a projector, whose answer to a history image is unmeasured.
- **(b) A stage of its own**, after V1 lands, measured on both stacks first. Same
  branch or its own PR, whichever you prefer.
- (c) Leave it. The `404` names its cause; the way out is a new chat.

## 5. What a live run has to show

- **Gateway, text-only** (`deepseek/deepseek-r1`): `/image attach` is refused with
  `ui.err.image_no_vision`; a `python_exec` chart is withheld with the note and the
  turn completes with no `404`.
- **Gateway, vision** (`google/gemma-4-31b-it`): attach with no "could not say" note
  (V1(a)), and the model names the colour.
- **Local `llama-server`**: unchanged. `/props` answers first and the catalogue isn't
  consulted for this.

## 6. Stage 1 — what was implemented

- **`ModelEntry::takes_images`** ([wire.rs](../../src/shared/api/openai/wire.rs)) —
  `architecture` kept as raw JSON beside `reasoning`, and read as a list of strings or
  nothing: `image` listed → `Some(true)`, a list without it → `Some(false)`, an empty
  list or any other shape → `None`.
- **`OpenAiClient::vision`** ([client.rs](../../src/shared/api/openai/client.rs)) — `/props`
  first, then the memoised catalogue entry for the configured model. A client with no
  model makes no catalogue request, so every llama.cpp test and setup without a name is
  byte-for-byte what it was. The retry decorator already delegated `vision`.
- Nothing downstream changed (§3): `/image attach` and a tool's images already acted on
  `Unsupported`.

**Tests**: 3266 green, 185 ignored (3263 / 184 before). The listing, read in its measured
shape and seven kinds of silence, with an odd key not costing the entry its window. On
the wire through the client: a text-only row, an image row, a row without the key and a
model the catalogue lacks, each after `/props`; and `/props` saying `vision: false`
standing against a catalogue that lists images, with no second request.
**Five mutations, all caught**, each by a named failing test: the catalogue never asked, the catalogue asked before `/props`, a text-only listing read as silence, an empty list read as a no, a list with odd items still answering. The second first *hung* the run for half an hour on an unbounded test stub, lessons §2's recorded trap a third time; the client's stubs now share one bounded accept (`accept_before`), and rerun, it fails in seconds.

**Live — GO**, 2026-09-16, one declared smoke on the app's own paths,
`a_gateways_catalogue_decides_whether_images_are_sent_live`
(`MINDFORK_LIVE_VISION_EXPECT`), run as `external` the way the app runs on a gateway:

| stack | model | result |
|---|---|---|
| OpenRouter | `qwen/qwen3-235b-a22b-2507` (text-only) | `/image attach` refused with `ui.err.image_no_vision`; in the sandbox, the `python_exec` chart sent **0** images with the note, no error, and the turn ended in a reply that said it could not view the chart and answered from the numbers in the request |
| OpenRouter | `google/gemma-4-31b-it` (vision) | attached with no "does not report" note; `image_attachment_e2e_live` green, `blue` + `square`, then `White` a turn later from history |
| LAN `llama-server` | Gemma 4 31B + mmproj, no model named | the same two smokes green: `/props` answers, the catalogue is never asked |

Two things the run turned up, neither this stage's:

- **The first text-only model tried, `deepseek/deepseek-chat-v3.1`, returned its answer
  inside the reasoning field** with reasoning on — `Stop`, empty content, 729 characters
  of "thoughts" ending in the actual reply. Nothing about images: it is that route's
  behaviour, so the smoke uses a model that is not a hybrid reasoner.
- **`python_exec`'s result contradicts itself** on a model that takes no images: the
  file's entry says `shown to you below`, and the loop's note beneath it says no image
  was shown. The tool writes its entry before the loop knows the answer. It predates
  this work (a local server without a projector says the same thing) and is filed as a
  separate task.
