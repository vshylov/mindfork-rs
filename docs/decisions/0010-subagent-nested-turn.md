# ADR 0010 — The sub-agent as a nested turn, its transcript on the call's record

**Status:** accepted (2026-08-23). Design and the decided forks —
[docs/research/subagent-chats.md](../research/subagent-chats.md). Implemented
from PR 2 of that track on (`feat/subagent-run`):
[src/app/orchestrator/generation.rs](../../src/app/orchestrator/generation.rs)
(`TurnLoop::run_subagent`, `TurnShared`, `RoundSink`),
[src/entities/subagent.rs](../../src/entities/subagent.rs) (`SubagentRun`),
[src/features/tools/subagent.rs](../../src/features/tools/subagent.rs) (the
schema-only tool). Related: [ADR 0004](0004-engine-contract-multi-provider.md)
(the client-side agentic loop), [ADR 0006](0006-data-schema-versioning.md)
(the additive record field; the first real settings step).

## Context

`call_subagent` (spec §9.3.2) was one request with a system message and a
single user message — no history, no tools, a token cap, a 60-second timeout —
returning a string. The user asked for a sub-agent with **every tool and
capability of the main agent except creating sub-agents**, whose call shows in
the chat list **as a chat nested under the one that made it**, renameable,
searchable, openable read-only, and **inseparable from the parent**: stored in
the parent's file, removed only when the spawning exchange is taken back or
regenerated.

Two facts of the codebase decided the shape. First, a `Tool` sees only
`ToolContext`; the registry, the dangerous-call confirmation channel, the UI
sender and the round budgets live in the orchestrator's agentic loop
(`TurnLoop`), which is `app`-layer code that `features` cannot reach (FSD). A
tool-using sub-agent therefore cannot be implemented *inside* the tool. Second,
while a turn runs its messages exist only inside the generation task and land
on `Chat` in one piece at the end; a child that is part of the turn is part of
that.

## Decision

1. **A loop-executed tool.** `call_subagent` keeps a `Tool` impl for the
   schema, the catalog and the profile toggle; the loop recognises the name in
   `resolve_call_result` and runs the sub-agent itself — after the disabled
   gate and the confirmation gate, inside the turn's cancellation, with a
   `ToolCall` card and a `ToolCallRecord` like any call. The precedent is the
   conversation-control pair (spec §9.3.3); the difference is that this tool
   still goes through every ordinary gate.
2. **The sub-agent is a child `TurnLoop` over the turn's shared part.** What
   every loop of one turn shares — backend, registry, the UI sender, the
   confirmation receiver and the "approved for this turn" set, the generation
   id, the limits — is `TurnShared`, owned by the task; the parent's loop
   borrows it, and the child borrows it from the parent for the duration of
   the call (sound: the parent is suspended inside `execute_call`). The child
   has its own request, context, allowed set, counters, a child cancellation
   token and a **muted event sink** (`RoundSink`): nothing of its stream
   reaches the parent's bubble except the token counter, re-based on the
   parent's. Every behaviour of the main loop — confirmation popups, thinking
   signatures, control tools, effects, images — is therefore the sub-agent's
   too, with no second loop to keep in step. A third loop beside the silent
   background one was rejected for exactly that reason.
3. **Tools and environment inherited, history not.** The child's set is the
   turn's effective set minus `call_subagent` (no nesting — and a loop below
   the top refuses the name regardless), `history_read`/`history_search` (the
   parent's folded history) and the self-model family plus its injection (the
   profile persona's identity). Its context is the parent's, cloned, under its
   own persona and sampling: the same attachments, project root and change
   journal, other-chats scope; its prompt carries the parent's environment
   blocks. Effects go to the chat they describe — `SetSystemMessage`/
   `SetSamplingOverride` to the run, `AddAttachment` to the parent.
4. **The transcript lives on the record.** `ToolCallRecord.subagent:
   Option<Box<SubagentRun>>` — persona, messages (the run's rounds exactly as
   any chat stores them), title, name, outcome, tokens, an id for `chat://`
   references. Additive (ADR 0006 F12): old records read `None`. Being inside
   the assistant message that made the call, the run travels with the
   exchange: `Ctrl+E`/`Ctrl+R` move it into `Chat.deleted`, hiding the parent
   hides it, request replay ignores it (`record_to_api` reads only the wire
   fields — a request with a stored run is byte-identical to one without).
5. **The child lands with the turn** (stage 1). No mid-turn channel: the run
   is attached to the record in the task and reaches `Chat` at `handle_done`
   with the parent's messages; a cancel lands the partial run as `Cancelled`,
   a timeout as `TimedOut`, a round budget as `RoundLimit`. Until the list
   snapshot carries the child, the `chat://` address in the card is plain
   text — the existing "resolves or it is not a reference" rule — which also
   prevents a click that would have cancelled the turn. Live viewing of a
   running child is a later stage, built on a side table fed through the
   turn's one result channel.
6. **Settings: one knob for the whole run.** `tools.subagent_timeout_secs`
   (one request) is replaced by `subagent_run_timeout_secs` (600 s, rounds and
   tool calls together) and the per-round reply cap's default rises to 4096 —
   through `SETTINGS_SCHEMA` 1→2, the scaffold's first real step: an
   unchanged old default is dropped so the new one applies, a value the user
   typed is carried over. Round budgets are the turn's own `max_tool_rounds`
   and `workspace.max_rounds`, applied per loop.

## Consequences

- A request from a chat that never delegated is unchanged; the only new
  stored shape is a field on a record, and old files need no migration for it.
  The chat-file migration the track does carry (`CHAT_SCHEMA` 2, PR 3) is for
  **old calls**: it synthesizes a run from what the record already holds, so
  the migrated file is a superset of the old one.
- The sub-agent's cost is visible: the counter grows with its tokens, the run
  records them, and the settings say `max_tool_rounds` applies to each run.
- `Message` is a recursive type (a run holds messages that hold records);
  `ChatActivated` and every save clone the run with the parent — the same
  class of cost as images in messages, and the first thing to measure if a
  chat with many long delegations ever feels slow.
- Stage 1 shows nothing of a running child but the counter; the list, the
  read-only screen, search and titles follow in the track's later PRs, the
  live view after a live run argues for it.
