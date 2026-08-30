# Research: the assistant learns its language model, and the profile keeps a history of changes

Status: **decided and implemented** (branch `feat/llm-name-history`), plus the
one-time backfill of §9 (branch `feat/llm-history-backfill`). The forks in §4
were confirmed by the user on 2026-08-29; §3 describes the design as adopted.
The shipped shape is documented in spec §9.14 — this file keeps the reasoning
and the options that were *not* taken.

## 1. The ask

1. A tool with which the assistant can learn the **name of the language model
   (LLM)** currently generating its replies.
2. The **profile** stores a history of language-model changes as dated records
   (date → LLM name), and the assistant has a tool to read it.
3. A history record is written **after a completed exchange** — the user sent a
   message and received the assistant's reply.
4. No record when the current model **does not differ** from the previous
   record.
5. No record when the API **does not expose** a model name.
6. Tool names must make the **language model** unmistakably distinct from the
   **self-model** (the assistant's stored personality, spec §17).

## 2. What exists today (survey)

The survey reshaped the task the way docs/lessons.md §3 predicts: almost every
mechanism this feature needs already exists; what is missing is exposure to the
assistant and the per-profile aggregate.

- **The name is already resolved once per turn.** `start_generation` reads
  `Orchestrator::effective_model_name()` exactly once (`generation.rs`, the
  "resolved once and used twice" comment) — config first
  (`EngineSettings::active_model_name()`: managed → GGUF display name, external
  → the typed "Model (opt.)" field, clouds → the provider's `model_name`), then
  the discovered fallback (`ModelDiscovery.known`, asked of the server via
  `EngineBackend::model_id()`: `GET /v1/models` when it lists exactly one, else
  `/props`; see [external-model-name.md](external-model-name.md)). The value is
  frozen into `GenSpawn.model_name` and lands in every reply's
  `MessageMetadata { mode, model, … }` (`entities/message.rs`).
- **"Cannot say" has exactly one spelling: `None`.** `model_id()` returns
  `Option<String>` with no error channel and no placeholder; clouds other than
  the OpenAI-compatible path answer `None` by trait default. So requirement 5
  maps 1:1 onto `MessageMetadata.model == None`.
- **The single commit point of an exchange is `Orchestrator::handle_done`**
  (`app/orchestrator/generation.rs`). Every turn — send, regenerate,
  `/continue`, cancelled, errored — ends there; a turn that produced nothing at
  all takes the early return (`messages/effects/deleted` all empty). Five
  post-turn follow-ups (auto-title, auto-reflect, auto-consolidate ×2,
  auto-compact) already hang off it. Background tasks (titling, compaction,
  impersonation, reflection) do **not** land messages there — they can never
  count as an exchange, which is exactly what requirement 3 wants.
- **Nothing exposes the name to the assistant.** No tool returns it, and the
  system prompt never mentions it. `ToolContext` carries the raw
  `engine: Arc<dyn EngineBackend>`, but asking `model_id()` from a tool would
  bypass the typed-name-wins precedence and spend a round trip — the
  orchestrator-side resolver is the single source of truth ("one value, one
  meaning", journal/engine.md).
- **`Profile` has no dated lists and no timestamps today**, but the additive
  recipe is well-worn: a `#[serde(default)]` field needs no migration
  (ADR 0006 F12); `Chat.deleted: Vec<DeletedExchange>` is the structural
  precedent for a dated `Vec<record>` on a JSON entity, and
  `children_expanded_is_additive_and_round_trips` (`entities/chat.rs`) is the
  contract-test template.
- **The naming minefield is real.** "Model" already means four things:
  `self_model` (the personality), `user_model` (a sub-struct of it),
  `*_model_name` / `MessageMetadata.model` (the LLM), and the embedding model
  (`embed_*`). The LLM side's established prefix in code is "engine"
  (`EngineBackend`, `EngineSettings`, `AppEvent::EngineModel`).
- **Considered and set aside: derive the history from chat metadata on
  demand.** Every reply already stores its model name, so a tool could scan the
  profile's chats and reconstruct the sequence. Rejected: the ask explicitly
  says the history lives **on the profile**; a derived history silently loses
  entries when chats are deleted/hidden; pre-feature exchanges carry no
  metadata; and a per-turn scan of every chat is work the stored aggregate does
  once. The chat metadata stays what it is — per-message ground truth — and the
  profile record is the durable aggregate.

## 3. Design overview

Three pieces, each small:

1. **Two read-only tools** in `ToolGroup::Introspection`, implemented in a new
   `src/features/tools/llm.rs` (the deliberate mirror of
   `features/tools/self_model.rs` — the two files *are* the naming boundary):
   - `get_llm_name` — answers with the turn's own frozen name and the engine
     mode, from new `ToolContext.model_name` / `engine_mode` snapshot fields.
     The turn's header, the stored metadata and the tool's answer come from
     the same single read and cannot disagree.
   - `get_llm_history` — renders the profile's stored history, oldest first,
     read live from `data.db` through `ctx.storage` — the same route every
     DB-backed organ (notes, self-model) takes, so an exchange landing
     mid-conversation is visible to the very next turn.
   Threading: `effective_model_name()` moves a few lines earlier in
   `start_generation` (before the `ToolContext` build — one read, now used
   three times); `TurnInfo` gains `model_name` and `engine_mode`; the
   `ToolContext` build sites (`generation.rs`, `background_tool_ctx`,
   `testkit::test_turn`, two test fixtures) follow.
2. **A recorder in `handle_done`.** Before the messages are moved into the
   chat (the same "read before the apply" position `is_first_reply` uses, and
   before `land_continuation` consumes a continuation's tail): take the newest
   assistant message's `metadata.model`/`mode`. After the commit block:
   resolve the chat's `profile_id` and call `Db::llm_history_note` — a
   read-compare-append under one mutex acquisition (the `self_model_update`
   pattern) that writes `{ changed_at: Utc::now(), model, mode }` only when
   the pair differs from the newest record. Best-effort: a failed write is
   logged, never fails the turn (lessons §8). No new setting gates the
   recorder — it is bookkeeping on the same tier as `MessageMetadata` itself.
3. **A new `data.db` table** (additive `CREATE TABLE IF NOT EXISTS`, no
   `DB_SCHEMA` bump, ADR 0006 F12):

   ```sql
   CREATE TABLE IF NOT EXISTS llm_history (
       rowid       INTEGER PRIMARY KEY,   -- explicit: VACUUM keeps order
       profile_id  TEXT NOT NULL,
       changed_at  TEXT NOT NULL,         -- RFC3339, like every entity date
       model       TEXT NOT NULL,
       mode        TEXT NOT NULL          -- ServerMode::key(), pinned to serde
   );
   ```

   The entity is `entities::profile::LlmChange { changed_at, model, mode }` —
   named `Llm*`, never `*Model*` alone, for the same reason the tools are.
   `ServerMode` gained `key()`/`from_key()` (the stable serde spelling,
   pinned by `server_mode_key_matches_serde`).

Dedup is against the **last record only** (requirement 4 says "differs from
the previous", not "never seen"): switching A→B→A legitimately writes three
records — that *is* the history of changes. The comparison is the **pair
(name, mode)** — a follow-on of F4: with the mode stored, deduping by name
alone would leave the newest record lying about the mode.

## 4. Forks

**F1. Tool names.**
- (a) **`get_language_model` + `get_language_model_history`** — the explicit
  compound; reads as the direct counterpart of `get_self_model`, so the
  distinction the ask demands is carried by the names themselves. Follows the
  Introspection group's `get_*` convention.
- (b) `get_llm` + `get_llm_history` — shorter; "llm" collides with nothing;
  but an abbreviation where the sibling family spells its noun out.
- (c) `get_engine_model` + `get_engine_model_history` — matches the codebase's
  internal "engine" prefix, but "engine model" is the least self-evident
  phrase to the model reading the schema.
- **Recommendation: (a).** UI labels: "show language model" /
  "language-model history".
- User's decision (2026-08-29): **a sharpened (b) — `get_llm_name` and
  `get_llm_history`** ("llm" collides with nothing, and the `_name` suffix
  keeps the first tool from reading as "return the model object"). UI labels:
  "show LLM name" / "LLM history".

**F2. Two tools or one.** One `get_language_model` returning both the current
name and the history would be one schema instead of two — but the current-name
question is the frequent one ("what model are you?") and would drag the whole
history into every such turn; the ask names two capabilities. **Recommendation:
two tools.** User's decision (2026-08-29): **two tools**.

**F3. Where the history lives.**
- (a) **`Profile.language_model_history` in `profiles.json`** — additive field,
  survives the `data.db`-lost-next-to-JSON scenario (`chats_without_db`),
  travels with backups of the JSON set, and "in the profile" is the letter of
  the ask. Growth is bounded by actual model switches (dedup), so the
  load-all/save-all `upsert_profile` cost stays trivial.
- (b) a `model_history` SQLite table keyed by `profile_id` — the per-profile DB
  precedent (notes, self_models); but the history silently starts empty
  whenever `data.db` goes missing while `profiles.json` survives, and nothing
  about the data needs SQL.
- **Recommendation: (a).** User's decision (2026-08-29): **(b), `data.db`** —
  the history is not valuable enough to earn a place in `profiles.json`; the
  lost-`data.db` scenario losing it is acceptable by the same judgement.

**F4. Record shape.** Minimal `{ changed_at, model }` (the ask: date → name),
or additionally `mode: ServerMode` (managed/external/openai/…)? The mode adds
context ("the same GGUF via managed vs. external") but complicates the dedup
question (does a mode change with the same name count?). **Recommendation:
minimal — name only; `mode` can be added additively later if it earns its
place.** User's decision (2026-08-29): **include `mode` now.** Follow-on
decision (implementer's, recorded in §3): the dedup therefore compares the
pair (name, mode) — a mode change with the same name writes a record, or the
stored mode of the newest record would lie.

**F5. Which turns write a record.**
- (a) **any turn that lands an assistant reply with a known model name** —
  send, regenerate, `/continue`, and a cancelled/errored turn that still
  produced a partial reply (the model demonstrably answered in this profile;
  the empty-turn early return already filters the rest);
- (b) only `finish == Stop` (a "clean" reply) — stricter reading of "received
  the answer", but then a model used only for one long cancelled reply never
  enters the history, and the extra rule buys nothing the reader can act on.
- **Recommendation: (a).** User's decision (2026-08-29): **(a)**.

**F6. The baseline record.** With an empty history, the first qualifying
exchange writes the profile's first record. Strictly it records "the model in
use", not yet a *change* — but without it the original model is unrecoverable
and the first real switch writes a history that starts mid-story.
**Recommendation: write the baseline record.** User's decision (2026-08-29):
**write it**.

**F7. Default enablement.** Both tools `enabled_by_default = true`, like the
rest of Introspection (`get_sampling`, `get_last_user_message_time`) — they are
read-only and reveal only what the chat header may already show;
`reconcile_tools` then auto-adds them to existing profiles. The off-by-default
precedent (self-model, cross-chat search) guards behavioural/abuse surface
these tools do not have. **Recommendation: on by default.** User's decision
(2026-08-29): **on by default**.

## 5. Degradation texts (the door must close, lessons §4)

- `get_llm_name`, name unknown: the engine does not report a model name
  (naming the mode), no other route exists this turn, and the user can name
  the model in the engine settings — the failing routes and the working one
  (`tool.get_llm_name.unknown`).
- `get_llm_history`, empty: no changes recorded yet, records appear after an
  exchange whose model name is known — and, when the turn knows it, the
  current model is named **in the same answer** rather than by advertising
  the sibling tool, which a profile may have toggled off
  (`tool.get_llm_history.empty` + `.current`).
- Known first-exchange gap, documented rather than fought: a turn sent in the
  same tick as bootstrap in external mode may legitimately precede discovery
  and record nothing (journal/engine.md) — the next exchange records.

## 6. Testing and the live gate

Unit (next to the code, as it is written):
- catalog membership + default-set membership; en/ru descriptions localized and
  distinct (the `descriptions_are_localized` pattern);
- recorder via orchestrator fixtures: first exchange writes the baseline;
  changed name → second record; unchanged → no duplicate; mode change with
  the same name → a record; metadata with `model: None` → no record;
  cancelled-with-partial per F5; profile isolation; vanished chat best-effort;
- storage: round trip, per-profile isolation, insertion order, pair-dedup
  against the newest record only; `ServerMode::key` pinned to serde;
- tool rendering: known name, unknown name, empty history (with and without a
  known current model), multi-record order, the stable line format.

Live (mandatory — tool + engine path, AGENTS.md §3): an `#[ignore]` e2e on the
live stack asking the assistant what model it runs on, asserting the tool **was
actually called** (a refusal must not read as a pass, lessons §9) and that the
answer carries the name the harness itself obtains from `model_id()`. The
history tool's logic is deterministic and stays unit-covered; the smoke's turn
also exercises its schema being offered.

Known rot: two new rows in the profile tools list change the `settings-tools`
demo frame — regenerate the dumps (`cargo test dump_demo_frames -- --ignored`,
`python tools/screenshots.py`).

## 7. Documentation impact (AGENTS.md §4)

spec §9.14 (new section, §9.11's anatomy: per-tool bullets, scope, defaults,
degradation) + the §9.3 roster table, both cross-referencing §17 for the
language-model/self-model distinction; architecture §8 (tool table, module
tree: `features/tools/llm.rs`) and the §7 storage diagram (`llm_history`);
journal/tools.md entry + index; CHANGELOG `[Unreleased]` → Added; CLAUDE.md
status/test count. i18n: `tool.get_llm_name.*`/`tool.get_llm_history.*` +
`ui.tool.label.*` in **both** bundles (keys passed whole, never assembled).

## 8. Out of scope

- Any user-facing UI for the history (a settings pane, an `F3`-style screen, a
  `/`-command) — the assistant-facing tools are the ask; the user can always
  ask the assistant, which is the point of the feature.
- Recording inside nested turns (`call_subagent`, `run_dialogue`) — they run on
  the parent's engine; the parent turn's record covers them. The new tools stay
  available to nested turns (harmless, read-only).
- A cap on the history's length — dedup bounds growth to actual switches;
  revisit only if a real profile shows bloat.
- Deriving history retroactively from existing chats' `MessageMetadata` — a
  one-time backfill is possible later (the `migrate_self_narrative` pattern)
  but is not part of the ask. **Done afterwards — see §9.**

## 9. Follow-up: the one-time seed (2026-08-30)

The feature shipped and immediately showed what the last bullet of §8 had
parked: on every profile that existed before it, `get_llm_history` answers
"nothing recorded yet" — truthfully, and about a past that is written down in
full. Every stored assistant reply already carries the model that generated it;
only the aggregate was missing. So the backfill was implemented
(`app/orchestrator/llm_history.rs`, spec §9.14).

**The rule, and why there is only one.** *The seed is what the recorder would
have written, had it existed when those replies were generated.* Everything
else follows and needed no separate decision: records are ordered by the
replies' own timestamps **across the profile's chats** (the recorder fired per
exchange, in time order — two chats are not each other's future), `changed_at`
is that timestamp rather than the seeding moment (a history dated "now" is a
list of names), and consecutive runs of the same (name, mode) collapse exactly
as `llm_history_note`'s dedup collapses them — A→B→A is still three records.
The rule also settles the scope questions: only chats the app can see
(a soft-deleted chat is gone from every other read path, and this is not the
place to make an exception), only `Chat::messages` (a sub-agent transcript runs
on the parent turn's engine — §8 above — and lives on a tool-call record, so it
is skipped by construction).

**One-time without a marker.** The condition is the data itself: a history with
**no records at all** is seeded, and the check happens inside the transaction
that writes (`Db::llm_history_seed`), so the first record ever written — seeded
or recorded — closes the door, and a seed that fails halfway leaves nothing
behind rather than a half-history that looks non-empty and can never be
completed. What the condition deliberately does *not* get is a "seeded already"
flag: a profile whose replies name no model (the pre-`MessageMetadata` corpus,
or an engine that never said a name) writes nothing and stays seedable, so a
later launch that does find something still fills it. The re-scan costs a walk
over chats that bootstrap has just loaded into memory — cheaper than the row
that would record having done it.

**Where it runs.** `Orchestrator::bootstrap`, after profiles and chats are
loaded and before anything can record a first record of its own; best-effort
per profile, like the recorder (a failed read or write is logged and skipped —
no part of startup hangs on bookkeeping). A side effect worth naming: this is
also the repair path for a history lost with a missing `data.db` next to
surviving chat files (`Storage::chats_without_db`) — the one part of that loss
that is reconstructible.

**Rejected, again.** Deriving the history *on demand* inside the tool, which §2
set aside for the original feature and which the seed does not resurrect: the
stored aggregate remains the source of truth, the derivation runs once at
startup, and after the first record exists nothing scans anything.
