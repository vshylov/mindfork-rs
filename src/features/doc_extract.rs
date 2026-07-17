//! Извлечение простого текста из бинарных форматов документов (PDF/DOCX) для
//! индексации в базу знаний (RAG, команда `/rag add`, этап B1b —
//! `docs/rag-sources-retrieval.md`). Чистые функции над байтами: диспетчеризация по
//! расширению и само извлечение — обязанность слоя `app` (`orchestrator/rag.rs`);
//! здесь лишь разбор форматов внешними крейтами (без кросс-слойных импортов FSD).
//!
//! Философия «лучшее усилие»: сбой извлечения возвращает `Err` (цикл индексации
//! ловит ошибку файла, логирует `warn` и увеличивает счётчик ошибок — не паникует).
//! Сканированный/картиночный PDF может дать пустой/пробельный текст — это допустимо.

use std::io::{Cursor, Read};

use anyhow::Context;
use quick_xml::Reader;
use quick_xml::events::Event;

/// Извлекает читаемый текст из DOCX (Office Open XML). DOCX — это ZIP-архив с
/// deflate-сжатием; текст документа лежит в `word/document.xml`. Разбираем его
/// потоковым ридером `quick-xml`, собирая содержимое элементов `<w:t>` (по
/// **локальному** имени, игнорируя префикс `w:`); конец абзаца `</w:p>` → перевод
/// строки, `<w:tab/>` → табуляция, `<w:br/>` → перевод строки. Сущности (`&amp;`
/// и т.п.) декодируются. Хвостовые пробелы срезаются.
///
/// Отсутствие `word/document.xml`, битый ZIP или некорректный XML → `Err` с
/// контекстом (вызывающий пропускает такой файл).
pub fn extract_docx(bytes: &[u8]) -> anyhow::Result<String> {
    let mut zip =
        zip::ZipArchive::new(Cursor::new(bytes)).context("DOCX: не удалось открыть ZIP-архив")?;
    let mut xml = Vec::new();
    zip.by_name("word/document.xml")
        .context("DOCX: в архиве нет word/document.xml")?
        .read_to_end(&mut xml)
        .context("DOCX: не удалось прочитать word/document.xml")?;

    let mut reader = Reader::from_reader(xml.as_slice());
    let mut buf = Vec::new();
    let mut out = String::new();
    // Текст в DOCX лежит только внутри `<w:t>`; вне его — незначимые пробелы между
    // тегами. Собираем содержимое лишь пока открыт `<w:t>`.
    let mut in_text = false;
    loop {
        match reader
            .read_event_into(&mut buf)
            .context("DOCX: ошибка разбора XML")?
        {
            // Совпадение по **локальному** имени (без префикса `w:`).
            Event::Start(e) => {
                if e.local_name().as_ref() == b"t" {
                    in_text = true;
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                b"t" => in_text = false,
                b"p" => out.push('\n'), // конец абзаца
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                b"tab" => out.push('\t'),
                b"br" => out.push('\n'),
                _ => {}
            },
            Event::Text(e) if in_text => {
                // Текст декодируется как UTF-8 (эскейпы quick-xml выдаёт отдельными
                // событиями GeneralRef, см. ниже — тут их уже нет).
                let decoded = e.decode().context("DOCX: не удалось декодировать текст")?;
                out.push_str(&decoded);
            }
            // quick-xml 0.39 отдаёт ссылки на сущности (`&amp;`, `&#38;`) отдельным
            // событием — в BytesRef лежит имя без `&`/`;`. Восстанавливаем и снимаем
            // эскейп (unescape понимает и именованные, и числовые ссылки).
            Event::GeneralRef(e) if in_text => {
                let name = e
                    .decode()
                    .context("DOCX: не удалось декодировать сущность")?;
                let entity = format!("&{name};");
                let text = quick_xml::escape::unescape(&entity)
                    .context("DOCX: не удалось снять эскейп сущности")?;
                out.push_str(&text);
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(out.trim_end().to_string())
}

/// Извлекает текст из PDF (крейт `pdf-extract`, качество «лучшее усилие»).
/// Сканированный/картиночный PDF может вернуть пустой/пробельный текст — это
/// допустимо (RAG просто не проиндексирует нечего). Битый PDF → `Err` с контекстом.
pub fn extract_pdf(bytes: &[u8]) -> anyhow::Result<String> {
    pdf_extract::extract_text_from_mem(bytes)
        .map_err(|e| anyhow::anyhow!("PDF: не удалось извлечь текст: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Собирает минимальный валидный DOCX в памяти: ZIP c `word/document.xml`,
    /// содержащим `abs` абзацев (каждый — `<w:p><w:r><w:t>…</w:t></w:r></w:p>`).
    fn build_docx(paragraphs: &[&str]) -> Vec<u8> {
        let mut body = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
             <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
             <w:body>",
        );
        for p in paragraphs {
            body.push_str("<w:p><w:r><w:t xml:space=\"preserve\">");
            body.push_str(p);
            body.push_str("</w:t></w:r></w:p>");
        }
        body.push_str("</w:body></w:document>");

        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("word/document.xml", opts).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn extract_docx_returns_paragraphs_newline_separated_with_unescaped_entities() {
        // Кириллица + сущность `&amp;` (должна раскрыться в `&`) + скобки в тексте.
        let docx = build_docx(&["Первый абзац &amp; продолжение", "Второй абзац документа"]);
        let text = extract_docx(&docx).unwrap();

        // Оба абзаца присутствуют, сущность раскрыта.
        assert!(
            text.contains("Первый абзац & продолжение"),
            "сущность &amp; должна раскрыться в &: {text:?}"
        );
        assert!(
            text.contains("Второй абзац документа"),
            "второй абзац должен присутствовать: {text:?}"
        );
        // Абзацы разделены переводом строки.
        assert_eq!(
            text, "Первый абзац & продолжение\nВторой абзац документа",
            "абзацы разделены \\n, хвостовой перевод строки срезан: {text:?}"
        );
        // XML-теги не протекают в вывод.
        assert!(!text.contains("<w:"), "теги не должны протекать: {text:?}");
        assert!(!text.contains("w:t"), "теги не должны протекать: {text:?}");
    }

    #[test]
    fn extract_docx_handles_tab_and_break() {
        // Ручная сборка с <w:tab/> и <w:br/> внутри рана.
        let body = "<?xml version=\"1.0\"?>\
             <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
             <w:body><w:p><w:r><w:t>до</w:t><w:tab/><w:t>после</w:t><w:br/><w:t>строка</w:t>\
             </w:r></w:p></w:body></w:document>";
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        zip.start_file("word/document.xml", opts).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
        let docx = zip.finish().unwrap().into_inner();

        let text = extract_docx(&docx).unwrap();
        assert_eq!(text, "до\tпосле\nстрока");
    }

    #[test]
    fn extract_docx_missing_document_xml_errors() {
        // Валидный ZIP, но без word/document.xml → ошибка (не паника).
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        zip.start_file("other.txt", opts).unwrap();
        zip.write_all(b"nope").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        assert!(extract_docx(&bytes).is_err());
    }

    #[test]
    fn extract_docx_non_zip_bytes_error() {
        // Совсем не ZIP → ошибка (не паника).
        assert!(extract_docx(b"this is not a zip archive at all").is_err());
        assert!(extract_docx(b"").is_err());
    }

    /// Минимальный валидный PDF с текстом «Hello World» (корректный xref,
    /// шрифт Helvetica, поток контента `BT … Tj ET`). Проверено на `pdf-extract`.
    const HELLO_PDF: &[u8] = include_bytes!("../../tests/fixtures/hello.pdf");

    #[test]
    fn extract_pdf_extracts_text_from_fixture() {
        let text = extract_pdf(HELLO_PDF).unwrap();
        assert!(
            text.contains("Hello World"),
            "из PDF-фикстуры должен извлечься текст «Hello World»: {text:?}"
        );
    }

    #[test]
    fn extract_pdf_non_pdf_bytes_error() {
        // Не PDF → ошибка (не паника).
        assert!(extract_pdf(b"definitely not a pdf document").is_err());
    }
}
