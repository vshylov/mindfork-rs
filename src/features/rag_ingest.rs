//! Сканирование файлов для индексации в базу знаний (RAG, команда `/rag add`).
//! Чистая, тестируемая файловая логика: обход пути, отбор поддерживаемых
//! расширений (txt/md/html/pdf/docx), опциональная рекурсия и чтение содержимого.
//! Сам процесс индексации (эмбеддинг + запись) ведёт фоновая задача оркестратора.
//! См. spec §9.3. Извлечение текста из HTML/PDF/DOCX — обязанность слоя `app`
//! (`orchestrator/rag.rs`), а не этого модуля (иначе `features → features/tools` —
//! боковой импорт, запрещённый FSD); здесь лишь опознаётся расширение.
//!
//! Здесь же живёт [`RagProgress`] — тип прогресса индексации. Он определён в слое
//! `features`, чтобы им могли пользоваться и `app` (эмитит события), и `screens`
//! (рисует индикатор), не нарушая направление зависимостей FSD.

use std::path::{Path, PathBuf};

/// Поддерживаемые расширения файлов (нижний регистр, без точки): текст, markdown,
/// HTML, PDF и DOCX. Из HTML/PDF/DOCX извлекается простой текст на слое `app`
/// (`orchestrator/rag.rs`), этот модуль лишь опознаёт расширение.
pub const SUPPORTED_EXTENSIONS: &[&str] = &["txt", "md", "html", "htm", "pdf", "docx"];

/// Прогресс фоновой индексации файлов в RAG. Шлётся задачей оркестратора и
/// отображается экраном чата (баннер со спиннером + итоговая заметка).
#[derive(Debug, Clone, PartialEq)]
pub enum RagProgress {
    /// Сканирование завершено — начинаем индексацию `total` файлов.
    Started { total: usize },
    /// Индексируется файл `index` из `total` (1-based) с именем `name` из `dir`.
    /// `chunks_done`/`chunks_total` — прогресс эмбеддинга **внутри** этого файла:
    /// сколько чанков уже эмбеддировано и записано и сколько всего (эмбеддинг идёт
    /// под-батчами, см. `orchestrator/rag.rs::EMBED_BATCH_CHUNKS`). `chunks_total == 0`
    /// — файл только начат (ещё не чанкован) либо у него нет чанков.
    Indexing {
        index: usize,
        total: usize,
        name: String,
        dir: String,
        chunks_done: usize,
        chunks_total: usize,
    },
    /// Индексация завершена (или прервана при `cancelled`).
    Finished {
        files: usize,
        chunks: usize,
        errors: usize,
        cancelled: bool,
    },
    /// Удаление из базы завершено (`/rag remove`): снято `chunks` фрагментов
    /// (0 — по указанному пути ничего не найдено).
    Removed { chunks: usize },
    /// Перечень источников базы знаний (`/rag list`): по источнику — счётчик чанков
    /// и дата. Пустой список — база пуста.
    Listed {
        sources: Vec<crate::entities::rag::RagSourceInfo>,
    },
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

/// Совпадает ли расширение пути с `ext` (регистронезависимо)?
fn has_ext(path: &Path, ext: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// HTML-файл по расширению (`html`/`htm`, регистронезависимо)? Слой `app` по этому
/// признаку решает, извлекать ли читаемый текст (иначе читает содержимое как есть).
pub fn is_html(path: &Path) -> bool {
    has_ext(path, "html") || has_ext(path, "htm")
}

/// PDF-файл по расширению (`pdf`, регистронезависимо)? Слой `app` извлекает из него
/// текст крейтом `pdf-extract` (см. `orchestrator/rag.rs`, `features/doc_extract.rs`).
pub fn is_pdf(path: &Path) -> bool {
    has_ext(path, "pdf")
}

/// DOCX-файл по расширению (`docx`, регистронезависимо)? Слой `app` извлекает из него
/// текст (ZIP + `word/document.xml`, см. `features/doc_extract.rs`).
pub fn is_docx(path: &Path) -> bool {
    has_ext(path, "docx")
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
        assert!(!is_supported(Path::new("a.rtf")));
        assert!(!is_supported(Path::new("noext")));
    }

    #[test]
    fn is_supported_and_helpers_match_pdf_and_docx_case_insensitively() {
        // PDF/DOCX поддержаны наравне с txt/md/html.
        assert!(is_supported(Path::new("doc.pdf")));
        assert!(is_supported(Path::new("dir/report.DOCX")));
        assert!(is_supported(Path::new("a.PDF")));
        // is_pdf/is_docx выделяют именно свои расширения (регистронезависимо)...
        assert!(is_pdf(Path::new("doc.pdf")));
        assert!(is_pdf(Path::new("dir/doc.PDF")));
        assert!(is_docx(Path::new("report.docx")));
        assert!(is_docx(Path::new("dir/report.DocX")));
        // ...и не срабатывают на чужих/неподдержанных расширениях.
        assert!(!is_pdf(Path::new("report.docx")));
        assert!(!is_pdf(Path::new("a.txt")));
        assert!(!is_docx(Path::new("doc.pdf")));
        assert!(!is_docx(Path::new("a.md")));
        assert!(!is_pdf(Path::new("noext")));
        // .doc (legacy) намеренно не поддержан.
        assert!(!is_supported(Path::new("old.doc")));
        assert!(!is_docx(Path::new("old.doc")));
    }

    #[test]
    fn is_supported_and_is_html_match_html_case_insensitively() {
        // HTML поддержан наравне с txt/md.
        assert!(is_supported(Path::new("page.html")));
        assert!(is_supported(Path::new("page.htm")));
        assert!(is_supported(Path::new("page.HTML")));
        // is_html выделяет именно HTML-расширения (регистронезависимо)...
        assert!(is_html(Path::new("page.html")));
        assert!(is_html(Path::new("dir/page.Htm")));
        assert!(is_html(Path::new("page.HTML")));
        // ...и не срабатывает на прочих поддержанных/неподдержанных расширениях.
        assert!(!is_html(Path::new("a.txt")));
        assert!(!is_html(Path::new("a.md")));
        assert!(!is_html(Path::new("noext")));
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
        write(&dir.path().join("c.rtf"), "c"); // не поддержан
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
