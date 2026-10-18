# DeepSeek's own API: the cache figure, and a `thinking` it refuses

*Research, 2026-10-18. The last leftover of the cache share
([cached-tokens.md](cached-tokens.md) §9.3, F8). Forks: §5.*

## 1. Why

The cache share left one item on demand: DeepSeek's own pair of keys,
`prompt_cache_hit_tokens` and `prompt_cache_miss_tokens`. Its API is the only
place that speaks it: the gateway normalises it away, and a DeepSeek GGUF on
llama.cpp speaks llama.cpp's fields (cached-tokens.md §9.1). The owner opened an
account and set `DEEPSEEK_API_KEY` on 2026-10-18, which is F8 (c) there: parse it,
and run it live.

The probe ran first, before any parser was written (cached-tokens.md §7). It
answered the question it was sent with, and the smoke after it found a larger
one: in `external` mode, DeepSeek's API refuses every request the app makes.

## 2. What was measured, 2026-10-18

### 2.1 The models

`GET https://api.deepseek.com/models` lists two:

| id | name | window | output cap | input | efforts listed (default) |
|---|---|---|---|---|---|
| `deepseek-flash` | DeepSeek-V4.1-Flash | 1048576 | 393216 | text, image | `low`, `high`, `max` (`high`) |
| `deepseek-v4-pro` | DeepSeek-V4-Pro | 1048576 | 393216 | text | `low`, `high`, `max` (`high`) |

The window is `context_window`, the input `input_modalities` at the entry's top
level, the efforts `effort.supported_levels`. None of these is a key the
catalogue reads (`ModelEntry` knows OpenRouter's `context_length` and
`architecture.input_modalities`, and xAI's `capabilities`).

### 2.2 The cache figure

`cache_probe_external_live`, with `MINDFORK_ENGINE_URL=https://api.deepseek.com/v1`:
the client's own body, about 9000 tokens, sent three times in a row.

| Model | Run | `prompt_tokens` | `prompt_cache_hit_tokens` | `prompt_cache_miss_tokens` | `prompt_tokens_details.cached_tokens` |
|---|---|---|---|---|---|
| `deepseek-flash` | 1 | 8988 | 0 | 8988 | 0 |
| | 2 | 8988 | **8832** | 156 | **8832** |
| | 3 | 8988 | 8832 | 156 | 8832 |
| `deepseek-v4-pro` | 1 | 9040 | 0 | 9040 | 0 |
| | 2 | 9040 | **8960** | 80 | **8960** |
| | 3 | 9040 | 8960 | 80 | 8960 |

- **DeepSeek sends both spellings**, equal on all six requests: its own pair, and
  the OpenAI-compatible `prompt_tokens_details.cached_tokens`.
- `prompt_tokens` is the whole prompt (8988 = 8832 + 156), as on OpenAI.
- A miss is a reported `0`, not an absent key.
- There is no write figure.
- The hits are whole multiples of 64 tokens (138 and 140 of them).

So `Usage::cached_tokens()` already reads DeepSeek's figure as it stands. The
dialect needs no parser.

### 2.3 The smoke: every turn refused

`cached_tokens_e2e_live` through `external` mode at the same URL failed on its
first turn, on both models:

```
422 Unprocessable Entity: Failed to deserialize the JSON body into the target
type: thinking: invalid type: boolean `false`, expected struct ThinkingOptions
```

`auto_title_first_reply_e2e_live` failed the same way on its first ordinary turn,
with `boolean \`true\``: the settings' default.

### 2.4 What DeepSeek takes, field by field

One request per row on `deepseek-flash` ("Say: ok"), each with one field added to
the baseline body:

| Field | Status | Effect |
|---|---|---|
| (baseline) | 200 | reasons (10 tokens) |
| `thinking: true` / `false` | **422** | the refusal above |
| `thinking: {"type": "disabled"}` | 200 | **no reasoning** |
| `thinking: {"type": "enabled"}` | 200 | reasons |
| `reasoning_effort: "none"` | 200 | **no reasoning** |
| `reasoning_effort`: `minimal`, `low`, `medium`, `high`, `xhigh`, `max` | 200 each | accepted, listed or not (one sample each, so no depth is read from it) |
| `reasoning_budget: 0` | 200 | ignored: reasons |
| `chat_template_kwargs: {"enable_thinking": false}` | 200 | ignored: reasons |
| `top_k`, `min_p`, `repeat_penalty`, `dry_multiplier`, `samplers` | 200 | ignored |
| `continue_final_message`, `add_generation_prompt` | 200 | ignored |

Only one field is refused, and only for its type. Every other field llama.cpp
reads is ignored.

### 2.5 A tool round

One tool call and its result, on both models, with thinking on and the reasoning
**not** passed back (the app keeps none between rounds): round two answers on both
(`deepseek-v4-pro` reasoned 69 characters before the call). Passing it back
changes nothing. Unlike Anthropic's signed thinking (spec §8.1), there is nothing
to return.

## 3. What the code does today

- `build_chat_request` sends the settings' switch as `thinking: <bool>` whenever
  it is set (spec §8: one of the llama.cpp fields). It is set on every request:
  - `default_sampling.thinking` is `Some(true)` out of the box;
  - the muted turns set `Some(false)`: the title (`title.rs`), the compaction roll
    (`compaction.rs`) and impersonation (`impersonation.rs`, and its own
    `impersonation_sampling`).
- So in `external` mode at DeepSeek's API **no request goes through**: ordinary
  turns are refused for `true`, muted turns for `false`.
- Nothing answers the refusal. It is a `422`, which the retry decorator does not
  retry (`EngineError::is_transient`). The two refusals the client does answer are
  `400`s about something else: reasoning that cannot be switched off
  (`should_stop_asking`) and xAI's refused `reasoning_effort`
  (`refuses_effort_parameter`).
- The `openrouter` mode leaves `thinking` out of the body (`for_gateway`). xAI
  takes the body as it is and ignores the field, as its smokes show.
- Once a request goes through, the cache share needs nothing (§2.2).
- **The window is unknown.** DeepSeek publishes `context_window`, which the
  catalogue does not read. As on any server that cannot say, the automatic
  compaction trigger stays inactive, and `compaction.context_tokens` sets the
  window by hand (spec §6.7). Reading it is not one alias: an entry that carries a
  window makes the catalogue count as **answered**
  (`model_capabilities` → `endpoint_catalogued`). That is the signal that
  switches on the gateway's behaviours: a tool's images re-homed into a user
  message, and `/continue` read by the catalogue
  ([gateway-images-and-continue.md](../history/gateway-images-and-continue.md) §2).

## 4. Design (under F1 (a), F2 (a), F3 (a))

- **The wire's `thinking` gets a second spelling.** `WireThinking` is
  `Switch(bool)`, llama.cpp's, or `Typed { type: "enabled" | "disabled" }`,
  DeepSeek's. `build_chat_request` writes the switch as it does today.
- **The client learns DeepSeek's spelling from its refusal.** This is
  `OpenAiClient::thinking_typed`, a memo of the kind `reasoning_off_refused`
  already is. The rule:
  - a `400` or `422` whose message names `thinking` and a boolean is answered by
    sending the same request once more, with the switch as `{type}`;
  - the memo is set only when that second request is **not** refused the same way,
    so a misread refusal cannot spoil the client for its lifetime;
  - from then on, every request is sent typed from the start;
  - the change is logged once, at info.
- **Where:** in `send_chat`, beneath the effort's own asking-again. The
  reasoning-off recovery in `chat_stream` then reads the outcome of the request as
  it was finally sent.
- **The switch keeps its meaning.** `true` is `enabled` and `false` is `disabled`.
  A muted turn is then muted twice over, since `reasoning_effort: "none"` (which
  DeepSeek honours, §2.4) travels beside it.
- **The cache figure:** a unit fixture of DeepSeek's usage, both spellings, read
  as `Some(8832)` with no write. The leftover moves out of the roadmap.

## 5. Forks

*User's decision, 2026-10-18: F1 (a), F2 (a), F3 (a), each the recommendation.*

**F1. How the client learns DeepSeek's spelling of `thinking`.**
- **(a) From the refusal**, as §4. The client needs no knowledge of the URL, so a
  proxy in front of DeepSeek is covered. The cost is one refused request (a `422`,
  not billed) per engine applied. *Recommended*: it is the shape the client
  already uses for refusals it can answer.
- (b) From the host: `api.deepseek.com` in `external` mode writes `{type}` from
  the first request. No refused request, but that host only.
- (c) A `deepseek` mode of its own, beside `grok`: the key in settings, the picker
  from `/models`, the window, image input and efforts read from its list. It adds
  a stored `ServerMode` value, so a `settings.json` schema step, and it is a track
  of its own. Not now: (a) is what that track would build on anyway.

**F2. DeepSeek's own cache keys.**
- **(a) Read nothing new.** DeepSeek sends `prompt_tokens_details.cached_tokens`
  beside its pair, equal on 6 of 6 requests. A fixture pins it. *Recommended*.
- (b) Also read `prompt_cache_hit_tokens` where `prompt_tokens_details` is
  absent. No server measured sends that shape.

**F3. DeepSeek's window.**
- **(a) Leave it unknown here.** `compaction.context_tokens` sets it by hand, and
  reading `context_window` waits until the catalogue's "answered" signal is split
  from the gateway's behaviours (§3). *Recommended*: at 1 048 576 tokens, the
  automatic trigger is the least of what an unknown window costs.
- (b) Read `context_window` in this PR, as a window that does not make the
  catalogue count as answered.

## 6. Not in this track

- **Efforts.** DeepSeek lists `low`, `high` and `max`, but accepted every other
  value with a `200` (§2.4). Nothing fails, and how it maps `medium` or `xhigh`
  was not measured.
- **Image input** on `deepseek-flash`. Not measured.
- **DeepSeek's other surfaces:** its Anthropic-compatible endpoint, the beta
  prefix completion (which `/continue` could use), and FIM.

## 7. The live run, as planned

- **A new client-level smoke** (`shared/api/openai/deepseek_live_tests.rs`,
  `MINDFORK_DEEPSEEK_KEY`): a muted turn and a turn with thinking on both
  complete; the first learns the memo, the second goes typed from its first
  request.
- **`cached_tokens_e2e_live` against DeepSeek, both models:** turn two reads the
  cache, and the control reads under 5% of its prompt.
- **The orchestrator's own muted turns** through `external` mode at DeepSeek:
  `auto_title_first_reply_e2e_live`, `compaction_preserves_a_planted_fact_e2e_live`,
  `impersonation_after_compaction_still_writes_live` and a tool round
  (`followup_tool_e2e_live`). Red before the change (§2.3).

## 8. What was built, and the live run (2026-10-18)

Built as §4:
- `wire::WireThinking` with its two spellings;
- `ChatCompletionRequest::with_typed_thinking` and `wire::refuses_boolean_thinking`;
- the `thinking_typed` memo, answered in `OpenAiClient::send_typed_thinking`;
- a DeepSeek fixture in the cache-figure test.

Unit tests:
- the respelling, its idempotence, and the body around it unchanged;
- the refusal's matcher under `400`, `422`, `500` and `503`, and three messages
  that must not match;
- the recovery and the memo against a scripted server: three bodies — `false`,
  `{"type": "disabled"}`, then the next turn's `{"type": "enabled"}` from its
  first request;
- four arms that must not learn: a server refusing both spellings, a `422` about
  another field, DeepSeek's words under a `503`, and a turn with no switch.

Each guard was mutated in turn, and each mutant turned its test red. That covers
learning from any refusal, never reading the memo, and any status.

The live run, on both models (every arm **GO**; before the change, the first two
rows and the orchestrator's smokes failed on their first request):

| Smoke | `deepseek-flash` | `deepseek-v4-pro` |
|---|---|---|
| `a_muted_turn_and_a_thinking_turn_complete_on_deepseek` | muted: 0 reasoning, learned; on: 22 reasoning | muted: 0 reasoning, learned; on: 37 reasoning |
| `the_first_turn_of_a_new_engine_completes_on_deepseek` | 17 reasoning, learned | 15 reasoning, learned |
| `cached_tokens_e2e_live` | turn 1 `0%`, turn 2 **8704 of 8956 (97%)**, control `0%` | turn 1 `0%`, turn 2 **8832 of 8958 (98%)**, control `0%` |
| `auto_title_first_reply_e2e_live` | a five-word Russian title on why the sky is blue | a four-word one on the same |
| `followup_tool_e2e_live` | the follow-up in its own bubble | the same |
| `compaction_preserves_a_planted_fact_e2e_live` | `ZARYA-8823` after the roll | the same |
| `impersonation_after_compaction_still_writes_live` | written, `Stop` | written, `Stop` |

A llama.cpp is never refused, so its body is unchanged. The serialisation test
asserts the boolean as before (`json["thinking"] == true`).
