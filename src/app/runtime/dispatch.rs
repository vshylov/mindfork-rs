//! Runtime — applies AppEvent to the screen + translates Intent → AppCommand. Part of the [`super`] module, split out of the
//! runtime.rs monolith (see docs/history/refactoring-god-objects.md, stage 7).

use super::*;

use crate::entities::chat::ChatSummary;
use crate::entities::profile::{CharacterNames, Profile};
use crate::entities::self_model::SelfModel;
use crate::features::chat_search::SearchGroup;
use crate::features::chat_search_sort::SortMode;
use crate::features::tools::mcp::McpSnapshot;
use crate::shared::config::AppConfig;
use crate::shared::secrets::SecretKey;
use crate::shared::server::ServerStatuses;

/// Applies an orchestrator event to the chat screen (a read-only projection).
/// A settings snapshot, when the settings screen is open, additionally
/// refreshes its working copy (reflects profile creation/deletion).
///
/// `back` is the search-results back-stack ([`Back`]): the
/// `ChatActivated` arm is where it gets dropped, because that event is the one
/// funnel every chat-opening route ends in.
pub(super) fn apply_event(
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
    back: &mut Option<Back>,
    clipboard: &mut Option<arboard::Clipboard>,
    cmd_tx: &UnboundedSender<AppCommand>,
    event: AppEvent,
) {
    match event {
        // Server statuses — into the chat (the status line) and, if open, into the
        // settings screen (chips in the "Model/server" section: connecting → ready is visible in place).
        AppEvent::ServerStatus(status) => apply_server_status(screen, active, status),
        // The list snapshot is applied to the chat always (for the next open/`Ctrl+N`),
        // and when the list screen is open — to it too (a live update).
        AppEvent::ChatList(chats) => apply_chat_list(screen, active, chats),
        AppEvent::ChatRenamed { id, title } => screen.rename_chat(id, title),
        // Content-search results only mean anything to an open chat list — the
        // reply to a query it asked for. Ignored when it is closed (a late reply
        // to a list the user has since left).
        AppEvent::ChatSearchResults { query, chat_ids } => {
            apply_content_search_results(active, query, chat_ids)
        }
        // Message-level results (`Ctrl+G`): the screen opens on the reply, not
        // on the key press — the same round-trip as `SelfModelView`, since the
        // orchestrator owns the index. A later reply refreshes the screen in
        // place instead of stacking a second one.
        AppEvent::MessageSearchResults {
            query,
            groups,
            total,
        } => show_message_search_results(screen, active, query, groups, total),
        // A list-operation error: into its status area, if the screen is open; otherwise
        // (a late auto-title reply after the list is closed) — as a note in the feed.
        AppEvent::ChatListError(message) => report_chat_list_error(screen, active, message),
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
            refresh_settings_screens(
                active,
                &config,
                &profiles,
                &language_locked,
                &mcp,
                &secrets_present,
            );
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
            feed_view,
            focus,
            compaction,
            child,
            live_turn,
        } => {
            clear_back_if_left(back, id);
            close_or_mark_chat_list(active, id);
            // The child view goes first: `activate_chat` reads it while
            // building the feed (the persona bubble, the transcript's message
            // copy), so setting it after would build every feed with the
            // previous chat's view — a transcript opening without its persona,
            // and the chat opened next inheriting one that is not its own.
            screen.set_child_view(child);
            screen.activate_chat(id, title, &messages, &draft, feed_view, focus, compaction);
            screen.set_live_turn(live_turn);
        }
        AppEvent::TranscriptGrew { id, messages } => screen.grow_transcript(id, &messages),
        AppEvent::TranscriptLine {
            generation_id,
            role,
        } => screen.set_transcript_line(generation_id, role),
        AppEvent::TranscriptReset { id, messages } => screen.reset_transcript(id, &messages),
        AppEvent::EngineModel(model) => screen.set_engine_model(model),
        AppEvent::EngineSlots(slots) => apply_engine_slots(screen, active, slots),
        AppEvent::UserMessage(text) => screen.push_user_message(text),
        AppEvent::RestoreInput(text) => screen.restore_input(text),
        AppEvent::GenerationStarted {
            generation_id,
            model,
            continuation,
        } => apply_generation_started(screen, generation_id, model, continuation),
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
        AppEvent::ToolCallStarted {
            generation_id,
            call_id,
            name,
            arguments,
        } => screen.push_tool_call_started(generation_id, call_id, name, arguments),
        AppEvent::ToolCall {
            generation_id,
            call_id,
            name,
            arguments,
            result,
            images,
        } => screen.push_tool_call(generation_id, call_id, name, arguments, result, images),
        AppEvent::AssistantContinue { generation_id } => screen.continue_assistant(generation_id),
        AppEvent::AssistantRewrite { generation_id } => screen.rewrite_assistant(generation_id),
        AppEvent::Finished {
            generation_id,
            reason,
            continuable,
        } => screen.finish_generation(generation_id, reason, continuable),
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
        AppEvent::ProjectProgress(progress) => screen.set_project_progress(progress),
        // Always applied to the chat screen (like `CharacterNames`): the chip
        // must be current whichever screen is visible right now.
        AppEvent::Attachments(items) => screen.set_attachments(items),
        AppEvent::ImageProgress(progress) => screen.set_image_progress(progress),
        // Same reasoning as the attachments chip: the staged-image cost has to be
        // current whichever screen happens to be visible.
        AppEvent::StagedImages(items) => screen.set_staged_images(items),
        // A reply to a self-model request/edit (`F3`): open the screen or refresh
        // the already-open one in place (keeping the selection — important during edits).
        AppEvent::SelfModelView { model, names } => show_self_model(screen, active, *model, names),
        AppEvent::WorkspaceChanges(set) => show_changes(screen, active, *set),
        // The "self-model" changed in the background/via tools — refresh ONLY the open
        // `F3` screen (re-request a fresh snapshot); ignored when closed.
        AppEvent::SelfModelChanged => refresh_self_model_screen(active, cmd_tx),
        AppEvent::TtsActive(on) => screen.set_speaking(on),
        AppEvent::BackgroundTask { kind, active: on } => apply_background_task(screen, kind, on),
        AppEvent::BackgroundRuns { out } => screen.set_background_runs(out),
        // The import runs from the settings screen and its outcome is shown
        // there, on the import row itself — a note in the feed would sit behind
        // the screen the user is standing on.
        AppEvent::McpImportResult(text) => report_mcp_import_result(active, text),
        AppEvent::Retrying {
            generation_id,
            attempt,
            max,
            delay_secs,
        } => screen.set_retrying(generation_id, attempt, max, delay_secs),
        AppEvent::SubagentProgress {
            generation_id,
            run,
            progress,
        } => screen.set_subagent_progress(generation_id, run, progress),
        AppEvent::Error(message) => screen.push_error(&message),
        AppEvent::Notice(message) => screen.push_note(&message),
        // A roll can finish for a chat the user has since switched away from.
        // Applying it would clear the open chat's own boundary and post the note
        // in the wrong conversation — the same staleness the `generation_id`
        // guards close for streaming events.
        AppEvent::Compacted {
            chat_id,
            boundary,
            summary,
            folded,
        } => apply_compacted(screen, chat_id, boundary, summary, folded),
    }
}

/// The `ServerStatus` arm of [`apply_event`]: into the chat's status line, and
/// into an open settings screen's chips too, so connecting → ready is visible
/// in place.
fn apply_server_status(screen: &mut ChatScreen, active: &mut ActiveScreen, status: ServerStatuses) {
    if let ActiveScreen::Settings(settings) = active {
        settings.set_server_statuses(status.clone());
    }
    screen.set_server_status(status);
}

/// The `EngineSlots` arm of [`apply_event`]: the chat keeps the engine's answer
/// for the next settings screen it builds, and an open one shows it at once
/// (the hint next to the `sessions` field, spec §11.6).
fn apply_engine_slots(screen: &mut ChatScreen, active: &mut ActiveScreen, slots: Option<u32>) {
    if let ActiveScreen::Settings(settings) = active {
        settings.set_engine_slots(slots);
    }
    screen.set_engine_slots(slots);
}

/// The `ChatList` arm of [`apply_event`]: the snapshot goes to the chat always
/// (for the next open/`Ctrl+N`), and to an open list screen as a live update.
fn apply_chat_list(screen: &mut ChatScreen, active: &mut ActiveScreen, chats: Vec<ChatSummary>) {
    if let ActiveScreen::ChatList(list) = active {
        list.set_chats(chats.clone());
    }
    screen.set_chat_list(chats);
}

/// The `ChatSearchResults` arm of [`apply_event`]: the reply to a query an open
/// chat list asked for; ignored when the list has since been closed.
fn apply_content_search_results(
    active: &mut ActiveScreen,
    query: String,
    chat_ids: Option<Vec<Uuid>>,
) {
    if let ActiveScreen::ChatList(list) = active {
        list.set_search_results(query, chat_ids);
    }
}

/// The `GenerationStarted` arm of [`apply_event`]: a continuation re-opens the
/// trailing assistant bubble for streaming (`/continue`, spec §6.4); an
/// ordinary turn begins a fresh one.
fn apply_generation_started(
    screen: &mut ChatScreen,
    generation_id: Uuid,
    model: Option<String>,
    continuation: bool,
) {
    if continuation {
        screen.begin_continuation(generation_id, model);
    } else {
        screen.begin_generation(generation_id, model);
    }
}

/// The `MessageSearchResults` arm of [`apply_event`]: refreshes an open results
/// screen in place, or opens a new one on the reply.
fn show_message_search_results(
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
    query: String,
    groups: Vec<SearchGroup>,
    total: usize,
) {
    match active {
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
    }
}

/// The `ChatListError` arm of [`apply_event`]: routes a list-operation error
/// into the open list's status area, or as a note in the feed.
fn report_chat_list_error(screen: &mut ChatScreen, active: &mut ActiveScreen, message: String) {
    match active {
        ActiveScreen::ChatList(list) => list.set_error(message),
        _ => screen.push_error(&message),
    }
}

/// The screen-refresh half of the `Settings` arm of [`apply_event`]: an open
/// settings screen gets the fresh working copy; other overlay screens get the
/// theme/locale broadcast. The chat's own snapshot is updated by the caller
/// (`set_settings` consumes the event's values).
fn refresh_settings_screens(
    active: &mut ActiveScreen,
    config: &AppConfig,
    profiles: &[Profile],
    language_locked: &[Uuid],
    mcp: &McpSnapshot,
    secrets_present: &[SecretKey],
) {
    match active {
        ActiveScreen::Settings(settings) => {
            settings.refresh(config.clone(), profiles.to_vec(), language_locked.to_vec());
            settings.set_mcp(mcp.clone());
            settings.set_secrets_present(secrets_present.to_vec());
        }
        // The theme/compatibility mode/UI language may have changed — refresh the
        // palette and locale of open overlay screens (list/self-model) in one broadcast.
        other => other.set_theme(
            Palette::for_theme(config.interface.theme)
                .with_compat(config.interface.terminal_compat),
            crate::shared::i18n::locale(config.interface.language),
        ),
    }
}

/// Drops the back-stack when a chat activation means the user left the chat it
/// leads out of (see [`Back`]).
fn clear_back_if_left(back: &mut Option<Back>, id: Uuid) {
    // THE clearing funnel for the back-stack, both kinds. Every route that
    // opens a chat — picking one in the list, `Ctrl+N`, a clone, a jump
    // from a hit, following a `chat://` reference, restoring the last chat
    // at startup — ends here, so this is the one place that can honestly
    // say the stash is no longer where the user came from. Enumerating the
    // routes by hand instead would rot silently the moment a new one is
    // added.
    //
    // The test is "a *different* chat": a re-activation of the same one
    // (regeneration, deleting an exchange, a repeat jump) rebuilds the
    // feed without leaving the chat, and must keep the way back.
    //
    // Following a reference is the one route that **writes** the stash on
    // its way through: it is set in `dispatch` before the command is sent,
    // and the activation that follows names the chat it points out of, so
    // this leaves it alone. Order is what makes that true, and it is the
    // reason the push cannot move into `apply_event`.
    if back.as_ref().is_some_and(|ret| ret.chat() != id) {
        *back = None;
    }
}

/// The open chat list's reaction to a chat activation: close it for a
/// just-created chat (`Ctrl+N` in the list), otherwise refresh its
/// active-chat marker.
fn close_or_mark_chat_list(active: &mut ActiveScreen, id: Uuid) {
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
}

/// The `SelfModelView` arm of [`apply_event`]: opens the `F3` screen or
/// refreshes the already-open one in place.
fn show_self_model(
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
    model: Option<SelfModel>,
    names: CharacterNames,
) {
    // Deliberately exhaustive by variant rather than a `_` catch-all: this
    // match *replaces* the active screen, so a new screen that forgot about
    // it would be silently stolen by an unrelated late event
    // (docs/history/chat-search-stage2.md §1.6). Every future variant has to say
    // whether it may be replaced.
    match active {
        ActiveScreen::SelfModel(view) => view.set_model(model, names),
        // A results list the user is reading must not be swapped out from
        // under them by a stale reply to a request they have left behind — and
        // neither must a diff the user is reading.
        ActiveScreen::Search(_) | ActiveScreen::Changes(_) => {}
        ActiveScreen::Chat | ActiveScreen::ChatList(_) | ActiveScreen::Settings(_) => {
            *active = ActiveScreen::SelfModel(Box::new(SelfModelScreen::new(
                model,
                names,
                screen.palette(),
                screen.loc(),
                screen.self_model_note_order(),
            )))
        }
    }
}

/// The `SelfModelChanged` arm of [`apply_event`]: the "self-model" changed in
/// the background/via tools — an open `F3` screen re-requests a fresh
/// snapshot; ignored when closed.
fn refresh_self_model_screen(active: &ActiveScreen, cmd_tx: &UnboundedSender<AppCommand>) {
    if matches!(active, ActiveScreen::SelfModel(_)) {
        let _ = cmd_tx.send(AppCommand::RequestSelfModel);
    }
}

/// The `McpImportResult` arm of [`apply_event`]: shown on an open settings
/// screen's import row; dropped otherwise (a feed note would sit behind the
/// screen the user is standing on).
fn report_mcp_import_result(active: &mut ActiveScreen, text: String) {
    if let ActiveScreen::Settings(settings) = active {
        settings.set_mcp_import_result(text);
    }
}

/// The `Compacted` arm of [`apply_event`]: applies a finished roll to the chat
/// still showing the conversation it rolled; a late one for a chat the user has
/// switched away from is dropped.
fn apply_compacted(
    screen: &mut ChatScreen,
    chat_id: Uuid,
    boundary: Uuid,
    summary: String,
    folded: usize,
) {
    if screen.active_chat() == Some(chat_id) {
        let note = screen
            .loc()
            .tf("ui.compact.done", &[("count", &folded.to_string())]);
        screen.set_compaction(Some((boundary, summary)));
        screen.push_note(&note);
    }
}

/// The `WorkspaceChanges` arm of [`apply_event`]: opens the `F4` screen, or
/// refreshes the already-open one after a revert.
///
/// Exhaustive by variant for the reason `show_self_model` is: this match
/// *replaces* the active screen, and a new screen that forgot about it would be
/// silently stolen by an unrelated late event.
fn show_changes(
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
    set: crate::features::workspace_diff::ChangeSet,
) {
    match active {
        ActiveScreen::Changes(view) => view.set_changes(set),
        // A results list the user is reading must not be swapped out from under
        // them by a stale reply to a request they have left behind.
        ActiveScreen::Search(_) => {}
        ActiveScreen::Chat
        | ActiveScreen::ChatList(_)
        | ActiveScreen::Settings(_)
        | ActiveScreen::SelfModel(_) => {
            *active = ActiveScreen::Changes(Box::new(ChangesScreen::new(
                set,
                screen.palette(),
                screen.loc(),
            )))
        }
    }
}

/// Translates a changes-screen intent: closing returns to the chat, `Quit` ends
/// the loop (`true`), a revert goes to the orchestrator — which does the work
/// and answers with a fresh change set, so the screen shows the result rather
/// than what it asked for.
pub(super) fn dispatch_changes(
    intent: ChangesIntent,
    cmd_tx: &UnboundedSender<AppCommand>,
    active: &mut ActiveScreen,
) -> bool {
    match intent {
        ChangesIntent::Close => {
            *active = ActiveScreen::Chat;
            false
        }
        ChangesIntent::Quit => true,
        ChangesIntent::Revert(path) => {
            let _ = cmd_tx.send(AppCommand::RevertWorkspaceFile { path });
            false
        }
    }
}

/// The `BackgroundTask` arm of [`apply_event`]: routes the activity flag to the
/// matching status-bar indicator.
fn apply_background_task(screen: &mut ChatScreen, kind: BackgroundKind, on: bool) {
    match kind {
        BackgroundKind::Reflection => screen.set_reflecting(on),
        BackgroundKind::Consolidation => screen.set_consolidating(on),
        BackgroundKind::SelfConsolidation => screen.set_self_consolidating(on),
        BackgroundKind::Compaction => screen.set_compacting(on),
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
    let loc = screen.loc();
    let (message, failed) = copy_text(clipboard, text, screen.clipboard_osc52()).message(loc);
    match active {
        ActiveScreen::ChatList(list) if failed => list.set_error(message),
        ActiveScreen::ChatList(list) => list.set_notice(message),
        _ if failed => screen.push_error(&message),
        _ => screen.push_note(&message),
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
    Changes(ChangesIntent),
    Search(SearchIntent),
}

/// Dispatches the active screen's intent to the corresponding translator.
/// Returns `true` if quitting was requested.
pub(super) fn dispatch_any(
    intent: AnyIntent,
    cmd_tx: &UnboundedSender<AppCommand>,
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
    back: &mut Option<Back>,
) -> bool {
    match intent {
        AnyIntent::Chat(i) => dispatch(i, cmd_tx, screen, active, back),
        AnyIntent::List(i) => dispatch_chat_list(i, cmd_tx, screen, active),
        AnyIntent::Settings(i) => dispatch_settings(i, cmd_tx, active),
        AnyIntent::SelfModel(i) => dispatch_self_model(i, cmd_tx, active),
        AnyIntent::Changes(i) => dispatch_changes(i, cmd_tx, active),
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
    back: &mut Option<Back>,
) -> bool {
    let command = match intent {
        ChatIntent::Quit => return true,
        ChatIntent::Send(text) => AppCommand::SendMessage(text),
        ChatIntent::RegenerateLast => AppCommand::RegenerateLast,
        ChatIntent::ContinueLast => AppCommand::ContinueLast,
        ChatIntent::DeleteLastExchange => AppCommand::DeleteLastExchange,
        ChatIntent::Cancel => AppCommand::Cancel,
        ChatIntent::Impersonate { seed } => AppCommand::Impersonate { seed },
        ChatIntent::CancelImpersonation => AppCommand::CancelImpersonation,
        ChatIntent::NewChat { profile_id } => AppCommand::NewChat { profile_id },
        ChatIntent::CopyChat(id) => AppCommand::CopyChat(id),
        ChatIntent::RenameChat { id, title } => AppCommand::RenameChat { id, title },
        // The chat list's `Ctrl+R` for the open chat; the result comes back as
        // `ChatRenamed`, a failure as `ChatListError` (which falls back to a
        // feed note when the list is closed). See spec §11.2.
        ChatIntent::AutoTitleChat(id) => AppCommand::AutoRenameChat(id),
        ChatIntent::CloneChat(id) => AppCommand::CloneChat(id),
        // `/subagents` — the chat screen's route to the fold the list's
        // `Ctrl+O` toggles; the orchestrator resolves a transcript's id to its
        // parent (spec §11.2).
        ChatIntent::SetChildrenExpanded { id, expanded } => {
            AppCommand::SetChildrenExpanded { id, expanded }
        }
        // `/subagents stop` — the run's id, resolved on the screen from the
        // list's cards (spec §9.3.2).
        ChatIntent::StopSubagentRun { id } => AppCommand::StopSubagentRun { id },
        ChatIntent::ExportChat { id, format, path } => AppCommand::ExportChat { id, format, path },
        // Profile CRUD and the self-model wipe reach the same orchestrator
        // commands the settings and self-model screens send — the typed routes
        // into those screens' territory (spec §11.7, stage 2).
        ChatIntent::CreateProfile { name } => AppCommand::CreateProfile {
            name,
            system_message: String::new(),
        },
        ChatIntent::DeleteProfile(id) => AppCommand::DeleteProfile(id),
        // Stage 3: the text subcommands (`/profile system|greeting`,
        // `/impersonation …`) commit through the same two commands the settings
        // screen's editors use (docs/history/commands-stage3.md §3.2–3.3).
        ChatIntent::UpdateProfile { id, edit } => AppCommand::UpdateProfile { id, edit },
        ChatIntent::UpdateConfig(config) => AppCommand::UpdateConfig(config),
        ChatIntent::ClearSelfModel => {
            AppCommand::UpdateSelfModel(crate::entities::self_model::SelfModelEdit::Clear)
        }
        // The results screen opens on the reply (`MessageSearchResults`), like
        // the chat list's `Ctrl+G` — the orchestrator owns the index. The chat
        // screen has no list in front of it to inherit a sort order from, so the
        // command asks for the default one the list itself starts on.
        ChatIntent::SearchMessages { query } => AppCommand::SearchMessages {
            query,
            sort: SortMode::default(),
        },
        // Following a `chat://` reference is an ordinary activation: an address
        // names a conversation, not a message, so there is nothing to focus on
        // (spec §11.3). It **stashes the way back**, though — the user drilled
        // down from the conversation they were reading, exactly as they do from
        // a search hit, and `Esc` should retrace that step rather than drop them
        // into the chat list (docs/research/chat-uri-links.md fork F6).
        //
        // Whatever was stashed before is replaced: the most recent step down is
        // the one `Esc` undoes. That is also strictly better than the old
        // behaviour, where following a reference out of a chat opened from a
        // search hit simply discarded the hits.
        ChatIntent::OpenChatLink(id) => {
            if let Some(origin) = screen.active_chat().filter(|from| *from != id) {
                *back = Some(Back::Link { origin, chat: id });
            }
            AppCommand::SwitchChat(id)
        }
        ChatIntent::RagAdd { path, recursive } => AppCommand::RagAdd { path, recursive },
        ChatIntent::RagDelete { path } => AppCommand::RagDelete { path },
        ChatIntent::RagList => AppCommand::RagList,
        ChatIntent::RagRebuild => AppCommand::RagRebuild,
        ChatIntent::Reindex => AppCommand::Reindex,
        ChatIntent::Compact => AppCommand::Compact,
        ChatIntent::FileAttach { path } => AppCommand::FileAttach { path },
        ChatIntent::FileRemove { target } => AppCommand::FileRemove { target },
        ChatIntent::FileList => AppCommand::FileList,
        ChatIntent::ProjectAttach { path } => AppCommand::ProjectAttach { path },
        ChatIntent::ProjectSlot { slot, action } => AppCommand::ProjectSlot { slot, action },
        ChatIntent::ProjectDetach => AppCommand::ProjectDetach,
        ChatIntent::ProjectStatus => AppCommand::ProjectStatus,
        ChatIntent::ImageAttach { path } => AppCommand::ImageAttach { path },
        ChatIntent::ImageRemove { target } => AppCommand::ImageRemove { target },
        ChatIntent::ImageList => AppCommand::ImageList,
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
                // What the engine said about its slot count, for the hint next
                // to the `sessions` field; afterward `apply_event` updates it
                // from the `EngineSlots` event.
                settings.set_engine_slots(screen.engine_slots());
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
            match back.take() {
                Some(Back::Search { screen, .. }) => *active = ActiveScreen::Search(screen),
                // Back to the conversation the reference was followed from — an
                // ordinary switch, so it goes through the orchestrator like any
                // other. The stash is already taken, so the activation that
                // follows finds nothing to clear.
                Some(Back::Link { origin, .. }) => {
                    let _ = cmd_tx.send(AppCommand::SwitchChat(origin));
                }
                None => {}
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
        // What the assistant changed in the attached project. The same round
        // trip as the self-model screen below: the orchestrator owns the journal
        // and the project, so the screen opens on the reply
        // (`WorkspaceChanges`), not on the key press.
        ChatIntent::OpenChanges => {
            let _ = cmd_tx.send(AppCommand::OpenChanges);
            return false;
        }
        // The help overlay never becomes an orchestrator command: it is opened
        // in `handle_key_event`, which owns the overlay — the one caller that
        // can produce this intent (`?`/`/help` on the chat). The arm exists so
        // the exhaustive match keeps asking about new intents.
        ChatIntent::OpenHelp => return false,
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
        // The feed's collapse state (`Ctrl+T`/`Ctrl+O`) — stored per chat, so it
        // goes to the orchestrator (the sole writer of `Chat`), not applied here.
        ChatIntent::SetFeedView(view) => {
            let _ = cmd_tx.send(AppCommand::SetFeedView(view));
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
        // Like `CopyToClipboard`: handled in `handle_key_event`, which is the only place
        // holding the `arboard` client. It never reaches here.
        ChatIntent::PasteImage { .. } => return false,
    };
    // A way back is for someone *looking* at the chat they drilled into. Once
    // they work in it — send, regenerate, take back an exchange, compact, attach
    // a file — they have arrived, and an `Esc` that silently teleported them out
    // would be a trap of its own. The other clearing rule (leaving for a
    // different chat) is the `ChatActivated` funnel; this one is about staying.
    //
    // Which commands count is `AppCommand`'s own answer, as an exhaustive match,
    // so a new command cannot slip past this unclassified.
    if command.works_on_the_open_chat() {
        *back = None;
    }
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
        // Folding a chat's transcripts keeps the list open too: the updated
        // rows come back via the `ChatList` event, like a rename's.
        ChatListIntent::SetChildrenExpanded { id, expanded } => {
            AppCommand::SetChildrenExpanded { id, expanded }
        }
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
    back: &mut Option<Back>,
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
        // come back to these exact hits (see [`Back`]).
        SearchIntent::OpenHit {
            chat,
            message,
            query,
        } => {
            if let ActiveScreen::Search(results) = std::mem::replace(active, ActiveScreen::Chat) {
                *back = Some(Back::Search {
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
