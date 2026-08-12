# Research: images returned by tools (MCP)

**Status:** research complete, forks await the user's decision (2026-08-13).
Roadmap item: "images from MCP/tool results". Everything in §2 was measured
**live** on 2026-08-13 against the reference stack and four real cloud keys —
each arm with a **control**, because the first probe round produced a confident
hallucination that looked exactly like success (§2.1).

Related: [multimodal-images.md](multimodal-images.md) (the track this closes out;
its §5 deferred this item), spec §9.10 (image input), spec §9.6 and
[ADR 0007](../decisions/0007-plugins-mcp-host-import-format.md) (the MCP host),
spec §13.4 (untrusted content).

## 1. Problem

An MCP tool can return image blocks — the protocol has had them from the start,
and a screenshot tool is one of the obvious things people plug in. The client
parses them today and **throws them away**: `McpClient::call_tool`
(`shared/mcp.rs`) concatenates `text` blocks and collapses everything else to
`[image content omitted]`. The model is told an image existed and cannot see it —
the "message that describes a situation without saying what is possible"
shape from [lessons §4](../lessons.md), except here nothing *is* possible.

Since the multimodality track landed, the machinery to carry an image to a model
exists and is proven on all five engines. What is missing is a way to attach one
to a **tool result** rather than to a user message. That is a different position
in the conversation, and the question this document answers is whether the
providers accept it there at all.

## 2. What we learned live (2026-08-13)

### 2.1 The first probe round was wrong, and the control is what showed it

Round 1 asked a model to describe a generated fixture (blue field, white square)
delivered inside a tool result. Every arm answered *"the background colour is
blue, and there is a white square in the centre"* — apparently a clean pass.

It proved nothing. Adding a **control arm with no image at all** produced
*"light blue background, white five-pointed star"* — the model was answering the
*question*, confidently, from nothing. Re-run with a fixture a guess would not
match (green field, white **circle**), the control's invention diverged from the
truth and the signal became readable. Every number below therefore comes from a
matched pair.

This is the trap [lessons §3](../lessons.md) records as "an instrumentation bug
looks exactly like evidence", and the reason [§2](../lessons.md) insists a smoke
must be able to fail.

### 2.2 Four of five providers carry an image inside a tool result

| Provider / model | Wire shape used | Control (no image) | With the image |
|---|---|---|---|
| **llama.cpp** local, `gemma-4-31B_q4_0-it` + mmproj | content parts on the `role:"tool"` message | *"light blue … five-pointed star"* (invented) | **"green … white circle"**, +51 prompt tokens |
| **Anthropic** `claude-haiku-4-5` | `tool_result.content` = `[text, image]` blocks | *"I'm unable to see the screenshot"* | **"green … white circle"** |
| **OpenAI Responses** `gpt-5-nano` | `function_call_output.output` = `[input_text, input_image]` | *"I can't view the screenshot content here"* | **"Green background and a white circle"** |
| **xAI** `grok-4.5` | content parts on the `role:"tool"` message | *"Unknown (no image data provided)"* | **"Green background with a white circle"** |
| **Gemini** `gemini-2.5-flash` | `functionResponse.parts` with `inline_data` | *"I cannot … view or interpret images"* | **HTTP 400** |

Gemini's refusal is explicit and worth quoting, because its wording bounds how
permanent it is:

> `Multimodal function responses are not supported for this model.`

"for this model" — so this is a per-model limitation rather than a protocol gap,
and a future Gemini may lift it. It is also a **hard 400**, i.e. the failure
lands mid-turn and costs the round, which is the worst of the failure modes and
the reason a fallback cannot be optional.

Two further findings:

- **The local path works despite the chat template.** Gemma's template extracts
  only `text` parts out of a tool result's content array, so reading it predicted
  the image would be dropped. It is not: llama.cpp's OpenAI-compat layer lifts
  media out of *any* message's content parts before templating and substitutes
  its `media_marker`. Reading the template was the wrong instrument — the same
  lesson as "use the project's own client, not curl and awk".
- **A user message after the tool result also works**, on every provider
  including Gemini (it is the path `/image attach` already uses). That makes it a
  viable uniform fallback, at the cost of attributing the image to the user
  rather than to the tool.

## 3. Baseline

None usable. The bytes are parsed and discarded in `call_tool`; nothing
downstream ever sees them. The nearest thing to a workaround today is for the
user to save the image and `/image attach` it, which defeats the point of the
tool having produced it.

## 4. Architecture: where it lands

The domain already has the field. `Message.images: Vec<MessageImage>` exists
(spec §9.10) and a tool result is an ordinary `Message` with `role: Tool` — so a
tool-produced image needs **no new storage, no migration, and no new persistence
path**. What changes:

1. **`shared/mcp.rs`** — `McpCallResult` gains `images: Vec<McpImage>` (base64 +
   mime), and `call_tool` stops collapsing `image` blocks. Audio and resource
   blocks keep their placeholder.
2. **The tool contract** (`features/tools`) — a tool's outcome can carry images.
   Only MCP produces them today, but the seam belongs on the contract rather than
   in the MCP branch, so a future built-in screenshot tool needs no rework.
3. **The orchestrator** — when it records the tool-result `Message`, it prepares
   the images through the existing `image_prepare` path (downscale, normalize,
   cap) and puts them on that message.
4. **`request::message_to_api`** — the `Tool` arm maps them onto `ApiMessage`,
   exactly as the `User` arm already does.
5. **The four wire builders** — each already knows how to serialize an image; what
   is new is emitting them on a *tool* message: content parts for Chat
   Completions (llama.cpp + Grok), `tool_result.content` blocks for Anthropic,
   `function_call_output.output` parts for Responses, and for Gemini the fallback
   of §5 F1.
6. **The feed** — a tool block whose result carried an image shows the same chip
   a sent image does.

## 5. Forks (need the user's decision before implementation)

### F1 — what happens on Gemini? *(the one that matters)*

| | A. Fall back to a user part | B. Keep the placeholder | C. Refuse the call |
|---|---|---|---|
| What the model sees | the image, in a user turn right after the tool result, labelled as that tool's output | `[image content omitted]`, as today | an error instead of a result |
| Measured | works (it is the `/image attach` path) | works, uselessly | — |
| Cost | the image is attributed to the user, not the tool | the feature simply does not exist on Gemini | a working tool becomes unusable |
| Risk | a model may read a user-role image as the user's own request | none | none |

**Recommendation: A.** B leaves one provider silently worse for no reason, and C
punishes the user for a provider limitation. The attribution cost of A is real
but small, and it is mitigated the same way the existing per-image label is: the
part says whose output it is (`Result of take_screenshot:`), in the profile
language (axis A). Note the sub-decision this implies: the fallback is chosen
**per provider**, not per failure — we do not send a request, catch a 400 and
retry, because a mid-turn retry after a hard error is exactly what the retry
decorator refuses to do once a turn is committed.

### F2 — how many images may one tool result carry, and how big?

An MCP server is third-party code; nothing stops it returning fifty screenshots
or a 50 MB PNG, and every one of them would then ride **every subsequent turn**
of the conversation.

**Recommendation:** reuse `config.images` — `downscale_px` and `max_bytes` per
image, applied by the existing `image_prepare` — plus a **per-result cap of 4**,
with the extras dropped and *stated* in the tool result text ("3 more images were
not included"). Silence there would be the "no silent caps" failure the workflow
warns about. `max_count` is deliberately not reused: it bounds what a *user*
stages for one message, which is a different budget from what a tool returns.

### F3 — is this gated, or on by default?

MCP tools already sit behind a double opt-in (the server is enabled, the tool is
enabled) and count as dangerous for the confirmation setting (spec §9.8). An
image adds a **new** hazard on top: instructions painted into pixels, which the
user cannot see at all today (the feed shows a chip, not the picture).

| | A. On by default | B. A setting, default on | C. A setting, default off |
|---|---|---|---|
| | inherits the MCP opt-in and nothing more | one switch to turn tool images off | the feature is invisible until found |

**Recommendation: B** — `tools.mcp_images` (or similar), default **on**. The
double opt-in is a real gate and a user who enabled a screenshot server wants its
screenshots; but "an image from a third party reaches the model" is exactly the
kind of thing a cautious user should be able to switch off without disabling the
server. Plus a line in spec §13.4: pixels are untrusted content, and the DATA
framing that protects text blocks has no image analogue.

### F4 — does the feed show the picture?

**Recommendation: no, not in this track** — the same chip a sent image gets, on
the tool block. Rendering pixels in a terminal is its own roadmap item, and this
feature does not change the argument either way.

### F5 — built-in tools too?

**Recommendation:** put the seam on the tool contract (an outcome may carry
images) but ship **only** the MCP producer. No built-in tool has an image to
return today, and inventing a consumer for an unused path is how dead code gets
comments explaining why it exists.

## 6. Scope estimate

| Piece | Size |
|---|---|
| `shared/mcp.rs` — parse image blocks, cap, `McpCallResult.images` | ~60 lines + tests |
| tool contract + the MCP tool adapter | ~40 lines |
| orchestrator — prepare and attach to the tool message | ~50 lines |
| `request.rs` — the `Tool` arm | ~10 lines |
| four wire builders + their "no images is unchanged" tests | ~120 lines |
| Gemini fallback (F1-A) | ~40 lines |
| setting (F3-B), feed chip, i18n ×2, docs | ~80 lines + prose |
| live smokes: one per provider through a fake MCP server returning an image | ~120 lines |

**One PR, roughly a day** including the live runs — the wire work is small
because every builder already serializes images; what is new is the position.

## 7. Open questions

- **Does Grok's Chat Completions path have its own cap** on images inside a tool
  result? Not probed beyond one image.
- **Which Gemini models, if any, accept multimodal function responses?** The
  error names the model, so a capability probe may become possible; until then
  F1-A applies to every Gemini model.
- **Does an image in a tool result survive history compaction?** The roll digest
  represents tool activity textually, so a folded image disappears like any other
  — consistent with spec §9.10, worth one line in the summary prompt.
