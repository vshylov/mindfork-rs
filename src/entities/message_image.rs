//! An image attached to a message (spec §9.10, docs/research/multimodal-images.md).
//!
//! Unlike a file attachment ([`super::attachment::Attachment`], spec §9.7), which is
//! chat-scoped and re-injected into the **system prompt** on every turn, an image is
//! **message-scoped**: it belongs to the turn that introduced it and is replayed
//! afterwards as ordinary history. That is fork F1 of the research doc, and it is not a
//! stylistic choice — every provider takes images as *message* content parts only, and
//! an image that could be added to or removed from the head of the prompt would
//! invalidate the prefix cache on every mutation (measured: the append-only shape keeps
//! `cache_n` at 73 of 78 tokens across an image turn).
//!
//! The payload is stored **base64 inside the chat file** (fork F2), like the extracted
//! text of an attachment: the chat stays self-contained for backup/export, and building a
//! request does no I/O. What makes that affordable is the downscale-on-attach in
//! [`crate::features::image_prepare`] — a prepared image is a few hundred KB, the same
//! order as the text snapshots the chat file already carries.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The patch size Anthropic bills by: an image costs `⌈w/28⌉ × ⌈h/28⌉` visual tokens.
/// Used for the *displayed* estimate only — the compaction trigger reads the server's
/// exact `usage.prompt_tokens`, which includes image tokens (measured, research §2.1).
/// Measured against the reference stack this over-estimates Gemma 4 (51–258 tokens) and
/// roughly matches Gemini/xAI (~256), which is the safe direction for a cost shown to the
/// user.
const PATCH_PX: u32 = 28;

/// An image attached to a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageImage {
    pub id: Uuid,
    /// Display name — the file name at attach time.
    pub name: String,
    /// The canonical path at attach time. The dedupe key while staged; kept afterwards
    /// so the feed can say where the image came from.
    pub source: String,
    pub added_at: DateTime<Utc>,
    /// The MIME type of [`data`](Self::data) — always `image/png` or `image/jpeg` after
    /// preparation, never the source format (xAI accepts only those two, so normalizing
    /// once at attach time keeps the provider matrix uniform).
    pub mime: String,
    pub width: u32,
    pub height: u32,
    /// The size of the encoded payload in bytes — what is actually re-sent every turn,
    /// not the size of the file on disk.
    pub bytes: usize,
    /// The encoded payload, base64 (no `data:` prefix — each wire format wants it
    /// differently, and Anthropic/Gemini want the bare string).
    pub data: String,
}

impl MessageImage {
    pub fn new(
        name: impl Into<String>,
        source: impl Into<String>,
        mime: impl Into<String>,
        width: u32,
        height: u32,
        data: String,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            source: source.into(),
            added_at: Utc::now(),
            mime: mime.into(),
            width,
            height,
            bytes: base64_decoded_len(&data),
            data,
        }
    }

    /// The estimated prompt cost of the image, in tokens (see [`PATCH_PX`]).
    pub fn est_tokens(&self) -> usize {
        let w = self.width.div_ceil(PATCH_PX) as usize;
        let h = self.height.div_ceil(PATCH_PX) as usize;
        w * h
    }

    /// Case-insensitive match on the display name or the full source path — the same
    /// contract as [`Attachment::matches`](super::attachment::Attachment::matches), so
    /// `/image remove` accepts what `/image list` shows (Windows path casing included).
    /// The same contract means the same folding, which is
    /// [`same_name`](super::chat_file::same_name)'s and so is Unicode rather than ASCII.
    /// Trimming stays `resolve_handle`'s, as the note in `resolve_target_by_index_name_and_path`
    /// records.
    pub fn matches(&self, target: &str) -> bool {
        let same = super::chat_file::same_name;
        same(&self.name, target) || same(&self.source, target)
    }
}

/// The size of the payload the base64 string encodes, without decoding it.
fn base64_decoded_len(b64: &str) -> usize {
    let len = b64.len();
    if len == 0 {
        return 0;
    }
    let padding = b64.bytes().rev().take_while(|&c| c == b'=').count();
    len / 4 * 3 - padding
}

/// Resolves `#N` (1-based), a name or a path against a staged list — the one resolution
/// [`crate::features::file_command::resolve_target`] uses too, since the user sees the
/// same `#N` handles in both listings and a name two items share is refused in both
/// (docs/research/remove-by-shared-name.md).
pub fn resolve_target(items: &[MessageImage], target: &str) -> super::attachment::Resolved {
    super::attachment::resolve_handle(items, target, MessageImage::matches)
}

/// A card for the UI: everything the feed and the status bar need, and **not** the
/// payload. The same reasoning as `AttachmentInfo` — a `Vec<MessageImage>` sent through
/// the event channel would carry megabytes of base64 to a widget that renders a chip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    pub name: String,
    /// Where it came from — a path or an address. The listing shows it on a line whose
    /// name another staged image shares.
    pub source: String,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
    pub est_tokens: usize,
}

impl From<&MessageImage> for ImageInfo {
    fn from(image: &MessageImage) -> Self {
        Self {
            name: image.name.clone(),
            source: image.source.clone(),
            mime: image.mime.clone(),
            width: image.width,
            height: image.height,
            bytes: image.bytes,
            est_tokens: image.est_tokens(),
        }
    }
}

/// The cards for a set of images, in order.
pub fn infos(images: &[MessageImage]) -> Vec<ImageInfo> {
    images.iter().map(ImageInfo::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(name: &str, w: u32, h: u32) -> MessageImage {
        MessageImage::new(
            name,
            format!("D:\\pics\\{name}"),
            "image/png",
            w,
            h,
            "AAAA".to_string(),
        )
    }

    #[test]
    fn est_tokens_counts_patches_and_rounds_up() {
        // 28x28 is exactly one patch; 29 px needs a second one on that axis.
        assert_eq!(image("a.png", 28, 28).est_tokens(), 1);
        assert_eq!(image("a.png", 29, 28).est_tokens(), 2);
        // The downscale ceiling: 1568 px is 56 patches a side.
        assert_eq!(image("a.png", 1568, 1568).est_tokens(), 56 * 56);
    }

    #[test]
    fn decoded_len_accounts_for_padding() {
        // "AAAA" -> 3 bytes, "AAA=" -> 2, "AA==" -> 1, "" -> 0.
        assert_eq!(base64_decoded_len("AAAA"), 3);
        assert_eq!(base64_decoded_len("AAA="), 2);
        assert_eq!(base64_decoded_len("AA=="), 1);
        assert_eq!(base64_decoded_len(""), 0);
        // And the constructor uses it, so `bytes` is the payload, not the base64 length.
        assert_eq!(image("a.png", 8, 8).bytes, 3);
    }

    #[test]
    fn matches_by_name_or_path_case_insensitively() {
        let i = image("Chart.PNG", 8, 8);
        assert!(i.matches("chart.png"));
        assert!(i.matches("d:\\pics\\Chart.PNG"));
        assert!(!i.matches("other.png"));
        // Folded over Unicode, not ASCII: `ru` is a supported locale, and the name the
        // listing shows has to be the name the removal takes.
        assert!(image("Отчёт.png", 8, 8).matches("отчёт.png"));
    }

    #[test]
    fn resolve_target_by_index_name_and_path() {
        use crate::entities::attachment::Resolved;
        let items = vec![image("a.png", 8, 8), image("b.png", 8, 8)];
        assert_eq!(resolve_target(&items, "#1"), Resolved::One(0));
        assert_eq!(resolve_target(&items, "#2"), Resolved::One(1));
        assert_eq!(resolve_target(&items, "b.png"), Resolved::One(1));
        // `MessageImage::matches` compares the name as given, so the quotes a name with
        // spaces is typed in are the resolution's to strip — the orchestrator no longer does.
        assert_eq!(resolve_target(&items, "\"b.png\""), Resolved::One(1));
        assert_eq!(resolve_target(&items, "D:\\pics\\a.png"), Resolved::One(0));
        // Out of range and the underflow case.
        assert_eq!(resolve_target(&items, "#0"), Resolved::Nothing);
        assert_eq!(resolve_target(&items, "#3"), Resolved::Nothing);
        assert_eq!(resolve_target(&items, "nope.png"), Resolved::Nothing);
    }

    #[test]
    fn info_carries_the_card_but_not_the_payload() {
        let i = image("a.png", 56, 28);
        let info = ImageInfo::from(&i);
        assert_eq!(info.name, "a.png");
        assert_eq!((info.width, info.height), (56, 28));
        assert_eq!(info.est_tokens, 2);
    }

    #[test]
    fn serde_roundtrip() {
        let i = image("a.png", 56, 28);
        let json = serde_json::to_string(&i).unwrap();
        let back: MessageImage = serde_json::from_str(&json).unwrap();
        assert_eq!(i, back);
    }
}
