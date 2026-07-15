//! Оркестрация миграций сохраняемых данных при старте приложения.
//!
//! Чистый Value-уровневый каркас (константы версий, определение версии, прогон шагов) —
//! в [`crate::shared::storage::schema`]; здесь — файловый I/O, гейты (downgrade / битый
//! файл), pre-migration бэкап и control-parse в типизированные структуры. Живёт в
//! `features` (не в `shared`), потому что pre-migration бэкап — это `features::backup`,
//! а `shared` не может зависеть от `features` (FSD). См.
//! [docs/history/release-engineering.md](../../docs/history/release-engineering.md) §3.4 и ADR 0006.
//!
//! Поток (release-engineering.md Ф9–Ф11): прочитать каждый файл как `Value` → определить
//! версию → `> current` — **отказ запуска** (данные новее приложения, Ф10); битый
//! `settings.json`/`profiles.json` — **отказ** (Ф11), битый `chats/<id>.json` — пропуск
//! с `warn`; `< current` — в план. План непуст → **один** бэкап перед любой записью
//! (не удался → миграция не начинается) → прогон шагов + control-parse + атомарная запись.
//!
//! Сегодня все схемы = 1, поэтому план всегда пуст: `run` фактически лишь валидирует
//! (гейты downgrade/битости), а движок миграций покрыт тестами на синтетическом артефакте.

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

/// Мигрирует данные приложения при старте (реальный реестр схем). Вызывается из
/// `main.rs` перед открытием хранилища (TUI и CLI `import`).
pub fn run(paths: &Paths, loc: &Locale) -> Result<()> {
    run_with(
        paths,
        loc,
        &schema::settings_artifact(),
        &schema::profiles_artifact(),
        &schema::chat_artifact(),
    )
}

/// Какой типизированной структурой валидировать мигрированное значение перед записью.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Settings,
    Profiles,
    Chat,
}

/// Файл, требующий миграции (`from → art.current`).
struct Planned {
    path: PathBuf,
    kind: Kind,
    from: u32,
    value: Value,
}

/// Ядро с внедрёнными артефактами (DI для тестов: синтетический артефакт с реальным
/// шагом проверяет бэкап+запись независимо от реального реестра, где все схемы = 1).
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
            // Битый файл чата не валит запуск и не теряется — остаётся на диске (Ф11).
            Err(err) => tracing::warn!(file = %path.display(), error = %err,
                "миграция: пропущен повреждённый файл чата"),
        }
    }

    // Координация с SQLite: downgrade-guard + учёт в общем pre-migrate моменте. Саму
    // миграцию БД (baseline/шаги) выполняет позже `Db::open`; здесь лишь заглядываем в
    // `user_version`, чтобы ОДИН бэкап покрыл и JSON, и БД (ADR 0006). БД в этот момент
    // ещё не открыта (quiescent) → бэкап-архив её файлов согласован без SQLite backup API.
    let db_uv = db::peek_user_version(&paths.data_db())?;
    if db_uv > schema::DB_SCHEMA {
        bail!(downgrade_msg(loc, "data.db", db_uv, schema::DB_SCHEMA));
    }
    let db_needs_migration = db::needs_step_migration(db_uv);

    if plan.is_empty() && !db_needs_migration {
        tracing::debug!("миграция данных не требуется");
        return Ok(());
    }

    // Один бэкап перед любой записью. fs_root песочницы не включаем (конфиг может сам
    // требовать миграции — читать его для fs_root преждевременно; критичные данные
    // settings/profiles/chats/db бэкап и так захватывает). Сбой → миграция отменяется.
    let backup_path = backup::default_backup_path(paths, "pre-migrate");
    backup::create_backup(paths, Some(backup_path.clone()), 9, None, loc).map_err(|e| {
        anyhow!(
            "{}",
            loc.tf("migrate.err.backup_failed", &[("err", &format!("{e:#}"))])
        )
    })?;
    tracing::info!(backup = %backup_path.display(), files = plan.len(),
        "создан бэкап перед миграцией данных");

    for item in &plan {
        let art = match item.kind {
            Kind::Settings => settings,
            Kind::Profiles => profiles,
            Kind::Chat => chat,
        };
        let migrated = art.apply_steps(item.value.clone(), item.from)?;
        // Control-parse: миграция, после которой файл не парсится, — ошибка; файл не
        // перезаписываем (запись идёт только после успешной валидации).
        control_parse(item.kind, &migrated).map_err(|e| {
            tracing::error!(file = %item.path.display(), error = %e, "миграция дала непарсимый результат");
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
            "мигрирован файл данных");
    }

    Ok(())
}

/// Оценивает один файл: `Ok(None)` — нет файла или актуален; `Ok(Some)` — в план;
/// `Err` — битый (по ключу `corrupt_key`) или downgrade (данные новее приложения).
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

/// Локализованное сообщение об отказе: данные созданы более новой версией приложения.
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

/// Валидирует мигрированное значение типизированным парсом (без записи).
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

/// Все `chats/*.json` (отсортированы для детерминизма). Нет каталога → пусто.
fn chat_files(paths: &Paths) -> Result<Vec<PathBuf>> {
    let dir = paths.chats_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
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

        // Ни одного pre-migrate бэкапа: план был пуст.
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
            "бэкап не должен создаваться без плана миграции"
        );
    }

    #[test]
    fn run_refuses_downgrade() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        // settings.json из «более новой» версии приложения (схема 999).
        let mut v: Value = serde_json::from_str(&valid_settings_json()).unwrap();
        v["schema_version"] = json!(999);
        let raw = serde_json::to_string(&v).unwrap();
        write(&paths.settings_file(), &raw);

        let err = run(&paths, ru()).unwrap_err().to_string();
        assert!(err.contains("более новой"), "{err}");
        // Данные не тронуты.
        assert_eq!(fs::read_to_string(paths.settings_file()).unwrap(), raw);
    }

    #[test]
    fn run_refuses_corrupt_settings() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        write(&paths.settings_file(), "{ это не валидный json");
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
        // data.db из «более новой» версии приложения (user_version = 999).
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
            "{ битый",
        );
        write(
            &paths.chat_file("33333333-3333-3333-3333-333333333333"),
            &valid_chat_json(),
        );
        // Битый чат пропущен с warn, запуск не падает, миграции нет.
        run(&paths, ru()).unwrap();
    }

    // Синтетический артефакт настроек «current = 2» со ступенью 1→2: проверяет полный
    // путь бэкап+запись+control-parse на реальном I/O (реальный реестр — всё v1).
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
        // v1 settings (валидный AppConfig, schema_version=1).
        write(&paths.settings_file(), &valid_settings_json());

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

        // Файл мигрирован до v2 и остаётся валидным AppConfig.
        let after: Value =
            serde_json::from_str(&fs::read_to_string(paths.settings_file()).unwrap()).unwrap();
        assert_eq!(after["schema_version"], json!(2));
        assert!(serde_json::from_value::<AppConfig>(after).is_ok());
        // Создан ровно pre-migrate бэкап.
        let backup_made = fs::read_dir(paths.backups_dir()).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .contains("pre-migrate")
        });
        assert!(backup_made, "ожидался pre-migrate бэкап");
    }
}
