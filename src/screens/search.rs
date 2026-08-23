//! Message-level search screen (FSD "page"): the messages matching a content
//! query, grouped by chat, with `Enter` jumping the feed straight onto one.
//! Opened from the chat list's content mode (`Ctrl+G`), closed via `Esc`.
//! See docs/history/chat-search-stage2.md (stage 2b).
//!
//! Stage 1 answers *"which chats mention this?"*; this screen answers *"where
//! exactly, and take me there"* — riding on stage 2a's jump
//! (`AppCommand::OpenChatAt`).
//!
//! The screen is a pure projection: the orchestrator owns the index and the
//! chats, builds the groups and snippets, and hands them over as
//! `AppEvent::MessageSearchResults`. Navigation is over **hits**, so chat
//! headers are skipped by construction rather than by a filter. Like the other
//! screens it knows nothing about `app`/channels (FSD) and answers with a
//! [`SearchIntent`].

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use uuid::Uuid;

use crate::features::chat_search::{SearchGroup, Snippet};
use crate::shared::i18n::Locale;
use crate::shared::keys;
use crate::shared::theme::Palette;
use crate::shared::ui::render_scrollbar;
use crate::shared::wrap::wrap_line;

/// Paged selection step for `PageUp`/`PageDown`. Fixed, like the chat list's:
/// the real list height is only known at render time, and hits are variable
/// height anyway (a snippet wraps).
const PAGE_STEP: usize = 5;

/// Intent from the search screen (translated by `app`). A counterpart to
/// [`ChatListIntent`](crate::screens::chat_list::ChatListIntent).
#[derive(Debug, Clone, PartialEq)]
pub enum SearchIntent {
    /// Close the results, return to the chat list (`Esc`).
    Close,
    /// Quit the application (`Ctrl+Q`/`F10`).
    Quit,
    /// Open a chat with the feed on this message (`Enter`) — stage 2a's jump.
    /// `query` rides along so the feed can highlight it inside that message
    /// (fork S3(b)): the screen is the one place that still knows which query
    /// these results answer.
    OpenHit {
        chat: Uuid,
        message: Uuid,
        query: String,
    },
}

/// What a rendered row points at: a hit (selectable, by its index in the
/// flattened hit list) or decoration (a chat header/spacer the cursor never
/// lands on).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Row {
    Hit(usize),
    Decoration,
}

/// The message-level search screen: the result snapshot + navigation state.
pub struct SearchScreen {
    /// The raw query these results answer (shown, and handed back on `Close` so
    /// the chat list reopens still searching for it).
    query: String,
    groups: Vec<SearchGroup>,
    /// True number of matching messages — may exceed what `groups` carries when
    /// the cap bit, which is what the "showing N of M" line is for.
    total: usize,
    palette: Palette,
    /// Interface locale (updated on `Settings`). See docs/i18n-ui.md.
    loc: &'static Locale,
    /// Index into the flattened hit list (see [`Self::hits`]).
    selected: usize,
    /// First visible visual row; recomputed in [`Self::render`] so the selected
    /// hit stays visible even when its snippet wraps over several rows.
    scroll: usize,
}

impl SearchScreen {
    /// Opens the screen on a result snapshot.
    pub fn new(
        query: String,
        groups: Vec<SearchGroup>,
        total: usize,
        palette: Palette,
        loc: &'static Locale,
    ) -> Self {
        Self {
            query,
            groups,
            total,
            palette,
            loc,
            selected: 0,
            scroll: 0,
        }
    }

    /// Replaces the results (a later `MessageSearchResults` for the same
    /// screen), putting the selection back at the top — the list is a different
    /// set of hits, so keeping an index into the old one would be meaningless.
    pub fn set_results(&mut self, query: String, groups: Vec<SearchGroup>, total: usize) {
        self.query = query;
        self.groups = groups;
        self.total = total;
        self.selected = 0;
        self.scroll = 0;
    }

    /// The query these results answer — the chat list is reopened still
    /// searching for it when the screen closes.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The index of the selected hit. Test-only: it is what proves the screen
    /// came back **whole** after a jump (`SearchReturn`) rather than being
    /// rebuilt from the query — `scroll` is derived from it inside
    /// [`Self::render`] (see `adjust_scroll`), so the selection is the state
    /// worth asserting on.
    #[cfg(test)]
    pub(crate) fn selected(&self) -> usize {
        self.selected
    }

    /// Updates the theme palette (the `AppEvent::Settings` event).
    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    /// Updates the interface locale (the `AppEvent::Settings` event).
    pub fn set_loc(&mut self, loc: &'static Locale) {
        self.loc = loc;
    }

    /// Every hit in display order, as `(chat, message)` — the navigation
    /// coordinate system. Headers are not in it, which is exactly why moving
    /// the selection cannot land on one.
    fn hits(&self) -> Vec<(Uuid, Uuid)> {
        self.groups
            .iter()
            .flat_map(|g| g.hits.iter().map(move |h| (g.chat_id, h.message_id)))
            .collect()
    }

    /// How many hits are actually shown (the numerator of "showing N of M").
    fn shown(&self) -> usize {
        self.groups.iter().map(|g| g.hits.len()).sum()
    }

    /// Handles a key press, returning an intent for `app` (`None` — handled
    /// internally: navigation).
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<SearchIntent> {
        if key.kind != KeyEventKind::Press {
            return None;
        }
        // Ctrl shortcuts go by "physical" Latin key, so they work under any
        // keyboard layout (see shared::keys).
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return match keys::hotkey_char(&key) {
                Some('q') => Some(SearchIntent::Quit),
                _ => None,
            };
        }
        let last = self.shown().saturating_sub(1);
        match key.code {
            KeyCode::F(10) => return Some(SearchIntent::Quit), // a second way to quit
            KeyCode::Esc => return Some(SearchIntent::Close),
            KeyCode::Enter => {
                let (chat, message) = *self.hits().get(self.selected)?;
                return Some(SearchIntent::OpenHit {
                    chat,
                    message,
                    query: self.query.clone(),
                });
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => self.selected = (self.selected + 1).min(last),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(PAGE_STEP),
            KeyCode::PageDown => self.selected = (self.selected + PAGE_STEP).min(last),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = last,
            _ => {}
        }
        None
    }

    /// Builds the logical rows: a chat header per group, then its hits, with a
    /// blank line between groups. A sub-agent transcript's group sits right
    /// under its parent's, its header indented with a `└` — the chat list's
    /// tree, in the results (spec §11.2.1); a parent with no hits of its own
    /// is a header saying "0 matches" that navigation never lands on.
    fn rows(&self) -> Vec<(Line<'static>, Row)> {
        let p = &self.palette;
        let loc = self.loc;
        let mut rows: Vec<(Line<'static>, Row)> = Vec::new();
        let mut hit_index = 0usize;
        for (i, group) in self.groups.iter().enumerate() {
            if i > 0 && group.parent.is_none() {
                rows.push((Line::from(String::new()), Row::Decoration));
            }
            let marker = match group.parent {
                Some(_) => "  └ ".to_string(),
                None => format!("{} ", p.glyphs().title_marker),
            };
            rows.push((
                Line::from(vec![
                    Span::styled(marker, p.accent_style()),
                    Span::styled(
                        group.title.clone(),
                        Style::new().fg(p.text).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(
                            "  {}",
                            loc.tf("ui.search.matches", &[("n", &group.hits.len().to_string())])
                        ),
                        p.muted_style(),
                    ),
                ]),
                Row::Decoration,
            ));
            for hit in &group.hits {
                rows.push((
                    self.hit_line(&hit.role, &hit.ts, &hit.snippet),
                    Row::Hit(hit_index),
                ));
                hit_index += 1;
            }
        }
        rows
    }

    /// One hit: a muted `role · date` prefix, then the snippet with its matched
    /// spans accented.
    fn hit_line(&self, role: &str, ts: &str, snippet: &Snippet) -> Line<'static> {
        let p = &self.palette;
        let mut spans = vec![Span::styled(
            format!("{} · {}  ", role_label(role, self.loc), format_ts(ts)),
            p.muted_style(),
        )];
        spans.extend(highlight_spans(snippet, p));
        Line::from(spans)
    }

    /// Draws the results full-screen.
    pub fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let palette = self.palette;
        let loc = self.loc;
        frame.render_widget(Clear, area);

        // The hotkey grid at the bottom, sized first so the panel gets exactly
        // the rest (as in the chat list).
        let hk: [(&str, &str, bool); 4] = [
            ("↑↓ PgUp/Dn Home/End", loc.t("ui.search.hk.select"), false),
            ("Enter", loc.t("ui.search.hk.open"), false),
            ("Esc", loc.t("ui.search.hk.back"), false),
            ("Ctrl+Q", loc.t("ui.search.hk.quit"), false),
        ];
        let hotkeys = palette.hotkey_grid(&hk, area.width as usize);
        let status_h = (hotkeys.len() as u16).max(1);
        let [panel_area, status_area] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(status_h)]).areas(area);

        let block = palette
            .panel(
                format!("{} {}", palette.glyphs().search, loc.t("ui.search.title")),
                true,
            )
            .title(
                Line::from(Span::styled(
                    format!(
                        " {} ",
                        loc.tf("ui.search.matches", &[("n", &self.total.to_string())])
                    ),
                    palette.muted_style(),
                ))
                .right_aligned(),
            );
        let inner = block.inner(panel_area);
        frame.render_widget(block, panel_area);

        // Inside: the query being answered, a "showing N of M" line when the
        // cap bit, then the results.
        let capped = self.shown() < self.total;
        let head_h = 1 + u16::from(capped);
        let [head_area, list_area] =
            Layout::vertical([Constraint::Length(head_h), Constraint::Min(1)]).areas(inner);

        let mut head = vec![Line::from(vec![
            Span::styled(
                format!("{} ", palette.glyphs().search),
                palette.muted_style(),
            ),
            Span::styled(self.query.clone(), Style::new().fg(palette.text)),
        ])];
        if capped {
            head.push(Line::from(Span::styled(
                loc.tf(
                    "ui.search.showing",
                    &[
                        ("n", &self.shown().to_string()),
                        ("total", &self.total.to_string()),
                    ],
                ),
                Style::new().fg(palette.warning),
            )));
        }
        frame.render_widget(Paragraph::new(head), head_area);

        if self.groups.is_empty() {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    loc.t("ui.search.empty"),
                    palette.muted_style(),
                )),
                list_area,
            );
            frame.render_widget(Paragraph::new(hotkeys), status_area);
            return;
        }

        // Word wrap, then manual row-by-row drawing — the same reason as on the
        // `F3` screen: `List` skips a multiline item outright when it does not
        // fit the remaining height, so a long trailing snippet would read as
        // "the end of the results". Content width = the area minus the `▌ `
        // selection-marker column.
        let content_width = (list_area.width as usize).saturating_sub(2).max(1);
        let rows = self.rows();
        self.selected = self.selected.min(self.shown().saturating_sub(1));

        let mut visual: Vec<(bool, Line<'static>)> = Vec::new();
        let (mut sel_start, mut sel_height) = (0usize, 1usize);
        for (line, kind) in &rows {
            let selected = *kind == Row::Hit(self.selected);
            let start = visual.len();
            let mut wrapped = wrap_line(line, content_width);
            if wrapped.is_empty() {
                wrapped.push(Line::from(String::new()));
            }
            if selected {
                sel_start = start;
                sel_height = wrapped.len();
            }
            for vl in wrapped {
                visual.push((selected, vl));
            }
        }

        let view_h = list_area.height as usize;
        self.scroll = adjust_scroll(self.scroll, sel_start, sel_height, view_h);

        for (offset, (selected, line)) in visual.iter().enumerate().skip(self.scroll).take(view_h) {
            let row = Rect {
                x: list_area.x,
                y: list_area.y + (offset - self.scroll) as u16,
                width: list_area.width,
                height: 1,
            };
            let prefix = if *selected {
                Span::styled("▌ ", Style::new().fg(palette.success))
            } else {
                Span::raw("  ")
            };
            let mut spans = vec![prefix];
            spans.extend(line.spans.iter().cloned());
            let mut para = Paragraph::new(Line::from(spans));
            if *selected {
                para = para.style(Style::new().bg(palette.keycap_bg));
            }
            frame.render_widget(para, row);
        }

        render_scrollbar(
            frame,
            Rect {
                x: panel_area.x,
                y: list_area.y,
                width: panel_area.width,
                height: list_area.height,
            },
            visual.len(),
            view_h,
            self.scroll,
            true,
            &palette,
        );
        frame.render_widget(Paragraph::new(hotkeys), status_area);
    }
}

/// Splits a snippet into styled spans: matched runs accented, the rest plain.
///
/// The ranges are **byte** ranges into `snippet.text` (see
/// [`crate::features::chat_search::build_snippet`]) — slicing on anything else
/// would panic on the first Cyrillic hit, which is why they are produced and
/// consumed in the same unit and never recomputed here.
fn highlight_spans(snippet: &Snippet, palette: &Palette) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut at = 0usize;
    for range in &snippet.matches {
        // Defensive: a malformed range must degrade to plain text, never panic
        // inside a render.
        if range.start < at || range.end > snippet.text.len() {
            continue;
        }
        if range.start > at {
            spans.push(Span::styled(
                snippet.text[at..range.start].to_string(),
                Style::new().fg(palette.text),
            ));
        }
        spans.push(Span::styled(
            snippet.text[range.clone()].to_string(),
            Style::new().fg(palette.accent).bold(),
        ));
        at = range.end;
    }
    if at < snippet.text.len() {
        spans.push(Span::styled(
            snippet.text[at..].to_string(),
            Style::new().fg(palette.text),
        ));
    }
    spans
}

/// A localized role label; an unknown role (a future one, or a hand-edited
/// index) shows as stored rather than disappearing.
fn role_label(role: &str, loc: &'static Locale) -> String {
    match role {
        "user" => loc.t("ui.search.role.user").to_string(),
        "assistant" => loc.t("ui.search.role.assistant").to_string(),
        "system" => loc.t("ui.search.role.system").to_string(),
        "tool" => loc.t("ui.search.role.tool").to_string(),
        other => other.to_string(),
    }
}

/// The stored RFC 3339 timestamp as a local date/time; an unparsable one is
/// shown as stored (the index is derived data — it must not be able to blank a
/// row).
fn format_ts(ts: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(ts) {
        Ok(dt) => dt
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        Err(_) => ts.to_string(),
    }
}

/// Recomputes scroll so the selected hit (visual rows
/// `[sel_start, sel_start + sel_height)`) stays visible; a hit taller than the
/// window is pinned to its **top**. Mirrors the `F3` screen's helper.
fn adjust_scroll(scroll: usize, sel_start: usize, sel_height: usize, view_h: usize) -> usize {
    if view_h == 0 {
        return scroll;
    }
    let sel_end = sel_start + sel_height;
    if sel_start < scroll {
        sel_start
    } else if sel_end > scroll + view_h {
        if sel_height >= view_h {
            sel_start
        } else {
            sel_end - view_h
        }
    } else {
        scroll
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::chat_search::{SearchHit, build_snippet};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn hit(text: &str, query: &str) -> SearchHit {
        SearchHit {
            message_id: Uuid::new_v4(),
            role: "user".into(),
            ts: "2026-07-29T10:00:00+00:00".into(),
            snippet: build_snippet(text, query, 160),
        }
    }

    /// Two chats, two hits each — so "skips headers" has something to skip.
    fn screen() -> SearchScreen {
        let groups = vec![
            SearchGroup {
                parent: None,
                chat_id: Uuid::new_v4(),
                title: "Первый чат".into(),
                hits: vec![
                    hit("первое сообщение про МАРКЕР здесь", "маркер"),
                    hit("второе сообщение про МАРКЕР тоже", "маркер"),
                ],
            },
            SearchGroup {
                parent: None,
                chat_id: Uuid::new_v4(),
                title: "Второй чат".into(),
                hits: vec![hit("третье сообщение про МАРКЕР", "маркер")],
            },
        ];
        SearchScreen::new("маркер".into(), groups, 3, Palette::default(), ru())
    }

    fn dump(s: &mut SearchScreen, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
        format!("{:?}", term.backend().buffer())
    }

    /// A transcript's group is drawn as the list draws it: under its parent,
    /// no blank line between, the header branched with `└`; a parent that
    /// matched nothing of its own is a "0 matches" header navigation steps
    /// over, and `Enter` on the transcript's hit opens the *transcript*
    /// (spec §11.2.1).
    #[test]
    fn a_transcript_group_sits_under_its_parent() {
        let parent = Uuid::new_v4();
        let child = Uuid::new_v4();
        let groups = vec![
            SearchGroup {
                parent: None,
                chat_id: parent,
                title: "Родитель".into(),
                hits: vec![],
            },
            SearchGroup {
                parent: Some(parent),
                chat_id: child,
                title: "Критик".into(),
                hits: vec![hit("сообщение про МАРКЕР внутри", "маркер")],
            },
        ];
        let mut s = SearchScreen::new("маркер".into(), groups, 1, Palette::default(), ru());
        let lines: Vec<String> = s
            .rows()
            .iter()
            .map(|(l, _)| {
                l.spans
                    .iter()
                    .map(|sp| sp.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert!(
            lines[0].contains("Родитель") && lines[0].contains("совпадений: 0"),
            "{lines:?}"
        );
        assert!(lines[1].starts_with("  └ Критик"), "{lines:?}");
        assert_eq!(
            lines.len(),
            3,
            "no spacer between a parent and its transcript: {lines:?}"
        );

        assert_eq!(s.selected(), 0);
        assert_eq!(
            s.handle_key(key(KeyCode::Enter)),
            Some(SearchIntent::OpenHit {
                chat: child,
                message: s.groups[1].hits[0].message_id,
                query: "маркер".into(),
            })
        );
        let dumped = dump(&mut s, 60, 12);
        assert!(dumped.contains("└ Критик"), "{dumped}");
    }

    /// Navigation is over hits, so a chat header between two hits is stepped
    /// over — the cursor can never land on something that cannot be opened.
    #[test]
    fn navigation_moves_between_hits_and_skips_headers() {
        let mut s = screen();
        let hits = s.hits();
        assert_eq!(hits.len(), 3);

        assert_eq!(s.handle_key(key(KeyCode::Down)), None);
        assert_eq!(s.selected, 1);
        // The next step crosses a group boundary (a spacer + a header) and
        // still lands on a hit — of the *other* chat.
        s.handle_key(key(KeyCode::Down));
        assert_eq!(s.selected, 2);
        assert_ne!(hits[2].0, hits[1].0, "the third hit is in another chat");
        // And it clamps at the end rather than running off into decoration.
        s.handle_key(key(KeyCode::Down));
        assert_eq!(s.selected, 2);

        s.handle_key(key(KeyCode::Home));
        assert_eq!(s.selected, 0);
        s.handle_key(key(KeyCode::Up));
        assert_eq!(s.selected, 0, "clamps at the top");
        s.handle_key(key(KeyCode::End));
        assert_eq!(s.selected, 2);
        s.handle_key(key(KeyCode::PageUp));
        assert_eq!(s.selected, 0);
        s.handle_key(key(KeyCode::PageDown));
        assert_eq!(s.selected, 2, "a page past the end clamps");
    }

    /// `Enter` opens the selected hit **and hands the query along**: this screen
    /// is the last place that knows it, and the feed needs it to highlight the
    /// match inside the message it lands on (fork S3(b)).
    #[test]
    fn enter_opens_the_selected_hit_with_the_query() {
        let mut s = screen();
        let hits = s.hits();
        assert_eq!(
            s.handle_key(key(KeyCode::Enter)),
            Some(SearchIntent::OpenHit {
                chat: hits[0].0,
                message: hits[0].1,
                query: "маркер".into(),
            })
        );
        s.handle_key(key(KeyCode::Down));
        s.handle_key(key(KeyCode::Down));
        assert_eq!(
            s.handle_key(key(KeyCode::Enter)),
            Some(SearchIntent::OpenHit {
                chat: hits[2].0,
                message: hits[2].1,
                query: "маркер".into(),
            })
        );
    }

    #[test]
    fn esc_closes_and_ctrl_q_or_f10_quits() {
        let mut s = screen();
        assert_eq!(s.handle_key(key(KeyCode::Esc)), Some(SearchIntent::Close));
        assert_eq!(
            s.handle_key(ctrl(KeyCode::Char('q'))),
            Some(SearchIntent::Quit)
        );
        // Also under a Cyrillic layout (physical Q = Ctrl+й).
        assert_eq!(
            s.handle_key(ctrl(KeyCode::Char('й'))),
            Some(SearchIntent::Quit)
        );
        assert_eq!(s.handle_key(key(KeyCode::F(10))), Some(SearchIntent::Quit));
    }

    #[test]
    fn enter_on_an_empty_result_is_a_no_op() {
        let mut s = SearchScreen::new("нечто".into(), vec![], 0, Palette::default(), ru());
        assert_eq!(s.handle_key(key(KeyCode::Enter)), None);
        assert_eq!(s.handle_key(key(KeyCode::Down)), None);
    }

    /// The matched run is drawn in the accent color — that is the whole point
    /// of carrying byte ranges through from the snippet builder.
    #[test]
    fn matches_are_drawn_in_the_accent_color() {
        let palette = Palette::default();
        let mut s = screen();
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
        let buf = term.backend().buffer();
        let accented: String = buf
            .content()
            .iter()
            .filter(|c| c.fg == palette.accent)
            .map(|c| c.symbol())
            .collect();
        assert!(
            accented.contains("МАРКЕР"),
            "the matched run must be accented, got {accented:?}"
        );
        // And the surrounding text is not.
        let plain: String = buf
            .content()
            .iter()
            .filter(|c| c.fg == palette.text)
            .map(|c| c.symbol())
            .collect();
        assert!(plain.contains("сообщение"), "{plain}");
    }

    #[test]
    fn empty_and_capped_results_render_without_panic() {
        let mut empty = SearchScreen::new("нечто".into(), vec![], 0, Palette::default(), ru());
        let d = dump(&mut empty, 80, 24);
        assert!(d.contains("Ничего не найдено"), "{d}");

        // Capped: 3 shown of 250 — the honest line must appear.
        let mut capped = screen();
        capped.set_results("маркер".into(), capped.groups.clone(), 250);
        let d = dump(&mut capped, 80, 24);
        assert!(d.contains("250"), "the true total must be shown: {d}");

        // Tight and generous geometries alike.
        for (w, h) in [(80u16, 24u16), (24, 6), (200, 60)] {
            dump(&mut screen(), w, h);
            dump(&mut empty, w, h);
        }
    }

    #[test]
    fn set_results_replaces_and_resets_the_selection() {
        let mut s = screen();
        s.handle_key(key(KeyCode::End));
        assert_eq!(s.selected, 2);
        s.set_results("другое".into(), vec![], 0);
        assert_eq!(s.selected, 0);
        assert_eq!(s.query(), "другое");
        assert!(s.hits().is_empty());
    }

    #[test]
    fn a_long_snippet_scrolls_into_view_instead_of_vanishing() {
        // The `F3` lesson: a tall trailing item must be clipped at the bottom
        // edge, not skipped — so the selection stays reachable.
        assert_eq!(adjust_scroll(0, 0, 1, 10), 0);
        assert_eq!(adjust_scroll(5, 2, 1, 10), 2);
        assert_eq!(adjust_scroll(0, 9, 1, 5), 5);
        assert_eq!(adjust_scroll(0, 8, 20, 5), 8, "a tall hit pins to its top");
        assert_eq!(adjust_scroll(3, 0, 1, 0), 3);
    }

    #[test]
    fn highlight_spans_survive_a_malformed_range() {
        // A render must never be the place a bad range panics.
        let palette = Palette::default();
        let snippet = Snippet {
            text: "текст".into(), // 10 bytes: two per character
            // A valid range followed by one running off the end.
            matches: vec![0..6, 6..999],
        };
        let spans = highlight_spans(&snippet, &palette);
        assert_eq!(spans.len(), 2, "{spans:?}");
        assert_eq!(spans[0].content, "тек");
        assert_eq!(spans[0].style.fg, Some(palette.accent));
        assert_eq!(spans[1].content, "ст", "the tail must survive intact");
    }

    #[test]
    fn timestamps_and_roles_degrade_to_what_is_stored() {
        assert_eq!(format_ts("не дата"), "не дата");
        assert!(format_ts("2026-07-29T10:00:00+00:00").starts_with("2026-07-29"));
        assert_eq!(role_label("странная роль", ru()), "странная роль");
        assert_ne!(role_label("user", ru()), "user");
    }
}
