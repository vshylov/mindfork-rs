# Research: reasoning-effort tiers a model does not have

**Status:** forks decided by the user on 2026-10-01 — all four at the
recommendation; both stages implemented (§8).

**Related:** spec §8.1 (the mapping onto each provider's API), architecture §6,
[openrouter-mode.md](openrouter-mode.md) §4.1 (the gateway's muted turn),
[grok-xai-provider.md](grok-xai-provider.md) §2.3 (xAI and `"none"`),
[docs/journal/engine.md](../journal/engine.md) — the entry *a muted turn on an
OpenAI model that has no "none"*, which this continues.

## 1. What was asked, and what the code does

The report that started it: on `gpt-6.1-sol` the settings still offer `none` and
`minimal`, which that model refuses, and do not offer `max`, which it takes.

`ReasoningEffort` (`entities/sampling.rs`) is one scale for every provider —
`none`, `minimal`, `low`, `medium`, `high`, `xhigh` — chosen once in the settings
(`default_sampling`, `impersonation_sampling`), per chat by `set_sampling`, and
translated per wire **[code]**:

| wire | what a chosen effort becomes |
|---|---|
| llama.cpp, `external` | the string as it is, in `reasoning_effort` |
| OpenAI Responses | the string as it is, in `reasoning.effort` |
| OpenRouter | the string as it is, in `reasoning_effort` |
| xAI | the string as it is; `none` left out (`with_effort_none_omitted`) |
| Anthropic | `minimal`→`low`, `xhigh`→`high`; sent only with thinking on |
| Gemini 3.x | `thinkingLevel`: `xhigh`→`high`; `minimal`→`low` when the name contains `pro` |
| Gemini 2.5 | a `thinkingBudget` in tokens |

So the scale already means "the nearest the provider has" on two wires — the
enum's own comment says the extreme tiers *map onto* low/medium/high during
translation — and "exactly this string, or the provider's `400`" on the others.

## 2. Measured (2026-10-01, the real APIs, one tiny request per cell)

**OpenAI Responses** — ten models, five different sets, and every refusal names
the set (`Supported values are: 'low', 'medium', …`):

| model | `none` | `minimal` | `low`–`high` | `xhigh` | `max` |
|---|---|---|---|---|---|
| `gpt-5` | 400 | yes | yes | 400 | 400 |
| `gpt-5.2`, `gpt-5.4`, `gpt-5.5` | yes | 400 | yes | yes | 400 |
| `gpt-5.6-sol`, `gpt-6-luna`, `gpt-6-sol` | yes | 400 | yes | yes | yes |
| `gpt-6-astra`, `gpt-6.1-sol` | 400 | 400 | yes | yes | yes |
| `o4-mini` | 400 | 400 | yes | 400 | 400 |

(`gpt-5`'s "yes" is its refusals' list, not a `200`: the key's organisation is
not verified for that model, so the values it accepts answered `404`.)

**Anthropic** (`output_config.effort`, adaptive thinking). The vocabulary is
`low`, `medium`, `high`, `xhigh`, `max` — `none` and `minimal` are schema errors.
`claude-opus-4-8`, `claude-sonnet-5-5`, `claude-opus-5-5` take all five;
`claude-sonnet-4-6` refuses `xhigh` and names what it has: *"Supported levels:
high, low, max, medium."* So `max` exists on every model tried, `xhigh` on the
newer ones, and the app sends neither: both ride as `high`.

**xAI** (`reasoning_effort`). `grok-4.3` takes `none`…`xhigh`; `grok-4.5`,
`4.6`, `4.7` take `minimal`…`xhigh` and refuse `none` (known, left out today).
All four refuse `max` — *"Invalid reasoning effort."*, with no list.

**Gemini 3.x** (`thinkingLevel`). The vocabulary is `minimal`, `low`, `medium`,
`high`; `xhigh` and `max` are schema errors. `minimal` is refused — *"Thinking
level MINIMAL is not supported for this model"* — by `gemini-3.1-pro-preview`,
**`gemini-3.7-flash`** and **`gemini-3.8-flash`**; `gemini-3-flash-preview`,
`3.1-flash-lite`, `3.5-flash`, `3.5-flash-lite`, `3.6-flash` take it.

**OpenRouter.** The catalogue's `supported_efforts` is listed by 192 of 462
models; `max` by 87 of them, `xhigh` by 95, `minimal` by 37, `none` by 60, in 26
different combinations. A chosen depth the model does **not** list is a `200`
every time — `minimal` on `openai/gpt-6.1-sol`, `max` on `openai/gpt-5.5`, on
`x-ai/grok-4.7` and on `google/gemini-3.8-flash`, `xhigh` on
`anthropic/claude-sonnet-4.6`: the gateway maps it itself. Only `none` is refused
(*"Reasoning is mandatory…"*), which the mode already answers.

**llama.cpp** (source, `tools/server/server-common.cpp`). `none` switches
thinking off; any other string goes to the chat template as `reasoning_effort`
unchecked, and templates exist that read `max`.

## 3. What is broken, precisely

1. **`max` cannot be chosen.** It is a real tier on OpenAI (five of the ten
   models), on Anthropic (every model tried) and in 87 gateway entries.
2. **Anthropic's `xhigh` is sent as `high`.** True when the mapping was written;
   since Opus 4.7 the API has the value.
3. **A depth the model lacks fails every ordinary turn on Responses.** `minimal`
   is refused by nine of the ten models, `xhigh` by two. The setting is global, so
   a value chosen for one model breaks the next one — the same report, one
   provider over.
4. **Muted turns fail on `gemini-3.7-flash` and `gemini-3.8-flash`** — the defect
   just fixed for OpenAI, on the Gemini wire. A muted turn asks 3.x for
   `thinkingLevel: "minimal"`, and the only models spared are those whose name
   contains `pro` (`is_gemini_3_pro`). The two newest Flash models refuse it too,
   so on them no chat gets a title, and the roll, impersonation, a page summary
   and a director's checkpoint fail with it **[code + measured on the API; not
   yet reproduced through the app]**. A name heuristic aged exactly the way the
   comment on `ResponsesClient::vision` says it would.

## 4. Forks

### E1. A depth the user chose and the model does not have — **(a), decided**

- **(a) The nearest the model has — recommended.** The request is made once
  more with the nearest value the refusal lists, and the answer is remembered
  for that client, so a session pays one refusal per value. It is what the
  Anthropic and Gemini wires already do (statically), what the gateway does
  itself (§2), and what the scale's own comment promises. "Nearest": among the
  depths the model lists, the closest on the ladder, the lower one on a tie —
  never more reasoning than was asked for when less is as near. `none` is a
  switch, not a depth: where it is refused it becomes the lowest depth (the
  rule merged in PR #663), and a depth never becomes `none`.
- **(b) The provider's refusal, as today.** The message does name the values;
  the user changes the setting. Keeps PR #663's sentence — *an effort the user
  chose is reported as it came* — and leaves the Anthropic and Gemini wires
  doing the opposite.
- **(c) Offer only what the model takes.** Not available as a rule: neither
  OpenAI nor Anthropic publishes the set anywhere but in the refusal, so the
  list could narrow only after a failed turn, and a stored value outside it
  would still need (a) or (b).

### E2. Saying that a depth was replaced — **(a), decided**

- **(a) The settings' description says so, and the log names each
  replacement — recommended.** One sentence under the field ("a value the
  model does not have becomes the nearest it has"), one `info` line per learned
  replacement. The Anthropic and Gemini replacements have been silent since
  they were written.
- **(b) A notice in the chat, once per model** — "`gpt-6.1-sol` has no
  `minimal`; `low` is used" — on the pattern of the no-vision notice. A new
  event from the clients to the feed, for a fact most users will not act on.
- **(c) The settings row shows the pair once it is known** — `minimal → low`.
  Needs the engine's facts pushed to the settings screen, and is empty until
  the first refusal.

### E3. Anthropic's top tiers — **(a), decided**

- **(a) Send what the API has — recommended:** `xhigh` as `xhigh`, `max` as
  `max`; a model without one (`claude-sonnet-4-6` has no `xhigh`) answers under
  E1. A chat set to `xhigh` on Opus 4.7+ starts reasoning deeper than it has
  been, at that tier's price.
- **(b) Keep `xhigh` → `high`,** and map `max` onto `high` beside it.

### E4. The Gemini defect (§3.4) — **(a), decided**

- **(a) Part of this track, first — recommended.** It is a live defect, it is
  the same seam, and E1's mechanism is its fix: a refused `minimal` is asked
  again as `low` and remembered, with the `pro` heuristic kept as what is known
  ahead of the refusal.
- **(b) A task of its own, later.**

## 5. Proposed shape (under the recommendations)

- **One rule, one place** — `shared/api/effort.rs`: the ladder
  (`minimal < low < medium < high < xhigh < max`), reading a refusal's list
  (quoted, as OpenAI writes it, and bare, as Anthropic does), and "nearest".
  `GATEWAY_EFFORTS` and `responses::wire::lowest_listed_effort` move onto it.
- **Each client keeps what it learned** — the wish and what was accepted in
  its place, set only once the second request is accepted (PR #663's rule),
  for the client's lifetime. The asking again is written once
  (`send_asking_again`), behind a trait a client implements (`EffortWire`).
- **Where a refusal lists nothing, the answer is the measured one:** xAI's
  `max` goes out as `xhigh`; Gemini's refused `minimal` as `low`.
- **`ReasoningEffort::Max`** — the settings' cycle, `set_sampling`'s enum, the
  field's description in both locales; a stored config needs no migration (a
  new variant of a string enum).

## 6. Stages

1. **`fix/effort-refused`** — E1 on the wires where a value can be refused
   **today**, E2, and with them §3.3 and §3.4: Responses (any value) and Gemini
   (a level). Anthropic is sent only `low`/`medium`/`high` until stage 2, and
   xAI takes every depth the scale has until `max` joins it, so neither has
   anything to refuse yet. No new tier; nothing in the settings but the
   description.
2. **`feat/effort-max`** — `ReasoningEffort::Max`, E3, and the two refusals
   they bring: Anthropic's per-model list (`claude-sonnet-4-6` has no `xhigh`)
   through `EffortWire`, and xAI's `max` sent as `xhigh`.

## 7. What a live run can show

Each arm declared rather than guessed, as in `responses::live_tests`:

- Responses: `minimal` on `gpt-6.1-sol` completes and learns `low`; `max` on
  `gpt-5.5` learns `xhigh`; a value the model has is sent once.
- Gemini: a muted turn on `gemini-3.8-flash` completes and learns `low`; the
  same turn on `gemini-3.5-flash` is sent once, as `minimal`.
- Anthropic: `xhigh` on `claude-sonnet-4-6` completes and learns `high`; on
  `claude-opus-5-5` it is sent once.
- xAI: `max` on `grok-4.7` completes.
- The control for §3.4 before the fix: the same Gemini smoke, red.

## 8. What was implemented

**Stage 1** (`fix/effort-refused`, 2026-10-01).

- `shared/api/effort.rs` — `LADDER`, `NONE`, `listed`, `nearest`, `EffortMemo`,
  `CarriesEffort`, `EffortWire`, `send_asking_again`. The list is read from the
  sentence after the **last** "supported values/levels" only: the refused value
  is quoted earlier in the same message, and PR #663's scan of the whole text
  would have listed it once the refused value could be a depth.
- Responses — `RespRequest` carries its effort (`CarriesEffort`);
  `ResponsesClient::answer` is the nearest listed value, a refused `none` with
  nothing listed going out with no effort. PR #663's `muted_effort`,
  `refuses_effort_none`, `respell_effort_none` and `lowest_listed_effort` are
  gone into it.
- Gemini — `GenRequest` carries its `thinkingLevel`; a refused level is asked
  again as `level_above`. `is_gemini_3_pro` stays as what is known ahead of the
  refusal.
- The settings' description of the field, both locales (E2).

**Live — GO** (the real APIs, Windows 11):

| smoke | model | outcome |
|---|---|---|
| a muted turn, before the change | `gemini-3.8-flash` | the `400` — the control, red |
| a muted turn | `gemini-3.8-flash` | two turns `Stop` with a title; learned `minimal → low` |
| a muted turn | `gemini-3.5-flash` | sent once, as `minimal`; nothing learned |
| a muted turn | `gpt-6.1-sol` | two turns, 0 reasoning tokens; learned `none → low` |
| a muted turn | `gpt-6-sol` | sent once, as `none`; nothing learned |
| a turn at `minimal` | `gpt-6.1-sol` | two turns `Stop`; learned `minimal → low` |

**Stage 2** (`feat/effort-max`, 2026-10-01).

- `ReasoningEffort::Max` — the settings' cycle and menu, `set_sampling`'s enum,
  the field's description in both locales. The enum derives `Ord`: the variants'
  order is the scale's. A stored config needs no migration.
- Anthropic (E3) — `xhigh` and `max` are sent as they are; `AntRequest` carries
  its effort, and `AnthropicClient` answers a refused level with the nearest its
  refusal lists. Measured after the decision, on the rest of the models: the 4.5
  generation has no adaptive thinking at all; `claude-opus-4-6` and
  `claude-sonnet-4-6` have `max` and no `xhigh`; `claude-opus-4-7` and everything
  newer take all five.
- xAI — `OpenAiClient::with_effort_capped_at(XHigh)`, set where the Grok client
  is built. **Sub-fork, decided at the recommendation:** configured rather than
  learned. The refusal lists nothing, all four models measured agree, and the
  client's request path already carries one learned recovery; the control smoke
  below is what says when the ceiling can go.
- Gemini — `max` rides as `high`, beside `xhigh`.

**Live — GO** (the real APIs, Windows 11):

| smoke | model | outcome |
|---|---|---|
| a thinking turn at `xhigh` | `claude-sonnet-4-6` | two turns `Stop`; learned `xhigh → high` |
| thinking turns at `xhigh` and `max` | `claude-sonnet-5-5` | each sent once, as it is |
| a turn at `max` | `gpt-5.5` | `Stop`; learned `max → xhigh` |
| a turn at `max`, under the ceiling | `grok-4.7` | `Stop` |
| the same turn, no ceiling | `grok-4.7` | `400 "Invalid reasoning effort."` — the control |

Stage 1's five smokes, run again on this branch: unchanged.

**Found beside it, not in this track:**

- The video client (`shared/video/gemini.rs`) muted thinking with the same name
  heuristic, on its own request path. **Measured and fixed** in its own task
  the same day: `youtube_watch` with `gemini-3.7-flash` or `gemini-3.8-flash`
  as the video model was the same `400`, and the client now asks once more at
  the level above ([docs/journal/tools.md](../journal/tools.md)).
- `claude-haiku-4-5` answers every request that carries `thinking: {type:
  "adaptive"}` with *"adaptive thinking is not supported on this model"*
  **[measured on the API; not reproduced through the app]** — the thinking
  switch on that model, whatever the effort.
