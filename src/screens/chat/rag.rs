//! The chat screen — the RAG indexing progress banner. Part of the [`super`]
//! module; split out of the chat.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 2).

use super::*;
use crate::shared::wrap;

/// Marks the `{name}` slot while a banner template is interpolated, so the
/// name can be kept apart from the text around it (see [`RagBanner::around_name`]).
/// U+FFFC is the OBJECT REPLACEMENT CHARACTER — a stand-in for an object in
/// text, which is what it is here.
const NAME_SLOT: &str = "\u{FFFC}";

impl RagBanner {
    /// A banner that names nothing (`Started`): all of its text is fixed.
    pub(super) fn plain(text: String) -> Self {
        Self {
            before: text,
            name: String::new(),
            location: String::new(),
            after: String::new(),
            tick: 0,
        }
    }

    /// A banner from a localized template with a `{name}` slot, interpolated with
    /// `args`; the name and its location are stored apart from the text around
    /// them so the renderer can shorten them on their own. The name itself is
    /// never in the string that is split, so a name containing the marker cannot
    /// confuse it; a template without the slot (a broken external locale) yields
    /// what `tf` would have — the text with no name in it.
    pub(super) fn around_name(
        loc: &Locale,
        key: &str,
        name: &str,
        location: String,
        args: &[(&str, &str)],
    ) -> Self {
        let mut all: Vec<(&str, &str)> = args.to_vec();
        all.push(("name", NAME_SLOT));
        let text = loc.tf(key, &all);
        let (before, after) = text.split_once(NAME_SLOT).unwrap_or((text.as_str(), ""));
        Self {
            before: before.to_string(),
            name: name.to_string(),
            location,
            after: after.to_string(),
            tick: 0,
        }
    }

    /// Replaces the text and keeps the spinner phase — a progress update must
    /// not restart the animation.
    pub(super) fn update(&mut self, next: RagBanner) {
        let tick = self.tick;
        *self = next;
        self.tick = tick;
    }

    /// The full text, as it reads with room to spare.
    pub(super) fn text(&self) -> String {
        format!(
            "{}{}{}{}",
            self.before, self.name, self.location, self.after
        )
    }

    /// The text fitted into `width` columns. Everything fits — the full text.
    /// Otherwise the location is dropped whole (a detail); if the name still
    /// does not fit beside the fixed text, it is shortened in the middle — both
    /// ends of a name carry meaning (`wrap::elide_middle`); and if even the
    /// fixed text overflows, the tail is cut with a marker rather than clipped
    /// by the terminal, the one case in which the counters can be lost.
    pub(super) fn fit(&self, width: usize) -> String {
        let fixed = wrap::str_width(&self.before) + wrap::str_width(&self.after);
        let full = fixed + wrap::str_width(&self.name) + wrap::str_width(&self.location);
        if full <= width {
            return self.text();
        }
        let name = wrap::elide_middle(&self.name, width.saturating_sub(fixed));
        wrap::truncate_to_width(&format!("{}{name}{}", self.before, self.after), width).0
    }
}

impl ChatScreen {
    /// Shows `banner`, keeping the spinner phase of one already up.
    pub(super) fn show_banner(&mut self, banner: RagBanner) {
        match &mut self.rag {
            Some(current) => current.update(banner),
            None => self.rag = Some(banner),
        }
    }

    /// Updates the background RAG-indexing indicator (`/rag add`). Start/progress
    /// show a banner with a spinner; completion/error clear it and leave a
    /// summary note in the feed. See spec §9.3.
    pub fn set_rag_progress(&mut self, progress: RagProgress) {
        match progress {
            RagProgress::Started { total } => {
                self.rag = Some(RagBanner::plain(
                    self.loc
                        .tf("ui.rag.started", &[("total", &total.to_string())]),
                ));
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
        // The location rides the template right after the name; it is kept as
        // a part of its own (the renderer drops it first when short of room), so
        // the template's own `{location}` slot is filled with nothing.
        let mut banner = RagBanner::around_name(
            self.loc,
            "ui.rag.indexing",
            name,
            location,
            &[
                ("location", ""),
                ("index", &index.to_string()),
                ("total", &total.to_string()),
            ],
        );
        // We show chunk progress only once the file has been chunked
        // (chunks_total>0). Otherwise (the file has just started) — the
        // previous look.
        if chunks_total > 0 {
            banner.after.push_str(&self.loc.tf(
                "ui.rag.chunks",
                &[
                    ("done", &chunks_done.to_string()),
                    ("total", &chunks_total.to_string()),
                ],
            ));
        }
        self.show_banner(banner);
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

    /// The attachment banner for a long page title at `done/total` (the
    /// `/file attach` shape — no location).
    fn attachment_banner(s: &ChatScreen, name: &str) -> RagBanner {
        RagBanner::around_name(
            s.loc,
            "ui.file.indexing",
            name,
            String::new(),
            &[("done", "64"), ("total", "128")],
        )
    }

    #[test]
    fn banner_parts_reassemble_to_the_interpolated_text() {
        // Keeping the name apart changes nothing about what the banner says:
        // the parts joined are byte-for-byte what `tf` with the name produces.
        let s = ChatScreen::new();
        let banner = attachment_banner(&s, "report.pdf");
        let expected = s.loc.tf(
            "ui.file.indexing",
            &[("name", "report.pdf"), ("done", "64"), ("total", "128")],
        );
        assert_eq!(banner.text(), expected);
        assert_eq!(banner.fit(200), expected, "room to spare — the full text");
        // A name carrying the slot marker itself cannot confuse the split: the
        // name is never in the string that is split.
        let odd = attachment_banner(&s, "a\u{FFFC}b");
        assert_eq!(odd.name, "a\u{FFFC}b");
        assert!(odd.before.ends_with("для "), "{:?}", odd.before);
    }

    #[test]
    fn banner_fit_shortens_the_name_and_keeps_the_counters() {
        // The screenshot case: a web page's title as the attachment name, a
        // terminal narrower than the line. The counters are what changes while
        // the banner is up, so they are what must survive — the name gives way,
        // from the middle, so both its start and its end still read.
        let s = ChatScreen::new();
        let name =
            "GitHub - openai/gpt-oss: gpt-oss-120b and gpt-oss-20b are two open-weight models";
        let banner = attachment_banner(&s, name);
        let full_w = wrap::str_width(&banner.text());
        for width in [full_w - 1, 80, 60] {
            let out = banner.fit(width);
            assert!(
                wrap::str_width(&out) <= width,
                "fits {width} columns: {out:?}"
            );
            assert!(
                out.starts_with(&banner.before) && out.ends_with(&banner.after),
                "the fixed text around the name is intact at {width}: {out:?}"
            );
            assert!(
                out.contains("64/128"),
                "the counters survive at {width}: {out:?}"
            );
            assert!(
                out.contains('…'),
                "the name was shortened at {width}: {out:?}"
            );
        }
        // With a few columns to give the name, both of its ends still read;
        // the 55 columns of fixed text leave 45 here.
        let out = banner.fit(100);
        assert!(
            out.contains("GitHub - openai") && out.contains("weight models"),
            "both ends of the name remain: {out:?}"
        );
    }

    #[test]
    fn banner_fit_drops_the_location_before_touching_the_name() {
        // `/rag add`: the banner names the file and where it is. The location is
        // the detail — when the row is short of columns it goes first, whole,
        // and the name stays intact; the name is shortened only after that.
        let mut s = ChatScreen::new();
        s.set_rag_progress(RagProgress::Indexing {
            index: 2,
            total: 7,
            name: "quarterly-report.pdf".into(),
            dir: "C:\\Users\\Vladimir\\Documents\\Projects\\archive\\2026".into(),
            chunks_done: 16,
            chunks_total: 42,
        });
        let banner = s.rag.as_ref().unwrap();
        let full = banner.text();
        assert!(
            full.contains("quarterly-report.pdf из C:\\Users") && full.contains("(2/7)"),
            "the full banner as before: {full:?}"
        );
        let without_location = format!("{}{}{}", banner.before, banner.name, banner.after);
        let out = banner.fit(wrap::str_width(&full) - 1);
        assert_eq!(out, without_location, "one column short: the location goes");
        assert!(
            out.contains("quarterly-report.pdf") && out.contains("(2/7)") && out.contains("16/42"),
            "{out:?}"
        );
        // Shorter still — now the name is shortened, the counters stay.
        let out = banner.fit(wrap::str_width(&without_location) - 4);
        assert!(out.contains("(2/7)") && out.contains("16/42"), "{out:?}");
        assert!(
            out.contains("quart") && out.contains(".pdf") && out.contains('…'),
            "the name is cut in the middle: {out:?}"
        );
    }

    #[test]
    fn banner_fit_cuts_the_tail_when_even_the_fixed_text_overflows() {
        // A terminal too narrow for the counters themselves: the line is cut
        // with a marker rather than clipped at the edge by the widget.
        let s = ChatScreen::new();
        let banner = attachment_banner(&s, "x.pdf");
        let out = banner.fit(12);
        assert!(wrap::str_width(&out) <= 12, "{out:?}");
        assert!(out.ends_with('…'), "{out:?}");
        assert_eq!(banner.fit(0), "");
    }

    #[test]
    fn banner_update_keeps_the_spinner_phase() {
        let mut s = ChatScreen::new();
        s.set_rag_progress(RagProgress::Started { total: 2 });
        s.rag.as_mut().unwrap().tick = 7;
        s.set_file_progress(crate::features::file_command::FileProgress::Indexing {
            name: "a.pdf".into(),
            done: 1,
            total: 9,
        });
        let banner = s.rag.as_ref().unwrap();
        assert_eq!(
            banner.tick, 7,
            "a progress update does not restart the spinner"
        );
        assert_eq!(banner.name, "a.pdf");
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
