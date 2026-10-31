# Fabula — a story engine used from the chat: track plan

**Status:** in progress — PR 1 built (2026-11-01). Plan of 2026-10-31. Every fork of the research is decided by the owner, and
so are the three this plan adds (§6) and where development runs (§4, *The development
stack*). The design —
the engine, its stores, its contracts, its evaluation — is
[research/fabula-engine.md](research/fabula-engine.md) (*the spec* below), and the
decision to build it here is [ADR 0014](decisions/0014-fabula-story-engine.md). This
plan says only how the work is cut into pull requests, what each one proves, and what
it touches.

## 1. What the track builds

A multi-agent story engine inside mindfork. Characters propose actions, a World
Master commits them to an append-only, branched event log, and one Author picks a
branch and writes the chapter; read-only reviewers return notes (spec §4–§7). The
person uses it from the chat: they attach sketches of the work, the assistant builds
the canon with them and calls `story_create`, and each chapter is written by a
background run started with `story_continue`, which lands the chapter in the chat and
asks its question there (spec §14, *The chat tools and the human in the loop*).

The simulation is a hypothesis. M3 measures it against one agent writing alone at
equal cost (spec §11), and only a GO there opens the long form (M4).

## 2. Decisions this plan rests on

All of them the owner's, on 2026-10-31; the reasoning is in the spec.

| Decision | Where |
| --- | --- |
| Inside mindfork, one crate, no workspace (option A) | spec §14 |
| Used from the chat through `story_create`, `story_continue`, `story_status`; sketches attached with `/file attach` | spec §14 |
| One model for every role, the one selected in the app; models per role deferred to the far term | spec §14, §15 |
| A chapter's own timeout, measured in hours; a budget per chapter | spec §14 |
| A run cut short by a quit resumes on request, and its cancelled landing says so | spec §14 |
| `RunKind::Story`, so `CHAT_SCHEMA` 7 → 8 | spec §14 |
| The human in the loop by messages: H1 (a), H2 (a), H3 (a) | spec §14, §15 |
| Structured output by schema-constrained decoding; the validator is the contract; no forced tools | spec §8 |
| Role prompts in English (an exception to axis A); the story's language per story | spec §8, §15 |
| A story's database in mindfork's data and backups; its call log beside it, outside backups | spec §6, §15 |
| Off by default until M3: `tools.story` | spec §15 |
| M3's systems as run options of a hidden runner; judging by a script under `tools/` | spec §15 |
| About 20 premise candidates drafted by Claude, 12 chosen by the owner; three human judges on the calibration set | spec §11, §15 |
| Development and calibration on Claude (`CLAUDE_API_KEY`) and OpenRouter (`OPENROUTER_API_KEY`); the local RTX 4090 for what only llama.cpp shows | §4 |
| T1 (a), T2 (a), T3 (a) | §6 |

## 3. Where the code goes

The layer rule holds (`tools/layer_check.py`): a layer names only the layers below it.

| Layer | What lives there |
| --- | --- |
| `entities/story/` | The domain types: canon, cast sheets, locations, setups, proposals, actions, visibility, events and effects, questions and assumptions; the canon's deterministic checks; the fold of world state over a branch path and the witnesses. The fold lives here, not in `features`, because the database actor runs it for every commit, and `shared` may name `entities` but never `features` |
| `shared/storage/story/` | The story database and its call-log sibling: the schema with append-only triggers, the database actor on a thread of its own with the only connections, the write capabilities, the read views (served by the actor too). `STORY_SCHEMA` and `STORY_CALLS_SCHEMA` join `shared/storage/schema.rs` |
| `shared/api` | The output-schema field on `ChatRequest` and its mapping in each wire (spec §8, §14) |
| `features/story/` | The engine: the deterministic validators of proposals, the scene runner, the chapter loop, the reviewers, the pauses; `LlmPort` with the replay adapter and the adapter over `EngineBackend`; the schemas and the dialect's check; the role prompts as English templates beside the code |
| `features/tools/story.rs` | The three tools' schemas and descriptions, en and ru like every tool's |
| `app/orchestrator/` | The third background payload beside the sub-agent's and the dialogue's; the landing (the chapter into the chat, the notification); the user's rows after a question, read verbatim for `story_continue`; the pending question's reminder line at the request's tail |
| `entities/subagent.rs` | `RunKind::Story` |
| `shared/config.rs` | `tools.story` (off), the chapter's timeout and budget (§6) |
| `main.rs`, `features/cli.rs` | The hidden runner for M3 |
| `tools/fabula_judge.py` | The blind judging of M3 |

The engine never reaches up for an engine of its own: the orchestrator hands it the
chat's `EngineBackend`, wrapped in the `LlmPort` adapter, and every call takes the
app's session budget as a background dialogue's calls do. The client factory that
models per role would need stays where it is until then (spec §14, *Integration
work*).

## 4. Stages and pull requests

Each line is one pull request on a branch of its own (AGENTS.md §2). A stage's gate
is checked before the next stage starts, and its outcome goes into the journal.

### The development stack

The owner's decision (2026-10-31): development and calibration run on cloud APIs. The
feature is complex, and weeks of iteration on the local RTX 4090 alone would be too
slow; a turn takes seconds on a cloud route, and arms run side by side.

- **Claude**, through `CLAUDE_API_KEY` on the Anthropic wire. The smokes read
  `MINDFORK_ANTHROPIC_KEY` and a model variable, so the key is passed as
  `MINDFORK_ANTHROPIC_KEY=$CLAUDE_API_KEY`. In the app, the Claude section's
  `api_key_env` names `CLAUDE_API_KEY`; its default is `ANTHROPIC_API_KEY`.
- **OpenRouter**, through `OPENROUTER_API_KEY`, for the open models near the gate:
  `qwen/qwen3.8-27b` pinned to DeepInfra (bf16) and `google/gemma-4-31b-it` pinned to
  DeepInfra (fp8). Both endpoints list `structured_outputs` (read 2026-10-31), while
  several other providers of the same models do not, so every request carries
  `provider: {only: [...], allow_fallbacks: false}` and `require_parameters: true`. The
  smokes take `MINDFORK_ENGINE_URL=https://openrouter.ai/api/v1`,
  `MINDFORK_ENGINE_KEY=$OPENROUTER_API_KEY` and `MINDFORK_ENGINE_MODEL=<id>`.
- **Thinking on Claude.** In the 5.5 generation, only Haiku 5.5 accepts
  `thinking: {type: "disabled"}`, and only at `high` effort or below. Sonnet 5.5 takes
  `"between_tools"`, which turns off only the thinking before its first output, and Opus 5.5
  and Fable 5.1 always think. So "thinking off for characters" means Haiku 5.5 with thinking
  disabled, or the lowest effort elsewhere, and M1 measures what that costs.
- **Which model where.** M1 iterates on Haiku 5.5 and the two open models, since they
  are cheap and M1 judges the shape of decisions. M2's quality runs and the G2 story use
  Sonnet 5.5 or Opus 5.5 as the one model. M3's model is chosen when M3 starts; one
  model serves every system there anyway.
- **What stays local.** The local RTX 4090 keeps what only it shows: llama.cpp's grammar
  with thinking, the managed server and its `draft-mtp` trap, and the gate's own Q4
  weights. A finding on a cloud route is confirmed on the gate's weights, locally or on
  the rented gate, before it is called; a cloud route is fp8 or bf16, not Q4_K_M
  (docs/lessons.md §9).
- **Spend.** The call log records every call's tokens. OpenRouter also reports cost, so
  T1's dollar cap applies there; Anthropic reports tokens only, so a Claude run is
  bounded by its token budget.

### Stage 0 — this plan

| PR | Content |
| --- | --- |
| 0 `docs/fabula-plan` | This plan and ADR 0014. Stacked on the spec's pull request (#854). |

### M0 — the skeleton, offline

| PR | Content | Proof |
| --- | --- | --- |
| 1 `feat/story-store` | `entities/story.rs`; the story database: the schema of spec §6 with the call log in its sibling file, the append-only triggers, the actor and its capabilities, `STORY_SCHEMA = 1`; the canon's import and its deterministic checks; the fold and the witnesses over a branch path | Unit tests: `UPDATE` and `DELETE` on `event` refused; a second `BranchWriter` for a branch refused; a branch never sees a sibling's events; the fold and the witnesses on fixtures. `BranchWriter` not being `Clone` is a compile-time assertion (§5). No live run: storage only, said in the PR |
| 2 `feat/story-ports` | `LlmPort`; the replay adapter keyed by call address (UUIDv5 ids, the prompt hash flagging drift); the adapter over `EngineBackend` with the fallback mechanism (the schema in the prompt, a lenient parse, one repair); the schemas derived from the types and the dialect's check; one scripted scene | **Gate G0:** the fixture replays identically; a second writer is refused; the assertion holds. One `#[ignore]` smoke records the fixture against a gate model |

**PR 1, as built (2026-11-01).** Where the code differs from the spec's sketch of §6 and
§9, and why, is in [journal/stories.md](journal/stories.md): the fold and the witnesses
sit in `entities` (§3); reads go through the actor as writes do, so the files keep the
default rollback journal rather than WAL; every table but `run` and `step` is append-only;
ids are derived from position, never drawn at random; routes and secrets are arrays of
closed objects. One finding passes to PR 2: serde reads a stray field on a variant
without fields in silence, so a reply is validated against its schema before it is
parsed.

### M1 — the simulation, live

| PR | Content | Proof |
| --- | --- | --- |
| 3 `feat/output-schema` | `ChatRequest.output_schema` on the two wires development runs on: `response_format` with `json_schema` on Chat Completions (llama.cpp, vLLM, xAI, OpenRouter with `provider.require_parameters`), and `output_config.format` on Anthropic. DeepSeek keeps the fallback, and an unset field leaves every request byte-identical | Unit tests of the unchanged requests. Live: the probe of spec §8 as a smoke on the two pinned OpenRouter routes and on Haiku 5.5 and Sonnet 5.5, thinking on and off, then once on a gate model's own weights through the local llama.cpp |
| 4 `feat/story-simulation` | The character and World Master contracts with their English prompts; the round (intent, initiative, reaction, reflection); the opening events; K branches, one after another under `sessions = 1`; the memory views; thinking off for characters; planted secret tokens and the leak count; an `#[ignore]` harness that simulates scenes from the fixture canon | **Gate G1** (spec §13): 20 scenes with zero leaks, at least 99% of calls schema-valid at the first attempt, the World Master's refusals reported per scene. Measured on the development models, then confirmed on a gate model's own weights. GO or NO-GO into the journal |
| 5 `feat/output-schema-clouds` | The same field on the remaining wires: Responses (`text.format`) and Gemini (`generationConfig.responseFormat`) | A live smoke per provider. Independent of PR 4; it lands before PR 8, since the model a person selects can be any of them |

### M2 — the Author's loop and the chat tools

| PR | Content | Proof |
| --- | --- | --- |
| 6 `feat/story-author` | The outline, the rolling scene plans, the pick against its rubric (two reruns, then the least bad), the chapter's draft, the Reader and the Continuity checker, the triage and at most two revisions, the markdown export; the pauses inside the engine: `question` and `assumption` rows, a pause that ends the run, a resume from the paused step; the budget counted per chapter | Live, through the harness: one chapter end to end on a gate model; the run killed at random steps resumes each time |
| 7 `feat/story-data` | Stories in mindfork's data: `data/stories/<id>.db` inside backups (the directory and the manifest's schema entry), `data/story-logs/` outside them; `mindfork stats` counts stories and compares them by id; a story is hidden and purged with its chat | A backup's round trip; the previous release on a backup that holds stories (docs/lessons.md §8). No live model |
| 8 `feat/story-run` | The third background payload; `RunKind::Story` with `CHAT_SCHEMA` 7 → 8 as a stamp-only step and its golden fixture; `tools.story`, the timeout and the budget; the landing: the chapter into the chat (§6) and a notification in the profile's language with the summary, the picks, the assumptions and the question; the cancelled landing that names its step; `story_status` | Orchestrator tests on the mock engine; the previous release refusing a chat file of schema 8 politely (AGENTS.md §6) |
| 9 `feat/story-conversation` | `story_create` and `story_continue`; the user's rows after the question read verbatim; the conservative reading; the evidence rows; the refusals of a woken turn and of `unclear`; the reminder line; spec.md §9.16, the README (experimental), the manual, the help overlay, the CHANGELOG | Live, the gate models as the chat's model: a valid canon from attached sketches; no approval without a user message after the question; a chapter that lands; claims without a call counted. **Gate G2** (spec §13): one short story end to end in the chat, read by the owner |

A release can follow G2: the tools are reachable only with `tools.story` on, and the
release carries the chat schema's step with the checks AGENTS.md §6 asks for.

### M3 — the evaluation

Before it starts, two judges besides the owner are found, and Claude drafts the premise
candidates for the owner to choose from (spec §15).

| PR | Content | Proof |
| --- | --- | --- |
| 10 `feat/story-eval` | The hidden runner: no line in `--help`, the single-instance lock skipped, stories written into its output directory and not into the data; the systems F, B0 and A1–A4 as run options; the export of the markdown and the JSON trace; the 12 premises frozen in the repository with their SHA-256 | Unit tests of each system's wiring; a live run of one premise through every system |
| 11 `feat/story-judge` | `tools/fabula_judge.py`: pairs shown blind in random order, verdicts as JSON, the binomial test; the LLM panel of other model families through OpenRouter, both orders; the calibration check against the three humans' majority | `--self-test` against fixtures, run in `lint` like the other tools' |

Then the run itself, outside any pull request: 36 pairs for each of the two decisions
(and the ablations), the calibration by three people, and the decision rule of spec §11,
recorded here and in the journal.

- **GO** (F beats B0 and A1) opens M4.
- **F loses to A1, and A1 beats B0:** a pull request switches the tools to A1's
  pipeline and removes the simulation; the long form continues on A1.
- **Neither beats B0:** the track stops. Whether the tools stay, running B0, is the
  owner's call then.

After M3, Russian prose under the English prompts is compared with prose under Russian
ones (spec §15).

### M4 — the long form, after a GO

Voice review, the Canon keeper with its pause P4, the settle branches, runs of several
chapters, and the tasks screen's view of a story run. **Gate G4** (spec §13): three
chapters with no more leaks or continuity findings per 1,000 words than M3's stories.
Its pull requests are cut when M4 starts.

## 5. Difficult spots

- **One engine for the chat and the story.** At `sessions = 1` a chapter's calls and
  the chat's turns take turns request by request, so the chat slows while a chapter
  runs. The tool's description and the notification say so, and a managed server can
  raise `sessions` with its slots.
- **Hours of work on a laptop.** Quitting cancels the run (decided); a resume costs only
  the step in flight, because every finished step is recorded (spec §9).
- **Characters on a model that always thinks.** Opus 5.5 and Fable 5.1 cannot turn
  thinking off, so on them a character's call costs several times what the cost model
  assumes. The tool's description says so, the budget bounds it, and the lowest effort
  keeps the thought short.
- **`json_schema` under `--spec-type draft-mtp`** crashes llama.cpp's grammar
  ([#30419](https://github.com/ggml-org/llama.cpp/issues/30419), open). While a managed
  server runs a draft model, the adapter takes the fallback mechanism.
- **A thought that never closes** runs to the cap and returns nothing: llama.cpp holds
  the end of the sequence until the grammar completes. A role's cap leaves room for
  thinking, and the cuts are counted (spec §8).
- **A model that says it continued without calling.** Qwen still does it about once in
  20 under the best wording measured ([journal/tools.md](journal/tools.md)). The
  reminder line and `story_status` make a missed call visible, and PR 9's smoke counts
  them.
- **Refusals on Claude 5.x.** A field that reads as the model's own reasoning is
  refused as `reasoning_extraction`, and the refusal is billed. Fabula's free-text
  fields are described as story content, and M1 counts refusals per role (spec §8).
- **Anthropic's prompt cache.** A changed output format drops the conversation's cache,
  and the roles alternate formats. The calls are stateless, so this costs prefix reuse
  on Anthropic only, and M1's call log shows how much.
- **The compile-fail gate.** The crate has no library target, so neither `trybuild` nor
  a `compile_fail` doctest can run. G0's "cannot be cloned" is a compile-time assertion
  instead, the ambiguity trick `static_assertions::assert_not_impl_any!` uses.
- **Coverage.** The deciding code goes into functions a unit test calls, and every file
  of live smokes is named as a test file, as `sonar-project.properties` explains.
- **CLAUDE.md sits near its cap** (30,000 bytes). PR 1 added the story engine's row to
  the document map and its status line, and dropped the release pipeline's, which had
  stopped being recent; each later PR keeps to the one line.

## 6. Forks this plan adds

The owner accepted all three recommendations on 2026-10-31.

- **T1. The budget's unit** (PR 6). (a) Tokens per chapter on every provider, plus a
  dollar cap where the provider reports cost, as OpenRouter does *(recommended)*.
  (b) Dollars only, which needs a price list for every provider. (c) No budget beyond the
  timeout. **Owner's decision (2026-10-31): (a).**
- **T2. The chapter's default timeout** (PR 8). Rough estimate: a chapter takes about an
  hour on a local 12B model and twice that on a 27B one, before reruns. (a) Six hours
  *(recommended)*: a backstop for a hung run, while the budget bounds the spend. (b) Three
  hours. (c) Twelve hours. **Owner's decision (2026-10-31): (a).**
- **T3. How a chapter lands** (PR 8). (a) As a text attachment of the chat *(recommended)*:
  the assistant reads it in full or by reference and can discuss it, and the person opens
  it as any attachment. (b) As a chat file only, which the assistant reaches through a tool.
  **Owner's decision (2026-10-31): (a).**

## 7. Not in this track

Models per role (the far term; the roadmap carries it as groundwork), the story tools
over the messenger bridge, interactive fiction, importing worlds from novels, MCP
elicitation, screenplays and poetry.

## 8. Documentation touch list

| What | When |
| --- | --- |
| [docs/journal/](journal/) — a new `stories.md`, its row in CLAUDE.md's document map | PR 1, then every PR |
| [spec.md](../spec.md) — §9.16, the story tools | PR 8 and PR 9 |
| [architecture.md](architecture.md) — the module map for each new module | Every code PR |
| [CHANGELOG.md](../CHANGELOG.md) — Added (experimental), and Data for the chat schema and the story database | PR 7, PR 8, PR 9 |
| [README.md](../README.md), [manual.md](manual.md), the help overlay | PR 9 |
| [roadmap.md](roadmap.md) — models per role as groundwork | PR 9 |
| [lessons.md](lessons.md) — a trap that will bite elsewhere | When one appears |
| This plan — each gate's outcome; moved to `docs/history/` when the track ends | Each gate |
