# Research: reasoning-effort tiers a model does not have

**Status:** forks decided by the user on 2026-10-01 — all four at the
recommendation; both stages implemented (§8). The item §8 left open —
xAI's published list — is closed in §9.

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
  **Corrected the same day** (the Claude 4.5 task, §8): true of OpenAI, false of
  Anthropic and of xAI — both publish it per model
  (`capabilities.effort` and `capabilities.thinking.types`;
  `capabilities.reasoning_effort`). Their model lists had been fetched for the ids
  alone. The decision stands on other grounds — a refused request is free
  and only the models that need it meet it, while asking ahead costs every
  session a request — and a narrowed list in the settings, for those two
  providers, is possible after all.

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
- The 4.5 generation (`claude-haiku-4-5`, `claude-sonnet-4-5`,
  `claude-opus-4-5`) answers every request that carries `thinking: {type:
  "adaptive"}` with *"adaptive thinking is not supported on this model"* —
  and the thinking switch is on by default, so every ordinary turn failed
  there. **Reproduced and fixed** in its own task the same day: the request is
  asked once more with a thinking budget
  ([docs/journal/engine.md](../journal/engine.md)).
- xAI publishes `capabilities.reasoning_effort` per model, and the client's
  two configured answers for it were not read from there — `grok-4.3` lists
  `none` and was never sent it. **Measured and fixed** in its own task the same
  day, and larger than it looked: §9.
- **`max` is a stored value, and this stage added it without a schema step.**
  The effort is written into `settings.json`, a profile's defaults and every
  chat file — beside each reply made under it — and a strict enum's new value
  is a breaking change of the files that hold it (spec §12.2). Nothing in this
  document asked what a version without the value does with such a file.
  **Found while preparing the release, measured on the 0.13.0 binary and
  fixed** before any release carried the value: 0.13.0 refuses the settings
  and leaves a chat holding `max` out of its list in silence; `settings.json`
  and the chat files are now stamped 4 → 5
  ([docs/journal/storage.md](../journal/storage.md)).

## 9. xAI: the list is read, not configured

**Status:** implemented 2026-10-01 (`fix/xai-published-efforts`). No new fork
went to the user: every decision below is E1 applied to a provider whose list is
published, and the sub-forks are recorded with what decided them.

### 9.1 Measured (2026-10-01, the real API)

**What is published, and where.** `capabilities` rides on a model's entry in all
of `GET /v1/models`, `/v1/models/{name}`, `/v1/language-models` and
`/v1/language-models/{name}`:

| model | `capabilities.reasoning_effort` | `default_reasoning_effort` |
|---|---|---|
| `grok-4.3` | `none`, `low`, `medium`, `high`, `xhigh` | `low` |
| `grok-4.5`, `grok-4.6`, `grok-4.7` | `low`, `medium`, `high`, `xhigh` | `high` |
| `grok-4.20-0309-reasoning`, `grok-4.20-0309-non-reasoning`, `grok-build-0.1` | no `capabilities` key at all | — |

**What each model does with each value** (one request per cell):

| model | the field left out | `none` | `minimal` | `low`–`xhigh` | `max` |
|---|---|---|---|---|---|
| `grok-4.3` | reasons, at its default | 200, **no reasoning tokens** | 200 | 200 | 400 |
| `grok-4.5`, `4.6`, `4.7` | reasons, at its default | 400 | 200 | 200 | 400 |
| the three with no list | 200 | 400 | 400 | 400 | 400 |

The three with no list refuse the **parameter**, in the same words for every
value: *"Model grok-build-0.1 does not support parameter reasoningEffort."* The
fourth entry without one, `grok-4.20-multi-agent-0309`, refuses Chat Completions
altogether, with or without the field.

**What a muted turn costs** — the title's kind of prompt, four runs a cell;
reasoning tokens and seconds:

| model | the field left out (before) | what the list says to send (after) |
|---|---|---|
| `grok-4.3` | 300–437 tokens, 2.9–4.4 s | `none`: 0 tokens, 0.7–0.8 s |
| `grok-4.5` | 119–154 tokens, 2.7–3.2 s | `low`: 37–55 tokens, 1.2–1.5 s |
| `grok-4.6` (one run) | 486 tokens, 8.5 s | `low`: 203 tokens, 4.4 s |
| `grok-4.7` | 85–151 tokens, 1.9–2.7 s | `low`: 84–102 tokens, 1.8–2.1 s |

**`minimal` is taken and not listed**, by all four. On `grok-4.5` it reasons
exactly as `low` does (37 tokens, four runs of four), on `grok-4.3` in `low`'s
range. The list is what the provider names, not every word it parses.

**Names.** `/v1/models/{name}` resolves an alias as the chat endpoint does — one
the entry lists (`grok-4.5-latest` → `grok-4.5`) and one no entry lists (`grok-4`
→ `grok-4.3`); a name it does not know is a `404`. The list of models holds
canonical ids, so the client's scan of it found no row for an alias. One entry
is 0.5–0.9 KB; the list is 6.2 KB.

### 9.2 What was broken

1. **A muted turn reasoned at the model's default.** The field was left out for
   every model. On `grok-4.3`, which can be switched off, the title, the roll,
   impersonation and a page's summary each reasoned some three hundred tokens
   for nothing; on `grok-4.5`–`4.7` the default they got is `high`.
2. **The thinking switch did nothing on xAI.** `thinking` is not in xAI's
   schema, and the settings offer the switch in that mode.
3. **A chosen effort made every turn a `400`** on the three models with no list
   — recorded in August as documented user error
   ([grok-xai-provider.md](grok-xai-provider.md), F4), which E1 has since decided
   the other way.
4. **A model named by an alias had no context window** from the catalogue.

### 9.3 Decisions

- **Asked ahead, unlike the other wires.** §4 kept the refusal as the source
  because it is free and asking costs a request. Neither holds here: xAI's
  refusal lists nothing; a muted turn meets no refusal at all — the field left
  out is an answer, the wrong one; and the client already fetches the model's
  entry once, for the window, so the list arrives in a request it was making.
- **The model's own route** (`GET /models/{name}`) in place of a scan of the
  list: it resolves an alias, and is a tenth of the bytes.
- **`none` where it is not listed is the lowest listed depth** — E1's rule for a
  refused `none`, in place of the field left out.
- **`minimal` goes out as `low`.** The list is followed where it and the API's
  leniency differ: the two reason alike, so nothing is lost, and a word the
  provider does not name may stop being parsed.
- **No list is silence, not "takes none".** A model without `capabilities` is
  sent the effort as chosen and its refusal is read: asked once more without the
  field, remembered once accepted (`EffortWire`). Dropping the field ahead, on
  silence, would make a chosen effort vanish without a word on the first model
  whose entry lags its release. Three of three such models refuse today, and the
  refusal is free.
- **With no entry at all** — a fetch that failed, a name xAI does not know — the
  two answers that hold on every model with a list stand in: `none` left out,
  anything above `xhigh` sent as `xhigh`. What the client did for every model
  before.
- **Named in the log, once per value** (E2); nothing in the chat.

### 9.4 What was implemented

- `OpenAiClient::for_xai()`, in place of `with_effort_none_omitted` and
  `with_effort_capped_at`: the entry by name, the effort as the list has it.
- `wire`: `ModelEntry::listed_efforts` (`capabilities.reasoning_effort`, in the
  scale's order), `effort_wish` (a request's wish: `none` for each of the three
  ways it says "do not reason"), `xai_effort`, `refuses_effort_parameter`.
- `ChatCompletionRequest: CarriesEffort` and `OpenAiClient: EffortWire` — the
  fourth wire on `send_asking_again`, answering one refusal: the parameter
  itself.
- `fetch_single_entry` — one helper for OpenRouter's `GET /model/{slug}` and
  xAI's `GET /models/{name}`.

**Live — GO** (the real API, Windows 11):

| smoke | models | before | after |
|---|---|---|---|
| a muted turn, twice | `grok-4.3` — also named `grok-4.3-latest` and `grok-4` | 108 reasoning tokens — red | 0, both turns |
| a muted turn | `grok-4.7`, `grok-4.5`, `grok-4.6` | no effort sent: the default | `Stop`; `none → low` |
| a turn at `max` | `grok-4.7`, `grok-4.5`, `grok-4.6` | `xhigh`, configured | `Stop`; `max → xhigh`, from the list |
| a turn at `high`, twice, then a muted one | `grok-build-0.1`, `grok-4.20-0309-reasoning`, `grok-4.20-0309-non-reasoning` | the `400` — red | `Stop` each; `high →` no effort |
| the window of a model named by an alias | `grok-4.5-latest`, `grok-4`, `grok-code-fast-1` | none — red | answered |
| `max` sent as it is — the control | `grok-4.7`, `grok-4.5`, `grok-4.6` | | still `400 "Invalid reasoning effort."` |
| the client's older smokes, five | `grok-4.5`, `grok-4.3` | | 5 of 5 on each; the thoughts smoke was empty-handed once in three on `grok-4.5`, on a request this change leaves byte-identical |

Not run: a chat inside the running app (the TUI needs a real terminal).

**Found beside it, not in this task.** xAI's `/language-models/{name}` also
publishes `input_modalities`, and this client's `vision()` reads only llama.cpp's
`/props` and OpenRouter's `architecture`, so on xAI it answers `Unknown`. Every
language model listed today takes images, so nothing is refused or withheld
wrongly; the day xAI lists a text-only one, an attached image will be its `400`.
