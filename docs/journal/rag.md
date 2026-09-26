# Journal — RAG, attachments and embeddings

The retrieval side of memory: the profile knowledge base (`/rag`), chat file attachments with their own chat-scoped index, and the embedding stack underneath both — model-change detection, re-embedding in place, per-model thresholds and input prefixes. Embeddings and attachments live here because they are retrieval infrastructure; what they serve is also read by [notes.md](notes.md) and [self-model.md](self-model.md).

**Reference documents for this area:** architecture.md §9, spec.md §9.5

Entries are verbatim and in chronological order, moved here from the CLAUDE.md
journal (see [docs/history/documentation-refactor.md](../history/documentation-refactor.md)).
They record what was done, why, what was measured and what was rejected — the reasoning
behind the code, not its current shape. For the current shape read the reference documents
named above; for the traps that recur across areas read [lessons.md](../lessons.md).

## Entries (20)

- Post-M9: loading/removing files in RAG via `/rag add|remove` commands (done)
- Post-M9: smart RAG chunking (overlap + markdown) + stitching on retrieval (done)
- Post-M9: RAG — `/rag list`, configurable chunking, `/rag rebuild` (done)
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
- Post-M9: `/reindex` rebuilds an attachment index that is missing entirely (done)
- Post-M9: `/file remove` and `/image remove` refuse a name two items share (done)
- Post-M9: `/file open 1` — a bare number is the listed `#N` (done)
- Post-M9: a fetched page and its birth turn — no search it cannot have, and a cut that says where (done)
- Post-M9: a fetched page searchable in its birth turn — the index starts at the round's end (done)

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

### Post-M9: RAG — indexing HTML sources (stage B1a) (done)
- **First stage of the "RAG: sources and retrieval" track** (design doc
  [docs/history/rag-sources-retrieval.md](../../docs/history/rag-sources-retrieval.md) §B1a,
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
  [docs/history/rag-sources-retrieval.md](../../docs/history/rag-sources-retrieval.md) §B2a,
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
  [docs/history/rag-sources-retrieval.md](../../docs/history/rag-sources-retrieval.md) §B1b,
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
  [docs/history/rag-sources-retrieval.md](../../docs/history/rag-sources-retrieval.md) §B2b,
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
  Research + plan — [docs/file-attachments.md](../../docs/file-attachments.md); forks
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
- **Completes the track** ([docs/file-attachments.md](../../docs/file-attachments.md)).
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
  [docs/research/embedding-model-change-reindex.md](../../docs/research/embedding-model-change-reindex.md),
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
  [docs/research/embedding-model-change-reindex.md](../../docs/research/embedding-model-change-reindex.md)
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
  tests "failed", six with `Cargo.toml`/`Cargo.lock`/`assets` **not found**
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
  [docs/research/embedding-model-change-reindex.md](../../docs/research/embedding-model-change-reindex.md)
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
  ([docs/research/embedding-input-prefixes.md](../../docs/research/embedding-input-prefixes.md),
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


### Post-M9: `/reindex` rebuilds an attachment index that is missing entirely (done)
- **The gap, and where it came from.** Stage 2 of the embedding-model track
  built `/reindex` around a work queue defined by the generation marker: a row
  whose `embed_gen` is not current. That is the right queue for a model change
  — the rows are all still there, they only need new vectors — and its own
  module doc says so: "attachments … the rows stay available for `/reindex` to
  rebuild without the user re-attaching anything." The audit of "chat files
  moved to another machine without `data.db`" found the case the queue cannot
  see: **no rows at all**. The attachment's snapshot text is in the chat file
  and travels with it, but the index does not, so the file was searchable
  before the move and unrecoverable after it — `/reindex` had nothing to work
  from, and the only route back was `/file attach` of an original that is on
  the other machine. Everything else degraded correctly (the pinned block stops
  offering `attachment_search`, the tool answers `not_indexed`, `attachment_read`
  keeps paging), so nothing was broken — only permanently poorer.
- **A stage, not a second command.** It runs inside `/reindex`, first, because
  the command already means "bring every vector store back in line", already
  has the banner, the cancellation and the resumability, and is what every
  document and the startup notice already point at. A separate command would
  have split one repair across two names.
- **Disjoint by construction.** The new query is
  `Db::attachment_known_ids` — any row, any generation — deliberately *not*
  `attachment_indexed_ids`, which answers "searchable now". A file whose rows
  are merely from an older model is the re-embed queue's work; a file with no
  rows is the backfill's. Asking the searchable question would have made a
  model change put every attachment through both paths: re-chunked by one,
  counted into the queue of the other, which would then come up short of the
  total the banner was promised. Both queries and the boundary between them are
  pinned by tests.
- **One writer for the rows.** The chunk-embed-insert core came out of
  `spawn_attachment_index` as `attachments::index_attachment`, and both callers
  now go through it — the file the user attaches today and the file the repair
  restores later cannot drift apart in chunking, in the replace-don't-duplicate
  delete, or in the row shape. The embedder precheck stayed in the attach
  wrapper on purpose: `/file attach` needs it (usually there is no embedder at
  all), while `/reindex` has pinged once for the whole job and must not ping per
  file. One deliberate order change came with the extraction: the attach path
  now prechecks the embedder *before* chunking rather than after, so a 32 MB
  file is not chunked to discover there is nowhere to send it. The only
  behavioural difference is for text that yields no chunks at all with no
  embedder configured — silent before, a skip note now, and the note is the more
  honest of the two.
- **What the scan costs.** Knowing whether a chat has attachments means parsing
  its file: the stat-only walk gives ids, and nothing indexes "which chats have
  attachments". So the scan parses every chat file once — the same order of work
  as the search index's first pass (~94 ms for a corpus of 171 chats), paid only
  on an explicit `/reindex`, which is a rare and already expensive command.
- **Memory, and why the chat files are read twice.** The scan keeps ids only.
  An attachment's text is what makes it big (up to 32 MB each), so holding every
  missing file's text just to *plan* the work could cost more than the work; the
  second pass loads one chat at a time. The re-read pays for itself: an
  attachment removed between planning and doing is simply not found, and not
  rebuilt.
- **Two counters, because one number cannot be both.** A re-embed stage writes
  one vector per unit of work; the backfill's unit is a *file*, which is many
  vectors. So the banner counts units against a total fixed before the work
  starts, and the closing note counts vectors actually written. Pinned by a
  fixture whose file spans several chunks: `Started { total: 1 }` and
  `rows > 1` in the same run.
- **Scope.** By-reference only (an inline attachment is never indexed — its
  whole text is in every request, so search would return duplicates of what the
  model can see) and visible chats only (a soft-deleted conversation must not
  have work done for it, let alone become searchable again).
- **Tests**: 2524 green (+6; 2517 on Linux, where the Windows-only tests do not
  compile). The rebuild end to end (searchable again, and the text really in the
  index); inline and hidden left alone; a mixed chat where only the by-reference
  half is rebuilt; the disjointness — a file whose rows are only stale keeps
  them verbatim and is not re-chunked; an attachment gone since the scan, which
  is skipped without an error while still spending its unit; and
  `known` vs `indexed` diverging at the DB level after a generation bump. Five
  mutations checked (stage disabled; the scan asking the searchable question;
  both scope filters; the per-attachment one alone) — each fails the test that
  should catch it. No live run: the job's live smoke
  (`reindex_restores_retrieval_after_a_model_swap_live`) covers the re-embed
  path and is unchanged; the backfill is chunking plus the same insert the
  attach path already smokes, over a mock embedder that is deterministic where a
  real one would only add noise.

### Post-M9: `/file remove` and `/image remove` refuse a name two items share (done)

**What.** The item the page-attachment-name track left: `/file remove <name>` acted on the
first attachment of that name. Research and forks:
[docs/research/remove-by-shared-name.md](../research/remove-by-shared-name.md) — the
user's decisions of 2026-09-11, every fork as recommended (F1a a shared name refused with
its candidates, F2a the source shown where a name is shared, F3a `/file` and `/image`
together). Branch `fix/file-remove-ambiguous-name`.

**Measured first**, through the orchestrator with `AppCommand`s. `a/notes.md` and
`b/notes.md` attached: `/file list` answered with two identical lines, and
`/file remove notes.md` answered "attachment removed — notes.md" while the next request
carried `b`'s text and not `a`'s — the first went, unnamed. Two `chart.png` staged from two
folders: `/image remove chart.png` unstaged the first, and `/image list` told them apart by
size alone. `attachment_read` already reported an ambiguous name; `/rag remove` works by
path, so it has no name to share.

**How.** `entities::attachment::resolve_handle` — `#N`, else every item `matches` accepts —
returns `Resolved::{One, Shared, Nothing}`, and both `resolve_target`s are that call over
their own `matches`. The orchestrator refuses `Shared` with each holder's `#N` and source
(`candidate_lines`, one per line); `name_is_shared` decides where `/file list` and
`/image list` show the source (`AttachmentInfo`/`ImageInfo` carry it now) and whether the
removal note names it — `FileProgress::Removed` and `ImageProgress::Removed` gained
`source: Option<String>`.

**Tests.** The resolution itself (`#N` and a path reach one, a shared name in either case
and quoted reaches both, out of range and unknown reach nothing, and which names are
shared); both commands end to end through the orchestrator — the shared name refused with
both `#N` and paths, nothing removed, `#N` removing exactly that one with its source in the
note, and the name reaching the last holder alone with none; the listings and the removal
notes on the screen. **Mutation-tested** — seventeen mutations, every one killed: `#N`
never read as a handle, a shared name resolving to its first holder or to nothing, an item
counted as sharing its own name, names compared case-sensitively, each command taking the
first holder, each removal note dropping the source (in the orchestrator and on the
screen), the refusal dropping the `#N`, each listing never showing the source, and each
card carrying none. One survived the first run — quotes around a handle left untrimmed,
invisible to `/file` because `Attachment::matches` strips them itself — and showed the
missing case: `MessageImage::matches` does not, and the orchestrator that used to strip
them for `/image remove` no longer does, so a quoted `#N` and a quoted image name are
pinned now. Not a live run: nothing here reaches the
engine, memory or a tool — the commands are the orchestrator's own, and its tests drive
them whole. Unit: 3072 green, 159 ignored (3067 / 159 before).

### Post-M9: `/file open 1` — a bare number is the listed `#N` (done)

**What.** Reported from a live chat on 2026-09-15 (v0.9.9, `ru`): `/file list` printed
`• #1 chart.png — 13.1 KB, image/png` and `#2 tool-image-1.png`, and `/file open 1`,
`/file open 2` and `/file remove 1` were each refused with only "«1» is not attached to
the chat", while `/file open chart.png` worked. The user's summary: you have to type the
name. `entities::attachment::resolve_handle` read a number only behind `#`; a bare `1` fell
through to the names and matched nothing — and the refusal described a *name*, so nothing
in it said the number wanted its `#` (lessons §4). The self-model's own resolver already
took its handle with or without the `#`. Branch `fix/file-handle-bare-number`.

**The rule.** A bare all-digit target is `#N` **only when no item answers to it as a
name**: a file really called `1` stays reachable by that name, and `#N` is still never read
as a name, so each spelling keeps one way to reach anything the other would shadow. One
core, `resolve_handle_by`, takes the name-first order and a `numbered` lookup — a position
for `/file` and `/image remove`, the carried handle for a turn's list — so the two `#`
parsers that existed (`resolve_handle` and `chat_inputs::resolve`) are one now. "All digits"
is ASCII digits and nothing else: `+1` parses as a `usize` and is not a number here.

**The model's `files` takes the same rule, and is not told about it.** The fork was whether
`python_exec`'s `files: ["2"]` should resolve or refuse. It resolves: the user and the model
name *one* list (spec §9.7), and a resolver that answered `/file open 2` and refused
`files: ["2"]` would be two rules over it. The risk that remains for the model is none the
user does not share — a name wins, and the confirmation popup states the resolved names.
The prompts still teach `#N` (`#2`, `#3`) and are unchanged: they are the measured text,
and leniency on input needs no advertising.

**The refusals close the door.** A miss is now answered by what was typed: a number with
the numbers there are (`this chat has no file #3 — its files are #1–#2, as /file list
shows`), a name with the listing (`/file list shows their names and numbers`), and a chat
with no files with `/file attach`. `/image remove` answers a missing number with the staged
range the same way (a new key; its name refusal already named `/image list`). The usage
lines, the help overlay and README keep `<name|#N>`: `#N` is the form the listings print,
and the bare number is the typo it now forgives, stated in spec §9.7.

**Folded in, same seam.** `python_exec`'s refusal of a shared name listed its candidates as
`#{position + 1}`, while inside a turn the handle is carried and outlives its position
(fork F12): after an item left mid-turn it offered `#3 notes.md` where `#3` was the
screenshot. It prints the carried handle now, tested with exactly that turn.

**Seen in the same run, no change.** After `/file remove chart.png` the remaining item
became `#1`: the listing's numbers are positions of a freshly built list, by design, so a
number read before a removal can miss — which is why the number refusal states the range.
And `/file open #2` on `tool-image-1.png` — the image `python_exec` handed back to the model
— refused as "not a file on this machine", correctly (its source is not a path), without
pointing at `chart.png` (`#1`), the stored copy holding the same bytes. Saying so would
need the image to know which stored file it came from; a refusal that guessed would name a
route that may not exist, so it stays a candidate for the file exchange's own track.

**Tests.** The resolver: a bare number reaches its item, quoted too; a file called `3` wins
over `#3`'s position while `#3` still reaches the third; `0`, out of range, `+1`, `1.5` and
an overflowing number reach nothing; `handle_number` and `handle_range`. The turn's list:
a bare number reaches the carried handle, and the handle of an item that left misses.
`/image` resolution by a bare number. `python_exec`: `files: ["2"]` stages `#2`, and the
shared-name refusal after a mid-turn departure names `#4`/`#5`, never `#3`. Through the
orchestrator, the reported sequence: `/file open 1` plans `chart.png`, `3` is refused
naming `#3` and `#1–#2`, `/file remove 1` removes, `#2` after the renumbering is refused
with `#1` alone, and the emptied chat points at `/file attach`; `/image remove 5` names the
staged range and `2` unstages the second. Not a live run: this is the command path and a
tool's argument resolution — nothing reaches the engine, memory or a model, and the
orchestrator's and the tool's tests drive both whole. Unit: 3246 green, 181 ignored (3240 / 181 before).

### Post-M9: a fetched page and its birth turn — no search it cannot have, and a cut that says where (done)

Stage 1 of [docs/research/attachment-birth-turn.md](../research/attachment-birth-turn.md);
forks decided by the user on 2026-09-26, all five at the recommendation. Stage 2 — indexing
inside the turn — is the next branch.

**The report.** The user pointed the assistant (`gpt-6-sol`) at the project's own
`spec.md` and asked why it read the pages as 1 · 9, 22, 38, 62 · 17, 19, 24, 70 · 72, 71,
13, 20 · 21, 68, 69, 11, and whether its context had been shuffled too. **It had not**:
every result is paired to its call by id, opens with its page number, and sits in call
order, and nothing reorders messages by time (the tool rows are stamped microseconds
*before* their assistant row, so a sort would have broken them). The order was the
model's, and the chat file showed what pushed it: `fetch_url` attached the page and offered
`attachment_search`; both searches answered "no search index was built"; with 72 pages
and no search it read the table of contents and guessed positions. The search could not
have worked — a tool's attachment reaches the turn's snapshot at the end of its round
(`sync_attachments`), which is why `attachment_read` works at once, but the index is
started by `insert_attachment`, which runs when the turn **lands**. The log: attached at
01:36:10, searched at 01:36:14, the turn's last reply at 01:36:48.824, the indexer's first
request at 01:36:48.826, searchable at 01:37:12. And the file was not the spec: the
400 000-character ceiling stopped it at character 399 961, inside §12.3 — §13–§17 absent,
§17 Self-model the very chapter the question was about — with a note that said only "not
the whole page" and an attachment whose last page ended mid-word.

**What changed.**
- **F1** — `fetch_url` and `youtube_watch` offer only `attachment_read` and say, in a
  sentence of its own, that `attachment_search` does not reach this attachment in this
  turn. Naming the dead route matters because the search tool's own description tells
  the model to prefer it for exactly this kind of file. "The whole page" is said only when
  it is; a cut page is attached as "what was received".
- **F2** — `ToolContext.born_this_turn`, filled by `sync_attachments` and inherited by a
  sub-agent's clone. `attachment_search` names every by-reference file it cannot see, with
  its page count and why: *attached in this turn — indexed after your reply* or *no search
  index*. When some file is indexed the search runs and ends with the same list, marked
  not searched, so "nothing found" no longer silently covers a file it never looked at. All
  files inline gets "shown in full", and an embedder that stopped answering gets its own
  sentence instead of "no index".
- **F3** — the cut is stated where the model will meet it: the result names the limit,
  the last Markdown heading before the cut (outside fences — a `# comment` in a shell block
  is not a section) and the page it starts on (`Attachment::page_at`, the page
  `attachment_read` returns), for a verbatim body the characters that never arrived, and
  the door — re-fetching returns the same beginning, tell the user which part is missing.
  The attachment carries it in its header, which the pinned excerpt repeats every later
  turn, and in a bracketed marker where its text stops.
- **F4** — `MAX_EXTRACT_CHARS` 400 000 → 1 000 000. `join_blocks` re-counted the growing
  output's characters once per block; measured on the Rust book's `print.html` (4 698
  blocks), 87 ms against 17 ms at the new ceiling in release (28 against 15 at the old) — the
  research doc's "seconds" was an estimate and was wrong by thirty times, corrected there;
  it is a running count now anyway. `PRIVACY.md` (and its translation, the site page and
  the Windows installer's two privacy pages) names the new figure, dated 2026-09-26 — the
  installer pages were first left stale, which CI's `wizard_rtf` gate caught (lessons §1).
- **F5** — `PageText.verbatim`: a body that is text is described as text, not as "extracted
  from HTML". **Found while testing it**: `text/markdown` went down the HTML path, found no
  `<p>` and failed as "no readable text"; any `text/*` that is not HTML/XML is text now.

**Live — GO.** llama.cpp b11191, `gemma-4-26B-A4B-it` Q4_1 (`-c 65536 --jinja`) and
`bge-m3` Q8_0, RTX 4090. Network smokes: the spec arrives whole (448 761 characters, 81
pages, §17 present, verbatim header, no search offered); War and Peace from Gutenberg
(3.36 MB `text/plain`) cuts at 1 000 450 characters / 172 pages with "about 2 293 649 more
characters were not received"; the Rust book's `print.html` cuts inside *Conditional `if
let` Expressions*, page 172 of 172, and gives no remainder figure. The birth-turn smoke
(`fetched_page_search_across_its_birth_turn_e2e_live`): turn 1 fetches the spec and is asked
for §17.5's `prompt_cap` default; after it lands the file is indexed (976 chunks); turn 2
asks about §9.3.1 by meaning. The first run was red on the stand, not the code: the
embedder had been started by hand without `-ub`, and a 560-token chunk exceeded llama.cpp's
512 physical batch — the managed launcher passes `-ub 8192 -b 8192`, the stand now does too.
Every birth-turn search in every run got the *attached in this turn* answer and the model
went on to `attachment_read`. What the texts do **not** do is stop Gemma trying: its first
search comes straight after the fetch, before any search answer, and across eleven runs
after the *searching again is pointless* sentence was added (from the first run, which
searched three times) it searched 1, 1, 1, 3, 1, 3, 1, 1, 2, 1, 2 times — each a cheap
local call. Turn 2's first question repeated turn 1's, and on the third run the model
answered from its own previous reply without searching; the question moved to a part of
the spec turn 1 does not read. With it, eight runs: seven GO, one red on the reply's
content (it did not state the number). Of the six runs whose turn-1 reply was kept, four
found 4000/1200 in the birth turn by sampling pages; two made eight calls — one fetch, one
search, six pages, the default `max_tool_rounds` — and ended on Gemma's raw
`<|tool_call>` markup as the reply. That is the case for stage 2: with search inside the
turn, one call finds §17.5.

**Tests.** The birth-turn and no-index answers (with and without the retry sentence),
the mixed chat's not-searched list on hits and on an empty answer, all-inline,
embedder-down; `sync_attachments` records the born id once across rounds, and a search in
the next round names it; the result offers no search as a route; the cut in the result
and in the attachment (header, excerpt, last page ending on the marker, inline path);
heading, page and remainder on a verbatim cut, no remainder figure on an HTML cut;
`last_heading` over fences, `#[derive]`, `#tag`, closing hashes, indentation and a
paragraph-long title; `text/markdown`, `text/x-markdown`, `text/csv` verbatim. Unit: 3443
green, 202 ignored (3435 / 199 before).

### Post-M9: a fetched page searchable in its birth turn — the index starts at the round's end (done)

Stage 2 of [docs/research/attachment-birth-turn.md](../research/attachment-birth-turn.md)
(§5); forks G1–G6 decided by the user on 2026-09-26, all at the recommendation. The track
is complete.

**Why.** Stage 1 made the birth turn honest, and the live runs showed what honesty alone
leaves: the model still has to find its place in 81 pages by sampling them, and two birth
turns of six spent the whole 8-round budget that way. The index was started only when the
turn landed; everything it needed was already in the loop's hands at the end of the round
that attached the page.

**What changed.**
- **G2 — the start.** `sync_attachments` returns the ids it mirrored for the first time,
  and `start_attachment_indexes` spawns the same `spawn_attachment_index` the landing
  used, right there. `features::attachment_index::IndexBoard` is the one owner of
  "started": an attachment reaches that point up to three times — its loop, a parent loop
  mirroring a sub-agent's attachment, the landing — and `begin` is claimed once;
  `insert_attachment` starts only what nobody has (`land` → `Unknown`).
- **G1 — the wait.** `attachment_search` waits for a file the board says is being built,
  up to 120 s and cancelled with the turn (`watch` counter, marked seen before the check
  so no change slips between check and `await`), then searches. Past the bound it searches
  what is indexed — the file's beginning, chunks being written in order — and lists the
  file as *still being built, done of total fragments*, with "search again later reaches
  more".
- **G3 — the note.** A feed note pushed while a reply streams opens a new bubble
  (`ensure_streaming_bubble` finds the note last), so an index finishing mid-turn would
  have split the reply. The task now sends `FileProgress::IndexEnded` (the banner comes
  down, only if it is this file's) and gives its outcome note to the board, which holds it
  until the landing: the feed reads "attached", then "searchable". An index still running
  at the landing says it itself when it ends.
- **G4** — `fetch_url` and `youtube_watch` say the index is being built and a search waits
  for it when an embedder is configured (`ToolContext.embed_configured`, from the embed
  server's status), and that the file is read by pages only when none is. **G5** — the
  pinned block offers search for a file whose index is being built (`searchable_ids`).
  **G6** — `born_this_turn` and its two sentences are gone: a file born in the turn is
  being indexed, indexed, or without an index, like any other.

**Live — GO.** llama.cpp b11191, `gemma-4-26B-A4B-it` Q4_1 (`-c 65536 --jinja`) and
`bge-m3` Q8_0 (`-ub 8192 -b 8192`), RTX 4090. `fetched_page_is_searched_in_its_birth_turn_e2e_live`
— one turn: fetch the real `spec.md`, name §17.5's `prompt_cap` default and what it was
raised from — six runs, six GO, each with **one** `attachment_search` and **no** page read,
28.5–33.2 s for the whole turn (the page summary included), 4000 and 1200 every time, and
"attached" before "searchable" at the landing. The control is stage 1's runs of the same
question: one to three searches answered "not indexed yet", four to six pages read, two of
six birth turns out of rounds. `attachment_search_e2e_live`, `attachment_read_e2e_live`,
`file_attachment_e2e_live` and the two `fetch_url` network smokes re-run green.

**Tests.** The board: one claim, a note held until the landing and released by it, a late
end speaking for itself, a landed attachment never held, a waiter woken by the end, the
bound and the cancellation (paused time). The search: waits for an index being built and
answers from it; past the bound says what it covered; a cancelled turn does not wait and
names the file as being built, not as index-less; a file with no index; the mixed chat's
list; `search_route` with and without an embedder. The loop and the landing: a round's
attachment claimed and searched in the next round with its note held; the landing
releasing a held note after "attached" and indexing nothing twice; an index still running
at the landing speaking when it ends. The UI: `IndexEnded` takes down only its own
banner and writes nothing. Unit: 3456 green, 202 ignored (3443 / 202 before).
