//! Import of the ecosystem's `mcpServers` JSON (Claude Desktop and every client
//! that copied its shape) into `config.mcp.servers`.
//!
//! Pure parsing and planning; applying the plan — storing the secrets and
//! writing the config — is the orchestrator's (`app/orchestrator/mcp.rs`), which
//! is the sole writer of `settings.json` and the only layer allowed to hold the
//! literal secrets such a file carries. See docs/history/mcp-server-editor.md §9
//! (forks S5–S7).
//!
//! Shape of the input:
//!
//! ```jsonc
//! { "mcpServers": {
//!     "filesystem": { "command": "npx",
//!                     "args": ["-y", "@modelcontextprotocol/server-filesystem", "D:/work"],
//!                     "env": { "GITHUB_TOKEN": "ghp_live_token" } },
//!     "remote":     { "type": "sse", "url": "https://example/mcp" }   // skipped: not stdio
//! } }
//! ```

use std::collections::BTreeMap;

use anyhow::{Context, Result};

use crate::shared::config::McpServerConfig;
use crate::shared::i18n::Locale;
use crate::shared::mcp::valid_env_name;

/// One server the plan would add, plus the literal `env` values that have to
/// become machine-bound secrets (S5: the map has no slot for a plaintext value,
/// and writing one would be exactly what ADR 0007 R8 exists to prevent).
#[derive(Debug, PartialEq)]
pub struct ImportedServer {
    pub cfg: McpServerConfig,
    /// `(child variable, literal value)` — stored encrypted, never written to
    /// `settings.json`.
    pub secrets: Vec<(String, String)>,
}

/// What an import would do. Nothing is applied until the caller says so.
#[derive(Debug, Default, PartialEq)]
pub struct ImportPlan {
    pub servers: Vec<ImportedServer>,
    /// Names present in the file whose id already exists here — skipped rather
    /// than overwritten, so a re-import is safe and never destroys a
    /// hand-tuned server (S7).
    pub skipped_existing: Vec<String>,
    /// Entries that are not stdio (`"type": "sse"/"http"`, or a `url` and no
    /// `command`) — reported, not silently dropped: HTTP transport is groundwork.
    pub skipped_unsupported: Vec<String>,
}

impl ImportPlan {
    /// How many secrets applying the plan would store.
    pub fn secret_count(&self) -> usize {
        self.servers.iter().map(|s| s.secrets.len()).sum()
    }
}

/// Parses an `mcpServers` JSON and plans what to add, given the ids that exist
/// already. Tolerant of unknown fields (clients keep adding their own), strict
/// about the shape — an unreadable file is an error, not a silent no-op.
pub fn plan_import(json: &str, existing_ids: &[String], loc: &Locale) -> Result<ImportPlan> {
    let root: serde_json::Value = serde_json::from_str(json.trim_start_matches('\u{feff}'))
        .with_context(|| loc.t("mcp.import.err.bad_json").to_string())?;
    // `mcpServers` is the Claude Desktop key; VS Code's `mcp.json` calls the same
    // map `servers`.
    let map = root
        .get("mcpServers")
        .or_else(|| root.get("servers"))
        .and_then(|v| v.as_object())
        .with_context(|| loc.t("mcp.import.err.no_servers").to_string())?;

    let mut plan = ImportPlan::default();
    let mut taken: Vec<String> = existing_ids.to_vec();
    for (name, entry) in map {
        let command = entry
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if command.is_empty() {
            plan.skipped_unsupported.push(name.clone());
            continue;
        }
        // An id that already exists here is skipped rather than renamed: a
        // re-import must be a no-op, not a second copy of the same server. The
        // uniquifier below is for collisions *within* the file — two free-form
        // names can sanitize to the same slug.
        let base = sanitize_id(name);
        if existing_ids.contains(&base) {
            plan.skipped_existing.push(name.clone());
            continue;
        }
        let id = unique_id(&base, &taken);
        let args = entry
            .get("args")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let (env, secrets) = plan_env(entry);
        taken.push(id.clone());
        plan.servers.push(ImportedServer {
            cfg: McpServerConfig {
                id,
                command,
                args,
                env,
                // Nothing is spawned until the user says so: an imported command
                // may not even exist on this machine (S7, and F5 for hand-created
                // servers).
                enabled: false,
                ..Default::default()
            },
            secrets,
        });
    }
    Ok(plan)
}

/// Plans one entry's `env` map: every literal value becomes a stored secret
/// and the variable is declared with an empty source (S5). A variable whose
/// name we cannot represent is dropped: the flat editor row and the secret's
/// storage name both need `[A-Za-z0-9_]`.
fn plan_env(entry: &serde_json::Value) -> (BTreeMap<String, String>, Vec<(String, String)>) {
    let mut env = BTreeMap::new();
    let mut secrets = Vec::new();
    if let Some(vars) = entry.get("env").and_then(|v| v.as_object()) {
        for (var, value) in vars {
            if !valid_env_name(var) {
                continue;
            }
            env.insert(var.clone(), String::new());
            let value = value.as_str().unwrap_or("").to_string();
            if !value.is_empty() {
                secrets.push((var.clone(), value));
            }
        }
    }
    (env, secrets)
}

/// A one-line localized summary of what an import did (axis B — a human reads it).
pub fn summary(plan: &ImportPlan, loc: &Locale) -> String {
    loc.tf(
        "mcp.import.done",
        &[
            ("n", &plan.servers.len().to_string()),
            (
                "skipped",
                &(plan.skipped_existing.len() + plan.skipped_unsupported.len()).to_string(),
            ),
            ("secrets", &plan.secret_count().to_string()),
        ],
    )
}

/// The ecosystem's keys are free-form (`My_Server`, `github tools`); ours are a
/// slug and part of every tool name (`mcp__<id>__<tool>`).
fn sanitize_id(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    let out: String = out.chars().take(32).collect();
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "server".to_string()
    } else {
        out
    }
}

/// Makes a sanitized id unique against the ids already spoken for. Two different
/// source names can sanitize to the same slug (`My Server` / `my-server`), so
/// this runs even when nothing in the file collided.
fn unique_id(base: &str, taken: &[String]) -> String {
    if !taken.iter().any(|t| t == base) {
        return base.to_string();
    }
    (2..)
        .map(|n| {
            let suffix = format!("-{n}");
            let head: String = base.chars().take(32 - suffix.len()).collect();
            format!("{head}{suffix}")
        })
        .find(|c| !taken.contains(c))
        .unwrap_or_else(|| base.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};

    fn ru() -> &'static Locale {
        locale(Lang::Ru)
    }

    const SAMPLE: &str = r#"{
      "mcpServers": {
        "My Server": {
          "command": "npx",
          "args": ["-y", "@modelcontextprotocol/server-filesystem", "D:/work"],
          "env": { "GITHUB_TOKEN": "ghp_live_secret", "EMPTY": "", "bad-name": "x" },
          "unknownFieldFromSomeOtherClient": true
        },
        "existing": { "command": "whatever" },
        "remote": { "type": "sse", "url": "https://example/mcp" }
      }
    }"#;

    #[test]
    fn plans_servers_secrets_and_skips() {
        let plan = plan_import(SAMPLE, &["existing".to_string()], ru()).unwrap();
        assert_eq!(plan.servers.len(), 1);
        let s = &plan.servers[0];
        assert_eq!(
            s.cfg.id, "my-server",
            "the free-form key is sanitized to a slug"
        );
        assert_eq!(s.cfg.command, "npx");
        assert_eq!(s.cfg.args.len(), 3);
        assert!(!s.cfg.enabled, "imported servers never spawn on arrival");
        // The literal value becomes a secret; the variable is declared with no
        // OS source, and an unrepresentable name is dropped.
        assert_eq!(
            s.secrets,
            vec![("GITHUB_TOKEN".to_string(), "ghp_live_secret".to_string())]
        );
        assert_eq!(
            s.cfg.env.keys().collect::<Vec<_>>(),
            vec!["EMPTY", "GITHUB_TOKEN"]
        );
        assert!(s.cfg.env.values().all(String::is_empty));
        assert_eq!(plan.skipped_existing, vec!["existing"]);
        assert_eq!(plan.skipped_unsupported, vec!["remote"]);
        assert_eq!(plan.secret_count(), 1);
    }

    #[test]
    fn reimport_adds_nothing() {
        let first = plan_import(SAMPLE, &[], ru()).unwrap();
        let ids: Vec<String> = first.servers.iter().map(|s| s.cfg.id.clone()).collect();
        let again = plan_import(SAMPLE, &ids, ru()).unwrap();
        assert!(again.servers.is_empty(), "a re-import is a no-op");
        assert_eq!(again.skipped_existing.len(), 2);
    }

    #[test]
    fn vs_code_servers_key_is_accepted() {
        let plan = plan_import(r#"{"servers":{"fs":{"command":"x"}}}"#, &[], ru()).unwrap();
        assert_eq!(plan.servers.len(), 1);
    }

    #[test]
    fn bad_input_is_an_error_not_an_empty_plan() {
        assert!(plan_import("not json", &[], ru()).is_err());
        assert!(plan_import(r#"{"other": 1}"#, &[], ru()).is_err());
    }

    #[test]
    fn id_sanitization_and_uniqueness() {
        assert_eq!(sanitize_id("My_Server"), "my-server");
        assert_eq!(sanitize_id("  ...  "), "server");
        assert_eq!(sanitize_id(&"a".repeat(40)).len(), 32);
        assert_eq!(unique_id("fs", &["fs".into()]), "fs-2");
        assert_eq!(unique_id("fs", &["fs".into(), "fs-2".into()]), "fs-3");
        // Two source names that sanitize to the same slug still get distinct ids.
        let plan = plan_import(
            r#"{"mcpServers":{"My Server":{"command":"a"},"my_server":{"command":"b"}}}"#,
            &[],
            ru(),
        )
        .unwrap();
        assert_eq!(plan.servers.len(), 2);
        assert_ne!(plan.servers[0].cfg.id, plan.servers[1].cfg.id);
    }
}
