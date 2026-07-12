//! Резервное копирование и восстановление пользовательских данных в zip-архив.
//!
//! Запускается аргументами командной строки (`--backup` / `--restore`) без TUI и
//! завершает процесс (см. `main.rs`). Состав архива (пути в архиве — относительно
//! корня данных):
//! - файлы `settings.json`, `profiles.json`, `data.db` (+ sidecar `-wal`/`-shm`,
//!   если есть), `personal_dictionary.txt`;
//! - каталоги `chats/` и `dictionaries/` (рекурсивно — попадают и их `*.bak`);
//! - все `*.bak` в корне (`settings.bak`, `profiles.bak`);
//! - каталог-«песочница» файловых инструментов (`config.tools.fs_root`) — **только
//!   если** он лежит внутри корня данных.
//!
//! Исключаются `backups/`, `logs/` и файлы установочных умолчаний `defaults.json`/
//! `location.json` (они — про установку, а не пользовательские данные).
//!
//! **Восстановление транзакционно.** Сначала архив валидируется (до любых
//! разрушительных действий). Если в корне уже есть данные — они автоматически
//! сохраняются в `backups/` (pre-restore копия), и лишь затем корень очищается и
//! распаковывается указанный архив. Если распаковка не удалась, а pre-restore копия
//! была создана — выполняется откат к ней. См. spec §12.3.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Local;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::shared::paths::Paths;

/// Файлы верхнего уровня, входящие в резервную копию (отсутствующие пропускаются).
const TOP_FILES: &[&str] = &[
    "settings.json",
    "profiles.json",
    "data.db",
    "data.db-wal",
    "data.db-shm",
    "personal_dictionary.txt",
];

/// Каталоги, входящие в резервную копию целиком (рекурсивно).
const TOP_DIRS: &[&str] = &["chats", "dictionaries"];

/// Запись для упаковки: абсолютный путь источника + имя внутри архива (со `/`).
struct Entry {
    abs: PathBuf,
    name: String,
}

/// Итог восстановления — что фактически произошло (для сообщений пользователю).
pub enum RestoreOutcome {
    /// Архив успешно распакован. `pre_restore` — путь авто-копии прежних данных,
    /// если они были и сохранялись.
    Restored { pre_restore: Option<PathBuf> },
    /// Распаковка не удалась, но прежние данные восстановлены из pre-restore копии.
    RolledBack {
        pre_restore: PathBuf,
        restore_error: anyhow::Error,
    },
    /// Распаковка не удалась и откат тоже (или откатывать было не из чего).
    /// Пользователю нужно вмешаться вручную (`pre_restore` — где лежит копия).
    Failed {
        pre_restore: Option<PathBuf>,
        restore_error: anyhow::Error,
        rollback_error: Option<anyhow::Error>,
    },
}

/// Создаёт резервную копию пользовательских данных.
///
/// `output` — путь к создаваемому архиву (`None` → авто-имя в `backups/`).
/// `level` — степень сжатия `0..=9` (`0` → без сжатия, store). `fs_root` — каталог
/// файловой песочницы из конфига (включается только если лежит внутри корня данных).
/// Возвращает путь к созданному архиву.
pub fn create_backup(
    paths: &Paths,
    output: Option<PathBuf>,
    level: i64,
    fs_root: Option<&Path>,
) -> Result<PathBuf> {
    let entries = gather_entries(paths, fs_root)?;
    let out_path = match output {
        Some(p) => p,
        None => default_backup_path(paths, "mindfork-backup"),
    };
    write_zip(&out_path, &entries, level)
        .with_context(|| format!("создание архива {}", out_path.display()))?;
    Ok(out_path)
}

/// Восстанавливает данные из архива `archive`, заменяя текущие.
///
/// Возвращает `Err` только при ошибке **до** разрушительных действий (нет файла,
/// повреждён/небезопасный архив). После начала замены всегда возвращает
/// `Ok(RestoreOutcome)`, описывающий исход (включая откат). `fs_root` — текущая
/// песочница (очищается, если внутри корня).
pub fn restore_backup(
    paths: &Paths,
    archive: &Path,
    fs_root: Option<&Path>,
) -> Result<RestoreOutcome> {
    // 1. Валидация архива до любых разрушительных действий.
    validate_archive(archive).with_context(|| format!("проверка архива {}", archive.display()))?;

    // 2. Авто-копия прежних данных, если они есть.
    let pre_restore = if has_existing_data(paths) {
        let path = create_backup(
            paths,
            Some(default_backup_path(paths, "pre-restore")),
            9,
            fs_root,
        )
        .context("создание pre-restore копии прежних данных")?;
        Some(path)
    } else {
        None
    };

    // 3. Очистка + распаковка.
    let attempt = (|| -> Result<()> {
        clear_user_data(paths, fs_root)?;
        extract_archive(paths, archive)
    })();

    match attempt {
        Ok(()) => Ok(RestoreOutcome::Restored { pre_restore }),
        Err(restore_error) => match &pre_restore {
            // 4. Откат к только что созданной pre-restore копии.
            Some(backup) => {
                let rollback = (|| -> Result<()> {
                    clear_user_data(paths, fs_root)?;
                    extract_archive(paths, backup)
                })();
                match rollback {
                    Ok(()) => Ok(RestoreOutcome::RolledBack {
                        pre_restore: backup.clone(),
                        restore_error,
                    }),
                    Err(rollback_error) => Ok(RestoreOutcome::Failed {
                        pre_restore: pre_restore.clone(),
                        restore_error,
                        rollback_error: Some(rollback_error),
                    }),
                }
            }
            None => Ok(RestoreOutcome::Failed {
                pre_restore: None,
                restore_error,
                rollback_error: None,
            }),
        },
    }
}

/// Собирает список файлов для упаковки (с дедупликацией по имени в архиве).
fn gather_entries(paths: &Paths, fs_root: Option<&Path>) -> Result<Vec<Entry>> {
    let root = paths.root();
    let mut out: Vec<Entry> = Vec::new();

    for f in TOP_FILES {
        let abs = root.join(f);
        if abs.is_file() {
            out.push(Entry {
                abs,
                name: (*f).to_string(),
            });
        }
    }

    // Все `*.bak` верхнего уровня (settings.bak, profiles.bak и т.п.).
    if let Ok(rd) = fs::read_dir(root) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_file()
                && path.extension().is_some_and(|e| e == "bak")
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
            {
                out.push(Entry {
                    abs: path.clone(),
                    name: name.to_string(),
                });
            }
        }
    }

    for d in TOP_DIRS {
        collect_dir(&root.join(d), d, &mut out)?;
    }

    // Песочница файловых инструментов — только если внутри корня данных.
    if let Some((abs, prefix)) = fs_root_under_root(root, fs_root) {
        collect_dir(&abs, &prefix, &mut out)?;
    }

    // Дедуп по имени в архиве (на случай пересечения fs_root с другими путями).
    let mut seen = HashSet::new();
    out.retain(|e| seen.insert(e.name.clone()));
    Ok(out)
}

/// Рекурсивно собирает файлы каталога `abs` под префиксом имени `prefix` (со `/`).
fn collect_dir(abs: &Path, prefix: &str, out: &mut Vec<Entry>) -> Result<()> {
    if !abs.is_dir() {
        return Ok(());
    }
    let rd = fs::read_dir(abs).with_context(|| format!("чтение каталога {}", abs.display()))?;
    for entry in rd {
        let entry = entry?;
        let ft = entry.file_type()?;
        let child_abs = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue; // не-UTF-8 имя пропускаем
        };
        let child_name = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if ft.is_dir() {
            collect_dir(&child_abs, &child_name, out)?;
        } else if ft.is_file() {
            out.push(Entry {
                abs: child_abs,
                name: child_name,
            });
        }
    }
    Ok(())
}

/// Записывает архив со списком записей и заданной степенью сжатия.
fn write_zip(out_path: &Path, entries: &[Entry], level: i64) -> Result<()> {
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("создание каталога {}", parent.display()))?;
    }
    let file =
        File::create(out_path).with_context(|| format!("создание файла {}", out_path.display()))?;
    let mut zip = ZipWriter::new(file);

    let level = level.clamp(0, 9);
    let options = if level == 0 {
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored)
    } else {
        SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(level))
    };

    for e in entries {
        zip.start_file(e.name.as_str(), options)
            .with_context(|| format!("запись {} в архив", e.name))?;
        let mut src =
            File::open(&e.abs).with_context(|| format!("открытие {}", e.abs.display()))?;
        io::copy(&mut src, &mut zip).with_context(|| format!("упаковка {}", e.abs.display()))?;
    }
    zip.finish().context("финализация архива")?;
    Ok(())
}

/// Проверяет, что архив открывается и все его записи — безопасные относительные пути
/// (без `..`/абсолютных, защита от zip-slip).
fn validate_archive(archive: &Path) -> Result<()> {
    let file =
        File::open(archive).with_context(|| format!("открытие архива {}", archive.display()))?;
    let mut zip = ZipArchive::new(file).context("архив повреждён или не является zip")?;
    for i in 0..zip.len() {
        let entry = zip.by_index(i)?;
        if entry.enclosed_name().is_none() {
            bail!("небезопасное имя записи в архиве: {}", entry.name());
        }
    }
    Ok(())
}

/// Распаковывает архив в корень данных (имена записей уже считаются безопасными —
/// `enclosed_name` отсекает выход за пределы корня).
fn extract_archive(paths: &Paths, archive: &Path) -> Result<()> {
    let file =
        File::open(archive).with_context(|| format!("открытие архива {}", archive.display()))?;
    let mut zip = ZipArchive::new(file).context("чтение архива")?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let rel = entry
            .enclosed_name()
            .ok_or_else(|| anyhow!("небезопасное имя записи в архиве: {}", entry.name()))?;
        let dest = paths.root().join(&rel);
        if entry.is_dir() {
            fs::create_dir_all(&dest)
                .with_context(|| format!("создание каталога {}", dest.display()))?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("создание каталога {}", parent.display()))?;
        }
        let mut out =
            File::create(&dest).with_context(|| format!("создание файла {}", dest.display()))?;
        io::copy(&mut entry, &mut out).with_context(|| format!("распаковка {}", dest.display()))?;
    }
    Ok(())
}

/// Удаляет пользовательские данные из корня, **сохраняя** `backups/`, `logs/` и
/// файлы умолчаний `defaults.json`/`location.json`. Очищается ровно тот же набор,
/// что входит в резервную копию.
fn clear_user_data(paths: &Paths, fs_root: Option<&Path>) -> Result<()> {
    let root = paths.root();

    for f in TOP_FILES {
        remove_file_if_exists(&root.join(f))?;
    }
    if let Ok(rd) = fs::read_dir(root) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().is_some_and(|e| e == "bak") {
                remove_file_if_exists(&path)?;
            }
        }
    }
    for d in TOP_DIRS {
        remove_dir_if_exists(&root.join(d))?;
    }
    if let Some((abs, _)) = fs_root_under_root(root, fs_root) {
        remove_dir_if_exists(&abs)?;
    }
    Ok(())
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("удаление {}", path.display())),
    }
}

fn remove_dir_if_exists(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("удаление каталога {}", path.display())),
    }
}

/// Есть ли в корне пользовательские данные (нужно ли делать pre-restore копию).
fn has_existing_data(paths: &Paths) -> bool {
    ["settings.json", "profiles.json", "data.db"]
        .iter()
        .any(|f| paths.root().join(f).exists())
        || dir_non_empty(&paths.chats_dir())
}

fn dir_non_empty(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|mut rd| rd.next().is_some())
}

/// Если `fs_root` задан и лежит внутри корня данных — возвращает (канонический путь,
/// имя-префикс в архиве). Иначе `None` (вне корня → в копию не входит, не очищается).
fn fs_root_under_root(root: &Path, fs_root: Option<&Path>) -> Option<(PathBuf, String)> {
    let fs_root = fs_root?;
    let root_c = fs::canonicalize(root).ok()?;
    let fs_c = fs::canonicalize(fs_root).ok()?;
    let rel = fs_c.strip_prefix(&root_c).ok()?;
    if rel.as_os_str().is_empty() {
        return None;
    }
    let prefix = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if prefix.is_empty() {
        return None;
    }
    Some((fs_c, prefix))
}

/// Авто-имя архива в `backups/`: `<prefix>-YYYYMMDD-HHMMSS.zip`.
fn default_backup_path(paths: &Paths, prefix: &str) -> PathBuf {
    let stamp = Local::now().format("%Y%m%d-%H%M%S");
    paths.backups_dir().join(format!("{prefix}-{stamp}.zip"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Готовит корень с типичным набором пользовательских данных.
    fn seed_data(root: &Path) {
        fs::write(root.join("settings.json"), b"{\"v\":1}").unwrap();
        fs::write(root.join("settings.bak"), b"{\"v\":0}").unwrap();
        fs::write(root.join("profiles.json"), b"[]").unwrap();
        fs::write(root.join("data.db"), b"SQLITE").unwrap();
        fs::write(root.join("personal_dictionary.txt"), b"foo\n").unwrap();
        fs::create_dir_all(root.join("chats")).unwrap();
        fs::write(root.join("chats").join("a.json"), b"{}").unwrap();
        fs::write(root.join("chats").join("a.bak"), b"{}").unwrap();
        fs::create_dir_all(root.join("dictionaries")).unwrap();
        fs::write(root.join("dictionaries").join("en.dic"), b"x").unwrap();
        // Не должно попасть в копию:
        fs::create_dir_all(root.join("logs")).unwrap();
        fs::write(root.join("logs").join("mindfork.log"), b"log").unwrap();
        fs::create_dir_all(root.join("backups")).unwrap();
        fs::write(root.join("location.json"), b"{\"mode\":\"portable\"}").unwrap();
        fs::write(
            root.join("defaults.json"),
            b"{\"mode\":\"portable\",\"default_language\":\"ru\"}",
        )
        .unwrap();
    }

    fn archive_names(archive: &Path) -> Vec<String> {
        let mut zip = ZipArchive::new(File::open(archive).unwrap()).unwrap();
        (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect()
    }

    #[test]
    fn backup_includes_expected_and_excludes_logs_marker() {
        let dir = tempfile::tempdir().unwrap();
        seed_data(dir.path());
        let paths = Paths::with_root(dir.path());

        let out = create_backup(&paths, None, 9, None).unwrap();
        assert!(out.starts_with(paths.backups_dir()));
        let names = archive_names(&out);

        for expected in [
            "settings.json",
            "settings.bak",
            "profiles.json",
            "data.db",
            "personal_dictionary.txt",
            "chats/a.json",
            "chats/a.bak",
            "dictionaries/en.dic",
        ] {
            assert!(
                names.contains(&expected.to_string()),
                "нет {expected} в {names:?}"
            );
        }
        // Логи, каталог backups и файлы умолчаний не попадают.
        assert!(!names.iter().any(|n| n.starts_with("logs/")));
        assert!(!names.iter().any(|n| n.starts_with("backups/")));
        assert!(!names.contains(&"location.json".to_string()));
        assert!(!names.contains(&"defaults.json".to_string()));
    }

    #[test]
    fn fs_root_included_only_when_inside_root() {
        // Внутри корня — включается.
        let dir = tempfile::tempdir().unwrap();
        seed_data(dir.path());
        let inside = dir.path().join("sandbox");
        fs::create_dir_all(&inside).unwrap();
        fs::write(inside.join("note.txt"), b"hi").unwrap();
        let paths = Paths::with_root(dir.path());
        let out = create_backup(&paths, None, 9, Some(&inside)).unwrap();
        assert!(archive_names(&out).contains(&"sandbox/note.txt".to_string()));

        // Снаружи корня — не включается.
        let outside_root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), b"no").unwrap();
        seed_data(outside_root.path());
        let paths2 = Paths::with_root(outside_root.path());
        let out2 = create_backup(&paths2, None, 0, Some(outside.path())).unwrap();
        assert!(
            !archive_names(&out2)
                .iter()
                .any(|n| n.contains("secret.txt"))
        );
    }

    #[test]
    fn restore_round_trip_replaces_data() {
        // Источник.
        let src = tempfile::tempdir().unwrap();
        seed_data(src.path());
        fs::write(src.path().join("settings.json"), b"{\"v\":42}").unwrap();
        let src_paths = Paths::with_root(src.path());
        let archive_path = src.path().join("backups").join("snap.zip");
        create_backup(&src_paths, Some(archive_path.clone()), 9, None).unwrap();

        // Цель с другими данными.
        let dst = tempfile::tempdir().unwrap();
        seed_data(dst.path());
        fs::write(dst.path().join("settings.json"), b"{\"v\":999}").unwrap();
        fs::write(dst.path().join("chats").join("stale.json"), b"{}").unwrap();
        let dst_paths = Paths::with_root(dst.path());

        let outcome = restore_backup(&dst_paths, &archive_path, None).unwrap();
        match outcome {
            RestoreOutcome::Restored { pre_restore } => {
                // Прежние данные были, значит pre-restore копия создана.
                let pre = pre_restore.expect("pre-restore копия должна быть создана");
                assert!(pre.exists());
                assert!(pre.starts_with(dst_paths.backups_dir()));
            }
            _ => panic!("ожидался успешный Restored"),
        }
        // Данные заменены содержимым архива.
        assert_eq!(
            fs::read(dst.path().join("settings.json")).unwrap(),
            b"{\"v\":42}"
        );
        // Устаревший чат, которого нет в архиве, удалён очисткой.
        assert!(!dst.path().join("chats").join("stale.json").exists());
        // Каталог backups сохранён (там лежит pre-restore копия).
        assert!(dst.path().join("backups").exists());
    }

    #[test]
    fn restore_rejects_corrupt_archive_without_touching_data() {
        let dst = tempfile::tempdir().unwrap();
        seed_data(dst.path());
        let paths = Paths::with_root(dst.path());
        let bad = dst.path().join("bad.zip");
        fs::write(&bad, b"this is not a zip file").unwrap();

        let err = restore_backup(&paths, &bad, None);
        assert!(
            err.is_err(),
            "повреждённый архив должен дать Err до очистки"
        );
        // Данные не тронуты.
        assert!(dst.path().join("settings.json").exists());
        assert!(dst.path().join("chats").join("a.json").exists());
    }

    #[test]
    fn restore_into_empty_root_makes_no_pre_restore() {
        let src = tempfile::tempdir().unwrap();
        seed_data(src.path());
        let archive = src.path().join("snap.zip");
        create_backup(
            &Paths::with_root(src.path()),
            Some(archive.clone()),
            9,
            None,
        )
        .unwrap();

        let dst = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dst.path());
        let outcome = restore_backup(&paths, &archive, None).unwrap();
        match outcome {
            RestoreOutcome::Restored { pre_restore } => assert!(pre_restore.is_none()),
            _ => panic!("ожидался Restored без pre-restore"),
        }
        assert!(dst.path().join("settings.json").exists());
    }

    #[test]
    fn restore_rolls_back_on_extraction_failure() {
        use std::io::Write;

        // Архив валиден (проходит validate_archive), но запись `blocker` — файл,
        // который на цели столкнётся с одноимённым каталогом → распаковка упадёт.
        let work = tempfile::tempdir().unwrap();
        let archive = work.path().join("evil.zip");
        {
            let f = File::create(&archive).unwrap();
            let mut zip = ZipWriter::new(f);
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zip.start_file("settings.json", opts).unwrap();
            zip.write_all(b"{\"from\":\"archive\"}").unwrap();
            zip.start_file("blocker", opts).unwrap();
            zip.write_all(b"x").unwrap();
            zip.finish().unwrap();
        }

        let dst = tempfile::tempdir().unwrap();
        seed_data(dst.path());
        fs::write(dst.path().join("settings.json"), b"{\"from\":\"original\"}").unwrap();
        // Каталог `blocker` не входит в whitelist → переживает очистку и ломает
        // распаковку одноимённого файла.
        fs::create_dir_all(dst.path().join("blocker")).unwrap();

        let paths = Paths::with_root(dst.path());
        let outcome = restore_backup(&paths, &archive, None).unwrap();
        match outcome {
            RestoreOutcome::RolledBack { pre_restore, .. } => assert!(pre_restore.exists()),
            _ => panic!("ожидался RolledBack при сбое распаковки"),
        }
        // Откат вернул исходные данные из pre-restore копии.
        assert_eq!(
            fs::read(dst.path().join("settings.json")).unwrap(),
            b"{\"from\":\"original\"}"
        );
        assert!(dst.path().join("chats").join("a.json").exists());
    }

    #[test]
    fn store_level_zero_produces_readable_archive() {
        let dir = tempfile::tempdir().unwrap();
        seed_data(dir.path());
        let paths = Paths::with_root(dir.path());
        let out = create_backup(&paths, None, 0, None).unwrap();
        // Архив валиден и открывается.
        validate_archive(&out).unwrap();
    }
}
