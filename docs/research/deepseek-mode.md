# Research: a `deepseek` mode of its own

*Research, 2026-10-19. The roadmap's DeepSeek leftover, F1 (c) of
[deepseek-api.md](deepseek-api.md) §5. **User's decision, 2026-10-19: F1 (a),
F2 (a), F3 (a)** (§5), each the recommendation — the mode, in two stages, with
`/continue` refused in `external` at DeepSeek's host.*

**Related:** [deepseek-api.md](deepseek-api.md) (the thinking switch, the cache
figure, and the field-by-field table this builds on),
[grok-xai-provider.md](grok-xai-provider.md) (the last direct cloud added, and
the wrong-type method), [openrouter-mode.md](openrouter-mode.md) (the last mode
added, and its schema stamp), [effort-tiers.md](effort-tiers.md) §9 (xAI's list
of efforts), [continue-generation.md](continue-generation.md),
[ADR 0004](../decisions/0004-engine-contract-multi-provider.md),
[ADR 0006](../decisions/0006-data-schema-versioning.md), spec §3.4, §6.4, §8,
§8.1, §9.10, §12.2.

Claims are tagged **[live]** (measured on the day above, key in
`DEEPSEEK_API_KEY`), **[code]** (read in this repository) and **[docs]**
(DeepSeek's own guide, `api-docs.deepseek.com/guides/thinking_mode`, unmeasured
unless a [live] line says otherwise).

## 1. Why

DeepSeek's API has worked in `external` mode since 0.20.0, and the manual names
that setup (manual §2). Measuring it once more for this question found two
defects in that setup, besides what a mode of its own would add:

1. **`/continue` restarts the answer.** **[live]** On `/v1` DeepSeek ignores a
   trailing assistant message: given `"1, 2, 3, 4,"` to continue, it answered
   `"1, 2, …, 10"` whole, with thinking on and off. **[code]** `external` claims
   `/continue` wherever the catalogue has not answered
   (`ServerMode::supports_continuation`), and DeepSeek's entry carries none of
   the keys that would answer it, so the restart is appended to the cut. The
   echo filter (`EchoFilter`) hides it only when the restart repeats the cut
   byte for byte — which a counting fixture does, so `continue_e2e_live` passes
   there while prose doubles.
2. **`deepseek-v4-pro` drops an image in silence.** **[live]** A request with an
   image is a `200`; asked whether it sees one, the model says `NONE` (2 of 2),
   and asked its colour it says "Brown" of a red square. `deepseek-flash` sees it
   (`SEEN red`, 2 of 2), in a user message and in a tool result alike. **[code]**
   `external` reads `/props`, then `architecture.input_modalities`; DeepSeek
   publishes `input_modalities` at the entry's top level, so the answer is
   `Unknown`, the image is sent, and the note the app has for a model that takes
   none never comes.

What a mode adds besides: the window (`context_window`, 1 048 576, unread
today), a sampling offer that matches what DeepSeek reads, the thinking switch
in DeepSeek's spelling from the first request rather than after a `422`, a row
in the mode selector, and — stage 2 — a `/continue` that works.

Two reasons that made the `grok` mode no longer hold: `external` keeps a stored
key (docs/history/external-api-key.md), and the `422` is answered (0.20.0).

## 2. What was measured, 2026-10-19

### 2.1 The list

`GET /models` **[live]**: the two models of deepseek-api.md §2.1, unchanged —
`deepseek-flash` (`DeepSeek-V4.1-Flash`, `input_modalities: [text, image]`) and
`deepseek-v4-pro` (`DeepSeek-V4-Pro`, `[text]`), both `context_window: 1048576`,
`max_output_tokens: 393216`, `effort: {supported_levels: [low, high, max],
default_level: high}`, under `data`. `POST /v1/embeddings` is a `404`: there is
no embedding API. `GET /user/balance` is free.

### 2.2 The request schema

DeepSeek deserializes strictly, so a field sent with a value of the wrong type
is a `422` when its schema reads the field and a `200` when it drops it
(the method of grok-xai-provider.md §2.5). **[live]**, `deepseek-flash`:

| Read (`422` on a wrong type) | Dropped (`200`) |
|---|---|
| `temperature`, `top_p`, `presence_penalty`, `frequency_penalty`, `seed`, `stop`, `logprobs`, `top_logprobs`, `response_format`, `n`, `tool_choice`, `parallel_tool_calls`, `stream_options`, `reasoning_effort`, `thinking` | `top_k`, `min_p`, `repeat_penalty`, `repetition_penalty`, `logit_bias`, `user`, `max_completion_tokens`, `prompt_cache_key`, `verbosity`, `service_tier`, `metadata` |

The ranges, from the refusals: `temperature` [0, 2], `top_p` (0, 1],
`presence_penalty` [-2, 2]; `n` is 1 only.

What reaches the reply **[live]**, thinking off: `temperature: 0` gives the same
first ten words twice; `seed: 7` twice gives the same first three words, then
diverges — honoured at best in part, never reproducible. **[docs]** In thinking
mode `temperature` and the penalties "will not trigger an error but will also
have no effect"; `top_p` applies on `deepseek-flash` only in thinking mode
(0.95–1.0), on `deepseek-v4-pro` only outside it.

### 2.3 Efforts

**[docs]** DeepSeek maps every value of the scale itself: `minimal`→`low`,
`low`→`low`, `medium`→`high`, `high`→`high`, `xhigh`→`high`, `max`→`max`.
`none` is not a value it documents for this wire; the switch is `thinking:
{type: disabled}`. **[live]** (deepseek-api.md §2.4) every value of the scale is
a `200`, and `none` gives a reply with no reasoning. Reasoning lengths on one
prompt, two samples per value, were indistinguishable — the prompt was too easy
to read a depth from, so the table above is the docs' and not measured.

### 2.4 Continuation

**[live]** `https://api.deepseek.com/beta` with `"prefix": true` on the trailing
assistant message continues it: `"1, 2, 3, 4,"` → `" 5, 6, 7, 8, 9, 10"`, thinking
on and off. No echo of the prefix. This is stage 2.

### 2.5 Reasoning across tool rounds

**[docs]** With `tools`, earlier `reasoning_content` "must be fully passed back
… If your code does not correctly pass back `reasoning_content`, the API will
return a 400 error." **[live]**, both models: a tool call, its result and the
next user turn, all without the reasoning, are `200` each, as in
deepseek-api.md §2.5. The rule is not enforced today. The app keeps no reasoning
between rounds; whether passing it back changes an answer is a lead (§6), not a
defect.

## 3. What the code does in `external` today

**[code]**, with DeepSeek's URL in `external`:

- the client is `OpenAiClient::new(url)` with the key and the model; the first
  request of each engine is refused and sent again with `{type}` (spec §8);
- `/props`, Ollama's `/api/ps`, `/api/version` and LM Studio's `/api/v1/models`
  are asked, and each is a `404`;
- the catalogue entry is found in `GET /models` and yields nothing:
  `context_length`, `supported_parameters` and `architecture` are absent, so
  `model_capabilities` is `None`, the window is unknown and the endpoint does
  not count as catalogued;
- the sampling offer is llama.cpp's whole set (`supported_sampling_fields(None)`);
- `supports_continuation(External, false)` is `true` (§1, defect 1);
- `vision()` is `Unknown` (§1, defect 2).

## 4. Design

### 4.1 Stage 1 — the mode

- **`ServerMode::DeepSeek`, `ImpersonationMode::DeepSeek`,
  `CloudProvider::DeepSeek`**, wire `"deepseek"`, display `DeepSeek`, base
  `https://api.deepseek.com/v1`. A `deepseek` section under the engine, the
  impersonation engine and the embedder, as every cloud has. In `ALL` after
  `grok` and before `openrouter`: the directs together, the gateway last.
- **The client: `OpenAiClient::for_deepseek()`**, beside `for_xai`:
  - the thinking switch is `{type}` from the first request;
  - a turn that asks for reasoning to be off sends `thinking: {type:
    disabled}` and no `reasoning_effort` — the switch DeepSeek documents,
    rather than a `none` it accepts without naming;
  - every other effort goes as it came: DeepSeek publishes its own mapping of
    the scale (§2.3), and a client-side nearest-tier rule would disagree with
    it on `medium` (the house rule takes the lower on a tie: `low`; DeepSeek
    reads `high`);
  - the facts come from the entry in `GET /models`, read in DeepSeek's words:
    `context_window` as the window, the top-level `input_modalities` as
    whether the model takes images. Read only on this client, so `external`
    reads exactly what it reads today (F3 (a) of deepseek-api.md stands);
  - `/props`, Ollama's and LM Studio's routes are never asked.
- **The window and the catalogue.** With a window read, the endpoint counts as
  catalogued, which switches on one gateway behaviour on this client: a tool's
  images are re-homed into a user message. **[live]** `deepseek-flash` reads
  them both ways (§1), so this changes nothing a user sees.
- **Images:** `deepseek-v4-pro` answers `Unsupported`, so an image is withheld
  and said so (spec §9.10); `deepseek-flash` answers `Supported`.
- **Sampling:** `temperature`, `top_p`, `presence_penalty`,
  `frequency_penalty`, `max_tokens`, `seed`, `thinking`, `reasoning_effort` —
  what the schema reads and the app has a field for (§2.2). The spec says what
  thinking mode leaves without effect.
- **No embeddings:** the embedder slot in this mode is unavailable with a
  warning, as on `claude` and `grok`.
- **`/continue`** is refused in this mode (stage 2 builds it).
- **`external` at `api.deepseek.com`:** `/continue` is refused by the host
  (F2 (a)). The server answers a prefill with a `200`, so there is no refusal to
  learn from, and nothing in the catalogue says it. A proxy in front of DeepSeek
  is not covered; the note says to use the `deepseek` mode.
- **The schema.** A new value of a stored enum is a breaking change
  (lessons §8). `settings.json` 6 → 7 is a stamp, as `settings_to_v4` was for
  `openrouter`. The chat files hold the mode too, in every reply's metadata, so
  they take a stamp as well, 5 → 6: the reasoning `chat_to_v5` gives for `max` —
  a chat file copied into an older root without its settings is skipped in
  silence otherwise — holds word for word for `deepseek` (D1).

### 4.2 Stage 2 — `/continue` through the beta prefix

- A continuation request on this client goes to `{origin}/beta/chat/completions`
  with `"prefix": true` on the trailing assistant message; every other request
  stays on `/v1`.
- `supports_continuation(DeepSeek)` becomes `true`; the `external` refusal of
  stage 1 stays, and its note names the mode.
- A live smoke cuts a reply **in prose**, where a restart cannot pass for an
  echo, and checks that the stored text holds the cut once.

## 5. Forks

*User's decision, 2026-10-19: F1 (a), F2 (a), F3 (a), each the recommendation.*

**F1. Scope.**
- **(a) A mode of its own, in two stages** (§4). *Recommended.*
- (b) Mend `external` only, reading DeepSeek's keys in the shared catalogue.
  That is the split deepseek-api.md F3 waits for: an entry with a window makes
  the endpoint catalogued, which switches the gateway's behaviours on for every
  `external` server whose list says as much.
- (c) Nothing; point users at OpenRouter, whose DeepSeek slugs are already
  honest about the window, images and `/continue`.

**F2. `/continue` in `external` at `api.deepseek.com`.**
- **(a) Refused by the host, in stage 1.** *Recommended*: the setup is in the
  manual, and a `200` gives nothing else to go on.
- (b) Left as it is, with a line in the manual pointing at the mode.

**F3. `/continue` in the mode.**
- **(a) Refused in stage 1, built on `/beta` in stage 2.** *Recommended.*
- (b) `/beta` in stage 1.
- (c) Refused for good — `/beta` is a beta.

**Decided in the design, under the go-ahead** (named here so a reviewer can
overturn them):

- **D1 — the chat files are stamped too** (§4.1), following `chat_to_v5`; the
  ledger gains `ServerMode` in the chat files, with `openrouter` at 4 — the
  schema the chats had in 0.13.0, the first release that wrote it.
- **D2 — efforts are not snapped** (§2.3, §4.1).
- **D3 — a muted turn is `thinking: disabled` with no effort** (§4.1).
- **D4 — `seed` is offered** although it is not reproducible (§2.2): the schema
  reads it, and `grok` offers it on the same footing.
- **D5 — the `external` refusal is by host name**, `api.deepseek.com` exactly,
  case aside.

## 6. Not in this track

- **Passing `reasoning_content` back across tool rounds** (§2.5) — the docs' rule,
  unenforced today; a quality lead for every wire that streams reasoning.
- **The balance** (`GET /user/balance`) as a key check before `Ready`, the way
  the `openrouter` mode checks its key.
- **DeepSeek's Anthropic-compatible endpoint and FIM.**
- **`external`'s reading of `context_window`** — deepseek-api.md F3 stands.

## 7. The live run, as planned

Stage 1, both models, `MINDFORK_DEEPSEEK_KEY`:

- a client-level smoke in the mode: a muted turn sends no effort and gets no
  reasoning; a turn with thinking on reasons; neither is refused first (no `422`
  in the log);
- the window and the images: `model_capabilities` reads 1 048 576, `vision()`
  is `Supported` on flash and `Unsupported` on pro;
- through the supervisor, in the `deepseek` mode: `cached_tokens_e2e_live`,
  `auto_title_first_reply_e2e_live`, `followup_tool_e2e_live`,
  `compaction_preserves_a_planted_fact_e2e_live`,
  `impersonation_after_compaction_still_writes_live` — the smokes
  deepseek-api.md §8 ran in `external`;
- `external` at DeepSeek: `/continue` is refused, with the note.

## 8. What was built, and the live run (2026-10-19) — stage 1

Built as §4.1:

- `ServerMode::DeepSeek`, `ImpersonationMode::DeepSeek`, `CloudProvider::DeepSeek`
  (wire `deepseek`), a `deepseek` section in the three slots, the mode after
  `grok` in the selector;
- `OpenAiClient::for_deepseek`: `thinking_typed` set at construction, a muted turn
  as `thinking: disabled` with no effort (`asks_reasoning_off`), no `/props`,
  `/api/ps` or LM Studio route; `ModelEntry::deepseek_window` and
  `takes_images_top_level`, read on this client only;
- `CatalogueShape::DeepSeek` for the picker (role `Chat`, the name, the window);
- `supported_sampling_fields(DeepSeek)`; the embedder unavailable;
- `EngineSettings::supports_continuation` with `is_deepseek_host`, and the note
  `ui.cmd.continue_unsupported_deepseek`;
- `settings_to_v7` and `chat_to_v6`, stamps, with the golden
  `settings_v6.json` (the `external`-at-DeepSeek setup) and
  `chat_v5_external_deepseek.json`; the ledger's `ServerMode in Chats`;
- for the smokes, `MINDFORK_ENGINE_DIALECT=deepseek` (the mode's client on the
  `external` path) and `cached_tokens_deepseek_e2e_live` (the mode on the
  production supervisor).

Unit tests (4093 green): the client over a scripted server — every way a turn is
muted is the switch off and no effort, a reasoning turn keeps `medium`/`xhigh`/
`max` as chosen, the window and the images from the list with nothing else asked,
and `external` on the same list reading nothing new; the host rule (four spellings
of the host refuse, seven look-alikes and blanks do not); the catalogue shape;
the supervisor's `Ready` and the unavailable embedder; the two stamps, their
fixtures, the run over a 0.20.0 root, and a file in the new mode as a newer
version to 0.20.0's readers.

The live run, both models (every arm **GO**):

| Smoke | `deepseek-flash` | `deepseek-v4-pro` |
|---|---|---|
| `the_mode_mutes_and_reasons_on_deepseek` | muted 0 reasoning; on 24 | muted 0; on 35 |
| `the_mode_reads_the_window_and_the_images_on_deepseek` | 1 048 576, `Supported` | 1 048 576, `Unsupported` |
| `the_deepseek_catalogue_e2e_live` | 2 entries, both `Chat`, named | — |
| `cached_tokens_deepseek_e2e_live` (the mode, production supervisor) | `0%` → **97%** → `0%` | `0%` → **98%** → `0%` |
| `auto_title_first_reply_e2e_live` | a three-word Russian title on why the sky is blue | the same, worded otherwise |
| `followup_tool_e2e_live` | the follow-up in its own bubble | the same |
| `compaction_preserves_a_planted_fact_e2e_live` | `ZARYA-8823` after the roll | the same |
| `impersonation_after_compaction_still_writes_live` | written | written |
| `a_gateways_catalogue_decides_whether_images_are_sent_live` | attached, no caveat | refused with the way out; a `python_exec` chart withheld, and the reply says it cannot view it |

The external-path smokes ran on the mode's client (`MINDFORK_ENGINE_DIALECT`);
`deepseek_live_tests`' two `external` arms stay green beside them.
