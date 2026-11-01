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

## Entries (3)

- Post-M9: Fabula — PR 1, the story database (done)
- Post-M9: Fabula — PR 2, the model port and replay; Gate G0 GO (done)
- Post-M9: Fabula — PR 3, the reply's schema on the wire (done)

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
