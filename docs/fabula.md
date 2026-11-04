# Fabula — a story engine used from the chat: track plan

**Status:** in progress — M0 done (PRs 1 and 2, Gate G0 GO); M1 done (PRs 3–5, Gate G1 GO, 2026-11-01); M2 done (PR 6, the Author's loop, 2026-11-01; PR 7, stories in mindfork's data, 2026-11-02; PR 8, the story run in the background, 2026-11-02; PR 9, the story driven from the chat, 2026-11-02; Gate G2 GO, 2026-11-03; PR 10, a scene played again, 2026-11-03); before M3, the readers' findings under way (PRs 11–12, from 2026-11-04). Plan of 2026-10-31. Every fork of the research is decided by the owner, and
so are the eighteen this plan adds (§6) and where development runs (§4, *The development
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
  disabled, or the lowest effort elsewhere, and M1 measures what that costs. As built in
  PR 4, the Anthropic wire sends every muted turn as `disabled`, and the client asks again
  in the shape the model's refusal names: `between_tools` on Sonnet 5.5, no `thinking`
  at effort `low` on Opus 5.5 and Fable 5.1. The fix reaches the chat's own muted turns
  too ([journal/engine.md](journal/engine.md), *Thinking off reaches Claude*).
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

**PR 2, as built (2026-11-01).** The port takes the engine's own `Schema` type rather than a
`serde_json::Value`: the crate's `serde_json` is built without `preserve_order`, so a `Value`
sorts its keys, and a schema handed over as one would reach the model with `action` before
`private_thought`. The schema prints itself in the order declared, and PR 3 carries that text
to the wire as it is, never through a `Value`. Both adapters feed their attempts through one
policy, so a replay repeats the live call's retries and repairs. M0's scene is ruled by a fixed
rule where M1's World Master will rule. **Gate G0: GO.** The fixture recorded against Qwen 3.8
27B (OpenRouter, pinned to DeepInfra) replays identically and with no drift; a second writer
is refused and the not-`Clone` assertion holds (PR 1's tests). The figures are in
[journal/stories.md](journal/stories.md).

**PR 3, as built (2026-11-01).** `ChatRequest.output_schema` carries the schema as the text
it was printed as (a `RawValue`; `serde_json` gains its `raw_value` feature), and
`EngineBackend::enforces_output_schema` says whether a backend constrains to it: `false` by
default, `true` on Chat Completions except DeepSeek and a gateway model whose catalogue
lacks `structured_outputs`, and on Anthropic. Through the gateway a schema also asks for
`provider.require_parameters`; it chooses no provider. The engine adapter keeps the schema
in the prompt under both mechanisms, since llama.cpp's grammar shows the model no
description; a managed server with a draft model is put on the fallback by
`EngineAdapter::fallback_only`, which PR 8 calls. Live on five routes, thinking off and on:
every first attempt that finished met the schema, and a reply asked in words for a bare
greeting came back as the schema's object on every route. On Claude the wire cannot turn
thinking off yet, so "off" there is the model's own adaptive default; PR 4 decides how the
characters mute it.

**PR 4, as built (2026-11-01).** A scene is K branches of rounds (intent, initiative,
reaction, reflection), one after another. The World Master proposes and the engine commits:
every event it writes is held to the folded world and to what its witnesses know before it
is committed. A refusal there is counted, never repaired. A ruling that does not answer its
proposals is a slip, recorded and skipped. The World Master is shown each proposal's deed and
never its thought, which the engine copies into the event. A character sees its own sheet,
what it perceived, its reflections and the secrets revealed to it. The characters run with
the application's mute and the World Master with thinking at a low effort. On Claude the mute
did not reach the model at all, since the 5.5 generation thinks when the field is absent, so
the Anthropic wire now asks for it off in the shape each model takes (*The development
stack*, above). **Gate G1: GO.** 20 scenes × 3 branches each on Haiku 5.5, Qwen 3.8 and
Gemma 4 (the open models through OpenRouter, pinned to DeepInfra), then 5 × 3 on Qwen 3.8's
gate weights through the local llama.cpp. On every route: no leak, while 10–38 secrets were
revealed and shown afterwards; at least 99.89% of first attempts schema-valid (the misses
were three cuts and errors of 6,418, each taken by the retry); the World Master's refusals
reported per scene. The figures are in [journal/stories.md](journal/stories.md).

**PR 5, as built (2026-11-01).** Responses sends the schema as `text.format`, strict, beside
`text.verbosity`; a refusal there comes as a part of its own and ends the turn as filtered, so
the port records a refusal instead of repairing a reply. Gemini sends it as
`generationConfig.responseJsonSchema`, not the `responseFormat` this plan named:
`gemini-2.5-flash` ignores `responseFormat` in silence and answers in prose, while
`responseJsonSchema` held on every model tried. Both clients now say they enforce a schema.
Live on `gpt-5.4-mini`, `gpt-6.1-sol`, `gemini-3.8-flash` and `gemini-2.5-flash`: every first
attempt met the schema, and the bare greeting came back as the schema's object. A small G1
run on `gpt-5.4-mini` and `gemini-3.8-flash` sent every simulation schema through each
provider: all first attempts valid, no leak. One finding passes to M2: under a schema,
Gemini 3.x thinks about a tenth as much and returns no thought summary, so a World Master
there rules on little thought. The figures are in [journal/stories.md](journal/stories.md).

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
| 10 `feat/story-rerun` | A named scene rerun at P2 (T8): the main line steps back to the scene asked for, the scenes after it and the chapter's text are made again under addresses of their own, and the reading of the human's reply gains the request | Engine tests on replay; live, one chapter's scene rerun at its checkpoint |

**PR 6, as built (2026-11-01).** A run goes from where the story stands to its next pause.
Each step reads the stores before it calls a model, so a run killed anywhere resumes, as a
new run, at the first step no run finished. A branch a stopped run left half-played is marked
lost, and a branch under the next index plays in its place. The live kill run found why:
counted as failed, two kills in a row cost a scene two of its three attempts. The least bad
pick of spec §7 costs no call, since the last attempt's pick may only pick. The pauses are
rows. At P2 the human accepts, with notes for the next chapter, or asks for a revision with
notes; *rerun a named scene* is not built, since it needs the main line to step back, and
moves to PR 10 (T8), after PR 9 reads what the human asked for. The budget counts every call under the
chapter's addresses, from the call log. **Live:** one chapter end to end on Qwen 3.8 27B
(OpenRouter, DeepInfra): 192 calls, every first attempt valid, no leak, 718,865 tokens and
$0.29 in 39 minutes. Killed at random 4 times on Qwen and 6 times on `gemini-3.8-flash`,
the chapter resumed each time to its checkpoint. **PR 5's finding, measured:** on
`gemini-3.8-flash` the schema does not change how much a role thinks; its effort does. At the
low level the World Master thought a median of 0 tokens under the schema and 0 without it,
at the high level 1,811 and 1,496, with no slip in any arm. The Continuity checker found
three planted faults in every call of every arm. No mechanism changes; a role's effort is
PR 8's to set. The figures are in [journal/stories.md](journal/stories.md).

**PR 7, as built (2026-11-02).** The owner decided two forks the plan had left open (§6, T4
and T5): a story is deleted softly, while its chat is hidden, and it lives at
`stories/<chat-id>/<story-id>.db`, its call log at `story-logs/<chat-id>/<story-id>.db`.
Backups pack `stories/` and clear it on restore, leave `story-logs/` in neither half, and
record the newest story's schema in the manifest. `mindfork stats` reads a story read-only,
counts it and its chapters, and compares two copies of one by a digest of each row, since
two machines continuing a story write the same positional ids with different content. The
restore test found that a story put back behind its call log reused a run id the log
held, so a run is now numbered past the last of either file, and a chapter's budget counts
only the story's own runs. The round trip found an older defect: restoring the pre-restore
copy within the second it was made wrote the new copy over it, so an archive the app
names itself never takes an existing name now. **The previous release** reads a backup
with stories, unpacks them as unknown files, and leaves them out of its own backups; PR 8's
release notes say so. The figures are in [journal/stories.md](journal/stories.md).

**PR 8, as built (2026-11-02).** The owner decided two forks the plan had left open (§6, T6
and T7): a chapter's budget is 2,000,000 tokens with no dollar cap unless one is set, and a
story run takes a background slot as a dialogue does. The run is the third background payload
(`RunSpec::Story`): PR 6's engine on the turn's backend, every role on the one model with the
sampling G1 and PR 6 measured, each call under the app's session budget, one run per story
claimed on a board the tools read. The time limit cancels the calls, not the run, so the
engine records the step either way. The chapter lands as an attachment of its chat, decided
against the chat's budget at the landing, and the notification carries the summary, the
picks, the assumptions and the question; a quit names the call the run was in. Wiring the
landing found that a story's record would have been auto-titled by a model call at every
chapter; it no longer is. `story_status` reads the chat's stories read-only beside a run.
`CHAT_SCHEMA` moves 7 → 8 by a stamp, and the engine modules keep their `allow(dead_code)`
for what PR 9 and M3 still have to reach. **The previous release** refuses the migrated
data as a newer version's and restores its pre-migration backup with no warning, to the same
fingerprint. **Live** on Claude Haiku 5.5 and Qwen 3.8 27B, a story run stopped at its budget,
at a stop and at its time limit, each resuming where the last stopped, every permit of the
session budget given back. The figures are in
[journal/stories.md](journal/stories.md).

**PR 9, as built (2026-11-02).** The owner decided four forks (§6, T8–T11): the named scene
rerun is PR 10, G2 runs on Claude Sonnet 5.5 in Russian, and the chapter's default budget is
6,000,000 tokens. `story_create` stores the canon the call carries as a draft, refused with every
problem named, under an id made from the title, and opens P1 with no model call. `story_continue`
is executed by the turn's loop: the person's rows written after the question was put — after the
canon's last change, after the landing that carried a chapter's or a scene's question — are read
verbatim by one schema-constrained call, conservatively; a woken turn and an unclear reading
resolve nothing, and the resolution records the messages' ids and quotes with the assistant's
notes apart. A change to the canon starts no run, the last chapter accepted finishes the story,
and a chapter whose budget is spent starts nothing. While a question waits, every request
carries a reminder line at its tail. **Live:** on Claude Haiku 5.5, Qwen 3.8 27B and Gemma 4 31B
a canon was composed from attached sketches in two or three calls, none tried to approve for the
person, and the approval turn called the tool every time (13 of 13, no claim without a call); a
Qwen chapter landed and was resumed. **Gate G2:** "The Last Ferry" (in Russian), two chapters and 2,696 words
in Russian on Sonnet 5.5, finished in the chat from the sketches to the last acceptance — 875
calls, 14.0 million prompt tokens, about $31 at list price, none of it from Anthropic's cache.
The run found the spent budget restarted in vain, the empty run after the last chapter, and the
cost's growth with the story (§5); the first two are fixed. **Gate G2 GO**: the owner's reading, 2026-11-03.
The figures are in [journal/stories.md](journal/stories.md).

**What G2's readers found (2026-11-03).** The owner read the story and passed the gate: a very
good result for Claude Sonnet 5.5. The owner also gave it to two other models for review, Gemini 4 Argon
and Claude Fable 5.1 at its maximum effort. Both called the prose strong for machine text — spare,
concrete, without filler — and named the same strengths and the same faults.

- *What holds:* the continuity of things, numbers and address (a sign blown down in one scene is
  picked up in the next; "three days" to the ferry is two on the second evening; who says *ty*
  and who *vy* to whom); the asymmetry of knowledge, three secrets kept and spent on time, the
  reader shown what the inspector misses; the shopkeeper's voice, distinct from the other two; the
  motifs echoed across the chapters (the lamp lit together, the moss scraped off a cross, the
  briefcase left unopened).
- *What fails is the story's deeper logic, not its language:*
  - **Planted clues that never pay off.** A logbook entry "in another hand" becomes "her own shaking
    hand", then "empty"; footsteps on the path end chapter 1 and are never someone's; the
    shopkeeper's steering him from a wall leads nowhere.
  - **Time and kinship that do not add up.** A father "lost a few years ago" lies under an old,
    mossed cross; "son of Marya, who left with him" has no clear referent, so the genealogy reads
    wrong.
  - **Every conflict resolved at once, at no cost.** Chapter 2 opens all three secrets in an evening
    and a morning; everyone forgives everyone, thanks run through the lines seven times, and the
    dead brother's letter absolves the keeper; a letter that long, written at the helm in a storm,
    strains belief.
  - **Lines that report the scene's state.** The inspector and the keeper speak in the same clipped
    telegraph, stating plans and boundaries aloud as if to the engine.
  - **A summary that contradicts itself.** Chapter 1's summary has the shopkeeper hide something in
    the chapel and carry something out of it; the text says the latter. Summaries are context for
    the chapters after, so this is drift at its source.
  - **What the reviewers let pass.** Who sits where, a pronoun's referent; and both chapters run the
    same course (an evening indoors, the lamp at night, the graves at dawn), which a longer story
    would show.
- *What it says of the engine.* The world state and what each character knows hold; what is missing
  is bookkeeping above them. The readers' proposals: a register of open threads, each closed or
  marked as left open; a table of dated facts (ages, deaths, "how long ago") checked by arithmetic
  as the ferry's days were; summaries made from the accepted text alone and checked for
  contradiction; a cheap pass of its own for staging and referents; and for the planner and the
  characters — opposing goals in a scene, someone paying a price, no line that opens on thanks or
  agreement, intentions shown rather than stated. None is decided: they are candidates for the
  Author's and the reviewers' prompts and checks, and whether they come before M3 is the owner's
  call, since M3 measures the simulation against one agent writing alone under the same Author.

**PR 10, as built (2026-11-03).** The owner decided two forks (§6, T12–T13): a take's budget
is its own, and the live check runs on Qwen 3.8 27B. At P2 the reading is shown the chapter's
scenes by number and may return `rerun` with one of them; the answer is recorded as a change
with a **take** of the chapter beside it, in one transaction, in a table of its own (no
schema step). The take carries the scenes before the one named; the next run plans that
scene again with the person's request and its old setup, plays it and every scene after it
— each scene's attempts numbered past its last, the take's plans at `ch1/t2/…` — and drafts,
reviews and summarizes the chapter anew from the chapter's next version, revisions counted
within the take. The main line runs through each chapter's latest take; every request made at
the chapter's checkpoints reaches the take's prompts. Engine tests on the scripted port kill
a take at each of its calls and replay a story through a take from its addresses alone.
**Live:** on Qwen 3.8 27B in Russian, chapter 1 landed with its three scenes numbered, the
request to play scene 2 again was read as a rerun of scene 2, and the take carried scene 1,
played scenes 2–3 again and a fourth the first take had not had, and reached its own
checkpoint at version 6 — 1,227 calls, $3.00 in all, the take twice the first making's 3.8
million tokens. The run found the Continuity checker cut at its cap on Qwen, a woken turn
resuming a stopped run unasked (now refused, T14), and a harness that gave up as the run's
time limit fell (fixed). The figures are in [journal/stories.md](journal/stories.md). M2 is
done.

A release can follow G2: the tools are reachable only with `tools.story` on, and the
release carries the chat schema's step with the checks AGENTS.md §6 asks for.

### Before M3 — what G2's readers found

The owner chose on 2026-11-04 to act on the readers' findings before M3, in two pull requests
(§6, T15–T18). M3 measures the simulation against one agent writing alone under the same Author,
so what the Author's loop gains, every system there gains; the characters' rules are the
simulation's own. Two causes are in the code: the summary was written by the Author with the
whole canon before it, so the secret of the shopkeeper's hiding place went into a summary of a
text that shows her carrying something out; and the Continuity checker reads the chapter against
its own scenes' record alone, so a chapter that contradicts a published one is checked by no one.

| PR | Content | Proof |
| --- | --- | --- |
| 11 `feat/story-ledger` | The Canon keeper's first part (spec §5): at a chapter's checkpoint it reads the version's text alone, with no canon and no record, and writes the story's **register**: the threads the chapter opens (a clue, a sound with no source, a promise, an act left unexplained), the earlier ones it pays off, and the facts of time, age and kinship it states, each with its quote and who states it. The **summary** is written from the text alone too; a check of its own names any sentence of it the text does not bear out, and the summary is written once more against them. The plans, the draft and the revisions see the open threads and the facts. The Continuity checker sees them, checks every statement of time, age and kinship against them, the canon and the chapter itself, and in the arc's last chapter reports each thread still open, which the Author pays off or rejects with its reason (T16); that chapter's checkpoint lists the threads left open with the reasons. The reviewers' and the keeper's reply cap goes from 16,384 to 32,000 tokens (PR 10's finding). The register is a table of its own: no schema step | Engine tests on the scripted port: one register per version, read by the next chapter's prompts; a story from before the register given one on the way; a run killed at each new call resumes. Live: known-answer probes on G2's own chapters, on Claude Sonnet 5.5 and Qwen 3.8 27B — chapter 1's summary without the chapel the canon put in it; its register with the logbook's other hand and the footsteps; chapter 2 checked against it finds the line gone empty and "a few years ago" against twenty-eight years. A two-chapter story on Qwen to its end |
| 12 `feat/story-craft` | The scenes and the lines (T17). A scene's intent names whose goals collide in it and what it may cost someone; the Author's rules pace the reveals and let no conflict end without a price. The characters act on a plan or a boundary instead of announcing it, open no reply on thanks or agreement the moment does not call for, and let a want show in the deed; the prose's rules say the same. A **staging pass** of its own, cheap and on the text alone, reads who is where and holds what, whose pronoun and whose kin, and marks a line that announces a plan or opens on thanks; its notes ride with the Continuity checker's | Engine tests of the prompts and of the pass's notes. Live: the pass on G2's chapter 2 finds "son of Marya, who left with him"; a story on Qwen; then **G2 again** on Claude Sonnet 5.5 from the same sketches, read by the owner against the first (T18) |

**PR 11, as built (2026-11-04).** The owner decided four forks (§6, T15–T18). At each
checkpoint the Canon keeper reads the version's text alone and writes its register — threads
opened and paid off, facts of time, age and kinship with who states them, ids given by position
— and then the summary, which a check of its own reads against the text; with a fault named,
the summary is written once more against it. The plans, the draft and the revisions read the
accepted chapters' registers, and in the arc's last chapter a line asks for each thread to be
paid off or left open on purpose. Where the table gives the check to the Continuity checker, it
is a call of the keeper's own (`…/register-check`, on the chapter's text and the register
alone) beside the checker's from chapter 2, its findings written in the checker's batch: given
to the checker first, the register was held to nothing on Qwen 3.8 27B, so the checker's prompt
is main's again. In the last chapter that check reports each thread still open, and the last
landing lists the threads the register still holds open, each with the Author's reason where it
decided against a note. A table of its own, no schema step; a story begun before it is given
its registers on the way; the reviewers' cap is 32,000. **Live:** on G2's own chapters, three
runs a probe, the keeper's reading held on both models — the old summary's chapel named and
kept out of the new one, the footsteps a thread, "a few years ago" a fact; the logbook's other
hand was a thread on Sonnet in 3 of 3 once the wording was sharpened, on Qwen in 0–1. Chapter 2
held to the register found the footsteps still open in 3 of 3 and the twenty-eight years in 2
on Sonnet, none of the three faults on Qwen. A two-chapter story on Qwen in Russian finished in
the chat at the default budget, 884 calls and $2.55: chapter 2 paid off five of chapter 1's
seven threads, the landing listed the four still open, and the keeper's check reported none of
them; the harness's three-hour limit (the app's is six) stopped chapter 1 once, and the woken
turn asked before anything resumed. The figures are in
[journal/stories.md](journal/stories.md).

### M3 — the evaluation

Before it starts, two judges besides the owner are found, and Claude drafts the premise
candidates for the owner to choose from (spec §15).

| PR | Content | Proof |
| --- | --- | --- |
| 13 `feat/story-eval` | The hidden runner: no line in `--help`, the single-instance lock skipped, stories written into its output directory and not into the data; the systems F, B0 and A1–A4 as run options; the export of the markdown and the JSON trace; the 12 premises frozen in the repository with their SHA-256 | Unit tests of each system's wiring; a live run of one premise through every system |
| 14 `feat/story-judge` | `tools/fabula_judge.py`: pairs shown blind in random order, verdicts as JSON, the binomial test; the LLM panel of other model families through OpenRouter, both orders; the calibration check against the three humans' majority | `--self-test` against fixtures, run in `lint` like the other tools' |

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
- **A chapter's cost grows with the story** (found in G2, PR 9). A character's call carries
  every reflection it has written since the story began, and the World Master's the scene's
  record, so the average character call grew from 5,800 prompt tokens in chapter 1's first scene
  to 17,900 in chapter 2's, and a chapter on Claude Sonnet 5.5 took 5.8 million tokens with no
  cache read: nothing asks Anthropic to cache. A memory window per character (the newest
  reflections and a summary of the rest) and Anthropic's cache breakpoints are the two levers;
  M4's long form needs the first.
- **CLAUDE.md sits near its cap** (30,000 bytes). PR 1 added the story engine's row to
  the document map and its status line, and dropped the release pipeline's, which had
  stopped being recent; each later PR keeps to the one line.

## 6. Forks this plan adds

The owner accepted the first three recommendations on 2026-10-31.

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

PR 7 added two more, since the plan's row and the spec disagreed on the first and the plan
left the second open. The owner accepted both recommendations on 2026-11-02.

- **T4. What "deleted with its chat" means** (PR 7). mindfork has no hard delete (spec §5):
  a deleted chat is marked `is_hidden`, and its `files/` and `workspace/` stay on disk.
  (a) Soft, as the chat's other data *(recommended)*: a story counts as deleted while its
  chat is hidden, which is read from the chat and never stored twice; its files stay.
  (b) Hard: hiding the chat removes the story's database and call log, the app's only
  irreversible delete. **Owner's decision (2026-11-02): (a).**
- **T5. How a story names its chat** (PR 7). (a) By its path,
  `stories/<chat-id>/<story-id>.db`, with its call log at
  `story-logs/<chat-id>/<story-id>.db`, as a chat's `files/<chat-id>/` and
  `workspace/<chat-id>/` do; a chat may hold several stories, and the schema does not change
  *(recommended)*. (b) One story per chat, `stories/<chat-id>.db`. (c) `stories/<story-id>.db`
  with a one-row table naming the chat. **Owner's decision (2026-11-02): (a).**

PR 8 added two more: T1 set the budget's unit but not its amount, and the plan said nothing
of the background slots. The owner accepted both recommendations on 2026-11-02.

- **T6. The chapter's default budget** (PR 8). (a) 2,000,000 tokens, no dollar cap unless
  one is set *(recommended)*: about twice the 718,865 tokens a chapter of three scenes took on
  Qwen 3.8 27B in PR 6, so a chapter of four with its reruns and revisions fits; a default in
  dollars would bound OpenRouter alone, since Anthropic reports no cost. (b) 2,000,000 tokens
  and $5. (c) No limit beyond the timeout. **Owner's decision (2026-11-02): (a).**
- **T7. Whether a story run takes a background slot** (`tools.subagent_background_max`, 2 by
  default; PR 8). (a) Yes, as a background dialogue does *(recommended)*: one bound on the
  background load; a chapter holds its slot for hours, so at the default one is left for
  sub-agents and dialogues; a story has one run at a time either way. (b) No: stories counted
  apart, bounded only by one run per story. **Owner's decision (2026-11-02): (a).**

PR 9 added three more, and how its gate is run. The owner accepted every recommendation on
2026-11-02.

- **T8. Where the named scene rerun goes** (PR 9). P2's fourth answer, *rerun a named scene*, needs
  the main line to step back: the scenes after the one named, the chapter's text and its review
  made again, under addresses of their own, since every store is append-only. (a) A pull request of
  its own after PR 9 *(recommended)*: PR 9 is large already and G2 does not need it; until then a
  request to rerun a scene reads as `change`, the Author revises the text with it, and the tool says
  the scene itself is not played again. (b) In PR 9, as the plan had it. **Owner's decision
  (2026-11-02): (a)**, as PR 10 `feat/story-rerun`.
- **T9. G2's model** (PR 9): one model for the chat and every role. (a) Claude Sonnet 5.5
  *(recommended)*: about $2 a chapter at PR 6's 720,000 tokens, and thinking off for the
  characters (`between_tools`). (b) Claude Opus 5.5: about twice the price, and it always thinks.
  (c) Qwen 3.8 27B: about $0.30 a chapter, weaker prose; it runs the tools' live checks anyway.
  **Owner's decision (2026-11-02): (a).**
- **T10. G2's language** (PR 9). (a) Russian *(recommended)*: the owner reads it in their own
  language, and the profile's Russian tool texts are exercised; the prose is Russian under English
  role prompts before §15's measurement after M3. (b) English. **Owner's decision (2026-11-02):
  (a).** G2 is run by Claude Code through the orchestrator, the human's replies scripted, and the
  owner reads the chat's transcript and the story.
- **T11. The chapter's default budget, again** (PR 9, during G2). T6's 2,000,000 tokens rested on
  PR 6's harness chapter; the app's own shape (four scenes, eight rounds) spent 2,000,000 by the
  third of four scenes on Qwen 3.8 27B ($0.55), and a whole chapter took 5.8 million on Claude
  Sonnet 5.5, so out of the box no chapter finished on any model tried. (a) 6,000,000 tokens
  *(recommended)*: a Qwen chapter fits with room and a Sonnet one just; at most about $13 a
  chapter on Sonnet and $1.60 on Qwen spent unasked. (b) Keep 2,000,000, which the user raises,
  told where by `story_continue`'s refusal. The owner first kept (b) on the Sonnet figure alone,
  then chose (a) on 2026-11-02 once the Qwen run showed the same. **Owner's decision
  (2026-11-02): (a).**

PR 10 added three more. The owner accepted every recommendation on 2026-11-03.

- **T12. A take's budget** (PR 10). Playing scene 2 of four again is most of a chapter: some 4
  million tokens on Claude Sonnet 5.5, where the first making had spent most of the chapter's
  6,000,000 already. (a) Each take has the chapter's budget of its own, counting the chapter's
  calls in the runs after it began *(recommended)*: the person asked for the rerun, and the
  budget is there to stop a chapter that runs away, not one asked for again; each further
  take is spent at the person's word. (b) One budget for the chapter and all its takes: on
  Sonnet a rerun would nearly always stop at it, and the person would raise it, told where
  by the refusal. **Owner's decision (2026-11-03): (a).**
- **T13. The live check's model** (PR 10): a chapter to its checkpoint, a scene asked for
  again in the chat, the take to its own checkpoint. (a) Qwen 3.8 27B through OpenRouter, as
  PR 9's live checks ran, about $1.50 *(recommended)*. (b) Claude Sonnet 5.5, as G2, some
  $20–25: better prose, which a check of the mechanism does not need. **Owner's decision
  (2026-11-03): (a).**
- **T14. Whether a woken turn may resume a stopped run** (PR 10, found live). A run that
  stops short of a question — stopped by the person (`F6`), at its time limit, at a call
  that gave nothing usable — lands with a notification that wakes the assistant, and
  `story_continue` refused a woken turn only an *answer*: with no question waiting it
  resumed. On Qwen the woken assistant did so after a failed step, unasked. Nothing stopped
  it after the person's own stop either, or a step that fails every time from looping —
  stop, wake, resume, stop — until the chapter's budget was spent. (a) A woken turn
  resumes nothing; it says where and why the story stopped and asks, and the person's
  word resumes *(recommended)*: one rule for every stop, the loop closed, at the cost of a
  message after a transient failure the person is told of anyway. (b) Refuse it only
  after the person's own stop: the loop stays. (c) Allow one woken resume per stopped
  step. (d) Leave it. **Owner's decision (2026-11-03): (a)**, in PR 10.

The readers' findings added four more (§4, *Before M3*). The owner accepted every
recommendation on 2026-11-04.

- **T15. When the findings are acted on, and how they are cut.** (a) Two pull requests before
  M3 *(recommended)*: the bookkeeping first — the register of threads and facts, the summary
  from the text alone, the new chapter checked against the published ones, the reviewers' cap —
  then the craft: the rules of the scenes and the lines, and the staging pass. (b) The
  bookkeeping before M3 and the craft after it, so M3 measures the characters as G2 had them.
  (c) One pull request. **Owner's decision (2026-11-04): (a).**
- **T16. Threads still open at the last chapter.** (a) In the arc's last chapter the Continuity
  checker reports each thread still open, the Author pays it off or rejects the note with its
  reason, and that chapter's checkpoint lists the threads left open with the reasons
  *(recommended)*: a thread left open is the Author's choice, said aloud. (b) Only listed at the
  last checkpoint, with no note and no revision. (c) Every thread must be paid off.
  **Owner's decision (2026-11-04): (a).**
- **T17. How strict the rules of the scenes and the lines are.** (a) In the Author's and the
  characters' prompts, a scene's intent naming whose goals collide and what it may cost, and the
  staging pass marking a line that announces a plan or opens on thanks *(recommended)*. (b) In
  the prompts alone. (c) Fields of their own in the plan (the collision, the price) and a fifth
  measure in the pick's rubric, which changes spec §7. **Owner's decision (2026-11-04): (a).**
- **T18. How the findings are proved live.** (a) Known-answer probes on G2's own chapters, a
  story on Qwen 3.8 27B in each pull request (about $3–5 each), and once both have landed G2
  again on Claude Sonnet 5.5, for the owner to read against the first (about $35–45)
  *(recommended)*. (b) Without G2 again. (c) G2 again alone. **Owner's decision (2026-11-04):
  (a).**

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
