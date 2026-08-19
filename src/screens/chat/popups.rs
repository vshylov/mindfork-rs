//! Chat screen — popups: spellcheck suggestions, confirmation, emoji, help. Part
//! of the [`super`] module; split out of the chat.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 2).

use super::render::centered_rect;
use super::*;
use crate::shared::credits;
use crate::shared::wrap;
use crate::widgets::logo::{LOCKUP_COLS, LOCKUP_ROWS, lockup_lines};

/// Left indent of the lockup — matches where the hotkey list starts.
const LOGO_INDENT: u16 = 2;

impl ChatScreen {
    /// Opens the suggestions popup for the misspelled word under the cursor (if any).
    pub(super) fn open_suggestions(&mut self) {
        let Some(spell) = &self.spell else { return };
        let (row, col) = self.input.cursor();
        let lines = self.input.line_strings();
        let Some(line) = lines.get(row) else { return };
        let Some(word) = spell.misspelled_word_at(line, col) else {
            return;
        };
        let mut items: Vec<SuggestItem> = spell
            .suggest(&word.text)
            .into_iter()
            .map(SuggestItem::Replace)
            .collect();
        items.push(SuggestItem::AddToDictionary);
        self.suggest = Some(SuggestPopup {
            word: word.text,
            row,
            start: word.start,
            end: word.end,
            items,
            selected: 0,
        });
    }

    /// Handles a key in the modal confirmation popup: `Enter` confirms (returns
    /// the intent), `Esc` cancels, other keys are ignored. Confirmation is
    /// suppressed while generation is running (the orchestrator gates the intent
    /// anyway).
    /// Keys of the dangerous-tool popup (spec §9.8): `Enter` runs this call,
    /// `A` runs it and stops asking about this tool until the turn ends, `Esc`
    /// declines. Any other key is ignored and the popup stays — the same rule
    /// as the destructive-keys popup, and the reason it is safe to be modal.
    ///
    /// `Esc` **declines rather than cancelling the turn**: the loop carries on
    /// with the refusal, and a second `Esc` — now that the popup is gone —
    /// cancels the turn as it always does.
    pub(super) fn handle_tool_confirm_key(&mut self, key: KeyEvent) -> Option<ChatIntent> {
        // Ctrl+Q/F10 punch through the popup to quit, as everywhere else.
        if key.code == KeyCode::F(10)
            || (key.modifiers.contains(KeyModifiers::CONTROL)
                && keys::hotkey_char(&key) == Some('q'))
        {
            self.tool_confirm = None;
            return Some(ChatIntent::Quit);
        }
        let decision = match key.code {
            KeyCode::Enter => ToolDecision::Allow,
            KeyCode::Esc => ToolDecision::Deny,
            // Layout-independent: `hotkey_char` maps the physical key, so this
            // is the same key on a Cyrillic layout.
            KeyCode::Char(_) if keys::hotkey_char(&key) == Some('a') => ToolDecision::AllowForTurn,
            _ => return None,
        };
        let pending = self.tool_confirm.take()?;
        Some(ChatIntent::ConfirmTool {
            generation_id: pending.generation_id,
            call_id: pending.call_id,
            decision,
        })
    }

    pub(super) fn handle_confirm_key(&mut self, key: KeyEvent) -> Option<ChatIntent> {
        let action = self.confirm.clone()?;
        // Ctrl+Q/F10 punch through the popup to quit (layout-independent). See spec §11.7.
        if key.code == KeyCode::F(10)
            || (key.modifiers.contains(KeyModifiers::CONTROL)
                && keys::hotkey_char(&key) == Some('q'))
        {
            self.confirm = None;
            return Some(ChatIntent::Quit);
        }
        match key.code {
            KeyCode::Enter => {
                self.confirm = None;
                (!self.generating).then(|| action.intent())
            }
            KeyCode::Esc => {
                self.confirm = None;
                None
            }
            _ => None,
        }
    }

    pub(super) fn handle_suggest_key(&mut self, key: KeyEvent) {
        let Some(popup) = &mut self.suggest else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.suggest = None,
            KeyCode::Up => {
                popup.selected = popup.selected.saturating_sub(1);
                return;
            }
            KeyCode::Down => {
                popup.selected = (popup.selected + 1).min(popup.items.len().saturating_sub(1));
                return;
            }
            KeyCode::Enter => self.apply_suggestion(),
            // Other keys don't change (or close) the popup — no redraw needed.
            _ => return,
        }
        // Closing the popup also removes the wide glyph "➕" of the "add to
        // dictionary" item: diff only sends its trailing cell when the glyph
        // carried a style visible on an empty cell (the selected row's
        // `keycap_bg` backdrop), so we play it safe with a full redraw. We don't
        // request one on a **selection shift** — the glyph stays wide, and the
        // full-redraw sentinel must skip its trailing cell (ratatui#2651), so
        // there's no benefit there: the highlight is cleared by the glyph itself
        // being reprinted. See [`Self::request_full_redraw`].
        self.request_full_redraw();
    }

    /// Applies the selected suggestion-popup item and closes it.
    pub(super) fn apply_suggestion(&mut self) {
        let Some(popup) = self.suggest.take() else {
            return;
        };
        match popup.items.get(popup.selected) {
            Some(SuggestItem::Replace(word)) => {
                self.input
                    .replace_range(popup.row, popup.start, popup.end, word);
            }
            Some(SuggestItem::AddToDictionary) => {
                if let Some(spell) = &mut self.spell
                    && let Err(err) = spell.add_to_personal(&popup.word)
                {
                    tracing::warn!(error = %err, "failed to append to the personal dictionary");
                }
            }
            None => {}
        }
        self.mark_input_changed();
    }

    /// Opens the `chat://` reference picker (`Ctrl+L`, spec §11.3).
    ///
    /// The references come from the feed's block cache — what is actually
    /// drawn — and are turned into cards by the chat-list snapshot the screen
    /// already keeps. A conversation the snapshot no longer holds is dropped
    /// rather than listed without a title: the two come from the same source of
    /// truth a frame apart, so this is a race, not a state.
    ///
    /// With nothing to follow the popup does not open at all; it leaves a note
    /// saying what a reference looks like, because an empty list would answer
    /// "no" without saying to what (docs/lessons.md §4).
    pub(super) fn open_chat_links(&mut self) {
        let linked = self.feed_view.chat_links();
        let cards: Vec<ChatSummary> = linked
            .iter()
            .filter_map(|id| self.chats.iter().find(|c| c.id == *id).cloned())
            .collect();
        if cards.is_empty() {
            self.push_note(self.loc.t("ui.chat.links_none"));
            return;
        }
        self.chat_links = Some(ChatLinkPickerState::new(cards, self.active_chat));
    }

    /// Handles a key in the reference picker: `Enter` follows the selected
    /// conversation, `Esc` closes. A reference back to the **open** chat is not
    /// a switch — it says so instead of quietly doing nothing.
    pub(super) fn handle_chat_link_key(&mut self, key: KeyEvent) -> Option<ChatIntent> {
        let picker = self.chat_links.as_mut()?;
        match picker.on_key(key) {
            ChatLinkAction::None => None,
            ChatLinkAction::Cancel => {
                self.chat_links = None;
                None
            }
            ChatLinkAction::Open(id) => {
                self.chat_links = None;
                if self.active_chat == Some(id) {
                    self.push_note(self.loc.t("ui.chat.links_here"));
                    return None;
                }
                Some(ChatIntent::OpenChatLink(id))
            }
        }
    }

    /// Handles a key in the emoji-picker popup: `Enter` inserts the selected
    /// emoji into the input box at the cursor and closes the popup, `Esc` closes
    /// it without inserting. See spec §11.5.
    pub(super) fn handle_emoji_key(&mut self, key: KeyEvent) {
        let Some(picker) = &mut self.emoji else {
            return;
        };
        let action = picker.on_key(key);
        // Read the selection right away (after `on_key` — it may have shifted),
        // so the borrow of `self.emoji` ends here and the whole of `self` is
        // available below.
        let selected = picker.selected();
        // Closing the popup removes wide emoji glyphs from the screen, and the
        // per-cell diff doesn't send the trailing half of UNstyled grid glyphs
        // (ratatui ≥ 0.1.2 only sends it for ones carrying a background/REVERSED)
        // → request a full redraw.
        //
        // We don't request one on a **selection shift**: the glyph stays wide,
        // and the full-redraw sentinel must skip its trailing cell (otherwise the
        // backend would print half of it with no `MoveTo` and shift the row —
        // ratatui#2651). I.e. for a shift it provably does nothing — the backdrop
        // is cleared by the glyph itself being reprinted through a normal diff. A
        // canary pinning both boundaries lives in `widgets::emoji_picker`. See
        // [`Self::request_full_redraw`].
        match action {
            EmojiPickerAction::None => {}
            EmojiPickerAction::Cancel => {
                // Remember the selection on cancel too (the popup recalls where
                // the cursor was).
                self.emoji_last = selected;
                self.emoji = None;
                self.request_full_redraw();
            }
            EmojiPickerAction::Pick(emoji) => {
                self.emoji_last = selected;
                self.emoji = None;
                self.request_full_redraw();
                // insert_str is safe for multi-scalar emoji (`❤️`, `👍🏽`) and
                // places the cursor right after the inserted text.
                self.input.insert_str(&emoji);
                self.mark_input_changed();
            }
        }
    }
}

/// Hotkeys for the "Hotkeys" tab (`F1`/`?`). Pairs `(keycap, desc_key)`: `keycap`
/// is a literal "key" (ASCII, universal) **or** a `ui.*` key where the label
/// itself has words (mouse); `desc_key` is always a `ui.*` description key. Both
/// are resolved through the locale in [`key_lines`]. Input-box commands (`/…`)
/// are split out into [`HELP_COMMANDS`] (a separate tab). Related keys sit in
/// display groups, encoded **outside** the table — [`KEY_GROUP_OPENERS`] names
/// the row that opens each group. See spec §11.7, docs/i18n-ui.md.
pub(super) const HELP_KEYS: &[(&str, &str)] = &[
    ("Enter", "ui.help.send"),
    ("Shift+Enter / Alt+Enter", "ui.help.newline"),
    ("Shift+←/→/↑/↓", "ui.help.select"),
    ("Ctrl+A", "ui.help.select_all"),
    ("Ctrl+C", "ui.help.copy"),
    ("Ctrl+X", "ui.help.cut"),
    ("Ctrl+V", "ui.help.paste"),
    ("Esc", "ui.help.esc"),
    // Only meaningful inside the chat list, but it belongs here: that is where
    // users look for "how do I search my history?".
    ("Ctrl+F", "ui.help.search_content"),
    ("Ctrl+G", "ui.help.search_messages"),
    ("Ctrl+N", "ui.help.new_chat"),
    ("F3", "ui.help.self_model"),
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
    ("Ctrl+P", "ui.help.settings"),
    ("Ctrl+T", "ui.help.thoughts"),
    ("Ctrl+O", "ui.help.tool_calls"),
    ("Ctrl+G", "ui.help.spell"),
    ("Ctrl+B", "ui.help.emoji"),
    ("Ctrl+L", "ui.help.chat_links"),
    ("Ctrl+W", "ui.help.mouse_toggle"),
    ("ui.help.k.mouse", "ui.help.mouse_action"),
    ("PageUp/PageDown", "ui.help.scroll"),
    ("F1 / ?", "ui.help.help"),
    ("Ctrl+Q / F10", "ui.help.quit"),
];

/// Input-box commands for the "Commands" tab (`F1`/`?`). Same format as
/// [`HELP_KEYS`], with the display groups likewise encoded outside the table
/// ([`COMMAND_GROUP_OPENERS`]); a command label (`/…`) is drawn in the command
/// color. Split out of the hotkeys so they don't clutter reading them. See
/// spec §11.7.
pub(super) const HELP_COMMANDS: &[(&str, &str)] = &[
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
pub(super) fn command_rows() -> Vec<(&'static str, &'static str)> {
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

/// The labels that **open a display group** of [`HELP_KEYS`]: [`key_lines`]
/// draws one blank line before each of these rows, so related keys read as
/// small groups (composing · selection/clipboard · navigation · the
/// conversation · editing · panels/toggles · the way out) without headers —
/// and without locale keys, since a break carries no text.
///
/// The breaks live **outside** the table on purpose: the rows are long-lived,
/// and any line added among them lands inside the region the SonarQube
/// copy-paste detector flags on these uniform tuple tables, where every *new*
/// line counts toward the duplication density (docs/lessons.md §2 — this
/// table's regrouping is the lesson's third recurrence).
/// `group_openers_open_real_rows` pins each opener to exactly one row.
pub(super) const KEY_GROUP_OPENERS: &[&str] =
    &["Shift+←/→/↑/↓", "Esc", "F3", "Ctrl+K", "Ctrl+P", "F1 / ?"];

/// [`COMMAND_GROUP_OPENERS`] is [`KEY_GROUP_OPENERS`] for the "Commands" tab:
/// files · images · the knowledge base · housekeeping · speech · the screens ·
/// the conversation · finding things · the feed · the way out. The last four
/// groups before the exit row are the typed routes ([`command_rows`]), grouped
/// the way the registry orders them.
const COMMAND_GROUP_OPENERS: &[&str] = &[
    "ui.help.k.image_attach",
    "ui.help.k.rag_add",
    "/reindex",
    "ui.help.k.tts",
    "ui.help.k.export",
    "/profile list",
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
pub(super) const HELP_MIN_WIDTH: u16 = 76;
pub(super) const HELP_MAX_WIDTH: u16 = 96;
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
pub(super) fn help_size(cols: u16, rows: u16) -> (u16, u16) {
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
pub(super) fn render_help(
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
        HelpTab::Hotkeys => key_lines(HELP_KEYS, KEY_GROUP_OPENERS, palette, loc, inner_w),
        HelpTab::Commands => key_lines(
            &command_rows(),
            COMMAND_GROUP_OPENERS,
            palette,
            loc,
            inner_w,
        ),
        HelpTab::License => license_lines(palette, inner_w),
        HelpTab::Disclaimer => disclaimer_lines(palette, inner_w),
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
pub(super) fn help_tab_strip(
    active: HelpTab,
    palette: &Palette,
    loc: &'static Locale,
) -> Line<'static> {
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

/// The "Hotkeys"/"Commands" tabs: a list of `(label, description)` pairs — a
/// "key" + a description (a command label `/…` uses the command color); a
/// blank line is drawn before every row named in `openers`, separating the
/// display groups. The locale resolves both label keys and descriptions.
/// Shared by both tabs ([`HELP_KEYS`]/[`HELP_COMMANDS`], with their
/// [`KEY_GROUP_OPENERS`]/[`COMMAND_GROUP_OPENERS`]).
///
/// Every description starts in the **same column** — one past the tab's widest
/// label — and a description too long for `width` **wraps**, hung under that
/// column ([`push_key_entry`]), rather than being clipped at the dialog's edge:
/// the tab is a plain `Paragraph` with no wrapping of its own, so an over-long
/// row used to lose its tail mid-word. Both are per-locale work — a row can fit
/// in `en` and overflow in `ru`, so whoever writes the label never sees it
/// (docs/lessons.md §7), and the label lengths differ per locale too, which is
/// why the column is measured here (in display columns — a label can carry `↔`
/// or a box-drawing glyph, where `.len()` would be bytes) rather than fixed as
/// a constant. `help_rows_fit_the_dialog_in_every_locale` is the gate. See
/// spec §11.7.
fn key_lines(
    entries: &[(&str, &str)],
    openers: &[&str],
    palette: &Palette,
    loc: &'static Locale,
    width: usize,
) -> Vec<Line<'static>> {
    // Resolve every label up front: the description column is shared by the
    // whole tab, so it has to be measured before any row can be built. Both
    // label styles pad with a space on each side, so one measurement covers
    // keycaps and command labels alike.
    let resolved: Vec<(bool, Span<'static>, String)> = entries
        .iter()
        .map(|(k, d)| {
            let key = loc.t(k).to_string();
            let key_span = if key.starts_with('/') {
                Span::styled(format!(" {key} "), Style::new().fg(palette.warning))
            } else {
                palette.keycap(key)
            };
            (openers.contains(k), key_span, loc.t(d).to_string())
        })
        .collect();
    let label_col = resolved
        .iter()
        .map(|(_, span, _)| span_width(span))
        .max()
        .unwrap_or(0);
    // Where every description starts: the indent + the label column + the
    // separating space.
    let indent = HELP_PAD.chars().count() + label_col + 1;
    let mut lines = vec![Line::raw("")];
    for (opens_group, key_span, desc) in &resolved {
        if *opens_group {
            lines.push(Line::raw("")); // a breath between the groups
        }
        push_key_entry(&mut lines, key_span, desc, palette, width, indent);
    }
    lines
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

/// The "License" tab: the app's license text (MIT). Paragraphs (separated by a
/// blank line in the file) are reassembled and word-wrapped to the content width
/// `width` — the source's hard wrap at ~76 columns would otherwise clip on the
/// right, while wrapping line-by-line would leave orphaned words. Wrapping
/// produces logical lines, so the scroll/scrollbar model (by line count) isn't
/// broken. Assembly via `lines()` is CRLF-safe.
fn license_lines(palette: &Palette, width: usize) -> Vec<Line<'static>> {
    let body_w = width.saturating_sub(HELP_PAD.len()).max(1);
    // Assemble paragraphs: non-empty lines are joined with a space, an empty one
    // is a boundary.
    let mut paras: Vec<String> = Vec::new();
    let mut cur = String::new();
    for raw in credits::LICENSE_TEXT.lines() {
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
fn disclaimer_lines(palette: &Palette, width: usize) -> Vec<Line<'static>> {
    let body_w = width.saturating_sub(HELP_PAD.len()).max(1);
    let rendered = crate::shared::markdown::render(credits::DISCLAIMER_TEXT, body_w, palette);
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

/// Draws the spellcheck-suggestion popup centered on screen.
pub(super) fn render_suggest(
    frame: &mut Frame,
    popup: &SuggestPopup,
    palette: &Palette,
    loc: &'static Locale,
) {
    let rows = (popup.items.len() as u16 + 2).min(frame.area().height);
    let area = centered_rect(40, rows, frame.area());
    frame.render_widget(Clear, area);

    let block = palette
        .panel(popup.word.clone(), true)
        .title_bottom(Line::from(Span::styled(
            loc.t("ui.suggest.footer"),
            palette.muted_style(),
        )));
    let selected = popup.selected.min(popup.items.len().saturating_sub(1));
    let items: Vec<ListItem> = popup
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            // Rail for the selected row (2 columns), like in the chat list and
            // settings; other rows get an indent of the same width so the text
            // doesn't "jump".
            let rail = if i == selected {
                Span::styled("▌ ", Style::new().fg(palette.success))
            } else {
                Span::raw("  ")
            };
            let body = match item {
                SuggestItem::Replace(word) => {
                    Span::styled(word.clone(), Style::new().fg(palette.text))
                }
                SuggestItem::AddToDictionary => Span::styled(
                    format!("{} {}", palette.glyphs().add, loc.t("ui.suggest.add")),
                    palette.success_style(),
                )
                .italic(),
            };
            ListItem::new(Line::from(vec![rail, body]))
        })
        .collect();
    // The selection uses a soft backdrop (as in the chat list, settings, and the
    // "self-model" screen), not inverting the whole line: reverse video swaps
    // fg↔bg per span independently, which smears the `▌` rail (a left
    // half-block) and gives spans different backgrounds.
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::new().bg(palette.keycap_bg));
    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

/// Draws the modal confirmation popup for an irreversible operation
/// (`Ctrl+R`/`Ctrl+E`) centered on screen. See spec §11.7.
pub(super) fn render_confirm(
    frame: &mut Frame,
    action: &ConfirmAction,
    palette: &Palette,
    loc: &'static Locale,
) {
    let width = 56u16.min(frame.area().width);
    let area = centered_rect(width, 5, frame.area());
    frame.render_widget(Clear, area);
    let block = palette.panel(loc.t("ui.confirm.title"), true).title_bottom(
        Line::from(Span::styled(
            loc.t("ui.confirm.footer"),
            palette.muted_style(),
        ))
        .centered(),
    );
    let body = Paragraph::new(Line::from(Span::styled(
        action.prompt(loc),
        Style::new().fg(palette.text),
    )))
    .block(block)
    .wrap(Wrap { trim: true });
    frame.render_widget(body, area);
}

/// Draws the dangerous-tool confirmation popup (spec §9.8).
///
/// The call is formatted through `features::tools::present` — the same
/// formatting the feed will show for it afterwards (fork F7), so `python_exec`
/// appears as readable code rather than as a JSON blob. The body is capped:
/// this is a decision prompt, not a viewer, and a popup taller than the screen
/// would hide its own footer.
pub(super) fn render_tool_confirm(
    frame: &mut Frame,
    pending: &ToolConfirm,
    palette: &Palette,
    loc: &'static Locale,
) {
    use crate::features::tools::present::{ArgDetail, ToolBlock, present};

    // Compact: this is a decision prompt, not a viewer — long arguments are cut
    // with "…" here (the card in the feed is where the call is read in full).
    let shown = present(&pending.name, &pending.arguments, "", ArgDetail::Compact);
    let header = match &shown.header_suffix {
        Some(suffix) => format!("{}({suffix})", pending.name),
        None => pending.name.clone(),
    };
    let mut body: Vec<Line> = vec![
        Line::from(Span::styled(
            loc.t("ui.confirm.tool.question"),
            Style::new().fg(palette.text),
        )),
        Line::from(Span::styled(header, Style::new().fg(palette.accent))),
    ];
    for block in &shown.args {
        let text = match block {
            ToolBlock::Code { text, .. } | ToolBlock::Plain(text) | ToolBlock::Markdown(text) => {
                text.as_str()
            }
            ToolBlock::Console(_) => continue,
        };
        for line in text.lines().take(ARG_PREVIEW_LINES) {
            body.push(Line::from(Span::styled(
                line.to_string(),
                palette.muted_style(),
            )));
        }
        if text.lines().count() > ARG_PREVIEW_LINES {
            body.push(Line::from(Span::styled("…", palette.muted_style())));
        }
    }

    let width = 72u16.min(frame.area().width);
    let height = (body.len() as u16 + 2).min(frame.area().height);
    let area = centered_rect(width, height, frame.area());
    frame.render_widget(Clear, area);
    let block = palette
        .panel(loc.t("ui.confirm.tool.title"), true)
        .title_bottom(
            Line::from(Span::styled(
                loc.t("ui.confirm.tool.footer"),
                palette.muted_style(),
            ))
            .centered(),
        );
    frame.render_widget(
        Paragraph::new(body).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

/// How many lines of a call's arguments the confirmation popup shows before
/// cutting with "…".
const ARG_PREVIEW_LINES: usize = 12;

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
                for (name, entries, openers) in [
                    ("HELP_KEYS", HELP_KEYS, KEY_GROUP_OPENERS),
                    ("commands tab", commands.as_slice(), COMMAND_GROUP_OPENERS),
                ] {
                    for line in key_lines(entries, openers, &palette, loc, w) {
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
            for (name, entries, openers) in [
                ("HELP_KEYS", HELP_KEYS, KEY_GROUP_OPENERS),
                ("commands tab", commands.as_slice(), COMMAND_GROUP_OPENERS),
            ] {
                let lines = key_lines(entries, openers, &palette, loc, HELP_MAX_WIDTH as usize);
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

        let mut out = String::new();
        for scroll in [0usize, usize::MAX] {
            let mut s = ChatScreen::new();
            s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
            let help = s.help.as_mut().unwrap();
            help.tab = HelpTab::Commands;
            help.scroll = scroll;
            let mut term = Terminal::new(TestBackend::new(90, 50)).unwrap();
            term.draw(|f| s.render(f)).unwrap();
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
    /// one row of its table (a renamed label would silently orphan its break —
    /// this is the desync the out-of-table encoding trades for keeping the
    /// long-lived rows untouched, so it is pinned here), never the first row
    /// (that would double the leading spacer), and each draws as exactly one
    /// blank line — the tab's blank rows are the openers plus the leading
    /// spacer, nothing else.
    #[test]
    fn group_openers_open_real_rows() {
        let palette = Palette::default();
        let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
        let commands = command_rows();
        for (name, entries, openers) in [
            ("HELP_KEYS", HELP_KEYS, KEY_GROUP_OPENERS),
            ("commands tab", commands.as_slice(), COMMAND_GROUP_OPENERS),
        ] {
            assert!(!openers.is_empty(), "{name} lost its group breaks");
            for opener in openers {
                assert_eq!(
                    entries.iter().filter(|(k, _)| k == opener).count(),
                    1,
                    "{name}: opener {opener:?} must name exactly one row"
                );
            }
            assert!(
                !openers.contains(&entries[0].0),
                "{name}: the first row cannot open a group"
            );
            let lines = key_lines(entries, openers, &palette, loc, HELP_MIN_WIDTH as usize);
            let blanks = lines
                .iter()
                .filter(|l| l.spans.iter().all(|s| s.content.trim().is_empty()))
                .count();
            assert_eq!(
                blanks,
                openers.len() + 1,
                "{name}: openers + the leading spacer"
            );
            // …and each break sits immediately BEFORE its opener's row, not
            // after it or somewhere else (the count alone can't tell).
            for opener in openers {
                let label = loc.t(opener);
                let at = lines
                    .iter()
                    .position(|l| l.spans.iter().any(|s| s.content.trim() == label))
                    .unwrap_or_else(|| panic!("{name}: opener {opener:?} is not rendered"));
                assert!(
                    lines[at - 1]
                        .spans
                        .iter()
                        .all(|s| s.content.trim().is_empty()),
                    "{name}: no blank line before opener {opener:?}"
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
