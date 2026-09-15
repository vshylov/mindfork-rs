# Track plan: the thinking switch reaches a gateway

**Status:** forks **decided** by the user, 2026-09-15 — all three at the
recommendation; **implemented** (§6) and **L1–L4 measured GO** (§7).

- **T1 — (a):** "on" with no effort sends `reasoning: {enabled: true}`.
- **T2 — (ii):** "off" too, behind both guards — the catalogue's explicit
  `mandatory: false`, and the F2 recovery widened to a refused `enabled: false`.
- **T3 — (a):** a request with an effort set goes out as today.

The settings' thinking switch goes out as a top-level `thinking` — a llama.cpp
field — and a gateway drops it unread. Found beside F5
([gateway-images-and-continue.md](gateway-images-and-continue.md) §9) on the
Anthropic half: `thinking: true` alone turned reasoning on at no route. Measured
here on both directions and on three kinds of model, it is wider than that finding:
the switch is inert **on** for a model that does not reason by default, and inert
**off** for one that does.

**Related:** spec §8.1 (the mapping onto the OpenAI-compatible API),
[gateway-capabilities.md](gateway-capabilities.md) (the catalogue, and why its
`reasoning` entry already stands for our `thinking` + `reasoning_effort`),
[openrouter-external.md](research/openrouter-external.md) §5 F2 and §9 (the
refusal a model that must reason answers with, and the memo that recovers from it),
[lessons.md](lessons.md) §3 (a `200` is not proof a parameter works), §9 (gateways).

## 1. What is broken, precisely

`wire::build_chat_request` ([wire.rs](../src/shared/api/openai/wire.rs)) sends
`SamplingConfig.thinking` as `thinking` and `reasoning_effort` only when an effort
is chosen. Through OpenRouter:

- **on, no effort** — `thinking: true` is dropped, so a model whose reasoning is off
  by default (Claude Haiku 4.5, Sonnet 4.6, Gemma 4) answers without reasoning. The
  settings screen offers the toggle (the catalogue lists `reasoning`, which
  `catalogue_aliases` maps onto `thinking`), the user sets it, and nothing happens —
  the `repeat_penalty` / `repetition_penalty` defect in another field;
- **off, no effort** — `thinking: false` is dropped too, so a model whose reasoning
  is **on** by default (Qwen 3.6, DeepSeek R1) reasons anyway;
- **an effort set** works today: `reasoning_effort` is OpenRouter's legacy spelling
  and it converts it.

The silent turns (title, compaction roll, impersonation) are not affected: they send
`reasoning_effort: "none"`, which the gateway reads.

## 2. What was measured — 2026-09-15

Instrument: one raw streamed request per shape, the body `build_chat_request` builds
for a tool turn (`stream_options.include_usage`, one tool, `tool_choice: "auto"`,
`max_tokens: 2048`) with only the reasoning fields varied; read
`usage.completion_tokens_details.reasoning_tokens` and `delta.reasoning_details`.
Default routing unless pinned. One sample per cell: the claim read off it is **zero
against non-zero**, not the size of a non-zero count.

| shape | haiku-4.5 (Bedrock) | qwen3.6-27b (Chutes) | deepseek-r1 (Novita) |
|---|---|---|---|
| S0 no reasoning field | 0 | 134 | — |
| S1 `thinking: true` — **today's toggle on** | **0** | 105 | 62 |
| S2 `thinking: true` + `reasoning: {enabled: true}` | **66**, signed — pinned: Bedrock 75, Anthropic 80, Vertex 81 | 207 | 197 |
| S3 `thinking: true` + `reasoning_effort: "medium"` — today with an effort | 101, signed | 907 | — |
| S4 S3 + `reasoning: {enabled: true}` — both spellings | 58, `200` | 189, `200` | — |
| S5 `thinking: true` + `reasoning: {effort: "medium"}` | 75 | 136 | — |
| S6 the silent turns' shape (`reasoning_effort: "none"`, budget 0, kwargs) | 0 | 0 | `400` *mandatory* |
| S7 `thinking: false` — **today's toggle off** | 0 | **100** | 83 |
| S8 `thinking: false` + `reasoning: {enabled: false}` | 0 | **0** | **`400` *mandatory*** |
| S9 `thinking: "banana"` | `200`, 0 | `200`, 148 | — |
| S10 `reasoning: {enabled: "banana"}` | `400` *expected boolean* | `400` *expected boolean* | — |
| S11 `reasoning: {enabled: true}` + `reasoning_effort: "none"` | 0 | 0 | `400` *mandatory* |

What it says:

- **S9/S10 are the lessons §3 probe**: a wrong type in `thinking` is a `200` — the
  field is never read — while a wrong type in `reasoning.enabled` is a `400`. The
  gateway's switch is `reasoning`; ours is not on its schema.
- **S1 → S2** is the fix for "on": zero to non-zero on every Anthropic route.
- **S7 → S8** is the fix for "off" on a default-on model — and **S8 on R1** is the
  trap: the endpoint that must reason refuses `enabled: false` with the very message
  the F2 memo recognises, but the memo only fires for a turn that sent
  `reasoning_effort: "none"`. Sent on an ordinary chat turn, it would be a `400` the
  client does not recover from.
- **S4** is accepted on both, so a request carrying both spellings is not refused.
  Which one wins is not resolvable from one sample (qwen S3 907 against S4 189).
- **S11**: `"none"` beats `enabled: true`, so a mapping cannot un-mute a silent turn.

**The catalogue says more than `supported_parameters`.** Every entry that carries
reasoning metadata carries a `reasoning` object — 314 of 446 models; always
`mandatory`, often `default_enabled`, `default_effort`, `supported_efforts`:

```
anthropic/claude-haiku-4.5   "reasoning": {"mandatory": false}
qwen/qwen3.6-27b             "reasoning": {"mandatory": false, "default_enabled": true}
deepseek/deepseek-r1         "reasoning": {"mandatory": true}
```

103 of the 314 are `mandatory: true`. OpenRouter documents `enabled: true` alone as
"medium" effort, and `effort: "none"` as refused where reasoning is mandatory
**[docs]**.

## 3. Decided, with the reason

- **Silence is never a claim.** The field is added only when the endpoint's catalogue
  entry for the configured model lists `reasoning` in `supported_parameters` — the
  same reading `catalogue_aliases` already gives that word. No catalogue (every
  llama.cpp), a list without `reasoning`, a blank model field: the body is
  byte-identical to today's, pinned by a test in the shape of
  `a_gateway_receives_a_tools_image_in_a_user_message`.
- **In the client, beside `shaped_for_endpoint`.** `OpenAiClient` owns this wire and
  already holds the memoised catalogue entry; the orchestrator, the sampling entity
  and the settings screen do not change. The offer was already right — it is the wire
  that made it a lie.
- **Only a request with no effort changes.** A chosen `reasoning_effort` works today
  (S3); leaving it alone keeps every request that already works exactly as it is, and
  keeps S4's undetermined precedence out of the product.
- **The silent turns do not change.** They send an effort (`"none"`), so the rule
  above already excludes them; S6 and S11 show the gateway honours that.
- **A zero `reasoning_budget` is "off".** Found reading the call sites: the
  orchestrator mutes some turns with the budget alone — the empty-reply re-ask
  (`generation.rs`), the director's checkpoints, `fetch_url`'s page summary — and
  those carry the user's `thinking: true` with no effort. Read as the switch, they
  would have sent `enabled: true` on exactly the turns meant not to reason; read as
  the mute they are (the Responses and Anthropic wires already do), they now reach
  a gateway as "off" too.
- **The shipped default is "on", so the fix is felt by default.** `AppConfig`'s
  `default_sampling` carries `thinking: Some(true)`; after this, a gateway model
  that reasons only when asked (Haiku 4.5, Sonnet 4.6, Gemma 4) reasons on an
  untouched configuration, as it already does through the direct Anthropic
  provider. That is what the setting has always claimed, and it spends tokens the
  gateway never spent before — stated in the CHANGELOG rather than left to be
  discovered on a bill.
- **What asks the catalogue.** Today only a turn with a tool image consults it. Now a
  turn with the switch set and no effort does too — once per client, memoised. A
  local server with a blank model field asks nothing (`fetch_catalogue_entry`
  returns before a request); a named local model costs one `GET /v1/models` per
  engine, whose answer lists ids only, so the body stays byte-identical.
- **`thinking` is still sent.** Removing it would change what a llama.cpp behind
  the same code path receives, for nothing: the gateway never reads it (S9).
- **Not in scope:** pre-empting the silent turns' refusal from `mandatory: true`. The
  F2 memo already turns that into one extra round trip per session; reading the flag
  would save that trip and is a separate, smaller change.

## 4. Forks

### T1. How "on" reaches the gateway — **recommendation (a)**

- **(a) add `reasoning: {enabled: true}`** when the switch is on, no effort is set and
  the catalogue lists `reasoning`. The documented switch, and the one S10 shows the
  gateway validates; lets the gateway apply its own default depth.
- (b) map "on" to a default `reasoning_effort` (`"medium"`). Works (S3), but invents a
  level the user did not pick, stored nowhere and shown nowhere, through the legacy
  spelling.
- (c) narrow the offer instead — keep the toggle with a hint that an effort is
  required. Documents the defect rather than fixing it; the toggle would still do
  nothing when used alone.

### T2. The "off" half — **recommendation (ii)**

- (i) **"on" only**; "off" recorded as measured and left for a follow-up.
- **(ii) "off" too**: `reasoning: {enabled: false}` when the switch is off, no effort is
  set, the catalogue lists `reasoning` **and** its `reasoning` object says
  `mandatory: false` explicitly (an absent flag is silence). Two guards, because S8 on
  R1 is an ordinary chat turn failing: the flag keeps the field off an endpoint that
  says it must reason, and the F2 recovery is widened to recognise the same refusal of
  a request that carried the mapped `enabled: false` — re-send without it, remember
  it on the same `reasoning_off_refused` memo — for a catalogue that is wrong or
  stale. Same seam, same file, measured; the user's "off" is as inert today as "on".

### T3. A request with an effort set — **recommendation (a)**

- **(a) unchanged**: the flat `reasoning_effort` goes out as today (S3 works).
- (b) translate it into `reasoning: {effort}` too, so a gateway only ever sees one
  spelling. Cleaner on the wire, but it changes requests that work today, on
  evidence (S4 vs S3 on qwen) that one sample cannot interpret.

## 5. What a live run has to show

Named smokes only — install.md §7.1.

| | what it proves |
|---|---|
| **L1** | through the client, `thinking: true` and no effort on `anthropic/claude-haiku-4.5` returns `reasoning_tokens > 0` — 0 before, so the smoke must fail without the field (checked by a live mutation that withholds it) |
| **L2** (T2 ii) | `thinking: false` and no effort on `qwen/qwen3.6-27b` returns `reasoning_tokens == 0` — today 100 |
| **L3** (T2 ii) | `thinking: false` and no effort on `deepseek/deepseek-r1` **completes** — the flag keeps the field off; the recovery is covered by a unit test with a scripted refusal |
| **L4** | a local `llama-server` turn with the switch on is unchanged — the body pinned by unit test, one thinking smoke on the LAN stack |

## 6. What was implemented

One behaviour, in the client that owns this wire:

- **`wire::gateway_reasoning(sampling, entry, off_refused)`**
  ([wire.rs](../src/shared/api/openai/wire.rs)) — the whole rule as a pure
  function: `None` unless the entry lists `reasoning` and no effort is chosen; a
  zero `reasoning_budget` is "off" whatever `thinking` says; "on" needs nothing
  more; "off" needs `mandatory: false` stated and no refusal remembered.
  `ChatCompletionRequest` gains `reasoning: Option<WireReasoning>`, which
  `build_chat_request` never sets — so every existing builder test, and every body
  that does not pass through the client's catalogue step, is untouched by
  construction.
- **`ModelEntry`** keeps the catalogue's `reasoning` key as raw JSON behind two
  readers, `lists_parameter` and `reasoning_mandatory`. Raw, because the same list
  carries the window and the parameter list: a gateway spelling this one key as a
  boolean must not fail the parse the two shipped features depend on.
- **`OpenAiClient::reasoning_switch`** consults the memoised catalogue only for a
  turn that could gain the field (a switch set or a zero budget, and no effort),
  and `send_chat` sets the field on the body it built.
- **The F2 recovery is widened by one clause**: `should_stop_asking` now counts a
  sent `enabled: false` as asking to mute, beside `reasoning_effort: "none"`. The
  retry recomputes the switch under the memo, which drops an "off" and keeps an
  "on" — one memo, `reasoning_off_refused`, silences both spellings of "off".

## 7. What the live run measured — 2026-09-15

Through the client, named smokes only (install.md §7.1):

| | smoke | model | result |
|---|---|---|---|
| **L1** | `a_gateway_reasons_when_the_switch_is_on` | `anthropic/claude-haiku-4.5` | **GO** — 53 reasoning tokens, finish `Stop` (0 before) |
| L1, mutated | the same, with `gateway_reasoning`'s "on" returning `None` | the same | **red**, as it must be — `reasoning_tokens=0`, "was asked to reason and reported no reasoning tokens" |
| **L2** | `a_gateway_stops_reasoning_when_the_switch_is_off` | `qwen/qwen3.6-27b` | **GO** — 0 reasoning tokens (100 before) |
| **L3** | `the_switch_off_completes_on_an_endpoint_that_must_reason` | `deepseek/deepseek-r1` | **GO** — completes, no `400`; 239 reasoning tokens, as a model that must reason spends |
| **L4** | `a_gateway_streams_thoughts_under_its_own_field_name`, model named | local CPU `llama-server`, `gemma-4-12b-it-qat-q4_0` | **GO** — thoughts through `reasoning_content`, finish `Stop`, **one** task in the server log; the catalogue answered with ids alone, so the body was the one it always was |

L2's reply is worth a line: asked which city hosted the Olympics 32 years before
2024, Qwen 3.6 with reasoning genuinely off answered *Atlanta, 1996*. Nothing is
wrong with the smoke — it asserts the reasoning count, not the arithmetic — and it
is the switch doing what the user asked of it.

L4 ran on the CPU build because the LAN stack was unreachable; the claim it carries
is the wire's, not the model's, so a 12B model is enough.
