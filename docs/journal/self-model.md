# Journal — The self-model

The agent's model of itself and of its interlocutor: the summary, goals, traits, the observation narrative (which became `@self` notes), reflection and consolidation, and the budgets that govern what of it reaches the prompt.

**Reference documents for this area:** architecture.md §9, spec.md §17

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)).
They record what was done, why, what was measured and what was rejected — the reasoning
behind the code, not its current shape. For the current shape read the reference documents
named above; for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (24)

- Post-M9: SelfModel MVP — agent "self-model" (probe, done)
- Post-M9: SelfModel Tier 2-A — narrative insights + contradictions in prose (done)
- Post-M9: SelfModel Tier 2-B — self-model viewer screen (`F3`) (done)
- Post-M9: SelfModel Tier 3-A — narrative/injection parameters in settings (done)
- Post-M9: SelfModel Tier 3-B — auto-reflection (background) (done)
- Post-M9: SelfModel Tier 3-C — manual model editing in the UI (`F3`) (done)
- Post-M9: SelfModel — partial display of a long item on the `F3` screen (done)
- Post-M9: SelfModel — repairs from live-test findings (Opus 4.8 API) (done)
- Post-M9: SelfModel Tier 2 — revision scar + narrative consolidation (done)
- Post-M9: self-model refinements — stage 1 (atomic write, race fix) (done)
- Post-M9: self-model refinements — stage 2 (time, narrative/goal caps) (done)
- Post-M9: self-model refinements — stage 3 (reflection cadence by watermark) (done)
- Post-M9: self-model refinements — stage 4 (interlocutor model) (done)
- Post-M9: self-model refinements — stage 5 (background-task observability) (done)
- Post-M9: self-model refinements — stage 6 (loop dedup + single-source policy) (done)
- Post-M9: self-model narrative as notes — Tier 1 (done)
- Post-M9: narrative as notes — Tier 2 (structure: relevance + graph) (done)
- Post-M9: narrative as notes — Tier 2, step C (related-traits gate) (done)
- Post-M9: self-consolidation overview in reflection (Tier 3, narrative as notes) (done)
- Post-M9: self-model summary as a snapshot, not a chronicle (anti-bloat) (done)
- Post-M9: self-model consolidation — stage A1 (background "sleep" on a timer) (done)
- Post-M9: self-model consolidation — stage A2 (summary↔observation semantics) (done)
- Post-M9: self-model consolidation — stage A3-light (interest aging) (done)
- Post-M9: self-model injection — per-section budgets (done)

### Post-M9: SelfModel MVP — agent "self-model" (probe, done)
- **A trimmed-down Phase 1 from the idea bank** ([docs/self-model.md](../../docs/history/self-model.md)) per
  the plan [docs/self-model-mvp.md](../../docs/history/self-model-mvp.md): a per-profile agent "self-
  model" — free-form text about itself + goals + a mental model of the interlocutor. The probe's goal
  is to check **behavioral** value (does a local model recall facts about
  the user and its own goals across chats), rather than build a full meta-cognitive
  machinery. Deliberately **left out**: `beliefs` with numeric "strengths", contradiction
  detect/resolve, a narrative, versioned history, timer-based auto-reflection, all
  "deep self-awareness" (Phase 4 of the source document).
- **Key departure from the source document**: it modeled the self-model via
  `ChatEffect` (as `Chat` state). In the actual architecture, per-profile data
  (notes/RAG) is written by tools **directly** into SQLite (`Storage` —
  thread-safe `Arc`+mutex), and `ChatEffect` only exists for `Chat`
  mutations (solely owned by the orchestrator). So **there are no new `ChatEffect`
  variants** — self-model tools write through `ctx.storage`, like `note_save`; the invariant
  "sole owner of `Chat`" is untouched.
- **Entity** `entities/self_model.rs`: `SelfModel { profile_id, version, summary,
  goals: Vec<Goal>, user_model: UserModel }`; `Goal { id, description, status:
  Active/Completed/Abandoned }`; `UserModel { perceived_traits, current_interests,
  relationship_dynamic }`. Methods `new`/`is_empty` (only counts **active**
  goals — a model made only of finished goals isn't considered informative)/`add_goal`/
  `set_goal_status`/`render_for_prompt(max_chars)` (a compact block "[Your self-
  model] About yourself:… / Active goals:… / About the interlocutor:…", truncated by characters for
  the sake of an 8k context). serde `#[serde(default)]`.
- **Storage** `shared/storage/db.rs`: table `self_models(profile_id PK, data JSON,
  version, updated_at)` (`CREATE TABLE IF NOT EXISTS` → without migration); methods
  `self_model_get`/`self_model_upsert` (INSERT OR REPLACE, the version is managed by the store;
  `version` is stored as `i64` — rusqlite doesn't support `u64`). Isolation by PK `profile_id`.
- **Tools** `features/tools/self_model.rs` (4, modeled on `notes.rs`):
  `get_self_model` (read), `reflect` (returns the current model + a rubric for
  self-reflection, without writing — an "entry point"), `update_self_model` (summary + goals:
  add/complete/abandon by id; goal management folded into one tool),
  `update_user_model` (lists replace the previous ones). Mutators read the fresh state from the DB (not
  the `ctx.self_model` snapshot) to see in-turn edits; errors are text, not a
  panic.
- **Optional, off by default** (like the control tools): in `all_tool_ids`, but
  **not** in `default_tool_ids` (`reconcile_tools` doesn't enable them for existing/new
  profiles; profile toggles are built from `all_tool_ids`). DB-only → in
  `effective_tool_ids` they pass through `_ => true` with no global gates.
- **Integration** `app/orchestrator/generation.rs::start_generation`: a snapshot
  `self_model_get(profile_id)` is put into `ToolContext.self_model` and **compactly**
  injected into `request.system` by a pure function `inject_self_model(system, model,
  enabled)` — gated on the profile having enabled `get_self_model` (opt-in). `ToolContext.self_model`
  is currently `#[allow(dead_code)]` (tools read from the DB; the snapshot is kept for the
  round-snapshot's completeness, like `chat_id`). `handle_done` is untouched (no effects).
- **Tests**: entity (goal merging, render/truncation, `is_empty` by active
  goals); db (round trip + per-profile isolation + version growth); tools (get/update/reflect,
  persistence, no-op with no arguments); mod (in the catalog, not in the defaults; passes through
  `effective_tool_ids`); orchestrator (pure `inject_self_model`: gate/empty/
  non-empty). **593 tests green**, clippy/fmt clean.
- **Probe verdict (done)**: a live `self_model_e2e_live` on Gemma 4 12B Q8_0 —
  in session 1 the model called `update_user_model`+`update_self_model` (data hit the DB), in
  session 2 (a new chat of the same profile) it precisely recalled the user/goals via
  injection into `system` + `get_self_model`. Go/no-go criterion — **go**, moved on to Tier 2.

### Post-M9: SelfModel Tier 2-A — narrative insights + contradictions in prose (done)
- **A "self over time" narrative** — the field `SelfModel.narrative: Vec<NarrativeSegment>`
  (`{id, text, created_at}`, `#[serde(default)]`): short insights/observations,
  append-only with a ceiling `MAX_NARRATIVE=50` (keep the freshest, `add_insight`
  trims old ones). **Contradictions are woven in here as prose** — without a separate
  type/`severity`/detect-resolve machinery (a deliberate Tier 2 decision).
- **`add_insight` tool** (`features/tools/self_model.rs`): records an observation
  into the narrative (including noticed contradictions as prose). Optional (in `all_tool_ids`,
  not in `default_tool_ids`), writes directly through `ctx.storage` (like the other
  self-model mutators, without `ChatEffect`). The `reflect` rubric got a question about
  contradictions and a mention of `add_insight`.
- **Render/injection**: `render_for_prompt` adds a "Recent observations:" block from
  the freshest `NARRATIVE_IN_PROMPT=3` insights (newest first; conserving context);
  `is_empty` accounts for the narrative (a model built only of insights is already informative).
- **Tests**: entity (append/ceiling/rendering the freshest); tool (`add_insight` persists +
  empty text → an error); catalog (`add_insight` in `all_tool_ids`, not in the defaults).
  **595 unit tests green**, clippy/fmt clean. Live smoke `self_model_insight_e2e_live`
  (`#[ignore]`) on Gemma 4 12B: the model calls `add_insight`, and prose about a
  contradiction ("volatility in response-length requirements") lands in the DB narrative.

### Post-M9: SelfModel Tier 2-B — self-model viewer screen (`F3`) (done)
- **A read-only viewer screen** of the active profile's self-model
  (`screens/self_model.rs`, `SelfModelScreen`): description, goals (with status
  ●/✓/✗), the interlocutor model, the narrative (newest on top). Opened from chat via
  **`F3`**, closed via `Esc`, `Ctrl+C` — quit; scrolling `↑↓`/`PgUp`/`PgDn`/`Home`.
  A fullscreen rounded panel + a hotkey line (modeled on `chat_list`).
- **Data is owned by the orchestrator** (`Storage`), so the screen **doesn't** open right
  away: `F3` → `ChatIntent::OpenSelfModel` → `runtime` sends `AppCommand::RequestSelfModel` →
  `handle_request_self_model` loads `self_model_get(active profile)` and emits
  `AppEvent::SelfModelView(Box<Option<SelfModel>>)` → `runtime::apply_event` creates
  `ActiveScreen::SelfModel`. A new `ActiveScreen` variant (the 4th screen); FSD is honored
  (`screens` doesn't know about `app`). The screen's palette is updated from `Settings` (theme).
- **Editing the model — for now only through the model itself** (self-model tools); manual
  editing via the UI — Tier 3 groundwork (data in SQLite, not JSON).
- **Tests**: screen (Esc/Ctrl+C intents layout-independent, scroll clamping,
  rendering an empty/populated model without a panic); runtime (`OpenSelfModel` sends
  `RequestSelfModel` and does NOT open the screen right away; `SelfModelView` opens the screen).
  **601 unit tests green**, clippy/fmt clean. Not tested against a live model
  (an interactive TUI — needs a real terminal). Added to the help overlay (`F1`/`?`).

### Post-M9: SelfModel Tier 3-A — narrative/injection parameters in settings (done)
- **Narrative size and prompt-injection volume moved from constants into config**
  (`config.self_model: SelfModelSettings` — `max_narrative`/`narrative_in_prompt`/
  `prompt_cap`; `#[serde(default)]` → old `settings.json` without migration; defaults =
  the previous 50/3/1200). The formerly hardcoded `MAX_NARRATIVE`/`NARRATIVE_IN_PROMPT`/
  `DEFAULT_PROMPT_CAP` are removed.
- **`SelfModelParams` type** (`entities/self_model.rs`, an analog of RAG's `ChunkParams`):
  `from_settings` sanitizes (max≥1; the prompt gets no more than what's stored; cap≥100).
  Entity methods are parameterized: `add_insight(text, max_narrative)`,
  `render_for_prompt(prompt_cap, narrative_in_prompt)`. Threaded into `ToolContext.
  self_model_params` (built from `config.self_model` in `start_generation`);
  the tools (`add_insight`/`render_or_empty`) and `inject_self_model` use it.
- **UI**: three numeric fields in the "Tools" section of the settings screen (next to the RAG
  chunking ones) with description tooltips (`field_description`); `FieldId::Sm*`.
- **Tests**: config (defaults in `partial_json_fills_defaults`); entity (custom
  parameters cap storage/injection; sanitization of inconsistent settings).
  **603 unit tests green**, clippy/fmt clean.

### Post-M9: SelfModel Tier 3-B — auto-reflection (background) (done)
- **Auto-reflection** (`app/orchestrator/reflection.rs`): every N assistant replies
  in a chat, a background task asks the model to review the recent conversation and
  **itself** update the "self-model". Enabled via `config.self_model.auto_reflect_every` (0 —
  off, by default). This was a deferred item from the original plan ("reflection after
  every N messages").
- **A mini agentic loop, not a single-turn request** (unlike auto-naming): reflection
  is given self-model tools (`get/update_self_model`/`update_user_model`/
  `add_insight`, intersected with the profile's set), and the loop **executes** their calls
  (the tools write directly into `Storage`). Up to `REFLECT_MAX_ROUNDS=6` rounds,
  a 120s timeout. The chat is **not mutated**, nothing is streamed to the UI — reflection
  is silent. The system message asks it to change only what actually changed and not write a reply
  to the user, only call tools.
- **Trigger** in `handle_done` (after a successful reply): `maybe_auto_reflect`
  counts replies (`reflect_counts: HashMap<chat_id,u32>`), resets and
  fires at the threshold. Gates: the feature is enabled, the profile enabled `get_self_model` (as with
  the injection), reflection isn't already running (`reflect_cancel`, one at a time), the server is `Ready`,
  there's enough of a conversation (a digest exists). A pure `due(count, every)` — testable.
- **Plumbing**: fields `reflect_cancel`/`reflect_counts`/`reflect_done_tx` on
  `Orchestrator`; an internal channel `reflect_done` (background → the loop clears the flag
  `handle_reflect_done`); cancellation on `Quit`. The digest — `rename_chat::
  build_conversation_digest` (as for auto-naming).
- **UI**: a "Self-model: auto-reflection (every N)" field in the "Tools" section
  of settings (`FieldId::SmAutoReflect`) with a tooltip.
- **Tests**: `due` (threshold/disabled); **604 unit tests green**, clippy/fmt clean.
  Live smoke `auto_reflect_e2e_live` (`#[ignore]`, polls the DB — reflection without
  a UI event) on Gemma 4 12B: with `auto_reflect_every=1`, after the first reply the model
  **on its own** (without being explicitly asked) called `update_user_model` → traits/
  interests of the interlocutor showed up in the DB.

### Post-M9: SelfModel Tier 3-C — manual model editing in the UI (`F3`) (done)
- **The `F3` screen became editable** (was read-only): editing the self-description, goals
  (add/rename/cycle status `Space`/delete `Del`), the interlocutor model
  (traits/interests as a comma-separated list, relationship dynamic), deleting
  narrative insights (`Del`), full clearing (`Ctrl+K` twice — with confirmation).
  Navigation `↑↓`/`Home`/`End`, `Enter` — edit (a text editor popup, self-
  description multi-line), `Esc` — close, `Ctrl+C` — quit.
- **Edit type** `SelfModelEdit` (`entities/self_model.rs`) + a pure `SelfModel::
  apply_edit(edit) -> bool` (whether it changed) and `cycle_goal_status`. UI↔
  orchestrator contract; the trait/interest lists are **replaced in full**.
- **Flow** (an edit doesn't mutate `Chat`, it goes through the `Storage` owner):
  `SelfModelIntent::Edit` → `AppCommand::UpdateSelfModel` → `handle_update_self_model`
  (load/create the profile's model → `apply_edit` → on change `self_model_upsert`
  → **re-emit** `AppEvent::SelfModelView`). `runtime::apply_event` updates the **already
  open** screen in place (`set_model`, keeping the selection), rather than recreating it.
  Clipboard paste is routed into the active field editor.
- **The editor** reuses `widgets::input_box::InputBox` (single-line for
  fields/goals/lists, multi-line for the self-description) — the same pattern as in
  the settings screen and chat rename.
- **Tests**: entity (`apply_edit` covers every operation + no-op on a repeat/nonexistent id);
  screen (Enter→edit summary, `Space`/`Del` on a goal, adding a goal + empty no-op,
  `Ctrl+K` confirm/cancel, list parsing, rendering an empty/full model without a panic);
  orchestrator (`update_self_model_persists_and_reemits` — an edit is saved and
  re-emitted). **610 unit tests green**, clippy/fmt clean. Manual editing
  via the UI is no longer Tier 3 groundwork — done.

### Post-M9: SelfModel — partial display of a long item on the `F3` screen (done)
- **Symptom**: on the "Self-model" screen (`F3`), a list item with a multi-line value
  (a long description/insight) that didn't fully fit in the remaining height was **not
  shown at all** — an empty spot in its place, creating the illusion of the list ending.
- **Cause**: the list was rendered with the `List` widget, which **entirely skips**
  a multi-line item that doesn't fit height-wise in the remaining area (its
  `get_items_bounds` stops at `height + item.height() > max_height`). The chat list
  window doesn't suffer from this — its items are single-line.
- **Fix** (`screens/self_model.rs`): the `List` widget was replaced with a **manual row-by-row
  rendering** of visual rows. Each logical line is expanded into visual
  rows (`wrap::wrap_line`), rendered one `Paragraph` per row of the area;
  a trailing item that doesn't fit height-wise gets **clipped at the bottom edge** (its
  top is visible), rather than skipped. A persistent `scroll` field was added (the first visible row)
  + a pure `adjust_scroll(scroll, sel_start, sel_height, view_h)`: keeps the selected
  row visible, and if it itself is taller than the window, pins to its **top**. The selected
  row's highlight stays the same — a backdrop across the whole row width (the base `Paragraph` colors the
  whole area) + a `▌` marker on each of its visual rows. Navigation/selection remain by
  logical lines (key behavior unchanged).
- **Tests**: `adjust_scroll_keeps_selection_visible_and_pins_top_of_tall_item`
  (the scroll logic) and `render_long_trailing_item_in_short_area_does_not_panic`
  (a long insight in a tight window). **625 tests green**, clippy/fmt clean.

### Post-M9: SelfModel — repairs from live-test findings (Opus 4.8 API) (done)
- **Following manual testing** of the "self-model" on Opus 4.8 (Anthropic API),
  identified and fixed issues that left effectively only the narrative
  (`add_insight`) working. No new tools, no DB schema change (the model is a JSON
  blob `self_models.data`, the new config flag is via `#[serde(default)]`) —
  **no migration needed**. Fixes touch rendering, tool arguments, descriptions, and
  one toggle.
- **Ellipsis in `get_self_model` (truncation).** Cause: read tools called the same
  **compact truncated** render that goes into the system prompt
  (`render_for_prompt(prompt_cap=1200, narrative_in_prompt=3)`) — the model saw "…"
  and complained. Fix: new `SelfModel::render_full()` (no truncation, full
  narrative, goals with id) — for `get_self_model`/`reflect`/echo after edits;
  `render_for_prompt` remains only for passive injection.
- **Goals never closed (write-once).** Mechanical blocker: `render_for_prompt`
  printed a goal as `- {description}` **without an id**, and `get_self_model`
  called exactly that → the model **never saw a goal's id**, while
  `complete_goals`/`abandon_goals` require one. Fix: `render_full` shows goals as
  `- #a1b2c3 (active) …` (short id = first 6 hex chars of the UUID) + recently
  closed ones (lifecycle visible); the resolver `SelfModel::match_goal` accepts a
  short `#id` **or** a full UUID (by unambiguous prefix, case-insensitive;
  ambiguity/miss — a clear report, not a panic). The `reflect`/auto-reflection
  rubrics were rewritten around "manage goals by #id".
- **`update_user_model` overwrote (the "mood swing" bug).** Cause: lists were
  replaced wholesale (`perceived_traits = […]`) — good mood → "kindest", bad mood →
  "merciless", each edit destroyed the accumulated data. Fix (notes philosophy
  "integration over accumulation"): **merge semantics** — `add_traits`/
  `remove_traits`, `add_interests`/`remove_interests` (case/Unicode-insensitive
  dedup) instead of replacement; the description was reframed as a *stable,
  integrated* model of the interlocutor (not a mood snapshot), transient
  observations → `add_insight`. The manual edit via `F3` (`SelfModelEdit::SetTraits`)
  remains a replacement — there a human is in the loop.
- **`update_self_model.summary`** — remains a replacement (natural for a coherent
  self-description), but the description/rubric now ask to **integrate** the
  previous with the new rather than rewriting from scratch; the current
  description is always visible (injection + `render_full`).
- **Predictability of tools independent of persona.** (1) A persona-neutral
  **"self-model maintenance protocol"** (`generation.rs::SELF_MODEL_MAINTENANCE_PROTOCOL`)
  is mixed into the system prompt on top of the profile persona: when to record
  changes, "transient — into observations", **"accuracy over agreeableness"** (a
  direct counter-measure against flattery from a kind persona). Toggle
  `config.self_model.maintenance_protocol` (`#[serde(default)]`, **on by default**),
  gated on the profile having enabled `get_self_model`; mixed in even for an empty
  model (bootstrapping the first record). (2) Background **auto-reflection**
  (`reflection.rs`, `auto_reflect_every`) remains **opt-in** (0 by default —
  background Opus API calls cost tokens), but its system message was rewritten
  around the new semantics/goals-by-id — enabling it gives you a deterministic
  writing path independent of the model's spontaneity.
- **UI**: "Self-model: maintenance protocol" toggle in the settings screen's
  "Tools" section (`FieldId::SmProtocol`, with a hint).
- **Tests**: entity (`render_full` shows goal ids and is not truncated;
  `match_goal` prefix/full/ambiguous; merge add/remove on user_model); tools
  (merge doesn't overwrite; closing a goal by short `#id`; goal miss — a report);
  orchestrator (injection: protocol on with an empty model, both with a non-empty
  one, off-behavior). **677 tests green** (+6), clippy/fmt clean. Live evaluation on
  Opus 4.8 — the next manual step.

### Post-M9: SelfModel Tier 2 — revision scar + narrative consolidation (done)
- **From the model's (Opus 4.8) feedback on the previous fix**: switching from
  "overwrite" to "accumulate" (add_/remove_) cured the *overwriting*, but pure
  accumulation has the mirror disease — **bloat and drift** (traits growing to
  forty, duplicates/staleness drowning the signal, and `remove_traits` erasing
  without a trace — "the biography doesn't remember it changed"). The identity
  fork of the feature was resolved: **the self-model is the current working
  snapshot of conclusions, while the scar-biography lives in the narrative** (we
  don't structure traits or clone the notes graph — the narrative already plays
  the role of "self-directed notes"). The DB schema was untouched (JSON blob +
  new entity methods), no migration needed.
- **Trait-revision scar (problem A)**: `update_user_model` gained an optional
  `note` — what changed and why; when present it goes into the narrative
  (`add_insight`), so a change of opinion about the interlocutor leaves a
  **trace** rather than vanishing without one. If `remove_traits`/
  `remove_interests` are non-empty and `note` wasn't passed — the result
  **reminds** the model to leave an explanation (the same warning pattern as
  `note_revise`). Traits remain a flat `Vec<String>` (no migration).
- **Consolidation against bloat (problem B)**: new tool
  `consolidate_narrative(remove[], add?)` — removes observations by `#id`
  (resolved the same way as goals: short hex prefix or full UUID,
  ambiguity/miss → a report) and optionally adds one **summarizing** entry in
  their place. This is integration (the durable is raised into
  summary/traits, the raw is folded/cleared), not silent loss from a FIFO cap —
  a mirror of `note_merge`/`consolidate_notes` in the self-model idiom,
  **without a graph**. Insights now show up in `render_full` with `#id`; the
  entity gained `match_insight`/`remove_insights` (a shared `resolve_handle`
  resolver with `match_goal`).
- **Optional, DB-only**: `consolidate_narrative` is in `all_tool_ids` (not the
  defaults), passes through `effective_tool_ids` via `_ => true`. The `reflect`
  rubric and the auto-reflection system message (`REFLECT_TOOL_IDS`) gained a
  consolidation item ("has the narrative bloated — raise the durable into
  summary/traits, clear out the raw").
- **Groundwork (not done)**: background auto-consolidation of the self-model on a
  timer (like `notes.auto_consolidate_every`) — consolidation is currently manual/
  via auto-reflection; and **unifying the memory organs** (narrative ≈ a second
  instance of notes) — still a large separate track (see notes-connectivity
  Tier 3).
- **Tests**: entity (`match_insight`/`remove_insights`; insights with `#id` in
  `render_full`); tools (trait revision with `note` → a scar in the narrative;
  removal without `note` → a reminder; `consolidate_narrative` clears duplicates +
  summarizes + reports a miss). **681 tests green** (+4), clippy/fmt clean. Live
  check on Opus 4.8 — a manual step.

### Post-M9: self-model refinements — stage 1 (atomic write, race fix) (done)
- Refinement plan — [docs/refinements.md](../../docs/history/refinements.md) (6 stages +
  the "narrative as notes" track). Branch `feat/self-model-refinements`.
- **Defect**: every self-model writer (turn tools, auto-reflection, manual `F3`
  edits) did read-modify-write as **three** calls (`self_model_get` → edit →
  `self_model_upsert`). `Db`'s mutex serializes individual calls but not the
  pair: auto-reflection (a background task running concurrently with the user)
  would read the model → the user would save an `F3` edit → reflection would
  write its own version on top, losing the edit.
- **Fix** (`shared/storage/db.rs`): `Db::self_model_update(profile_id, |m| -> bool)`
  — SELECT + `mutate` + upsert under **one** mutex acquisition. The public
  `self_model_get`/`self_model_upsert` delegate to private `*_conn` helpers
  (`std::sync::Mutex` is non-reentrant → the `mutate` closure **cannot** call
  `Db` methods — it can only mutate the `SelfModel` value; documented in a doc
  comment as a deadlock warning). `self_model_upsert` was kept (a symmetric
  primitive + tests, `#[allow(dead_code)]`).
- **Writers** converted: 4 tools (`add_insight`/`update_self_model`/
  `update_user_model`/`consolidate_narrative`; side data — unresolved handles,
  deletion flags, counters — are collected via `&mut` capture in the closure) and
  the F3 handler (`orchestrator/mod.rs`). Readers (`get_self_model`/`reflect`) use
  `self_model_get`.
- **Tests**: `self_model_update_is_atomic_under_concurrency` (2 threads × 50
  writes → exactly 100 insights, version=100 — under non-atomicity some would be
  lost), no-op doesn't write. **685 tests green** (+4), clippy/fmt clean.

### Post-M9: self-model refinements — stage 2 (time, narrative/goal caps) (done)
- Stage 2 of the [refinements.md](../../docs/history/refinements.md) plan: "me over
  time" gains time, and "silent loss" (narrative FIFO, unbounded growth of
  closed goals) gains visibility and integration.
- **Age labels** (`entities/self_model.rs`): a pure `age_label(at, now)` — day
  granularity (today/yesterday/N days/weeks/months/years). **Day granularity
  chosen deliberately**: the text is stable within a day, so the self-model
  injection into the system prompt doesn't change turn to turn (the local
  model's prefix cache suffers no more than once a day beyond actual edits).
  `render_full(now)` and `render_for_prompt(cap, n, now)` show the age of goals
  (closed ones — from the new `Goal.closed_at` field, `#[serde(default)]` → no
  migration) and observations. The `F3` screen adds a date to goals (local time
  zone, like insights).
- **Eviction made visible**: `add_insight(text, max) -> Vec<NarrativeSegment>`
  returns what was evicted past the cap; the `add_insight` tool reports
  "narrative N/M" and **what left** (a last chance to raise the durable). A new
  `narrative_fill_hint(max)` (≥80% full) makes the static maintenance protocol
  **data-aware** — a "time for consolidate_narrative" note is mixed into the
  system prompt (`inject_self_model`) and into the `reflect` rubric.
- **Closed-goal cap**: `fold_closed_goals(keep, max_narrative)` folds the oldest
  closed goals beyond `keep` into a narrative scar "[goal archive] …" and removes
  them from the structure (the same "integrate, don't lose" philosophy). Called
  in `update_self_model` and the F3 handler. New config
  `SelfModelSettings.max_closed_goals` (default **10**, `#[serde(default)]`) →
  `SelfModelParams` (sanitized to ≥1).
- **Prefix-cache trade-off locked in** (user decision 2026-07-03): the self-model
  injection stays in `system`; losing prefix cache on every update is the
  accepted price for the capability. Moving the block to the end of history is
  **not** being done. See architecture.md §9.
- **Tests**: entity (`age_label` buckets + a future timestamp; `closed_at` gets
  set/cleared; `add_insight` returns what was evicted; `narrative_fill_hint`
  80% threshold; `fold_closed_goals` archives the oldest beyond keep); tools
  (`add_insight` reports eviction; `update_self_model` folds closed goals);
  config (`max_closed_goals` default). **692 tests green** (+7), clippy/fmt
  clean.

### Post-M9: self-model refinements — stage 3 (reflection cadence by watermark) (done)
- Stage 3 of the [refinements.md](../../docs/history/refinements.md) plan:
  auto-reflection stops re-reading the same early material and no longer loses
  the cycle on a skip.
- **Three bugs**: (1) the reflection digest was built from the **entire** chat
  history → every cycle re-read what had already been reflected on → duplicate
  insights, later cleaned up by consolidation; (2) the cadence counter was reset
  **before** the "already running"/"server not ready" gates — a skipped run lost
  the whole cycle (with `every=10` the next attempt was 10 responses away);
  (3) in-memory counters (`reflect_counts`/`consolidate_counts`) were lost on
  restart, even though the data is per-profile.
- **Watermark in `Chat`** (`entities/chat.rs`): `reflected_upto: Option<usize>`
  (a watershed index — how many leading messages have been covered) +
  `reflected_at` (`#[serde(default, skip_serializing_if=Option::is_none)]` → old
  chat files read without migration, empty ones don't clutter the JSON; lives
  with the chat → survives restarts).
- **Window-based cadence** (`orchestrator/reflection.rs`): a pure
  `reflect_window(messages, reflected_upto) -> (wm, count)` — clamps the
  watermark to the length (resilient to `Ctrl+R`/`Ctrl+E` truncation) + counts
  non-empty assistant responses in the window `messages[wm..]`. `due(count,
  every)` as before. The digest is `build_conversation_digest(&messages[wm..])`
  (the signature already took a slice). The `reflect_counts` field was
  **removed** from the orchestrator (cadence is now computed from data).
- **Watermark shifts only on spawn**: after **all** gates (profile enabled the
  self-model, the window accumulated `every`, the digest is non-empty,
  reflection isn't already running, server is `Ready`) — `reflected_upto =
  messages.len()`, `reflected_at = now`, `mark_dirty` (debounced save).
  `modified_at` is untouched (reflection shouldn't bump the chat up in the
  list). A skip on any gate leaves the watermark alone → the cycle isn't lost,
  self-heals.
- **Notes consolidation** (`consolidation.rs`): still on an in-memory counter
  (its digest is a notes overview, not the conversation), but **the counter
  reset moved to after all gates** — closing the same cycle-loss bug.
- **Tests**: pure (`reflect_window` counts from the watermark + clamp after
  truncation); serde (an old chat JSON without watermark fields → defaults,
  empty ones aren't serialized); integration (`maybe_auto_reflect` advances the
  watermark when the engine is ready and does **not** advance it when the
  server isn't ready). **697 tests green** (+5), clippy/fmt clean.

### Post-M9: self-model refinements — stage 4 (interlocutor model) (done)
- Stage 4 of the [refinements.md](../../docs/history/refinements.md) plan: wiring
  already-existing interlocutor data into mechanisms that weren't using it.
- **`user_model` → impersonation** (4a): impersonation (`Ctrl+U`) writes a
  reply **on behalf of** the interlocutor, yet `user_model` — literally a model
  of that interlocutor — wasn't seen by `build_impersonation_request`. New
  `UserModel::render_for_impersonation(cap)` is mixed into the impersonation
  system prompt (`build_impersonation_request(..., user_hint)`); gated on the
  same opt-in as passive injection (profile enabled `get_self_model`).
- **Behavioral signals in the reflection digest** (4b): `Ctrl+R`
  (regeneration = "the answer wasn't good enough"), `Ctrl+E` (deleting an
  exchange), and rewrite rounds are already archived into `Chat.deleted` — the
  strongest implicit evidence about the interlocutor, which reflection wasn't
  seeing. New `DeletedCause` (`DeleteExchange`/`Regenerate`/`Rewrite`) + field
  `DeletedExchange.cause` (`#[serde(default, skip_serializing_if)]` → no
  migration; `record_deleted` gained a parameter, 3 call sites updated). A pure
  `behavior_markers(chat, since)` (`since` = the former `reflected_at` from
  stage 3) counts deletions within the window and appends a "Behavioral signals
  about the interlocutor:…" block to the digest (regeneration/deletion —
  about the interlocutor, rewrite — about the agent's own behavior; entries
  without `cause` aren't counted). `REFLECT_SYSTEM_MESSAGE` clarifies that the
  markers are evidence (an observation, not a judgment).
- **Scar on replacing relationship dynamic** (4c): `relationship_dynamic` — the
  most significant field of the interlocutor model — was replaced wholesale
  without a trace (the `note` reminder only fired on `remove_traits`/
  `remove_interests`). Now replacing a **non-empty** dynamic without `note`
  also produces a reminder scar (`replaced_dynamic`); initial population — no
  reminder. The tool description was updated.
- **Tests**: entity (`render_for_impersonation` Some/None + all fields);
  reflection (`behavior_markers` — count by cause, `since` filter, old entries
  without `cause` aren't counted, own behavior gets a separate phrase);
  impersonation (`build_impersonation_request` mixes in `user_hint`); tools
  (replacing a non-empty dynamic without `note` → reminder, initial population
  — none, with `note` → scar in the narrative). **701 tests green** (+4),
  clippy/fmt clean.

### Post-M9: self-model refinements — stage 5 (background-task observability) (done)
- Stage 5 of the [refinements.md](../../docs/history/refinements.md) plan:
  reflection/consolidation are silent background tasks whose failures are easy
  to miss; an open `F3` after a background edit showed a stale snapshot; a dead
  field in the contract.
- **Task outcome + failure streak** (5.1): the internal done channels for
  reflection/consolidation now carry `Result<(), String>` instead of `()`;
  errors are logged with `warn` (including `profile_id`). The orchestrator
  counts consecutive failures (`reflect_failures`/`consolidate_failures`); at
  the threshold `BACKGROUND_FAILURE_ALERT=3` it emits `AppEvent::Error`
  **once**, then stays quiet until the first success (reset) — observability
  without spam.
- **Status-bar indicator** (5.2): new event `AppEvent::BackgroundTask{kind:
  BackgroundKind, active}` (emitted on spawn/finish). `ChatScreen` holds flags
  (`set_reflecting`/`set_consolidating` — the `BackgroundKind` mapping is done
  by runtime, so `screens` doesn't depend on the `app` contract, FSD);
  `status_bar` draws a quiet muted chip `✻ reflection`/`✻ notes sleep` (a
  1-column-wide glyph — the hotkey grid layout doesn't "shift"). New parameter
  `background: Option<&str>` on `render`/`height`/`lines`.
- **Freshness of an open `F3`** (5.3): new event `AppEvent::SelfModelChanged`
  (no snapshot). Emitted after **successful** reflection (`handle_reflect_done(Ok)`)
  and in `handle_done` if the turn included SelfModel-tool calls (detected via
  a new `self_model::is_self_model_tool` + `ALL_IDS`). `runtime::apply_event`:
  if the `F3` screen is open → sends `AppCommand::RequestSelfModel` (re-fetch a
  fresh snapshot); if closed — ignored (doesn't open the screen, unlike
  `SelfModelView`). Consolidation doesn't send `SelfModelChanged` (it changes
  notes, not the self-model).
- **Contract cleanup** (5.4): removed the dead field `ToolContext.self_model`
  (`#[allow(dead_code)]`; tools read from the DB, reflection was passing
  `None`) — the contract no longer makes a false promise that "a snapshot is
  available". Removed from 8 construction sites (generation, reflection,
  consolidation, testkit + 4 tool test contexts). The snapshot for prompt
  injection now lives as a local variable in `start_generation`, not a context
  field.
- **Tests**: runtime (`SelfModelChanged` with `F3` open sends
  `RequestSelfModel`, with it closed — doesn't); orchestrator (3 consecutive
  failures → one error, success resets + sends `SelfModelChanged`;
  `handle_done` with a self-model call → `SelfModelChanged`); tools
  (`is_self_model_tool` recognizes the group); status_bar tests updated for the
  new parameter. **705 tests green** (+4), clippy/fmt clean.

### Post-M9: self-model refinements — stage 6 (loop dedup + single-source policy) (done)
- Final stage of the [refinements.md](../../docs/history/refinements.md) plan: a
  mechanical refactor, no behavior change.
- **Shared silent runner** (6.1, `app/orchestrator/tool_loop.rs`): the body of
  the mini agentic loop (stream → call accumulator → execute allowed tools →
  round, tolerant of Thoughts/ThoughtsSignature/Usage) was duplicated
  **verbatim** in `reflection.rs` and `consolidation.rs` (differing only in
  limits and the log label). Now there's one — `spawn_silent_loop(SilentLoop {
  backend, registry, ctx, request, allowed, cancel, max_rounds, timeout, label,
  profile_id, done_tx })` + a private `run_rounds`. Shared spawn tail (timeout
  + `warn` log + sending the outcome to the done channel). The `due` cadence
  predicate also moved here (was in both modules). Both sites got thinner: they
  build a `SilentLoop` and call the runner; their `ReflectSpawn`/
  `ConsolidateSpawn`/`spawn_reflection`/`spawn_consolidation`/`due` were
  removed. **The main generation loop was deliberately not merged in** —
  streaming to the UI, control-flow tools, Anthropic thinking signatures,
  usage, effects; its complexity doesn't pay for a shared sink (noted in the
  module doc).
- **Single-source maintenance policy** (6.2): the rule wording was duplicated in
  `SELF_MODEL_MAINTENANCE_PROTOCOL` (`generation.rs`) and
  `REFLECT_SYSTEM_MESSAGE` (`reflection.rs`) and had already drifted slightly.
  Introduced a canonical constant `self_model::POLICY_CORE` (integrate
  summary; manage goals by #id; merge user_model; transient → add_insight;
  accuracy over agreeableness; consolidate the narrative). Both texts are
  assembled from it: `self_model::maintenance_protocol()` = `POLICY_CORE`
  framed as "you manage this yourself"; `reflection::reflect_system_message()`
  = preamble + `POLICY_CORE` + an explanation of behavioral signals (built at
  runtime — `format!` doesn't work for `const`). The interactive `reflect`
  rubric was **deliberately** left as-is — a different genre (questions, not
  an imperative), covering the same topics.
- **Field-grouping `BackgroundLoop`** (plan 6.1) — **not done**: reflection and
  consolidation differ (consolidation has a `consolidate_counts` counter,
  reflection has the watermark), the payoff is cosmetic, and the risk of
  smearing the invariant across call sites doesn't pay off. The `*_cancel`/
  `*_failures`/`*_done_tx` fields were left on `Orchestrator`.
- **Tests**: `due` — one set (in `tool_loop`); `maintenance_protocol_wraps_policy_core`
  and `reflect_system_message_composes_from_policy_core` (composition from
  `POLICY_CORE` + its own framing). Loop behavior is checked by the prior
  integration tests (`auto_reflect_advances_watermark_on_spawn` and others —
  via the shared runner). **706 tests green**, clippy/fmt clean. **Self-model
  refinements (stages 1–6, refinements.md) — complete.**

### Post-M9: self-model narrative as notes — Tier 1 (done)
- **Unifying the memory organs** per the [docs/narrative-as-notes.md](../../docs/history/narrative-as-notes.md)
  plan: the SelfModel narrative (`Vec<NarrativeSegment>` in the JSON blob, FIFO
  cap, no semantics/dedup/graph) moves into **regular notes** with the reserved
  tag **`@self`**, getting embeddings, semantic search, duplicate gates,
  scarred replacement, and auto-"sleep" for free. This closes a long-standing
  item of groundwork, "linking the memory organs" (notes-connectivity "out of
  scope", architecture §9.9). A probe, as with SelfModel/notes: a minimal
  implementation for a go/no-go on a live model.
- **Forks confirmed by the user**: the `@self` tag (a leading `@` doesn't occur
  in natural tags; a collision is rare and harmless); self-notes are **hidden**
  from the user-facing `note_recall` (memory about oneself ≠ memory about the
  interlocutor — mixing the output is risky; full mixing with an `[about self]`
  marker is deferred to Tier 2).
- **Unification at the storage level, not at retrieval** (`features/tools/notes.rs`):
  self-notes share tables/embeddings/graph/consolidation with regular ones but
  are excluded from user-facing `note_recall` by a tag filter (`is_self_note`)
  (the substring path `list_user_notes` — reads without a limit, drops self
  notes, then truncates; the semantic `semantic_recall` — filter + extra
  candidate margin; spreading activation also skips self notes), from the
  `note_save` gate, and from the consolidation overview
  (`build_consolidation_overview` + the ≥2-notes gate for auto-"sleep" only
  count user notes). DB methods were untouched.
- **Recording observations → @self notes**: `add_insight` = `create_note(@self)`
  + a **gate** (the core of the hypothesis: `self_note_similar` — semantically
  close observations with a hint to rewrite via `note_revise`/`note_supersede`
  instead of a near-duplicate); the trait-revision scar
  `update_user_model.note` → an @self note; `fold_closed_goals` now **returns**
  scars (`Vec<String>`), and `update_self_model`/the F3 handler write them as
  @self notes.
- **Reading observations → from notes by recency**: `render_for_prompt`/
  `render_full` gained a `recent: &[NarrativeSegment]` parameter (prepared by
  the caller — `self_notes_recent`; the main ripple — `render_*` stopped being
  pure with respect to the narrative — a deliberate cost). Observations in
  `render_full` — with the **full** id (they're rewritten by
  note_revise/supersede), goals — the old `#id`. `inject_self_model` (the
  orchestrator) and `get_self_model`/`reflect` (tools) read fresh self-notes;
  `is_empty` no longer counts the narrative.
- **Consolidating observations**: `consolidate_narrative` was **removed**
  (superseded by the stronger note tools: scarred replacement); in
  `REFLECT_TOOL_IDS` it's replaced by `note_revise`/`note_supersede`/
  `note_merge` (`note_recall` isn't given — it hides self-notes; the model
  takes full ids from `get_self_model`). `note_supersede`/`note_merge` now
  **inherit tags** from the source(s) (including `@self` — a self-note doesn't
  "fall out" into user-facing output on replacement/merge; `Db::note_get`).
  `POLICY_CORE` and the `reflect` rubric were rewritten around the notes idiom;
  the entity methods `add_insight`/`remove_insights`/`narrative_fill_hint`/
  `match_insight` were removed (the `narrative` field is kept for backfill and
  for reconstructing the `F3` snapshot).
- **Backfill** (`migrate_self_narrative`): a one-time idempotent migration of
  the blob narrative → @self notes (an **atomic drain** of the narrative under
  the mutex → no duplicates even on repeat; `created_at` is preserved; the
  vector is embedded lazily). Called best-effort in `start_generation` before
  reading observations (under the same opt-in gate, `get_self_model`).
- **F3**: the `SelfModelView` snapshot **reconstructs** `narrative` from
  self-notes (for display only — the snapshot isn't persisted, writing goes
  against the real model, which is empty on `narrative`); the screen itself
  didn't change. `SelfModelEdit::DeleteInsight` → deletes a self-note
  (`Db::note_delete` became profile-scoped + removes its vector); `Clear` →
  wipes @self notes and the blob.
- **Invariants intact**: notes are DB-only (no `ChatEffect`), isolated by
  `profile_id`, no schema migrations (`@self` is a plain tag; the `narrative`
  field is `#[serde(default)]`).
- **Tests**: notes (recall/gate/overview hide self notes; supersede/merge
  inherit tags; backfill migrates and is idempotent; note_delete is isolated +
  removes the vector); self_model (add_insight writes @self + the gate shows a
  similar one; get_self_model assembles from notes; trait scar → self-note;
  goal folding → self-notes); entity (`render_*` over `recent`; `is_empty`
  without the narrative; `fold_closed_goals` returns scars); orchestrator
  (injection reads observations; F3 DeleteInsight/Clear act on notes).
  **711 tests green**, clippy/fmt clean. **Probe assessment: GO** — a run of
  `self_model_gate_e2e_live` on Gemma 4 31B + bge-m3: the `add_insight` gate
  showed a similar observation, the model integrated it (`note_merge`/
  `note_revise`), 3/3 runs a near-duplicate was resolved. Graceful degradation
  verified (embed without `--embeddings` → the gate goes empty, nothing panics).
  → Tier 2 (below).

### Post-M9: narrative as notes — Tier 2 (structure: relevance + graph) (done)
- Continuation of Tier 1 ([docs/narrative-as-notes.md](../../docs/history/narrative-as-notes.md)):
  observation-notes gain **structure**. Forks confirmed by the user: scope
  **A+B** (C — semantic gates for traits — deferred); relevance-based injection
  — in the **system prompt** (prefix cache is invalidated every turn, an
  accepted cost, §9.3).
- **A. Relevance-based injection** (`orchestrator/generation.rs`): observations
  in the system prompt are mixed in not only by recency but by **relevance to
  the latest message** — an old but topically relevant observation surfaces
  when the topic returns. `notes::self_notes_relevant` (backfills vectors →
  embeds the query → searches among @self by cosine similarity, top-K);
  `blend_self_notes` (K relevant + a guaranteed freshest one for continuity,
  dedup, cap); assembled into `injection_recent`. **The injection moved from
  the sync `start_generation` into the async task `spawn_generation`** —
  relevance requires an async embedding of the latest message, and the
  command handler is synchronous; `GenSpawn` carries `self_model`/params/flags/
  `last_user`, the token estimate is emitted after injection.
  `ensure_note_vectors` was generalized to `(storage, embedder, profile)`.
  Graceful degradation to recency (no embedder/reply).
- **B. Graph over observations**: the mechanics (`note_link`/`note_neighbors`)
  already worked on self-notes (they are notes, the tag isn't filtered) —
  Tier 2 **uses and surfaces** it. (B1) `REFLECT_TOOL_IDS` += `note_link`/
  `note_neighbors`, the `reflect` message/rubric nudge linking related
  observations (`contradicts`/`refines`/`relates`) by full id from
  `get_self_model`. (B2) `notes::self_related_block` — a "Related observations"
  block: graph edges touching the shown observations (structure — "what
  relates to what" that a flat list can't give), **self↔self only**; a
  neighbor outside the shown set is brought in with its text (spreading
  activation); edge dedup. `self_model::render_self_read` (= `render_full` +
  the block) is used in `get_self_model`/`reflect`. The passive injection does
  **not** show the graph (prompt compactness). The self-consolidation overview
  is deferred.
- **Not included**: C (semantic gates for `user_model` traits —
  near-duplicate traits), the self-consolidation overview, cross-organ links
  (self↔user↔RAG, Tier 3), full output mixing (`[about self]` in general
  recall, Tier 3) — per the scope decision.
- **Tests**: `self_notes_relevant` (ranking + @self filter + empty query);
  `blend_self_notes` (relevant first, freshest guaranteed, dedup, cap);
  `injection_recent_surfaces_relevant_over_fresh` (an old relevant observation
  surfaces above a fresh one — deterministic on `MockEmbedder` + a temp
  storage); `get_self_model` shows "Related observations" + the link type;
  `REFLECT_TOOL_IDS` contains the graph tools; `reflect_system_message` nudges
  `note_link`. Live smoke `self_model_graph_e2e_live` (`#[ignore]`,
  `spawn_orch_live`): the model links contradicting observations. **716 tests
  green**, 20 `#[ignore]`, clippy/fmt clean.
- **Graph smoke — GO** (`self_model_graph_e2e_live` on Gemma 4 31B + bge-m3):
  the model on its own recorded two contradicting observations
  (`add_insight`), saw them with full ids via `get_self_model`, and linked them
  with a `contradicts` edge (`note_link`) — the edge appeared in the
  observation graph. Relevance/graph are put to use on a live model.

### Post-M9: narrative as notes — Tier 2, step C (related-traits gate) (done)
- **Semantic gate for related `user_model` traits** — the deferred step C of
  Tier 2 ([docs/narrative-as-notes.md](../../docs/history/narrative-as-notes.md)), a
  **mirror of the `add_insight` gate**, but over the flat interlocutor-trait
  list: on `add_traits`, for each actually-added trait the closest one among
  the **prior** traits (existing before this edit) is searched, above the
  cosine-similarity threshold `TRAIT_SIMILARITY=0.72`; if a close one is found,
  the tool **shows it** and asks the model to decide: a **duplicate** (merge
  via `remove_traits`) or a **contradiction** (record as an `add_insight`
  observation). **A soft gate** (not a block — the decision is left to the
  model, both traits are kept).
- **The 0.72 threshold was calibrated from a live test, not guessed.** The
  initial 0.85 (by analogy with the notes consolidation overview) on a live
  run **missed genuine paraphrases**: bge-m3 compresses short traits into a
  narrow band, "values brevity" ↔ "appreciates concise answers" = **0.77**
  (< 0.85 → the gate stayed silent, even though it's a duplicate). Calibration
  via our own client: paraphrases 0.73–0.83, unrelated 0.51–0.69 → threshold
  0.72. **Key observation:** bge-m3 groups traits by **dimension/topic**, not
  by direction of meaning, so **antonyms** also fall in the band ("values
  brevity" ↔ "values long explanations" = 0.71). This isn't a bug but a
  **reframing of intent**: the gate's wording was changed from "near-duplicate"
  to "related trait — duplicate or contradiction?" (consistent with the
  "integration over accumulation" philosophy + contradictions living in the
  narrative). A curl+awk measurement gave corrupted values (0.97 on
  everything, 1.000 on antonyms) — the correct values come from our own
  `OpenAiClient` (dim=1024, real bge-m3).
- **On-the-fly trait embedding** (`self_model::near_duplicate_traits`): traits
  have no stored vectors (`Vec<String>`, unlike notes with `note_vectors`), so
  the added traits + the prior ones are embedded in **one request** and
  compared via `cosine` (made `pub(crate)` in `notes.rs` for reuse). **Graceful
  degradation**: embedder unavailable / mismatched vector count → empty (like
  `add_insight`/`note_save`). The gate only fires when traits were actually
  added (an empty set skips the embedding call).
- **Snapshot of prior traits — inside the atomic edit** (the
  `self_model_update` closure, captured via `&mut existing_before_traits`);
  the embedding itself happens **afterward** (async/storage outside the
  closure, like the revision scars). The actually-added ones are computed
  outside the closure (requested ∖ prior, case-insensitive, deduplicated
  within the batch).
- **Traits stay `Vec<String>`** (no schema migration); the `update_user_model`
  description now mentions the gate. **Invariants intact**: DB-only, isolated
  by `profile_id`, no `ChatEffect`.
- **Tests**: `add_trait_gate_surfaces_near_duplicate` (a related trait raises
  the gate + a `remove_traits` hint), `add_trait_gate_silent_for_dissimilar`
  (unrelated stays silent, both traits are kept). On `MockEmbedder` ("aaaa
  bbbb" ↔ "aaab" cosine ≈ 0.89 > the threshold). Live `#[ignore]` smoke
  `trait_gate_e2e_live` (`spawn_orch_live`, a real embedder). **718 tests
  green**, 21 `#[ignore]`, clippy/fmt clean.
- **Smoke — GO** (Gemma 4 31B + bge-m3): the model added a similar trait → the
  gate showed a related one ("values brevity" ≈ "appreciates concise
  answers"), the model replied "this is a duplicate" and merged them into one
  trait via `remove_traits`. The gate fires, duplicate recognition/integration
  work.

### Post-M9: self-consolidation overview in reflection (Tier 3, narrative as notes) (done)
- **The "self-consolidation overview" deferred in Tier 2 is now enabled** after
  confirming the value of linking observations (Tier 3, GO): auto-reflection
  and the `reflect` tool now get **concrete data** for the self-memory "sleep",
  not just a rubric. A mirror of `build_consolidation_overview` (an overview of
  user notes for `consolidate_notes`), but over `@self` observations.
- **`build_self_consolidation_overview(storage, profile_id) -> Option<String>`**
  (`features/tools/notes.rs`): a pure DB read (vectors already in the DB) over
  **only** `@self` observations (mirroring the exclusion of self from the
  user-notes overview — the observation "sleep" doesn't touch memory about the
  interlocutor). Three sections: **similar pairs** (possible observation
  duplicates, pairwise cosine ≥ `CONSOLIDATE_SIMILARITY` 0.85, descending by
  similarity), **`contradicts` links** among observations (both ends `@self`),
  **observations without links** (candidates to link). `None` if there are
  fewer than 2 observations (nothing to consolidate). Isolated by
  `profile_id`.
- **Wiring**: `Reflect::invoke` (`features/tools/self_model.rs`) mixes the
  overview in between "Current self-model" and the rubric (empty when < 2
  observations); auto-reflection (`app/orchestrator/reflection.rs`) appends
  the overview to the digest after the borrowed block (`let mut digest`);
  `reflect_system_message` nudges using the "Observations overview for
  consolidation" block (merge similar pairs via `note_merge`/`note_supersede`,
  check `contradicts`, link unlinked ones via `note_link`).
- **Invariants intact**: DB-only, isolated by `profile_id`, no `ChatEffect`, no
  schema migrations (`@self` is a plain tag; vectors are already in
  `note_vectors`; the graph is in `note_links`).
  **Tests**: notes (`self_consolidation_overview_covers_self_only` — covers
  only observations, excludes user notes, `None` when < 2, shows a similar
  pair and `contradicts`); self_model
  (`reflect_includes_self_consolidation_overview` — with ≥2 observations
  reflect mixes in the overview; the `reflect_returns_current_and_rubric` test
  updated — with < 2 there's no overview); reflection (an assert in
  `reflect_message_nudges_cross_organ_linking`). **729 tests green**, 25
  `#[ignore]`, clippy/fmt clean.
- **Smoke — GO** (`self_consolidation_overview_e2e_live`, Gemma 4 + bge-m3):
  the model recorded two similar observations, called `reflect` — its result
  carried the "Observations overview for consolidation" block with the
  similar pair (a real embedder, similarity **0.89 ≥ 0.85**), after which the
  model merged the duplicates via `note_merge` into one observation. The full
  chain (observations → reflect with the overview → merge) worked.
- **Deferred** (groundwork): vec0 as the number of observations/notes grows;
  a background auto-consolidation of the self-model on a timer (currently
  the overview flows through auto-reflection/the interactive `reflect`).

### Post-M9: self-model summary as a snapshot, not a chronicle (anti-bloat) (done)
- **Problem** (user observation + a live profile): the self-model's `summary`
  was ballooning into a dense essay (~4.5–5k chars) with duplicates. Cause:
  every organ **except `summary`** has anti-bloat mechanics: observations have
  duplicate gates/replacement/graph, goals have a closed cap, traits have a
  0.72 semantic gate and merge, but `summary` had no target, no gate, no
  consolidation, no size feedback, and `POLICY_CORE` routed event-like
  conclusions there (a "durable/transient" axis instead of "state/event").
  Plan — four stage PRs, design doc
  [docs/history/summary-as-snapshot.md](../../docs/history/summary-as-snapshot.md).
- **Stage 1 — a genre boundary (text only, 0 logic):** `POLICY_CORE` (the
  single source for the maintenance protocol and auto-reflection) was
  rewritten around the axis "state → `summary`, event/conclusion →
  `add_insight` **even if durable**" (an observation isn't lost: it surfaces
  by relevance, gets linked, gets consolidated); `summary` becomes a compact
  snapshot of "who you are, what you value, how you work", and editing it
  should "integrate **and SHORTEN**" + a step "read it in full via
  `get_self_model` before editing (it's truncated in the prompt)". The
  `update_self_model`/`add_insight` descriptions and the `reflect` rubric were
  brought to the same boundary.
- **Stage 2 — a soft gate on `summary` size** (a gate, not a cap — data isn't
  truncated): new config `self_model.summary_target_chars` (default 1000,
  `#[serde(default)]`, sanitized to a floor of 200 in `SelfModelParams`); a
  pure `SelfModel::summary_fill_hint(target)` (`None` within target, otherwise
  text with the current size — an analogue of the former
  `narrative_fill_hint`); **three display points** — reading
  (`render_self_read` → seen by `get_self_model`/`reflect`/auto-reflection), a
  data-aware note after `maintenance_protocol()` in `inject_self_model`, and a
  "Description: N chars (target ≤ M)" line in `update_self_model`'s echo when
  the summary is edited; a "Self-model: description target (chars)" field in
  the settings "Tools" section (`FieldId::SmSummaryTarget`).
- **Stage 3 — per-section injection budgets** (only the rendering changes,
  the data doesn't): `render_for_prompt` truncates the "About you" section
  to **half** the limit (`max_chars/2`), guaranteeing the rest of the budget
  for goals/interlocutor/observations — a bloated summary no longer crowds
  them out of the prompt (before, a single final truncation was eating
  everything past the description); `truncate_chars_word` — truncation at a
  word boundary (falling back to the last space, one long word →
  char-by-char); a final truncation of the whole block is a safety net
  (char-exact); **`render_full` (full read) is NOT truncated** — a lesson
  from before.
- **Stage 4 — a leaner edit echo:** `update_self_model`/`update_user_model`
  now return **deltas** instead of the full `render_full` (full reads remain
  the job of `get_self_model`) — saves tokens and removes the "anchoring" on
  the essay genre. `update_self_model`: a size line + added goals with `#id`
  + goals closed as done/no-longer-relevant by `#id` + the count folded into
  observations (deltas are collected in the atomic edit via `&mut` capture;
  along the way the `changed` tracking was corrected — an empty `add_goal` no
  longer flags a change). `update_user_model`: compact final lists for the
  interlocutor + a scar confirmation (the `note` text); the trait gate and the
  `note` reminder are unchanged. Public `entities::self_model::short_id(&Uuid)`
  — a `#id` handle for the echo.
- **Invariants intact**: SelfModel mutations are DB-only (no `ChatEffect`),
  isolated by `profile_id`, no migrations (`#[serde(default)]`); injection
  stays in `system` (the prefix-cache trade-off accepted 2026-07-03).
- **Tests**: entity (`summary_fill_hint`, sanitization, the per-section budget
  doesn't crowd out other sections, word-boundary truncation, delta ids);
  tools (echo deltas, the reading hint, the size line, the POLICY_CORE/rubric
  genre markers); orchestrator (the data-aware note in `inject_self_model`);
  config (default). **770 unit tests green** (+11), clippy/fmt clean. **Live
  run — GO** (Gemma 4 31B + bge-m3): `summary_gate_e2e_live` — the model saw
  "grown to: 1600 ≤ 1000", shrank `summary` 1600 → 57 chars; no regression
  found — the `trait_gate`/`auto_reflect`/`self_model_e2e`/`self_model_gate`
  smokes are green (the new echo, trait gate, `note` scar, reflection,
  `add_insight` gate all work).
- **Groundwork**: timed auto-consolidation of the self-model (stage 2's gate
  will give it the "summary is bloated" signal); semantic comparison of
  summary paragraphs with @self observations in the self-consolidation
  overview; aging of `current_interests` — see the design doc's "Out of
  scope".

### Post-M9: self-model consolidation — stage A1 (background "sleep" on a timer) (done)
- **First stage of the "self-model consolidation" track** (design doc
  [docs/history/self-model-consolidation.md](../../docs/history/self-model-consolidation.md),
  branch `feat/self-model-auto-consolidate`): a periodic background "sleep"
  specifically for memory-about-self — mirroring `notes.auto_consolidate_every`.
  Previously self-model consolidation only happened via auto-reflection/the
  interactive `reflect`; A1 adds a **separate** periodic task that itself
  merges duplicate observations (`@self` notes), compresses a bloated
  `summary`, and links contradictions. The scaffolding was **specifically
  prepared** by SOLID stage 2 ("family task #3 doesn't touch `run()`/`Quit`")
  — adding it amounted to mirroring `consolidation.rs`/`reflection.rs`.
- **A mini agentic-loop, like reflection/note consolidation**
  (`app/orchestrator/self_consolidation.rs`, new): `maybe_auto_self_consolidate`
  is called in `handle_done`; gates — the feature is enabled
  (`self_model.auto_consolidate_every`, 0=off), the profile has enabled
  self-model tools (`get_self_model`), **there's something to consolidate**
  (`@self` observations ≥ 2 **or** `summary` exceeds `summary_target_chars`
  per `summary_fill_hint`), no "sleep" is already running, the server is
  `Ready`. Cadence tracked by the `self_consolidate_counts` counter (reset only
  on an actual spawn — a gate skip doesn't lose the cycle, same as its
  siblings). Tool set (intersected with the profile):
  `get/update_self_model`/`update_user_model` + `note_revise`/`supersede`/
  `merge`/`link`/`neighbors` over `@self`; **`note_recall` is withheld** (it
  hides `@self`; full ids come from `get_self_model`). Digest —
  `build_self_consolidation_overview` + a `summary_fill_hint` line. The system
  message `prompt.self_consolidate.system` is assembled from
  `self_model::policy_core` (a single source of rules, same as
  `reflect_system_message`). DB-only, the "sole owner of `Chat`" invariant
  intact.
- **Observability** (`background.rs`): `handle_bg_done` now emits
  `SelfModelChanged` on success for **both** reflection **and**
  `SelfConsolidation` (an open `F3` reloads the snapshot); a run of failures →
  `ui.err.bg_self_consolidation`. A new `BackgroundKind::SelfConsolidation`
  (events.rs) → `dispatch.rs` → `ChatScreen::set_self_consolidating` → a quiet
  status-bar chip. `background_hint` **was generalized** from a 2-flag match
  into a `·`-joined list of active-task labels (scales to N tasks; the key
  `ui.chat.bg.both` was dropped, `ui.chat.bg.self_consolidate` added).
- **Config/UI**: `SelfModelSettings.auto_consolidate_every` (`#[serde(default)]`,
  default 0 — no migration; mirrors `auto_reflect_every`); field
  `SmAutoConsolidate` in the "Memory" → "Self-model" section (catalog/mod/spec),
  i18n keys for the fields/descriptions.
- **A separate toggle, not a shared one** (user's decision): gates/data for the
  self-model and for notes are already kept apart, its own counter is more
  precise. **A2** (summary↔observation semantics) and **A3** (interest aging) —
  deliberately NOT in this PR (next stages).
- **Tests**: integration (`orchestrator/tests/self_consolidation.rs` — spawns
  at the threshold + resets the counter; the counter stays intact when the
  server isn't ready; a gate when the feature is disabled; a "nothing to
  consolidate" gate that preserves the counter; success →
  `SelfModelChanged`); unit (the system message is composed from `policy_core`
  + per-locale; the tool set excludes `note_recall`). **1136 unit tests
  green** (+8), 51 `#[ignore]`, clippy `-D warnings`/fmt/i18n gates clean.
  Live smoke `self_consolidation_e2e_live` (`#[ignore]`, mirroring
  `auto_reflect_e2e_live`) — a run against real Gemma 4 + bge-m3 was a manual
  step. **Live smoke green** (Gemma 4 31B q4 + bge-m3): with
  `auto_consolidate_every=1`, two similar observations were merged, a bloated
  summary compressed.

### Post-M9: self-model consolidation — stage A2 (summary↔observation semantics) (done)
- **Second stage of the track** (design doc
  [docs/history/self-model-consolidation.md](../../docs/history/self-model-consolidation.md)
  §A2, branch `feat/self-model-summary-semantics`): the self-consolidation
  overview gained a section "a paragraph of the self-description (`summary`)
  semantically overlaps observation X → extract/stitch". Observations (`@self`
  notes) hold vectors in the DB, but `summary` has none (free-form text) — so
  paragraphs are embedded **on the fly** in a single request (a direct mirror
  of the trait gate `self_model::near_duplicate_traits`).
- **An async layer over a synchronous handler** (a key nuance):
  `build_self_consolidation_overview` stays a **pure synchronous** DB read
  (the 3 previous sections, tests intact). A2 semantics — a separate **async**
  helper `notes::summary_observation_overlaps(storage, embedder, profile,
  loc)` (graceful degradation: no embedder / a vector-count mismatch →
  `None`). Wired into three places: (1) interactive `reflect`
  (`Reflect::invoke`, already async) — appends the section; (2)+(3)
  background reflection/`self_consolidation` — their handlers are
  **synchronous** and spawn a task, so the section is computed **inside the
  spawned task**: `SilentLoop` gained a new optional field
  `summary_semantics: Option<SummarySemantics{embedder,storage,profile_id,
  loc}>`, and `spawn_silent_loop` `await`s the helper before the loop and
  appends the result to the request's first user message. Note consolidation
  (`consolidation.rs`) passes `None` (its overview is about the interlocutor's
  notes, not about `summary`).
- **The threshold was calibrated on live bge-m3** (not guessed; the smoke
  `summary_obs_calibration_e2e_live` prints cosines for labeled pairs):
  paraphrase pairs "paragraph ↔ observation" scored **0.69–0.80**, unrelated
  pairs — **0.48–0.51**; a clean gap 0.51→0.69 → `SUMMARY_OBS_SIMILARITY =
  0.62` (inside the gap, with margin on both sides). Paragraphs are longer
  than short traits, so paraphrases score a bit lower than the trait gate
  (0.73–0.83, threshold 0.72). One match per paragraph (to avoid noise);
  fragments < 40 characters are dropped.
- **No config/migration** — the threshold is currently a code constant (no
  extra settings field added, as in the design doc); an i18n key for the
  section `notes.self_overview.summary_obs` (ru+en). DB-only, the "sole owner
  of `Chat`" invariant intact; FSD (the helper lives in `features`,
  `tool_loop` calls it).
- **Tests**: unit tests on `MockEmbedder` (threshold-robust: a match ≈1.0,
  unrelated ≈0.0) — the section surfaces a matched paragraph and doesn't
  surface an unrelated one; graceful degradation (no observations / an empty
  summary / a vector mismatch → `None`). Live `#[ignore]`:
  `summary_obs_calibration_e2e_live` (prints cosines for calibration) and
  `summary_obs_overlap_e2e_live` (the section surfaces on real bge-m3 —
  measured at 0.77). **1138 unit tests green** (+2), 53 `#[ignore]`, clippy
  `-D warnings`/fmt/i18n gates clean. **Live run green** (bge-m3): calibration
  + section surfacing confirmed; the 0.62 threshold catches all paraphrases,
  filters out unrelated pairs.
- **A3** (aging of `current_interests`) — the next/final stage of the track.

### Post-M9: self-model consolidation — stage A3-light (interest aging) (done)
- **Final stage of the track** (design doc
  [docs/history/self-model-consolidation.md](../../docs/history/self-model-consolidation.md)
  §A3, branch `feat/self-model-interests-aging`): the interlocutor's
  `current_interests` — a plain `Vec<String>` — was never washed out by
  anything. **The light path** was chosen (no schema/config change, in the
  project's spirit of "integration by the model itself"): a **nudge** in the
  canonical "maintenance protocol" `selfmodel.policy_core` — "for 'current'
  interests: remove via `remove_interests` those the interlocutor hasn't
  confirmed in a while." The tool `update_user_model` already supports
  `remove_interests` — code/schema untouched, only the bundle text was edited
  (ru+en).
- **One source → three consumers**: `policy_core` feeds, through composition,
  the turn injection (`maintenance_protocol`), auto-reflection, and self-model
  auto-consolidation — the nudge propagates to all three without duplication.
  **The heavy path** (`current_interests: Vec<Interest{text, updated_at}>` —
  real time-based aging) was deliberately **not** done: it would break the
  flat `Vec<String>` and ~a dozen call sites; left as future work in case the
  light path proves insufficient on a live model.
- **Tests**: `policy_core_nudges_interest_aging` (the nudge carries
  `remove_interests` + the aging idea). **1139 unit tests green** (+1), 53
  `#[ignore]`, clippy `-D warnings`/fmt/i18n gates clean. The effect is
  behavioral/soft (the composed prompts are already covered by live
  reflection/consolidation smokes) — a separate live run isn't needed. **The
  "self-model consolidation" track (A1–A3-light) is complete** (A3-heavy —
  future work).

### Post-M9: self-model injection — per-section budgets (done)

- **Found while measuring for the prompt-caching research**
  ([prompt-caching.md §2.1](../../docs/research/prompt-caching.md)), fixed as its own
  task; plan and decisions D1–D5 —
  [self-model-injection-budget.md](../../docs/history/self-model-injection-budget.md)
  (user, 2026-08-08). Behaviour: spec §17.4. Branch
  `fix/self-model-injection-budget`.
- **The defect**: `render_for_prompt` assembles description → goals →
  interlocutor → observations, and **only the description had a budget**
  (`max_chars / 2`, from summary-as-snapshot stage 3). Everything after it was
  unbounded and the block was cut at `prompt_cap` — so it was not a budget but a
  queue: whoever renders first eats. Measured on the real dev profile against the
  default 1200: description 1349, goals 855, traits 1893, interests 1039,
  dynamic 223 — **5359 characters of content**, of which the model saw the
  description (600) and the goals, cut mid-item. **Nothing else.** So
  `update_user_model` — a tool with its own semantic gate and its own "scar" on
  replacement — wrote into a structure the model never passively saw, and
  `injection_recent` queried the embedder every turn for a relevance selection
  that truncation then discarded, while the injected text told the model that
  observations "surface by relevance". The code comment stated the intent
  correctly ("so a bloated summary doesn't crowd goals/interlocutor/observations
  out"); it had only ever been implemented for the description.
- **Not data loss**: `render_full` (what `get_self_model` returns and `F3` shows)
  is untruncated, so the model could always read the whole thing on request. The
  defect is in what is *passively* injected — which is the mechanism the whole
  self-model track calls "the main mechanism of value".
- **Fix — fixed shares with carry-forward** (D1/D2): description 40%, goals 20%,
  interlocutor 20%, observations 20%; each section's allowance is its share plus
  whatever earlier sections left unused. The load-bearing property is that a
  share is a **ceiling**: a bloated early section cannot reach past it, which
  gives later sections a floor **without a second mechanism** — the "reserve a
  minimum for observations" fork dissolved once the shares were ceilings rather
  than targets.
- **Lists lose whole items and say how many** (D3/D4): a trait cut in half reads
  as a *different* trait, and the count ("… +N more") tells the model it is
  seeing a part, with the full list one `get_self_model` away. One deliberate
  degradation: when not even one item fits, the section falls back to a character
  cut of the first item rather than disappearing — an absent section reads as
  "no traits", which is a stronger and wronger claim than a truncated one.
- **`prompt_cap` default 1200 → 4000** (D5). **An existing `settings.json` keeps
  its own value** — the field is always serialized, so this only reaches fresh
  installs; a migration that rewrote it was deliberately not done, since it would
  also overwrite a deliberately-chosen 1200 and has no correctness argument
  behind it (ADR 0006 treats a value rewrite as a real migration). Recorded in
  the constant's doc comment, the spec and the CHANGELOG rather than left for
  someone to discover.
- **Tests**: the direct regression (all four sections present on a model shaped
  like the measured profile — at the new default **and** at the old 1200, where
  the fix also has to degrade sensibly), carry-forward (a short description
  leaves the goals more room), whole-item truncation with the count, and the
  oversized-single-item fallback. **1952 unit tests green** (+5), 81 `#[ignore]`,
  clippy `-D warnings`/fmt/`cyrillic_scan`/`link_check` clean.
- **Mutation-tested**, and one of the four mis-fired first: restoring the old
  "only the summary is bounded" fails 14 tests, cutting items instead of dropping
  them fails its own, dropping the "+N more" marker fails its own — but the
  carry-forward mutation initially **survived**, because I had removed the carry
  from the *interlocutor* section while the test measures *goals*. Retargeted, it
  fails exactly that test. A reminder that a surviving mutation is as likely to
  indict the mutation as the test.
- **One test's premise legitimately changed**: `render_truncates_to_cap` asserted
  the block is *exactly* `prompt_cap` long, which was true when the block-wide cut
  was the only limit. Now the section budget cuts first, so the block comes out
  shorter; the assertion moved to the invariant that actually matters (never
  exceeds the cap, and the description was truncated).
- **Verified on the real profile**, the same data that exposed the defect: at
  cap 1200 and at 4000, all four sections now render (before: two). **A live model
  run is not required** (AGENTS.md §3) — this is a pure rendering function in
  `entities`, no engine, memory-write or tool path is touched, and the behaviour
  is fully determined by its inputs.
- **Left alone deliberately**: what the model *writes* (summary target, trait
  counts) is a different question, already served by the summary size gate; and
  the wasted embedder call disappears on its own now that observations render.
