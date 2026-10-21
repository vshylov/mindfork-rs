# Journal — Notes and their graph

Memory about the interlocutor and the world as an integrated structure rather than a growing pile: semantic recall, duplicate gates, revision and merging, the link graph, and the cross-organ edges that tie notes to the self-model and to the knowledge base.

**Reference documents for this area:** architecture.md §9, spec.md §9.5

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)).
They record what was done, why, what was measured and what was rejected — the reasoning
behind the code, not its current shape. For the current shape read the reference documents
named above; for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (7)

- Post-M9: notes connectivity — MVP probe (Tier 1, done)
- Post-M9: notes connectivity — Tier 2 (graph + revision history, done)
- Post-M9: notes connectivity — Tier 3 (consolidation / "sleep", done)
- Post-M9: linking the memory organs — Tier 3, Path 1 (cross-organ links) (done)
- Post-M9: linking the memory organs — Tier 3, Path 2 (output mixing) (done)
- Post-M9: linking the memory organs — Tier 3, Path 3 (RAG ↔ notes) (done)
- Post-M9: the companion's notes and earlier conversations in the prompt (done)

### Post-M9: notes connectivity — MVP probe (Tier 1, done)
- **A shift of memory from accumulation to integration** per the plan
  [docs/notes-connectivity.md](../../docs/history/notes-connectivity.md). Motive — from a live
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
- Continuation of Tier 1 ([docs/notes-connectivity.md](../../docs/history/notes-connectivity.md)):
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
- Completion of the track ([docs/notes-connectivity.md](../../docs/history/notes-connectivity.md)):
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

### Post-M9: linking the memory organs — Tier 3, Path 1 (cross-organ links) (done)
- **The original long-range connectivity goal**
  ([docs/narrative-as-notes.md](../../docs/history/narrative-as-notes.md)): link the
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
  ([docs/narrative-as-notes.md](../../docs/history/narrative-as-notes.md), Path 2,
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
  ([docs/narrative-as-notes.md](../../docs/history/narrative-as-notes.md), Path 3): a
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

### Post-M9: the companion's notes and earlier conversations in the prompt (done)
- **What.** Stage 2's first half of the companion track
  ([docs/research/companion-platform.md](../research/companion-platform.md) C2, §3.4):
  two profile switches in `CompanionFeatures`, both off by default (the owner's
  decision of 2026-10-21, §8 there). `notes_in_prompt` puts a block of the profile's
  notes — about the person and their world, never the `@self` observations — into
  `system`; `earlier_chats` lists the three conversations before this one, with
  their `chat://` addresses when the turn offers `chat_read` (spec §6.6).
- **Why.** Only `@self` notes ever reached the prompt; a note about the person
  reached the model only through a `note_recall` it chose to make
  (research §2.2 finding 2), and Gemma 4 — a gate model — makes none.
- **The probe decided the shape** (research §3.4: 14 planted notes about one person,
  eight openings, five models through OpenRouter, Sonnet 5.5 as judge). bge-m3 on a
  short opening compresses cosines — the note an opening was about scored
  0.489–0.582, the best note for an unrelated opening at most 0.380 — so the
  observations' 0.50 on the lead does not transfer. A "latest notes" fallback for an
  opening near none (the observations' shape) carried the person's freshest news into
  answers about TCP and the planets on all five models (3–9 of 10) and doubled
  gpt-oss's wrong dates: **no-go**. Relevance alone — every pick over **0.45**, at most
  four, no block when none clears — left those answers clean and raised the fact's
  use where the model does not search: Gemma 4 1/15 → 13/15, gpt-oss 4/15 → 13/14.
  The cost, recorded: a block makes a searching model search less (Haiku 34/44 →
  10/45), so a block of noise picks can hide what a search would find. The earlier
  conversations: "pick up where we left off" taken up 4/4, 4/5, 4/4, 5/5 from 0–1 on
  four models; Gemma asked which, with or without the list.
- **Key decisions (the agent's, within F3 (a)/F4 (a)).** One **memory cut**
  (`features::companion_memory::memory_cut`) — the first user message the request
  carries, after a summary when one is in force — decides both blocks: notes written
  before it, conversations with a user message before it ordered by their last
  message before it. So both blocks are the same text every turn and move with a
  compaction roll, as the observations do (spec §6.6); a reply in an older chat
  after the cut moves nothing. The notes use the observations' query
  (`observation_query`) and water-filling (`fit_observations`, now `pub(crate)`, with
  `age_label`); cap 1 600 characters. Order in `system`: project → earlier
  conversations (`PromptContext.earlier_chats`, synchronous) → notes
  (`generation::inject_prompt_notes`, in the task — it embeds) → self-model. A
  sub-agent's request carries neither. The trailer names `note_recall` only when
  offered; a longer one inviting the search was measured and not kept.
- **Code.** `features/companion_memory.rs` (cut, `earlier_chats`, `render_earlier`,
  `render_notes`), `features/tools/notes/prompt_notes.rs` (`NOTES_FLOOR`,
  `notes_for_prompt`), `PromptContext.earlier_chats`, `GenSpawn.prompt_notes`, two
  settings rows (`PNotesInPrompt`, `PEarlierChats`), `prompt.memory.*` in both
  bundles; the settings screenshots moved with the field count (Profiles 62 → 64).
- **Tests.** 8 unit tests (`features::companion_memory`: the cut, the pick and its
  order, nothing moved by a message after the cut, addresses only with `chat_read`,
  one-line titles, the cap, every locale) and 4 orchestrator tests over a capturing
  engine (`tests/companion_memory.rs`: both blocks reach the model and are identical
  on the next turn; no addresses without `chat_read`; off by default the request is
  untouched; an opening near no note gets no block), and one settings test (each switch flips
  its own feature). A trap on the way: the app's
  embedding guard calibrates the similarity scale against the test `MockEmbedder`
  and raises the floor, so a note at cosine 0.60 fell under it — the fixtures are
  spelt in characters sharing one bucket (docs/lessons.md §2). **4138 tests green, 285
  `#[ignore]`**; fmt, clippy and the repository gates clean.
- **Smoke — GO** (`tests/companion_memory_live.rs`: the planted fact used, an
  unrelated question left clean, "pick up where we left off" taken up). On the gate's
  weights (llama.cpp b11719, RTX 4090, bge-m3 Q8_0 as the embedder): **Gemma 4 31B
  q4_0 3/3 three times, Qwen 3.8 27B Q4_K_M 3/3 twice**; the control — Gemma with
  the notes switch off — failed the fact smoke 3 of 3 ("What makes you say that?").
  In the app Gemma, thinking on, read the latest conversation with `chat_read` in
  all three runs where the probe's thinking-off route asked. Through OpenRouter:
  gpt-oss-120b, Claude Haiku 5.5 and DeepSeek v4.1 flash 3/3 each.
- **Left for later.** C3, the notes screen, is the stage's second pull request. A
  pick on the opening does not follow the conversation (the second-turn case — the
  greeting, then the cat — gained nothing); per-message picks would cost the prefix.
