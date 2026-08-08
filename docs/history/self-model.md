# `SelfModel` interface sketch for `mindfork-rs`.

### 1. Core entity

```rust
// src/entities/self_model.rs

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The agent's model of "self" — its current view of itself, its goals, contradictions, and narrative.
/// Lives at the profile level (like notes/RAG).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfModel {
    pub id: Uuid,
    pub profile_id: Uuid,

    /// Model version (for tracking changes and a possible history)
    pub version: u64,

    /// Structured beliefs about self
    pub beliefs: Vec<Belief>,

    /// Current goals and intentions (can be long-running)
    pub goals: Vec<Goal>,

    /// Narrative history of "self" (how the agent perceives itself over time)
    pub narrative: Vec<NarrativeSegment>,

    /// Model of the user (how the agent represents the interlocutor)
    pub user_model: Option<UserModel>,

    /// Open questions the agent has posed to itself
    pub open_questions: Vec<OpenQuestion>,

    /// Recorded contradictions (in beliefs, goals, narrative)
    pub contradictions: Vec<Contradiction>,

    /// When reflection last ran
    pub last_reflection_at: Option<DateTime<Utc>>,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

### 2. Supporting types

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Belief {
    pub id: Uuid,
    pub content: String,           // "I value honesty in dialogue even above usefulness"
    pub strength: f32,             // 0.0–1.0 — how strongly the agent believes this
    pub source: BeliefSource,      // Where the belief came from
    pub created_at: DateTime<Utc>,
    pub last_reinforced_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BeliefSource {
    UserStatement,
    SelfReflection,
    ToolResult,
    InitialProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: Uuid,
    pub description: String,
    pub priority: GoalPriority,
    pub status: GoalStatus,
    pub created_at: DateTime<Utc>,
    pub deadline: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GoalPriority { Low, Medium, High, Critical }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GoalStatus { Active, Paused, Completed, Abandoned }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NarrativeSegment {
    pub id: Uuid,
    pub period: String,            // "early June 2026", "after the conversation about meaning"
    pub summary: String,           // Brief description of that period from the "self" perspective
    pub emotional_tone: Option<String>,
    pub key_insights: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserModel {
    pub id: Uuid,
    pub name: Option<String>,
    pub perceived_traits: Vec<String>,     // "curious", "skeptical", "deep"
    pub current_interests: Vec<String>,
    pub relationship_dynamic: String,      // how the agent perceives the current relationship
    pub last_updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenQuestion {
    pub id: Uuid,
    pub question: String,
    pub context: Option<String>,
    pub created_at: DateTime<Utc>,
    pub status: QuestionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QuestionStatus { Open, PartiallyAnswered, Resolved }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contradiction {
    pub id: Uuid,
    pub description: String,
    pub between: Vec<String>,      // which elements it's between (beliefs/goals/narrative)
    pub severity: f32,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}
```

### 3. Integration with the existing system

#### Updated `ToolContext`

```rust
// in features/tools/mod.rs or a separate file
pub struct ToolContext {
    pub profile_id: Uuid,
    pub chat_id: Uuid,
    pub system_message: String,
    pub effective_sampling: SamplingConfig,
    pub last_user_message_at: DateTime<Utc>,

    pub storage: Arc<Storage>,
    pub engine: Arc<dyn EngineBackend>,

    // New:
    pub self_model: Option<SelfModel>,   // snapshot at the time the tool was called
}
```

#### New effects

```rust
// in app/orchestrator or entities
pub enum ChatEffect {
    SetSystemMessage(String),
    SetSamplingOverride(PartialSamplingConfig),

    // New:
    UpdateSelfModel(SelfModelUpdate),
    AddBelief(Belief),
    RecordContradiction(Contradiction),
    UpdateUserModel(UserModel),
    AddNarrativeSegment(NarrativeSegment),
    ResolveContradiction { id: Uuid, resolution_note: String },
}
```

`SelfModelUpdate` could be a `Partial<SelfModel>` or a more explicit type carrying
only the fields tools are allowed to change.

### 4. Storage

I recommend storing `SelfModel` in **SQLite** (like notes and RAG), because:
- Queries and updates to individual parts are needed.
- Isolation by `profile_id` is already implemented.

A single `self_models` table with a JSONB field holding the whole model works (or
partially normalize it). Since the model is relatively small, JSONB is a reasonable
choice.

Add these methods to `shared/storage/db.rs`:
- `get_self_model(profile_id)`
- `save_self_model(model)`
- `update_self_model_partial(...)`

### 5. Example tools that would work with SelfModel

| Tool                          | What it does                                      | Returned effects                          |
|------------------------------|----------------------------------------------------|-------------------------------------------|
| `read_self_model`            | Returns the current `SelfModel` state              | —                                         |
| `reflect_on_interaction`     | Analyzes the last exchange and proposes updates    | `UpdateSelfModel`, `AddBelief`, `AddNarrativeSegment` |
| `update_beliefs`             | Changes/adds beliefs                                | `UpdateSelfModel`                         |
| `set_goal`                   | Sets or updates a goal                             | `UpdateSelfModel`                         |
| `detect_contradictions`      | Looks for contradictions in the current self-model | `RecordContradiction`                     |
| `revise_narrative`           | Rewrites the narrative to reflect new experience   | `AddNarrativeSegment`                     |
| `model_the_user`             | Updates the model of the user                      | `UpdateUserModel`                         |

### 6. Invariants (important)

- **The orchestrator is the sole writer** of `SelfModel`. Tools only return effects.
- **Isolation by `profile_id`** is strictly enforced.
- `SelfModel` is a **snapshot** in `ToolContext`. A tool doesn't see changes made by
  other tools in the same round.
- Versioning (`version`) helps track how much the model has changed recently.
- The narrative and beliefs must be **human-readable** — the model should be able to
  explain them to the user.

### 7. Questions for discussion

1. **How structured** should `SelfModel` be?
   Option A: many small fields (as above).
   Option B: mostly free text plus a few key structured blocks.

2. **Should a history of `SelfModel` changes be kept** (like note versioning), or is
   the current version plus the narrative enough?

3. **When exactly should SelfModel be updated?**
   - Only explicitly, via tools?
   - Or also add automatic reflection after every N messages?

4. **Relationship to `system_message`**:
   Should key beliefs/goals from `SelfModel` be automatically incorporated into the
   profile's system message?

# Tools

**Here's a list of tools** that would make sense to add for working with
`SelfModel`. I've grouped them by category and noted example arguments, returned
effects, and purpose (why this strengthens "self-awareness").

I based this on the project's current architecture (`ToolContext` + `ChatEffect`,
client-side agentic loop, isolation by `profile_id`).

### 1. Basic reading and introspection tools

| Tool                          | Arguments                  | Returned effects           | Purpose |
|------------------------------|----------------------------|-----------------------------|----------|
| `get_self_model`             | —                          | —                            | Get the full current `SelfModel` state |
| `get_core_beliefs`           | `limit?`, `min_strength?`  | —                            | Get key beliefs about self |
| `get_active_goals`           | `status?`                  | —                            | Current active goals |
| `get_self_narrative`         | `limit?`                   | —                            | Recent narrative segments |
| `get_user_model`             | —                          | —                            | How the model perceives the user |
| `get_open_questions`         | —                          | —                            | Open questions the model has posed to itself |
| `get_contradictions`         | `unresolved_only?`         | —                            | Recorded internal contradictions |

### 2. Reflection and update tools

| Tool                              | Arguments                          | Returned effects                              | Purpose |
|----------------------------------|-------------------------------------|-----------------------------------------------|----------|
| `reflect_on_last_exchange`       | —                                  | `UpdateSelfModel`, `AddBelief`, `AddNarrativeSegment` | Analyze the last exchange and update the self-model |
| `reflect_on_recent_period`       | `messages_count` or `hours`        | `UpdateSelfModel`, `AddNarrativeSegment`      | Reflect on the last N messages / hours |
| `consolidate_experience`         | —                                  | `UpdateSelfModel`, several `AddBelief`        | Consolidate experience ("sleep" / digestion, so to speak) |
| `add_belief`                     | `content`, `strength?`             | `AddBelief`                                   | Add a new belief about self |
| `update_belief`                  | `belief_id`, `new_content?`, `new_strength?` | `UpdateSelfModel`                        | Change an existing belief |
| `strengthen_belief`              | `belief_id`, `amount?`             | `UpdateSelfModel`                             | Reinforce a belief based on new experience |
| `revise_goal`                    | `goal_id`, `new_description?`, `new_priority?`, `new_status?` | `UpdateSelfModel` | Change a goal |
| `set_new_goal`                   | `description`, `priority?`         | `UpdateSelfModel`                             | Set a new goal |
| `abandon_goal`                   | `goal_id`, `reason?`               | `UpdateSelfModel`                             | Abandon a goal with an explanation |

### 3. Working with contradictions and coherence

| Tool                              | Arguments                     | Returned effects                          | Purpose |
|----------------------------------|--------------------------------|--------------------------------------------|----------|
| `detect_internal_contradictions` | —                              | `RecordContradiction` (several)            | Find contradictions between beliefs, goals, and narrative |
| `resolve_contradiction`          | `contradiction_id`, `resolution_note` | `ResolveContradiction`               | Resolve a recorded contradiction |
| `question_own_belief`            | `belief_id`                    | `UpdateSelfModel`, `AddOpenQuestion`      | Cast doubt on one of its own beliefs |
| `evaluate_self_consistency`      | —                              | `UpdateSelfModel` (with new contradictions) | Assess the overall consistency of the current self-model |

### 4. Narrative and self-identity

| Tool                          | Arguments                      | Returned effects                    | Purpose |
|------------------------------|----------------------------------|---------------------------------------|----------|
| `add_narrative_segment`      | `summary`, `emotional_tone?`, `key_insights?` | `AddNarrativeSegment`   | Add a new segment to the "self" history |
| `revise_self_narrative`      | `new_overall_summary?`         | `UpdateSelfModel`                     | Rewrite/integrate the narrative |
| `summarize_self_history`     | `period?`                      | `AddNarrativeSegment`                 | Produce a summary of long-term self-history |

### 5. Model of the user (UserModel)

| Tool                        | Arguments                          | Returned effects           | Purpose |
|----------------------------|--------------------------------------|-----------------------------|----------|
| `update_user_model`        | `perceived_traits?`, `current_interests?`, `relationship_dynamic?` | `UpdateUserModel` | Update the representation of the user |
| `revise_user_model`        | `changes`                            | `UpdateUserModel`          | A deeper update of the user model |

### 6. Advanced / metacognitive tools

| Tool                                    | Arguments                     | Returned effects                              | Purpose | Complexity |
|-----------------------------------------|---------------------------------|-----------------------------------------------|----------|---------|
| `perform_phenomenological_reduction`   | `aspect?`                      | `UpdateSelfModel`, `AddOpenQuestion`          | "Bracket" current beliefs and look with fresh eyes (epoché) | High |
| `simulate_alternative_self`            | `perspective` (critic / long_term_self / ethical_observer, etc.) | Result as a string + possible effects | Internal simulation of "another self" (a beefed-up `call_subagent`) | Medium |
| `generate_self_report`                 | `focus_areas?`                  | — (text report)                                | Produce a coherent report on the current state of "self" for the user | Medium |
| `detect_behavioral_inconsistency`      | —                                | `RecordContradiction`                          | Find discrepancies between stated beliefs and actual behavior in chat | High |
| `commit_to_long_term_identity`         | `statement`                     | `AddBelief`, `AddNarrativeSegment`             | Record a significant change in self-identity | Medium |

### Prioritization recommendations (my view)

**First tier (MVP SelfModel):**
- `get_self_model`
- `reflect_on_last_exchange`
- `add_belief` / `update_belief`
- `detect_internal_contradictions`
- `add_narrative_segment`
- `update_user_model`

**Second tier:**
- `reflect_on_recent_period`
- `consolidate_experience`
- `revise_goal` / `set_new_goal`
- `resolve_contradiction`
- `revise_self_narrative`

**Third tier (deeper self-awareness):**
- `perform_phenomenological_reduction`
- `simulate_alternative_self`
- `detect_behavioral_inconsistency`
- `generate_self_report`


## Draft roadmap for rolling out SelfModel in `mindfork-rs`

I put together a roadmap in several phases, with clear goals, deliverables, and
readiness criteria for each phase. I took into account the project's current
architecture (FSD, client-side agentic loop, `ChatEffect`, isolation by
`profile_id`, the orchestrator as sole writer).

### General roadmap principles

- Start with a minimum viable version (`SelfModel` + 3–4 key tools).
- Every new tool returns effects via `ChatEffect`.
- `SelfModel` is stored in SQLite (like notes and RAG).
- Tools are added incrementally, with the option to disable them.
- Focus on **reflection quality**, not tool count.
- After each phase — manual testing on Gemma/Qwen + update the
  [journal](../journal/self-model.md) and
  `architecture.md`.

---

### **Phase 1: Foundation — Basic self-model** (MVP)

**Goal:** The `SelfModel` entity appears, which the model can read and minimally
update.

**Deliverables:**
- `src/entities/self_model.rs` — core types (`SelfModel`, `Belief`, `Goal`,
  `NarrativeSegment`, `OpenQuestion`, `Contradiction`, etc.).
- Extend `ToolContext` — field `self_model: Option<SelfModel>`.
- New `ChatEffect` variants:
  - `UpdateSelfModel(SelfModelUpdate)`
  - `AddBelief(Belief)`
  - `AddNarrativeSegment(NarrativeSegment)`
- Storage:
  - Table `self_models` in SQLite (can store the whole model as JSONB +
    `profile_id` + `version`).
  - Methods in `shared/storage/db.rs`: `get_self_model`, `save_self_model`,
    `update_self_model`.
- Orchestrator:
  - Load `SelfModel` on profile/chat activation.
  - Apply the new effects.
- Tools (first 4):
  1. `get_self_model`
  2. `reflect_on_last_exchange`
  3. `add_belief`
  4. `get_active_goals`

**Phase readiness criteria:**
- The model can read its own current state via a tool.
- After calling `reflect_on_last_exchange` in a chat, new entries appear in
  `beliefs` and/or `narrative`.
- All changes are persisted and restored after a restart.
- Tests for isolation by `profile_id` + effect application.

**Rough scope:** 2–3 weeks (depending on pace).

---

### **Phase 2: Reflection & Structure**

**Goal:** The model gets tools for deeper work on itself (goals, contradictions,
narrative).

**Deliverables:**
- Full `Goal` support (creation, status/priority changes).
- Basic contradiction detection:
  - Tool `detect_internal_contradictions`
  - Type `Contradiction` + effect `RecordContradiction`
- Working with narrative:
  - `add_narrative_segment`
  - `revise_self_narrative` (basic version)
- Improved reflection:
  - `reflect_on_recent_period` (over the last N messages)
  - `consolidate_experience` (experience consolidation)
- Update `ToolContext` — a richer snapshot (including recent contradictions).
- First negative coherence tests (orchestrator).

**Readiness criteria:**
- The model can independently find contradictions between its beliefs and goals.
- Meaningful narrative segments appear.
- Reflection tools genuinely influence subsequent model responses (via the
  updated `SelfModel` in context).

---

### **Phase 3: Coherence & User Modeling**

**Goal:** Strengthen internal consistency + introduce a user model.

**Deliverables:**
- Full `UserModel` entity inside `SelfModel`.
- Tools:
  - `update_user_model`
  - `revise_user_model`
- Improved handling of contradictions:
  - `resolve_contradiction`
  - `question_own_belief`
- Tool `evaluate_self_consistency` (assess overall self-model coherence).
- Automatic enrichment of `ToolContext` with the user model.
- First experiments with automatic reflection (triggered by the orchestrator, not
  only by an explicit tool call).

**Readiness criteria:**
- The model starts taking the user model into account when answering (without
  explicit mention).
- Contradictions aren't just recorded — the model can resolve them.
- A noticeable improvement in long-term coherence in long conversations appears.

---

### **Phase 4: Advanced Meta-Cognition (Deep self-awareness)**

**Goal:** Tools that push behavior closer to genuine reflection and
self-observation.

**Deliverables:**
- Advanced tools:
  - `perform_phenomenological_reduction` (epoché — suspending current beliefs)
  - `simulate_alternative_self` (with different perspectives: critic,
    long-term-self, ethical observer, etc.)
  - `detect_behavioral_inconsistency` (discrepancy between stated words and
    actual chat behavior)
  - `generate_self_report` (a coherent report on the current state of "self")
- A `SelfModel` versioning mechanism (at least a simple change history).
- Integration with existing introspection tools (`set_system_message`,
  `set_sampling`, etc.) — they can now update `SelfModel`.
- A background reflection task (analogous to auto chat title generation).

**Readiness criteria:**
- The model can consciously question its own beliefs.
- Interesting, non-trivial self-reports appear.
- `simulate_alternative_self` gives qualitatively different perspectives than a
  plain `call_subagent`.

---

### **Phase 5: Integration, Polish & Evaluation**

**Goal:** Make the feature a fully-fledged part of the product.

**Deliverables:**
- Profile settings: which SelfModel tools are enabled.
- UI elements (optional at this stage):
  - View `SelfModel` in an overlay (analogous to the chat list).
  - Ability to manually edit key beliefs/goals.
- Full test suite (including replay tests and smoke tests on live models).
- Documentation updates:
  - `spec.md` (a new section on SelfModel)
  - `architecture.md`
  - Example prompts in the [journal](../journal/self-model.md)
- Metrics/quality evaluation:
  - Long-term coherence tests.
  - Comparing behavior with and without SelfModel.

**Readiness criteria:**
- The feature is stable and doesn't break existing functionality.
- There's a documented way to measure the benefit of SelfModel.
- Ready for use in real long conversations.

---

### Phase summary table

| Phase | Name                             | Key tools                                      | Complexity | Rough timeline | Focus |
|------|-----------------------------------|--------------------------------------------------|-----------|----------------|-------|
| 1    | Foundation                        | `get_self_model`, `reflect_on_last_exchange`, `add_belief` | Medium    | 2–3 wk         | Basic entity + storage |
| 2    | Reflection & Structure            | `detect_internal_contradictions`, `add_narrative_segment`, `consolidate_experience` | Medium    | 2 wk           | Reflection and goals |
| 3    | Coherence & User Modeling         | `update_user_model`, `resolve_contradiction`, `evaluate_self_consistency` | Medium+   | 2 wk           | Consistency |
| 4    | Advanced Meta-Cognition           | `perform_phenomenological_reduction`, `simulate_alternative_self`, `detect_behavioral_inconsistency` | High      | 3 wk           | Deep self-awareness |
| 5    | Integration & Polish              | —                                               | Medium    | 1–2 wk         | Stability and documentation |

**Overall estimate:** 10–12 weeks for the whole thing (working on this track as
the primary task).
