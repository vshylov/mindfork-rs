//! Runtime — applies AppEvent to the screen + translates Intent → AppCommand. Part of the [`super`] module, split out of the
//! runtime.rs monolith (see docs/history/refactoring-god-objects.md, stage 7).

use super::*;

/// Applies an orchestrator event to the chat screen (a read-only projection).
/// A settings snapshot, when the settings screen is open, additionally
/// refreshes its working copy (reflects profile creation/deletion).
pub(super) fn apply_event(
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
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
            api_keys_present,
        } => {
            match active {
                ActiveScreen::Settings(settings) => {
                    settings.refresh((*config).clone(), profiles.clone(), language_locked.clone());
                    settings.set_mcp(mcp.clone());
                    settings.set_api_keys_present(api_keys_present.clone());
                }
                // The theme/compatibility mode/UI language may have changed — refresh the
                // palette and locale of open overlay screens (list/self-model) in one broadcast.
                other => other.set_theme(
                    Palette::for_theme(config.interface.theme)
                        .with_compat(config.interface.terminal_compat),
                    crate::shared::i18n::locale(config.interface.language),
                ),
            }
            screen.set_settings(*config, profiles, language_locked, mcp, api_keys_present);
        }
        AppEvent::ChatActivated {
            id,
            title,
            messages,
            draft,
        } => {
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
            screen.activate_chat(id, title, &messages, &draft);
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
        // A reply to a self-model request/edit (`F3`): open the screen or refresh
        // the already-open one in place (keeping the selection — important during edits).
        AppEvent::SelfModelView(model) => match active {
            ActiveScreen::SelfModel(view) => view.set_model(*model),
            _ => {
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
}

/// Dispatches the active screen's intent to the corresponding translator.
/// Returns `true` if quitting was requested.
pub(super) fn dispatch_any(
    intent: AnyIntent,
    cmd_tx: &UnboundedSender<AppCommand>,
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
) -> bool {
    match intent {
        AnyIntent::Chat(i) => dispatch(i, cmd_tx, screen, active),
        AnyIntent::List(i) => dispatch_chat_list(i, cmd_tx, screen, active),
        AnyIntent::Settings(i) => dispatch_settings(i, cmd_tx, active),
        AnyIntent::SelfModel(i) => dispatch_self_model(i, cmd_tx, active),
    }
}

/// Translates a chat intent into an orchestrator command (or opens the settings/
/// chat-list screen). Returns `true` for [`ChatIntent::Quit`].
pub(super) fn dispatch(
    intent: ChatIntent,
    cmd_tx: &UnboundedSender<AppCommand>,
    screen: &ChatScreen,
    active: &mut ActiveScreen,
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
        ChatIntent::Tts(scope) => AppCommand::Tts(scope),
        ChatIntent::TtsStop => AppCommand::TtsStop,
        ChatIntent::TtsPause => AppCommand::TtsPause,
        ChatIntent::TtsResume => AppCommand::TtsResume,
        ChatIntent::OpenSettings => {
            if let Some((config, profiles, language_locked, mcp, api_keys)) =
                screen.settings_snapshot()
            {
                let mut settings = SettingsScreen::new(config, profiles, language_locked);
                // The initial server-status snapshot (chips in the "Model/server" section);
                // afterward `apply_event` updates them from the `ServerStatus` event.
                settings.set_server_statuses(screen.server_statuses());
                // The MCP-host snapshot (the tool catalog + server statuses);
                // afterward `apply_event` updates it from the `Settings` event.
                settings.set_mcp(mcp);
                // Which keys are stored on this machine (the "API key" status field).
                settings.set_api_keys_present(api_keys);
                *active = ActiveScreen::Settings(Box::new(settings));
            }
            return false;
        }
        // The chat list opens from a snapshot the chat keeps up to date.
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
        SettingsIntent::SetApiKey { provider, key } => AppCommand::SetApiKey { provider, key },
    };
    let _ = cmd_tx.send(command);
    false
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
