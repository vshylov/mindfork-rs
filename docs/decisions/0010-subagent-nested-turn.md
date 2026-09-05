# ADR 0010 — The subagent as a nested turn, its transcript on the call's record

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
returning a string. The user asked for a subagent with **every tool and
capability of the main agent except creating subagents**, whose call shows in
the chat list **as a chat nested under the one that made it**, renameable,
searchable, openable read-only, and **inseparable from the parent**: stored in
the parent's file, removed only when the spawning exchange is taken back or
regenerated.

Two facts of the codebase decided the shape. First, a `Tool` sees only
`ToolContext`; the registry, the dangerous-call confirmation channel, the UI
sender and the round budgets live in the orchestrator's agentic loop
(`TurnLoop`), which is `app`-layer code that `features` cannot reach (FSD). A
tool-using subagent therefore cannot be implemented *inside* the tool. Second,
while a turn runs its messages exist only inside the generation task and land
on `Chat` in one piece at the end; a child that is part of the turn is part of
that.

## Decision

1. **A loop-executed tool.** `call_subagent` keeps a `Tool` impl for the
   schema, the catalog and the profile toggle; the loop recognises the name in
   `resolve_call_result` and runs the subagent itself — after the disabled
   gate and the confirmation gate, inside the turn's cancellation, with a
   `ToolCall` card and a `ToolCallRecord` like any call. The precedent is the
   conversation-control pair (spec §9.3.3); the difference is that this tool
   still goes through every ordinary gate.
2. **The subagent is a child `TurnLoop` over the turn's shared part.** What
   every loop of one turn shares — backend, registry, the UI sender, the
   confirmation receiver and the "approved for this turn" set, the generation
   id, the limits — is `TurnShared`, owned by the task; the parent's loop
   borrows it, and the child borrows it from the parent for the duration of
   the call (sound: the parent is suspended inside `execute_call`). The child
   has its own request, context, allowed set, counters, a child cancellation
   token and a **muted event sink** (`RoundSink`): nothing of its stream
   reaches the parent's bubble except the token counter, re-based on the
   parent's. Every behaviour of the main loop — confirmation popups, thinking
   signatures, control tools, effects, images — is therefore the subagent's
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
- The subagent's cost is visible: the counter grows with its tokens, the run
  records them, and the settings say `max_tool_rounds` applies to each run.
- `Message` is a recursive type (a run holds messages that hold records);
  `ChatActivated` and every save clone the run with the parent — the same
  class of cost as images in messages, and the first thing to measure if a
  chat with many long delegations ever feels slow.
- Stage 1 shows nothing of a running child but the counter; the list, the
  read-only screen, search and titles follow in the track's later PRs, the
  live view after a live run argues for it.

## Amendment (2026-09-04) — the parallel group over the shared part

Decision 2 above lent `TurnShared` to the child **mutably**, sound because
the parent was suspended for the duration. The parallel-subagent track
([docs/research/parallel-subagents.md](../research/parallel-subagents.md),
forks F1–F10 at their recommended options, user's decision 2026-09-03)
replaces that rule:

1. **`TurnShared` is borrowed immutably by every loop of the turn.** Its
   two mutable parts — the confirmation reply receiver and the "approved for
   this turn" set — sit behind one `tokio::sync::Mutex` held for the whole
   ask-and-wait, which is what makes the popup one question at a time; the
   turn's token totals are atomics every loop adds to.
2. **A round's `call_subagent` calls form its parallel group.** The
   ordinary calls resolve first, in the model's order; the group runs as
   futures inside the generation task (`buffer_unordered`, at most
   `tools.subagent_parallel` at once — default 1, the sequential behaviour
   of decision 1); each child is `run_child` over `&TurnShared` and a
   `ChildSpec` the parent built, owning nothing of the parent; the records
   and tool messages are written in the model's order afterwards. Each
   child's stream takes a permit of the engine's `sessions` semaphore for
   the stream alone.
3. **Every progress step names its run.** The orchestrator's mirror holds
   `children: Vec<InflightChild>` keyed by run id; the chip is a set.
4. Not in the group: `run_dialogue` (ADR 0011's one-request contract, run at
   its position), nested loops (a loop below the top still refuses the
   name), and ordinary tools — a per-tool concurrency mark is a track of its
   own.

Decisions 3–6 stand unchanged.

## Amendment (2026-09-05) — the run outside the turn

Decision 5 above — *the child lands with the turn* — was the whole of what
kept a sub-agent inside the round that started it. The background track
([docs/research/background-subagents.md](../research/background-subagents.md),
forks F1–F10 decided by the user 2026-09-05, F3 against every confirmation)
adds a second tool and lets a run outlive its turn:

1. **`start_subagent` is a twin, not a flag.** Measured (research §3.1): an
   optional boolean on `call_subagent` is silently omitted by one provider,
   a second tool is used 3/3 on every cloud and 5/5 on the local gate
   model. Offered only when `tools.subagent_background` is on (a gate, like
   `web_search`'s), so the default catalog is byte-identical.
2. **The run is owned by the orchestrator, not borrowed from the turn.** The
   loop builds the same `ChildSpec` a group child gets, over a fresh token,
   and hands it over as progress; the orchestrator spawns it as a task of
   its own over a `TurnShared` built from the turn's cloneable parts, with
   no confirmation round trip and the **app-wide** session budget — one
   `Arc` every turn and run streams under, so the run and the next turn
   take turns at `sessions = 1` and the admission guard covers both.
3. **The record lands with the turn as a placeholder and is filled in by
   id** — the auto-title's late-write route — deferred to the turn's
   landing when one is still running in the chat, so a result never
   precedes the round that was in flight when it arrived.
4. **The result is a stored task notification**: a `System` row with the
   run's id, user text on the wire merged in front of the next user
   message, and a turn the app starts itself when the chat is open and
   idle. A notification is a user-side row for regeneration and deletion.
5. **No confirmations in a background run** (the user's decision): its
   calls run as with `confirm_dangerous` off; the profile's tool set is the
   control. `Esc` ends the turn and not the run — and on the run's own open
   transcript, which streams under no turn, it merely goes back; `F6` there,
   `/subagents stop`, the deletion of the spawning exchange, a deleted chat
   and `Quit` end the run, the last landing every run *cancelled* from its
   mirror before the exit flush. A result landing in a chat the user is not
   looking at marks that chat unread in the list until it is opened.

Decisions 1–4 and 6 stand unchanged; decision 5 holds for a foreground run.
