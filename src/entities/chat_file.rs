//! A file stored with a chat: bytes a `python_exec` call saved to `/w/out`, kept in the
//! chat's folder (`data/files/<chat-id>/`) for the user. See
//! docs/sandbox-file-exchange.md (F1, F2, F4, §11), spec §9.7.
//!
//! Unlike an [`Attachment`](super::attachment::Attachment), a stored file is not text the
//! model is shown on every turn: the chat lists it and the bytes live on disk. The
//! listing holds the file's **name only** — the folder is computed from the data root, so
//! a restored backup resolves on any machine (docs/lessons.md §6).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Longest name a stored file gets, in characters (F4): short enough that the data root,
/// `files/<chat-id>/` and the name stay inside a Windows path.
pub const MAX_NAME_CHARS: usize = 120;

/// Longest extension the length cap keeps whole (§11 S4); a longer "extension" is just
/// part of the name.
const MAX_EXT_CHARS: usize = 16;

/// Where a stored file came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOrigin {
    /// Written by a `python_exec` call into `/w/out`.
    Sandbox,
    /// Found in the chat's folder at startup with no listing — a file a call wrote whose
    /// chat was not saved before the app stopped. Adopted, never deleted (§11 S6).
    Recovered,
}

/// A file stored with a chat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatFile {
    pub id: Uuid,
    /// The file's name in the chat's folder: the handle `/file remove` takes and the name
    /// on disk at once (§11 S3). Unique within the chat, compared case-insensitively
    /// ([`same_name`]).
    pub name: String,
    pub origin: FileOrigin,
    pub mime: String,
    pub bytes: u64,
    /// Lowercase hex SHA-256 of the content — an output with a listed name and the same
    /// bytes is not stored twice (F4).
    pub sha256: String,
    pub added_at: DateTime<Utc>,
}

impl ChatFile {
    /// A listing for `content` stored as `name` (already sanitized and versioned). Tests
    /// only: the store builds its listings from a hash it has already taken, streamed for a
    /// file it adopts (`features::chat_files`).
    #[cfg(test)]
    pub fn new(name: impl Into<String>, origin: FileOrigin, content: &[u8]) -> Self {
        let name = name.into();
        Self {
            id: Uuid::new_v4(),
            mime: mime_for(&name, content).to_string(),
            name,
            origin,
            bytes: content.len() as u64,
            sha256: sha256_hex(content),
            added_at: Utc::now(),
        }
    }

    /// Whether a `/file remove` target names this file (case-insensitively, as names are
    /// unique).
    pub fn matches(&self, target: &str) -> bool {
        same_name(&self.name, target)
    }
}

/// Whether two stored names are the same name. Case-insensitive: a chat's folder may be
/// restored onto Windows, where `Chart.png` and `chart.png` are one file.
pub fn same_name(a: &str, b: &str) -> bool {
    a == b || a.to_lowercase() == b.to_lowercase()
}

/// The name a guest-chosen file name is stored under, or `None` when nothing usable is
/// left (F4, §11 S4). Only the last component survives, split on **both** separators; a
/// control character or one Windows refuses becomes `_`; trailing dots and spaces go; a
/// device name gets a `_` prefix; the result is at most [`MAX_NAME_CHARS`] characters,
/// keeping a short extension.
pub fn sanitize_name(raw: &str) -> Option<String> {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let replaced: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut name = replaced.trim_end_matches(['.', ' ']).to_string();
    if name.is_empty() {
        return None;
    }
    if is_device_name(&name) {
        name.insert(0, '_');
    }
    if name.chars().count() > MAX_NAME_CHARS {
        name = shorten(&name);
    }
    Some(name)
}

/// `CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9` — reserved on Windows whatever
/// follows the first dot, and in any case.
fn is_device_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end()
        .to_ascii_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        s if s.len() == 4 && (s.starts_with("COM") || s.starts_with("LPT")) => {
            matches!(s.as_bytes()[3], b'1'..=b'9')
        }
        _ => false,
    }
}

/// Cuts a long name to [`MAX_NAME_CHARS`], keeping an extension of up to
/// [`MAX_EXT_CHARS`].
fn shorten(name: &str) -> String {
    let (stem, ext) = split_ext(name);
    let ext_chars = ext.chars().count();
    if ext.is_empty() || ext_chars > MAX_EXT_CHARS {
        let cut: String = name.chars().take(MAX_NAME_CHARS).collect();
        return cut.trim_end_matches(['.', ' ']).to_string();
    }
    let keep = MAX_NAME_CHARS - ext_chars;
    let stem: String = stem.chars().take(keep).collect();
    format!("{}{ext}", stem.trim_end_matches(['.', ' ']))
}

/// Splits `name` into its stem and extension (with the dot) at the last dot; a name whose
/// only dot leads it (`.bashrc`) has no extension.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

/// The `n`-th name in a file's version family: `chart.png`, `chart (2).png`,
/// `chart (3).png`… (F2).
pub fn versioned(name: &str, n: u32) -> String {
    if n <= 1 {
        return name.to_string();
    }
    let (stem, ext) = split_ext(name);
    format!("{stem} ({n}){ext}")
}

/// The image type `content` actually is — by its first bytes, never its name — among
/// those the tool-image path can decode and show: PNG, JPEG, GIF, WebP, BMP (§11 S8).
pub fn sniff_image(content: &[u8]) -> Option<&'static str> {
    if content.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if content.starts_with(b"\xFF\xD8\xFF") {
        Some("image/jpeg")
    } else if content.starts_with(b"GIF87a") || content.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if content.len() >= 12 && &content[..4] == b"RIFF" && &content[8..12] == b"WEBP" {
        Some("image/webp")
    } else if content.len() >= 26 && content.starts_with(b"BM") {
        Some("image/bmp")
    } else {
        None
    }
}

/// The MIME type a stored file is listed with. An image or a PDF is recognised by its
/// bytes; everything else by its extension, and a name claiming an image its bytes are
/// not is listed as what it is — unknown.
pub fn mime_for(name: &str, content: &[u8]) -> &'static str {
    if let Some(image) = sniff_image(content) {
        return image;
    }
    if content.starts_with(b"%PDF-") {
        return "application/pdf";
    }
    let ext = split_ext(name)
        .1
        .trim_start_matches('.')
        .to_ascii_lowercase();
    match ext.as_str() {
        "svg" => "image/svg+xml",
        "csv" => "text/csv",
        "tsv" => "text/tab-separated-values",
        "json" => "application/json",
        "md" => "text/markdown",
        "txt" | "log" => "text/plain",
        "py" => "text/x-python",
        "html" | "htm" => "text/html",
        "xml" => "application/xml",
        "yaml" | "yml" => "application/yaml",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// Whether a stored file of this MIME type is text worth quoting the head of in a result
/// (F5 (c)). An SVG is XML but a drawing, so it is not.
pub fn is_text_like(mime: &str) -> bool {
    mime.starts_with("text/")
        || matches!(
            mime,
            "application/json" | "application/xml" | "application/yaml"
        )
}

/// Lowercase hex SHA-256 of `content`.
pub fn sha256_hex(content: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(content)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_an_ordinary_name() {
        assert_eq!(sanitize_name("chart.png").as_deref(), Some("chart.png"));
        assert_eq!(
            sanitize_name("отчёт 2025.xlsx").as_deref(),
            Some("отчёт 2025.xlsx")
        );
    }

    #[test]
    fn sanitize_takes_the_last_component_on_both_separators() {
        assert_eq!(sanitize_name("a/b/c.png").as_deref(), Some("c.png"));
        // A Windows-shaped path, read on any host — `Path` would keep it whole on Linux.
        assert_eq!(
            sanitize_name(r"C:\Users\x\chart.png").as_deref(),
            Some("chart.png")
        );
        assert_eq!(
            sanitize_name(r"..\..\evil.txt").as_deref(),
            Some("evil.txt")
        );
    }

    #[test]
    fn sanitize_refuses_names_with_nothing_left() {
        for raw in ["", ".", "..", "...", "dir/", r"dir\", " . ", "a/.."] {
            assert_eq!(sanitize_name(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn sanitize_replaces_characters_windows_refuses_and_control_characters() {
        assert_eq!(sanitize_name("a<b>:c.png").as_deref(), Some("a_b__c.png"));
        assert_eq!(sanitize_name("q?\"|*.txt").as_deref(), Some("q____.txt"));
        assert_eq!(
            sanitize_name("tab\there\n.txt").as_deref(),
            Some("tab_here_.txt")
        );
    }

    #[test]
    fn sanitize_trims_trailing_dots_and_spaces() {
        assert_eq!(sanitize_name("name. . ").as_deref(), Some("name"));
        assert_eq!(sanitize_name("report.csv.").as_deref(), Some("report.csv"));
    }

    #[test]
    fn sanitize_prefixes_device_names_whatever_the_extension_and_case() {
        assert_eq!(sanitize_name("CON").as_deref(), Some("_CON"));
        assert_eq!(sanitize_name("con.txt").as_deref(), Some("_con.txt"));
        assert_eq!(sanitize_name("Nul.tar.gz").as_deref(), Some("_Nul.tar.gz"));
        assert_eq!(sanitize_name("COM1.csv").as_deref(), Some("_COM1.csv"));
        assert_eq!(sanitize_name("lpt9").as_deref(), Some("_lpt9"));
        // Not device names: a longer stem, `COM0`, a device name inside a word.
        assert_eq!(sanitize_name("COM10.txt").as_deref(), Some("COM10.txt"));
        assert_eq!(sanitize_name("COM0").as_deref(), Some("COM0"));
        assert_eq!(sanitize_name("console.log").as_deref(), Some("console.log"));
    }

    #[test]
    fn sanitize_caps_the_length_keeping_a_short_extension() {
        let long = format!("{}.png", "a".repeat(200));
        let name = sanitize_name(&long).unwrap();
        assert_eq!(name.chars().count(), MAX_NAME_CHARS);
        assert!(name.ends_with(".png"), "{name}");
        // Counted in characters, not bytes.
        let cyrillic = format!("{}.csv", "ж".repeat(150));
        let name = sanitize_name(&cyrillic).unwrap();
        assert_eq!(name.chars().count(), MAX_NAME_CHARS);
        assert!(name.ends_with(".csv"));
        // An "extension" too long to be one is cut with the rest.
        let odd = format!("x.{}", "e".repeat(150));
        assert_eq!(sanitize_name(&odd).unwrap().chars().count(), MAX_NAME_CHARS);
    }

    #[test]
    fn versioned_names_number_before_the_extension() {
        assert_eq!(versioned("chart.png", 1), "chart.png");
        assert_eq!(versioned("chart.png", 2), "chart (2).png");
        assert_eq!(versioned("notes", 3), "notes (3)");
        assert_eq!(versioned(".bashrc", 2), ".bashrc (2)");
        assert_eq!(versioned("archive.tar.gz", 2), "archive.tar (2).gz");
    }

    #[test]
    fn names_compare_case_insensitively() {
        assert!(same_name("Chart.PNG", "chart.png"));
        assert!(same_name("Отчёт.csv", "отчёт.csv"));
        assert!(!same_name("chart.png", "chart (2).png"));
    }

    #[test]
    fn images_are_recognised_by_their_bytes_not_their_name() {
        assert_eq!(sniff_image(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(sniff_image(b"\xFF\xD8\xFF\xE0...."), Some("image/jpeg"));
        assert_eq!(sniff_image(b"GIF89a...."), Some("image/gif"));
        assert_eq!(sniff_image(b"RIFF\x10\0\0\0WEBPVP8 "), Some("image/webp"));
        let mut bmp = b"BM".to_vec();
        bmp.resize(64, 0);
        assert_eq!(sniff_image(&bmp), Some("image/bmp"));
        assert_eq!(sniff_image(b"BM short"), None);
        assert_eq!(sniff_image(b"month,total\n"), None);
        // A text file named like a PNG is not listed as one.
        assert_eq!(
            mime_for("chart.png", b"not an image"),
            "application/octet-stream"
        );
        assert_eq!(mime_for("photo.bin", b"\xFF\xD8\xFF\xE0"), "image/jpeg");
    }

    #[test]
    fn mime_types_by_extension_and_what_counts_as_text() {
        assert_eq!(mime_for("totals.CSV", b"a,b"), "text/csv");
        assert_eq!(mime_for("drawing.svg", b"<svg/>"), "image/svg+xml");
        assert_eq!(mime_for("report.pdf", b"%PDF-1.7"), "application/pdf");
        assert_eq!(
            mime_for("book.xlsx", b"PK\x03\x04"),
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        );
        assert_eq!(mime_for("blob", b"\0\0"), "application/octet-stream");
        assert!(is_text_like("text/csv"));
        assert!(is_text_like("application/json"));
        assert!(!is_text_like("image/svg+xml"));
        assert!(!is_text_like("application/pdf"));
    }

    #[test]
    fn sha256_is_lowercase_hex() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_listing_round_trips_and_names_its_origin() {
        let file = ChatFile::new("chart.png", FileOrigin::Sandbox, b"\x89PNG\r\n\x1a\nxx");
        assert_eq!(file.mime, "image/png");
        assert_eq!(file.bytes, 10);
        let json = serde_json::to_string(&file).unwrap();
        assert!(json.contains("\"origin\":\"sandbox\""), "{json}");
        let back: ChatFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back, file);
        assert!(file.matches("CHART.png"));
        assert!(!file.matches("chart"));
    }
}
