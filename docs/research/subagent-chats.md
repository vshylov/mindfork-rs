# Sub-agent chats: a sub-agent with the agent's own tools, kept inside the parent conversation

Status: **accepted 2026-08-23** — every fork in §5 decided (the user's
clarification of the same day: the child is part of the parent's file, never a
file of its own). In progress, PR by PR (§7): PR 1 merged (#356), PR 2 merged (#357 — the
engine, the record-borne run, the settings step; live go on Gemma 4 31B, 5/5),
PR 3 — `feat/subagent-migration` (`CHAT_SCHEMA` 2, the synthesized runs). [ADR 0010](../decisions/0010-subagent-nested-turn.md)
records the decision. Date: 2026-08-23.

## 1. What and why

Today `call_subagent` (spec §9.3.2) is the most primitive thing it could be: one
request to the same model with a system message and a single user message, **no
history, no tools**, a token cap and a 60-second timeout, and the reply comes
back as the tool's string result. The feed shows it as one tool card.

The request is to grow it into a real delegated agent, and to make each
delegation a conversation of its own *that belongs to the one that made it*:

1. **The sub-agent gets every tool and capability the main agent has, except
   creating sub-agents** (no nesting).
2. **A sub-agent call shows in the chat list nested under the chat that made
   it** (an indent or a `└` marker). It can be renamed — by hand and by the
   model — and `interface.auto_title` applies to it. It cannot be deleted or
   cloned (`Ctrl+D`) from the list.
3. **The child is part of the parent chat** — stored in the parent's JSON,
   never in a file of its own; it *looks* like a separate chat but is
   inseparable from its parent. It disappears only when the parent's exchange
   that spawned it is taken back (`Ctrl+E`) or regenerated (`Ctrl+R`). The user
   suggested attaching it to the tool-call message — §3.1 does exactly that.
4. **Search** finds the child when it contains the text and then shows the
   parent together with it; text found only in the parent shows the parent
   alone.
5. **Opening a child** shows a read-only conversation that "looks like a chat
   with a system message": sending is disabled; only the commands that make
   sense in a transcript (`/copy`, `/rename`, `/export`, …) work.
6. **Old `call_subagent` records are migrated** into the new shape. Breaking
   changes to **settings** are acceptable (no public release yet); **chat files
   must not break** — there are a lot of them.
7. The design has to be **flexible enough for the next feature**: two
   sub-agents with different system messages talking to each other, a dialogue
   directed by the main agent and viewable as an ordinary chat.

The original brief (docs/history/request.md) asked for exactly the primitive
version — "calling the subagent wouldn't differ from an ordinary tool call" —
and M6 built it (docs/history/plan.md §M6). Everything this document adds is
new ground; the self-model track had once sketched a "beefed-up
`call_subagent`" (`simulate_alternative_self`, docs/history/self-model.md) and
never built it.

## 2. What exists today (inventory)

Read before designing, per docs/lessons.md §3 — and as usual it reshaped the
task. The decisive fact is **where the machinery lives**.

### 2.1 The tool is in the wrong layer for what it has to become

`src/features/tools/subagent.rs` builds a `ChatRequest` against `ctx.engine`
with `tools: Vec::new()` and returns `ToolOutcome::text(...)`. A tool sees only
`ToolContext` (`src/features/tools/mod.rs:55`): engine, storage, embedder, the
turn snapshot, a cancellation token. It does **not** see the registry, the UI
event sender, the dangerous-tool confirmation channel, the image limits, the
round budgets — everything a tool-using loop needs. Those live in the
orchestrator's agentic loop, `TurnLoop` (`src/app/orchestrator/generation.rs:912`),
which is `app`-layer code that `features` cannot import (FSD, dependencies only
downward).

So "the sub-agent gets all the tools" cannot be implemented *inside* the tool.
The codebase already has the precedent for a tool whose execution belongs to the
loop: the conversation-control tools (`src/features/tools/control.rs`, spec
§9.3.3) keep a `Tool` impl for schema/registration/gating only, and
`TurnLoop::resolve_call_result` (`generation.rs:1290`) recognises them by name.
`call_subagent` becomes the second member of that category — a **loop-executed
tool** — with the difference that it still goes through every ordinary gate
(disabled-check, confirmation, the `select!` with the cancel token, the
`ToolCall` event, the `ToolCallRecord`).

### 2.2 There are two loops, and the right one to reuse is the big one

- `TurnLoop` (`generation.rs:912–1360`): UI streaming (`Chunk`/`Thoughts`/
  `ToolCall`/`TokenUsage`), control tools, Anthropic/OpenAI thinking
  signatures, Gemini per-call signatures, the confirmation round trip
  (`confirm_call`, `:710`), effects, images from tools, the two round budgets
  (`max_tool_rounds` and `workspace.max_rounds`, `:1029`), the "final round
  without tools" when a budget runs out (`:1063`). It never touches `Chat`;
  results go back in `GenResult` through `done_tx`.
- The silent loop `tool_loop.rs::run_rounds` for reflection/consolidation: no
  UI, no effects, no confirmation, tolerant of everything. Its module doc says
  explicitly the main loop was left alone because "its complexity doesn't pay
  for a shared sink right now".

A sub-agent with the main agent's tools needs every one of the main loop's
behaviours (a dangerous `fs_write` inside a sub-agent must ask the user exactly
as it would outside; a Gemini 3 call needs its signature; `send_followup_message`
should work). Hence **the child run is a `TurnLoop`**, not a third loop.

### 2.3 The turn's channels, and where a turn's messages are until it lands

Task → orchestrator: only `done_tx: UnboundedSender<GenResult>` at the end of
the turn. Orchestrator → task: only `confirm_rx` (tool confirmations, fork F8 of
docs/history/tool-confirmation.md). Task → UI: `evt_tx` (`AppEvent`). **While a
turn runs, its messages exist only inside the task** (`TurnLoop.messages`) and
reach `Chat` in one piece at `handle_done` (`:498`); a cancelled turn still
lands what it had (`GenState::Cancelling`, gen_state.rs). A crash mid-turn
loses the turn. Whatever the child is, it is part of that turn.

### 2.4 Chats, the list, deletion, cloning

- `Orchestrator.chats: Vec<Chat>` holds **visible** chats only (hidden ones are
  filtered at `bootstrap`, `orchestrator/mod.rs:497–503`); about forty sites do
  `self.chats.iter().find(|c| c.id == id)` and **fail closed** (early return)
  for an unknown id. `active_id: Option<Uuid>`.
- One file per chat, `chats/<uuid>.json` (the id *is* the file name). `Message`
  holds `tool_calls: Vec<ToolCallRecord>` (`entities/message.rs:23`) — id,
  name, arguments, result, a Gemini thought signature, an image count; every
  field since the first is additive. `Ctrl+E`/`Ctrl+R` move whole messages
  (records included) into `Chat.deleted` (`chat.rs:145`, kept "only for manual
  recovery"); `handle_clone` (`chats.rs:153`) deep-copies the whole `Chat` with
  a new id; `handle_regenerate` already re-emits the list (`generation.rs:171`).
- `emit_chat_list` (`mod.rs:1004`) sends `Vec<ChatSummary>` — `id, profile_id,
  title, created_at, modified_at, message_count` (`entities/chat.rs:295`), flat;
  the widget re-sorts and filters in one function, `ChatListState::visible()`
  (`widgets/chat_list.rs:186`), and draws a row in `item_line` (`:689`) with a
  hard-coded `PREFIX_W = 4` prefix (rail + dot). **No grouping, tree or
  indentation exists anywhere.** `Del` → `DeleteChat` and `Ctrl+D` → `CloneChat`
  go straight out with **no confirmation** (`dispatch.rs:685–693`).
- `Chat.character_names` is a creation-time copy the feed **does not use** (names
  come from the profile, `emit_character_names`, `mod.rs:966`);
  `CharacterNames.system` is "not displayed anywhere yet".

### 2.5 Search

`cache.db` indexes `message.text` of all four roles per chat file, keyed by
`chat_id` + `message_id` (`storage/cache/mod.rs:522`), no chat-level metadata;
`CACHE_SCHEMA` is bumped freely — a mismatch wipes and rebuilds the file
(architecture §7). `SearchChats` answers a flat id list; **the intersection
with the list is done in the widget** (`visible()`, content branch).
`SearchMessages` groups hits in `group_hits` (`orchestrator/search.rs:120`)
into `SearchGroup { chat_id, title, hits }` in the list's sort order; the
screen's `Row::{Hit, Decoration}` already tolerates non-hit rows.

### 2.6 Titles

`maybe_auto_title`, `start_title_task` and `handle_title_result`
(`orchestrator/title.rs`) are **chat-id-addressed** and never read `active_id`;
`is_first_reply` (`generation.rs:482`) is asked before the turn's messages land
and fires on `res.chat_id`. What is new for a child is *resolving* its id (§3.10).

### 2.7 Nothing read-only exists

`handle_enter` (`screens/chat/input.rs:329`) ends in `if !self.generating {
Send }` — a *silent* gate. The registry dispatch `run_ui_command`
(`screens/chat/commands.rs:48`) refuses with a note through `note(key)` /
`busy_note(alias)` — the module's stated rule is that every blocked command says
so and names the route that works (docs/lessons.md §4). The feed has three
roles, `User`/`Assistant`/`Note` — **the system message is never drawn**
(`FeedMessage::from_message` drops `System`, `message_feed.rs:121`).

### 2.8 `chat://` references already do most of the linking work

`chat://` + 8 hex chars (`features/chat_links.rs`), resolved **only** against
the active profile's chats from the `ChatList` snapshot
(`ChatScreen::refresh_known_chats`); **unresolvable text stays plain** — "a
reference is only a reference if it resolves". Tool-card text is styled and
clickable only when the card is expanded; the header is the one line a collapsed
card shows. `Ctrl+L`/click → `ChatIntent::OpenChatLink` → `Back::Link` → `Esc`
returns to the origin (architecture §10).

### 2.9 The migration scaffold is dormant and ready

ADR 0006: a step is a pure per-file `fn(Value) -> Result<Value>` registered in
`chat_artifact().steps` (`shared/storage/schema.rs:136`), run once for every
file whose detected version is below `current`, after **one** pre-migration
backup and before a control-parse into the typed struct; `detect_chat` reads the
`v` field (absent → 1). `real_registry_is_all_current_v1` (`schema.rs:237`)
pins the dormant state and is the first test to change. `AppConfig` does not
`deny_unknown_fields`, so a renamed settings key would be silently dropped
without a step. `Uuid::new_v5` over a namespace is the project's way of making
synthesized ids deterministic (`features/import.rs:279`).

## 3. Design

### 3.1 The child lives on the tool-call record

A sub-agent run is stored **on the `ToolCallRecord` of the `call_subagent` call
that made it** — one call, one transcript, inside the assistant message that
holds the call, inside the parent's file:

```rust
// entities/message.rs — additive; old records read `None`, a record with no run writes no key
pub struct ToolCallRecord {
    …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent: Option<Box<SubagentRun>>,
}

// entities/subagent.rs
pub struct SubagentRun {
    /// Identity for links, the list, search and activation — never a file name.
    pub id: Uuid,
    pub kind: RunKind,                       // Subagent today; Dialogue later (§3.14)
    pub title: String,
    #[serde(default, skip_serializing_if = "not")] pub renamed_manually: bool,
    /// The persona's display name (the optional `name` argument), else the localized label.
    pub name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// The persona the parent composed; `set_system_message` inside the run changes it.
    pub system_message: String,
    pub sampling_override: Option<SamplingConfig>,
    /// `User(message)`, then the run's rounds exactly as any chat stores them:
    /// assistant messages with their own tool records, tool messages, thoughts.
    pub messages: Vec<Message>,
    /// `Completed | Cancelled | TimedOut | Failed | RoundLimit`; `None` = interrupted.
    pub outcome: Option<RunOutcome>,
    /// What the run cost, for the card and the list.
    pub tokens: u64,
}
```

The record rather than the assistant message, because a message can hold
several calls and each needs its own transcript; the record rather than the
`Tool`-role result message, because the feed draws cards from records and never
sees `Tool` messages. `ToolCallRecord.result` stays what the parent's model was
given (§3.11) — the source of truth for request replay is untouched.

What this placement buys, all by construction:

- **Inseparable.** The child has no file, no `profile_id`, no `is_hidden`: it is
  a value inside the parent. Hiding the parent hides it; backing up the parent
  backs it up; an import that drops tool calls drops it.
- **Deleted exactly when the user asked.** `Ctrl+E` and `Ctrl+R` move the
  exchange's messages into `Chat.deleted` with their records — the run travels
  along, vanishes from the list (which is derived from live messages) and stays
  recoverable by the same manual JSON edit the archive already promises. A
  rewrite round never spawns a child (side-effect calls are skipped there
  today), and a child born in a round the loop then discards goes into
  `deleted` with the round. No cascade code exists to get wrong.
- **Request replay unchanged.** `record_to_api` (`orchestrator/request.rs:64`)
  reads id/name/arguments/signature and ignores the rest, so a request with a
  stored run is **byte-identical** to today's.
- **Reflection, consolidation, compaction** read `chat.messages` and see the
  call's result text as today; the transcript inside is not part of the parent's
  conversation and is never mistaken for it.
- **The `Message` type becomes recursive** (`Box`, the reason it is boxed); the
  child's own records never carry a run, because there is no nesting, but the
  type does not have to know that.

The parent's projection for the list: `ChatSummary.children: Vec<ChildSummary
{ id, title, created_at, finished_at, message_count, outcome, running }>`, built
by `Chat::summary()` walking `messages[].tool_calls[].subagent` in order —
children are naturally in creation order. `Chat::child(id)` / `child_mut(id)`
find a run by id; the orchestrator resolves a child id to its parent by walking
`self.chats` (a handful of chats with a handful of children each; no index
needed).

Per-chat names, resolved **at activation** so they never go stale: in a child,
the `User`-role header is the parent persona's name (the profile's assistant
name, else the localized assistant label) — the one who wrote the instruction —
and the `Assistant`-role header is `run.name`, else the localized "Sub-agent";
the system bubble is headed by `CharacterNames.system` (its first use).

### 3.2 The run is a nested turn

`call_subagent` stays a registered `Tool` (catalog, profile toggle, schema,
description) whose `invoke` is never the executor. In
`TurnLoop::resolve_call_result`, after the disabled-gate and the confirmation
gate, a call by that name runs a **child `TurnLoop`** instead of
`registry.invoke` — inside the same `select!` with the turn's cancellation
token, so `Esc` ends the child exactly as it ends any long tool.

What the child loop shares with the parent (borrowed `&mut` for the duration of
the call, which is sound because the parent is suspended inside
`execute_call` while the child runs): the backend, the registry, the
confirmation receiver and `allowed_for_turn` ("approve for the rest of this
turn" covers the sub-agent's calls — same turn, same user request), the image
limits, the event sender, the parent's `generation_id` (so `ToolConfirmRequest`
reaches the popup the user knows and `ConfirmTool` is routed back by the
existing id guard), the engine mode and model name for the metadata snapshot.
This is a mechanical split of `TurnLoop` into a shared part and a per-loop part;
`GenSpawn`'s fields already fall into those two groups.

What the child has of its own: its request (system + the one user message), its
`ToolContext` (§3.3), its allowed set, its round counters, its message and
effect accumulators (the per-loop `messages` becomes `run.messages`), a
`depth = 1` flag, a **child cancellation token** (`cancel.child_token()`: a
timeout cancels the child alone, `Esc` on the parent cancels both), and an
**event sink** deciding which of its events reach the UI (§3.5). When it ends,
the run is assembled — messages, identity effects applied, outcome, tokens —
and attached to the parent's record in `execute_call`; the parent's round files
it like any record, and it lands with the turn.

No nesting, twice over: the child's allowed set and schemas exclude
`call_subagent`, so the model never sees it; and a loop at `depth ≥ 1` refuses
the name anyway with the ordinary disabled text.

The child's rounds spend **its own** budgets: `max_tool_rounds` and
`workspace.max_rounds` apply per loop (F6), with the same final "sum up without
tools" round when one runs out; the parent's turn spends exactly one round on
the call, as today. The whole run is bounded by a wall-clock timeout (F7).

### 3.3 What the child gets, and what it does not

"Every tool and capability of the main agent" is read as: the **turn's
effective tool set and the turn's environment**, not the parent's
conversation. The child's `ToolContext` is the parent's **cloned** with
`system_message` and `effective_sampling` replaced — the same attachments
snapshot, the same workspace root and change journal, the same other-chats
scope, the same cancellation lineage — which is what makes `attachment_read`,
`attachment_search` (indexed under the parent's `chat_id`, which the child
keeps as its context id), the `code_*` family (edits journaled under the
parent's `data/workspace/<parent>/`, so `F4` in the parent shows and reverts
what the sub-agent changed), notes, RAG, web, Python, `fs_*`, MCP and the
control tools work unchanged. The child's system prompt is its own system
message **plus the parent's environment blocks** (the attachment block, the
workspace block) — docs/lessons.md §4: a tool the prompt does not name does not
get used. This needs `build_request` to take the environment (attachments,
workspace, compaction) separately from `&Chat` — a preparatory refactor (§7,
PR 1).

Deliberately **not** inherited (F2, F3 — decided):

- `call_subagent` — the requirement.
- `history_read`/`history_search` — they read the **parent's** folded history;
  the sub-agent has no history by design, and these would be a side door into it.
- The self-model family (`get_self_model`, `reflect`, `update_self_model`,
  `update_user_model`, `add_insight`) **and** the self-model injection — the
  self-model is the profile persona's identity (spec §17); a "critic" persona
  reflecting into it would write observations made by somebody else. Notes are
  different — `note_save` writes the profile's knowledge, which the sub-agent's
  work legitimately adds to.
- The parent's messages: the parent decides what to pass, exactly as today.

### 3.4 Effects go to the chat they describe

- `SetSystemMessage`, `SetSamplingOverride` — **the run** (its persona and knobs;
  `get_system_message` answers with the child's).
- `AddAttachment` (a `youtube_watch` transcript, an over-budget `fetch_url`) —
  **the parent**: the attachment index and snapshot are the parent's, and the
  parent's model will want to read what its delegate fetched. Mirrored into
  *both* loops' contexts by `sync_attachments`, so the child reads it next round
  and the parent after the call.

One rule — environment effects to the parent, identity effects to the child —
which is also what the dialogue feature will need (two identities, one
environment).

### 3.5 What the UI sees while the child runs, and when the child appears

**Stage 1: the child lands with the turn.** It is part of the parent's turn in
every sense, including this one: it appears in the list and becomes openable at
`handle_done`, together with the parent's messages; a cancel lands the partial
run with `outcome: Cancelled` (the turn lands, `GenState::Cancelling`); a crash
loses it with the turn. No mid-turn channel, no side table, nothing to keep in
sync. Until the list snapshot carries the child, the `chat://` address in the
parent's card **renders as plain text** — the existing "resolves or it is not a
reference" rule — and turns into a link the moment `emit_chat_list` runs after
landing (the chat book is part of the feed's cache key, so the card re-renders).
Clicking a not-yet-landed address is therefore impossible, which matters:
`switch_to` cancels a running generation *before* it looks the target up
(`chats.rs:126–132`), and a premature jump would have cancelled the turn.

During the run the child's `Chunk`/`Thoughts`/`GenerationStarted`/`Finished`/
`AssistantContinue`/`AssistantRewrite`/`ToolCall` events are **muted** by its
sink (they would land in the parent's live bubble); `ToolConfirmRequest` passes
through; `TokenUsage` is re-based on the parent's totals so the counter keeps
growing with the child's completion tokens (the user pays for them) while the
context figure stays the parent's. A quiet status-bar chip ("sub-agent «name» ·
round N · tool") comes in PR 6.

**Stage 2 (live, §7 PR 7)** adds an in-flight side table in the orchestrator fed
by progress events on the `done_tx` channel (one channel, so FIFO with the
final `GenResult` is guaranteed), the child's stream routed to its own feed when
it is the open chat, and a parent ↔ child switch that does not cancel the turn.
Nothing in stage 1 has to be undone for it.

### 3.6 Persistence, clone, export

- The run is saved when the parent is — the debounced queue, the atomic write,
  the `.bak`, the post-save index pass, backups: all the parent's.
- **Cloning a parent copies its children** (F9, revised): they are inside the
  messages that get copied. `handle_clone` must give every copied run a fresh
  `id` — a duplicate child id in two parents would make `chat://` resolution
  ambiguous ("exactly one match" refuses the link) and `SwitchChat` pick the
  first. The earlier "share by reference" answer was an artefact of the
  separate-file model and is gone with it.
- **Export.** `/export md|json` and `F5`/`/copy` of a child work on its messages
  (it is a transcript like any other); a child exported as `mindfork-import`
  JSON becomes an ordinary chat on import — a copy, which is the point of an
  export. A parent's JSON export does not carry its children: the v1 document is
  single-chat and drops tool calls (`chat_export.rs:216`); its Markdown export
  includes the call's result text under `copy_tool_results`, not the transcript.
  Stated in the docs, not solved.
- **The `deleted` archive** of a *run* (its own `rewrite_current_message`
  rounds): dropped, not kept — the archive's promise is manual recovery of
  something the user lost, and a sub-agent's discarded draft is not that.

### 3.7 The chat list becomes a two-level tree

`ChatSummary.children` is flattened by the widget into rows with a depth:
parents in the current sort mode, each followed by its children in **creation
order** (they are the steps of the parent's work; "newest first" would read
them backwards). Row rendering gains an indent and the `└` marker (WGL4, one
column — lessons §5) in the `PREFIX_W` arithmetic, plus a small outcome mark for
a run that was cancelled, timed out or did not finish.

The filter rule, identical in both scopes: **a row is shown iff it matches; a
parent is additionally shown (dimmed) when any of its children matches.** In
title mode "matches" is the substring test; in content mode it is membership in
the `ChatSearchResults` id set (§3.9). A matched child therefore never appears
without its parent, an unmatched child does not pad a matched parent, and text
found only in the parent shows the parent alone — the rule the user stated.

Selection, `PageUp`/`Home`/`End`, virtualization: unchanged — the tree is the
same `Vec` the widget already walks. Keys on a child row: `Enter` opens, `F2`
renames, `Ctrl+R` titles, `F5` copies; `Del`/`Ctrl+D` refuse with a notice in the
status area naming the two routes that remove a child (`Ctrl+E`/`Ctrl+R` in the
parent); the hotkey grid greys the two out while a child is selected. The
orchestrator refuses `DeleteChat`/`CloneChat` for a child id too (command-only
routes, races).

### 3.8 Opening a child: a read-only transcript with a system bubble

`SwitchChat(id)` resolves a child id (the top-level lookup fails, the child walk
succeeds) and runs `activate_child`: `ChatActivated` carries the child's `id`,
`title`, `messages`, an empty `draft`, the **parent's** `feed_view` (`Ctrl+T`/
`Ctrl+O` state is shared — the child is part of the parent), `focus`, no
`compaction`, plus two new fields filled only for a child: `origin { parent,
parent_title }` and `system: String`. The chat screen stores `read_only`, draws
the system message as a first **system bubble** (a new `FeedRole::System`, muted
rail), titles the input box "read-only transcript" and unfocuses it, and shows a
status chip.

**`active_id` holds the child's id while a child is open.** Every orchestrator
path that looks the active id up in `self.chats` then fails closed — which is
the read-only behaviour for free, with no list of handlers to keep in step. The
few paths that must work on a child opt in through one resolver,
`view(id) -> Top(&Chat) | Child { parent: &Chat, run: &SubagentRun }`: rename
(`F2`, `/rename`), `Ctrl+R` titling, `F5`/`/copy`, `/export`, `/tts`,
`SetFeedView` (routed to the parent's field), `OpenChatAt` (a search hit inside
a child), `remember_active_chat` (so restarting on a transcript reopens it),
`SetDraft` (ignored — a child has no draft). `Back::Link` and `Back::Search`
need nothing: they stash ids.

Refused **with a note** (not silently, lessons §4): `Send`, `Ctrl+R`/`/regen`,
`Ctrl+E`/`/takeback`, `Ctrl+U`/`/impersonate`, `/compact`, `/file`, `/image`,
`/project`, `/clone`. Working as usual: `/copy`, `/rename`, `/export`, `/search`,
`/find`, `/links`, `/chats`, `/help`, `/settings`, `/thoughts`, `/toolcalls`,
`/tts`, `/new`, `/self`, `/rag`, `/exit`; `Ctrl+T`/`Ctrl+O`, scrolling,
selection, `Ctrl+L`, `Esc`.

A child cannot be open while its own run is in flight in stage 1 (it does not
exist outside the task until the turn lands), so the cancel-on-switch rule needs
no change yet.

### 3.9 Search

- **The index learns a sub-scope.** `messages` gains a nullable `sub_id` column
  (`CACHE_SCHEMA` 1→2 — a free bump: the file is wiped and rebuilt by the
  startup pass); `indexed_messages(chat)` emits the parent's own messages with
  `sub_id = NULL` and each run's messages with `sub_id = run.id`, all under the
  parent's `chat_id` so the per-file bookkeeping, the guarded re-index and
  `forget_chat` keep working untouched. Message ids are uuids on both levels,
  so the `(message_id, text_hash)` diff needs no change.
- **`search_chats` returns `DISTINCT COALESCE(sub_id, chat_id)`** — a flat id
  set in which a child id stands for itself, so the widget rule of §3.7 is one
  membership test. `MessageHit` gains `sub_id`; `matching_messages_in_chat`
  takes the scope (`sub_id = ?` for a child, `chat_id = ? AND sub_id IS NULL`
  for a parent), so `Enter` on a parent row lands on the parent's own first
  match and `Enter` on a child row on the child's.
- **`Ctrl+G`**: `SearchGroup` gains `parent: Option<Uuid>`; `group_hits` groups
  by `(chat_id, sub_id)` and emits a parent group before its matched children —
  a parent with no hits of its own gets a header row with "0 matches", so a
  child is never orphaned in the results; child headers carry the `└`.
  Navigation is over hits by construction, so extra header rows cost nothing.
  `OpenChatAt { chat: child_id, message }` opens the child on the hit (§3.8).
- **`chat_search`/`chat_read`** (F4, taken by recommendation): the other-chats
  snapshot gains the profile's children as `ChatRef`s carrying their parent (so
  `chat_read` knows which file to open), results label them as "sub-agent
  transcript of «parent»", and `search_messages_in` scopes on
  `COALESCE(sub_id, chat_id)`.

### 3.10 Titles

The child starts with a title it can stand on before any model names it (F13):
`name` when given, else the first line of `message`, sanitized and truncated.
Then `interface.auto_title` applies. A child's whole life lands at once
(§3.5), so **both** trigger points fire at landing — `AfterUserMessage` and
`AfterAssistantReply` alike — for every landed run with a substantive reply;
`Off` fires nothing. Titling at landing also closes the single-slot hazard of
the earlier draft (a title request racing the parent's next round): the turn is
over when the requests go out. `start_title_task`/`handle_title_result` resolve
the id through the same `view()` as §3.8, build the digest from `run.messages`,
write `run.title`, and respect `run.renamed_manually` — a hand rename (`F2` in
the list, `/rename` in the open transcript) wins, as everywhere. A child rename
marks the parent dirty without bumping its `modified_at` (a rename is not a
conversation change — the rule the draft and the feed view follow).

### 3.11 The parent's card and what the parent's model gets back

The tool result string = the child's final reply text, followed by one trailer
line naming the transcript — `chat://xxxxxxxx` — so the parent can cite it in
its own reply (the feed turns it into a navigable reference once the turn
lands) and, from the next turn on, read it with `chat_read` (F4). The trailer
does not promise `chat_read` *now*: the other-chats snapshot was taken at the
turn's start (lessons §4 — only advertise what exists). A run that ended by
cancel/timeout/limit says so in the same sentence.

The feed card shows the child's **title and address in its header** (visible
collapsed — the one place a collapsed card shows anything; the record has the
run right there, so a rename shows on the card too) and renders the reply as
markdown rather than `Plain`; `Ctrl+L` and a click follow the address.
`present.rs`'s field order for the tool becomes `name`, `system_message`,
`message`.

### 3.12 Budgets, timeouts, cancellation, settings

- Rounds: `max_tool_rounds` / `workspace.max_rounds` per loop (F6).
- Time: **one knob for the whole run**, `tools.subagent_run_timeout_secs`
  (default 600), replacing `subagent_timeout_secs` (F7, decided: rename). The
  rename is done by a `SETTINGS_SCHEMA` 1→2 step rather than by letting serde
  drop the unknown key: the step carries a changed value over (a stored `60`,
  the old default, is dropped so the new default applies; anything else is kept
  as the run limit). Timeout → the child's token is cancelled, the partial run
  lands with `outcome: TimedOut`, the parent is told in the result.
- Tokens: `subagent_max_tokens` stays the per-round reply cap, min'ed with the
  effective `max_tokens` as today; the default rises to 4096 — a sub-agent that
  has just done three searches writes a longer answer than a one-shot opinion —
  and the same settings step lifts a stored `1024` to it.
- `Esc`: the parent's token is cancelled, the child token with it; the run lands
  with the turn, `outcome: Cancelled`. `Quit` mid-run: the same path as any turn.

### 3.13 Migration

Two artifacts change shape; both follow ADR 0006 to the letter, and both are
the scaffold's first real use.

**Chat files — `CHAT_SCHEMA` 1→2 (F10, decided: migrate).** A step that walks
`messages[]` **and** `deleted[].messages[]`, and for every `tool_calls[]` entry
named `call_subagent` that has no `subagent` yet synthesizes one from what the
record already holds: `id = Uuid::new_v5(SUBAGENT_NS, "<chat id>/<call id>")`
(deterministic, so re-running the step on a restored backup yields the same
addresses), `system_message = arguments.system_message`, `messages =
[User(arguments.message) @ the assistant message's timestamp, Assistant(result)
@ the matching `Tool` message's timestamp]`, `title` = the first line of the
message, `outcome: Completed`, `tokens: 0`, `name: None`; then `v = 2`. Nothing
existing is removed or rewritten — the migrated file is a **superset** of the
old one, which is what "chat files must not break" asks for; the version bump
exists so the synthesis runs exactly once, through the backup-then-write path.
Idempotent by construction (a record that has a run is skipped). `Chat` gains
`v: u32` (default 1 on read, written as `CHAT_SCHEMA` from now on) so a re-saved
file does not detect as 1 again. A golden fixture of a real pre-migration chat
with one old call pins the step; `real_registry_is_all_current_v1` becomes
"chats at 2, steps non-empty". An old binary refuses a v2 file (the downgrade
guard) — fine before the first public release. Only **Completed** is
synthesized: the old tool wrote its error/timeout/empty texts into `result` in
the profile's language, and guessing an outcome from wording is not worth a
wrong one.

**Settings — `SETTINGS_SCHEMA` 1→2**: the rename and the lifted cap of §3.12.
`config::SCHEMA_VERSION` moves to 2 with it (the invariant test ties the two).

Both steps are pure `Value → Value` in `shared/storage/schema/`; the title
sanitizer they need (`sanitize_title`, today in `features/rename_chat.rs`)
moves down to `shared` so the step can use the same one the list uses. One
pre-migration backup covers both artifacts on a user's first start after the
upgrade (`data_migration::run_with`).

### 3.14 Extensibility: the two-agent dialogue

The next feature — two sub-agents with different system messages talking to
each other under the main agent's direction, viewable as an ordinary chat — maps
onto the seams above without reopening them:

- `RunKind::Dialogue` on the same `SubagentRun`; the list nesting, the read-only
  screen, the deletion semantics, the search scoping and the titles all key off
  "this record has a run", never off the kind. The record of the director's
  call holds the whole dialogue — one call, one transcript, as now.
- A `Message.author: Option<String>` (additive, added then) names which
  participant wrote an assistant message; the feed header shows it, and a
  request for participant A is built by the **role swap** impersonation already
  does (`swap_role_message`, `orchestrator/impersonation.rs:232`): A's messages
  are `assistant`, B's are `user`, A's tool records travel with A's messages.
- The director tool (`run_dialogue { a: {name, system_message}, b: {…},
  opening, max_turns }`) is a second loop-executed tool; each participant's turn
  is one child `TurnLoop` run over the shared transcript — the same child runner
  §3.2 builds, called alternately. The effect rule of §3.4 (environment → the
  parent, identity → the participant) needs no change; `participants:
  Vec<Participant>` joins `SubagentRun` then.
- Deliberately **not** built now: `Message.author`, `RunKind::Dialogue`, the
  director tool, `participants`. `RunKind` and `RunOutcome` are the only two
  places this stage leaves room on purpose.

## 4. Difficult spots (named explicitly)

1. **"Every capability" vs. "no history."** Three capabilities *are* history in
   disguise — `history_read`/`history_search`, `chat_read` of the parent, the
   self-model. §3.3 excludes them (decided).
2. **The child is invisible until the turn lands** (stage 1). A long run is a
   growing token counter and, from PR 6, a status chip; nothing to open. This is
   the honest consequence of "part of the parent's turn", and stage 2 is where
   live viewing goes.
3. **The first real schema bumps.** Both steps are small, but they are the
   first time the dormant scaffold runs on real data: the pre-migration backup
   fires once per install, a downgrade is refused, and a corrupt chat that
   cannot be control-parsed is skipped with a warning rather than migrated
   (ADR 0006 §5) — it keeps its old shape and its old card, which the code must
   therefore still render (the `None` branch).
4. **A clone must re-id its children** or two parents answer to one `chat://`
   prefix.
5. **JSON export is single-chat** and drops tool calls: children do not
   round-trip through `/export json`. Documented, not solved.
6. **Token cost becomes visible.** A sub-agent with eight rounds is eight
   requests; the settings descriptions say that `max_tool_rounds` applies to
   each sub-agent, and the parent's counter grows with the child's tokens.
7. **Message cloning gets heavier.** `ChatActivated` clones the parent's
   messages, runs included; a parent with many long delegations pays that on
   every activation and save. Acceptable today; the same class of cost as
   images in messages (spec §9.10), and the first thing to measure if a real
   chat ever feels slow.
8. **The system bubble** is a new feed role drawn for child chats only (F12);
   drawing every chat's system message would be a separate UX decision.

## 5. Forks

Decisions recorded 2026-08-23; "by recommendation" = proposed with a
recommendation, not objected to when the first draft was reviewed — still open
to a veto.

- **F1. Where the sub-agent runs.** (a) **A loop-executed tool: `call_subagent`
  is recognised in `TurnLoop::resolve_call_result` and runs a child `TurnLoop`**
  with the parent's shared machinery *(by recommendation)*. (b) A runner trait
  injected into `ToolDeps`. (c) A separate `start_generation` on a child chat.
- **F2. Tools withheld besides `call_subagent`.** (a) **Also
  `history_read`/`history_search`** *(by recommendation)*. (b) Only `call_subagent`.
- **F3. The self-model.** **Exclude the five tools and the injection.**
  *User's decision (2026-08-23).*
- **F4. Child transcripts in the other-chats scope** (`chat_search`/`chat_read`).
  (a) **Included, labelled as a sub-agent transcript of their parent** *(by
  recommendation)*. (b) Excluded.
- **F5. The result trailer.** (a) **Final reply + one line with the `chat://`
  address and, when the run did not complete, why** *(by recommendation)*.
- **F6. Round budgets for the child.** (a) **Reuse `max_tool_rounds` and
  `workspace.max_rounds` per loop** *(by recommendation)*. (b) A separate knob.
- **F7. Time and token limits.** **Rename `subagent_timeout_secs` →
  `subagent_run_timeout_secs` (600, the whole run) through a settings step; lift
  `subagent_max_tokens` to 4096 in the same step.** *User's decision
  (2026-08-23): rename — settings may break before the first release.*
- **F8. When the child's auto-title fires.** **At landing, for both trigger
  points** — resolved by construction under the embedded model (§3.10).
- **F9. Cloning a parent.** **The children are copied with the messages and
  re-id'd** — the separate-file answer ("share by reference") no longer applies.
- **F10. Old `call_subagent` records.** **Migrate: `CHAT_SCHEMA` 2, a step that
  synthesizes a run per old record; the migrated file is a superset of the old
  one.** *User's decision (2026-08-23).*
- **F11. Where the child is stored and how it is deleted.** **Inside the
  parent's file, on the call's record; removed only by `Ctrl+E`/`Ctrl+R` of the
  spawning exchange (and with the parent itself).** *User's decision
  (2026-08-23).*
- **F12. The system bubble.** (a) **Child chats only** *(by recommendation)*.
- **F13. Initial title.** (a) **`name` if given, else the first line of
  `message`** *(by recommendation)*.
- **F14. The optional `name` argument.** **Add it.** *User's decision (2026-08-23).*
- **F15. Staging.** **Many small PRs** (§7). *User's decision (2026-08-23).*
- **F16 (new). Search rule.** **A child matches → shown with its parent; only
  the parent matches → the parent alone.** *User's decision (2026-08-23);* the
  same rule in title mode, by recommendation.
- **Considered and rejected here:** a file per child (the user's clarification
  — and it had needed a cascade, a profile field, an `is_hidden`, a separate
  index walk and a "share or copy on clone" question, all of which the embedded
  model dissolves); attaching the run to the `Tool`-role message (the feed
  never sees those); a third loop; storing the parent's attachments on the run;
  confirming `call_subagent` itself as dangerous — its dangerous sub-calls are
  confirmed one by one, which is the protection that matters; a per-call
  `tools: bool` argument — possible later, not asked for.

## 6. Test plan

Unit (scripted backends, the `ctx_with_backends`/`testkit` fixtures):
- the child's schemas = the parent's minus `call_subagent`, `history_*`, the
  self-model family; a child calling `call_subagent` is refused with the
  disabled text; `depth` never exceeds 1;
- the child's system prompt carries the attachment and workspace blocks, not the
  self-model; its context keeps the parent's `chat_id`, journal path and
  other-chats snapshot;
- effects routing: `SetSystemMessage` lands on the run, `AddAttachment` on the
  parent and in both contexts;
- the run lands on the record: assembled messages, outcome, tokens; a request
  built from a chat with stored runs is byte-identical to one without;
- cancel mid-child → `Cancelled`, the rounds so far on the record; run timeout →
  `TimedOut`; round limit → the final round, `RoundLimit`;
- confirmation inside the child reaches the popup with the parent's id and
  `AllowForTurn` covers a later child call;
- `Ctrl+E`/`Ctrl+R` over the spawning exchange move the run into `deleted` and
  out of the list; `Del`/`Ctrl+D` on a child row → a notice and no command;
  `DeleteChat`/`CloneChat` with a child id are refused; a clone re-ids runs;
- list tree: parents by mode, children in creation order; the filter rule in
  both scopes (F16); `Enter`/`F2`/`Ctrl+R`/`F5` on a child; the row prefix width
  and the `└` glyph (WGL4, width 1);
- read-only screen: every refused command/chord produces its note; every
  allowed one works; `SetDraft` is ignored, `SetFeedView` reaches the parent;
  the system bubble renders with the `system` name and is excluded from search
  highlighting like the role header; the `User` header names the parent persona;
- `chat://`: a landed child resolves, an unlanded address is plain text, a
  prefix shared by two runs refuses; `Back::Link` returns to the parent;
- index: child messages carry `sub_id`; `search_chats` yields child ids;
  `matching_messages_in_chat` scopes per level; `Ctrl+G` nests a child group
  after its parent with a hit-less parent header; `chat_read` of a child opens
  the parent's file;
- titles: the initial title from `name`/`message`; both modes fire at landing,
  `Off` does not; `renamed_manually` wins; a child rename leaves the parent's
  `modified_at` alone;
- migration: the chat step on the golden fixture (records in `messages` and in
  `deleted`, deterministic ids, idempotent on a second run, a record with a run
  untouched, control-parse passes, `v = 2` written, a re-saved file detects
  as 2); the settings step (rename; `60` dropped, `120` carried; `1024` lifted,
  `2048` kept); `real_registry` expectations updated; the downgrade guard on a
  v2 file under `current = 1`;
- i18n: `en`/`ru` keys present, no `{}` leftovers, no Cyrillic in `en`.

Live smoke — the track's **go/no-go** (`live.rs`, `MINDFORK_ENGINE_URL`-gated,
Gemma 4 and Qwen 3.6): the parent is told to delegate a question that can only
be answered by reading a planted fixture file (`fs_read` under `fs_root`, or
`chat_search` over a seeded chat); assert that `call_subagent` was called, that
the record carries a run with at least one tool record, that the parent's answer
carries the planted token, and that the list snapshot nests the child. **Go** =
the sub-agent used a tool and the token came through on ≥ 4 of 5 runs per
model; **no-go** = the models narrate instead of calling (the known Gemma
failure mode, lessons §2) — then the tool's description needs the kind of
wording the workspace block had to learn, before anything is built on top.

## 7. Stages — one PR each

1. **`refactor/turn-loop-child-seam`** — no behaviour change: `build_request`
   takes the environment apart from `&Chat` (`RequestEnv { attachments,
   workspace, compaction }`); `TurnLoop` splits into a shared part and a
   per-loop part with a `depth`; `sanitize_title` moves to `shared`. Existing
   tests green, nothing else.
2. **`feat/subagent-run`** — the engine and the data: `SubagentRun` on
   `ToolCallRecord`, the child loop with tools minus the exclusions, the `name`
   argument, effects routing, outcomes, the result trailer, the settings rename
   with its step and fixture, `Chat.v`. The card shows the title and address as
   text. **The live smoke is this PR's go/no-go.**
3. **`feat/subagent-migration`** — `CHAT_SCHEMA` 2: the synthesis step, the
   golden fixture, the CHANGELOG "Data" item.
4. **`feat/subagent-list`** — `ChatSummary.children`, the tree rows and
   refusals, `activate_child` and the read-only screen with the system bubble,
   `active_id` as a child id with the `view()` resolver, per-child names,
   `chat://` resolution (the card address becomes a link), `Back::Link`,
   rename/`Ctrl+R`/`F5`/`/export` on a child, clone re-ids, `last_active_chat`,
   the help overlay and key tables.
5. **`feat/subagent-search`** — `sub_id` in the index (`CACHE_SCHEMA` 2), the
   id set, the list rule, `Ctrl+G` grouping, `OpenChatAt` into a child,
   `chat_search`/`chat_read` over children.
6. **`feat/subagent-title-chip`** — auto-title at landing, the status-bar chip
   during a run, the demo fixture and screenshot needles, `/copy` wording.
7. **`feat/subagent-live`** (stage 2, after a live run argues for it) — the
   in-flight side table, the child's stream into its own feed, a parent ↔ child
   switch that does not cancel, a "running" card in the parent.

An ADR (0010: the sub-agent as a nested turn, the transcript on the call's
record) goes in with PR 2. The two-agent dialogue is its own research document
afterwards.

## 8. Documentation touch list (AGENTS.md §4)

spec §5.1 (`SubagentRun`, `Chat.v`), §9.3.2 (rewritten), §9.3 roster row, §9.11
(children in scope), §11.2 (tree rows, refusals, the hotkey grid), §11.2.1
(grouping), §11.3 (the system bubble, the card), §11.6 (settings), §11.7 (key
table), §12.2 (the first real steps), §13.3; architecture §5 (the nested turn),
§7 (the record-borne run, the index's `sub_id`, the two steps), §8 (the group
table row and an implementation note), §10 (read-only, `active_id` as a child);
`docs/journal/tools.md`, `storage.md` and `ui-screens.md` entries; CHANGELOG
(Added/Changed/Data); README (the list keys, the tool, the settings);
`docs/roadmap.md` (the dialogue track, stage 2); the help overlay; ADR 0010.
