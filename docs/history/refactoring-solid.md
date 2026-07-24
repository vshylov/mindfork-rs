# Refactoring plan: targeted SOLID improvements (2026-07)

Design plan for this track. Status: **stages 1, 2, 4 done** (PR #125, in `main`);
**stage 3 done** — steps 3.1/3.2/3.3 (branch `refactor/settings-field-descriptors`).
Track finished.

Origin — a SOLID-compliance assessment across the whole codebase (2026-07-08):
architecture is healthy (transport behind traits, FSD, single sources of truth,
god objects broken up — [refactoring-god-objects.md](refactoring-god-objects.md)),
remaining debt is **targeted** and concentrated on four axes the project is
actively growing along. Plan — four independent stages, each its own PR.

Baseline: **808 unit tests green, 26 `#[ignore]` smokes** (main,
2026-07-08).

## 1. Context and diagnosis

| # | Axis | Principle | Symptom | Hot spot |
|---|-----|---------|---------|------|
| 1 | Assembling `ToolContext` | ISP, shotgun surgery | 11 fields as a literal in **8 places**; a new field means editing all of them (lived through in self-model refinement stage 5.4) | `features/tools/mod.rs:43` + 3 production and 5 test sites |
| 2 | Orchestrator background tasks | SRP, OCP along a growing axis | a trio of fields + a channel + a `select!` branch + a handler **per task**; the roadmap already lists the next one (self-model auto-consolidation) | `app/orchestrator/mod.rs` (fields, `run()`, `Quit`), `reflection.rs`, `consolidation.rs` |
| 3 | Settings screen fields | SRP, OCP | aspects of one field are smeared across **5 match sites** in 4 files (~80 `FieldId` variants, 311 usages) | `screens/settings/{mod,catalog,apply,helpers,choice}.rs` |
| 4 | Small targeted items | ISP, SRP | 10-argument status-bar signatures; named screen enumerations in runtime; 4 copy-pasted intent-dispatch blocks | `widgets/status_bar.rs`, `app/runtime/{dispatch,input,mod}.rs`, `screens/chat/mod.rs` |

Diagnosis without overreach: **no systemic rework is needed**. Closed enums +
exhaustive match are the idiomatic Rust choice (the compiler walks you through
every edit site), so part of the "OCP violation" is a deliberate cost, not
debt. Only treat axes where ripple actually gets in the way on every
extension.

## 2. Method and rules (shared across all stages)

Same playbook as the god-object breakup
([refactoring-god-objects.md §2](refactoring-god-objects.md)); additions to
suit these stages (here it's not moving code but **introducing small types/
helpers without changing behavior**):

1. **Behavior doesn't change.** No new features, no changes to message/event/
   log text (background-task error text is byte-for-byte: tests and the log
   depend on it). Diffs read as "fields grouped / sites collapsed into a
   helper."
2. **Invariants hold**: the orchestrator remains the sole owner of `Chat`; FSD
   (`screens`/`widgets` don't know about `app`); no new channels except the
   explicitly stated merge of two channels into one (stage 2).
3. **Gates on every PR**: `cargo fmt`, `cargo clippy --all-targets -- -D
   warnings`, `cargo test` — green; test count **hasn't dropped** (808+).
   Existing test function names don't change; only what's inside them is
   edited (e.g. building `ToolContext` via the new constructor).
4. **One stage — one PR**, branch from `main`. Stages are independent (except
   3.2 after 3.1) — order can be reshuffled.
5. **Docs**: architecture.md (touched §§) + a CLAUDE.md log entry per stage;
   once the track is finished — a note here.

---

## 3. Stage 1 — `ToolContext`: dependency bundles + constructor (ISP / ripple)

**Goal:** a new tool-context field touches **one file**, not 8; the
11-line literal at each assembly site disappears.

### Current state

`ToolContext` — 11 flat fields ([tools/mod.rs:43-69](../src/features/tools/mod.rs)):
turn identity (`profile_id`, `chat_id`), a chat snapshot (`system_message`,
`effective_sampling`, `last_user_message_at`), shared dependencies
(`storage`, `engine`, `embedder`), and config parameters (`chunk_params`,
`self_model_params`, `recall_includes_self`). Assembled as a **raw literal**:

- production: [generation.rs:231](../src/app/orchestrator/generation.rs),
  [reflection.rs:238](../src/app/orchestrator/reflection.rs),
  [consolidation.rs:125](../src/app/orchestrator/consolidation.rs);
- tests: `tools/mod.rs:390` (testkit), `web.rs:952`, `subagent.rs:181`,
  `rag.rs:789`, `fetch.rs:261`.

Config parameters are the growing part (three added by three different
features); every new one edits all 8 sites. The ISP slack (`calculate`/
`current_time` get `storage`/`engine` they don't need) — **not fixed**:
segregating the context by tool group isn't worth it.

### Target state

Three building blocks + a constructor; **`ToolContext`'s flat public fields
stay** → tool code (`ctx.storage`, `ctx.chunk_params`, …) is untouched:

```rust
/// Long-lived shared dependencies (an Arc bundle; changes on server restart,
/// not turn to turn).
#[derive(Clone)]
pub struct ToolDeps {
    pub storage: Arc<Storage>,
    pub engine: Arc<dyn EngineBackend>,
    pub embedder: Arc<dyn Embedder>,
}

/// Tool parameters from config (a per-turn snapshot).
#[derive(Clone)]
pub struct ToolParams {
    pub chunk_params: rag::ChunkParams,
    pub self_model_params: SelfModelParams,
    pub recall_includes_self: bool,
}
impl ToolParams {
    pub fn from_config(cfg: &AppConfig) -> Self { … } // the one place that maps config
}

/// Turn snapshot (what a tool sees about the current chat).
pub struct TurnInfo {
    pub profile_id: Uuid,
    pub chat_id: Uuid,
    pub system_message: String,
    pub effective_sampling: SamplingConfig,
    pub last_user_message_at: Option<DateTime<Utc>>,
}

impl ToolContext {
    /// Unpacks the bundles into the old flat fields.
    pub fn new(deps: ToolDeps, params: ToolParams, turn: TurnInfo) -> Self { … }
}
```

FSD stays clean: `features/tools` already imports `shared::config` (`ToolConfig`
references `DEFAULT_SUBAGENT_*`), so `ToolParams::from_config(&AppConfig)` is
legal.

### Steps

1. `tools/mod.rs`: add `ToolDeps`/`ToolParams`/`TurnInfo` +
   `ToolContext::new` (struct fields don't change).
2. Orchestrator: a helper `fn tool_deps(&self, backend: Arc<dyn EngineBackend>) ->
   ToolDeps` (in `orchestrator/mod.rs`, next to the other shared helpers);
   the three production sites switch to
   `ToolContext::new(self.tool_deps(…), ToolParams::from_config(&self.config), TurnInfo { … })`.
3. testkit: `ctx_with_storage` builds via `new`; add
   `ctx_with_backends(profile_id, engine, embedder)` for the four sites with a
   custom engine/embedder (web/subagent/rag/fetch) — their literals go away.
4. Ripple check: a "trial" edit (add a dummy field to `ToolParams`, confirm
   the compiler only demands an edit to `tools/mod.rs`) — reverted before
   commit.

### What we do NOT do

- Don't nest bundles as `ToolContext` fields (`ctx.deps.storage`) — would
  touch all ~15 tool files for zero functional gain.
- Don't touch `#[allow(dead_code)] chat_id` or the snapshot semantics.
- Don't introduce trait segregation of the context by tool group.

**Risks:** minimal (constructor + swapping literals). **Effort:** ~0.5 session.

**DoD:** 808+ tests green; `rg "ToolContext \{" src` finds **only**
`ToolContext::new` (the single literal in the constructor); tool files
(besides test constructions) unchanged.

**Status: done** (branch `refactor/tool-context-bundles`). `ToolDeps`/
`ToolParams`/`TurnInfo` + `ToolContext::new` in `tools/mod.rs`; a helper
`Orchestrator::tool_deps`; three production sites (generation/reflection/
consolidation) switched to `new` (reflection/consolidation get parameters via
`ToolParams::from_config`; generation also holds `ToolParams::from_config`,
but `self_model_params` is computed separately — it still flows into
`GenSpawn` for injection). testkit gained `ctx_with_backends` (custom
engine/embedder) and `ctx_with_deps` (a shared bundle for the rag isolation
test); literals in web/subagent/rag/fetch collapsed onto them. Ripple check:
adding a field to `ToolContext` requires editing **only**
`ToolContext::new`. **808 tests green**, 26 `#[ignore]`, clippy `-D warnings`/
fmt clean.

---

## 4. Stage 2 — background tasks: a single done channel + slot registry (SRP / OCP)

**Goal:** adding a third silent background task (the roadmap already names
self-model auto-consolidation on a timer — architecture.md §9.9) doesn't
touch the `run()` skeleton, the `Quit` branch, or spawn more fields/channels/
handlers.

### Current state

The family of "silent" background tasks (a UI-less mini agentic loop:
reflection + consolidation, sharing the `tool_loop::spawn_silent_loop`
runner) is served by **lifecycle copy-paste**:

- `Orchestrator` fields ([mod.rs:232-280](../src/app/orchestrator/mod.rs)):
  `reflect_cancel`/`reflect_done_tx`/`reflect_failures` +
  `consolidate_cancel`/`consolidate_done_tx`/`consolidate_failures` (6 total);
- two channels (`reflect_done`, `consolidate_done`) and two `select!`
  branches in `run()` ([mod.rs:186-195](../src/app/orchestrator/mod.rs));
- the `Quit` branch manually enumerates every cancellation token
  ([mod.rs:341-357](../src/app/orchestrator/mod.rs));
- near-identical handlers `handle_reflect_done`
  ([reflection.rs:290-310](../src/app/orchestrator/reflection.rs)) and
  `handle_consolidate_done`
  ([consolidation.rs:175-192](../src/app/orchestrator/consolidation.rs)):
  drop the cancel token → clear the indicator → on `Ok` reset the failure
  streak (+`SelfModelChanged` for reflection) → on `Err` bump the streak and
  fire one error at the `BACKGROUND_FAILURE_ALERT` threshold.

Self-model refinement stage 6 rejected "field grouping" as cosmetic — back
then there were only two tasks and ripple wasn't growing. The reassessment
trigger, fixed in the write-up: **a third task in the family** (already on
the roadmap). This stage is prep work for the framework — the new task
itself is **not** added (the refactor doesn't mix with the feature).

### Target state

The registry key is the existing `BackgroundKind` (`app/events.rs:197`, no
new enum). A new module `app/orchestrator/background.rs` (~80 lines):

```rust
/// A silent background-task slot: the active run's token + a failure streak.
/// The streak outlives a single run (survives completions) — hence a slot,
/// not a task.
#[derive(Default)]
pub(super) struct BgSlot {
    cancel: Option<CancellationToken>, // Some — a run is in progress (one at a time)
    failures: u32,
}

impl Orchestrator {
    pub(super) fn bg_running(&self, kind: BackgroundKind) -> bool { … }
    /// Records a start: slot.cancel = Some + BackgroundTask{active:true}.
    pub(super) fn begin_bg(&mut self, kind: BackgroundKind, cancel: CancellationToken) { … }
    /// Shared outcome handler (former handle_reflect_done/handle_consolidate_done).
    pub(super) fn handle_bg_done(&mut self, kind: BackgroundKind, result: Result<(), String>) { … }
    pub(super) fn cancel_all_bg(&self) { … } // for the Quit branch
}

fn kind_label(kind: BackgroundKind) -> &'static str {
    // "Auto-reflection" / "Auto-consolidation" — error text is assembled
    // from label BYTE-FOR-BYTE with the current strings (tests depend on it).
}
```

- `Orchestrator` fields: 6 → 2 (`bg: HashMap<BackgroundKind, BgSlot>` +
  `bg_done_tx: UnboundedSender<(BackgroundKind, Result<(), String>)>`).
- `run()`: two channels and two `select!` branches → one channel and one
  branch (`orch.handle_bg_done(kind, res)`).
- `SilentLoop` gains a `kind: BackgroundKind` field; `done_tx` sends
  `(kind, outcome)` ([tool_loop.rs:49](../src/app/orchestrator/tool_loop.rs)).
- Kind-specific logic lives **in one place** (`handle_bg_done`): `Reflection`
  on `Ok` additionally sends `SelfModelChanged`; `Consolidation` does not
  (current behavior, see the comment at consolidation.rs:173).
- The "already running" gates in `maybe_auto_reflect`/`maybe_auto_consolidate`
  → `self.bg_running(kind)`; spawn tails (setting the cancel token +
  indicator) → `self.begin_bg(kind, cancel)`.
- `Quit`: the reflect/consolidate token enumeration → `cancel_all_bg()`
  (gen/rag/imp stay as they are).

### What we do NOT do (family boundaries)

- **Impersonation** — not in the family: its own done-channel protocol
  (`(Uuid, FinishReason)`), streams to the UI, `imp_gen`. Not touched.
- **RAG indexing** — not in the family: no done channel (progress goes
  through `AppEvent::RagProgress` directly), only `rag_cancel`. Not touched.
- **Auto-naming** (`title_tx`) — its own result carrying a chat id. Not
  touched.
- `consolidate_counts` (per-chat cadence) — cadence data, not task
  lifecycle; stays a field as is (reflection tracks cadence via a
  watermark on `Chat` — the two **cannot** be unified without a behavior
  change).

**Risks:** medium — touches the `run()` skeleton; mitigation: error/event
text stays byte-for-byte, behavior is checked by existing integration tests
(a streak of 3 failures → one error; success resets it and sends
`SelfModelChanged`; the watermark only moves on spawn). The `bare_orch*`
fixtures in `tests/mod.rs` build `Orchestrator` as a literal — field edits
happen in **one** place. **Effort:** ~1 session.

**DoD:** 808+ tests green with no renames; the `reflect_*`/
`consolidate_cancel|_done_tx|_failures` fields are gone; `run()` has one bg
branch; `rg "BACKGROUND_FAILURE_ALERT" src` shows a single consumer —
`handle_bg_done`.

**Status: done** (branch `refactor/bg-task-slots`). New module
`orchestrator/background.rs`: `BgSlot { cancel, failures }` + methods
`bg_running`/`begin_bg`/`handle_bg_done`/`cancel_all_bg` (+
`#[cfg(test)] bg_failures`) + `kind_label` (error text byte-for-byte).
Orchestrator fields 6 → 2 (`bg: HashMap<BackgroundKind, BgSlot>` +
`bg_done_tx`); `consolidate_counts` kept (cadence data). `run()`: two
channels/branches → one `bg_done` + one branch; `Quit` → `cancel_all_bg`.
`SilentLoop` gained `kind`, `done_tx` sends `(kind, outcome)`.
`BackgroundKind` gained `Hash`. Tests switched to the new API with no
renames. **808 tests green**, clippy/fmt clean.

---

## 5. Stage 3 — settings: field descriptors (SRP / OCP; staged)

The biggest shotgun-surgery node: adding one settings field today touches up
to **five match sites in four files** — the catalog row
([catalog.rs](../src/screens/settings/catalog.rs)), `apply_text` (~50 arms,
[apply.rs:415-633](../src/screens/settings/apply.rs)) or
`toggle_field`/`cycle_field` (apply.rs:256-387), `field_description` (~40
arms, [helpers.rs:21-208](../src/screens/settings/helpers.rs)), validation
(`field_num_kind`), plus a default for `Del` reset and the `•` marker via
`default_fields`.

The god-object plan (§4) deliberately deferred the descriptor table with a
return trigger: "if `catalog.rs`+`apply.rs` keep growing faster than the
rest after the split." The log confirms this (the flow of settings PRs
hasn't stopped) — this stage exercises the deferred option, **but in steps
with their own standalone value**, so it's possible to stop after any of
them.

An important precedent on this very screen: **sampling is already built
descriptor-style** — `SamplingParam` carries `label`/`field_name`/
`num_kind`/`group`/`description`
([settings/mod.rs:247-427](../src/screens/settings/mod.rs)), values flow
through shared `sampling_row`/`apply_sampling_text` (helpers.rs). 28
parameters × 2 subsections are served by one table — the pattern is proven
in the codebase.

### Step 3.1 — field description moves into `FieldRow` (cheap, standalone value)

- `FieldRow` gains `description: Option<&'static str>` + a builder method
  `fn describe(self, d: &'static str) -> FieldRow`.
- Text from `field_description` moves to where each row is built: section
  ones — into catalog.rs; shared between the Assistant/Impersonation pair —
  into `managed_rows`/`cloud_rows` (helpers.rs), where **one** description
  string automatically covers both `FieldId`s of the pair (currently
  duplicated across arms like `XNoMmap | IxNoMmap => …`); sampling —
  `sampling_row` supplies `p.description()` (source untouched).
- Consumers: the bottom panel ([render.rs](../src/screens/settings/render.rs))
  and the search index ([search.rs](../src/screens/settings/search.rs)) read
  `row.description` instead of calling `field_description(id)`; the
  190-line match is removed.
- Tests calling `field_description` directly switch to reading the catalog
  row's string (test names unchanged).

**Step outcome:** label + group + description of a field live in **one**
place. Low risk. Effort ~0.5 session.

**Status: done** (branch `refactor/settings-field-descriptors`). `FieldRow`
gained `description: Option<&'static str>` + the builder `describe(d)`.
`field_description` text (the 190-line match) moved to where each row is
built: section-specific (Tools/Memory/Interface) — inline literals in
catalog.rs; shared across multiple places (mode/model name/API key env/
subsection) — `const DESC_*` in helpers.rs; `-ngl`/`--jinja` (differ
between assistant/impersonation) — in `ManagedFieldIds`; other managed
fields (no-mmap/flash-attn/spec-*) — inline in `managed_rows`; sampling —
`sampling_row` supplies `p.description()`. Consumers (the bottom panel in
render.rs, the search index `collect_hits`) read `row.description`; the
match was removed. Note: descriptions now only exist for **visible** rows
(draft fields — only when spec_type=draft-*); tests for
`field_description(id)` were switched to the `field_desc(&screen, id)`
helper (builds section fields and finds the string). **808 tests green**,
clippy/fmt clean.

### Step 3.2 — value access via a field spec table (core)

Collapse the remaining four match sites (**toggle/cycle/apply_text/
validation** + the derived `reset_field`/`•` marker) into **one table** —
honest framing: five `FieldId` matches → one.

```rust
/// Access to a config field's value. fn pointers (not closures) — 'static, no
/// capturing; mode routing (external vs cloud_mut) lives INSIDE the setter —
/// it has access to the whole AppConfig.
enum Access {
    Toggle { get: fn(&AppConfig) -> bool,   set: fn(&mut AppConfig, bool) },
    Text   { get: fn(&AppConfig) -> String, set: fn(&mut AppConfig, &str) },
    Choice { get: fn(&AppConfig) -> String, cycle: fn(&mut AppConfig, i32),
             options: fn(&AppConfig) -> (Vec<String>, usize) },
}

struct FieldSpec {
    label: &'static str,
    description: Option<&'static str>,
    num: Option<NumKind>, // editor validation
    access: Access,
}

/// The ONE match by FieldId (the table). Returns None for fields outside
/// scope (profile fields, subsection selectors, sampling — see boundaries).
fn field_spec(id: FieldId) -> Option<FieldSpec> { … }
```

Consumers after this step:

- `toggle_field`/`cycle_field`/`apply_text` → "if a spec exists, apply via
  `access` and `save_config()`; else the old path" (profiles/sampling/
  navigation);
- `field_validation_error` → `spec.num`;
- `reset_field` → `set(cfg, get(&AppConfig::default()))` — the current
  "`default_fields` + repeated navigation" combo simplifies;
- the `•` "modified" marker → `get(cfg) != get(&default)`;
- catalog builders → `spec_row(&self.config, id)` (label/description/value
  from the spec), order and **mode-driven visibility stay imperative** in
  catalog.rs — that's deliberate display logic, not a field property;
- the Choice popup ([choice.rs](../src/screens/settings/choice.rs)) →
  `options` from the spec (step 3.3, can be split off).

**Scope boundaries (important for staying mechanical):**

- **Config fields only.** Out of scope: profile fields (`PName`/`PSystem`/
  `PGreeting`/`PImpSystem`/`PSelect`/`PTool(idx)` — operate over
  `profiles[profile_idx]`, not `AppConfig`; 6 kinds, old path stays),
  subsection selectors (`ModelSub`/`SamplingSub`/`ProfileSub` — navigation),
  `IDicts` — in scope if desired (the setter parses a list).
- **Sampling untouched**: `S(p)`/`IS(p)` are already descriptor-based via
  `SamplingParam`; wrapping a table in a table would be redundant
  indirection.
- The Assistant/Impersonation pairs (`X*`/`Ix*`) — separate specs with
  shared `const` text (fn pointers can't capture "which engine," so it's
  one spec per FieldId; descriptions are already shared after 3.1).

**Rollout order** — by family, each a green commit: (a) Interface + Tools +
Memory (simple direct fields — dry-run of the pattern); (b) engines
X*/Ix*/E* (external/cloud routing inside setters); (c) `reset_field`/`•`
to a `get` comparison; (d) 3.3 — Choice-field options.

**Trade-off, accepted deliberately:** loses the compiler's exhaustiveness
check across five matches ("forgetting a field" now equals "forgetting a
row in one table" — the exact same risk as "forgetting a catalog row"
today). In exchange — a field is read in one place. Safety net — the
existing ~134 settings tests (tests.rs, 1244 lines) + the
`all_labels_fit_alignment_cap` gate.

**Rollback/stop criterion:** if after step (a) the table reads worse than
the old matches, or the test diffs balloon — stop at 3.1 (it stands on its
own) and record the decision here.

**Risks:** medium-high (the heaviest UI node, 5118 lines); mitigated by
staging and tests. **Effort:** 3.1 — ~0.5 session; 3.2 — 1–2 sessions;
3.3 — ~0.5.

**DoD (full stage):** `field_description`/`toggle_field` config arms/
`cycle_field` config arms/config branches of `apply_text` removed; only
**two** structural matches remain by `FieldId` (the `field_spec` table +
catalog builders) instead of six; 808+ tests green.

**Status: done** (branch `refactor/settings-field-descriptors`). 3.2/3.3: a
new module `screens/settings/spec.rs` — `enum Access { Toggle(fn(&mut AppConfig)) |
Text(fn(&mut AppConfig,&str)) | Choice { cycle, options } }` + `FieldSpec { access, num }`
+ a single `field_spec(id) -> Option<FieldSpec>` over config fields (fn
pointers, external/cloud routing inside the setters). Consumers collapsed
onto the table: `toggle_field`/`cycle_field`/`apply_text` (config arms) →
`field_spec`; `field_num_kind` → `field_spec.num`; `choice_menu` (config
Choice) → `field_spec.options` (this is step 3.3). Outside the table (old
path): sampling `S(p)`/`IS(p)`, profile fields, subsection selectors/
`PSelect` — navigation. **Step (c) (reset/`•` marker on a `get` comparison)
was NOT done**: `reset_field`/the marker already work generically through
`default_fields()` (no per-field arm) — there was no collapse target there.
Catalog builders (label/value/description) untouched. **808 tests green**,
clippy/fmt clean. **Track (stages 1–4) finished.**

---

## 6. Stage 4 — small targeted improvements (ISP / SRP)

Four independent mini-fixes; 4a+4b — one PR, 4c/4d — optional.

### 4a. Status-bar view model

`status_bar::render`/`height` carry **10 arguments**
(`#[allow(clippy::too_many_arguments)]`,
[status_bar.rs:36-89](../src/widgets/status_bar.rs)); every new indicator
(the last were the reflection/consolidation `background` chips) extends both
signatures and every call site.

```rust
/// State snapshot for the status line (the screen assembles it in one place).
pub struct StatusModel<'a> {
    pub statuses: &'a ServerStatuses,
    pub generating: bool,
    pub tokens: u64,
    pub context: Option<u64>,
    pub context_exact: bool,
    pub mouse_scroll: bool,
    pub background: Option<&'a str>,
}
pub fn render(frame: &mut Frame, area: Rect, model: &StatusModel, palette: &Palette)
pub fn height(width: usize, model: &StatusModel, palette: &Palette) -> u16
```

`ChatScreen` assembles the model with one private helper (`status_model()`
in `chat/render.rs`) — a new indicator = a field + filling it in + rendering,
no signature churn. Both `#[allow(too_many_arguments)]` are removed.
status_bar tests switch to a model literal (mechanical). Effort: ~0.3
session.

### 4b. Canonical `ActiveScreen` broadcast helpers + a single intent dispatcher

Right now adding a screen touches 6–8 scattered places; we collapse the
screen enumerations to **one canonical place** — methods on `ActiveScreen`
itself (`app/runtime/mod.rs`, next to the enum):

- `ActiveScreen::set_palette(&mut self, palette: Palette)` — folds the
  named palette block
  [dispatch.rs:62-77](../src/app/runtime/dispatch.rs) (the `Settings` arm
  keeps its own `refresh` — its semantics are broader than palette);
- `ActiveScreen::handle_paste(&mut self, chat: &mut ChatScreen, text: &str)`
  — folds the paste routing [input.rs:166-175](../src/app/runtime/input.rs);
- a local `enum AnyIntent { Chat(..), List(..), Settings(..), SelfModel(..) }`
  + one `dispatch_any(intent, cmd_tx, screen, active) -> bool` — replacing
  "pull 4 Options + 4 near-identical if blocks"
  ([input.rs:180-209](../src/app/runtime/input.rs); the current shape is a
  workaround for a borrow conflict, `dispatch_any` resolves it with single
  ownership);
- and, folded into the same PR, 4d: moving the clipboard side effect out of
  `apply_event`
  ([dispatch.rs:43-58](../src/app/runtime/dispatch.rs)) into a private
  helper `deliver_clipboard(screen, active, clipboard, text)` — `apply_event`
  no longer knows about `arboard`.

The `match` over screens doesn't disappear (that's not the goal — enum
dispatch is idiomatic here), but a new screen adds branches in
**predictable canonical places** within one module. Effort: ~0.4 session.

### 4c. (Optional) grouping `ChatScreen` fields

~40 fields ([chat/mod.rs:174-249](../src/screens/chat/mod.rs)) — the
implementation is already split across submodules, but the state is one
struct. Following the orchestrator Phase 3 precedent
(`EngineManager`/`SaveQueue`), group two cohesive trios:

- `TokenCounters { tokens, context, context_exact }` (the `gen_*` fields);
- `SpellState { checker, dirty, last_edit }` (the `spell`/`spell_dirty`/
  `last_edit` fields; `draft_dirty` is not included, it's about the draft).

Popups are **not** grouped and a modal enum is **not** introduced — already
deferred by the god-object plan §4 as a behavioral change. 4c's value is
readability; do it only if `chat/` is being touched anyway (not as a
standalone PR just for this).

**Stage 4 risks:** low (mechanics + tests as a safety net). **Effort:**
4a+4b(+4d) — ~0.5–1 session in one PR; 4c — ~0.5 alongside other work.

**DoD:** status-bar signatures ≤4 parameters, no `allow`; `dispatch.rs`/
`input.rs` have no named screen enumerations outside `ActiveScreen` methods/
`dispatch_any`; 808+ tests green.

**Status: 4a/4b/4d done** (branch `refactor/status-bar-runtime`). 4a:
`StatusModel<'a>` (a snapshot from `ChatScreen::status_model`), `render`/
`height` — 4/3 parameters, no `too_many_arguments`. 4b:
`ActiveScreen::set_palette`/`handle_paste` (palette broadcast/paste
routing), `AnyIntent` + `dispatch_any` (single ownership instead of 4
`Option`s). 4d: `deliver_clipboard` (`apply_event` doesn't know about
`arboard`). The per-event `match` in `apply_event` was deliberately left
as is. **4c (grouping `ChatScreen` fields) — not done** (per the plan —
only alongside `chat/` edits, not worth a standalone PR). 808 tests
green, clippy/fmt clean.

---

## 7. What we deliberately do NOT do (track boundaries)

Fixed by the 2026-07-08 assessment — these are trade-offs, not debt:

- **`Storage` behind a trait / repository abstractions** — a concrete
  `Arc<Storage>` is deliberate: tests are fast (`:memory:`), no second
  implementation is anticipated, a trait over ~47 methods would be a header
  interface. The future seam already exists (`db/` split by domain).
- **`trait Screen`/`trait Widget`** — enum dispatch of screens is idiomatic
  and transparent; a trait would blur the differing per-screen
  snapshots/intents.
- **Merging `ChatIntent` ↔ `AppCommand`** — the 1:1 duplication is the price
  of the "screens don't know about app" FSD invariant; merging would break
  the layering.
- **A `ProviderProfile` table** (centralizing provider capability knowledge)
  — that axis is already served by `supported_sampling_fields`/
  `WireDialect`/`cloud()`; a table only pays off at provider #4+.
- **An extension bag for sampling** — deferred by ADR 0004, status unchanged.
- **Folding the generation loop into `tool_loop`** — decided "no" (self-model
  refinement stage 6): streaming/control-flow/thinking signatures don't pay
  for a shared sink.
- **A modal enum for `ChatScreen` popups** — deferred by the god-object plan
  §4 (a behavioral change).
- **Crate split** — rejected by ADR 0004.

## 8. Order, independence, effort estimate

Stages are independent (3.2 comes after 3.1). Recommended order — by
value/cost:

| Order | Stage | Effort | Risk |
|---|------|-------|------|
| 1 | Stage 1 — `ToolContext` | ~0.5 session | low |
| 2 | Stage 4a+4b(+4d) — status bar + runtime | ~0.5–1 | low |
| 3 | Stage 2 — background tasks | ~1 | medium |
| 4 | Stage 3.1 — descriptions in `FieldRow` | ~0.5 | low |
| 5 | Stage 3.2 (+3.3) — field specs | 1–2 | medium-high |
| — | Stage 4c — grouping `ChatScreen` | ~0.5 | low (alongside other work) |

After stages 1–2 and 4, pause and reassess: if the flow of settings edits
continues — proceed to 3.2; if it has tapered off — stop at 3.1.

## 9. Definition of Done (whole track)

- A new `ToolContext` field touches one file; a new silent background task
  doesn't touch `run()`/`Quit`/the outcome handler; a new config settings
  field is described in ≤2 places (the table + the section builder); a new
  status-bar indicator doesn't change signatures.
- Behavior unchanged: event/error/log text byte-for-byte; test count hasn't
  dropped (808+), test function names preserved; `#[ignore]` smokes
  untouched.
- `cargo fmt` / `cargo clippy --all-targets -- -D warnings` clean after
  every stage.
- architecture.md (§3 module map, §8 tools, §11 concurrency — as touched)
  and CLAUDE.md (log) updated at every stage; stage status marked in this
  document.
