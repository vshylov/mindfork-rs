//! The help/"About" dialog (`F1`): the tabs, the per-screen hotkey
//! sections and their rendering. A widget of its own so the runtime can draw
//! it over whatever screen is active (docs/history/help-hotkeys-context.md, stage 2);
//! it grew up in `screens/chat/popups.rs` and moved here unchanged.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph};

use crate::shared::credits;
use crate::shared::i18n::Locale;
use crate::shared::keys;
use crate::shared::theme::Palette;
use crate::shared::ui::{centered_rect, render_scrollbar};
use crate::shared::wrap;
use crate::widgets::logo::{LOCKUP_COLS, LOCKUP_ROWS, lockup_lines};

/// Left indent of the lockup — matches where the hotkey list starts.
const LOGO_INDENT: u16 = 2;

/// A tab of the help/"About" dialog (`F1`), KDE/Qt-style. See spec §11.7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpTab {
    /// Name/author/version and links (site, repository, crate).
    About,
    /// The hotkey list.
    Hotkeys,
    /// Input-box commands (`/rag …`, `/tts …`).
    Commands,
    /// The application's license text (MIT).
    License,
    /// The disclaimer covering model output, tools and automated actions
    /// (`DISCLAIMER.md`) — a supplement to the license, not part of it.
    Disclaimer,
    /// Third-party components, their versions and licenses.
    Components,
}

impl HelpTab {
    /// Tabs in display order (the tab strip's order).
    pub const ALL: [HelpTab; 6] = [
        Self::About,
        Self::Hotkeys,
        Self::Commands,
        Self::License,
        Self::Disclaimer,
        Self::Components,
    ];

    /// The tab's position in [`Self::ALL`].
    fn index(self) -> usize {
        Self::ALL.iter().position(|&t| t == self).unwrap()
    }

    /// The locale key for the tab's name (for the tab strip).
    pub fn label_key(self) -> &'static str {
        match self {
            Self::About => "ui.help.tab.about",
            Self::Hotkeys => "ui.help.tab.hotkeys",
            Self::Commands => "ui.help.tab.commands",
            Self::License => "ui.help.tab.license",
            Self::Disclaimer => "ui.help.tab.disclaimer",
            Self::Components => "ui.help.tab.components",
        }
    }
}

/// The tab the help dialog opens on by default (and until a choice is first
/// remembered): `F1` — the familiar help key, and "Hotkeys" is the most
/// sought-after content; "About" is the neighboring tab.
pub const DEFAULT_HELP_TAB: HelpTab = HelpTab::Hotkeys;

/// The screen the help dialog was opened from — the "Shortcuts" tab marks that
/// screen's section "you are here" ([`HELP_SECTIONS`]) and, opened anywhere but
/// the chat, scrolls to it (docs/history/help-hotkeys-context.md, forks F1/F2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpContext {
    /// The chat screen (the feed + the input box).
    Chat,
    /// The chat-list overlay (`Esc` from the chat).
    ChatList,
    /// The settings screen (`Ctrl+P`).
    Settings,
    /// The self-model screen (`F3`).
    SelfModel,
    /// The attached project's changes screen (`F4`).
    Changes,
    /// The message-search results (`Ctrl+G` in the chat list).
    Search,
}

/// The help dialog's state: the active tab + its content's scroll position
/// (reset on tab switch), and the screen it was opened from. Owned by the
/// runtime and drawn over whatever screen is active; opens on `F1` (any
/// screen), `?` and `/help` (the chat). See spec §11.7.
pub struct HelpState {
    pub tab: HelpTab,
    /// The first visible row of the active tab's content (clamped in `render_help`).
    pub scroll: usize,
    /// The invoking screen, for the "you are here" marker.
    pub context: HelpContext,
    /// Scroll the "Shortcuts" tab to the invoking screen's section on the
    /// next render (fork F2 — a non-chat opener). Consumed at render time,
    /// where the width — and therefore the wrapped rows above the section —
    /// is known.
    pending_anchor: bool,
}

impl HelpState {
    /// Open from the chat on the given tab (on reopening — the last-selected
    /// one, the runtime's `help_last_tab` memory), scrolled to the top: the
    /// chat's section sits right under the short "Everywhere" block, so an
    /// anchor would only hide the latter (fork F2).
    pub fn open(tab: HelpTab) -> Self {
        Self {
            tab,
            scroll: 0,
            context: HelpContext::Chat,
            pending_anchor: false,
        }
    }

    /// Open from any other screen: always the "Shortcuts" tab, scrolled to
    /// that screen's section with its "you are here" header on top (fork F2).
    pub fn open_at(context: HelpContext) -> Self {
        Self {
            tab: HelpTab::Hotkeys,
            scroll: 0,
            context,
            pending_anchor: true,
        }
    }

    /// The next tab (wrapping); resets scroll.
    pub fn next_tab(&mut self) {
        let n = HelpTab::ALL.len();
        self.tab = HelpTab::ALL[(self.tab.index() + 1) % n];
        self.scroll = 0;
    }

    /// The previous tab (wrapping); resets scroll.
    pub fn prev_tab(&mut self) {
        let n = HelpTab::ALL.len();
        self.tab = HelpTab::ALL[(self.tab.index() + n - 1) % n];
        self.scroll = 0;
    }

    /// One key of the open dialog: `Tab`/`←→` switch tabs, `↑↓`/`PgUp`/`PgDn`/
    /// `Home` scroll the active tab, `Esc` (and a repeat `F1`) close,
    /// `Ctrl+Q`/`F10` quit — the layout-independent punch-through every modal
    /// has. Other keys are ignored (they don't close it — otherwise navigation
    /// would get confusing). Scroll clamping — in [`render_help`]. The caller
    /// closes/quits on the returned outcome; on [`HelpKeyOutcome::Close`] the
    /// final tab is still readable for the last-tab memory. See spec §11.7.
    pub fn handle_key(&mut self, key: &KeyEvent) -> HelpKeyOutcome {
        if key.code == KeyCode::F(10)
            || (key.modifiers.contains(KeyModifiers::CONTROL)
                && keys::hotkey_char(key) == Some('q'))
        {
            return HelpKeyOutcome::Quit;
        }
        match key.code {
            KeyCode::Tab | KeyCode::Right => self.next_tab(),
            KeyCode::BackTab | KeyCode::Left => self.prev_tab(),
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down => self.scroll = self.scroll.saturating_add(1),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(HELP_PAGE_SCROLL),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(HELP_PAGE_SCROLL),
            KeyCode::Home => self.scroll = 0,
            KeyCode::Esc | KeyCode::F(1) => return HelpKeyOutcome::Close,
            _ => {}
        }
        HelpKeyOutcome::Handled
    }
}

/// What a key did to the open dialog ([`HelpState::handle_key`]): stayed
/// inside, asked to close, or asked to quit the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpKeyOutcome {
    /// Consumed (navigation, or ignored) — the dialog stays open.
    Handled,
    /// Close the dialog (`Esc`, a repeat `F1`); the owner remembers
    /// [`HelpState::tab`] for the next open.
    Close,
    /// Quit the application (`Ctrl+Q`/`F10` punch through the dialog).
    Quit,
}

/// The dialog's `PgUp`/`PgDn` step, in rows — the chat feed's page step, which
/// is where the dialog's keys grew up.
const HELP_PAGE_SCROLL: usize = 8;

/// One section of the "Shortcuts" tab (`F1`): the keys of one screen under
/// a localized header. A key is listed once per screen where it does something,
/// so a chord with two meanings is two short rows in two sections — the header
/// says the context the descriptions used to spell out ("in the chat list: …")
/// per locale, or dropped entirely (the settings screen's keys were absent).
/// Rows are `(keycap, desc_key)` pairs: `keycap` is a literal "key" (ASCII,
/// universal) **or** a `ui.*` key where the label itself has words (mouse,
/// typing); `desc_key` is always a `ui.*` description key. Both are resolved
/// through the locale in [`key_lines`]. Input-box commands (`/…`) are split out
/// into [`HELP_COMMANDS`] (a separate tab). See spec §11.7,
/// docs/history/help-hotkeys-context.md, docs/i18n-ui.md.
pub struct HelpSection {
    /// Locale key of the header (`ui.help.sec.*`). The route to the screen is
    /// part of the localized title ("Settings (Ctrl+P)") — the header doubles
    /// as "how do I get there".
    pub title: &'static str,
    /// The screen this section documents — the one whose header carries the
    /// "you are here" marker when the dialog was opened from it. `None` — the
    /// "Globally" rows, never marked.
    pub context: Option<HelpContext>,
    /// The section's `(keycap, desc_key)` rows.
    pub rows: &'static [(&'static str, &'static str)],
    /// The rows that open an intra-section display group: a blank line is
    /// drawn before each (never before the section's first row — the header
    /// already separates). Encoded **outside** the rows on purpose: the rows
    /// are long-lived, and a line added among them lands inside the region the
    /// SonarQube copy-paste detector flags on these uniform tuple tables
    /// (docs/lessons.md §2). `group_openers_open_real_rows` pins each opener
    /// to exactly one row of its section.
    pub openers: &'static [&'static str],
}

/// The "Globally" rows — the keys that mean the same thing on every
/// screen. The other sections live **next to the key handlers they document**
/// (the chat's in `screens/chat`, the list's in `widgets/chat_list`, …), and
/// the app layer composes the display order — the one place that knows every
/// screen exists (docs/history/help-hotkeys-context.md §6). Proximity of a table to
/// its `match` is the anti-drift force, and AGENTS.md §3 sends every new key
/// to its owner's table.
pub static GLOBAL: HelpSection = HelpSection {
    title: "ui.help.sec.global",
    context: None,
    // `F1` is routed at the runtime level, above every screen — and it is the
    // only help key: `?` was listed here while it worked on the chat screen
    // alone, and on an empty input box at that (spec §11.7). `/help` is the
    // typed route.
    rows: &[("F1", "ui.help.help"), ("Ctrl+Q / F10", "ui.help.quit")],
    openers: &[],
};

/// Input-box commands for the "Commands" tab (`F1`). Same format as
/// [`HELP_KEYS`], with the display groups likewise encoded outside the table
/// ([`COMMAND_GROUP_OPENERS`]); a command label (`/…`) is drawn in the command
/// color. Split out of the hotkeys so they don't clutter reading them. See
/// spec §11.7.
pub const HELP_COMMANDS: &[(&str, &str)] = &[
    // Attachments come first — they are the commands a user reaches for while
    // writing a message (see docs/file-attachments.md §4.8).
    ("ui.help.k.file_attach", "ui.help.file_attach"),
    ("ui.help.k.file_remove", "ui.help.file_remove"),
    ("/file list", "ui.help.file_list"),
    // Images sit right after the files: the same verbs and the same `#N`
    // addressing, but staged for the **next message** rather than pinned to the
    // chat — the descriptions carry that difference (spec §9.10).
    ("ui.help.k.image_attach", "ui.help.image_attach"),
    ("ui.help.k.image_remove", "ui.help.image_remove"),
    ("/image list", "ui.help.image_list"),
    // Listed as a command, not only as `Ctrl+V`: whether that key ever reaches the app is
    // the terminal's decision (Windows Terminal binds it to its own paste), and an image
    // on the clipboard produces no text for the terminal to inject.
    ("/image paste", "ui.help.image_paste"),
    // A project is the third thing attached to *this chat*, after files and
    // images — and the only one the assistant reaches through tools rather than
    // through the prompt (spec §9.12).
    ("ui.help.k.project_attach", "ui.help.project_attach"),
    ("/project detach", "ui.help.project_detach"),
    ("/project status", "ui.help.project_status"),
    // The command slots: a project the assistant can only read is half the
    // feature, and these two rows are the only way to give it the other half.
    ("ui.help.k.project_cmd", "ui.help.project_cmd"),
    ("ui.help.k.project_clear", "ui.help.project_clear"),
    // The other half of what the editing tools promise: they change a file
    // without asking, and this is where the user sees what changed and puts any
    // of it back (spec §9.12).
    ("/changes", "ui.help.changes"),
    ("ui.help.k.rag_add", "ui.help.rag_add"),
    ("ui.help.k.rag_remove", "ui.help.rag_remove"),
    ("/rag list", "ui.help.rag_list"),
    ("/rag rebuild", "ui.help.rag_rebuild"),
    // Sits next to the knowledge-base commands, but is deliberately top-level:
    // it re-embeds notes, attachments and every profile's base at once.
    ("/reindex", "ui.help.reindex"),
    // Chat-scoped, unlike the two above — but still top-level, and it belongs
    // with the other "housekeeping" commands rather than among the attachments.
    ("/compact", "ui.help.compact"),
    ("ui.help.k.tts", "ui.help.tts"),
    ("/tts stop", "ui.help.tts_stop"),
    ("/tts pause · resume", "ui.help.tts_pause"),
    // The profile family closes this table because the registry's rows follow
    // it, and "the screens" (`/settings`, `/self`) is where profiles belong —
    // the two groups end up adjacent. `/profile` keeps a parser of its own: it
    // is the one typed route with a subcommand *and* an argument (stage 2).
    ("ui.help.k.export", "ui.help.cmd_export"),
    ("/profile list", "ui.help.cmd_profile_list"),
    ("ui.help.k.profile_new", "ui.help.cmd_profile_new"),
    ("ui.help.k.profile_delete", "ui.help.cmd_profile_delete"),
    // The profile's two text fields (stage 3) — editable without the settings
    // screen, the reserved word `clear` removing a value.
    ("ui.help.k.profile_system", "ui.help.cmd_profile_system"),
    ("ui.help.k.profile_greeting", "ui.help.cmd_profile_greeting"),
    // The impersonation profiles (the user personas, spec §11.8) — the same
    // family one list over, `/impersonation use` being the link the assistant
    // profile keeps (docs/history/commands-stage3.md §3.3).
    ("/impersonation list", "ui.help.cmd_imp_list"),
    ("ui.help.k.imp_new", "ui.help.cmd_imp_new"),
    ("ui.help.k.imp_delete", "ui.help.cmd_imp_delete"),
    ("ui.help.k.imp_use", "ui.help.cmd_imp_use"),
    ("ui.help.k.imp_system", "ui.help.cmd_imp_system"),
];

/// The way out closes the tab, whatever stands in front of it: this is the typed
/// route out, for terminals that keep `Ctrl+Q` and `F10` for themselves (VS
/// Code's integrated one binds both). Same reasoning as `/image paste` above.
/// Kept apart from [`HELP_COMMANDS`] so the registry's rows
/// ([`command_rows`]) can be inserted between the two without this one moving.
const HELP_COMMANDS_TAIL: &[(&str, &str)] = &[("/exit · /quit", "ui.help.exit")];

/// The "Commands" tab's rows: the commands with parsers of their own
/// ([`HELP_COMMANDS`]), then every typed route in the registry, then the way out
/// ([`HELP_COMMANDS_TAIL`]).
///
/// The middle section is **derived** rather than written out again, so a command
/// cannot exist without a help row — a table repeating the registry would be one
/// more place to forget (the `supported_sampling_fields` single-source pattern).
/// The registry's own order carries its four display groups; the openers below
/// name each group's first row.
pub fn command_rows() -> Vec<(&'static str, &'static str)> {
    HELP_COMMANDS
        .iter()
        .copied()
        .chain(
            crate::features::ui_command::COMMANDS
                .iter()
                .map(|s| (s.label, s.description)),
        )
        .chain(HELP_COMMANDS_TAIL.iter().copied())
        .collect()
}

/// [`COMMAND_GROUP_OPENERS`] is [`HelpSection::openers`] for the "Commands" tab:
/// files · images · the code project · the knowledge base · housekeeping ·
/// speech · the profiles · the impersonation profiles · the screens ·
/// the conversation · finding things · the feed · the way out. The last four
/// groups before the exit row are the typed routes ([`command_rows`]), grouped
/// the way the registry orders them.
const COMMAND_GROUP_OPENERS: &[&str] = &[
    "ui.help.k.image_attach",
    "ui.help.k.project_attach",
    "ui.help.k.rag_add",
    "/reindex",
    "ui.help.k.tts",
    "ui.help.k.export",
    "/profile list",
    "/impersonation list",
    "/settings",
    "ui.help.k.new",
    "ui.help.k.find",
    "/thoughts",
    "/exit · /quit",
];

/// Width bounds of the help/"About" dialog in columns (excluding the border).
/// The dialog follows the terminal between them ([`help_size`]): the minimum is
/// the `ru` tab strip's exact budget (its gate test measures against it), the
/// maximum caps line length for readability on wide terminals. One size for
/// every tab, so the window doesn't "jump" on switching.
pub const HELP_MIN_WIDTH: u16 = 76;
pub const HELP_MAX_WIDTH: u16 = 96;
/// Height bounds in rows (excluding the border) — same idea as the widths: more
/// rows on a tall terminal mean less scrolling through the key list.
const HELP_MIN_HEIGHT: u16 = 34;
const HELP_MAX_HEIGHT: u16 = 44;
/// Screen columns/rows the dialog leaves free around itself while growing
/// toward its maximum. Below the minimum it stops shrinking and
/// [`centered_rect`] clamps it to the screen instead (hard degradation).
const HELP_AIR: u16 = 6;

/// The dialog's content size for a `cols`×`rows` screen: grows with the
/// terminal between the min and max bounds, keeping [`HELP_AIR`] around
/// itself. The caller adds the border and hands the result to
/// [`centered_rect`], which clamps to the screen when even the minimum
/// doesn't fit.
pub fn help_size(cols: u16, rows: u16) -> (u16, u16) {
    (
        cols.saturating_sub(HELP_AIR)
            .clamp(HELP_MIN_WIDTH, HELP_MAX_WIDTH),
        rows.saturating_sub(HELP_AIR)
            .clamp(HELP_MIN_HEIGHT, HELP_MAX_HEIGHT),
    )
}

/// Draws the help/"About" dialog centered on screen (KDE/Qt-style): the logo
/// lockup, a tab strip, and scrollable content for the active tab with a
/// scrollbar on the right border. Navigation lives in [`ChatScreen::handle_key`];
/// `scroll` is clamped here (the popup's height is only known at render time).
/// See spec §11.7.
pub fn render_help(
    frame: &mut Frame,
    help: &mut HelpState,
    sections: &[&HelpSection],
    palette: &Palette,
    loc: &'static Locale,
) {
    let full = frame.area();
    let (content_w, content_h) = help_size(full.width, full.height);
    let area = centered_rect(content_w + 2, content_h + 2, full);
    frame.render_widget(Clear, area);

    // The title carries the brand name+version (language-neutral) next to the
    // localized title; the footer covers tab/scroll/close navigation.
    let title = format!(
        "{}{} · {} v{}",
        palette.glyphs().help_icon,
        loc.t("ui.help.title"),
        credits::APP_NAME,
        env!("CARGO_PKG_VERSION"),
    );
    let block = palette.panel(title, true).title_bottom(
        Line::from(Span::styled(
            loc.t("ui.help.footer.tabs"),
            palette.muted_style(),
        ))
        .centered(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let inner_w = inner.width as usize;

    // Header: (optional lockup + spacer) + tab strip + separator line. The
    // lockup is drawn only when there's room to spare (docs/branding.md §5) —
    // on a cramped terminal it would push the tabs aside; a hard degradation,
    // like the scrollbar/Mermaid.
    let show_logo = inner.height >= LOCKUP_ROWS + 6 && inner.width > LOCKUP_COLS + LOGO_INDENT;
    let mut header: Vec<Line> = Vec::new();
    if show_logo {
        let pad = " ".repeat(LOGO_INDENT as usize);
        header.push(Line::raw("")); // spacer above the logo
        header.extend(lockup_lines(palette.text).into_iter().map(|line| {
            let mut spans = vec![Span::raw(pad.clone())];
            spans.extend(line.spans);
            Line::from(spans)
        }));
        header.push(Line::raw(""));
    }
    header.push(help_tab_strip(help.tab, palette, loc));
    header.push(Line::styled(
        "─".repeat(inner_w),
        palette.border_style(false),
    ));
    let header_h = header.len() as u16;

    let [head_area, body_area] =
        Layout::vertical([Constraint::Length(header_h), Constraint::Min(0)]).areas(inner);
    frame.render_widget(Paragraph::new(header), head_area);

    // Content for the active tab (language-neutral data — license/components —
    // is read straight from `shared::credits`, bypassing locales).
    let content = match help.tab {
        HelpTab::About => about_lines(palette, loc, inner_w),
        HelpTab::Hotkeys => {
            let (lines, anchor) = hotkeys_tab(sections, help.context, palette, loc, inner_w);
            // A non-chat opener lands with its section's header on top (fork
            // F2); consumed here, where the wrapped rows above are known.
            if help.pending_anchor {
                help.scroll = anchor;
                help.pending_anchor = false;
            }
            lines
        }
        HelpTab::Commands => key_lines(
            &command_rows(),
            COMMAND_GROUP_OPENERS,
            palette,
            loc,
            inner_w,
        ),
        HelpTab::License => license_lines(palette, loc, inner_w),
        HelpTab::Disclaimer => disclaimer_lines(palette, loc, inner_w),
        HelpTab::Components => component_lines(palette, loc, inner_w),
    };
    let total = content.len();
    let view_h = body_area.height as usize;
    help.scroll = help.scroll.min(total.saturating_sub(view_h));
    frame.render_widget(
        Paragraph::new(Text::from(content)).scroll((help.scroll as u16, 0)),
        body_area,
    );
    // The scrollbar sits on the right border line along the content area (not
    // the full height, so the thumb doesn't intrude on the tab header).
    let bar_area = Rect {
        x: area.x,
        y: body_area.y,
        width: area.width,
        height: body_area.height,
    };
    render_scrollbar(frame, bar_area, total, view_h, help.scroll, true, palette);
}

/// Tab strip for the help dialog: `About │ Hotkeys │ …`. The active tab sits on
/// a muted backdrop, bold (like the selected settings tab); the `│` separator
/// and the content are WGL4-safe. The dialog is always modal (in focus), so the
/// active tab's highlight is always the "focused" one.
pub fn help_tab_strip(active: HelpTab, palette: &Palette, loc: &'static Locale) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
    for (i, tab) in HelpTab::ALL.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" │", palette.border_style(false)));
        }
        let label = loc.t(tab.label_key());
        let style = if *tab == active {
            Style::new().fg(palette.text).bg(palette.keycap_bg).bold()
        } else {
            palette.muted_style()
        };
        spans.push(Span::styled(format!(" {label} "), style));
    }
    Line::from(spans)
}

/// Left indent of the tab content (the same column as the lockup).
const HELP_PAD: &str = "  ";

/// The "About" tab: the name and tagline, then the facts — version, license
/// and build target, the links (site/crate/repository) and the author — as a
/// leader table, the geometry the "Components" tab already uses
/// ([`leader_row`]): the label on the left margin, the values in one column
/// anchored so the widest of them touches the mirrored right margin, and the
/// run between bridged by a dotted leader. Values used to start one column
/// past the widest label, which left the right ~30 columns of a wide dialog
/// empty — the same complaint the "Components" tab was fixed for, and the same
/// fix (user's decision, 2026-08-19). Links use the accent color (like
/// "command keys"), the labels are muted.
fn about_lines(palette: &Palette, loc: &'static Locale, width: usize) -> Vec<Line<'static>> {
    let rows: Vec<(Span<'static>, Span<'static>)> = [
        (
            loc.t("ui.about.version"),
            env!("CARGO_PKG_VERSION").to_string(),
            palette.text,
        ),
        (
            loc.t("ui.about.license"),
            credits::LICENSE_ID.to_string(),
            palette.text,
        ),
        (
            loc.t("ui.about.platform"),
            credits::platform(),
            palette.text,
        ),
        (
            loc.t("ui.about.site"),
            credits::SITE_URL.to_string(),
            palette.accent,
        ),
        (
            loc.t("ui.about.crate"),
            credits::CRATE_URL.to_string(),
            palette.accent,
        ),
        (
            loc.t("ui.about.repo"),
            credits::REPO_URL.to_string(),
            palette.accent,
        ),
        (
            loc.t("ui.about.author"),
            credits::AUTHOR.to_string(),
            palette.text,
        ),
    ]
    .into_iter()
    .map(|(label, value, color)| {
        (
            Span::styled(format!("{label}:"), palette.muted_style()),
            Span::styled(value, Style::new().fg(color)),
        )
    })
    .collect();
    // The one column every value starts in — measured in display columns, since
    // a label carries Cyrillic in `ru` and a value could carry anything.
    let value_col = width.saturating_sub(
        HELP_PAD.chars().count() + rows.iter().map(|(_, v)| span_width(v)).max().unwrap_or(0),
    );
    let mut lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            format!("{HELP_PAD}{}", credits::APP_NAME),
            Style::new().fg(palette.assistant).bold(),
        )),
        Line::from(Span::styled(
            format!("{HELP_PAD}{}", loc.t("ui.about.desc")),
            palette.muted_style(),
        )),
        Line::raw(""),
    ];
    // Items separated by a blank line — the list "breathes" (requested: spacing
    // between items). The leader carries the eye across the gap the spacing
    // opens up, which is what makes the anchored column readable.
    for (label, value) in rows {
        lines.push(Line::raw(""));
        lines.push(leader_row(label, vec![value], value_col, palette));
    }
    lines
}

/// A table's section for [`table_lines`]: an optional header (the localized
/// title key + whether it carries the "you are here" marker) over rows in the
/// [`HelpSection`] format. The "Commands" tab is one headerless section.
struct TabSection<'a> {
    header: Option<(&'a str, bool)>,
    rows: &'a [(&'a str, &'a str)],
    openers: &'a [&'a str],
}

/// The "Shortcuts" tab: the composed `sections` under their headers, the
/// section for `context` marked "you are here" (fork F1), plus that section's
/// header row — the anchor a non-chat opener scrolls to (fork F2). The anchor
/// is computed rather than stored, because the rows above the section wrap by
/// `width`, so it is only knowable where the width is. The caller (the app
/// layer) owns the composed list. See docs/history/help-hotkeys-context.md.
pub fn hotkeys_tab(
    sections: &[&HelpSection],
    context: HelpContext,
    palette: &Palette,
    loc: &'static Locale,
    width: usize,
) -> (Vec<Line<'static>>, usize) {
    let sections: Vec<TabSection<'_>> = sections
        .iter()
        .map(|s| TabSection {
            header: Some((s.title, s.context == Some(context))),
            rows: s.rows,
            openers: s.openers,
        })
        .collect();
    let (lines, marked) = table_lines(&sections, false, palette, loc, width);
    // Every context has a section (`help_sections_cover_every_context`), so
    // the fallback never fires in practice — but a missing anchor must
    // degrade to the top, not panic mid-render.
    (lines, marked.unwrap_or(0))
}

/// One headerless section of `(label, description)` rows — the "Commands" tab
/// (a command label `/…` uses the command color), and the shape the table
/// tests exercise directly.
fn key_lines(
    entries: &[(&str, &str)],
    openers: &[&str],
    palette: &Palette,
    loc: &'static Locale,
    width: usize,
) -> Vec<Line<'static>> {
    let section = TabSection {
        header: None,
        rows: entries,
        openers,
    };
    table_lines(std::slice::from_ref(&section), true, palette, loc, width).0
}

/// The "Shortcuts"/"Commands" tabs: sections of `(label, description)` rows —
/// a "key" + a description; a section's header row precedes its rows, and a
/// blank line is drawn before every row named in its `openers`, separating the
/// display groups. The locale resolves label keys, titles and descriptions.
/// `command_labels` draws `/…` labels in the command color (the "Commands"
/// tab); the hotkeys tab is keycaps throughout, so its `/` key row (the
/// settings search) is not mistaken for a command.
///
/// Every description starts in the **same column** — one past the whole tab's
/// widest label — and a description too long for `width` **wraps**, hung under
/// that column ([`push_key_entry`]), rather than being clipped at the dialog's
/// edge: the tab is a plain `Paragraph` with no wrapping of its own, so an
/// over-long row used to lose its tail mid-word. Both are per-locale work — a
/// row can fit in `en` and overflow in `ru`, so whoever writes the label never
/// sees it (docs/lessons.md §7), and the label lengths differ per locale too,
/// which is why the column is measured here (in display columns — a label can
/// carry `↔` or a box-drawing glyph, where `.len()` would be bytes) rather
/// than fixed as a constant. `help_rows_fit_the_dialog_in_every_locale` is the
/// gate. See spec §11.7.
fn table_lines(
    sections: &[TabSection<'_>],
    command_labels: bool,
    palette: &Palette,
    loc: &'static Locale,
    width: usize,
) -> (Vec<Line<'static>>, Option<usize>) {
    // Resolve every label up front: the description column is shared by the
    // whole tab, so it has to be measured before any row can be built. Both
    // label styles pad with a space on each side, so one measurement covers
    // keycaps and command labels alike.
    type Rows<'a> = (Option<(&'a str, bool)>, Vec<(bool, Span<'static>, String)>);
    let resolved: Vec<Rows<'_>> = sections
        .iter()
        .map(|section| {
            let rows = section
                .rows
                .iter()
                .map(|(k, d)| {
                    let key = loc.t(k).to_string();
                    let key_span = if command_labels && key.starts_with('/') {
                        Span::styled(format!(" {key} "), Style::new().fg(palette.warning))
                    } else {
                        palette.keycap(key)
                    };
                    (section.openers.contains(k), key_span, loc.t(d).to_string())
                })
                .collect();
            (section.header, rows)
        })
        .collect();
    let label_col = resolved
        .iter()
        .flat_map(|(_, rows)| rows.iter())
        .map(|(_, span, _)| span_width(span))
        .max()
        .unwrap_or(0);
    // Where every description starts: the indent + the label column + the
    // separating space.
    let indent = HELP_PAD.chars().count() + label_col + 1;
    let mut lines = vec![Line::raw("")];
    // The marked header's row — the anchor [`hotkeys_tab`] hands back.
    let mut marked_at = None;
    for (si, (header, rows)) in resolved.iter().enumerate() {
        if si > 0 {
            lines.push(Line::raw("")); // a breath between the sections
        }
        if let Some((title, marked)) = header {
            if *marked {
                marked_at = Some(lines.len());
            }
            lines.push(section_header(title, *marked, palette, loc, width));
        }
        for (opens_group, key_span, desc) in rows {
            if *opens_group {
                lines.push(Line::raw("")); // a breath between the groups
            }
            push_key_entry(&mut lines, key_span, desc, palette, width, indent);
        }
    }
    (lines, marked_at)
}

/// A section's header row: the localized title, the "you are here" marker when
/// the dialog was opened from that screen (docs/history/help-hotkeys-context.md, fork
/// F1), and a rule to the dialog's edge, so the sections read as chapters.
fn section_header(
    title: &str,
    marked: bool,
    palette: &Palette,
    loc: &'static Locale,
    width: usize,
) -> Line<'static> {
    let mut spans = vec![
        Span::raw(HELP_PAD),
        Span::styled(
            format!("{} ", loc.t(title)),
            Style::new().fg(palette.text).bold(),
        ),
    ];
    if marked {
        spans.push(Span::styled(
            format!("· {} ", loc.t("ui.help.here")),
            Style::new().fg(palette.accent).bold(),
        ));
    }
    let used: usize = spans.iter().map(span_width).sum();
    spans.push(Span::styled(
        "─".repeat(width.saturating_sub(used)),
        palette.border_style(false),
    ));
    Line::from(spans)
}

/// One entry of a [`key_lines`] table: the label row with its description
/// starting at the shared column `indent`, plus wrapped continuations hung
/// under that same column.
fn push_key_entry(
    lines: &mut Vec<Line<'static>>,
    key_span: &Span<'static>,
    desc: &str,
    palette: &Palette,
    width: usize,
    indent: usize,
) {
    // The gap that carries this row's description to the shared column.
    let gap = " ".repeat(indent - HELP_PAD.chars().count() - span_width(key_span));
    // `max(1)` only guards against a pathological label eating the whole
    // dialog (wrap_ranges must not be handed a zero width); with a label that
    // long the rows overflow anyway, and the gate test is what catches it.
    let body = width.saturating_sub(indent).max(1);
    let chars: Vec<char> = desc.chars().collect();
    for (i, (from, to)) in wrap::wrap_ranges(&chars, body).into_iter().enumerate() {
        // `wrap_ranges` spills the break's whitespace into the row it ends
        // (so the next row starts on a word); it is invisible, but it would
        // make a row measure wider than it draws.
        let text: String = chars[from..to].iter().collect();
        let text = text.trim_end().to_string();
        lines.push(if i == 0 {
            Line::from(vec![
                Span::raw(HELP_PAD),
                key_span.clone(),
                Span::raw(gap.clone()),
                Span::styled(text, Style::new().fg(palette.text)),
            ])
        } else {
            Line::from(vec![
                Span::raw(" ".repeat(indent)),
                Span::styled(text, Style::new().fg(palette.text)),
            ])
        });
    }
}

/// A span's width in terminal columns (not bytes, not `char`s).
fn span_width(span: &Span<'_>) -> usize {
    wrap::display_width(&span.content.chars().collect::<Vec<_>>())
}

/// The "License" tab: the app's license text (MIT), in the interface language —
/// `credits::license_text` picks the authoritative English original or its
/// Russian translation (docs/history/legal-ru-translations.md). Paragraphs (separated by
/// a blank line in the file) are reassembled and word-wrapped to the content
/// width `width` — the source's hard wrap at ~76 columns would otherwise clip on
/// the right, while wrapping line-by-line would leave orphaned words. Wrapping
/// produces logical lines, so the scroll/scrollbar model (by line count) isn't
/// broken. Assembly via `lines()` is CRLF-safe.
fn license_lines(palette: &Palette, loc: &'static Locale, width: usize) -> Vec<Line<'static>> {
    let body_w = width.saturating_sub(HELP_PAD.len()).max(1);
    // Assemble paragraphs: non-empty lines are joined with a space, an empty one
    // is a boundary.
    let mut paras: Vec<String> = Vec::new();
    let mut cur = String::new();
    for raw in credits::license_text(loc.lang()).lines() {
        if raw.trim().is_empty() {
            if !cur.is_empty() {
                paras.push(std::mem::take(&mut cur));
            }
        } else {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(raw.trim());
        }
    }
    if !cur.is_empty() {
        paras.push(cur);
    }

    let mut lines = vec![Line::raw("")];
    for para in paras {
        let src = Line::from(Span::styled(para, Style::new().fg(palette.text)));
        for wrapped in crate::shared::wrap::wrap_line(&src, body_w) {
            let mut spans = vec![Span::raw(HELP_PAD)];
            spans.extend(wrapped.spans);
            lines.push(Line::from(spans));
        }
        lines.push(Line::raw("")); // spacer between paragraphs
    }
    lines
}

/// The "Disclaimer" tab: `DISCLAIMER.md` — what the author does not answer for
/// when a model, chosen and downloaded by the user, writes every word on screen
/// (see spec §11.7). A supplement to the license, which is why it is a tab of
/// its own rather than a tail on the "License" tab.
///
/// Unlike the license, the source is markdown, so it goes through our own
/// renderer (ADR 0003) — headings, emphasis and bullets survive. The renderer
/// deliberately does not wrap paragraphs (that is the feed's job), so the
/// logical lines it returns are wrapped here, exactly as
/// [`crate::widgets::message_feed`] does it.
///
/// Which of the two texts, like the license tab's: the interface language picks
/// it, and the translation is written to the same markdown subset, so nothing
/// new reaches the renderer.
fn disclaimer_lines(palette: &Palette, loc: &'static Locale, width: usize) -> Vec<Line<'static>> {
    let body_w = width.saturating_sub(HELP_PAD.len()).max(1);
    let rendered =
        crate::shared::markdown::render(credits::disclaimer_text(loc.lang()), body_w, palette);
    let mut lines = vec![Line::raw("")];
    for line in rendered.lines {
        // A wrapped list item is indented under its own marker: the feed can
        // live without that (its lists are short), but here the items run three
        // and four rows long, and without the hang a continuation row is
        // indistinguishable from the next item.
        let plain: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let hang = if plain.trim_start().starts_with("- ") {
            LIST_HANG
        } else {
            0
        };
        for (i, wrapped) in
            crate::shared::wrap::wrap_line(&line, body_w.saturating_sub(hang).max(1))
                .into_iter()
                .enumerate()
        {
            if wrapped.spans.iter().all(|s| s.content.trim().is_empty()) {
                lines.push(Line::raw("")); // no trailing pad on blank rows
                continue;
            }
            let indent = if i == 0 { 0 } else { hang };
            let mut spans = vec![Span::raw(format!("{HELP_PAD}{}", " ".repeat(indent)))];
            // A heading carries its color, bold and underline on the **line**,
            // and a line style covers the indent columns too — which showed as
            // an underline running out to the left of the heading's text. Fold
            // the line style into the content spans instead (each span keeps
            // its own overrides), and leave the padding unstyled.
            let line_style = wrapped.style;
            spans.extend(
                wrapped
                    .spans
                    .into_iter()
                    .map(|s| Span::styled(s.content, line_style.patch(s.style))),
            );
            lines.push(Line::from(spans));
        }
    }
    lines
}

/// Continuation indent for a wrapped list item on the "Disclaimer" tab — the
/// width of the `- ` marker the markdown writer emits.
const LIST_HANG: usize = 2;

/// The "Components" tab: two "table of contents"-style tables — the crates,
/// then the vendored grammars. The name sits on the left margin; the other two
/// columns are anchored against the right margin, version under version and
/// license under license; the run between is a dotted leader in the tab strip's
/// highlight color, so a row reads across the full width without the columns
/// drifting apart visually ([`leader_table`]). Left-aligned like every other
/// tab — an earlier centered-block cut was rejected because nothing else in
/// the app (or on the site) centers text (user's decision, 2026-08-13).
fn component_lines(palette: &Palette, loc: &'static Locale, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            format!("{HELP_PAD}{}", loc.t("ui.components.intro")),
            palette.muted_style(),
        )),
        Line::raw(""),
    ];
    lines.extend(leader_table(credits::COMPONENTS, palette, width));

    // Vendored syntax grammars — third-party data rather than crates, hence a
    // section of their own (see shared/credits.rs and syntaxes/SOURCES.md).
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        format!("{HELP_PAD}{}", loc.t("ui.components.grammars")),
        palette.muted_style(),
    )));
    lines.push(Line::raw(""));
    let grammars: Vec<(&str, &str, &str)> = credits::GRAMMARS.iter().copied().collect();
    lines.extend(leader_table(&grammars, palette, width));
    lines
}

/// One table of the "Components" tab: `(name, mid, right)` rows with the name
/// on the left margin, the `mid` and `right` columns aligned under each other
/// against the right margin (which mirrors [`HELP_PAD`]), and the gap bridged
/// by the dotted leader [`leader_row`] draws.
/// Column math is in characters: every value here is ASCII (crate names,
/// versions, SPDX expressions, repository pins).
fn leader_table(
    rows: &[(&str, &str, &str)],
    palette: &Palette,
    width: usize,
) -> Vec<Line<'static>> {
    let pad = HELP_PAD.chars().count();
    let mid_w = rows
        .iter()
        .map(|(_, m, _)| m.chars().count())
        .max()
        .unwrap_or(0);
    let right_w = rows
        .iter()
        .map(|(.., r)| r.chars().count())
        .max()
        .unwrap_or(0);
    // Where the two anchored columns start.
    let right_col = width.saturating_sub(pad + right_w);
    let mid_col = right_col.saturating_sub(2 + mid_w);
    rows.iter()
        .map(|(name, mid, right)| {
            leader_row(
                Span::styled((*name).to_string(), Style::new().fg(palette.text)),
                vec![
                    Span::styled(format!("{mid:<mid_w$}"), palette.muted_style()),
                    Span::raw("  "),
                    Span::styled((*right).to_string(), palette.muted_style()),
                ],
                mid_col,
                palette,
            )
        })
        .collect()
}

/// One row of a "table of contents"-style table: `left` on the left margin,
/// `tail` starting at column `tail_col`, and the run between bridged by a
/// dotted leader in `keycap_bg` — the backdrop the active tab sits on
/// ([`help_tab_strip`]), a step quieter than the border, so the leaders read as
/// alignment rather than as content. Shared by the "About" and "Components"
/// tabs: one geometry, one leader color, and neither can drift from the other.
///
/// A space sits on each side of the dots. On a dialog clamped below the minimum
/// width the dots run out and the row just clips on the right — the same hard
/// degradation as every other tab's rows.
fn leader_row(
    left: Span<'static>,
    tail: Vec<Span<'static>>,
    tail_col: usize,
    palette: &Palette,
) -> Line<'static> {
    let dots = tail_col.saturating_sub(HELP_PAD.chars().count() + span_width(&left) + 2);
    let mut spans = vec![
        Span::raw(HELP_PAD),
        left,
        Span::styled(
            format!(" {} ", ".".repeat(dots)),
            Style::new().fg(palette.keycap_bg),
        ),
    ];
    spans.extend(tail);
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The width a rendered line occupies in terminal columns.
    fn line_width(line: &Line<'_>) -> usize {
        line.spans.iter().map(span_width).sum()
    }

    /// The widget's own tests exercise the tabs that read no sections; the
    /// sectioned "Shortcuts" tab is tested where the real composed list
    /// lives — `app::runtime::tests` (docs/history/help-hotkeys-context.md §6).
    const NO_SECTIONS: &[&HelpSection] = &[];

    /// The gate behind the wrapping in [`key_lines`]: no row of the commands
    /// tab may run past the dialog, in ANY bundled locale. The hazard is
    /// one-sided — a row that fits in the language you happen to be reading
    /// can overflow in the other, and what disappears is the tail of a
    /// sentence (docs/lessons.md §7). The tabs are plain `Paragraph`s with no
    /// wrapping of their own, so "too wide" means "silently clipped". The
    /// hotkeys tab's twin runs where the composed sections live —
    /// `app::runtime::tests`.
    #[test]
    fn help_rows_fit_the_dialog_in_every_locale() {
        let palette = Palette::default();
        // Both ends of the adaptive range: the floor is where the columns are
        // tightest, the ceiling is where a bound mistake would hide.
        let commands = command_rows();
        for w in [HELP_MIN_WIDTH as usize, HELP_MAX_WIDTH as usize] {
            for &lang in crate::shared::i18n::Lang::ALL {
                let loc = crate::shared::i18n::locale(lang);
                for line in key_lines(&commands, COMMAND_GROUP_OPENERS, &palette, loc, w) {
                    let width = line_width(&line);
                    assert!(
                        width <= w,
                        "commands-tab row is {width} columns wide, the dialog is {w} ({lang:?}): {}",
                        line.spans
                            .iter()
                            .map(|s| s.content.as_ref())
                            .collect::<String>()
                    );
                }
            }
        }
    }

    /// The neatness the tabs are built around: every description — and every
    /// wrapped continuation — starts in the same column, whatever the width of
    /// the label in front of it. Per locale, since the column is measured from
    /// the localized labels. The hotkeys tab's twin runs in
    /// `app::runtime::tests`, over the composed sections.
    #[test]
    fn descriptions_share_one_column_per_tab() {
        let palette = Palette::default();
        let commands = command_rows();
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let w = HELP_MAX_WIDTH as usize;
            let lines = key_lines(&commands, COMMAND_GROUP_OPENERS, &palette, loc, w);
            // The description is always the last span; everything before it
            // (indent, label, gap — or the hanging indent) is its column.
            let starts: Vec<usize> = lines
                .iter()
                .filter(|l| l.spans.iter().any(|s| !s.content.trim().is_empty()))
                .map(|l| {
                    l.spans[..l.spans.len() - 1]
                        .iter()
                        .map(span_width)
                        .sum::<usize>()
                })
                .collect();
            assert!(!starts.is_empty(), "no rows rendered ({lang:?})");
            assert!(
                starts.iter().all(|s| s == &starts[0]),
                "descriptions start at {starts:?} ({lang:?}) — not one column"
            );
            assert!(
                starts[0] > HELP_PAD.chars().count(),
                "the description column collapsed onto the margin ({lang:?})"
            );
        }
    }

    /// The dialog follows the terminal between its bounds: the classic 76×34 on
    /// a small screen, growing to the cap on a large one, never past it.
    #[test]
    fn the_dialog_follows_the_terminal_between_its_bounds() {
        // Floor: an 80×24 terminal keeps the historic size (centered_rect then
        // clamps the height to the screen — that part is not help_size's job).
        assert_eq!(help_size(80, 24), (HELP_MIN_WIDTH, HELP_MIN_HEIGHT));
        // In between: the dialog grows with the terminal, keeping its air.
        assert_eq!(help_size(90, 42), (84, 36));
        // Ceiling: a wide terminal doesn't stretch the lines past readability.
        assert_eq!(help_size(200, 60), (HELP_MAX_WIDTH, HELP_MAX_HEIGHT));
    }

    /// The other direction (docs/lessons.md §2 — a gate that passes is
    /// indistinguishable from one that does nothing): a description far too long
    /// for the dialog produces SEVERAL rows that fit, rather than one that does
    /// not, and the continuation is hung under the description column instead of
    /// starting back at the margin.
    #[test]
    fn an_overlong_description_wraps_under_its_column() {
        let palette = Palette::default();
        let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
        // Unknown bundle keys are echoed back by `t`, which is what makes a
        // synthetic entry possible here. No dots — a dotted literal would be read
        // as a bundle key by the i18n gates.
        let long =
            "a description far too long for the dialog to hold on one single row and then some";
        let lines = key_lines(&[("/x", long)], &[], &palette, loc, HELP_MIN_WIDTH as usize);
        // [0] is the leading blank line.
        let rows = &lines[1..];
        assert!(rows.len() > 1, "expected a wrap, got {} row(s)", rows.len());
        for line in rows {
            assert!(line_width(line) <= HELP_MIN_WIDTH as usize, "{line:?}");
        }
        // The hanging indent: HELP_PAD + " /x " + the separating space = 7.
        let indent = HELP_PAD.chars().count() + "/x".chars().count() + 2 + 1;
        assert_eq!(rows[1].spans[0].content.as_ref(), " ".repeat(indent));
        // Nothing was dropped in the process — every word survives the wrap.
        let joined: String = rows
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<String>();
        for word in long.split_whitespace() {
            assert!(joined.contains(word), "{word} was lost: {joined}");
        }
    }

    /// The "Commands" tab of the help dialog, rendered to text.
    ///
    /// Read in **two passes** — unscrolled, then scrolled past the end (the
    /// renderer clamps) — and concatenated: since the typed routes joined it the
    /// tab is taller than the dialog, and a one-pass read would silently stop
    /// asserting about everything below the fold. What each test wants is "this
    /// row renders somewhere in the tab", not "on the first screen of it".
    /// The `ru` locale — the default the ported screen-level tests ran under.
    fn ru() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    /// Renders the dialog on a `w`×`h` test terminal and returns the buffer as
    /// text (rows joined by newlines) — the shared read behind the tests that
    /// assert on visible content.
    fn dialog_text(help: &mut HelpState, loc: &'static Locale, w: u16, h: u16) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let palette = Palette::default();
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| render_help(f, help, NO_SECTIONS, &palette, loc))
            .unwrap();
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

    fn commands_tab_text() -> String {
        let mut out = String::new();
        for scroll in [0usize, usize::MAX] {
            let mut help = HelpState::open(HelpTab::Commands);
            help.scroll = scroll;
            out.push_str(&dialog_text(&mut help, ru(), 90, 50));
        }
        out
    }

    /// The opener contract behind the group breaks: every opener names exactly
    /// one row of its section (a renamed label would silently orphan its
    /// break — this is the desync the out-of-table encoding trades for keeping
    /// the long-lived rows untouched, so it is pinned here), never the first
    /// row (the header — or the leading spacer — already separates), and each
    /// draws as exactly one blank line: a tab's blank rows are the leading
    /// spacer, the section breaks and the openers, nothing else.
    /// The sections' half of this contract runs in `app::runtime::tests`.
    #[test]
    fn group_openers_open_real_rows() {
        let palette = Palette::default();
        let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
        let is_blank =
            |l: &Line<'_>| -> bool { l.spans.iter().all(|s| s.content.trim().is_empty()) };

        // The commands tab keeps the headerless contract.
        let commands = command_rows();
        assert!(
            !COMMAND_GROUP_OPENERS.is_empty(),
            "commands tab lost its group breaks"
        );
        for opener in COMMAND_GROUP_OPENERS {
            assert_eq!(
                commands.iter().filter(|(k, _)| k == opener).count(),
                1,
                "commands tab: opener {opener:?} must name exactly one row"
            );
        }
        assert!(
            !COMMAND_GROUP_OPENERS.contains(&commands[0].0),
            "commands tab: the first row cannot open a group"
        );
        let lines = key_lines(
            &commands,
            COMMAND_GROUP_OPENERS,
            &palette,
            loc,
            HELP_MIN_WIDTH as usize,
        );
        assert_eq!(
            lines.iter().filter(|l| is_blank(l)).count(),
            COMMAND_GROUP_OPENERS.len() + 1,
            "commands tab: openers + the leading spacer"
        );
        for opener in COMMAND_GROUP_OPENERS {
            let label = loc.t(opener);
            let at = lines
                .iter()
                .position(|l| l.spans.iter().any(|s| s.content.trim() == label))
                .unwrap_or_else(|| panic!("commands tab: opener {opener:?} is not rendered"));
            assert!(
                is_blank(&lines[at - 1]),
                "commands tab: no blank line before opener {opener:?}"
            );
        }
    }

    /// The "About" tab is a leader table too (user's decision 2026-08-19 — the
    /// left-aligned value column left the right ~30 columns of a wide dialog
    /// empty, the same complaint the "Components" tab was fixed for): labels on
    /// the left margin, every value in ONE column, the widest of them touching
    /// the mirrored right margin, dotted leaders in `keycap_bg` bridging the
    /// gap. Pinned at both bounds of the width range, and in `ru` — the locale
    /// with the long labels, where a value column can be squeezed out of
    /// existence without anyone noticing in `en`.
    #[test]
    fn about_rows_anchor_right_with_leaders() {
        let palette = Palette::default();
        for lang in [crate::shared::i18n::Lang::Ru, crate::shared::i18n::Lang::En] {
            let loc = crate::shared::i18n::locale(lang);
            for w in [HELP_MIN_WIDTH as usize, HELP_MAX_WIDTH as usize] {
                let lines = about_lines(&palette, loc, w);
                for line in &lines {
                    assert!(line_width(line) <= w, "row wider than the dialog: {line:?}");
                }
                // A table row is [pad][label][leader][value]; the name/tagline
                // rows above are single-span.
                let rows: Vec<&Line<'static>> =
                    lines.iter().filter(|l| l.spans.len() == 4).collect();
                assert_eq!(rows.len(), 7, "expected every fact row: {rows:?}");
                let col_of = |line: &Line<'static>| -> usize {
                    line.spans[..3].iter().map(span_width).sum()
                };
                let value_col = col_of(rows[0]);
                let mut value_end = 0;
                for row in &rows {
                    assert_eq!(span_width(&row.spans[0]), HELP_PAD.chars().count());
                    assert_eq!(col_of(row), value_col, "the value column drifts: {row:?}");
                    let leader = &row.spans[2];
                    assert_eq!(
                        leader.style.fg,
                        Some(palette.keycap_bg),
                        "leader color: {row:?}"
                    );
                    assert!(
                        leader.content.trim().chars().all(|c| c == '.'),
                        "leader is not dots: {row:?}"
                    );
                    value_end = value_end.max(value_col + span_width(&row.spans[3]));
                }
                assert_eq!(
                    value_end,
                    w - HELP_PAD.chars().count(),
                    "the values are not anchored to the right margin at {w} ({lang:?})"
                );
                // …and the leaders are real runs of dots, not stubs — the whole
                // point of anchoring right.
                assert!(
                    rows.iter()
                        .any(|r| r.spans[2].content.matches('.').count() >= 3),
                    "no visible leaders at {w} ({lang:?})"
                );
            }
        }
        // The facts the tab exists to show, including the two the leaders made
        // room for (`LICENSE_ID` is read from the manifest, so a manifest change
        // shows up here rather than silently).
        let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
        let text: String = about_lines(&palette, loc, HELP_MAX_WIDTH as usize)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        for fact in [
            credits::LICENSE_ID,
            &credits::platform(),
            credits::SITE_URL,
            credits::AUTHOR,
            env!("CARGO_PKG_VERSION"),
        ] {
            assert!(text.contains(fact), "the About tab lost {fact:?}");
        }
    }

    /// The "Components" tab is two leader tables (user's decision 2026-08-13 —
    /// nothing in the app or on the site centers text): names on the left
    /// margin, version/license (and repository/license) columns aligned under
    /// each other against the right margin, dotted leaders bridging the gap in
    /// `keycap_bg` — the same color the active tab's backdrop uses, a step
    /// quieter than the border. Pinned per table and at both bounds of the
    /// width range.
    #[test]
    fn components_columns_anchor_right_with_leaders() {
        let palette = Palette::default();
        let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
        let dim = Some(palette.keycap_bg);
        for w in [HELP_MIN_WIDTH as usize, HELP_MAX_WIDTH as usize] {
            let lines = component_lines(&palette, loc, w);
            for line in &lines {
                assert!(line_width(line) <= w, "row wider than the dialog: {line:?}");
            }
            // The two tables are split by the grammars heading; table rows are
            // the 6-span lines ([pad][name][leader][mid][gap][right]).
            let heading = lines
                .iter()
                .position(|l| {
                    l.spans
                        .iter()
                        .any(|s| s.content.contains(loc.t("ui.components.grammars")))
                })
                .expect("the grammars heading");
            let tables = [&lines[..heading], &lines[heading..]];
            for (which, table) in tables.into_iter().enumerate() {
                let rows: Vec<&Line<'static>> =
                    table.iter().filter(|l| l.spans.len() == 6).collect();
                assert!(
                    rows.len() > 10,
                    "table {which} rendered {} rows",
                    rows.len()
                );
                // Names start on the left margin; mid and right columns start
                // at one shared column each; the widest right value touches
                // the mirrored right margin.
                let col_of = |line: &Line<'static>, span: usize| -> usize {
                    line.spans[..span].iter().map(span_width).sum()
                };
                let mid_col = col_of(rows[0], 3);
                let right_col = col_of(rows[0], 5);
                let mut right_end = 0;
                for row in &rows {
                    assert_eq!(span_width(&row.spans[0]), HELP_PAD.chars().count());
                    assert_eq!(col_of(row, 3), mid_col, "mid column drifts: {row:?}");
                    assert_eq!(col_of(row, 5), right_col, "right column drifts: {row:?}");
                    // The leader is dots in the tab-highlight color, spaces aside.
                    let leader = &row.spans[2];
                    assert_eq!(leader.style.fg, dim, "leader color: {row:?}");
                    assert!(
                        leader.content.trim().chars().all(|c| c == '.'),
                        "leader is not dots: {row:?}"
                    );
                    right_end = right_end.max(col_of(row, 5) + span_width(&row.spans[5]));
                }
                assert_eq!(
                    right_end,
                    w - HELP_PAD.chars().count(),
                    "table {which} is not anchored to the right margin at {w}"
                );
                // At least the short names get a real dotted run, not a stub.
                assert!(
                    rows.iter()
                        .any(|r| r.spans[2].content.matches('.').count() >= 3),
                    "table {which} has no visible leaders"
                );
            }
        }
    }

    #[test]
    fn image_commands_are_listed_in_the_commands_tab() {
        // AGENTS.md §3 — a new command has to be discoverable in `F1`. They sit
        // right after the `/file` block: same verbs, neighbouring wording.
        let at = |label: &str| {
            HELP_COMMANDS
                .iter()
                .position(|(k, _)| *k == label)
                .unwrap_or_else(|| panic!("{label} is missing from HELP_COMMANDS"))
        };
        let files = at("/file list");
        assert_eq!(at("ui.help.k.image_attach"), files + 1);
        assert_eq!(at("ui.help.k.image_remove"), files + 2);
        assert_eq!(at("/image list"), files + 3);

        // …and all three actually render, descriptions included — and those
        // descriptions say "next message", not "this chat": that is the whole
        // difference from `/file`, and the help is where a user learns it.
        let text = commands_tab_text();
        for label in ["/image attach", "/image remove", "/image list"] {
            assert!(text.contains(label), "the command label {label}: {text}");
        }
        assert!(
            text.contains("к следующему сообщению"),
            "the staged-for-the-next-message wording: {text}"
        );
    }

    #[test]
    fn reindex_is_listed_in_the_commands_tab() {
        // The catalog carries it next to the knowledge-base commands (AGENTS.md
        // §3 — a new command must be discoverable in `F1`).
        let pos = HELP_COMMANDS
            .iter()
            .position(|(k, _)| *k == "/reindex")
            .expect("/reindex is missing from HELP_COMMANDS");
        let rebuild = HELP_COMMANDS
            .iter()
            .position(|(k, _)| *k == "/rag rebuild")
            .expect("/rag rebuild is missing from HELP_COMMANDS");
        assert_eq!(
            pos,
            rebuild + 1,
            "/reindex belongs next to the /rag entries"
        );

        // …and it actually renders, description included.
        let text = commands_tab_text();
        assert!(text.contains("/reindex"), "the command label: {text}");
        assert!(
            text.contains("пересобрать все векторы"),
            "the localized description: {text}"
        );
    }

    #[test]
    fn compact_is_listed_in_the_commands_tab() {
        // AGENTS.md §3 — a new command has to be discoverable in `F1`, or it
        // exists only for whoever read the source.
        let pos = HELP_COMMANDS
            .iter()
            .position(|(k, _)| *k == "/compact")
            .expect("/compact is missing from HELP_COMMANDS");
        let reindex = HELP_COMMANDS
            .iter()
            .position(|(k, _)| *k == "/reindex")
            .expect("/reindex is missing from HELP_COMMANDS");
        assert_eq!(
            pos,
            reindex + 1,
            "/compact belongs with the other housekeeping commands"
        );

        let text = commands_tab_text();
        assert!(text.contains("/compact"), "the command label: {text}");
        assert!(
            text.contains("свернуть раннюю часть"),
            "the localized description: {text}"
        );
    }

    #[test]
    fn exit_is_listed_in_the_commands_tab() {
        // AGENTS.md §3 — a new command has to be discoverable in `F1`. This one
        // more than most: a user reaches for it precisely when the documented
        // keys did not work.
        let label = "/exit · /quit";
        let rows = command_rows();
        assert_eq!(
            rows.iter()
                .position(|(k, _)| *k == label)
                .expect("the quit commands are missing from the commands tab"),
            rows.len() - 1,
            "the quit commands close the list, whatever is inserted in front of them"
        );
        // Both spellings the parser accepts are shown — the label is the only
        // place a user learns that `/quit` works too.
        for alias in crate::features::exit_command::ALIASES {
            assert!(
                label.contains(alias),
                "{alias} is missing from the help label {label:?}"
            );
        }

        let text = commands_tab_text();
        assert!(text.contains(label), "the command label: {text}");
        assert!(
            text.contains("перехватывающих Ctrl+Q"),
            "the localized description: {text}"
        );
    }

    /// The dialog's key handling ([`HelpState::handle_key`]): scroll, tab
    /// switching with a scroll reset, closing, and the quit punch-through.
    /// These semantics lived in the chat screen's tests while it owned the
    /// dialog; the runtime's tests cover the overlay around them.
    #[test]
    fn help_keys_navigate_scroll_close_and_quit() {
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let mut h = HelpState::open(DEFAULT_HELP_TAB);
        assert_eq!(h.handle_key(&key(KeyCode::Down)), HelpKeyOutcome::Handled);
        assert_eq!(
            h.handle_key(&key(KeyCode::PageDown)),
            HelpKeyOutcome::Handled
        );
        assert_eq!(h.scroll, 1 + HELP_PAGE_SCROLL);
        h.handle_key(&key(KeyCode::Up));
        assert_eq!(h.scroll, HELP_PAGE_SCROLL);
        h.handle_key(&key(KeyCode::Home));
        assert_eq!(h.scroll, 0);
        // A scroll then a tab switch: the next tab starts at the top.
        h.handle_key(&key(KeyCode::Down));
        h.handle_key(&key(KeyCode::Tab));
        assert_eq!((h.tab, h.scroll), (HelpTab::Commands, 0));
        h.handle_key(&key(KeyCode::Left));
        assert_eq!(h.tab, HelpTab::Hotkeys);
        // Any other key is consumed and changes nothing — a stray press must
        // not close the dialog mid-reading. `?` is one of those keys now: it
        // used to close the dialog as `F1`'s alias, and stopped being a help
        // key at all when it turned out to work on one screen only (spec §11.7).
        for code in [KeyCode::Char('x'), KeyCode::Char('?')] {
            assert_eq!(h.handle_key(&key(code)), HelpKeyOutcome::Handled);
        }
        assert_eq!((h.tab, h.scroll), (HelpTab::Hotkeys, 0));
        // Esc (or a repeat F1) asks to close; the quit keys punch through,
        // layout-independently.
        assert_eq!(h.handle_key(&key(KeyCode::Esc)), HelpKeyOutcome::Close);
        assert_eq!(h.handle_key(&key(KeyCode::F(1))), HelpKeyOutcome::Close);
        assert_eq!(h.handle_key(&key(KeyCode::F(10))), HelpKeyOutcome::Quit);
        assert_eq!(
            h.handle_key(&KeyEvent::new(KeyCode::Char('й'), KeyModifiers::CONTROL)),
            HelpKeyOutcome::Quit
        );
    }

    /// An over-scrolled state clamps at render time, and a short terminal —
    /// where the tab is taller than the view — draws the scrollbar thumb.
    #[test]
    fn help_scroll_clamps_and_draws_scrollbar_on_short_terminal() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let palette = Palette::default();
        let mut help = HelpState::open(HelpTab::Commands);
        help.scroll = 10_000; // "over-scrolled" — the render clamps it
        let mut term = Terminal::new(TestBackend::new(90, 12)).unwrap();
        term.draw(|f| render_help(f, &mut help, NO_SECTIONS, &palette, ru()))
            .unwrap();
        // The bound is measured at the MINIMUM width — the most wraps the tab
        // can ever have, so an upper estimate whatever width the render used.
        let lines = key_lines(
            &command_rows(),
            COMMAND_GROUP_OPENERS,
            &palette,
            ru(),
            HELP_MIN_WIDTH as usize,
        );
        assert!(help.scroll < lines.len(), "scroll clamps to the maximum");
        let buf = term.backend().buffer();
        let mut thumb = false;
        for y in buf.area.top()..buf.area.bottom() {
            for x in buf.area.left()..buf.area.right() {
                thumb |= buf[(x, y)].symbol() == "█";
            }
        }
        assert!(thumb, "on a short terminal help has a scrollbar thumb");
    }

    /// The lockup in the dialog's header is drawn when the terminal height
    /// allows it; the mark is left-aligned to the key list's margin
    /// (docs/branding.md §5).
    #[test]
    fn help_shows_logo_when_terminal_is_tall() {
        use crate::widgets::logo::LOGO_COLS;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::style::Color;
        const ORANGE: Color = Color::Rgb(0xc2, 0x5a, 0x27);

        let palette = Palette::default();
        let mut help = HelpState::open(DEFAULT_HELP_TAB);
        // Tall enough to reach the dialog's height cap (44 content rows +
        // border + air) — the sectioned key list is taller than any dialog, so
        // the lockup condition is about the dialog's own height.
        let mut term = Terminal::new(TestBackend::new(90, 52)).unwrap();
        term.draw(|f| render_help(f, &mut help, NO_SECTIONS, &palette, ru()))
            .unwrap();

        let buf = term.backend().buffer();
        let mut orange: Vec<(u16, u16)> = Vec::new();
        for y in buf.area.top()..buf.area.bottom() {
            for x in buf.area.left()..buf.area.right() {
                let c = &buf[(x, y)];
                if c.style().fg == Some(ORANGE) || c.style().bg == Some(ORANGE) {
                    orange.push((x, y));
                }
            }
        }
        // The glyph's brand-color stem is in every one of its rows, plus
        // "fork" in the word.
        assert!(
            orange.len() > LOCKUP_ROWS as usize,
            "too little brand color — the mark isn't drawn (found {})",
            orange.len()
        );
        // The popup's left border: a rounded corner (default palette — Auto).
        let corner = (buf.area.top()..buf.area.bottom())
            .flat_map(|y| (buf.area.left()..buf.area.right()).map(move |x| (x, y)))
            .filter(|&(x, y)| buf[(x, y)].symbol() == "╭")
            .max_by_key(|&(x, _)| x)
            .expect("the popup's border");
        // The mark is left-aligned: the glyph's stem (columns 4-5 of its ink)
        // sits exactly on the key list's margin — the border + two spaces.
        let left = orange.iter().map(|(x, _)| *x).min().unwrap();
        assert_eq!(
            left,
            corner.0 + 1 + 2 + 4,
            "the glyph's stem is not on the key list's left margin"
        );
        // The wordmark's "fork" — to the right of the glyph, past its edge.
        let right = orange.iter().map(|(x, _)| *x).max().unwrap();
        assert!(
            right > corner.0 + 1 + 2 + LOGO_COLS,
            "the wordmark's \"fork\" is not drawn to the right of the glyph"
        );
    }

    /// On a short terminal the logo isn't drawn at all — the key list doesn't
    /// shift and doesn't need extra scrolling (a hard degradation,
    /// docs/branding.md §5).
    #[test]
    fn help_hides_logo_when_terminal_is_short() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::style::Color;
        const ORANGE: Color = Color::Rgb(0xc2, 0x5a, 0x27);

        let palette = Palette::default();
        let mut help = HelpState::open(DEFAULT_HELP_TAB);
        // A dialog height of 13 (11 rows inside) doesn't fit the lockup with
        // breathing room → it isn't drawn, the tabs don't shift down.
        let mut term = Terminal::new(TestBackend::new(90, 13)).unwrap();
        term.draw(|f| render_help(f, &mut help, NO_SECTIONS, &palette, ru()))
            .unwrap();

        let buf = term.backend().buffer();
        for y in buf.area.top()..buf.area.bottom() {
            for x in buf.area.left()..buf.area.right() {
                let c = &buf[(x, y)];
                assert_ne!(c.style().fg, Some(ORANGE), "the logo must not be drawn");
                assert_ne!(c.style().bg, Some(ORANGE), "the logo must not be drawn");
            }
        }
    }

    /// The tab strip is one line and the dialog has a fixed width, so a tab
    /// label that is a few columns too long in *some* locale silently
    /// truncates the last tab — and the tab that gets cut is the rightmost
    /// one, which nobody looking at the developer's locale would notice.
    /// Adding the "Disclaimer" tab pushed the `ru` strip six columns over the
    /// edge, and the fix was to shorten the hotkeys label in
    /// `locales/ru.json` — so the budget is checked for every bundled locale
    /// rather than left to luck.
    #[test]
    fn the_help_tab_strip_fits_the_dialog_in_every_locale() {
        use crate::shared::i18n::{Lang, locale};

        // The budget is the dialog's MINIMUM width: the strip has to fit the
        // smallest window the adaptive sizing ever grants.
        let palette = Palette::default();
        for lang in Lang::ALL {
            let strip = help_tab_strip(HelpTab::About, &palette, locale(*lang));
            let chars: Vec<char> = strip.spans.iter().flat_map(|s| s.content.chars()).collect();
            let width = crate::shared::wrap::display_width(&chars);
            assert!(
                width <= HELP_MIN_WIDTH as usize,
                "the {} tab strip is {width} columns wide, the dialog is {HELP_MIN_WIDTH}",
                lang.code()
            );
        }
    }

    /// The legal tabs are the one place where a whole *document*, not a UI
    /// string, follows the interface language: `ru` gets the translations
    /// under `docs/legal/`, every other language the authoritative English
    /// (docs/history/legal-ru-translations.md §4.2).
    #[test]
    fn the_legal_tabs_follow_the_interface_language() {
        use crate::shared::i18n::{Lang, locale};

        let text_for = |lang: Lang, tab: HelpTab| -> String {
            let mut help = HelpState::open(tab);
            dialog_text(&mut help, locale(lang), 90, 60)
        };

        // `(language, tab, the marker that must show, the one that must not)`.
        let cases = [
            (Lang::En, HelpTab::License, "MIT License", "перевод"),
            (
                Lang::Ru,
                HelpTab::License,
                "неофициальный перевод",
                "MIT License",
            ),
            (
                Lang::En,
                HelpTab::Disclaimer,
                "mindfork is a client",
                "клиент",
            ),
            (Lang::Ru, HelpTab::Disclaimer, "это клиент", "is a client"),
        ];
        for (lang, tab, wanted, unwanted) in cases {
            let text = text_for(lang, tab);
            assert!(
                text.contains(wanted),
                "the {} {tab:?} tab does not show {wanted:?}",
                lang.code()
            );
            assert!(
                !text.contains(unwanted),
                "the {} {tab:?} tab shows the other language's text ({unwanted:?})",
                lang.code()
            );
        }
    }

    /// A level-1 markdown heading is accent + bold + **underlined**, and the
    /// writer puts that on the `Line` rather than on its spans — so the
    /// "Disclaimer" tab's left indent inherited it and the underline visibly
    /// ran out to the left of the heading's text. The style belongs on the
    /// content spans; the indent stays blank. Reported from a real screenshot.
    #[test]
    fn the_disclaimer_indent_does_not_inherit_the_heading_style() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::style::Modifier;

        let palette = Palette::default();
        let mut help = HelpState::open(HelpTab::Disclaimer);
        let mut term = Terminal::new(TestBackend::new(90, 40)).unwrap();
        term.draw(|f| render_help(f, &mut help, NO_SECTIONS, &palette, ru()))
            .unwrap();

        let buf = term.backend().buffer();
        // Find the heading row and the column its `#` marker starts at.
        let (y, x) = (buf.area.top()..buf.area.bottom())
            .find_map(|y| {
                (buf.area.left()..buf.area.right())
                    .find(|&x| buf[(x, y)].symbol() == "#")
                    .map(|x| (y, x))
            })
            .expect("the disclaimer heading is not on screen");
        assert!(
            buf[(x, y)]
                .style()
                .add_modifier
                .contains(Modifier::UNDERLINED),
            "the heading itself lost its underline"
        );
        for dx in 1..=2 {
            let cell = &buf[(x - dx, y)];
            assert_eq!(cell.symbol(), " ", "the indent is not blank");
            assert!(
                !cell.style().add_modifier.contains(Modifier::UNDERLINED),
                "the indent column {dx} left of the heading is underlined"
            );
        }
    }

    /// Every tab draws its own distinctive content, and the tab strip carries
    /// all six tabs.
    #[test]
    fn help_tabs_render_distinct_content() {
        // The same render, scrolled to the bottom: a tab taller than the
        // popup would otherwise hide rows at *both* scroll extremes.
        let scrolled_to_end = |tab: HelpTab| -> String {
            let mut help = HelpState::open(tab);
            help.scroll = usize::MAX;
            dialog_text(&mut help, ru(), 90, 60)
        };
        let text_for = |tab: HelpTab| -> String {
            let mut help = HelpState::open(tab);
            dialog_text(&mut help, ru(), 90, 60)
        };

        // The tab strip carries all six labels on any tab.
        let about = text_for(HelpTab::About);
        for label in [
            "О программе",
            "Клавиши",
            "Команды",
            "Лицензия",
            "Дисклеймер",
            "Компоненты",
        ] {
            assert!(about.contains(label), "missing the \"{label}\" tab label");
        }
        // "About": the brand name, author, version, links.
        assert!(about.contains("Vladimir Shylov"), "missing the author");
        assert!(
            about.contains(env!("CARGO_PKG_VERSION")),
            "missing the version"
        );
        assert!(
            about.contains("https://mindfork.io"),
            "missing the site link"
        );
        assert!(
            about.contains("https://crates.io/crates/mindfork"),
            "missing the crate link"
        );

        // "Commands": input-box commands, read at both scroll extremes.
        let commands = format!(
            "{}{}",
            text_for(HelpTab::Commands),
            scrolled_to_end(HelpTab::Commands)
        );
        assert!(
            commands.contains("/rag add"),
            "missing the /rag add command"
        );
        assert!(commands.contains("/tts"), "missing the /tts command");
        // Attachments are listed FIRST — they're the commands used while
        // writing a message (docs/file-attachments.md §4.8).
        assert!(
            commands.contains("/file attach"),
            "missing the /file attach command"
        );
        assert!(
            commands.find("/file attach") < commands.find("/rag add"),
            "the /file commands must come before /rag: {commands}"
        );

        // "License": the MIT text. This render runs in `ru`, and the legal
        // tabs follow the interface language — the markers here are the
        // translation's; the two-language rule itself is pinned by
        // `the_legal_tabs_follow_the_interface_language`.
        let license = text_for(HelpTab::License);
        assert!(license.contains("MIT"), "missing the license header");
        assert!(license.contains("ГАРАНТИЙ"), "missing the license body");
        // The disclaimer is a separate tab, not a tail on the license: the
        // MIT text must stay pure (see `credits::LICENSE_TEXT`).
        assert!(
            !license.contains("это клиент"),
            "the disclaimer leaked into the license tab"
        );

        // "Disclaimer": rendered through our own markdown renderer — headings
        // keep their styled `#` prefix, but emphasis markers are consumed.
        let disclaimer = text_for(HelpTab::Disclaimer);
        assert!(
            disclaimer.contains("Дисклеймер"),
            "missing the disclaimer heading"
        );
        assert!(
            disclaimer.contains("это клиент"),
            "missing the disclaimer body"
        );
        assert!(
            !disclaimer.contains("**"),
            "raw markdown emphasis markers on screen — the renderer was bypassed"
        );

        // "Components": name, version, and license (from the list's start —
        // it is long and scrolls).
        let components = text_for(HelpTab::Components);
        assert!(components.contains("ansi-to-tui"), "missing the component");
        assert!(
            components.contains("8.0.1"),
            "missing the component version"
        );
        assert!(
            components.contains("Zlib OR Apache-2.0 OR MIT"),
            "missing the component license"
        );
    }

    /// The vendored syntax grammars are third-party data we redistribute, so
    /// the "Components" tab must name them and their licences — like the
    /// crates above. They sit at the end of a long list, so this scrolls to
    /// the bottom rather than reading the first screen (`syntaxes/SOURCES.md`).
    #[test]
    fn components_tab_lists_the_vendored_grammars() {
        let mut help = HelpState::open(HelpTab::Components);
        help.scroll = usize::MAX / 2; // the render clamps to the last page
        let out = dialog_text(&mut help, ru(), 100, 40);

        assert!(
            out.contains("грамматики"),
            "missing the grammars section header: {out}"
        );
        // The last row of the manifest — whichever it is, must be on the last page.
        let (lang, repo, licence) = *crate::shared::credits::GRAMMARS
            .last()
            .expect("the manifest lists grammars");
        assert!(out.contains(lang), "missing the grammar {lang}: {out}");
        assert!(out.contains(repo), "missing its upstream {repo}");
        assert!(out.contains(licence), "missing its licence {licence}");
    }
}
