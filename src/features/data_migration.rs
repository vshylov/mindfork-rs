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
//! a corrupt `chats/<id>.json`, or one that holds no chat before its steps or after
//! them (`holds_no_chat`) — skip with a `warn`; `< current` — into the plan. A non-empty
//! plan → **one** backup before any write (a failure → the migration doesn't start) → run
//! the steps + control-parse + an atomic write.
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
                Assessment::Migrate { from } => match holds_no_chat(&v, chat, from) {
                    // Skipped and left as it is, as a file that is not JSON is (F11).
                    Some(why) => tracing::warn!(file = %path.display(), reason = %why,
                        "migration: skipped a chat file that holds no chat"),
                    None => plan.push(Planned {
                        path,
                        kind: Kind::Chat,
                        from,
                        value: v,
                    }),
                },
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

/// Why a chat file at an older schema stays out of the plan, or `None` when it
/// holds a chat to migrate.
///
/// A file in `chats/` holds a chat when it parses as one — before its steps,
/// or after them, which is what the input of a step that changes the shape
/// looks like. A file that parses neither way (`{}`, an array, a note saved
/// there by hand) is no chat: it is skipped and left on disk, as a file that is
/// not JSON is, and the start goes on (spec §12.2). It used to be read as
/// schema 1 — it has no `v` — stepped, and refused by the control parse as a
/// migration bug, on every start.
///
/// The control parse keeps the refusal it was written for: a file that parses
/// as a chat before its steps and not after them is a step's fault, and
/// `run_with` refuses it when it applies the steps. Every chat an older
/// release wrote parses before its steps today (a test holds the fixtures to
/// it); a future breaking step whose input does not would leave a file it
/// broke skipped rather than refused — on disk, unchanged, either way.
fn holds_no_chat(v: &Value, art: &JsonArtifact, from: u32) -> Option<String> {
    // A chat is an object, and the steps index their value as one: on an
    // array or a string that panics.
    if !v.is_object() {
        return Some("not a JSON object".into());
    }
    // A chat as it stands goes into the plan; a step that breaks it is refused there.
    let Err(before) = control_parse(Kind::Chat, v) else {
        return None;
    };
    let parses_after = art
        .apply_steps(v.clone(), from)
        .is_ok_and(|migrated| control_parse(Kind::Chat, &migrated).is_ok());
    (!parses_after).then(|| before.to_string())
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
        // Stamped 5, and on to wherever the chain ends now.
        assert_eq!(settings["schema_version"], json!(schema::SETTINGS_SCHEMA));
        assert_eq!(
            settings["default_sampling"]["reasoning_effort"],
            json!("xhigh")
        );
        let mut chat = read(paths.chat_file(id));
        // Stamped 5, and on to wherever the chain ends now.
        assert_eq!(chat["v"], json!(schema::CHAT_SCHEMA));
        chat["v"] = json!(4);
        let before: Value = serde_json::from_str(chat_before).unwrap();
        assert_eq!(chat, before, "the chat holds what it held");
        assert_eq!(fs::read_to_string(paths.profiles_file()).unwrap(), profiles);
        assert_eq!(pre_migrate_backups(&paths), 1, "one pre-migrate backup");

        run(&paths, ru()).unwrap();
        assert_eq!(pre_migrate_backups(&paths), 1);
    }

    /// A root whose settings still hold the self-model cap of 0.9.4, through
    /// the real registry: backed up once, the cap raised to 4000 and the file
    /// stamped 6, read by today's config as 4000; the next start finds nothing
    /// to do.
    #[test]
    fn run_raises_a_self_model_cap_left_at_the_old_default() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        write(
            &paths.settings_file(),
            include_str!("../shared/storage/fixtures/settings_v5.json"),
        );
        write(&paths.profiles_file(), "[]");

        run(&paths, ru()).unwrap();

        let settings: Value =
            serde_json::from_str(&fs::read_to_string(paths.settings_file()).unwrap()).unwrap();
        assert_eq!(settings["schema_version"], json!(schema::SETTINGS_SCHEMA));
        assert_eq!(settings["self_model"]["prompt_cap"], json!(4000));
        let cfg: AppConfig = serde_json::from_value(settings).unwrap();
        assert_eq!(cfg.self_model.prompt_cap, 4000);
        assert_eq!(pre_migrate_backups(&paths), 1, "one pre-migrate backup");

        run(&paths, ru()).unwrap();
        assert_eq!(pre_migrate_backups(&paths), 1);
    }

    /// A root as 0.20.0 left it for the setup its manual names — DeepSeek's API
    /// in the `external` mode — through the real registry: the golden v6
    /// settings and a v5 chat are backed up once and stamped 7 and 6, the first
    /// schemas in which a mode may read `deepseek`; nothing they hold moves, and
    /// the next start finds nothing to do.
    #[test]
    fn run_stamps_a_root_of_the_version_before_the_deepseek_mode() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        let settings_before = include_str!("../shared/storage/fixtures/settings_v6.json");
        write(&paths.settings_file(), settings_before);
        write(&paths.profiles_file(), "[]");
        let id = "5c0ffee0-4d1e-4b2a-9e3f-6a7b8c9d0e1f";
        let chat_before = include_str!("../shared/storage/fixtures/chat_v5_external_deepseek.json");
        write(&paths.chat_file(id), chat_before);

        run(&paths, ru()).unwrap();

        let read = |path: PathBuf| -> Value {
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
        };
        let mut settings = read(paths.settings_file());
        assert_eq!(settings["schema_version"], json!(7));
        settings["schema_version"] = json!(6);
        let before: Value = serde_json::from_str(settings_before).unwrap();
        assert_eq!(settings, before, "the settings hold what they held");
        let mut chat = read(paths.chat_file(id));
        // Stamped 6, and on to wherever the chain ends now.
        assert_eq!(chat["v"], json!(schema::CHAT_SCHEMA));
        chat["v"] = json!(5);
        let before: Value = serde_json::from_str(chat_before).unwrap();
        assert_eq!(chat, before, "the chat holds what it held");
        assert_eq!(pre_migrate_backups(&paths), 1, "one pre-migrate backup");

        run(&paths, ru()).unwrap();
        assert_eq!(pre_migrate_backups(&paths), 1);
    }

    /// A root as 0.22.0 left it — settings at 7, a chat at 6 holding a file a
    /// `python_exec` call stored — through the real registry: the chat alone is
    /// backed up and stamped 7, the first schema in which a stored file may be a
    /// generated picture (spec §9.15); the settings need no step, since the
    /// picture slot is a section an older reader skips whole; nothing moves, and
    /// the next start finds nothing to do.
    #[test]
    fn run_stamps_a_root_of_the_version_before_generated_pictures() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        let mut settings_before: Value =
            serde_json::from_str(include_str!("../shared/storage/fixtures/settings_v6.json"))
                .unwrap();
        settings_before["schema_version"] = json!(7);
        write(
            &paths.settings_file(),
            &serde_json::to_string_pretty(&settings_before).unwrap(),
        );
        write(&paths.profiles_file(), "[]");
        let id = "6c0ffee0-4d1e-4b2a-9e3f-6a7b8c9d0e1f";
        let chat_before = include_str!("../shared/storage/fixtures/chat_v6_sandbox_file.json");
        write(&paths.chat_file(id), chat_before);

        run(&paths, ru()).unwrap();

        let read = |path: PathBuf| -> Value {
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
        };
        assert_eq!(
            read(paths.settings_file()),
            settings_before,
            "the settings take no step"
        );
        let mut chat = read(paths.chat_file(id));
        // Stamped 7, and on to wherever the chain ends now.
        assert_eq!(chat["v"], json!(schema::CHAT_SCHEMA));
        chat["v"] = json!(6);
        let before: Value = serde_json::from_str(chat_before).unwrap();
        assert_eq!(chat, before, "the chat holds what it held");
        assert_eq!(pre_migrate_backups(&paths), 1, "one pre-migrate backup");

        run(&paths, ru()).unwrap();
        assert_eq!(pre_migrate_backups(&paths), 1);
    }

    /// A root as 0.23.1 left it — settings at 7, a chat at 7 holding a dialogue
    /// run out in the background and a generated picture — through the real
    /// registry: the chat alone is backed up and stamped 8, the first schema in
    /// which a run may be a story's (docs/history/fabula.md §4, PR 8); the settings
    /// need no step, since the story's switch, timeout and budget are fields an
    /// older reader ignores; nothing moves, and the next start finds nothing to
    /// do. The backup records the chat at 7, what 0.23.1 restores without the
    /// "newer version" warning.
    #[test]
    fn run_stamps_a_root_of_the_version_before_story_runs() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        let mut settings_before: Value =
            serde_json::from_str(include_str!("../shared/storage/fixtures/settings_v6.json"))
                .unwrap();
        settings_before["schema_version"] = json!(7);
        write(
            &paths.settings_file(),
            &serde_json::to_string_pretty(&settings_before).unwrap(),
        );
        write(&paths.profiles_file(), "[]");
        let id = "7d1a0b2c-3e4f-4a5b-8c6d-7e8f9a0b1c2d";
        let chat_before =
            include_str!("../shared/storage/fixtures/chat_v7_background_dialogue.json");
        write(&paths.chat_file(id), chat_before);

        run(&paths, ru()).unwrap();

        let read = |path: PathBuf| -> Value {
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
        };
        assert_eq!(
            read(paths.settings_file()),
            settings_before,
            "the settings take no step"
        );
        let mut chat = read(paths.chat_file(id));
        assert_eq!(chat["v"], json!(8));
        chat["v"] = json!(7);
        let before: Value = serde_json::from_str(chat_before).unwrap();
        assert_eq!(chat, before, "the chat holds what it held");
        assert_eq!(pre_migrate_manifest(&paths).schemas.chat, 7);

        run(&paths, ru()).unwrap();
        assert_eq!(pre_migrate_backups(&paths), 1);
    }

    /// The manifest of the root's one pre-migration backup.
    fn pre_migrate_manifest(paths: &Paths) -> backup::BackupManifest {
        let archives: Vec<PathBuf> = fs::read_dir(paths.backups_dir())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.to_string_lossy().contains("pre-migrate"))
            .collect();
        assert_eq!(archives.len(), 1, "{archives:?}");
        backup::read_manifest(&archives[0])
            .unwrap()
            .expect("a manifest")
    }

    /// A root of 0.22.0 — settings at 7, a bare profiles array, a chat at 6 and
    /// no database — as the files before the chats' step 6 → 7.
    fn root_before_generated_pictures() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        let mut settings: Value =
            serde_json::from_str(include_str!("../shared/storage/fixtures/settings_v6.json"))
                .unwrap();
        settings["schema_version"] = json!(7);
        write(&paths.settings_file(), &settings.to_string());
        write(&paths.profiles_file(), "[]");
        write(
            &paths.chat_file("6c0ffee0-4d1e-4b2a-9e3f-6a7b8c9d0e1f"),
            include_str!("../shared/storage/fixtures/chat_v6_sandbox_file.json"),
        );
        (dir, paths)
    }

    /// The pre-migration backup is the way back to the release before the step,
    /// so its manifest says what that release finds in it — the chats at 6, not
    /// this build's 7 — and 0.22.0 restores it without the "newer version"
    /// warning it printed for 0.23.0's (spec §12.3). A kind with no file is 0.
    #[test]
    fn the_pre_migrate_backup_records_the_schemas_before_the_step() {
        let (_dir, paths) = root_before_generated_pictures();

        run(&paths, ru()).unwrap();

        let m = pre_migrate_manifest(&paths);
        let before_the_step = backup::SchemaVersions {
            settings: 7,
            profiles: 1,
            chat: 6,
            db: 0,
            story: 0,
        };
        assert_eq!(m.schemas, before_the_step);
        assert_eq!(m.app_version, env!("CARGO_PKG_VERSION"), "who wrote it");
    }

    /// One chat already at this build's schema among older ones makes the
    /// manifest say so: the release before the step would refuse that file, and
    /// the warning is due.
    #[test]
    fn the_pre_migrate_backup_records_the_newest_chat() {
        let (_dir, paths) = root_before_generated_pictures();
        write(
            &paths.chat_file("00000000-0000-4000-8000-000000000001"),
            &valid_chat_json(),
        );

        run(&paths, ru()).unwrap();

        assert_eq!(
            pre_migrate_manifest(&paths).schemas.chat,
            schema::CHAT_SCHEMA
        );
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

    /// A chat at the schema before the last step, as 0.23.1 wrote it.
    const CHAT_AT_7: &str =
        include_str!("../shared/storage/fixtures/chat_v7_background_dialogue.json");
    const CHAT_AT_7_ID: &str = "7d1a0b2c-3e4f-4a5b-8c6d-7e8f9a0b1c2d";

    /// A file in `chats/` that is JSON but no chat — `{}` — is read as schema
    /// 1, as is any file without `v`. It used to be stepped and then refused by
    /// the control parse as a migration bug, on every start (found by Fabula's
    /// PR 8 live check, docs/journal/stories.md). It is skipped now and left as
    /// it is, and the chat beside it is migrated.
    #[test]
    fn run_skips_an_empty_object_in_chats_and_migrates_the_rest() {
        let (_dir, paths) = root_with(&valid_settings_json(), "[]");
        let stray = paths.chats_dir().join("notes-to-self.json");
        write(&stray, "{}");
        write(&paths.chat_file(CHAT_AT_7_ID), CHAT_AT_7);

        run(&paths, en()).unwrap();

        assert_eq!(fs::read_to_string(&stray).unwrap(), "{}");
        assert!(!stray.with_extension("bak").exists());
        let chat: Value =
            serde_json::from_str(&fs::read_to_string(paths.chat_file(CHAT_AT_7_ID)).unwrap())
                .unwrap();
        assert_eq!(chat["v"], json!(schema::CHAT_SCHEMA));
        assert_eq!(pre_migrate_backups(&paths), 1);

        // The skipped file is not planned: the next start takes no backup for it.
        run(&paths, en()).unwrap();
        assert_eq!(pre_migrate_backups(&paths), 1);
        assert_eq!(fs::read_to_string(&stray).unwrap(), "{}");
    }

    /// JSON that is not an object holds no chat either, and is skipped before
    /// any step runs: the steps index their value as an object, which panics on
    /// an array or a string. Alone in the root, it plans no backup.
    #[test]
    fn run_skips_json_that_is_not_an_object_in_chats() {
        for raw in ["[]", r#"[{"id": 1}]"#, r#""a note""#, "42", "true", "null"] {
            let (_dir, paths) = root_with(&valid_settings_json(), "[]");
            let stray = paths.chats_dir().join("stray.json");
            write(&stray, raw);

            run(&paths, en()).unwrap();

            assert_eq!(fs::read_to_string(&stray).unwrap(), raw, "{raw}");
            assert!(nothing_was_written(&paths, &stray), "{raw}");
        }
    }

    /// A synthetic chat artifact at `current = 2` over the given steps.
    fn synth_chat(steps: &'static [schema::Step]) -> JsonArtifact {
        JsonArtifact {
            name: "chats/<id>.json",
            current: 2,
            detect: schema::chat_artifact().detect,
            steps,
        }
    }

    /// A broken step: `messages` becomes a string, which no chat parses.
    fn break_messages(mut v: Value) -> Result<Value> {
        v["messages"] = json!("lost");
        v["v"] = json!(2);
        Ok(v)
    }
    const BREAKING_STEPS: &[schema::Step] = &[schema::Step {
        to: 2,
        summary: "test: a broken step",
        apply: break_messages,
    }];

    /// A real chat at schema 1, as text: today's chat with `v` set back.
    fn chat_at_1() -> String {
        let mut chat: Value = serde_json::from_str(&valid_chat_json()).unwrap();
        chat["v"] = json!(1);
        chat.to_string()
    }

    /// The control arm: the refusal stays for what it was meant for. A file
    /// that parses as a chat before its steps and not after them is a step's
    /// fault, and the start is refused naming the file, which is left as it was.
    /// A `{}` beside it does not change that.
    #[test]
    fn run_refuses_a_step_that_breaks_a_real_chat() {
        let (_dir, paths) = root_with(&valid_settings_json(), "[]");
        let raw = chat_at_1();
        let file = paths.chat_file("88888888-8888-4888-8888-888888888888");
        write(&file, &raw);
        write(&paths.chats_dir().join("notes-to-self.json"), "{}");

        let err = run_with(
            &paths,
            en(),
            &schema::settings_artifact(),
            &schema::profiles_artifact(),
            &synth_chat(BREAKING_STEPS),
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("migration bug"), "{err}");
        assert!(err.contains(&file.display().to_string()), "{err}");
        assert_eq!(fs::read_to_string(&file).unwrap(), raw);
        assert!(!file.with_extension("bak").exists());
    }

    /// A step of the shape a breaking change takes — a field renamed.
    fn rename_heading_to_title(mut v: Value) -> Result<Value> {
        if let Some(obj) = v.as_object_mut()
            && let Some(heading) = obj.remove("heading")
        {
            obj.insert("title".into(), heading);
        }
        v["v"] = json!(2);
        Ok(v)
    }
    const RENAMING_STEPS: &[schema::Step] = &[schema::Step {
        to: 2,
        summary: "test: a field renamed",
        apply: rename_heading_to_title,
    }];

    /// A file that parses as a chat only once its steps have run — what the
    /// input of a breaking step looks like — is a chat, and is migrated; a `{}`
    /// beside it is stepped too, still holds no chat, and is skipped.
    #[test]
    fn run_migrates_a_chat_that_parses_only_after_its_steps() {
        let (_dir, paths) = root_with(&valid_settings_json(), "[]");
        let mut chat: Value = serde_json::from_str(&chat_at_1()).unwrap();
        let title = chat.as_object_mut().unwrap().remove("title").unwrap();
        chat["heading"] = title.clone();
        let file = paths.chat_file("99999999-9999-4999-8999-999999999999");
        write(&file, &chat.to_string());
        let stray = paths.chats_dir().join("notes-to-self.json");
        write(&stray, "{}");

        run_with(
            &paths,
            en(),
            &schema::settings_artifact(),
            &schema::profiles_artifact(),
            &synth_chat(RENAMING_STEPS),
        )
        .unwrap();

        let after: Chat = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(after.v, 2);
        assert_eq!(json!(after.title), title);
        assert_eq!(fs::read_to_string(&stray).unwrap(), "{}");
    }

    fn refuse_everything(_: Value) -> Result<Value> {
        bail!("test: the step refuses")
    }
    const FAILING_STEPS: &[schema::Step] = &[schema::Step {
        to: 2,
        summary: "test: a failing step",
        apply: refuse_everything,
    }];

    /// A step that fails is one more reason to skip a file that holds no chat;
    /// on a chat it is a migration bug, and the start is refused as before.
    #[test]
    fn a_failing_step_skips_a_non_chat_and_refuses_a_chat() {
        let (_dir, paths) = root_with(&valid_settings_json(), "[]");
        let stray = paths.chats_dir().join("notes-to-self.json");
        write(&stray, "{}");
        let artifacts = |paths: &Paths| {
            run_with(
                paths,
                en(),
                &schema::settings_artifact(),
                &schema::profiles_artifact(),
                &synth_chat(FAILING_STEPS),
            )
        };

        artifacts(&paths).unwrap();
        assert_eq!(fs::read_to_string(&stray).unwrap(), "{}");
        assert!(nothing_was_written(&paths, &stray));

        let raw = chat_at_1();
        let file = paths.chat_file("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        write(&file, &raw);
        let err = format!("{:#}", artifacts(&paths).unwrap_err());
        assert!(err.contains("the step refuses"), "{err}");
        assert_eq!(fs::read_to_string(&file).unwrap(), raw);
    }

    /// What the line between the two rests on: every chat an older release
    /// wrote parses as today's `Chat` before its steps — each chat step so far
    /// is additive or a stamp — so a step that broke one is refused, not
    /// skipped. A breaking step whose input no longer parses takes its fixture
    /// out of this list, and with it that guarantee for files of that age: a
    /// file the step then breaks is skipped and left on disk instead.
    #[test]
    fn every_older_chat_fixture_parses_as_a_chat_before_its_steps() {
        for (name, raw) in [
            (
                "v1",
                include_str!("../shared/storage/fixtures/chat_v1_call_subagent.json"),
            ),
            (
                "v3",
                include_str!("../shared/storage/fixtures/chat_v3_cut_run_title.json"),
            ),
            (
                "v4",
                include_str!("../shared/storage/fixtures/chat_v4_effort_xhigh.json"),
            ),
            (
                "v5",
                include_str!("../shared/storage/fixtures/chat_v5_external_deepseek.json"),
            ),
            (
                "v6",
                include_str!("../shared/storage/fixtures/chat_v6_sandbox_file.json"),
            ),
            ("v7", CHAT_AT_7),
        ] {
            let v: Value = serde_json::from_str(raw).unwrap();
            assert!(
                matches!(
                    schema::chat_artifact().assess(&v),
                    Assessment::Migrate { .. }
                ),
                "{name}"
            );
            control_parse(Kind::Chat, &v).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
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
