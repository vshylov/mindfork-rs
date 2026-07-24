# Plan: RAG — new source formats + ranking/dedup/progress

> Status: **track complete** (B1a, B1b, B2a, B2b done; branches
> `feat/rag-html-sources`, `feat/rag-pdf-docx`, `feat/rag-chunk-progress`,
> `feat/rag-cross-source-dedup`; log — CLAUDE.md post-M9). B2b, by the user's
> decision — **text-based** dedup (not embedding-based: `rag_search` is a hot
> path, vec0 already ranks). Closed two pieces of roadmap groundwork in the
> "Memory, self-model, knowledge" section (RAG). Independent of the
> "self-model" track. **Archived** (track complete).

## Starting point (current state)

- **Formats:** `features/rag_ingest.rs::SUPPORTED_EXTENSIONS = ["txt","md"]`;
  `is_supported` by extension; `read_text` — plain UTF-8 (strips BOM).
- **Indexing:** `app/orchestrator/rag.rs::index_source` already branches md/txt
  (`chunk_markdown` by headings vs `chunk_text`); chunking → batch embedding →
  write to sqlite-vec, isolated by `profile_id`.
- **Progress:** `RagProgress::Indexing { index, total, name, dir }` — granularity
  **per file** (a file's embedding runs as a single batch).
- **Extraction on search:** `features/tools/rag.rs::RagSearch` → `stitch_hits`
  merges adjacent chunks **within a source** on verbatim overlap
  (`MIN_STITCH_OVERLAP=24`); cross-source dedup/ranking **doesn't exist** (order
  is by `distance`).
- **Reference reranking pattern:** `features/tools/web.rs::rerank_by_embeddings` /
  `rerank_order` (embed the query + results, sort by cosine, stable on ties,
  graceful degradation without an embedder).

## Principles (in the project's spirit)

- **"Best effort"** — an extraction failure on one source is logged and doesn't
  fail indexing (as with web enrichment); reranking without an embedder is
  skipped.
- **Isolation by `profile_id`** — in every query (invariant).
- **License check for new dependencies** — via `deny.toml` (as with the mermaid
  crates).
- **Split by dependency risk** — formats with no new crates get their own PR,
  separate from formats that add crates.

---

## Stage B1 — new source formats

Introduce a text-extraction dispatcher by extension instead of calling
`read_text` directly. Split into two PRs by dependency footprint.

### Step B1a — HTML (no new dependencies)

- Reuse `web::extract_readable` (scraper; already `pub(crate)`) — readable
  text from `<article>`/`<main>`, dropping nav/header/footer/aside.
- `SUPPORTED_EXTENSIONS` += `html`/`htm`; `read_text` → `extract_text(path)`
  with a dispatcher (txt/md → as-is; html → `extract_readable`).
- `index_source` (`orchestrator/rag.rs`) already branches by extension — add a
  branch.
- **Clean win, low risk.** Tests: readable-text extraction from an HTML
  fixture on tempdir (nav/scripts dropped).

### Step B1b — PDF/DOCX (new dependencies)

- **PDF:** `pdf-extract`/`lopdf` crate — text extraction; quality varies
  (truncation/garbage acceptable — "best effort", log it).
- **DOCX:** `docx-rs` or zip+xml parsing (`document.xml`).
- `extract_text` grows branches; extracted text → `chunk_text` (pdf/docx have
  no heading structure).
- **`Cargo.toml`/`deny.toml`** — new crates + licenses in the allowlist. Tests:
  fixtures for each format on tempdir; no network/live run needed.

**Risk:** medium (PDF extraction quality); mitigated by "best effort" + logging.

---

## Stage B2 — cross-source ranking/dedup + per-chunk progress

### Step B2a — per-chunk indexing progress

- Currently `RagProgress::Indexing` is per-file. Add "chunk i/N within file"
  granularity (a `chunk`/`chunk_total` field on `Indexing`, or a separate
  variant).
- `spawn_rag_ingest`/`index_source` emit progress between chunks (the embedding
  is currently a single batch per file — either split into sub-batches, or
  emit after writing each chunk).
- `screens/chat/rag.rs` — banner shows chunk progress. Low risk, UI plumbing.

### Step B2b — cross-source dedup/ranking in `rag_search`

- On top of `stitch_hits` (within-source), add filtering of near-duplicate
  passages from **different** sources by cosine + optional query reranking
  (reuse the `rerank_by_embeddings`/`rerank_order` pattern from web.rs).
- **Graceful degradation** without an embedder — keeps `distance` order (as
  with web/RAG reranking).
- Tests: a near-duplicate from two sources collapses; stable on ties; without
  an embedder — order unchanged.

**Assessment:** B2a — fast and useful; B2b — a search-quality improvement (the
roadmap places it last in this section).

---

## Out of scope (future work)

- **Extraction from binary formats beyond pdf/docx** (rtf/epub/pptx) — on
  demand.
- **`/rag list` with content preview**, byte-level progress — beyond this
  scope.
- **Cross-profile dedup** — `profile_id` isolation is preserved, dedup stays
  within a profile.

## Order and DoD

1. **B1a** (HTML) — fast, no dependencies.
2. **B2a** (per-chunk progress) — independent, small.
3. **B1b** (PDF/DOCX) — with license checks.
4. **B2b** (ranking/dedup) — per the roadmap above.

DoD for each stage: `cargo fmt`/`clippy -D warnings`/`test` green; new crates
in `deny.toml`; entry in CLAUDE.md + CHANGELOG; fixture tests for
extraction/dedup.
