//! Загрузка Hunspell-словарей (`spellbook`) из каталога `dictionaries/` и
//! персонального словаря. См. spec §11.5.
//!
//! Каталог сканируется на пары `*.aff` + `*.dic` с общим именем (`en_US.aff` +
//! `en_US.dic`). Каждая успешно загруженная пара — отдельный словарь; битые
//! пропускаются с записью в лог (спелл-чек деградирует, не падает). Загрузка
//! тяжёлая (парсинг `.dic`) — вызывается в фоновом потоке (см. `main.rs`).

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use spellbook::Dictionary;

use super::check::SpellChecker;

/// Загружает словари и персональный словарь из `personal_path` согласно настройкам
/// интерфейса (spec §11.6):
/// - `enabled = false` → словари не грузятся (вернётся отключённый чекер);
/// - `selected` непуст → грузятся только пары с базовым именем из списка
///   (`en_US`/`ru_RU`/…); пустой `selected` → все найденные.
///
/// Словари ищутся сперва в `dict_dir` (корень данных), затем в `bundled_dir` —
/// портативной раскладке рядом с бинарником, куда инсталлятор/пакет кладёт словари при
/// не-портативном режиме хранения (П1, §4.2 installers.md). Пара, чьё базовое имя уже
/// загружено из `dict_dir`, из `bundled_dir` **не** перегружается (пользовательский
/// словарь того же имени выигрывает). В портативном режиме `bundled_dir` совпадает с
/// `dict_dir` — второй проход ничего не добавляет (все имена уже загружены).
///
/// Отсутствие каталога/файлов — не ошибка (отключённый чекер). Загрузка тяжёлая
/// (парсинг `.dic`) — вызывается в фоновом потоке (см. `app/runtime.rs`).
pub fn load(
    dict_dir: &Path,
    bundled_dir: Option<&Path>,
    personal_path: &Path,
    enabled: bool,
    selected: &[String],
) -> SpellChecker {
    let mut dicts = Vec::new();
    if !enabled {
        tracing::info!("спелл-чек выключен в настройках — словари не загружаются");
    } else {
        // Уже загруженные базовые имена — чтобы `bundled_dir` не дублировал словарь,
        // взятый из `dict_dir` (и не перегружал один и тот же в портативном режиме).
        let mut loaded = HashSet::new();
        load_dir(dict_dir, selected, &mut loaded, &mut dicts);
        if let Some(bundled) = bundled_dir
            && bundled != dict_dir
        {
            load_dir(bundled, selected, &mut loaded, &mut dicts);
        }
    }

    let personal = load_personal(personal_path);
    SpellChecker::new(dicts, personal, Some(personal_path.to_path_buf()))
}

/// Загружает пары `*.aff`/`*.dic` из одного каталога, пропуская базовые имена, уже
/// присутствующие в `loaded` (и добавляя загруженные туда же). Фильтрует по `selected`.
fn load_dir(
    dir: &Path,
    selected: &[String],
    loaded: &mut HashSet<String>,
    dicts: &mut Vec<Dictionary>,
) {
    match fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let aff = entry.path();
                if aff.extension().and_then(|e| e.to_str()) != Some("aff") {
                    continue;
                }
                // Фильтр по выбранным словарям (по базовому имени файла).
                let stem = aff.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
                if !selected.is_empty() && !selected.iter().any(|s| s == stem) {
                    continue;
                }
                // Имя уже загружено (из приоритетного каталога) — не перегружаем.
                if loaded.contains(stem) {
                    continue;
                }
                let dic = aff.with_extension("dic");
                if !dic.exists() {
                    continue;
                }
                match load_pair(&aff, &dic) {
                    Ok(dict) => {
                        tracing::info!(dict = %aff.display(), "словарь загружен");
                        loaded.insert(stem.to_string());
                        dicts.push(dict);
                    }
                    Err(err) => {
                        tracing::warn!(dict = %aff.display(), error = %err, "словарь пропущен");
                    }
                }
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!(dir = %dir.display(), "каталог словарей отсутствует");
        }
        Err(err) => {
            tracing::warn!(dir = %dir.display(), error = %err, "не удалось прочитать каталог словарей");
        }
    }
}

/// Загружает одну пару `.aff`/`.dic`.
fn load_pair(aff: &Path, dic: &Path) -> anyhow::Result<Dictionary> {
    let aff_text = fs::read_to_string(aff)?;
    let dic_text = fs::read_to_string(dic)?;
    Dictionary::new(&aff_text, &dic_text).map_err(|e| anyhow::anyhow!("разбор словаря: {e}"))
}

/// Читает персональный словарь (по слову на строку; пустые и `#`-комментарии
/// игнорируются). Отсутствие файла — пустой набор.
pub fn load_personal(path: &Path) -> HashSet<String> {
    let mut set = HashSet::new();
    if let Ok(text) = fs::read_to_string(path) {
        for line in text.lines() {
            let word = line.trim();
            if !word.is_empty() && !word.starts_with('#') {
                set.insert(word.to_string());
            }
        }
    }
    set
}

/// Дописывает слово в файл персонального словаря (создаёт при необходимости).
pub fn append_personal(path: &Path, word: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{word}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Крошечный английский словарь для тестов (формат Hunspell).
    const AFF: &str = "SET UTF-8\n";
    const DIC: &str = "3\nhello\nworld\ncat\n";

    fn write(dir: &Path, name: &str, content: &str) {
        fs::write(dir.join(name), content).unwrap();
    }

    fn load_all(dir: &Path) -> SpellChecker {
        load(dir, None, &dir.join("personal.txt"), true, &[])
    }

    #[test]
    fn loads_aff_dic_pair() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "en.aff", AFF);
        write(dir.path(), "en.dic", DIC);
        let checker = load_all(dir.path());
        assert!(checker.is_enabled());
        assert!(checker.check_word("hello"));
        assert!(!checker.check_word("zxcvb"));
    }

    #[test]
    fn missing_dir_disables_checker() {
        let dir = tempfile::tempdir().unwrap();
        let checker = load_all(&dir.path().join("nope"));
        assert!(!checker.is_enabled());
    }

    #[test]
    fn aff_without_dic_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "en.aff", AFF); // нет en.dic
        let checker = load_all(dir.path());
        assert!(!checker.is_enabled());
    }

    #[test]
    fn disabled_skips_loading() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "en.aff", AFF);
        write(dir.path(), "en.dic", DIC);
        // enabled = false → словари не грузятся, чекер выключен.
        let checker = load(
            dir.path(),
            None,
            &dir.path().join("personal.txt"),
            false,
            &[],
        );
        assert!(!checker.is_enabled());
    }

    #[test]
    fn selection_filters_dictionaries() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "en_US.aff", AFF);
        write(dir.path(), "en_US.dic", DIC);
        write(dir.path(), "ru_RU.aff", AFF);
        write(dir.path(), "ru_RU.dic", "1\nпривет\n");
        // Выбран только en_US → ru_RU не загружается.
        let checker = load(
            dir.path(),
            None,
            &dir.path().join("personal.txt"),
            true,
            &["en_US".to_string()],
        );
        assert!(checker.check_word("hello")); // из en_US
        assert!(!checker.check_word("привет")); // ru_RU не загружен
    }

    #[test]
    fn bundled_dir_supplies_missing_dictionaries() {
        // Корень данных (system-режим) пуст, словари лежат рядом с бинарником — берём
        // их из bundled-каталога (П1).
        let data = tempfile::tempdir().unwrap();
        let bundled = tempfile::tempdir().unwrap();
        write(bundled.path(), "en.aff", AFF);
        write(bundled.path(), "en.dic", DIC);
        let checker = load(
            data.path(),
            Some(bundled.path()),
            &data.path().join("personal.txt"),
            true,
            &[],
        );
        assert!(checker.is_enabled());
        assert!(checker.check_word("hello"));
    }

    #[test]
    fn data_dir_dictionary_wins_over_bundled() {
        // Одноимённый словарь есть и в корне данных, и рядом с бинарником — берётся из
        // корня данных (пользовательский приоритетнее вшитого); дубля нет.
        let data = tempfile::tempdir().unwrap();
        let bundled = tempfile::tempdir().unwrap();
        write(data.path(), "en.aff", AFF);
        write(data.path(), "en.dic", "1\nhello\n"); // только hello
        write(bundled.path(), "en.aff", AFF);
        write(bundled.path(), "en.dic", DIC); // hello/world/cat
        let checker = load(
            data.path(),
            Some(bundled.path()),
            &data.path().join("personal.txt"),
            true,
            &[],
        );
        assert!(checker.check_word("hello"));
        // "world" есть только в bundled-версии — раз выиграл словарь корня, его нет.
        assert!(!checker.check_word("world"));
    }

    #[test]
    fn personal_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("personal.txt");
        fs::write(&path, "# комментарий\nмойтермин\n\n").unwrap();
        let set = load_personal(&path);
        assert!(set.contains("мойтермин"));
        assert_eq!(set.len(), 1);

        append_personal(&path, "ещёслово").unwrap();
        let set2 = load_personal(&path);
        assert!(set2.contains("ещёслово"));
        assert!(set2.contains("мойтермин"));
    }
}
