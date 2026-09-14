# Track plan: the endpoint is asked what the model can do

**Status:** design complete, **all forks decided** (G3 and the scope by the user,
2026-09-14) and **implemented** — see §6. The live run (§5) is owed: this session
has no route to the service.

- **G3 — (ii): the offer *and* the metadata snapshot.** The wire keeps sending
  everything; what narrows is what the settings screen and `set_sampling` offer,
  and what `Message.metadata` records as applied.
- **G4 — both in one PR.** One fetch, one memo, one journal entry.

The two proposals the OpenRouter review left buildable, and measured as
buildable: F3(b) — fill the compaction window from the catalogue — and F4(c) —
stop offering sampling fields the endpoint will drop. They are one track because
they are **one HTTP request**:
[openrouter-external.md](research/openrouter-external.md) §5, §8.1 (M5).

Measured, `GET /v1/models` on OpenRouter carries, per model:

```
id                   context_length  supported_parameters
deepseek/deepseek-r1         64000   frequency_penalty, include_reasoning, max_tokens,
                                     presence_penalty, reasoning, repetition_penalty,
                                     response_format, seed, stop, temperature,
                                     tool_choice, tools, top_k, top_p
```

**Related:** spec §6.7 (the context window), §8 (sampling), §3.4 (the engine's
lifecycle), [ADR 0004](decisions/0004-engine-contract-multi-provider.md),
[docs/journal/engine.md](journal/engine.md),
[external-model-name.md](research/external-model-name.md) (why `/v1/models` is
already fetched, and why a many-model catalogue is not guessed at).

## 1. What is broken, precisely

Both are **silent** degradations on a gateway, measured live:

- **the window.** `Orchestrator::context_budget`
  ([compaction.rs:259](../src/app/orchestrator/compaction.rs)) resolves an
  explicit setting → a managed server's `-c` → what the engine reports
  (llama.cpp's `/props`). A gateway has no `/props`, so there is no source and
  the automatic trigger stays inactive: measured, a conversation reached 22 567
  tokens with nothing folded, and the next step is the provider's "context length
  exceeded" rather than a roll (spec §6.7);
- **the sampling set.** `supported_sampling_fields(None)`
  ([sampling.rs:247](../src/entities/sampling.rs)) returns *every* field for
  `external`, because `external` means llama.cpp there. Through a gateway,
  `min_p`, `typical_p`, `top_n_sigma`, dynatemp, adaptive, mirostat, DRY, XTC and
  `samplers` are dropped on the way — and `repeat_penalty` is worse than dropped,
  since the gateway's field is spelled `repetition_penalty`, so the knob looks
  set and does nothing. The same list is what `set_sampling` advertises to the
  model and what `Message.metadata` records as "applied".

## 2. What the answer costs to get **[code]**

`OpenAiClient::listed_models` ([client.rs:153](../src/shared/api/openai/client.rs))
already fetches `GET /v1/models` — for the model's *name*, and it reads only
`id`. The two fields this track wants ride the same response, so the marginal
cost of the whole track is **parsing two more keys**, plus the plumbing to carry
the answer to three consumers.

## 3. Decided, with the reason

- **One fetch, one landing.** The budget question is already asked in the
  background once per applied engine (`ContextDiscovery`,
  [compaction.rs:113](../src/app/orchestrator/compaction.rs)). This track does
  not add a second background task: the same task learns both, and lands one
  answer. Two tasks against one endpoint, racing to fill two memos, is how a
  future reader gets a heisenbug.
- **Discovered state lives in the orchestrator, not in the config.** It is
  learned, epoch-stamped and thrown away when the engine changes — exactly what
  `ContextDiscovery` already is, and exactly what `AppConfig` must not become
  (it is persisted, backed up and edited by hand).
- **Silence means today's behaviour.** A catalogue that lists no
  `context_length`, a model field left blank (nothing to look up), a fetch that
  fails, a llama.cpp `/v1/models` that carries neither key — all of it resolves
  to what happens now: no window, every field offered. The feature can only
  *narrow* from a positive answer; it never invents one.
- **The window is used as reported**, never divided or scaled — the same rule
  `/props` already follows (spec §6.7, and the `total_slots` trap behind it).
- **An explicit setting still wins.** `compaction.context_tokens` is the user's
  own number; the catalogue slots in *after* it and after a managed server's
  `-c`, before "cannot say".
- **The trait method needs a delegation test.** `EngineBackend`'s
  question-methods default to "cannot say", and `RetryBackend` wraps every
  external and cloud turn: three times already a new method was left
  un-delegated and answered the default, invisibly, because "I cannot say" and "I
  forgot to ask" are the same value (lessons §9). One test per method that
  asserts a value the decorator could not have produced by falling through.

## 4. Forks

### G1. Where the catalogue is read — **recommendation (a)**

- **(a) a new `EngineBackend` method**, `model_capabilities() -> Option<…>`,
  defaulting to `None`; `OpenAiClient` implements it off `/v1/models`, every
  other backend inherits the default. The clouds answer `None` and keep their
  static tables — their limits are known at compile time and are not a gateway's
  business.
- (b) read it inside `OpenAiClient::context_budget` only, and drop F4(c).
  Cheaper, and it leaves the sampling screen lying.

### G2. How the answer reaches the consumers — **recommendation (a)**

- **(a) a second discovery in the orchestrator**, symmetric to
  `ContextDiscovery`, feeding the compaction trigger *and* the two gates that
  today read `supported_sampling_fields(provider)` — the settings screen
  ([helpers.rs:19](../src/screens/settings/helpers.rs)) and the sampling tools
  ([introspection.rs:103](../src/features/tools/introspection.rs),
  `ToolGates::sampling_provider`). FSD holds: the orchestrator owns the engine
  and hands screens and features a value, as it already does for the tool gates.
- (b) put the discovered set in `AppConfig` — rejected above.
- (c) ask at render time — rejected: HTTP on a draw path.

### G3. What F4(c) actually changes — **open, and the one that needs your call**

The question is not *whether* to narrow the offer, but how far the narrowing
travels:

- **(i) the offer only** — the settings screen and the `set_sampling` schema stop
  showing what the endpoint does not list. The wire is untouched (the gateway
  drops those fields anyway, at no cost), and `Message.metadata` still records
  them as applied.
- **(ii) the offer and the metadata snapshot** — `retain_supported` also narrows,
  so a message's record of "what was applied" stops naming fields the endpoint
  discarded. **Recommended**: the snapshot claiming a `min_p` that never reached
  the model is the same lie as the settings screen showing it, and it is the copy
  that survives into the chat file.
- (iii) the offer, the metadata **and** the wire — also stop *sending* the
  unlisted fields. Not recommended: OpenRouter's list is per model, while what
  actually serves the request is a per-provider route, so a field the route does
  accept but the catalogue does not advertise would be silently dropped **by
  us** — trading their silent drop for ours, and this time unrecoverable.

### G4. Scope — **open**

- **(i) both, one track, one PR.** One fetch, one memo, one journal entry; the
  plumbing for F4(c) is the larger half and F3(b) rides it for free.
- (ii) **F3(b) first**, F4(c) after. The window is the degradation that costs a
  conversation; the sampling offer costs a wrong expectation. A smaller PR lands
  sooner and the contract change gets its own review.

## 5. What a live run has to show

Against OpenRouter (`MINDFORK_ENGINE_URL`, `MINDFORK_ENGINE_MODEL`), and against
a local `llama-server` for the half that must not regress:

| | what it proves |
|---|---|
| **N1** | with the model set and no explicit window, a long chat folds by itself — the trigger fires on 64 000 rather than never |
| **N2** | the settings screen's sampling group shows the catalogue's set, and `set_sampling`'s schema carries the same |
| **N3** | a local `llama-server` is unchanged: `/props` still answers first, every field still offered, no second request where there was none |
| **N4** | a blank model field, a fetch failure and a catalogue without the keys each leave today's behaviour exactly |

## 6. What was implemented

- **`EngineBackend::model_capabilities`** (contract.rs) → `ModelCapabilities
  { context_length, sampling_fields }`, defaulting to `None`. `OpenAiClient`
  overrides it off the entry for the **configured model** in `GET /v1/models`
  (`catalogue_entry`); every cloud inherits the default. `RetryBackend`
  delegates it, with the delegation test §3 promised.
- **One task, one landing.** `EngineFacts { budget, caps }` replaces the bare
  budget on the channel the background question already used; the memo
  (`ContextDiscovery`) keeps both under one epoch, and the landing rebuilds the
  tool registry **only when the published set changed** — `set_sampling`'s schema
  is baked into the registry, and a re-ask after a readiness flip should cost
  nothing.
- **F3(b):** `Orchestrator::context_budget` gains the catalogue as its last
  source, after `/props`.
- **F4(c):** `entities::sampling::available_sampling_fields(provider, endpoint)`
  is the one place the two narrowings meet, with an explicit alias table for the
  two vocabularies (`repetition_penalty` ↔ `repeat_penalty`; one `reasoning` ↔
  `thinking` + `reasoning_effort`). It feeds the settings screen (via
  `AppEvent::EngineSamplingFields`, on the `EngineSlots` pattern), the
  `set_sampling`/`get_sampling` schema and filter, the tool gate, and
  `retain_supported` for the metadata snapshot. The settings screen's own
  per-provider predicate was **deleted** — it was this function's first half, and
  a second spelling is how the two drift apart.

**Tests**: 3232 green, 179 ignored (3224 / 178 before) on the tracked count; on
Linux 3225 / 174. Eight new, including the narrowing's own fixture — the list
OpenRouter actually returned for `deepseek/deepseek-r1`, not an invented one —
and the three shapes of silence, each asserted to leave behaviour as it shipped.
