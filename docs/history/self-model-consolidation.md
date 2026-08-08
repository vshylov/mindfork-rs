# Plan: "self-model" consolidation (auto-"sleep" + summary semantics + interest aging)

> Status: **A1, A2, A3-light — done; the track is complete** (A3-heavy —
> future work). Branches `feat/self-model-auto-consolidate`, `feat/self-model-summary-semantics`,
> `feat/self-model-interests-aging`; log — [docs/journal/self-model.md](../journal/self-model.md); the A2 threshold
> `SUMMARY_OBS_SIMILARITY=0.62` was calibrated against live bge-m3; A3-light — a nudge in
> `selfmodel.policy_core`. This track continues
> [summary-as-snapshot](summary-as-snapshot.md) ("Out of scope") and
> [refinements](refinements.md); it closed three future-work items from the roadmap's
> "Memory, self-model, knowledge" section. **Archived** (the track is complete).

## The task's nerve

Every organ of the "self-model" has anti-bloat mechanics **except the periodic
one**: observations (`@self` notes) are consolidated only via auto-reflection and
the interactive `reflect`; `summary` has a size gate (`summary_fill_hint`), but
nothing acts on it outside a turn; `current_interests` are "current," but nothing
washes them out. The track adds three things:

1. **A1 — background "sleep" for the self-model on a timer** (a mirror of
   `notes.auto_consolidate_every`): periodically merge duplicate observations, replace
   stale ones with a "scar," **shrink a bloated `summary`**, link contradictions — regardless
   of recent turns.
2. **A2 — semantic comparison of `summary` paragraphs with observations** in the
   self-consolidation overview: "a description paragraph resembles observation X → extract/stitch."
3. **A3 — aging of `current_interests`**: remove interests that haven't been confirmed in a while.

## What's already in place (anchors in the code)

- **The background-task scaffold was built for exactly this.** SOLID stage 2
  (`app/orchestrator/background.rs` — the `BgSlot` slot registry by `BackgroundKind`;
  `app/orchestrator/tool_loop.rs::spawn_silent_loop`) explicitly prepared for "family
  task #3 (timed self-model auto-consolidation)": adding it "no longer
  touches `run()`/`Quit`."
- **Siblings to copy from:** `app/orchestrator/consolidation.rs` (notes) and
  `app/orchestrator/reflection.rs` (self-model) — an identical lifecycle
  (gates → cadence counter → `build_*_overview` → `spawn_silent_loop` → `begin_bg`).
- **Data for the "sleep" is already collected:** `features/tools/notes/overview.rs::
  build_self_consolidation_overview` (similar observation pairs / `contradicts` /
  "no links"). The bloated-summary signal — `entities/self_model.rs::
  SelfModel::summary_fill_hint`.
- **Observability feedback:** `background.rs::handle_bg_done` already, on
  **reflection**'s success, emits `SelfModelChanged` (an open `F3` refreshes) and tracks a run
  of failures (`BACKGROUND_FAILURE_ALERT`).

## Principles (in the project's spirit)

- **DB-only, no `ChatEffect`** — all self-model/notes tools write directly
  through `ctx.storage`; the "sole owner of `Chat`" invariant is untouched.
- **Opt-in, off by default** — like `auto_reflect_every`/`auto_consolidate_every`
  (background calls cost tokens).
- **Graceful degradation** — the embedder is unavailable / the server isn't ready →
  quietly skip, the cadence counter isn't lost (as in `reflection.rs`/`consolidation.rs`).
- **Integration, not loss** — stale content is folded into a "scar" observation, not
  silently deleted (like `fold_closed_goals`, `note_supersede`).
- **Gates, not hard rules** — the decision always stays with the model; we give data and a nudge.
- **No schema migrations** — new config fields via `#[serde(default)]`.

---

## Stage A1 — background auto-consolidation of the "self-model" ⭐ first

### Step A1.1 — a `BackgroundKind` variant and the status bar

- `app/events.rs` — `enum BackgroundKind` += `SelfConsolidation`.
- `app/runtime/dispatch.rs` — mapping `SelfConsolidation → screen.set_self_consolidating(on)`.
- `screens/chat/*` — a flag field + a `set_self_consolidating` setter (mirroring
  `set_reflecting`/`set_consolidating`).
- `widgets/status_bar.rs` — a quiet muted chip (e.g. `✻ self sleep`; a glyph 1
  column wide — compat-safe, like the other background chips).

### Step A1.2 — the `maybe_auto_self_consolidate` handler

A new module `app/orchestrator/self_consolidation.rs` — a copy of `consolidation.rs`:

- **Config gate:** `self_model.auto_consolidate_every == 0` → exit.
- **Cadence:** a counter `self_consolidate_counts: HashMap<chat_id, u32>` on
  `Orchestrator` (like `consolidate_counts`); `tool_loop::due(count, every)`; reset —
  only on an actual spawn (a gate skip doesn't lose the cycle).
- **Gates:** the profile enabled self-model tools (`GET_SELF_MODEL_ID`); there's
  something to consolidate — observations ≥ 2 **or** `summary` past
  `summary_target_chars` (via `summary_fill_hint`); `bg_running(SelfConsolidation)` == false; the server is
  `Ready` (`engines.backend_if_ready`).
- **Tool set** (intersected with the profile): `note_revise`/`note_supersede`/
  `note_merge`/`note_link`/`note_neighbors` (over `@self` observations) +
  `update_self_model`/`update_user_model` (shrink summary / update the interlocutor) +
  `get_self_model` (full observation ids). `note_recall` is **withheld** — it hides
  `@self`; the model gets full ids from `get_self_model` (as with reflection).
- **Digest:** `build_self_consolidation_overview(...)` + a `summary_fill_hint` line
  (if summary is bloated). Empty/no signal → exit without resetting the counter.
- **Request:** the system message `prompt.self_consolidate.system` (built on
  `self_model::policy_core(loc)` — a single source of rules, like
  `reflect_system_message`), `max_tokens ≈ 2048`, `temperature ≈ 0.3`,
  `spawn_silent_loop(SilentLoop { kind: SelfConsolidation, ... })` + `begin_bg`.

### Step A1.3 — trigger and observability

- `app/orchestrator/mod.rs::handle_done` — call `maybe_auto_self_consolidate`
  (next to `maybe_auto_reflect`/`maybe_auto_consolidate`); a counter field.
- `background.rs::handle_bg_done` — extend the special case: on
  `SelfConsolidation`'s success also emit `SelfModelChanged` (it changes the self-model, like
  reflection). Error key `ui.err.bg_self_consolidation`.

### Step A1.4 — config and UI

- `shared/config.rs` — `SelfModelSettings.auto_consolidate_every: usize`
  (`#[serde(default)]`, default 0) + a `DEFAULT_*` constant.
- `screens/settings/*` — a field in the "Memory" section, "Self-model" group (next to
  "auto-reflection"), a description hint + an i18n key.
- `locales/{ru,en}.json` — `prompt.self_consolidate.system`, the chip label, the error,
  the field label/description. i18n gates (key/placeholder parity, `no_cyrillic`,
  key-in-code) cover this automatically.

### Tests A1

- Unit: `due`-cadence is already covered in `tool_loop`; a gate on summary_fill_hint.
- Integration (`orchestrator/tests/`): `handle_done` spawns at the threshold; gates
  hold when the feature is off / < 2 observations and summary isn't bloated / the
  server isn't ready; the counter isn't reset on a skip; success → `SelfModelChanged`.
- Runtime: `SelfConsolidation` → a chip + doesn't open `F3`, instead re-requests the snapshot.
- **Live** `#[ignore]` (Gemma 4 31B + bge-m3): with `auto_consolidate_every=1`, two
  similar observations are merged via `note_merge`; a bloated summary (> target) is shrunk via
  `update_self_model`. GO/no-go criterion as with prior probes.

### Fork A1 (for the user)

- **A separate `self_model.auto_consolidate_every`** (recommended) — the self-model and
  notes are already kept apart by gates/data; a dedicated toggle is more precise.
- **A shared "memory sleep" toggle** — reuse `notes.auto_consolidate_every`.
  Simpler UI, but ties two different organs together. Not recommended.

---

## Stage A2 — "summary ↔ observations" semantics in the overview

**What.** `build_self_consolidation_overview`
(`features/tools/notes/overview.rs`) currently compares **observations against each other**
(pairwise cosine) + `contradicts` + "no links." Add a section: segment
`summary` into paragraphs, **embed them on the fly** (summary has no stored vectors — like
the trait gate `self_model.rs::near_duplicate_traits`), compare against the vectors of
`@self` observations (`notes_with_vectors`, filtered by `is_self_note`), and surface pairs
"a summary paragraph ≈ observation X" with a hint to "extract/stitch."

**Nuance (important).** The function is currently a **pure DB read** (no embedder needed). The new
section needs an `Embedder`, so it's **optional**: no embedder / a mismatch →
the section is skipped (graceful degradation, like RAG/web reranking). So the signature and
callers (`reflect`, `reflection.rs`, and also `self_consolidation.rs` from A1) need to be
extended with an embedder; the pure part stays, the semantic section is an add-on.

**Files.** `overview.rs` (segmenting paragraphs by blank lines + the section),
`features/tools/self_model.rs` / `app/orchestrator/reflection.rs` /
`app/orchestrator/self_consolidation.rs` (threading the embedder through), `locales`.

**Threshold.** Calibrate against live bge-m3 (like `TRAIT_SIMILARITY=0.72`), don't guess.

**Tests.** On `MockEmbedder`: a paragraph similar to an observation is surfaced; a dissimilar one —
stays silent; without an embedder — the section is absent, no panic.

---

## Stage A3 — aging of `current_interests`

`current_interests` — a flat `Vec<String>` (`entities/self_model.rs::UserModel`),
nothing washes them out. Two forks:

- **A3-light (recommended).** No schema change: a nudge — "check whether
  interests are stale; ones not confirmed in a while — remove via `remove_interests`" — in the
  system message for A1/reflection and the "maintenance protocol" (`policy_core`). Consistent with
  the project's philosophy (integration by the model itself) and the direct recommendation in
  [summary-as-snapshot](summary-as-snapshot.md) ("treated by the same
  gates/reflection"). Cost — bundle text. Do it as part of A1.
- **A3-heavy (future work).** `current_interests: Vec<Interest { text, updated_at }>` —
  real time-based aging. Breaks the flat `Vec<String>` and touches ~a dozen
  spots: `render_user_model`, `add_interests`/`remove_interests`, `apply_edit`
  (`SetInterests`), `UserModel::render_for_impersonation`, gates, a serde migration
  (`#[serde(default)]` + a step). Justified only if A3-light on a live model
  proves insufficient.

---

## Out of scope (future work)

- **A3-heavy** (an interest structure with timestamps) — if the light version proves insufficient.
- **Moving aging into vec0** — unrelated; see [notes-vec0](../notes-vec0.md).
- **Consolidation of the interlocutor model as a separate organ** — currently `user_model`
  is managed via merge semantics + scars; we're not adding a separate "sleep" for it.

## Order and DoD

1. **A1** (auto-"sleep" + the A3-light nudge in the same prompts) — one PR, the flagship.
2. **A2** (summary↔observation semantics) — the next PR, on top of A1/reflection.

Stage DoD: `cargo fmt`/`clippy -D warnings`/`test` green; the A1 live smoke — **go**;
an entry in the [journal](../journal/self-model.md) + CHANGELOG (the rubric matching the effect); settings fields and
i18n keys in place.
