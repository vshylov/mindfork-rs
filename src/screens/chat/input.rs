//! Chat screen — key/mouse/paste handling, draft, spellcheck, commands. Part of
//! the [`super`] module; split out of the chat.rs monolith (see
//! docs/history/refactoring-god-objects.md, stage 2).

use super::feed::feed_msg_has_risky_glyph;
use super::*;

/// One slash-command parser of [`ChatScreen::COMMAND_PARSERS`]: `Some` when it
/// claimed the line (carrying what `Enter` then yields), `None` to pass.
type CommandParser = fn(&mut ChatScreen, &str) -> Option<Option<ChatIntent>>;

impl ChatScreen {
    // ---------- input ----------

    /// Handles a key press, returning an intent for `app` (or `None` if the
    /// key was handled inside the screen: input, scrolling, overlay
    /// navigation).
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<ChatIntent> {
        if key.kind != KeyEventKind::Press {
            return None;
        }
        if let Some(handled) = self.route_modal_key(key) {
            return handled;
        }
        // Ctrl shortcuts are matched by the "physical" Latin key — so they
        // work under any layout (Russian JCUKEN gives `Ctrl+д` instead of
        // `Ctrl+l`). See shared::keys, spec §11.7.
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && let Some(physical) = keys::hotkey_char(&key)
            && let Some(handled) = self.handle_ctrl_shortcut(physical)
        {
            return handled;
        }
        self.handle_plain_key(key)
    }

    /// Routes the key to whichever modal state is open (in-feed search, the
    /// help dialog, the impersonation preview, the popups, the profile
    /// overlay). Returns `Some(result)` when a modal state consumed the key
    /// (the value is what [`Self::handle_key`] returns), `None` when nothing
    /// modal is open. The ORDER of the checks is load-bearing — e.g. the
    /// tool-confirmation popup is checked **before** the generation gate in
    /// the ordinary routing.
    fn route_modal_key(&mut self, key: KeyEvent) -> Option<Option<ChatIntent>> {
        // In-feed search (`Ctrl+F`) captures input while it is open: keys go to
        // the query field, `Enter`/`↓` and `Shift+Enter`/`↑` step through matches,
        // `Esc` closes. See docs/history/in-feed-search.md §3.
        if self.search.is_some() {
            return Some(self.handle_search_key(key));
        }
        if self.help.is_some() {
            return Some(self.handle_help_key(key));
        }
        if self.impersonation.is_some() {
            return Some(self.handle_impersonation_key(key));
        }
        // A dangerous tool call is waiting for an answer (spec §9.8). Checked
        // **before** the generation gate below, because unlike every other
        // popup this one is open precisely while the turn runs — the loop is
        // parked on it.
        if self.tool_confirm.is_some() {
            return Some(self.handle_tool_confirm_key(key));
        }
        // The modal confirmation popup (`Ctrl+R`/`Ctrl+E`): Enter — yes, Esc
        // — no, other keys are ignored (the popup stays open). See spec
        // §11.7.
        if self.confirm.is_some() {
            return Some(self.handle_confirm_key(key));
        }
        if self.suggest.is_some() {
            self.handle_suggest_key(key);
            return Some(None);
        }
        if self.emoji.is_some() {
            self.handle_emoji_key(key);
            return Some(None);
        }
        if self.chat_links.is_some() {
            return Some(self.handle_chat_link_key(key));
        }
        if self.profile_overlay.is_some() {
            return Some(self.handle_profile_overlay_key(key));
        }
        None
    }

    /// The help/"About" dialog (`F1`/`?`) intercepts input: `Tab`/`←→`
    /// switch tabs, `↑↓`/`PgUp`/`PgDn`/`Home` scroll the active tab, `Esc`
    /// (and a repeat `F1`/`?`) close it, `Ctrl+Q`/`F10` — quit. Other keys
    /// are ignored (they don't close it — otherwise navigation would get
    /// confusing). Scroll clamping — in `render_help`. See spec §11.7.
    fn handle_help_key(&mut self, key: KeyEvent) -> Option<ChatIntent> {
        let help = self.help.as_mut()?;
        // Quit punches through the dialog (layout-independent), as in the
        // confirmation popup.
        if key.code == KeyCode::F(10)
            || (key.modifiers.contains(KeyModifiers::CONTROL)
                && keys::hotkey_char(&key) == Some('q'))
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
        None
    }

    /// During impersonation the input box is hidden (a preview is shown
    /// instead): react only to cancel (`Esc`) and quit (`Ctrl+Q`/`F10`);
    /// ignore other keys.
    fn handle_impersonation_key(&self, key: KeyEvent) -> Option<ChatIntent> {
        if key.code == KeyCode::F(10) {
            return Some(ChatIntent::Quit);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && keys::hotkey_char(&key) == Some('q') {
            return Some(ChatIntent::Quit);
        }
        if key.code == KeyCode::Esc {
            return Some(ChatIntent::CancelImpersonation);
        }
        None
    }

    /// A layout-independent `Ctrl+<key>` shortcut (`physical` — the physical
    /// Latin key). Returns `Some(result)` when the shortcut was recognized
    /// (the value is what [`Self::handle_key`] returns), `None` for an
    /// unclaimed key, which then falls through to the ordinary routing
    /// (e.g. `Ctrl+A`/`Ctrl+Z` belong to the input box).
    /// `pub(super)` because the typed routes reach their action through this
    /// very function (`super::commands`): a command and its chord must not grow
    /// two behaviours. See docs/history/command-only-control.md §4.3.
    pub(super) fn handle_ctrl_shortcut(&mut self, physical: char) -> Option<Option<ChatIntent>> {
        match physical {
            // Quit moved to Ctrl+Q/F10 (F10 — in `handle_plain_key`);
            // Ctrl+C was freed up for copying. See
            // docs/history/input-selection-undo-mouse.md §B.
            'q' => Some(Some(ChatIntent::Quit)),
            // Copy the selection to the clipboard (Ctrl+C). Writing it is
            // a runtime side effect (`AppCommand` isn't needed — the text
            // is already in the UI). No-op without a selection.
            'c' => {
                if self.input.has_selection() {
                    let text = self.input.selected_text().unwrap_or_default();
                    self.input.clear_selection();
                    return Some(Some(ChatIntent::CopyToClipboard(text)));
                }
                Some(None)
            }
            // Cut the selection (Ctrl+X): copy + delete.
            'x' => {
                if self.input.has_selection() {
                    let text = self.input.selected_text().unwrap_or_default();
                    self.input.delete_selection();
                    self.mark_input_changed();
                    return Some(Some(ChatIntent::CopyToClipboard(text)));
                }
                Some(None)
            }
            // Paste (Ctrl+V): an image off the clipboard if there is one, otherwise the
            // text — which is what the help overlay has always advertised this key for.
            // Reaching the app at all is up to the terminal: Windows Terminal binds
            // Ctrl+V to its own paste and never forwards it, which is exactly why
            // `/image paste` exists as well (spec §9.10).
            'v' => Some(Some(ChatIntent::PasteImage {
                text_fallback: true,
            })),
            // The settings screen (Ctrl+P) — opens once the settings
            // snapshot has arrived.
            'p' => Some(
                self.settings_snapshot
                    .is_some()
                    .then_some(ChatIntent::OpenSettings),
            ),
            'n' => Some(self.request_new_chat()),
            // Regenerate / delete the last exchange (only while not
            // generating). See spec §11.7.
            'r' if self.refuse_read_only("Ctrl+R") => Some(None),
            'e' if self.refuse_read_only("Ctrl+E") => Some(None),
            'u' if self.refuse_read_only("Ctrl+U") => Some(None),
            'r' => Some(self.trigger_destructive(ConfirmAction::Regenerate)),
            'e' => Some(self.trigger_destructive(ConfirmAction::DeleteExchange)),
            // Impersonation: write a message on the user's behalf (spec
            // §11.8). `seed` — the text already typed (the model
            // continues it).
            'u' => Some((!self.generating).then(|| ChatIntent::Impersonate {
                seed: self.input.text(),
            })),
            // In-feed text search (docs/history/in-feed-search.md). `/` is not
            // available: the input box is always focused, and `/` in an
            // empty box is how a command starts (§1.1).
            'f' => {
                self.open_feed_search();
                Some(None)
            }
            // Spellcheck suggestions for the word under the cursor (spec
            // §11.5).
            'g' => {
                self.open_suggestions();
                Some(None)
            }
            // Following a `chat://` reference the assistant wrote (spec
            // §11.3). A key rather than a click: mouse capture is `Ctrl+W` and
            // off by default, so a click is unreachable for most users
            // (docs/research/chat-uri-links.md fork F4).
            'l' => {
                self.open_chat_links();
                Some(None)
            }
            // The emoji picker popup (spec §11.5). Restore the previous
            // selection.
            'b' => {
                self.emoji = Some(EmojiPickerState::with_selected(self.emoji_last));
                Some(None)
            }
            // Collapsing "thoughts" (spec §11.3).
            // Collapsing "thoughts" (spec §11.3). The new state goes back to
            // the orchestrator, which stores it on the chat — the collapse
            // state is per chat, like the draft (docs/feed-collapse.md).
            't' => {
                self.feed_view.toggle_thoughts();
                // Collapsing reshapes all feed blocks — the same kind of
                // content change as streaming/a note.
                self.mark_feed_changed();
                Some(Some(ChatIntent::SetFeedView(self.feed_view.view())))
            }
            // Collapsing tool calls (spec §11.3) — the header stays, the
            // arguments and results fold away.
            'o' => {
                self.feed_view.toggle_tools();
                self.mark_feed_changed();
                Some(Some(ChatIntent::SetFeedView(self.feed_view.view())))
            }
            // The toggle between wheel scrolling ↔ mouse text selection
            // (spec §11.3). `Ctrl+M` doesn't work for this: the terminal
            // reports it as Enter.
            'w' => {
                self.mouse_scroll = !self.mouse_scroll;
                Some(Some(ChatIntent::SetMouseCapture(self.mouse_scroll)))
            }
            _ => None,
        }
    }

    /// Routing for a key with no modal state open and no Ctrl shortcut
    /// claimed (function keys, scrolling, `Esc`, `Enter`/send, plain typing
    /// into the input box).
    fn handle_plain_key(&mut self, key: KeyEvent) -> Option<ChatIntent> {
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
            // What the assistant changed in the attached project. `F4` was the
            // one function key free in the app and unclaimed by both measured
            // hosts (VS Code's `commandsToSkipShell`, browser tabs) — design
            // fork F10.
            (KeyCode::F(4), _) => Some(ChatIntent::OpenChanges),
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
            (KeyCode::Enter, _) => self.handle_enter(),
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

    /// The slash-command parsers, in dispatch order. Each claims the line by
    /// answering `Some` (the inner value being what `Enter` yields) and passes
    /// on it with `None`; the first to claim it wins, so the order *is* the
    /// precedence. A claimed line never goes out as a message.
    const COMMAND_PARSERS: &[CommandParser] = &[
        // A read-only transcript refuses what would change it, before any
        // parser gets the line (spec §11.2); the rest of the commands work.
        Self::try_read_only_refusal,
        Self::try_rag_command,
        Self::try_reindex_command,
        Self::try_compact_command,
        Self::try_project_command,
        Self::try_file_command,
        Self::try_image_command,
        Self::try_tts_command,
        // The registry of typed routes (`/settings`, `/find`, `/regen`, …).
        // Like the commands above it runs before the `generating` gate — some of
        // its own arms are the ones that answer during a turn (`/stop`), and the
        // rest report why they cannot.
        Self::try_ui_command,
        // `/profile …` has a subcommand plus a name, so it keeps a parser of its
        // own rather than a registry row (stage 2, fork F5).
        Self::try_profile_command,
        // `/export [md|json] [path]` — a format word and a path, so it too keeps
        // a parser of its own rather than a registry row.
        Self::try_export_command,
        Self::try_exit_command,
    ];

    /// `Enter` on the input box: slash commands (`/rag`, `/reindex`,
    /// `/compact`, `/file`, `/image`, `/tts`, `/exit`) are intercepted and never
    /// go out as messages; anything else is sent (unless a turn is already
    /// generating).
    fn handle_enter(&mut self) -> Option<ChatIntent> {
        let text = self.input.text();
        if text.trim().is_empty() {
            return None;
        }
        if let Some(intent) = self.try_command(&text) {
            return intent;
        }
        if self.read_only() {
            // Said, not swallowed: the box is still there for commands.
            self.refuse_read_only(self.loc.t("ui.chat.read_only.sending"));
            return None;
        }
        if self.generating {
            return None;
        }
        self.input.clear();
        self.mark_input_changed();
        Some(ChatIntent::Send(text))
    }

    /// Offers the line to every parser of [`Self::COMMAND_PARSERS`] in turn;
    /// `None` when no command claimed it and it is a message to send.
    fn try_command(&mut self, text: &str) -> Option<Option<ChatIntent>> {
        Self::COMMAND_PARSERS
            .iter()
            .find_map(|parse| parse(self, text))
    }

    /// The commands a sub-agent transcript cannot take — the ones that would
    /// change the conversation or its environment — refused with the note that
    /// names the parent (spec §11.2). `None` when the line is not one of them.
    fn try_read_only_refusal(&mut self, text: &str) -> Option<Option<ChatIntent>> {
        if !self.read_only() {
            return None;
        }
        const BLOCKED: &[&str] = &[
            "/compact",
            "/file",
            "/image",
            "/project",
            "/regen",
            "/retry",
            "/takeback",
            "/impersonate",
            "/clone",
        ];
        let head = text.split_whitespace().next()?;
        let head = head.to_ascii_lowercase();
        if !BLOCKED.contains(&head.as_str()) {
            return None;
        }
        self.input.clear();
        self.mark_input_changed();
        self.refuse_read_only(&head);
        Some(None)
    }

    /// A RAG slash command (`/rag add …`) — isn't sent as a message and
    /// works independently of generation (background indexing). Returns
    /// `None` when the text is not a `/rag` command.
    fn try_rag_command(&mut self, text: &str) -> Option<Option<ChatIntent>> {
        use crate::features::rag_command::RagCommand;
        let parsed = crate::features::rag_command::parse(text, self.loc)?;
        self.input.clear();
        self.mark_input_changed();
        Some(match parsed {
            Ok(RagCommand::Add { path, recursive }) => Some(ChatIntent::RagAdd { path, recursive }),
            Ok(RagCommand::Delete { path }) => Some(ChatIntent::RagDelete { path }),
            Ok(RagCommand::List) => Some(ChatIntent::RagList),
            Ok(RagCommand::Rebuild) => Some(ChatIntent::RagRebuild),
            Err(msg) => {
                self.push_note(&format!("RAG: {msg}"));
                None
            }
        })
    }

    /// The re-embedding command (`/reindex`) — also not a message,
    /// and also background work. It takes no arguments; a malformed
    /// one leaves a hint instead of going out to the model. See
    /// docs/research/embedding-model-change-reindex.md §8.1. Returns `None`
    /// when the text is not a `/reindex` command.
    fn try_reindex_command(&mut self, text: &str) -> Option<Option<ChatIntent>> {
        let parsed = crate::features::reindex_command::parse(text, self.loc)?;
        self.input.clear();
        self.mark_input_changed();
        Some(match parsed {
            Ok(()) => Some(ChatIntent::Reindex),
            Err(msg) => {
                self.push_note(&msg);
                None
            }
        })
    }

    /// The history-compaction command (`/compact`) — not a message either, and
    /// also background work. It takes no arguments; a malformed one leaves a
    /// hint instead of going out to the model. See spec §6.7. Returns `None`
    /// when the text is not a `/compact` command.
    fn try_compact_command(&mut self, text: &str) -> Option<Option<ChatIntent>> {
        let parsed = crate::features::compact_command::parse(text, self.loc)?;
        self.input.clear();
        self.mark_input_changed();
        Some(match parsed {
            Ok(()) => Some(ChatIntent::Compact),
            Err(msg) => {
                self.push_note(&msg);
                None
            }
        })
    }

    /// A file-attachment slash command (`/file …`) — not a message
    /// either; works during generation (reading happens in the
    /// background). See docs/file-attachments.md. Returns `None` when the
    /// text is not a `/file` command.
    fn try_file_command(&mut self, text: &str) -> Option<Option<ChatIntent>> {
        use crate::features::file_command::FileCommand;
        let parsed = crate::features::file_command::parse(text, self.loc)?;
        self.input.clear();
        self.mark_input_changed();
        Some(match parsed {
            Ok(FileCommand::Attach { path }) => Some(ChatIntent::FileAttach { path }),
            Ok(FileCommand::Remove { target }) => Some(ChatIntent::FileRemove { target }),
            Ok(FileCommand::List) => Some(ChatIntent::FileList),
            Err(msg) => {
                self.push_error(&self.loc.tf("ui.file.failed", &[("err", &msg)]));
                None
            }
        })
    }

    /// A code-workspace slash command (`/project …`) — not a message either.
    /// Sits beside `/file`: both attach something to *this chat* for the whole
    /// conversation. Works during generation, like its neighbours — attaching a
    /// project is a directory check, not a turn. Returns `None` when the text is
    /// not a `/project` command. See docs/history/code-workspace.md, spec §9.12.
    fn try_project_command(&mut self, text: &str) -> Option<Option<ChatIntent>> {
        use crate::features::project_command::ProjectCommand;
        let parsed = crate::features::project_command::parse(text, self.loc)?;
        self.input.clear();
        self.mark_input_changed();
        Some(match parsed {
            Ok(ProjectCommand::Attach { path }) => Some(ChatIntent::ProjectAttach { path }),
            Ok(ProjectCommand::Detach) => Some(ChatIntent::ProjectDetach),
            Ok(ProjectCommand::Status) => Some(ChatIntent::ProjectStatus),
            Ok(ProjectCommand::Slot { slot, action }) => {
                Some(ChatIntent::ProjectSlot { slot, action })
            }
            Err(msg) => {
                self.push_error(&self.loc.tf("ui.project.failed", &[("err", &msg)]));
                None
            }
        })
    }

    /// An image slash command (`/image …`) — not a message either; the
    /// decoding and downscaling happen in the background, so it works during
    /// generation like `/file`. Sits right after `/file` in the chain: the two
    /// are neighbours in wording and the user reaches for both while writing.
    /// See spec §9.10. Returns `None` when the text is not an `/image` command.
    fn try_image_command(&mut self, text: &str) -> Option<Option<ChatIntent>> {
        use crate::features::image_command::ImageCommand;
        let parsed = crate::features::image_command::parse(text, self.loc)?;
        self.input.clear();
        self.mark_input_changed();
        Some(match parsed {
            Ok(ImageCommand::Attach { path }) => Some(ChatIntent::ImageAttach { path }),
            Ok(ImageCommand::Remove { target }) => Some(ChatIntent::ImageRemove { target }),
            Ok(ImageCommand::List) => Some(ChatIntent::ImageList),
            // A typed command must not silently turn into a text paste: the user asked
            // for an image, so an absent one is reported rather than substituted.
            Ok(ImageCommand::Paste) => Some(ChatIntent::PasteImage {
                text_fallback: false,
            }),
            Err(msg) => {
                self.push_error(&self.loc.tf("ui.image.failed", &[("err", &msg)]));
                None
            }
        })
    }

    /// A speech slash command (`/tts …`) — also not a message, and
    /// it also works during generation (a snapshot gets spoken).
    /// See spec §11.9. Returns `None` when the text is not a `/tts` command.
    fn try_tts_command(&mut self, text: &str) -> Option<Option<ChatIntent>> {
        use crate::features::tts_command::TtsCommand;
        let parsed = crate::features::tts_command::parse(text)?;
        self.input.clear();
        self.mark_input_changed();
        Some(match parsed {
            Ok(TtsCommand::Speak(scope)) => Some(ChatIntent::Tts(scope)),
            Ok(TtsCommand::Stop) => Some(ChatIntent::TtsStop),
            Ok(TtsCommand::Pause) => Some(ChatIntent::TtsPause),
            Ok(TtsCommand::Resume) => Some(ChatIntent::TtsResume),
            Err(arg) => {
                self.push_note(&self.loc.tf("ui.tts.bad_arg", &[("arg", &arg)]));
                None
            }
        })
    }

    /// A quit slash command (`/exit`, `/quit`) — a typed route out for terminals
    /// that swallow both `Ctrl+Q` and `F10` (VS Code's integrated terminal binds
    /// each of them to the editor). Like the keys, it quits during generation
    /// too — hence its place after the check chain but before the `generating`
    /// gate. See spec §11.7.
    ///
    /// The box is cleared first: the draft of every keystroke has already reached
    /// the orchestrator, so leaving `/exit` in it would greet the user with the
    /// command they used to leave. The runtime flushes this last draft on its way
    /// out (`app::runtime::run_loop`).
    fn try_exit_command(&mut self, text: &str) -> Option<Option<ChatIntent>> {
        let parsed = crate::features::exit_command::parse(text, self.loc)?;
        self.input.clear();
        self.mark_input_changed();
        Some(match parsed {
            Ok(()) => Some(ChatIntent::Quit),
            Err(msg) => {
                self.push_note(&msg);
                None
            }
        })
    }

    /// Inserts clipboard text into the input box (the `Event::Paste` event —
    /// bracketed paste). The paste goes in as one chunk, line breaks are kept
    /// as text (and are NOT treated as Enter/send). Works only in the main
    /// view: with help/a popup/an overlay open (their single-line fields) —
    /// a no-op. A paste never sends a message even with line breaks inside.
    /// See spec §11.5.
    /// Opens in-feed search, resuming the query last typed in this chat.
    ///
    /// The message input box is left completely alone — every mode in this screen
    /// preserves it, and a half-written message must survive a search.
    pub(super) fn open_feed_search(&mut self) {
        let mut field = InputBox::new();
        // Before `set_text`: the single-line invariant (see `InputBox`).
        field.set_single_line(true);
        if !self.search_last.is_empty() {
            field.set_text(&self.search_last);
        }
        self.search = Some(Box::new(field));
        let q = self.search_last.clone();
        self.feed_view
            .set_search((!q.is_empty()).then_some(q.as_str()));
        self.request_full_redraw();
    }

    /// Leaves in-feed search, dropping its highlight. The query itself is kept
    /// for the next `Ctrl+F` in this chat (fork F5).
    pub(super) fn close_feed_search(&mut self) {
        self.search = None;
        self.feed_view.clear_search();
        self.request_full_redraw();
    }

    /// Keys while the search field is open (fork F5): `Enter`/`↓` next,
    /// `Shift+Enter`/`↑` previous, `Esc` closes, `Ctrl+Q`/`F10` still quit.
    /// Everything else is text, and re-runs the search as you type — which costs
    /// a warm frame, not a re-render (stage 3a, §1.2).
    fn handle_search_key(&mut self, key: KeyEvent) -> Option<ChatIntent> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && keys::hotkey_char(&key) == Some('q') {
            return Some(ChatIntent::Quit);
        }
        match (key.code, key.modifiers) {
            (KeyCode::F(10), _) => return Some(ChatIntent::Quit),
            (KeyCode::Esc, _) => {
                self.close_feed_search();
                return None;
            }
            (KeyCode::Enter, m) if m.contains(KeyModifiers::SHIFT) => {
                self.feed_view.prev_match();
                return None;
            }
            (KeyCode::Enter, _) | (KeyCode::Down, _) => {
                self.feed_view.next_match();
                return None;
            }
            (KeyCode::Up, _) => {
                self.feed_view.prev_match();
                return None;
            }
            _ => {}
        }
        if let Some(field) = &mut self.search
            && field.on_key(key).edited()
        {
            let q = field.text();
            self.search_last = q.clone();
            self.feed_view
                .set_search((!q.is_empty()).then_some(q.as_str()));
        }
        None
    }

    pub fn handle_paste(&mut self, text: &str) {
        // Fast typing arrives as one coalesced paste (§1.5), so the search field
        // needs its own target — without this, typing quickly into it would land
        // in the message the user was writing.
        if let Some(field) = &mut self.search {
            if !text.is_empty() {
                field.insert_str(text);
                let q = field.text();
                self.search_last = q.clone();
                self.feed_view.set_search(Some(&q));
            }
            return;
        }
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
    /// on): the wheel scrolls the chat feed; a left click on a `chat://`
    /// reference in the feed follows it (spec §11.3); a left-button click/drag
    /// in the input box places the cursor / grows the selection (stage D of the
    /// plan). Works only in the main view — with an overlay/popup/help open
    /// this is a no-op (scrolling/cursor edits under them would be
    /// unexpected). A click elsewhere in the feed is still a no-op (feed
    /// selection is a separate track). See spec §11.3, §11.5.
    ///
    /// The reference is tried **before** the input box, and the two cannot
    /// compete: they own disjoint areas of the screen, and `mouse_press`
    /// already answers `false` outside its own.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> Option<ChatIntent> {
        if self.search.is_some()
            || self.help.is_some()
            || self.suggest.is_some()
            || self.emoji.is_some()
            || self.chat_links.is_some()
            || self.profile_overlay.is_some()
            || self.confirm.is_some()
        {
            return None;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => self.feed_view.scroll_up(WHEEL_SCROLL),
            MouseEventKind::ScrollDown => self.feed_view.scroll_down(WHEEL_SCROLL),
            // Click/drag in the input box — only when the field is visible
            // (during impersonation a preview sits in its place, and the
            // field's `last_area` is stale).
            MouseEventKind::Down(MouseButton::Left) if self.impersonation.is_none() => {
                if let Some(chat) = self.feed_view.chat_link_at(mouse.column, mouse.row) {
                    // Clicking the conversation you are already in is not a
                    // switch — the picker's rule, and the same wording.
                    if self.active_chat == Some(chat) {
                        self.push_note(self.loc.t("ui.chat.links_here"));
                        return None;
                    }
                    return Some(ChatIntent::OpenChatLink(chat));
                }
                self.input.mouse_press(mouse.column, mouse.row);
            }
            MouseEventKind::Drag(MouseButton::Left) if self.impersonation.is_none() => {
                self.input.mouse_drag(mouse.column, mouse.row);
            }
            _ => {}
        }
        None
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

    /// Whether the current input is a command (`/rag …`, `/file …`, `/project …`,
    /// `/image …`, `/tts …`, `/reindex`, `/compact`, `/exit`, and everything in the
    /// typed-route registry). Such text is highlighted yellow and isn't
    /// spellchecked. See spec §11.5.
    ///
    /// The arms below are the same parsers, in the same order, as the dispatch
    /// chain in `submit` — the box and `Enter` have to agree about what a command
    /// is, and the one time they drifted (`/project`, added to the chain alone)
    /// the command worked while typing it looked like prose.
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
            || crate::features::reindex_command::parse(&text, self.loc).is_some()
            || crate::features::compact_command::parse(&text, self.loc).is_some()
            || crate::features::file_command::parse(&text, self.loc).is_some()
            || crate::features::project_command::parse(&text, self.loc).is_some()
            || crate::features::image_command::parse(&text, self.loc).is_some()
            || crate::features::tts_command::parse(&text).is_some()
            || crate::features::ui_command::parse(&text, self.loc).is_some()
            || crate::features::profile_command::parse(&text, self.loc).is_some()
            || crate::features::export_command::parse(&text, self.loc).is_some()
            || crate::features::exit_command::parse(&text, self.loc).is_some()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn type_str(s: &mut ChatScreen, text: &str) {
        for c in text.chars() {
            s.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    fn enter(s: &mut ChatScreen) -> Option<ChatIntent> {
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    }

    #[test]
    fn reindex_command_intercepted_on_enter() {
        let mut s = ChatScreen::new();
        type_str(&mut s, "/reindex");
        assert_eq!(enter(&mut s), Some(ChatIntent::Reindex));
        assert!(s.input.is_empty(), "the field is cleared after the command");
        assert!(
            !s.feed.iter().any(|m| m.role == FeedRole::User),
            "a command must not go out as a message"
        );
    }

    #[test]
    fn invalid_reindex_command_shows_note_and_does_not_send() {
        let mut s = ChatScreen::new();
        type_str(&mut s, "/reindex now");
        assert_eq!(enter(&mut s), None, "a malformed command isn't sent");
        assert!(
            s.feed.iter().any(|m| m.role == FeedRole::Note),
            "the syntax hint goes into the feed as a note"
        );
        assert!(!s.feed.iter().any(|m| m.role == FeedRole::User));
    }

    #[test]
    fn reindex_input_is_recognized_as_command() {
        let mut s = ChatScreen::new();
        type_str(&mut s, "/reindex");
        assert!(
            s.input_is_command(),
            "the input box highlights it and skips spellcheck"
        );
        // A malformed one is recognized too — it's still a command, not prose.
        s.input.clear();
        type_str(&mut s, "/reindex now");
        assert!(s.input_is_command());
        // Prose that merely mentions the word isn't.
        s.input.clear();
        type_str(&mut s, "please reindex the base");
        assert!(!s.input_is_command());
    }

    #[test]
    fn compact_command_intercepted_on_enter() {
        let mut s = ChatScreen::new();
        type_str(&mut s, "/compact");
        assert_eq!(enter(&mut s), Some(ChatIntent::Compact));
        assert!(s.input.is_empty(), "the field is cleared after the command");
        assert!(
            !s.feed.iter().any(|m| m.role == FeedRole::User),
            "a command must not go out as a message"
        );
    }

    #[test]
    fn invalid_compact_command_shows_note_and_does_not_send() {
        let mut s = ChatScreen::new();
        type_str(&mut s, "/compact now");
        assert_eq!(enter(&mut s), None, "a malformed command isn't sent");
        assert!(
            s.feed.iter().any(|m| m.role == FeedRole::Note),
            "the syntax hint goes into the feed as a note"
        );
        assert!(!s.feed.iter().any(|m| m.role == FeedRole::User));
    }

    #[test]
    fn compact_input_is_recognized_as_command() {
        let mut s = ChatScreen::new();
        type_str(&mut s, "/compact");
        assert!(
            s.input_is_command(),
            "the input box highlights it and skips spellcheck"
        );
        // A malformed one is recognized too — it's still a command, not prose.
        s.input.clear();
        type_str(&mut s, "/compact now");
        assert!(s.input_is_command());
        // Prose that merely mentions the word isn't.
        s.input.clear();
        type_str(&mut s, "please compact the history");
        assert!(!s.input_is_command());
    }

    #[test]
    fn exit_input_is_recognized_as_command() {
        for cmd in crate::features::exit_command::ALIASES {
            let mut s = ChatScreen::new();
            type_str(&mut s, cmd);
            assert!(
                s.input_is_command(),
                "{cmd}: the input box highlights it and skips spellcheck"
            );
            // A malformed one is recognized too — it's still a command, not prose.
            s.input.clear();
            type_str(&mut s, &format!("{cmd} now"));
            assert!(s.input_is_command(), "{cmd} now");
            // A longer word that merely starts with it is prose, and so is a
            // sentence containing the word — both go out as messages, so both
            // must look like messages while being typed.
            s.input.clear();
            type_str(&mut s, &format!("{cmd}ing"));
            assert!(!s.input_is_command(), "{cmd}ing");
            s.input.clear();
            type_str(&mut s, "how do I quit vim?");
            assert!(!s.input_is_command());
        }
    }
}
