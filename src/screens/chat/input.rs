//! Chat screen — key/mouse/paste handling, draft, spellcheck, commands. Part of
//! the [`super`] module; split out of the chat.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 2).

use super::feed::feed_msg_has_risky_glyph;
use super::*;

impl ChatScreen {
    // ---------- input ----------

    /// Handles a key press, returning an intent for `app` (or `None` if the
    /// key was handled inside the screen: input, scrolling, overlay
    /// navigation).
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<ChatIntent> {
        if key.kind != KeyEventKind::Press {
            return None;
        }
        // The help/"About" dialog (`F1`/`?`) intercepts input: `Tab`/`←→`
        // switch tabs, `↑↓`/`PgUp`/`PgDn`/`Home` scroll the active tab, `Esc`
        // (and a repeat `F1`/`?`) close it, `Ctrl+Q`/`F10` — quit. Other keys
        // are ignored (they don't close it — otherwise navigation would get
        // confusing). Scroll clamping — in `render_help`. See spec §11.7.
        if let Some(help) = &mut self.help {
            // Quit punches through the dialog (layout-independent), as in the
            // confirmation popup.
            if key.code == KeyCode::F(10)
                || (key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(key.code, KeyCode::Char(c) if keys::physical_char(c) == 'q'))
            {
                self.help = None;
                return Some(ChatIntent::Quit);
            }
            match key.code {
                KeyCode::Tab | KeyCode::Right => help.next_tab(),
                KeyCode::BackTab | KeyCode::Left => help.prev_tab(),
                KeyCode::Up => help.scroll = help.scroll.saturating_sub(1),
                KeyCode::Down => help.scroll = help.scroll.saturating_add(1),
                KeyCode::PageUp => help.scroll = help.scroll.saturating_sub(PAGE_SCROLL),
                KeyCode::PageDown => help.scroll = help.scroll.saturating_add(PAGE_SCROLL),
                KeyCode::Home => help.scroll = 0,
                KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('?') => {
                    // Remember the tab to restore it on the next open.
                    self.help_last_tab = help.tab;
                    self.help = None;
                }
                _ => {}
            }
            return None;
        }
        // During impersonation the input box is hidden (a preview is shown
        // instead): react only to cancel (`Esc`) and quit (`Ctrl+Q`/`F10`);
        // ignore other keys.
        if self.impersonation.is_some() {
            if key.code == KeyCode::F(10) {
                return Some(ChatIntent::Quit);
            }
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && let KeyCode::Char(c) = key.code
                && keys::physical_char(c) == 'q'
            {
                return Some(ChatIntent::Quit);
            }
            if key.code == KeyCode::Esc {
                return Some(ChatIntent::CancelImpersonation);
            }
            return None;
        }
        // The modal confirmation popup (`Ctrl+R`/`Ctrl+E`): Enter — yes, Esc
        // — no, other keys are ignored (the popup stays open). See spec
        // §11.7.
        if self.confirm.is_some() {
            return self.handle_confirm_key(key);
        }
        if self.suggest.is_some() {
            self.handle_suggest_key(key);
            return None;
        }
        if self.emoji.is_some() {
            self.handle_emoji_key(key);
            return None;
        }
        if self.profile_overlay.is_some() {
            return self.handle_profile_overlay_key(key);
        }
        // Ctrl shortcuts are matched by the "physical" Latin key — so they
        // work under any layout (Russian JCUKEN gives `Ctrl+д` instead of
        // `Ctrl+l`). See shared::keys, spec §11.7.
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && let KeyCode::Char(c) = key.code
        {
            match keys::physical_char(c) {
                // Quit moved to Ctrl+Q/F10 (F10 — in the code match below);
                // Ctrl+C was freed up for copying. See
                // docs/history/input-selection-undo-mouse.md §B.
                'q' => return Some(ChatIntent::Quit),
                // Copy the selection to the clipboard (Ctrl+C). Writing it is
                // a runtime side effect (`AppCommand` isn't needed — the text
                // is already in the UI). No-op without a selection.
                'c' => {
                    if self.input.has_selection() {
                        let text = self.input.selected_text().unwrap_or_default();
                        self.input.clear_selection();
                        return Some(ChatIntent::CopyToClipboard(text));
                    }
                    return None;
                }
                // Cut the selection (Ctrl+X): copy + delete.
                'x' => {
                    if self.input.has_selection() {
                        let text = self.input.selected_text().unwrap_or_default();
                        self.input.delete_selection();
                        self.mark_input_changed();
                        return Some(ChatIntent::CopyToClipboard(text));
                    }
                    return None;
                }
                // The settings screen (Ctrl+P) — opens once the settings
                // snapshot has arrived.
                'p' => {
                    return self
                        .settings_snapshot
                        .is_some()
                        .then_some(ChatIntent::OpenSettings);
                }
                'n' => return self.request_new_chat(),
                // Regenerate / delete the last exchange (only while not
                // generating). See spec §11.7.
                'r' => return self.trigger_destructive(ConfirmAction::Regenerate),
                'e' => return self.trigger_destructive(ConfirmAction::DeleteExchange),
                // Impersonation: write a message on the user's behalf (spec
                // §11.8). `seed` — the text already typed (the model
                // continues it).
                'u' => {
                    return (!self.generating).then(|| ChatIntent::Impersonate {
                        seed: self.input.text(),
                    });
                }
                // Spellcheck suggestions for the word under the cursor (spec
                // §11.5).
                'g' => {
                    self.open_suggestions();
                    return None;
                }
                // The emoji picker popup (spec §11.5). Restore the previous
                // selection.
                'b' => {
                    self.emoji = Some(EmojiPickerState::with_selected(self.emoji_last));
                    return None;
                }
                // Collapsing "thoughts" (spec §11.3).
                't' => {
                    self.feed_view.toggle_thoughts();
                    // Collapsing "thoughts" reshapes all feed blocks — the
                    // same kind of content change as streaming/a note.
                    self.mark_feed_changed();
                    return None;
                }
                // The toggle between wheel scrolling ↔ mouse text selection
                // (spec §11.3). `Ctrl+M` doesn't work for this: the terminal
                // reports it as Enter.
                'w' => {
                    self.mouse_scroll = !self.mouse_scroll;
                    return Some(ChatIntent::SetMouseCapture(self.mouse_scroll));
                }
                _ => {}
            }
        }
        match (key.code, key.modifiers) {
            // Quit — Ctrl+Q (above) or F10 (a second option in case the
            // terminal intercepts Ctrl+Q; F10 often opens the emulator's menu
            // in Linux DEs, but it's dismissible).
            (KeyCode::F(10), _) => Some(ChatIntent::Quit),
            // Help/"About": F1 always works; `?` — only on empty input
            // (otherwise the character gets typed). Opens on the
            // last-selected tab. See spec §11.7.
            (KeyCode::F(1), _) => {
                self.help = Some(HelpState::open(self.help_last_tab));
                None
            }
            (KeyCode::Char('?'), KeyModifiers::NONE) if self.input.is_empty() => {
                self.help = Some(HelpState::open(self.help_last_tab));
                None
            }
            // View of the active profile's "self-model" (a read-only view).
            (KeyCode::F(3), _) => Some(ChatIntent::OpenSelfModel),
            // Copy the active chat's conversation to the clipboard (like F5
            // in the chat list). The confirmation/error arrives as a note in
            // the feed (no overlay).
            (KeyCode::F(5), _) => self.active_chat.map(ChatIntent::CopyChat),
            // Feed scrolling (spec §11.3).
            (KeyCode::PageUp, _) => {
                self.feed_view.scroll_up(PAGE_SCROLL);
                None
            }
            (KeyCode::PageDown, _) => {
                self.feed_view.scroll_down(PAGE_SCROLL);
                None
            }
            // Esc opens the chat list screen (`app` builds it from the list
            // snapshot; `Esc` there closes the screen — toggling
            // "list ↔ chat"). During generation, Esc first cancels it. Quit
            // — `Ctrl+C`. See spec §11.7.
            (KeyCode::Esc, _) => {
                if self.generating {
                    Some(ChatIntent::Cancel)
                } else {
                    Some(ChatIntent::OpenChatList)
                }
            }
            // Shift+Enter — a line break; Enter — send (spec §11.7).
            // Alt+Enter — the same line break: a fallback for "bare" unix
            // terminals without the kitty keyboard protocol, where
            // Shift+Enter is indistinguishable from Enter (both send CR),
            // while Alt+Enter arrives as Enter+ALT (an ESC meta prefix) and
            // so is recognized. See item 11.
            (KeyCode::Enter, m) if m.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
                self.input.insert_newline();
                self.mark_input_changed();
                None
            }
            (KeyCode::Enter, _) => {
                let text = self.input.text();
                if text.trim().is_empty() {
                    return None;
                }
                // A RAG slash command (`/rag add …`) — isn't sent as a
                // message and works independently of generation (background
                // indexing).
                if let Some(parsed) = crate::features::rag_command::parse(&text, self.loc) {
                    use crate::features::rag_command::RagCommand;
                    self.input.clear();
                    self.mark_input_changed();
                    return match parsed {
                        Ok(RagCommand::Add { path, recursive }) => {
                            Some(ChatIntent::RagAdd { path, recursive })
                        }
                        Ok(RagCommand::Delete { path }) => Some(ChatIntent::RagDelete { path }),
                        Ok(RagCommand::List) => Some(ChatIntent::RagList),
                        Ok(RagCommand::Rebuild) => Some(ChatIntent::RagRebuild),
                        Err(msg) => {
                            self.push_note(&format!("RAG: {msg}"));
                            None
                        }
                    };
                }
                // A speech slash command (`/tts …`) — also not a message, and
                // it also works during generation (a snapshot gets spoken).
                // See spec §11.9.
                if let Some(parsed) = crate::features::tts_command::parse(&text) {
                    use crate::features::tts_command::TtsCommand;
                    self.input.clear();
                    self.mark_input_changed();
                    return match parsed {
                        Ok(TtsCommand::Speak(scope)) => Some(ChatIntent::Tts(scope)),
                        Ok(TtsCommand::Stop) => Some(ChatIntent::TtsStop),
                        Ok(TtsCommand::Pause) => Some(ChatIntent::TtsPause),
                        Ok(TtsCommand::Resume) => Some(ChatIntent::TtsResume),
                        Err(arg) => {
                            self.push_note(&self.loc.tf("ui.tts.bad_arg", &[("arg", &arg)]));
                            None
                        }
                    };
                }
                if !self.generating {
                    self.input.clear();
                    self.mark_input_changed();
                    Some(ChatIntent::Send(text))
                } else {
                    None
                }
            }
            _ => {
                // Mark the input "dirty" only on an actual edit — a bare
                // cursor move (`Moved`) shouldn't needlessly wake the
                // spellcheck debounce or send `SetDraft`. See [`KeyOutcome`].
                if self.input.on_key(key).edited() {
                    self.mark_input_changed();
                }
                None
            }
        }
    }

    /// Inserts clipboard text into the input box (the `Event::Paste` event —
    /// bracketed paste). The paste goes in as one chunk, line breaks are kept
    /// as text (and are NOT treated as Enter/send). Works only in the main
    /// view: with help/a popup/an overlay open (their single-line fields) —
    /// a no-op. A paste never sends a message even with line breaks inside.
    /// See spec §11.5.
    pub fn handle_paste(&mut self, text: &str) {
        if self.help.is_some()
            || self.suggest.is_some()
            || self.emoji.is_some()
            || self.profile_overlay.is_some()
            || self.confirm.is_some()
        {
            return;
        }
        if text.is_empty() {
            return;
        }
        self.input.insert_str(text);
        self.mark_input_changed();
    }

    /// Handles a mouse event (only arrives when mouse capture `Ctrl+W` is
    /// on): the wheel scrolls the chat feed; a left-button click/drag in the
    /// input box places the cursor / grows the selection (stage D of the
    /// plan). Works only in the main view — with an overlay/popup/help open
    /// this is a no-op (scrolling/cursor edits under them would be
    /// unexpected). A click/drag outside the field's area (into the feed) —
    /// a no-op (feed selection is a separate track). See spec §11.3, §11.5.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.help.is_some()
            || self.suggest.is_some()
            || self.emoji.is_some()
            || self.profile_overlay.is_some()
            || self.confirm.is_some()
        {
            return;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => self.feed_view.scroll_up(WHEEL_SCROLL),
            MouseEventKind::ScrollDown => self.feed_view.scroll_down(WHEEL_SCROLL),
            // Click/drag in the input box — only when the field is visible
            // (during impersonation a preview sits in its place, and the
            // field's `last_area` is stale).
            MouseEventKind::Down(MouseButton::Left) if self.impersonation.is_none() => {
                self.input.mouse_press(mouse.column, mouse.row);
            }
            MouseEventKind::Drag(MouseButton::Left) if self.impersonation.is_none() => {
                self.input.mouse_drag(mouse.column, mouse.row);
            }
            _ => {}
        }
    }

    /// Takes the "the feed was just scrolled" flag and tells the loop
    /// whether a full terminal redraw is needed (`terminal.clear()` — wipes
    /// "hanging" artifacts).
    ///
    /// A full redraw (with a brief flicker from the escape screen-clear) is
    /// done **only if** the feed has clusters that "drift" on legacy
    /// terminals — VS16 emoji (containing the U+FE0F selector, e.g.
    /// `🕸️`/`🗂️`): Command Prompt/conhost draws them physically wider than
    /// ratatui's model, content shifts, and a page-jump scroll leaves a
    /// "hanging" character behind. Plain text and ordinary wide emoji
    /// produce no artifacts — scrolling doesn't flicker there. See
    /// [`crate::widgets::message_feed::MessageFeed::take_scrolled`].
    pub(super) fn take_feed_scrolled(&mut self) -> bool {
        // The internal flag must always be reset, even if a redraw isn't
        // needed.
        if !self.feed_view.take_scrolled() {
            return false;
        }
        self.feed.iter().any(feed_msg_has_risky_glyph)
    }

    /// Requests a full terminal redraw on the next frame.
    ///
    /// Needed wherever a **wide glyph** (emoji) disappears from the screen.
    /// The mechanics of the defect (verified against `Buffer::diff`): a wide
    /// emoji occupies two cells — its own (`symbol`) and a **trailing** one,
    /// which ratatui resets to the default. When the glyph goes away, the
    /// cell in its place is redrawn, but the trailing one isn't: in both
    /// buffers it's the default space, the diff considers it unchanged and
    /// skips it. conhost/Command Prompt doesn't clear the second half on its
    /// own — a piece of it stays on screen (visible when the cell had a
    /// background).
    ///
    /// `ratatui-core` 0.1.2 (ratatui#2585) closed **part** of the case: the
    /// tail is now sent when a wide glyph is replaced by narrower content AND
    /// carried a style visible on an empty cell (a background,
    /// `REVERSED`/`UNDERLINED`/`BLINK`/`CROSSED_OUT`). Unstyled glyphs (the
    /// emoji-popup grid) stay uncovered — the request exists for their sake.
    ///
    /// **Not** needed when the glyph stays wide and only changes style (a
    /// selection shift): the full-redraw sentinel must skip a wide glyph's
    /// trailing cell (otherwise the backend would print it with no `MoveTo`
    /// and shift the row — ratatui#2651), so there's no benefit there; the
    /// backdrop is cleared by reprinting the glyph itself via the plain diff.
    pub(super) fn request_full_redraw(&mut self) {
        self.full_redraw = true;
    }

    /// Takes the request for a full terminal redraw (the `app/runtime` loop
    /// rewrites every cell in response — see the sentinel-buffer "\0"
    /// technique there too). Sources of the request: scrolling/a feed change
    /// with risk-group glyphs ([`Self::take_feed_scrolled`],
    /// [`super::ChatScreen::mark_feed_changed`]) and closing the
    /// emoji/spellcheck popups ([`Self::request_full_redraw`]).
    pub fn take_full_redraw(&mut self) -> bool {
        // Both flags are taken unconditionally (not via `||`):
        // `take_feed_scrolled` must reset the feed widget's internal state
        // regardless of the second request.
        let scrolled = self.take_feed_scrolled();
        let requested = std::mem::take(&mut self.full_redraw);
        scrolled || requested
    }

    /// Marks the input as changed (starts the spellcheck-recheck debounce and
    /// saves the draft in the active chat).
    pub(super) fn mark_input_changed(&mut self) {
        self.spell_dirty = true;
        self.draft_dirty = true;
        self.last_edit = Some(Instant::now());
        // The same class as in the feed (`mark_feed_changed`): an edit TO THE
        // LEFT of a VS16 emoji shifts it onto a foreign character's cell,
        // the diff sends the trailing half, and the backend prints it with
        // no `MoveTo` — the row shifts right (ratatui#2651). It only
        // triggers when a non-space character immediately follows the
        // glyph, so it comes up less often in the input box than in the
        // feed, but the mechanics are the same. Checked in a streaming pass
        // and only when a risk-group glyph is present — a no-op on regular
        // text.
        if self.input.any_char(super::feed::is_risky_glyph) {
            self.full_redraw = true;
        }
    }

    /// Takes the changed input draft for saving in the active chat (or
    /// `None` if it hasn't changed since last time). The loop sends it via
    /// the `SetDraft` command; the orchestrator's disk write is debounced.
    /// See spec §11.7.
    pub fn take_dirty_draft(&mut self) -> Option<String> {
        if !self.draft_dirty {
            return None;
        }
        self.draft_dirty = false;
        Some(self.input.text())
    }

    /// Rechecks the input's spelling once the debounce has elapsed. Returns
    /// `true` if the error highlighting was recomputed (a redraw is needed).
    /// Called from the loop every tick (which is also what wakes it once the
    /// debounce elapses — rendering no longer runs on every tick by itself).
    /// See spec §11.5.
    pub fn maybe_recheck_spelling(&mut self) -> bool {
        let Some(spell) = &self.spell else {
            return false;
        };
        if !self.spell_dirty {
            return false;
        }
        // Commands (`/rag …`) and file paths aren't spellchecked — clear any
        // underlines (they're highlighted yellow as a whole when rendered).
        if self.input_is_command() {
            self.input.set_misspelled(Vec::new());
            self.spell_dirty = false;
            return true;
        }
        if let Some(t) = self.last_edit
            && t.elapsed() < SPELL_DEBOUNCE
        {
            return false; // still typing — don't flag the current word
        }
        let ranges = self
            .input
            .line_strings()
            .iter()
            .map(|line| spell.misspellings(line))
            .collect();
        self.input.set_misspelled(ranges);
        self.spell_dirty = false;
        true
    }

    /// Whether the current input is a command (`/rag …`, `/tts …`). Such
    /// text is highlighted yellow and isn't spellchecked. See spec §11.5.
    /// Checked every frame, so first — a cheap guard: a command always
    /// starts with `/` (the first non-whitespace character), and only then
    /// do we parse the full text (an allocation via `text()` + parsing). For
    /// regular input (letters, Cyrillic) `text()` is never built.
    pub(super) fn input_is_command(&self) -> bool {
        if self.input.first_non_whitespace() != Some('/') {
            return false;
        }
        let text = self.input.text();
        crate::features::rag_command::parse(&text, self.loc).is_some()
            || crate::features::tts_command::parse(&text).is_some()
    }

    /// Triggers an irreversible operation (`Ctrl+R`/`Ctrl+E`): either returns
    /// the intent right away, or — if confirmation is enabled — opens a
    /// modal popup. During generation, both operations are ignored (as
    /// before). See spec §11.7.
    pub(super) fn trigger_destructive(&mut self, action: ConfirmAction) -> Option<ChatIntent> {
        if self.generating {
            return None;
        }
        if self.confirm_destructive {
            self.confirm = Some(action);
            None
        } else {
            Some(action.intent())
        }
    }

    /// Requests creating a chat: with >1 profile, opens the picker overlay,
    /// otherwise creates it right away from the sole/default profile (spec
    /// §10). Public: `app` calls it when `Ctrl+N` is pressed in the chat
    /// list screen (profile selection lives here, in the chat screen).
    pub fn request_new_chat(&mut self) -> Option<ChatIntent> {
        if self.profiles.len() > 1 {
            self.profile_overlay = Some(ProfileListState::new(self.profiles.clone()));
            None
        } else {
            Some(ChatIntent::NewChat {
                profile_id: self.profiles.first().map(|p| p.id),
            })
        }
    }

    pub(super) fn handle_profile_overlay_key(&mut self, key: KeyEvent) -> Option<ChatIntent> {
        let overlay = self.profile_overlay.as_mut()?;
        match overlay.on_key(key) {
            ProfileListAction::None => None,
            ProfileListAction::Cancel => {
                self.profile_overlay = None;
                None
            }
            ProfileListAction::Pick(id) => {
                self.profile_overlay = None;
                Some(ChatIntent::NewChat {
                    profile_id: Some(id),
                })
            }
        }
    }
}
