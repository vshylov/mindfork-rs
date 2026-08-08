# Journal — Memory: self-model, notes, RAG, attachments

The agent's memory organs and the machinery that keeps them integrated rather than merely accumulating: the self-model, notes with their graph, the RAG knowledge base, chat attachments, embeddings and the model-change/reindex machinery.

**Reference documents for this area:** architecture.md §9, spec.md §9.5, §17

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)). They record what was done,
why, what was measured and what was rejected — the reasoning behind the code, not
its current shape. For the current shape read the reference documents named above;
for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (45)

- Post-M9: loading/removing files in RAG via `/rag add|remove` commands (done)
- Post-M9: smart RAG chunking (overlap + markdown) + stitching on retrieval (done)
- Post-M9: RAG — `/rag list`, configurable chunking, `/rag rebuild` (done)
- Post-M9: SelfModel MVP — agent "self-model" (probe, done)
- Post-M9: SelfModel Tier 2-A — narrative insights + contradictions in prose (done)
- Post-M9: SelfModel Tier 2-B — self-model viewer screen (`F3`) (done)
- Post-M9: SelfModel Tier 3-A — narrative/injection parameters in settings (done)
- Post-M9: SelfModel Tier 3-B — auto-reflection (background) (done)
- Post-M9: SelfModel Tier 3-C — manual model editing in the UI (`F3`) (done)
- Post-M9: SelfModel — partial display of a long item on the `F3` screen (done)
- Post-M9: notes connectivity — MVP probe (Tier 1, done)
- Post-M9: notes connectivity — Tier 2 (graph + revision history, done)
- Post-M9: notes connectivity — Tier 3 (consolidation / "sleep", done)
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
- Post-M9: linking the memory organs — Tier 3, Path 1 (cross-organ links) (done)
- Post-M9: linking the memory organs — Tier 3, Path 2 (output mixing) (done)
- Post-M9: linking the memory organs — Tier 3, Path 3 (RAG ↔ notes) (done)
- Post-M9: self-consolidation overview in reflection (Tier 3, narrative as notes) (done)
- Post-M9: self-model summary as a snapshot, not a chronicle (anti-bloat) (done)
- Post-M9: self-model consolidation — stage A1 (background "sleep" on a timer) (done)
- Post-M9: self-model consolidation — stage A2 (summary↔observation semantics) (done)
- Post-M9: self-model consolidation — stage A3-light (interest aging) (done)
- Post-M9: RAG — indexing HTML sources (stage B1a) (done)
- Post-M9: RAG — per-chunk indexing progress (stage B2a) (done)
- Post-M9: RAG — indexing PDF/DOCX (stage B1b) (done)
- Post-M9: RAG — cross-source dedup of search results (stage B2b) (done)
- Post-M9: chat file attachments — stage 1 (`/file attach`) (done)
- Post-M9: chat file attachments — stage 2 (`attachment_read`) (done)
- Post-M9: chat file attachments — stage 3 (`attachment_search`) (done)
- Post-M9: rag_search — the same fragment separation, and a rendering defect it uncovered (done)
- Post-M9: embedding-model change detection (stage 1) (done)
- Post-M9: embedding-model change — stage 2 (re-embedding in place) (done)
- Post-M9: embedding-model change — stage 3 (per-model similarity thresholds) (done)
- Post-M9: per-model input prefixes for embeddings (done)
- Post-M9: self-model injection — per-section budgets (done)

### Post-M9: loading/removing files in RAG via `/rag add|remove` commands (done)
- **Commands in the input box**: `/rag add <path>` indexes a file or directory into the
  active profile's knowledge base; `/rag add <path> -r` (or `--recursive`) — recurses;
  `/rag remove <path>` removes a file or directory (and everything under
  it) from the base. **The word `delete` is deliberately not supported** (users worried it
  would delete the file itself on disk) — only `remove`. Only
  `*.txt`/`*.md` are currently supported. Parsing is pure
  logic in `features/rag_command.rs::parse` (case-insensitive, a path with spaces and
  quotes, the flag in any position; a shared `extract_path`). The screen (`screens/chat.rs`),
  on `Enter`, first tries to parse a command: recognized → `ChatIntent::RagAdd { path,
  recursive }` / `ChatIntent::RagDelete { path }` (doesn't go out as a message, works even
  during generation — the operations are background/fast); a syntax error → a note-hint
  in the feed; otherwise a normal send.
- **Canonical source key + idempotency**: `source` in the DB is a canonical path
  (`rag_ingest::canonical_source` = `fs::canonicalize` without the `\\?\` verbatim
  prefix), the same whether added as a file, added as a folder, or deleted (robust
  to case/separator/relativity). **A repeated `/rag add` of the same file replaces**
  its previous chunks instead of duplicating them: `index_file` calls
  `Db::rag_delete_by_source` (an exact `source` match) before inserting.
- **Deletion** — `Db::rag_delete_under(profile_id, path)`: removes documents at an exact
  path and everything under it (a directory); the `norm_path` comparison is robust to
  `/`↔`\`, a trailing slash, and case (Windows); **doesn't require the file to exist on
  disk** (records of already-deleted files can be cleaned up). Removes from both
  `rag_documents` and `vec0` (`rag_vectors`) by rowid, strictly within the profile
  (isolation). The orchestrator (`handle_rag_delete`) does this **on the spot** (DB only,
  no embedding): if the path exists on disk — the canonical key, otherwise — the entered
  string (normalizing the DB); result — `RagProgress::Removed { chunks }` (0 → "nothing found").
- **Scanning** — `features/rag_ingest.rs::scan` (std::fs, testable on tempdir):
  a file path → itself when the extension is supported; a directory → all supported
  files (recursively with the flag), the result is sorted; nested-directory errors
  are skipped, an inaccessible root is an error. `read_text` strips a UTF-8 BOM.
- **Background indexing** — `app/orchestrator/rag.rs::spawn_rag_ingest` (a separate
  tokio task, doesn't block the orchestrator): scan → an embedder precheck (a fast
  bail-out if RAG isn't configured) → reads/chunks per file (reusing
  `features::tools::rag::chunk_text`, now `pub(crate)`)/embeds/writes with
  **isolation by `profile_id`** (the profile comes from the active chat). Cancellable
  (`rag_cancel: CancellationToken` on the orchestrator): a new indexing run cancels
  the previous one, `Quit` cancels the current one. `Storage` is thread-safe (an internal
  mutex), embedding is async.
- **Progress + spinner**: the `RagProgress` type (`Started/Indexing/Finished/Failed`)
  is defined in `features/rag_ingest.rs` — used by both `app` (emits
  `AppEvent::RagProgress`) and `screens` (renders it), without breaking FSD. The task
  sends progress via `evt_tx` directly (no internal channel — `Chat` state isn't
  touched). `ChatScreen::set_rag_progress` runs a banner line between the feed and the input
  (`files found: N` → `indexing file.md from dir (i/total)`) with a spinner
  (`⠋⠙⠹…`); completion/error clear the banner and leave a summary as a note in the feed.
  The `app/runtime.rs` loop repaints every tick while `screen.is_rag_active()` (the
  spinner animation) — outside of indexing, idle ticks don't repaint (the `dirty` flag,
  see the cursor-blink fix). Contract: `AppCommand::RagAdd → handle_rag_add`. The command
  was added to the help overlay (`F1`/`?`).
- **Command highlighting + no spellcheck**: if the input is recognized as a command
  (`rag_command::parse(...).is_some()`), the whole input box is colored `warning`
  (yellow), and spellcheck doesn't apply to it (commands and file paths aren't words).
  `InputBox::render` accepts a `command` flag; `ChatScreen::input_is_command` computes
  it, and `maybe_recheck_spelling` clears underlines and skips checking for commands.
- **Future work**: other formats (pdf/docx/html), readable-text extraction, `/rag list`
  (showing sources), per-chunk progress — for later.

### Post-M9: smart RAG chunking (overlap + markdown) + stitching on retrieval (done)
- **The chunker was rewritten** (`features/tools/rag.rs::chunk_text`): instead of
  "paragraphs + hard 800-char windows" (word-breaking, no overlap, spawning tiny
  chunks from a single line) — a packer following RAG best practices. Text is segmented
  into atomic units (`segment_units`: a whole paragraph if it fits the target; otherwise
  sentences via `split_sentences`; too-long ones get word windows via `break_long`, and
  a single gigantic word — by character), then `pack_units` packs units into chunks
  up to `CHUNK_TARGET_CHARS=800`, starting each next one with **the tail of the previous**
  (overlap ≤ `CHUNK_OVERLAP_CHARS=150`). Small neighboring paragraphs are grouped into
  one chunk; boundaries fall on words/sentences (never mid-word); `CHUNK_MAX_CHARS=
  1200` is the ceiling for an indivisible run. Lengths are counted in **characters** (`clen`),
  not bytes (Cyrillic). Both `rag_add` and background indexing use it.
- **Semantic markdown chunking** (`chunk_markdown`, for `.md` files): splits on
  ATX headings (`split_sections`, `is_atx_heading`), **protects fenced code
  blocks** (``` and `~~~` — a `#` inside them isn't a heading), prepends each section
  chunk with its heading as a **semantic anchor** (improves retrieval). Inside a section
  it's the same `pack_units` with overlap. A document with no headings → falls back to
  regular `chunk_text`. Dispatch by extension lives in `orchestrator::rag::index_file` (md →
  `chunk_markdown`, otherwise `chunk_text`); the `rag_add` tool (plain text, no format) —
  always `chunk_text`.
- **Stitching on retrieval** (`stitch_hits`, called from `RagSearch::invoke`): since
  overlap is baked in at chunking time, adjacent chunks from the same source have a
  **verbatim-matching** tail/head. `stitch_hits` groups hits by source and
  iteratively merges any two pieces with real overlap (`merge_overlap` →
  `overlap_len` finds the longest suffix of `a` that equals a prefix of `b`, ≥
  `MIN_STITCH_OVERLAP=24` chars, to avoid catching coincidence) into one contiguous
  fragment **with no duplication** — saves context and doesn't confuse the model with
  a repeat. A repeated markdown heading at the start of the second chunk is accounted for
  (stripped before matching, not duplicated). Fragments are ordered by best (minimum)
  distance. **No DB schema/entity changes** (deliberately no `seq` column added):
  adjacency is determined from the overlap text itself — the minimal edit that gives
  exactly the effect this project needs.
- **Future work**: ranking/dedup across sources, configurable chunk/overlap sizes.

### Post-M9: RAG — `/rag list`, configurable chunking, `/rag rebuild` (done)
- **Configurable chunk/overlap sizes** (`config.rag: RagSettings` —
  `chunk_target_chars`/`chunk_overlap_chars`/`chunk_max_chars`, `#[serde(default)]` →
  old `settings.json` files read without migration; defaults = the former constants 800/150/1200).
  The previously hardcoded `CHUNK_*` are removed; instead a `ChunkParams` type
  (`features/tools/rag.rs`, `pub`) which `chunk_text`/`chunk_markdown`/
  `segment_units` take as a parameter. `ChunkParams::from_settings` sanitizes
  input (zero target → default; overlap < target; max ≥ target). Threaded into
  `ToolContext.chunk_params` (for the `rag_add` tool, built from `config.rag` in
  `orchestrator/generation.rs`) and into background RAG tasks. UI — three text fields in
  the "Tools" section of the settings screen (with description tooltips).
- **Storage of source raw text** (`rag_sources(profile_id, source, content,
  created_at)`, PK on `(profile_id, source)`; `CREATE TABLE IF NOT EXISTS` — without
  migration). File indexing (`/rag add`) **replaces** the source
  (`rag_source_upsert`), the `rag_add` tool (accumulates chunks) — **appends**
  (`rag_source_append`). `/rag remove` also clears `rag_sources` (same path predicate).
  Needed for `/rag rebuild` without touching files on disk.
- **`/rag list`** — the active profile's knowledge-base sources (chunk count + date of
  the earliest chunk): `Db::rag_list_sources` (`GROUP BY source`,
  `entities::rag::RagSourceInfo`); orchestrator `handle_rag_list` (in place, no task) →
  `RagProgress::Listed { sources }` → a note in the feed (`format_rag_sources`).
- **`/rag rebuild`** — reindexing via a background task (`spawn_rag_rebuild`): gathers
  sources from the DB, resolves text (stored → else reads the file by path for
  legacy data; unrecoverable ones count as an error), re-chunks/re-embeds with
  current parameters. **Changing the embedding model's dimensionality**: the vector dimension in
  sqlite-vec is one for the whole DB, so on `new_dim != current_dim` the task checks
  `Db::rag_other_profiles_have_docs` — if other profiles use the DB, it refuses with
  a clear message (not overwriting someone else's data); otherwise `Db::rag_reset_vectors`
  (drop the vectors table + reset `meta.rag_dim`) and reindex at the new dimensionality. The shared
  logic for file indexing and reindexing was factored into `index_source`
  (`orchestrator/rag.rs`). Markdown is detected by the source's `*.md` extension.
- **Contract**: `RagCommand::List/Rebuild` (`features/rag_command.rs`) →
  `ChatIntent::RagList/RagRebuild` → `AppCommand::RagList/RagRebuild` → orchestrator
  `handle_rag_list/handle_rag_rebuild`. `reset_rag_cancel`/`active_profile_id`/
  `fail_rag` were moved into shared helpers in `orchestrator/rag.rs`. Commands added to
  the help overlay (`F1`/`?`). **433 tests green**, clippy/fmt clean.

### Post-M9: SelfModel MVP — agent "self-model" (probe, done)
- **A trimmed-down Phase 1 from the idea bank** ([docs/self-model.md](docs/history/self-model.md)) per
  the plan [docs/self-model-mvp.md](docs/history/self-model-mvp.md): a per-profile agent "self-
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

### Post-M9: notes connectivity — MVP probe (Tier 1, done)
- **A shift of memory from accumulation to integration** per the plan
  [docs/notes-connectivity.md](docs/history/notes-connectivity.md). Motive — from a live
  conversation with the model (the "self-aware AI" profile): "inertia lives **not in
  accumulating notes, but in their connectivity**"; "identity — the mode in which I relate to notes
  (what I accept, what I reject, what I rewrite)." Previously notes could only pile up
  (a flat list, substring search, append-only). The probe checks the **behavioral**
  payoff (will the model start rewriting a duplicate instead of writing an almost-copy, will
  semantic search find relevant content that substring search missed) — go/no-go before Tier 2
  (an explicit graph), as with the SelfModel probe.
- **Three MVP pieces** (Tier 1; graph/supersede/merge/consolidation — deferred):
  1. **Embeddings on notes + a semantic `note_recall`**. A side table
     `note_vectors(note_id PK, profile_id, embedding TEXT)` (`CREATE TABLE IF NOT
     EXISTS` → without migration; the vector — a JSON f32 array, **deliberately NOT vec0**: notes
     number in the tens–hundreds, cosine similarity is computed brute-force in Rust — `db::cosine` +
     `note_search_semantic`, isolated by `WHERE n.profile_id`). `note_recall` with a query
     goes the semantic route; **graceful degradation** (like RAG reranking) — the embedder is
     unavailable (`UnavailableEmbedder`)/notes have no vectors → fall back to substring/tag
     matching (`semantic_recall(...) -> Option`, `None` → the previous `note_list`). Tags are a filter
     on top of the ranking.
  2. **`note_revise(id, content)`** — the core of integration: rewrite a note in place
     (`db::note_update`, isolated via `WHERE … AND profile_id`, `updated_at` bumps →
     it bubbles up in the list) + re-embedding (best-effort). A foreign/nonexistent id →
     a text error, not a panic. **In `default_tool_ids`** (safe, DB-only, central)
     → `reconcile_tools` will enable it for existing profiles too.
  3. **Compatibility gate in `note_save`** — after insertion it embeds (best-effort) →
     `note_vector_upsert` → semantically close existing notes get appended to the
     result ("Similar notes … rewrite via note_revise instead of a new entry"),
     so the model **at save time** sees a duplicate/conflict and decides: keep / rewrite
     / don't proliferate. This is the "ability to say no."
- **Backfilling "old" notes** (`ensure_note_vectors`, `db::notes_missing_vectors`):
  notes without a vector (created before the feature, imported, saved while the embedder was
  unavailable at the time) weren't seen by semantic search/the gate — a finding from a live test (the model
  called `note_recall` and didn't see notes it created earlier). The helper transparently
  (at the start of `semantic_recall` and before the `note_save` gate) embeds missing notes in batches
  (best-effort). Effectively a one-time cost per profile — afterward the list is empty and the call is nearly
  free (a single SELECT).
- **Architecture**: tools write **directly** through `ctx.storage` (like `note_save`),
  **without new `ChatEffect` variants** (notes aren't `Chat` state); the invariant "sole
  owner of `Chat`" is untouched. The orchestrator wasn't changed.
- **Tests**: db (`note_update` only affects its own profile; `note_search_semantic` ranks and
  isolates; upsert replaces the vector); tools (the gate surfaces a similar note on save;
  semantic recall finds a non-substring match; revision rewrites in
  place; a bad/nonexistent id; backfilling "old" notes for both recall and the
  `note_save` gate); mod (`note_revise` in the defaults). Semantic tests use `MockEmbedder`
  (a bag-of-characters + L2). **642 tests green**, clippy/fmt clean. A live run/evaluation of the
  probe on a local model — a manual step (a go/no-go criterion in the plan).
- **The probe was verified against a live model** (the model surfaced similar notes) → go,
  moved on to Tier 2 (below).

### Post-M9: notes connectivity — Tier 2 (graph + revision history, done)
- Continuation of Tier 1 ([docs/notes-connectivity.md](docs/history/notes-connectivity.md)):
  note connectivity as an **explicit structure** + integration via supersession/merging.
- **A link graph**: a table `note_links(profile_id, from_id, to_id, relation,
  created_at)` (a PK against duplicates, indexes on from/to, isolated by `profile_id`,
  `CREATE TABLE IF NOT EXISTS` → without migration). Tools `note_link(from_id, to_id,
  relation)` — types `supports`/`contradicts`/`refines`/`relates` (idempotent
  `INSERT OR IGNORE`, checking `note_is_active` at both ends, no self-links) and
  `note_neighbors(id, relation?)` — neighbors in both directions (`UNION ALL` from/to) with
  direction (→/←), excluding superseded ones. `note_recall` mixes in a "Related
  notes" block — **spreading activation** (neighbors of the top-3 hits, excluding those already
  shown/superseded, up to `RELATED_IN_RECALL=5`), so recall surfaces the whole cluster.
- **Revision history with a "scar"**: a table `note_superseded(note_id PK,
  profile_id, superseded_by, superseded_at)`. `note_supersede(old_id, content)` creates
  a new version (`create_note` = insert + embed) and marks the old one superseded;
  `note_merge(ids[], content)` folds ≥2 active notes into one, the originals are superseded.
  Superseded ones are **hidden** from `note_list`/`note_search_semantic`/`notes_missing_vectors`/
  `note_neighbors` (an anti-join against `note_superseded`), but kept for the change trail and
  manual recovery. A simple in-place edit is provided by `note_revise` (Tier 1).
- **Unchanged architecture**: all five tools are DB-only, in `default_tool_ids`
  (`reconcile_tools` will enable them for existing profiles), write directly via
  `ctx.storage`, no `ChatEffect`; the orchestrator untouched.
- **Tests**: db (supersede hides from the list/semantics/`is_active`; links and neighbors in
  both directions + filter by type + idempotency + a superseded neighbor disappearing);
  tools (link→neighbors + an unknown type/self-link; supersede hides the old one, shows the
  new one; merge folds and requires ≥2; recall surfaces a related but non-similar note).
- **Fixes from a live-model stress test** (the model exercised the graph's edge cases):
  - **`note_link` — an honest answer about a duplicate.** Previously repeating the same
    link answered "Link created" both times (there's no duplicate in the DB — there's a PK + `INSERT OR IGNORE`, but
    the message was misleading). Now `note_link_insert` returns "was it created"
    (row count), and the tool answers "The link already existed" when nothing was inserted.
  - **`note_revise` — a graph-integrity warning.** An in-place edit of a node with
    incoming edges could make them wrong (created a `contradicts` link when the note
    denied X; after the revision it asserts X — the edge now lies). `note_revise` now, given
    existing links (`note_link_count`), adds a warning and points to
    `note_supersede` (which preserves the superseded version the links refer to). **Not a
    ban** — the judgment call remains with the model (a cheap edit of unlinked notes doesn't suffer).
  **649 tests green**, clippy/fmt clean.
- Tier 2 merged (PR #86), the live-model stress test passed cleanly → Tier 3 (below).

### Post-M9: notes connectivity — Tier 3 (consolidation / "sleep", done)
- Completion of the track ([docs/notes-connectivity.md](docs/history/notes-connectivity.md)):
  integration happening **outside a single chat** (the project's original goal).
- **`consolidate_notes`** — a read-only entry point (like SelfModel's `reflect`): a
  knowledge-base overview — similar pairs (possible duplicates, pairwise cosine ≥ 0.85),
  `contradicts` links, notes with no links — + a rubric. The overview logic —
  `notes::build_consolidation_overview` (a pure DB read: `db::notes_with_vectors` +
  `db::note_links_all`). It changes nothing itself; the model then calls merge/supersede/
  revise/link.
- **Carrying links over on merge**: `note_merge` moves the edges of the source notes onto
  the merged one (`db::note_links_retarget` — deduped by PK, self-loops dropped), so the graph
  doesn't get orphaned.
- **Auto-"sleep"** (`app/orchestrator/consolidation.rs`, modeled on `reflection.rs`):
  every N replies (`config.notes.auto_consolidate_every`, opt-in, 0=off), a background
  task = a **mini agentic loop** with note tools; it's fed the overview, the model
  consolidates on its own (its calls execute against `Storage`). Gates: the feature is enabled, the
  profile enabled `note_merge`, active notes ≥ 2, the server is `Ready`, one at a time; the
  chat/feed aren't touched. Plumbing mirrors reflection: fields `consolidate_cancel`/`_counts`/`_done_tx`,
  an internal channel, cancellation on `Quit`, invocation in `handle_done`. A "Notes:
  auto-consolidation (every N)" field in settings.
- **Tests**: db (`notes_with_vectors` only active ones; `note_links_retarget` moves/
  dedups/drops self-loops); tools (`consolidate_notes` shows duplicates/
  orphans; `note_merge` carries links to the new note); consolidation (`due`).
  **654 tests green**, clippy/fmt clean. A live evaluation of auto-"sleep" — a manual step.
- **Out of scope (groundwork)**: vec0 as the note count grows; **linking memory organs**
  (notes / self-model narrative / RAG are unconnected) — an observation from the live test, a separate
  large track.

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
- Refinement plan — [docs/refinements.md](docs/history/refinements.md) (6 stages +
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
- Stage 2 of the [refinements.md](docs/history/refinements.md) plan: "me over
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
- Stage 3 of the [refinements.md](docs/history/refinements.md) plan:
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
- Stage 4 of the [refinements.md](docs/history/refinements.md) plan: wiring
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
- Stage 5 of the [refinements.md](docs/history/refinements.md) plan:
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
- Final stage of the [refinements.md](docs/history/refinements.md) plan: a
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
- **Unifying the memory organs** per the [docs/narrative-as-notes.md](docs/history/narrative-as-notes.md)
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
- Continuation of Tier 1 ([docs/narrative-as-notes.md](docs/history/narrative-as-notes.md)):
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
  Tier 2 ([docs/narrative-as-notes.md](docs/history/narrative-as-notes.md)), a
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

### Post-M9: linking the memory organs — Tier 3, Path 1 (cross-organ links) (done)
- **The original long-range connectivity goal**
  ([docs/narrative-as-notes.md](docs/history/narrative-as-notes.md)): link the
  memory organs (self-notes ↔ user notes ↔ RAG). The fork (confirmed by the
  user) — **Path 1: cross-organ edges** (Paths 2, "output mixing via an
  `[about self]` marker", and 3, "RAG ↔ notes", deferred).
- **Key point: the graph mechanics were already cross-organ** — `note_link`/
  `note_neighbors` take any id **without a tag filter**, so linking a self-note
  to a user note was already possible; it just never surfaced anywhere.
  Tier 3 (1) **surfaces** such edges in the output and (2) gives the model
  **addressability** of user notes. The organs remain SEPARATE at
  storage/retrieval level — only an edge **deliberately created** by the model
  surfaces (not "contamination" of the output, unlike Path 2).
- **Surfacing cross-edges** (`features/tools/notes.rs`): `related_block` (used
  by user-facing `note_recall`) no longer skips neighbor "about self"
  observations — it shows them marked **`[about self]`**;
  `self_related_block` (reading the self-model) shows neighbor user notes
  marked **`[note]`**. Regular search/spreading still doesn't pull in
  self-notes (Tier 1's hiding is intact) — the filter was lifted **only** for
  neighbors reached via an explicit edge.
- **Addressability** (`format_notes`): `note_recall` now prints note **ids** —
  otherwise the model couldn't reference a user note in `note_link`. This also
  closes a long-standing gap: `note_link`'s description promised "id from
  note_recall", but the id wasn't printed (notes from recall weren't
  addressable even for a regular graph). New constant `NOTE_RECALL_ID`.
- **Nudge** (`orchestrator/reflection.rs`, `self_model::Reflect`):
  `note_recall` was added to `REFLECT_TOOL_IDS` (gives reflection the ids of
  user notes; it still hides self-notes); the auto-reflection system message
  and the interactive `reflect` rubric suggest linking an "about self"
  observation with an "about the interlocutor" fact (the observation's id from
  `get_self_model`, the note's id from `note_recall`).
- **Invariants intact**: DB-only, isolated by `profile_id`, no `ChatEffect`, no
  schema migrations (the `note_links` graph already existed). **Tests**:
  `recall_shows_note_ids`; `recall_surfaces_cross_organ_self_neighbor_marked`
  (`[about self]` + the primary output without self); `self_related_block_surfaces_cross_organ_user_note_marked`
  (`[note]`); `reflect_tools_include_graph` (+`note_recall`);
  `reflect_message_nudges_cross_organ_linking`. **722 tests green**, 22
  `#[ignore]`, clippy/fmt clean.
- **Smoke — GO** (`cross_organ_link_e2e_live`, Gemma 4 31B + bge-m3): the model
  recorded a fact "about the interlocutor" (`note_save`) and an observation
  "about itself" (`add_insight`), then via `get_self_model` + `note_recall`
  (took the note's id) called `note_link` → creating a **cross-organ edge**
  `contradicts` ("the observation about verbosity ↔ the note about preferring
  brevity"). The model uses cross-links meaningfully; the go criterion — **go**.
- **Deferred**: Path 2 (`[about self]` in general recall, behind a toggle),
  Path 3 (RAG ↔ notes), the self-consolidation overview, vec0 as note counts
  grow.

### Post-M9: linking the memory organs — Tier 3, Path 2 (output mixing) (done)
- **Full output mixing behind a toggle**
  ([docs/narrative-as-notes.md](docs/history/narrative-as-notes.md), Path 2,
  confirmed by the user): `config.notes.recall_includes_self`
  (`#[serde(default)]`, **off** by default) enables showing self-notes
  (`@self`) in the general `note_recall` marked **`[about self]`**. Off =
  Tier 1 behavior (self hidden: memory about oneself ≠ memory about the
  interlocutor) — the reversal of hiding is deliberately kept **behind a
  toggle** so it can be validated safely.
- **Wiring**: `NotesSettings.recall_includes_self` →
  `ToolContext.recall_includes_self` (threaded through all construction sites
  — generation/reflection/consolidation/testkit + 4 tool test contexts). The
  recall paths (`list_user_notes` substring + `semantic_recall`), when the
  toggle is on, no longer drop self-notes; `format_notes` marks them
  `[about self]` and **hides the internal `@self` tag** from the tag display
  (the marker replaces it). Toggle in the settings screen's "Tools" section
  (`FieldId::NotesRecallIncludesSelf`, with a hint).
- **Invariants intact**: DB-only, isolated by `profile_id`, no `ChatEffect`, no
  migrations; behavior unchanged by default. **Tests**:
  `recall_includes_self_notes_when_enabled` (self shows up in both recall
  branches with the marker, without `@self`); default off
  (`partial_json_fills_defaults`). **723 tests green**, 23 `#[ignore]`,
  clippy/fmt clean.
- **Smoke — GO** (`recall_includes_self_e2e_live`, Gemma 4 31B + bge-m3): with
  the toggle on, `note_recall` returned both the user note and the "about
  self" observation marked `[about self]`, and the model's reply **cleanly
  separated the organs** ("— About you: values brevity; — About myself: tends
  toward verbosity") — **no contamination**, the marker works as intended (go
  on mixing safety).
- **Deferred**: Path 3 (RAG ↔ notes), the self-consolidation overview, vec0 as
  notes grow.

### Post-M9: linking the memory organs — Tier 3, Path 3 (RAG ↔ notes) (done)
- **The third memory organ (the RAG knowledge base) is linked to notes/observations**
  ([docs/narrative-as-notes.md](docs/history/narrative-as-notes.md), Path 3): a
  note can **cite a RAG source**, and search works **across both organs**.
  Completes the "narrative as notes" track (Tiers 1–3).
- **Key decision: cite the source's NAME, not the chunk id.** Chunk ids are
  **unstable** — `/rag rebuild` drops and reindexes documents with new uuids,
  and a link to a chunk-uuid would break; the source's name (path/label) is
  stable (stored in `rag_sources`). So the link targets `source`.
- **Schema**: table `note_rag_links(profile_id, note_id, source, created_at,
  PK(profile_id, note_id, source))` (`CREATE TABLE IF NOT EXISTS` → no
  migration, + indexes on note_id/source). DB methods: `rag_source_exists`
  (validation — you can only cite an existing source, in `rag_documents` OR
  `rag_sources`), `note_cite_source_insert` (idempotent, `INSERT OR IGNORE`),
  `note_cited_sources` (forward: a note's sources), `notes_citing_source`
  (reverse: a source's active notes, hiding superseded ones via an anti-join
  on `note_superseded`). `note_delete` cleans up `note_rag_links`. All isolated
  by `profile_id`.
- **Tool** `note_cite_source(note_id, source)` (`features/tools/notes.rs`):
  validates the note is active (`note_is_active`) + the source exists;
  understandable text refusals (not a panic), a "created / already existed"
  message. Added to `default_tool_ids` (like `note_link` — `reconcile_tools`
  enables it for existing profiles), registered. DB-only → passes through
  `effective_tool_ids` via `_ => true`.
- **Bidirectional output** ("search across both organs"): `note_recall` and
  `get_self_model` (`render_self_read`) show a "Source citations" block
  (note→source, `notes::cited_sources_block`); `rag_search` shows a "Notes
  citing these sources" block (source→notes, `notes_citing_source` over the
  sources of the matched passages, deduped by id, self-observations marked
  `[about self]`).
- **Invariants intact**: DB-only, isolated by `profile_id`, no `ChatEffect`, no
  migrations. **Tests**: db (`note_rag_links_bidirectional_and_isolated` —
  forward/reverse + isolation + cleanup on delete; `notes_citing_source_hides_superseded`);
  tool (`note_cite_source_links_and_recall_shows_it` — source/note validation,
  idempotency, showing up in recall); rag_search
  (`search_surfaces_notes_citing_matched_source`). **727 tests green**, 24
  `#[ignore]`, clippy/fmt clean.
- **Smoke — GO** (`note_cite_source_e2e_live`, Gemma 4 31B + bge-m3): the model
  added a document (`rag_add`), then within one turn `rag_search` →
  `note_save` → `note_cite_source`, linking the conclusion "The capital of
  France is Paris" to the "facts" source. In the DB: 1 note cites "facts". The
  full chain (search → note → citation) worked.
- **Deferred**: vec0 as note counts grow; a nudge for `note_cite_source` in
  reflection (currently — in regular turns, where `rag_search` is available).

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
  [docs/history/summary-as-snapshot.md](docs/history/summary-as-snapshot.md).
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
  [docs/history/self-model-consolidation.md](docs/history/self-model-consolidation.md),
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
  [docs/history/self-model-consolidation.md](docs/history/self-model-consolidation.md)
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
  [docs/history/self-model-consolidation.md](docs/history/self-model-consolidation.md)
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

### Post-M9: RAG — indexing HTML sources (stage B1a) (done)
- **First stage of the "RAG: sources and retrieval" track** (design doc
  [docs/history/rag-sources-retrieval.md](docs/history/rag-sources-retrieval.md) §B1a,
  branch `feat/rag-html-sources`): `/rag add` indexes `.html`/`.htm` alongside
  `.txt`/`.md`. **No new dependencies** — readable text is extracted by the
  already-existing `web::extract_readable` (scraper: paragraphs from
  `<article>`/`<main>`, dropping nav/header/footer/aside/scripts;
  `pub(crate)`, covered by tests).
- **An FSD-clean layout** (a deliberate departure from the letter of the design
  doc, which proposed an `extract_text` dispatcher inside `rag_ingest`):
  `features/rag_ingest.rs` stays a **pure file module**
  (`SUPPORTED_EXTENSIONS` += `html`/`htm` + a helper `is_html`), while
  **extraction is dispatched in the `app` layer**
  (`orchestrator/rag.rs::read_source_text`) — otherwise `features →
  features/tools` would be a sideways import, forbidden by FSD; `app`,
  however, may call `features/tools/web`. `read_source_text` (raw via
  `read_text` → for HTML, `extract_readable(&raw, usize::MAX)`, since RAG
  chunks it whole — no truncation) replaced `read_text` at the two points that
  read files (`index_file` and the legacy file read during `/rag rebuild`).
- **`index_source` untouched**: an HTML source isn't markdown → it goes to
  `chunk_text` (extracted text has no heading structure). The single
  user-facing string about formats (`ui.err.rag_no_files`) was updated to
  `.txt/.md/.html`.
- **Tests**: `is_supported`/`is_html` on html/htm/HTML (case-insensitively);
  `read_source_text` extracts an article paragraph and drops
  nav/header/script, passes non-HTML through verbatim (tempdir). **1141 unit
  test green** (+2), clippy `-D warnings`/fmt/i18n gates clean. No live run
  needed (pure extraction without an engine). Track future work: **B1b**
  (pdf/docx — with new crates and a license review), **B2** (ranking/dedup +
  per-chunk progress).

### Post-M9: RAG — per-chunk indexing progress (stage B2a) (done)
- **Second stage of the "RAG: sources and retrieval" track** (design doc
  [docs/history/rag-sources-retrieval.md](docs/history/rag-sources-retrieval.md) §B2a,
  branch `feat/rag-chunk-progress`): previously `RagProgress::Indexing` was
  **per file**, and `index_source` embedded all of a file's chunks **in a
  single batch** — on a large file the banner froze until embedding finished.
  Now embedding runs in **sub-batches** and progress moves as chunks become
  ready.
- **Mechanics**: `RagProgress::Indexing` gained fields
  `chunks_done`/`chunks_total` (0/0 — the file has just started, chunking is
  still ahead → the banner is unchanged from before). `index_source` now
  takes a callback `progress: impl FnMut(usize, usize)` and embeds/writes in
  a loop over sub-batches `chunks.chunks(EMBED_BATCH_CHUNKS=16)`, calling
  `progress(done, total)` after each; `index_file` forwards the callback.
  Both background tasks (`spawn_rag_ingest`, `spawn_rag_rebuild`) emit an
  initial `Indexing{…,0,0}`, then updated chunk counts from the callback. The
  banner (`screens/chat/rag.rs`), when `chunks_total>0`, appends the
  `ui.rag.chunks` suffix (" · chunks N/M").
- **A batching change** (an improvement): a file with ≤16 chunks — still a
  single request (identical result); a large file now sends **bounded-size**
  requests instead of one giant one (bounded memory/request + live progress).
  Chunk order and stored documents unchanged. Requests are **smaller** than
  the previous single batch, hence strictly safer against a real server.
- **Tests**: `index_source_reports_chunk_progress_in_subbatches` (a file with
  > 16 chunks: the first tick `(0,N)`, the last `(N,N)`, monotonicity, all N
  documents written and found by `rag_search`),
  `index_source_empty_content_single_zero_tick`, a banner test (the chunk
  suffix when `chunks_total>0`, none when 0). **1143 unit tests green** (+2),
  clippy `-D warnings`/fmt/i18n gates clean. Live smoke `rag_en_e2e_live` on
  real bge-m3 green (the embed path intact; sub-batching introduced no
  regressions). `index_source` grew to 8 arguments — a targeted
  `#[allow(clippy::too_many_arguments)]` with an explanation (cohesive
  arguments, carving out a bundle just for one parameter would be extra
  churn). Track future work: **B1b** (pdf/docx), **B2b** (ranking/dedup
  across sources).

### Post-M9: RAG — indexing PDF/DOCX (stage B1b) (done)
- **Continuation of the "RAG: sources and retrieval" track** (design doc
  [docs/history/rag-sources-retrieval.md](docs/history/rag-sources-retrieval.md) §B1b,
  branch `feat/rag-pdf-docx`): `/rag add` indexes `.pdf` and `.docx`
  alongside `.txt`/`.md`/`.html`. Extracted plain text → `chunk_text` (no
  heading structure). Extraction is best-effort: a scanned PDF with no text
  layer yields nothing (0 chunks), a corrupt file is skipped with a `warn`
  (the existing ingest loop already handles file errors).
- **New module `features/doc_extract.rs`** (pure functions over bytes,
  external crates, no cross-layer imports — FSD): `extract_pdf(&[u8])` (crate
  `pdf-extract`), `extract_docx(&[u8])` (DOCX = a deflate ZIP; reads
  `word/document.xml` via the already-present `zip` + `quick-xml`, assembling
  text from `<w:t>` matched by **local** name, `</w:p>`→`\n`, `<w:tab/>`→`\t`,
  `<w:br/>`→`\n`). **A quick-xml 0.39 nuance**: entities (`&amp;`) arrive as a
  separate `GeneralRef` event (the name without `&;`) — we reconstruct and
  unescape via `quick_xml::escape::unescape`.
- **Dispatching lives in the `app` layer** (as in B1a):
  `orchestrator/rag.rs::read_source_text` became `anyhow::Result<String>` and
  branches html→`web::extract_readable`, pdf/docx→`doc_extract` (raw
  **bytes** via `fs::read`, not `read_text`), else→`read_text`. `rag_ingest`
  only gained the extensions (`SUPPORTED_EXTENSIONS` += pdf/docx) and helpers
  `is_pdf`/`is_docx`. `index_source` untouched (pdf/docx aren't markdown →
  `chunk_text`).
- **Dependencies**: `pdf-extract 0.12` (MIT, **pure Rust**, no C/`*-sys`;
  pulls in lopdf + font/CFF/CMap parsers + RustCrypto for encrypted PDFs —
  **~28 new transitive crates**, all with permissive licenses in the
  allowlist); `quick-xml 0.39.4` was promoted from transitive to direct
  (pinned to the version in Cargo.lock — no duplicate). `cargo deny` stays
  clean: one ignore was added for `RUSTSEC-2026-0192` (ttf-parser
  unmaintained — an advisory about being unmaintained, not a vulnerability;
  local user files, best-effort) + the rationale for the quick-xml DoS
  advisories was extended (now also covering direct use for DOCX). Allowlist
  licenses unchanged (everything already covered). A warn-level duplicate
  `thiserror 1.x/2.x` (pdf-extract pulls in 1.x) — not a blocker.
- **Tests**: doc_extract (DOCX: paragraphs via `\n`, Cyrillic, `&amp;`
  unescaping, tags don't leak, `<w:tab/>`/`<w:br/>`, corrupt/non-ZIP → `Err`;
  PDF: extraction from a **checked-in fixture**
  `tests/fixtures/hello.pdf` — a minimal valid 587-byte PDF with a correct
  xref, non-PDF → `Err`); rag_ingest (`is_supported`/`is_pdf`/`is_docx`);
  `read_source_text_routes_docx_and_pdf` (routing through extraction, txt
  passed through verbatim). **1151 unit test green** (+8), clippy
  `-D warnings`/fmt/i18n/**`cargo deny`** clean. No live run needed
  (extraction is offline-testable, the embed path unchanged — verified in
  B2a). Future work: **B2b** (ranking/dedup across sources).

### Post-M9: RAG — cross-source dedup of search results (stage B2b) (done)
- **Final stage of the "RAG: sources and retrieval" track** (design doc
  [docs/history/rag-sources-retrieval.md](docs/history/rag-sources-retrieval.md) §B2b,
  branch `feat/rag-cross-source-dedup`): `rag_search` removes near-identical
  passages from **different** sources (the same content indexed from two
  files), so the model isn't fed a repeat.
- **Textual dedup, not embedding-based** (a fork, user's decision): the
  design doc proposed embedding-based dedup+reranking, but `rag_search` is a
  **hot path** (every search), and vec0 **already ranks** by relevance
  (distance). Re-embedding passages on every search would add latency, and
  reranking would largely duplicate vec0's sort. So — deterministic textual
  dedup: **no embedder, no threshold/calibration, no latency**.
- **Mechanics** (`features/tools/rag.rs::dedup_passages`, a pure function):
  after `stitch_hits` (stitching adjacent chunks within a source), passages
  go through dedup — in ranking order, a passage is kept only if its
  normalized text (Unicode lowercased + whitespace collapsed + trimmed) is
  **not wholly contained** in the text of an already-kept passage (a
  `k.contains(&norm)` check covers both equality and subset-inclusion).
  vec0's order is preserved (no re-sorting). Source-agnostic (the main case
  is a cross-source duplicate, but an intra-source repeat is noise too). **A
  lower-ranked superset** of a more relevant passage — both stay (no
  information lost: only a passage that's a subset of a **more relevant**
  already-kept one is dropped). Wired in as `dedup_passages(stitch_hits(hits))`
  in `RagSearch::invoke` before formatting; the format/empty-result path is
  unchanged.
- **Tests**: an identical duplicate from another source → one kept (the more
  relevant one); a subset of a more relevant one → dropped; distinct passages
  → all kept, order intact; dedup is case/whitespace-insensitive; a
  lower-ranked superset → both remain; empty input → empty output. **1157
  unit tests green** (+6), clippy `-D warnings`/fmt clean. No new
  dependencies/config/i18n. No live run needed (pure textual logic; the embed
  path untouched). **The "RAG: sources and retrieval" track (B1a html + B1b
  pdf/docx + B2a per-chunk progress + B2b dedup) is complete.**

### Post-M9: chat file attachments — stage 1 (`/file attach`) (done)
- **A new track** (user request): attach text files to a chat via `/file attach`/
  `/file remove`, with the commands at the top of the help popup's command list.
  Research + plan — [docs/file-attachments.md](docs/file-attachments.md); forks
  **F1–F10 confirmed by the user 2026-07-27**. Branch `feat/file-attachments`
  (stacked on `docs/file-attachments`, the precedent being `feat/generic-import`
  over `docs/plugins-research`).
- **The central question was "inline vs RAG", and RAG loses on three counts** (§3
  of the plan): it **hard-depends on the embedding server** (ADR 0002 — often
  unconfigured, so the feature would simply not work), it is **profile-scoped**
  (a file attached in one chat would surface in every other chat of the profile),
  and — decisively — **retrieval ≠ guaranteed reading**: top-k fragments are the
  wrong model for "summarize this document"/"review this file", and nothing tells
  the model it missed something. Plus `/rag add` **already is** the RAG path, so
  a `/file attach` that indexed into the knowledge base would be a second name
  for an existing command. **The key observation for the user's question "how do
  we make the model read everything it needs": a model doesn't search a store it
  doesn't know exists** — any RAG-backed variant still needs a pointer in the
  prompt. Once a prompt-side block is required anyway, the honest design puts the
  *content* there when it fits.
- **Decision (F1d+F5b): the hybrid in full** — inline block **plus**
  `attachment_read` **plus** a chat-scoped semantic index, delivered as three
  PRs. This stage is the first: entity + commands + extraction + the block +
  modes/budgets + UI. **F11 (where attachment vectors live) — a separate
  chat-scoped index, agreed**: `rag_vectors` is a vec0 table partitioned by
  `profile_id` and applies `k` **inside** the partition, so a `WHERE chat_id`
  join would filter *after* kNN and silently return fewer than `k`; and
  `rag_documents` has no `chat_id` column (`CREATE TABLE IF NOT EXISTS` doesn't
  add columns → a guarded `ALTER` or the first real `DB_STEPS` bump). Reusing the
  profile base would also pollute `/rag list|rebuild|remove` and cross-source
  dedup with per-chat data.
- **Domain**: `Chat.attachments: Vec<Attachment>` (`#[serde(default,
  skip_serializing_if)]` → old chat files read without migration, no schema bump,
  ADR 0006 F12). The **extracted text is a snapshot** stored in the chat file
  (F3): the conversation stays coherent if the file later changes/disappears,
  `build_request` stays synchronous with no I/O, and the chat is self-contained
  for backup/export — the same reasoning behind RAG's `rag_sources`. Sizes are in
  **estimated tokens** (F6, `shared::tokens`) — characters mislead across scripts
  (Cyrillic ≈2 chars/token vs ≈4 for Latin).
- **Delivery**: `request::inject_attachments` (pure, testable) appends the block
  to `ChatRequest.system` (F2) — one code path, no per-provider wire risk
  (Anthropic top-level `system` / Gemini `systemInstruction` / OpenAI
  `instructions` are all already handled), precedent `inject_self_model`, and a
  position at the front of the prefix so the conversation after it stays
  prefix-cached (spec §6.6); re-prefilled only when the attachment set changes.
  The header is in the **profile** language (axis A) and marks the content as
  **DATA, not instructions** (prompt injection, spec §13); **section fences widen**
  (`fence_width`) so a file quoting `>>>` can't close its own section — covered by
  a test.
- **Two modes, and nothing is ever refused for size** (a better story than the
  original "refuse above the budget"): within `max_file_tokens` **and** the chat's
  remaining `max_total_tokens` → **inline** (full text); otherwise → **by
  reference** (metadata + head excerpt; exhaustive reading arrives in stage 2).
  Refusal is reserved for a missing file, a directory, >32 MB, undecodable
  content, or empty content.
- **Formats (F8): any valid UTF-8** plus html/pdf/docx via the extractors RAG
  already uses — RAG's extension allowlist is wrong here, since the most obvious
  attachment is a source file (`main.rs`, `config.toml`, a log). Extraction reuses
  `orchestrator/rag.rs::read_source_text` (promoted to `pub(super)`), which stays
  in `app` because it reaches into `features/tools/web` — a sideways import
  `features → features/tools` is forbidden by FSD (the documented precedent from
  the RAG html/pdf/docx work).
- **Reading runs in a background task** (`spawn_blocking` + an internal
  `attach_tx` channel, mirroring `title_tx`): a large PDF must not block the
  orchestrator's command loop. The orchestrator — the sole owner of `Chat` —
  decides the mode against the budget and inserts the attachment; re-attaching the
  same path **replaces** the previous snapshot (idempotent, like re-adding a RAG
  source).
- **UI**: `/file attach|remove|list` parsed by `features/file_command.rs` (a
  direct sibling of `rag_command.rs`, **`remove` and never `delete`** — the same
  wording decision RAG made); a feed note per outcome; a quiet status-bar chip
  `§ files: N (~tokens)` — attachments cost tokens on **every** turn, so the
  standing cost must be visible (the `§` glyph is WGL4 and one column wide, so it
  needs no compat replacement and doesn't shift the hotkey grid — the `♪`
  precedent; an emoji paperclip would). `/file` entries head `HELP_COMMANDS` as
  requested. Budgets — three fields in the settings "Memory" section
  ("Attachments" group).
- **Tests**: entity (token estimate, name/path matching, excerpt on a word
  boundary and never splitting a character, `format_bytes`, serde); parser
  (subcommands, quoted paths, `#N`/name/path resolution, the per-locale error
  gate); injection (no-op when empty, inline vs excerpt, **fence widening**,
  standalone block, per-locale); orchestrator integration through the real `run`
  loop with a capturing backend (the text reaches `system` and the conversation
  stays clean, persistence to the chat file, `/file remove` takes it back out of
  the request, over-budget → by reference + `#N` addressing, re-attach doesn't
  duplicate, a missing file reports an error); screen (the commands aren't sent as
  messages, an invalid one leaves a note, the chip counts only inline weight,
  `/file` first in the help popup). **1330 unit tests green** (+36), **60
  `#[ignore]`** (+1), clippy `-D warnings`/fmt/`cyrillic_scan`/i18n gates clean.
- **Live run — GO** (Gemma 4 31B q4_0 + bge-m3, external `llama-server`,
  `--jinja`): `file_attachment_e2e_live` — the **baseline** chat (nothing
  attached) answered "couldn't find any information regarding an internal build
  code", while the chat with the file attached answered exactly `ZARYA-7719`;
  the attachment came back `Inline`, 139 B / ~35 tokens. So the block reaches the
  model through the real wire path and is actually used. The mirror half — that
  `/file remove` takes the text back out of the request — is deterministic and
  covered by a unit test with a capturing backend, so it needs no model.
- **Regression — clean**: all **22** orchestrator live e2e smokes green (598 s) —
  memory/self-model/notes/RAG/graph/cross-organ links/control tools/i18n/TTS.
  Worth running in full here because `build_request` sits on **every** generation
  path and its signature changed. Client-level smokes (`OpenAiClient`) were not
  re-run — that layer is untouched.

### Post-M9: chat file attachments — stage 2 (`attachment_read`) (done)
- **Triggered by a live in-app run of stage 1** (user, GPT-5.6): a small PDF
  (131 KB, ~3.5k tokens, inline) worked perfectly — the model read the article
  and reviewed it. A 1.6 MB / ~418k-token TXT went **by reference** (correct —
  inlining would have destroyed the context), but the model **could not read
  it**: it had the excerpt and no reader, so it improvised — `fs_list`,
  `fs_read` (into the sandbox error), four `web_search` calls — and ended with
  "send a few pages or reload the file", which the user cannot do. Six wasted
  tool rounds and an impossible suggestion.
- **Two defects, not one.** The missing tool is stage 2 by design; but the
  by-reference block **not telling the model what is and isn't possible** was a
  stage-1 wording bug of mine. The entry now states the page range, names
  `attachment_read`, and says the file is unreachable by other means — pinned by
  a regression test (`by_reference_entry_tells_the_model_how_to_read_the_rest`).
- **`attachment_read(name, page)`** (`features/tools/attachment.rs`): returns one
  page of the stored snapshot with a `name — page N of M` header. **Pages, not
  character offsets** (fork F12): discrete and enumerable, so the model can walk
  `1..M` and *know* it read everything — the guarantee retrieval cannot give.
  Failure paths answer usefully instead of erroring: an unknown name **lists what
  is attached**, an out-of-range page **reports the real count** — so the retry
  can succeed. No gate, enabled by default: unlike `fs_read` this **narrows**
  access (only what the user explicitly attached, never the filesystem).
- **Plumbing**: `TurnInfo.attachments`/`ToolContext.attachments` as
  `Arc<[Attachment]>` — the turn snapshot pattern already used for
  `system_message`; `Arc` because `ToolContext` is `Clone` and texts can be
  hundreds of KB. Background loops (reflection/consolidation) pass an empty
  snapshot — they run outside a chat turn. `page_tokens` rides `ToolParams` from
  config, with a settings field next to the other attachment budgets.
- **Token accounting corrected** (also from the screenshot): the chip showed
  a cost of `~0` for a by-reference file. Technically it carried no inline text, but
  its excerpt **is** re-sent every turn, so "free" was a lie. New
  `Attachment::prompt_tokens` — inline: the whole file, by reference: the
  excerpt — and the chip/`/file list` report that. The **budget** still counts
  inline text only (that is what `max_total_tokens` governs); the two figures are
  deliberately different and documented as such.
- **A real pagination bug caught by its own test**: the cut landed *before* the
  separator, so a line break started the next page instead of ending the current
  one, shaving a word off every page (pages came out as `["line", " one\nlin",
  "e two\nlin", …]` — every page starting with the previous one's separator). Fixed to
  cut *after* the separator, with a quarter-budget floor so a boundary near the
  start doesn't waste the page. Pagination is lossless (`pages.concat() == text`)
  and never splits a character — both pinned by tests.
- **Tests**: entity (pagination is lossless / prefers line breaks / never splits
  a character; a short or empty text is one page and `page 1` always exists;
  by-reference cost is the excerpt, non-zero); tool (walking `1..M` reassembles
  the file byte for byte; `page` defaults to 1; unknown name lists attachments;
  out-of-range reports the count; per-locale description gate); injection (the
  by-reference entry names the tool and the range, an inline one doesn't);
  orchestrator (the turn snapshot actually carries the chat's attachments, and
  the tool is registered under its wire name). **1341 unit tests green** (+11),
  **61 `#[ignore]`** (+1), clippy `-D warnings`/fmt/i18n gates/`cyrillic_scan`
  clean.
- **Live run — GO** (Gemma 4 31B q4_0): `attachment_read_e2e_live` — a file
  forced by reference (1454 tokens, `prompt_tokens: 59` — the excerpt only), the
  answer planted on the **last** page. The model called `attachment_read` **five
  times**, walked the pages and answered `ZARYA-8823`. Exactly the behaviour the
  live stage-1 run lacked. **Regression — clean**: all **23** orchestrator live
  e2e smokes green (718 s), the turn snapshot/`ToolParams` changes touching every
  tool path.
- **Along the way**, `excerpt` and `paginate` were deduplicated onto a shared
  `cut_point` — they had grown two independent "back off to a character, then a
  word boundary" implementations that had already drifted (half- vs
  quarter-budget floor, and one kept the separator while the other dropped it).
  One visible consequence: an excerpt now **ends with** its separator, like a
  page (harmless — a newline follows it in the prompt). Both attachment live
  smokes were re-run after the refactor.

### Post-M9: chat file attachments — stage 3 (`attachment_search`) (done)
- **Completes the track** ([docs/file-attachments.md](docs/file-attachments.md)).
  Stage 2 made a big by-reference file **readable** (`attachment_read` walks
  pages `1..M`), but it did not make it **searchable**: on the user's real 1.6 MB
  file (~280 pages), finding a specific place by paging is hopeless — a dozen-plus
  rounds, and `max_tool_rounds` runs out first. Stage 3 adds a **chat-scoped
  semantic index** and `attachment_search`. The two are complementary, not
  redundant: search answers *where* to look, `attachment_read` guarantees
  *everything* can be read.
- **Fork F11(a) — a separate index, not a `chat_id` column on `rag_documents`**
  (confirmed by the user 2026-07-27). Three technical reasons, in order of weight:
  (1) `rag_vectors` is a vec0 table partitioned by `profile_id` and the `k`
  constraint applies **inside** the partition — a `WHERE chat_id` in the join
  would filter *after* kNN and silently return fewer than `k` hits; (2) the column
  doesn't exist and `CREATE TABLE IF NOT EXISTS` can't add one → a guarded `ALTER`
  or the first real `DB_STEPS` bump; (3) the user's profile knowledge base is
  **curated** — one chat's attachments would pollute `/rag list|rebuild|remove`
  and cross-source dedup. New `shared/storage/db/attachments.rs`:
  `attachment_documents` + a vec0 `attachment_vectors` partitioned by **`chat_id`**.
  Scoping by chat is **strictly narrower** than the `profile_id` isolation
  invariant (spec §10.3) — a chat belongs to exactly one profile, so it holds a
  fortiori (documented in the module doc). Purely **additive** DDL
  (`CREATE TABLE IF NOT EXISTS` in `baseline_ddl`) → **no `DB_STEPS` bump, no
  migration** (ADR 0006 F12); the table rides the existing backup as part of
  `data.db`.
- **A shared dimensionality, and the trap it opened.** Per F11 the vector size
  stays one per DB (`meta.rag_dim`): mixing vectors from two embedding models is
  meaningless anyway. `ensure_vec_table` was split into `ensure_dim` (registers
  the shared size, errors on a mismatch) + a per-table `CREATE VIRTUAL TABLE IF
  NOT EXISTS`. That exposed a latent read bug: `rag_search` and `delete_matching`
  used "is a dimension recorded?" as a proxy for "does `rag_vectors` exist?" —
  true before, false now (an attachment can register the size first, leaving RAG's
  table absent → a SQL error on a table that isn't there). Both switched to a real
  `table_exists` check, pinned by a regression test.
- **`rag_reset_vectors` → `reset_vectors`, and it now drops both.** A dimension
  change (`/rag rebuild` with a new model) must drop `attachment_vectors` too,
  **and** delete `attachment_documents`: their vectors are gone and sqlite reuses
  rowids, so surviving rows would join onto whatever lands on those rowids next —
  stale text at wrong distances. Keeping this in one method was deliberate:
  splitting it into two calls at the call site would make the pair forgettable,
  and forgetting it is silent corruption. It returns the number of chunks dropped
  so the rebuild task can `warn` about the loss instead of hiding it. The
  attachment index is **derived** data (the text snapshot lives in the chat file),
  so re-attaching rebuilds it — recorded as roadmap groundwork.
- **Indexing is a background task** (`spawn_attachment_index` in
  `orchestrator/attachments.rs`, the `spawn_rag_ingest` pattern): chunking via
  RAG's own `chunk_text`/`chunk_markdown` (`ChunkParams` from `config.rag` — no
  new settings), sub-batched embedding (`EMBED_BATCH_CHUNKS`, both made
  `pub(super)`), progress through the **same banner slot** the RAG banner uses
  (`RagBanner` — both are "an index is being built in the background", and they
  don't overlap in practice; the field's doc says so, and `is_rag_active` already
  gates the spinner). Only **by-reference** files are indexed (fork F13): an
  inline one is already in the prompt in full, so search would return duplicates
  of what the model can see.
- **Graceful degradation is the load-bearing property** (ADR 0002 pattern): with
  no embedder the index is skipped with a note (`IndexSkipped`), and the block,
  `attachment_read` and everything else keep working. The feature never *depends*
  on RAG being configured — which is the whole reason attachments exist as a
  separate mechanism (§3 of the plan).
- **The block only advertises what exists.** `inject_attachments` gained an
  `indexed: &[Uuid]` argument (one `attachment_indexed_ids` query per turn, and
  only when the chat has attachments): a by-reference entry names
  `attachment_search` **only** for a file that really has an index — promising
  search over an unindexed file is exactly the "sent down a dead end" failure
  stage 2 was created to fix. The tool likewise distinguishes "nothing indexed
  here" from "no hits", pointing at page reading in both cases.
- **No cancellation machinery, by design.** The obvious race — an indexing task
  finishing *after* its file was removed — is closed where it actually matters:
  `attachment_search` filters hits by the **turn's attachment snapshot**, so a
  removed file can never surface, and `attachment_prune(chat, keep)` (called on
  attach and on remove) collects the leftover rows. That replaced a
  `HashMap<Uuid, CancellationToken>` + lifecycle bookkeeping with one DB
  primitive. Along the way `ToolContext.chat_id` finally got a real consumer (its
  `#[allow(dead_code)]` is gone).
- **Tests**: db (chat isolation on kNN — chat B's identical vector must not leak
  into A; delete/prune scoped to one chat; re-index replaces; the shared dimension
  + `reset_vectors` clearing both; the `rag_search`-without-its-table regression);
  tool (finds by meaning and names the file; **hides files no longer attached**;
  reports "nothing indexed" and degrades when the embedder is gone — both pointing
  at `attachment_read`; empty query); injection (search offered only for an
  indexed file, page reading either way); orchestrator through the real `run` loop
  (a by-reference file is indexed and searchable, another chat sees nothing of it;
  an inline file is **not** indexed; `/file remove` drops its index). **1356 unit
  tests green** (+15), **62 `#[ignore]`** (+1), clippy `-D warnings`/fmt/i18n
  gates/`cyrillic_scan` clean.
- **Live run — GO on the first attempt** (`attachment_search_e2e_live`, Gemma 4
  31B q4_0 + real bge-m3): a 240-item document with the payload buried at item
  121, indexed into 29 fragments; the model made **one** `attachment_search` call
  and answered `ZARYA-4417` in ~9 s. That is exactly the stage's criterion —
  "finds the right place by meaning in one call instead of paging through".
- **The regression run turned up the stage's most interesting finding — in the
  stage-2 smoke.** `attachment_read_e2e_live` failed: with an embedder configured
  the by-reference file is now indexed too, and the model **stopped walking pages
  entirely** — one `attachment_search` call, correct answer (`ZARYA-8823`). Not a
  defect: it is the feature working, and the narrow "must call `attachment_read`"
  assertion had simply become wrong. Rewritten as two turns: turn 1 keeps the real
  stage-1 regression (the answer is found **and** the model stays inside the
  attachment tools — no `fs_read`/`web_search` improvising), turn 2 asks for a
  specific page, which search cannot answer, keeping the guaranteed path covered
  live. Both green (turn 2 quoted page 1's first line). Worth remembering as a
  pattern: a new capability can invalidate an older smoke's *assertion* while
  improving its *outcome*.
- **Regression — clean otherwise**: the remaining **23** orchestrator live e2e
  smokes green (571 s) — memory/self-model/notes/RAG/graph/cross-organ
  links/control tools/i18n. Worth the full set here: `build_request` gained an
  argument, `rag_search`/`delete_matching` changed their "is anything indexed?"
  guard, and `reset_vectors` was renamed and widened.
- **Follow-up after the merge — numbering the search fragments.** A live in-app
  run (GPT-5.6 over a real 1.6 MB collection, 1441 fragments indexed) showed the
  feature working end to end — including the epistemics we were after: the model
  answered *and* volunteered that "the file is 281 pages, so this is a choice
  among the candidates I found, not the result of reading the whole collection".
  But the **result format didn't survive real data**: a fragment is a whole chunk
  (~800 chars) and is routinely multi-line, while `- [name] ` marked only its
  first line — ten fragments ran together into one wall of text, boundaries lost
  both for the reader in the feed and for the model parsing the result. Now each
  fragment is numbered, its text starts on its own line, and a blank line
  separates them (`1. [name]\n<text>`). The number separates, it doesn't address —
  no tool takes a fragment index, and the comment says so. Deliberately **not**
  routed through `present.rs`'s markdown path: file fragments are arbitrary text,
  and markdown would turn a leading `#`/`- ` into headings and lists. No CHANGELOG
  entry — the feature itself is still in `[Unreleased]`, so this is polish on
  something nobody has seen released. **1357 unit tests** (+1), gates clean; a
  pure formatting change, no live run needed.

### Post-M9: rag_search — the same fragment separation, and a rendering defect it uncovered (done)
- **Asked for as "do the same for `rag_search`"** (numbering the fragments, after
  the same fix landed for `attachment_search`). Porting it blindly would have
  made things **worse**, so the format was checked against the real renderer
  first — three probes, and each overturned an assumption:
  1. `rag_search` results go through `present.rs`'s **markdown** path
     (`PROSE_RESULT_TOOLS`), unlike `attachment_search`, which is `Plain`. So in
     the feed markdown collapses a multi-line fragment into one item line anyway
     — the numbering alone would have changed `-` into `1.` and nothing else.
  2. Worse: putting the fragment's text on its own line lets a **block construct
     inside the fragment escape its list item**. A `## Heading` renders as a
     document heading in the middle of the tool result and splits the fragment in
     two — and `chunk_markdown` **deliberately prepends a section heading to every
     `*.md` chunk**, so this is the common case, not a corner one.
  3. And the probe showed the defect **already exists today**: a heading on any
     line after the first breaks out of the current `- [source] …` bullet just the
     same. A pre-existing bug, not one the change would have introduced.
- **So the fix is the one `attachment_search` already had**: `rag_search` leaves
  `PROSE_RESULT_TOOLS` and renders `Plain`, and its passages get the same
  numbering (`1. [source]
<text>`, blank line between). Both halves are needed —
  numbering without plain rendering is invisible, plain rendering without
  numbering leaves the boundaries unmarked. Fragments of the user's files are
  **data, rendered verbatim**; that markdown was ever applied to them was the
  actual mistake.
- **`web_search`/`fetch_url`/`note_recall` keep markdown**: their payload is prose
  (summaries, the user's own notes), not verbatim file content. The "linked notes"
  block inside `rag_search`'s result also stays a plain `-` list — notes carry
  real ids for addressing, so numbering them would add a second, fake handle.
- **Tests**: `rag_search` numbers passages and separates them; a `present.rs`
  regression test pinning both directions — the fragment tools render verbatim,
  the prose tools stay markdown. **1359 unit tests** (+2), gates clean. No live
  run needed: the change is to a result string's shape and to feed routing, both
  covered deterministically (and the underlying search behaviour was verified live
  in the attachment-index stage).

### Post-M9: embedding-model change detection (stage 1) (done)
- **Stage 1 of a new track** (research
  [docs/research/embedding-model-change-reindex.md](docs/research/embedding-model-change-reindex.md),
  forks R1–R7 accepted by the user as recommended — options "a" — 2026-07-27;
  branch `feat/embed-model-change-detection`): **detect** that the embedding
  model changed and **invalidate** the vectors it orphaned. No reindexing —
  that is stage 2. ADR 0002 deferred "switching the model requires reindexing"
  from the start; this converts the worst failure mode (silent) into a visible
  one.
- **The finding that drives everything: dimensionality is not identity.** It was
  the *only* signal the app had, and `bge-m3` and
  `multilingual-e5-large-instruct` are **both 1024-d** — so a swap between them
  passed `ensure_dim`, passed `/rag rebuild`'s `dim_changed` check, and passed
  every other guard, while turning retrieval into noise: the same text embedded
  by both scores a cosine of **0.37**, and on a 4-document probe corpus the
  retrieval margin collapsed 3x (0.449 → 0.149), with the *correct* hit after a
  swap (0.315) scoring below an *irrelevant* hit in the healthy run (0.283).
  Silent: no error, no warning, no mismatch.
- **Notes were the worst case, in both directions** — and this is the single
  most valuable fix here, since memory-about-self is the project's flagship
  track. `note_vectors` was **never refreshed by anything**
  (`notes_missing_vectors` returns only notes with *no* vector row, so
  `ensure_note_vectors` backfilled but never refreshed; `/rag rebuild` never
  touched the table). Same dimension → stale vectors silently mixed with fresh
  queries. Different dimension → `db::cosine` returns `0.0` on a length
  mismatch, so semantic recall scored **everything** at 0.0, sorted by a
  constant, and returned **arbitrary** notes as "semantically relevant" while
  every duplicate gate stopped firing. RAG in the same situation fails loudly on
  insert; notes failed silently.
- **Identity is established behaviourally** (`shared/embed_identity.rs`, pure):
  embed a fixed `CANARY_TEXT`, store the vector, compare next time
  (`EmbedFingerprint { canary, model_id }`, `matches()`/`display_id()`).
  Measured on the live pair: same model **1.000000** (both on a repeat call and
  inside a differently-sized batch), cross-model **0.368940** → a margin of
  **0.63**, so `CANARY_MATCH = 0.999` only has to sit above a single provider's
  numeric noise (a cloud provider is not bit-exact the way a local
  `llama-server` is). A canary catches what a config fingerprint cannot: the
  same GGUF path re-pointed at another file, a requantization, or a server
  restarted with different pooling/normalization flags. `model_id` (from the new
  `EmbedSettings::active_model_name()`) is **display metadata only, never the
  trigger** — a generic id or an unchanged name after a file swap makes it
  unreliable alone.
- **A decorator, checked lazily** (`app/orchestrator/embed_guard.rs`):
  `EmbedGuard` wraps `Embedder` and runs the check on the **first real embed
  call** (`tokio::sync::OnceCell::get_or_try_init`). Embeddings are deliberately
  lazy (ADR 0002 — `apply_embed` runs no probe), so there is no startup moment
  when a managed embedding server is known to be up; the first real use is the
  moment it has demonstrably answered. Wrapping also makes the check impossible
  to forget at a call site. A **failed** check is never cached (it retries) and
  never blocks the real call — the call below reports the real error itself.
  Installed in `apply_embed_settings`, which runs at bootstrap and on every
  embedding-settings change, so changing the model in settings re-arms it.
- **Each store gets the cheapest correct route, all of which already existed**:
  **notes** — `note_vectors` dropped; the note text is intact, so
  `notes_missing_vectors` lists them and the existing `ensure_note_vectors`
  backfill re-embeds them on the next semantic path (self-healing within one
  `note_recall`, and it costs the user nothing). **Chat attachments** — index
  dropped; derived data, so `attachment_search` degrades to its `not_indexed`
  answer pointing at `attachment_read` (the guaranteed path) and re-attaching
  rebuilds it. **RAG** — **never touched**: re-embedding it needs the full
  ingest pipeline (stage 2), and it is the user's own data. The affected
  profiles are recorded instead and `rag_search` **refuses** with a message
  naming `/rag rebuild`. **Refusing rather than warning** is the point: the
  vectors are in a different space, so results would be noise dressed up as
  answers.
- **Staleness is per profile, not global**, because `/rag rebuild` is
  per-profile. `/rag rebuild` lifts the mark **right after it deletes the old
  chunks**, not at the end — so it stays correct even if the rebuild is
  cancelled or some sources fail, since nothing old survives either way.
  `/rag remove` lifts it once the base is empty, closing the dead end "removed
  everything, re-added under the new model, still refused" (the mark would
  otherwise only be liftable by a rebuild, which needs sources to rebuild from).
- **Honesty over noise**: the fingerprint is recorded **after** invalidation, so
  an interrupted run redoes it and a healthy launch never re-invalidates; and a
  first run with nothing recorded is **silent** — with no prior fingerprint
  there is no evidence anything is stale, and claiming otherwise would cry wolf
  on every first launch. The user is notified only when something was actually
  invalidated, and only the knowledge base asks anything of them.
- **Storage — three keys in the existing `meta` table**: `embed_canary` (a JSON
  f32 array), `embed_model_id`, `rag_stale_profiles` (a JSON uuid array).
  Purely additive → **no schema bump, no migration** (ADR 0006 F12). New `Db`
  methods `embed_fingerprint`/`set_embed_fingerprint`, `profiles_with_rag_docs`,
  `rag_stale_profiles`/`set_rag_stale_profiles`/`clear_rag_stale_profile`/
  `rag_is_stale`, `note_vectors_clear_all`, `attachment_index_clear_all`; new
  private `meta_get`/`meta_set`/`meta_del` helpers now back `vec_dim`/
  `ensure_dim` too. `reset_vectors` clears all three new keys as well — it is
  the "start completely fresh" primitive, and with no vectors left there is
  nothing to be stale *relative to*, so a leftover fingerprint would report a
  change against data that no longer exists. Corrupt `meta` values (a
  hand-edited `data.db`, an empty canary) deliberately read as "nothing
  recorded" rather than bricking startup; the next launch repairs the record.
- **i18n**: `ui.embed.model_changed`/`ui.embed.rag_stale` (axis B — the notice
  is for the user) and `tool.rag_search.err.stale` (axis A — the refusal is read
  by the model), both bundles.
- **Tests**: fingerprint matching (scaling and numeric noise still match, a
  cross-model figure and any dimension change do not, the canary string is
  pinned against a careless edit — changing it would report a model change for
  every existing installation); the guard against a `SaltedEmbedder` fixture —
  two instances standing for two models at the **same** dimensionality, the case
  no dimension check can see (first run records silently; an unchanged model
  invalidates nothing; a swap drops note vectors while the notes survive; RAG is
  **marked, not deleted**; the check is cached per instance; an unavailable
  embedder records nothing); DB round-trips, corrupt-value degradation, the
  empty-list-removes-the-key invariant, `reset_vectors` forgetting the model;
  `rag_search` refusing on a stale base, naming the fix, and healing after the
  mark is cleared. **1387 unit tests green** (+28), **63 `#[ignore]`** (+1),
  clippy `-D warnings`/fmt/`cyrillic_scan` clean.
- **Live run — GO** (`same_dimension_model_swap_detected_live`, needs
  `MINDFORK_EMBED_URL` + `MINDFORK_EMBED_URL_ALT`; real `llama-server` instances
  holding `bge-m3-Q8_0` on :8001 and `multilingual-e5-large-instruct-q8_0` on
  :8002): the same model twice stayed **silent** — the more important half, since
  a false positive would wipe the note vectors and nag on every launch, and real
  servers are not obliged to be bit-exact the way a mock is — and the
  same-dimension swap was detected, dropped the note vectors, marked the
  knowledge base stale, **left the base itself intact**, and notified the user.
  The smoke also asserts both models report the same dimensionality, so it stays
  meaningful only for the case no existing guard can catch.
- **Regression — clean**: all **25** orchestrator e2e live smokes green (530 s)
  on Gemma 4 31B q4_0 (external `llama-server`, `--jinja`) + bge-m3 — memory/
  self-model/notes/graph/cross-organ links/RAG/attachments/control tools/i18n/
  MCP. Worth the full set here: **every** embedder call now goes through the
  guard, `rag_search` gained a pre-query check, and `vec_dim`/`ensure_dim` were
  rerouted through the new `meta` helpers — so the blast radius is the whole
  memory subsystem, not just the new code.
- **Deliberately not in this stage**: re-embedding in place and per-row
  fingerprints (stage 2, fork R4a) — re-embedding is a *different operation*
  from re-chunking (it needs only the chunk text, which all three stores already
  hold), so it belongs in one DB-global resumable job rather than in
  `/rag rebuild`; and per-model similarity thresholds (stage 3, fork R6a). Keep
  the second-order finding visible: `CONSOLIDATE_SIMILARITY = 0.85`,
  `TRAIT_SIMILARITY = 0.72` and `SUMMARY_OBS_SIMILARITY = 0.62` are calibrated
  on bge-m3, and on e5 an **unrelated** trait pair scores 0.751 (above the 0.72
  gate) while an antonym pair scores 0.887 (above 0.85) — so even a perfectly
  correct reindex would flip the gates from "silently never fire" to "fire on
  everything", trading a silent failure for a loud wrong one.

### Post-M9: embedding-model change — stage 2 (re-embedding in place) (done)
- **Stage 2 of the same track** (research
  [docs/research/embedding-model-change-reindex.md](docs/research/embedding-model-change-reindex.md)
  §8, sub-decisions **S1–S5 in §8.1**, recorded before implementation per
  AGENTS.md §1; same branch `feat/embed-model-change-detection`): **re-embed in
  place**, plus the mechanism that replaces stage 1's blunt deletion. Stage 1
  changed what stage 2 is *for* — notes and attachments already heal
  themselves, and `/rag rebuild` already repairs a knowledge base — so the
  target is what stays genuinely broken: a rebuild **loses** legacy rows whose
  stored text is absent and whose file is gone (it counts them as errors and
  drops them), it is **per profile** (a model change means switching into each
  one in turn), and attachment indexes come back only on **re-attach**.
- **Embedding generations (S1, S3) — the core.** A monotonic `meta.embed_gen`
  counter plus an `embed_gen INTEGER` column on the three **plain** tables a
  vector belongs to (`note_vectors`, which holds its vectors itself, plus
  `rag_documents`/`attachment_documents`). The two `vec0` virtual tables are
  deliberately untouched — a virtual table cannot take an `ALTER`, and each
  joins by `rowid` to one of those plain tables, which can. Identity
  itself already lives in the canary; a *row* only needs to say **which
  generation produced it**, so the marker is a small integer, not a vector.
  Writers (`note_vector_upsert`/`rag_insert`/`attachment_insert`) stamp
  themselves **under the lock they already hold** — an unstamped vector is
  therefore impossible to write, and **not one of the ~10 call sites changed**.
  Readers ignore foreign generations (`note_search_semantic`,
  `notes_with_vectors`, `attachment_indexed_ids`, `attachment_search`), while
  `notes_missing_vectors` **lists** foreign-generation notes, so the existing
  `ensure_note_vectors` backfill re-embeds them with no new code at all.
- **So stage 1 stopped deleting anything** (S3): the guard bumps the counter
  instead — **one increment retires the whole database**. `note_vectors_clear_all`
  and `attachment_index_clear_all` are gone. Keeping the rows is what makes
  re-embedding possible at all (it works from the text they already hold), what
  lets attachment indexes come back without re-attaching, and what makes
  switching **back** to the previous model cost exactly nothing — a generation
  the DB has already seen makes its vectors current again, with zero work.
  `reset_vectors` deliberately leaves the counter alone: it is monotonic, and
  reusing a number would make a surviving old row read as current.
- **Schema (S2) — a guarded `ALTER`, not the first `DB_STEPS` bump.**
  `ALTER TABLE … ADD COLUMN` in `baseline_ddl`, made idempotent by
  `PRAGMA table_info` (`column_exists`/`add_column_if_missing`) rather than by
  matching on the "duplicate column name" error string, which would also swallow
  a genuinely different failure. **No `DB_SCHEMA` bump, no step, no migration, no
  pre-migration backup**: a nullable column is additive and backward-compatible —
  every query names its columns explicitly, so an older binary ignores it — which
  is exactly the case ADR 0006 F12 says needs no bump, and `CREATE TABLE IF NOT
  EXISTS` simply cannot express it. A bump would also force a backup of `data.db`
  on every upgrade and exercise never-before-run machinery for a change that
  doesn't need it.
- **`NULL` must read as foreign, and that is the load-bearing detail**: every row
  in a real user's database predates the marker. A plain `embed_gen = ?`/`<> ?`
  evaluates to `NULL` — neither true nor false — and would silently skip exactly
  the rows most in need of the work, so every predicate folds through
  `IFNULL(embed_gen, NULL_EMBED_GEN)` against a `-1` sentinel that can never
  collide (generations start at 1). A corrupt counter reads as the first
  generation — the usual "garbage in `meta` degrades to the natural empty state"
  rule, and the safe direction: rows stamped higher then read as foreign and get
  re-embedded, rather than being served from a space nothing matches.
- **`/reindex` (S4)** — a new top-level chat command
  (`features/reindex_command.rs` + `app/orchestrator/reembed.rs`), **DB-global**.
  Not a `/rag` subcommand: it spans notes, chat attachments and **every**
  profile's knowledge base, so filing it under the knowledge-base family would
  misdescribe its scope; `/rag rebuild` keeps its own meaning (re-chunk one
  profile after a chunking-parameter change). The global scope is **not** a
  breach of the `profile_id` isolation invariant (spec §9.5): the invariant
  governs what one profile's *queries* may see, and the job serves no query — it
  rewrites a row's vector under the partition key the row already carries.
  Trailing arguments are **reported, not ignored** (unlike `/rag list` there's no
  subcommand to disambiguate a typo from, so silence would hide it).
- **Re-embedding is not re-chunking** (research §3) — the whole reason this is
  its own operation. It needs no source text and no chunker, so it repairs legacy
  rows whose file is gone, covers every profile in one run, brings attachment
  indexes back without re-attaching, and **keeps chunk ids stable** so nothing
  downstream is invalidated. One loop over the three stores: *for each row whose
  generation is not current, embed its stored text, replace the vector, stamp the
  generation.* Order within `Store::ALL` is cheapest-first (notes → attachments →
  knowledge base): notes restore memory almost immediately, and the base is both
  the largest and the one held back by a stale mark until the end anyway.
- **Resumable by construction**: stamping a row removes it from the queue
  (`ORDER BY rowid` + `LIMIT`, batches of `EMBED_BATCH_CHUNKS`=16), so an
  interrupted run leaves a consistent partial state and a rerun continues exactly
  where it stopped. Inside `set_vector` the step order is load-bearing:
  `ensure_table` **first** (a dimensionality mismatch must fail before anything is
  written, or the row would be stamped current while holding the old vector); then
  skip a row that is gone or belongs to another partition (the user may delete a
  source between the job reading a batch and writing it back — an ordinary race,
  and inserting anyway would orphan a vector on a rowid sqlite later reuses);
  then **vector, then stamp, never the reverse** (a crash between the two makes
  the row look foreign and it is simply redone). Two loop guards: an embedder
  failure or a mismatched vector count is **fatal** for the job (retrying would
  spin on the same batch forever), and a batch that wrote **nothing** breaks that
  store (the queue would otherwise return the same rows forever).
- **The stale marks stay, and keep doing the honesty job**: `rag_search` still
  refuses while a base is mixed. `/reindex` lifts them only when the queue is
  **genuinely empty** — derived from `count_rows_to_reembed`, not from "the loop
  ran" — so a cancelled or partly failed run correctly leaves search refused. The
  stage 1 notice and the `rag_search` refusal now name `/reindex`.
- **A dimensionality change needs no special case (S5)**: a `vec0` table is
  fixed-width, so the job drops both up front via a new `drop_vector_tables` and
  every row then reads as foreign and takes the same path. Deliberately distinct
  from `reset_vectors`: this one keeps the document rows, the fingerprint and the
  stale marks ("keep the texts, re-embed them"), while `reset_vectors` is the
  "start over" primitive that additionally deletes the attachment rows and forgets
  which model produced everything — losing that distinction would mean a
  dimensionality change silently discarded every chat's index instead of
  rebuilding it.
- **Plumbing**: `AppCommand::Reindex`/`ChatIntent::Reindex`; a new terminal event
  `RagProgress::Reembedded { rows, errors, cancelled }` whose cancelled wording
  says a rerun continues rather than reading like a failure; progress reuses the
  **RAG banner** and the single background-indexing slot (`reset_rag_cancel`), so
  `/reindex` and the `/rag` commands are one-at-a-time by construction. `/reindex`
  is in `HELP_COMMANDS` (`F1`), highlighted as a command in the input box and
  skipped by spellcheck. i18n: 5 `ui.reindex.*` + 2 `ui.rag.reembedded*` +
  `ui.embed.reindex_hint` + `ui.help.reindex` keys, both bundles.
- **Tests**: the counter (starts at 1, increments, survives reopening, corrupt
  reads low); **`NULL` reads as foreign everywhere** (queue readers list it, the
  lazy backfill lists it, and no semantic path serves it); the queue (carries
  partition/text, spans every profile and chat, stable batches with no repeats,
  superseded notes excluded, the count agrees with the list); `set_vector`
  (replaces without duplicating and keeps the rowid, skips a deleted/foreign row,
  refuses a dimension mismatch **without stamping**, works at a new width after
  the drop); `drop_vector_tables` keeps what `reset_vectors` would discard;
  **switching back needs no work**; the guarded `ALTER` is idempotent and keeps
  the previous run's stamps; the job (drains all stores and lifts the mark, "no
  work" reports zero plainly, a dead embedder leaves the queue **and** the mark
  untouched so a rerun redoes it, cancelled-before-start keeps search refused, a
  dimension change handled in the same loop); the parser (bare/whitespace/case,
  trailing args rejected, neighbours like `/rag rebuild` and `/reindexer` are not
  it, per-locale errors); the screen (intercepted on Enter, a malformed one leaves
  a note instead of going out to the model, recognized as a command) and the
  `Reembedded` note (clean/errors/cancelled). **1420 unit tests green** (+33),
  **64 `#[ignore]`** (+1), clippy `-D warnings`/fmt/`cyrillic_scan` clean.
- **Live run — GO** (`reindex_restores_retrieval_after_a_model_swap_live`, needs
  `MINDFORK_EMBED_URL` + `MINDFORK_EMBED_URL_ALT`; real `llama-server` instances
  holding `bge-m3-Q8_0` on :8001 and `multilingual-e5-large-instruct-q8_0` on
  :8002, **both 1024-d** — the case no dimension check can see): a corpus and a
  note indexed under bge-m3, the swap detected and everything queued, `/reindex`
  re-embedded 4 rows, the queue drained and the stale mark lifted. The payoff is
  the assertion that matters — **retrieval actually recovered**: an e5 query ranks
  the correct chunk first again and the note is found by semantic recall.
  Reporting success was deliberately not enough for this test.
- **Regression — clean**: all **25** orchestrator e2e live smokes green (501 s)
  on Gemma 4 31B q4_0 (external `llama-server`, `--jinja`) + bge-m3. The full set
  is the right scope here: four readers gained a generation predicate
  (`note_search_semantic`, `notes_with_vectors`, `attachment_indexed_ids`,
  `attachment_search`), **every** vector writer now stamps, and `baseline_ddl`
  gained an `ALTER` that runs on every open — the blast radius is the whole
  memory subsystem, not just the new code.
- **A process trap worth recording** (cost ~20 min of false debugging): a
  subagent building the crate in a *copy* of the tree poisoned the shared
  `target/`, so `cargo test` ran artifacts compiled from other sources — seven
  tests "failed", six with `Cargo.toml`/`Cargo.lock`/`artwork` **not found**
  (`CARGO_MANIFEST_DIR` baked in from the copy) and one asserting against a
  locale string it had never been compiled with. `cargo clean -p mindfork-rs`
  restored a clean 1420/0. When delegating, keep subagents out of a second build
  of the same crate — or treat a sudden cluster of path-not-found failures as a
  build-artifact symptom, not a code one.
- **Deliberately not in this stage**: per-model similarity thresholds (stage 3,
  fork R6a) — and stages 1–2 make it the *last* thing standing between the app and
  a supported model swap, since a switch is now both detectable and completable.
  `CONSOLIDATE_SIMILARITY = 0.85`, `TRAIT_SIMILARITY = 0.72` and
  `SUMMARY_OBS_SIMILARITY = 0.62` are calibrated on bge-m3; on e5 an *unrelated*
  trait pair scores 0.751 (above the 0.72 gate) and an antonym pair 0.887 (above
  0.85), so a perfectly correct reindex flips the gates from "silently never fire"
  to "fire on everything".

### Post-M9: embedding-model change — stage 3 (per-model similarity thresholds) (done)
- **The final stage of the track** (research
  [docs/research/embedding-model-change-reindex.md](docs/research/embedding-model-change-reindex.md)
  §8.2, sub-decisions **S6–S10** recorded before implementation per AGENTS.md §1;
  same branch `feat/embed-model-change-detection`). Stages 1–2 made a model swap
  **detectable** and **completable**; §6 is what still made it *wrong*. The three
  gates that decide when two pieces of memory mean the same thing —
  `CONSOLIDATE_SIMILARITY = 0.85`, `TRAIT_SIMILARITY = 0.72`,
  `SUMMARY_OBS_SIMILARITY = 0.62` — are absolute cosines derived from live runs
  against **bge-m3**, i.e. positions inside *that* model's distribution, not
  universal constants.
- **The measurement, on a fixed probe corpus against both live servers**
  (2026-07-27):

  | | bge-m3 | e5-large-instruct |
  |---|---|---|
  | paraphrase mean | **0.8176** (0.660–0.909) | **0.9456** (0.901–0.973) |
  | unrelated mean | **0.4128** (0.345–0.500) | **0.7897** (0.726–0.834) |
  | usable span | **0.4048** | **0.1559** |

  e5's usable range is **2.6× narrower** — the whole problem in one number: a
  constant tuned inside bge-m3's range lands somewhere else entirely inside e5's.
  Concretely, the trait gate would have fired on **8/8 unrelated** probe pairs.
  Without this stage a *correct* re-embedding would have traded a silent failure
  ("the gates never fire") for a loud wrong one ("the gates fire on everything").
- **Calibration is automatic, not a table (S7).** R6a's "threshold profiles keyed
  by fingerprint" only helps models someone has already measured — an arbitrary
  local GGUF would still be handed bge-m3's numbers, which is the same failure the
  stage exists to fix, just rarer. Instead the corpus is embedded **once**, on the
  very path that already runs exactly once per model (`embed_guard.rs::calibrate`,
  right where the canary fingerprint is recorded): one extra request of 32 short
  strings, and the two means are stored in `meta` beside the fingerprint
  (`embed_cal_unrelated`/`embed_cal_paraphrase` — keys, so **no schema bump, no
  migration**, ADR 0006 F12).
- **An affine map anchored on two measured points (S8)**:
  `t' = u + (t − u_ref)·(p − u)/(p_ref − u_ref)`, with `u_ref = 0.4128` and
  `p_ref = 0.8176` (`REFERENCE_UNRELATED`/`REFERENCE_PARAPHRASE` in
  `shared/embed_calibration.rs`). **bge-m3 maps to itself**, so nothing moves for
  the model the project is tuned on. The thresholds keep their present values and
  meaning (S6) — what changes is only that they are *read* in whatever range the
  active model actually has. The map equalizes **scale**; it cannot equalize
  semantics, and is not meant to.
- **Failure is always downhill (S9) — the property that makes this safe to ship.**
  Nothing calibrated → `SimilarityScale::identity()`, whose `map(t)` returns `t`
  **exactly** rather than through arithmetic that merely ought to cancel out. So
  every existing installation is bit-for-bit unaffected until the model actually
  changes. A calibration that cannot be measured, cannot be read back, or comes
  out degenerate (non-finite, outside the cosine range, or `paraphrase <=
  unrelated` — a zero span would collapse all three gates onto the unrelated mean,
  i.e. make everything a duplicate) falls back to the same identity, and mapped
  values are clamped to a sane cosine range as a backstop. A failed calibration
  can therefore only leave the gates exactly as they are today — never make them
  wilder. Calibration failure is logged, never fatal.
- **The probe corpus is a fixture, not prose (S10)** — `shared/embed_probes.json`
  (`include_str!`), 8 paraphrase pairs and 8 unrelated pairs, deliberately
  **bilingual** (so is the application) and deliberately phrased as the short
  trait/preference/observation statements the gates actually judge: calibrating on
  encyclopaedic prose would measure a different distribution than the one the
  thresholds operate in. It **must never be edited casually** — changing a probe
  silently invalidates `u_ref`/`p_ref` and therefore every mapped threshold, and
  would make already-calibrated installations disagree with freshly calibrated
  ones. The reasoning is in the file's own `_comment` header, and it earns a
  deliberate entry in `tools/cyrillic_scan.py`'s allowlist (measurement data, not
  prose to translate). A malformed fixture degrades to an empty corpus rather than
  panicking (this runs inside a TUI), with a test pinning that the shipped file
  parses.
- **Reading it back**: `Db::similarity_scale()` is **infallible** on purpose — a
  threshold is needed on paths that have no way to report a storage error, and
  every failure has the same right answer, the identity. Four gate sites read it
  (`notes/overview.rs` ×2 — the user-notes and `@self` consolidation overviews;
  `notes/overview.rs::summary_observation_overlaps`;
  `self_model.rs::near_duplicate_traits`), each mapping **once**, outside the
  nested loop it feeds. The two places that *show* the threshold to the model now
  show the **effective** one, formatted `{:.2}` (`format_threshold`): fixed
  precision earns two properties — an uncalibrated overview prints as it did before
  calibration existed (`0.85`, byte-identical), and because rounding is monotonic
  a listed pair (`s >= threshold`) can never *display* below the displayed
  threshold, so the model is never shown a number that contradicts the selection
  it is looking at.
- **Lifecycle**: `reset_vectors` clears the calibration ("start over" — it already
  discards the fingerprint, and the calibration describes the same model);
  `drop_vector_tables` deliberately **keeps** it, because by the time the re-embed
  job drops the tables the guard has already recorded and calibrated the *new*
  model, and clearing there would throw away a fresh correct calibration and leave
  the gates uncorrected for the very model being re-embedded into.
- **Validated on the corpus** — the mapped thresholds fire on the same pairs:
  consolidate **3/8 vs 3/8** paraphrase, trait **6/8 vs 7/8**, summary↔obs
  **8/8 vs 8/8**, and **0/8 unrelated on both models** — against **8/8 unrelated**
  with the raw constants on e5. The residual 6/8 vs 7/8 is real model difference,
  not calibration error.
- **Tests**: the fixture (shape and non-empty probes, a stable flattening order
  `measure` reads back); `measure` refusing a batch that cannot describe the
  corpus (wrong count, an empty vector, mixed widths — a scale guessed from a
  mismatched batch would be a *wrong* scale, worse than none); the identity
  passing thresholds through by **exact** equality; the reference calibration
  coming out as the identity; e5 reproducing the §8.2 numbers (0.85→0.958,
  0.72→0.908, 0.62→0.869) *and* the point of the exercise — e5's unrelated mean
  sits above the raw 0.72 gate and below the mapped one; a narrower range moving
  every gate up while keeping their order; every degeneracy falling back to the
  identity; the clamp; DB round-trip, corrupt values reading as "never
  calibrated", `reset_vectors` forgetting vs `drop_vector_tables` keeping; and the
  four gate sites plus the two display sites. The gate wiring was
  **mutation-tested**: reverting the four comparisons fails four tests
  one-to-one, and reverting only the two display sites fails exactly the two
  overview tests. **1440 unit tests green** (+20), **65 `#[ignore]`** (+1), clippy
  `-D warnings`/fmt/`cyrillic_scan` clean.
- **Live run — GO** (`similarity_scale_follows_the_model_live`, needs
  `MINDFORK_EMBED_URL` + `MINDFORK_EMBED_URL_ALT`; real `llama-server` instances
  holding `bge-m3-Q8_0` on :8001 and `multilingual-e5-large-instruct-q8_0` on
  :8002): bge-m3 calibrated to **0.41277 / 0.81764** — matching the reference
  constants to four decimals, i.e. **identity confirmed against a live server**,
  not just against its own arithmetic; e5 to **0.78968 / 0.94561**, mapping
  0.72 → **0.9080** and 0.85 → **0.9581**, exactly the designed values. The
  behavioural payoff is the assertion that matters: an unrelated pair ("values
  brevity" / "the train leaves from platform nine") scores **0.7479** on e5 —
  **above** the raw 0.72, so the raw constant would have called it a duplicate,
  and **below** the calibrated 0.9080, so the corrected gate rejects it.
- **Regression — clean**: all **25** orchestrator e2e live smokes green (543 s)
  on Gemma 4 31B q4_0 (external `llama-server`, `--jinja`) + bge-m3. Two of them
  are the rewired gates themselves — `trait_gate_e2e_live` and
  `summary_obs_overlap_e2e_live` — so the identity guarantee is confirmed end to
  end through the orchestrator on a live model, not only in unit tests.
- **The embedding-model change track is complete (stages 1–3)**: a swap is
  detected behaviourally, nothing is deleted, `/reindex` re-embeds every stored
  vector in place, and the memory gates follow the model instead of one fixed
  calibration. Groundwork left in research §9: e5-style `query:`/`passage:` input
  prefixes (the `Embedder` contract has no notion of input role), vec0 for notes,
  and cross-model migration without re-embedding.

### Post-M9: per-model input prefixes for embeddings (done)
- **The last groundwork item of the embedding-model change track**
  ([docs/research/embedding-input-prefixes.md](docs/research/embedding-input-prefixes.md),
  forks **R1–R6 accepted by the user as recommended — options "a" — 2026-07-27**;
  branch `feat/embed-input-prefixes`). The e5 family expects each input marked
  with its role (`query:`/`passage:`); the `Embedder` contract had no notion of
  an input role — a query and a stored chunk went through the same call. Only
  relevant now that a model swap is actually supported (stages 1–3 of the
  previous track).
- **The measurement came first, and it reshaped the recommendation.** The
  roadmap justified this with one number on a small corpus (margin 0.155 →
  0.186). Re-measured on **40 documents / 14 queries** in the register the app
  actually indexes (notes, `@self` observations, knowledge-base chunks,
  bilingual, with deliberate near-neighbours): on e5 the prefixes changed **no
  ranking at all** — 12/14 top-1 under every convention, MRR moving by 0.001.
  The entire benefit is separation: mean margin +15%, and the **minimum** margin
  **25×** (0.0002 → 0.0056). A 0.0002 margin is an arbitrary tie-break, so that
  part is real robustness — but it is not a correctness fix, and the docs say so
  rather than overselling it. R6 was therefore offered as a genuine "don't
  implement" option; the user chose to implement with the default **off**, so
  existing installations are bit-identical until they opt in.
- **A convention is a 3-way per-family choice, not a boolean.**
  `multilingual-e5-large-instruct` — the model the earlier figure was measured on
  — does **not** use `query:`/`passage:`; the `-instruct` variants want
  `Instruct: <task>` + `Query: ` and a **bare** passage. So that number was
  measured with the wrong convention for that model and still improved. And the
  wrong convention is measurably **harmful**: on bge-m3 (which wants bare text)
  `e5-instruct` costs a rank (11/14 → 10/14) and 31% of the mean margin. Hence
  `EmbedConvention::None` is the default, and the convention is **never** applied
  automatically — when a model change is detected and the new name looks like an
  e5, the notice merely says which convention it suggests (R3a: a hint, never an
  action).
- **The role lives on the call (R1a)**: `Embedder::embed(texts, role)` with
  `EmbedRole { Query, Passage }` — deliberately **no `Default`**, since a
  silently defaulted role is exactly the failure the type exists to prevent. One
  method, 5 impls, and every batch in the codebase is homogeneous except
  web-search reranking, which now issues two requests (a path that already does N
  parallel page fetches).
- **The prefix is applied by a decorator, and its position is load-bearing
  (R2a)**: `EmbedGuard { PrefixedEmbedder { real embedder } }`, built in
  `apply_embed_settings`. The guard's canary and calibration probes therefore go
  **through** the prefixer, which is what makes both traps self-solving rather
  than merely documented:
  - **calibration** — prefixing bge-m3 moves the corpus's unrelated mean +0.097
    and narrows its span 16%, which would silently invalidate
    `REFERENCE_UNRELATED`/`REFERENCE_PARAPHRASE`. Measuring through the same path
    means bge-m3 stays on `none` (constants valid by construction) and e5
    measures its own means under its own convention. The live calibration smoke
    still reproduces 0.41277 / 0.81764 exactly;
  - **detection** — a prefixed canary scores 0.78–0.9965 against a bare one, all
    below the 0.999 detector, so turning prefixes on reads as the change of
    vector space it really is: the generation is bumped, memory re-embeds itself,
    `/reindex` is offered. Verified, not assumed.
- **Two refinements the measurement argued for.** The canary carries
  **`Passage`**: stored vectors are all passage-role, so the passage marker alone
  defines the space the database is in — a change to the *query* marker alters
  retrieval but leaves every stored vector valid and must **not** force a
  reindex, and tracking the passage role gets that granularity right for free.
  And since e5's passage margin to the threshold is only **0.0025**, the
  convention id joins `EmbedFingerprint` as an **exact second trigger** (R5a) —
  it can only *add* detections, never mask one, which is what separates it from
  the config-only fingerprint rejected as D2 in the previous track. A fingerprint
  written before this exists has no convention field and reads as the default, so
  no installation reports a spurious change on upgrade.
- **Role assignment is the specification, not bookkeeping (R4a).** Research §5 is
  a 20-site table, and its load-bearing finding is that **all four calibrated
  gate sites are symmetric passage↔passage** — `self_note_similar` (the
  `add_insight` gate), the `note_save` duplicate gate,
  `summary_observation_overlaps`, `near_duplicate_traits`. They *read* like
  queries but must be passages, which is also why the calibration corpus is
  passage-role. Mismatching one side costs −0.027 on e5 = **17% of its entire
  usable range**. `/reindex` must use the identical role to the original writers,
  or it would quietly re-create the mixed-space problem the previous track exists
  to kill.
- **Wiring**: `EmbedConvention` + `PrefixedEmbedder` in a new
  `shared/embed_prefix.rs`; `EmbedSettings.convention` (`#[serde(default)]` → **no
  schema bump, no migration**, ADR 0006 F12); `meta.embed_convention` beside the
  canary; an "Input prefixes" Choice field in the Embeddings tab (independent of
  the mode — the convention is a property of the *model*, not of where it runs);
  i18n for the field, its description and the hint, both bundles.
- **Tests**: a new `features/tools/embed_roles_tests.rs` — the executable form of
  the §5 table, using a `RoleRecorder` embedder, because a wrong role is
  otherwise **invisible** (it changes no return value and no other assertion);
  the decorator order (a canary embedded through the guard carries the passage
  marker, and so do the 32 calibration probes); a convention switch bumping the
  generation while **keeping** the note; the hint appearing only when it differs
  from what is set; `web.rs` issuing exactly `[Query, Passage]` with the query
  excluded from the page batch; the default convention passing text through
  **byte for byte**; `suggested_for` recognising the family and nothing else; the
  settings field and the config default. **1466 unit tests green** (+26), **66
  `#[ignore]`** (+1), clippy `-D warnings`/fmt/`cyrillic_scan`/i18n gates clean.
- **A real bug caught by the existing suite**: splitting `web.rs` into two
  requests left the old `results.len() + 1` length check and the `vecs[0]`
  indexing that assumed the query still rode in the same batch — reranking
  silently returned the provider order. `rerank_reorders_results_by_query` failed
  immediately, which is exactly what that test is for.
- **Live run — GO** (`conventions_behave_as_measured_live`, real bge-m3 :8001 +
  e5-large-instruct :8002): e5's own convention widened the relevant/irrelevant
  gap **0.1213 → 0.1789**, and the wrong convention narrowed bge-m3's **0.4406 →
  0.3317** — both directions confirmed on live models, matching the research
  spike. The smoke deliberately asserts on the **margin**, not on top-1: the
  research measured that prefixes change no ranking, so asserting a recovered
  rank would assert something that was never true.
- **Regression — clean**: the three two-server smokes of the previous track (swap
  detection, `/reindex` restoring retrieval, the calibration scale) and all **25**
  orchestrator e2e live smokes green (584 s) on Gemma 4 31B q4_0 + bge-m3. The
  full set is the right scope: **every** embedding call site changed signature,
  and the guard's canary/calibration path was rewired.
- **Groundwork** (research §9): tuning the `-instruct` task string; other
  families' conventions (BGE-v1.5's retrieval instruction, Nomic's
  `search_query:`/`search_document:`) — the design is a table, so a row is cheap;
  per-role calibration is explicitly **not** needed, since all four gate sites are
  passage↔passage.

### Post-M9: self-model injection — per-section budgets (done)

- **Found while measuring for the prompt-caching research**
  ([prompt-caching.md §2.1](docs/research/prompt-caching.md)), fixed as its own
  task; plan and decisions D1–D5 —
  [self-model-injection-budget.md](docs/history/self-model-injection-budget.md)
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
