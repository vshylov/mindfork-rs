//! Schema versions of saved data and a **pure JSON-migration framework**.
//!
//! The single home for the schema-version constants (per artifact: settings/profiles/
//! chats/DB change at different rates, one global number would force "migrating"
//! untouched files). Only Value-level logic lives here (version detection, running
//! steps); file I/O, backups, and control-parsing into typed structs live in
//! [`crate::features::data_migration`] (orchestration; `shared` cannot depend on
//! `features`, and the pre-migration backup lives in `features::backup`). See
//! [docs/history/release-engineering.md](../../../docs/history/release-engineering.md) §3.4 and ADR 0006.
//!
//! **Bump policy** (release-engineering.md F12): an additive change (a new field with
//! `#[serde(default)]`, a new table/column with a default) — **no bump**, as before;
//! a breaking one (renaming/moving/changing semantics/removing a field) — bump the
//! constant + a migration step + a golden fixture of the old format + a CHANGELOG
//! entry (the "Data" rubric).

use std::cmp::Ordering;

use anyhow::{Context, Result};
use serde_json::Value;

/// Schema version of `settings.json`. Matches [`crate::shared::config::SCHEMA_VERSION`]
/// (the default of the `AppConfig.schema_version` field) — the invariant is checked by
/// a test.
pub const SETTINGS_SCHEMA: u32 = 1;
/// Schema version of `profiles.json`.
pub const PROFILES_SCHEMA: u32 = 1;
/// Schema version of a chat file `chats/<id>.json`.
pub const CHAT_SCHEMA: u32 = 1;
/// SQLite schema version (`PRAGMA user_version`). The DB migration runner is in
/// [`crate::shared::storage::db`] (baseline 0→1 + steps in transactions).
pub const DB_SCHEMA: u32 = 1;

/// A JSON migration step: a pure transformation "version `< to` → `to`".
pub struct Step {
    /// The target version this step produces.
    pub to: u32,
    /// A short description for the migration log (not user-facing text). Consumer —
    /// authoring the first real migration (there are no steps yet).
    #[allow(dead_code)]
    pub summary: &'static str,
    /// The value transformation. Must be pure (no I/O).
    pub apply: fn(Value) -> Result<Value>,
}

/// Description of a versioned JSON artifact: how to detect a value's version and the
/// chain of steps to the current one. The format doesn't change today (all schemas = 1,
/// `steps` are empty) — the framework is "in production" on empty migrations; the first
/// real migration will add a step + a fixture.
pub struct JsonArtifact {
    /// Name for logging/errors (`"settings.json"`).
    pub name: &'static str,
    /// The current (target) schema version.
    pub current: u32,
    /// Detects a value's version structurally (existing files aren't rewritten:
    /// "no version field → 1").
    pub detect: fn(&Value) -> u32,
    /// Steps `1→2→…→current`, in ascending order of `to`.
    pub steps: &'static [Step],
}

/// Verdict on a value's version relative to the artifact's current schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assessment {
    /// The version matches the current one — no migration needed.
    UpToDate,
    /// The version is older — steps `from → current` need to run.
    Migrate { from: u32 },
    /// The version is **newer** than the app (data from a newer mindfork version) —
    /// migration is impossible; the caller refuses to start (release-engineering.md F10).
    Downgrade { from: u32 },
}

impl JsonArtifact {
    /// Detects a value's version and compares it to the current one.
    pub fn assess(&self, v: &Value) -> Assessment {
        let from = (self.detect)(v);
        match from.cmp(&self.current) {
            Ordering::Equal => Assessment::UpToDate,
            Ordering::Less => Assessment::Migrate { from },
            Ordering::Greater => Assessment::Downgrade { from },
        }
    }

    /// Runs the steps `from → current`. The result is not yet validated by a typed
    /// parse — the orchestrator does that (control-parse before writing).
    pub fn apply_steps(&self, mut v: Value, from: u32) -> Result<Value> {
        for step in self.steps.iter().filter(|s| s.to > from) {
            v = (step.apply)(v)
                .with_context(|| format!("migration step {} → v{}", self.name, step.to))?;
        }
        Ok(v)
    }
}

/// `settings.json` version — by the `schema_version` field (absent → 1).
fn detect_settings(v: &Value) -> u32 {
    v.get("schema_version").and_then(Value::as_u64).unwrap_or(1) as u32
}

/// `profiles.json` version — historically a bare array (→ 1); an envelope shape with a
/// `schema_version` field will appear at the first breaking change.
fn detect_profiles(v: &Value) -> u32 {
    if v.is_array() {
        1
    } else {
        v.get("schema_version").and_then(Value::as_u64).unwrap_or(1) as u32
    }
}

/// Chat-file version — by the `v` field (absent → 1). The field is **not** written
/// while the schema stays at 1 (zero churn in existing files).
fn detect_chat(v: &Value) -> u32 {
    v.get("v").and_then(Value::as_u64).unwrap_or(1) as u32
}

/// The registry of artifacts (real, all `current = 1`, `steps` empty).
pub fn settings_artifact() -> JsonArtifact {
    JsonArtifact {
        name: "settings.json",
        current: SETTINGS_SCHEMA,
        detect: detect_settings,
        steps: &[],
    }
}

pub fn profiles_artifact() -> JsonArtifact {
    JsonArtifact {
        name: "profiles.json",
        current: PROFILES_SCHEMA,
        detect: detect_profiles,
        steps: &[],
    }
}

pub fn chat_artifact() -> JsonArtifact {
    JsonArtifact {
        name: "chats/<id>.json",
        current: CHAT_SCHEMA,
        detect: detect_chat,
        steps: &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn schema_constants_match_config_default() {
        // Invariant: the default of `AppConfig.schema_version` and settings' current stay in sync.
        assert_eq!(SETTINGS_SCHEMA, crate::shared::config::SCHEMA_VERSION);
    }

    #[test]
    fn detect_uses_field_or_defaults_to_one() {
        assert_eq!(detect_settings(&json!({"schema_version": 3})), 3);
        assert_eq!(detect_settings(&json!({})), 1);
        assert_eq!(detect_profiles(&json!([])), 1); // a bare array = v1
        assert_eq!(detect_profiles(&json!({"schema_version": 2})), 2);
        assert_eq!(detect_chat(&json!({"v": 5})), 5);
        assert_eq!(detect_chat(&json!({"title": "x"})), 1);
    }

    // A synthetic "current = 2" artifact with a 1→2 step — checks the engine separately
    // from the real registry (where all schemas = 1 and there are no steps).
    fn to_v2(mut v: Value) -> Result<Value> {
        v["schema_version"] = json!(2);
        Ok(v)
    }
    const SYNTH_STEPS: &[Step] = &[Step {
        to: 2,
        summary: "test bump",
        apply: to_v2,
    }];
    fn synth() -> JsonArtifact {
        JsonArtifact {
            name: "synthetic",
            current: 2,
            detect: detect_settings,
            steps: SYNTH_STEPS,
        }
    }

    #[test]
    fn assess_classifies_up_to_date_migrate_downgrade() {
        let a = synth();
        assert_eq!(
            a.assess(&json!({"schema_version": 2})),
            Assessment::UpToDate
        );
        assert_eq!(
            a.assess(&json!({"schema_version": 1})),
            Assessment::Migrate { from: 1 }
        );
        assert_eq!(a.assess(&json!({})), Assessment::Migrate { from: 1 });
        assert_eq!(
            a.assess(&json!({"schema_version": 3})),
            Assessment::Downgrade { from: 3 }
        );
    }

    #[test]
    fn apply_steps_runs_only_needed_steps() {
        let a = synth();
        let out = a
            .apply_steps(json!({"schema_version": 1, "x": 7}), 1)
            .unwrap();
        assert_eq!(out["schema_version"], json!(2));
        assert_eq!(out["x"], json!(7), "other fields are preserved");
        // from == current → no steps run (the value stays unchanged).
        let noop = a.apply_steps(json!({"schema_version": 2}), 2).unwrap();
        assert_eq!(noop["schema_version"], json!(2));
    }

    #[test]
    fn apply_step_error_propagates() {
        fn boom(_v: Value) -> Result<Value> {
            anyhow::bail!("broken")
        }
        const STEPS: &[Step] = &[Step {
            to: 2,
            summary: "boom",
            apply: boom,
        }];
        let a = JsonArtifact {
            name: "x",
            current: 2,
            detect: detect_settings,
            steps: STEPS,
        };
        assert!(a.apply_steps(json!({}), 1).is_err());
    }

    #[test]
    fn real_registry_is_all_current_v1() {
        for a in [settings_artifact(), profiles_artifact(), chat_artifact()] {
            assert_eq!(a.current, 1);
            assert!(a.steps.is_empty(), "{}: no steps yet", a.name);
        }
    }
}
