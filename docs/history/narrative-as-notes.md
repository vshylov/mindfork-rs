# Narrative as notes: unifying memory organs

This document designs the direction where the **self-model narrative** (an
append-only insight list) stops being a separate, weak store and moves into
**notes** (`notes`), gaining for free everything notes are already richer at:
embeddings and semantic search, duplicate gates on write, a typed link graph,
supersession with a "scar", consolidation, and auto-"sleep". This implements a
long-standing recorded item of groundwork — "linking memory organs" (see
[notes-connectivity.md](notes-connectivity.md) "out of scope" and
[architecture.md](../architecture.md) §9.9). Structure mirrors
[self-model-mvp.md](self-model-mvp.md) and [notes-connectivity.md](notes-connectivity.md):
motivation → current state → idea catalog → MVP probe (Tier 1) → deferred →
open questions → go/no-go criterion.

This is a **design doc + probe**, not a ready PR: first we fix the frame and
resolve the forks, then do a minimal implementation to test the hypothesis.

## Task nerve

The self-model narrative is, in essence, a **second, weak instance of notes**:

| | Notes (`notes`) | SelfModel narrative |
|---|---|---|
| Storage | SQLite `notes` (+ `note_vectors`/`note_links`/`note_superseded`) | `Vec<NarrativeSegment>` in the `self_models.data` JSON blob |
| Search | semantic (embeddings, cosine) + substring + tags | none (only "N latest" in order) |
| Growth | managed via consolidation/supersession | **FIFO cap** (`max_narrative`) — the oldest is silently evicted |
| Duplicates | **gate** on `note_save` (semantically close ones are shown) | none — every `add_insight` just appends |
| Links | typed graph (`supports`/`contradicts`/`refines`/`relates`) | none |
| Supersession | `note_supersede` with a "scar" | none — deletion only (`consolidate_narrative`) |
| "Sleep" | auto-consolidation (`consolidation.rs`) | manual `consolidate_narrative` |

That is, the narrative is **exactly the "accumulation notebook" that notes has
already moved past** (connectivity Tiers 1–3). Refinement stages 1–6
([refinements.md](refinements.md)) gave the narrative palliatives (visible
eviction, manual consolidation, age labels), but fundamentally it stayed a
FIFO list without semantics. Unification closes this in one move **and
shrinks code** (it removes the parallel narrative machinery), rather than
adding to it.

**Usefulness criterion** (as with notes connectivity): does the move increase
integration — does it give the narrative semantic recall, dedup gates, and
supersession — rather than just move the data around.

## Current state (baseline)

### Narrative in SelfModel

- `entities/self_model.rs`: `SelfModel.narrative: Vec<NarrativeSegment>`
  (`{id, text, created_at}`). Methods: `add_insight(text, max) -> Vec<...>`
  (append + FIFO trim, returns what was evicted), `remove_insights(ids)`,
  `narrative_fill_hint(max)`, `match_insight(handle)`; rendered in
  `render_for_prompt` (N latest) and `render_full` (all, with `#id`).
- Tools (`features/tools/self_model.rs`): `add_insight` (write),
  `consolidate_narrative` (remove by `#id` + opt. rollup), plus the `note`
  scar in `update_user_model` goes through `add_insight`.
- Injection (`orchestrator/generation.rs::inject_self_model`) and the
  `reflect` rubric show the narrative; auto-reflection (`REFLECT_TOOL_IDS`)
  gives the model `add_insight`/`consolidate_narrative`.
- UI `F3` (`screens/self_model.rs`): renders `m.narrative` as lines,
  `RowAction::Insight(id)`; `SelfModelEdit::DeleteInsight(id)` removes.

### Notes (`notes`) — what they already do (map from the survey)

- **Entity** `Note { id, profile_id, content, tags: Vec<String>, created_at,
  updated_at }`. Tags — `Vec<String>`, serialized as JSON into the
  `notes.tags` column; **no reserved tags**; tag filtering is **AND-all in
  Rust** after SQL/semantics (not in SQL).
- **Tools** (`features/tools/notes.rs`, all in `default_tool_ids`):
  `note_save {content, tags?}` (insert + embedding + **gate**: up to 3
  semantically close notes, cosine threshold; tags are ignored by the gate),
  `note_recall {query?, tags?, limit?}` (semantic when `query` + embedder are
  present, otherwise substring; tag filter in Rust after ranking; **spreading
  activation** — neighbors of the top-3 hits), `note_revise {id, content}`
  (in-place edit + re-embed + graph warning), `note_link`/`note_neighbors`
  (graph), `note_supersede {old_id, content}`/`note_merge {ids[], content}`
  (supersession/merge with a "scar", link transfer), `consolidate_notes`
  (overview: duplicates ≥ 0.85, `contradicts`, orphans).
- **Storage** (`shared/storage/db.rs`): `note_insert`, `note_list(profile,
  query, tags, limit)` (tag filter in Rust, anti-join `note_superseded`,
  ORDER BY `updated_at DESC`), `note_update`, `note_vector_upsert`,
  `note_search_semantic(profile, &[f32], k)` (**no** tag filter — pure
  semantics, anti-join superseded, brute-force cosine in Rust),
  `notes_missing_vectors`, `note_is_active`, `note_link_*`, `note_neighbors`,
  `note_supersede_mark`, `notes_with_vectors`, `note_links_all`,
  `note_links_retarget`. Isolation via `WHERE profile_id = ?` everywhere.
- **Helpers** (`notes.rs`): `create_note(ctx, content, tags)` (insert +
  best-effort embedding — **a ready wrapping point** for `add_insight`),
  `ensure_note_vectors` (vector backfill), `semantic_recall`, `parse_tags`,
  `format_notes`.
- **Auto-"sleep"** (`orchestrator/consolidation.rs`): background note
  consolidation every N replies (gate — profile enabled `note_merge`). **It
  would already cover self-notes** — this is the key to the fate of
  `consolidate_narrative`.

## Full idea catalog

★ — goes into the MVP probe (Tier 1); the rest are deferred tiers.

### A. Storage migration

| Idea | What it does |
|---|---|
| ★ narrative → self-notes | `NarrativeSegment` → `Note` with a reserved tag; `add_insight` = a wrapper over `create_note(tags=[SELF])` |
| ★ backfill of the existing narrative | one-off idempotent migration of `Vec<NarrativeSegment>` → self-notes (preserving `created_at`), then `narrative` is cleared |
| ★ dedup gate on `add_insight` | after writing — semantically close **self-notes** (rewrite via `note_revise`/`note_supersede` instead of a near-duplicate) |

### B. Recall by meaning

| Idea | What it does |
|---|---|
| ★ semantic narrative reading | `get_self_model`/`reflect`/injection pull self-notes semantically/by freshness, not a FIFO slice |
| relevance-based injection | mix into the prompt self-notes **relevant to the current turn** (embedding of the last user message), not just the freshest ones — Tier 2 |

### C. Integration over accumulation (free from notes)

| Idea | What it does |
|---|---|
| ★ supersession instead of deletion | `consolidate_narrative` → `note_supersede`/`note_merge` with a "scar" (the narrative remembers it changed) |
| ★ growth managed by "sleep" | the FIFO cap disappears; note auto-consolidation folds in self-notes too |
| graph over self-notes | `note_link` between insights (`contradicts`/`refines`) + spreading activation — Tier 2 |
| linking self-notes with regular ones | an "about self" insight linked to an "about the user" note — Tier 3, the original "organ linking" goal |

### D. Adjacent (same mechanism)

| Idea | What it does |
|---|---|
| embedding gate for `user_model` traits | near-duplicate traits (`curious`/`inquisitive`) caught semantically, like note duplicates — Tier 2 |

---

# MVP probe (Tier 1)

**Hypothesis:** moving the narrative into notes will produce a **behavioral**
shift — (1) `add_insight` will show the gate and the model will start
**rewriting** near-duplicates instead of breeding them; (2) semantic reading
will bring back old insights that FIFO was losing; (3) narrative growth will
be governed by "sleep"/supersession, not silent FIFO. Go/no-go before Tier 2
(graph/relevance/organ linking).

## Key architectural decision: where unification happens

We unify **at the storage level**, not at the recall level. Self-notes are
ordinary `Note`s with a reserved tag; they share tables/embeddings/graph/
consolidation with regular notes, but are **excluded from user-facing
`note_recall` by default** (memory "about self" and memory "about the user"
are different kinds; mixing them in general recall is risky). This way the
probe gets all the benefit (semantics, gate, supersession, "sleep") **without**
the risk that "I'm verbose" surfaces in an answer to "what do you know about
me". Full recall mixing (the `[about self]` marker in general recall) is
Tier 2/3 (see open questions).

## Step 1 — reserved tag + recall filtering

- Constant `SELF_NOTE_TAG` (`features/tools/notes.rs` or `self_model.rs`).
  The value should be **recognizable and unlikely to collide**; proposal —
  `"@self"` (a leading `@` doesn't occur in natural tags; see open question 1
  on collisions). Documented as reserved.
- **Exclusion from user-facing recall**: the `note_recall`/`semantic_recall`/
  `note_list` path drops notes with `SELF_NOTE_TAG` **unless** the caller
  explicitly requested them (i.e. a normal "recall about the user" call from
  the model doesn't see them). The change is a small Rust filter in
  `notes.rs` (both the semantic and the substring path); DB methods aren't
  touched. The `note_save` gate (showing similar notes) also excludes
  self-notes — so a normal save doesn't stumble over insights.
- **Reading self-notes** for the narrative uses a separate path:
  `note_list(profile, None, &[SELF_NOTE_TAG], Some(n))` (ORDER BY
  `updated_at DESC` → "recent observations") for the recency slice;
  `note_search_semantic` + a Rust tag filter — for the semantic path
  (get_self_model/reflect). No new DB methods needed.

## Step 2 — `add_insight` → note wrapper + gate

- `AddInsight::invoke`: instead of `SelfModel::add_insight` — `create_note(ctx,
  text, vec![SELF_NOTE_TAG])` (insert + best-effort embedding, an existing
  helper).
- **Gate** (core of the hypothesis): after writing — `note_search_semantic`
  among **self-notes** (Rust tag filter), excluding the one just created; a
  non-empty result → append "Similar observations (possible duplicate — if
  needed, rewrite via `note_revise`/`note_supersede` instead of a new
  entry): …". A direct mirror of the `note_save` gate.
- The `note` scar in `update_user_model` — goes through `create_note(tags=
  [SELF])` too.
- The tool result no longer counts "narrative N/M" (no FIFO cap anymore);
  instead — a confirmation + the gate.

## Step 3 — reading the narrative into the render (ripple at the entity↔orchestrator boundary)

`SelfModel::render_for_prompt`/`render_full` are **pure** (only over `self`),
but the narrative is now in the DB → **the observation block is assembled
outside the entity** and passed as a parameter:

- Signatures: `render_for_prompt(cap, n, now, recent: &[String])` and
  `render_full(now, recent: &[String])` — `recent` = rendered lines of
  self-notes (fresh/semantic), which the caller prepares.
- Preparers: `inject_self_model` (orchestrator — has `storage`),
  `get_self_model`/`reflect` (tools — have `ctx.storage`/`ctx.embedder`).
  They query self-notes and pass them into the render.
- `SelfModel::{add_insight, remove_insights, narrative_fill_hint, match_insight}`
  and the `narrative` field are **removed** (or `narrative` stays empty/hidden
  until the backfill finishes — see Step 6). `is_empty` no longer counts the
  narrative.

This is the main price of the move — the render stops being pure with
respect to the narrative. Accepted knowingly.

## Step 4 — `consolidate_narrative` and "sleep"

- **Deprecate** `consolidate_narrative` in favor of `note_supersede`/
  `note_merge` over self-notes (supersession with a "scar" instead of
  deletion — stronger than the previous "remove by id"). It's removed from
  `REFLECT_TOOL_IDS`; auto-reflection instead gets the note tools
  (`note_revise`/`note_supersede`/`note_merge`/`note_recall`) — the same ones
  auto-consolidation gets.
- **Growth managed by "sleep"**: note auto-consolidation
  (`consolidation.rs`) already folds duplicates/stale entries; self-notes
  fall under it automatically (gate — `note_merge` enabled). The narrative
  FIFO cap disappears along with the field.
- The `reflect` rubric and system messages (POLICY_CORE from stage 6) are
  edited: "an observation → `add_insight`; a duplicate/revision →
  `note_revise`/`note_supersede`".

## Step 5 — UI `F3`

- `SelfModelView` now carries not only `SelfModel` but also **fresh
  self-notes** (a snapshot prepared by the orchestrator on
  `RequestSelfModel`). A new payload type (`{model, narrative: Vec<Note|
  (id,text,created_at)>}`).
- The screen renders observations from this list (as it currently does from
  `m.narrative`); `SelfModelEdit::DeleteInsight(id)` → deletion/supersession
  of the **note** (the orchestrator calls `note_delete`/
  `note_supersede_mark` under the profile).
- In-place editing of an insight (not available now — deletion only) can be
  added later via `note_revise`.

## Step 6 — idempotent backfill

- One-off migration on the profile's first access: for every
  `NarrativeSegment` in `SelfModel.narrative` → `create_note(content=text,
  tags=[SELF_NOTE_TAG])`, **preserving `created_at`** (needs a variant of
  `note_insert` with an explicit date — either a small method or set
  `created_at` on the entity before insert), best-effort embedding; then
  `narrative.clear()` + `self_model_upsert`. Idempotency — by the emptiness
  of `narrative` (migrated → cleared → a repeat pass is empty). Where to
  call it: lazily in `start_generation`/`handle_request_self_model`
  (best-effort, like `ensure_note_vectors`), under the same opt-in gate
  (`get_self_model` enabled).
- `created_at` ordering keeps "recent observations" correct after the move.

## Step 7 — tests

- db: the tag filter excludes `SELF_NOTE_TAG` from the user-facing path and
  includes it in the dedicated one; `note_insert` with a preserved
  `created_at`.
- tools: `add_insight` writes a self-note and shows the gate when a similar
  one already exists; a normal `note_recall` **doesn't** see self-notes;
  `get_self_model` sees only its own; the backfill migrates the narrative and
  clears it; a repeat backfill is a no-op.
- entity: `render_*` accept `recent` and render what's passed; `is_empty`
  without the narrative.
- orchestrator/UI: `SelfModelView` carries observations; `DeleteInsight`
  removes the note; F3 renders without panicking.
- Semantic tests — on `MockEmbedder` (as in the notes-connectivity tests).

## Step 8 — probe evaluation (the whole point)

2–3 long multi-session conversations (profile "self-aware AI"), an off/on
run. Record **before** merging:
- whether the **gate** fires — does the model rewrite a near-duplicate
  insight via `note_revise`/`note_supersede` instead of a new entry;
- does semantic reading find old insights that FIFO was losing;
- did the narrative stop duplicating (fewer near-repeats);
- does `F3` work (view/delete);
- did the move "leak" into normal `note_recall` (self-notes shouldn't
  surface there).
The go/no-go criterion — whether to proceed to Tier 2.

---

## Open questions (forks resolved here)

1. **Reserved tag and collisions.** Tags are free-form strings; the model
   could set a tag on a note that matches `SELF_NOTE_TAG`. Recommendation: a
   recognizable prefix sentinel (`"@self"`), unlikely in natural tags; a
   collision is rare and **harmless** (such a note would simply be treated
   as an insight). A guaranteed alternative — a `kind` column in `notes`
   (migration), but that goes against the "no migrations" principle and
   breaks reuse of the tag paths. **Probe decision: tag `"@self"`, documented
   as reserved.**
2. **Visibility of self-notes in general `note_recall`.** MVP recommendation
   — **hide** them (memory about self ≠ memory about the user; mixing is
   risky). Full mixing with an `[about self]` marker ("observations about
   self are the same notes", the connectivity thesis) is appealing in
   principle, but is tested separately in Tier 2 (can be behind a toggle).
3. **Fate of `consolidate_narrative`.** Recommendation — **deprecate**:
   supersession (`note_supersede`/`note_merge`) is stronger than deletion,
   and growth is folded by "sleep". Remove from `REFLECT_TOOL_IDS`, give
   reflection the note tools. Nuance: prompts/POLICY_CORE need rewriting to
   the note idiom.
4. **Editing the narrative in `F3`.** Extend the `SelfModelView` payload
   with self-notes; `DeleteInsight` → note deletion/supersession. In-place
   editing — later, via `note_revise`.
5. **Embedding gate for `user_model` traits.** Related (the same near-
   duplicate semantics), but traits are a flat `Vec<String>` with exact
   dedup. Recommendation — **defer to Tier 2** and solve it with the same
   mechanism (best-effort semantic check on `add_traits`), not mixing it
   with the narrative probe.
6. **Render purity ripple.** `render_*` stop being pure with respect to the
   narrative (they receive `recent` as a parameter). An accepted price; the
   alternative (carrying a note snapshot inside `SelfModel` on load) would
   bring back a stale snapshot — worse.

## Deferred (Tiers 2–3)

- **Relevance-based injection** (embedding of the last user message →
  relevant self-notes into the prompt, not just the freshest ones).
- **Graph over self-notes** (`note_link`/spreading activation between
  insights).
- **Linking memory organs** (self-notes ↔ regular notes ↔ RAG) — the
  original far goal of "connectivity".
- **Full recall mixing** (`[about self]` in general `note_recall`).
- **Embedding gate for `user_model` traits**.
- **vec0 for notes** — as the count grows (brute-force cosine is enough for
  now).

## Honest assessment (is it worth it)

**For:** removes real narrative defects (silent FIFO loss, no dedup or
semantics, weak "deletion" instead of supersession); reaches the recorded
far goal of "linking memory organs"; **shrinks** code (removes the parallel
narrative machinery in entity/tools); reuses proven note paths.

**Against:** the reserved tag is a soft contract (collision is possible);
the render loses purity with respect to the narrative (entity↔orchestrator
ripple); `F3` gets more complex (payload + note deletion instead of editing
a blob); a one-off backfill is needed; risk of "polluting" user-facing
`note_recall` with self memory (mitigated by the tag filter).

The fork isn't obvious upfront — **exactly why this is a probe**: a minimal
implementation (Tier 1) for a go/no-go on a live model, as with the
SelfModel and notes probes.

## Scope

~2–3 days of code+tests (patterns are ready: wrapper ≈ `create_note`/
`note_save`; reading ≈ `semantic_recall`/`note_list`; backfill ≈
`ensure_note_vectors`; render/injection/`F3` edits — mechanical). The main
work isn't new code but **carefully working through the ripple** (render,
`SelfModelView`, `REFLECT_TOOL_IDS`, prompts) and the backfill. Afterward —
update `CLAUDE.md`/`architecture.md` (§9 "Self-model": narrative → notes;
memory-organs table) and the status in this document.

## Tier 1 — status: implemented (steps 1–7), probe evaluation is a manual step

Tier 1 is implemented (code + tests, **711 tests green**, clippy/fmt clean;
log — in [CLAUDE.md](../../CLAUDE.md), map — [architecture.md](../architecture.md) §9).
Forks resolved (confirmed by the user): **tag `@self`**; self-notes are
**hidden** from user-facing `note_recall`. Per step:

- **Step 1** — `SELF_NOTE_TAG="@self"` + `is_self_note`; self-notes are
  excluded from user-facing `note_recall` (substring/semantic paths +
  spreading activation), from the `note_save` gate, and from the
  consolidation overview / auto-"sleep" gate.
- **Step 2** — `add_insight` = `create_note(@self)` + **gate**
  (`self_note_similar` → rewrite via `note_revise`/`note_supersede`); the
  `update_user_model.note` scar and rolled-up closed goals are also
  self-notes now.
- **Step 3** — `render_for_prompt`/`render_full` accept
  `recent: &[NarrativeSegment]` (prepared by the caller); `is_empty` without
  the narrative; observations in `render_full` — with the full id; the
  narrative entity methods are removed (the `narrative` field remains for
  the backfill/`F3`).
- **Step 4** — `consolidate_narrative` removed; `REFLECT_TOOL_IDS` = the note
  tools (`note_revise`/`note_supersede`/`note_merge`); `note_supersede`/
  `note_merge` inherit tags (a self-note doesn't "fall out");
  `POLICY_CORE`/rubric — note idiom.
- **Step 5** — `SelfModelView` reconstructs observations from self-notes
  (display only, not persisted); `DeleteInsight`/`Clear` on `F3` go through
  notes (`note_delete`).
- **Step 6** — `migrate_self_narrative` (one-off idempotent backfill: an
  atomic drain → no duplicates, `created_at` preserved), best-effort in
  `start_generation`.
- **Step 7** — tests for all of the above (semantics — on `MockEmbedder`).

**Step 8 — probe evaluation on a live model: GO.** A run of
`self_model_gate_e2e_live` on **Gemma 4 31B + bge-m3** (real chat + embedder):
in session 2, `add_insight` showed the **gate** ("Similar observations …
rewrite via note_revise / supersede with note_supersede"), and the model
reacted with **integration** — `note_merge` (result: 1 self-note instead of
2). Across 3 runs behavior was stable: merge / revise / merge, each time the
near-duplicate is folded, not accumulated. Graceful degradation checked
"in production": with an embed server without `--embeddings`, the gate went
quiet, nothing broke, and the model still integrated. All criteria met →
**moving to Tier 2**.

## Tier 2 — plan: structure over self-notes

**Tier 2 hypothesis:** after storage unification (Tier 1), the next lever is
**structure**: (1) the right observation surfaces at the right moment
(injection by **relevance**, not just freshness); (2) observations are
**linked** to each other (`contradicts`/`refines` graph + spreading
activation); (3) user traits don't breed near-duplicates (semantic trait
gate). All of this reuses the graph/embeddings already working on notes.
Like Tier 1 — a minimal probe for go/no-go.

### Probe decisions (confirmed by the user)

- **MVP probe scope — A + B** (relevance-based injection + graph over
  self-notes). **C (semantic gate for `user_model` traits) deferred** —
  independent (about traits, not observations), to be picked up as a
  separate step/probe later.
- **Location of relevance-based injection — the system prompt** (relevant
  observations go into `system`, like the passive "self-model" block). The
  local `llama-server` prefix cache is invalidated **on every turn** (the
  relevant observations change with the topic of the reply) — this is an
  **accepted price**, consistent with the decision in
  [architecture.md](../architecture.md) §9.3 ("self-model injection stays in
  `system`; losing prefix cache is an accepted price for the capabilities").
  We don't add it to the user turn.

### What Tier 1 left (baseline)

- **Injection — by freshness** (`self_notes_recent`, `updated_at DESC`, the
  last N): an observation older than N doesn't make it into the prompt even
  if it's relevant to the current turn.
- **The graph on self-notes already works structurally** — `note_link`/
  `note_neighbors` take ids and **don't filter by tag**, so two self-notes
  can already be linked today. **But** it never surfaces: reflection isn't
  given `note_link`/`note_neighbors`; `get_self_model`/`reflect`/injection
  don't show neighbors; user-facing `note_recall` spreading activation
  **skips** self-notes (Tier 1) — so the self-graph needs a separate
  surfacing path.
- **User traits — a flat `Vec<String>` with case-insensitive dedup only**:
  "curious" and "inquisitive" both accumulate.

### Idea catalog (★ — in the Tier 2 MVP probe)

| Idea | What it does | Tier |
|---|---|---|
| ★ A. Relevance-based injection | embedding of the last user reply → **relevant** self-notes go into the prompt (not just the freshest); graceful degradation to freshness | 2 |
| ★ B. Graph over self-notes | reflection gets `note_link`/`note_neighbors`; `get_self_model`/`reflect` show linked observations; self-consolidation overview | 2 |
| C. Semantic trait gate | `add_traits` warns of a near-duplicate trait (embedding of the new one ∩ existing ones), mirrors the `add_insight` gate | 2b ✅ (done) |
| D. Linking memory organs | self-notes ↔ user notes ↔ RAG (cross-organ links) | 3 |
| E. Full recall mixing | `[about self]` marker in general `note_recall` (behind a toggle) | 3 |
| vec0 for notes | performance as the note count grows | groundwork |

### MVP probe steps

**A. Relevance-based injection** (the main step, with a fork — open question 1):
- In `start_generation`, embed the last user reply (`ctx.embedder`), rank
  self-notes by cosine (`note_search_semantic` + `@self` filter), take the
  top-K relevant ones.
- **Mix with freshness** (don't replace): K relevant + M fresh (dedup) — so
  both the relevant surfaces and "what's in focus right now" is visible.
- **Graceful degradation**: embedder unavailable / no reply → fall back to
  pure freshness (as in Tier 1).
- **Location fork** (open question 1): the system prompt (invalidates the
  prefix cache every turn) vs. appending to the last user turn (cache
  intact). The passive "self-model" block (summary/goals/user) stays in
  `system`; only **observations** go by relevance.

**B. Graph over self-notes**:
- `REFLECT_TOOL_IDS` += `note_link`/`note_neighbors` — reflection links
  observations (`contradicts`/`refines`/`relates`).
- `render_full` (`get_self_model`/`reflect`) mixes in a "Linked observations"
  block — graph neighbors (both directions), self-notes only (modeled on
  `related_block`, but not skipping self).
- Self-consolidation overview for auto-reflection: an analogue of
  `build_consolidation_overview`, but **over self-notes** (similar pairs +
  `contradicts` among observations).

**C. Semantic trait gate**:
- `update_user_model.add_traits`: before adding — embed the new trait, look
  for close ones among the existing (best-effort); on a close match — append
  "looks like trait X — clarify/merge instead of adding a duplicate".
  Graceful degradation without an embedder; traits stay `Vec<String>`
  (no migration); the decision stays with the model.

**Tests**: db/tools on `MockEmbedder` (relevant self-note recall; self-graph
neighbors; trait gate); orchestrator (injection mixes relevant + fresh;
degradation to freshness). Live smoke — as in Tier 1 (`spawn_orch_live`,
real embedder).

### Open questions (forks)

1. **Location of relevance-based injection: system vs. user turn.**
   Injecting into the system prompt every turn would change it with the
   topic of the reply → the local `llama-server` prefix cache is invalidated
   **every turn** (unlike Tier 1, where `system` only changes on real edits,
   and age granularity is a day; see
   [architecture.md](../architecture.md) §9.3). Appending relevant
   observations to the **last user turn** preserves the prefix cache and is
   semantically more precise ("relevant to this turn"). **Recommendation —
   user turn.**
2. **Relevance vs. freshness: replace or mix.** Recommendation — **mix**
   (K relevant + M fresh, dedup): pure relevance loses "what's in focus".
3. **Scope of the graph in reading.** Minimum — neighbors in
   `get_self_model`/`reflect`; spreading activation in the injection itself
   — defer (could clutter the prompt). Recommendation — neighbors in reading.
4. **Trait gate — hard or soft.** Recommendation — **soft** (a warning), as
   with observations.
5. **Cross-organ links (self↔user↔RAG) — Tier 3.** In Tier 2, self-notes
   link only to self-notes (organ isolation is preserved).

### Go/no-go criterion

A live multi-session run (profile with the tools enabled):
- does an **old but relevant** observation surface when a topic returns
  (relevance-based injection) — something freshness was losing;
- does the model link observations (`contradicts`/`refines`) and use
  neighbors;
- does the **trait gate** fire (does the model merge a near-duplicate trait);
- is the latency/cache-invalidation cost of the injection location
  acceptable.
Criterion — whether to proceed to Tier 3 (linking memory organs).

### Scope

~2–3 days. The main work is A (relevance injection + location) and B
(surfacing the graph + self-consolidation overview); C is small. The graph
mechanism already exists (we reuse `note_link`/`note_neighbors`/
`note_search_semantic`). Afterward — update `CLAUDE.md`/`architecture.md` §9
and the status in this document.

### Tier 2 — status: A+B+C implemented; graph — GO on a live model

**A (relevance-based injection)**, **B (graph over observations)**, and **C
(semantic trait gate)** are all implemented. Log — [CLAUDE.md](../../CLAUDE.md),
map — [architecture.md](../architecture.md) §9. Gate is green (718 tests,
20 `#[ignore]`).

- **A** — `self_notes_relevant` (embedding of the last reply → self-notes by
  cosine) + `blend_self_notes` (K relevant + freshest guaranteed) assembled
  into `injection_recent`; the injection was moved from the sync
  `start_generation` into the async `spawn_generation` task (relevance needs
  async embedding). Goes into **system** (confirmed; the prefix-cache price
  is accepted, §9.3). Graceful degradation to freshness.
- **B** — reflection is given `note_link`/`note_neighbors` + a nudge to link
  observations; `get_self_model`/`reflect` show a "Observation links" block
  (graph edges, self↔self, `self_related_block`). Passive injection doesn't
  show the graph (compactness). The self-consolidation overview was deferred
  (needed if the probe shows the value of linking) — **enabled in Tier 3**
  after the graph GO (see below). **Smoke `self_model_graph_e2e_live` — GO**
  (Gemma 4 31B + bge-m3): the model independently linked two contradicting
  observations with a `contradicts` edge (called `note_link`, the edge
  appeared in the graph).
- **C** — a gate for **related** `user_model` traits (mirrors the
  `add_insight` gate): for every trait actually added, `add_traits` looks
  for the closest one among the prior ones above a `TRAIT_SIMILARITY=0.72`
  threshold and shows it. Traits have no stored vectors — the embedding is
  computed on the fly (new + prior in one request); graceful degradation
  without an embedder. Soft gate (the decision stays with the model). Traits
  stay `Vec<String>` (no migration). **The threshold was calibrated on a
  live test** (`trait_gate_e2e_live` on bge-m3): the original 0.85 let real
  rephrasings through (on bge-m3, short traits compress into a narrow band —
  "prefers brevity" ↔ "values conciseness" = 0.77). Calibration: rephrasings
  0.73–0.83, unrelated 0.51–0.69 → threshold 0.72. **Key finding:** bge-m3
  brings traits close by *dimension/topic*, not by direction of meaning, so
  antonyms fall into the band too (0.71) — the gate was **reworded** from
  "near-duplicate" to "related trait — check: a duplicate (merge via
  `remove_traits`) or a contradiction (record it as an observation with
  `add_insight`)". Consistent with the integration philosophy. **Smoke —
  GO**: the gate fires, the model recognizes the duplicate and merges it
  into one trait.
- **Tests**: `self_notes_relevant` (ranking/filter); `blend_self_notes`
  (mixing policy); `injection_recent_surfaces_relevant_over_fresh` (an old
  relevant one surfaces over a fresh one — deterministic on `MockEmbedder`);
  `get_self_model` shows "Observation links"; `REFLECT_TOOL_IDS` contains
  the graph tools; `add_trait_gate_surfaces_near_duplicate`/
  `add_trait_gate_silent_for_dissimilar` (trait gate: a close one surfaces,
  an unrelated one stays quiet).

**Graph and trait gate — GO on a live model** (`self_model_graph_e2e_live`,
`trait_gate_e2e_live` on Gemma 4 31B + bge-m3). **Remaining (manual step):** a
live multi-session run of relevance-based injection — does an old relevant
observation surface when a topic returns, and does the model **use** it. The
overall Tier 2 go/no-go criterion — whether to proceed to Tier 3 (linking
memory organs self↔user↔RAG).

## Tier 3 — status: Paths 1, 2 and 3 implemented, GO

The original far goal of "connectivity" — **linking memory organs**
(self-notes ↔ user notes ↔ RAG). **All three paths** are implemented:
**Path 1** (cross-organ self↔user edges), **Path 2** (recall mixing with an
`[about self]` marker, behind a toggle), and **Path 3** (RAG ↔ notes). All
three are GO on a live model.

**Key point:** the graph mechanism (`note_link`/`note_neighbors`) **was
already cross-organ** — it takes any ids without a tag filter, so a
self-note could already be linked to a user note before. Tier 3 (1)
**surfaces** such edges in recall and (2) gives the model **addressability**
of user notes. The organs stay SEPARATE for storage/search — only an edge
the model **deliberately created** surfaces (not "pollution" of recall,
unlike Path 2).

- **Surfacing cross-organ edges**: `related_block` (user-facing
  `note_recall`) no longer skips "about self" observation neighbors — it
  shows them with an **`[about self]`** marker; `self_related_block`
  (self-model reading) shows user-note neighbors with a **`[note]`** marker.
  Regular search still doesn't pull in self-notes.
- **Addressability**: `note_recall` (`format_notes`) now outputs note **ids**
  — otherwise the model can't reference a user note in `note_link` (this
  also closes a long-standing gap: the `note_link` description promised "id
  from note_recall", but no id was output).
- **Nudge**: `note_recall` was added to `REFLECT_TOOL_IDS` (user note ids for
  reflection); the auto-reflection system message and the interactive
  `reflect` rubric suggest linking an "about self" observation with an
  "about the counterpart" fact.
- **Invariants**: DB-only, isolated by `profile_id`, no `ChatEffect`, no
  migrations.
- **Tests**: `recall_shows_note_ids`;
  `recall_surfaces_cross_organ_self_neighbor_marked` (`[about self]`);
  `self_related_block_surfaces_cross_organ_user_note_marked` (`[note]`);
  `reflect_tools_include_graph` (+`note_recall`);
  `reflect_message_nudges_cross_organ_linking`. **722 tests green**,
  clippy/fmt clean.
- **Path 1 smoke — GO** (`cross_organ_link_e2e_live`, Gemma 4 31B + bge-m3):
  the model saved a fact "about the counterpart" (`note_save`) and an
  observation "about self" (`add_insight`), then via `get_self_model` +
  `note_recall` (took the note id) called `note_link` → creating a
  **cross-organ edge** `contradicts` ("observation about verbosity ↔ note
  about a preference for brevity"). The model uses cross-links meaningfully.

### Path 2 — recall mixing (behind a toggle)

The toggle `config.notes.recall_includes_self` (**off** by default —
reversing Tier 1's hiding is risky: memory about self ≠ memory about the
counterpart) enables showing self-notes in general `note_recall` with an
**`[about self]`** marker. When off — Tier 1 behavior (self hidden).

- `NotesSettings.recall_includes_self` (`#[serde(default)]` → no migration)
  → `ToolContext.recall_includes_self` (threaded through every construction
  site).
- The recall paths (`list_user_notes` substring + `semantic_recall`) don't
  drop self-notes when the toggle is on; `format_notes` marks them
  `[about self]` and **hides the service tag `@self`** from the tag display.
- Toggle in the "Tools" section of the settings screen
  (`FieldId::NotesRecallIncludesSelf`).
- **Tests**: `recall_includes_self_notes_when_enabled` (self shows up in
  both recall branches, marked, without `@self`); default off
  (`partial_json_fills_defaults`).
- **Smoke — GO** (`recall_includes_self_e2e_live`, Gemma 4 31B + bge-m3):
  with the toggle on, `note_recall` returned both the note and the "about
  self" observation with the `[about self]` marker, and in its reply the
  model **cleanly separated the organs** ("— About you: prefers brevity; —
  About myself: tends to be verbose") — **no pollution**, the marker works.

### Path 3 — RAG ↔ notes

The third memory organ (the RAG knowledge base) is linked to notes/
observations: a note can **cite a source** in RAG, and search works
**through both organs**.

- **Key decision**: the link points to the **source name** (path/label), NOT
  a chunk id — chunk ids are **unstable** (`/rag rebuild` drops and
  re-indexes documents with new uuids), while the source name is stable
  (`rag_sources`).
- **Schema**: table `note_rag_links(profile_id, note_id, source)`
  (`CREATE TABLE IF NOT EXISTS` → no migration). DB methods:
  `rag_source_exists` (validation), `note_cite_source_insert` (idempotent),
  `note_cited_sources` (forward), `notes_citing_source` (reverse, hides
  superseded). `note_delete` cleans up links. Isolated by `profile_id`.
- **Tool** `note_cite_source(note_id, source)`: validates the note is active
  + the source exists; in `default_tool_ids` (`reconcile_tools` enables it
  for existing profiles).
- **Bidirectional output**: `note_recall` and `get_self_model` show a
  "Source citations" block (note→source); `rag_search` shows "Notes citing
  these sources" (source→notes, self marked `[about self]`).
- **Tests**: db (bidirectional + isolation + cleanup on delete + hiding
  superseded); tool (source/note validation, idempotency, shown in recall);
  rag_search (reverse direction). **727 tests green**, clippy/fmt clean.
- **Smoke — GO** (`note_cite_source_e2e_live`, Gemma 4 31B + bge-m3): the
  model added a document (`rag_add`), then in one turn `rag_search` →
  `note_save` → `note_cite_source`, linking the output "The capital of
  France is Paris" to the "facts" source. The note↔source link was formed
  in the DB; search through both organs works.

### Self-consolidation overview (deferred from Tier 2 B — done)

Enabled after the graph GO (Tier 3): auto-reflection and the `reflect` tool
get **concrete data** for the "sleep" of "about self" memory, not just the
rubric. `notes::build_self_consolidation_overview(storage, profile_id)` —
pure DB read over **only** `@self` observations (mirrors
`build_consolidation_overview` for user notes): similar pairs (duplicates by
cosine ≥ `CONSOLIDATE_SIMILARITY`), `contradicts` links among observations,
observations without links; `None` when < 2 observations. Mixed into
`reflect` (between the model and the rubric) and into the auto-reflection
digest; `reflect_system_message` nudges merging similar pairs
(`note_merge`/`note_supersede`), checking `contradicts`, and linking
unlinked ones (`note_link`). Isolated by `profile_id`, DB-only, no
migrations. **Tests**: `self_consolidation_overview_covers_self_only`
(notes), `reflect_includes_self_consolidation_overview` (self_model).
**729 tests green**, 25 `#[ignore]`, clippy/fmt clean. **Smoke — GO**
(`self_consolidation_overview_e2e_live`, Gemma 4 + bge-m3): the model wrote
two similar observations, called `reflect` (its result carried the overview
with the similar pair, similarity 0.89 on the real embedder) and merged the
duplicates with `note_merge` into one observation — the full chain worked.

**Direction "narrative as notes" is complete** (Tiers 1–3). **Deferred:**
vec0 as the note count grows, a `note_cite_source` nudge in reflection,
timed self-model auto-consolidation (currently the overview flows through
auto-reflection/interactive `reflect`).
