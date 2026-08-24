//! Chat screen — popups: spellcheck suggestions, confirmation, emoji, help. Part
//! of the [`super`] module; split out of the chat.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 2).

use super::render::centered_rect;
use super::*;

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
            scroll: ListScroll::default(),
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
                self.confirmed_intent(action)
            }
            KeyCode::Esc => {
                self.confirm = None;
                None
            }
            _ => None,
        }
    }

    /// Turns a confirmed [`ConfirmAction`] into its intent. The four fixed
    /// actions keep the long-standing `generating` gate (they mutate the turn's
    /// conversation); an impersonation-profile deletion is a config edit built
    /// from the **current** settings snapshot — the snapshot may have refreshed
    /// while the popup was open — and is independent of the turn.
    fn confirmed_intent(&mut self, action: ConfirmAction) -> Option<ChatIntent> {
        match action {
            ConfirmAction::DeleteImpersonation { id, name } => self.delete_impersonation(id, &name),
            other => {
                if self.generating {
                    return None;
                }
                other.intent()
            }
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
            .filter_map(|id| self.summary_card(*id))
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

/// Draws the spellcheck-suggestion popup centered on screen.
pub(super) fn render_suggest(
    frame: &mut Frame,
    popup: &mut SuggestPopup,
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
    let len = popup.items.len();
    popup.scroll.render(
        frame,
        list,
        area,
        len,
        area.height.saturating_sub(2) as usize, // the panel's borders
        Some(selected),
    );
}

/// Draws the modal confirmation popup for an irreversible operation
/// The modal confirmation popup for a destructive chat action (`Ctrl+R`,
/// `Ctrl+E`, `/profile delete`, `/self clear`).
///
/// The drawing lives in [`crate::shared::ui::confirm_popup`], shared with the
/// changes screen's revert question; what stays here is *which* question, since
/// that is the part the two screens do not have in common.
pub(super) fn render_confirm(
    frame: &mut Frame,
    action: &ConfirmAction,
    palette: &Palette,
    loc: &'static Locale,
) {
    crate::shared::ui::confirm_popup(
        frame,
        palette,
        loc.t("ui.confirm.title"),
        &action.prompt(loc),
        loc.t("ui.confirm.footer"),
    );
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
