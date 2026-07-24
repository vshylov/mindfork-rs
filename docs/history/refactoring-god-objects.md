# Refactoring plan: breaking up god objects (2026-07)

Design plan for this track. Status: **done** (stages 1–7 done). Candidate for
moving to `docs/history/` (like refinements.md and others).

**Progress:** all seven stages done and merged into one branch `refactor/god-object-split`
(originally each was a separate branch from `main`). Summary table and details — §6;
full per-stage log — in [CLAUDE.md](../CLAUDE.md).

## 1. Context and diagnosis

Architecture overall is healthy: FSD layers are respected (`screens`/`widgets` don't
know about `app`), the "sole owner of `Chat`" invariant holds, the orchestrator is
already stratified (Phases 1–3: `orchestrator/` directory + `EngineManager`/
`SaveQueue`/`RestartQueue`/`GenState`). The debt is concentrated not in architecture
but in **file granularity**: several modules have grown into monolith files with
800–2000-line impl blocks, where every new feature adds "one more method." This
already produces classic god-object symptoms: any change goes through one file,
review diffs drown, navigating 4–5k lines is slow.

Measurement (2026-07-07, `main`, total lines / of which code before `mod tests`):

| File | Total | Code | Tests | Symptom |
|---|---:|---:|---:|---|
| `screens/settings.rs` | 4727 | ~3790 | ~940 | one `impl SettingsScreen` ≈ 2040 lines + 1100 lines of free helpers; 6+ responsibilities |
| `app/orchestrator/tests.rs` | 3207 | — | 3207 | test monolith: ~150 tests for every orchestrator feature in one file |
| `screens/chat.rs` | 2445 | ~1585 | ~860 | `impl ChatScreen` ≈ 1110 lines; screen + 4 popups + impersonation + RAG banner + spellcheck |
| `features/tools/notes.rs` | 2114 | ~1230 | ~880 | 9 tools + self-note subsystem + consolidation overviews + utilities |
| `shared/markdown.rs` | 1863 | ~1600 | ~265 | three independent subsystems: walker/Writer, tables, LaTeX converter |
| `shared/storage/db.rs` | 1537 | ~1075 | ~460 | one `impl Db` ≈ 820 lines, ~45 methods, 4 domains (notes/graph/self_model/RAG) |
| `widgets/input_box.rs` | 1496 | ~940 | ~560 | one cohesive widget, but impl ≈ 770 lines |
| `app/runtime.rs` | 1050 | ~860 | ~190 | loop + paste batching + clipboard + 4 dispatch functions |

The hot-spot claim is backed by the log: the last three PRs (#117–#119) were all
in `screens/settings.rs`; the memory tracks (notes/self-model) constantly touch
`notes.rs` and `db.rs`.

**Not god objects** (leaving alone): `Orchestrator` (already stratified, 16
fields), `shared/config.rs` (flat data types — many, but no logic),
`entities/self_model.rs` and `features/tools/self_model.rs` (borderline, under
watch), `widgets/message_feed.rs`/`chat_list.rs`, `features/tools/web.rs`
(< ~700 lines of code).

## 2. Method: the proven orchestrator playbook

Breaking up `app/orchestrator.rs` (≈1.8k code + 1.5k tests → a directory of 15
files) has already been done in this repo and yielded working rules. Every stage
below is **the same purely mechanical move**, with no change to types, fields,
channels, or behavior:

1. **File → directory module.** `foo.rs` → `foo/mod.rs` + subfiles. The module's
   public path doesn't change (`crate::screens::settings::SettingsScreen` stays
   put); outward-facing — re-exports from `mod.rs`.
2. **The struct and contract stay in `mod.rs`.** Methods move into separate
   `impl X` blocks in files grouped by responsibility (Rust allows many impl
   blocks for one type across different files of a module).
3. **Visibility.** Private struct fields from `mod.rs` are visible to child
   modules (Rust visibility rule: descendants see the ancestor's private items) —
   no need to make fields `pub`. Methods called from another file get `pub(super)`
   pointwise.
4. **Imports are pointwise**, no `use super::*` outside tests (otherwise dangling
   imports under `-D warnings`).
5. **Tests.** Where tests are domain-scoped (notes, db, markdown) — distribute
   into `mod tests` of their subfiles (the "tests live next to the code"
   convention); where an entire screen is tested through `handle_key`/`render`
   (settings, chat) — a single `tests.rs` submodule (orchestrator precedent).
   Shared fixtures go in `#[cfg(test)] mod testkit` in `mod.rs`. Test function
   names don't change (the CLAUDE.md log references them).
6. **Gates on every PR**: `cargo fmt`, `cargo clippy --all-targets -- -D
   warnings`, `cargo test` — green; test count hasn't dropped (794 at the time).
7. **One stage — one PR**, the split isn't mixed with functional changes. The
   move commit is separate from small visibility fixes — the diff reads as a
   pure move.
8. **Docs**: update the module map in architecture.md §3 + a CLAUDE.md log entry
   (per-PR convention).

Thresholds ("yellow zone", entry into the plan when violated + expected churn):
**~1000 lines of code** per file, **~500 lines** per impl block.

## 3. Stages

Each stage is independent; order is by (size × heat), but it can be reordered.
Effort estimate: 0.5–1 session per stage, settings — 1–2.

### Stage 1 — `screens/settings.rs` → `screens/settings/` (priority 1)

The largest file in the repo and the hottest (6-stage redesign + the last three
PRs). Responsibilities already split cleanly along existing method boundaries:

```
screens/settings/
├─ mod.rs           SettingsIntent, Section/Subsection/ModelTab/Focus, struct
│                   SettingsScreen, new/refresh/set_server_statuses, top-level
│                   handle_key dispatcher, move_section, palette()          (~350)
├─ catalog.rs       the "form model": FieldId, FieldKind/FieldRow, NumKind,
│                   SamplingParam + SAMPLING_PARAMS, section catalogs
│                   (model_fields_for/sampling_fields_for/tool_fields/
│                   memory_fields/interface_fields/profile_fields_for),
│                   row/grouped/text_row/num_row/sampling_row, managed_rows/
│                   cloud_rows + ManagedFieldIds, label functions, tool_catalog,
│                   is_subsection/is_profile_field                       (~950)
├─ descriptions.rs  field_description (≈190 lines), gate_hint            (~230)
├─ apply.rs         mutations: toggle_field/toggle_profile_tool/cycle_field/
│                   cycle_sampling_field/apply_text/apply_profile_text/
│                   apply_sampling_text/save_config/save_profile,
│                   reset_field/default_fields, validation (field_num_kind/
│                   field_validation_error), cycle_* (mode/imp/theme/opt_bool/
│                   reasoning), gate_disabled/sampling_cloud_provider,
│                   parse_opt*/parse_list/parse_breakers/decode/encode   (~750)
├─ editor.rs        Editor, handle_editor_key/handle_paste/field_seed,
│                   multiline_popup_height                               (~150)
├─ choice.rs        ChoiceState, choice_menu/open_choice/apply_choice/
│                   handle_choice_key, SERVER_MODES/IMP_MODES/THEMES,
│                   index_menu/flash_menu/spec_menu/sampling_choice_menu (~200)
├─ search.rs        SearchHit/SearchState, build_search_index/collect_hits/
│                   open_search/search_filter/jump_to_selected/
│                   handle_search_key                                    (~250)
├─ render.rs        render/render_menu/render_fields/render_choice/
│                   render_search, tab_strip_line/header_line/
│                   render_field_line/server_status_chip/value_text,
│                   section_label_col/label_width/truncate_to_width/
│                   span_width/centered_rect*                            (~800)
└─ tests.rs         all screen tests (through handle_key/render)         (~940)
```

External surface is minimal: only `app/runtime.rs` imports from the module
outside (`SettingsIntent`, `SettingsScreen`) — re-exported from `mod.rs`.

Note: `catalog.rs`/`apply.rs`/`render.rs` are linked through `FieldId` — that's
fine (id is a shared vocabulary of the form). Splitting `FieldId` by section is
**not** needed.

### Stage 2 — `screens/chat.rs` → `screens/chat/` (priority 1)

`ChatScreen` — the second hottest: every UI feature goes through it. Popup
states are already extracted into structs (`SuggestPopup`, `ImpersonationState`,
`RagBanner`) — only splitting methods remains:

```
screens/chat/
├─ mod.rs            ChatIntent, struct ChatScreen, new + snapshot setters/
│                    getters (set_settings/set_server_status/set_chat_list/
│                    set_profile_list/spellchecker/background flags)     (~330)
├─ feed.rs           projecting AppEvent into the feed: activate_chat/rename_chat/
│                    push_user_message/begin_generation/push_chunk/
│                    push_thoughts/push_tool_call/continue_assistant/
│                    rewrite_assistant/set_token_usage/finish_generation/
│                    push_error/push_note/restore_input/take_feed_scrolled/
│                    feed_msg_has_vs16                                   (~350)
├─ impersonation.rs  ImpersonationState + begin/push/finish/is_impersonating (~120)
├─ rag.rs            RagBanner + set_rag_progress/is_rag_active/
│                    format_rag_sources                                  (~130)
├─ input.rs          handle_key/handle_paste/handle_mouse,
│                    mark_input_changed/take_dirty_draft/input_is_command/
│                    maybe_recheck_spelling, request_new_chat/
│                    handle_profile_overlay_key, trigger_destructive     (~420)
├─ popups.rs         SuggestItem/SuggestPopup, ConfirmAction, HELP_KEYS,
│                    open_suggestions/handle_suggest_key/apply_suggestion,
│                    handle_confirm_key, handle_emoji_key,
│                    render_help/render_suggest/render_confirm           (~380)
├─ render.rs         render, model_meta, visual_line_count, centered_rect (~220)
└─ tests.rs          all screen tests                                    (~860)
```

Note: `handle_key` is a modality router (help → confirm → suggestions →
emoji → profile overlay → normal input). During the split **do not reorder
branches** — the order is the modality contract.

### Stage 3 — `app/orchestrator/tests.rs` → `app/orchestrator/tests/` (cheap, ~0 risk)

3.2k lines, ~150 tests for every feature in one file — slows down every
orchestrator change. Split mirroring the source modules:

```
app/orchestrator/tests/
├─ mod.rs            shared fixtures (testkit: spawn_orch, environment,
│                    MockSupervisor wiring) + mod declarations
├─ generation.rs · chats.rs · profiles.rs · settings.rs · title.rs
├─ impersonation.rs · rag.rs · reflection.rs · consolidation.rs · self_model.rs
└─ live.rs           all #[ignore] e2e_live smokes (run as a batch)
```

The module path doesn't change (`mod tests;` in `orchestrator/mod.rs` already
exists). Pure cut-paste; the only work is pulling shared helpers into `mod.rs`.

### Stage 4 — `features/tools/notes.rs` → `features/tools/notes/` (priority 2)

The hottest of the `features` files (memory tracks will keep going: vec0
groundwork, citation nudges). Currently in one file: 9 tools, the self-note
subsystem, consolidation overviews, pure utilities.

```
features/tools/notes/
├─ mod.rs            ID constants, thresholds (CONSOLIDATE_SIMILARITY etc.),
│                    SELF_NOTE_TAG, is_self_note, parse_id/parse_tags/clip,
│                    pub-use of all tools and pub(crate) helpers
├─ save.rs           NoteSave, create_note, similarity gate, ensure_note_vectors
├─ recall.rs         NoteRecall, list_user_notes, semantic path,
│                    related_block, cited_sources_block, format_notes
├─ edit.rs           NoteRevise, NoteSupersede, NoteMerge
├─ graph.rs          NoteLink, NoteNeighbors
├─ cite.rs           NoteCiteSource
├─ overview.rs       ConsolidateNotes, build_consolidation_overview,
│                    build_self_consolidation_overview
├─ self_notes.rs     self_notes_recent/self_notes_relevant/self_related_block/
│                    migrate_self_narrative, cosine
└─ (tests — in mod tests of the corresponding subfiles; shared ctx fixtures —
    #[cfg(test)] testkit in mod.rs)
```

External surface is wide (orchestrator, `self_model.rs`, `rag.rs`,
`tools/mod.rs`, `meta.rs` all call `notes::*` — constants, `is_self_note`,
`create_note`, `self_notes_*`, overviews) — all preserved via re-exports from
`mod.rs`, external `use` sites untouched.

### Stage 5 — `shared/storage/db.rs` → `shared/storage/db/` (priority 2)

One `impl Db` with ~45 methods across 4 domains. Split by data domain:

```
shared/storage/db/
├─ mod.rs            struct Db, open/open_in_memory/from_conn,
│                    register_sqlite_vec, migrate() (schema), ensure_vec_table/
│                    vec_dim, shared helpers (row_to_note/parse_uuid/parse_dt/
│                    cosine/norm_path)
├─ notes.rs          note_insert/list/get/update/delete/is_active +
│                    note_vector_upsert/note_search_semantic/
│                    notes_missing_vectors/notes_with_vectors
├─ graph.rs          note_link_insert/count/neighbors/links_all/links_retarget,
│                    note_supersede_mark, note_cite_source_insert/
│                    note_cited_sources/notes_citing_source
├─ self_model.rs     self_model_get/upsert/update + *_conn helpers
└─ rag.rs            rag_insert/search/count/delete_*/source_*/stored_sources/
│                    list_sources/dimension/other_profiles_have_docs/
│                    reset_vectors + delete_matching/delete_sources_matching +
│                    rag_source_exists
```

Keep the schema (`migrate()`) as one piece in `mod.rs` — the entire DB is
visible at a glance from it. Split tests across domain subfiles.

### Stage 6 — `shared/markdown.rs` → `shared/markdown/` (priority 3)

Cold right now, but the seams are ideal — three independent subsystems:

```
shared/markdown/
├─ mod.rs      render/render_with + styles (heading/code/link/blockquote)
├─ writer.rs   Writer (walker over pulldown-cmark events), heading_number
├─ code.rs     syntect: resolve_syntax/canonical_lang/code_theme/
│              build_code_theme/gray/scope_item/to_syn
├─ table.rs    TableBuilder, render_table/fit_columns/border_line/render_row/
│              pad_cell/clip_line, MIN_COL/MAX_MIN
└─ latex.rs    normalize_delimiters + latex_to_unicode + command tables
               (~570 lines — the largest isolable chunk)
```

### Stage 7 — `app/runtime.rs` → `app/runtime/` (priority 3)

Not critical yet (~860 code), but the seams are clean and the file grows with
every screen:

```
app/runtime/
├─ mod.rs        run/run_loop, ActiveScreen, SpellLoader, TICK
├─ input.rs      PASTE_BURST/PASTE_GAP, collect_press/paste_char, Chunk/
│                chunk_batch/process_input_batch, reconcile_paste/
│                paste_projection_matches (all Windows paste handling)
├─ clipboard.rs  read_clipboard_text/write_clipboard (arboard slot)
└─ dispatch.rs   apply_event + dispatch/dispatch_chat_list/dispatch_settings/
                 dispatch_self_model (Intent → AppCommand)
```

### Stage 8 (optional, as it grows)

- **`widgets/input_box.rs`** — a cohesive widget; split only if it keeps
  growing: `input_box/{mod,edit,nav,render}.rs` (text edits / visual
  navigation / rendering).
- **`shared/config.rs`** — flat types; on growth — `config/{engine,tools,
  memory,interface}.rs` with re-exports.
- **Watch list** (revisit at the next measurement):
  `entities/self_model.rs` (~720 code), `features/tools/self_model.rs` (~655),
  `features/tools/web.rs` (~620), `app/orchestrator/generation.rs` (~700),
  `widgets/message_feed.rs`, `widgets/chat_list.rs`, `screens/self_model.rs`.

## 4. What we deliberately do NOT do (and why)

- **Crate split** — rejected already in ADR 0004 for `shared/api`; same logic
  applies elsewhere: one binary crate, boundaries drawn by modules.
- **Declarative descriptor table for settings fields** (collapsing five
  `FieldId` match sites — catalog/apply/toggle/descriptions/validation — into
  one table of getters/setters-as-closures). The reason settings.rs keeps
  growing is O(fields) across five places, and a table would cure that,
  **but**: it's a rewrite with real regression risk, and the current match
  approach is simple and compiler-checked for exhaustiveness. Revisit only if
  `catalog.rs`+`apply.rs` keep growing faster than the rest after the split.
- **Modal enum for `ChatScreen` popups** (`enum Modal { Help | Confirm | … }`
  instead of independent `Option` fields) — would make the "one modal at a
  time" invariant true by construction, but that's a behavioral change, not a
  mechanical move. Consider it as a separate PR **after** stage 2, if the
  split reveals modality-priority ambiguities.
- **Folding the main generation loop into `tool_loop`** — already decided
  "no" (self-model refinement stage 6): streaming/control-flow/thinking
  signatures don't pay for a shared sink.
- **Further splitting `Orchestrator`** — already stratified (Phases 1–3), 16
  fields, the `Chat`-ownership invariant holds. Leave alone.

## 5. Definition of Done (whole track)

- No file over ~1600 lines total; in touched modules ≤ ~1000 lines of code
  per file and ≤ ~500 lines per impl block.
- Public module paths unchanged (external `use` unmodified, except free
  functions that became sibling methods — there should be none of those).
- 794+ unit tests green after every stage, count hasn't dropped;
  `#[ignore]` smokes untouched.
- `cargo fmt` / `cargo clippy --all-targets -- -D warnings` clean.
- architecture.md §3 (module map) and CLAUDE.md (log) updated at every stage.

## 6. Done

### Stage 1 — `screens/settings.rs` → `screens/settings/` (done)

The 4966-line monolith split into 8 files by a purely mechanical move
(byte-exact slices by line range — zero transcription risk; behavior/types/
fields/the `SettingsIntent` contract unchanged). Result (lines): `mod.rs` 650
(form types + section enums + `struct SettingsScreen` + submodule
declarations), `catalog.rs` 565 (constructor + section/subsection field
builders + gates), `apply.rs` 675 (key handling + editor + toggles/cycles +
saving), `render.rs` 530 (rendering), `helpers.rs` 1130 (free functions — row
builders, descriptions, parsers, enum-value cycles), `choice.rs` 150,
`search.rs` 155, `tests.rs` 1175. The former `impl SettingsScreen` (~2040
lines) split into 4 impl blocks across files (≤ ~660 lines each).

Key visibility rules (cementing the §2 playbook for UI screens):
- Submodules pull `mod.rs` items via `use super::*;` — the glob also pulls in
  the parent's private `use` imports (ratatui/uuid/crate::…), so submodules
  don't duplicate external imports and `mod.rs` doesn't get "dangling" ones
  (everything is "used" transitively → zero warnings).
- Free functions in `helpers.rs` are marked `pub(super)` (otherwise not
  visible to siblings); consumers do `use super::helpers::*;`.
- Private `impl SettingsScreen` methods called from another file are marked
  `pub(super)` (method privacy in Rust is scoped by the defining module).

**Deviations from §3 of the plan:** `descriptions.rs` and `editor.rs` weren't
split out — `field_description`/`gate_hint` stayed in `helpers.rs`, and the
field editor (`handle_editor_key`/`field_seed`) — in `apply.rs` (tightly
coupled to key handling). `helpers.rs` was left a single module of free
functions (1130 lines, but these are independent pure functions, not a
tangled impl block) — further splitting by responsibility (catalog/apply/
render/descriptions) is deferred as low priority. Gates green: **805 tests**
(0 failed, 26 `#[ignore]`), clippy `-D warnings` clean, `cargo fmt --check`
clean.

### Stages 2–7 (done)

All remaining stages were done with the same playbook (§2). The full
per-stage breakdown is in the [CLAUDE.md](../CLAUDE.md) log ("God-object
refactor — stage N" entries); here — a "before → largest file after" summary
and where things landed.

| Stage | File | Before | Largest after | Key layout |
|---|---|---:|---:|---|
| 1 | `screens/settings.rs` | 4966 | 1175 (tests) | mod/catalog/apply/render/choice/search/helpers/tests |
| 2 | `screens/chat.rs` | 2616 | 1034 (tests) | mod/feed/input/popups/impersonation/rag/render/tests |
| 3 | `app/orchestrator/tests.rs` | 3506 | 959 (live) | mod (fixtures) + per feature + live.rs |
| 4 | `features/tools/notes.rs` | 2242 | 1005 (tests) | mod/save/recall/edit/graph/cite/overview/self_notes |
| 5 | `shared/storage/db.rs` | 1665 | 591 (tests) | mod (schema) + notes/graph/self_model/rag |
| 6 | `shared/markdown.rs` | 1995 | 578 (latex) | mod/writer/code/table/latex/tests |
| 7 | `app/runtime.rs` | 1099 | 334 (dispatch) | mod/input/dispatch/clipboard/tests |

Playbook refinements found along the way (locked in as §2 rules):

- **Re-export only when needed.** The external surface of a submodule (e.g.
  `notes::*` — ~30 symbols from the orchestrator/`self_model`/`rag`/`meta`) is
  preserved with a re-export `pub(crate) use <submod>::*` from `mod.rs`. For
  **purely internal** wiring (markdown, runtime) the re-export is **private**
  `use self::{…::*}` — `pub(crate) use` triggers clippy's "glob import
  doesn't reexport anything with visibility pub(crate)" here, since items are
  `pub(super)`, not `pub`.
- **Inherent methods need no re-exports** (stage 5, `db.method()` via the
  `Storage` facade) — they resolve by type regardless of file; compiled on the
  first try.
- **Submodule name collision** (stage 3): the test submodule
  `tests::generation` shadows the source module `orchestrator::generation` —
  references were rewritten to `super::super::generation::`.
- **cfg gating carries over byte-for-byte** (stage 7); `clipboard.rs` only
  uses `arboard::`+the prelude → its `use super::*` was removed (otherwise an
  unused-import on Linux, where `read_clipboard_text` under
  `#[cfg(windows)]` is absent).
- **clippy `-D warnings` as the safety net**: caught an orphaned doc comment
  (stage 4, a prose `///` ending in `;`, mistakenly parsed as an item boundary)
  and a cross-platform unused import (stage 7) — both slipped past `cargo
  build`.

**Track outcome:** the largest source file dropped from 4966 to ≤1175 lines;
no impl block > ~660 lines; public module paths unchanged (external `use`
untouched); **805 unit tests** green after every stage (count unchanged —
pure move), 26 `#[ignore]` smokes untouched; `cargo clippy --all-targets -- -D
warnings` and `cargo fmt --check` clean. DoD (§5) met.
