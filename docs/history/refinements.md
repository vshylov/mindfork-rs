# Refinement plan: self-model/user model and background mechanisms

This document is an actionable improvement plan following a review of
[architecture.md](../architecture.md) and the live code (July 2026). Covers: one
real write race, underused data (dates, behavioral signals, user model), the
auto-reflection cadence, background task observability, agentic-loop
deduplication, and the strategic move "narrative as notes."
Structure — same as [self-model-mvp.md](self-model-mvp.md): stages = separate PRs,
each with code sketches, tests, and a scope estimate.

**Fixed decision (2026-07-03):** the self-model injection into `system` stays
as is. Losing the local model's prefix cache on every self-model update is an
**accepted cost** of the mechanism's capabilities; moving the block to the end
of history is not done (see "Out of scope").

## Principles (in the project's spirit)

- **Each stage is a separate PR** with a green gate (`cargo fmt`, `clippy -D
  warnings`, `cargo test`); after merge — update the CLAUDE.md log and
  architecture.md (§9/§11).
- **No migrations**: new fields — `#[serde(default)]` (the `self_models` JSON
  blob, chat files), new tables — `CREATE TABLE IF NOT EXISTS`.
- **Invariants stay untouched**: the orchestrator remains the sole owner of
  `Chat`; SelfModel mutations stay DB-only (no `ChatEffect`); isolation by
  `profile_id`; FSD dependency direction.
- **Pure functions for logic** — testable without the engine/tokio; live runs
  are `#[ignore]` smokes.

Recommended order: 1 → 2 → 3 → 4 → 5 → 6 (4b depends on 3; 6 is a mechanical
refactor, best done after 3–5 have settled the loop code). Stage 7 is a
separate track with its own design doc and probe.

---

## Stage 1 — Atomic self-model write (defect: load-modify-save race)

**Problem.** Auto-reflection is a background task that runs concurrently with
the user *by design*. All self-model writers do
`self_model_get → edit → self_model_upsert` as three separate calls: tools
([tools/self_model.rs::load](../../src/features/tools/self_model.rs)),
manual `F3` editing (`orchestrator/mod.rs::handle_update_self_model`). The `Db`
mutex serializes *individual* calls but not the read-modify-write pair:
reflection reads the model → the user saves an edit in `F3` → the reflection
tool writes its own version on top — the user's edit is silently lost.

### Step 1.1 — `Db::self_model_update` (closure under one mutex lock)

```rust
/// Atomic read-edit-write of a profile's self-model: SELECT + mutate + INSERT OR
/// REPLACE under ONE lock of the connection mutex. `mutate` returns `true`
/// if the model changed (otherwise the write and version bump don't happen).
/// Returns the model after the edit and whether it was written.
pub fn self_model_update(
    &self,
    profile_id: Uuid,
    mutate: impl FnOnce(&mut SelfModel) -> bool,
) -> Result<(SelfModel, bool)>
```

**Important (deadlock):** `std::sync::Mutex` is not reentrant — inside
`self_model_update` you cannot call the public `self_model_get`/
`self_model_upsert` (a second `lock()` from the same thread = deadlock). Split
out private helpers that take `&Connection`
(`self_model_get_conn`/`self_model_upsert_conn`) and call them from all three
public methods.

A CAS-by-`version` alternative (`upsert_if_version` + retry) was considered
and rejected: the closure is simpler, no retry loop; `version` remains an
indicator.

### Step 1.2 — migrate the writers

- `features/tools/self_model.rs`: `add_insight`, `update_self_model`,
  `update_user_model`, `consolidate_narrative` — the mutation moves into the
  closure (local `unresolved`/`removed` are collected by capturing `&mut`
  variables, which `FnOnce` allows). Readers (`get_self_model`, `reflect`)
  stay on `self_model_get`.
- `orchestrator/mod.rs::handle_update_self_model` (`F3`) — the same API
  (`apply_edit` inside the closure).
- `self_model_upsert` stays public (DB tests, potential import use), but its
  doc comment steers writers to `self_model_update`.

### Tests

- Two threads × 50 `self_model_update` calls (each appending an insight with
  its own prefix under a generous `max_narrative`) → exactly 100 insights in
  the end, nothing lost (atomicity under the mutex).
- No write happens when `mutate → false` (version doesn't grow).
- Existing tool tests are green without semantic changes.

**Scope:** ~0.5 day. Files: `shared/storage/db.rs`,
`features/tools/self_model.rs`, `app/orchestrator/mod.rs`.

---

## Stage 2 — Time and ceilings: dates in renders, eviction report, folding closed goals

**Problem.** "Self in time" has no time: `created_at` is stored on goals and
insights, but neither `render_full` nor `render_for_prompt` shows it — the
model can't tell yesterday's observation from a three-month-old one. The
narrative's FIFO ceiling silently discards the oldest entries (the "silent
loss" named as a pathology in [notes-connectivity.md](notes-connectivity.md)).
Goals are the only organ without a ceiling: closed ones pile up in the blob
forever.

### Step 2.1 — record age in renders

- `entities/self_model.rs`: pure helper
  `fn age_label(at: DateTime<Utc>, now: DateTime<Utc>) -> String` — coarse
  buckets: "today", "yesterday", "N d.", "N wk.", "N mo.", "N yr.".
  **Day granularity**: within a day the text is stable, so the system prompt
  doesn't change between turns and the prefix cache suffers no more than once
  a day (on top of real model changes).
- `render_full(&self, now)` — goals `- #a1b2c3 (active · 3 wk.) …`, closed
  `(completed · 2 d.) …`, observations `- #id (5 d.) …`.
- `render_for_prompt(&self, cap, n, now)` — age on active goals and
  observations (compact, same label).
- `Goal` gains `#[serde(default)] closed_at: Option<DateTime<Utc>>` — set in
  `set_goal_status`/`cycle_goal_status` on leaving `Active` (and reset on
  reactivation). A closed goal's age is computed from `closed_at` (fallback —
  `created_at`). Blob + `serde(default)` → no migration.
- Call sites: tools and `inject_self_model` pass `Utc::now()`; tests use a
  fixed `now`. `screens/self_model.rs` (`F3`) builds its lines itself — add
  the date to goals/insights there too (local time zone, as is the UI's
  convention).

### Step 2.2 — narrative eviction is visible, the protocol is data-aware

- `SelfModel::add_insight(...) -> Vec<NarrativeSegment>` — returns segments
  evicted past the ceiling (currently `drain` is silent).
- The `add_insight` tool: result "Observation recorded (narrative N/M)."; on
  eviction — "Evicted oldest: '…' — if there was something durable in them,
  raise it into summary/traits or consolidate via consolidate_narrative."
  The `note` branch of `update_user_model` gets the same fullness note.
- The `reflect` rubric shows fullness: "narrative N/M".
- The maintenance protocol becomes dynamic: a pure
  `fn maintenance_note(model: Option<&SelfModel>, params) -> Option<String>` —
  at ≥ 80% fullness, appends a line to the protocol: "N of M observations —
  time to consolidate (consolidate_narrative)." `inject_self_model` assembles
  the constant base + the note.

### Step 2.3 — folding old closed goals (ceiling through integration)

- `SelfModelSettings.max_closed_goals: usize` (default **10**,
  `#[serde(default)]`) → `SelfModelParams` (sanitization: ≥ 1).
- `SelfModel::fold_closed_goals(&mut self, keep, max_narrative) -> usize`:
  closed goals beyond `keep` (oldest by `closed_at`/`created_at`) become a
  narrative scar "[goal archive] completed: …" / "[goal archive] abandoned: …"
  and are removed from `goals`. Integration, not loss — a trace remains.
- Call sites: in `update_self_model` after processing complete/abandon; in
  `handle_update_self_model` after `apply_edit` (uniform for `F3` too).

### Tests

`age_label` (buckets, boundaries); renders with a fixed `now` (labels on
goals and observations; a closed goal — from `closed_at`); returning evicted
entries + tool text; `maintenance_note` (80% threshold, empty model →
`None`); `fold_closed_goals` (holds K, oldest → scars, active goals
untouched); `closed_at` is set/reset; settings sanitization.

**Scope:** ~1 day. Files: `entities/self_model.rs`, `shared/config.rs`,
`features/tools/self_model.rs`, `app/orchestrator/generation.rs`,
`app/orchestrator/mod.rs`, `screens/self_model.rs`.

---

## Stage 3 — Reflection cadence: watermark instead of a counter, digest window

**Problem.** (1) The reflection digest is built from the *entire* chat —
every N replies, the model re-reflects over the same early material → insight
duplicates that consolidation then has to fix. (2) The cadence counter is
reset *before* the "already running"/"server not ready" gates — a skipped run
loses an entire cycle (at `every=10`, the next attempt is 10 replies away).
(3) The counters are per-chat, in-memory — lost on restart.

### Step 3.1 — a watermark in `Chat`

```rust
/// How many chat messages background auto-reflection has already covered (a
/// watermark index into `messages`) and when it last ran. Lives with the chat
/// (survives restart); history truncation (Ctrl+R/Ctrl+E) is handled by
/// clamping on read.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub reflected_upto: Option<usize>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub reflected_at: Option<DateTime<Utc>>,
```

Old chat files read fine without migration; `skip_serializing_if` keeps the
JSON clean for anyone with the feature off.

### Step 3.2 — `maybe_auto_reflect` on a window

- `wm = chat.reflected_upto.unwrap_or(0).min(chat.messages.len())` (clamp
  after history truncations); window `&chat.messages[wm..]`.
- Cadence: number of assistant replies in the window ≥ `every` → due. The
  orchestrator's `reflect_counts` field is **removed** (surviving restart is
  free — computed from data).
- Digest: `build_conversation_digest(&chat.messages[wm..])` — the signature
  already takes a slice, no need to change it. First run on an old chat
  (`wm=None`) — window = whole chat, as now, once.
- **The watermark advances only on an actual spawn** (all gates passed):
  `reflected_upto = len`, `reflected_at = now`, `mark_dirty(chat_id)`
  (debounced save already exists). Skipping a gate doesn't move the
  watermark — the cycle isn't lost.

### Step 3.3 — same reset semantics for consolidation

`maybe_auto_consolidate` stays on a counter (its digest is a notes overview,
not the conversation), but the counter reset moves to **after** all gates —
cycles stop being lost. (Optional unification onto a watermark is groundwork,
not in this PR.)

### Tests

Window: counting assistant replies from `wm`; clamping after truncation; the
watermark doesn't move when the server isn't ready/reflection is already
running (via `MockSupervisor` and existing orchestrator integration
patterns); it does move on spawn; serde round-trip of old JSON without the
fields. Consolidation: skipping a gate doesn't reset the counter.

**Scope:** ~0.5–1 day. Files: `entities/chat.rs`,
`app/orchestrator/reflection.rs`, `app/orchestrator/consolidation.rs`,
`app/orchestrator/mod.rs`.

---

## Stage 4 — User model: impersonation, behavioral signals, relationship dynamic

### Step 4a — `user_model` → impersonation

**Idea.** Impersonation writes a reply *on behalf of the user*, and
`user_model` is literally the model of that user; currently
`build_impersonation_request` doesn't see it.

- `entities/self_model.rs`:
  `UserModel::render_for_impersonation(&self, max_chars) -> Option<String>` —
  "Known about the person you're writing as: traits — …; interests — …;
  relationship with the assistant — …" (`None` when the model is empty).
- `orchestrator/impersonation.rs::handle_impersonate`: before assembling the
  request — `self_model_get(profile_id)`; the **gate** is the same as for the
  injection: the profile has `get_self_model` enabled (opt-in). The block is
  passed via a new parameter `build_impersonation_request(chat, system, seed,
  sampling, user_hint: Option<&str>)` and appended to the impersonation
  system message.
- Sampling/reasoning-forcing is left untouched (it has its own invariants,
  see the log).

Tests: the block appears in `request.system` when the gate is on and the
model is non-empty; absent when the gate is off / the model is empty;
existing builder tests updated (new parameter is `None`).

### Step 4b — the user's behavioral signals in the reflection digest

**Idea.** `Ctrl+R` (regenerate = "the reply wasn't good enough"), `Ctrl+E`
(delete exchange), and rewrite rounds are already archived into
`Chat.deleted` — strong implicit evidence that reflection never sees.

- `entities/chat.rs`: `enum DeletedCause { DeleteExchange, Regenerate, Rewrite }`
  + a field `#[serde(default)] cause: Option<DeletedCause>` on
  `DeletedExchange` (`None` = old entries, excluded from markers).
  `record_deleted(messages, draft, cause)` — three call sites to update:
  `generation.rs` (delete_last → `DeleteExchange`, regenerate →
  `Regenerate`, rewrite round → `Rewrite`).
- `orchestrator/reflection.rs`: a pure
  `fn behavior_markers(chat: &Chat, since: Option<DateTime<Utc>>) ->
  Option<String>` — counts `deleted` entries with `deleted_at > since`
  (= `reflected_at` from stage 3) by cause. Format: "Behavioral signals for
  this window: the user regenerated your reply ×2 (likely the reply wasn't
  good enough), deleted an exchange ×1. You yourself rewrote a reply ×1." —
  `Rewrite` is labeled as a signal about the agent's own behavior, not
  attributed to the user (also grist for the self-model). The block is
  appended to the digest in the reflection user message.
- `REFLECT_SYSTEM_MESSAGE`: clarify that the markers are evidence for
  `update_user_model`/`add_insight` (an observation, not a judgment; "accuracy
  over agreeableness" is already in the text).

Tests: `behavior_markers` — count by cause, `since` window filter, old
entries without `cause` are skipped, empty → `None`; serde round-trip of
`DeletedExchange` without the field.

### Step 4c — `relationship_dynamic` stops being silently overwritten

Relationship dynamic is the most significant field of the user model, and it
gets replaced wholesale with no trace (the `note` reminder currently only
fires for `remove_traits`/`remove_interests`). Change to `update_user_model`:
if the prior dynamic is non-empty, the new one differs, and `note` wasn't
passed — the same reminder-scar (widen the condition
`removed_traits || removed_interests || replaced_dynamic`). Tool description:
"…when the dynamic changes substantially, pass note — what changed and why."
Test.

**Stage scope:** ~1 day. Files: `entities/self_model.rs`, `entities/chat.rs`,
`app/orchestrator/impersonation.rs`, `app/orchestrator/reflection.rs`,
`app/orchestrator/generation.rs`, `features/tools/self_model.rs`.

---

## Stage 5 — Background task observability + contract cleanup

**Problem.** Reflection/consolidation errors are `tracing::debug`-only: a
stale cloud key means the feature silently stops working for months. An open
`F3` screen doesn't refresh after auto-reflection. `ToolContext.self_model` is
a dead field.

### Step 5.1 — task outcome and failure streak

- The `reflect_done`/`consolidate_done` channels carry `Result<(), String>`
  instead of `()`; in the background tasks the error log level goes
  `debug` → `warn` (+ `profile_id` in the fields).
- The orchestrator counts consecutive failures (`failures: u32` per task): on
  the 3rd in a row — a one-time `AppEvent::Error("Auto-reflection failed
  three times in a row: …last reason…")`, then silence until the first
  success (counter reset). The user learns the feature is broken, without
  spam.

### Step 5.2 — status-bar activity indicator

- `AppEvent::BackgroundTask { kind: BackgroundKind /* Reflection |
  Consolidation */, active: bool }` — emitted on spawn and on done.
- `ChatScreen` holds the flags; `status_bar` draws a quiet muted line
  "✻ reflecting" / "✻ notes sleep" next to the server chips while active.
  A static marker (no spinner) — needs no redraw ticks, disappears on the
  done event (the existing dirty mechanism).

### Step 5.3 — freshness of an open `F3`

- New event `AppEvent::SelfModelChanged` (no snapshot). Emitted: (a) after
  successful reflection; (b) in `handle_done`, if the turn included calls to
  the SelfModel tool group (scan the names in `ToolCallRecord` of
  `GenResult` messages against a new constant `self_model::ALL_IDS`).
- `runtime::apply_event`: if `ActiveScreen::SelfModel` is open — sends
  `AppCommand::RequestSelfModel` (re-request a fresh snapshot). **Does not**
  open the screen (unlike `SelfModelView`) and does nothing when closed.

### Step 5.4 — cleanup: dead field `ToolContext.self_model`

Tools read from the DB, reflection passes `None` — nobody uses the field
(`#[allow(dead_code)]`). Remove the field, fix three construction sites +
`testkit`. The contract stops making the false promise "a snapshot is
available."

### Tests

Orchestrator: a streak of 3 failures emits `Error` once, success resets it;
`SelfModelChanged` after a turn with a self_model call and after reflection;
runtime: `SelfModelChanged` sends `RequestSelfModel` when `F3` is open,
nothing when closed; status_bar: marker visible while active and disappears.

**Scope:** ~1 day. Files: `app/events.rs`, `app/orchestrator/{mod,reflection,
consolidation,generation}.rs`, `app/runtime.rs`, `screens/chat.rs`,
`widgets/status_bar.rs`, `features/tools/{mod,self_model}.rs`.

---

## Stage 6 — Shared silent agentic-loop runner + single-sourced policy

**Problem.** Three handwritten copies of the "stream → accumulate → invoke"
loop (generation, reflection, consolidation) + a duplicated `due()` + a
`cancel/counts/done_tx` triple × 2. The self-model maintenance policy is
smeared across three texts (the protocol, `REFLECT_SYSTEM_MESSAGE`, the
`reflect` rubric) and has already drifted slightly.

### Step 6.1 — `app/orchestrator/tool_loop.rs`: a silent runner

```rust
pub(super) struct SilentLoopParams { pub max_rounds: u32, pub timeout: Duration }

/// Silent agentic-loop for background tasks: stream → tool-call accumulator →
/// run allowed tools → next round; no UI events. Tolerates
/// Thoughts/ThoughtsSignature/Usage (ignored). Returns on a finish != ToolCalls,
/// empty tool calls, or the round limit; timeout and cancel are handled by the
/// caller (a spawn helper).
pub(super) async fn run_silent_tool_loop(
    backend: Arc<dyn EngineBackend>,
    registry: Arc<ToolRegistry>,
    ctx: ToolContext,
    request: ChatRequest,
    allowed: Vec<ToolId>,
    cancel: CancellationToken,
    params: SilentLoopParams,
) -> Result<()>
```

- Content = the current `spawn_reflection` loop; consolidation reuses it. A
  shared spawn helper (timeout + `warn` log + sending the outcome to the
  stage-5 done channel).
- The shared `due()` moves here, duplicates removed.
- Orchestrator fields: a `BackgroundLoop { cancel: Option<CancellationToken>,
  done_tx, failures: u32 }` type × 2 (reflection — no counter after stage 3;
  consolidation keeps its counter).
- **The main generation loop is deliberately left alone**: UI streaming,
  control-flow tools, Anthropic thinking signatures, usage, effects — its
  complexity doesn't pay for a shared sink trait right now. Groundwork: if
  background loops ever need Claude thinking, the runner will need to attach
  the signature, same as `generation.rs` (flag with a TODO comment).

### Step 6.2 — single-sourced maintenance policy

- In `features/tools/self_model.rs` — building-block constants:
  `POLICY_CORE` (integrate summary; work goals by #id; merge user_model;
  the fleeting → add_insight; accuracy over agreeableness; consolidate the
  narrative).
- `SELF_MODEL_MAINTENANCE_PROTOCOL` moves out of `generation.rs` here and is
  assembled from `POLICY_CORE` (FSD: `app → features` — allowed);
  `REFLECT_SYSTEM_MESSAGE` = reflection preamble + `POLICY_CORE`; the
  `reflect` rubric refers to the same wording rather than duplicating it.
- Existing text tests updated for the composition (check key phrases).

### Tests

Reflection and consolidation pass the previous integration tests with no
behavior change; `due` — one test set; compiles with no `#[allow]` shims.

**Scope:** ~1 day, mechanical. Files: `app/orchestrator/{tool_loop,
reflection, consolidation, mod, generation}.rs`, `features/tools/self_model.rs`.

---

## Stage 7 — Narrative as notes (separate track: design doc + probe)

**Not a PR, but the next document** (`docs/history/narrative-as-notes.md`, in the format
of this doc and [notes-connectivity.md](notes-connectivity.md)) — only the
decision frame is recorded here.

**Motive** (from architecture.md §9.9 and notes-connectivity's "out of
scope"): the narrative is a second, weaker copy of notes: append-only, FIFO,
no embeddings, no graph, no supersession — exactly the "accumulation
notebook" that notes have already moved past.

**Frame:**

- `SelfModel` remains the structural core: `summary` + `goals` + `user_model`.
  The narrative moves to `notes` under a reserved tag (e.g. `self`):
  `add_insight` becomes a wrapper over `note_save(tags=[self])` (keep the
  tool id — models already know it), getting **for free**: embeddings and
  semantic recall, duplicate gates on save, the link graph, supersede/merge
  with a "scar," consolidation, and auto-"sleep." The FIFO ceiling
  disappears — growth is managed by consolidation.
- `render_for_prompt`: the "Recent observations" block = N recent self-tagged
  notes (query by tag, `ORDER BY created_at DESC LIMIT N`).
- A one-time idempotent backfill of the existing narrative into notes (on a
  profile's first access; no schema change — the blob simply empties out).

**Questions to resolve in the doc (not here):** visibility of self-tagged
notes in general `note_recall` (proposal — visible with an `[about self]`
marker: "observations about self are the same kind of note," which also
supports the connectivity thesis); the fate of `consolidate_narrative`
(a thin wrapper over note tools, or deprecation in favor of direct
`note_merge`/`note_supersede`); editing the narrative from `F3` (Del on an
insight → supersede/hide the note); embedding-based gates for `user_model`
traits (near-duplicates "curious"/"inquisitive") — resolve at the same time,
with the same mechanism.

**Probe criterion (go/no-go):** does semantic recall find old insights that
FIFO used to lose; has the narrative stopped duplicating notes; does the
model use the gate on `add_insight` (rewriting instead of a near-duplicate).

---

## Out of scope (resolved/deferred)

- **The `system` injection stays as is** — the loss of the local model's
  prefix cache is accepted as the price of the feature (decision
  2026-07-03). The only action item is a note paragraph in architecture.md
  §9 about the conscious trade-off (do it alongside stage 2, where the
  render gets dates).
- **A sink trait unifying the main generation loop with the silent runner**
  — groundwork; revisit only if background loops gain streaming/signatures.
- **Embedding-based gates for `user_model` traits** — deferred pending
  stage 7's decision (same mechanism as notes; resolve as one piece).
- **Unifying the consolidation cadence onto a watermark** — groundwork (the
  counter with a fixed reset is sufficient).
- Prior groundwork items unchanged: vec0 for notes, linking notes ↔ RAG,
  structured traits with a lifecycle (rejected, see architecture.md §9.9).

## Summary

| Stage | Gist | Type | Scope |
|---|---|---|---|
| 1 | Atomic `self_model_update` | defect (race) | ~0.5 d. |
| 2 | Dates in renders, eviction report, folding closed goals | behavior | ~1 d. |
| 3 | Reflection watermark, digest window, reset-on-spawn | behavior | ~0.5–1 d. |
| 4 | user_model → impersonation; behavioral markers; dynamic with a scar | behavior | ~1 d. |
| 5 | Background task observability; fresh `F3`; `ToolContext` cleanup | UX/hygiene | ~1 d. |
| 6 | Silent loop runner; single-sourced policy | refactor | ~1 d. |
| 7 | Narrative as notes | track (doc+probe) | separate |

Total for stages 1–6: ~5–6 days of clean work, six independent PRs. After
each — a CLAUDE.md log entry and an architecture.md edit (§9 "Self-model,"
§11 "Concurrency").
