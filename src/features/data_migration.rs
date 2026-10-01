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
//! `settings.json`/`profiles.json` — **refuse** (F11), and so is one at the current version
//! that does not parse into its typed structure (docs/research/settings-typed-parse.md);
//! a corrupt `chats/<id>.json` — skip
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
        &RefusalKeys {
            corrupt: "migrate.err.settings_corrupt",
            invalid: "migrate.err.settings_invalid",
        },
    )? {
        plan.push(p);
    }
    if let Some(p) = assess_file(
        &paths.profiles_file(),
        profiles,
        Kind::Profiles,
        loc,
        &RefusalKeys {
            corrupt: "migrate.err.profiles_corrupt",
            invalid: "migrate.err.profiles_invalid",
        },
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

/// The backup password stored in the settings, read straight from the raw JSON
/// ([`backup::backup_settings`]).
///
/// Runs before storage opens and before any migration, so the typed `AppConfig`
/// isn't available. Every failure (no file, corrupt JSON, no entry, a foreign
/// machine's entry) reads as "no password", which is the pre-existing behaviour.
fn stored_backup_password(paths: &Paths) -> Option<String> {
    backup::backup_settings(paths).ok()?.stored_password
}

/// The two refusals a file can earn by what it holds: it is not JSON
/// (`corrupt`), or it is JSON at the current version and not the structure the
/// app reads it into (`invalid`).
struct RefusalKeys {
    corrupt: &'static str,
    invalid: &'static str,
}

/// Assesses one file: `Ok(None)` — no file, or already current; `Ok(Some)` — into the plan;
/// `Err` — corrupt, invalid (both via `keys`) or a downgrade (data newer than the app).
///
/// A file at the current version is parsed into its **typed** structure here,
/// not only as a `Value`: it is what the app is about to load, and a value the
/// structure refuses — a misspelt enum value, a newer version's one, a string
/// where a number belongs — fails the whole file. Loaded with a fallback to the
/// defaults, that cost the user every setting and then the file itself
/// (docs/research/settings-typed-parse.md); so the refusal is made here, before
/// anything is opened, in serde's words — they name the value, what would have
/// been accepted, and the line.
fn assess_file(
    path: &Path,
    art: &JsonArtifact,
    kind: Kind,
    loc: &Locale,
    keys: &RefusalKeys,
) -> Result<Option<Planned>> {
    let shown = path.display().to_string();
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => bail!("{}", loc.tf(keys.corrupt, &[("path", &shown)])),
    };
    let Ok(v) = serde_json::from_slice::<Value>(&bytes) else {
        bail!("{}", loc.tf(keys.corrupt, &[("path", &shown)]))
    };
    match art.assess(&v) {
        Assessment::UpToDate => {
            // From the bytes, not from `v`: only there does the error carry a position.
            control_parse(kind, &mut serde_json::Deserializer::from_slice(&bytes)).map_err(
                |e| {
                    tracing::error!(file = %shown, error = %e,
                        "startup refused: the file holds a value this version cannot read");
                    anyhow!(
                        "{}",
                        loc.tf(keys.invalid, &[("path", &shown), ("err", &e.to_string())])
                    )
                },
            )?;
            Ok(None)
        }
        Assessment::Migrate { from } => Ok(Some(Planned {
            path: path.to_path_buf(),
            kind,
            from,
            value: v,
        })),
        Assessment::Downgrade { from } => {
            bail!(downgrade_msg(loc, art.name, from, art.current))
        }
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

/// Validates via typed parsing (without writing). Generic over the source: a
/// migrated `Value`, or the bytes of a file that needed no migration.
fn control_parse<'de, D>(kind: Kind, de: D) -> Result<(), D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    match kind {
        Kind::Settings => AppConfig::deserialize(de).map(drop),
        Kind::Profiles => Vec::<Profile>::deserialize(de).map(drop),
        Kind::Chat => Chat::deserialize(de).map(drop),
    }
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

    /// How many pre-migration backups the root holds.
    fn pre_migrate_backups(paths: &Paths) -> usize {
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
        let backups = || pre_migrate_backups(&paths);
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

    /// A root as 0.13.0 left it, through the real registry: the golden v4
    /// settings and a v4 chat are backed up once and stamped 5 — the first
    /// schema in which an effort may read `max` — a profile is left as it is,
    /// and the next start finds nothing to do.
    #[test]
    fn run_stamps_a_root_of_the_version_before_the_max_effort() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        write(
            &paths.settings_file(),
            include_str!("../shared/storage/fixtures/settings_v4.json"),
        );
        let profiles = serde_json::to_string(&vec![Profile::new("A", "s")]).unwrap();
        write(&paths.profiles_file(), &profiles);
        let id = "7e1d4a90-3b52-4c68-9f0e-2a6b8c4d1e35";
        let chat_before = include_str!("../shared/storage/fixtures/chat_v4_effort_xhigh.json");
        write(&paths.chat_file(id), chat_before);

        run(&paths, ru()).unwrap();

        let read = |path: PathBuf| -> Value {
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
        };
        let settings = read(paths.settings_file());
        assert_eq!(settings["schema_version"], json!(5));
        assert_eq!(
            settings["default_sampling"]["reasoning_effort"],
            json!("xhigh")
        );
        let mut chat = read(paths.chat_file(id));
        assert_eq!(chat["v"], json!(5));
        chat["v"] = json!(4);
        let before: Value = serde_json::from_str(chat_before).unwrap();
        assert_eq!(chat, before, "the chat holds what it held");
        assert_eq!(fs::read_to_string(paths.profiles_file()).unwrap(), profiles);
        assert_eq!(pre_migrate_backups(&paths), 1, "one pre-migrate backup");

        run(&paths, ru()).unwrap();
        assert_eq!(pre_migrate_backups(&paths), 1);
    }

    /// `profiles.json` has no step for the `max` effort (`schema::settings_to_v5`
    /// says why): a version that does not know the value reads `settings.json`
    /// first and refuses there, as a newer version's data. Pinned here as the
    /// order of the reads — the refusal names the settings and their version,
    /// and is made although the profiles hold what that version cannot parse.
    #[test]
    fn a_reader_before_the_max_effort_refuses_at_the_settings_not_at_a_profile() {
        let mut profile = Profile::new("A", "s");
        profile.default_sampling = Some(crate::entities::sampling::SamplingConfig {
            reasoning_effort: Some(crate::entities::sampling::ReasoningEffort::Max),
            ..Default::default()
        });
        let profiles = serde_json::to_string(&vec![profile]).unwrap();
        assert!(
            profiles.contains(r#""reasoning_effort":"max""#),
            "{profiles}"
        );
        let (_dir, paths) = root_with(&valid_settings_json(), &profiles);

        // The settings reader 0.13.0 ships.
        let before = JsonArtifact {
            name: "settings.json",
            current: 4,
            detect: schema::settings_artifact().detect,
            steps: &[],
        };
        let err = run_with(
            &paths,
            en(),
            &before,
            &schema::profiles_artifact(),
            &schema::chat_artifact(),
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("settings.json"), "{err}");
        assert!(err.contains("newer version"), "{err}");
        assert!(!err.contains("max"), "{err}");
        assert!(nothing_was_written(&paths, &paths.profiles_file()));
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

    fn en() -> &'static Locale {
        locale(Lang::En)
    }

    /// A root holding `settings.json` and `profiles.json` as given, and nothing else.
    fn root_with(settings: &str, profiles: &str) -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        write(&paths.settings_file(), settings);
        write(&paths.profiles_file(), profiles);
        (dir, paths)
    }

    /// Whether anything was written next to `file`: its `.bak`, or a
    /// pre-migration backup.
    fn nothing_was_written(paths: &Paths, file: &Path) -> bool {
        !file.with_extension("bak").exists() && !paths.backups_dir().exists()
    }

    /// JSON at the current schema that `AppConfig` refuses is refused **here**,
    /// naming the file and the value: loaded with a fallback to the defaults it
    /// cost the user every setting, and the next save the file
    /// (docs/research/settings-typed-parse.md). The three ways to hold such a
    /// value — a typo, a newer version's value, a wrong type.
    #[test]
    fn run_refuses_settings_holding_a_value_the_config_cannot_read() {
        for (field, serde_says) in [
            (
                r#""interface": {"theme": "drak"}"#,
                "unknown variant `drak`",
            ),
            (
                r#""engine": {"mode": "bedrock"}"#,
                "unknown variant `bedrock`",
            ),
            (r#""max_tool_rounds": "3""#, "expected u32"),
        ] {
            let raw = format!(
                "{{\n  \"schema_version\": {},\n  \"tools\": {{\"python_enabled\": true}},\n  {field}\n}}",
                schema::SETTINGS_SCHEMA
            );
            let (_dir, paths) = root_with(&raw, "[]");

            let err = run(&paths, en()).unwrap_err().to_string();

            let file = paths.settings_file();
            assert!(err.contains(&file.display().to_string()), "{err}");
            assert!(err.contains(serde_says), "{field}: {err}");
            // Where in the file: the position survives only a parse from bytes.
            assert!(err.contains("at line 4 column"), "{field}: {err}");
            assert_eq!(fs::read_to_string(&file).unwrap(), raw, "{field}");
            assert!(nothing_was_written(&paths, &file), "{field}");
        }
    }

    /// The refusal is a bundle text, not serde's sentence alone.
    #[test]
    fn the_refusal_of_an_unreadable_value_is_localized() {
        let raw = format!(
            r#"{{"schema_version": {}, "interface": {{"theme": "drak"}}}}"#,
            schema::SETTINGS_SCHEMA
        );
        let (_dir, paths) = root_with(&raw, "[]");
        let err = run(&paths, ru()).unwrap_err().to_string();
        assert!(err.contains("не может прочитать"), "{err}");
        assert!(err.contains("`drak`"), "{err}");
    }

    /// The same gate over `profiles.json` (fork F3): a refusal that names the
    /// file, where the orchestrator's failed load ended the session pointing at
    /// the log.
    #[test]
    fn run_refuses_profiles_holding_a_value_a_profile_cannot_read() {
        let mut list = serde_json::to_value(vec![Profile::new("A", "s")]).unwrap();
        list[0]["is_hidden"] = json!("yes");
        let raw = serde_json::to_string_pretty(&list).unwrap();
        let (_dir, paths) = root_with(&valid_settings_json(), &raw);

        let err = run(&paths, en()).unwrap_err().to_string();

        let file = paths.profiles_file();
        assert!(err.contains(&file.display().to_string()), "{err}");
        assert!(err.contains("profiles file"), "{err}");
        assert!(err.contains("expected a boolean"), "{err}");
        assert_eq!(fs::read_to_string(&file).unwrap(), raw);
        assert!(nothing_was_written(&paths, &file));
    }

    /// The control arm: what the gate refuses is decided by the typed parse and
    /// by nothing of its own. A field read leniently (`interface.theme_mode`,
    /// spec §11.6) and a key this version does not have both still start.
    #[test]
    fn a_value_the_config_reads_leniently_passes_the_gate() {
        let raw = format!(
            r#"{{"schema_version": {}, "interface": {{"theme_mode": "sepia"}}, "a_newer_key": 1}}"#,
            schema::SETTINGS_SCHEMA
        );
        let (_dir, paths) = root_with(&raw, "[]");
        run(&paths, en()).unwrap();
        assert_eq!(fs::read_to_string(paths.settings_file()).unwrap(), raw);
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
