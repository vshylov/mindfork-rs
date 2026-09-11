//! The chat screen — attached files (`/file attach`): the feed note for a
//! command's outcome and the status-bar chip. Part of the [`super`] module.
//! See docs/file-attachments.md.

use super::*;
use crate::entities::attachment::format_bytes;
use crate::features::file_command::{FileProgress, StoredInfo, mode_label};

impl ChatScreen {
    /// Updates the active chat's attachment cards (`AppEvent::Attachments`) —
    /// the source for the status-bar chip.
    pub fn set_attachments(&mut self, items: Vec<AttachmentInfo>) {
        self.attachments = items;
    }

    /// Reports the outcome of a `/file` command as a note in the feed.
    pub fn set_file_progress(&mut self, progress: FileProgress) {
        match progress {
            FileProgress::Attached {
                info,
                total_tokens,
                read_as,
            } => {
                let mut msg = self.loc.tf(
                    "ui.file.attached",
                    &[
                        ("name", &info.name),
                        ("size", &format_bytes(info.bytes)),
                        ("tokens", &format_tokens(info.est_tokens)),
                        ("mode", mode_label(info.mode, self.loc)),
                        ("total", &format_tokens(total_tokens)),
                    ],
                );
                if let Some(encoding) = read_as {
                    msg.push(' ');
                    msg.push_str(&self.loc.tf("ui.file.read_as", &[("encoding", encoding)]));
                }
                self.push_note(&msg);
            }
            FileProgress::Removed { name, source } => {
                let msg = match source {
                    Some(source) => self.loc.tf(
                        "ui.file.removed_source",
                        &[("name", &name), ("source", &source)],
                    ),
                    None => self.loc.tf("ui.file.removed", &[("name", &name)]),
                };
                self.push_note(&msg);
            }
            FileProgress::RemovedStored { name } => {
                let msg = self.loc.tf("ui.file.removed_stored", &[("name", &name)]);
                self.push_note(&msg);
            }
            FileProgress::StoredFile {
                name,
                bytes,
                mime,
                dir,
            } => {
                let msg = self.loc.tf(
                    "ui.file.attached_stored",
                    &[
                        ("name", &name),
                        ("size", &format_bytes(bytes as usize)),
                        ("mime", &mime),
                        ("dir", &dir),
                    ],
                );
                self.push_note(&msg);
            }
            FileProgress::RemovedPair { name } => {
                let msg = self.loc.tf("ui.file.removed_pair", &[("name", &name)]);
                self.push_note(&msg);
            }
            FileProgress::Saved { names, dir } => {
                let msg = self.loc.tf(
                    "ui.file.saved",
                    &[("names", &names.join(", ")), ("dir", &dir)],
                );
                self.push_note(&msg);
            }
            FileProgress::Listed {
                items,
                stored,
                images,
                dir,
            } => {
                let text = format_file_list(&items, &stored, &images, &dir, self.loc);
                self.push_note(&text);
            }
            FileProgress::Indexing { name, done, total } => {
                // Reuses the background-indexing banner slot (see the `rag`
                // field): both are "a background index is being built", and they
                // don't overlap in practice.
                let banner = RagBanner::around_name(
                    self.loc,
                    "ui.file.indexing",
                    &name,
                    String::new(),
                    &[("done", &done.to_string()), ("total", &total.to_string())],
                );
                self.show_banner(banner);
            }
            FileProgress::Indexed { name, chunks } => {
                self.rag = None;
                let msg = self.loc.tf(
                    "ui.file.indexed",
                    &[("name", &name), ("chunks", &chunks.to_string())],
                );
                self.push_note(&msg);
            }
            FileProgress::IndexSkipped { name, reason } => {
                self.rag = None;
                // A note, not an error: reading the file page by page still
                // works, only search is unavailable.
                let msg = self.loc.tf(
                    "ui.file.index_skipped",
                    &[("name", &name), ("reason", &reason)],
                );
                self.push_note(&msg);
            }
            FileProgress::Failed(err) => {
                self.push_error(&self.loc.tf("ui.file.failed", &[("err", &err)]));
            }
        }
    }

    /// The status-bar chip label for attached files (`None` — nothing attached).
    /// Attachments cost tokens on **every** turn, so their presence and price
    /// have to be visible without opening anything.
    pub(super) fn attachments_hint(&self) -> Option<String> {
        if self.attachments.is_empty() {
            return None;
        }
        // The real standing cost: inline text in full plus by-reference
        // excerpts. Counting a by-reference file at full weight would overstate
        // wildly (a 418k-token file costs ~300); counting it as zero would
        // claim it is free, which it isn't.
        let cost: usize = self.attachments.iter().map(|a| a.prompt_tokens).sum();
        Some(self.loc.tf(
            "ui.file.chip",
            &[
                ("n", &self.attachments.len().to_string()),
                ("tokens", &format_tokens(cost)),
            ],
        ))
    }
}

/// A compact token count for the UI: `840`, `3.1k`, `12k`. Keeps the chip and
/// notes short — the exact number is never the point, the order of magnitude is.
pub(super) fn format_tokens(tokens: usize) -> String {
    if tokens < 1000 {
        return tokens.to_string();
    }
    let k = tokens as f64 / 1000.0;
    if k < 10.0 {
        format!("{k:.1}k")
    } else {
        format!("{k:.0}k")
    }
}

/// Formats the `/file list` reply: the attachments, then the stored files numbered on
/// from them — the same `#N` handles `/file remove` accepts
/// (docs/sandbox-file-exchange.md §11 S11).
pub(super) fn format_file_list(
    items: &[AttachmentInfo],
    stored: &[StoredInfo],
    images: &[crate::entities::message_image::ImageInfo],
    dir: &str,
    loc: &'static Locale,
) -> String {
    if stored.is_empty() && images.is_empty() {
        return format_attachments(items, loc);
    }
    let mut out = if items.is_empty() {
        String::new()
    } else {
        format!("{}\n", format_attachments(items, loc))
    };
    if !stored.is_empty() {
        let total: u64 = stored.iter().map(|f| f.bytes).sum();
        out.push_str(&loc.tf(
            "ui.file.stored_header",
            &[
                ("n", &stored.len().to_string()),
                ("size", &format_bytes(total as usize)),
                ("dir", dir),
            ],
        ));
        for (i, file) in stored.iter().enumerate() {
            let n = (items.len() + i + 1).to_string();
            let size = format_bytes(file.bytes as usize);
            out.push_str(&loc.tf(
                "ui.file.stored_item",
                &[
                    ("i", &n),
                    ("name", &file.name),
                    ("size", &size),
                    ("mime", &file.mime),
                ],
            ));
            if file.missing {
                out.push_str(loc.t("ui.file.stored_missing"));
            }
        }
    }
    // The images the conversation carries, numbered on from the rest: the code can read
    // one by that `#N` (docs/sandbox-file-exchange.md §12 T4), so the user sees the same
    // handles the model does.
    if !images.is_empty() {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&loc.tf("ui.file.images_header", &[("n", &images.len().to_string())]));
        for (i, image) in images.iter().enumerate() {
            let n = (items.len() + stored.len() + i + 1).to_string();
            let size = format_bytes(image.bytes);
            out.push_str(&loc.tf(
                "ui.file.image_item",
                &[
                    ("i", &n),
                    ("name", &image.name),
                    ("size", &size),
                    ("w", &image.width.to_string()),
                    ("h", &image.height.to_string()),
                ],
            ));
        }
    }
    out
}

/// Formats the attachments half of the `/file list` reply: a header plus one line per
/// attachment with its `#N` handle (the same handle `/file remove #N` accepts).
pub(super) fn format_attachments(items: &[AttachmentInfo], loc: &'static Locale) -> String {
    if items.is_empty() {
        return loc.t("ui.file.list_empty").to_string();
    }
    let cost: usize = items.iter().map(|a| a.prompt_tokens).sum();
    let mut out = loc.tf(
        "ui.file.list_header",
        &[
            ("n", &items.len().to_string()),
            ("total", &format_tokens(cost)),
        ],
    );
    for (i, a) in items.iter().enumerate() {
        let n = (i + 1).to_string();
        let size = format_bytes(a.bytes);
        let tokens = format_tokens(a.est_tokens);
        let mut args = vec![
            ("i", n.as_str()),
            ("name", a.name.as_str()),
            ("size", size.as_str()),
            ("tokens", tokens.as_str()),
            ("mode", mode_label(a.mode, loc)),
        ];
        // A name another attachment shares is told apart by its source — which is also
        // what `/file remove` accepts (docs/research/remove-by-shared-name.md F2a).
        let key =
            if crate::entities::attachment::name_is_shared(items, i, |other| other.name.as_str()) {
                args.push(("source", a.source.as_str()));
                "ui.file.list_item_source"
            } else {
                "ui.file.list_item"
            };
        out.push_str(&loc.tf(key, &args));
        // The chat also keeps this file's original bytes (F8a) — the half `python_exec`
        // reads, and the half a removal deletes from disk.
        if a.has_original {
            out.push_str(loc.t("ui.file.list_item_original"));
        }
    }
    out
}

#[cfg(test)]
mod stored_list_tests {
    use super::*;
    use crate::entities::attachment::AttachMode;

    #[test]
    fn stored_files_are_numbered_on_from_the_attachments_and_a_missing_one_is_marked() {
        let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
        let items = vec![AttachmentInfo {
            name: "notes.md".into(),
            source: "/tmp/notes.md".into(),
            bytes: 10,
            est_tokens: 3,
            prompt_tokens: 3,
            mode: AttachMode::Inline,
            has_original: false,
        }];
        let stored = vec![
            StoredInfo {
                name: "chart.png".into(),
                bytes: 2048,
                mime: "image/png".into(),
                missing: false,
            },
            StoredInfo {
                name: "gone.csv".into(),
                bytes: 10,
                mime: "text/csv".into(),
                missing: true,
            },
        ];
        let text = format_file_list(&items, &stored, &[], "/data/files/c1", loc);
        assert!(text.contains("#1 notes.md"), "{text}");
        assert!(
            text.contains("Stored files: 2, 2.0 KB — /data/files/c1"),
            "{text}"
        );
        assert!(text.contains("#2 chart.png — 2.0 KB, image/png"), "{text}");
        assert!(
            text.contains("#3 gone.csv — 10 B, text/csv — missing from the folder"),
            "{text}"
        );
        // Stored files alone: no attachments header, and not "nothing attached".
        let alone = format_file_list(&[], &stored[..1], &[], "/d", loc);
        assert!(alone.starts_with("Stored files: 1"), "{alone}");
        assert_eq!(
            format_file_list(&[], &[], &[], "/d", loc),
            loc.t("ui.file.list_empty")
        );
    }

    /// The images the conversation carries are numbered on from the rest — the same `#N`
    /// the code names in `files` — and an attachment whose original the chat kept says so
    /// (docs/sandbox-file-exchange.md §12 T2, T4, T9).
    #[test]
    fn images_are_numbered_after_the_files_and_a_kept_original_is_marked() {
        let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
        let items = vec![AttachmentInfo {
            name: "report.pdf".into(),
            source: "C:\\report.pdf".into(),
            bytes: 1024,
            est_tokens: 100,
            prompt_tokens: 100,
            mode: AttachMode::ByReference,
            has_original: true,
        }];
        let stored = vec![StoredInfo {
            name: "chart.png".into(),
            bytes: 2048,
            mime: "image/png".into(),
            missing: false,
        }];
        let images = vec![crate::entities::message_image::ImageInfo {
            name: "shot.png".into(),
            source: "C:\\shot.png".into(),
            mime: "image/png".into(),
            width: 800,
            height: 600,
            bytes: 4096,
            est_tokens: 600,
        }];
        let text = format_file_list(&items, &stored, &images, "/d", loc);
        assert!(text.contains("#1 report.pdf"), "{text}");
        assert!(
            text.contains("the original is kept with the chat"),
            "{text}"
        );
        assert!(text.contains("#2 chart.png"), "{text}");
        assert!(text.contains("Images in this chat: 1"), "{text}");
        assert!(text.contains("#3 shot.png — 4.0 KB, 800×600"), "{text}");
        // Images with nothing attached or stored: the tail is the whole list.
        let alone = format_file_list(&[], &[], &images, "/d", loc);
        assert!(alone.starts_with("Images in this chat: 1"), "{alone}");
        assert!(alone.contains("#1 shot.png"), "{alone}");
    }
}
