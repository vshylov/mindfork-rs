# Research — reindexing the vector stores after an embedding-model change

**Status:** decided and implemented, except stage 3. Forks R1–R7 (§7) —
**user's decision, 2026-07-27: R1a–R7a**, all as recommended. **Stage 1 of §8
(detection and honesty) is done** — canary fingerprint in `meta`, the check on
first embedder use, per-store invalidation, `rag_search` refusing over a stale
knowledge base (spec §9.3.4, `shared/embed_identity.rs` +
`app/orchestrator/embed_guard.rs`). **Stage 2 (re-embed in place) is done** —
embedding generations instead of deletion, plus the DB-global `/reindex` job
that drains the queue they define (`shared/storage/db/embed_gen.rs`,
`app/orchestrator/reembed.rs`, `features/reindex_command.rs`); its
sub-decisions S1–S5 are recorded in §8.1 below. Stage 3 (per-model thresholds)
is open — see [roadmap](../roadmap.md). Research date: 2026-07-27.

**Question.** What happens to the stored vectors when the embedding model
changes, and how should the application detect it and reindex?

**Related:** [ADR 0002](../decisions/0002-embeddings-dedicated-server.md)
(dedicated embedding server, where "switching the model requires reindexing"
was first deferred), spec §9.3 (`/rag` commands) and §9.7 (chat attachments),
[roadmap](../roadmap.md) (the attachment-reindex groundwork item this research
partly supersedes — see §7).

All numbers below are **measured** on a live pair of local `llama-server`
instances (2026-07-27), not estimated:

- `bge-m3-Q8_0.gguf` on `:8001` — the model the project is calibrated against;
- `multilingual-e5-large-instruct-q8_0.gguf` on `:8002` — a plausible swap.

---

## 1. The finding that drives everything

**Both models are 1024-dimensional.** Dimensionality is currently the *only*
signal the application has for "the embedding model changed" — so a swap
between these two is completely invisible to every existing guard:

- `ensure_dim` compares `1024 == 1024` → no error, new vectors are written
  straight into the old index;
- `/rag rebuild` computes `dim_changed = current_dim != new_dim` → `false` →
  it never resets anything;
- nothing else looks at model identity at all.

The two models are nevertheless **different vector spaces**. Embedding the
*same* text with both and comparing gives a cosine of only **0.37–0.44**:

| same text, embedded by both models | cosine |
|---|---|
| a Russian sentence about the capital of France | +0.357 |
| a Russian sentence about a cat on a windowsill | +0.428 |
| an English sentence about Rust | +0.440 |
| a Russian sentence containing a build code | +0.391 |

So after a same-dimension swap, every stored vector is noise relative to
every new query. Retrieval on a 4-document corpus:

| | top hit | runner-up | margin |
|---|---|---|---|
| query bge-m3 vs index bge-m3 (correct) | **+0.732** | +0.283 | **0.449** |
| query e5 vs index bge-m3 (after a swap) | +0.315 | +0.166 | 0.149 |

The ranking happens to survive with four documents, but the margin collapses
by 3x and the absolute score of the *correct* hit (0.315) drops below the
score of an *irrelevant* hit in the healthy run (0.283). On a real corpus —
the 1441-fragment collection from the attachment-index live run — a 3x margin
collapse is misranking, not a curiosity. And it is **completely silent**: no
error, no warning, no dimension mismatch.

---

## 2. What is actually affected

Three tables hold embeddings. The current model change story covers **one**:

| store | vectors | source text for re-embedding | fixed by `/rag rebuild` today |
|---|---|---|---|
| `rag_documents` + `rag_vectors` | RAG chunks | `rag_sources.content` (in DB) | **yes** |
| `note_vectors` | notes **and `@self` observations** | `notes.content` (in DB) | **no — never touched** |
| `attachment_documents` + `attachment_vectors` | chat attachment chunks | `attachment_documents.chunk_text` (in DB) | only on a dimension change (dropped, not rebuilt) |

### 2.1 Notes are the worst case, in both directions

`note_vectors` is a side table of JSON f32 arrays compared brute-force by
`db::cosine`, and **nothing re-embeds an existing note, ever**:
`notes_missing_vectors` returns only notes with *no* vector row, so
`ensure_note_vectors` backfills but never refreshes. `/rag rebuild` does not
touch the table.

- **Same dimension (bge-m3 → e5):** stale vectors are silently mixed with new
  queries. `note_recall`, spreading activation, the self-note relevance
  injection and every duplicate gate quietly degrade.
- **Different dimension (e.g. → a 768-d model):** `cosine` returns `0.0` on a
  length mismatch. Semantic recall then scores *everything* at 0.0, sorts by a
  constant, and returns arbitrary notes as "semantically relevant"; the
  `add_insight` and `note_save` duplicate gates never fire again. RAG in the
  same situation fails **loudly** (`embedding dim mismatch` on insert). Notes
  fail **silently**.

This is the most valuable single fix in the whole area: memory-about-self is
the project's flagship track, and it degrades without a single symptom.

### 2.2 Attachments need no chat walk

The roadmap item on attachment reindexing assumes it "needs a walk over all
chats". It does not: `attachment_documents.chunk_text` is stored in the DB, so
re-embedding is a plain `SELECT` + embed + update. A walk is only required to
re-*chunk* — which a model change does not require (§3).

---

## 3. Re-embedding is not re-chunking (the core design point)

`/rag rebuild` today does one fused operation: re-read sources → re-chunk →
re-embed → replace. That is right for a **chunking-parameter** change. It is
the wrong shape for a **model** change:

|  | needs source text | needs chunker | preserves chunk ids |
|---|---|---|---|
| re-chunk + re-embed (`/rag rebuild` today) | yes | yes | no (new uuids) |
| **re-embed in place** (what a model change needs) | **no** | **no** | **yes** |

Re-embedding only needs the chunk text, and all three tables already store it.
That makes the model-change operation **uniform across all three stores**,
independent of the chunker, and it:

- works for legacy RAG rows whose source file is gone (today `/rag rebuild`
  counts those as errors and drops them);
- covers attachments without touching chat files;
- keeps `rag_documents.id` stable, so nothing downstream is invalidated;
- is naturally idempotent and resumable if rows are tagged (R4).

Separating these two concerns is the recommendation this research is built
around. `/rag rebuild` keeps its current meaning (chunking changed);
re-embedding becomes its own operation over the whole DB.

---

## 4. Detection: how do we know the model changed?

| # | mechanism | catches a same-dim swap | catches a file replaced in place | works for external/cloud | cost |
|---|---|---|---|---|---|
| D1 | dimensionality (today) | **no** | no | yes | free |
| D2 | config fingerprint (mode + model path / model name / url) | yes | **no** | weak for external | free |
| D3 | server-reported model id | yes | depends | yes | one request |
| D4 | **canary vector** | **yes** | **yes** | **yes** | one embed call |
| D5 | D4 + D3 stored for display | yes | yes | yes | one embed call |

**D4 measured.** Store the embedding of a fixed short string in `meta`; on
startup embed it again and compare:

| comparison | cosine |
|---|---|
| bge-m3, two separate calls | **1.000000** |
| bge-m3, alone vs. inside a 4-item batch | **1.000000** |
| e5, two separate calls | **1.000000** |
| **bge-m3 vs e5** | **0.368940** |

A separation margin of **0.63**. A threshold anywhere around `0.999` is safe,
and the check is bit-stable across repeat calls and batch positions.

The canary is the only mechanism that detects what config cannot: the same
GGUF path re-pointed at a different file, a requantization, or a server
restarted with different pooling/normalization flags — all of which silently
change the vector space.

D3 is worth storing alongside it for a human-readable message: both servers
volunteer the model in the response body and at `/v1/models` (here, the full
GGUF path). It is display metadata, not the trigger — a generic id like
`"gpt"` or an unchanged name after a file swap makes it unreliable alone.

**When to check.** Embeddings are deliberately lazy (ADR 0002 — `apply_embed`
runs no probe), so a startup check would either block on a still-loading
managed server or run before it is up. The natural point is the **first
embedder use after launch** (or after embed settings change), with the result
cached for the process.

---

## 5. Reindex policy: what happens when a change is detected

| # | policy | UX | risk |
|---|---|---|---|
| P1 | do nothing, log only | silent corruption, today's behavior | unacceptable |
| P2 | **notify + offer**: banner/note, user runs a command | explicit, cheap | user may ignore it and keep a mixed index |
| P3 | auto-reindex in the background on detection | invisible, self-healing | a long unrequested job; embedder load |
| P4 | **degrade until reindexed**: mark stale, disable semantic paths (fall back to substring recall), reindex on command | never returns wrong results | semantic features off until the user acts |
| P5 | lazy per-row: tag each vector with the fingerprint, treat foreign rows as missing, re-embed on read | self-healing, no big job | partial index during the transition; read-path complexity |

P5 is attractive for notes specifically, because the machinery already exists
— `notes_missing_vectors`/`ensure_note_vectors` becomes
"needing-vectors-for-this-fingerprint" with a one-line predicate change, and
notes number in the tens–hundreds, so the whole set re-embeds on the first
`note_recall` in well under a second. It scales badly to a 1441-chunk RAG
corpus on a read path.

A defensible combination: **P4 for correctness + P2 for the prompt + P5 for
notes** — i.e. never serve mixed results, tell the user plainly, and let the
cheap store heal itself.

---

## 6. Second-order finding: the thresholds are model-bound

Even a **perfectly executed** reindex does not make the application correct on
a new model, because the project's similarity gates are absolute constants
calibrated against bge-m3 (`TRAIT_SIMILARITY = 0.72` and
`SUMMARY_OBS_SIMILARITY = 0.62` are documented in-code as "calibrated on live
bge-m3"; `CONSOLIDATE_SIMILARITY = 0.85` empirically).

The two models have very different cosine distributions:

| trait pair | bge-m3 | e5 |
|---|---|---|
| paraphrase ("values brevity" / "prefers concise answers") | 0.862 | 0.953 |
| unrelated ("values brevity" / a sentence about rain) | **0.446** | **0.751** |
| antonym ("values brevity" / "likes long explanations") | 0.776 | 0.887 |

On e5 the *unrelated* pair scores **0.751 — above the 0.72 trait gate**, and
the *antonym* pair scores **0.887 — above the 0.85 consolidation gate**. So
after a clean, correct reindex to e5 the gates flip from "silently never fire"
(mixed vectors) to "fire on everything" (compressed distribution): the model
would be told unrelated traits are duplicates and contradictions are
duplicates.

Conclusion: **any "switch the embedding model" feature must either carry
per-model thresholds or explicitly document that thresholds are tuned for
bge-m3.** Shipping reindexing alone would trade a silent failure for a loud
wrong one. This is a fork (R6), not a detail.

Related, measured while checking e5: the e5 family expects `query:` /
`passage:` prefixes. Adding them widened the retrieval margin from 0.155 to
0.186 on the probe corpus — a real but modest effect, and a per-model input
convention the `Embedder` contract has no place for today. Noted as
groundwork, not proposed here.

---

## 7. Forks for the user

**R1 — Scope of the operation.** A model change invalidates *all* profiles at
once (the embedder is global), so a per-profile operation is the wrong shape;
this is also why `/rag rebuild` has to refuse when other profiles have
documents.
  - (a) **Recommended:** a new DB-global "re-embed everything" operation
    covering all three stores; `/rag rebuild` keeps its per-profile,
    re-chunking meaning.
  - (b) Extend `/rag rebuild` to all three stores and make it global.
  - (c) RAG + notes now, attachments later.

**R2 — Detection.** (a) **Recommended: D5** — canary vector as the trigger,
server-reported model id stored for the message. (b) D2 config fingerprint
only (cheaper, misses in-place file swaps). (c) Keep D1 and rely on the user
to know.

**R3 — Policy.** (a) **Recommended: P4 + P2** — mark stale, keep semantic
paths from returning mixed results, tell the user, reindex on command.
(b) P3 auto-reindex in the background. (c) P2 alone (notify only).

**R4 — Per-row fingerprint.** Tag each vector row with the fingerprint it was
produced under?
  - (a) **Recommended: yes** — makes the reindex resumable and idempotent
    (a cancelled or crashed run leaves a consistent partial state), enables P5
    for notes, and turns "is this index consistent?" into a query. Additive
    columns, `CREATE TABLE IF NOT EXISTS` cannot add them — so either a
    guarded `ALTER` or the first real `DB_STEPS` bump (ADR 0006).
  - (b) No — a single global `meta` fingerprint, all-or-nothing reindex.

**R5 — Notes healing.** (a) **Recommended: P5** — extend the existing
backfill to treat foreign-fingerprint vectors as missing (cheap, self-healing,
minimal new code). (b) Fold notes into the same explicit bulk operation as
RAG.

**R6 — Thresholds (§6).** (a) **Recommended:** ship per-model threshold
profiles (a small table keyed by fingerprint, defaults for bge-m3, plus the
existing `summary_obs_calibration_e2e_live`-style calibration smoke to derive
new ones). (b) Make the three constants configurable settings and document the
bge-m3 calibration. (c) Out of scope for this track; document the limitation
and treat non-bge-m3 models as unsupported.

**R7 — Reporting.** How loud should the reindex be? (a) **Recommended:** reuse
the existing `RagProgress` banner + a feed note, as `/rag add` does.
(b) A dedicated screen/section. (c) Log only.

---

## 8. Recommended plan (if the recommendations are adopted)

1. **Stage 1 — detection and honesty.** Canary + model id in `meta`; compute
   the fingerprint on first embedder use; on mismatch mark the index stale,
   stop serving mixed semantic results, and surface a clear note. No
   reindexing yet. This alone converts the worst failure (silent) into a
   visible one.
2. **Stage 2 — re-embed in place.** The DB-global operation over all three
   stores, with per-row fingerprints (R4), progress via `RagProgress`,
   cancellable, resumable. Notes heal lazily via the extended backfill.
3. **Stage 3 — thresholds.** Per-model threshold profiles (R6), with a
   calibration smoke against a live pair of models.

Live verification is mandatory (AGENTS.md §3) and cheap here — the two servers
used for this research are exactly the fixture needed: index under bge-m3,
switch settings to e5, assert the stale state is detected, reindex, assert
recall recovers. A same-dimension pair is the strongest possible test case
precisely because no existing guard notices it.

### 8.1. Stage 2 — sub-decisions left open by the plan

Recorded before implementation (AGENTS.md §1). Stage 1 changed what stage 2 is
*for*: notes and attachments already heal themselves, and `/rag rebuild`
already repairs a knowledge base. What remains genuinely broken is narrower,
and it is what stage 2 targets:

- `/rag rebuild` **loses** sources whose stored text is absent and whose file
  is gone (legacy rows) — it counts them as errors and drops them. Re-embedding
  needs no source text, only the chunk text already in the DB (§3).
- It is **per profile**, so a model change with several profiles means
  switching into each one in turn.
- Attachment indexes come back only when the user **re-attaches** each file.
- Re-chunking is wasted work and churns chunk ids for what is only a vector
  problem.

**S1 — how the per-row marker is stored (R4a).** An `embed_gen INTEGER` column
on `note_vectors`, `rag_documents` and `attachment_documents`, against
`meta.embed_gen` bumped on each detected model change. A small integer, not a
vector: identity already lives in the canary, and a row only needs to say
*which generation* produced it. The two vec0 tables are deliberately **not**
touched — a virtual table cannot take an `ALTER`, and both are joined by
`rowid` to a plain table that can.

**S2 — schema mechanism.** A guarded `ALTER TABLE ... ADD COLUMN` in
`baseline_ddl`, **not** the first `DB_STEPS` bump. Adding a nullable column is
additive and backward-compatible (every query names its columns explicitly, so
an older binary ignores it), which is exactly the case ADR 0006 F12 says needs
no bump; `CREATE TABLE IF NOT EXISTS` simply cannot express it. A bump would
also force a pre-migration backup of `data.db` on every upgrade and would
exercise never-before-run machinery for a change that does not need it.

**S3 — invalidation becomes non-destructive.** With generations, stage 1's
"delete every note vector" is replaced by leaving the rows in place and letting
them read as foreign. Same healing (the backfill overwrites them), no data
thrown away, and switching *back* to the previous model needs no work at all.

**S4 — one DB-global command, `/reindex`.** Not a `/rag` subcommand: it spans
notes, attachments and every profile's knowledge base, so filing it under the
knowledge-base family would misdescribe it. `/rag rebuild` keeps its meaning
(re-chunk one profile after a chunking-parameter change). The stage 1 notice is
updated to name `/reindex` instead.

**S5 — a dimension change is still incremental.** If the new model's
dimensionality differs, the vec0 tables are dropped and recreated up front (a
fixed-width table cannot hold both), after which every row simply reads as
foreign and takes the same path. One algorithm for both cases: *for each row
whose generation is not current, embed its stored text, replace the vector,
stamp the generation.* Interrupting it leaves a consistent partial state, and
the stale marks from stage 1 keep search honest until it finishes.

---

## 9. Out of scope / groundwork

- e5-style per-model input prefixes (`query:`/`passage:`) — the `Embedder`
  contract has no notion of input role today.
- vec0 for notes (existing roadmap item) — orthogonal; a fingerprint column
  would land naturally in the same schema step if both are done together.
- Re-chunking attachments (needs the chat walk the roadmap describes) —
  unnecessary for a model change, still needed if chunk parameters change.
- Cross-model migration without re-embedding (learned projections between
  vector spaces) — research-grade, not worth it at this corpus size.
