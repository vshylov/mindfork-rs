//! The chat screen — attached files (`/file attach`): the feed note for a
//! command's outcome and the status-bar chip. Part of the [`super`] module.
//! See docs/file-attachments.md.

use super::*;
use crate::entities::attachment::format_bytes;
use crate::features::file_command::{FileProgress, mode_label};

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
            FileProgress::Removed { name } => {
                let msg = self.loc.tf("ui.file.removed", &[("name", &name)]);
                self.push_note(&msg);
            }
            FileProgress::Listed { items } => {
                let text = format_attachments(&items, self.loc);
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

/// Formats the `/file list` reply: a header plus one line per attachment with
/// its `#N` handle (the same handle `/file remove #N` accepts).
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
        out.push_str(&loc.tf(
            "ui.file.list_item",
            &[
                ("i", &(i + 1).to_string()),
                ("name", &a.name),
                ("size", &format_bytes(a.bytes)),
                ("tokens", &format_tokens(a.est_tokens)),
                ("mode", mode_label(a.mode, loc)),
            ],
        ));
    }
    out
}
