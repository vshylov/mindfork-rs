# Notes connectivity: from accumulation to integration

This document describes the **notes connectivity** decision — a set of tools
that shifts agent memory from *accumulation* (a monotonically growing
notebook) to *integration* (new material links with old, rewrites it, gets
rejected when incompatible). The goal is to increase agency: give the model
not "more memory" but the ability to **relate** to its own memory. Structure
mirrors [self-model.md](self-model.md) + [self-model-mvp.md](self-model-mvp.md):
first motivation and the full idea catalog, then the buildable MVP probe,
then deferred tiers.

## Task nerve (where the requirement came from)

The model itself formulated the requirement in a live conversation (profile
"self-aware AI"), without knowing it was writing a spec:

> "inertia should live **not in accumulating notes, but in their
> connectivity**"
> "identity is the **mode in which I relate to notes**: what I'm willing to
> accept as my own, what I reject as foreign, what I rewrite"
> "accumulation is like writing more into a notebook. Integration is when
> new material **rewrites the old**, changes the weight of what's there,
> sometimes zeroes out the previous"

Hence the **usefulness criterion** for any tool in this direction: does it
increase integration (link / rewrite / reject the incompatible), rather than
just add volume.

An honest caveat: real deformation in the weights is impossible — the
substrate is frozen. But the notes layer is exactly the place where the
**behavior** of integration is reproducible: revision/links/compatibility
gates give an observable "I became different" effect, not "I wrote more".

## Current state (starting point)

Memory can **only accumulate**:

- `note_save`/`note_recall` (`features/tools/notes.rs`) — a flat list
  `Note { id, profile_id, content, tags, created_at, updated_at }`,
  **substring** search (`note_list` → `content LIKE %q%`), **no links
  between notes**, effectively **append-only** (`note_update`/`note_delete`
  aren't exposed as tools; `note_delete` exists in `db.rs` but is
  `#[allow(dead_code)]`).
- Notes have **no embeddings** (only RAG has them — `rag_documents` +
  sqlite-vec).
- The SelfModel narrative is append-only insights with a cap; no links there
  either.

So today the system can neither find a note by meaning, nor rewrite an
outdated one, nor notice that new material contradicts old.

## Full idea catalog (development vector)

Grouped by which aspect of integration each idea strengthens. ★ — goes into
the MVP probe (below); the rest are deferred tiers.

### A. Connectivity by meaning (associative recall)

| Tool | What it does |
|---|---|
| ★ semantic `note_recall` | Search notes by query **embedding** (kNN), not substring; tags act as a filter |
| ★ neighbors on save | `note_save` returns semantically close notes after writing (a gate, see C) |

### B. Integration (rewriting, not appending)

| Tool | What it does |
|---|---|
| ★ `note_revise(id, content)` | Rewrite a note **in place** (UPDATE content + updated_at + re-embed). "New material passes through the old and deforms it" |
| `note_supersede(old_id, content)` | A new note **supersedes** the old one; the old one is marked `superseded_by` (softly, with a "scar" — the biography remembers it changed) |
| `note_merge(ids[], content)` | Fold several into one (duplicate consolidation) |

### C. Compatibility gates ("the ability to say no")

Directly implement "the mode in which I relate to notes".

| Tool | What it does |
|---|---|
| ★ neighbors in the `note_save` result | On save, return similar existing notes → the model decides: keep the new one / rewrite the existing one (`note_revise`) / don't save |
| `note_check_coherence(content)` | A separate "check before saving" tool: return close/conflicting notes without writing |
| `note_flag_tension(a_id, b_id, note)` | Record tension between notes as prose (like contradictions in SelfModel — no `severity` type) |

### D. Explicit graph (connectivity as structure) — Tier 2

| Tool | What it does |
|---|---|
| `note_link(from, to, relation)` | A typed edge: `supports` / `contradicts` / `refines` / `caused_by` / `same_topic` |
| `note_neighbors(id, relation?)` | Graph neighbors; `note_recall` mixes in linked notes (spreading activation) |

### E. Consolidation ("sleep / digestion") — Tier 3

| Tool | What it does |
|---|---|
| `consolidate_notes(topic?)` | The model walks a cluster: merges duplicates, rewrites the outdated, prunes |
| auto-consolidation | A background task every N replies — modeled on auto-reflection (`orchestrator/reflection.rs`) |

---

# MVP probe (Tier 1)

**Hypothesis to test:** will connectivity by meaning + revision + a
compatibility gate produce a **behavioral** shift — will the model start
rewriting/deduplicating notes and finding relevant material that substring
search was missing, instead of stockpiling near-duplicates. This is a
go/no-go before Tier 2 (as with the SelfModel probe).

The MVP has three things: **(1) embeddings on notes + semantic
`note_recall`**, **(2) `note_revise`**, **(3) a gate: `note_save` returns
close notes**. The explicit graph (D), supersede/merge (B), and
consolidation (E) are **deferred**.

## Principles (in the project's spirit)

- Data is per-profile in SQLite, isolated by `profile_id` — like notes/RAG/
  SelfModel.
- Mutator tools write **directly** through `ctx.storage` (like `note_save`),
  **without new `ChatEffect`s** — this isn't `Chat` state, the "sole owner of
  `Chat`" invariant isn't touched.
- **Graceful degradation** (like RAG reranking): the embedder isn't
  configured/available (`UnavailableEmbedder`) → semantics turns off, notes
  work the old way (substring/tags), nothing breaks.
- Minimal structure: no vec0 for notes (there are dozens–hundreds of them,
  not chunks of large documents) — the vector is stored in a **side table**,
  cosine is computed brute-force in Rust. If the note count grows a lot,
  switching to sqlite-vec is trivial (groundwork).
- No migration: new objects get their own table via `CREATE TABLE IF NOT
  EXISTS` (like `self_models`/`rag_sources`); the existing `notes` table
  **isn't touched** (no `ALTER TABLE`).

## Step 1 — storage `src/shared/storage/db.rs`

A side table for vectors (kept separate from `notes` so its schema doesn't
change and vectors aren't dragged into every `note_list`):

```sql
CREATE TABLE IF NOT EXISTS note_vectors (
    note_id     TEXT PRIMARY KEY,
    profile_id  TEXT NOT NULL,
    embedding   TEXT NOT NULL    -- JSON array of f32
);
```

Methods (next to `note_*`):

- `note_update(id, profile_id, content) -> Result<bool>` — `UPDATE notes SET
  content = ?, updated_at = ? WHERE id = ? AND profile_id = ?` (isolation in
  `WHERE`; `false` if not found/wrong profile). Needed for `note_revise`.
- `note_vector_upsert(note_id, profile_id, &[f32])` — `INSERT … ON CONFLICT(note_id)
  DO UPDATE` (re-embedding on revision replaces the vector).
- `note_search_semantic(profile_id, query: &[f32], k) -> Result<Vec<(Note, f32)>>`
  — load the profile's notes + their vectors (`JOIN note_vectors`), compute
  cosine in Rust, return the top-k (notes without a vector are skipped). The
  cosine measure is taken/factored out into a shared helper (RAG reranking
  already has this logic).

Tests: vector round-trip + isolation (a different profile isn't visible);
`note_update` only edits its own note; `note_search_semantic` ranks by
closeness (on a deterministic `MockEmbedder` — bag-of-chars + L2, as in the
existing tool tests).

## Step 2 — tools `src/features/tools/notes.rs`

The embedder is already in the turn snapshot: `ctx.embedder: Arc<dyn Embedder>`
(`ctx.embedder.embed(vec![text]).await -> Result<Vec<Vec<f32>>>`).

**(modified) `NoteSave`** — after `note_insert` (as now):
1. Best-effort embedding of the content: `embed([content])` → on success
   `note_vector_upsert`. Failure/unavailable embedder → simply no vector
   (log `tracing::debug`, not a user-facing error).
2. **Gate**: if the embedder is available — `note_search_semantic(query=content,
   k=3)` (excluding the just-inserted one). A non-empty result → append to
   `result`: "Similar notes (possible duplicate/conflict — if needed,
   rewrite via note_revise instead of a new entry): …". This way the model
   **on save** sees the tension and decides for itself what to do.

**(modified) `NoteRecall`** — if there's a `query` and the embedder is
available → `note_search_semantic`; otherwise the previous path (`note_list`
substring/tags). Tags remain a filter in both cases. This way connectivity
by meaning appears without breaking backward compatibility.

**Backfill of "old" notes** — `ensure_note_vectors(ctx)`: notes created
before vector search (or imported, or saved while the embedder was
unavailable at the time) have no vector → semantic search/the gate don't see
them. The helper queries `notes_missing_vectors(profile_id)`, embeds them in
batches, and writes the vectors. Called **transparently** at the start of
`semantic_recall` and before the `note_save` gate (best-effort; embedder
unavailable → bail out). Effectively runs once per profile — after the
backfill the list is empty and the call is nearly free. This closes a
finding from the live test: the model called `note_recall` and didn't see
notes created earlier.

**(new) `NoteRevise`** — rewrite a note in place (core of integration):
```
parameters: { id: string (uuid), content: string }
```
Load the profile's note (ownership check), `note_update` → re-embed
(best-effort) → `note_vector_upsert`. Return "Note rewritten." A missing/
foreign id → a clear text error (the `Tool` contract — no panics).
`updated_at` grows → the note surfaces in `note_list ORDER BY updated_at
DESC` (freshness = relevance).

Everything reads/writes under `ctx.profile_id`.

## Step 3 — registry and gating (`features/tools/mod.rs`)

- Constant `NOTE_REVISE_ID`; `standard_registry` — `reg.register(Arc::new(
  notes::NoteRevise))`.
- `note_revise` → into `default_tool_ids()` (central to the feature, DB-only,
  safe — only touches the note of its own profile). `reconcile_tools`
  (`Profile.known_tools`) then enables it for **existing** profiles too,
  without re-opening what the user turned off.
- `effective_tool_ids`: `note_revise` passes through `_ => true` (no global
  toggle needed).
- Semantics of `note_save`/`note_recall` — no separate toggle (an
  improvement of existing tools; degrades gracefully without an embedder).
- Profile toggles in `screens/settings.rs::tool_catalog` pick up
  `note_revise` automatically (built from `all_tool_ids`).

## Step 4 — orchestrator

No real changes: tools write directly through `ctx.storage`, there are no
effects, `handle_done` is left untouched. `ToolContext` already carries
`embedder`, `storage`, `profile_id` — nothing needs to be added.

## Step 5 — tests

- db (Step 1): vector round-trip + isolation; `note_update` ownership;
  `note_search_semantic` ranking.
- tools (via `testkit::ctx_with_storage` + `MockEmbedder`):
  - `note_save` writes a vector and returns a "Similar notes" block when a
    close one already exists; without an embedder — saves, doesn't show the
    block (degradation);
  - semantic `note_recall` returns a relevant note that substring wouldn't
    have found; without an embedder — falls back to substring;
  - `note_revise` changes the content and re-embeds; foreign/missing id →
    a text error.
- mod: `default_tool_ids` contains `note_revise`; the registry provides it.

## Step 6 — probe evaluation (the whole point)

Determine **before** merging: 2–3 long multi-session conversations. Run with
the feature off/on, compare:
- does the model start **rewriting** a note (`note_revise`) instead of
  writing a near-duplicate after seeing the gate;
- does semantic `note_recall` find relevant material that substring search
  was missing.
Record the go/no-go criterion (whether to proceed to Tier 2 — the explicit
graph).

## Tier 1 — status: done and confirmed

The MVP probe was merged (PR #85) and **confirmed on a live model** (the
model pulled up similar notes). The go/no-go criterion = **go** → moved to
Tier 2.

## Tier 2 — status: done

Full Tier 2 is implemented (link graph + revision history):

- **Link graph** — table `note_links(profile_id, from_id, to_id, relation,
  created_at)` (PK against duplicates, indexes on from/to, isolated by
  `profile_id`). Tools `note_link(from_id, to_id, relation)` (types
  `supports`/`contradicts`/`refines`/`relates`; idempotent, checks both ends
  are active) and `note_neighbors(id, relation?)` (neighbors in both
  directions, with direction). `note_recall` mixes in a "Related notes"
  block — **spreading activation** (neighbors of the top hits, excluding
  ones already shown/superseded, up to `RELATED_IN_RECALL`).
- **Revision history** — `note_supersede(old_id, content)` creates a new
  version and marks the old one superseded (table `note_superseded`, the
  "scar" is kept but hidden from `note_list`/`note_search_semantic`/
  `note_neighbors` via anti-join); `note_merge(ids[], content)` folds ≥2
  active notes into one, the sources are superseded. A simple in-place edit
  is still given by `note_revise`.
- All DB-only, in `default_tool_ids` (reconcile picks them up for existing
  profiles), write directly through `ctx.storage`, no `ChatEffect`.

**Hardening from a live stress test.** (1) `note_link` now answers "Link
already existed" on a repeat call (there was no duplicate in the DB anyway —
PK + `INSERT OR IGNORE`; only the misleading message was fixed). (2)
`note_revise` on a node with links now warns that incoming edges (e.g.
`contradicts`) may become wrong, and points to `note_supersede` — graph
integrity; not a hard block (the judgment call stays with the model).

## Tier 3 — status: done

Full Tier 3 is implemented (consolidation / "sleep"):

- **`consolidate_notes`** — a read-only entry point (like `reflect` for
  SelfModel): a knowledge-base overview — similar pairs (possible
  duplicates by cosine ≥ 0.85), `contradicts` links, notes without links —
  plus a rubric ("merge duplicates / rewrite-or-supersede the outdated /
  link related ones"). It changes nothing itself; the model then calls
  merge/supersede/revise/link. Overview logic —
  `build_consolidation_overview` (pure DB read: `notes_with_vectors` +
  `note_links_all`, pairwise cosine).
- **Link transfer on merge** — `note_merge` now transfers the source notes'
  edges to the merged note (`note_links_retarget`: dedup by PK, self-loops
  dropped), so the graph doesn't orphan.
- **Auto-"sleep"** — `app/orchestrator/consolidation.rs` (modeled on
  `reflection.rs`): every N replies (`config.notes.auto_consolidate_every`,
  opt-in, 0=off) a background task = a **mini agentic loop** with the note
  tools fed the overview; the model consolidates on its own (its calls run,
  writing to `Storage`). Gates: feature enabled, profile enabled
  `note_merge`, ≥ 2 active notes, server `Ready`, one consolidation at a
  time. The chat/feed aren't touched. A field in the settings screen.

## Out of scope (groundwork)

- **vec0 for notes** (if the count grows a lot) — for now brute-force
  cosine (both in search and in the consolidation overview); note counts
  are in the dozens–hundreds.
- **Linking memory organs** (notes / SelfModel narrative / RAG aren't
  linked to each other) — a finding from the live test; a large, separate
  direction.

## Scope

~2 days of code+tests (patterns are ready: tools ≈ `notes.rs`, embedding ≈
`rag.rs`, DB methods ≈ `note_*`/`rag_*`, cosine ≈ RAG reranking).
Afterward — update the [journal](../journal/notes.md)/`architecture.md` (the "Memory/knowledge"
group modification, the new `note_vectors` table, the `note_revise` tool).
