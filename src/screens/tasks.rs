//! The tasks screen (FSD "page", `F7` / `/tasks`): everything the app is
//! doing in the background, on one surface — every sub-agent and dialogue
//! run, in flight with its position or landed with its outcome, across every
//! chat and profile, then the app's own silent tasks. See spec §11.10,
//! [docs/research/tasks-screen.md](../../docs/research/tasks-screen.md).
//!
//! Like the other screens it is a **pure projection**: the orchestrator
//! builds the snapshot ([`TaskList`]) off its seats and the chats' records and
//! hands it over; this file knows nothing about `app`, channels or storage
//! (FSD), and answers with a [`TasksIntent`]. Every action is an existing
//! route — `Enter` opens the transcript exactly as the chat list does, `F6`
//! stops a run through the command `/subagents stop` sends, `P` is an
//! ordinary chat switch — so the screen is not a second control surface
//! (research §4.5). The footer names only the keys that work on the selected
//! row (spec §11.2, research R5).

use std::time::Instant;

use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use uuid::Uuid;

use crate::app::events::{AppTask, BackgroundKind, RunProgressKind, TaskList, TaskRun};
use crate::entities::subagent::RunOutcome;
use crate::shared::i18n::Locale;
use crate::shared::keys;
use crate::shared::theme::Palette;
use crate::shared::ui::{keep_visible, render_scrollbar, screen_chrome};
use crate::shared::wrap;
use crate::widgets::chat_list::run_state_key;
use crate::widgets::help_dialog::{HelpContext, HelpSection};

/// The tasks screen's "Shortcuts" section (`F1`): one row per key
/// `handle_key` matches — a new arm gets a row here (AGENTS.md §3).
pub(crate) static HELP_SECTION: HelpSection = HelpSection {
    title: "ui.help.sec.tasks",
    context: Some(HelpContext::Tasks),
    rows: &[
        ("↑/↓", "ui.help.tk_select"),
        ("Enter", "ui.help.tk_open"),
        ("P", "ui.help.tk_parent"),
        ("F6", "ui.help.tk_stop"),
        ("Esc", "ui.help.tk_close"),
    ],
    openers: &[],
};

/// How far `PageUp`/`PageDown` move the selection. Fixed rather than a
/// screenful, like the other screens: the height is only known at render time.
const PAGE_STEP: usize = 10;

/// How often a screen with a running row repaints on its own, so the elapsed
/// column moves without a keypress (research fork F5).
const REPAINT_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// Widest the parent-chat and state columns get; the title takes the rest.
const PARENT_MAX: usize = 22;
const STATE_MAX: usize = 28;
/// The title never shrinks below this; the right-hand columns yield first.
const TITLE_MIN: usize = 16;

/// Intent from the tasks screen (translated by `app`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TasksIntent {
    /// Close and go back to the chat (`Esc`).
    Close,
    /// Quit the application (`Ctrl+Q`/`F10`).
    Quit,
    /// Open the run's transcript, read-only (`Enter` on a run row).
    OpenRun(Uuid),
    /// Stop a running background run (`F6` on such a row) — the route
    /// `/subagents stop` and the transcript's `F6` take (spec §9.3.2).
    StopRun(Uuid),
    /// Open the run's parent chat (`P` on a run row).
    OpenParent(Uuid),
}

/// A selectable row: which half of the snapshot, and the index into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Run(usize),
    App(usize),
}

/// What identifies a selected row across snapshots: a run by its id, a
/// silent task by its kind. An index would drift as runs land above it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ident {
    Run(Uuid),
    App(BackgroundKind),
}

/// The tasks screen: the latest snapshot plus where the user is in it.
pub struct TasksScreen {
    /// `None` until the orchestrator's first reply — the screen opens on the
    /// key and says it is waiting, rather than showing an empty list that
    /// would read as "nothing is running" (docs/lessons.md §4).
    list: Option<TaskList>,
    palette: Palette,
    /// Interface locale (updated on `Settings`). See docs/i18n-ui.md.
    loc: &'static Locale,
    /// Index into [`Self::rows`].
    selected: usize,
    /// First visible display line.
    scroll: usize,
    /// When the screen was last drawn, for the once-a-second repaint that
    /// moves the elapsed column ([`Self::needs_repaint`]).
    last_drawn: Instant,
}

impl TasksScreen {
    pub fn new(palette: Palette, loc: &'static Locale) -> Self {
        Self {
            list: None,
            palette,
            loc,
            selected: 0,
            scroll: 0,
            last_drawn: Instant::now(),
        }
    }

    /// Replaces the snapshot — the reply to the opening request, or a later
    /// unasked one. The selection is kept **by identity** (a run's id, a
    /// task's kind), not by index: a run landing moves every row below it.
    pub fn set_list(&mut self, list: TaskList) {
        let previous = self.selected_ident();
        self.list = Some(list);
        let rows = self.rows();
        self.selected = previous
            .and_then(|ident| {
                rows.iter()
                    .position(|row| self.ident_of(*row) == Some(ident))
            })
            .unwrap_or_else(|| self.selected.min(rows.len().saturating_sub(1)));
    }

    /// Updates the theme palette (the `AppEvent::Settings` event).
    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    /// Updates the interface locale (the `AppEvent::Settings` event).
    pub fn set_loc(&mut self, loc: &'static Locale) {
        self.loc = loc;
    }

    /// Whether a run is in flight on the screen — the elapsed column moves,
    /// so the runtime's tick repaints it ([`Self::needs_repaint`]).
    pub fn has_running(&self) -> bool {
        self.list
            .as_ref()
            .is_some_and(|l| l.runs.iter().any(|r| r.running))
    }

    /// The runtime's once-a-second repaint while a run is out; nothing when
    /// the list is all landed (research R6: no timer that runs for nothing).
    pub fn needs_repaint(&self) -> bool {
        self.has_running() && self.last_drawn.elapsed() >= REPAINT_EVERY
    }

    /// The selectable rows in display order: the runs, then the tasks.
    fn rows(&self) -> Vec<Row> {
        let Some(list) = &self.list else {
            return Vec::new();
        };
        (0..list.runs.len())
            .map(Row::Run)
            .chain((0..list.app.len()).map(Row::App))
            .collect()
    }

    fn selected_row(&self) -> Option<Row> {
        self.rows().get(self.selected).copied()
    }

    fn selected_run(&self) -> Option<&TaskRun> {
        match self.selected_row()? {
            Row::Run(i) => self.list.as_ref()?.runs.get(i),
            Row::App(_) => None,
        }
    }

    fn ident_of(&self, row: Row) -> Option<Ident> {
        let list = self.list.as_ref()?;
        Some(match row {
            Row::Run(i) => Ident::Run(list.runs.get(i)?.id),
            Row::App(i) => Ident::App(list.app.get(i)?.kind),
        })
    }

    fn selected_ident(&self) -> Option<Ident> {
        self.ident_of(self.selected_row()?)
    }

    /// Whether `F6` does anything on the selected row: only a **running
    /// background** run has a seat the stop command can cancel — a child of
    /// the turn in flight is ended by cancelling the turn, and a landed run
    /// has nothing to stop.
    fn selected_stoppable(&self) -> Option<Uuid> {
        self.selected_run()
            .filter(|r| r.running && r.background)
            .map(|r| r.id)
    }

    /// Handles a key press, returning an intent for `app` (`None` — handled
    /// internally: navigation).
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<TasksIntent> {
        if key.kind != KeyEventKind::Press {
            return None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::F(10) || (ctrl && keys::hotkey_char(&key) == Some('q')) {
            return Some(TasksIntent::Quit);
        }
        if ctrl {
            return None;
        }
        let last = self.rows().len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => return Some(TasksIntent::Close),
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => self.selected = (self.selected + 1).min(last),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(PAGE_STEP),
            KeyCode::PageDown => self.selected = (self.selected + PAGE_STEP).min(last),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = last,
            KeyCode::Enter => return self.selected_run().map(|r| TasksIntent::OpenRun(r.id)),
            KeyCode::F(6) => return self.selected_stoppable().map(TasksIntent::StopRun),
            KeyCode::Char(_) if keys::hotkey_char(&key) == Some('p') => {
                return self
                    .selected_run()
                    .map(|r| TasksIntent::OpenParent(r.parent));
            }
            _ => {}
        }
        None
    }

    /// The footer's hints for the selected row — built from the same values
    /// `handle_key` dispatches on, in the same frame (spec §11.2): `Enter`
    /// and `P` on a run row, `F6` only where a seat can be cancelled.
    fn hints(&self) -> Vec<(&'static str, &'static str, bool)> {
        let loc = self.loc;
        let mut hk = vec![("↑↓", loc.t("ui.tasks.hk.select"), false)];
        if self.selected_run().is_some() {
            hk.push(("Enter", loc.t("ui.tasks.hk.open"), false));
            hk.push(("P", loc.t("ui.tasks.hk.parent"), false));
        }
        if self.selected_stoppable().is_some() {
            hk.push(("F6", loc.t("ui.tasks.hk.stop"), false));
        }
        hk.extend([
            ("F1", loc.t("ui.tasks.hk.help"), false),
            ("Esc", loc.t("ui.tasks.hk.back"), false),
            ("Ctrl+Q", loc.t("ui.tasks.hk.quit"), false),
        ]);
        hk
    }

    /// The right-aligned title: how many are running, how many landed.
    fn summary(&self) -> Option<String> {
        let list = self.list.as_ref()?;
        let running = list.runs.iter().filter(|r| r.running).count();
        let landed = list.runs.len() - running + list.more_landed;
        Some(self.loc.tf(
            "ui.tasks.summary",
            &[
                ("running", &running.to_string()),
                ("landed", &landed.to_string()),
            ],
        ))
    }

    /// Draws the screen full-screen.
    pub fn render(&mut self, frame: &mut Frame) {
        self.last_drawn = Instant::now();
        let palette = self.palette;
        let loc = self.loc;
        let hk = self.hints();
        let chrome = screen_chrome(
            frame,
            &palette,
            format!(
                "{} {}",
                palette.glyphs().title_marker,
                loc.t("ui.tasks.title")
            ),
            self.summary(),
            &hk,
        );
        let (inner, status_area, hotkeys) = (chrome.inner, chrome.status, chrome.hotkeys);
        frame.render_widget(Paragraph::new(hotkeys), status_area);

        let Some(list) = &self.list else {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    loc.t("ui.tasks.loading"),
                    palette.muted_style(),
                )),
                inner,
            );
            return;
        };

        let lines = self.lines(list, inner.width as usize, Utc::now());
        let total = lines.len();
        let view = inner.height as usize;
        let selected_line = lines
            .iter()
            .position(|(_, row)| *row == Some(self.selected))
            .unwrap_or(0);
        self.scroll = keep_visible(self.scroll, selected_line, view);
        let shown: Vec<Line<'static>> = lines
            .into_iter()
            .skip(self.scroll)
            .take(view)
            .map(|(line, _)| line)
            .collect();
        frame.render_widget(Paragraph::new(shown), inner);
        render_scrollbar(
            frame,
            chrome.panel,
            total,
            view,
            self.scroll,
            true,
            &palette,
        );
    }

    /// The display lines, each with the selectable row it draws (`None` for a
    /// header, the remainder line, a blank): the run section, then the app's
    /// own work.
    fn lines(
        &self,
        list: &TaskList,
        width: usize,
        now: DateTime<Utc>,
    ) -> Vec<(Line<'static>, Option<usize>)> {
        let p = &self.palette;
        let loc = self.loc;
        let header = |key: &str| {
            (
                Line::from(Span::styled(
                    loc.t(key).to_string(),
                    Style::new().fg(p.accent).add_modifier(Modifier::BOLD),
                )),
                None,
            )
        };
        let mut out = vec![header("ui.tasks.sec.runs")];
        if list.runs.is_empty() {
            // Wrapped: the line names the four tools that would put a row
            // here, and a cut at the panel's edge would lose exactly them.
            let note = Line::from(Span::styled(
                format!("  {}", loc.t("ui.tasks.no_runs")),
                p.muted_style(),
            ));
            out.extend(
                wrap::wrap_line(&note, width.max(1))
                    .into_iter()
                    .map(|line| (line, None)),
            );
        } else {
            let cols = Columns::measure(self, list, width, now);
            for (i, run) in list.runs.iter().enumerate() {
                let selected = self.selected_row() == Some(Row::Run(i));
                out.push((self.run_line(run, &cols, selected, now), Some(i)));
            }
            if list.more_landed > 0 {
                out.push((
                    Line::from(Span::styled(
                        format!(
                            "  {}",
                            loc.tf("ui.tasks.more", &[("n", &list.more_landed.to_string())])
                        ),
                        p.muted_style(),
                    )),
                    None,
                ));
            }
        }
        out.push((Line::default(), None));
        out.push(header("ui.tasks.sec.app"));
        let base = list.runs.len();
        let label_w = list
            .app
            .iter()
            .map(|t| wrap::str_width(loc.t(app_label_key(t.kind))))
            .max()
            .unwrap_or(0);
        for (i, task) in list.app.iter().enumerate() {
            let selected = self.selected_row() == Some(Row::App(i));
            out.push((self.app_line(task, label_w, selected), Some(base + i)));
        }
        out
    }

    /// One run's row: marker, state glyph, title, parent chat, position or
    /// outcome, elapsed or finish time, tokens — in the measured columns.
    fn run_line(
        &self,
        run: &TaskRun,
        cols: &Columns,
        selected: bool,
        now: DateTime<Utc>,
    ) -> Line<'static> {
        let p = &self.palette;
        let (glyph, glyph_style) = self.glyph_of(run);
        let title_style = if selected {
            Style::new().fg(p.text).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(p.text)
        };
        let (title, title_w) = wrap::truncate_to_width(&run.title, cols.title);
        let mut spans = vec![
            Span::styled(
                if selected { "▌" } else { " " }.to_string(),
                p.accent_style(),
            ),
            Span::styled(format!("{glyph} "), glyph_style),
            Span::styled(
                format!(
                    "{title}{:pad$}",
                    "",
                    pad = cols.title.saturating_sub(title_w)
                ),
                title_style,
            ),
        ];
        if cols.parent > 0 {
            let (parent, w) = wrap::truncate_to_width(&run.parent_title, cols.parent);
            spans.push(Span::styled(
                format!("  {parent}{:pad$}", "", pad = cols.parent.saturating_sub(w)),
                p.muted_style(),
            ));
        }
        if cols.state > 0 {
            let (state, w) = wrap::truncate_to_width(&self.state_of(run), cols.state);
            spans.push(Span::styled(
                format!("  {state}{:pad$}", "", pad = cols.state.saturating_sub(w)),
                if run.running {
                    Style::new().fg(p.text)
                } else {
                    glyph_style
                },
            ));
        }
        spans.push(Span::styled(
            format!("  {:>w$}", time_of(run, now), w = cols.time),
            p.muted_style(),
        ));
        if cols.tokens > 0 {
            spans.push(Span::styled(
                format!("  {:>w$}", tokens_label(run.tokens), w = cols.tokens),
                p.muted_style(),
            ));
        }
        Line::from(spans)
    }

    /// One silent task's row: the label, and running or idle.
    fn app_line(&self, task: &AppTask, label_w: usize, selected: bool) -> Line<'static> {
        let p = &self.palette;
        let loc = self.loc;
        let label = loc.t(app_label_key(task.kind));
        let pad = label_w.saturating_sub(wrap::str_width(label));
        let (glyph, state, state_style) = if task.running {
            (
                p.glyphs().background,
                loc.t("ui.tasks.app.running"),
                p.accent_style(),
            )
        } else {
            (" ", loc.t("ui.tasks.app.idle"), p.muted_style())
        };
        Line::from(vec![
            Span::styled(
                if selected { "▌" } else { " " }.to_string(),
                p.accent_style(),
            ),
            Span::styled(format!("{glyph} "), p.accent_style()),
            Span::styled(
                format!("{label}{:pad$}", "", pad = pad),
                if selected {
                    Style::new().fg(p.text).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(p.text)
                },
            ),
            Span::styled(format!("  {state}"), state_style),
        ])
    }

    /// The state glyph and its colour: running, completed, ended badly, or
    /// ended short (round limit / never reported).
    fn glyph_of(&self, run: &TaskRun) -> (&'static str, Style) {
        let p = &self.palette;
        let g = p.glyphs();
        if run.running {
            return (g.busy, p.accent_style());
        }
        match run.outcome {
            Some(RunOutcome::Completed) => (g.ok, p.success_style()),
            Some(RunOutcome::Cancelled | RunOutcome::TimedOut | RunOutcome::Failed) => {
                (g.failed, Style::new().fg(p.error))
            }
            Some(RunOutcome::RoundLimit) | None => (g.warn, Style::new().fg(p.warning)),
        }
    }

    /// The state column: a running run's position (its latest step), a
    /// landed run's outcome in the chat list's words.
    fn state_of(&self, run: &TaskRun) -> String {
        let loc = self.loc;
        if run.running {
            return match &run.position {
                None => loc.t("ui.tasks.pos.starting").to_string(),
                Some(pos) => {
                    let n = pos.round.to_string();
                    match (pos.kind, &pos.tool) {
                        (RunProgressKind::DialogueLine, _) => {
                            loc.tf("ui.tasks.pos.line", &[("line", &n)])
                        }
                        (RunProgressKind::DialogueDirector, _) => {
                            loc.t("ui.tasks.pos.director").to_string()
                        }
                        (RunProgressKind::Subagent, Some(tool)) => {
                            loc.tf("ui.tasks.pos.round_tool", &[("round", &n), ("tool", tool)])
                        }
                        (RunProgressKind::Subagent, None) => {
                            loc.tf("ui.tasks.pos.round", &[("round", &n)])
                        }
                    }
                }
            };
        }
        match run_state_key(run.outcome, false, run.background) {
            Some(key) => loc.t(key).to_string(),
            None => loc.t("ui.tasks.completed").to_string(),
        }
    }
}

/// The run rows' column widths for one frame: measured from the rows, so
/// every state and every time ends in the same column, then fitted to the
/// width — the right-hand columns yield before the title shrinks below
/// [`TITLE_MIN`], since the title is what identifies a row.
struct Columns {
    title: usize,
    parent: usize,
    state: usize,
    time: usize,
    tokens: usize,
}

impl Columns {
    fn measure(screen: &TasksScreen, list: &TaskList, width: usize, now: DateTime<Utc>) -> Self {
        let widest = |f: &dyn Fn(&TaskRun) -> String| {
            list.runs
                .iter()
                .map(|r| wrap::str_width(&f(r)))
                .max()
                .unwrap_or(0)
        };
        let mut cols = Columns {
            title: 0,
            parent: widest(&|r| r.parent_title.clone()).min(PARENT_MAX),
            state: widest(&|r| screen.state_of(r)).min(STATE_MAX),
            time: widest(&|r| time_of(r, now)),
            tokens: widest(&|r| tokens_label(r.tokens)),
        };
        // The marker, the glyph and its space; two columns of gap before
        // every right-hand column that is shown.
        let fixed = |c: &Columns| 3 + gap(c.parent) + gap(c.state) + 2 + c.time + gap(c.tokens);
        for shed in [
            &mut |c: &mut Columns| c.tokens = 0,
            &mut |c: &mut Columns| c.parent = 0,
            &mut |c: &mut Columns| c.state = 0,
        ] as [&mut dyn FnMut(&mut Columns); 3]
        {
            if width.saturating_sub(fixed(&cols)) >= TITLE_MIN {
                break;
            }
            shed(&mut cols);
        }
        cols.title = width.saturating_sub(fixed(&cols)).max(1);
        cols
    }
}

/// A right-hand column's width with its gap, or nothing when it is hidden.
fn gap(col: usize) -> usize {
    if col == 0 { 0 } else { 2 + col }
}

/// The bundle key of a silent task's row label.
fn app_label_key(kind: BackgroundKind) -> &'static str {
    match kind {
        BackgroundKind::Reflection => "ui.tasks.app.reflection",
        BackgroundKind::Consolidation => "ui.tasks.app.consolidation",
        BackgroundKind::SelfConsolidation => "ui.tasks.app.self_consolidation",
        BackgroundKind::Compaction => "ui.tasks.app.compaction",
    }
}

/// The time column: how long a running run has been out, rendered from
/// `created_at` (nothing is stored, research fork F5); when a landed one
/// finished — the clock today, the date before that.
fn time_of(run: &TaskRun, now: DateTime<Utc>) -> String {
    if run.running {
        elapsed_label(run.created_at, now)
    } else {
        finish_label(run.finished_at.unwrap_or(run.created_at), now)
    }
}

/// `m:ss` under an hour, `h:mm:ss` from then on; a clock ahead of the run's
/// start reads as zero rather than negative.
fn elapsed_label(created: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - created).num_seconds().max(0);
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// The local clock for a run that ended today, its local date otherwise.
fn finish_label(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let local = at.with_timezone(&chrono::Local);
    if local.date_naive() == now.with_timezone(&chrono::Local).date_naive() {
        local.format("%H:%M").to_string()
    } else {
        local.format("%Y-%m-%d").to_string()
    }
}

/// Tokens as the list shows them: bare under a thousand, `1.2k` under ten
/// thousand, `12k` above; nothing for zero (a run that has not reported).
fn tokens_label(tokens: u64) -> String {
    match tokens {
        0 => String::new(),
        t if t < 1_000 => t.to_string(),
        t if t < 10_000 => format!("{:.1}k", t as f64 / 1000.0),
        t => format!("{}k", t / 1000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::events::SubagentProgress;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn loc() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    fn en() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::En)
    }

    /// A landed, completed sub-agent run under a chat called "Plans".
    fn landed(title: &str) -> TaskRun {
        TaskRun {
            finished_at: Some(Utc::now() - chrono::Duration::minutes(5)),
            outcome: Some(RunOutcome::Completed),
            running: false,
            tokens: 3400,
            ..TaskRun::fixture(title)
        }
    }

    /// A background run out right now, inside `fs_read` on round 3.
    fn running(title: &str) -> TaskRun {
        TaskRun {
            tokens: 1200,
            position: Some(SubagentProgress {
                name: title.into(),
                round: 3,
                tool: Some("fs_read".into()),
                kind: RunProgressKind::Subagent,
            }),
            ..TaskRun::fixture(title)
        }
    }

    fn app_tasks() -> Vec<AppTask> {
        [
            BackgroundKind::Reflection,
            BackgroundKind::Consolidation,
            BackgroundKind::SelfConsolidation,
            BackgroundKind::Compaction,
        ]
        .into_iter()
        .map(|kind| AppTask {
            kind,
            running: kind == BackgroundKind::Reflection,
        })
        .collect()
    }

    fn list(runs: Vec<TaskRun>) -> TaskList {
        TaskList {
            runs,
            more_landed: 0,
            app: app_tasks(),
        }
    }

    /// A screen holding `runs` — the fixture every test starts from
    /// (docs/lessons.md §2: the third test that opens the same way is a
    /// fixture).
    fn screen(runs: Vec<TaskRun>) -> TasksScreen {
        let mut s = TasksScreen::new(Palette::default(), loc());
        s.set_list(list(runs));
        s
    }

    fn press(s: &mut TasksScreen, code: KeyCode) -> Option<TasksIntent> {
        s.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn keys(s: &TasksScreen) -> Vec<&'static str> {
        s.hints().iter().map(|(k, _, _)| *k).collect()
    }

    /// Rendering the screen and reading the buffer back as text.
    fn text_at(s: &mut TasksScreen, width: u16, height: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
        let buf = term.backend().buffer();
        let mut out = String::new();
        for y in buf.area.top()..buf.area.bottom() {
            for x in buf.area.left()..buf.area.right() {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn text(s: &mut TasksScreen) -> String {
        text_at(s, 100, 24)
    }

    /// The footer offers exactly the keys the selected row answers (spec
    /// §11.2): `Enter`/`P` on a run, `F6` only on a running background run,
    /// none of them on a silent task — and the key does what the hint says,
    /// or nothing where there is no hint.
    #[test]
    fn the_footer_and_the_keys_follow_the_selected_row() {
        let out = running("Critic");
        let done = landed("Reviewer");
        let (out_id, done_id, done_parent) = (out.id, done.id, done.parent);
        let mut s = screen(vec![out, done]);

        // The running background run: open, parent, stop.
        assert!(keys(&s).contains(&"Enter") && keys(&s).contains(&"P"));
        assert!(keys(&s).contains(&"F6"), "{:?}", s.hints());
        assert_eq!(
            press(&mut s, KeyCode::F(6)),
            Some(TasksIntent::StopRun(out_id))
        );
        assert_eq!(
            press(&mut s, KeyCode::Enter),
            Some(TasksIntent::OpenRun(out_id))
        );

        // A landed run: open and parent, but nothing to stop.
        press(&mut s, KeyCode::Down);
        assert!(keys(&s).contains(&"Enter"));
        assert!(!keys(&s).contains(&"F6"), "{:?}", s.hints());
        assert_eq!(press(&mut s, KeyCode::F(6)), None);
        assert_eq!(
            press(&mut s, KeyCode::Enter),
            Some(TasksIntent::OpenRun(done_id))
        );
        assert_eq!(
            press(&mut s, KeyCode::Char('p')),
            Some(TasksIntent::OpenParent(done_parent))
        );

        // A silent task: nothing to open, nothing to stop.
        press(&mut s, KeyCode::Down);
        for k in ["Enter", "P", "F6"] {
            assert!(
                !keys(&s).contains(&k),
                "{k} offered on a task row: {:?}",
                s.hints()
            );
        }
        assert_eq!(press(&mut s, KeyCode::Enter), None);
        assert_eq!(press(&mut s, KeyCode::F(6)), None);
        assert_eq!(press(&mut s, KeyCode::Char('p')), None);

        // The two global keys, always.
        assert!(keys(&s).contains(&"F1") && keys(&s).contains(&"Esc"));
    }

    /// A child of the turn in flight is running but has no seat of its own:
    /// `F6` would name a run the stop command ignores, so it is not offered.
    #[test]
    fn a_turn_child_cannot_be_stopped_from_here() {
        let mut child = running("Helper");
        child.background = false;
        let mut s = screen(vec![child]);
        assert!(!keys(&s).contains(&"F6"), "{:?}", s.hints());
        assert_eq!(press(&mut s, KeyCode::F(6)), None);
        assert!(keys(&s).contains(&"Enter"), "it can still be opened");
    }

    /// Every hint the footer can show has a row in the help section (the
    /// per-screen gate the other screens carry) — `F1` and `Ctrl+Q` are the
    /// "Globally" section's.
    #[test]
    fn every_footer_key_is_in_the_help_section() {
        let s = screen(vec![running("Critic")]);
        for key in keys(&s) {
            if key == "F1" || key == "Ctrl+Q" || key == "↑↓" {
                continue;
            }
            assert!(
                HELP_SECTION.rows.iter().any(|(k, _)| *k == key),
                "{key} is advertised but has no help row"
            );
        }
        assert!(HELP_SECTION.rows.iter().any(|(k, _)| *k == "↑/↓"));
    }

    #[test]
    fn both_sections_and_every_row_are_drawn() {
        let mut s = screen(vec![running("Critic"), landed("Reviewer")]);
        let shown = text(&mut s);
        for needle in [
            loc().t("ui.tasks.sec.runs"),
            loc().t("ui.tasks.sec.app"),
            "Critic",
            "Reviewer",
            "Plans",
            "fs_read",
            "3.4k",
            "1.2k",
            loc().t("ui.tasks.completed"),
            loc().t("ui.tasks.app.reflection"),
            loc().t("ui.tasks.app.running"),
            loc().t("ui.tasks.app.idle"),
        ] {
            assert!(shown.contains(needle), "missing {needle:?}:\n{shown}");
        }
    }

    /// Every state and every time ends in one column across the rows: sized
    /// per row, `round 3 · fs_read` and `completed` would start at different
    /// offsets and the eye could not scan down them.
    #[test]
    fn the_columns_line_up_across_rows() {
        let mut s = screen(vec![running("Critic"), landed("Reviewer")]);
        let shown = text(&mut s);
        let row = |needle: &str| -> String {
            shown
                .lines()
                .find(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("no row with {needle}: {shown}"))
                .to_string()
        };
        // Columns, not bytes: the rows hold Cyrillic and box glyphs.
        let col_of = |row: &str, needle: &str| row[..row.find(needle).unwrap()].chars().count();
        let (a, b) = (row("Critic"), row("Reviewer"));
        assert_eq!(col_of(&a, "Plans"), col_of(&b, "Plans"), "{shown}");
        assert_eq!(col_of(&a, "1.2k"), col_of(&b, "3.4k"), "{shown}");
    }

    /// Before the orchestrator answers the screen says it is waiting; an
    /// answered, empty list says what would put something here; the cap
    /// says how much it hid (docs/lessons.md §4).
    #[test]
    fn waiting_empty_and_capped_states_explain_themselves() {
        let mut s = TasksScreen::new(Palette::default(), loc());
        assert!(text(&mut s).contains(loc().t("ui.tasks.loading")));
        assert!(
            keys(&s).contains(&"Esc"),
            "a waiting screen can still be left"
        );

        s.set_list(list(Vec::new()));
        let shown = text(&mut s);
        assert!(shown.contains("call_subagent"), "{shown}");
        assert!(
            shown.contains(loc().t("ui.tasks.app.reflection")),
            "the app's own rows are there even with no runs: {shown}"
        );

        let mut capped = list(vec![landed("Reviewer")]);
        capped.more_landed = 7;
        s.set_list(capped);
        let shown = text(&mut s);
        assert!(shown.contains("7"), "{shown}");
        assert!(
            shown.contains(&loc().tf("ui.tasks.more", &[("n", "7")])),
            "{shown}"
        );
    }

    /// The right-hand title counts the landed runs **including** the ones
    /// past the cap — that is what "landed" means to the user.
    #[test]
    fn the_summary_counts_past_the_cap() {
        let mut capped = list(vec![running("Critic"), landed("Reviewer")]);
        capped.more_landed = 7;
        let mut s = TasksScreen::new(Palette::default(), en());
        s.set_list(capped);
        assert_eq!(s.summary().as_deref(), Some("1 running · 8 landed"));
    }

    /// The selection walks the runs then the tasks, and cannot leave either end.
    #[test]
    fn the_selection_is_clamped_at_both_ends() {
        let mut s = screen(vec![landed("a"), landed("b")]);
        for _ in 0..5 {
            press(&mut s, KeyCode::Up);
        }
        assert_eq!(s.selected, 0);
        press(&mut s, KeyCode::End);
        assert_eq!(s.selected, 2 + app_tasks().len() - 1);
        for _ in 0..5 {
            press(&mut s, KeyCode::Down);
        }
        assert_eq!(s.selected, 2 + app_tasks().len() - 1);
        press(&mut s, KeyCode::Home);
        assert_eq!(s.selected, 0);
        press(&mut s, KeyCode::PageDown);
        assert_eq!(
            s.selected,
            2 + app_tasks().len() - 1,
            "a page past the end stops at it"
        );
    }

    /// A fresh snapshot keeps the selection on the same run, wherever its row
    /// moved: a run landing above it would otherwise shift `Enter` onto a
    /// neighbour the user never chose.
    #[test]
    fn the_selection_follows_the_run_not_the_index() {
        let (a, b) = (running("a"), landed("b"));
        let b_id = b.id;
        let mut s = screen(vec![a.clone(), b.clone()]);
        press(&mut s, KeyCode::Down);
        assert_eq!(s.selected_run().map(|r| r.id), Some(b_id));
        // "a" landed newest-first above "b", and a third run appeared before both.
        let mut a_landed = a;
        a_landed.running = false;
        a_landed.outcome = Some(RunOutcome::Cancelled);
        s.set_list(list(vec![running("c"), a_landed, b]));
        assert_eq!(s.selected_run().map(|r| r.id), Some(b_id));
        // A task row is kept by kind the same way.
        press(&mut s, KeyCode::End);
        s.set_list(list(vec![running("d")]));
        assert_eq!(
            s.selected_row(),
            Some(Row::App(app_tasks().len() - 1)),
            "the last task is still selected"
        );
    }

    /// …and a selection whose run is gone is clamped rather than left dangling.
    #[test]
    fn losing_the_selected_run_clamps() {
        let mut s = screen(vec![landed("a")]);
        s.set_list(TaskList::default());
        assert_eq!(s.selected, 0);
        assert_eq!(press(&mut s, KeyCode::Enter), None);
    }

    /// The selected row stays on screen as the selection moves through a
    /// list taller than the panel.
    #[test]
    fn the_selection_stays_visible_when_the_list_is_tall() {
        let runs: Vec<TaskRun> = (0..30).map(|i| landed(&format!("run-{i:02}"))).collect();
        let mut s = screen(runs);
        press(&mut s, KeyCode::End);
        let shown = text_at(&mut s, 100, 12);
        assert!(
            shown.contains(loc().t("ui.tasks.app.compaction")),
            "the last task row must be visible after End:\n{shown}"
        );
        assert!(!shown.contains("run-00"), "the top scrolled away:\n{shown}");
        press(&mut s, KeyCode::Home);
        let shown = text_at(&mut s, 100, 12);
        assert!(shown.contains("run-00"), "{shown}");
    }

    /// A narrow terminal drops the right-hand columns before it cuts the
    /// title down: the title is what identifies a row.
    #[test]
    fn a_narrow_screen_keeps_the_title() {
        let mut s = screen(vec![running("A run with a fairly long title")]);
        let shown = text_at(&mut s, 40, 12);
        assert!(
            shown.contains("A run with"),
            "the title start must survive at 40 columns:\n{shown}"
        );
    }

    /// The elapsed column is derived from `created_at` and a clock — nothing
    /// is stored, and the format switches to hours past sixty minutes.
    #[test]
    fn elapsed_and_finish_labels() {
        let t0 = Utc::now();
        let at = |secs: i64| t0 + chrono::Duration::seconds(secs);
        assert_eq!(elapsed_label(t0, at(0)), "0:00");
        assert_eq!(elapsed_label(t0, at(252)), "4:12");
        assert_eq!(elapsed_label(t0, at(3600 + 65)), "1:01:05");
        assert_eq!(
            elapsed_label(at(10), t0),
            "0:00",
            "a clock behind the start"
        );
        // Today — a clock; another day — a date.
        assert_eq!(finish_label(t0, t0).len(), 5);
        let long_ago = t0 - chrono::Duration::days(3);
        assert_eq!(finish_label(long_ago, t0).len(), 10);
    }

    #[test]
    fn tokens_label_scales() {
        assert_eq!(tokens_label(0), "");
        assert_eq!(tokens_label(999), "999");
        assert_eq!(tokens_label(1234), "1.2k");
        assert_eq!(tokens_label(12_345), "12k");
    }

    /// A running row keeps the tick alive; a landed list stops it (research
    /// R6: no timer that runs when nothing moves).
    #[test]
    fn the_repaint_tick_follows_the_running_rows() {
        let mut s = screen(vec![landed("a")]);
        assert!(!s.has_running());
        assert!(!s.needs_repaint());
        s.set_list(list(vec![running("b")]));
        assert!(s.has_running());
        assert!(!s.needs_repaint(), "just drawn — no repaint yet");
        s.last_drawn = Instant::now() - REPAINT_EVERY;
        assert!(s.needs_repaint());
    }

    /// The position column words every kind of step, and a run with none
    /// yet says it is starting rather than showing a blank.
    #[test]
    fn the_position_is_worded_for_every_kind() {
        let s = TasksScreen::new(Palette::default(), en());
        let mut run = running("Critic");
        assert_eq!(s.state_of(&run), "round 3 · fs_read");
        run.position.as_mut().unwrap().tool = None;
        assert_eq!(s.state_of(&run), "round 3");
        run.position.as_mut().unwrap().kind = RunProgressKind::DialogueLine;
        assert_eq!(s.state_of(&run), "line 3");
        run.position.as_mut().unwrap().kind = RunProgressKind::DialogueDirector;
        assert_eq!(s.state_of(&run), "director");
        run.position = None;
        assert_eq!(s.state_of(&run), "starting");
        // Landed: the chat list's words, plus "completed" which the list omits.
        let mut done = landed("Reviewer");
        assert_eq!(s.state_of(&done), "completed");
        done.outcome = Some(RunOutcome::Cancelled);
        assert_eq!(s.state_of(&done), en().t("ui.chatlist.run.cancelled"));
        done.outcome = None;
        assert_eq!(s.state_of(&done), en().t("ui.chatlist.run.unfinished"));
        done.background = false;
        assert_eq!(s.state_of(&done), en().t("ui.chatlist.run.interrupted"));
    }

    #[test]
    fn quit_and_close() {
        let mut s = screen(vec![landed("a")]);
        assert_eq!(press(&mut s, KeyCode::F(10)), Some(TasksIntent::Quit));
        assert_eq!(press(&mut s, KeyCode::Esc), Some(TasksIntent::Close));
    }
}
