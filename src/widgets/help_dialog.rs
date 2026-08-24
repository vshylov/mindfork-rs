//! The help/"About" dialog (`F1`/`?`): the tabs, the per-screen hotkey
//! sections and their rendering. A widget of its own so the runtime can draw
//! it over whatever screen is active (docs/help-hotkeys-context.md, stage 2);
//! it grew up in `screens/chat/popups.rs` and moved here unchanged.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph};

use crate::shared::credits;
use crate::shared::i18n::Locale;
use crate::shared::theme::Palette;
use crate::shared::ui::{centered_rect, render_scrollbar};
use crate::shared::wrap;
use crate::widgets::logo::{LOCKUP_COLS, LOCKUP_ROWS, lockup_lines};

/// Left indent of the lockup — matches where the hotkey list starts.
const LOGO_INDENT: u16 = 2;

/// A tab of the help/"About" dialog (`F1`/`?`), KDE/Qt-style. See spec §11.7.
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
/// remembered): `F1`/`?` — the familiar help key, and "Hotkeys" is the most
/// sought-after content; "About" is the neighboring tab.
pub const DEFAULT_HELP_TAB: HelpTab = HelpTab::Hotkeys;

/// The screen the help dialog was opened from — the "Shortcuts" tab marks that
/// screen's section "you are here" (`popups::HELP_SECTIONS`). Today only the
/// chat opens the dialog; stage 2 of docs/help-hotkeys-context.md threads the
/// real invoking screen through and anchors the tab's scroll to its section.
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
/// (reset on tab switch), and the screen it was opened from. Opens on
/// `F1`/`?`. See spec §11.7.
pub struct HelpState {
    pub tab: HelpTab,
    /// The first visible row of the active tab's content (clamped in `render_help`).
    pub scroll: usize,
    /// The invoking screen, for the "you are here" marker.
    pub context: HelpContext,
}

impl HelpState {
    /// Open on the given tab (on reopening — on the last-selected one,
    /// the opener's `help_last_tab` memory). The chat screen is the only opener
    /// today, so the context is fixed (docs/help-hotkeys-context.md §6 threads
    /// the real one in stage 2).
    pub fn open(tab: HelpTab) -> Self {
        Self {
            tab,
            scroll: 0,
            context: HelpContext::Chat,
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
}

/// One section of the "Shortcuts" tab (`F1`/`?`): the keys of one screen under
/// a localized header. A key is listed once per screen where it does something,
/// so a chord with two meanings is two short rows in two sections — the header
/// says the context the descriptions used to spell out ("in the chat list: …")
/// per locale, or dropped entirely (the settings screen's keys were absent).
/// Rows are `(keycap, desc_key)` pairs: `keycap` is a literal "key" (ASCII,
/// universal) **or** a `ui.*` key where the label itself has words (mouse,
/// typing); `desc_key` is always a `ui.*` description key. Both are resolved
/// through the locale in [`key_lines`]. Input-box commands (`/…`) are split out
/// into [`HELP_COMMANDS`] (a separate tab). See spec §11.7,
/// docs/help-hotkeys-context.md, docs/i18n-ui.md.
pub struct HelpSection {
    /// Locale key of the header (`ui.help.sec.*`). The route to the screen is
    /// part of the localized title ("Settings (Ctrl+P)") — the header doubles
    /// as "how do I get there".
    pub title: &'static str,
    /// The screen this section documents — the one whose header carries the
    /// "you are here" marker when the dialog was opened from it. `None` — the
    /// "Everywhere" rows, never marked.
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

/// The "Shortcuts" tab as per-screen sections (spec §11.7,
/// docs/help-hotkeys-context.md): "Everywhere" first, then the screens by how
/// often the user is on them. Rows follow each screen's actual key handler —
/// completeness against the `match` arms is the point, and AGENTS.md §3 sends
/// every new key here.
pub const HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        title: "ui.help.sec.everywhere",
        context: None,
        rows: &[("Ctrl+Q / F10", "ui.help.quit")],
        openers: &[],
    },
    // The chat's groups: composing · selection/clipboard · the conversation ·
    // editing · panels and toggles (with the way out at the end).
    HelpSection {
        title: "ui.help.sec.chat",
        context: Some(HelpContext::Chat),
        rows: &[
            ("Enter", "ui.help.send"),
            ("Shift+Enter / Alt+Enter", "ui.help.newline"),
            ("Shift+←/→/↑/↓", "ui.help.select"),
            ("Ctrl+A", "ui.help.select_all"),
            ("Ctrl+C", "ui.help.copy"),
            ("Ctrl+X", "ui.help.cut"),
            ("Ctrl+V", "ui.help.paste"),
            ("Esc", "ui.help.esc"),
            ("Ctrl+N", "ui.help.new_chat"),
            ("F2", "ui.help.rename_chat"),
            ("F5", "ui.help.copy_chat"),
            ("Ctrl+R", "ui.help.regenerate"),
            ("Ctrl+E", "ui.help.delete_exchange"),
            ("Ctrl+U", "ui.help.impersonate"),
            ("Ctrl+K", "ui.help.clear_input"),
            ("Ctrl+Z / Ctrl+Y", "ui.help.undo_redo"),
            ("Ctrl+←/→", "ui.help.word_move"),
            ("Ctrl+Backspace/Delete", "ui.help.word_delete"),
            ("Home", "ui.help.line_home"),
            ("End", "ui.help.line_end"),
            ("Ctrl+Home/End", "ui.help.doc_move"),
            ("Ctrl+G", "ui.help.spell"),
            ("Ctrl+P", "ui.help.settings"),
            ("F3", "ui.help.self_model"),
            ("F4", "ui.help.changes"),
            ("Ctrl+F", "ui.help.find_in_chat"),
            ("Ctrl+T", "ui.help.thoughts"),
            ("Ctrl+O", "ui.help.tool_calls"),
            ("Ctrl+B", "ui.help.emoji"),
            ("Ctrl+L", "ui.help.chat_links"),
            ("Ctrl+W", "ui.help.mouse_toggle"),
            ("ui.help.k.mouse", "ui.help.mouse_action"),
            ("PageUp/PageDown", "ui.help.scroll"),
            ("F1 / ?", "ui.help.help"),
        ],
        openers: &["Shift+←/→/↑/↓", "Esc", "Ctrl+K", "Ctrl+P"],
    },
    // The list's groups: searching · acting on the selection · managing chats.
    HelpSection {
        title: "ui.help.sec.chat_list",
        context: Some(HelpContext::ChatList),
        rows: &[
            ("ui.help.k.type", "ui.help.list_type"),
            ("Ctrl+F", "ui.help.list_scope"),
            ("Ctrl+G", "ui.help.list_messages"),
            ("Enter", "ui.help.list_open"),
            ("↑/↓", "ui.help.list_select"),
            ("Tab", "ui.help.list_sort"),
            ("Esc", "ui.help.list_close"),
            ("F2", "ui.help.list_rename"),
            ("F5", "ui.help.list_copy"),
            ("Ctrl+R", "ui.help.list_autotitle"),
            ("Ctrl+N", "ui.help.new_chat"),
            ("Ctrl+D", "ui.help.list_clone"),
            ("Del", "ui.help.list_delete"),
            ("Ctrl+O", "ui.help.subagents_fold"),
        ],
        openers: &["Enter", "F2"],
    },
    // The settings' groups: moving through sections and fields · the tools
    // over them (search, undo, the two list sections' CRUD).
    HelpSection {
        title: "ui.help.sec.settings",
        context: Some(HelpContext::Settings),
        rows: &[
            ("Tab / Shift+Tab", "ui.help.set_sections"),
            ("↑/↓", "ui.help.set_rows"),
            ("Enter", "ui.help.set_enter"),
            ("←/→", "ui.help.set_cycle"),
            ("Space", "ui.help.set_toggle"),
            ("Del", "ui.help.set_reset"),
            ("Esc", "ui.help.set_back"),
            ("/", "ui.help.set_search"),
            ("Ctrl+Z / Ctrl+Y", "ui.help.set_undo"),
            ("Ctrl+N", "ui.help.set_new"),
            ("Ctrl+D", "ui.help.set_delete"),
        ],
        openers: &["/"],
    },
    HelpSection {
        title: "ui.help.sec.self_model",
        context: Some(HelpContext::SelfModel),
        rows: &[
            ("↑/↓", "ui.help.sm_select"),
            ("Enter", "ui.help.sm_edit"),
            ("Space", "ui.help.sm_goal"),
            ("Del", "ui.help.sm_delete"),
            ("Ctrl+K Ctrl+K", "ui.help.sm_clear"),
            ("Esc", "ui.help.sm_close"),
        ],
        openers: &[],
    },
    HelpSection {
        title: "ui.help.sec.changes",
        context: Some(HelpContext::Changes),
        rows: &[
            ("Tab", "ui.help.ch_pane"),
            ("↑/↓", "ui.help.ch_select"),
            ("PageUp/PageDown", "ui.help.ch_scroll"),
            ("R", "ui.help.ch_revert"),
            ("Esc", "ui.help.ch_close"),
        ],
        openers: &[],
    },
    HelpSection {
        title: "ui.help.sec.search",
        context: Some(HelpContext::Search),
        rows: &[
            ("↑/↓", "ui.help.sr_select"),
            ("Enter", "ui.help.sr_open"),
            ("Esc", "ui.help.sr_back"),
        ],
        openers: &[],
    },
];

/// Input-box commands for the "Commands" tab (`F1`/`?`). Same format as
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
        HelpTab::Hotkeys => hotkeys_lines(help.context, palette, loc, inner_w),
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

/// The "Shortcuts" tab: [`HELP_SECTIONS`] under their headers, the section for
/// `context` marked "you are here" (docs/help-hotkeys-context.md, fork F1).
pub fn hotkeys_lines(
    context: HelpContext,
    palette: &Palette,
    loc: &'static Locale,
    width: usize,
) -> Vec<Line<'static>> {
    let sections: Vec<TabSection<'_>> = HELP_SECTIONS
        .iter()
        .map(|s| TabSection {
            header: Some((s.title, s.context == Some(context))),
            rows: s.rows,
            openers: s.openers,
        })
        .collect();
    table_lines(&sections, false, palette, loc, width)
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
    table_lines(std::slice::from_ref(&section), true, palette, loc, width)
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
) -> Vec<Line<'static>> {
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
    for (si, (header, rows)) in resolved.iter().enumerate() {
        if si > 0 {
            lines.push(Line::raw("")); // a breath between the sections
        }
        if let Some((title, marked)) = header {
            lines.push(section_header(title, *marked, palette, loc, width));
        }
        for (opens_group, key_span, desc) in rows {
            if *opens_group {
                lines.push(Line::raw("")); // a breath between the groups
            }
            push_key_entry(&mut lines, key_span, desc, palette, width, indent);
        }
    }
    lines
}

/// A section's header row: the localized title, the "you are here" marker when
/// the dialog was opened from that screen (docs/help-hotkeys-context.md, fork
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

    /// The gate behind the wrapping in [`key_lines`]: no row of either tab may
    /// run past the dialog, in ANY bundled locale. The hazard is one-sided —
    /// a row that fits in the language you happen to be reading can overflow in
    /// the other, and what disappears is the tail of a sentence (docs/lessons.md
    /// §7). The tabs are plain `Paragraph`s with no wrapping of their own, so
    /// "too wide" means "silently clipped".
    #[test]
    fn help_rows_fit_the_dialog_in_every_locale() {
        let palette = Palette::default();
        // Both ends of the adaptive range: the floor is where the columns are
        // tightest, the ceiling is where a bound mistake would hide.
        let commands = command_rows();
        for w in [HELP_MIN_WIDTH as usize, HELP_MAX_WIDTH as usize] {
            for &lang in crate::shared::i18n::Lang::ALL {
                let loc = crate::shared::i18n::locale(lang);
                for (name, lines) in [
                    (
                        "hotkeys tab",
                        hotkeys_lines(HelpContext::Chat, &palette, loc, w),
                    ),
                    (
                        "commands tab",
                        key_lines(&commands, COMMAND_GROUP_OPENERS, &palette, loc, w),
                    ),
                ] {
                    for line in lines {
                        let width = line_width(&line);
                        assert!(
                            width <= w,
                            "{name} row is {width} columns wide, the dialog is {w} ({lang:?}): {}",
                            line.spans
                                .iter()
                                .map(|s| s.content.as_ref())
                                .collect::<String>()
                        );
                    }
                }
            }
        }
    }

    /// The neatness the tabs are built around: every description — and every
    /// wrapped continuation — starts in the same column, whatever the width of
    /// the label in front of it. Per tab and per locale, since the column is
    /// measured from the localized labels.
    #[test]
    fn descriptions_share_one_column_per_tab() {
        let palette = Palette::default();
        let commands = command_rows();
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let w = HELP_MAX_WIDTH as usize;
            for (name, lines) in [
                (
                    "hotkeys tab",
                    hotkeys_lines(HelpContext::Chat, &palette, loc, w),
                ),
                (
                    "commands tab",
                    key_lines(&commands, COMMAND_GROUP_OPENERS, &palette, loc, w),
                ),
            ] {
                // The description is always the last span; everything before it
                // (indent, label, gap — or the hanging indent) is its column.
                // Section headers end in a rule, not a description — skip them.
                let starts: Vec<usize> = lines
                    .iter()
                    .filter(|l| l.spans.iter().any(|s| !s.content.trim().is_empty()))
                    .filter(|l| !l.spans.iter().any(|s| s.content.contains('─')))
                    .map(|l| {
                        l.spans[..l.spans.len() - 1]
                            .iter()
                            .map(span_width)
                            .sum::<usize>()
                    })
                    .collect();
                assert!(!starts.is_empty(), "{name} rendered no rows ({lang:?})");
                assert!(
                    starts.iter().all(|s| s == &starts[0]),
                    "{name} descriptions start at {starts:?} ({lang:?}) — not one column"
                );
                assert!(
                    starts[0] > HELP_PAD.chars().count(),
                    "{name} description column collapsed onto the margin ({lang:?})"
                );
            }
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
    fn commands_tab_text() -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let palette = Palette::default();
        let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
        let mut out = String::new();
        for scroll in [0usize, usize::MAX] {
            let mut help = HelpState::open(HelpTab::Commands);
            help.scroll = scroll;
            let mut term = Terminal::new(TestBackend::new(90, 50)).unwrap();
            term.draw(|f| render_help(f, &mut help, &palette, loc))
                .unwrap();
            let buf = term.backend().buffer();
            for y in buf.area.top()..buf.area.bottom() {
                for x in buf.area.left()..buf.area.right() {
                    out.push_str(buf[(x, y)].symbol());
                }
                out.push('\n');
            }
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
    #[test]
    fn group_openers_open_real_rows() {
        let palette = Palette::default();
        let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
        let is_blank =
            |l: &Line<'_>| -> bool { l.spans.iter().all(|s| s.content.trim().is_empty()) };

        // The per-section contract of the "Shortcuts" tab.
        for section in HELP_SECTIONS {
            assert!(
                !section.rows.is_empty(),
                "{}: an empty section",
                section.title
            );
            for opener in section.openers {
                assert_eq!(
                    section.rows.iter().filter(|(k, _)| k == opener).count(),
                    1,
                    "{}: opener {opener:?} must name exactly one row",
                    section.title
                );
            }
            assert!(
                !section.openers.contains(&section.rows[0].0),
                "{}: the first row cannot open a group",
                section.title
            );
        }

        // Rendered blanks: the leading spacer, one break before every section
        // after the first, one per opener — headers are rule rows, not blanks.
        let lines = hotkeys_lines(HelpContext::Chat, &palette, loc, HELP_MIN_WIDTH as usize);
        let openers_total: usize = HELP_SECTIONS.iter().map(|s| s.openers.len()).sum();
        assert_eq!(
            lines.iter().filter(|l| is_blank(l)).count(),
            1 + (HELP_SECTIONS.len() - 1) + openers_total,
            "hotkeys tab: the leading spacer + section breaks + openers"
        );

        // …and each break sits immediately BEFORE its opener's row. Key labels
        // repeat across sections ("Esc", "Enter"), so each opener is looked up
        // inside its own section's slice, delimited by the (unique) headers.
        let header_at: Vec<usize> = HELP_SECTIONS
            .iter()
            .map(|s| {
                lines
                    .iter()
                    .position(|l| l.spans.iter().any(|sp| sp.content.trim() == loc.t(s.title)))
                    .unwrap_or_else(|| panic!("{}: header is not rendered", s.title))
            })
            .collect();
        for (i, section) in HELP_SECTIONS.iter().enumerate() {
            let end = header_at.get(i + 1).copied().unwrap_or(lines.len());
            for opener in section.openers {
                let label = loc.t(opener);
                let at = lines[header_at[i]..end]
                    .iter()
                    .position(|l| l.spans.iter().any(|s| s.content.trim() == label))
                    .map(|p| header_at[i] + p)
                    .unwrap_or_else(|| {
                        panic!("{}: opener {opener:?} is not rendered", section.title)
                    });
                assert!(
                    is_blank(&lines[at - 1]),
                    "{}: no blank line before opener {opener:?}",
                    section.title
                );
            }
        }

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

    /// Fork F1 of docs/help-hotkeys-context.md: the section for the screen the
    /// dialog was opened from — and only it — carries the "you are here"
    /// marker, next to its own title. Pinned for two contexts so the marker
    /// provably follows the context rather than sticking to the chat.
    #[test]
    fn the_invoking_screens_section_is_marked() {
        let palette = Palette::default();
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let here = loc.t("ui.help.here");
            for (context, title) in [
                (HelpContext::Chat, "ui.help.sec.chat"),
                (HelpContext::Settings, "ui.help.sec.settings"),
            ] {
                let lines = hotkeys_lines(context, &palette, loc, HELP_MIN_WIDTH as usize);
                let marked: Vec<&Line<'_>> = lines
                    .iter()
                    .filter(|l| {
                        l.spans
                            .iter()
                            .any(|s| s.content.contains(here) && s.content != here)
                    })
                    .collect();
                assert_eq!(marked.len(), 1, "one marker per dialog ({lang:?})");
                assert!(
                    marked[0]
                        .spans
                        .iter()
                        .any(|s| s.content.trim() == loc.t(title)),
                    "the marker must sit on the {title} header ({lang:?}): {:?}",
                    marked[0]
                );
            }
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
}
