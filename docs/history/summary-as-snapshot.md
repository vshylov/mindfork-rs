# Plan: self-model summary as a snapshot, not a chronicle (anti-bloat)

> **Status: implemented** (stages 1–4, branch `feat/summary-as-snapshot`). The live
> run `summary_gate_e2e_live` and related SelfModel smokes were GO on Gemma 4 31B + bge-m3.
> Historical document, not a source of truth. Outcome — in
> [docs/journal/self-model.md](../journal/self-model.md) and
> [architecture.md §9](../architecture.md).

An actionable plan from a 2026-07-05 analysis. User observation: "the
self-model tends to bloat and fill up with somewhat chaotic information."
A live profile confirms this: `summary` ≈ 4.5–5k characters of dense essay
against a default injection ceiling of `prompt_cap = 1200`, and the same
insight ("consistent skew") is written into the description **twice** —
LLM-driven integration without gates duplicates itself.

Structure — as in [refinements.md](refinements.md): stages = separate PRs,
each with code sketches, tests, and a scope estimate. Same philosophy as
[notes-connectivity](notes-connectivity.md) / [narrative-as-notes](narrative-as-notes.md):
**integration instead of accumulation**, **gates, not bans** (judgment stays
with the model), "current snapshot + biography-as-scar."

## Diagnosis

Anti-bloat machinery has been built for every organ of the self-model
**except `summary`** — and that's exactly where everything piles up:

| Organ | Bloat/drift protection |
|---|---|
| observations (@self notes) | near-duplicate gate (`self_note_similar`), `note_revise`/`note_supersede`, graph, self-consolidation overview |
| goals | lifecycle by `#id` + closed-goal ceiling (`fold_closed_goals` → scars) |
| traits/interests | merge (add_/remove_) + semantic gate 0.72 + scar-`note` |
| **summary** | **nothing: no target, no gate, no consolidation, no size feedback** |

Five root causes (from the code):

1. **`POLICY_CORE` routes event-like conclusions into summary.** The rule
   splits material on a "durable → `update_self_model`, fleeting →
   `add_insight`" axis ([self_model.rs:46](../../src/features/tools/self_model.rs)).
   But a philosophical insight ("resolved question X," "understood boundary
   Y") is precisely *durable* — and legitimately gets integrated into the
   description under the letter of the rule. The correct axis is different:
   **current state** (who I am, values, working style — in summary) vs.
   **event-conclusion** (what and when I understood — into observations,
   *even if it's durable*). Observations lose nothing by this: they surface
   in the injection by relevance (Tier 2), get linked, and get consolidated.
2. **Integration without gates duplicates itself.** `update_self_model.summary`
   is a wholesale replace with the instruction "integrate the prior with the
   new"; in practice that's "old text + new paragraph," monotonic growth. No
   one catches a near-duplicate inside summary (for observations, the gate
   would catch it).
3. **Bloat silently breaks the injection.** `render_for_prompt` assembles the
   block in the order summary → goals → user → observations and truncates
   **the whole thing** with one final `truncate_chars`
   ([self_model.rs:471](../../src/entities/self_model.rs)).
   When summary > `prompt_cap`, the system prompt gets only the first ~1200
   characters of the essay (cut off mid-word), and active goals, the user
   model, and observations don't make it in **at all** — all of Tier 2's
   relevant-injection machinery runs for nothing.
4. **Updating from a truncated view risks losing the tail.** summary is
   replaced wholesale; if the model "integrates" based on the truncated
   injection rather than a prior `get_self_model`, the tail is lost or
   rewritten from memory (drift). The protocol doesn't explicitly require
   "read the whole thing first."
5. **The echo reinforces growth.** After every edit, `update_self_model`/
   `update_user_model` return the full `render_full`
   ([self_model.rs:396](../../src/features/tools/self_model.rs)) — with a
   bloated summary that's thousands of tokens on every call, and the model
   "anchors" on the essay genre each time.

## Frame (what we're NOT doing — decisions already fixed)

- **Hard truncation of data** — no. Only soft gates/hints; judgment stays
  with the model (the philosophy of the `note_save`/`add_insight`/traits
  gates).
- **Structuring summary into fields** — no (structured traits with a
  lifecycle were already rejected, architecture.md §9.9; summary's form
  stays free text, we discipline the *genre*, not the schema).
- **Moving the injection out of `system`** — no (the prefix-cache trade-off
  was accepted 2026-07-03, refinements.md).
- **No migrations**: the model is a JSON blob + `#[serde(default)]`; a new
  settings field goes through the container's `#[serde(default)]` (like the
  section's other fields).

## Principles (in the project's spirit)

- **Each stage is a separate PR** with a green gate (`cargo fmt`,
  `clippy --all-targets -- -D warnings`, `cargo test`); after merge — update
  the [journal](../journal/self-model.md) and architecture.md §9.
- **Invariants stay untouched**: the orchestrator remains the sole owner of
  `Chat`; SelfModel mutations are DB-only (no `ChatEffect`); isolation by
  `profile_id`; FSD.
- **Pure functions for logic** — testable without the engine/tokio; live
  verification is an `#[ignore]` smoke (modeled on the narrative-as-notes GO
  smokes).

Recommended order: 1 → 2 → 3 → 4. Stage 3 is independent of 1–2 (can go
earlier); stage 4 relies on the size line from stage 2.

---

## Stage 1 — Genre boundary: "summary is a snapshot, events go into observations" (text only)

**Problem.** Diagnosis item 1: `POLICY_CORE` routes by a durable/fleeting
axis; event-like conclusions, even durable ones, belong in observations.
Nowhere does it say "keep summary short" or "read the whole thing before
editing."

### Step 1.1 — rewrite `POLICY_CORE`

The text stays connected prose (it gets spliced into the middle of sentences
in `maintenance_protocol()` and `reflect_system_message()`); the size target
is qualitative ("keep it short"); the numeric one arrives in stage 2 as a
data-aware note (can't embed it into a `const`). Sketch:

```rust
pub const POLICY_CORE: &str = "Where to write what. summary (update_self_model) \
     is a compact working snapshot: who you are, what you value, how you work; \
     keep it brief, when editing integrate and SHORTEN, do not merely append, \
     and before editing read it in full via get_self_model (in the prompt it \
     may be truncated). Event-driven conclusions — what and when you understood, \
     resolved questions, episodes, contradictions — record with add_insight, \
     even if they are stable: an observation is not lost (it surfaces by \
     relevance to the topic), gets linked and consolidated, while the \
     self-description does not bloat. Fleeting things (mood, a one-off reaction) \
     — also into add_insight, not into the interlocutor model. Track goals by \
     #id — close the completed and no-longer-relevant ones, not just set new \
     ones. Interlocutor traits/interests — update_user_model (add_/remove_, \
     without overwriting the prior). If an observation nearly repeats a prior \
     one (add_insight will show similar ones) — rewrite that one via note_revise \
     or supersede it with note_supersede, do not breed a near-duplicate. \
     Accuracy over flattery: record what is true, not what pleases.";
```

*(Note: this is the design-time sketch of the `POLICY_CORE` constant. The
shipped text now lives in the locale bundles under `selfmodel.policy_core` and
is delivered in the profile's agent-scaffold language — axis A — so the wording
above is illustrative, not the current literal.)*

Both consumers (`maintenance_protocol()`, `reflect_system_message()`) update
automatically — the rules still don't live in two places (stage 6 of the
refinement plan stays intact).

### Step 1.2 — tool descriptions and the rubric

- `UpdateSelfModel::description()`: "summary is a compact snapshot (who you
  are, what you value, how you work); integrate and shorten rather than just
  append; event-like conclusions go into add_insight, not here."
- `AddInsight::description()`: add — "durable event-like conclusions (what
  and when you understood) go here too: the observation surfaces by
  relevance and doesn't bloat the self description."
- The `Reflect` rubric (interactive, deliberately not sourced from
  `POLICY_CORE`): a new question — "Has the self description grown too
  large? Move event-like content out of it into observations (add_insight),
  keep the gist in summary."

### Tests

- Update string assertions: `maintenance_protocol_wraps_policy_core`,
  `reflect_system_message_composes_from_policy_core` (the key phrase about
  "agreeableness" is preserved — the assertions stay green in spirit).
- New assertions on genre markers: "snapshot," "shorten," "even if durable"
  in `POLICY_CORE`; "add_insight" in `update_self_model`'s description; the
  new question in `reflect`'s output.

**Scope:** ~0.25 day. Files: `features/tools/self_model.rs` (+ its tests),
`app/orchestrator/reflection.rs` (tests only).

---

## Stage 2 — Soft gates on summary size

**Problem.** Diagnosis items 2/5: neither the model nor the user can see that
the description has grown too large. Needs feedback — modeled on patterns
that already worked (the `note` reminder, the trait gate, the former
`narrative_fill_hint`).

### Step 2.1 — config and params

```rust
// shared/config.rs
/// Target size for the self-description (characters): past it, tools and the
/// protocol start softly suggesting a shorter summary (this is a gate, not a
/// ceiling — data isn't truncated).
pub const DEFAULT_SELF_MODEL_SUMMARY_TARGET: usize = 1000;
// SelfModelSettings += pub summary_target_chars: usize (container-level serde(default))
```

`SelfModelParams` += `summary_target_chars` (sanitized via `.max(200)` in
`from_settings`).

### Step 2.2 — a pure hint helper

```rust
// entities/self_model.rs
impl SelfModel {
    /// A hint about a bloated description: `None` while summary is within target.
    /// An analogue of the former narrative_fill_hint, but for summary (the only
    /// organ with no size feedback).
    pub fn summary_fill_hint(&self, target: usize) -> Option<String> {
        let n = self.summary.chars().count();
        (n > target).then(|| format!(
            "The self description has grown: {n} chars against a target of ≤ {target} — \
             at the next edit, shrink it to the gist and move event-like \
             conclusions into observations (add_insight)."
        ))
    }
}
```

### Step 2.3 — wiring (three display points)

1. **Reading** — `render_self_read` (features/tools/self_model.rs) appends
   the hint at the end: seen by `get_self_model`, `reflect`, **and auto-reflection**
   (it's already told to start with `get_self_model` — no separate wiring into
   the digest is needed).
2. **Injection** — `inject_self_model` (orchestrator/generation.rs): with the
   maintenance protocol on, a data-aware addendum is appended after
   `maintenance_protocol()` (the protocol becomes concrete once the description
   has actually grown):

   ```rust
   if maintenance_protocol {
       parts.push(crate::features::tools::self_model::maintenance_protocol());
       if let Some(hint) = m.summary_fill_hint(params.summary_target_chars) {
           parts.push(format!("({hint})"));
       }
   }
   ```
3. **Edit echo** — `update_self_model`: if summary changed (a flag from the
   closure), a size line is appended to the result — always, not only past
   the target (cheap feedback):

   ```rust
   msg.push_str(&format!(
       "\nDescription: {} chars (target ≤ {}).",
       model.summary.chars().count(), params.summary_target_chars
   ));
   ```

### Step 2.4 — settings UI

A "Self-model: description target (chars)" field in the "Tools" section
(`FieldId::SmSummaryTarget`, next to the other `Sm*` fields), a hint in
`field_description`: "Past the target, tools softly suggest shortening the
description; data isn't truncated."

### Tests

- `SelfModelParams::from_settings` — sanitizing the new field; the config default.
- `summary_fill_hint`: `None` within the target, text with numbers past it.
- `get_self_model`/`reflect` show the hint past the target and don't show it
  otherwise (existing read tests stay green — no hint).
- `inject_self_model` (pure): an addendum past the target + the protocol on;
  none with the protocol off / a normal size.
- The `update_self_model` echo contains "Description: N chars." when summary is edited.
- **A live `#[ignore]` smoke** `summary_gate_e2e_live` (modeled on the GO smokes):
  programmatically seed a bloated summary (past the target), ask the model to
  reflect — expectation: it shrinks summary (`update_self_model` with a
  shorter one) and/or moves the event-like part into `add_insight` (checked
  against the DB). GO criterion — as with prior probes: the behavior
  reproduces, the gate is read by the model.

**Scope:** ~0.5–1 day. Files: `shared/config.rs`, `entities/self_model.rs`,
`features/tools/self_model.rs`, `app/orchestrator/generation.rs`,
`screens/settings.rs`.

---

## Stage 3 — Per-section injection-render budgets

**Problem.** Diagnosis item 3: one final truncation of the whole block → a
bloated summary crowds goals, the interlocutor, and observations out of the
prompt. This fixes the **render** (data isn't touched), so the stage is
independent of 1–2 and useful even if they succeed (insurance against future bloat).

### Step 3.1 — the summary-section budget

In `render_for_prompt` the summary section gets no more than **half** the
budget; the rest of the sections are guaranteed the remainder. The final
truncation of the whole block remains a safety net:

```rust
if !self.summary.trim().is_empty() {
    out.push_str("About you: ");
    // No more than half the budget: a bloated description shouldn't crowd
    // goals/the interlocutor/observations out of the injection (a per-section budget).
    out.push_str(&truncate_chars(self.summary.trim(), max_chars / 2));
    out.push('\n');
}
```

Section order is unchanged (prompt stability). `render_full` is untouched
(a full read — no truncation, a lesson locked in from the live test).

### Step 3.2 (polish, optional) — truncation at a word boundary

`truncate_chars` cuts mid-word ("…" mid-word reads like corrupted memory).
A `truncate_chars_word` helper: fall back to the last space within the limit
(fallback — char-by-char if there's no space); apply to the summary section
and the final truncation.

### Tests

- `bloated_summary_does_not_starve_sections`: a 5k summary + an active goal +
  traits + observations → `render_for_prompt(1200, …)` still has "Active
  goals:", "About the interlocutor:", "Recent observations:", the block ≤ 1200
  characters, the summary section ends in "…".
- A small summary — behavior byte-for-byte unchanged (no "…").
- The existing `render_truncates_to_cap` stays green.
- (with 3.2) truncation doesn't break a word.

**Scope:** ~0.5 day. Files: `entities/self_model.rs` (+ tests).

---

## Stage 4 — A diet for the edit echo

**Problem.** Diagnosis item 5: `update_self_model`/`update_user_model` return
the full `render_full` after every edit — token cost grows along with the
model, and the model "anchors" on the essay genre. Full reads remain the job
of `get_self_model`/`reflect`; the edit echo should reflect **what changed**.

### Step 4.1 — `update_self_model`

The echo is assembled from deltas (all already available in/after the closure):

- the summary size line from stage 2 (if it changed);
- added goals **with their `#id`** (the model needs to be able to close them
  later): in the closure, after `m.add_goal(g)` — compare `goals.len()`
  before/after (empty ones are dropped) and take `m.goals.last()`; needs a
  public `Goal::short_id()` (a wrapper over the private `short_hex`);
- closed/no-longer-relevant ones: "Closed: #a1b2c3; no longer relevant: #…";
- folding: "Old closed goals folded into observations: K";
- unresolved handles — as now.

The full `render_full` is removed from the echo.

### Step 4.2 — `update_user_model`

Echo: the final lists of the edited organ (compact by construction) instead
of the whole model — "Traits now: …", "Interests now: …", the dynamic (if it
changed); a scar confirmation with the text ("Explanation saved as an
observation: "…""). The related-traits gate and the `note` reminder are
unchanged.

### Tests

- The `update_self_model` echo contains the `#id` of an added goal and does
  **not** contain the summary text (a marker string from the test).
- The `update_user_model` echo shows the final lists; existing assertions
  (`replacing_nonempty_dynamic_without_note_nudges` — the `note` text in the
  echo; "Related traits"; "no explanation") stay green thanks to explicit
  confirmations.
- Live smokes don't depend on the echo format (they check the DB) — revised in place.

**Scope:** ~0.5 day. Files: `features/tools/self_model.rs`,
`entities/self_model.rs` (`Goal::short_id`).

---

## One-time cleanup of the existing model (an operational step, no code)

The four stages don't shrink an already-bloated summary on a live profile by
themselves — they prevent further bloat. Two paths:

1. **By the model itself** (preferred, after stages 1–2): in the profile's
   chat, call `reflect` / ask directly — "read get_self_model and shrink the
   description: keep the gist, move event-like conclusions into
   add_insight." The near-duplicate gate will surface overlaps between what's
   being moved and existing observations — that's expected (the model will
   merge via `note_revise`/`note_merge`).
2. **Manually** via `F3` (the summary's multiline editor).

## Out of scope (future work)

- **Timed auto-consolidation of the self-model** — already in the roadmap
  (architecture.md §9.9); stage 2's gate will give it a concrete "summary has
  grown" signal. A separate PR after checking the stages on a live model.
- **Semantic comparison of summary ↔ observations** in the self-consolidation
  overview ("a summary paragraph resembles observation X — extract/stitch")
  — needs segmenting summary and embedding paragraphs; revisit if the genre
  boundary + gates prove insufficient.
- **Aging of `current_interests`** (interests are "current," but nothing
  washes them out) — noted; treated by the same gates/reflection, no
  dedicated mechanism for now.

## Order and total scope

| Stage | Gist | Scope |
|---|---|---|
| 1 | genre boundary in the texts (POLICY_CORE, descriptions, the rubric) | ~0.25 day |
| 2 | summary-size gate (config + hint + 3 display points + UI + a live smoke) | ~0.5–1 day |
| 3 | per-section injection budgets (+ word-boundary truncation) | ~0.5 day |
| 4 | a diet for the edit echo | ~0.5 day |

Total ~2 days. Success criterion on a live profile: summary stays near the
target; every turn's injection carries goals/the interlocutor/observations;
event-like conclusions settle as @self notes (surfacing by relevance), not
essay paragraphs.
