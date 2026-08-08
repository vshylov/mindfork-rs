# Plan: SelfModel MVP probe (trimmed-down Phase 1)

This document is an implementable plan for a minimal version of the agent's "self-model",
distilled from the idea bank [self-model.md](self-model.md). Goal — check the
**behavioral** payoff (does a local model recall facts about the user and its own goals
across chats), rather than build a full meta-cognitive machinery.

## Key departure from the source document (deliberate)

The source document modeled `SelfModel` via `ChatEffect`, as `Chat` state. In the actual
architecture, per-profile data (notes, RAG) is written by tools **directly** into SQLite
(`ctx.storage.db().note_insert(...)`; `Storage` — thread-safe `Arc` + an internal mutex).
`ChatEffect` only exists for `Chat` mutations (solely owned by the orchestrator, and it
isn't in the DB during a turn). `SelfModel` is per-profile data, like notes, therefore:

- **No new `ChatEffect` variants are needed** — mutator tools write `SelfModel` directly,
  like `note_save`.
- The invariant "sole owner of `Chat`" is untouched (`SelfModel` isn't `Chat`).

## Principles

- `SelfModel` lives in SQLite, isolated by `profile_id` (like notes/RAG), one row per
  profile.
- The orchestrator reads `SelfModel` at the start of a turn: puts a snapshot into
  `ToolContext` and **compactly** injects it into the `system` prompt (that's where the
  payoff shows up).
- Tools are **optional** (like `send_followup_message`): in `all_tool_ids`, but not in
  `default_tool_ids` — not forced on existing profiles, visible as toggles in profile
  settings.
- Minimal structure: free-form text + goals + a user model. No `strength: f32`/`severity`
  (they invite meaningless numbers).

## Step 1 — Entity `src/entities/self_model.rs`

```rust
pub struct SelfModel {
    pub profile_id: Uuid,
    pub version: u64,
    pub summary: String,             // free-form text "about yourself"
    pub goals: Vec<Goal>,
    pub user_model: UserModel,
    pub updated_at: DateTime<Utc>,
}
pub struct Goal { pub id: Uuid, pub description: String, pub status: GoalStatus }
pub enum GoalStatus { Active, Completed, Abandoned }
pub struct UserModel {            // all Vec<String>/String, no id
    pub perceived_traits: Vec<String>,
    pub current_interests: Vec<String>,
    pub relationship_dynamic: String,
}
```

Methods: `new(profile_id)` (empty), `is_empty()`,
`render_for_prompt(max_chars) -> Option<String>` (a compact block "[Your self-model]
About yourself: … / Active goals: … / About the interlocutor: …", truncated for the sake
of an 8k context). serde, `#[serde(default)]` on new fields per convention.

## Step 2 — Storage `src/shared/storage/db.rs`

In `migrate()`, add (next to `notes`/`rag_sources`, `CREATE TABLE IF NOT EXISTS` → no
migration needed):

```sql
CREATE TABLE IF NOT EXISTS self_models (
    profile_id  TEXT PRIMARY KEY,
    data        TEXT NOT NULL,    -- JSON of the whole model
    version     INTEGER NOT NULL,
    updated_at  TEXT NOT NULL
);
```

Methods: `self_model_get(profile_id) -> Result<Option<SelfModel>>`,
`self_model_upsert(&SelfModel)` (INSERT OR REPLACE, version+1). Isolation — via PK
`profile_id`. Test: `self_model_isolated_by_profile` + an upsert→get round trip.

## Step 3 — `ToolContext` (`features/tools/mod.rs`)

- Add a field `pub self_model: Option<SelfModel>` (a snapshot at the start of the turn).
- Add `self_model: None` to `testkit::ctx_with_storage`.

## Step 4 — Tools `src/features/tools/self_model.rs` (4 of them, modeled on `notes.rs`)

1. **`get_self_model`** — reads `ctx.self_model` (or from the DB), returns `render`.
   No writing.
2. **`reflect`** — returns the current model + a short nudging rubric ("what changed
   about you / about the interlocutor / about your goals?"). Plain text, no writing —
   a reflection entry point; afterward the model calls the update tools itself.
3. **`update_self_model`** — args: `summary?`, `add_goals: [string]?`,
   `complete_goals: [uuid]?`, `abandon_goals: [uuid]?`. Merges into the stored model,
   writes via `self_model_upsert`. (Goal management is folded in here to avoid
   proliferating `set_goal`/`revise_goal`/`abandon_goal`.)
4. **`update_user_model`** — args: `perceived_traits?`, `current_interests?`,
   `relationship_dynamic?`. Merge + upsert.

All read/write under `ctx.profile_id`. Errors — as text in the result, not a panic
(the `Tool` contract).

## Step 5 — Registry and gating (`features/tools/mod.rs`)

- Id constants; `standard_registry` — `reg.register(...)` for all four.
- Add the four ids to `all_tool_ids()` (**not** to `default_tool_ids`).
- `effective_tool_ids`: they pass through `_ => true` (DB-only, safe, no global switch
  needed).
- Profile toggles in `screens/settings.rs::tool_catalog` will pick them up automatically
  (it's built from `all_tool_ids`).

## Step 6 — Orchestrator (`app/orchestrator/generation.rs`, `start_generation`)

- Load the snapshot: `let self_model = self.storage.db().self_model_get(profile_id).ok().flatten();`
- Put it into `ToolContext { …, self_model: self_model.clone() }`.
- **Injection into the prompt** (after `build_request`): if the model is non-empty *and*
  the profile has `get_self_model` enabled (opt-in), append `render_for_prompt(CAP)` to
  `request.system`. A small helper; `ChatRequest.system: Option<String>`.
- `handle_done` — **left untouched** (no effects).
- The snapshot in `ToolContext` is deliberately allowed to go stale within a turn (like
  notes) — the next turn re-reads it from the DB.

## Step 7 — Tests

- entity: merging goals (add/complete/abandon), `render_for_prompt`, truncation,
  `is_empty`.
- db: isolation + round trip (Step 2).
- tools: `get` renders; `update_self_model` merges and persists;
  `update_user_model`; `reflect` returns the current state + a rubric. Via
  `testkit::ctx_with_storage`.
- mod: `all_tool_ids` contains the four, `default_tool_ids` doesn't.
- orchestrator (focused): injection into `request.system` for a non-empty model +
  the tool enabled; no injection for an empty/disabled one.

## Step 8 — Probe assessment (the whole point of doing this)

Determine **before** merging: 2–3 long multi-session dialogues, a run with the feature
off/on, compare — does the model recall facts about the user and its own goals across
chats. Record a go/no-go criterion (whether to continue to Tier 2).

## Explicitly out of MVP scope

`beliefs` with numbers, `contradictions`/detect/resolve, a narrative, versioned history,
timer-based auto-reflection, all of Phase 4, a UI viewer overlay. If the probe takes off —
Tier 2 as a separate pass.

## Scope

~1.5–2 days of code+tests (patterns already exist: the entity ≈ `note.rs`, tools ≈
`notes.rs`, DB ≈ notes' methods). Afterward — update the
[journal](../journal/self-model.md)/`architecture.md`
(a new tool group, the `ToolContext` field, a DB table).
