# Background sub-agents — a run that outlives its round — research

> Status: **research, forks open** (2026-09-05). The last open item of the
> parallel sub-agent track's §8
> ([parallel-subagents.md](parallel-subagents.md), fork F1b): a
> `call_subagent` run that does not end with the round that started it —
> the parent gets the run's id at once, keeps answering, and is told the
> result in a **later** turn, the way Claude Code delivers a task
> notification for a background agent. Measured this session on the four
> clouds (§3.1): the shape of the ask decides everything — an optional
> boolean flag is *not* set by Claude (0/3) and not always by Gemini (2/3),
> while a second tool or a required `mode` is used 3/3 on all four, and every
> model consumes the notification without calling the tool again. The local
> arm (Qwen 3.6 27B, Gemma 4 31B) is pending: the LAN stack was down. Builds
> on [subagent-chats.md](subagent-chats.md) / [ADR 0010](../decisions/0010-subagent-nested-turn.md)
> (the nested turn; its decision 5, *the child lands with the turn*, is what
> this track amends), [subagent-live.md](../history/subagent-live.md) (the
> in-flight mirror), [parallel-subagents.md](parallel-subagents.md) (the
> owned `ChildSpec`, the sessions budget) and
> [admission-by-budget.md](admission-by-budget.md) (why a stream outside the
> budget is a hazard under a unified pool).
>
> The ask (roadmap, the parallel track's §8): *a child that outlives its
> round, with the parent notified in a later round — needs a "task
> notification" round shape and a place for a result that arrives after the
> turn ends.*

## 1. Requirements

R1. **The model decides, per call.** As in Claude Code, "background" is
    something the model asks for when the result is not needed for the
    reply it is writing; the harness runs the child outside the turn and
    tells the model later. No new argument to the user's message, no mode
    switch. (How the model asks is a measured fork, F1 — §3.1 shows that the
    obvious shape, an optional flag, does not work on every model.)
R2. **Nothing changes at the defaults.** The feature is opt-in
    (`tools.subagent_background`, default off): the tool catalog, every
    request and every file of a chat that never uses it stay byte-identical.
    New fields are `#[serde(default)]`; no schema step.
R3. **A background run is a sub-agent.** Everything spec §9.3.2 gives a run
    — the turn's tools, the parent's environment, its transcript on the
    call's record, the row under its parent while it runs, the read-only
    screen, search, titling, `chat://`, per-run budgets and timeout — holds
    for a background run.
R4. **The parent is not blocked.** The turn continues the moment the run is
    started; the user can send the next message while the run is out, and
    that turn is admitted like any other.
R5. **The result is delivered once, in order, as part of the
    conversation.** The notification is a stored message of the parent
    chat: visible in the feed, replayed on the wire, so a later request
    sees exactly what the model saw when it read the result.
R6. **No unbounded server load.** A background run's streams share the
    engine's `sessions` and the KV-pool admission with the turn's; the
    number of background runs is capped.
R7. **Never orphaned, never a phantom.** A run can be stopped; `Quit`, the
    deletion of the exchange that started it and a profile switch end it and
    land what it had; a crash leaves a record that reads as *unfinished*,
    never as *running*.
R8. **Honest surfaces.** The list marks the run as running under its parent,
    the status bar says one is out, and its transcript opens and streams
    while it runs — the parallel track's mirror, outliving the turn.

## 2. What exists today (inventory)

### 2.1 A turn's result lands whole, and the child lands with it

`start_generation` ([generation.rs](../../src/app/orchestrator/generation.rs) `:479`)
is the one spawn point: it snapshots everything the turn needs, mints the
turn's `CancellationToken` (`:607`), builds the turn's `SessionBudget`
(`:626`) and spawns the generation task. The task owns `TurnShared`
(`:1413`) — backend, registry, the confirmation mutex, the token counters,
the generation id, the limits, the budget, the two senders — and the loop
borrows it; a child loop borrows it from its parent for the duration of the
call, "sound because the parent is suspended inside `execute_call`"
(`:1406`). Everything the turn produced travels back as one `GenResult` on
`done_tx`; `handle_done` (`:790`) is the only landing path: it appends the
messages, applies the effects, drops the confirmation sender (`:801`) and
retires the mirror (`carry_inflight_rename`, `:940`). A `SubagentRun` is
attached to its `ToolCallRecord` inside the task, in `record_call`
(`:2390`), before the record is serialized: **`result` and `subagent` are
written once.** A background run has nothing to be a `tool_calls` entry of
after that point — unless the record lands with a placeholder and is filled
in later (§4.3).

### 2.2 What a child owns already

Since the parallel track, a child is a `ChildSpec` (`:2614`) that "owns
everything of its own, so several can be built by one parent and run at
once": the parsed args, the run id (minted at `:2599`), the allowed set, the
request over the parent's environment, its own `ToolContext`, a token
(`self.cancel.child_token()`, `:2555` — a child of the turn's, which is why
`Esc` ends children too), the user message, the limits, the depth.
`run_child(&TurnShared, loc, spec)` (`:2640`) borrows only the shared part.
So the *child* is already portable; what is not is the *shared part* it
borrows and the *channel* its progress and result travel on (`done_tx`, the
turn's).

### 2.3 The mirror dies with the turn

`InflightTurn` ([mod.rs](../../src/app/orchestrator/mod.rs) `:467`) holds the
filed rounds, the round in progress and `children: Vec<InflightChild>`
keyed by run id; created in `start_generation`, fed by `handle_progress`
(which drops a step from another generation, `:1150`), dropped at landing.
`view()` falls through to it as its third arm (`:1092`), so a running
transcript opens with no second path; `with_child_mut` (`:1122`) writes to
the mirror while the run is in flight and to the landed record afterwards,
marking the parent dirty; the list marks a live child through
`ChildSummary::of(&child.run)` + `running = outcome.is_none()` (`:1644`),
which `Chat::summary()` never sets for a landed run. "Never a source of
truth: `GenResult` is" (`:611`). It is singular (`Option<InflightTurn>`),
and so is `self.confirm` (`:603`), the sender of the confirmation round
trip, keyed by the turn's id.

### 2.4 Things that already outlive a turn

- **The silent loops** ([tool_loop.rs](../../src/app/orchestrator/tool_loop.rs)
  `:48`): `SilentLoop` is a fully owned struct — `Arc<dyn EngineBackend>`,
  `Arc<ToolRegistry>`, a `ToolContext`, a built request, a token, budgets,
  a `done_tx: UnboundedSender<(BackgroundKind, Result)>` — spawned by a free
  function; it borrows nothing of the orchestrator, which is why it survives
  the caller. `BgSlot` ([background.rs](../../src/app/orchestrator/background.rs))
  is a per-kind lifecycle: a running token, a failure streak,
  `handle_bg_done`, `cancel_all_bg` on `Quit`. They write `Storage`, never a
  chat, and run **outside** the sessions budget (`background_tool_ctx`,
  mod.rs `:1404`: "as every background task is — fork F9").
- **The auto-title's late write** ([title.rs](../../src/app/orchestrator/title.rs)
  `:83`, `:201`): a task spawned at landing, whose result comes back on
  its own channel after the turn is over and is written onto a landed
  chat or a landed run **by id** — `apply_title` → `with_child_mut` →
  `parent_of` → `chat_mut(parent).child_mut(id)` → `mark_dirty` — under the
  `renamed_manually` guard. The one existing "patch a landed record" path.
- **Compaction's landing** ([compaction.rs](../../src/features/compaction.rs)
  `:350`): the one background family that lands on a chat, on its own typed
  channel, and re-finds its boundary by id — "if it is gone, the summary is
  dropped". The model for a result that must survive an edited history.
- **The continuation's tool-result tail** (generation.rs `:371`): a turn
  that starts from *no new user message* — the trailing `Tool` row — through
  the ordinary `start_generation`. That is the shape of a turn the app starts
  by itself.

### 2.5 The wire and the feed

`message_to_api` ([request.rs](../../src/app/orchestrator/request.rs) `:16`):
a stored `System` row is **not sent** (the persona goes as
`ChatRequest::system`; a dialogue director's intervention is stored as a
`System` row for the feed alone); `record_to_api` (`:64`) reads `id`,
`name`, `arguments`, `thought_signature` and ignores `result`, `images`,
`subagent`. The feed renders `MessageRole::System` as `FeedRole::Note`
([message_feed.rs](../../src/widgets/message_feed.rs) `:152`); the export
skips `System` and `Tool` rows ([chat_export.rs](../../src/features/chat_export.rs)
`:64`). `Message` has no origin or kind field (`entities/message.rs:103`);
`MessageRole` is `System | User | Assistant | Tool`. App-composed text the
model reads exists on both sides already: the environment blocks in the
system prompt (`inject_compaction`/`inject_attachments`/`inject_workspace`,
request.rs `:175`) and synthetic user messages in every background family
(the title digest, the reflection digest, the `/compact` brief).

### 2.6 The budget is the turn's

`SessionBudget` ([session_budget.rs](../../src/shared/session_budget.rs)) is
built once per turn (generation.rs `:626`) from `engine.active_sessions()`
and `session_pool()`; a permit covers one open stream and is released when
the stream ends; under a pool a stream that would not fit waits. Nothing
holds a budget across turns, and nothing outside a turn takes a permit. For
a background run, whose streams overlap the *next* turn's, this is the one
piece of the parallel track's design that has to move (§4.7): a stream
outside the budget is exactly what admission-by-budget.md §3 reproduced as
the server's collective "Context size has been exceeded".

### 2.7 The state machine is one-turn-at-a-time, per active chat

`GenState` ([gen_state.rs](../../src/app/gen_state.rs)) — `Idle`,
`Generating`, `Cancelling`; `begin` refuses from anything but `Idle`;
`handle_send`, `handle_regenerate`, `handle_continue`, `handle_delete_last`
return early unless `Idle`. A chat switch cancels the turn, except between a
parent and its own running transcript (`switch_within_turn`,
[chats.rs](../../src/app/orchestrator/chats.rs) `:150`). A turn runs for the
active chat only. `Quit` cancels the turn's token and every `BgSlot`, waits
for nothing, and returns.

## 3. Measurements (2026-09-05)

### 3.1 How the model asks, and what it does with the answer

Instrument: a Python script over each provider's chat-completions endpoint
(OpenAI over its Responses API — gpt-5.6 refuses function tools with
reasoning on `/v1/chat/completions`, which is also why the app's client is
a Responses client; Anthropic and Gemini over their OpenAI-compatible
endpoints, which changes nothing about the model), kept out of the tree
like the earlier probes. One tool, the app's own `call_subagent` text and
parameters, plus the background in one of four shapes:

- **a** — an optional boolean `background` on `call_subagent`, described;
- **b** — **a**, plus one sentence in the tool's description saying when to
  use it (the F5 precedent of the parallel track);
- **c** — a **required** `mode` enum `{wait, background}` on `call_subagent`;
- **d** — a **second tool**, `start_subagent`, same parameters, its own
  description.

Two scenarios. **S1**: the user wants a critical review of a 90-word plan
from a sub-agent ("a big task, I do not need it right away — start it and
move on") and, right now, which is larger, 17 × 23 or 400. **S2**: "ask a
sub-agent to compute 17 × 23 and tell me the number". In S1 the harness
answers a background call with *"Sub-agent «name» started in the background
as run r1; its result will arrive as a task notification at the start of a
later message"*, a foreground call with the review at once. When the run
was started in the background, the turn is allowed to end and the next
**user** message is the notification alone — *"[Task notification — not a
message from the user] The background sub-agent run r1 («Critic») has
finished. Its final reply: …"* — then a fourth message asks, in one line,
for the run's main objection. Three trials of S1 and two of S2 per cell,
temperature 1.

| backend | model | shape | S1: asked for the background | S2: stayed in the foreground | r3: used the notification, no re-call | r4: main objection right |
|---|---|:-:|:-:|:-:|:-:|:-:|
| OpenAI Responses | gpt-5.6 | b | **3/3** | 2/2 | 3/3 | 3/3 |
| Anthropic | claude-sonnet-5 | b | **0/3** — the parameter omitted every time | 2/2 | — | — |
| Anthropic | claude-sonnet-5 | c | **3/3** | 2/2 | 3/3 | 3/3 |
| Anthropic | claude-sonnet-5 | d | **3/3** | 2/2 | 3/3 | 3/3 |
| Gemini (OpenAI-compat) | gemini-3.1-pro-preview | b | **2/3** — one trial: no call, an empty reply | 2/2 | 2/2 | 2/2 |
| Gemini | gemini-3.1-pro-preview | c | **3/3** | 2/2 | 3/3 | 3/3 |
| Gemini | gemini-3.1-pro-preview | d | **3/3** | 2/2 | 3/3 | 3/3 |
| xAI | grok-4.6 | b | **3/3** | 2/2 | 3/3 | 3/3 |
| xAI | grok-4.6 | c | **3/3** | 2/2 | 3/3 | 3/3 |
| llama.cpp (LAN) | Qwen 3.6 27B, Gemma 4 31B | — | *pending — the stack was down* | | | |

Four readings:

- **The optional flag is the wrong shape.** Claude never passed
  `background`, in three trials — and *narrated* it every time: "Task
  started (background)", "I've started the critique in the background" —
  then took the foreground result the harness returned and relayed it as
  "early feedback". Gemini passed it in two of three and produced an empty
  reply in the third. A required field or a second tool got **3/3 on all
  four clouds**, which settles F1 between (c) and (d), against (a)/(b).
- **No model asked for the background when the answer was the point.** S2
  stayed in the foreground in every cell, under every shape — the
  description's "when the result is not needed for your current reply" is
  understood.
- **Nobody invented the result.** In every background trial the second
  round answered the arithmetic and said the review was running; none
  attributed findings to the run. (The word heuristic flagged Claude under
  **c** three times; a reading cleared it — it listed what the reviewer
  *would* look at, idempotency and rollback among them, as a preview of
  topics, not as results.)
- **The notification is consumed as a message.** With the result arriving
  as a user-side block and no user words, every model reported the review
  and, asked a line later for the main objection, answered from it; not one
  called the tool again to "fetch" the result. The plain text form — a
  bracketed preamble, the run's id, the reply, the transcript address — is
  enough; no XML, no special role.

### 3.2 The reference shape, observed

Claude Code's own contract, as this session ran under it (the agent tool
that produced §2's inventories): the tool returns at once with an id
("Subagents run in the background by default; you'll be notified when one
completes"); the completion arrives as a block inside a **user** turn —
a `[SYSTEM NOTIFICATION — NOT USER INPUT]` preamble, then
`<task-notification>` with a task id, a summary and the result — and the
model is invoked on it with no user words; the same id can notify more than
once (an agent resumed by the user); and the description carries the two
sentences the models above followed unprompted: *never fabricate or predict
a pending agent's results; if the user asks before it arrives, say it is
still running.* The preamble's insistence that a notification is not the
user's consent to anything is the one line worth copying verbatim into
ours (§4.10).

### 3.3 The server side: nothing new to measure

A background run's stream beside the next turn's is the parallel track's
§3.1–§3.3 (four slots by default; past them the server queues, nothing
fails) and the admission track's §3 (two streams that do not fit a unified
pool end *every* conversation at once — the reason §4.7 puts the run inside
the budget rather than beside it). The parked set applies as measured
(parallel §3.6–§3.7): a background run alongside a long chat is one more
context to park, and on the 31B the default `--cache-ram` parks **one**,
so on that model a background run and its parent's turn re-prefill each
other every round unless `--cache-ram` is raised — install.md §3 already
says so for `subagent_parallel`, and this is the same advice.

## 4. Design

### 4.1 The ask: a second loop-executed tool, present only when enabled

`start_subagent` (F1d): the same parameters as `call_subagent` — `name`,
`system_message`, `message` — and a description of its own: *start a
subagent in the background; this call returns at once with the run's id,
and the subagent's final reply arrives later as a task notification at the
start of a subsequent user message; use it for a delegation whose result
you do not need for your current reply; never guess at its result — if
asked before it arrives, say it is still running.* A `Tool` impl for the
schema, the catalog and the profile toggle, like `call_subagent`'s
([subagent.rs](../../src/features/tools/subagent.rs)); the loop recognises
the name beside `CALL_SUBAGENT_ID` in `is_group_call`'s neighbourhood and
withholds it from a child with the rest of `withheld_from_subagent` (no
nesting, as today). It exists in the catalog only when
`tools.subagent_background` is on, so at the default the catalog — and
every request — is byte-identical to today's (R2). `call_subagent`'s own
text is untouched; the number sentence of `subagent_parallel` stays as it
is.

### 4.2 The run: owned, spawned by the orchestrator, outside the turn's token

Inside the turn, `start_subagent` resolves like a group call up to
`child_spec` — the parsed args, the run id, the allowed set minus the
withheld, the request over the parent's environment, the context, the user
message — and then, instead of `run_child` over `&TurnShared`, hands the
`ChildSpec` to the orchestrator as progress: `TurnProgress::BackgroundStart
{ spec, shared: RunShared }`, where `RunShared` is the owned twin of what a
child reads from `TurnShared` — `Arc<dyn EngineBackend>`,
`Arc<ToolRegistry>`, the image config, the limits, the engine mode and
model name, the locale, `evt_tx` — the `SilentLoop` shape (§2.4) with the
child's loop inside it. The orchestrator spawns the task
(`tokio::spawn`, one per run), gives it a **fresh** `CancellationToken`
(not a child of the turn's: `Esc` ends the turn, not the run — R4), a
progress sender of its own (`bg_tx`, the compaction precedent, §2.4) and a
seat in `background_runs: Vec<BackgroundRun>` — run id, parent chat id, the
record's call id, the token, an `InflightChild` mirror, a `queued`
notification slot (§4.4). The tool result the parent's loop records at once
is the localized *started* line with the run's `chat://` address. The
parent's turn goes on; the record lands with the turn carrying a
**placeholder** run — `SubagentRun { id, background: true, messages:
[user], outcome: None, .. }` — so the transcript row exists under the chat
from the moment the parent lands (R3, R8).

### 4.3 Landing: the record is filled in by id

When the task ends — completed, failed, timed out, cancelled — it sends
`BackgroundDone { run: SubagentRun }` on `bg_tx`; the orchestrator, sole
owner of `Chat`, re-finds the record by run id the way the auto-title does
(`with_child_mut`'s route: `parent_of` → `chat_mut(parent).child_mut(id)`,
§2.4), replaces the placeholder with the landed run (messages, outcome,
tokens, `finished_at`; a title given while it ran is kept, as
`carry_inflight_rename` keeps one today), marks the parent dirty and frees
the seat. A record that is gone — the exchange deleted or regenerated while
the run was out — is the compaction rule: the run is not orphaned into
thin air but written onto the copy in `Chat.deleted` (F8), so the
`chat://` address a later message may cite still resolves the way a deleted
exchange's does today.

### 4.4 Delivery: a stored notification, and a turn the app starts

The run's landing appends a **notification message** to the parent chat
(F5): a `System` row — the feed's note look for free, §2.5 — with an
additive `origin: Notification { run }` marker, whose text is the measured
form of §3.1: the bracketed preamble, *sub-agent «name» (chat://…)
finished*, the final reply (or the outcome line for a run that did not
complete — the same localized lines `call_subagent` returns for a
cancelled, failed, timed-out or round-limited run). On the wire the marker
turns the rule of §2.5 around for this one kind of row: the request builder
sends it as **user** text, merged in front of the user message that
follows it — one wire message, the notification block first, the user's
words after — which is legal on every provider without relying on
consecutive same-role turns; alone, it is the whole user message.

Then the round shape. If the parent chat is the **active** chat and
`GenState` is `Idle`, and `tools.subagent_background_wake` is on (default
on within the feature), the orchestrator starts a turn at once with no new
user message — the continuation's precedent (§2.4): `start_generation` over
the chat whose last row is the notification — and the model's reply lands
as an ordinary assistant message: "the review is in: …". If a turn is
running in that chat, the notification is held in the run's `queued` slot
until `handle_done` appends it after the turn's messages (order: a result
never appears *before* the round that was in flight when it arrived), and
the wake follows then. If the chat is not active, the notification is
appended now — no turn can be running in a non-active chat — and there is
no wake: the note is there when the user comes back, and their next message
carries it. Claude Code's harness wakes the model on completion; a chat app
has the second case, a conversation the user is not looking at, and the
answer there is "say it when they return", not a reply into the void.

### 4.5 Stopping, quitting, deleting

- `Esc` cancels the turn; it does not touch a background run (R4).
- A run is stopped by the user through `/subagents stop [n]` in the parent
  chat (the existing `/subagents` command gains a verb) and by one key on
  the run's open transcript, advertised in that screen's footer under spec
  §11.2's rule (a key that works, or no hint). The stop cancels the run's
  token; the task lands `Cancelled` through §4.3 like any end.
- `Quit` (F7): every background run's token is cancelled and its partial
  transcript — which the orchestrator holds in the mirror — is written
  onto the record as `Cancelled`, then saved, before the loop returns; no
  waiting for the tasks.
- A profile switch, the hiding of the parent chat, `Ctrl+E`/`Ctrl+R` on the
  spawning exchange: the run is cancelled and lands into the record where
  the exchange now lives (`Chat.deleted` for the last two, F8).
- A crash leaves `background: true, outcome: None` on the record: at load
  that reads as **unfinished** (rendered like a cancelled run with its own
  note), never as running — running is a property of the mirror, which a
  fresh process does not have (R7).

### 4.6 Dangerous calls: withheld from a background run

A dangerous call inside a run asks the user through the turn's confirmation
round trip — one popup at a time, keyed by the turn (§2.3). A background run
has no turn to ask through, and a popup opening in a chat the user is not
looking at, or on top of a running turn's own popup, is a second question
the UI has one slot for. V1 (F3a): when `tools.confirm_dangerous` is on, the
dangerous tools are withheld from a background run the way `call_subagent`
is — the tool's description says so, and a run that needs one says it
could not and why. When confirmations are off, the run has them all, as a
foreground run does. Routing a popup queue by run id is the stage-2
candidate (§8).

### 4.7 The budget becomes the app's

`SessionBudget` moves from `start_generation` to the orchestrator: one
`Arc<SessionBudget>` per active engine section, built when the section is
chosen or its `sessions`/pool change (the `/props` refresh included) and
cloned into every `TurnShared` and every `RunShared`. A background run's
stream takes a permit and a reservation like a turn's — so at the default
`sessions = 1` the run and the next turn **take turns round by round**, the
parallel track's F3a semantics, and under a unified pool a stream that
would not fit beside the other waits instead of provoking the collective
failure (R6). The silent background tasks stay outside (the parent's F9;
the admission track's §8 names bringing them in as its own item). The
parked-set advice of §3.3 goes into the tool's settings hint.

### 4.8 Limits

`tools.subagent_background_max` (default 2): a `start_subagent` call past
it is refused with a localized result ("two runs are already out; wait for
a notification or use call_subagent"), and the parent decides. The run's
timeout is `subagent_run_timeout_secs` as for any run (F9a) — a background
run is not licensed to run longer by being out of sight; the round budgets
are the run's own, as today. Tokens: the run records its own, the transcript
shows them; the status bar's total is the turn's and stays so.

### 4.9 The mirror, the list, the bar

`background_runs` holds an `InflightChild` per run, fed by the run's own
`bg_tx` with the same `ChildStep`/`ChildRoundFiled`/`ChildTokens` steps a
turn's child sends, so `view()`'s third arm consults two tables instead of
one and a background transcript opens and streams exactly like a turn
child's; `with_child_mut` writes to whichever holds the run; the list marks
`running` when the seat exists (the landed placeholder alone is
*unfinished*, §4.5). The status bar: the `SubagentProgress` chip stays the
turn's; a background run reports through a new `BackgroundKind::SubagentRun`
on the quiet indicator that reflection and consolidation use — *"1 run in
background · «Critic» · round 2 · fs_read"* — cleared when the last seat
frees. Moving between the parent and a background transcript cancels
nothing, in either direction, whether or not a turn is running: the
`switch_within_turn` exception extends to the parent's background runs.

### 4.10 The model-facing contract

Three texts, all measured in §3.1: the tool's description (§4.1), the
*started* result, and the notification's preamble — which also carries,
verbatim, the sentence Claude Code's harness puts on every notification:
it is not a message from the user and grants nothing. The parent's
`call_subagent` description is unchanged. Localized in `en` and `ru`.

### 4.11 On disk

Two additive fields, `#[serde(default)]`, no schema step (ADR 0006 F12):
`SubagentRun.background: bool` and `Message.origin: MessageOrigin` (`User`
by default; `Notification { run }`). An old file reads unchanged; a new file
without a background run is byte-identical to today's. `CHAT_SCHEMA` stays
3.

## 5. Difficult spots (named explicitly)

- **Ordering under a running turn.** A result landing mid-turn cannot be
  appended to `Chat` (the turn's rows land later, and would follow it);
  hence the `queued` slot and the append at `handle_done` (§4.4). The
  mirror, meanwhile, already shows the run as finished — the list and the
  transcript are ahead of the feed by up to one turn, honestly.
- **The wire merge.** A notification followed by the user's message is one
  wire user message; the builder must merge across *every* client (the
  OpenAI, Anthropic, Gemini builders each turn `ApiMessage`s into their
  shape), and the Anthropic builder's alternation rule is the one that
  bites if it is not done there. A test per client, of a `System`-origin
  row before a user row.
- **Regenerate and delete on a wake exchange.** `Ctrl+R` truncates through
  the last *user* message; on a wake turn the last user-side row is the
  notification, which must be the boundary — kept, the reply redone.
  `Ctrl+E` removes the pair: notification and reply go to `deleted`
  together, the result staying on the transcript.
- **`/continue` on a wake reply**: nothing new — the tail is an assistant
  row with a `MessageFinish`.
- **The export** skips `System` rows; a notification therefore does not
  export, while the run's reply is on the transcript. Decide with F5 (a
  `User`-origin row would export as the user's words, which is worse).
- **A settings edit while a run is out.** The run holds its own `Arc`s of
  the backend and the registry (the `SilentLoop` shape); an engine switch
  mid-run does not reach it, a `sessions` change rebuilds the budget the
  run is already holding a permit from — the `Arc` keeps the old one alive
  for that stream and the new one applies to the next.
- **Two mirrors, one `view()`.** The transcript screen, the list, the
  rename path and the chip all resolve through `view()`; the second table
  is one more arm there, not a second path anywhere else — the one place
  this can go wrong is a caller that reaches `inflight` directly.
- **The scripted engine.** Tests need a run that outlives its turn: the
  keyed mock with a delayed reply for the child's persona, so the parent's
  turn lands first and the `BackgroundDone` arrives while the test is
  already asserting the placeholder.
- **The parked set** (§3.3): a background run is one more context to park;
  on the 31B that is the whole default cache. The hint says so.

## 6. Forks

Recommendations are marked; nothing is decided until the user says so.

- **F1. How the model asks.** (a) **A second tool, `start_subagent`**
  *(recommended — 3/3 on all four clouds; a catalog entry the profile can
  toggle; `call_subagent`'s text untouched at the default)*. (b) A required
  `mode` enum on `call_subagent` (3/3 as well; changes the existing tool's
  schema for every model, and a model that omits a required field needs a
  default that makes the field optional again). (c) An optional boolean —
  **rejected by measurement** (0/3 on Claude, 2/3 on Gemini).
- **F2. Delivery.** (a) **At the next user message, plus a wake turn when
  the parent chat is active and idle** *(recommended — Claude Code's
  shape where it can apply; "say it when they return" where it cannot)*,
  under `tools.subagent_background_wake` (default on). (b) The next user
  message only. (c) A wake into a non-active chat — not possible with one
  turn per active chat, and not wanted.
- **F3. Dangerous calls in a background run.** (a) **Withheld while
  confirmations are on** *(recommended for v1; honest, no popup out of
  context)*. (b) A popup queue routed by run id (stage 2).
- **F4. The budget.** (a) **App-wide `SessionBudget`, background runs
  inside it** *(recommended — R6; the admission guard covers both)*. (b)
  Outside, like the silent tasks — the collective failure the admission
  track reproduced. (c) Background only when `sessions ≥ 2` — hides the
  hazard behind a knob.
- **F5. The stored notification.** (a) **A `System` row with an
  `origin: Notification` marker, sent as user text merged into the next
  user message** *(recommended — the note look and the export's skip come
  free; one wire rule keyed by the marker)*. (b) A `User` row with the
  marker and a feed rule for the look (exports as the user's words).
- **F6. Stopping a run.** (a) **`/subagents stop [n]` and one key on the
  open transcript** *(recommended)*. (b) Only the timeout and `Quit`.
- **F7. `Quit`.** (a) **Cancel every run and land it `Cancelled` from the
  mirror before exit** *(recommended — R7)*. (b) Let the process end and
  read the record as unfinished at the next load (what a crash gets
  anyway).
- **F8. The spawning exchange deleted or regenerated.** (a) **Cancel the
  run and land its partial onto the copy in `Chat.deleted`** *(recommended
  — the address still resolves the way a deleted exchange's does)*. (b) Let
  it finish and drop the result.
- **F9. Limits.** (a) **`subagent_background_max` default 2; the run
  timeout shared with foreground runs** *(recommended)*. (b) A separate,
  longer background timeout. (c) No cap.
- **F10. Staging.** (a) **Two PRs** *(recommended, §7)*: stage 1 the tool,
  the owned run, the app-wide budget, landing, delivery with the wake, the
  mirror and the list, stop and `Quit`; stage 2 the popup queue (F3b), an
  unread mark on the list for a chat whose run finished while it was not
  active, and whatever the live run argues for. (b) One PR.

## 7. Stages and the test plan

**Before code — the local arm of §3.1**, on the LAN stack: Qwen 3.6 27B and
Gemma 4 31B, shape (d), five trials of S1 and S2. Go: ≥ 4/5 background on
S1, 0/5 on S2, ≥ 4/5 notification use without a re-call. A model below that
bar is not a no-go for the track (the clouds are there) but a reason to
keep the feature off by default for that stack and say so in the hint.

**Stage 1 — the run outside the turn** (`feat/background-subagents`).
§4.1–§4.5, §4.7–§4.11. Unit tests over the keyed and counting mocks: the
catalog is byte-identical with `subagent_background` off, and carries
`start_subagent` on; a `start_subagent` call records the *started* line and
lands a placeholder with the turn; the run's `BackgroundDone` fills the
record by id and marks the chat dirty; the notification row is appended
after the turn when a turn was running, at once when not; the wake turn
starts when the chat is active and idle and not otherwise (not active; a
turn running; the setting off); the request builder merges a
notification row into the following user message on all three wire shapes,
and sends it alone for a wake turn; `Esc` leaves the run alive and lands the
turn; `/subagents stop` and `Ctrl+E` on the exchange cancel it and land
`Cancelled` where the record lives; `Quit` lands every run; with
`sessions = 1` the wake turn's stream waits for the run's stream (the
counting mock sees one at a time); `subagent_background_max` refuses the
third; the dangerous set is withheld while confirmations are on; a stored
`background: true, outcome: None` run reads as unfinished. Live:
`background_subagent_e2e_live` on both gate models — the parent starts a
run over a planted file and answers a question at once (both in one
reply), the run reads the file, the wake turn's reply carries the planted
token, the transcript has the run — and one cloud arm (Anthropic, the
strictest about turn shape) for the merged user message.

**Stage 2 — what the live run argues for.** The popup queue (F3b) if the
withheld set proves too blunt; the unread mark; the settings-screen rows'
polish; the docs' final pass.

## 8. Not in this track (recorded so they are not re-derived)

- **A popup for a background run** (F3b): a confirmation queue keyed by run
  id, and the "allow for this turn" semantics across a run that has no
  turn.
- **Background dialogues** (`run_dialogue` in the background): ADR 0011's
  one-request contract; the same shape would serve, later.
- **Nested background** (a child starting a background run): no nesting, as
  for `call_subagent`.
- **The silent tasks under the budget**: admission-by-budget.md §8's item,
  unchanged by this track's app-wide budget (they stay outside).
- **A tasks screen** listing every background run across chats: the list's
  rows under each parent are the v1 surface.
- **Notifications across profiles**: a run belongs to a chat, a chat to a
  profile; a switch cancels (§4.5).

## 9. Documentation touch list (AGENTS.md §4)

- spec §9.3.2 (the background run: the tool, the placeholder, the
  notification, the wake, stop), §6.3 (a turn the app starts), §11.2 (the
  row, *running*/*unfinished*, the stop key), §11.3 (the notification note),
  §11.6 (the three settings and the hint), §11.7 (`/subagents stop`).
- architecture §5 (`RunShared`, the app-wide budget, the second mirror
  table), §8 (`start_subagent`), §10 (the note, the indicator).
- ADR: an addendum to ADR 0010 amending decision 5 (*the child lands with
  the turn*) — or a new ADR if the user prefers; decide with F10.
- install.md §3 (the parked-set advice for a background run).
- journal: tools.md (both stages), ui-screens.md (the list and the note);
  CHANGELOG `[Unreleased]` Added; README (settings); roadmap (this item
  moves; §8 lists the leftovers); CLAUDE.md's status line when the track
  ships; locales `en`/`ru`.
