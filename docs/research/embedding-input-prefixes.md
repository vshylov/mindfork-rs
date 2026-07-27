# Research — per-model input prefixes for embeddings (`query:` / `passage:`)

**Status:** decided and implemented. Forks R1–R6 (§7) — **user's decision,
2026-07-27: R1a–R6a**, all as recommended. Research date: 2026-07-27.
Branch: `feat/embed-input-prefixes`.

**Question.** The e5 family expects role markers on its input (`query:` /
`passage:`). The `Embedder` contract has no notion of an input *role* — a query
and a stored chunk go through the same `embed(texts)` call. Should the contract
gain one, and what does that cost?

**Origin.** The last open groundwork item of the embedding-model change track
([embedding-model-change-reindex.md](embedding-model-change-reindex.md) §9;
`docs/roadmap.md`). It became relevant only once a model swap was actually
supported (that track's stages 1–3), because prefixes are a *per-model*
convention and the project previously had exactly one model.

**Related:** [ADR 0002](../decisions/0002-embeddings-dedicated-server.md)
(dedicated embedding server, the `Embedder` contract), research §6/§8.2 (the
similarity gates and their per-model calibration), spec §9.3/§9.7.

Every number below is **measured** on the live stand (2026-07-27), against the
same two servers the previous track used:

- `bge-m3-Q8_0.gguf` on `:8001` — the model the project is calibrated against,
  and one that expects **no** prefix;
- `multilingual-e5-large-instruct-q8_0.gguf` on `:8002` — the plausible swap.

---

## 1. A "convention" is not a boolean

The roadmap item, and research §9 that it came from, both say "the e5 family
expects `query:`/`passage:`". That is the convention of **base** e5. The
`-instruct` variant on the stand — the model actually being measured — documents
a *different* one:

| convention | query side | passage side |
|---|---|---|
| `none` | *(bare)* | *(bare)* |
| `e5` (base e5, multilingual-e5-*) | `query: ` | `passage: ` |
| `e5-instruct` (e5-*-instruct) | `Instruct: <task>\nQuery: ` | *(bare)* |

So the earlier 0.155 → 0.186 figure was measured with the *wrong* convention
for that model, and still improved — which says the effect is robust in
direction but that the design cannot be a single on/off switch. It is a
per-model-family choice of at least three values, and picking the wrong one is
worse than picking none (§2).

---

## 2. What the prefixes actually buy (the measurement)

### 2.1 A corpus big enough to misrank

40 documents in the register the application really indexes (notes about the
user, `@self` observations, knowledge-base chunks; bilingual, with deliberate
near-neighbours), 14 queries with a known target. **Top-1 accuracy and MRR are
what matter** — a margin is cosmetic if it never changes a decision.

| model | convention | top-1 | MRR | mean margin | min margin |
|---|---|---|---|---|---|
| bge-m3 | `none` *(today)* | **11/14** | 0.881 | 0.1480 | 0.0072 |
| bge-m3 | `e5` | 12/14 | 0.903 | 0.1125 | 0.0055 |
| bge-m3 | `e5-instruct` | **10/14** | 0.814 | 0.1027 | 0.0156 |
| e5-instruct | `none` *(today)* | **12/14** | 0.898 | 0.0400 | 0.0002 |
| e5-instruct | `e5` | 12/14 | 0.898 | 0.0426 | 0.0017 |
| e5-instruct | `e5-instruct` | 12/14 | **0.899** | **0.0458** | **0.0056** |

**The headline, stated plainly: on e5 the prefixes changed not one ranking
decision.** Top-1 is 12/14 under all three conventions and MRR moves by 0.001.
The entire measured benefit is separation: the mean margin improves 15% and the
**minimum** margin improves **25×** (0.0002 → 0.0056). A 0.0002 margin is an
arbitrary tie-break, so that part is real robustness — but it is robustness, not
a correctness fix, and this corpus cannot demonstrate it turning into one.

**And the downside is measurable.** On bge-m3 — a model that wants no prefix —
the wrong convention costs a rank (11/14 → 10/14 under `e5-instruct`) and cuts
the mean margin by 31%. Whatever is built must therefore default to `none` and
must never guess silently.

### 2.2 A smaller corpus, for continuity with the earlier figure

The 6-document corpus reproduces the direction of the original number
(margin 0.104 → 0.120 under `e5`, → 0.133 under `e5-instruct` on e5;
0.364 → 0.278 → 0.238 on bge-m3, i.e. harmful). Same conclusion, both scales.

### 2.3 Getting the role *wrong* on one side is expensive

The failure mode of §4: a comparison meant to be homogeneous, with one side
prefixed as a query and the other as a passage. Mean cosine over the corpus's
paraphrase pairs:

| model | convention | both passage | query vs passage | delta |
|---|---|---|---|---|
| bge-m3 | `e5` | 0.8501 | 0.7648 | **−0.0853** |
| bge-m3 | `e5-instruct` | 0.8176 | 0.6362 | **−0.1814** |
| e5-instruct | `e5` | 0.9457 | 0.9341 | −0.0116 |
| e5-instruct | `e5-instruct` | 0.9456 | 0.9186 | **−0.0270** |

On e5 the whole usable dynamic range is **0.156** (research §8.2), so a −0.027
mismatch consumes **17% of it** — enough to silently push paraphrases below a
gate that was calibrated correctly. This is why §4 is the load-bearing part of
the design and not bookkeeping.

---

## 3. Trap A — the calibration is measured *without* prefixes

`REFERENCE_UNRELATED = 0.4128` / `REFERENCE_PARAPHRASE = 0.8176`
(`shared/embed_calibration.rs`) are the means of `shared/embed_probes.json` on
bge-m3 **with no prefix**. Applying a prefix moves them:

| model | convention (passage role) | unrelated | paraphrase | span |
|---|---|---|---|---|
| bge-m3 | `none` | **0.4128** | **0.8176** | 0.4049 |
| bge-m3 | `e5` | 0.5097 | 0.8501 | 0.3404 |
| bge-m3 | `e5-instruct` | 0.4128 | 0.8176 | 0.4049 |
| e5-instruct | `none` | 0.7897 | 0.9456 | 0.1559 |
| e5-instruct | `e5` | 0.7923 | 0.9457 | 0.1534 |
| e5-instruct | `e5-instruct` | 0.7897 | 0.9456 | 0.1559 |

(The first row reproduces the two reference constants to four decimals — the
measurement harness agrees with the shipped ones.)

The trap is real: prefixing bge-m3 shifts the unrelated mean by **+0.097** and
narrows the span by 16%, which would silently invalidate the reference constants
and therefore every mapped gate.

**But it is self-neutralizing if the calibration is embedded through the same
role-aware path as everything else** — which is the whole argument for the
decorator in R2. Then:

- bge-m3 keeps convention `none`, so its corpus is embedded bare and the
  reference constants stay valid **by construction**;
- e5 measures its own means *under its own convention*, and the affine map
  absorbs whatever they are.

The one thing that must never happen is calibrating through one path and
querying through another. The design must make that impossible, not merely
discouraged.

**The probe corpus itself is not edited** (its header forbids it, and the
`cyrillic_scan.py` allowlist records why); nothing here requires touching it.

## 4. Trap B — turning prefixes on *is* a change of vector space

Verified rather than assumed. Cosine between the canary embedded bare and the
same canary embedded with a prefix (the detector fires below **0.999**):

| model | prefix applied | cosine | detected as a model change? |
|---|---|---|---|
| bge-m3 | `e5` / query | 0.967087 | yes |
| bge-m3 | `e5` / passage | 0.970937 | yes |
| bge-m3 | `e5-instruct` / query | 0.782527 | yes |
| e5-instruct | `e5` / query | 0.985155 | yes |
| e5-instruct | `e5` / passage | **0.996530** | yes *(by 0.0025)* |
| e5-instruct | `e5-instruct` / query | 0.974039 | yes |

So **provided the canary is embedded through the prefixer**, switching the
convention already reads as a model change: the generation is bumped, old
vectors read as foreign, `/reindex` is offered, the knowledge base is marked
stale. That is exactly the desired behaviour and it requires no new mechanism —
only the correct decorator order (R2).

Two refinements this table argues for:

1. **The canary must carry the `passage` role, not `query`.** The stored vectors
   are all passage-role, so the passage prefix alone defines the space the
   database is in. A change to the *query* prefix alters retrieval but leaves
   every stored vector valid, and should therefore **not** trigger a reindex.
   Tracking the passage role gets that granularity exactly right for free.
2. **The margin can get thin.** e5 + `passage: ` clears the threshold by only
   0.0025. It works here, but it is thin enough that identity should not rest on
   it alone: folding the convention's name into the fingerprint as a second
   trigger makes detection *exact* rather than probabilistic. It can only add
   detections, never remove them (unlike a config-only fingerprint, which is what
   D2 was rejected for in the previous research).

---

## 5. Where the role lives at each call site

The load-bearing inventory. **A wrong role here is silent** and costs up to 17%
of e5's usable range (§2.3), so this table is the specification, not a summary.

### 5.1 Query role — asymmetric retrieval, query side

| site | what is embedded |
|---|---|
| `features/tools/rag.rs` — `RagSearch` | the search query |
| `features/tools/notes/recall.rs` — `semantic_recall` | the recall query |
| `features/tools/notes/self_notes.rs` — `self_notes_relevant` | the relevance query (self-note injection) |
| `features/tools/attachment.rs` — `AttachmentSearch` | the search query |
| `features/tools/web.rs` — `rerank_by_embeddings` | the web query **only** |

### 5.2 Passage role — text that is (or matches) a stored vector

| site | what is embedded |
|---|---|
| `features/tools/rag.rs` — `RagAdd` | chunks |
| `features/tools/notes/save.rs` — `NoteSave`, `create_note` | note content (stored) |
| `features/tools/notes/save.rs` — `ensure_note_vectors` | backfill of note content |
| `features/tools/notes/edit.rs` — `NoteRevise` | rewritten note content |
| `app/orchestrator/rag.rs` — `index_source` | knowledge-base chunks |
| `app/orchestrator/attachments.rs` — attachment indexing | attachment fragments |
| `app/orchestrator/reembed.rs` — `/reindex` | **every store's stored text** |
| `features/tools/web.rs` — `rerank_by_embeddings` | the result texts |

`/reindex` is the one that must not be got wrong in a subtler way: it rewrites
vectors that were originally written by the sites above, so it has to use the
identical role or it would quietly re-create the mixed-space problem the whole
previous track exists to prevent.

### 5.3 Symmetric comparisons — *look* like queries, must be passages

The tricky category the user singled out. Each embeds text on the fly and
compares it against **stored, passage-role** vectors (or against other on-the-fly
text of the same kind), so both sides must be `passage`:

| site | comparison | why passage |
|---|---|---|
| `features/tools/notes/save.rs` — `self_note_similar` (the `add_insight` gate) | new observation ↔ stored `@self` notes | the other side is stored |
| `features/tools/notes/save.rs` — `NoteSave`'s duplicate gate | the note being saved ↔ stored notes | the same vector is then stored |
| `features/tools/notes/overview.rs` — `summary_observation_overlaps` | `summary` paragraphs ↔ stored `@self` notes | the other side is stored |
| `features/tools/self_model.rs` — `near_duplicate_traits` | new traits ↔ prior traits | homogeneous; must match the calibration corpus |
| `features/tools/notes/overview.rs` — both consolidation overviews | stored ↔ stored | *(no embed call — already correct)* |

**These four are exactly the sites governed by the mapped thresholds**
(`CONSOLIDATE_SIMILARITY`, `TRAIT_SIMILARITY`, `SUMMARY_OBS_SIMILARITY`). Since
all of them are passage↔passage, the calibration corpus must be embedded with the
**passage** role — which is what §3 concluded independently.

### 5.4 Infrastructure

| site | role | note |
|---|---|---|
| `app/orchestrator/embed_guard.rs` — canary | **passage** | defines the stored space (§4) |
| `app/orchestrator/embed_guard.rs` — calibration probes | **passage** | must match §5.3 (§3) |
| availability pings (`rag.rs`, `attachments.rs`, `reembed.rs`) | passage | role irrelevant; pick one and be consistent |

**Total: 5 query sites, 8 passage sites, 4 symmetric-passage sites, 3 probes.**
One site (`web.rs`) mixes both roles in a single request today.

---

## 6. Design options

### 6.1 Where the role lives in the contract (R1)

| # | shape | mixed batch | impls to touch | can a site stay silent? |
|---|---|---|---|---|
| a | `embed(texts, role: EmbedRole)` | no — `web.rs` splits into 2 calls | 1 method × 5 impls | **no** (enum has no `Default`) |
| b | `embed_query()` / `embed_passage()` | no | 2 methods × 5 impls | no |
| c | `embed(Vec<EmbedInput>)`, role per item | yes | 1 method × 5 impls | no, but noisier at every site |

(a) keeps the trait surface as it is, makes the role impossible to omit (the
parameter is required and the enum deliberately gets no `Default`), and matches
how batches are actually used — every batch in §5 is homogeneous except
`web.rs`. Its one cost is that `web.rs` needs two requests instead of one, on a
path that already issues N parallel page fetches.

### 6.2 Who applies the prefix (R2)

| # | where | consequence |
|---|---|---|
| a | inside `OpenAiClient` | mocks and fakes never prefix, so tests cannot exercise the real path; the convention has to be threaded through the supervisor into the client |
| b | **a `PrefixedEmbedder` decorator**, installed in `apply_embed` | mirrors `EmbedGuard`; one place; testable with any inner embedder |
| c | at the call sites | 17 chances to forget, silently |

Order is load-bearing: **`EmbedGuard` must wrap `PrefixedEmbedder`**, i.e.
`EmbedGuard { inner: PrefixedEmbedder { inner: real } }`. Then the guard's own
canary and calibration calls go *through* the prefixer, which is precisely what
makes trap A self-neutralizing (§3) and trap B self-detecting (§4). The reverse
order would break both.

### 6.3 How the convention is chosen (R3)

| # | mechanism | risk |
|---|---|---|
| a | auto-detect from the model name | the name is often absent (external servers report whatever they like) and a wrong guess is measurably harmful (§2.1) |
| b | **an explicit setting**, default `none` | the user must know to set it |
| c | b + a *hint* (never an action) in the existing model-change notice | same as b, plus discoverability |

The local `llama-server` does report a usable name (`/v1/models` returns the full
GGUF path), so (a) is *feasible* — but §2.1 measured the cost of a wrong
convention as a lost rank and a 31% narrower margin, and silence is the one thing
the user asked this design to avoid. (c) keeps the behaviour explicit while
solving discoverability: the notice already fires exactly when the model changed.

---

## 7. Forks for the user

**R1 — Where the role lives in the contract.**
  - (a) **Recommended:** `embed(texts, role: EmbedRole)` — one method, a required
    enum parameter with no `Default`, per-call granularity. `web.rs` splits into
    two requests.
  - (b) Two methods, `embed_query` / `embed_passage`.
  - (c) `EmbedInput { role, text }` per item — mixed batches stay one request.

**R2 — Who applies the prefix.**
  - (a) **Recommended:** a `PrefixedEmbedder` decorator installed in
    `apply_embed`, wrapped *inside* `EmbedGuard`, so the canary and the
    calibration go through it (§3, §4).
  - (b) Inside `OpenAiClient`.
  - (c) At the call sites.

**R3 — How the convention is selected.**
  - (a) **Recommended:** an explicit `Choice` setting in the Embeddings tab
    (`none` / `e5` / `e5-instruct`), default `none`, plus a non-acting hint in
    the model-change notice when the new model's name looks like an e5.
  - (b) Explicit setting only.
  - (c) Auto-detect from the model name, with an override.

**R4 — Role assignment.** Adopt the table in §5 as specified — in particular
that the four symmetric gate sites (§5.3), the canary and the calibration corpus
all use the **passage** role.
  - (a) **Recommended:** yes, as specified.
  - (b) Something else (please say which site).

**R5 — Fingerprint.** Fold the convention's name into `EmbedFingerprint` as a
second, exact trigger alongside the canary?
  - (a) **Recommended:** yes — the canary already detects it behaviourally, but
    by as little as 0.0025 (§4); the config component makes it exact and can only
    add detections.
  - (b) No — rely on the canary alone.

**R6 — Scope.** Given that the measured benefit is *margin only* (zero ranking
changes on a 40-document corpus, §2.1):
  - (a) **Recommended:** implement the contract + decorator + setting, shipping
    with the default `none`. Existing installations are bit-identical until the
    user opts in; the e5 family becomes properly supported; the cost is paid once.
  - (b) Do not implement. Record the measurement, close the roadmap item as
    "measured, not worth the contract change". Fully defensible on these numbers.
  - (c) Implement the contract only, no setting (a constant `none`) — pointless
    on its own; listed for completeness.

---

## 8. If R6a is adopted — plan

1. **Contract + roles.** `EmbedRole` in `shared::api::contract`, the parameter on
   `Embedder::embed`, and every one of the 20 call sites in §5 given its role
   explicitly. `web.rs` split into two requests.
2. **`PrefixedEmbedder`** (`shared/embed_prefix.rs`): the three conventions as
   data, `EmbedConvention` in `EmbedSettings` (`#[serde(default)]` → no
   migration, ADR 0006 F12), installed in `apply_embed` inside `EmbedGuard`.
3. **Fingerprint + settings UI**: the convention as a second trigger (R5), a
   `Choice` field in the Embeddings tab, i18n for both bundles, the hint in the
   model-change notice (R3a).
4. **Tests**: role assignment pinned per site (a recording embedder asserting the
   exact prefixed text — the only way a silent role error is caught); the
   decorator order (a canary embedded through the guard carries the passage
   prefix); the identity convention changing nothing; live smokes for the switch
   being detected and for retrieval under e5's own convention.

Live verification is mandatory (AGENTS.md §3) and the same two-server stand
covers it. A **memory regression run** is required regardless of the default,
because every embedder call site is touched.

---

## 9. Out of scope

- Instruction *tuning* for the `e5-instruct` task string (it is a free-text task
  description; this research uses one fixed sentence and does not explore it).
- Other families' conventions (BGE-v1.5's English retrieval instruction, Nomic's
  `search_query:`/`search_document:`) — the design is a table, so adding a row is
  cheap once the shape exists.
- Per-role calibration (measuring the query↔passage distribution separately). Not
  needed: all four gate sites are passage↔passage (§5.3).
