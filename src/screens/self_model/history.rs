//! The self-model's history view on the `F3` screen (spec §17.9;
//! docs/research/companion-platform.md C9): every version of the profile's
//! self-model, newest first, each with its date, who wrote it and what changed
//! from the one before; `Enter` opens a version in full, and `←`/`→` step to
//! the older or newer one while it is open.
//!
//! Read-only: a version is never restored from here — the model as it stands
//! is edited on the screen's own fields, and that edit is the next version.
//! The data is a snapshot the orchestrator answers `H` with
//! ([`SelfModelHistoryView`]); the view keeps the selected version by its
//! number when a fresh snapshot replaces it.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::entities::profile::CharacterNames;
use crate::entities::self_model::{GoalStatus, SelfModel};
use crate::entities::self_model_history::{
    Change, SelfModelHistoryView, SelfModelVersion, VersionAuthor, changes,
};
use crate::shared::i18n::Locale;
use crate::shared::keys;
use crate::shared::theme::Palette;
use crate::shared::ui::{SCREEN_MIN_SIZE, screen_chrome};
use crate::shared::wrap::wrap_line;

use super::{adjust_scroll, draw_visual_rows, expand_visual_rows};

/// How far `PageUp`/`PageDown` move: fixed, like the other screens.
const PAGE_STEP: usize = 10;

/// What a key did in the history view, for the screen around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HistoryKey {
    /// Handled here; the view stays.
    Stay,
    /// `Esc` on the list: back to the screen's fields.
    Back,
    /// `Ctrl+Q`/`F10`: quit the application.
    Quit,
}

/// The history view's state.
pub(super) struct History {
    view: SelfModelHistoryView,
    /// The selected version, as an index into `view.versions` (oldest first);
    /// the list shows them newest first.
    selected: usize,
    /// The selected version is open in full.
    open: bool,
    /// The first visible visual row of the list, and of an open version.
    list_scroll: usize,
    detail_scroll: usize,
}

impl History {
    /// The view over a snapshot, the newest version selected.
    pub(super) fn new(view: SelfModelHistoryView) -> Self {
        Self {
            selected: view.versions.len().saturating_sub(1),
            view,
            open: false,
            list_scroll: 0,
            detail_scroll: 0,
        }
    }

    /// Replaces the snapshot, keeping the selected version by its number when
    /// it is still there, and the newest one otherwise.
    pub(super) fn refresh(&mut self, view: SelfModelHistoryView) {
        let kept = self
            .view
            .versions
            .get(self.selected)
            .and_then(|v| view.versions.iter().position(|n| n.version == v.version));
        self.selected = kept.unwrap_or(view.versions.len().saturating_sub(1));
        if kept.is_none() {
            self.open = false;
        }
        self.view = view;
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) -> HistoryKey {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if (ctrl && keys::hotkey_char(&key) == Some('q')) || key.code == KeyCode::F(10) {
            return HistoryKey::Quit;
        }
        if ctrl {
            return HistoryKey::Stay;
        }
        let last = self.view.versions.len().saturating_sub(1);
        if self.open {
            match key.code {
                KeyCode::Esc => self.open = false,
                KeyCode::Left if self.selected > 0 => self.step_to(self.selected - 1),
                KeyCode::Right if self.selected < last => self.step_to(self.selected + 1),
                KeyCode::Up => self.detail_scroll = self.detail_scroll.saturating_sub(1),
                KeyCode::Down => self.detail_scroll += 1,
                KeyCode::PageUp => {
                    self.detail_scroll = self.detail_scroll.saturating_sub(PAGE_STEP)
                }
                KeyCode::PageDown => self.detail_scroll += PAGE_STEP,
                KeyCode::Home => self.detail_scroll = 0,
                _ => {}
            }
            return HistoryKey::Stay;
        }
        // The list is newest first: up is newer, down is older.
        match key.code {
            KeyCode::Esc => return HistoryKey::Back,
            KeyCode::Enter if !self.view.versions.is_empty() => {
                self.open = true;
                self.detail_scroll = 0;
            }
            KeyCode::Up => self.selected = (self.selected + 1).min(last),
            KeyCode::Down => self.selected = self.selected.saturating_sub(1),
            KeyCode::PageUp => self.selected = (self.selected + PAGE_STEP).min(last),
            KeyCode::PageDown => self.selected = self.selected.saturating_sub(PAGE_STEP),
            KeyCode::Home => self.selected = last,
            KeyCode::End => self.selected = 0,
            _ => {}
        }
        HistoryKey::Stay
    }

    fn step_to(&mut self, index: usize) {
        self.selected = index;
        self.detail_scroll = 0;
    }

    /// The footer: what the keys do in the list, or in an open version.
    fn hints(&self, loc: &'static Locale) -> Vec<(&'static str, &'static str, bool)> {
        let mut hk = Vec::new();
        if self.open {
            hk.push(("↑↓", loc.t("ui.self_model.history.hk.scroll"), false));
            if self.view.versions.len() > 1 {
                hk.push(("←→", loc.t("ui.self_model.history.hk.step"), false));
            }
        } else if !self.view.versions.is_empty() {
            hk.push(("↑↓", loc.t("ui.self_model.hk.select"), false));
            hk.push(("Enter", loc.t("ui.self_model.history.hk.open"), false));
        }
        hk.extend([
            ("F1", loc.t("ui.self_model.hk.help"), false),
            ("Esc", loc.t("ui.self_model.history.hk.back"), false),
            ("Ctrl+Q", loc.t("ui.self_model.hk.quit"), false),
        ]);
        hk
    }

    /// Draws the view full-screen, in the screen's own chrome.
    pub(super) fn render(
        &mut self,
        frame: &mut Frame,
        palette: &Palette,
        loc: &'static Locale,
        names: &CharacterNames,
    ) {
        let title = format!(
            "{} {}",
            palette.glyphs().assistant_icon,
            loc.t("ui.self_model.history.title")
        );
        let right = (!self.view.versions.is_empty()).then(|| {
            loc.tf(
                "ui.self_model.history.count",
                &[("n", &self.view.versions.len().to_string())],
            )
        });
        let hints = self.hints(loc);
        let chrome = screen_chrome(frame, palette, title, right, &hints, SCREEN_MIN_SIZE);
        frame.render_widget(Paragraph::new(chrome.hotkeys), chrome.status);
        let area = chrome.inner;
        if self.view.versions.is_empty() {
            let empty = Line::styled(loc.t("ui.self_model.history.empty"), palette.muted_style());
            frame.render_widget(Paragraph::new(empty), area);
            return;
        }
        let width = (area.width as usize).saturating_sub(2).max(1);
        let view_h = area.height as usize;
        if self.open {
            let mut visual: Vec<Line<'static>> = Vec::new();
            for line in self.detail_lines(palette, loc, names) {
                let wrapped = wrap_line(&line, width);
                if wrapped.is_empty() {
                    visual.push(Line::from(String::new()));
                }
                visual.extend(wrapped);
            }
            self.detail_scroll = self.detail_scroll.min(visual.len().saturating_sub(view_h));
            let shown: Vec<Line<'static>> = visual
                .into_iter()
                .skip(self.detail_scroll)
                .take(view_h)
                .map(|line| {
                    let mut spans = vec![Span::raw("  ")];
                    spans.extend(line.spans);
                    Line::from(spans)
                })
                .collect();
            frame.render_widget(Paragraph::new(shown), area);
            return;
        }
        let rows = self.list_rows(palette, loc);
        let sel = self.view.versions.len() - 1 - self.selected;
        let (visual, sel_start, sel_height) = expand_visual_rows(&rows, sel, width);
        self.list_scroll = adjust_scroll(self.list_scroll, sel_start, sel_height, view_h);
        draw_visual_rows(frame, area, &visual, sel, self.list_scroll, palette);
    }

    /// One row per version, newest first: its number, date and author, then —
    /// muted — what it changed.
    fn list_rows(&self, palette: &Palette, loc: &'static Locale) -> Vec<(Line<'static>, ())> {
        let versions = &self.view.versions;
        (0..versions.len())
            .rev()
            .map(|i| {
                let v = &versions[i];
                let prev = i.checked_sub(1).map(|p| &versions[p]);
                let line = Line::from(vec![
                    Span::styled(format!("v{}", v.version), palette.accent_style()),
                    Span::styled(format!("  {}  ", local(v)), palette.muted_style()),
                    Span::raw(self.author(v, loc)),
                    Span::styled(format!(" — {}", areas(prev, v, loc)), palette.muted_style()),
                ]);
                (line, ())
            })
            .collect()
    }

    /// An open version: who wrote it and when, what it changed, then the model
    /// as it was — the screen's two halves, read-only.
    fn detail_lines(
        &self,
        palette: &Palette,
        loc: &'static Locale,
        names: &CharacterNames,
    ) -> Vec<Line<'static>> {
        let i = self.selected;
        let v = &self.view.versions[i];
        let prev = i.checked_sub(1).map(|p| &self.view.versions[p]);
        let bold = palette.accent_style().add_modifier(Modifier::BOLD);
        let label = |key: &str| Span::styled(loc.t(key).to_string(), palette.accent_style());
        let dim = |s: String| Span::styled(s, palette.muted_style());
        let mut out = vec![
            Line::from(vec![
                Span::styled(
                    loc.tf(
                        "ui.self_model.history.version",
                        &[("n", &v.version.to_string())],
                    ),
                    bold,
                ),
                dim(format!("  {}", local(v))),
            ]),
            Line::from(vec![
                label("ui.self_model.history.by"),
                Span::raw(self.author_full(v, loc)),
            ]),
            Line::from(String::new()),
        ];
        match prev {
            None => out.push(Line::from(dim(loc
                .t("ui.self_model.history.first")
                .to_string()))),
            Some(p) => {
                let list = changes(&p.model, &v.model);
                let n = p.version.to_string();
                if list.is_empty() {
                    out.push(Line::from(dim(
                        loc.tf("ui.self_model.history.no_change", &[("n", &n)])
                    )));
                } else {
                    out.push(Line::from(label_owned(
                        loc.tf("ui.self_model.history.changed_from", &[("n", &n)]),
                        palette,
                    )));
                    for change in &list {
                        out.push(Line::from(format!("• {}", change_line(change, loc))));
                    }
                }
            }
        }
        out.push(Line::from(String::new()));
        out.extend(model_lines(&v.model, palette, loc, names));
        out
    }

    /// Who wrote a version, short — the list's form.
    fn author(&self, v: &SelfModelVersion, loc: &'static Locale) -> String {
        match &v.author {
            VersionAuthor::Model { name, .. } => model_name(name, loc),
            VersionAuthor::Person => loc.t("ui.self_model.history.author.person_short").into(),
            VersionAuthor::App => loc.t("ui.self_model.history.author.app_short").into(),
            VersionAuthor::Unknown => loc.t("ui.self_model.history.author.unknown").into(),
        }
    }

    /// Who wrote a version, in full — an open version's form: the model with
    /// its mode and the conversation, or for a version with no recorded author,
    /// the model the profile was talking to at the time, said as only that.
    fn author_full(&self, v: &SelfModelVersion, loc: &'static Locale) -> String {
        match &v.author {
            VersionAuthor::Model {
                name,
                mode,
                chat_id,
                background,
            } => {
                let model = format!("{} ({})", model_name(name, loc), mode.key());
                let chat = chat_id.and_then(|id| self.view.chat_titles.get(&id));
                let key = match (background, chat.is_some()) {
                    (false, true) => "ui.self_model.history.author.turn",
                    (false, false) => "ui.self_model.history.author.turn_nochat",
                    (true, true) => "ui.self_model.history.author.background",
                    (true, false) => "ui.self_model.history.author.background_nochat",
                };
                let chat = chat.map(String::as_str).unwrap_or_default();
                loc.tf(key, &[("model", &model), ("chat", chat)])
            }
            VersionAuthor::Person => loc.t("ui.self_model.history.author.person").into(),
            VersionAuthor::App => loc.t("ui.self_model.history.author.app").into(),
            VersionAuthor::Unknown => match self.view.llm_at(v.written_at) {
                Some(then) => loc.tf(
                    "ui.self_model.history.author.unknown_then",
                    &[("model", &then.model)],
                ),
                None => loc.t("ui.self_model.history.author.unknown").into(),
            },
        }
    }
}

/// A label span from an owned string.
fn label_owned(text: String, palette: &Palette) -> Span<'static> {
    Span::styled(text, palette.accent_style())
}

/// A version's date and time on the local clock.
fn local(v: &SelfModelVersion) -> String {
    v.written_at
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

fn model_name(name: &Option<String>, loc: &'static Locale) -> String {
    name.clone()
        .unwrap_or_else(|| loc.t("ui.self_model.history.author.unnamed").into())
}

/// The list's compact "what changed": the parts of the model a version
/// touched, in the screen's order — or that it is the first one kept.
fn areas(prev: Option<&SelfModelVersion>, v: &SelfModelVersion, loc: &'static Locale) -> String {
    let Some(prev) = prev else {
        return loc.t("ui.self_model.history.area.first").into();
    };
    let mut parts: Vec<&'static str> = Vec::new();
    for change in changes(&prev.model, &v.model) {
        let key = match change {
            Change::Summary => "ui.self_model.history.area.summary",
            Change::GoalAdded(_)
            | Change::GoalRemoved(_)
            | Change::GoalReworded { .. }
            | Change::GoalStatus { .. } => "ui.self_model.history.area.goals",
            Change::TraitsAdded(_) | Change::TraitsRemoved(_) => {
                "ui.self_model.history.area.traits"
            }
            Change::InterestsAdded(_) | Change::InterestsRemoved(_) => {
                "ui.self_model.history.area.interests"
            }
            Change::Relationship => "ui.self_model.history.area.relationship",
        };
        let part = loc.t(key);
        if !parts.contains(&part) {
            parts.push(part);
        }
    }
    if parts.is_empty() {
        return loc.t("ui.self_model.history.area.none").into();
    }
    parts.join(", ")
}

/// One change, worded for an open version.
fn change_line(change: &Change, loc: &'static Locale) -> String {
    let one = |key: &str, goal: &str| loc.tf(key, &[("goal", goal)]);
    let items = |key: &str, list: &[String]| loc.tf(key, &[("items", &list.join(", "))]);
    match change {
        Change::Summary => loc.t("ui.self_model.history.change.summary").into(),
        Change::GoalAdded(g) => one("ui.self_model.history.change.goal_added", g),
        Change::GoalRemoved(g) => one("ui.self_model.history.change.goal_removed", g),
        Change::GoalReworded { from, to } => loc.tf(
            "ui.self_model.history.change.goal_reworded",
            &[("from", from), ("to", to)],
        ),
        Change::GoalStatus { goal, to, .. } => one(
            match to {
                GoalStatus::Completed => "ui.self_model.history.change.goal_completed",
                GoalStatus::Abandoned => "ui.self_model.history.change.goal_abandoned",
                GoalStatus::Active => "ui.self_model.history.change.goal_reopened",
            },
            goal,
        ),
        Change::TraitsAdded(l) => items("ui.self_model.history.change.traits_added", l),
        Change::TraitsRemoved(l) => items("ui.self_model.history.change.traits_removed", l),
        Change::InterestsAdded(l) => items("ui.self_model.history.change.interests_added", l),
        Change::InterestsRemoved(l) => items("ui.self_model.history.change.interests_removed", l),
        Change::Relationship => loc.t("ui.self_model.history.change.relationship").into(),
    }
}

/// A version's model as the screen shows the one standing — the two halves
/// under the profile's names — without the editing rows.
fn model_lines(
    m: &SelfModel,
    palette: &Palette,
    loc: &'static Locale,
    names: &CharacterNames,
) -> Vec<Line<'static>> {
    let bold = palette.accent_style().add_modifier(Modifier::BOLD);
    let label = |key: &str| Span::styled(loc.t(key).to_string(), palette.accent_style());
    let or_dash = |s: &str| {
        if s.trim().is_empty() {
            Span::styled("—".to_string(), palette.muted_style())
        } else {
            Span::raw(s.trim().to_string())
        }
    };
    let named = |custom: Option<&str>, key: &str| {
        custom
            .map(str::to_string)
            .unwrap_or_else(|| loc.t(key).to_string())
    };
    let blank = || Line::from(String::new());
    let mut out = vec![
        Line::styled(
            named(names.assistant_name(), "ui.self_model.assistant"),
            bold,
        ),
        blank(),
        Line::from(vec![label("ui.self_model.summary"), or_dash(&m.summary)]),
    ];
    if !m.goals.is_empty() {
        out.push(blank());
    }
    let glyphs = palette.glyphs();
    for g in &m.goals {
        let (marker, style) = match g.status {
            GoalStatus::Active => ("● ".to_string(), palette.success_style()),
            GoalStatus::Completed => (format!("{} ", glyphs.ok), palette.muted_style()),
            GoalStatus::Abandoned => (format!("{} ", glyphs.failed), palette.muted_style()),
        };
        out.push(Line::from(vec![
            Span::styled(marker, style),
            Span::raw(g.description.trim().to_string()),
        ]));
    }
    let u = &m.user_model;
    out.extend([
        blank(),
        Line::styled(named(names.user_name(), "ui.self_model.user"), bold),
        blank(),
        Line::from(vec![
            label("ui.self_model.traits"),
            or_dash(&u.perceived_traits.join(", ")),
        ]),
        Line::from(vec![
            label("ui.self_model.interests"),
            or_dash(&u.current_interests.join(", ")),
        ]),
        Line::from(vec![
            label("ui.self_model.relationship"),
            or_dash(&u.relationship_dynamic),
        ]),
    ]);
    out
}

#[cfg(test)]
mod tests {
    use super::super::{HELP_SECTION, SelfModelIntent, SelfModelScreen};
    use super::*;
    use crate::entities::profile::LlmChange;
    use crate::shared::config::{NoteOrder, ServerMode};
    use chrono::{TimeZone, Utc};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use uuid::Uuid;

    fn en() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::En)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn at(day: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, 12, 0, 0).unwrap()
    }

    /// Three versions: one found with no author (v1), a turn's (v2), the
    /// person's (v3) — each a step from the one before.
    fn view() -> SelfModelHistoryView {
        let pid = Uuid::new_v4();
        let chat = Uuid::new_v4();
        let mut m = SelfModel::new(pid);
        m.summary = "I am Sol.".into();
        let v1 = SelfModelVersion {
            version: 1,
            written_at: at(1),
            author: VersionAuthor::Unknown,
            model: m.clone(),
        };
        m.add_goal("learn chess");
        m.user_model.perceived_traits = vec!["curious".into()];
        let v2 = SelfModelVersion {
            version: 2,
            written_at: at(2),
            author: VersionAuthor::Model {
                name: Some("gemma-4-31b".into()),
                mode: ServerMode::External,
                chat_id: Some(chat),
                background: false,
            },
            model: m.clone(),
        };
        m.summary = "I am Sol, Alex's friend.".into();
        let v3 = SelfModelVersion {
            version: 3,
            written_at: at(3),
            author: VersionAuthor::Person,
            model: m,
        };
        SelfModelHistoryView {
            versions: vec![v1, v2, v3],
            chat_titles: [(chat, "Chess openings".to_string())].into(),
            llm: vec![LlmChange {
                changed_at: at(1) - chrono::Duration::days(3),
                model: "gpt-6.1-sol".into(),
                mode: ServerMode::OpenAi,
            }],
        }
    }

    fn screen() -> SelfModelScreen {
        SelfModelScreen::new(
            None,
            CharacterNames::default(),
            Palette::default(),
            en(),
            NoteOrder::default(),
        )
    }

    fn text(s: &mut SelfModelScreen) -> String {
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn h_asks_for_the_history_on_either_layout() {
        let mut s = screen();
        assert_eq!(
            s.handle_key(key(KeyCode::Char('h'))),
            Some(SelfModelIntent::History)
        );
        // The same physical key under a Russian layout.
        assert_eq!(
            s.handle_key(key(KeyCode::Char('р'))),
            Some(SelfModelIntent::History)
        );
        assert!(
            !s.history_open(),
            "the view opens on the answer, not the key"
        );
    }

    #[test]
    fn the_list_is_newest_first_with_authors_and_what_changed() {
        let mut s = screen();
        s.show_history(view());
        let shown = text(&mut s);
        let (p3, p2, p1) = (
            shown.find("v3").unwrap(),
            shown.find("v2").unwrap(),
            shown.find("v1").unwrap(),
        );
        assert!(p3 < p2 && p2 < p1, "{shown}");
        assert!(shown.contains("you — about me"), "{shown}");
        assert!(shown.contains("gemma-4-31b — goals, traits"), "{shown}");
        assert!(
            shown.contains("not recorded — the first version kept"),
            "{shown}"
        );
        assert!(shown.contains("versions: 3"), "{shown}");
    }

    #[test]
    fn an_open_version_says_who_and_what_changed_and_steps_with_arrows() {
        let mut s = screen();
        s.show_history(view());
        s.handle_key(key(KeyCode::Down)); // v2
        s.handle_key(key(KeyCode::Enter));
        let shown = text(&mut s);
        assert!(shown.contains("Version 2"), "{shown}");
        assert!(
            shown.contains("Written by: gemma-4-31b (external), in «Chess openings»"),
            "{shown}"
        );
        assert!(shown.contains("Changed from version 1:"), "{shown}");
        assert!(shown.contains("• goal added: learn chess"), "{shown}");
        assert!(shown.contains("• traits added: curious"), "{shown}");
        assert!(shown.contains("● learn chess"), "{shown}");

        // ← is the older one: the first kept, its author told by what the
        // profile was talking to then — and only that.
        s.handle_key(key(KeyCode::Left));
        let shown = text(&mut s);
        assert!(shown.contains("Version 1"), "{shown}");
        assert!(
            shown.contains("not recorded — the profile was talking to gpt-6.1-sol then"),
            "{shown}"
        );
        assert!(shown.contains("The first version kept."), "{shown}");
        s.handle_key(key(KeyCode::Left)); // nothing older: stays
        assert!(text(&mut s).contains("Version 1"));

        s.handle_key(key(KeyCode::Right));
        s.handle_key(key(KeyCode::Right));
        let shown = text(&mut s);
        assert!(shown.contains("Version 3"), "{shown}");
        assert!(shown.contains("Written by: you, on this screen"), "{shown}");
        assert!(shown.contains("• about me rewritten"), "{shown}");
    }

    #[test]
    fn esc_steps_back_version_list_fields_then_closes() {
        let mut s = screen();
        s.show_history(view());
        s.handle_key(key(KeyCode::Enter));
        assert_eq!(s.handle_key(key(KeyCode::Esc)), None); // version → list
        assert!(s.history_open());
        assert_eq!(s.handle_key(key(KeyCode::Esc)), None); // list → fields
        assert!(!s.history_open());
        assert_eq!(
            s.handle_key(key(KeyCode::Esc)),
            Some(SelfModelIntent::Close)
        );
    }

    #[test]
    fn quit_works_from_the_history() {
        let mut s = screen();
        s.show_history(view());
        let ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(s.handle_key(ctrl_q), Some(SelfModelIntent::Quit));
        s.handle_key(key(KeyCode::Enter));
        assert_eq!(
            s.handle_key(key(KeyCode::F(10))),
            Some(SelfModelIntent::Quit)
        );
    }

    /// A fresh snapshot (the model changed while the history stood) keeps the
    /// version the reader is on, by its number — not by its place in the list.
    #[test]
    fn a_refresh_keeps_the_selected_version_by_number() {
        let mut h = History::new(view());
        h.handle_key(key(KeyCode::Down)); // v2
        h.handle_key(key(KeyCode::Enter));
        let mut grown = view();
        let mut v4 = grown.versions[2].clone();
        v4.version = 4;
        v4.author = VersionAuthor::Person;
        grown.versions.push(v4);
        h.refresh(grown);
        assert_eq!(h.view.versions[h.selected].version, 2);
        assert!(h.open);

        // Its version gone (the model was cleared and written anew): the
        // newest one, back on the list.
        let mut anew = view();
        anew.versions.truncate(1);
        h.refresh(anew);
        assert_eq!(h.selected, 0);
        assert!(!h.open);
    }

    #[test]
    fn an_empty_history_says_so() {
        let mut s = screen();
        s.show_history(SelfModelHistoryView::default());
        assert_eq!(s.handle_key(key(KeyCode::Enter)), None);
        let shown = text(&mut s);
        assert!(shown.contains("No versions yet"), "{shown}");
        assert!(!shown.contains("Enter"), "nothing to open: {shown}");
    }

    #[test]
    fn every_history_hint_is_in_the_help_section() {
        let mut h = History::new(view());
        let norm = |k: &str| k.replace(['/', ' '], "");
        for open in [false, true] {
            h.open = open;
            for (key, _, _) in h.hints(en()) {
                if key == "F1" || key == "Ctrl+Q" {
                    continue;
                }
                assert!(
                    HELP_SECTION
                        .rows
                        .iter()
                        .any(|(k, _)| norm(k).starts_with(&norm(key))),
                    "{key} is advertised but not in the F1 section"
                );
            }
        }
    }
}
