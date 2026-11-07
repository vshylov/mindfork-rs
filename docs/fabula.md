# Fabula — a story engine used from the chat: track plan

**Status:** in progress — M0 done (PRs 1 and 2, Gate G0 GO); M1 done (PRs 3–5, Gate G1 GO, 2026-11-01); M2 done (PR 6, the Author's loop, 2026-11-01; PR 7, stories in mindfork's data, 2026-11-02; PR 8, the story run in the background, 2026-11-02; PR 9, the story driven from the chat, 2026-11-02; Gate G2 GO, 2026-11-03; PR 10, a scene played again, 2026-11-03); before M3, the readers' findings settled (PR 11, the story's register; PR 12, the craft of the scenes and the lines; a chapter's choices standing for its later steps; a map every scene can reach; 2026-11-04 to 2026-11-05); M3 begun (PR 13, the hidden runner and the twelve premises frozen, 2026-11-06). Plan of 2026-10-31. Every fork of the research is decided by the owner, and
so are the twenty-three this plan adds (§6) and where development runs (§4, *The development
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

**PR 12, as built (2026-11-04).** By T17 (a) the Author's rules gain two: a scene is a
collision, its intent naming whose goals collide in it and what it may cost someone, and no
conflict ends without a price, the version where it is paid the one kept; and the reveals are
paced, a secret spent where it costs its keeper most, one at a time. The intent's description
in the plan says the same. The characters' rules and the prose's say a line is a deed: a plan
or a boundary acted on rather than announced, no reply that opens on thanks, apology or
agreement the moment does not call for, a want shown in what is done — and the prose may cut
or show in a deed a line of the record that does otherwise. The **staging pass**
(`…/staging`) runs on every chapter beside the Continuity checker, its notes in the checker's
batch under four kinds of their own (`staging`, `referent`, `announced`, `thanks`): no new
reviewer, no schema step. Where the table says the text alone, it reads the published
chapters' summaries before the text: on the text alone it took a brother chapter 1 named for
a stranger. A list of the chapter's ties of kin before the findings was tried and not kept.
**Live:** on G2's own chapter 2, three runs, the pass found "son of Marya, who left with him",
a reply opening on thanks and a line that announces in 3 of 3 each on Sonnet 5.5, and in 0, 2
and 0 on Qwen 3.8 27B. A two-chapter story on Qwen finished in the chat, 775 calls and $2.28.
**G2 again** on Sonnet 5.5 from the same sketches finished at $32.57 against the first's
$31.18, chapter 2 at the first G2's 14,000,000-token ceiling: every scene planned with its
collision and its price, the secrets spent one at a time across both chapters, all six of
chapter 1's threads paid off, the staging pass's 41 notes 35 accepted, and no reply opening
on thanks where the first opened five (and three on an apology); for the owner to read
against the first (T18). The run found the landing's assumptions answering one doubt nine
ways, each step deciding it again without the earlier answers. The figures are in
[journal/stories.md](journal/stories.md).

**After PR 12: a chapter's choices stand (2026-11-04).** Where the Author could have asked
the human and did not, it records the question and the answer it chose (spec §14, *Pauses*):
at the outline, at each scene's plan and at each pick. No step sees another's answers, so the
next one meets the same doubt and decides it again. G2 again decided who signed the closure
order at nine of chapter 2's steps, nine ways, and whether Galina knows Timur for a local at
six of chapter 1's; PR 12's Qwen story decided what Nikita's last logbook entry says at
three. Both models, both chapters: the cause is the loop's. The owner asked for the fix after
#874 merged (2026-11-04); the forks below are the agent's, open to the owner's review.

- **What fixes it.** (a) The chapter's choices so far reach every Author prompt of the chapter
  — they stand, and the plans, the picks and the prose go by them — and the landing lists each
  question once, with its latest answer *(taken)*. (b) The landing alone keeps the last answer:
  the scenes would still be planned on nine. (c) Every doubt asked once, at the outline: the
  ones met later would be lost.
- **Whose choices.** (a) The chapter's own, from every run and take of it *(taken)*: an earlier
  chapter's were the human's to read at its checkpoint, and live on in its text, its summary
  and its register. (b) Every chapter's.
- **A change.** (a) A later step may answer a question again when what was played or what the
  human asked calls for it, writing the question as it stands; the latest answer stands
  *(taken)*. (b) Never: a choice made before a scene was played would outrank its record.
- **The same question.** (a) By its words — letters and digits, case ignored — since the step
  copies the question it is shown *(taken)*. (b) A model's call judging which questions are
  one: a call per landing, with errors of its own.

Proof: engine tests on the scripted port (the choices in the next steps' prompts, after a
resume too; one line per question at the landing); live, a two-chapter story on Qwen 3.8 27B
from G2's sketches, its questions answered again counted against PR 12's.

As built (2026-11-05): the live story answered no question again — none of chapter 1's 2
assumptions, none of chapter 2's 4 — but its plans and picks recorded almost no new ones. Their
prompts, replayed with and without the list, showed why: told only that its choices stand, the
Author recorded a seventh of its new questions. A step whose reply records assumptions (the
outline, the plans, the picks) is now told to record every question the choices do not
answer, and leave out only the ones they do; measured on the same prompts, the repeats stay
gone and new questions come back to about half PR 12's rate. The draft and the revisions read
the list without that sentence. Chapter 2 stopped at its third scene on its token budget, and
the run found a canon whose places no road connects let through its check: the journal has the
counts, the wordings measured and rejected, and the defect
([docs/journal/stories.md](journal/stories.md), *a chapter's choices stand*).

**After PR 12: a map every scene can reach (2026-11-05).** The choices' live story met a
canon Qwen 3.8 composed from the sketches whose four places had no route at all, and the
canon's check let it through: it asks that a route lead somewhere, not that a place be
reached. Chapter 1's second scene, planned at the lighthouse, was refused by the world at
its opening 51 minutes in, and the refusal ended the run as one that could not start,
though it had kept a scene. The owner asked for the fix on 2026-11-05; the forks below are
the agent's, open to the owner's review.

- **The map.** (a) Every place reachable from every other along the routes, back or around
  *(taken)*: a scene's opening brings everyone in it to its place from wherever they are,
  and `story_create` already asks for the way back to be listed; of the eight canons composed
  live since PR 11, the seven with routes all pass it. (b) Every place reachable from every character's
  start: a place of no return would pass, and every plan bringing someone out of it would be
  refused. (c) The starts reachable from one another: a place no one can reach would pass.
- **A plan.** (a) Held to the world by its opening: the opening's events admitted against
  the world before the scene plays, so a plan the world refuses is asked for once more in
  the world's words, and a rerun the pick asks for, refused, keeps the cast and the place of
  the scene it reruns *(taken)*. (b) A reachability check of its own beside the opening's:
  one rule in two places, which drift.
- **A refusal while a scene plays.** (a) A stop at the scene's step, in the world's words,
  told as the engine's fault *(taken)*: a plan held to the world leaves no other cause. (b)
  Every branch failed, so the scene's reruns and then the human's question (P3): reruns of a
  scene that cannot open, and a question to the human about an engine's fault.

Proof: canon tests (the roadless map's problems, a place with no way back, a one-way ring
that passes); engine tests (the plan held to the world by its opening, the refused rerun,
the stop at a scene); live, the slip put into Qwen's first `story_create` of a fresh chat,
the map mended from the refusal; the stored roadless story resumed.

As built (2026-11-05), as forked. Live, Qwen 3.8 27B mended all five roadless maps the smoke put
into its first `story_create` with a map, four on the call after the refusal; the stored
roadless story, resumed, stops at its second scene in the world's words, with no call
([docs/journal/stories.md](journal/stories.md), *a map every scene can reach*).

### M3 — the evaluation

Before it starts, two judges besides the owner are found, and Claude drafts the premise
candidates for the owner to choose from (spec §15).

| PR | Content | Proof |
| --- | --- | --- |
| 13 `feat/story-eval` | The hidden runner: no line in `--help`, the single-instance lock skipped, stories written into its output directory and not into the data; the systems F, B0 and A1–A4 as run options; the export of the markdown and the JSON trace; the 12 premises frozen in the repository with their SHA-256 | Unit tests of each system's wiring; a live run of one premise through every system |
| 14 `feat/story-judge` | `tools/fabula_judge.py`: pairs shown blind in random order, verdicts as JSON, the binomial test; the LLM panel of other model families through OpenRouter, both orders; the calibration check against the three humans' majority | `--self-test` against fixtures, run in `lint` like the other tools' |

**PR 13's forks (2026-11-05).** The owner accepted every recommendation below on 2026-11-05
(§6, T19–T23). Claude drafted twenty premise candidates against §15's rubric, five a genre,
each a canon the canon's check passes with a planted token on every sheet, for the owner to
choose twelve. Choices the agent made, open to the owner's review:

- **A3** runs no reviewer at all: no Reader, Continuity checker, staging pass or keeper's check,
  and so no revision. The keeper's register and summary are still written at the checkpoint.
- **A4** gives every character every event of its branch, other people's thoughts included,
  while the sheets stay their own.
- **A seed** moves the routine-break draws. It is not sent as the requests' `seed`: sibling
  branches send byte-identical first prompts, and one seed for both would play them alike on a
  provider that honours it. Seed 0 is today's draws, so a story made before stays replayable.
- **The runner's engine** is named by its own options, the smokes' routes as a command's,
  rather than read from the app's settings, which would start a managed server beside the
  app's (spec §14). OpenRouter's provider may be pinned there, the runner's option alone.
- **B0** is given the canon as the Author is and the same rules of the prose. Only the way of
  working differs.

**Where it runs (the owner, 2026-11-05).** A run of days must not depend on the owner's
computer, its power or its line. The batch runs on a small cloud machine with no GPU, since
the model is reached through OpenRouter. It runs as a service that restarts itself, and no
Claude session has to stay up for it. A story resumes from its last recorded step, and the
batch run again skips the finished stories.

The runner, `mindfork story-eval`, is hidden: no line in `--help`.

- `story-eval premises` lists the frozen premises with their hashes.
- `story-eval run --premise ID|FILE --system F|B0|A1|A2|A3|A4 --out DIR` writes one story.
- `story-eval batch --out DIR` writes every premise × system × seed asked for:
  - by default F, B0 and A1 (T23) and seeds 1–3;
  - `--parallel 8` at a time;
  - B0 after its premise's F stories, to their average spend: in dollars where the calls are
    priced, in tokens where they are not. Anthropic names no price, so a Claude call is counted
    at the list price the runner holds for the model.
- The model is named by `--engine openrouter|compatible|anthropic|openai|gemini --model NAME`.
  The key is read from the variable `--key-env` names (`OPENROUTER_API_KEY` by default), and
  `--provider deepinfra` pins OpenRouter's routing.
- Each story is written to `DIR/<premise>/<system>/s<seed>/`: `story.db`, `calls.db`,
  `story.md` and `trace.json` (the premise's hash, the ending, the spend by role, the words,
  the scenes and picks, the notes, the leaks).
- `DIR/batch.jsonl` has one line per story run.

The host:

- A droplet with Ubuntu 24.04, 2 vCPU and 4 GB.
- The binary of the merged commit, built there or in `rust:1.96.0-bookworm`, at
  `/opt/mindfork/mindfork`.
- The key in `/etc/fabula-m3.env` (`MINDFORK_CLAUDE_API_KEY=…`, mode 600), written from the
  droplet's own variable of that name; a unit does not inherit a login shell's environment.
- A unit:

```ini
[Unit]
Description=Fabula M3 batch
Wants=network-online.target
After=network-online.target

[Service]
EnvironmentFile=/etc/fabula-m3.env
ExecStart=/opt/mindfork/mindfork story-eval batch --out /var/lib/fabula-m3 --engine anthropic --model claude-haiku-5-5 --key-env MINDFORK_CLAUDE_API_KEY --window 1000000 --budget-tokens 9000000 --parallel 8
Restart=on-failure
RestartSec=60

[Install]
WantedBy=multi-user.target
```

The unit names Haiku 5.5, T19's answer. Its chapter budget is 9 million tokens, not the app's
6: Haiku's tokenizer counts some 30% more than Sonnet's, and a closing scene may follow the
fourth. A Claude Code session on the droplet itself starts and watches the run (the owner,
2026-11-07): it builds the binary of the merged commit, writes the key file and the unit, and
stops the unit if the stories' dollars pass the $70 the owner approved. To watch it, `tail -f /var/lib/fabula-m3/batch.jsonl`. When it ends,
`rsync` the directory back. A reboot or a crash restarts the batch, which skips what ended. A
story whose call failed is tried again three times in a batch, and `systemctl restart` tries it
once more.

**PR 13, as built (2026-11-06).** The owner chose twelve of the twenty premises, three a genre:
`mystery-2`, `-4`, `-5`; `clash-3`, `-4`, `-5`; `villain-1`, `-3`, `-4`; `quiet-2`, `-3`,
`-5`. They are frozen in `src/features/story/premises/` with `SHA256SUMS`, and a test holds
them to their hashes and to §15's rubric. Each system is an option of the chapter's own
configuration, so each differs from F in one thing, and F's Author prompts are main's byte for
byte. B0 is `single.rs`: one session on `LlmPort::session`, rebuilt from the call log on a
resume. A power cut mid-pilot stopped five stories; the batch run again resumed each from its
stores. **Live:** the harbour fixture through all six systems on Qwen 3.8 27B, every story
finished and none leaking, $3.36 in all. T19's pilot ran F, B0 and A1 on `mystery-2`, on
Sonnet 5.5 ($22.49 at list price, F in 25 minutes) and on Qwen ($1.94, F in 130
minutes), for the owner to read. It found a reply cut and reported finished, so B0 no longer
keeps a version that breaks off mid-sentence. It also found most of Qwen's stories under the
1,200 words asked for. The owner read them blind on 2026-11-06, and F came first nowhere:
Sonnet's B0 best, Qwen's F on `mystery-2` a failure, A1 best on the harbour. That is one
reader and one premise a model, not M3's verdict. F's story polished by B0's own loop lost
to B0's as well, and the polish erased what the simulation had given it. The full run's
model is still open (T19).
The figures are in [journal/stories.md](journal/stories.md).

**After the pilot: the record is material (2026-11-06).** Two more diagnostics on F's Sonnet
story found where its prose fails:
- A line edit that kept every event mended words and left the rest.
- The draft call, replayed on the same record, showed the cause. The Author drafts the whole
  chapter in one pass, from a record of some 14,000 words (four scenes, English, with every
  character's thoughts) for 1,200–2,500 words of Russian, told to "tell what the record holds".
  It retold. Talks came out as "he asked whether…, she answered that…", the narrator gave a
  secret away before its keeper confessed, and a plan that came to nothing kept its scene.

The writing rules now say:
- the record is material, not a text to retell;
- what led nowhere is cut or told in a phrase, but a secret spent in such a scene is still
  told in direct speech;
- a secret enters the prose when a scene spends it;
- a talk keeps a line or two of direct speech and is never retold.

The last two are rules of the prose, and B0 has them too, as T21 gives it the same rules. The
first two belong to a record, so they go to F and A1–A4. The replayed draft kept the
simulation's scenes and told its secrets on time. This is one story, not a verdict: the full
run tests the rules on premises they were not drawn from.

**Claude at list price; the rules on `villain-4` (2026-11-06).** Anthropic names no price,
so B0 matched F in tokens on Claude. On Haiku 5.5, whose prompt past 100,000 tokens costs
five times more, B0 would have spent three times F's money. The runner now prices Sonnet 5.5
and Haiku 5.5 at list price, and B0 stops at F's dollars. **Live:** `villain-4`, seed 1, the
first premise the new rules were not drawn from. Haiku's F and B0 cost $0.66 each. Sonnet's F
replayed a scene and ran past the chapter budget of 6 million tokens; under 9 million it
finished at $16.80. A full run comes to some $1,300 on Sonnet and $50 on Haiku. The owner
reads the three stories blind, and the model stays open (T19).

**The last version checked (2026-11-06).** The owner read the three `villain-4` stories and
found Haiku far ahead of Qwen; an outside reading listed their slips. Traced to the calls that
made them, three of F's four slips and the fall under 1,200 words came from the last revision,
which spec §7's loop never reviews: `permit` left in English and Pavel's patronymic changed
(Haiku), a contradiction about what Galina knew and the closing scene cut (Sonnet). A theft
with no thief came from a scene's opening, which said the letters were gone while the world
never moved them. Four fixes, the owner's decision on 2026-11-06, each as recommended:

- **F1, the last revision reviewed.** It is reviewed and triaged like every version before it,
  and never revised. When the Author accepts more high-severity notes on it than on the version
  before, that version's text goes on as a version of its own. (b) One more revision: the same
  gap one step later. (c) As it was.
- **F2, the length counted.** The engine counts each version's words. A version outside the
  range asked gets a high-severity note of the engine's own, as a planted marker does, and the
  reviser is told the count of the text it revises.
- **F3, an opening holds no deed.** What has just happened is what the record holds, or what
  no character did; a thing taken, moved, opened or broken happens only in a scene that plays it.
- **F4, names kept.** A revision keeps every name, patronymic and form of address as the text
  has them unless a note asks otherwise. The staging pass reports a person named two ways and a
  word left in another language.

Proof: engine tests of the loop (the last version reviewed, the earlier text going on, a
resume that asks nothing again) and of the prompts; the opening's rule measured by replaying
Haiku's plan of scene 2; then `villain-4` again on Haiku, F and B0, for the owner to read.

As built (2026-11-06), as decided; B0's revision is also told its last version's count, as F's
reviser is (T21). Replayed four times each way, Haiku's plan of scene 2 claimed a change no
event made in three of four openings before and in none after. On `villain-4` again, F cost
$0.49 for 1,288 words, its revision drew no high note, and the letters left the drawer only
when Pavel took them. B0 kept 1,066 words at $0.51, 938 without its count. Haiku renamed both
leads in F's revision, consistently, so F4's rule does not hold on it. In all three F stories
the chapter ends before its goal, which nothing in the loop checks
([journal/stories.md](journal/stories.md), *the last version checked*).

**A chapter that reaches its goal (2026-11-06).** In five F stories, three on `villain-4` and
the pilot's two on `mystery-2`, one chapter reached its goal: Sonnet's on `mystery-2`, its last
scene the noon confession. B0 reached it in both of its `villain-4` stories. What the records
show:

- Every chapter played all four scenes; the Author never ended one early, and after the fourth
  the chapter is written whatever its goals.
- Ten of the twenty scenes kept ran to the eight-round cap, the characters playing for time.
- On `villain-4`, a story of one chapter, Haiku's picks put the outcome off "to the next
  chapter" four times in ten. The plan and the pick say "Chapter 1. Its goals", never that it
  is the arc's last.
- The world keeps no hour. Haiku's World Master struck nine in the first scene of both its
  runs, then a quarter and half past, with no director coming; Sonnet's four scenes stayed
  near one in the night.
- The pick scores tension, surprise, truth to character and consequences, not the goals.
- The Author narrows the arc's goal ("someone is betrayed before nine") into one character's
  decision ("Nina signs"), which the simulated Nina refused in all three runs.

Five changes, the owner's decision on 2026-11-06, each as recommended:

- **G1, the arc's place.** The plan and the pick name the chapter as N of M, and the arc's last
  as the one nothing waits after.
- **G2, the pick weighs the goals.** A fifth score, how far a version moves the chapter toward
  its goals; in the chapter's last scene the version that moves them furthest is kept unless it
  breaks a character's truth. (b) The prompt's words alone. (c) As it was.
- **G3, the hour.** Each scene's opening names the hour, later than the one before; where the
  goals hang on a deadline, no scene passes it before the last, which opens as it comes. The
  World Master's clock moves only as far as a scene's events take it, never past a deadline its
  pressure names. (b) A clock in the engine on the world's ticks: later, if this is not enough.
- **G4, goals by more than one road.** The chapter's goals are outcomes the world can reach
  whatever one character decides; the characters choose.
- **G5, a closing scene.** When the scene cap is reached and the Author does not end the
  chapter, one more scene may be planned, the chapter's very last, opening at the moment the
  goals hang on. (b) As it was. (c) A question to the human: none in M3.

The round cap stays at eight until these are measured. Proof: tests of the prompts, the pick's
schema and the closing scene; then F on Haiku, `villain-4` and `mystery-2`, seeds 1 to 3,
before and after, the chapters that reach their goals counted.

As built (2026-11-07), as decided, with one change the measurement asked for. Played like the
others, the closing scene cost Haiku 1.5 to 3.4 million tokens, a late scene's record being
long, and took three chapters of six past their budget of 9 million. The owner chose a light
one: one branch, half the rounds, no rerun. Then it cost 0.2 to 0.6 million, and every chapter
finished at 4.0 to 7.0 million tokens, as before at 4.6 to 6.6. Read by the agent, two chapters
of six reached their goals before (none on `villain-4`) and three after, two before and none
after not at all. After, 25 openings of 30 named the hour (11 of 24 before), no chapter struck
its deadline before its last scene (two before), and none put its outcome off. Six stories a
side is a direction, not a verdict ([journal/stories.md](journal/stories.md), *a chapter that
reaches its goal*).

The owner read the six `villain-4` stories blind and two outside readings agreed: the best was
the one whose closing scene opened in the anteroom at nine with the director there; the others
opened on the stairs at half past eight and in the laboratory at two minutes to nine. Told only
"the moment its goals hang on", the Author placed it loosely, so the closing plan now asks for
the deadline itself, neither before nor after, in the place where it falls due, with whoever
decides there, arriving in its pressure if not of the cast (the owner's go-ahead, 2026-11-07).
Replayed on the six chapters' stored closing plans, the anteroom at nine came six times of six,
from none of three; on `mystery-2` the selamlık at noon came two of six, from one of three,
since in two chapters the record had passed noon before the closing scene.

Then the run itself, outside any pull request: 36 pairs for each of the two decisions
(and the ablations, after a GO: T23), the calibration by three people, and the decision rule
of spec §11, recorded here and in the journal.

- **GO** (F beats B0 and A1) opens M4.
- **F loses to A1, and A1 beats B0:** a pull request switches the tools to A1's
  pipeline and removes the simulation; the long form continues on A1.
- **Neither beats B0:** the track stops. Whether the tools stay, running B0, is the
  owner's call then.

**The full run (2026-11-07).** On the owner's DigitalOcean droplet, a Claude Code session there
built `main` at `64c12819`, the code of PR #886, and ran the batch as a unit: all 108 stories,
F, B0 and A1 on the twelve premises and seeds 1 to 3, on Haiku 5.5, in 3 h 34 min for $55.76
(F $26.92, B0 $27.21, A1 $1.64). No story leaked a token, and no call was refused for its rate.
103 finished. Five F stories hold no accepted text:
- four stopped on the chapter budget of 9 million tokens, three of them in a rerun of the
  fourth scene and one at its summary's check, after a draft of 1,529 words;
- one failed when Haiku refused a character's reflection: the doctor of `clash-3` weighing her
  patient's heart, all 30 of the run's refusals in 22,884 calls, all that step, all `clash-3`.

B0 spent 5–12% above F's average on every premise, its last revision crossing the target.
Most stories came in under the 1,200 words asked: 20 of F's 32 texts, 29 of B0's 36, 12 of A1's
36. How a pair without an F story is judged is the owner's to decide before PR 14 reads any
([journal/stories.md](journal/stories.md), *M3's full run*).

The owner chose to complete the five (2026-11-07). A reflection the model refuses is now left
out and counted, and the branch plays on; the five were resumed from their stores on this
computer with that fix and a chapter budget of 15 million tokens. All five finished, at 1,031 to
1,529 words, for $1.41 more; `clash-3`'s doctor was refused nine times more, each left out. B0
had spent to the average of the finished F stories alone, so with the five counted it spent
15–36% less than F on `clash-3`, `clash-4`, `mystery-2` and `mystery-4`. The owner had those
twelve B0 sessions carried on to F's new average, for $3.36; now B0 spends 2–13% more than F on
every premise. The run holds 108 finished stories for $60.53: F $28.32, B0 $30.57, A1 $1.64
([journal/stories.md](journal/stories.md), *the five F stories completed*).

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

PR 13 added five more, M3's own. The owner accepted every recommendation on 2026-11-05.

- **T19. M3's model**, one for every system. (a) Qwen 3.8 27B through OpenRouter, about $1.1 a
  story of F (PR 12) and some $200 for all 216 *(recommended)*. (b) Claude Sonnet 5.5, some $16
  a story of F and $2,500 for all. **Owner's decision (2026-11-05): (a)**, then reopened the same
  day. On a small model every system may write poorly whatever the engine does, and stories
  too weak to tell apart are judged by chance, a NO-GO from noise. The runner does not depend
  on the model, so PR 13's live run is a pilot: F, B0 and A1 on one premise on Qwen 3.8 and on
  Sonnet 5.5, read by the owner. The full run's model is chosen after it. The owner's reading
  (2026-11-06) found Qwen's stories mostly weak, and its F on `mystery-2` a failure.
  Sonnet's were all good, at some $810 for the 108 stories; the owner finds that too much, and
  Haiku 5.5 is the cheaper candidate. Read on `villain-4` against Sonnet and Qwen, Haiku's
  stories were far ahead of Qwen's, with slips of language Sonnet does not make, at $0.66 a story
  of F against Sonnet's $16.80. **Owner's decision (2026-11-07): (c) Claude Haiku 5.5**, some
  $1.35 a premise and seed for F, B0 and A1, about $50 for all 108, run on a DigitalOcean
  droplet.
- **T20. The stories' language.** (a) Russian, as G2: the owner reads them in their own
  language, and the other two judges read Russian too *(recommended)*. (b) English, the
  prompts' language. **Owner's decision (2026-11-05): (a).**
- **T21. How B0 reaches F's cost.** (a) One session: a draft, then turns of critique and of
  revision, each seeing the whole session, until it has spent what F spent on average on the
  same premise, or until the next turn would not fit the model's window; the story is the last
  revision *(recommended)*. (b) Two revisions, as spec §10's cost model had it: cheaper, but not
  the equal cost §11 asks for. **Owner's decision (2026-11-05): (a).**
- **T22. A1's shape.** (a) F's loop without the simulation: the same outline and scene plans,
  each plan reading the plans before it as what happened, no branches and no pick; the draft
  written from the plans, the Author inventing every event; the same reviewers, revisions,
  register and summary *(recommended)*: it differs from F in one thing. (b) A draft from the
  canon and the outline alone, without scene plans. **Owner's decision (2026-11-05): (a).**
- **T23. When the ablations run.** (a) After a GO: F, B0 and A1 first, 108 stories, about $100
  and a day on the cloud machine; spec §11's decisions are made on them, and A2–A4 explain a GO,
  so they wait for one *(recommended)*. (b) All six systems at once, as this plan had it.
  **Owner's decision (2026-11-05): (a).** PR 13 still builds all six.

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
