# Plan: vec0 for notes and observations (perf, conditional)

> Status: **DEFERRED** (user's decision, after tracks A and B of the "Memory"
> section are done). Reason — premature optimization: brute-force cosine is
> cheap up to thousands of notes (single-digit ms on `note_recall`), vec0 would
> speed up **only the query path** (not the O(n²) consolidation, see
> §Key nuances), and the implementation requires a schema migration for a
> stable integer rowid (notes use a `TEXT` uuid PK).
> Revisit **once the note count actually grows into the thousands**. The plan
> below is implementation-ready; the document stays in `docs/` (not archived —
> the track isn't done).

## Task nerve

**Note** embeddings (and `@self` observations) are stored as JSON text, cosine
is brute-force in Rust:

- `shared/storage/db/notes.rs`: `note_vectors(note_id PK, profile_id, embedding TEXT)`;
  `note_search_semantic` scans all vectors for the profile and computes `cosine`
  in Rust; `notes_with_vectors` returns all pairs for pairwise consolidation.
- **RAG** for comparison already uses `vec0`: `shared/storage/db/rag.rs` —
  `rag_vectors` (a `vec0` virtual table, partition key `profile_id`, kNN via
  `MATCH ... AND k = ?`), dimensionality fixed globally (`meta.rag_dim`).

For dozens–hundreds of notes brute-force is **cheap** — roadmap says outright:
do it "on a multiple-fold growth." This is a perf optimization, not a
user-facing feature.

## Key nuances (must account for in the implementation)

1. **vec0 only helps the query path.** It speeds up `note_search_semantic`
   (`note_recall`) and `self_notes_relevant` (relevant-observation injection into
   the prompt) — this is kNN "query → top-k". **Pairwise dedup** in consolidation
   overviews (`notes_with_vectors`, O(n²) over all pairs in `overview.rs`) —
   **is not a kNN query**, vec0 doesn't cheaply replace it. This is the main
   "why and when" argument: as the note count grows, recall latency hurts
   first, not consolidation. The pairwise path could be reformulated as
   per-note kNN, but that's a separate decision.
2. **vec0 dimensionality is per-table.** Needs a **separate** `note_vectors_vec`
   with its own dimensionality key (`meta.note_dim`), not shared with RAG. Same
   bge-m3 → same dimensionality, but tables stay separate (notes/RAG are
   independent; changing one's embed model shouldn't break the other).
3. **Migrating existing JSON vectors → vec0** — a real data migration
   ([ADR 0006](decisions/0006-data-schema-versioning.md)): either a `DB_STEP`
   (reads `note_vectors`, dimensionality from the first JSON vector, populates
   `note_vectors_vec`), or a lazy backfill on first access. **A hybrid is
   recommended:** keep `note_vectors` (JSON) as durable storage + a parallel
   `vec0` index for the query path; the pairwise path (`notes_with_vectors`)
   stays on JSON. This makes the migration additive and reversible, not a
   "rewrite everything."
4. **Isolation and sync.** `note_vectors_vec` — partition key `profile_id` (like
   RAG). `note_vector_upsert`/`note_delete` write/clear **both** tables in sync.

## Principles (in the project's spirit)

- **Additive, no risk to existing data** — durable JSON stays, vec0 is an
  add-on; the backfill is idempotent.
- **Isolation by `profile_id`** — a mandatory partition key/`WHERE` (invariant).
- **Graceful degradation** — without a vec0 index (before backfill), the query
  path falls back to brute-force JSON.

---

## Implementation sketch

### Step 1 — schema

- `db/mod.rs` — `ensure_note_vec_table(conn, dim)` (mirrors `ensure_vec_table`),
  key `meta.note_dim`; `CREATE VIRTUAL TABLE note_vectors_vec USING vec0(profile_id
  TEXT partition key, embedding float[dim])`. Lazily created on first insert.

### Step 2 — writers

- `note_vector_upsert` — write both to JSON `note_vectors` (durable) and to
  `note_vectors_vec` (by the note's rowid/its own rowid).
- `note_delete` — clear both tables.

### Step 3 — query path

- `note_search_semantic` — if a vec0 index exists and dimensionality matches, go
  through kNN via `MATCH ... AND k = ?` (like `rag_search`); otherwise — the
  previous brute-force JSON path (graceful degradation before backfill).
- `self_notes_relevant` (`notes/self_notes.rs`) — same path.

### Step 4 — backfill

- Lazy: on first query access, populate `note_vectors_vec` from JSON
  `note_vectors` (dimensionality — from the first vector). Or a `DB_STEP`
  (ADR 0006) with a golden fixture.

### Step 5 — pairwise path (leave as is)

- `notes_with_vectors` / consolidation overviews (`overview.rs`) — **don't
  touch**: O(n²) pairwise dedup doesn't benefit from kNN. Document that vec0 is
  about recall, not consolidation.

## Tests

- kNN over notes is isolated by profile (mirrors `rag_knn_respects_profile_isolation`).
- Backfill populates vec0 from JSON and is idempotent.
- Changing the notes' embed model (different dimensionality) — reset/recreate,
  doesn't affect RAG vectors (and vice versa).
- Degradation: without a vec0 index, `note_search_semantic` works through JSON.

## Out of scope (groundwork)

- **Porting pairwise consolidation to per-note kNN** — a separate decision, if
  the O(n²) actually becomes a bottleneck.
- **A shared vec0 table for notes and RAG** — deliberately NOT doing this
  (would couple the dimensionality/lifecycle of two organs).

## When to do it

**Last** among the memory tracks, or once the note count actually grows into
the thousands. Before that, brute-force is cheap; premature optimization isn't
warranted.
