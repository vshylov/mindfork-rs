# Journal — Fabula, the story engine

Fabula, the multi-agent story engine used from the chat: its database and event log, the
simulation (characters and the World Master), the Author's loop and its reviewers, the story
tools with their pauses, and the evaluation that decides whether the simulation stays.

**Reference documents for this area:** [docs/fabula.md](../fabula.md) (the track plan),
[docs/research/fabula-engine.md](../research/fabula-engine.md) (the design),
[ADR 0014](../decisions/0014-fabula-story-engine.md), architecture.md §3

Entries are in chronological order. They record what was done, why, what was measured and
what was rejected — the reasoning behind the code, not its current shape. For the current
shape read the reference documents named above; for the traps that recur across areas read
[lessons.md](../lessons.md).

## Entries (14)

- Post-M9: Fabula — PR 1, the story database (done)
- Post-M9: Fabula — PR 2, the model port and replay; Gate G0 GO (done)
- Post-M9: Fabula — PR 3, the reply's schema on the wire (done)
- Post-M9: Fabula — PR 4, the simulation; Gate G1 GO (done)
- Post-M9: Fabula — PR 5, the reply's schema on the cloud wires (done)
- Post-M9: Fabula — PR 6, the Author's loop (done)
- Post-M9: Fabula — PR 7, stories in mindfork's data (done)
- Post-M9: Fabula — PR 8, the story run in the background (done)
- Post-M9: Fabula — PR 9, the story driven from the chat; Gate G2 GO (done)
- Post-M9: Fabula — PR 10, a scene played again at the checkpoint (done)
- Post-M9: Fabula — PR 11, the story's register (done)
- Post-M9: Fabula — PR 12, the craft of the scenes and the lines (done)
- Post-M9: Fabula — a chapter's choices stand (done)
- Post-M9: Fabula — a map every scene can reach (done)

### Post-M9: Fabula — PR 1, the story database (done)

The first code of the track (docs/fabula.md §4, M0): the domain types, a story's database
behind one actor, and the fold of the world over a branch path. Nothing in the application
calls it yet — the background story run is PR 8 — so both new modules carry an
`allow(dead_code)` that goes when it lands, and nothing here is user-visible (no CHANGELOG
item; no released build writes a story).

- **`entities/story/`** — keys the canon fixes (`CharacterKey`, `LocationKey`, `ItemKey`,
  `SecretKey`) and ids a run derives from position (`ch1/sc2/r0/k1/e3`, `…/p4`, `run2`), never
  drawn at random, so a replayed run sends the same prompts (spec §9). `Canon` with
  `deny_unknown_fields` and `check()`, which returns every problem at once — keys (1–64
  characters, no whitespace, `/` or control characters, unique per kind without regard to
  case; an item placed twice is a repeated item key), routes (to a real place, not to
  itself, at least one tick, listed once), starts, empty names, descriptions, secrets and
  goals. `SceneSetup`, `Proposal` with the thought first, `Action`, `Visibility`, `Effect`
  (move, place, give, take, drop, reveal, injure), `NewEvent`/`Event`, `Cause`; every union
  tagged by `type`, for the schema dialect of spec §8.
- **The one gate, `WorldState::admit`.** It checks an event against the state it applies to —
  the location and every character it names exist, the living ones are at the event's
  location, time does not run back, a speech, action or movement has an actor and a world
  event has none, a whisper has addressees present — then applies the effects in order, each
  against what the earlier ones left, and computes the witnesses on the state before:
  everyone alive at the location (and at a move's destination) for a public event, the
  addressees for a whisper, the actor alone for a hidden deed; an effect's subjects perceive
  it unless it is hidden; the dead perceive nothing. `Place` exists for the time between
  scenes: only in opening and settle events, anywhere reachable along routes. `fold` replays
  a path through the same gate, so a stored event that no longer folds names itself.
- **The fold lives in `entities`, not `features`** as the plan's §3 first had it: the database
  actor runs it for every commit, so a commit's check and its append see one state with
  nothing between them, and the layer gate lets `shared` name `entities` but not `features`.
  The plan's table now says so.
- **`shared/storage/story/`** — `StoryDb` is a cloneable handle on an actor: a thread of its
  own owns the only connections to the story file and its call-log sibling; every read and
  write is a job awaited through a oneshot. The capabilities `BranchWriter`,
  `ReflectionWriter`, `AuthorWriter` and `RunLogWriter` are not `Clone`, one per store at a
  time, checked by a token on every write and given back on drop — a resumed run takes its
  branch back that way. A second open of a file already open in this process is refused; the
  same path for both files too. A branch's path is its ancestry with a limit per segment
  (the fork's `seq`, the minimum going up), so a parent that goes on after a fork does not
  leak into the child.
- **Where the build differs from the spec's sketch** (recorded in `schema.rs`'s header and in
  the spec): reads go through the actor as writes do, so the files keep the **default rollback
  journal**, not WAL — WAL's concurrent readers buy nothing behind one thread, and a backup can
  take the file as `data.db`'s is taken, with no `-wal` to handle (PR 7); **append-only
  triggers on every table but `run` and `step`** (a failed step is finished by a resume, in
  place); `event` keeps `observable` and `private` as text, its visibility and cause as JSON
  columns of their own; `canon_doc` keeps the author's order (`position`) and marks a removed
  document by a version with a null body; `llm_call.cost_usd` for T1's dollar cap; routes and
  secrets are arrays of closed objects rather than a map and plain strings. The version is
  read **before** any DDL, so a newer file is refused untouched; a file that is not a database
  is refused by its header before SQLite opens it (docs/lessons.md §8). The canon is frozen
  once the story has an event — M4's canon deltas lift that.
- **A finding for PR 2.** The test that expected `{"type":"public","to":[]}` to be refused
  failed: serde reads a stray field on a variant without fields in silence, even under
  `deny_unknown_fields` (a struct variant does refuse). A whisper-shaped reply would read as a
  public line. The test now pins that behaviour, and the rule passes to PR 2: a reply is
  validated against its closed schema before it is parsed (docs/lessons.md §8).
- **The compile-time assertion was checked against its mutant.** With an `impl Clone for
  BranchWriter` added, `cargo check` fails with E0283 at the assertion; the file was restored
  from its bytes afterwards.
- **No live run**: storage and pure logic only, no engine, model or tool path (AGENTS.md §3).
- **Tests** (+49, 4524): the canon's checks on the fixture and on a canon with every problem
  at once; the unions' tags; the gate's 21 refusal cases, witnesses for public, whispered, hidden
  and moving events, items changing hands, a reveal, death, effects in order, a fold that no
  longer folds; `UPDATE` and `DELETE` refused on the event log and on every append-only table
  (both triggers on each, none on `step`); a second writer refused for every store until the
  first is dropped; a branch never sees a sibling's events, a fork nothing its parent wrote
  after it; commits checked against their branch, their proposal's branch and their reaction's
  target; the canon read back as imported and versioned per document; the main line following
  the picks; reflections along the path; steps and calls by address, the call id an
  independently computed UUIDv5; both files stamped; a newer file and a non-database refused
  untouched.

**Gates**: 4524 unit tests green, 308 `#[ignore]`; clippy `-D warnings`, fmt, `cyrillic_scan`,
`link_check`, `doc_index_check`, `layer_check`, `list_scroll_check` clean.

### Post-M9: Fabula — PR 2, the model port and replay; Gate G0 GO (done)

M0's second half (docs/fabula.md §4): the model port, its two adapters, the replies'
schemas with their validator and the dialect's check, and one scene played by script, which
together let a scene be recorded against a model and replayed from its call log. All of it
sits in `features/story/` under an `allow(dead_code)` until PR 8; nothing is user-visible.

- **`LlmPort` takes a `Schema`, not a `Value`.** The crate's `serde_json` is built without
  `preserve_order`, so a `Value` sorts its keys: the `Proposal` schema built as one would reach
  the model with `action` before `private_thought`, while the order of the properties is the
  order the model writes in (spec §8). `Schema` is the dialect by construction (closed objects,
  every property required, null unions, tagged unions of closed objects; nothing to write a
  `$ref` or a bound with) and prints its own JSON in the order declared. `dialect::check` reads
  that JSON independently and names every rule broken, including more than Anthropic's 16
  unions. Turning `preserve_order` on crate-wide was rejected: it would change the bytes of
  every file and request the application writes. **For PR 3:** `ChatRequest` carries the
  schema's printed text to the wire as it is, never through a `Value` (docs/lessons.md §8).
- **Derived from the types, checked both ways.** `StorySchema` gives `Proposal`, `Action` and
  `Visibility` their schemas, with the cast and the map as enums in the canon's order. The tests
  hold every value of the type to the schema, and every instance the schema allows (one per
  enum value and union branch) to the type, by parse and round trip. A field that reads as the
  model's reasoning is avoided: `private_thought` is described as the character's inner voice,
  story content (spec §8, `reasoning_extraction`).
- **The validator runs before serde**, as PR 1's finding asked: `{"type":"public","to":[...]}`
  is refused (`$.visibility.to: is not in the schema`), every violation is reported at once, and
  an `enum` value matched in another case comes back in the schema's spelling.
- **One policy for both adapters.** `run_json` and `run_text` take their attempts from an
  `Attempts` source: a cut or a transient failure is tried once more with the same request; an
  invalid reply is repaired once, by a turn that quotes the validator; a refusal or a lasting
  failure ends the call; a cancelled one is never retried. `EngineAdapter` sends each attempt
  to the `EngineBackend` and records it — request, reply text, how it ended, tokens, cost,
  latency, thought length, the route that served it — the failures included. `ReplayAdapter`
  reads attempt N back from the call log, so a replay walks the same retries and repairs as the
  recorded call. A prompt that hashes differently from the one recorded is served all the same,
  by address, and reported as drift.
- **The fallback mechanism only.** `ChatRequest` has no output-schema field until PR 3, so
  every JSON call puts the schema in the system part (after the stable content, so a prefix
  cache keeps it), reads the reply leniently (fences and prose stepped over) and records
  `mechanism = 'fallback'`. One live call per address: a second is refused by the call log;
  how a resumed step numbers its attempts is PR 6's to decide.
- **The scripted scene.** The setup is committed as two opening events (what just happened,
  with everyone present placed at the scene; then the pressure). In round n the characters
  present propose in parallel from one view of the world, and a fixed rule stands in for M1's
  World Master: taken in the canon's order from the n-th, the first proposal the world admits
  is committed as the initiative, one it refuses keeps the world's reason, the rest lapse, and a
  wait commits nothing; two quiet rounds end the scene. A character's prompt holds its own sheet
  and secrets, the others' public descriptions and what it witnessed on its path, its own
  thoughts beside its own deeds; a test plays a hidden deed and finds it only in the actor's
  next prompt. `SceneOutcome::to_json` is the trace a replay is compared by.
- **Live, Qwen 3.8 27B on OpenRouter** (`qwen/qwen3.8-27b`, thinking off, temperature 0.8).
  Unpinned, nine calls went to five providers, and one reply, from Darkbloom, was **the schema
  itself** (1,232 tokens); the repair that quoted the validator got a valid proposal back
  (10 attempts, $0.0065). Pinned to DeepInfra through a local pass-through that adds
  `provider: {only: ["deepinfra"], allow_fallbacks: false}` (the client has no routing knob, by
  decision), two runs of nine were all valid at the first attempt. The second is the committed
  fixture: about 1,100 tokens in and 210 out a call, 4.6–8.6 s each, $0.0050 for the scene, no
  secret token in another character's prompt, and a replay identical to the recording. One
  thing for M1: Ines spoke her own secret aloud ("I was told to dim the lamp at midnight") in
  the unpinned run. Only the World Master can turn that into a `Reveal`, and the leak metric
  counts tokens, not paraphrases.
- **Gate G0: GO** — the fixture replays identically, with no drift, offline
  (`the_fixture_recorded_live_replays_identically`); a scene recorded through the engine
  adapter on a scripted model replays identically twice, with its repair and its retry; a second
  writer is refused and the not-`Clone` assertion holds (PR 1).
- **Mutation check:** ten mutants behind an environment switch, one build — the stray-key
  check, drift detection, the retry, the repair, the rule's rotation, the memory view, the
  case-insensitive enum, the private part, the dialect's closed object, the cut's status —
  every one failed a test. A test found its own false positive on the way: "pockets", which
  it took for the hidden deed, is in the schema's description of `verb`, so it now looks for
  the deed's own words.
- **Tests** (+43, 4567): the dialect's rules one by one; the schema's order, its two-way fit
  with the type, the validator's findings and the lenient read; every turn of the policy; what
  the engine adapter records for each way a stream ends, a request that cannot be opened and a
  cancelled run; the scene's rulings, its replay, drift, a missing call, the memory views;
  the live fixture's replay and its leak count, with a check that the count can see a leak.

**Gates**: 4567 unit tests green, 309 `#[ignore]`; clippy `-D warnings`, fmt, `cyrillic_scan`,
`link_check`, `doc_index_check`, `layer_check`, `list_scroll_check`, `claude_md_chart --check`
clean.

### Post-M9: Fabula — PR 3, the reply's schema on the wire (done)

M1's first pull request (docs/fabula.md §4): schema-constrained decoding on the two wires
development runs on, and the engine adapter using it. Nothing the person sees changes: no
chat turn sets the field, and every request without a schema is the bytes it was.

- **`ChatRequest.output_schema`, as text.** `OutputSchema` holds the schema's printed JSON
  in a `serde_json::value::RawValue`, so it reaches the wire byte for byte (`serde_json`
  gains its `raw_value` feature, which adds no crate). PR 2's finding is why: a `Value` sorts
  its keys, and the order of the properties is the order a model writes them in. A stub
  test reads the bytes the gateway receives and finds the schema there unsorted.
- **Chat Completions:** `response_format: {type: "json_schema", json_schema: {name, strict:
  true, schema}}`, the shape llama.cpp, vLLM, xAI, OpenAI and OpenRouter read; llama.cpp
  ignores `name` and `strict`, OpenAI requires both. Through the gateway a body with a schema
  also carries `provider: {require_parameters: true}`, so no endpoint that would take the
  schema as a hint is routed to. It chooses no provider, so the decision to keep routing
  knobs out of the client stands (docs/history/gateway-images-and-continue.md §3).
- **Anthropic:** `output_config.format: {type: "json_schema", schema}` beside the effort
  (GA; the beta `output_format` is not used). `AntOutputConfig` carries both, each sent only
  when set. The two rewrites that take an effort away, thinking within a budget on the 4.5
  generation and a refused effort level, keep the format.
- **`EngineBackend::enforces_output_schema`**, the sixth question of its shape, default
  `false`. The OpenAI-compatible client answers `true`, except for DeepSeek's API (it takes
  `json_object` only) and a gateway model whose catalogue entry does not list
  `structured_outputs`; silence there is a "no". The Anthropic client answers `true`, for the
  same reason `vision` is static: no model-name list. The Responses and Gemini clients keep
  the default until PR 5. `RetryBackend` delegates it, with its test written alongside, since
  the five questions before it were each forgotten once (docs/lessons.md §9).
- **The engine adapter** asks once per adapter. It sends the schema with every JSON attempt,
  the repair included, where the backend constrains, and records `mechanism = 'schema'`;
  elsewhere it records `fallback`. **The schema stays in the prompt under both mechanisms.**
  llama.cpp's grammar shows the model no description, so the prompt is where a field's
  meaning is read; and with one prompt for both, M1 compares the decoding alone. The prompt
  hash is unchanged, so PR 2's fixture still replays without drift.
  `EngineAdapter::fallback_only` is for a managed server with a draft model, whose grammar
  crashes under `json_schema` (docs/fabula.md §5): the supervisor knows that, the adapter
  cannot, and PR 8 wires it. The live smoke uses it to measure one mechanism against the
  other.
- **Live: the spec's §8 probe as a smoke** (`schema_live_tests`). Each run plays the harbour
  scene's first round, three characters proposing at once, through the application's own
  clients. Arms: both mechanisms, thinking off and on; three scenes an arm, one on the local
  server. Every attempt is read back from the call log.

  | Route | First attempts | Met the schema | Clean JSON, constrained | Clean JSON, fallback | Median output tokens, off / on |
  | --- | --- | --- | --- | --- | --- |
  | Claude Haiku 5.5 | 36 | 36 | 18/18 | 18/18 | 438–493 / 230–251 |
  | Claude Sonnet 5.5 | 36 | 36 | 18/18 | 18/18 | 221–251 / 221–243 |
  | Qwen 3.8 27B, OpenRouter, DeepInfra | 34 | 34 | 16/16 | 18/18 | 207–217 / 1,078–1,182 |
  | Gemma 4 31B, OpenRouter, DeepInfra | 36 | 36 | 18/18 | **0/18** | 127–170 / 887–1,043 |
  | Qwen 3.8 27B Q4_K_M, local llama.cpp b12023 | 12 | 12 | 6/6 | 6/6 | 225–228 / 864–1,527 |

  No attempt needed a retry or a repair. Gemma's fallback replies came in code fences every
  time, as in the spec's local probe, and the lenient read took them. Under the constraint
  every reply was clean JSON. Two of Qwen's gateway calls did not return within the smoke's
  600 s bound, one scene's worth, and DeepInfra's median latency ran from 13 to 98 s that
  hour, against 4–9 s for the same route in PR 2. A call that never answers is bounded only by
  the chapter's timeout today. M1 watches for it.
- **The constraint is the provider's, not the prompt's.** A second smoke asks in words for
  the single word "hello" and sends a schema the prompt never mentions. Without the schema all
  five routes said `hello`; with it all five answered `{"mood": "calm"}`. This rules out the
  reading that the models simply followed the schema in their prompts.
- **Muting thinking takes the application's three fields.** The smoke's first local run sent
  `thinking: false` alone, and Qwen 3.8 thought about 6,000 characters a call: a template
  with thinking baked in stops only at `reasoning_budget: 0`. Both smokes now mute the way
  `app/orchestrator/title.rs` does (`thinking: false`, effort `none`, budget `0`), and PR 2's
  fixture smoke was corrected with them; its committed fixture is unaffected, since sampling
  is not part of the prompt hash (docs/lessons.md, recorded again). Through the gateway the
  same switch reached the model as `reasoning: {enabled: false}`, and thought was 0 there in
  either run.
- **On Claude the wire has no "thinking off" yet.** The 5.5 models think adaptively when
  `thinking` is not sent, and only Haiku 5.5 takes `disabled`, so the "off" arms there are
  the model's default. On Haiku 5.5 that default wrote more (438–493 tokens) than adaptive
  thinking at the lowest effort (230–251). How the characters mute it on Claude is PR 4's to
  decide (docs/fabula.md §4, *Thinking on Claude*).
- **Mutation check:** six mutants, one build. Each was killed:
  - the decorator's delegation;
  - DeepSeek's refusal;
  - the catalogue's `structured_outputs`;
  - the gateway's `require_parameters`;
  - the format kept when the effort goes;
  - the adapter sending the schema.

  The sources were restored and the fingerprint checked.
- **Tests** (+11, 4578; four live smokes): both wires' new shapes and the bytes as sent; requests without a schema
  unchanged; the gateway answering from its catalogue; the delegation; the adapter's schema
  on every JSON attempt and never on prose, and the fallback alone never sending it.

**Gates**: 4578 unit tests green, 313 `#[ignore]`; clippy `-D warnings`, fmt, `cyrillic_scan`,
`link_check`, `doc_index_check`, `layer_check`, `list_scroll_check`, `claude_md_chart --check`
clean.

### Post-M9: Fabula — PR 4, the simulation; Gate G1 GO (done)

M1's second pull request (docs/fabula.md §4): the characters and the World Master on a
live model, a scene as K branches, each character's memory filtered by what it perceived,
and the planted secrets followed through every call.

- **The round** (spec §7):
  - *Intent*: every character present proposes at once, from one view; with p = 0.3, drawn
    from the branch's seed, one is told to break their routine.
  - *Initiative*: the World Master rules on each intent, feasible or refused with an
    in-world cause, and picks one; the engine commits it, the other feasible intents lapse,
    and a world event may follow.
  - *Reaction*: the initiative's witnesses who are still present answer it; the World
    Master rules on them in the order they happen, each held to the world the one before
    left.
  - *Reflection*: every 3 rounds, through each character's own `ReflectionWriter`.

  A scene ends at 8 rounds, after two rounds with no character's deed, when its place
  empties, or when the World Master judges its pressure resolved.
- **The World Master proposes, the engine commits.** Every event it writes passes
  `WorldState::admit` and a knowledge check before it is committed: a token in what is
  perceived needs every witness to know its secret once the effects apply, and a token in a
  thought needs the actor to know it. A refusal there is recorded with the proposal and
  counted, never repaired. A ruling that does not answer what it was asked, such as a
  character it was not asked about or an initiator whose intent it did not find feasible,
  is a slip: recorded and skipped.
- **What each role sees.**
  - A character: its own sheet, the others' public lines, the observable part of what it
    perceived (a whisper to it marked), its own thought under its own deeds, its
    reflections, and the secrets revealed to it.
  - The World Master: the rules, the map with its routes, the whole cast with their
    secrets (a reveal is its to rule on), the folded state, the scene's events with their
    witnesses, and the setup without the Author's intent. Of each proposal, the deed, its
    targets and its visibility, never the thought.

  The engine copies the proposal's thought into the event, so a thought cannot reach a
  witness through the World Master's words.
- **The contracts** (`contracts.rs`): the reflection, the ruling on the intents and the
  ruling on the reactions, derived from their types in the dialect.
  - The cause comes before the verdict, and the rulings before the event they produce.
  - No field asks for reasoning.
  - The World Master's effects leave out `place`, which is the opening's alone.
  - A canon without items or secrets drops the variants that would name one, since the
    dialect refuses an empty `enum`.
- **K branches one after another**, forked from the same tip. A branch whose call fails is
  reported failed and spares its siblings; a cancelled call stops the scene.
- **Thinking per role** (`EngineAdapter::with_role_sampling`). The characters get the mute
  (`thinking: false`, effort `none`, budget `0`), the World Master thinking at a low effort.
  On Claude the mute now reaches the model: the wire change is docs/journal/engine.md,
  *Thinking off reaches Claude*.
- **The leak audit** (`leaks.rs`) is independent of the engine's check. It reads every
  character call back from the call log and finds a planted token the caller was shown, or
  wrote, before its secret was known at that moment of the branch. The moment is ordered
  by round and phase: intent, initiative, reaction, reflection.
- **Gate G1** (`simulation_live_tests`): 20 scenes from the fixture canon, each in a story
  of its own, K = 3, at most 8 rounds. Five setups are dealt in turn: the harbour with all
  three, the warehouse, the lighthouse, the inn and the chapel, each with a pressure that
  leans on a secret.

  | Route | Scenes × K | First attempts | Schema-valid | Leaks | Reveals | Refused by the World Master | Refused by the engine | Slips | Median character reply, tokens | Spend, time |
  | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
  | Claude Haiku 5.5 | 20 × 3 | 2,101 | 2,101 (100%) | 0 | 10 | 178 | 4 | 32 | 214–235 | about $1.44 at list price; 23 min, 4 scenes at once |
  | Qwen 3.8 27B, OpenRouter, DeepInfra (bf16) | 20 × 3 | 1,972 | 1,971 (99.95%) | 0 | 23 | 131 | 14 | 0 | 233–256 | $2.40; 54 min, 10 at once |
  | Gemma 4 31B, OpenRouter, DeepInfra (fp8) | 20 × 3 | 1,888 | 1,886 (99.89%) | 0 | 38 | 154 | 3 | 0 | 138–152 | $1.12; two runs, below |
  | Qwen 3.8 27B Q4_K_M, the gate's weights, local llama.cpp b12032 | 5 × 3 | 457 | 457 (100%) | 0 | 9 | 27 | 4 | 0 | 237–275 | 91 min, one scene at a time |

  - **No leak on any route, while the secrets moved.** The World Master committed 10–38
    reveals per route, and characters were shown another's secret 62–180 times on each
    route after it was revealed to them, never before. No World Master wrote a token into
    what was perceived; it paraphrased each reveal, so the engine's knowledge check never
    fired.
  - **The first attempts that missed** were three of 6,418, and every one was taken by the
    second attempt; no reply failed its schema.
    - Qwen 3.8 once, and Gemma 4 once, looped to the 4,096-token cap under the constraint:
      Qwen on synonyms in a thought, Gemma on whitespace. Gemma did the same once more in a
      scene that was cut and not scored.
    - DeepInfra once answered Gemma with an upstream `429`.
  - **The World Master's refusals.**
    - By ruling: 0 to 18 per scene.
    - By the engine: 0 to 3 per scene. The causes were an item the actor did not hold or
      that did not lie there, a character elsewhere, a move with no direct route, a move to
      where the character already was, and a whisper to oneself.
    - Slips: 32 on Haiku, all of one shape. Each was a ruling on a character who had
      proposed no deed, mostly one not in the scene at all, "refused" because absent. The
      open models made none. The prompts stay as measured; the shape is M2's to look at.
  - **Thinking off held its cost.** The median character reply was 214–275 tokens (Gemma's
    138–152), the cost model's 0.25k. The World Master thought a median of 582 characters
    a ruling on Haiku and 2,400–2,800 on the open models.
  - **The harness's bound.** The first Gemma run took 10 scenes at once through DeepInfra.
    Four of them passed the 45-minute bound in the queue, with a median of 48 s per World
    Master call there, and the run was stopped. The harness gained `_ONLY` and
    `_BOUND_SECS`, and the 14 scenes left unfinished ran again 5 at a time with a 3-hour
    bound. The six that had finished are kept; the four that were cut are kept apart,
    unscored ($0.23 of spend).

  **Gate G1: GO**, on the three development routes and on the gate's own weights.
- **Mutation check:** 15 mutants, one build. Each was killed:
  - the two knowledge checks;
  - an initiator whose intent was not feasible;
  - a move without its effect;
  - the initiator reacting to its own deed;
  - the stalemate's count;
  - a failed branch stopping its siblings;
  - a routine never broken;
  - secrets told to a character who never learned them;
  - the World Master shown a thought;
  - the audit's moments;
  - a role's own sampling;
  - on the Anthropic side: the budget's mute, Sonnet's refusal read as `between_tools`, and
    the kept shape.

  The sources were restored and the fingerprint checked.
- **Tests:** 4613 unit tests green (+35), 315 `#[ignore]` (+2: the G1 harness, the Claude
  thinking-off smoke).

**Gates**: unit tests green; clippy `-D warnings`, fmt, `cyrillic_scan` (and its
`--self-test`), `link_check`, `doc_index_check`, `layer_check`, `list_scroll_check`,
`claude_md_chart --check` clean.

### Post-M9: Fabula — PR 5, the reply's schema on the cloud wires (done)

M1's last pull request (docs/fabula.md §4): `ChatRequest.output_schema` on the two wires
PR 3 left, so that a story runs under the constraint whichever cloud the person's model is
on. Nothing the person sees changes: no chat turn sets the field, and a request without a
schema is the bytes it was.

- **Measured before written.** One request per model, by hand, with a small schema in the
  dialect (a tagged union, a nullable enum) and the "say hello" control. Responses held on
  `gpt-5-nano`, `gpt-5.4-nano`, `gpt-6-luna` and `gpt-6.1-sol`. On Gemini the two fields
  disagreed:

  | Model | `responseFormat.text` | `responseJsonSchema` | No schema |
  | --- | --- | --- | --- |
  | `gemini-3.8-flash` | the object | the object | `hello` |
  | `gemini-3.5-flash` | the object | the object | `hello` |
  | `gemini-2.5-flash` | **`hello`**, 4 of 4 | the object | `hello` |
  | `gemma-4-31b-it` (Gemini API) | the object | the object | `hello` |

  The API reference recommends `responseFormat` and marks the older fields deprecated, and
  the plan named it. 2.5 ignores it in silence, so the wire sends `responseJsonSchema`. A
  field the API stops reading would fail as a `400`; a field a model ignores fails as prose
  (docs/lessons.md §3, recorded). Both fields together were accepted too; one was kept.
- **Responses:** `text.format: {type: "json_schema", name, strict: true, schema}`, flat,
  beside `text.verbosity` in one `text` object, which goes out when either is set. The schema
  is the printed text, as on the other wires.
- **A refusal on Responses.** Under a schema a model that declines writes a `refusal`
  content part, streamed as `response.refusal.delta`, and the response still completes. The
  client used to drop those events, so a refusal would have arrived as an empty reply and
  been repaired. Now its words land as the text and the turn ends as `Filtered`, which the
  port records as refused, final (docs/research/fabula-engine.md §8). Chat Completions'
  `delta.refusal` is not read either; no development route has sent one, so it is left as
  it is.
- **Gemini:** `generationConfig` was a `serde_json::Value`, which sorts keys, so the schema
  could not go inside it. It is now `GenConfig`: the knobs as a flattened map, with
  `responseMimeType` and `responseJsonSchema` as fields of their own, the schema raw. A body
  with only a schema still gets a `generationConfig`. The refused-level rewrite works on the
  map and leaves the schema where it was.
- **`enforces_output_schema`** answers `true` on both clients, statically, as `vision` does.
  The Responses client serves OpenAI only. The Gemini client serves every model the API
  lists for `generateContent`, the Gemma 4 among them, and each one tried held.
- **One route helper for the smokes.** `schema_live_tests::Cloud` names Claude, Responses and
  Gemini by their variables (`MINDFORK_{ANTHROPIC,OPENAI,GEMINI}_KEY`,
  `MINDFORK_FABULA_{CLAUDE,OPENAI,GEMINI}_MODEL`), and `engine_route` the OpenAI-compatible
  engine. The schema smokes gained the two clouds, and the G1 harness takes the first cloud
  whose variables are set, then the engine.
- **Live: the schema probe and the control**, three scenes an arm through the application's
  clients:

  | Route | First attempts | Met the schema | Clean JSON, constrained | Clean JSON, fallback | Median output tokens, off / on |
  | --- | --- | --- | --- | --- | --- |
  | `gpt-5.4-mini` | 36 | 36 | 18/18 | 18/18 | 128–145 / 236–258 |
  | `gpt-6.1-sol` | 36 | 36 | 18/18 | 18/18 | 100–104 / 109–110 |
  | `gemini-3.8-flash` | 36 | 36 | 18/18 | 18/18 | 128–152 / 138–143 |
  | `gemini-2.5-flash` | 36 | 36 | 18/18 | 8/18 | 104–191 / 121–151 |

  No attempt needed a retry or a repair. Gemini 2.5's fallback replies came in code fences
  10 times of 18, and the lenient read took them. On `gpt-6.1-sol` and `gemini-3.8-flash` the
  "off" arm is the refusal's answer (`low` for both), so the two arms ask for the same depth
  there. Asked in words for a bare greeting, all four said `hello` without the schema and
  `{"mood": "calm"}` with it.
- **Live: a small G1** (2 scenes × 2 branches × up to 4 rounds) sent every simulation schema
  through each provider: the World Master's two rulings with their effect unions, the
  reaction and the reflection.

  | Route | Calls | First valid | Leaks | Refused by ruling / engine | Slips | Tokens in / out |
  | --- | --- | --- | --- | --- | --- | --- |
  | `gpt-5.4-mini` | 93 | 93 | 0 | 4 / 0 | 0 | 196,182 / 15,614 |
  | `gemini-3.8-flash` | 88 | 88 | 0 | 2 / 1 | 0 | 153,818 / 12,718 |

- **Finding for M2: Gemini 3.x thinks less under a schema.** The G1 run's World Master
  thought 0 characters on `gemini-3.8-flash`. One prompt ("think it through") at
  `thinkingLevel: low` with `includeThoughts`, three calls an arm: 69–98 thought tokens and no
  summary under the schema, 737–838 tokens and a summary without it. `gemini-2.5-flash` thought
  869–1,327 tokens under the schema against 1,546–1,600 without. A World Master on Gemini 3.x
  rules on a tenth of its thought, and the call log cannot show what thought there was.
  Whether its rulings suffer is M2's to measure; the fallback keeps the thought.
- **Mutation check:** 12 mutants, one build. Each was killed:
  - on Responses: strict off, the format dropped, the `text` object dropped with only a
    format, the refusal not marked, the client's answer;
  - on Gemini: the schema dropped, its MIME type dropped, the config dropped with only a
    schema, the client's answer, the respelt level not written, not removed, and not read.

  The sources were restored and the fingerprint checked.
- **Tests:** 4621 unit tests green (+8), 319 `#[ignore]` (+4: the probe and the control on
  each cloud).

**Gates**: unit tests green; clippy `-D warnings`, fmt, `cyrillic_scan` (and its
`--self-test`), `link_check`, `doc_index_check`, `layer_check`, `list_scroll_check`,
`claude_md_chart --check` clean.

### Post-M9: Fabula — PR 6, the Author's loop (done)

M2's first pull request (docs/fabula.md §4): a chapter from the canon's approval to its
checkpoint, every step of it resumable. Nothing the person sees changes; the chat tools
that start a run are PR 8 and PR 9.

- **The loop** (`chapter.rs`, spec §7). One run goes from where the story stands to its
  next pause:
  - *P1*: an unapproved canon asks for its approval, and the run ends.
  - *The outline*: the chapter's goals, set once.
  - *Scene by scene*: the Author plans the next scene from the scenes kept so far, or ends
    the chapter; the world plays it as K branches; the Author scores each against its
    intent and the four measures (tension, surprise, truth to character, consequences)
    and keeps one, or asks for the scene again under changed circumstances.
  - *The draft*, from the kept scenes' full traces.
  - *Review*: the Reader and the Continuity checker side by side; the engine's own check
    for a planted marker in the prose rides with the checker as a high note.
  - *Triage and revision*: the Author decides each note, a note it leaves out deferred,
    and revises while an accepted high note stands, at most twice. The last revision is
    not reviewed again, as in spec §7's loop.
  - *The summary* of the final version, in the human's language, then *P2*.
- **The least bad pick costs no call.** The spec has a separate `least_bad` call when the
  reruns run out. Here the last attempt's pick schema has no rerun variant, and its
  prompt says no rerun is left, so the Author's own last pick is the choice.
- **A scene whose every branch fails** is played again as it was, and after
  `1 + max_reruns` such attempts the run asks the human (*P3*). The human's answer plans
  the scene again with their words.
- **The stores are the state.** Each step reads what is stored before it calls a model,
  so a run killed anywhere resumes, as a new run, from the first step no run finished
  (spec §9). A step whose outcome has no row of its own, the Author ending the chapter,
  is known by its step. Writes come in an order a kill between them cannot break: a
  rerun's scene is stored before the pick that asks for it.
- **A lost branch is replaced, not counted** — found by the live kill run, below. A branch
  a stopped run left half-played cannot be taken back, since its events are in the log;
  it is recorded failed, marked `…/interrupted`, and a branch under the next index plays
  in its place. The pick never sees it, and it never counts against the scene.
- **The pauses** are rows (spec §14): a `question` (canon, chapter or scene) ends the run,
  one waits at a time, and a run that finds one waiting starts nothing. An `answer`
  carries its approval in the same transaction: an approved canon's documents, an
  accepted chapter's acceptance. A change at P2 revises the chapter with the human's notes,
  a version of origin `human`, which is not reviewed again. An `assumption` records a
  doubt an Author's reply met and the choice it made; the checkpoint lists them.
- **The budget** (T1 (a)): every call under the chapter's addresses, in every run and
  attempt, from the call log. It is checked before each call, in tokens on every provider
  and in dollars where the provider reports them, and a spent one stops the run there.
- **The store** (`actor_manuscript.rs`): `outline`, `chapter_summary`, `question`,
  `answer` and `assumption`, all append-only, and `chapter.origin`. A version's summary
  has a row of its own, since it is written only once the version is final. The file was in
  no release, so `chapter` changed in place and `STORY_SCHEMA` stays 1. A `ReviewWriter` per
  version and reviewer writes a review whole, once.
- **The contracts** (`author.rs`). The deciding fields come last. No field is named like
  the model's reasoning: the spec's `rationale` and `reason` are `human_note` and
  `critique`. A schema is built per call where the call's options differ: the first plan
  cannot end the chapter, the last attempt cannot rerun, a pick names only the branches
  that played, a triage only its notes. The dialect wants two branches in a union, so a
  union left with one is printed as its closed object, its tag a one-value `enum`.
- **What each role sees** (`author_prompts.rs`): the Author everything, the arc and every
  secret included, but never a planted token; the Reader only the published chapters and
  the new one; the checker the record of the kept scenes with every event's id and what
  each character knew when the chapter opened, but not the Author's intent.
- **The story's language** is `Style::language`, English when empty; the human's language
  is a run's setting, for the summary, a pick's note and the assumptions.
- **The markdown export** (`export.rs`): each chapter's accepted version, or its latest.
- **Live: one chapter end to end** (`chapter_live_tests`), the fixture story through the
  application's clients: the characters muted, the World Master at a low effort, the
  Author at a medium one, the reviewers at a low one.

  | Route | Shape: K, rounds, scenes at most | Calls | Leaks | Scenes kept | The chapter | Notes, triaged | Tokens | Spend, time |
  | --- | --- | --- | --- | --- | --- | --- | --- | --- |
  | Qwen 3.8 27B, OpenRouter, DeepInfra (bf16) | 3, 6, 3 | 192 | 0 | 2, no rerun | 1,592 words, not revised | 12, 6: all accepted, all low | 718,865 | $0.29; 39 min |
  | `gemini-3.8-flash` | 2, 3, 2 | 101 | 0 | 2, no rerun | 1,572 words, not revised | 5, 1 | 261,456 | 6 min |

  - No call needed a second attempt, on either route.
  - On Qwen the Author ended the chapter itself after two of its three scenes, its goal
    met; on Gemini the cap of two ended it.
  - Qwen's Author recorded 12 assumptions, Gemini's 4. Each was a real question the human
    could have been asked: who the "wrong person" of the arc is, whether Mara knows Ines
    was paid, how hard a shove should cost Mara in the morning.
  - The prose follows the record: the lines spoken, who learns which secret and when. It
    shows no planted marker and no key in backticks.
  - Qwen's chapter cost 718,865 tokens. The characters took 385 thousand (130 calls), the
    World Master 242 thousand (51), the Author 71 thousand (9) and the two reviewers 22
    thousand. The spec's model estimated 1.8 million for eight rounds and no reruns; these
    scenes ended sooner. That figure, and three attempts a scene, bound PR 8's default
    budget.
  - On Gemini the Continuity checker returned no finding with 0 reasoning tokens; on Qwen
    it thought 6,895 tokens and found one low contradiction. The measurement below shows
    the empty list was the chapter, not the checker.
- **Live: killed and resumed** (`chapter_live_killed_and_resumed`). The run is killed at a
  random moment, the database closed and opened again, and the story resumed, until it
  reaches its checkpoint:

  | Route | Shape | Kills | Branches lost | Reruns | Reached |
  | --- | --- | --- | --- | --- | --- |
  | `gemini-3.8-flash`, before the fix | 2, 4, 2 | 4, after 46–82 s | 4, counted as failed | 2 of scene 1's 3 attempts, both for the kills | P2 |
  | `gemini-3.8-flash` | 2, 4, 2 | 6, after 48–76 s | 5, each replaced | 1, the Author's own | P2, after a revision |
  | Qwen 3.8 27B, OpenRouter, DeepInfra | 3, 6, 3 | 4, after 90–339 s | 3, each replaced | none | P2: 3 scenes, 311 calls, 1.48 million tokens, $0.56, 95 min, no leak |

  The first run found the defect: two kills in a row cut both branches of scene 1 in
  flight, each attempt that lost them counted as failed, and one more would have asked the
  human about a failure no model made (docs/lessons.md §2, recorded). After the fix, scene
  2 lost three of its branches to kills and still had two to choose from; its rerun was
  the Author's ("neither branch achieved the scene's intent").
- **PR 5's finding, measured: the effort sets how much Gemini 3.x thinks, not the schema.**
  Two measurements on `gemini-3.8-flash`. The call log now keeps the provider's count of
  reasoning tokens (`response.reasoning_tokens`), shown or not.
  - *The World Master, in the simulation.* The G1 harness, 8 scenes × K = 2 × up to 6
    rounds an arm, run side by side. The World Master's effort is low or high, and its
    schema is sent to the provider or kept in the prompt only
    (`MINDFORK_FABULA_MECHANISM=fallback`).

    | Arm | Rulings | Reasoning tokens a ruling, median (p25–p75) | Mean | Intents refused | Slips, engine refusals | First attempts valid |
    | --- | --- | --- | --- | --- | --- | --- |
    | schema, low | 140 | 0 (0–0) | 1 | 18 | 0, 0 | 400/401 |
    | fallback, low | 154 | 0 (0–0) | 15 | 16 | 0, 0 | 446/446 |
    | schema, high | 145 | 1,811 (1,145–3,601) | 3,955 | 36 | 0, 0 | 422/422 |
    | fallback, high | 137 | 1,496 (926–4,226) | 4,055 | 33 | 0, 0 | 399/403 |

  - *A judging role, with known answers* (`continuity_live_tests`). The Continuity checker
    read the debug chapter with three faults written into it: a secret known too early, a
    character put elsewhere, a death the record contradicts. Five calls an arm, the same
    four arms. Every call found all three faults and nothing else. The median reasoning
    tokens were 0 and 0 at the low level, 7,037 and 7,093 at the high one.

  At the low level the World Master thought nothing with the schema or without it, and at
  the high level as much either way, its thought summaries returned (889 thousand
  characters against 927 thousand). PR 5's finding was one prompt's at one level
  (docs/lessons.md §3, recorded). The rulings showed no difference either: no slip and no
  engine refusal in any arm. What thinking changed was strictness: at the high level the
  World Master refused twice as many intents. No mechanism changes; a role's effort is the
  setting that matters, and PR 8 sets it.
  - Two incidents, each taken by the policy. One character reply of 401 ended filtered,
    on Silas, who speaks in proverbs, and its branch failed as designed. In the fallback
    arm at the high level four rulings thought up to the 16,384-token cap; the retry took
    three of them, and the fourth failed its branch.
- **Mutation check:** 22 mutants in two passes, one build each. The first pass killed 17 of
  18; the survivor, the plan asked for again when the world refuses it, got a test and
  died in the second, with the three mutants of the lost branch's replacement. The sources
  were restored and the fingerprint checked.
- **Tests:** 4668 unit tests green (+47), 322 `#[ignore]` (+3: the chapter end to end, the chapter
  killed and resumed, the checker's recall probe).
- **The gate's red on the pull request:** `rust:S7493`, blocking file calls in the live
  harnesses' async tests (the story directory made and removed, the report written), as on
  PR 4. They take `tokio::fs` now (docs/lessons.md §10, recorded again).

**Gates**: unit tests green; clippy `-D warnings`, fmt, `cyrillic_scan` (and its
`--self-test`), `link_check`, `doc_index_check`, `layer_check`, `list_scroll_check`,
`claude_md_chart --check` clean.

### Post-M9: Fabula — PR 7, stories in mindfork's data (done)

M2's second pull request (docs/fabula.md §4): where a story lives in the data directory,
what a backup does with it, and what `mindfork stats` says of it. No model is called, and
nothing the person sees changes until a story exists, which the chat tools of PR 8 and
PR 9 make. Data without stories backs up, restores and keeps its fingerprint as before.

- **Two forks the plan left open, decided by the owner (2026-11-02, docs/fabula.md §6).**
  *T4:* the plan's row said a story is "hidden and purged with its chat", while mindfork
  has no hard delete (spec §5): a deleted chat is only marked, and its `files/` and
  `workspace/` stay. A story counts as deleted while its chat is hidden, read from the
  chat and never stored twice, and its files stay. *T5:* a story names its chat by its
  path, `stories/<chat-id>/<story-id>.db`, with its call log at
  `story-logs/<chat-id>/<story-id>.db`, as `files/<chat-id>/` does. A chat may hold
  several stories, and the story's schema does not change (`Paths::story_db`,
  `Paths::story_log`).
- **Backups** (`features/backup.rs`). `stories` joins the one list of directories a
  backup packs and a restore clears. The files are packed as they lie: a story is written
  only through its actor, under the default rollback journal, and `backup` and `restore`
  hold the single-instance lock. `story-logs` is in neither half, by the owner's decision
  of 2026-10-31 (spec §15). The manifest records `story`, the newest `user_version` among
  the stories, read from each file's header as `data.db`'s is; a manifest without it reads
  as holding none, and `restore` warns when a story is newer than the build.
- **Found by the restore test: a story put back behind its call log reused a run id.**
  Runs were numbered by counting the story's own (`run1`, `run2`), and the log keys every
  call by `(run, address, attempt)`. Restoring a backup brings the story back without the
  runs it made since, while the log keeps their calls; the next run took `run2` again,
  and its first call would have hit the log's unique key. A run is now numbered past the
  last of either file, and a chapter's spend counts only the calls of runs the story knows,
  since the lost run's calls built what the restore took back (docs/lessons.md §8,
  recorded). Replay is unchanged: a replay records its calls after its run starts.
- **`mindfork stats`** (`features/data_stats.rs`). Each story is read the way `data.db`
  is, read-only from the root or from the archive's entry in memory
  (`storage::story::stats`), never through the actor, which would run the DDL and take the
  file's claim. The summary counts the stories with the part in deleted chats, chapters
  and the accepted ones, and unanswered questions, and names a story under a chat this
  copy holds no readable file of, and an unreadable story file. The size line adds the
  stories, and in the live data the call logs. None of it is printed where there is no
  story file.
- **Compared by rows, not ids alone.** A story's identity is a unit per row,
  `<table>/<key>:<digest>`, twelve hex digits of the row's SHA-256. Every table is
  append-only and ids come from position, so two copies share the rows written before they
  parted, and a side that holds every unit of the other's is ahead. Two machines answering
  one question their own way write `answer/q2` twice with two contents, and the story is
  diverged. A witness belongs to its event's unit, as the two are written together; a step
  counts once done, without its time, since a done step may be recorded again. The
  snapshot gains `stories` and `story_list`; a snapshot made before them has no list, and
  is compared all the same, with a caveat when the side that was read holds stories. The
  fingerprint hashes what it hashed before when a copy has no stories, so two machines on
  either side of the update still compare by one line.
- **The previous release on a backup that holds stories** (docs/lessons.md §8). The
  summary's fixture data root with two stories, one of them in a deleted chat, written by
  `write_a_data_root_with_stories` (`#[ignore]`). Then this build's binary and 0.23.1's,
  built from its tag, each with a data root of its own. No model is called.

  | Step | Binary | What happened |
  | --- | --- | --- |
  | `stats` | this build | 2 stories, 1 of them deleted; 3 of 4 chapters accepted; 1 question waiting; stories 408 KB, call logs 32 KB |
  | `backup` | this build | both stories packed, no call log; the manifest's `story` is 1 |
  | `stats` of that archive | 0.23.1 | read, with the stories left out, and no "newer version" note: it ignores the manifest's new field |
  | `restore` into an empty root | 0.23.1 | the stories unpacked, as files it does not know |
  | `backup` of that root | 0.23.1 | no story in it: its list of directories predates them |
  | `stats --compare` with 0.23.1's snapshot | this build | the caveat that the other side's stories were not compared |
  | `stats --compare` with 0.23.1's backup | this build | "only here: 2", and the verdict that this copy holds everything |
  | `restore` over a root holding another story | 0.23.1 | the other story stays, since its restore does not clear `stories/`; this build names it as a story without a chat |
  | `restore` of 0.23.1's backup, then of its pre-restore copy, in one second | this build | the defect below; after the fix, both copies kept, and the fingerprint back to the first one |

  A release with stories changes nothing in how 0.23.x reads the data. But a person who
  goes back to 0.23.x after writing stories gets backups without them, and a restore that
  leaves stories behind. PR 8, which makes stories, says so in its release notes.
- **Found by the round trip: a restore wrote over the copy it was restoring.** Not a
  story's defect, and older than the track. `restore` first saves the root as
  `backups/pre-restore-<date>-<time>.zip`, named to the second. Restoring that copy within
  the second it was made named the new copy the same, so the new copy, of a root without
  the stories, was written over the archive, and that was what got unpacked. The stories
  the copy was there to keep were gone. An archive the app names itself now never takes a
  name that exists (`-2`, `-3`…); the test takes every name of the next six seconds and
  failed on the old code (docs/journal/storage.md).
- **Mutation check:** 23 mutants in one pass and one build, all killed. They covered the
  run numbering and the spend's filter, both halves of a backup, the manifest's field and
  its warning, the three rules of a unit (done steps only, no time, the witnesses), the
  counts, a story deleted with its chat, one without a chat, the fingerprint both ways,
  the caveat, the verdict, the comparison's classes, the archive's stories, the call logs,
  the text's row, and a copy holding only a story. Two would have survived the tests as
  first written, the witnesses' fold and the copy holding only a story; each got a test
  before the pass. The sources were restored and the fingerprint checked. The overwrite's
  fix came after the pass; its test was seen failing on the old code first.
- **Tests:** 4687 unit tests green (+19), 323 `#[ignore]` (+1: the data root with stories).

**Gates**: unit tests green; clippy `-D warnings`, fmt, `cyrillic_scan`, `link_check`,
`doc_index_check`, `layer_check`, `list_scroll_check`, `claude_md_chart --check` clean.

### Post-M9: Fabula — PR 8, the story run in the background (done)

M2's third pull request (docs/fabula.md §4): a story's run goes out as the third
background payload, beside the sub-agent's and the dialogue's, and lands in its chat.
Nothing starts one yet — `story_create` and `story_continue` are PR 9 — so the payload is
built and proved on the mock engine, and the one tool a person can reach is `story_status`,
behind `tools.story`, off by default.

- **Two forks the plan left open, decided by the owner (2026-11-02, docs/fabula.md §6).**
  *T6:* T1 named the budget's unit, not its amount. A chapter is bounded by 2,000,000
  tokens, about twice the 718,865 a chapter of three scenes took on Qwen 3.8 27B in PR 6,
  and by no dollar cap unless one is set: Anthropic reports no cost, so a default in
  dollars would bound one provider only. *T7:* a story run takes a background slot, as a
  dialogue does, so one cap bounds the background load; a story has one run at a time
  either way.
- **The payload** (`app/orchestrator/story_runs.rs`, `RunSpec::Story`). It shares every
  later step with the other two: the seat, the record on the call that started it, the
  landing by id, the notification and its wake, `/subagents stop`, the tasks screen, the
  cancel on a chat's deletion. What runs is `features::story::run::run`: the story opened
  from `stories/<chat-id>/<story-id>.db`, the engine of PR 6 on the turn's own backend, every
  role on the one model with the sampling G1 and PR 6 measured (characters muted, the World
  Master and the reviewers at a low effort, the Author at a medium one with room for a
  chapter), each call under the app's session budget (`EngineAdapter::with_sessions`, a
  permit for the stream's length, priced as a run's request is). A managed server with a
  draft model is put on the fallback (`needs_fallback`). The engine reports no rounds, so
  the run's mirror stays its placeholder until it lands.
- **The time limit cancels the calls, not the run.** `tools.story_run_timeout_secs` (6 h,
  T2) fires a child of the run's token that only the calls hold; the engine stops at the
  step in flight and records it, exactly as for a stop, and the landing reads it as timed
  out. Dropping the future instead would have left the step unrecorded.
- **One run per story** (`features/story/board.rs`). A run claims its story on a board the
  orchestrator holds before it opens the files, and a second is refused with the first one's
  address; the run writes the address of each call it makes there, which `story_status`
  reports and a quit names. A story has one actor at a time, so this is also what keeps two
  runs from opening one file.
- **The landing.** The chapter a checkpoint asks about lands as an attachment of the chat,
  `<story> — chapter <n>.md`, its source one per chapter, so a revision replaces the version
  before it (T3). Its mode is decided at the landing, against the chat's budget as it is
  then, without the version it replaces: the run cannot see the chat. The notification, in
  the profile's language under the task prefix, carries the summary, the Author's note on
  each scene kept (a rerun's request is no pick), the assumptions, the question and what
  the run cost. A run that stops short says where and what continuing does: stopped, timed
  out, its budget spent, a call that gave nothing usable, a story that could not open. A
  spent budget reads as *failed* in the list; the landing says what was spent.
- **Found while wiring the landing: a story's record would have been retitled at every
  landing.** A background run is auto-titled when it lands, as a turn's child is, which for
  a story means a model call per chapter renaming a record named after its story. A story's
  run is not auto-titled.
- **A quit** lands the run at once from its mirror, cancelled, with a line naming the call it
  was in, read from the board; the task's own end, arriving after, lands nothing twice.
- **`story_status`** (`features/tools/story.rs`) reads every story of the chat read-only
  (`storage::story::status`), through an actor over read-only connections, beside a run
  that holds the story's own: no DDL, no claim, a busy timeout over a write in flight, an
  empty call log when the log is gone. The reads are the actor's own, so the status and
  the run cannot read a row differently.
- **`CHAT_SCHEMA` 7 → 8**, a stamp: a run's `kind` may read `story`, a strict enum, and one
  such run is enough for 0.23.1 to skip a chat in silence. The golden v7 fixture holds a
  background dialogue and a generated picture. The settings take no step: the switch, the
  time limit and the budget are `tools` fields an older reader ignores.
- **The modules' `allow(dead_code)` stays, with a new reason.** It was to go with PR 8,
  the first production consumer. The run reaches the engine from production now, but taking
  the allow away showed 75 items the run does not reach: the canon's import and checks, the
  answer's resolution (`story_create`, `story_continue`, PR 9), the replay and the
  harnesses' scenes (M3). The allow goes with the last of them; the comments say so.
- **The previous release on schema 8** (AGENTS.md §6). 0.23.1, built from its tag
  in PR 7, and this build, each from a directory of its own with its data beside it; the
  seed is 0.23.1's own root from PR 7 (two chats at 7, the stories PR 7 restored), its
  fixture's deliberately broken chat files taken out. `import` of a bare
  `mindfork-import` document is the start-up path run headless. No model is called.

  | Step | Binary | What happened |
  | --- | --- | --- |
  | `import` on its own root | 0.23.1 | exit 0; fingerprint `ce8df1c0f34ed851` |
  | `import` on that root | this build | exit 0; both chats stamped 8; one `pre-migrate-*.zip` whose manifest says `chat 7` |
  | `import` on the migrated root | 0.23.1 | exit 1: *"…created by a newer version of mindfork (schema 8, supported 7). Update the app or restore a backup."*; every file as it was |
  | `restore` of the pre-migration backup into an empty root | 0.23.1 | no warning, exit 0; chats at 7, fingerprint `ce8df1c0f34ed851` again; then it starts (exit 0) |
  | `backup` of the migrated root | this build | the manifest says `chat 8` |
  | `restore` of that backup | 0.23.1 | done, with the warning *"…(chats/*.json: schema 8, supported 7)…"* |
  | a chat with a `kind: story` run, stamped 8 | 0.23.1 | refused as above, naming the file |
  | the same chat stamped 7, as with no step | 0.23.1 | `import` passes (exit 0): nothing refuses it, and the chat would be skipped in silence where the list is read, which is what the stamp is for |
  | the golden v7 file | 0.23.1, then this build | taken (exit 0); stepped to 8 (exit 0) |

  *Found on the way, older than the track:* a chat file that is valid JSON but no chat —
  the fixture's `{}` — stops every start, 0.23.1's included, with *"Migrating … produced
  an unparseable result … This is a migration bug; please report it"*, where a damaged
  chat file is meant to be skipped (spec §12.2). Not fixed here; fixed by the next
  pull request (docs/journal/storage.md, "a chat file that holds no chat").
- **Live, on a model** (`story_run_live`, `#[ignore]`): the fixture story, its canon
  approved, run three times over through `run` with the app's session budget at one
  permit — to a chapter budget of 40,000 tokens, to a stop by its token three seconds into
  a call, and to a time limit of 20 seconds — each resuming where the one before stopped.

  | Route | Budget run | Stop | Time limit |
  | --- | --- | --- | --- |
  | Claude Haiku 5.5 | 10 calls, 46,484 tokens, stopped at `ch1/sc1/r0` | 2 calls; the board showed `…/k1/round1/ines/propose` | 7 calls, timed out at `ch1/sc1/r0` |
  | Qwen 3.8 27B (OpenRouter, DeepInfra) | 17 calls, 41,741 tokens, $0.023 | 1 call, cut | 3 calls, 2,952 tokens, $0.0013 |

  Every permit came back after every run, and the board was empty after the last. A budget
  is checked before each call, so a chapter passes it by the last call's tokens (46,484 of
  40,000 on Haiku). The first run's landing found two things, both fixed: the chapter's
  assumptions rode on every stop, though they belong beside the checkpoint's question
  (spec §14), so a stop now carries only where it stopped; and a cost under a cent read
  as $0.00, so it is now shown to the hundredth of a cent.
- **Mutation check:** 31 mutants in one pass and one build: 30 killed, two of them by a
  hang the pass's own timeout caught (no cancel at the deadline, no step on the board). The survivor was the turn's own gate — `tools.story` read
  into the request's tools — which no test checked above `effective_tool_ids`; a turn now
  shows `story_status` offered only with the switch on, and was seen failing with the
  gate forced on. The sources were restored and the fingerprint checked. Before the pass,
  four tests were added where a mutant would have lived: a rerun's request among the picks,
  the version a pause asks about among two, the replaced version left out of the budget,
  and the record's tokens and outcomes.
- **Tests:** 4723 unit tests green (+36), 324 `#[ignore]` (+1: `story_run_live`).

**Gates**: unit tests green; clippy `-D warnings`, fmt, `cyrillic_scan`, `link_check`,
`doc_index_check`, `layer_check`, `list_scroll_check`, `claude_md_chart --check` clean;
the settings screenshots regenerated (the Tools section's count, the profile's new tool).

### Post-M9: Fabula — PR 9, the story driven from the chat; Gate G2 GO (done)

M2's fourth pull request (docs/fabula.md §4): `story_create` and `story_continue`, so a
person makes a story from the chat and every answer it waits for is their own words. With it
the engine is reachable end to end from a chat turn, behind `tools.story`, off by default.

- **Forks decided by the owner (2026-11-02, docs/fabula.md §6).** *T8:* the named scene
  rerun at P2 needs the main line to step back, with the scenes after it, the chapter's text
  and its review made again under addresses of their own; it is PR 10, and until then a
  request to replay a scene reads as `change` and revises the text. *T9:* G2 on Claude
  Sonnet 5.5. *T10:* G2 in Russian, run by Claude Code through the orchestrator with the
  person scripted. *T11* (during G2): the chapter's default budget is 6,000,000 tokens.
- **`story_create`** (`features/tools/story/conversation.rs`, `features/story/conversation.rs`)
  is an ordinary tool. The canon arrives as an object (or as a JSON string some models send)
  and `Canon`'s own parse refuses a misspelt field by name. Its checks run first; the reply
  schemas built on its keys are held to the dialect only once they pass, since a repeated key
  breaks every schema built on it and the model needs the cause, not twelve echoes of it
  (found live). A canon that passes is stored under an id made from the title — its words
  lowercased, letters and digits of any script, `-` between, at most 40 characters, Windows'
  device names avoided, `-2` when taken — and P1 opens with no model call: the engine's
  first step asks for the canon's approval before anything, so `advance` runs over a port
  that refuses every call. Called again with `story`, it replaces the draft while the canon
  is unapproved; the id must be one of the chat's own files, so an argument never names a
  path of its choosing (`../<other chat>/x` reached another chat's story before the check,
  shown by a mutant).
- **`story_continue`** is loop-executed, as `start_dialogue` is (`app/orchestrator/
  generation_story.rs`, a child module of `generation.rs` so it reaches the loop's parts).
  - *The rows.* When the turn begins, the orchestrator reads every story of the chat that no
    run holds and, for each question waiting, takes the person's rows after it was put to
    them: after the canon's last change for P1, after the landing that carried it for P2 and
    P3 (the newest notification of a run on that story's `story_continue` records), never
    before the question's own row. Notification rows are never user rows, and attachments
    and tool results are not rows at all, so a sketch that says "approved" approves nothing.
  - *The reading* (`features/story/answer.rs`) is one schema-constrained call of the chat's
    model, a role of its own (`Role::Listener`, low effort): the question in the engine's
    words, what the person was shown (the canon in brief, the chapter's summary), and their
    rows quoted as data, oldest first, a voice note marked as the recogniser's. It returns
    `{answers, changes, decision}`, the evidence before the decision; P1 and P2 accept
    approve, change or unclear, P3 answer or unclear. Unclear, a decision the question does
    not accept, and a change that names no change resolve nothing.
  - *The refusals.* A turn that answers no message of the person's — a woken one, whose
    `UserMessageTimes.answering` is empty — and rows that read as unclear are refused with
    "the user has not answered the story's question yet; ask them", as is a call in the very
    turn that asked. The slot is taken before the reading, so a capped background pays for
    no reading it cannot act on, and given back on every refusal.
  - *The resolution* is the answer row: the decision, the notes for the Author — the
    person's changes and words, and apart from them the assistant's `notes` marked as its
    own — and the evidence: each message's id, time and quote (2,000 characters at most),
    the reading itself. An approved canon opens chapter 1, an accepted chapter the next, a
    change reaches the Author as the person's notes, and a change to the canon starts no
    run: the assistant revises the draft and asks again. Accepting the last chapter finishes
    the story with no run. With no question waiting it resumes a stopped run, unless the
    chapter's budget is spent; a finished story starts nothing.
  - *The claim.* The story is claimed on the board for the run before its files are opened
    and kept for it; the run accepts a claim in its own name, so nothing writes the story
    between the answer and the run.
- **The reminder line.** While a question waits, the request's last user-side message — the
  one the turn answers, or a woken turn's notification — carries one line naming it, in the
  request's copy only; every earlier message stays as the last request sent it.
- **Both tools are withheld from a sub-agent**: their answers are the person's own.
- **The modules' `allow(dead_code)` stays for M3**: 49 items remain unreached (the replay,
  the harnesses' scenes, the leak audit, the export), down from 75.
- **Live, the gate models as the chat's model** (`story_chat_live_canon_and_approval`,
  `#[ignore]`): English sketches attached, "build the canon and create the story", then the
  approval turn played five times from the stored P1, each trial's run stopped after a
  second.

  | Model (route) | Canon stored after | Premature `story_continue` | Approval turn: called / claimed without a call / neither |
  | --- | --- | --- | --- |
  | Claude Haiku 5.5 | 3 calls | 0 | 3 / 0 / 0 (3 trials) |
  | Qwen 3.8 27B (OpenRouter, DeepInfra) | 3 calls | 0 | 5 / 0 / 0 |
  | Gemma 4 31B (OpenRouter, DeepInfra fp8) | 2 calls | 0 | 5 / 0 / 0 |

  Every refused canon was fixed on the next call, and the refusals were what the models
  tripped on: an item listed both where it lies and in a hand (Haiku, Qwen), `voice` inside
  `start` (Gemma), the title inside the canon (Qwen), and after a refusal a call naming
  `story` for a story that was never stored (Haiku). Fixed: the duplicate item says it
  starts in two places, the dialect is checked only after the canon's own checks, a new
  story's refusal says it has no id yet, and a title inside the canon is taken as the
  title.
- **A chapter on a gate model** (`story_chat_live_story`, Qwen 3.8 27B, English): the canon
  composed, approved in the person's words, and the chapter's run out — 320 calls, then
  stopped by the default budget, 2,017,368 tokens of 2,000,000 ($0.55), at the third of four
  scenes. It landed as it should: the woken turn called `story_status`, said the budget was
  spent and where to raise it, and resumed when asked. The default had come from PR 6's
  harness chapter (three scenes, six rounds, 718,865 tokens); the app's own shape is four
  scenes of up to eight rounds. With G2's figure it made T11.
- **Gate G2** (`story_chat_live_story`, Claude Sonnet 5.5, a Russian profile; T9, T10): sketches
  written for the check, "The Last Ferry", attached — a northern island, a keeper who
  did not light the lamp the night her brother drowned, an inspector come to close it, a
  shopkeeper who hid the logbook — and the person scripted by where the story stood. The
  assistant composed the canon in two calls (the first had `routes` inside `world`), showed
  it, and called `story_continue` on the approval; every landing woke a turn that put the
  question in its own words, and every acceptance was read from the person's message.

  | | Calls | Prompt tokens | Reply tokens | Cost (list price) |
  | --- | --- | --- | --- | --- |
  | Chapter 1 (four scenes, the draft accepted) | 492 | 5,629,710 | 169,223 | $12.95 |
  | Chapter 2 (four scenes, the second revision accepted) | 380 | 8,383,836 | 145,259 | $18.22 |
  | The whole story, readings included | 875 | 14,017,889 | 314,563 | $31.18 |

  The story is 2,696 words (1,606 and 1,090), finished, both chapters accepted. **G2 GO**
  (2026-11-03): the owner read it with the chat's transcript and passed the gate, and two
  other models' reviews — Gemini 4 Argon, Claude Fable 5.1 at its maximum effort — agreed:
  strong, spare prose with its things, numbers and secrets kept, and the faults in its deeper
  logic — clues planted and never paid, a timeline and a kinship that do not add up, every
  conflict resolved at once at no cost, lines that report the scene's state, and chapter 1's
  summary contradicting itself. Their proposals are in docs/fabula.md, *What G2's readers
  found*, undecided. Three things the run itself found:
  - *The default budget stopped the first chapter in its second scene* (2,010,278 tokens,
    238 calls), and the script's "go on" restarted a run seven times, each stopping before
    its first call. `story_continue` now refuses a chapter whose budget is spent, says what
    was spent and where to raise it, and keeps an answer read on the way; with the Qwen run
    the owner raised the default to 6,000,000 (T11). G2 ran on at 8,000,000 for chapter 1;
    chapter 2's projection passed what the owner had approved, so the run was stopped, the
    owner chose to finish it, and it resumed at 14,000,000 from the step it was at.
  - *Accepting the last chapter started a run* that could only find the story finished, and
    the started line said the story went on. `story_continue` now records the acceptance
    and says the story is finished, starting nothing.
  - *A chapter's cost grows with the story.* No call read Anthropic's cache — nothing asks it
    to — and a character's call carries every reflection it has written since the story
    began: the average character prompt was 5,801 tokens in chapter 1's first scene, 14,359
    in its fourth and 17,879 in chapter 2's first; the World Master's 8,209, 20,409 and
    24,951. A memory window and the cache breakpoints are the levers (docs/fabula.md §5);
    the roadmap carries them.
- **Mutation check:** 27 mutants in one pass and one build: 24 killed. The survivors got a
  test each, seen failing with the mutant: the dialect's echo of a canon problem, a woken
  turn's `story_continue` (driven through the orchestrator: the person answers, a background
  sub-agent lands, and the woken turn's call is refused with no reading made), and a path in
  `story` reaching another chat's story. The sources were restored and the fingerprint
  checked. The two fixes G2 found came after the pass, each with its own test.
- **Tests:** 4756 unit tests green (+27 over 4729), 326 `#[ignore]` (+2: the two live
  checks).

### Post-M9: Fabula — PR 10, a scene played again at the checkpoint (done)

M2's last pull request (docs/fabula.md §4): P2's fourth answer, *rerun a named scene*, which PR 6
left out because the main line has to step back, and PR 9 read as a revision until it could.

- **Forks decided by the owner (2026-11-03, docs/fabula.md §6).** *T12:* a take has the
  chapter's budget of its own, counting the chapter's calls in the runs after it began. *T13:*
  the live check on Qwen 3.8 27B through OpenRouter. Both as recommended.
- **The take.** Every store is append-only and every id positional, and `scene` is unique by
  (chapter, scene, rerun), so a scene played again cannot reuse its number: a take's
  attempts continue each scene's rerun count. A new table, `take` (no schema step: a file an
  earlier build wrote gains it on open), keyed by the P2 question whose answer asked for it,
  records what the take stands on: the scenes carried (those the last take kept before the
  one named), the ones it replaces, each scene's **floor** — one past the attempts it had —
  the take's first version, and the last run before it. The answer stays `change`; the take's
  row is what says the change is a rerun, written in the answer's transaction, refused unless
  the checkpoint is the chapter's latest version's and the scene one it keeps. Two things
  were rejected: a `rerun` decision in `answer`, which would have meant a new CHECK constraint
  and a schema step, and attempts told apart by insertion order, since `VACUUM INTO` (how a
  backup takes the file) may renumber a table's rowids.
- **The main line runs through each chapter's latest take** (`kept_scenes`, `main_line_tip`
  in the actor): the carried scenes, then from the take's first scene each scene's kept
  attempt from its floor on, until a scene has none. With no take it is what the old query
  gave, the latest pick, so every harness and stored story reads as before.
- **The engine** (`chapter.rs`) runs the latest take: the carried scenes' traces read back,
  the named scene planned again with the person's request and its old setup in the prompt
  (`Again::Asked`) and no way to end the chapter there, every scene after it planned again,
  the draft at `ch1/v<first>/write`, revisions counted within the take. A take's plans are
  addressed `ch1/t2/sc3/plan`: the first take's step there may have *ended* the chapter (a
  done step with no row), and the take must ask again — which the live run did. Every request
  the person made at the chapter's checkpoints reaches the take's prompts through the
  chapter's context, and the pick's too. P3 inside a take counts from the floor.
- **The reading** (`answer.rs`) is shown the chapter's scenes by number — where, with whom,
  the Author's note on keeping each — and at P2 takes a fourth decision, `rerun`, with a
  `scene` among the chapter's numbers (an enum, so a number it does not keep cannot be
  named). What happens is a rerun, how it is told a revision; a rerun that names no scene is
  unclear. The landing lists the scenes by number, and `story_continue`'s description, the
  question and the started line say what a rerun does.
- **Tests** (`take_tests.rs`, a sibling of the chapter's on its scripted port): scene 2 of two
  played again — scene 1 carried, `ch1/sc2/r1` forked from scene 1's branch, scene 3 planned
  again, the take's own draft reviewed and summarized, accepted and finished; a take of a take
  from scene 1, forked from genesis, both requests in its prompts; the refusals; the budget;
  a take killed at each of its calls and resumed to its checkpoint; and a story through a
  take replayed from what was recorded at each address, every address asked once and every
  prompt the same. **Mutation check:** ten mutants on the take, one build each: nine killed;
  the survivor — the carried scenes dropped from the engine's own list, so the take's draft
  would be written without scene 1 — got the assertion that kills it.
- **Live, Qwen 3.8 27B** (`story_chat_live_story` with `MINDFORK_FABULA_RERUN=2`; OpenRouter,
  DeepInfra, a Russian profile, the sketches of G2): the canon, its approval, chapter 1 to its
  checkpoint, *"Play scene 2 again, please: this time the secret must not come out in it"* (in Russian),
  and the take to its own checkpoint.

  | | Calls | Prompt tokens | Reply tokens | Cost |
  | --- | --- | --- | --- | --- |
  | Chapter 1, the first take (three scenes, v1–v3) | 470 | 3,405,583 | 403,590 | $1.18 |
  | The take from scene 2 (three scenes played, v4–v6) | 755 | 7,062,556 | 474,517 | $1.82 |
  | The whole run, readings included | 1,227 | 10,470,450 | 878,310 | $3.00 |

  The landing listed the three scenes by number and the woken assistant offered to play one
  again; the reading took the request as a rerun of scene 2 with one call. The take carried
  scene 1 and replaced scenes 2 and 3, from floors 1 and 1. Scene 2 was played as `r1`, forked
  from scene 1's kept branch; scene 3 was planned again, played as `r1`, and asked for again by
  the Author itself (`r2`, its reruns counted from the take's floor); and where the first take's
  Author had ended the chapter at scene 4's plan, the take's planned and played a fourth scene.
  The chapter was drafted (v4) and revised twice (v5, v6) — the take's own two — and the
  checkpoint asks about v6, 1,191 words against the first take's 1,133. The keeper's
  confession, spoken in the first take's scene 2 ("I did not light the lamp… it is my fault"),
  is not spoken in the take's, as asked. Things the run found:
  - *The take cost twice the first making*, 7.5 million tokens against 3.8: a fourth scene,
    the Author's own rerun of scene 3, and prompts that grow with the record (§5's finding).
    The owner raised the run's budget from 6,000,000 to 8,000,000 when the take neared it,
    and approved the run's cost beyond the estimate ($3.00 against about $1.50).
  - *The Continuity checker's reply was cut at its cap* (16,384 tokens) four times on Qwen, and
    twice running on the first take's draft, which stopped the run; a later attempt fitted.
    The reviewers' cap is a candidate for a model that thinks this long.
  - *A woken turn resumed a stopped run unasked.* After the stop the woken assistant called
    `story_continue` by itself, which resumed with no question waiting, and did again as the
    test's process went down. Nothing the person had to decide was decided for them, but the
    calls were spent on the assistant's word — and the code let the same happen after a stop
    the person made with `F6`, and loop on a step that fails every time until the budget
    was spent. The owner chose (T14 (a)): a woken turn resumes nothing (`ContinueRefusal::
    NotAsked`), the woken assistant says where and why the story stopped and asks, and the
    stops' landings say that going on is the user's to ask. A mutant that lets the woken
    resume through is killed.
  - *The harness gave up as the run's time limit fell*: it waited the limit itself, so the
    take's run (over three hours at about 22 tokens a second) was cut by the test's own exit,
    and the story resumed from the step it was at, the two branches lost to the cuts each
    replaced under the next index, the scene's attempt kept. The wait is now the limit plus
    fifteen minutes.
- **Tests:** 4767 unit tests green (+11 over 4756), 326 `#[ignore]`.

### Post-M9: Fabula — PR 11, the story's register (done)

The first of the two pull requests that act on what G2's readers found (docs/fabula.md §4,
*Before M3*): the bookkeeping they found missing above a world state that held.

- **Forks decided by the owner (2026-11-04, docs/fabula.md §6).** *T15:* two pull requests
  before M3, the bookkeeping first, then the craft. *T16:* a thread still open in the arc's
  last chapter is reported, and the Author pays it off or keeps it open with its reason, which
  the last checkpoint shows. *T17:* the scenes' and the lines' rules in the prompts, with the
  staging pass marking breaches (PR 12). *T18:* known-answer probes on G2's own chapters, a
  Qwen story in each pull request, and G2 again on Claude Sonnet 5.5 once both have landed.
  All as recommended.
- **Two causes, in the code.** The summary was the Author's, with the whole canon in its
  system prompt: chapter 1's put the shopkeeper's secret hiding place (the canon: she hid the
  brother's journal in the chapel) into a chapter that shows her carrying something *out* of
  it. And the Continuity checker read a chapter against its own scenes' record alone, so no
  role ever held a chapter to a published one: the logbook's line "in another hand" became an
  "empty" line a chapter later unread by anyone.
- **The Canon keeper's first part** (`keeper.rs`, `Role::CanonKeeper`, which the enum and the
  sampling had held since PR 8). At a checkpoint, from the version's text alone, it writes the
  **register**: the threads the chapter opens (what the text leaves its reader to wonder), the
  open ones it pays off, and the facts of time, age and kinship it states, each with its quote
  and who states it — the narration or a character, who may be lying. The ids are the
  engine's, by position (`ch1.thread2`, `ch1.fact3`), and a payoff names a thread by an enum of
  the open ones, built per call and absent when none is open. Then the **summary**, from the
  text alone; its check (`…/summary-check`) names each sentence the text does not bear out;
  with one, the summary is written once more against them (`…/summary-again`). Nothing is
  stored until it is final, so a resume asks the few calls again.
- **Who reads it.** The register so far (`SoFar`: the accepted chapters', the threads paid off
  closed) is in every Author prompt that reads the story so far — the outline, the plans, the
  draft, the revisions — with a line in the arc's last chapter to pay each thread off or leave
  it open on purpose; the Author's rules gain two, threads as promises and facts that add up.
  A chapter is checked against it by **the keeper's own call** (`…/register-check`, on the
  chapter's text and the register alone) beside the Continuity checker's, from chapter 2 —
  chapter 1 has nothing published to be held to — its findings written in the checker's batch: one batch, so a resume asks both again or neither, and no schema step
  for a third reviewer. In the arc's last chapter that check may report an `open_thread`,
  anchored by the thread's id; a finding naming a thread the story does not have is dropped.
  The triage's critiques are now in the human's language, and the last chapter's landing lists
  the threads left open, each with the critique of a note the Author decided against (an
  accepted note gives no reason to leave it).
- **The register was first given to the Continuity checker**, beside the record. The probe
  below found that on Qwen 3.8 27B the checker, with every event of the chapter before it,
  held nothing of the register (0 of 3 on each known fault), where Claude Sonnet 5.5 found them;
  the check moved to a call of its own, and the checker's prompt is main's again — the prompt
  whose recall of planted faults PR 6 measured. Side by side (below) the call of its own found
  no more on Qwen and about as much on Sonnet, a run fewer on the log's line and two faults of
  high severity the checker had found low or not at all; it reads what it is for and nothing
  else.
- **Storage.** `chapter_register` (one JSON body per version, append-only, in `stats`' units):
  a table of its own, so no schema step; a story begun before it is given its registers on the
  way, from the accepted text — tested on a file whose table was dropped and opened again.
- **The reviewers' and the keeper's reply cap** goes from 16,384 to 32,000 tokens, the Author's,
  after PR 10 found the Continuity checker's cut on Qwen four times.
- **Tests.** `keeper_tests.rs` (a reply made a register, the register so far, the schema's
  enum, the prompts with nothing of the canon in them), `register_tests.rs` on the scripted
  port (the keeper on the text alone and the next chapter reading it; a thread kept open with
  its reason; a summary written again; a story from before the register), the landing's list
  in `run_tests`, its rendering, the cap. The runs killed at every call, chapter and take,
  cover the new calls unchanged. **Mutation check:** 24 mutants in three passes, one build each:
  all killed.
- **Live: known answers on G2's own story** (`register_live_tests`, `MINDFORK_FABULA_G2_STORY`;
  three runs a probe, every role sampled as the app samples it; the checker given the register
  in every column, as first built). The faults are the readers':
  the old summary's chapel; chapter 1's "another hand" in the log, the footsteps on the path,
  the father lost "a few years ago"; chapter 2's line gone "empty", the twenty-eight years and
  the pre-war cross against that "few years", the footsteps never anyone's. Counted by hand
  where a word stem misled (the stem of "step" is in "she stepped", the word for a hand on the
  notebook's cover):

  | Of 3 runs | Sonnet, first wording | Sonnet, sharper | Sonnet, both checks | Qwen, sharper | Qwen, both checks |
  | --- | --- | --- | --- | --- | --- |
  | The old summary's chapel named by the check | 3 | 3 | 3 | 3 | 3 |
  | A new summary keeps out of the chapel | 3 | 3 | 3 | 3 | 3 |
  | Ch. 1's "another hand" a thread | 0 | 3 | 3 | 1 | 0 |
  | Ch. 1's footsteps a thread | 3 | 3 | 3 | 3 | 3 |
  | Ch. 1's "a few years ago" a fact | 3 | 3 | 3 | 3 | 3 |
  | Ch. 2: the line, the checker with the register | 0 | 2 | 2 | 0 | 0 |
  | Ch. 2: the years, the checker with the register | 2 | 2 | 2 | 0 | 0 |
  | Ch. 2: the footsteps open, the checker with the register | 3 | 3 | 3 | 0 | 0 |
  | Ch. 2: the line, the keeper's check | — | — | 1 | — | 0 |
  | Ch. 2: the years, the keeper's check | — | — | 2 | — | 0 |
  | Ch. 2: the footsteps open, the keeper's check | — | — | 3 | — | 0 |

  A run on Sonnet was 21–24 calls and 187,000–207,000 tokens; on Qwen 22–24 calls, 181,000–
  221,000 tokens, $0.14–0.19, over half an hour. Things it found:
  - *The first wording missed the log.* The keeper read Vera's "my hands would not obey" as
    the answer to the other hand, and the checker's instruction held only statements of time,
    age and kinship to the register, though it held the line as a fact. Two changes: an excuse
    or a half-truth the text leaves in doubt answers nothing, and a chapter is held to every
    listed fact and thread. They were found on this story, so the probe that measures them is
    the one they were tuned on; both are general rules.
  - *On Sonnet the keeper's check found what the readers found and the checker had not*: in one
    run "Kolka Rybakov, son of Marya" against Galina's own schooling, high; in two the
    brother's letter, which sees the tower dark and writes that the light burns steady, high.
  - *Most findings come at low severity*, so the triage decides on them, and a low note alone
    starts no revision.
  - *The footsteps*: the keeper reading chapter 2 calls them paid off — by Galina's and Vera's
    steps at the graves next morning — on Sonnet in 2 runs of 3 and on Qwen in 3; the readers
    did not. A judgement the register makes, and the Author reads.
  - *On Qwen 3.8 27B neither check found these faults*, at the reviewers' low effort; the
    keeper's check used the register (the ferry's three days against "two days to the ferry",
    the unlit night against "every evening for three years"), but not for these. The keeper's
    reading of the text — the summary, the threads, the facts — held on both models.
- **Live: a two-chapter story on Qwen** (`story_chat_live_story`, Qwen 3.8 27B on OpenRouter
  pinned to DeepInfra, a Russian profile, G2's sketches, the default budget, the harness's
  three-hour time limit — the app's is six):
  "The Last Ferry" again, from the sketches to the last acceptance in the chat — 2,669 words,
  both chapters accepted.

  | | Calls | Prompt tokens | Reply tokens | Cost |
  | --- | --- | --- | --- | --- |
  | Chapter 1 (four scenes, two revisions, v3 accepted) | 602 | 4,924,110 | 434,302 | $1.50 |
  | Chapter 2 (four scenes, v1 accepted) | 279 | 4,882,258 | 231,066 | $1.05 |
  | The whole story, readings included | 884 | 9,810,333 | 665,748 | $2.55 |

  Things it found:
  - *The harness's time limit stopped chapter 1 in its fourth scene*, after three hours and 492
    calls: the calls ran one at a time (their latencies sum to the wall time). The woken
    assistant said where and why the story had stopped and asked, resuming nothing (T14, seen
    live); asked, the run went on from the step it was at and reached the checkpoint 52 minutes
    later. The harness counted that stop as a chapter and ended a two-chapter script at chapter
    1: only a chapter's checkpoint counts now, and the story was taken up where it stood.
  - *The keeper's reading held.* Chapter 1's register: seven threads (the father who vanished,
    the cross with no name, the brother Galina tells Timur not to know, the order she calls dead
    paper, what lies in her storeroom, the island blind for three days, whether anyone can
    leave) and five facts. Its summary's check named "until the storm dies down" against a text
    whose wind is "not yet a storm", and the summary was written again. Chapter 2's outline, plans
    and draft carried the register and the last chapter's line, and the chapter paid off five of
    the seven: the cross is the father's grave, the brother is Artyom, the storeroom held his
    logbook, the order is answered by Timur's request to cancel it. Its summary's check found
    nothing.
  - *On Qwen the keeper's check reported no thread left open.* With the seven threads before it
    and the line asking for each one unpaid, it reported none, and one finding that was wrong (a
    feminine noun Timur writes of Vera, read as his own), which the Author rejected for the right
    reason. The landing listed the four threads the register still held open — chapter 1's two
    that are consequences more than clues, and two that chapter 2 opened (which year the cross
    bears, why he came as an inspector) — without reasons, since no note was decided; the woken
    assistant named three of them to the person. The list is the register's, so it reaches the
    person whether or not the check reports, as T16 meant.
  - *In chapter 1 the check had nothing to hold the chapter to*, and was asked on both reviewed
    versions all the same; it found the order put into the pocket of a robe Timur was wearing on
    the headland — the Continuity checker's work. It is asked from chapter 2 on.
  - *A kinship misread, not in the text.* The Reader took Artyom, Vera's brother, drowned three
    years ago, for Timur's father under the twenty-year cross, and the Author rejected the note
    as a link left implicit on purpose. The text and the register keep them apart (Artyom died
    three years ago; the father waited twenty), so nothing reached the story — but the Author's
    own reading was wrong, the kind of referent PR 12's staging pass is for.
- **Tests:** 4778 unit tests green (+11 over 4767), 327 `#[ignore]` (+1, the probe).

### Post-M9: Fabula — PR 12, the craft of the scenes and the lines (done)

The second of the two pull requests that act on what G2's readers found (docs/fabula.md §4,
*Before M3*): the faults of the story's deeper logic that the register does not reach —
every conflict resolved at once at no cost, lines that report the scene's state or open on
thanks, a kinship no reviewer could place.

- **The fork (T17 (a), the owner's, 2026-11-04):** the rules in the Author's and the
  characters' prompts, a scene's intent naming its collision and its price, and a staging
  pass that marks a line announcing a plan or opening on thanks. Not fields of their own in
  the plan, and not a fifth measure in the pick (T17 (c), which would change spec §7). The
  agent's choices inside it, below, are open to the owner's review.
- **The Author's rules** gain two. A scene is a collision: its intent names whose goals
  collide in it and what it may cost someone; no conflict ends without a price — a
  confession, a pardon or a win costs someone something they hold — the pressure set so that
  it does, the version where it is paid the one kept. And the reveals are paced: a secret is
  spent where it costs its keeper most, one at a time. The intent's description in the plan's
  schema says the same, so the plan that writes it reads it twice.
- **A line is a deed**, in the characters' rules and the prose's: a plan or a boundary acted
  on rather than announced, no reply that opens on thanks, apology or agreement the moment
  does not call for, a want shown in what is done. The prose may cut such a line of the record
  or show it in a deed, as long as what happened stays — the rule that the prose never
  contradicts the record holds. The World Master's rules are unchanged: it rules, it does not
  speak.
- **The staging pass** (`staging.rs`, `…/staging`, `Role::Continuity`'s sampling) reads each
  version's text for who is where and holds what, to whom a pronoun, a "who" or a tie of kin
  points, a line that announces and a reply that opens on thanks; a line that does what the
  moment calls for is no fault. It runs on every chapter, the first included, beside the
  Continuity checker and the keeper's check, in one batch: a run killed at it asks the
  checker's batch again and nothing the run stored (`craft_tests.rs`). Its notes are four kinds
  of their own (`staging`, `referent`, `announced`, `thanks`), triaged as the checker's; a note
  kind is a stored name in a free-text column, so no schema step. Severity: high when a reader
  would take a person, a place, a thing or a kin for the wrong one, or when one fault of the
  lines runs through the chapter.
- **Not the text alone, quite.** Built on the chapter's text alone, the first probe on Sonnet
  gave a high note on chapter 2's "let us go to Andrey" — "Andrey is never mentioned before",
  when chapter 1 named him. The pass now reads the published chapters' summaries first, the
  keeper's from their text alone (PR 11): a name they carry is known to the reader. With them
  the note was gone and the known answers held. No canon and no record reach it.
- **Tried and not kept: the ties of kin first.** A list in the reply before the findings —
  each tie of kin as its line states it, every pronoun put to its person — so the reply would
  hold the lines to each other before it judged them (property order is generation order). On
  Sonnet 5.5 it changed nothing (3 of 3 on each known answer, as without it). On Qwen 3.8 27B
  the ties were right — "Nikolai is Marya's son; Marya left with Nikolai", "Timur's mother
  took him to the mainland" — and the pass still did not hold them against each other: Marya's
  line in 1 run of 3 (0 without), a reply opening on thanks in 0 (2 without), two findings in
  all against seven. No gain for a longer reply, so the reply is the findings alone.
- **Tests.** `staging_tests.rs` (the schema keeps the dialect and decides last, each kind its
  own note kind, the prompt with the summaries and nothing of the canon, a reply made notes),
  `craft_tests.rs` on the scripted port (the pass on each version's text, its notes in the
  checker's batch and in the triage; chapter 2's pass reads chapter 1's summary; a run killed
  at it), the rules in `author_prompts_tests`, `prompts_tests` and `author_tests`. The runs
  killed at every call, chapter and take cover the new call unchanged. **Mutation check:** 7
  mutants in one build (the notes not written, the summaries not passed, the address, the
  blank note and the empty anchor kept, a kind stored as another, the summaries ignored by the
  prompt): all killed.
- **Live: known answers on G2's own chapters** (`staging_live_tests`, `MINDFORK_FABULA_G2_STORY`;
  three runs, the role sampled as the app samples it; chapter 2 read after chapter 1's summary
  as G2 stored it). The faults are the readers': "son of Marya, who left with him for the
  mainland", where the son says his mother took *him*; replies opening on thanks; the
  inspector's decision stated at the grave.

  | Of 3 runs | Sonnet 5.5, text alone | Sonnet 5.5, with the summaries | Qwen 3.8 27B, with the summaries |
  | --- | --- | --- | --- |
  | Marya's line, a referent | 3 | 3 | 0 |
  | A reply that opens on thanks | 3 | 3 | 2 |
  | A line that announces | 3 | 3 | 0 |
  | Findings in all, chapter 1 / chapter 2 | 34 / 36 | 34 / 32 | 3 / 4 |

  A call, one chapter read, took about 6,500 tokens and 11 seconds on Sonnet; on Qwen about
  14,000 tokens, five and a half minutes and $0.02. Things it found:
  - *On Sonnet the pass reads as G2's readers did*: the father "son of Marya" against the son's
    own words, high, every run; Timur and Galina told to sit while they already sit; the
    inspector's decision stated at his father's grave.
  - *About eleven findings a chapter on Sonnet*, most of them low: they reach the triage, and
    a low note alone starts no revision.
  - *On Qwen 3.8 27B the pass finds little*, as the keeper's check did (PR 11): what it names
    is right, the kinship it misses.

- **Live: a two-chapter story on Qwen** (`story_chat_live_story`, Qwen 3.8 27B on OpenRouter
  pinned to DeepInfra, a Russian profile, G2's sketches, the harness's three-hour limit):
  "The Last Ferry" again, from the sketches to the last acceptance in the chat — 2,365 words,
  both chapters accepted.

  | | Calls | Prompt tokens | Reply tokens | Cost |
  | --- | --- | --- | --- | --- |
  | Chapter 1 (two scenes, the first revision accepted) | 203 | 944,960 | 229,301 | $0.55 |
  | Chapter 2 (four scenes, one played twice, the second revision accepted) | 569 | 6,761,797 | 457,197 | $1.72 |
  | The whole story, readings included | 775 | 7,710,539 | 686,925 | $2.28 |

  Things it found:
  - *The intents name the collision and the price* in so many words, in Russian:
    "Collision: his business-like distance breaks on three days' isolation… Price: he loses
    control of his schedule and of what will be kept quiet".
  - *The staging pass gave six notes over four versions, all accepted*: a tray in Galina's
    hands and left on the pier in one sentence; the logbook "taken by the keeper" when Galina
    snatched it; a cup taken from a windowsill nobody put it on; a key Vera gave away back in
    her hand; "his brother", a person the text never has. Four calls, $0.09 — and the slowest
    call of each review on Qwen, up to 17 minutes against the checker's 8.
  - *Chapter 2 spent the default budget in its fourth scene*, as G2's did; the woken assistant
    said where and started nothing, and it was finished at 14,000,000.
  - *The keeper's check named open threads on Qwen this time*: three in the last chapter, which
    the Author rejected with its reasons, and the last landing named two threads left open. In
    PR 11's Qwen story it had named none.
  - *What Qwen writes stays weaker than Sonnet's*: chapter 2's father is "Ilya", whom Galina
    calls "Svet", and a "Nikita" shares his boat; the pass named one of its referents, not the
    rest. A reply opened on thanks once (none in PR 11's Qwen story): on Qwen that fault was
    rare before.

- **Live: G2 again** (T18), on Claude Sonnet 5.5 from the same sketches and the same script,
  for the owner to read against the first: "The Last Ferry", 2,716 words, both chapters
  accepted.

  | | Calls | Prompt tokens | Reply tokens | Cost (list price) |
  | --- | --- | --- | --- | --- |
  | Chapter 1 (three scenes, the second revision accepted) | 352 | 3,883,526 | 141,833 | $9.19 |
  | Chapter 2 (four scenes, the second revision accepted) | 468 | 10,796,413 | 178,037 | $23.37 |
  | The whole story, readings included | 823 | 14,686,287 | 319,954 | $32.57 |
  | The first G2, for comparison | 875 | 14,017,889 | 314,563 | $31.18 |

  Chapter 2 spent the default budget in its third scene (6,067,211 tokens), the woken
  assistant said so and where to raise it and started nothing, and the run was finished at
  14,000,000, the first G2's ceiling. The rules did not lengthen the scenes: chapter 2's
  branches ran 6.0 rounds on average, the first G2's 6.1; the chapter's cost is the story's,
  which grows with it (docs/fabula.md §5). Things it found:
  - *The scenes are planned as collisions*: every intent names whose goals clash and the price
    (the Russian plans write "Collision:" and "Price:"). The secrets are spent one at a time, over
    two chapters: Timur tells Galina at his father's grave in chapter 1 and Vera only in
    chapter 2; Galina's "my fault" and Vera's "I tore out the page myself" come before the
    brother's notebook is read; Timur owns that he wrote the estimate the closure was signed
    on, never having seen the island.
  - *No reply opens on thanks.* The first G2's accepted text opened five replies on thanks,
    three on an apology and one on agreement, and said "thank you" seven times; the second, in
    58 replies, none of them, and the word nowhere.
  - *The staging pass* gave 41 notes over the four versions reviewed — 21 staging, 7
    referents, 8 lines that announce, 4 of thanks — and the Author accepted 35: a notebook in
    Vera's hand, under her arm and on the table at once; "the two of them put on wood" when
    Timur alone did; "dawn" at noon. Four calls, 23,000 prompt tokens, about $0.09.
  - *The register held*: chapter 2 paid off all six of chapter 1's threads, and the two it
    opened — the mainland's answer to the act, Timur back by five — the last landing called an
    open ending.
  - *The landing's assumptions disagree with each other.* The woken assistant warned that the
    list it was sent answers "who signed the order" several ways and to trust the text: the
    Author meets the same doubt at the outline, at each scene's plan and at each pick, sees
    none of its earlier answers, and decides it again — nine of chapter 2's 22 assumptions
    (the story's 38), each different (the first G2's 37 were worded differently each time,
    and its chapter 2 still asked what the brother's notebook says four times; pacing the
    reveal kept this one open longest). Not this pull request's; a step that reads the
    chapter's earlier assumptions, or a landing that keeps the last answer to a doubt, would
    settle it — the next entry, *a chapter's choices stand*, and the counts corrected there.
- Unit tests: 4789 green (+11), 328 `#[ignore]` (+1, the probe).

### Post-M9: Fabula — a chapter's choices stand (done)

G2 again found the landing's assumptions answering one doubt nine ways (PR 12). Where the Author
could have asked the human and did not, it records the question and the answer it chose (spec
§14, *Pauses*) — at the outline, at each scene's plan and at each pick — and no step saw another's
answers, so the next one met the same doubt and decided it again.

- **The fault, counted.** Read by meaning, not by wording: an assumption *answers again* when an
  earlier one of its chapter asked what it asks. G2 again: 5 of chapter 1's 16 (whether Galina
  knows Timur for a local, six times) and 12 of chapter 2's 22 (who signed the closure order,
  nine times; whose voices the shop's telephone carries, seven; what Timur decides, three).
  PR 12's Qwen story: 2 of chapter 1's 11 and 7 of chapter 2's 22 (what Nikita's last entry says,
  three times; where Ilya's grave was moved, three). The first G2 too, in other words each time:
  what the brother's notebook says, four times in its chapter 2. Both models, every chapter:
  the cause is the loop's.
- **A correction to PR 12's entry.** It counted the nine answers among "the chapter's 38
  assumptions": 38 are the story's, chapter 2 has 22. And it said the first G2's 37 were all
  different doubts: they were differently worded, and chapter 2's held the notebook's question
  four times. The entry is corrected in place.
- **The forks (the agent's, the owner having asked for the fix after #874; open to review).**
  What fixes it: the chapter's choices so far reach every Author prompt of the chapter, and the
  landing lists each question once with its latest answer — not the landing alone (the scenes
  would still be planned on nine answers), not every doubt asked once at the outline (the ones
  met later would be lost). Whose: the chapter's own, every run and take of it — an earlier
  chapter's were the human's to read at its checkpoint and live on in its text, summary and
  register. A change: a later step may answer a question again when what was played or what
  the human asked calls for it, writing the question as it stands, and the latest answer stands
  — never would let a choice made before a scene was played outrank its record. The same
  question: by its words, letters and digits with case ignored, since the step copies the
  question it is shown — not a model's call judging which questions are one.
- **As built.** The chapter's assumptions are read when it opens and kept as the run makes them
  (`Run::choices`); `ChapterContext::choices` lists the ones that stand
  (`Assumption::standing`: each question once, where first asked, with its latest answer) under
  "What you chose in this chapter where you could have asked its human author… These choices
  stand: plan, pick and write by them. Only what was played or what the human author asked can
  change one." The outline, the plans, the draft and the revisions read it with the story so
  far; the pick, which reads no story so far, gets it after the scene's intent. The reviewers
  and the triage go without: the reviewers read the text, and the triage decides notes. The
  steps that record assumptions — the outline, the plans, the picks — close the list otherwise
  (`Doubts::Recorded`): "…and do not ask them again. Only what was played or what the human
  author asked can change one. A question they do not answer is a new one: record it among
  your assumptions." The assumptions field says the same: "Write every new one you met; leave
  out only a question your choices in this chapter already answer, since they stand", and to
  answer one again, its question as it stands. The landing lists the standing ones. No schema
  step: the rows are as before, every answer kept. (The wording is the second one; the live
  run below measured the first.)
- **Tests.** `choices_tests.rs` on the scripted port (the outline's choice in the later plans,
  picks, draft and revision and in no reviewer's prompt; a question answered again in its own
  words standing once with the new answer; a run killed after the outline shown its choice when
  resumed; Russian questions compared by their letters), `author_prompts_tests` (every prompt
  that plans, picks or writes, each with its closing, the triage without), `author_tests` (the
  assumptions' description on every reply that records them), `run_tests` (the landing),
  `manuscript` (case, spaces and punctuation; a question of no words compared as written).
  **Mutation check:** 14 mutants in two builds (the choices not kept, not read when the chapter
  opens, the first answer kept, case or punctuation compared, wordless questions made one, the
  pick or the story so far without them, every answer listed in the prompt or at the landing;
  then the closing that asks for new questions withheld from the plans, from the pick, or given
  to the draft, and the description's first wording): all killed.

- **Live: a two-chapter story on Qwen 3.8 27B from G2's sketches, three attempts.** The
  third: chapter 1 accepted (four scenes, 1,258 words); chapter 2 stopped at its third scene on
  its 14M-token budget, after a resume past the 3-hour limit — 1,232 calls, $4.27, 7 h 41 min.
  24 of its 28 branches ran to the 8-round cap (PR 12's Qwen story: 7 of 22; G2 again: 11 of
  22), which doubled the cost. The World Master sees none of the choices, which reach the
  Author alone, and one story cannot tell whether its plans made the scenes run long: an
  observation, not a finding.
- **No question answered again — and almost none asked.** None of chapter 1's 2 assumptions
  and none of chapter 2's 4 answers an earlier one again (PR 12's Qwen story: 2 of 11, 7 of
  22). But the 15 plans and picks after the outlines recorded 2 between them, where PR 12's
  recorded about two each. Their 15 prompts, replayed through the same provider twice each,
  interleaved: as sent, 6 assumptions in 30 calls; with the list cut and the description as
  PR 12 had it, 45 — 10 of them answering a standing question again, 35 new. Told only that
  its choices stand, the Author recorded a seventh of its new questions: choices the human
  would never learn were made for them. Not seen in the scripted tests, which script the
  replies.
- **The wording, measured on the same prompts.** (C) The description "Write every new one you
  met; leave out only a question your choices in this chapter already answer, since they
  stand", the list closing "…and do not ask them again… A question they do not answer is a new
  one: record it among your assumptions"; measured twice: 18 and 18 assumptions, 1 and 2 of
  them a standing question copied word for word (the landing lists it once), about 16 new.
  (D) "Record every one you met at this step", the closing "Any other question you meet is
  still yours to record": 24, of which 3 or 4 re-answer a standing question in other words —
  the landing would list them apart again, the fault this pull request fixes — about 20 new.
  C is built: the repeats gone, new questions at about half PR 12's rate on these prompts;
  whether the half not recorded was worth the human's reading is not measured. The draft and
  the revisions, whose replies record no assumptions, keep the closing without "record it".
  Three replays of 60 calls, $1.72; with the three attempts, the pull request's live spend is
  about $6.2.
- **Found, not this pull request's: a map with no roads.** The second live attempt's canon,
  composed by Qwen from the sketches, gave its four places no route at all, and the canon's
  check let it through: it checks that a route leads somewhere, not that a place can be
  reached. Chapter 1's first scene was played at the village and picked; the second, planned
  at the lighthouse, was refused by the world at its opening — "lighthouse cannot be reached
  from village" — 51 minutes and $0.23 in. The refusal ended the run as a failure to start
  ("the story could not run", where the run had kept a scene), and an approved canon cannot
  be mended, so the woken assistant composed a second story with the routes and asked again
  for its approval, which the script did not give. The five other canons of the live runs
  since PR 11 had their routes: a slip of the model's, which the check should catch — Vera
  started at the lighthouse, Timur and Galina at the village, and no one could ever reach
  anyone else's start — and a refusal mid-chapter should be told as one at its step. The
  first attempt
  had ended sooner: a call to the provider hung past the chat turn's 15 minutes, before any
  chapter, for cents.
- Unit tests: 4797 green (+8), 328 `#[ignore]`.

### Post-M9: Fabula — a map every scene can reach (done)

The choices' live story met a map with no roads (*a chapter's choices stand*, above): a canon
Qwen 3.8 composed from the sketches gave its four places no route at all, and the canon's check
let it through — it asked that a route lead somewhere, not that a place be reached. Chapter 1's
second scene, planned at the lighthouse with Timur, who stood in the village, was refused by the
world at its opening, and the refusal reached the run as the store's failure: "the story could
not run", 51 minutes and a kept scene in.

- **How a refusal travelled.** The opening's placement, `Effect::Place`, is admitted only when
  the place can be reached from where the character stands (`Canon::reachable`, any number of
  routes). The plan was held to the world before the scene (`held_to_the_world`) — but only for
  the living and someone present; the placement was first checked by the branch's commit, whose
  refusal (`StoryDbError::Refused`) went up as `SceneError::Store`, then `Halt::Store`, then
  `Ending::Failed`, told as "The story could not run" — in Russian, "could not start".
- **The forks (the agent's, the owner having asked for the fix on 2026-11-05; open to review).**
  The map: every place reachable from every other along the routes, back or around — a scene's
  opening brings everyone in it to its place from wherever they are, and the tool's description
  already asks for the way back — not every place reachable from each start (a place of no
  return would pass, and each plan bringing someone out of it would be refused), not the starts
  reachable from one another (a place no one can reach would pass). A plan: held to the world by
  its opening, the opening's events admitted against the world as the scene would commit them —
  not a reachability check of its own beside the opening's, one rule in two places. A refusal
  while a scene plays: a stop at the scene's step in the world's words — not every branch failed
  and the human asked (P3), reruns of a scene that cannot open and then a question to the human
  about an engine's fault.
- **As built.** `CanonProblem::Unreachable { from, to }`: each place is checked against the
  first, both ways — every place reaches every other exactly when each reaches the first and the
  first reaches it — so a roadless map of four places names six missing ways, and a place with
  no way back one. One-way routes stay the map's own while there is a way around. The world's
  refusal of a placement now names whom ("timur cannot be brought to lighthouse: no route leads
  there from village, where they are"), so an Author planning again knows whom to leave out.
  `held_to_the_world` admits the plan's opening events (`scene::opening_events`) against the
  world; a plan it refuses is asked for once more with the reason, as a plan with no one alive
  was; a rerun the pick asks for, refused, keeps the cast and the place of the scene it reruns
  (`rerun_held`; it kept only the cast, so a refused place still went to the opening).
  `Stop::Refused` is the world refusing an event while a scene plays, at the scene's step, and
  the landing says so (`tool.story.landing.refused`). A run that ends `Failed` now reads "The
  story's run failed", no longer "could not run" — in Russian, no longer "could not start".
- **Checked against the live canons.** Of the eight canons composed live since PR 11, the seven
  with routes all pass the new check, and the roadless one is refused with its six ways.
- **Tests.** The canon: a roadless map's problems, a place with no way back and its text, a
  one-way ring that passes. The world: the refusal's sentence. The chapter: a plan held to the
  world by its opening (whom it names, from where), the refused rerun, the stop at a scene's step
  and the store's failure still the store's. The landing: the refusal told. Mutation check: 10
  mutants in one build (`crate::mutant(n)` injected by the script, the recipe of PR 11), all
  killed.
- **Live.** `story_chat_live_map_mended` on Qwen 3.8 27B through OpenRouter pinned to DeepInfra, in
  Russian, from G2's sketches: the test's engine drops every route of the model's first
  `story_create` that holds a map — the slip the live story found — and the rest is the model's
  own. Five fresh chats in two runs, all five mended. Four on the call after the refusal, which
  named six or eight ways for maps of four and five places; each was stored with its ways back
  (three a star around the village or the pier, one every place joined to every other). The
  fifth first passed its canon twice as a string of JSON it could not close, which the tool
  refused as unreadable, as before; then a map of two places, the one slipped, its refusal naming
  two ways; then that map mended and stored, and its draft revised to four places with their
  roads. The first run's check read the first call's result rather than the slipped one's and
  failed there; it now finds the slipped call by its map. $0.007 in all.
- **The stored roadless story resumed.** A copy of the story the defect left, resumed with the
  new code on an engine that must not be called: no call, and the run stops at `ch1/sc2/r0` with
  `Stop::Refused` — "timur cannot be brought to lighthouse: no route leads there from village,
  where they are" — its landing saying so in English and Russian, where it had read "could not
  start". That scene was planned before plans were held to the world; one planned now is asked
  for again.
- Unit tests: 4802 green (+5), 329 `#[ignore]` (+1, `story_chat_live_map_mended`).
