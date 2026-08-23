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
pub const SETTINGS_SCHEMA: u32 = 2;
/// Schema version of `profiles.json`.
pub const PROFILES_SCHEMA: u32 = 1;
/// Schema version of a chat file `chats/<id>.json`.
pub const CHAT_SCHEMA: u32 = 2;
/// SQLite schema version (`PRAGMA user_version`). The DB migration runner is in
/// [`crate::shared::storage::db`] (baseline 0→1 + steps in transactions).
pub const DB_SCHEMA: u32 = 1;

/// A JSON migration step: a pure transformation "version `< to` → `to`".
pub struct Step {
    /// The target version this step produces.
    pub to: u32,
    /// A short description for the migration log (not user-facing text).
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
            tracing::info!(
                artifact = self.name,
                to = step.to,
                "migration step: {}",
                step.summary
            );
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

/// Chat-file version — by the `v` field (absent → 1). The field was not written
/// while the schema stayed at 1; since 2 every save writes it (`Chat::v`), so a
/// migrated file is never detected as 1 again.
fn detect_chat(v: &Value) -> u32 {
    v.get("v").and_then(Value::as_u64).unwrap_or(1) as u32
}

/// The values `settings.json` v1 wrote for the sub-agent knobs by default —
/// frozen here, because a migration step describes the past and must not follow
/// the constants in `config.rs` when those move again.
const V1_SUBAGENT_TIMEOUT_SECS: u64 = 60;
const V1_SUBAGENT_MAX_TOKENS: u64 = 1024;

/// `settings.json` 1→2: the sub-agent gained tools (spec §9.3.2,
/// docs/research/subagent-chats.md §3.12), so its one-request timeout
/// `tools.subagent_timeout_secs` became the whole-run `subagent_run_timeout_secs`
/// with a default ten times larger, and its per-round reply cap's default rose.
/// A value the user left at the old default is **dropped** (the new default
/// applies on read — `ToolSettings` is `#[serde(default)]`); a value the user
/// changed is carried over under the new name, because a number somebody typed
/// is a decision. Nothing else in the file is touched.
fn settings_to_v2(mut v: Value) -> Result<Value> {
    if let Some(tools) = v.get_mut("tools").and_then(Value::as_object_mut) {
        if let Some(old) = tools.remove("subagent_timeout_secs")
            && old.as_u64() != Some(V1_SUBAGENT_TIMEOUT_SECS)
        {
            tools.insert("subagent_run_timeout_secs".into(), old);
        }
        if tools.get("subagent_max_tokens").and_then(Value::as_u64) == Some(V1_SUBAGENT_MAX_TOKENS)
        {
            tools.remove("subagent_max_tokens");
        }
    }
    v["schema_version"] = Value::from(2);
    Ok(v)
}

const SETTINGS_STEPS: &[Step] = &[Step {
    to: 2,
    summary: "the sub-agent's one-request timeout becomes a whole-run limit",
    apply: settings_to_v2,
}];

/// The registry of artifacts. `settings.json` and the chat files are at 2 (one
/// step each); `profiles.json` is still at 1 with no steps.
pub fn settings_artifact() -> JsonArtifact {
    JsonArtifact {
        name: "settings.json",
        current: SETTINGS_SCHEMA,
        detect: detect_settings,
        steps: SETTINGS_STEPS,
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

const CHAT_STEPS: &[Step] = &[Step {
    to: 2,
    summary: "a transcript is synthesized for every old call_subagent record",
    apply: super::chat_steps::chat_to_v2,
}];

pub fn chat_artifact() -> JsonArtifact {
    JsonArtifact {
        name: "chats/<id>.json",
        current: CHAT_SCHEMA,
        detect: detect_chat,
        steps: CHAT_STEPS,
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
    fn real_registry_versions_and_steps() {
        // Settings and chats took their first real step; profiles are still dormant.
        for a in [settings_artifact(), chat_artifact()] {
            assert_eq!(a.current, 2, "{}", a.name);
            assert_eq!(a.steps.len(), 1, "{}", a.name);
            assert_eq!(a.steps[0].to, 2);
        }
        let profiles = profiles_artifact();
        assert_eq!(profiles.current, 1);
        assert!(profiles.steps.is_empty());
    }

    /// The golden v1 shape: the two sub-agent knobs under the names and
    /// defaults `settings.json` carried before the step, beside fields the step
    /// must leave alone.
    fn v1_settings(timeout: u64, max_tokens: u64) -> Value {
        json!({
            "schema_version": 1,
            "max_tool_rounds": 8,
            "tools": {
                "web_enabled": true,
                "subagent_max_tokens": max_tokens,
                "subagent_timeout_secs": timeout,
                "confirm_dangerous": false
            }
        })
    }

    #[test]
    fn settings_step_drops_old_defaults_so_the_new_ones_apply() {
        let out = settings_artifact()
            .apply_steps(v1_settings(60, 1024), 1)
            .unwrap();
        assert_eq!(out["schema_version"], json!(2));
        let tools = out["tools"].as_object().unwrap();
        assert!(!tools.contains_key("subagent_timeout_secs"));
        assert!(!tools.contains_key("subagent_run_timeout_secs"));
        assert!(!tools.contains_key("subagent_max_tokens"));
        // Untouched neighbours.
        assert_eq!(tools["web_enabled"], json!(true));
        assert_eq!(tools["confirm_dangerous"], json!(false));
        assert_eq!(out["max_tool_rounds"], json!(8));
        // And the migrated value parses into today's config with today's defaults.
        let cfg: crate::shared::config::AppConfig = serde_json::from_value(out).unwrap();
        assert_eq!(
            cfg.tools.subagent_run_timeout_secs,
            crate::shared::config::DEFAULT_SUBAGENT_RUN_TIMEOUT_SECS
        );
        assert_eq!(
            cfg.tools.subagent_max_tokens,
            crate::shared::config::DEFAULT_SUBAGENT_MAX_TOKENS
        );
    }

    #[test]
    fn settings_step_carries_a_changed_value_under_the_new_name() {
        let out = settings_artifact()
            .apply_steps(v1_settings(120, 2048), 1)
            .unwrap();
        let tools = out["tools"].as_object().unwrap();
        assert!(!tools.contains_key("subagent_timeout_secs"));
        assert_eq!(tools["subagent_run_timeout_secs"], json!(120));
        assert_eq!(tools["subagent_max_tokens"], json!(2048));
        let cfg: crate::shared::config::AppConfig = serde_json::from_value(out).unwrap();
        assert_eq!(cfg.tools.subagent_run_timeout_secs, 120);
        assert_eq!(cfg.tools.subagent_max_tokens, 2048);
    }

    #[test]
    fn settings_step_tolerates_a_file_without_the_tools_section() {
        let out = settings_artifact()
            .apply_steps(json!({"schema_version": 1}), 1)
            .unwrap();
        assert_eq!(out["schema_version"], json!(2));
    }
}
