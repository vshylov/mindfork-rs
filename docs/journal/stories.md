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

## Entries (6)

- Post-M9: Fabula — PR 1, the story database (done)
- Post-M9: Fabula — PR 2, the model port and replay; Gate G0 GO (done)
- Post-M9: Fabula — PR 3, the reply's schema on the wire (done)
- Post-M9: Fabula — PR 4, the simulation; Gate G1 GO (done)
- Post-M9: Fabula — PR 5, the reply's schema on the cloud wires (done)
- Post-M9: Fabula — PR 6, the Author's loop (done)

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

**Gates**: unit tests green; clippy `-D warnings`, fmt, `cyrillic_scan` (and its
`--self-test`), `link_check`, `doc_index_check`, `layer_check`, `list_scroll_check`,
`claude_md_chart --check` clean.
