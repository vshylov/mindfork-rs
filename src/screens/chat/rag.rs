//! The chat screen — the RAG indexing progress banner. Part of the [`super`]
//! module; split out of the chat.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 2).

use super::*;

impl ChatScreen {
    /// Updates the background RAG-indexing indicator (`/rag add`). Start/progress
    /// show a banner with a spinner; completion/error clear it and leave a
    /// summary note in the feed. See spec §9.3.
    pub fn set_rag_progress(&mut self, progress: RagProgress) {
        match progress {
            RagProgress::Started { total } => {
                self.rag = Some(RagBanner {
                    text: self
                        .loc
                        .tf("ui.rag.started", &[("total", &total.to_string())]),
                    tick: 0,
                });
            }
            RagProgress::Indexing {
                index,
                total,
                name,
                dir,
                chunks_done,
                chunks_total,
            } => self.set_indexing_banner(index, total, &name, &dir, chunks_done, chunks_total),
            RagProgress::Finished {
                files,
                chunks,
                errors,
                cancelled,
            } => self.push_finished_note(files, chunks, errors, cancelled),
            // Re-embedding (`/reindex`) — the same shape as `Finished`: clear the
            // banner and leave a summary note. Cancelling is safe and resumable
            // (every rewritten row is stamped with the current generation), so the
            // interrupted wording says so instead of reading like a failure.
            RagProgress::Reembedded {
                rows,
                errors,
                cancelled,
            } => self.push_reembedded_note(rows, errors, cancelled),
            RagProgress::Removed { chunks } => {
                let msg = if chunks == 0 {
                    self.loc.t("ui.rag.removed_none").to_string()
                } else {
                    self.loc
                        .tf("ui.rag.removed", &[("chunks", &chunks.to_string())])
                };
                self.push_note(&msg);
            }
            RagProgress::Listed { sources } => {
                self.push_note(&format_rag_sources(&sources, self.loc));
            }
            RagProgress::Failed(err) => {
                self.rag = None;
                self.push_error(&self.loc.tf("ui.rag.failed", &[("err", &err)]));
            }
        }
    }

    /// Updates the banner for the `Indexing` progress of one file (see
    /// [`Self::set_rag_progress`]).
    fn set_indexing_banner(
        &mut self,
        index: usize,
        total: usize,
        name: &str,
        dir: &str,
        chunks_done: usize,
        chunks_total: usize,
    ) {
        let location = if dir.is_empty() {
            String::new()
        } else {
            self.loc.tf("ui.rag.from", &[("dir", dir)])
        };
        let mut text = self.loc.tf(
            "ui.rag.indexing",
            &[
                ("name", name),
                ("location", &location),
                ("index", &index.to_string()),
                ("total", &total.to_string()),
            ],
        );
        // We show chunk progress only once the file has been chunked
        // (chunks_total>0). Otherwise (the file has just started) — the
        // previous look.
        if chunks_total > 0 {
            text.push_str(&self.loc.tf(
                "ui.rag.chunks",
                &[
                    ("done", &chunks_done.to_string()),
                    ("total", &chunks_total.to_string()),
                ],
            ));
        }
        match &mut self.rag {
            Some(banner) => banner.text = text,
            None => self.rag = Some(RagBanner { text, tick: 0 }),
        }
    }

    /// Clears the banner and leaves a summary note for the `Finished`
    /// progress (see [`Self::set_rag_progress`]).
    fn push_finished_note(&mut self, files: usize, chunks: usize, errors: usize, cancelled: bool) {
        self.rag = None;
        let mut msg = if cancelled {
            self.loc.tf(
                "ui.rag.finished_cancelled",
                &[("chunks", &chunks.to_string())],
            )
        } else {
            self.loc.tf(
                "ui.rag.finished",
                &[
                    ("files", &files.to_string()),
                    ("chunks", &chunks.to_string()),
                ],
            )
        };
        if errors > 0 {
            msg.push_str(
                &self
                    .loc
                    .tf("ui.rag.errors_suffix", &[("errors", &errors.to_string())]),
            );
        }
        self.push_note(&msg);
    }

    /// Clears the banner and leaves a summary note for the `Reembedded`
    /// progress (`/reindex`; see [`Self::set_rag_progress`]).
    fn push_reembedded_note(&mut self, rows: usize, errors: usize, cancelled: bool) {
        self.rag = None;
        let mut msg = if cancelled {
            self.loc.tf(
                "ui.rag.reembedded_cancelled",
                &[("rows", &rows.to_string())],
            )
        } else {
            self.loc
                .tf("ui.rag.reembedded", &[("rows", &rows.to_string())])
        };
        if errors > 0 {
            msg.push_str(
                &self
                    .loc
                    .tf("ui.rag.errors_suffix", &[("errors", &errors.to_string())]),
            );
        }
        self.push_note(&msg);
    }

    /// Whether background RAG indexing is in progress (the loop repaints frames
    /// for the spinner animation while this is `true`).
    pub fn is_rag_active(&self) -> bool {
        self.rag.is_some()
    }
}

pub(super) fn format_rag_sources(
    sources: &[crate::entities::rag::RagSourceInfo],
    loc: &'static Locale,
) -> String {
    if sources.is_empty() {
        return loc.t("ui.rag.list_empty").to_string();
    }
    let total: usize = sources.iter().map(|s| s.chunks).sum();
    let mut out = loc.tf(
        "ui.rag.list_header",
        &[
            ("n", &sources.len().to_string()),
            ("total", &total.to_string()),
        ],
    );
    for s in sources {
        let date = s
            .created_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string();
        out.push_str(&loc.tf(
            "ui.rag.list_item",
            &[
                ("source", &s.source),
                ("chunks", &s.chunks.to_string()),
                ("date", &date),
            ],
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text of the last note in the feed (the default locale is ru, as in
    /// the other chat-screen tests).
    fn last_note(s: &ChatScreen) -> String {
        let last = s.feed.last().expect("a note in the feed");
        assert_eq!(last.role, FeedRole::Note);
        last.text.clone()
    }

    #[test]
    fn reembedded_progress_pushes_note() {
        let mut s = ChatScreen::new();
        s.set_rag_progress(RagProgress::Reembedded {
            rows: 120,
            errors: 0,
            cancelled: false,
        });
        let note = last_note(&s);
        assert!(note.contains("завершена"), "a clean finish: {note}");
        assert!(note.contains("120"), "the row count: {note}");
        assert!(
            !note.contains("ошибками"),
            "no error suffix without errors: {note}"
        );
        assert!(!s.is_rag_active(), "the banner is cleared");
    }

    #[test]
    fn reembedded_progress_reports_errors() {
        let mut s = ChatScreen::new();
        s.set_rag_progress(RagProgress::Reembedded {
            rows: 98,
            errors: 3,
            cancelled: false,
        });
        let note = last_note(&s);
        assert!(note.contains("98"), "the row count: {note}");
        assert!(note.contains("с ошибками: 3"), "the error suffix: {note}");
    }

    #[test]
    fn reembedded_progress_cancelled_says_a_rerun_continues() {
        let mut s = ChatScreen::new();
        s.set_rag_progress(RagProgress::Reembedded {
            rows: 40,
            errors: 0,
            cancelled: true,
        });
        let note = last_note(&s);
        assert!(
            note.contains("прервана"),
            "distinct from a clean finish: {note}"
        );
        assert!(
            note.contains("40"),
            "the work done so far isn't lost: {note}"
        );
        assert!(
            note.contains("/reindex"),
            "cancelling is resumable — the note must say so: {note}"
        );
    }
}
