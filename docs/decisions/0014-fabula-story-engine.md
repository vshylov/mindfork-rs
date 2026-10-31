# ADR 0014 — Fabula: a story engine inside mindfork, used from the chat through tools

**Status:** accepted (2026-10-31) — the owner chose option A (inside mindfork-rs, against
the research's recommendation of a separate repository), the tool form, one model for
every role, and the answers to every open question (*Decision* below). Not built yet; the
stages are [docs/fabula.md](../fabula.md). The design and its evidence —
[docs/research/fabula-engine.md](../research/fabula-engine.md) (*the spec*). Related:
[ADR 0004](0004-engine-contract-multi-provider.md) (the engine contract the roles call
through), [ADR 0006](0006-data-schema-versioning.md) (the story database's schema),
[ADR 0010](0010-subagent-nested-turn.md) and [ADR 0011](0011-dialogue-directed-run.md)
(the background seat a chapter runs in).

## Context

Earlier role-based writing pipelines lost to one agent iterating alone. The spec proposes
a design that answers their failures: characters with only the context they witnessed,
one writer per store (a World Master for the event log, an Author for the text), K
independent branches per scene, and read-only review. Whether it beats one agent at equal
cost is unproven, and the spec's M3 is the experiment that decides it (spec §11).

What mindfork has decides most of the shape (spec §14, checked against the code at 0.23.1):

1. **Provider wires and a narrow engine contract.** `EngineBackend::chat_stream` serves
   seven providers; nothing in `shared/api` reaches above it except in tests.
2. **A background seat.** `start_subagent` and `start_dialogue` run beside the turn under
   the app's session budget, land on their chat by id, apply files to it
   (`ChatEffect::AddAttachment`, `AddChatFile`), and deliver a task notification that can
   wake the assistant. Quitting cancels them.
3. **A notification is not consent.** It is a `System` row in the feed and user text on
   the wire, under a prefix saying it grants nothing, and `UserMessageTimes.answering` is
   empty on a turn it woke.
4. **No schema mode on any chat wire.** Structured replies go through tool schemas with
   `tool_choice` fixed at `auto`, and forcing a tool is no longer an option: four current
   Claude models refuse it on every request.
5. **Model-facing text is localized per profile** (axis A), with the en and ru key sets
   held equal by a test.

The research weighed four options (spec §14): a feature inside mindfork (A), a separate
repository (B), a workspace member before the experiment (C), and B now with an adoption
after M3 (D, recommended). The owner chose A.

## Decision

1. **Inside the one crate.** No workspace and no second crate on crates.io. The types are
   in `entities/story.rs`, the store in `shared/storage/story/`, the engine in
   `features/story/`, the tools in `features/tools/story.rs`, the background payload in
   the orchestrator.
2. **Used from the chat through three tools.** `story_create` builds the canon from the
   attached sketches and the conversation. `story_continue` runs or resumes a chapter as
   a background run, the third payload beside the sub-agent's and the dialogue's.
   `story_status` reports where the story stands. A story run is `RunKind::Story`, so
   `CHAT_SCHEMA` moves 7 → 8 with a stamp-only step.
3. **One model.** Every role runs on the model selected in the app, each call taking the
   app's session budget. Thinking stays a per-call setting. Models per role are deferred
   to the far term, together with the client factory they would need.
4. **Structured output by schema-constrained decoding.** An output-schema field on
   `ChatRequest` is mapped by each wire to its provider's mode. The engine's validator is
   the contract whatever produced the reply; a model without such a mode takes the
   schema in the prompt and one repair. There is one schema dialect, the modes'
   intersection, and no forced tools (spec §8).
5. **The human in the loop by messages.** The engine pauses only at defined points, and a
   pause ends the run. The question rides the run's notification. Only the rows the user
   typed after it, read verbatim by the orchestrator and interpreted conservatively, can
   resolve it, so a woken turn approves nothing. A question waits until it is answered.
   There are no popups (spec §14).
6. **Prompts in English.** The role prompts are an explicit exception to axis A, and the
   story's language is a per-story setting. Text for the human is written in the
   profile's language, and the tools' own descriptions take en and ru as every tool's do.
7. **A story is mindfork's data.** Its database lives in the data directory, with a schema
   version, inside backups, counted by `mindfork stats` and deleted with its chat. Its call
   log is a sibling file outside backups.
8. **Off by default until M3.** The `tools.story` setting keeps the tools out of the
   catalog, as `tools.subagent_background` does for `start_subagent`. The README and the
   CHANGELOG call the feature experimental.
9. **M3 decides the simulation.** The systems of spec §11 are run options of a hidden
   runner, the judging is a script under `tools/`, and three people calibrate an LLM
   panel. If the simulation loses to A1, the tools keep A1's pipeline (Author and
   reviewers) and the simulation is removed.

## Consequences

- **The experiment ships inside the product.** Its code meets mindfork's bar from the
  first pull request: live smokes, 80% coverage on new code, journal entries. It stays
  unreachable until `tools.story` is turned on.
- **The chat slows while a chapter runs** on a single-session engine, since the story's
  calls and the chat's turns share it. A chapter takes hours, not minutes, so it has a
  timeout of its own instead of the dialogue's 30 minutes.
- **Two new stored formats.** The chat schema's step and the story database's version
  follow ADR 0006, with the release checks AGENTS.md §6 asks for.
- **An exception to axis A** has to stay visible: the plan measures after M3 whether
  Russian prose loses anything under English prompts.
- **The output-schema field is engine groundwork** that any later structured caller can
  use, not only Fabula.
- **M3 needs people.** Two judges besides the owner, for the 12-pair calibration set.
- **Later options stay open.** Models per role come with a client factory moved down into
  `shared/api`, the tasks screen can show a story run, and the bridge could offer the
  tools.
