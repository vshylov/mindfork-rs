# Research: a chat whose history carries images, on a model that takes none

**Status:** measured (§2); fork W1 decided by the user on 2026-09-16 at the
recommendation — **(a)**, a note once per chat until the engine changes; implemented
(§6).

Stage 2 of [gateway-vision-catalogue.md](gateway-vision-catalogue.md), its fork
V2(b). Stage 1 made the engine's "no" reach a gateway, so no *new* image goes to a
model that cannot see it. A chat that got an image while a vision model was selected
still replays that image on every turn after a switch to one that cannot see — and
the measurement there showed a text-only route refusing each such turn. V2(b) asked
to measure the local side first, since the request path is shared.

Claims are tagged **[live]** (measured 2026-09-16) or **[code]**.

**Related:** spec §9.10 (an image is message-scoped and replayed as history),
[gateway-vision-catalogue.md](gateway-vision-catalogue.md) §2.2,
[docs/history/sandbox-file-exchange.md](../history/sandbox-file-exchange.md) §10–§11
(why the no-vision notes are directive), [docs/journal/engine.md](../journal/engine.md).

## 1. Where history images reach the wire **[code]**

One place: `request.rs::api_messages` → `message_to_api`, called by
`build_request_in` for every turn (and so for `/regen`, `/continue` and a woken turn).
A user message and a tool result both carry their `MessageImage`s as `ApiImage`s,
labelled `Image #N — "name":`. Nothing consults the engine's answer: the compaction
roll, the title and reflection read text only, so the turn is the one request that
matters.

The turn already has a lazily asked, per-turn memo of that answer
(`TurnLoop::vision`), used today only when a tool returns an image.

## 2. What was measured **[live]**

### 2.1 A local `llama-server` without a projector

CPU build b10936, `gemma-4-E4B-it` with no `--mmproj`; `/props` says
`modalities.vision: false`. The same request shapes as stage 1:

| shape | answer |
|---|---|
| image in the new message | `500 "image input is not supported - hint: if this is unexpected, you may need to provide the mmproj"` |
| image in an **earlier** message, a text-only follow-up | the same `500` |
| a tool result carrying the image as parts | the same `500` |
| a text marker where the image was | `200` |

So the stuck chat is not a gateway's alone. A managed or external llama.cpp started
without its projector refuses every turn of a chat that holds an image, exactly as
OpenRouter did with its `404`. On `external`, where the retry decorator runs, a `500`
is also retried twice before it is shown.

### 2.2 What a model does once the image is gone

The scenario that makes the choice. The fixture is a blue field with a white square in
the middle and a **small red square in the top-left corner**. History holds the image,
and an earlier assistant answer that mentions only the blue field and the white
square. The new question asks the colour of the corner square. The request then
carries the image either **dropped** (its part simply removed) or replaced by a
**marker** in the directive shape `loop.images_no_vision` was measured into:

> `[Image #1 — "figure.png" is not included: the current model does not accept images.
> You have not seen it — do not describe what it shows; say that you cannot see it.]`

Five runs per arm:

| model | dropped | marker |
|---|---|---|
| `qwen/qwen3-235b-a22b-2507` (OpenRouter) | **5/5** "There is no small square in the top-left corner of the image you sent." | **5/5** "I cannot see the image you sent, so I cannot determine the color…" |
| Gemma 4 31B (LAN, thinking off) | **5/5** "There is no small square…", 3 of them adding "there is only a white square in the center" | **5/5** "I cannot see the image, so I don't know what colour the square is." |

Dropping the image does not make a model say it cannot see. It makes the model treat
its own earlier description as the whole picture and **deny what is there**, 10/10.
The marker turns that into an honest decline, 10/10.

## 3. Decided, with the reason

- **A marker, never a silent drop** (§2.2). The marker is prompt scaffold, so it is in
  the profile language (axis A), like the label it replaces, and it names the image the
  way the label did. Then "the image I sent" and `/image list` still point at the same
  thing.
- **Only on the engine's "no".** `Unsupported` — `/props` `vision: false`, or a
  catalogue that lists no `image` (stage 1). `Unknown` sends the images as today (the
  optimistic path, fork F4 of multimodal-images.md); `Supported` does too, and every
  cloud answers `Supported`.
- **The request copy only.** The stored messages keep their images, so switching back
  to a vision model shows them again, and `/image`/`/file` still list them.
- **Asked in the turn, through the memo it already has.** Before the first round, a
  request whose messages carry any image asks `TurnLoop::vision` once. A chat without
  images asks nothing, as before. A chat with images on a server that has its projector
  now pays one `/props` per turn, under the same 5-second cap. Asking at build time
  instead would need an engine answer outside the turn that nothing keeps.
- **Every message the request carries**, the new one included. An image staged while
  the engine could not say, then sent after it said no, would otherwise be the same
  `500`/`404`.

## 4. Forks

### W1. What the user is told — **recommendation (a)**

The model says it cannot see when asked (§2.2), but the user is not asking the model
why a picture they can still see in the feed is ignored.

- **(a) a note once per chat, until the engine changes.** For example: "2 images in
  this chat were not sent: the current model does not accept images. Switch to a
  vision model to use them." Shown on the first turn that leaves images out; not
  repeated on the next turn of the same chat under the same engine; shown again after
  the engine is re-applied or after an app restart.
- (b) a note on every turn that leaves images out — accurate, and in a long chat on a
  text-only model the same line under every reply.
- (c) no note; the model's own "I cannot see it" is the explanation.

## 5. What a live run has to show

- **Local `llama-server` without a projector** (the CPU build above): a chat whose
  stored history holds an image completes a turn — no `500` — and asked about a detail
  of the image, the model declines. The note appears once (W1(a)).
- **Gateway, text-only** (`qwen/qwen3-235b-a22b-2507`): the same, with no `404`.
- **Regression, vision** (LAN Gemma 4 31B with its projector):
  `image_attachment_e2e_live` still sees the image a turn later, from history.

## 6. What was implemented

- **`request::withhold_images`** — a pure function over the request's messages: every
  image becomes `prompt.images.withheld`, naming it by its label (the label's own
  trailing colon dropped), after the message's text; an unlabelled image is "An image".
  It returns the count.
- **`TurnLoop::run`**, before the first round: when the request carries an image and
  `TurnLoop::vision` answers `Unsupported`, the images go as markers and the count rides
  `GenResult.images_withheld`. A request without images asks nothing.
- **`Orchestrator::note_withheld_images`** — `ui.chat.images_withheld` as a notice, once
  per chat (`images_withheld_noted`), the set cleared by `refresh_engine_facts`, which
  runs when the settings are applied and when the server's readiness flips.
- The stored messages are untouched.

**Tests**: 3269 green, 186 ignored (3266 / 185 before). The markers: two images on a
user message, a tool result's, an image-only message, an unlabelled one, a message
without images left alone, and the wording the measurement ran with, pinned. A chat on
a mock engine switched mid-chat from seeing to not: a turn without images asks nothing,
the vision turn sends the image, the next turn sends the marker and is told once, the one
after is not told again, and the stored message keeps its image. The once-per-chat rule
on a bare orchestrator, including "again after the engine's facts are asked anew".
**Seven mutations, all caught**, each by a named failing test, every run under a wall-clock cap: the turn never withholding, a silent drop instead of the marker, the marker before the text, the note on every turn, the note never again after the engine changes, the note when nothing was withheld, the count not carried to the result.

**Live — GO**, 2026-09-16, `a_history_image_on_an_engine_without_vision_live`: the real
sequence on the app's own paths, two starts of the app on one data root — a vision engine
attaches and sees the image, then the restart on an engine that takes none is asked
about the corner square the conversation never mentioned.

| vision engine → engine without vision | vision reply | the switched turn |
|---|---|---|
| LAN Gemma 4 31B + mmproj → CPU `llama-server` b10936, `gemma-4-E4B` without `--mmproj` | "Blue background, white square in the center." | "I cannot see the image you sent." — no error, told once, the next turn not told, the image still stored |
| OpenRouter `google/gemma-4-31b-it` → `qwen/qwen3-235b-a22b-2507` | "Blue background, white square." | "I cannot see the image you sent, so I cannot determine the color of the small square." — the same |
| **control**: the local pair with the rewrite switched off | "The background colour is blue…" | **red in 18 s**: both turns `500 "image input is not supported"`, the stuck chat reproduced |

And the regression half: `image_attachment_e2e_live` on the LAN Gemma 4 31B with its
projector, green — the turn now asks `/props` because the history holds an image, and
`Supported` changes nothing. The control arm first **hung** for ten minutes: the smoke
waited for the note without a bound, and the control arm has no note to give. Lessons §2
records it a fourth time; every wait in the smoke and the unit test is bounded now.
