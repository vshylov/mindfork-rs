//! Extracts plain text from binary document formats (PDF/DOCX) for
//! indexing into the knowledge base (RAG, the `/rag add` command, stage B1b —
//! `docs/history/rag-sources-retrieval.md`). Pure functions over bytes: dispatching by
//! extension and the extraction itself is the `app` layer's responsibility (`orchestrator/rag.rs`);
//! here only the format parsing via external crates (no cross-layer FSD imports).
//!
//! A "best effort" philosophy: an extraction failure returns `Err` (the indexing loop
//! catches the file's error, logs a `warn`, and bumps the error counter — it doesn't panic).
//! A scanned/image-only PDF may yield empty/whitespace-only text — that's acceptable.

use std::io::{Cursor, Read};

use anyhow::Context;
use quick_xml::Reader;
use quick_xml::events::Event;

/// Extracts readable text from a DOCX (Office Open XML). A DOCX is a ZIP archive with
/// deflate compression; the document's text lives in `word/document.xml`. We parse it via
/// `quick-xml`'s streaming reader, collecting the content of `<w:t>` elements (by
/// **local** name, ignoring the `w:` prefix); a paragraph's end `</w:p>` → a line
/// break, `<w:tab/>` → a tab, `<w:br/>` → a line break. Entities (`&amp;`
/// etc.) are decoded. Trailing whitespace is trimmed.
///
/// A missing `word/document.xml`, a corrupt ZIP, or invalid XML → `Err` with
/// context (the caller skips such a file).
pub fn extract_docx(bytes: &[u8]) -> anyhow::Result<String> {
    let mut zip =
        zip::ZipArchive::new(Cursor::new(bytes)).context("DOCX: failed to open the ZIP archive")?;
    let mut xml = Vec::new();
    zip.by_name("word/document.xml")
        .context("DOCX: the archive has no word/document.xml")?
        .read_to_end(&mut xml)
        .context("DOCX: failed to read word/document.xml")?;

    let mut reader = Reader::from_reader(xml.as_slice());
    let mut buf = Vec::new();
    let mut out = String::new();
    // Text in a DOCX lives only inside `<w:t>`; outside it — insignificant whitespace between
    // tags. We collect content only while `<w:t>` is open.
    let mut in_text = false;
    loop {
        match reader
            .read_event_into(&mut buf)
            .context("DOCX: XML parse error")?
        {
            // Matched by **local** name (without the `w:` prefix).
            Event::Start(e) => {
                if e.local_name().as_ref() == b"t" {
                    in_text = true;
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                b"t" => in_text = false,
                b"p" => out.push('\n'), // end of a paragraph
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                b"tab" => out.push('\t'),
                b"br" => out.push('\n'),
                _ => {}
            },
            Event::Text(e) if in_text => {
                // The text decodes as UTF-8 (quick-xml emits escapes as separate
                // GeneralRef events, see below — none are present here anymore).
                let decoded = e.decode().context("DOCX: failed to decode text")?;
                out.push_str(&decoded);
            }
            // quick-xml 0.39 emits entity references (`&amp;`, `&#38;`) as a separate
            // event — `BytesRef` holds the name without `&`/`;`. We reconstruct and
            // unescape it (`unescape` understands both named and numeric references).
            Event::GeneralRef(e) if in_text => {
                let name = e.decode().context("DOCX: failed to decode an entity")?;
                let entity = format!("&{name};");
                let text = quick_xml::escape::unescape(&entity)
                    .context("DOCX: failed to unescape an entity")?;
                out.push_str(&text);
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(out.trim_end().to_string())
}

/// Extracts text from a PDF (the `pdf-extract` crate, "best effort" quality).
/// A scanned/image-only PDF may return empty/whitespace-only text — that's
/// acceptable (RAG simply won't index anything). A corrupt PDF → `Err` with context.
pub fn extract_pdf(bytes: &[u8]) -> anyhow::Result<String> {
    pdf_extract::extract_text_from_mem(bytes)
        .map_err(|e| anyhow::anyhow!("PDF: failed to extract text: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Builds a minimal valid DOCX in memory: a ZIP with `word/document.xml`
    /// containing `paragraphs` paragraphs (each — `<w:p><w:r><w:t>…</w:t></w:r></w:p>`).
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
        // Cyrillic + the entity `&amp;` (should unescape to `&`) + brackets in the text.
        let docx = build_docx(&["Первый абзац &amp; продолжение", "Второй абзац документа"]);
        let text = extract_docx(&docx).unwrap();

        // Both paragraphs are present, the entity is unescaped.
        assert!(
            text.contains("Первый абзац & продолжение"),
            "the entity &amp; should unescape to &: {text:?}"
        );
        assert!(
            text.contains("Второй абзац документа"),
            "the second paragraph should be present: {text:?}"
        );
        // Paragraphs are separated by a line break.
        assert_eq!(
            text, "Первый абзац & продолжение\nВторой абзац документа",
            "paragraphs are separated by \\n, the trailing line break is trimmed: {text:?}"
        );
        // XML tags don't leak into the output.
        assert!(!text.contains("<w:"), "tags shouldn't leak: {text:?}");
        assert!(!text.contains("w:t"), "tags shouldn't leak: {text:?}");
    }

    #[test]
    fn extract_docx_handles_tab_and_break() {
        // A manual build with <w:tab/> and <w:br/> inside a run.
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
        // A valid ZIP, but without word/document.xml → an error (not a panic).
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        zip.start_file("other.txt", opts).unwrap();
        zip.write_all(b"nope").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        assert!(extract_docx(&bytes).is_err());
    }

    #[test]
    fn extract_docx_non_zip_bytes_error() {
        // Not a ZIP at all → an error (not a panic).
        assert!(extract_docx(b"this is not a zip archive at all").is_err());
        assert!(extract_docx(b"").is_err());
    }

    /// A minimal valid PDF with the text "Hello World" (a correct xref,
    /// the Helvetica font, a content stream `BT … Tj ET`). Verified against `pdf-extract`.
    const HELLO_PDF: &[u8] = include_bytes!("../../tests/fixtures/hello.pdf");

    #[test]
    fn extract_pdf_extracts_text_from_fixture() {
        let text = extract_pdf(HELLO_PDF).unwrap();
        assert!(
            text.contains("Hello World"),
            "the text \"Hello World\" should be extracted from the PDF fixture: {text:?}"
        );
    }

    #[test]
    fn extract_pdf_non_pdf_bytes_error() {
        // Not a PDF → an error (not a panic).
        assert!(extract_pdf(b"definitely not a pdf document").is_err());
    }
}
