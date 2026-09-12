//! What a chat can hand to `python_exec` — its attachments, its stored files and its
//! images, as **one numbered list** (docs/sandbox-file-exchange.md §12 T2).
//!
//! One list, four readers: the pinned block that tells the model what it may name, the
//! resolver behind the tool's `files` argument, the confirmation popup's line, and the
//! staging that writes the bytes into `/w/in`. None of them can disagree about what `#3`
//! is, which is the whole point of the popup. `/file list` and `/file remove` number the
//! same list (spec §9.7), so the `#N` the user sees is the `#N` the model names.
//!
//! Two things are decided here rather than at staging time:
//!
//! - **the name a file gets in `/w/in`** (§12 T3) — the model writes `/w/in/<name>` into
//!   its code *before* any result exists, so a name chosen while copying could never reach
//!   it. Names are sanitized and made unique across the whole list, and the block shows
//!   them;
//! - **a linked pair is one item** (§12 T9) — an attached document keeps its original
//!   beside the extracted text, and if the two were listed apart, the very name the user
//!   typed would resolve to two items and be refused by our own shared-name rule.
//!
//! Pure: no I/O and no localization. The tool renders its refusals in the agent's
//! language (axis A), the screen renders the popup in the interface's (axis B).

use std::path::Path;

use crate::entities::attachment::{Attachment, Resolved, resolve_handle};
use crate::entities::chat_file::{ChatFile, mime_for, sanitize_name, versioned};
use crate::entities::message_image::MessageImage;

/// One thing a call can name in `files`.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatInput {
    /// 1-based position — the `#N` of `/file list`, of the pinned block and of `files`.
    pub handle: usize,
    /// The item's own name, as the listings show it.
    pub name: String,
    /// Where it came from: an attachment's path, a stored file's place in the chat's
    /// folder, an image's source. Shown on a line whose name another item shares, and
    /// matched by `/file remove <path>`.
    pub source: String,
    /// The name this item gets in `/w/in` — unique across the list.
    pub staged: String,
    /// The size of what is staged.
    pub bytes: u64,
    pub mime: String,
    /// Position in the chat's attachments, when the item has one.
    pub attachment: Option<usize>,
    /// The name in the chat's folder, when the item has a stored file: an output of a
    /// call, or the original of an attached document.
    pub file: Option<String>,
    /// Position in the chat's images, when the item is one.
    pub image: Option<usize>,
}

impl ChatInput {
    /// Does a handle typed as a name or a path reach this item? The same contract
    /// [`Attachment::matches`] has, so a listing's line and a removal accept the same text.
    pub fn matches(&self, target: &str) -> bool {
        let t = target.trim().trim_matches(|c| c == '"' || c == '\'');
        self.name.eq_ignore_ascii_case(t) || self.source.eq_ignore_ascii_case(t)
    }

    /// Whether the item is one of the chat's images.
    pub fn is_image(&self) -> bool {
        self.image.is_some()
    }
}

/// The chat's files as one numbered list: attachments, then the stored files no
/// attachment links, then the images. `dir` is the chat's stored-files folder — where a
/// stored file's `source` points; nothing is read from it here.
pub fn items(
    attachments: &[Attachment],
    files: &[ChatFile],
    images: &[&MessageImage],
    dir: &Path,
) -> Vec<ChatInput> {
    let mut items: Vec<ChatInput> = Vec::new();
    for (i, a) in attachments.iter().enumerate() {
        // The original, when `/file attach` kept one beside the extracted text (F8a).
        let linked = a.file_id.and_then(|id| files.iter().find(|f| f.id == id));
        items.push(ChatInput {
            handle: 0,
            name: a.name.clone(),
            source: a.source.clone(),
            // A pair stages its original — that is the file the name means.
            staged: match linked {
                Some(f) => f.name.clone(),
                None => text_name(&a.name),
            },
            bytes: linked.map_or(a.bytes as u64, |f| f.bytes),
            mime: linked.map_or_else(|| text_mime(&a.name).to_string(), |f| f.mime.clone()),
            attachment: Some(i),
            file: linked.map(|f| f.name.clone()),
            image: None,
        });
    }
    let linked: Vec<uuid::Uuid> = attachments.iter().filter_map(|a| a.file_id).collect();
    for f in files.iter().filter(|f| !linked.contains(&f.id)) {
        items.push(ChatInput {
            handle: 0,
            name: f.name.clone(),
            source: dir.join(&f.name).display().to_string(),
            staged: f.name.clone(),
            bytes: f.bytes,
            mime: f.mime.clone(),
            attachment: None,
            file: Some(f.name.clone()),
            image: None,
        });
    }
    for (i, im) in images.iter().enumerate() {
        items.push(ChatInput {
            handle: 0,
            name: im.name.clone(),
            source: im.source.clone(),
            staged: image_name(&im.name, &im.mime),
            bytes: im.bytes as u64,
            mime: im.mime.clone(),
            attachment: None,
            file: None,
            image: Some(i),
        });
    }
    number(&mut items);
    items
}

/// Resolves a handle — `#N`, a name or a path — against the list, refusing a name several
/// items share the way `/file remove` does (docs/research/remove-by-shared-name.md).
pub fn resolve(items: &[ChatInput], target: &str) -> Resolved {
    resolve_handle(items, target, ChatInput::matches)
}

/// The path a handle means on disk, for `/file open` (fork F9, §13 U2). Pure — the caller
/// checks the path once and words the refusal.
///
/// The chat's own copy for a stored file **and** for an attached document that kept its
/// original: our copy is the half guaranteed to be there, while the user's path may have
/// moved since the attach. Everything else means its `source` — a plain text attachment's
/// own file, an image's. That source is not always a file, and nothing here pretends
/// otherwise: a pasted image's is `clipboard:<uuid>`, a fetched page's attachment carries
/// a URL, and a listed copy can be gone from the folder.
pub fn open_path(item: &ChatInput, dir: &Path) -> std::path::PathBuf {
    match &item.file {
        Some(name) => dir.join(name),
        None => std::path::PathBuf::from(&item.source),
    }
}

/// What a `python_exec` call would copy into `/w/in`, resolved for the confirmation popup
/// (§12 T6). The popup presents a call's arguments compactly and drops arrays outright, so
/// the one argument that decides what leaves the chat would otherwise not be shown at all
/// — and it is resolved here, with the list and the rule the call itself uses, so the two
/// cannot show different sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmInputs {
    /// One entry per handle the call named, in the order it named them.
    pub files: Vec<ConfirmFile>,
    /// Whether the sandbox has network access for this call — the other half of what the
    /// user is consenting to.
    pub net: bool,
}

/// One handle a call named, as the popup shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmFile {
    /// The handle as the call wrote it — `#3`, or a name.
    pub handle: String,
    /// The file it reaches and that file's size. `None` when the handle reaches nothing,
    /// or a name several files share: the call will refuse, and the popup says so rather
    /// than showing a file that is not going anywhere.
    pub resolved: Option<(String, u64)>,
}

/// Resolves the handles a call named, for the popup.
pub fn for_confirm(items: &[ChatInput], handles: &[String], net: bool) -> ConfirmInputs {
    let files = handles
        .iter()
        .map(|handle| ConfirmFile {
            handle: handle.trim().to_string(),
            resolved: match resolve(items, handle) {
                Resolved::One(at) => Some((items[at].name.clone(), items[at].bytes)),
                Resolved::Shared(_) | Resolved::Nothing => None,
            },
        })
        .collect();
    ConfirmInputs { files, net }
}

/// The file handles a call names in its `files` argument. One rule, two readers: the tool
/// reads the parsed arguments, the popup the same value off the wire.
pub fn named_files(args: &serde_json::Value) -> Vec<String> {
    args.get("files")
        .and_then(|v| v.as_array())
        .map(|named| {
            named
                .iter()
                .filter_map(|f| f.as_str())
                .filter(|f| !f.trim().is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Fills in the handles and makes every staged name valid and unique: sanitized, and
/// versioned case-insensitively against the names already taken — two `notes.md` from two
/// folders become `notes.md` and `notes (2).md`. A name nothing survives sanitizing (`..`,
/// a lone separator) falls back to the handle, so every item has a name code can open.
fn number(items: &mut [ChatInput]) {
    let mut taken: Vec<String> = Vec::new();
    for (i, item) in items.iter_mut().enumerate() {
        item.handle = i + 1;
        let base = sanitize_name(&item.staged).unwrap_or_else(|| format!("file-{}", i + 1));
        let name = (1..)
            .map(|n| versioned(&base, n))
            .find(|candidate| {
                let lower = candidate.to_lowercase();
                !taken.contains(&lower)
            })
            .unwrap_or(base);
        taken.push(name.to_lowercase());
        item.staged = name;
    }
}

/// The name an attachment's **text** is staged under. It gains `.txt` exactly where the
/// text is not the file — a document an extractor read (pdf, docx, html), or a name with
/// no extension at all, which a tool-produced attachment often has; `main.rs` and
/// `notes.md` keep their own names (§12 T3).
fn text_name(name: &str) -> String {
    match extension(name) {
        Some(ext) if EXTRACTED.contains(&ext.as_str()) => format!("{name}.txt"),
        Some(_) => name.to_string(),
        None => format!("{name}.txt"),
    }
}

/// The MIME type an attachment's text is listed with: what its name claims, and
/// `text/plain` for a name that claims nothing — the staged bytes are text either way.
fn text_mime(name: &str) -> &'static str {
    match mime_for(name, b"") {
        "application/octet-stream" => "text/plain",
        mime => mime,
    }
}

/// The name a prepared image is staged under: its own, with the extension of the format
/// it was actually prepared into when the name does not already carry it (a `photo.heic`
/// normalized to PNG is staged as `photo.heic.png`).
fn image_name(name: &str, mime: &str) -> String {
    let ext = match mime {
        "image/jpeg" => "jpg",
        _ => "png",
    };
    match extension(name) {
        Some(have) if have == ext || (ext == "jpg" && have == "jpeg") => name.to_string(),
        _ => format!("{name}.{ext}"),
    }
}

/// The lowercase extension of a file name, without the dot; `None` when it has none.
fn extension(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let (stem, ext) = base.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then(|| ext.to_ascii_lowercase())
}

/// The extensions whose text comes from an extractor rather than from the file's own bytes
/// (`orchestrator::rag::read_source_text`).
const EXTRACTED: &[&str] = &["pdf", "docx", "html", "htm"];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::attachment::AttachMode;
    use crate::entities::chat_file::FileOrigin;

    fn attached(name: &str, source: &str) -> Attachment {
        Attachment::new(name, source, "text".into(), 4, AttachMode::Inline)
    }

    fn stored(name: &str) -> ChatFile {
        ChatFile::new(name, FileOrigin::Sandbox, b"bytes")
    }

    /// A stored original of the kind `/file attach` keeps beside extracted text: its type
    /// is read from its bytes, as `mime_for` reads every file's.
    fn stored_pdf(name: &str) -> ChatFile {
        ChatFile::new(
            name,
            FileOrigin::Attached,
            b"%PDF-1.7\nnot-really-a-document",
        )
    }

    fn image(name: &str, mime: &str) -> MessageImage {
        MessageImage::new(
            name,
            format!("C:\\pics\\{name}"),
            mime,
            10,
            10,
            "AAAA".into(),
        )
    }

    /// The images are borrowed, not cloned: a listing must not copy a chat's base64
    /// payloads to read their names.
    fn shown(images: &[MessageImage]) -> Vec<&MessageImage> {
        images.iter().collect()
    }

    fn dir() -> &'static Path {
        Path::new("C:\\data\\files\\chat")
    }

    /// §13 U2: what `/file open` hands to the shell — our copy where one exists, the
    /// item's own source otherwise, and no pretence that every source is a file.
    #[test]
    fn a_handle_opens_our_copy_when_there_is_one_and_the_source_otherwise() {
        let shot = [image("shot.png", "image/png")];
        let original = stored_pdf("report.pdf");
        let pair = attached("report.pdf", "D:\\downloads\\report.pdf").with_file(original.id);
        let list = items(
            &[pair, attached("notes.md", "C:\\notes.md")],
            &[original, stored("chart.png")],
            &shown(&shot),
            dir(),
        );
        // The pair opens the copy in the chat's folder, not the path it was attached
        // from: that one may have moved since, ours is always there.
        assert_eq!(open_path(&list[0], dir()), dir().join("report.pdf"));
        // Plain text keeps no second copy, so its own file is what opens.
        assert_eq!(open_path(&list[1], dir()), Path::new("C:\\notes.md"));
        assert_eq!(open_path(&list[2], dir()), dir().join("chart.png"));
        // An image means its source — a path here, `clipboard:<uuid>` for a paste, which
        // the caller's existence check is what refuses.
        assert_eq!(open_path(&list[3], dir()), Path::new("C:\\pics\\shot.png"));
    }

    #[test]
    fn the_three_kinds_are_numbered_in_one_list() {
        let shot = [image("shot.png", "image/png")];
        let list = items(
            &[attached("notes.md", "C:\\notes.md")],
            &[stored("chart.png")],
            &shown(&shot),
            dir(),
        );
        let handles: Vec<(usize, &str)> =
            list.iter().map(|i| (i.handle, i.staged.as_str())).collect();
        assert_eq!(
            handles,
            vec![(1, "notes.md"), (2, "chart.png"), (3, "shot.png")]
        );
        assert!(list[0].attachment.is_some() && list[0].file.is_none());
        assert_eq!(list[1].file.as_deref(), Some("chart.png"));
        assert!(list[2].is_image());
        // A stored file's source is where it lies — what `/file list` shows.
        assert!(list[1].source.ends_with("chart.png"));
    }

    #[test]
    fn an_attached_document_and_its_original_are_one_item_that_stages_the_original() {
        let original = stored_pdf("report.pdf");
        let mut att = attached("report.pdf", "C:\\report.pdf");
        att.file_id = Some(original.id);
        let list = items(&[att], &[original], &[], dir());
        assert_eq!(list.len(), 1, "the pair is one item: {list:?}");
        // Both halves on one item: the attachment's text and the original's bytes.
        assert!(list[0].attachment.is_some() && list[0].file.is_some());
        // The original, not `report.pdf.txt`: the name means the file.
        assert_eq!(list[0].staged, "report.pdf");
        assert_eq!(list[0].mime, "application/pdf");
    }

    #[test]
    fn an_extracted_document_without_its_original_is_staged_as_text() {
        let list = items(&[attached("report.pdf", "C:\\report.pdf")], &[], &[], dir());
        // What goes in is the extracted text, and the listing says so rather than
        // promising a PDF the guest would fail to open.
        assert_eq!(list[0].staged, "report.pdf.txt");
        assert_eq!(list[0].mime, "text/plain");
    }

    #[test]
    fn a_source_file_keeps_its_name_and_a_nameless_attachment_gains_txt() {
        let list = items(
            &[
                attached("main.rs", "C:\\src\\main.rs"),
                attached("Rust by Example", "https://example.com/page"),
            ],
            &[],
            &[],
            dir(),
        );
        assert_eq!(list[0].staged, "main.rs");
        assert_eq!(list[1].staged, "Rust by Example.txt");
        assert_eq!(list[0].mime, "text/plain");
    }

    #[test]
    fn a_name_two_items_share_is_versioned_once_for_the_guest() {
        let list = items(
            &[
                attached("notes.md", "C:\\a\\notes.md"),
                attached("notes.md", "C:\\b\\notes.md"),
            ],
            &[stored("notes.md")],
            &[],
            dir(),
        );
        let staged: Vec<&str> = list.iter().map(|i| i.staged.as_str()).collect();
        assert_eq!(staged, vec!["notes.md", "notes (2).md", "notes (3).md"]);
    }

    /// The fallback is defence in depth — a name reaching [`number`] has been through
    /// `text_name`/`image_name` first, which append an extension to a name that has none,
    /// so `..` arrives as `...txt`. Tested where it lives, since no item can carry the
    /// name that triggers it.
    #[test]
    fn a_name_that_survives_nothing_falls_back_to_its_handle() {
        let mut crafted = items(&[attached("notes.md", "C:\\a\\notes.md")], &[], &[], dir());
        crafted[0].staged = "..".into();
        number(&mut crafted);
        assert_eq!(crafted[0].staged, "file-1");
    }

    #[test]
    fn an_image_is_staged_under_the_format_it_was_prepared_into() {
        let pictures = [
            image("shot.png", "image/png"),
            image("photo.heic", "image/jpeg"),
            image("scan.JPEG", "image/jpeg"),
        ];
        let list = items(&[], &[], &shown(&pictures), dir());
        let staged: Vec<&str> = list.iter().map(|i| i.staged.as_str()).collect();
        assert_eq!(staged, vec!["shot.png", "photo.heic.jpg", "scan.JPEG"]);
    }

    #[test]
    fn a_handle_resolves_by_number_name_or_path_and_a_shared_name_refuses() {
        let list = items(
            &[
                attached("notes.md", "C:\\a\\notes.md"),
                attached("notes.md", "C:\\b\\notes.md"),
            ],
            &[stored("chart.png")],
            &[],
            dir(),
        );
        assert_eq!(resolve(&list, "#3"), Resolved::One(2));
        assert_eq!(resolve(&list, "chart.png"), Resolved::One(2));
        assert_eq!(resolve(&list, "C:\\b\\notes.md"), Resolved::One(1));
        assert_eq!(resolve(&list, "notes.md"), Resolved::Shared(vec![0, 1]));
        assert_eq!(resolve(&list, "#9"), Resolved::Nothing);
        assert_eq!(resolve(&list, "nothing.csv"), Resolved::Nothing);
        // The staged name is not a handle: `#2` and the path are what reach the second one.
        assert_eq!(resolve(&list, "notes (2).md"), Resolved::Nothing);
    }
}
