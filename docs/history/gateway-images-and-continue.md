# Track plan: through a gateway, a tool's image and `/continue` belong to the route

**Status:** design complete, **all forks decided** (the user, 2026-09-15), and
**both stages implemented and live GO** — H2 (§7) and H1 (§8); F5 measured and
closed (§9).
**M3 measured — GO for the app** (§1.1). The two items the OpenRouter review left
as "worth knowing, not worth a blind change" (F6) are now measured, and both
**harden into real limits** (§1.2, §1.3).

- **H1 — (b), with H1.1 (i):** re-home a tool's images when the catalogue
  answered, decided inside `OpenAiClient`.
- **H2 — (b):** on a gateway `/continue` refuses, except the slugs whose direct
  mode continues (Anthropic ≤ 4.5, Gemini).
- **H3 — (ii):** H2 first, H1 second; F5 is measured beside H2.

F6 of [openrouter-external.md](../research/openrouter-external.md) §5 said of both
halves that the answer lives one hop downstream — in the provider the gateway
routes to, not in the gateway and not in this client. M3 was the run that could
turn that from a prediction into a fact. It did, in opposite directions:

- **a tool's images mostly arrive** — and on a minority of routes fail in three
  different ways, one of which is indistinguishable from success;
- **`/continue` mostly does not work** — and where it does not, the app stores
  the restart glued onto the partial, in the same message, with no error.

**Related:** spec §6.4 (continuation), §9.10 (images in a message),
[mcp-tool-images.md](../research/mcp-tool-images.md) §2.2 and fork F1 (the
user-part fallback Gemini already takes),
[continue-generation.md](../research/continue-generation.md) §2, §7.1 (the
per-provider continuation table this extends),
[gateway-capabilities.md](gateway-capabilities.md) (the positive signal every
fork below keys on), [ADR 0004](../decisions/0004-engine-contract-multi-provider.md),
[docs/journal/engine.md](../journal/engine.md).

## 1. What was measured

### 1.1 The app itself (M3) — GO

Run by the author on 2026-09-15: the v0.9.9 dev build in Windows Terminal, on a
throwaway portable data root, `external` → `https://openrouter.ai/api/v1`,
`anthropic/claude-haiku-4.5` (served by **Amazon Bedrock** — the tool call's id,
`toolu_bdrk_…`, names the route), `python_exec` in the Wasmer sandbox with the
network off, `tools.python_images` on.

- **A chat turn** — answered.
- **A chart the model could only know by looking at it.** The request closed the
  text channel: *numpy, seed 7, six integers 10–100, each bar's colour drawn from
  the same generator, print nothing, save the PNG, then say from the chart which
  colour the tallest bar is and roughly how tall.* The stored call printed
  nothing — its result is the `files:` section alone, with `images: 1` on the
  tool message — and the reply, *"the tallest bar is blue (index 4), about
  93–94"*, matches the rendered chart (bar 4, blue, 93). No model computes
  `RandomState(7)` in its head; the image was the only channel.
- **The catalogue** answered for a second model: the log's
  `context_length=Some(200000) sampling_fields=12` is N1 of the previous track,
  re-measured on a model that is not `deepseek/deepseek-r1`.
- **The `/file` round trip.** `/file list` showed `#1 chart.png` (stored) and
  `#2 tool-image-1.png` (the chat's images) in one numbering; `/file open
  chart.png` opened the system viewer on the right bytes; `/file folder` opened
  the chat's folder; `/file open #2` refused with the "lives in the
  conversation, not on disk" note; `/file remove chart.png` removed the item and
  deleted the copy (`data/files/<chat>/` is empty after); the list then showed
  `#1 tool-image-1.png`.
- **One defect, out of this track's scope:** a bare `1` is not a handle —
  `resolve_handle` recognises only `#N` — and the refusal (*"not attached
  «1»"*) does not say `#1` would work, the lessons §4 shape. Filed as a task of
  its own.

### 1.2 A tool's images, per routed provider

**Instrument, in two steps.** First the project's own smoke,
`tool_result_image_is_seen_live`, through `OpenAiClient` on the gateway's
default routing, blind arm beside the seeing one: green on `openai/gpt-4.1-mini`
(four runs), `anthropic/claude-haiku-4.5`, `google/gemini-2.5-flash`,
`qwen/qwen3.6-27b` and `x-ai/grok-4.5`, and **red once on
`google/gemma-4-31b-it`**, whose reply was `ครั้ง`. The client discards which
route answered, so the second step sends the same turn as raw SSE pinned with
`provider.only` and fallbacks off, two arms each, in **two shapes**:

- **tool** — the image as a content part of the `role:"tool"` message, which is
  today's wire;
- **re-homed** — the tool message text-only, and the image in a `user` message
  right after it, behind a label naming the call.

The criterion is the smoke's (`green` and `circle`). Nearly every blind arm
invented another picture or said it could not see one — **three runs did not**,
all Qwen: on Venice 1 of 8 blind runs in the re-homed shape, on DeepInfra 2 of 8
in today's shape, and the one of those whose text was kept reads *"green, and
the shape in the centre is a black circle"* — a guess the keyword criterion
cannot tell from sight (lessons §3). So a single seeing run proves little on
Qwen, and every seeing count below is read against its own route's blind count:
8/8 against 1/8 separates, 1/1 would not have.

| route | model | tool shape (today) | re-homed |
|---|---|---|---|
| OpenAI | gpt-4.1-mini | sees, 5/5 | sees, 5/5 |
| Azure | gpt-4.1-mini · claude-haiku-4.5 | sees, 5/5 · sees | sees, 5/5 · sees |
| Anthropic · Amazon Bedrock · Google Vertex | claude-haiku-4.5 | sees | sees |
| Google · Google AI Studio | gemini-2.5-flash | sees | sees |
| xAI | grok-4.5 | sees | sees |
| Parasail | gemma-4-31b-it | sees — and default routing chose it 20/20 | sees |
| Novita · SambaNova · Together · SiliconFlow · Crusoe | gemma-4-31b-it | sees | sees (Crusoe: `429`) |
| Friendli | gemma-4-31b-it | sees 4/5, with template tokens (`ครั้ง-<\|channel>thought <channel\|>`) leaking into the reply — the shape of the smoke's one red | sees |
| Chutes · SiliconFlow · Phala · Alibaba | qwen3.6-27b | sees | sees |
| Chutes | gemma-4-31b-it | **silently blind**, 0/3 — a `200` describing another picture | sees, 3/3 |
| Venice | qwen3.6-27b · mistral-small-3.2 | **silently blind**, 0/8 — *"grey, … a triangle"*, *"green, … a white snake"* · **silently blind**, 0/2 | sees, 8/8 (blind 1/8) · sees, 2/2 |
| DeepInfra | gemma-4-31b-it · qwen3.6-27b · mistral-small-3.2 | **`422`** *"Input should be a valid string"* on `messages.2.tool.content` (Qwen 0/8) | sees, 3/3 · sees, 8/8 · sees, 2/2 |
| Parasail | mistral-small-3.2 | **`400`** | sees, 2/2 |
| ModelRun | gemma-4-31b-it | **`400`** *"templated prompt has 0 media marker tokens for 1 expected runs"*, 0/3 | sees, 3/3 |
| CoreWeave | gemma-4-31b-it | **`502`**, 0/3 | sees, 3/3 |

A count is "n/N" where the pair was repeated; the rest are single runs.
**In total, 29 route-and-model pairs**: in today's shape 20 see, 6 refuse the
request and **3 answer confidently about a picture they never received**. In the
re-homed shape **every pair that answered — 28 — saw the image**.

Two notes on the instrument. The Mistral routes also reject a tool call id that
is not nine alphanumerics, so their rows were measured with such an id — real
turns carry the id the model generated. And the first pass used a 128 px
fixture where the smoke uses 256 px: at 128 px the OpenAI route read the
tool-message image blind 0/3 and the re-homed one 2/3, while at 256 px both were
5/5 — a second, size-dependent effect of that route, not of the shape. The app's
images are chart-sized (850×546 in §1.1), so the table uses 256 px wherever a
route disagreed with itself.

### 1.3 `/continue`, per routed provider

**Instrument.** The body `/continue` puts on the wire — the trailing assistant
partial from `continue_probe.rs` (*"Per the Zorbville atlas, the capital of
France is"*, whose invented marker makes echo, continuation and restart three
different byte patterns), plus `continue_final_message: true`,
`add_generation_prompt: false` and `chat_template_kwargs.enable_thinking:
false` — sent raw over SSE with `provider.only` pinning, beside a control arm
**without** the three fields. On every route of a model that does not reason the
two arms **agreed**: through the gateway those fields do nothing. Where they
differed — Qwen on Chutes and on Phala, R1 on Novita — it was one run of a
reasoning model going either way, not the fields taking effect.

| model | routes | outcome |
|---|---|---|
| `anthropic/claude-haiku-4.5` | Anthropic, Amazon Bedrock, Google Vertex, Azure | **continues** — `" Paris."`, 4/4 routes |
| `google/gemini-2.5-flash` | Google | **continues** — `" Paris."` |
| `anthropic/claude-sonnet-4.6` | — | **`400`** *"This model does not support assistant message prefill"*, passed through |
| `openai/gpt-4.1-mini` | OpenAI | **restarts** — `"The capital of France is Paris."` |
| `google/gemma-4-31b-it` | DeepInfra, CoreWeave, Venice, Chutes, Friendli, Novita, Parasail, SambaNova, Together, ModelRun, SiliconFlow | **restarts on all 11** that answered (Crusoe: `429`) |
| `qwen/qwen3.6-27b` | Chutes, SiliconFlow, Phala, DeepInfra, Venice, Alibaba | **restarts** where it answered at all, after 1–9k characters of reasoning; DeepInfra and one SiliconFlow arm spent the whole 4096-token cap reasoning and returned no text; one Chutes arm restated the partial before answering, and one Phala arm continued (`"Paris, though the Zorbville atlas is fictional."`) after 8.5k characters of reasoning |
| `x-ai/grok-4.5` | xAI | restates the partial and completes it **wrongly** — `London.`, `Berlin. e1f2a` |
| `deepseek/deepseek-r1` | Novita | a new sentence (`"Paris is the capital of France."`), or the answer followed by leaked reasoning |
| `mistralai/mistral-small-3.2-24b-instruct` | Venice | continues with the **seam lost** — `"Paris."`, no space |

At the probe's original 256-token cap every Qwen route but Alibaba returned
**empty text**:
`enable_thinking: false` does not reach the route, so a "resume" reopens the
reasoning and spends the cap on it — the same starvation lessons §3 records,
arriving through a field the gateway drops.

**What the app does with a restart [code].** `EchoFilter`
([generation.rs:207](../../src/app/orchestrator/generation.rs)) withholds bytes
only while they match the seed; a restart diverges at the first byte and flows
straight into the same message. The stored reply becomes

```
Per the Zorbville atlas, the capital of France isThe capital of France is Paris.
```

— same id, same bubble, no note, no error, and it replays into every later turn.
On Grok's shape the filter does exactly what it was built to do, strips the
restated partial, and appends ` London.` — which reads as a continuation and is
wrong.

## 2. Why the existing gates cannot see it [code]

- **`ServerMode::supports_continuation`**
  ([config.rs:186](../../src/shared/config.rs)) answers `true` for `External`,
  because `external` meant llama.cpp when the table was written (spec §6.4,
  continue-generation §2). Through a gateway the answer is a property of the
  route, and the mode cannot know it.
- **`wire_content`** ([wire.rs:169](../../src/shared/api/openai/wire.rs)) puts a
  tool's images on the `role:"tool"` message as content parts — measured good on
  llama.cpp and on xAI directly, and outside the OpenAI spec's letter, which
  gives a tool message text parts only.
- **The positive signal already exists.** A catalogue that answered for the
  configured model — `OpenAiClient::catalogue_entry`, landed in the orchestrator
  as `context.caps` — is evidence the endpoint is a gateway-shaped catalogue. A
  llama.cpp `/v1/models` carries neither key (gateway-capabilities §5.2, N3), so
  a change keyed on that answer leaves every local stack exactly as it is.

## 3. Decided, with the reason

- **Silence keeps today's behaviour.** No catalogue, a blank model field, a
  failed fetch: the request is byte-identical to what ships. Only a positive
  answer narrows or reshapes anything — the invariant the last track rests on.
- **No routing knobs.** The review kept OpenRouter's `provider` controls out of
  `external` (openrouter-external §6), and a fix that works only on a pinned
  route is not a fix for the default one.
- **No resend after a refusal.** A retry that re-homes the images after a `4xx`
  would cover the loud failures and never the silent ones, and a mid-turn resend
  after a hard error is what mcp-tool-images F1 decided against — the fallback is
  chosen per provider, not per failure. F2's recovery differs in kind: there the
  refused request had reached nothing, and the second one differed by one field.

## 4. Forks

### H1. A tool's images through a gateway — **recommendation (b)**

- (a) **note only** — install.md §3 and spec §9.10 say a tool's images depend on
  the route. Honest, and it leaves 3 pairs in 29 describing a picture they never
  saw, which is exactly the failure `tools.python_images` and the MCP note were
  built to prevent — except that here the app believes the image was sent, so it
  cannot say otherwise. Default routing happened to choose a seeing route 20/20
  for Gemma, which makes the failure rare and does nothing to make it visible.
- **(b) re-home a tool's images when the catalogue answered.** The tool message
  goes out text-only and its images follow in one `user` message after the
  round's last tool result, in call order, each behind the label it already
  carries — the file name the tool's own result names (*decided at
  implementation*: a label naming the call would need the profile's language,
  which the wire layer does not have, and it would add nothing the file name does
  not already tie together — the same call Gemini's F1-A fallback made).
  **Recommended**: 28 of 28 answering pairs saw the image in that shape,
  including all three silent routes and all six refusals, with every repeated
  pair's blind arm well below its seeing one; it is the fallback Gemini's builder has taken since
  mcp-tool-images F1-A, so the attribution cost — a `user`-role image — is one the
  project has already accepted and labels the same way; and a local llama.cpp,
  which publishes no catalogue, keeps its measured-good shape byte for byte.
  One `user` message per round rather than one per result, because the OpenAI
  wire requires a round's tool messages to follow its `tool_calls` contiguously.
- (c) **re-home on every `external`** — rejected: it changes the llama.cpp path
  that is measured good, and its prefix cache with it (wire.rs keeps a text-only
  message a bare string for that reason), to fix routes it never reaches.
- (d) **retry re-homed after a refusal** — rejected in §3: it catches the six and
  misses the three.

**H1.1 — where the switch lives.**

- **(i) in `OpenAiClient`, keyed on its own catalogue memo** — the client already
  owns `catalogue_entry` and remembers the reasoning refusal the same way; no
  contract field, and the decision sits beside the wire it shapes.
  **Recommended.** The build must await the same memo the background question
  fills rather than race it, so the first turn after a restart is shaped like
  the second.
- (ii) a `ChatRequest` field the orchestrator sets from `context.caps` — the
  decision becomes visible in request tests, and every other backend gains a
  field it must ignore.

**Found while preparing H1 [code]: there is no memo to await.**
`OpenAiClient::catalogue_entry` ([client.rs](../../src/shared/api/openai/client.rs))
issues a fresh `GET /v1/models` on every call — harmless while its only caller
was the once-per-engine background question, and a request per turn the moment
the chat path reads it. So H1.1 (i) needs the memo it was written as if it had:
keep the answer of a fetch that **succeeded** — an entry, or a list without the
model — for the client's lifetime (a new engine gets a new client), and keep
**no** answer from a transport failure, so a gateway that was briefly unreachable
is asked again rather than filed as "no catalogue" and sent the tool shape it
cannot see. The background question and the request builder then read one value.

### H2. `/continue` through a gateway — **recommendation (b)**

- (a) **refuse on a gateway** — whenever the catalogue answered, `/continue`
  refuses with a note naming `/regen`. The simplest correct thing, and it takes
  continuation away from the two families measured continuing through the
  gateway on every route (Anthropic ≤ 4.5, Gemini).
- **(b) refuse on a gateway except where the direct mode already continues.**
  Read the vendor off the slug and ask the table that already exists: an
  `anthropic/…` model through `anthropic_model_continues` (so `claude-haiku-4.5`
  continues and `claude-sonnet-4.6` refuses — which is its measured `400`, turned
  into a note before the request instead of an error after it), a
  `google/gemini-…` model continues, everything else refuses. **Recommended**:
  the allowlist is the one spec §6.4 already maintains; what it would allow is
  measured continuing through the gateway (`claude-haiku-4.5` on four routes,
  `gemini-2.5-flash`), what it would refuse among Anthropic's models is the
  measured `400`, and a vendor the table does not know is refused rather than
  guessed at. Mistral's seamless `"Paris."` on one route is
  not enough to allow a family on.
- (c) keep allowing, and detect a restart from the stream — rejected: a restart
  with no invented marker in the partial is the same bytes as a continuation that
  begins a sentence, and R1's `"Paris is the capital of France."` is exactly that
  case.
- (d) note only — rejected: the failure is a corrupted stored message, not a
  wrong expectation.

In every option the refusal note (`ui.cmd.continue_unsupported`) names the
gateway case and `/regen`, and an `external` endpoint with no catalogue keeps
continuing as today.

**H2.1 — when the catalogue is asked (decided at implementation, on the
recommendation).** The discovery was lazy: the first turn after a (re)connect
kicked the question off. That is fine for a compaction window, which is not needed
until a conversation is long, and wrong for this gate — `/continue` as the first
command after a restart is the command's main case (a reply broke off, the app
was closed, it is opened again), and it would meet an unanswered question and fall
back to the behaviour a gateway does not have. So the question is now asked when
the engine is applied and on a readiness flip, the same rule the model's name
already follows (`refresh_model_name`). The alternative — refusing "still asking,
try again" — would have changed a local `external` user's first `/continue` too.

### H3. Scope

- (i) both halves in one PR — one client, one plan, one journal entry;
- **(ii) H2 first, H1 second** — H2 is a gate and a note and stops a stored
  message being corrupted; H1 reshapes a request on the wire and deserves its own
  review. The two may share a branch.

## 5. What a live run has to show

Nothing above is built yet; this is the gate each stage owes before its PR.

- **H2.** `/continue` through the gateway **refuses** on `google/gemma-4-31b-it`
  with the note, and **continues** on `anthropic/claude-haiku-4.5`
  (`continue_probe`'s word-boundary arm through `live_client`, with
  `MINDFORK_ENGINE_MODEL` naming the slug); and the regression half — a local
  `llama-server` still continues, the N3 shape of the previous track.
- **H1.** `tool_result_image_is_seen_live` stays green on default routing; the
  body the client now builds for a catalogue endpoint is **serialized by the
  client and replayed** pinned to one route of each failure kind — Chutes on
  Gemma (silent), DeepInfra (`422`), ModelRun (`400`) — each now seeing, with its
  blind arm; and a local `llama-server` request is byte-identical to today's.
  Replaying the client's own bytes, rather than a probe's reconstruction of them,
  is what keeps the pinned arm measuring the shipped shape (lessons §3).
- **Named smokes only** — install.md §7.1; the whole `--ignored` set is not run
  against a metered gateway.

## 7. Stage H2 — what was implemented

- **`ServerMode::supports_continuation(model, catalogued)`**
  ([config.rs](../../src/shared/config.rs)) — `External` with a catalogue answers
  through `gateway_model_continues`: the slug's vendor against the spec §6.4
  table (`anthropic/…` through the existing version allowlist, `google/gemini-…`),
  a `:variant` suffix dropped first so `4.6:batch` cannot read as 4.0, and every
  other vendor, alias or router refused. Without a catalogue the arm is the one
  that shipped.
- **One answer per turn.** `Orchestrator::continuation_supported` is read by the
  command's gate and snapshotted into the turn (`GenSpawn` → `TurnShared` →
  `SharedParts`), where `Finished.continuable` and the mid-stream interruption
  note read it — so no note can offer `/continue` where the command would refuse.
- **The gateway's own note**, `ui.cmd.continue_unsupported_gateway` (en, ru): the
  generic one says external engines continue, which is exactly what is untrue here.
- **The catalogue is asked when the engine is applied** (H2.1) —
  `refresh_engine_facts` replaces the bare invalidation at both sites (settings
  applied, readiness flipped).

**Tests** — four unit tests: the route table (including the `:batch` trap, an
alias, a router, a vendorless id, and the silence case); the gate on a bare
orchestrator (the gateway note for a restarting family, `true` again once the
catalogue is forgotten, a turn started for Claude ≤ 4.5); the whole route on a
running orchestrator, where the catalogue must land **before any turn**, a
length-cut reply is announced as not continuable, and the command refuses with the
gateway note; and the readiness flip's own re-ask (`handle_chat_status`, extracted
from the loop's arm so the order it keeps can be tested where it lives). Eight
mutations, all caught. One `#[ignore]` smoke, `continue_through_a_gateway_live`, declared by
`MINDFORK_LIVE_CONTINUE_EXPECT`.

**Live — GO on the three stacks §5 names** (2026-09-15): through OpenRouter,
`google/gemma-4-31b-it` is announced not continuable and refused with the gateway
note, and `anthropic/claude-haiku-4.5` resumes `"The capital of France"` with
`" is Paris."`; on the LAN `llama-server` (Gemma 4 31B, no catalogue) the reply
continues exactly as before. In every run the engine's facts landed before the
first turn. The gate on the wire is unchanged for a local server by construction,
and this is the run that shows it.

## 8. Stage H1 — what was implemented

- **`wire::rehome_tool_images`** ([wire.rs](../../src/shared/api/openai/wire.rs)) — a
  pure function over the conversation: every tool result text-only, the images of
  a run of tool results appended to one `user` message after the run, in call
  order, behind their existing labels; `None` when no tool result carries an
  image, so the caller sends the request it has rather than a copy. The request
  builder itself is untouched, so every body it produced before is still the body
  it produces.
- **`OpenAiClient::shaped_for_endpoint`**
  ([client.rs](../../src/shared/api/openai/client.rs)) — the first thing
  `chat_stream` does: a request with no tool image goes out as it came, without
  so much as a catalogue lookup; one with a tool image is re-homed only when the
  catalogue answered for the model (`model_capabilities` is `Some`), which is the
  same positive signal H2 reads.
- **The catalogue memo** (H1.1) — `catalogue_entry` behind a `tokio::sync::OnceCell`
  filled by a fetch that *answered*; a transport failure, `5xx` or `429` leaves it
  empty and the next question asks again.

**Tests** — two wire tests (a round of two tool results and a later round, each
run getting one user message, the tool results bare strings on the wire, labels
and order kept; a conversation with only a user's image left alone) and two client
tests (the memo, including "not now" not being remembered; and on the wire — a
gateway gets the re-homed body, a llama.cpp-shaped catalogue gets exactly the body
`build_chat_request` always produced, and a turn with no tool image makes no
catalogue request). One `#[ignore]` smoke,
`a_re_homed_tool_image_is_seen_on_the_routes_that_failed_live`, which serializes the
request with the client's own builder and adds nothing but the route pin.

**Live** — pinned to the three routes that failed today's shape on
`google/gemma-4-31b-it`, the builder's re-homed body is seen on each: Chutes
(silently blind before), DeepInfra (`422` before), ModelRun (`400` before), every
blind arm describing some other picture; on `qwen/qwen3.6-27b` the same on Venice
(silently blind before) and DeepInfra (`422` before). Through the client itself, on
the gateway's default routing, `tool_result_image_is_seen_live` stays green on
`google/gemma-4-31b-it`, `qwen/qwen3.6-27b`, `openai/gpt-4.1-mini` and
`anthropic/claude-haiku-4.5`, each against its blind arm; and on the LAN
`llama-server` (Gemma 4 31B with its projector, no catalogue) the smoke is green on
the unchanged shape — the regression half, 2026-09-15.

## 9. F5 — measured beside H2, closed with nothing to build

`reasoning_details` across tool rounds, the review's third item. Its precondition
was a thinking Anthropic-family model through the gateway with signed blocks, and
the account has them. Instrument: a raw non-streaming replay (so the echoed array
is the gateway's own, not a reassembly), a `get_weather` round with reasoning
forced by `reasoning.max_tokens`, then the second request in several shapes.

| second request | through the gateway | Anthropic's own API |
|---|---|---|
| **no reasoning blocks** — what `OpenAiClient` sends | `200`, a normal answer — haiku-4.5 on default, Anthropic, Amazon Bedrock and Google Vertex; sonnet-4.6 on its default route | `200` |
| the exact blocks echoed | `200`, indistinguishable | `200` |
| the text changed, signature intact | `200` | `200` |
| the signature replaced | **`400`** *"Invalid `signature` in `thinking` block"* | **`400`**, the same words |

So the blocks are forwarded and their signatures checked, and the rejection the
reports describe is real — but it is reachable only by **sending** blocks, and the
wire that sends none cannot hit it. What an exact echo would buy is continuity of
reasoning, and this instrument shows none to buy: the second round reasoned zero
tokens in every arm, the echo included. Building it would add the one failure mode
the current wire is immune to, for a gain no run has shown. **F5 is closed as
measured.**

**Found beside it, and not F5's to fix.** The app's own thinking switch sends
`thinking: true`, and **alone that turns reasoning on at no route** — the
gateway drops the field; only with `reasoning_effort` set did any route reason
(81 reasoning tokens on the same prompt). On a gateway the settings' thinking
toggle is therefore inert unless an effort is chosen too — the same class of
defect as `repeat_penalty` under its other name. A separate task — taken up, and
measured in both directions, in [gateway-thinking-switch.md](gateway-thinking-switch.md).
