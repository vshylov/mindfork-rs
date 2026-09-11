# Journal — Structural refactors

Changes that moved code without changing behaviour: the god-object splits, the targeted SOLID work, and the single-source-of-truth consolidations.

**Reference documents for this area:** architecture.md §3

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)). They record what was done,
why, what was measured and what was rejected — the reasoning behind the code, not
its current shape. For the current shape read the reference documents named above;
for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (20)

- Post-M9: the generation state machine split out of the orchestrator (`GenState`) (done)
- Post-M9: the orchestrator god object split by feature (done)
- Post-M9: Phase 3 — extracted the EngineManager / SaveQueue sub-structures (done)
- God-object refactor — stage 1: `screens/settings.rs` → `screens/settings/` (done)
- God-object refactor — stage 2: `screens/chat.rs` → `screens/chat/` (done)
- God-object refactor — stage 3: `orchestrator/tests.rs` → `orchestrator/tests/` (done)
- God-object refactor — stage 4: `features/tools/notes.rs` → `features/tools/notes/` (done)
- God-object refactor — stage 5: `shared/storage/db.rs` → `shared/storage/db/` (done)
- God-object refactor — stage 6: `shared/markdown.rs` → `shared/markdown/` (done)
- God-object refactor — stage 7: `app/runtime.rs` → `app/runtime/` (done)
- Post-M9: co-locating db/markdown tests with their submodules (done)
- Post-M9: tool-catalog metadata — a single source of truth in the `Tool` trait (done)
- Post-M9: SOLID refactor — stage 1: `ToolContext` (dependency bundles + constructor) (done)
- Post-M9: SOLID refactor — stage 2: background tasks (slot registry + a single done channel) (done)
- Post-M9: SOLID refactor — stage 4: status-bar view-model + canonical runtime helpers (done)
- Post-M9: SOLID refactor — stage 3, step 3.1: settings-field description in `FieldRow` (done)
- Post-M9: SOLID refactor — stage 3, steps 3.2/3.3: a field-value access table (`field_spec`) (done)
- Post-M9: the child-loop seam — `TurnShared`, `RequestEnv`, `sanitize_title` in `shared` (done)
- Post-M9: the dialogue driver off `TurnLoop` — an explicit `DialogueCtx` (done)
- Post-M9: the decoding order out of `http_text` — `shared::text_decode` (done)

### Post-M9: the generation state machine split out of the orchestrator (`GenState`) (done)
- **The generation state was moved** out of `app/orchestrator.rs` into a separate
  `app/gen_state.rs` module — a `GenState` type (`Idle`/`Generating{id,cancel}`/
  `Cancelling{id}`). Implementation choice — **a plain enum with transition methods**,
  not `statig`/`rust-fsm`: states carry data (`Uuid` + `CancellationToken`), which
  those frameworks don't fit well (rust-fsm is for state-less FSMs; statig would be an
  HSM overkill for 3 states), and the transitions are entangled with side effects
  (spawning tasks, cancelling the token, emitting events, writing to `Chat`), which
  need to stay with the orchestrator — the owner of `Chat`. No new dependencies added.
- **Transitions are encapsulated in methods**, correct by construction: `begin(id,cancel)`
  (`Idle→Generating`, a no-op if busy), `request_cancel()` (`Generating→Cancelling`,
  returns the token — the orchestrator actually triggers the cancellation), `finish(id)`
  (`→Idle` only on matching `id`, anti-staleness against `Stop→Send`), `active_cancel()`
  (a token for `Quit` with no transition), `current_id()`/`is_idle()`. The type is
  **pure** (no I/O) → unit-testable without a tokio runtime (5 tests in `gen_state.rs`).
- **The orchestrator got thinner**: the transition invariant is no longer smeared
  across `match` sites (`handle_send`/`handle_regenerate`/`handle_delete_last`/`handle_impersonate`
  gate through `is_idle()`; `Cancel`/`handle_switch` — `request_cancel()`; `Quit` —
  `active_cancel()`; `start_generation` — `begin()`; `handle_done` — `finish()`).
  The `state` field was renamed to `gen_state` (`gen` is a reserved word in edition
  2024). Impersonation (`imp_gen`) and RAG (`rag_cancel`) were **deliberately** not
  folded in — those are concurrent sub-states (RAG runs in parallel with generation).

### Post-M9: the orchestrator god object split by feature (done)
- **`app/orchestrator.rs` (≈1.8k lines of code + ≈1.5k of tests) was split into
  the `app/orchestrator/` directory** — a purely mechanical refactor (Phases 1+2),
  with no change to `Orchestrator`'s structure/fields and no new channels: the
  **"sole owner of `Chat`" invariant is preserved** (see architecture.md §1, §10).
  Only `run`/`OrchestratorDeps` still stick out of the module (`main.rs` untouched).
- **The skeleton — `mod.rs`** (≈450 lines): `struct Orchestrator`, the `run()` loop with
  `select!`, the `handle_command` dispatcher, `bootstrap`, and shared helpers (emitters
  `emit_chat_list/profile_list/settings`, `chat_mut`, `effective_sampling`,
  `activate`, `mark_dirty`, `flush_saves`, `backend_if_ready`, `new_chat_value`).
  Command handlers — separate `impl Orchestrator` blocks in per-feature files:
  `generation.rs` (send/regenerate/delete_last/done + the agentic-loop task),
  `chats.rs`, `profiles.rs`, `settings.rs` (+apply_*), `title.rs`, `impersonation.rs`,
  `rag.rs`; domain↔engine mapping — `request.rs`. All tests (411 of them) — in `tests.rs`.
- **Visibility**: methods called from the dispatcher/another file, and internal-channel
  types (`GenResult`, `TitleResult`), are marked `pub(super)`; the shared
  helpers stayed private in `mod.rs` (child modules see the parent's private items
  under Rust's visibility rule). Child modules import exactly their own
  dependencies (no `use super::*` in non-test code → no "dangling" imports under
  `-D warnings`). Background tasks (`spawn_generation/_title/_impersonation/
  _rag_ingest`) are private free functions in their own feature files.
- **Phase 3** (extracting cohesive sub-structures) — **done as a separate PR**, see below.

### Post-M9: Phase 3 — extracted the EngineManager / SaveQueue sub-structures (done)
- **`Orchestrator` was shrunk from 27 to 16 fields** by extracting two cohesive
  units (no behavior change, no new channels; the "sole owner of `Chat`" invariant
  intact):
  - **`EngineManager`** (`app/orchestrator/engines.rs`) — the lifecycle of the
    inference/embedding servers: owns `backend`/`imp_backend`/`embedder`, handles to
    managed processes (`*_handle`, `kill_on_drop`), readiness statuses
    (`server_status`/`imp_status`), and probe channels (`status_tx`/`imp_status_tx`) +
    `supervisor`. Exposes `apply_chat` (→ returns the immediate status, the caller
    emits it), `apply_embed`, `apply_impersonation`, `set_chat_status`/
    `set_imp_status` (from the probe loop), `backend_if_ready`,
    `impersonation_backend_if_ready(mode)`, `embedder()`. A counterpart to the
    `ServerSupervisor` trait. Server logic (~120 lines) moved out of the orchestrator;
    `settings.rs` got thinner (81 → 57 lines) — `apply_*_settings` now just delegate.
  - **`SaveQueue`** (`app/orchestrator/save_queue.rs`) — a debounce queue for
    deferred saving: `dirty: HashSet<Uuid>` + `deadline`. Methods `mark`/`forget`/
    `deadline`/`take` (+ `is_dirty` under `#[cfg(test)]`). `SAVE_DEBOUNCE` moved
    here. The actual write stayed with the orchestrator (`flush_saves` calls `saves.take()`).
- **Visibility**: the types are `pub(super)`; fields that tests/the loop set
  (`engines.backend`, `engines.server_status`, `engines.embedder`) are `pub(super)`,
  the rest private. Delegating helpers `Orchestrator::mark_dirty`/`flush_saves`
  are kept (minimal changes at call sites); the orchestrator's direct `backend_if_ready`
  was removed — calls go through `self.engines.backend_if_ready()`.
- **Not included**: impersonation's *generation* state (`imp_cancel`/`imp_gen`/
  `imp_done_tx`) and `rag_cancel` — those are concurrent turn sub-states, not
  servers; left on `Orchestrator` (like `gen_state`). `cargo fmt`/`clippy`/`test`
  green (411 passed, 7 ignored).

### God-object refactor — stage 1: `screens/settings.rs` → `screens/settings/` (done)
- **Track plan** — [docs/history/refactoring-god-objects.md](../../docs/history/refactoring-god-objects.md)
  (7 stages + optional): splitting up several monolithic files that had grown
  into god objects (settings/chat/orchestrator-tests/notes/db/markdown/
  runtime). Method — the same playbook used for the orchestrator split
  (Phases 1–3): a **purely mechanical move** of a file into a directory
  module, no change to types/fields/channels/behavior.
- **Stage 1**: `screens/settings.rs` (4966 lines, one `impl SettingsScreen`
  ~2040 lines + ~1100 lines of free functions) was split into **8 files** of
  the `screens/settings/` directory (byte-exact slices by line ranges — zero
  transcription risk): `mod.rs` (650: `SettingsIntent`, the
  section/subsection enums `Section`/`Subsection`/`ModelTab`, the form types
  `FieldId`/`FieldKind`/`FieldRow`/`Editor`/`Focus`/`SearchHit`/`SearchState`/
  `ChoiceState`, `SamplingParam` + `SAMPLING_PARAMS`, `struct SettingsScreen`,
  submodule declarations), `catalog.rs` (565: the constructor + field
  builders for all sections/subsections + gates `gate_disabled`/
  `sampling_cloud_provider`), `apply.rs` (675: `handle_key` + dispatchers +
  the field editor + toggles/cycles + `apply_text`/`save_*`), `choice.rs`
  (150: the Choice popup + `reset_field`/`default_fields`), `search.rs`
  (155: the `/` search overlay), `render.rs` (530: all rendering),
  `helpers.rs` (1130: free functions — `row`/`grouped`/`sampling_row`/
  `managed_rows`/`cloud_rows`, `field_description`/`gate_hint`,
  `render_field_line`/`header_line`/`tab_strip_line`, `cycle_*` cycles/label
  functions, `parse_*` parsers/`decode/encode_escapes`), `tests.rs` (1175).
  The former `impl SettingsScreen` was split into 4 impl blocks (≤ ~660
  lines each).
- **Visibility rules (a playbook for UI screens):** (1) submodules pull
  `mod.rs`'s items in via `use super::*;` — the glob also picks up the
  parent's private `use` imports (ratatui/uuid/crate::…), so submodules
  don't duplicate outside imports, and none go unused in `mod.rs` (all are
  used transitively → zero warnings). (2) `helpers.rs` free functions are
  marked `pub(super)` (otherwise siblings can't see them); consumers use
  `use super::helpers::*;`. (3) private `impl SettingsScreen` methods called
  from another file are marked `pub(super)` (method privacy in Rust follows
  the defining module).
- **Deviations from the plan**: `descriptions.rs`/`editor.rs` weren't split
  out (`field_description`/`gate_hint` → `helpers.rs`; the field editor →
  `apply.rs`, tightly coupled with key handling). `helpers.rs` was left as
  one module of free functions — further splitting was deferred (independent
  pure functions, not a tangled impl block). The module's public path was
  unchanged (`crate::screens::settings::{SettingsScreen, SettingsIntent}`),
  outside `use` sites (`app/runtime.rs`) weren't touched. **805 tests green**
  (0 failed, 26 `#[ignore]`; the count is unchanged — a pure move), clippy
  `-D warnings`/fmt clean. Docs: architecture.md §3.

### God-object refactor — stage 2: `screens/chat.rs` → `screens/chat/` (done)
- **Stage 2** of the god-object split plan
  (docs/history/refactoring-god-objects.md — living on the stage-1 branch;
  this stage — branch `refactor/chat-module-split` off `main`).
  `screens/chat.rs` (2616 lines, one `impl ChatScreen` with ~54 methods over
  ~1100 lines + render helpers) was split into the `screens/chat/` directory
  — a purely mechanical move (item-level slices: methods cut on the closing
  4-space `}`, free functions on column-0 closures; types/fields/the
  `ChatIntent` contract/behavior unchanged).
- **Layout**: `mod.rs` (409: `ChatIntent`, popup types `ConfirmAction`/
  `SuggestPopup`/`ImpersonationState`/`RagBanner`, `struct ChatScreen`,
  snapshot accessors — settings/list/status/palette setters/getters),
  `input.rs` (367: `handle_key`/`handle_paste`/`handle_mouse` + draft/spell/
  commands + `trigger_destructive`/`handle_profile_overlay_key`), `popups.rs`
  (281: the spellcheck/confirmation/emoji/help popups +
  `render_help`/`render_suggest`/`render_confirm`/`HELP_KEYS`), `feed.rs`
  (241: projecting `AppEvent` into the feed + `feed_msg_has_vs16`),
  `render.rs` (190: `render`/`model_meta` + `centered_rect`/
  `visual_line_count`), `rag.rs` (94: the banner + `format_rag_sources`),
  `impersonation.rs` (51), `tests.rs` (1034). The former `impl ChatScreen`
  was split into 7 impl blocks (≤ ~360 lines each).
- **Visibility rules** (the same playbook as stage 1): submodules pull
  `mod.rs` via `use super::*;`; private methods called across files, and free
  functions, are marked `pub(super)`. Cross-module free functions get
  targeted `use`s: `input.rs` → `super::feed::feed_msg_has_vs16`, `popups.rs`
  → `super::render::centered_rect`, `render.rs` →
  `super::popups::{render_help, render_suggest, render_confirm}`, `tests.rs`
  → `super::popups::HELP_KEYS`.
- The module's public path was unchanged (`crate::screens::chat::{ChatScreen,
  ChatIntent}`), outside `use` sites (`app/runtime.rs`) weren't touched.
  **805 tests green** (0 failed, 26 `#[ignore]`; the count is unchanged — a
  pure move), clippy `-D warnings`/fmt clean. Docs: architecture.md §3.

### God-object refactor — stage 3: `orchestrator/tests.rs` → `orchestrator/tests/` (done)
- **Stage 3** of the god-object split plan
  (docs/history/refactoring-god-objects.md — living on the stage-1 branch
  `refactor/settings-module-split`; this stage — branch
  `refactor/orchestrator-tests-split` off `main`, the plan doc is
  synchronized on merge). The test monolith `app/orchestrator/tests.rs`
  (3506 lines, 74 tests for every feature in one file) was split into the
  `app/orchestrator/tests/` directory — a purely mechanical move (item-level
  slices, no test behavior/name changes).
- **Layout**: `mod.rs` (315: **all fixtures** —
  `spawn_orch`/`spawn_orch_cfg`/`wait_for`/`bare_orch*`/`enable_all_tools`/
  `orch_*`/live helpers + submodule declarations) and one file per feature,
  mirroring the source modules: `generation.rs` (755), `live.rs` (959, all
  `#[ignore]` e2e smokes), `chats.rs` (323), `self_model.rs` (317), `rag.rs`
  (267), `impersonation.rs` (169), `settings.rs` (158), `profiles.rs` (113),
  `title.rs` (107), `reflection.rs` (64), `request.rs` (17). The largest file
  dropped from 3506 to 959.
- **Key rules** (a playbook for test modules): (1) **every** non-`#[test]`
  helper → `mod.rs`, so any submodule can see them via `use super::*;`
  (eliminates cross-file fixture visibility issues). (2) A name collision:
  the test submodule `tests::generation` shadows the source module
  `orchestrator::generation` — in `self_model.rs`, references to self-model
  injection (`inject_self_model`/`blend_self_notes`/`injection_recent`/
  `GenResult`) were rewritten from `super::generation::` to
  `super::super::generation::` (up to `orchestrator`). No other submodule
  has this collision.
- The `tests` module's path was unchanged (`mod tests;` in
  `orchestrator/mod.rs`). **805 tests green** (0 failed, 26 `#[ignore]`; the
  count is unchanged — a pure move), clippy `-D warnings`/fmt clean. Docs:
  architecture.md §3.

### God-object refactor — stage 4: `features/tools/notes.rs` → `features/tools/notes/` (done)
- **Stage 4** of the god-object split plan
  (docs/history/refactoring-god-objects.md — on the stage-1 branch; this
  stage — branch `refactor/notes-module-split` off `main`).
  `features/tools/notes.rs` (2242 lines: 9 tools + the self-notes subsystem
  + consolidation overviews) was split into the `features/tools/notes/`
  directory — a purely mechanical move (item-level slices, no behavior
  change).
- **Layout**: `mod.rs` (123: the `NOTE_*_ID` id constants, thresholds,
  `SELF_NOTE_TAG`, shared helpers `is_self_note`/`parse_id`/`parse_tags`/
  `clip`/`cosine`, re-exports), `recall.rs` (234: `NoteRecall` +
  `list_user_notes`/`semantic_recall`/`related_block`/
  `cited_sources_block`/`format_notes`), `edit.rs` (222: `NoteRevise`/
  `NoteSupersede`/`NoteMerge`), `overview.rs` (206: `ConsolidateNotes` +
  `build_consolidation_overview`/`build_self_consolidation_overview`),
  `save.rs` (161: `NoteSave` + `create_note`/`ensure_note_vectors`/
  `self_note_similar`), `self_notes.rs` (149: `self_notes_recent`/
  `self_notes_relevant`/`self_related_block`/`migrate_self_narrative`),
  `graph.rs` (115: `NoteLink`/`NoteNeighbors`), `cite.rs` (66:
  `NoteCiteSource`), `tests.rs` (1005).
- **Key point — preserving the external surface** (broad: the orchestrator/
  `self_model`/`rag`/`tools::mod`/`meta` call ~30 `notes::X` items): mod.rs
  re-exports everything via `pub(crate) use self::{cite::*, edit::*, …}::*;`,
  so outside `use crate::features::tools::notes::{NoteSave, create_note,
  self_notes_recent, …}` sites weren't touched. Private cross-submodule
  helpers (`related_block`/`list_user_notes`/`semantic_recall`/
  `format_notes`/`ensure_note_vectors`) were widened to `pub(crate)`;
  submodules pull everything in via `use super::*;` (a glob through the
  parent's re-export). Constants and small shared helpers stayed in `mod.rs`
  (visible to submodules as the parent's private items).
- **A parsing subtlety** (handled): a `///` line doc comment ending with `;`
  (prose like "…for storage/search;") was falsely treated as an item
  boundary and split the doc comment — the parser gained a guard: "a `;`
  isn't treated as a boundary on a line starting with `//`".
- The module's public path was unchanged. **805 tests green** (0 failed, 26
  `#[ignore]`; the count is unchanged — a pure move), clippy
  `-D warnings`/fmt clean. Docs: architecture.md §3.

### God-object refactor — stage 5: `shared/storage/db.rs` → `shared/storage/db/` (done)
- **Stage 5** of the god-object breakup plan (docs/history/refactoring-god-objects.md — on
  stage 1's branch; this stage — branch `refactor/db-module-split` off `main`).
  `shared/storage/db.rs` (1665 lines, one `impl Db` with ~41 methods, 4 data domains)
  split into the `shared/storage/db/` directory by domain — a purely mechanical move
  (item-level slices, DB behavior/schema unchanged).
- **Layout**: `mod.rs` (222: `struct Db`, `open`/`open_in_memory`/`from_conn`,
  `register_sqlite_vec`, **`migrate()` — the whole schema in one block**, `ensure_vec_table`/
  `vec_dim`, shared helpers `row_to_note`/`parse_uuid`/`parse_dt`/`cosine`, declarations),
  `rag.rs` (321: documents/search/sources/dimensionality + delete-by-path +
  `delete_matching`/`delete_sources_matching`/`norm_path`), `notes.rs` (242: insert/
  list/edit/delete + embeddings/semantics), `graph.rs` (218: link graph +
  supersession + source citation), `self_model.rs` (102: get/upsert/atomic
  update), `tests.rs` (591). The former `impl Db` split into 5 impl blocks (≤ ~320 lines each).
- **Key point**: `Db` methods are inherent methods (`pub`, called as
  `db.method()` through the `Storage` facade), so splitting `impl Db` across files
  **requires no re-exports** (the method resolves by type regardless of file). Private
  `struct Db` fields (`conn`) are visible to submodules (children see the parent's
  private items); shared free helpers in `mod.rs` (private) are picked up by
  submodules via `use super::*;` (a glob pulls in the parent's private items).
  It compiled on the first try — no visibility fixes, no explicit imports.
- The `db` module path is unchanged (`shared::storage::db::Db`). **805 tests green**
  (0 failed, 26 `#[ignore]`; the count didn't change — a pure move), clippy
  `-D warnings`/fmt clean. Docs: architecture.md §3.

### God-object refactor — stage 6: `shared/markdown.rs` → `shared/markdown/` (done)
- **Stage 6** of the god-object breakup plan (docs/history/refactoring-god-objects.md — on
  stage 1's branch; this stage — branch `refactor/markdown-module-split` off `main`).
  `shared/markdown.rs` (1995 lines, three subsystems: walker `Writer`, tables, LaTeX
  converter + code highlighting) split into the `shared/markdown/` directory — a purely
  mechanical move (item-level slices with awareness of dependencies, behavior unchanged).
- **Layout**: `mod.rs` (146: `render`/`render_with` — **the entire external surface** +
  palette-derived styles `heading_style`/`code_style`/… + wiring), `latex.rs` (578: LaTeX→
  unicode — delimiter normalization + command converter, **self-contained**),
  `writer.rs` (418: `Writer` — pulldown-cmark event walker → lines + `heading_number`),
  `table.rs` (312: `TableBuilder` + `render_table` + column layout), `code.rs` (176:
  syntect highlighting — syntax + theme from the palette), `tests.rs` (397).
- **Key point — `Writer` is the hub** (calls styles from `mod.rs`, highlighting from
  `code`, tables from `table`, LaTeX from `latex`; `mod.rs::render_with` builds the
  `Writer`). Wiring: subsystem items used across files are marked `pub(super)`
  (including `TableBuilder` fields — built/mutated by `Writer` and read by
  `render_table` — and the `Writer.lines`/`soft_break_as_newline` fields, which
  `render_with` accesses); `mod.rs` collects them with a private glob
  `use self::{code::*, latex::*, table::*, writer::*};`, and submodules pick up
  everything via `use super::*;` (styles from `mod.rs` — as the parent's private items).
- **Two clippy lessons** (not caught by `cargo build`, only `-D warnings`): (1) a
  re-export of internal wiring must be a **private** `use …::*` (not `pub(crate) use`) —
  otherwise "glob import doesn't reexport anything with visibility pub(crate)", since
  the items are `pub(super)`, not `pub`; (2) `latex.rs` is self-contained — its
  `use super::*` turned out unused and was removed.
- The external surface (`markdown::render`/`render_with`, used only by `message_feed`)
  is untouched. **805 tests green** (0 failed, 26 `#[ignore]`; the count didn't change —
  a pure move), clippy `-D warnings`/fmt clean. Docs: architecture.md §3.

### God-object refactor — stage 7: `app/runtime.rs` → `app/runtime/` (done)
- **Stage 7** (final) of the god-object breakup plan
  (docs/history/refactoring-god-objects.md — on stage 1's branch; this stage — branch
  `refactor/runtime-module-split` off `main`). `app/runtime.rs` (1099 lines: the TUI
  loop + paste batching + clipboard + dispatch) split into the `app/runtime/`
  directory — a purely mechanical move (item-level slices, behavior unchanged).
- **Layout**: `mod.rs` (309: `run`/`run_loop` — the loop + dirty repaint,
  `ActiveScreen`, `SpellLoader`, `TICK` + wiring), `dispatch.rs` (334: `apply_event`
  applies `AppEvent` to the screen + `dispatch`/`dispatch_chat_list`/`dispatch_settings`/
  `dispatch_self_model` translate Intent→`AppCommand`), `input.rs` (218: input batching
  and clipboard-paste handling on Windows — `Chunk`/`chunk_batch`/`process_input_batch`/
  `collect_press`/`paste_char`/`reconcile_paste`/`paste_projection_matches`),
  `clipboard.rs` (30: `read_clipboard_text`/`write_clipboard` via arboard), `tests.rs`
  (238). The external surface — only `run` (main.rs) — remains `pub` in `mod.rs`.
- **Wiring** (as in markdown): cross-file items are marked `pub(super)`, `mod.rs`
  collects them with a private glob `use self::{clipboard::*, dispatch::*, input::*};`
  (the `run_loop` hub calls them by their short name); submodules pick up
  `ActiveScreen`/types via `use super::*;`. **cfg gating is preserved** (`#[cfg(windows)]` on
  `read_clipboard_text`/`paste_projection_matches`, `#[cfg(windows)]`/`#[cfg(not(windows))]` on `reconcile_paste`).
- **Linux-build nuance**: `clipboard.rs` only uses `arboard::` (full path) and
  the prelude — its `use super::*` turned out unused (on Linux `read_clipboard_text`
  is absent under cfg, leaving only `write_clipboard`) and was removed to avoid an
  unused-import hit under `-D warnings` on non-Windows targets.
- The `runtime` module path is unchanged. **805 tests green** (0 failed, 26 `#[ignore]`;
  the count didn't change — a pure move), clippy `-D warnings`/fmt clean. Docs:
  architecture.md §3. **God-object breakup (stages 1–7) complete**: settings/chat/
  orchestrator-tests/notes/db/markdown/runtime; the largest source file dropped from 4966 to
  ≤1175 lines, all impl blocks ≤ ~660.

### Post-M9: co-locating db/markdown tests with their submodules (done)
- **Tail of the god-object breakup** (docs/history/refactoring-god-objects.md §2.5): when
  breaking up stages 5 (db) and 6 (markdown), the domain tests were collected into a
  single `tests.rs`, even though the plan called for distributing them into `mod tests`
  in their own subfiles (following the "tests next to code" convention). Brought in
  line with the plan — a **pure mechanical move** (byte-exact slices of test bodies,
  names unchanged; behavior/types/schema untouched). Screens
  (`settings`/`chat` — tests "via `handle_key`/`render`") and `orchestrator/tests/`
  (already split by feature) were deliberately left as-is; `notes/tests.rs` was left
  untouched (many cross-tool integration tests save→recall→cite with no single
  "home"); `runtime/tests.rs` is small (238 lines) — not worth splitting.
- **db** (`shared/storage/db/tests.rs`, 591 → removed): 27 tests distributed into `mod tests`
  in the subfiles — `notes.rs` (9), `graph.rs` (5: supersede/links/merge/cite), `self_model.rs`
  (3), `rag.rs` (10). Each `mod tests` — `use super::*` + a local `fn db() ->
  Db { Db::open_in_memory().unwrap() }` (3 lines, self-contained; `Db`'s inherent methods
  resolve by type regardless of file).
- **markdown** (`shared/markdown/tests.rs`, 397 → removed): ~33 tests in `mod tests`
  in the subfiles — `code.rs` (resolve_syntax + highlighting; +`use crate::shared::config::Theme`
  to shadow syntect's `Theme`), `latex.rs` (latex_to_unicode/normalize + math-via-
  render), `table.rs` (table layout), `writer.rs` (base render/lists/quotes).
  Shared render test helpers (`rendered_text`/`rendered_text_w`/`max_line_width`/
  `fg_colors` + the `TABLE_MD`/`CODE_MD` constants) were moved into `#[cfg(test)] pub(super) mod
  testkit` in `mod.rs` (the §2.5 playbook); submodules pick them up via `use super::super::testkit::*`.
- **Visibility**: `mod tests { use super::* }` inside a subfile sees `mod.rs` items
  transitively (the subfile itself does `use super::*`; the descendant sees the
  parent's private imports — the same trick used in the orchestrator split);
  testkit helpers are `pub(super)` (public within the markdown module → visible to
  all its test descendants). Indentation is normalized by
  `cargo fmt`. **808 tests green** (count unchanged — a pure move), 26 `#[ignore]`,
  clippy `-D warnings`/fmt clean. Docs: architecture.md §3.

### Post-M9: tool-catalog metadata — a single source of truth in the `Tool` trait (done)
- **Metadata for profile toggles (semantic group, short label, global
  gate, "enabled by default") moved from centralized match tables in
  `features/tools/meta.rs` into the `Tool` trait itself** — each tool declares them
  right next to its own code (single source of truth). Previously this information
  lived in four `match id { … }` blocks (`tool_group`/`tool_description`/`tool_gate` +
  the `default_tool_ids`/`all_tool_ids` list), which could easily drift from the
  actual set of tools.
- **Trait contract** (`features/tools/mod.rs`): `fn group(&self) -> meta::ToolGroup`
  and `fn ui_label(&self) -> &'static str` — **mandatory** (no default), so a
  new tool **cannot** be added without declaring a group and a label (compile-time
  instead of the previous runtime test with fallbacks `_ => "Other"`/`_ => ""`);
  `fn gate(&self) -> Option<meta::ToolGate>` and `fn enabled_by_default(&self) -> bool`
  — with sensible defaults (`None`/`true`) for the typical case (optional control-/
  self-model tools declare `false`).
- **`ToolGroup` — an enum** (instead of `TOOL_GROUPS: [&str; 8]`): the variant order = the
  display order of groups (`derive(Ord)` → stable toggle sorting without string
  matching); `title()` gives the header, `ToolGroup::ALL`/`group_titles()` enumerate
  them. `meta.rs` now carries only **types** (`ToolGroup`/`ToolGate`/`ToolInfo`), not values.
- **Catalog from the registry**: `ToolRegistry::infos()` takes a `Vec<ToolInfo>` snapshot
  (id + group + label + gate + default) from the live tools; a static `CATALOG: LazyLock<Vec<ToolInfo>>`
  = `standard_registry(&ToolConfig::default()).infos()` (metadata doesn't depend on
  `ToolConfig` → built once, so `effective_tool_ids`, called on every agentic-loop
  round, doesn't rebuild the registry). `default_tool_ids`/`all_tool_ids`/`tool_catalog`
  are derived from `CATALOG` by filter/projection; `effective_tool_ids` takes `ToolGate` from
  the metadata (the dynamic sampling gate by provider remains a separate branch).
- **Settings UI** (`screens/settings/`): `SettingsScreen::tool_catalog()` returns a
  `Vec<ToolInfo>` instead of `Vec<String>`; profile-toggle grouping sorts by
  `ToolGroup` (Ord), the label/gate come from `ToolInfo` (not from `meta::tool_*`). The
  `PTool(idx)` index is still — a position in the stable `CATALOG` (toggle correctness
  is preserved: display and toggling read the same static).
- **Behavior change (cosmetic):** `default_tool_ids`/`all_tool_ids` are now in
  **alphabetical** order (the registry is a `BTreeMap`), rather than the previous manual
  grouped order. This only affects the order tools are presented to the model in new/
  reconciled profiles (`migration.rs`/`reconcile_tools`) and the order of `PTool`
  indices — it doesn't affect correctness (`tool_choice=auto`; the UI resorts by group
  anyway). `all_tool_ids`/`group_titles` are now only used by tests
  (`#[allow(dead_code)]`, kept as a public API).
- **Tests**: `meta` (every tool in the catalog has a group and a non-empty label;
  gates `web_search`/`fetch_url`→Web, `python_exec`→Python, `fs_write`→Fs, `note_save`→
  None — now via `tool_catalog()`); `mod` (the registry contains the whole catalog; the
  fake `Echo` declares `group`/`ui_label`); settings tests moved to `ToolInfo`/
  `tool_catalog()` (the toggle is picked up by the index of an actually enabled tool,
  since the catalog is ordered by id). **808 tests green** (a pure refactor, no engine),
  clippy `-D warnings`/fmt clean. Docs: architecture.md §8.

### Post-M9: SOLID refactor — stage 1: `ToolContext` (dependency bundles + constructor) (done)
- **First stage of the targeted SOLID-improvements track**
  ([docs/history/refactoring-solid.md](../../docs/history/refactoring-solid.md), branch
  `refactor/tool-context-bundles`): eliminated shotgun surgery when adding a
  `ToolContext` field — previously an 11-line literal was repeated in **8 places** (3
  production + 5 test), a new field meant editing all of them. A purely structural
  refactor (behavior unchanged).
- **Three building blocks + a constructor** (`features/tools/mod.rs`, **the flat
  public `ToolContext` fields are preserved** → tool code such as `ctx.storage`/
  `ctx.chunk_params`/… is untouched): `ToolDeps` (shared `Arc`s: storage/engine/
  embedder), `ToolParams` (a snapshot of parameters from the config; `from_config(&AppConfig)` —
  the **single** place that does the mapping) and `TurnInfo` (a snapshot of the turn:
  identity + `Chat` fields); `ToolContext::new(deps, params, turn)` unpacks them into the
  previous fields.
- **Orchestrator**: a helper `tool_deps(&self, backend) -> ToolDeps` (next to the shared
  helpers in `mod.rs`); the three production call sites switched to `new` — reflection/
  consolidation via `ToolParams::from_config(&self.config)`, generation too
  (`self_model_params` is computed separately there — it's still passed into `GenSpawn`
  for injection). **Borrow nuance**: in generation, `chat_mut` holds `&mut self`, so
  `TurnInfo` (the last access to `chat`) is built into a local before `new`, after
  which the `chat` borrow ends and `self.config`/`tool_deps` can be read.
- **testkit**: `ctx_with_storage` via `new`; added `ctx_with_backends`
  (custom engine/embedder — web/subagent/fetch delegate their local
  `ctx_with_engine` to it) and `ctx_with_deps` (a shared bundle for the rag isolation
  test, where two contexts share one storage). All literals in tool tests were removed.
- **Ripple check**: adding a field to `ToolContext` now requires editing **only**
  `ToolContext::new` (verified with a trial field). **808 tests green** (count
  unchanged — a refactor), 26 `#[ignore]`, clippy `-D warnings`/fmt clean. Docs:
  architecture.md §8.

### Post-M9: SOLID refactor — stage 2: background tasks (slot registry + a single done channel) (done)
- **Stage 2** of the SOLID-improvements track
  ([docs/history/refactoring-solid.md §4](../../docs/history/refactoring-solid.md), branch
  `refactor/bg-task-slots`): the family of "silent" background tasks (self-model
  auto-reflection + notes auto-consolidation — a UI-less mini agentic loop via the
  shared runner `tool_loop::spawn_silent_loop`) was maintained by copy-pasting its
  lifecycle — a triplet of fields + a channel + a `select!` arm + a handler per task.
  Prepares the ground for family member #3 (self-model auto-consolidation on a timer,
  roadmap §9.9): adding it will no longer touch `run()`/`Quit`. Purely structural,
  behavior unchanged (error/event texts byte-for-byte).
- **Slot registry** (`app/orchestrator/background.rs`, a new module): `BgSlot { cancel:
  Option<CancellationToken>, failures: u32 }` (a failure streak outlives a single run →
  belongs to the slot, not the task); the key is the existing `BackgroundKind` (given
  `Hash`). Methods on `impl Orchestrator`: `bg_running(kind)` (the "one at a time" gate), `begin_bg(kind, cancel)`
  (sets the "running" flag + the status-bar indicator), `handle_bg_done(kind, result)` (a
  **shared** outcome handler: clearing the indicator, a failure streak → a single error
  at the `BACKGROUND_FAILURE_ALERT` threshold, on **reflection**'s success — `SelfModelChanged`,
  none for consolidation), `cancel_all_bg()` (for `Quit`), `#[cfg(test)] bg_failures(kind)`.
  Error texts are assembled from `kind_label(kind)` ("Auto-reflection"/"Auto-consolidation")
  **byte-for-byte** with the previous ones — tests check exactly those.
- **Orchestrator fields 6 → 2**: `reflect_cancel`/`reflect_done_tx`/`reflect_failures` +
  `consolidate_cancel`/`consolidate_done_tx`/`consolidate_failures` → `bg: HashMap<
  BackgroundKind, BgSlot>` + `bg_done_tx: UnboundedSender<(BackgroundKind, Result<(),
  String>)>`. `consolidate_counts` (the per-chat consolidation cadence) **kept** — it's
  cadence data, not task lifecycle. In `run()`: two channels/two `select!` arms
  → one `bg_done` + one arm; `Quit` — enumerate the tokens → `cancel_all_bg()`.
- **`SilentLoop`** (`tool_loop.rs`) gained a `kind: BackgroundKind` field; `done_tx` now
  sends `(kind, outcome)` instead of a bare outcome. The spawn tails of
  `maybe_auto_reflect`/`maybe_auto_consolidate` were switched to `begin_bg` (sets the
  cancel token + the indicator), the "already running" gates — to
  `bg_running`; `handle_reflect_done`/`handle_consolidate_done` removed.
- **Family boundaries** (untouched): impersonation (its own done channel `(Uuid,
  FinishReason)`, streaming to the UI), RAG indexing (no done channel, progress via
  `RagProgress`), auto-title (`title_tx`, a result carrying the chat id). `gen_state`/
  `rag_cancel`/`imp_cancel` in `Quit` are unchanged.
- **DoD**: the `reflect_*`/`consolidate_cancel|_done_tx|_failures` fields removed; a single bg
  arm in `run()`; `BACKGROUND_FAILURE_ALERT` — the sole consumer of `handle_bg_done`.
  Tests moved onto the new API without renames (`bg_running`/`handle_bg_done`/
  `bg_failures`). **808 tests green** (count unchanged — a refactor), 26 `#[ignore]`,
  clippy `-D warnings`/fmt clean. Docs: architecture.md §3 (module map), §11
  (concurrency).

### Post-M9: SOLID refactor — stage 4: status-bar view-model + canonical runtime helpers (done)
- **Stage 4 (small, targeted)** of the SOLID-improvements track
  ([docs/history/refactoring-solid.md §6](../../docs/history/refactoring-solid.md), branch
  `refactor/status-bar-runtime`): removed the status bar's 10-argument signatures and
  scattered named screen enumerations in runtime. Purely structural (behavior
  unchanged). Did 4a/4b/4d; 4c (grouping `ChatScreen` fields) — **not done**
  (per the plan, only incidentally while touching `chat/`; not worth a standalone PR).
- **4a — status-bar view-model** (`widgets/status_bar.rs`): `render`/`height` carried
  10 arguments each (`#[allow(too_many_arguments)]`). Introduced `StatusModel<'a>` (a
  snapshot: statuses/generating/tokens/context/context_exact/mouse_scroll/background); `render`
  → 4 parameters, `height` → 3, both `allow`s removed. `ChatScreen` builds the snapshot via one
  private helper, `status_model()` (`chat/render.rs`) — a new indicator = a field +
  one place to fill it, no signature churn. **Borrow nuance**: the helper borrows all
  of `&self`, and between `height` and `render` there's `&mut self.feed_view` — so the
  snapshot is built as a temporary at each of the two call sites (a short-lived borrow
  that doesn't overlap the `&mut`), rather than held in a local. Tests were switched to
  a `StatusModel` literal (via the test helper
  `model()`; the `ready()` snapshot is bound to a local — otherwise the temporary would
  outlive the borrow).
- **4b — canonical screen enumerations** (`app/runtime/`): the palette broadcast (theme/
  compat-mode change) and paste routing were consolidated into methods on
  `ActiveScreen` itself (`set_palette`/`handle_paste`, next to the enum in `mod.rs`) —
  the `Settings` arm in `apply_event` keeps its own `refresh` (broader than the
  palette), other screens are handled by `other.set_palette(...)`. Intent
  taken off the active screen is now dispatched through a single
  `enum AnyIntent { Chat|List|Settings|SelfModel }` +
  `dispatch_any` (single ownership instead of 4 parallel `Option`s and 4 nearly
  identical `if` blocks — the previous shape was a workaround for `active`/`screen`
  borrow conflicts).
- **4d — clipboard out of `apply_event`**: the write + confirmation/error routing were
  moved into `deliver_clipboard(screen, active, clipboard, text)` (`dispatch.rs`) —
  `apply_event` no longer knows about `arboard` (the `CopyToClipboard` arm is a single call).
- **Deliberately left as-is**: the per-event `match active` in `apply_event`
  (ServerStatus/ChatList/ChatActivated/…) — this is event logic with different
  semantics per screen, not a "screen enumeration"; enum dispatch is idiomatic here
  (plan §6: "the `match` over screens doesn't go away — that's not the goal").
- **DoD**: status-bar signatures ≤4 with no `allow`; no named screen enumerations in
  `dispatch.rs`/`input.rs` outside of `ActiveScreen`/`dispatch_any`. **808 tests
  green** (count unchanged — a refactor), 26 `#[ignore]`, clippy `-D warnings`/fmt
  clean. Docs: architecture.md §9.

### Post-M9: SOLID refactor — stage 3, step 3.1: settings-field description in `FieldRow` (done)
- **Step 3.1** of stage 3 (settings-field descriptors,
  [docs/history/refactoring-solid.md §5](../../docs/history/refactoring-solid.md), branch
  `refactor/settings-field-descriptors`): the aspects of a single settings field were
  smeared across five match sites; 3.1 co-locates the **description** at the row-
  building site (SRP groundwork). Purely structural, behavior unchanged.
- **`FieldRow`** gained `description: Option<&'static str>` + a builder `describe(d)`
  (`row(...).describe("…")`). The 190-line `field_description(id)` match was **removed**;
  the texts moved: section-level ones (Tools/Memory/Interface) — as inline literals in
  `catalog.rs`; texts shared across several construction sites (engine mode, cloud
  model name, API-key env-var name, subsection selector) — as `const DESC_*` in `helpers.rs`;
  `-ngl`/`--jinja` (texts **differ** between assistant and impersonation) — as new fields
  `ngl_desc`/`jinja_desc` in `ManagedFieldIds`; other managed fields (no-mmap/flash-attn/
  spec-*, shared by both engines) — inline in `managed_rows`; sampling — `sampling_row`
  sets `p.description()` (the `SamplingParam::description` source is untouched).
- **Consumers** (the bottom panel in `render.rs`, the search trap `collect_hits` in
  `helpers.rs`) now read `row.description` instead of calling `field_description(f.id)`.
- **Nuance** (a consequence of co-location): a description now exists only for
  **visible** rows (speculative-decoding draft fields — only when
  `spec_type=draft-*`). Behavior parity was preserved: `XModelName`/`IxModelName`/`EModelName` in
  external mode also get a description (the old match matched by id regardless of
  mode). The `field_description(id)` tests were switched to a helper `field_desc(&screen, id)`
  (builds the section/subsection fields and looks up the row; the draft test enables
  `spec_type=draft-mtp`) — test names unchanged.
- **DoD of the step**: label + group + a field's description live in one place;
  `field_description` removed. **808 tests green** (count unchanged — a refactor), 26 `#[ignore]`, clippy
  `-D warnings`/fmt clean. Steps 3.2 (value access via `field_spec`) and 3.3
  (Choice options) — next per the plan.

### Post-M9: SOLID refactor — stage 3, steps 3.2/3.3: a field-value access table (`field_spec`) (done)
- **Steps 3.2 (core) + 3.3** of stage 3
  ([docs/history/refactoring-solid.md §5](../../docs/history/refactoring-solid.md), branch
  `refactor/settings-field-descriptors`): access to a config field's value in
  settings was smeared across **four** `FieldId` match sites (`toggle_field`/`cycle_field`/
  config branches of `apply_text`/`field_num_kind`) + Choice options (`choice_menu`).
  Consolidated into **a single table**. Purely structural, behavior unchanged — a safety
  net of ~134 settings tests.
- **New module `screens/settings/spec.rs`**: `enum Access { Toggle(fn(&mut AppConfig))
  | Text(fn(&mut AppConfig,&str)) | Choice { cycle: fn(&mut AppConfig,i32), options:
  fn(&AppConfig)->(Vec<String>,usize) } }` + `FieldSpec { access, num: Option<NumKind> }`
  + a **single** `field_spec(id) -> Option<FieldSpec>` covering all config fields.
  fn pointers (not closures) — `'static`, no capturing; **mode-based routing**
  (external → `external.*`, cloud → `cloud_mut()`) lives **inside** the setter (which
  has access to the whole `AppConfig`). Per-field parsing semantics (opt/`if let Ok`/
  `parse_opt_num`/the host special case/the dictionary list) live in that field's setter,
  byte-for-byte with the previous arms.
- **Consumers reduced to the table**: `toggle_field` (`Access::Toggle` + `save_config`;
  `PTool` — the previous path), `cycle_field` (`Access::Choice.cycle`; subsections/`S`/`IS`/
  `PSelect` — the previous path), `apply_text` (`Access::Text.set`; `S`/`IS`/profile
  fields — the previous path), `field_num_kind` (`field_spec.num`; `S`/`IS` — `p.num_kind()`),
  `choice_menu` (`Access::Choice.options` — this is **3.3**; `S`/`IS`/`PSelect` —
  the previous path).
- **Scope boundaries** (outside the table, the previous path in apply.rs): sampling
  parameters `S(p)`/`IS(p)` (their own `SamplingParam` descriptor), profile fields (over
  `profiles[idx]`, not `AppConfig`), subsection selectors and `PSelect` (navigation).
  The remaining `match id`s in apply/choice/helpers only serve these out-of-scope
  fields.
- **Step (c) (reset_field/the `•` marker via `get`-comparison) deliberately skipped**:
  `reset_field` and the marker already work generically through `default_fields()`
  (comparing `fields()` values against the default, **no per-field arm**) — there's no
  collapse target there; adding `get` to `Access` for this alone would be extra
  indirection. Catalog builders (label/value/description from 3.1) are untouched.
- **DoD of the stage**: `field_description` (3.1) + the config arms in `toggle_field`/`cycle_field`/
  `apply_text` + `field_num_kind`'s config branch removed; for config values, `FieldId`
  is now handled by **one** structural match (`field_spec`) + catalog builders — instead of
  the previous six. **808 tests green** (count unchanged — a refactor), 26 `#[ignore]`,
  clippy `-D warnings`/fmt clean. Docs: architecture.md §3. **The targeted
  SOLID-improvements track (stages 1–4) is complete.**

### Post-M9: the child-loop seam — `TurnShared`, `RequestEnv`, `sanitize_title` in `shared` (done)
- **Why**: PR 1 of the sub-agent track
  ([docs/research/subagent-chats.md](../research/subagent-chats.md) §7), cut as its
  own branch so that the behaviour change that follows lands on a diff that is
  only the behaviour change (AGENTS.md §2: a mechanical refactor and a behaviour
  change don't mix). Three seams, no behaviour change, test count unchanged.
- **`TurnLoop` → `TurnShared` + `TurnLoop<'a>`** (`app/orchestrator/generation.rs`).
  What every loop of one turn shares — backend, registry, the UI sender, the
  confirmation receiver and `allowed_for_turn`, the generation id, the image
  limits, the round limits, engine mode/model name, the two locale/flag
  snapshots — moved into `TurnShared`, owned by the generation task; the loop
  keeps what is its own (request, context, cancellation token, allowed set,
  accumulators, counters, `pending_new_bubble`) plus a `depth` (0; read by the
  sub-agent run in PR 2, hence a pointwise `allow(dead_code)` with the reason).
  A child loop will be the *same type* over `&mut *self.shared`, borrowed from
  the parent for the duration of the call — sound because the parent is
  suspended inside `execute_call` while the child runs. The alternative, a third
  loop beside `tool_loop.rs`, was rejected in the research: every one of the
  main loop's behaviours (confirmation, thinking signatures, control tools,
  effects, images) is exactly what a tool-using sub-agent needs.
- **`build_request` → `RequestEnv` + `build_request_in`** (`orchestrator/request.rs`).
  The environment the system prompt is assembled around — attachments, the
  attached project, the compaction view — is now a struct borrowed from the
  chat (`RequestEnv::of`), and the assembly takes persona, messages and
  environment separately. `build_request` keeps its signature and is a one-line
  wrapper, so the two call sites and the request tests did not move. A
  sub-agent's request is its own persona and messages over its **parent's**
  environment (research §3.3), which the old `&Chat`-only signature could not
  express without a fake `Chat`.
- **`sanitize_title`/`MAX_TITLE_LEN` → `shared/title.rs`**, with their four tests.
  The chat-file migration step of PR 3 names a synthesized transcript with the
  same sanitizer the list uses, and `shared/storage/schema.rs` cannot reach
  `features` (FSD). Call sites (`widgets/chat_list.rs`, `screens/chat/commands.rs`,
  the chat-screen tests) point at `shared` directly — no re-export, no
  indirection; `features/rename_chat.rs` keeps the digest and the generated-title
  cleaner and imports the sanitizer.
- **Live run**: not required — a pure refactor; `cargo fmt`/clippy `-D warnings`
  clean, **2447 tests green, 106 `#[ignore]`** (four tests moved, none added or
  removed).

### Post-M9: the dialogue driver off `TurnLoop` — an explicit `DialogueCtx` (done)
- **Why**: the background-dialogue track (its design doc,
  `docs/research/background-dialogues.md`, arrives with the track itself —
  fork F6(b), the user's decision 2026-09-05) needs the scene runnable from a
  task that has no turn. The driver was a `TurnLoop` method and reached into
  the loop for five unrelated things — `ToolContext`'s locale and sampling,
  the parent chat's persona, the turn's live request tail (from which the
  director's brief is folded) and the turn's cancellation token — while
  ignoring the loop's other twenty fields. AGENTS.md §2 keeps a mechanical
  refactor out of a behaviour change, so the lift ships alone, ahead of the
  track.
- **What**: those five become an explicit `DialogueCtx` (plus the nesting
  depth, so the guard keeps firing where it did, after the arguments are
  parsed), the whole `impl TurnLoop` block that held `run_dialogue` and the
  ten `dialogue_*` methods becomes `impl DialogueCtx`, and `TurnLoop::run_dialogue`
  stays as the six-line wrapper that names what the scene needs and delegates
  to `DialogueCtx::run`. The context is immutable — only `DialogueState` was
  ever written — so every receiver becomes `&self`. Two helpers moved with
  the driver: `progress` (a copy of the loop's, keyed by the same turn id)
  and `dialogue_chip`, whose only caller the scene has always been.
  `finalize_message` now takes the `SamplingConfig` it records instead of the
  whole `ToolContext` — the one thing it read — which is what let the driver
  stop carrying a tool context it otherwise had no use for; the three call
  sites pass `&ctx.effective_sampling`, the value they passed inside the
  context before.
- **The gate**: no test changed. 2824 unit green (the same number, the same
  names), `cargo fmt`/`clippy` clean, and `dialogue_e2e_live` re-run against
  the LAN stack — see the PR. A refactor whose diff is +118/−60 in one file
  and whose test suite is untouched is the shape this was aimed at.

### Post-M9: the decoding order out of `http_text` — `shared::text_decode` (done)

- **Why.** The order that decides a fetched page's encoding
  ([docs/research/page-charset.md](../research/page-charset.md) §3) is a function of
  bytes and of what was declared about them, and its next caller reads files, not
  responses: the local-files research
  ([docs/research/local-file-encoding.md](../research/local-file-encoding.md), the
  user's decision F5b of 2026-09-11) found `code_edit` rewriting a windows-1251 file
  with `EF BF BD` for every letter and every other reading path decoding lossily or
  refusing. A module named for HTTP that decodes files on disk would mislead whoever
  looks for it next, and AGENTS §2 keeps a mechanical move out of the PR that changes
  behaviour — so the move lands first, on its own.
- **What moved.** `EncodingSource`, `decode`, `Utf8Evidence`, `reads_alike`, `detect`
  and the declaration readers — `header_charset`, `document_charset` with the XML
  declaration, the `<meta>` prescan and its attribute walker — went to
  `src/shared/text_decode.rs` unchanged, with their tests: the 28-row decision table
  and the GBK premise. `http_text` keeps what is the transport's: `read`, the
  content-coding undo, the header reader, the host's TLD label, the test kit, and the
  tests of those plus the live corpus, which now imports from `text_decode`. Five
  items became `pub(crate)` for `read` and that corpus; no signature changed.
- **One test input changed**: the TLD test encoded the moved table's Russian prose and
  now encodes a sentence of its own — its point is the label's form, which the prose
  never touched.
- **The gate**: 3050 unit green and 158 ignored, the numbers `main` has, two of the
  tests under a new module path; `cargo fmt`/`clippy` clean. No live run: no behaviour
  moved with the code.
