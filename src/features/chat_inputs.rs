//! What a chat can hand to `python_exec` — its attachments, its stored files and its
//! images, as **one numbered list** (docs/history/sandbox-file-exchange.md §12 T2).
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
    /// What this item **is**, across a re-derivation of the list: the attachment's, the
    /// stored file's or the image's own id. The one field [`reconcile`] matches on, so a
    /// number promised to the model keeps meaning the same thing even when the list
    /// underneath it grows or reorders.
    pub id: uuid::Uuid,
    /// The `#N` of `/file list`, of the pinned block and of `files`. 1-based, and on a
    /// freshly built list it is the position — but never assume that: within a turn it is
    /// carried by [`reconcile`], so it outlives the position it was born at, and a gap is
    /// possible when an item leaves mid-turn. Resolve it with [`resolve`], not by index.
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
        let same = crate::entities::chat_file::same_name;
        same(&self.name, t) || same(&self.source, t)
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
    let mut items = derive(attachments, files, images, dir);
    number(&mut items);
    items
}

/// The same list, re-derived from the chat as it is **now**, carrying the promises the
/// turn has already made: an item `previous` numbered keeps its `#N` **and** its `/w/in`
/// name, and whatever arrived since is appended with fresh ones.
///
/// This is what makes the pinned block true for a whole turn (fork F12,
/// docs/history/sandbox-file-exchange.md §12 T2–T3). The block is written once, before the
/// model writes any code; the list under it then grows every round — `fetch_url` lands an
/// attachment, a `python_exec` call stores its chart — and a plain re-derivation would
/// renumber everything after the insertion point and re-version the staged names with it.
/// The model would then read `#2 sales.csv` from the block, name `#2`, and be handed
/// whatever slid into that slot, or open `/w/in/notes (2).md` that is now `notes (3).md`.
///
/// Re-derived rather than frozen on purpose: `attachment` and `image` are positions in the
/// live context, so a frozen copy would go stale exactly when `sync_attachments` reorders.
/// Only the two fields the model was *told* are carried.
///
/// A number is never reused: it counts on from the highest the turn has issued, so a
/// handle for an item that has since left the chat misses rather than landing on a
/// newcomer.
pub fn reconcile(
    previous: &[ChatInput],
    attachments: &[Attachment],
    files: &[ChatFile],
    images: &[&MessageImage],
    dir: &Path,
) -> Vec<ChatInput> {
    let mut out = derive(attachments, files, images, dir);
    let mut taken: Vec<String> = Vec::new();
    let mut next = previous.iter().map(|p| p.handle).max().unwrap_or(0);
    for item in out.iter_mut() {
        if let Some(p) = previous.iter().find(|p| p.id == item.id) {
            item.handle = p.handle;
            item.staged = p.staged.clone();
            taken.push(p.staged.to_lowercase());
        }
    }
    for item in out.iter_mut().filter(|i| i.handle == 0) {
        next += 1;
        item.handle = next;
        item.staged = unique_staged(&item.staged, next, &mut taken);
    }
    // What the turn already promised keeps its order; the newcomers follow, in the order
    // they arrived — which is the order their own results announced them in.
    out.sort_by_key(|i| i.handle);
    out
}

/// The three groups in listing order, unnumbered: attachments, the stored files no
/// attachment links, then the images.
fn derive(
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
            id: a.id,
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
            id: f.id,
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
            id: im.id,
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
    items
}

/// Resolves a handle — `#N`, a name or a path — against the list, refusing a name several
/// items share the way `/file remove` does (docs/research/remove-by-shared-name.md).
///
/// `#N` is matched against the handle the model was **given**, never against the position:
/// inside a turn the two part company (see [`reconcile`]), and a number for an item that
/// has left has to miss rather than land on whoever took its place.
pub fn resolve(items: &[ChatInput], target: &str) -> Resolved {
    let target = target.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    if let Some(digits) = target.strip_prefix('#')
        && let Ok(n) = digits.trim().parse::<usize>()
    {
        return match items.iter().position(|i| i.handle == n) {
            Some(at) => Resolved::One(at),
            None => Resolved::Nothing,
        };
    }
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
pub fn named_files(args: &serde_json::Value) -> NamedFiles {
    use serde_json::Value;

    let Some(value) = args.get("files") else {
        return NamedFiles::Named(Vec::new());
    };
    match value {
        Value::Null => NamedFiles::Named(Vec::new()),
        // One name where a list was asked for. Models emit this routinely and the intent is
        // not in doubt, so it is taken rather than refused: a round spent teaching JSON is
        // a round the user pays for.
        Value::String(one) if !one.trim().is_empty() => NamedFiles::Named(vec![one.clone()]),
        Value::String(_) => NamedFiles::Named(Vec::new()),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item.as_str() {
                    // A non-string element is not a handle, and quietly dropping it would
                    // stage three of the four files the call asked for — the very thing §12
                    // T7 refuses to do for every other cause.
                    None => return NamedFiles::Malformed,
                    Some(handle) if handle.trim().is_empty() => continue,
                    Some(handle) => out.push(handle.to_string()),
                }
            }
            NamedFiles::Named(out)
        }
        _ => NamedFiles::Malformed,
    }
}

/// What a call's `files` argument names — or that it cannot be read at all.
///
/// One reading for the tool and for the confirmation popup, because two would let the popup
/// state a set the call does not stage (§12 T6). Before this existed both took
/// `as_array().unwrap_or_default()`, so `"files": "sales.csv"` silently became *no files*:
/// nothing staged, nothing refused, the code run against an empty `in/` and the model left
/// to read a `FileNotFoundError` and try the same shape again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamedFiles {
    /// The handles, in the order the call gave them; empty when the argument is absent.
    Named(Vec<String>),
    /// Present, and not a list of names. The call is refused rather than run with nothing.
    Malformed,
}

/// Fills in the handles and makes every staged name valid and unique: sanitized, and
/// versioned case-insensitively against the names already taken — two `notes.md` from two
/// folders become `notes.md` and `notes (2).md`. A name nothing survives sanitizing (`..`,
/// a lone separator) falls back to the handle, so every item has a name code can open.
fn number(items: &mut [ChatInput]) {
    let mut taken: Vec<String> = Vec::new();
    for (i, item) in items.iter_mut().enumerate() {
        item.handle = i + 1;
        item.staged = unique_staged(&item.staged, item.handle, &mut taken);
    }
}

/// One staged name, valid and unused: sanitized, then versioned case-insensitively against
/// `taken`, which it grows. `handle` only feeds the fallback name.
fn unique_staged(raw: &str, handle: usize, taken: &mut Vec<String>) -> String {
    let base = sanitize_name(raw).unwrap_or_else(|| format!("file-{handle}"));
    let name = (1..)
        .map(|n| versioned(&base, n))
        .find(|candidate| !taken.contains(&candidate.to_lowercase()))
        .unwrap_or(base);
    taken.push(name.to_lowercase());
    name
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

    /// The fold is Unicode's, not ASCII's. `ru` is a fully supported locale, so a name
    /// the listing shows capitalised is typed back in lower case — and the ASCII fold
    /// answered `Nothing`, which reads as "no such file" for a file that is right there.
    /// The shared-name refusal folds the same way, or two names that are one name to the
    /// user would resolve to two different items without a word about it.
    #[test]
    fn a_handle_folds_case_outside_ascii_too() {
        let list = items(
            &[attached("Отчёт.csv", "C:\\a\\Отчёт.csv")],
            &[stored("Диаграмма.png")],
            &[],
            dir(),
        );
        assert_eq!(resolve(&list, "отчёт.csv"), Resolved::One(0));
        assert_eq!(resolve(&list, "ДИАГРАММА.PNG"), Resolved::One(1));
        assert_eq!(resolve(&list, "c:\\a\\отчёт.csv"), Resolved::One(0));
        // ...and a name two items share is still one name, folded the same way.
        let shared = items(
            &[
                attached("Отчёт.csv", "C:\\a\\Отчёт.csv"),
                attached("отчёт.csv", "C:\\b\\отчёт.csv"),
            ],
            &[],
            &[],
            dir(),
        );
        assert_eq!(resolve(&shared, "отчёт.csv"), Resolved::Shared(vec![0, 1]));
    }

    /// Fork F12, §12 T2: the number the pinned block gave the model has to mean the same
    /// item for the whole turn. The block is written once, before the model writes a line
    /// of code; the list under it then grows every round.
    #[test]
    fn a_number_the_block_promised_survives_what_lands_mid_turn() {
        let shot = [image("shot.png", "image/png")];
        let notes = attached("notes.md", "C:\\notes.md");
        let sales = stored("sales.csv");
        let block = items(
            std::slice::from_ref(&notes),
            std::slice::from_ref(&sales),
            &shown(&shot),
            dir(),
        );
        assert_eq!(
            (block[0].handle, block[1].handle, block[2].handle),
            (1, 2, 3)
        );

        // Round 1: `fetch_url` lands an attachment — `sync_attachments` pushes it onto the
        // end of the attachments, which in a freshly derived list sits *before* every
        // stored file and every image.
        let page = attached("A page", "https://example.com/a");
        let after = reconcile(
            &block,
            &[notes.clone(), page.clone()],
            std::slice::from_ref(&sales),
            &shown(&shot),
            dir(),
        );

        let by_handle = |n: usize| {
            let Resolved::One(at) = resolve(&after, &format!("#{n}")) else {
                panic!("#{n} resolves to nothing");
            };
            after[at].name.clone()
        };
        assert_eq!(by_handle(1), "notes.md");
        assert_eq!(by_handle(2), "sales.csv", "a stored file must not slide");
        assert_eq!(by_handle(3), "shot.png", "an image must not slide either");
        assert_eq!(
            by_handle(4),
            "A page",
            "the newcomer is numbered after them"
        );
    }

    /// The other half of the same promise (§12 T3): the model writes `/w/in/<name>` into
    /// its code before any result exists, so the staged name cannot move either — and it
    /// is versioned by list position, which is exactly what an insertion changes.
    #[test]
    fn a_staged_name_the_block_promised_survives_too() {
        let a = attached("notes.md", "C:\\a\\notes.md");
        let b = attached("notes.md", "C:\\b\\notes.md");
        let block = items(&[a.clone(), b.clone()], &[], &[], dir());
        assert_eq!(block[0].staged, "notes.md");
        assert_eq!(block[1].staged, "notes (2).md");

        // A third `notes.md` arrives and sorts ahead of `b` — a plain re-derivation would
        // hand `b` the name `notes (3).md` while the block still says `notes (2).md`.
        let c = attached("notes.md", "C:\\c\\notes.md");
        let after = reconcile(&block, &[a, c, b], &[], &[], dir());
        let staged = |source: &str| {
            let Resolved::One(at) = resolve(&after, source) else {
                panic!("{source} resolves to nothing");
            };
            after[at].staged.clone()
        };
        assert_eq!(staged("C:\\a\\notes.md"), "notes.md");
        assert_eq!(staged("C:\\b\\notes.md"), "notes (2).md");
        assert_eq!(
            staged("C:\\c\\notes.md"),
            "notes (3).md",
            "the newcomer takes a name neither of the promised two holds"
        );
    }

    /// `sync_attachments` re-fetching one source does `retain` + `push`: the item moves to
    /// the end of the attachments without the list changing length, so a position-based
    /// number would change meaning while nothing was added at all.
    #[test]
    fn an_attachment_that_moved_keeps_the_number_it_was_given() {
        let first = attached("notes.md", "C:\\notes.md");
        let page = attached("A page", "https://example.com/a");
        let block = items(&[first.clone(), page.clone()], &[], &[], dir());
        assert_eq!(block[0].name, "notes.md");

        let after = reconcile(&block, &[page, first], &[], &[], dir());
        let Resolved::One(at) = resolve(&after, "#1") else {
            panic!("#1 resolves to nothing");
        };
        assert_eq!(after[at].name, "notes.md");
    }

    /// One reading for the tool and the popup, and it has to tell "names nothing" from
    /// "cannot be read". Taking `as_array().unwrap_or_default()` made the two the same
    /// answer, so a single name — the shape models emit most often — silently staged
    /// nothing and the code met an empty `in/`.
    #[test]
    fn the_files_argument_is_read_once_and_says_when_it_cannot_be() {
        use serde_json::json;

        let named = |v: serde_json::Value| named_files(&v);
        // Absent, null and empty all name nothing, and that is not an error.
        assert_eq!(named(json!({"code": "x"})), NamedFiles::Named(vec![]));
        assert_eq!(named(json!({"files": null})), NamedFiles::Named(vec![]));
        assert_eq!(named(json!({"files": []})), NamedFiles::Named(vec![]));
        assert_eq!(named(json!({"files": "  "})), NamedFiles::Named(vec![]));

        assert_eq!(
            named(json!({"files": ["sales.csv", "#2"]})),
            NamedFiles::Named(vec!["sales.csv".into(), "#2".into()])
        );
        // A single name where a list was asked for: the intent is not in doubt, so it is
        // taken rather than costing the user a round.
        assert_eq!(
            named(json!({"files": "sales.csv"})),
            NamedFiles::Named(vec!["sales.csv".into()])
        );
        // Blank entries are dropped; they name nothing and refusing the call over one would
        // be harsher than the handle deserves.
        assert_eq!(
            named(json!({"files": ["sales.csv", "", "  "]})),
            NamedFiles::Named(vec!["sales.csv".into()])
        );

        // A non-string element **is** a refusal: dropping it would stage three of the four
        // files the call asked for, which is what §12 T7 exists to prevent.
        assert_eq!(named(json!({"files": ["a", 3]})), NamedFiles::Malformed);
        assert_eq!(named(json!({"files": 7})), NamedFiles::Malformed);
        assert_eq!(named(json!({"files": {"a": 1}})), NamedFiles::Malformed);
    }

    /// A number is never handed on. An item that leaves mid-turn takes its handle with it,
    /// so a model still holding that `#N` misses — rather than being quietly given
    /// whatever arrived next.
    #[test]
    fn a_number_is_never_reused_by_a_newcomer() {
        let notes = attached("notes.md", "C:\\notes.md");
        let sales = stored("sales.csv");
        let block = items(
            std::slice::from_ref(&notes),
            std::slice::from_ref(&sales),
            &[],
            dir(),
        );
        assert_eq!(block[1].handle, 2);

        // `sales.csv` is gone and a chart arrives in the same round.
        let after = reconcile(
            &block,
            std::slice::from_ref(&notes),
            &[stored("chart.png")],
            &[],
            dir(),
        );
        assert_eq!(resolve(&after, "#2"), Resolved::Nothing);
        let Resolved::One(at) = resolve(&after, "#3") else {
            panic!("#3 resolves to nothing");
        };
        assert_eq!(after[at].name, "chart.png");
    }
}
