//! Сканирование файлов для индексации в базу знаний (RAG, команда `/rag add`).
//! Чистая, тестируемая файловая логика: обход пути, отбор поддерживаемых
//! расширений (txt/md), опциональная рекурсия и чтение содержимого. Сам процесс
//! индексации (эмбеддинг + запись) ведёт фоновая задача оркестратора. См. spec §9.3.
//!
//! Здесь же живёт [`RagProgress`] — тип прогресса индексации. Он определён в слое
//! `features`, чтобы им могли пользоваться и `app` (эмитит события), и `screens`
//! (рисует индикатор), не нарушая направление зависимостей FSD.

use std::path::{Path, PathBuf};

/// Поддерживаемые расширения файлов (нижний регистр, без точки). Пока — текст и
/// markdown (см. постановку задачи).
pub const SUPPORTED_EXTENSIONS: &[&str] = &["txt", "md"];

/// Прогресс фоновой индексации файлов в RAG. Шлётся задачей оркестратора и
/// отображается экраном чата (баннер со спиннером + итоговая заметка).
#[derive(Debug, Clone, PartialEq)]
pub enum RagProgress {
    /// Сканирование завершено — начинаем индексацию `total` файлов.
    Started { total: usize },
    /// Индексируется файл `index` из `total` (1-based) с именем `name` из `dir`.
    Indexing {
        index: usize,
        total: usize,
        name: String,
        dir: String,
    },
    /// Индексация завершена (или прервана при `cancelled`).
    Finished {
        files: usize,
        chunks: usize,
        errors: usize,
        cancelled: bool,
    },
    /// Удаление из базы завершено (`/rag delete`): снято `chunks` фрагментов
    /// (0 — по указанному пути ничего не найдено).
    Removed { chunks: usize },
    /// Не удалось выполнить операцию (путь недоступен, нет файлов, эмбеддер не
    /// настроен и т.п.).
    Failed(String),
}

/// Поддерживается ли файл по расширению (регистронезависимо).
pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SUPPORTED_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Собирает список поддерживаемых файлов по пути:
/// - путь-файл → он сам, если расширение поддерживается (иначе пусто);
/// - путь-директория → все поддерживаемые файлы (рекурсивно при `recursive`).
///
/// Результат отсортирован для детерминизма. Ошибки чтения вложенных директорий
/// пропускаются (обход устойчив), а недоступность корневого пути — это ошибка.
pub fn scan(root: &Path, recursive: bool) -> std::io::Result<Vec<PathBuf>> {
    let meta = std::fs::metadata(root)?;
    let mut out = Vec::new();
    if meta.is_file() {
        if is_supported(root) {
            out.push(root.to_path_buf());
        }
        return Ok(out);
    }
    collect_dir(root, recursive, &mut out);
    out.sort();
    Ok(out)
}

/// Рекурсивный обход директории (ошибки отдельных входов пропускаются).
fn collect_dir(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if ft.is_dir() {
            if recursive {
                collect_dir(&path, recursive, out);
            }
        } else if ft.is_file() && is_supported(&path) {
            out.push(path);
        }
    }
}

/// Канонический строковый ключ источника для записи в RAG: абсолютный путь без
/// вербатим-префикса `\\?\` (Windows). Используется и при добавлении (стабильный
/// `source`), и при удалении (тот же ключ независимо от того, как путь введён —
/// относительно, иным регистром или разделителем). Если канонизация не удалась
/// (файла уже нет на диске) — путь как есть (lossy).
pub fn canonical_source(path: &Path) -> String {
    match std::fs::canonicalize(path) {
        Ok(abs) => strip_verbatim(abs.to_string_lossy().into_owned()),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// Снимает вербатим-префикс `\\?\`, который `canonicalize` добавляет на Windows
/// (чтобы хранимый/показываемый путь был обычным).
fn strip_verbatim(s: String) -> String {
    match s.strip_prefix(r"\\?\") {
        Some(rest) => rest.to_string(),
        None => s,
    }
}

/// Читает текстовый файл в строку, отбрасывая ведущий UTF-8 BOM. Не-UTF-8 или
/// нечитаемый файл → ошибка (вызывающий пропускает такой файл).
pub fn read_text(path: &Path) -> std::io::Result<String> {
    let mut content = std::fs::read_to_string(path)?;
    if content.starts_with('\u{feff}') {
        content.remove(0);
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn is_supported_matches_txt_and_md_case_insensitively() {
        assert!(is_supported(Path::new("a.txt")));
        assert!(is_supported(Path::new("a.MD")));
        assert!(is_supported(Path::new("dir/b.Md")));
        assert!(!is_supported(Path::new("a.pdf")));
        assert!(!is_supported(Path::new("noext")));
    }

    #[test]
    fn scan_single_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.md");
        write(&file, "hello");
        let found = scan(&file, false).unwrap();
        assert_eq!(found, vec![file]);

        // Неподдерживаемый файл → пустой список (не ошибка).
        let other = dir.path().join("data.bin");
        write(&other, "x");
        assert!(scan(&other, false).unwrap().is_empty());
    }

    #[test]
    fn scan_directory_non_recursive_skips_subdirs_and_other_exts() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.txt"), "a");
        write(&dir.path().join("b.md"), "b");
        write(&dir.path().join("c.pdf"), "c"); // не поддержан
        write(&dir.path().join("sub/d.txt"), "d"); // в подпапке

        let found = scan(dir.path(), false).unwrap();
        let names: Vec<_> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["a.txt", "b.md"]);
    }

    #[test]
    fn scan_directory_recursive_includes_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a.txt"), "a");
        write(&dir.path().join("sub/d.txt"), "d");
        write(&dir.path().join("sub/deep/e.md"), "e");

        let found = scan(dir.path(), true).unwrap();
        assert_eq!(found.len(), 3, "{found:?}");
    }

    #[test]
    fn scan_missing_path_errors() {
        let dir = tempfile::tempdir().unwrap();
        assert!(scan(&dir.path().join("nope"), false).is_err());
    }

    #[test]
    fn read_text_strips_bom() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bom.txt");
        write(&file, "\u{feff}содержимое");
        assert_eq!(read_text(&file).unwrap(), "содержимое");
    }

    #[test]
    fn canonical_source_has_no_verbatim_prefix_and_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("doc.txt");
        write(&file, "x");
        let src = canonical_source(&file);
        assert!(!src.starts_with(r"\\?\"), "{src}");
        // Каноничный ключ совпадает для одного и того же файла.
        assert_eq!(src, canonical_source(&file));
        // Несуществующий путь → возвращается как есть (не паникует).
        let missing = dir.path().join("nope.txt");
        assert_eq!(canonical_source(&missing), missing.to_string_lossy());
    }
}
