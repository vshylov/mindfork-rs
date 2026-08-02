//! Runtime — applies AppEvent to the screen + translates Intent → AppCommand. Part of the [`super`] module, split out of the
//! runtime.rs monolith (see docs/history/refactoring-god-objects.md, stage 7).

use super::*;

/// Applies an orchestrator event to the chat screen (a read-only projection).
/// A settings snapshot, when the settings screen is open, additionally
/// refreshes its working copy (reflects profile creation/deletion).
///
/// `back` is the search-results back-stack ([`SearchReturn`]): the
/// `ChatActivated` arm is where it gets dropped, because that event is the one
/// funnel every chat-opening route ends in.
pub(super) fn apply_event(
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
    back: &mut Option<SearchReturn>,
    clipboard: &mut Option<arboard::Clipboard>,
    cmd_tx: &UnboundedSender<AppCommand>,
    event: AppEvent,
) {
    match event {
        // Server statuses — into the chat (the status line) and, if open, into the
        // settings screen (chips in the "Model/server" section: connecting → ready is visible in place).
        AppEvent::ServerStatus(status) => {
            if let ActiveScreen::Settings(settings) = active {
                settings.set_server_statuses(status.clone());
            }
            screen.set_server_status(status);
        }
        // The list snapshot is applied to the chat always (for the next open/`Ctrl+N`),
        // and when the list screen is open — to it too (a live update).
        AppEvent::ChatList(chats) => {
            if let ActiveScreen::ChatList(list) = active {
                list.set_chats(chats.clone());
            }
            screen.set_chat_list(chats);
        }
        AppEvent::ChatRenamed { id, title } => screen.rename_chat(id, title),
        // Content-search results only mean anything to an open chat list — the
        // reply to a query it asked for. Ignored when it is closed (a late reply
        // to a list the user has since left).
        AppEvent::ChatSearchResults { query, chat_ids } => {
            if let ActiveScreen::ChatList(list) = active {
                list.set_search_results(query, chat_ids);
            }
        }
        // Message-level results (`Ctrl+G`): the screen opens on the reply, not
        // on the key press — the same round-trip as `SelfModelView`, since the
        // orchestrator owns the index. A later reply refreshes the screen in
        // place instead of stacking a second one.
        AppEvent::MessageSearchResults {
            query,
            groups,
            total,
        } => match active {
            ActiveScreen::Search(search) => search.set_results(query, groups, total),
            _ => {
                *active = ActiveScreen::Search(Box::new(SearchScreen::new(
                    query,
                    groups,
                    total,
                    screen.palette(),
                    screen.loc(),
                )))
            }
        },
        // A list-operation error: into its status area, if the screen is open; otherwise
        // (a late auto-title reply after the list is closed) — as a note in the feed.
        AppEvent::ChatListError(message) => match active {
            ActiveScreen::ChatList(list) => list.set_error(message),
            _ => screen.push_error(&message),
        },
        // Writing to the clipboard is a UI-layer side effect; the write itself and
        // routing the confirmation/error are factored into `deliver_clipboard`
        // (`apply_event` doesn't know about `arboard`).
        AppEvent::CopyToClipboard(text) => deliver_clipboard(screen, active, clipboard, &text),
        AppEvent::ProfileList(profiles) => screen.set_profile_list(profiles),
        AppEvent::Settings {
            config,
            profiles,
            language_locked,
            mcp,
            secrets_present,
        } => {
            match active {
                ActiveScreen::Settings(settings) => {
                    settings.refresh((*config).clone(), profiles.clone(), language_locked.clone());
                    settings.set_mcp(mcp.clone());
                    settings.set_secrets_present(secrets_present.clone());
                }
                // The theme/compatibility mode/UI language may have changed — refresh the
                // palette and locale of open overlay screens (list/self-model) in one broadcast.
                other => other.set_theme(
                    Palette::for_theme(config.interface.theme)
                        .with_compat(config.interface.terminal_compat),
                    crate::shared::i18n::locale(config.interface.language),
                ),
            }
            screen.set_settings(*config, profiles, language_locked, mcp, secrets_present);
        }
        // Always applied to the chat screen (like `ChatList`): the names must be
        // current whether or not the chat is the visible screen right now.
        AppEvent::CharacterNames(names) => screen.set_character_names(names),
        AppEvent::ChatActivated {
            id,
            title,
            messages,
            draft,
            focus,
        } => {
            // THE clearing funnel for the search back-stack. Every route that
            // opens a chat — picking one in the list, `Ctrl+N`, a clone, a jump
            // from a hit, restoring the last chat at startup — ends here, so
            // this is the one place that can honestly say the results are no
            // longer where the user came from. Enumerating the routes by hand
            // instead would rot silently the moment a new one is added.
            //
            // The test is "a *different* chat": a re-activation of the same one
            // (regeneration, deleting an exchange, a repeat jump) rebuilds the
            // feed without leaving the chat, and must keep the way back.
            if back.as_ref().is_some_and(|ret| ret.chat != id) {
                *back = None;
            }
            if let ActiveScreen::ChatList(list) = active {
                if list.take_pending_new_chat() {
                    // Activation of a just-created chat arrived (`Ctrl+N` in
                    // the list) — close the list and show the new chat. This makes
                    // the old→new transition atomic, with no intermediate flash.
                    *active = ActiveScreen::Chat;
                } else {
                    // Deleting the active chat while the list is open changes the active one —
                    // refresh its marker in the list.
                    list.set_active(Some(id));
                }
            }
            screen.activate_chat(id, title, &messages, &draft, focus);
        }
        AppEvent::UserMessage(text) => screen.push_user_message(text),
        AppEvent::RestoreInput(text) => screen.restore_input(text),
        AppEvent::GenerationStarted { generation_id } => screen.begin_generation(generation_id),
        AppEvent::Chunk {
            generation_id,
            text,
        } => screen.push_chunk(generation_id, &text),
        AppEvent::Thoughts {
            generation_id,
            text,
        } => screen.push_thoughts(generation_id, &text),
        AppEvent::TokenUsage {
            generation_id,
            completion,
            context,
            context_exact,
            reasoning,
        } => screen.set_token_usage(generation_id, completion, context, context_exact, reasoning),
        AppEvent::ToolConfirmRequest {
            generation_id,
            call_id,
            name,
            arguments,
        } => screen.request_tool_confirm(generation_id, call_id, name, arguments),
        AppEvent::ToolCall {
            generation_id,
            name,
            arguments,
            result,
        } => screen.push_tool_call(generation_id, name, arguments, result),
        AppEvent::AssistantContinue { generation_id } => screen.continue_assistant(generation_id),
        AppEvent::AssistantRewrite { generation_id } => screen.rewrite_assistant(generation_id),
        AppEvent::Finished {
            generation_id,
            reason,
        } => screen.finish_generation(generation_id, reason),
        AppEvent::ImpersonationStarted { generation_id } => {
            screen.begin_impersonation(generation_id)
        }
        AppEvent::ImpersonationChunk {
            generation_id,
            text,
        } => screen.push_impersonation_chunk(generation_id, &text),
        AppEvent::ImpersonationFinished {
            generation_id,
            reason,
        } => screen.finish_impersonation(generation_id, reason),
        AppEvent::RagProgress(progress) => screen.set_rag_progress(progress),
        AppEvent::FileProgress(progress) => screen.set_file_progress(progress),
        // Always applied to the chat screen (like `CharacterNames`): the chip
        // must be current whichever screen is visible right now.
        AppEvent::Attachments(items) => screen.set_attachments(items),
        // A reply to a self-model request/edit (`F3`): open the screen or refresh
        // the already-open one in place (keeping the selection — important during edits).
        // Deliberately exhaustive by variant rather than a `_` catch-all: this
        // arm *replaces* the active screen, so a new screen that forgot about
        // it would be silently stolen by an unrelated late event
        // (docs/history/chat-search-stage2.md §1.6). Every future variant has to say
        // whether it may be replaced.
        AppEvent::SelfModelView(model) => match active {
            ActiveScreen::SelfModel(view) => view.set_model(*model),
            // A results list the user is reading must not be swapped out from
            // under them by a stale reply to a request they have left behind.
            ActiveScreen::Search(_) => {}
            ActiveScreen::Chat | ActiveScreen::ChatList(_) | ActiveScreen::Settings(_) => {
                *active = ActiveScreen::SelfModel(Box::new(SelfModelScreen::new(
                    *model,
                    screen.palette(),
                    screen.loc(),
                )))
            }
        },
        // The "self-model" changed in the background/via tools — refresh ONLY the open
        // `F3` screen (re-request a fresh snapshot); ignored when closed.
        AppEvent::SelfModelChanged => {
            if matches!(active, ActiveScreen::SelfModel(_)) {
                let _ = cmd_tx.send(AppCommand::RequestSelfModel);
            }
        }
        AppEvent::TtsActive(on) => screen.set_speaking(on),
        AppEvent::BackgroundTask { kind, active: on } => match kind {
            BackgroundKind::Reflection => screen.set_reflecting(on),
            BackgroundKind::Consolidation => screen.set_consolidating(on),
            BackgroundKind::SelfConsolidation => screen.set_self_consolidating(on),
        },
        // The import runs from the settings screen and its outcome is shown
        // there, on the import row itself — a note in the feed would sit behind
        // the screen the user is standing on.
        AppEvent::McpImportResult(text) => {
            if let ActiveScreen::Settings(settings) = active {
                settings.set_mcp_import_result(text);
            }
        }
        AppEvent::Error(message) => screen.push_error(&message),
    }
}

/// Writes the conversation to the clipboard and routes the confirmation/error
/// into the chat-list screen's status (if it was opened for copying), or, if it's
/// closed, as a note in the feed. Encapsulates the single call to `arboard`
/// ([`write_clipboard`]) so `apply_event` doesn't need to know about the clipboard.
pub(super) fn deliver_clipboard(
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
    clipboard: &mut Option<arboard::Clipboard>,
    text: &str,
) {
    let result = write_clipboard(clipboard, text);
    let loc = screen.loc();
    match active {
        ActiveScreen::ChatList(list) => match result {
            Ok(()) => list.set_notice(loc.t("ui.chat.copied").into()),
            Err(err) => list.set_error(loc.tf("ui.err.copy_failed", &[("err", &err)])),
        },
        _ => match result {
            Ok(()) => screen.push_note(loc.t("ui.chat.copied")),
            Err(err) => screen.push_error(&loc.tf("ui.err.copy_failed", &[("err", &err)])),
        },
    }
}

/// An intent from any of the screens — a unified type, so an intent taken off the
/// active screen can be dispatched through a single ownership (instead of 4 parallel
/// `Option`s and 4 nearly identical `if` blocks that conflicted over borrows).
pub(super) enum AnyIntent {
    Chat(ChatIntent),
    List(ChatListIntent),
    Settings(SettingsIntent),
    SelfModel(SelfModelIntent),
    Search(SearchIntent),
}

/// Dispatches the active screen's intent to the corresponding translator.
/// Returns `true` if quitting was requested.
pub(super) fn dispatch_any(
    intent: AnyIntent,
    cmd_tx: &UnboundedSender<AppCommand>,
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
    back: &mut Option<SearchReturn>,
) -> bool {
    match intent {
        AnyIntent::Chat(i) => dispatch(i, cmd_tx, screen, active, back),
        AnyIntent::List(i) => dispatch_chat_list(i, cmd_tx, screen, active),
        AnyIntent::Settings(i) => dispatch_settings(i, cmd_tx, active),
        AnyIntent::SelfModel(i) => dispatch_self_model(i, cmd_tx, active),
        AnyIntent::Search(i) => dispatch_search(i, cmd_tx, screen, active, back),
    }
}

/// Translates a chat intent into an orchestrator command (or opens the settings/
/// chat-list screen). Returns `true` for [`ChatIntent::Quit`].
pub(super) fn dispatch(
    intent: ChatIntent,
    cmd_tx: &UnboundedSender<AppCommand>,
    screen: &ChatScreen,
    active: &mut ActiveScreen,
    back: &mut Option<SearchReturn>,
) -> bool {
    let command = match intent {
        ChatIntent::Quit => return true,
        ChatIntent::Send(text) => AppCommand::SendMessage(text),
        ChatIntent::RegenerateLast => AppCommand::RegenerateLast,
        ChatIntent::DeleteLastExchange => AppCommand::DeleteLastExchange,
        ChatIntent::Cancel => AppCommand::Cancel,
        ChatIntent::Impersonate { seed } => AppCommand::Impersonate { seed },
        ChatIntent::CancelImpersonation => AppCommand::CancelImpersonation,
        ChatIntent::NewChat { profile_id } => AppCommand::NewChat { profile_id },
        ChatIntent::CopyChat(id) => AppCommand::CopyChat(id),
        ChatIntent::RagAdd { path, recursive } => AppCommand::RagAdd { path, recursive },
        ChatIntent::RagDelete { path } => AppCommand::RagDelete { path },
        ChatIntent::RagList => AppCommand::RagList,
        ChatIntent::RagRebuild => AppCommand::RagRebuild,
        ChatIntent::Reindex => AppCommand::Reindex,
        ChatIntent::FileAttach { path } => AppCommand::FileAttach { path },
        ChatIntent::FileRemove { target } => AppCommand::FileRemove { target },
        ChatIntent::FileList => AppCommand::FileList,
        ChatIntent::Tts(scope) => AppCommand::Tts(scope),
        ChatIntent::TtsStop => AppCommand::TtsStop,
        ChatIntent::TtsPause => AppCommand::TtsPause,
        ChatIntent::TtsResume => AppCommand::TtsResume,
        ChatIntent::OpenSettings => {
            if let Some((config, profiles, language_locked, mcp, secrets)) =
                screen.settings_snapshot()
            {
                let mut settings = SettingsScreen::new(config, profiles, language_locked);
                // The initial server-status snapshot (chips in the "Model/server" section);
                // afterward `apply_event` updates them from the `ServerStatus` event.
                settings.set_server_statuses(screen.server_statuses());
                // The MCP-host snapshot (the tool catalog + server statuses);
                // afterward `apply_event` updates it from the `Settings` event.
                settings.set_mcp(mcp);
                // Which secrets are stored on this machine (every secret field's status).
                settings.set_secrets_present(secrets);
                *active = ActiveScreen::Settings(Box::new(settings));
            }
            return false;
        }
        // `Esc` in the chat means "go back", and the chat screen deliberately
        // cannot know where back is (FSD). When the chat was reached by opening
        // a search hit, one step back is the **results** — the user drilled down
        // from them and is most likely working through the hits, so dropping
        // them into the chat list would throw the whole list away. The stashed
        // screen is restored whole, selection and scroll included, and is
        // consumed: the next `Esc`, now from the results, goes on to the list as
        // it always did.
        ChatIntent::OpenChatList if back.is_some() => {
            if let Some(ret) = back.take() {
                *active = ActiveScreen::Search(ret.screen);
            }
            return false;
        }
        // Otherwise the chat list opens from a snapshot the chat keeps up to date.
        ChatIntent::OpenChatList => {
            *active = ActiveScreen::ChatList(Box::new(ChatListScreen::new(
                screen.chat_summaries(),
                screen.active_chat(),
                screen.palette(),
                screen.loc(),
            )));
            return false;
        }
        // Viewing the self-model: the orchestrator owns the data — request a snapshot,
        // the screen opens on the `SelfModelView` event (see `apply_event`).
        ChatIntent::OpenSelfModel => {
            let _ = cmd_tx.send(AppCommand::RequestSelfModel);
            return false;
        }
        // The wheel-scroll toggle: enable/disable terminal mouse capture.
        // This is a purely terminal side effect (FSD: the screen doesn't know about
        // the terminal, it just reports the desired state). With capture on, text
        // selection is available with Shift held.
        ChatIntent::SetMouseCapture(on) => {
            let _ = if on {
                execute!(stdout(), EnableMouseCapture)
            } else {
                execute!(stdout(), DisableMouseCapture)
            };
            return false;
        }
        // Copying to the clipboard is intercepted earlier — in `process_input_batch`
        // (which has the `arboard` slot, whereas `screen` here is immutable). It never reaches here.
        ChatIntent::ConfirmTool {
            generation_id,
            call_id,
            decision,
        } => AppCommand::ConfirmTool {
            generation_id,
            call_id,
            decision,
        },
        ChatIntent::CopyToClipboard(_) => return false,
    };
    let _ = cmd_tx.send(command);
    false
}

/// Translates a chat-list-screen intent into a command (or screen management).
/// Returns `true` for [`ChatListIntent::Quit`] (the loop ends).
pub(super) fn dispatch_chat_list(
    intent: ChatListIntent,
    cmd_tx: &UnboundedSender<AppCommand>,
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
) -> bool {
    let command = match intent {
        // Closing/switching to a chat returns to the base screen.
        ChatListIntent::Close => {
            *active = ActiveScreen::Chat;
            return false;
        }
        ChatListIntent::Quit => return true,
        ChatListIntent::Switch(id) => {
            *active = ActiveScreen::Chat;
            AppCommand::SwitchChat(id)
        }
        // Creating a chat starts the new-chat flow on the chat screen (that's where
        // profile selection lives — an overlay when there's >1 profile).
        ChatListIntent::NewChat => {
            match screen.request_new_chat() {
                // A single profile: the orchestrator creates the chat (round-trip). The list
                // is KEPT open until the new chat's `ChatActivated` arrives —
                // otherwise the previous active chat would flash for the duration of the round-trip. Once
                // the activation arrives, `apply_event` switches to the new chat.
                Some(ChatIntent::NewChat { profile_id }) => {
                    if let ActiveScreen::ChatList(list) = active {
                        list.set_pending_new_chat();
                    }
                    let _ = cmd_tx.send(AppCommand::NewChat { profile_id });
                }
                // >1 profile: `request_new_chat` opened the profile-picker overlay on
                // the chat screen — show the chat so the overlay is visible.
                _ => *active = ActiveScreen::Chat,
            }
            return false;
        }
        ChatListIntent::Clone(id) => {
            *active = ActiveScreen::Chat;
            AppCommand::CloneChat(id)
        }
        // Copy/delete/rename/auto-title don't close the list:
        // the confirmation/error arrives in its status area, the updated set —
        // via the `ChatList` event.
        ChatListIntent::Copy(id) => AppCommand::CopyChat(id),
        ChatListIntent::Delete(id) => AppCommand::DeleteChat(id),
        ChatListIntent::Rename { id, title } => AppCommand::RenameChat { id, title },
        ChatListIntent::AutoRename(id) => AppCommand::AutoRenameChat(id),
        // Content search: the raw query goes to the orchestrator, which escapes
        // it and answers with `ChatSearchResults`. The list stays open.
        ChatListIntent::SearchContent(query) => AppCommand::SearchChats(query),
        // The message-level screen opens on the reply (`MessageSearchResults`),
        // not here — the orchestrator owns the index, so this is a round-trip
        // like `RequestSelfModel`. The list stays on screen meanwhile.
        ChatListIntent::SearchMessages { query, sort } => {
            AppCommand::SearchMessages { query, sort }
        }
        // Opening a chat at its first match closes the list, exactly as a plain
        // `Switch` does.
        ChatListIntent::OpenFirstMatch { chat, query } => {
            *active = ActiveScreen::Chat;
            AppCommand::OpenChatAtFirstMatch { chat, query }
        }
    };
    let _ = cmd_tx.send(command);
    false
}

/// Translates a settings-screen intent into a command (or closes it).
/// Returns `true` for [`SettingsIntent::Quit`] (the loop ends).
pub(super) fn dispatch_settings(
    intent: SettingsIntent,
    cmd_tx: &UnboundedSender<AppCommand>,
    active: &mut ActiveScreen,
) -> bool {
    let command = match intent {
        SettingsIntent::Close => {
            *active = ActiveScreen::Chat;
            return false;
        }
        SettingsIntent::Quit => return true,
        SettingsIntent::SaveConfig(config) => AppCommand::UpdateConfig(config),
        SettingsIntent::SaveProfile { id, edit } => AppCommand::UpdateProfile { id, edit },
        SettingsIntent::CreateProfile {
            name,
            system_message,
        } => AppCommand::CreateProfile {
            name,
            system_message,
        },
        SettingsIntent::DeleteProfile(id) => AppCommand::DeleteProfile(id),
        SettingsIntent::ConfirmMcpCatalog(server) => AppCommand::ConfirmMcpCatalog(server),
        SettingsIntent::ReconnectMcpServer(server) => AppCommand::ReconnectMcpServer(server),
        SettingsIntent::SetSecret { key, value } => AppCommand::SetSecret { key, value },
        SettingsIntent::ImportMcpServers(path) => AppCommand::ImportMcpServers(path),
    };
    let _ = cmd_tx.send(command);
    false
}

/// Translates a message-level search intent. Returns `true` for
/// [`SearchIntent::Quit`] (the loop ends).
///
/// `Close` goes back to the **chat list still searching for the same query**
/// rather than to a blank one: the user got here from a content search, and
/// dropping them into an empty title-mode list would throw that away. The list
/// is rebuilt from the chat screen's snapshot (as `OpenChatList` does) and the
/// content results are refetched by the same round-trip typing a query uses.
pub(super) fn dispatch_search(
    intent: SearchIntent,
    cmd_tx: &UnboundedSender<AppCommand>,
    screen: &ChatScreen,
    active: &mut ActiveScreen,
    back: &mut Option<SearchReturn>,
) -> bool {
    match intent {
        SearchIntent::Quit => true,
        SearchIntent::Close => {
            // Leaving the results for the chat list — there is nothing to come
            // back to any more. Normally the stash is already empty here (it is
            // taken when the results are restored), but a late
            // `MessageSearchResults` can open a fresh screen over a chat that
            // still holds one.
            *back = None;
            let query = match active {
                ActiveScreen::Search(search) => search.query().to_string(),
                _ => String::new(),
            };
            let mut list = ChatListScreen::new(
                screen.chat_summaries(),
                screen.active_chat(),
                screen.palette(),
                screen.loc(),
            );
            list.restore_content_query(query.clone());
            *active = ActiveScreen::ChatList(Box::new(list));
            let _ = cmd_tx.send(AppCommand::SearchChats(query));
            false
        }
        // The jump itself is stage 2a's; here it only leaves the results, the
        // way `ChatListIntent::Switch` leaves the chat list — except that the
        // screen is **stashed** rather than dropped, so `Esc` in the chat can
        // come back to these exact hits (see [`SearchReturn`]).
        SearchIntent::OpenHit {
            chat,
            message,
            query,
        } => {
            if let ActiveScreen::Search(results) = std::mem::replace(active, ActiveScreen::Chat) {
                *back = Some(SearchReturn {
                    screen: results,
                    chat,
                });
            }
            let _ = cmd_tx.send(AppCommand::OpenChatAt {
                chat,
                message,
                query,
            });
            false
        }
    }
}

/// Translates a self-model-viewer-screen intent: closing returns to the chat,
/// `Quit` ends the loop (`true`). Sends no orchestrator commands (read-only).
pub(super) fn dispatch_self_model(
    intent: SelfModelIntent,
    cmd_tx: &UnboundedSender<AppCommand>,
    active: &mut ActiveScreen,
) -> bool {
    match intent {
        SelfModelIntent::Close => {
            *active = ActiveScreen::Chat;
            false
        }
        SelfModelIntent::Quit => true,
        // An edit: a command to the orchestrator; the screen stays open and refreshes on
        // the reply `SelfModelView` (see `apply_event`).
        SelfModelIntent::Edit(edit) => {
            let _ = cmd_tx.send(AppCommand::UpdateSelfModel(edit));
            false
        }
    }
}
