//! Orchestrates migrations of saved data at application startup.
//!
//! The pure Value-level framework (version constants, version detection, running the steps) —
//! lives in [`crate::shared::storage::schema`]; here — file I/O, gates (downgrade / a corrupt
//! file), the pre-migration backup, and control-parsing into typed structures. Lives in
//! `features` (not `shared`), because the pre-migration backup is `features::backup`,
//! and `shared` can't depend on `features` (FSD). See
//! [docs/history/release-engineering.md](../../docs/history/release-engineering.md) §3.4 and ADR 0006.
//!
//! Flow (release-engineering.md F9-F11): read each file as `Value` → detect the
//! version → `> current` — **refuse to start** (data newer than the app, F10); a corrupt
//! `settings.json`/`profiles.json` — **refuse** (F11), a corrupt `chats/<id>.json` — skip
//! with a `warn`; `< current` — into the plan. A non-empty plan → **one** backup before any write
//! (a failure → the migration doesn't start) → run the steps + control-parse + an atomic write.
//!
//! `settings.json` is at schema 2 (the sub-agent track's step, `schema::settings_to_v2`);
//! `profiles.json` and the chat files are still at 1. The engine is additionally covered
//! by tests on a synthetic artifact, independently of the real registry.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use serde_json::Value;

use crate::entities::chat::Chat;
use crate::entities::profile::Profile;
use crate::features::backup;
use crate::shared::config::AppConfig;
use crate::shared::i18n::Locale;
use crate::shared::paths::Paths;
use crate::shared::storage::schema::{self, Assessment, JsonArtifact};
use crate::shared::storage::{db, json};

/// Migrates the application's data at startup (the real schema registry). Called from
/// `main.rs` before opening storage (the TUI and the CLI `import`).
pub fn run(paths: &Paths, loc: &Locale) -> Result<()> {
    run_with(
        paths,
        loc,
        &schema::settings_artifact(),
        &schema::profiles_artifact(),
        &schema::chat_artifact(),
    )
}

/// Which typed structure to validate a migrated value with before writing.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Settings,
    Profiles,
    Chat,
}

/// A file requiring migration (`from → art.current`).
struct Planned {
    path: PathBuf,
    kind: Kind,
    from: u32,
    value: Value,
}

/// The core with injected artifacts (DI for tests: a synthetic artifact with a real
/// step checks backup+write independently of the real registry, where every schema = 1).
fn run_with(
    paths: &Paths,
    loc: &Locale,
    settings: &JsonArtifact,
    profiles: &JsonArtifact,
    chat: &JsonArtifact,
) -> Result<()> {
    let mut plan: Vec<Planned> = Vec::new();

    if let Some(p) = assess_file(
        &paths.settings_file(),
        settings,
        Kind::Settings,
        loc,
        "migrate.err.settings_corrupt",
    )? {
        plan.push(p);
    }
    if let Some(p) = assess_file(
        &paths.profiles_file(),
        profiles,
        Kind::Profiles,
        loc,
        "migrate.err.profiles_corrupt",
    )? {
        plan.push(p);
    }

    for path in chat_files(paths)? {
        match json::read_json::<Value>(&path) {
            Ok(None) => {}
            Ok(Some(v)) => match chat.assess(&v) {
                Assessment::UpToDate => {}
                Assessment::Migrate { from } => plan.push(Planned {
                    path,
                    kind: Kind::Chat,
                    from,
                    value: v,
                }),
                Assessment::Downgrade { from } => {
                    bail!(downgrade_msg(
                        loc,
                        &path.display().to_string(),
                        from,
                        chat.current
                    ))
                }
            },
            // A corrupt chat file doesn't crash startup and isn't lost — stays on disk (F11).
            Err(err) => tracing::warn!(file = %path.display(), error = %err,
                "migration: skipped a corrupt chat file"),
        }
    }

    // Coordination with SQLite: a downgrade guard + factoring into the shared pre-migrate
    // moment. The actual DB migration (baseline/steps) is run later by `Db::open`; here we
    // only peek at `user_version` so ONE backup covers both the JSON and the DB (ADR 0006). The
    // DB isn't open yet at this point (quiescent) → the backup archive of its files is consistent, no SQLite backup API needed.
    let db_uv = db::peek_user_version(&paths.data_db())?;
    if db_uv > schema::DB_SCHEMA {
        bail!(downgrade_msg(loc, "data.db", db_uv, schema::DB_SCHEMA));
    }
    let db_needs_migration = db::needs_step_migration(db_uv);

    if plan.is_empty() && !db_needs_migration {
        tracing::debug!("no data migration needed");
        return Ok(());
    }

    // One backup before any write. The sandbox's fs_root isn't included (the config may itself
    // need migrating — reading it for fs_root would be premature; the critical
    // settings/profiles/chats/db data is already captured by the backup). A failure → the migration is cancelled.
    //
    // The stored backup password *is* read, though (docs/history/backup-password.md §4 F4):
    // a setting that says "my backups are encrypted" must not have an exception
    // that quietly writes a plaintext copy of everything. Safe to read ahead of
    // the migration — the secrets list is additive and has never been migrated.
    let backup_path = backup::default_backup_path(paths, "pre-migrate");
    let password = stored_backup_password(paths);
    backup::create_backup(
        paths,
        Some(backup_path.clone()),
        9,
        None,
        password.as_deref(),
        loc,
        // Silent: this runs on startup, before the TUI, where a stream of
        // progress lines would only look like noise before the app appears.
        |_| {},
    )
    .map_err(|e| {
        anyhow!(
            "{}",
            loc.tf("migrate.err.backup_failed", &[("err", &format!("{e:#}"))])
        )
    })?;
    tracing::info!(backup = %backup_path.display(), files = plan.len(),
        "created a backup before migrating data");

    for item in &plan {
        let art = match item.kind {
            Kind::Settings => settings,
            Kind::Profiles => profiles,
            Kind::Chat => chat,
        };
        let migrated = art.apply_steps(item.value.clone(), item.from)?;
        // Control-parse: a migration after which the file doesn't parse is an error; the file
        // isn't overwritten (writing happens only after successful validation).
        control_parse(item.kind, &migrated).map_err(|e| {
            tracing::error!(file = %item.path.display(), error = %e, "migration produced an unparsable result");
            anyhow!(
                "{}",
                loc.tf(
                    "migrate.err.control_parse",
                    &[("file", &item.path.display().to_string())]
                )
            )
        })?;
        json::write_json(&item.path, &migrated)?;
        tracing::info!(file = %item.path.display(), from = item.from, to = art.current,
            "migrated a data file");
    }

    Ok(())
}

/// The backup password stored in the settings, read straight from the raw JSON.
///
/// Runs before storage opens and before any migration, so the typed `AppConfig`
/// isn't available — `api_keys` is pulled out of the `Value` instead. Every
/// failure (no file, corrupt JSON, no entry, a foreign machine's entry) reads as
/// "no password", which is the pre-existing behaviour.
fn stored_backup_password(paths: &Paths) -> Option<String> {
    let value: Value = json::read_json(&paths.settings_file()).ok()??;
    let entries: Vec<crate::shared::secrets::ApiKeyEntry> =
        serde_json::from_value(value.get("api_keys")?.clone()).ok()?;
    crate::shared::secrets::stored_key(&entries, crate::shared::secrets::BACKUP_PASSWORD_KEY)
}

/// Assesses one file: `Ok(None)` — no file, or already current; `Ok(Some)` — into the plan;
/// `Err` — corrupt (via the `corrupt_key`) or a downgrade (data newer than the app).
fn assess_file(
    path: &Path,
    art: &JsonArtifact,
    kind: Kind,
    loc: &Locale,
    corrupt_key: &str,
) -> Result<Option<Planned>> {
    match json::read_json::<Value>(path) {
        Ok(None) => Ok(None),
        Ok(Some(v)) => match art.assess(&v) {
            Assessment::UpToDate => Ok(None),
            Assessment::Migrate { from } => Ok(Some(Planned {
                path: path.to_path_buf(),
                kind,
                from,
                value: v,
            })),
            Assessment::Downgrade { from } => {
                bail!(downgrade_msg(loc, art.name, from, art.current))
            }
        },
        Err(_) => bail!(
            "{}",
            loc.tf(corrupt_key, &[("path", &path.display().to_string())])
        ),
    }
}

/// A localized refusal message: the data was created by a newer app version.
fn downgrade_msg(loc: &Locale, file: &str, found: u32, current: u32) -> String {
    loc.tf(
        "migrate.err.downgrade",
        &[
            ("file", file),
            ("found", &found.to_string()),
            ("current", &current.to_string()),
        ],
    )
}

/// Validates a migrated value via typed parsing (without writing).
fn control_parse(kind: Kind, v: &Value) -> Result<()> {
    match kind {
        Kind::Settings => {
            serde_json::from_value::<AppConfig>(v.clone())?;
        }
        Kind::Profiles => {
            serde_json::from_value::<Vec<Profile>>(v.clone())?;
        }
        Kind::Chat => {
            serde_json::from_value::<Chat>(v.clone())?;
        }
    }
    Ok(())
}

/// All `chats/*.json` (sorted for determinism). No directory → empty.
fn chat_files(paths: &Paths) -> Result<Vec<PathBuf>> {
    let dir = paths.chats_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(OsStr::to_str) == Some("json") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::i18n::{Lang, locale};
    use serde_json::json;

    fn ru() -> &'static Locale {
        locale(Lang::Ru)
    }

    fn write(path: &Path, s: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, s).unwrap();
    }

    fn valid_settings_json() -> String {
        serde_json::to_string(&AppConfig::default()).unwrap()
    }

    fn valid_chat_json() -> String {
        let p = Profile::new("A", "s");
        serde_json::to_string(&Chat::from_profile(&p, "t")).unwrap()
    }

    #[test]
    fn run_is_noop_when_all_current() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        write(&paths.settings_file(), &valid_settings_json());
        write(&paths.profiles_file(), "[]");
        write(
            &paths.chat_file("11111111-1111-1111-1111-111111111111"),
            &valid_chat_json(),
        );

        run(&paths, ru()).unwrap();

        // No pre-migrate backup at all: the plan was empty.
        let backups = paths.backups_dir();
        let has_backup = backups.exists()
            && fs::read_dir(&backups).unwrap().any(|e| {
                e.unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains("pre-migrate")
            });
        assert!(
            !has_backup,
            "a backup shouldn't be created with an empty migration plan"
        );
    }

    /// The first real chat-file step, end to end through the real registry: a
    /// v1 chat with an old `call_subagent` record is backed up, migrated (the
    /// run synthesized, `v = 2` written) and left alone on the next start.
    #[test]
    fn run_migrates_a_v1_chat_with_an_old_subagent_call() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        write(&paths.settings_file(), &valid_settings_json());
        write(&paths.profiles_file(), "[]");
        let id = "4b2c5e1a-7d3f-4e2b-9a1c-0f6e8d7c5b4a";
        write(
            &paths.chat_file(id),
            include_str!("../shared/storage/fixtures/chat_v1_call_subagent.json"),
        );

        run(&paths, ru()).unwrap();

        let after: Chat =
            serde_json::from_str(&fs::read_to_string(paths.chat_file(id)).unwrap()).unwrap();
        assert_eq!(after.v, schema::CHAT_SCHEMA);
        let run_ = after.messages[1].tool_calls[0]
            .subagent
            .as_deref()
            .expect("a synthesized run");
        assert_eq!(run_.final_reply(), Some("Идея X слаба: …"));
        let backups = || {
            fs::read_dir(paths.backups_dir())
                .map(|d| {
                    d.filter(|e| {
                        e.as_ref()
                            .unwrap()
                            .file_name()
                            .to_string_lossy()
                            .contains("pre-migrate")
                    })
                    .count()
                })
                .unwrap_or(0)
        };
        assert_eq!(backups(), 1, "one pre-migrate backup");

        // The next start finds the file current: no step, no second backup.
        run(&paths, ru()).unwrap();
        assert_eq!(backups(), 1);
        let again: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(paths.chat_file(id)).unwrap()).unwrap();
        assert_eq!(
            schema::chat_artifact().assess(&again),
            schema::Assessment::UpToDate
        );
    }

    /// A chat this binary writes carries `v` and reads as current — so a file
    /// saved after the step is never handed to the step again.
    #[test]
    fn a_freshly_saved_chat_is_current() {
        let v: serde_json::Value = serde_json::from_str(&valid_chat_json()).unwrap();
        assert_eq!(v["v"], json!(schema::CHAT_SCHEMA));
        assert_eq!(
            schema::chat_artifact().assess(&v),
            schema::Assessment::UpToDate
        );
    }

    #[test]
    fn run_refuses_downgrade() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        // settings.json from a "newer" app version (schema 999).
        let mut v: Value = serde_json::from_str(&valid_settings_json()).unwrap();
        v["schema_version"] = json!(999);
        let raw = serde_json::to_string(&v).unwrap();
        write(&paths.settings_file(), &raw);

        let err = run(&paths, ru()).unwrap_err().to_string();
        assert!(err.contains("более новой"), "{err}");
        // The data is untouched.
        assert_eq!(fs::read_to_string(paths.settings_file()).unwrap(), raw);
    }

    #[test]
    fn run_refuses_corrupt_settings() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        write(&paths.settings_file(), "{ this is not valid json");
        let err = run(&paths, ru()).unwrap_err().to_string();
        assert!(
            err.contains("настроек") && err.contains("повреждён"),
            "{err}"
        );
    }

    #[test]
    fn run_refuses_db_downgrade() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        // data.db from a "newer" app version (user_version = 999).
        {
            let conn = rusqlite::Connection::open(paths.data_db()).unwrap();
            conn.execute_batch("PRAGMA user_version = 999;").unwrap();
        }
        let err = run(&paths, ru()).unwrap_err().to_string();
        assert!(err.contains("более новой"), "{err}");
    }

    #[test]
    fn run_skips_corrupt_chat_without_failing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        write(&paths.settings_file(), &valid_settings_json());
        write(
            &paths.chat_file("22222222-2222-2222-2222-222222222222"),
            "{ corrupt",
        );
        write(
            &paths.chat_file("33333333-3333-3333-3333-333333333333"),
            &valid_chat_json(),
        );
        // A corrupt chat is skipped with a warn, startup doesn't fail, no migration happens.
        run(&paths, ru()).unwrap();
    }

    // A synthetic settings artifact at "current = 2" with a 1→2 step: checks the full
    // backup+write+control-parse path on real I/O (the real registry is all v1).
    fn to_v2(mut v: Value) -> Result<Value> {
        v["schema_version"] = json!(2);
        Ok(v)
    }
    const SYNTH_STEPS: &[schema::Step] = &[schema::Step {
        to: 2,
        summary: "test",
        apply: to_v2,
    }];

    #[test]
    fn run_with_migrates_file_and_creates_backup() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        // v1 settings: a valid AppConfig, pinned at `schema_version = 1` so the
        // synthetic 1→2 artifact below has something to migrate whatever the real
        // settings schema is today.
        let mut v1: Value = serde_json::from_str(&valid_settings_json()).unwrap();
        v1["schema_version"] = json!(1);
        write(&paths.settings_file(), &v1.to_string());

        let synth_settings = JsonArtifact {
            name: "settings.json",
            current: 2,
            detect: schema::settings_artifact().detect,
            steps: SYNTH_STEPS,
        };
        run_with(
            &paths,
            ru(),
            &synth_settings,
            &schema::profiles_artifact(),
            &schema::chat_artifact(),
        )
        .unwrap();

        // The file was migrated to v2 and remains a valid AppConfig.
        let after: Value =
            serde_json::from_str(&fs::read_to_string(paths.settings_file()).unwrap()).unwrap();
        assert_eq!(after["schema_version"], json!(2));
        assert!(serde_json::from_value::<AppConfig>(after).is_ok());
        // Exactly one pre-migrate backup was created.
        let backup_made = fs::read_dir(paths.backups_dir()).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .contains("pre-migrate")
        });
        assert!(backup_made, "expected a pre-migrate backup");
    }
}
