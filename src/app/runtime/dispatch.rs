//! Runtime — применение AppEvent к экрану + трансляция Intent → AppCommand. Часть модуля [`super`]; разбито из монолита
//! runtime.rs (см. docs/refactoring-god-objects.md, этап 7).

use super::*;

/// Применяет событие оркестратора к экрану чата (read-only-проекция). Снимок
/// настроек при открытом экране настроек дополнительно обновляет его рабочую
/// копию (отражает создание/удаление профилей).
pub(super) fn apply_event(
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
    clipboard: &mut Option<arboard::Clipboard>,
    cmd_tx: &UnboundedSender<AppCommand>,
    event: AppEvent,
) {
    match event {
        // Статус серверов — в чат (строка статуса) и, если открыт, в экран настроек
        // (чипы в секции «Модель/сервер»: подключение → готов видно на месте).
        AppEvent::ServerStatus(status) => {
            if let ActiveScreen::Settings(settings) = active {
                settings.set_server_statuses(status.clone());
            }
            screen.set_server_status(status);
        }
        // Снимок списка применяем к чату всегда (для следующего открытия/`Ctrl+N`),
        // а при открытом экране списка — ещё и к нему (живое обновление).
        AppEvent::ChatList(chats) => {
            if let ActiveScreen::ChatList(list) = active {
                list.set_chats(chats.clone());
            }
            screen.set_chat_list(chats);
        }
        AppEvent::ChatRenamed { id, title } => screen.rename_chat(id, title),
        // Ошибка операции списка: в его область статуса, если экран открыт; иначе
        // (поздний ответ авто-названия при закрытом списке) — заметкой в ленту.
        AppEvent::ChatListError(message) => match active {
            ActiveScreen::ChatList(list) => list.set_error(message),
            _ => screen.push_error(&message),
        },
        // Запись в буфер обмена — side-effect UI-слоя; подтверждение/ошибку шлём в
        // область статуса экрана списка чатов (его и открывали для копирования);
        // если он уже закрыт — заметкой в ленту.
        AppEvent::CopyToClipboard(text) => {
            let result = write_clipboard(clipboard, &text);
            match active {
                ActiveScreen::ChatList(list) => match result {
                    Ok(()) => list.set_notice("Переписка скопирована в буфер обмена".into()),
                    Err(err) => {
                        list.set_error(format!("Не удалось скопировать в буфер обмена: {err}"))
                    }
                },
                _ => match result {
                    Ok(()) => screen.push_note("Переписка скопирована в буфер обмена"),
                    Err(err) => {
                        screen.push_error(&format!("Не удалось скопировать в буфер обмена: {err}"))
                    }
                },
            }
        }
        AppEvent::ProfileList(profiles) => screen.set_profile_list(profiles),
        AppEvent::Settings { config, profiles } => {
            match active {
                ActiveScreen::Settings(settings) => {
                    settings.refresh((*config).clone(), profiles.clone())
                }
                // Тема/режим совместимости могли смениться — обновим палитру
                // открытых экранов.
                ActiveScreen::ChatList(list) => list.set_palette(
                    Palette::for_theme(config.interface.theme)
                        .with_compat(config.interface.terminal_compat),
                ),
                ActiveScreen::SelfModel(view) => view.set_palette(
                    Palette::for_theme(config.interface.theme)
                        .with_compat(config.interface.terminal_compat),
                ),
                ActiveScreen::Chat => {}
            }
            screen.set_settings(*config, profiles);
        }
        AppEvent::ChatActivated {
            id,
            title,
            messages,
            draft,
        } => {
            if let ActiveScreen::ChatList(list) = active {
                if list.take_pending_new_chat() {
                    // Пришла активация только что созданного чата (`Ctrl+N` в
                    // списке) — закрываем список и показываем новый чат. Так
                    // переход прежний→новый атомарен, без промежуточного мигания.
                    *active = ActiveScreen::Chat;
                } else {
                    // Удаление активного чата при открытом списке меняет активный —
                    // обновим его метку в списке.
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
        } => screen.set_token_usage(generation_id, completion, context, context_exact),
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
        // Ответ на запрос/правку модели себя (`F3`): открываем экран либо обновляем
        // уже открытый на месте (сохраняя выделение — важно при правках).
        AppEvent::SelfModelView(model) => match active {
            ActiveScreen::SelfModel(view) => view.set_model(*model),
            _ => {
                *active = ActiveScreen::SelfModel(Box::new(SelfModelScreen::new(
                    *model,
                    screen.palette(),
                )))
            }
        },
        // «Модель себя» изменилась фоном/инструментами — обновляем ТОЛЬКО открытый
        // экран `F3` (перезапрос свежего снимка); при закрытом — игнор.
        AppEvent::SelfModelChanged => {
            if matches!(active, ActiveScreen::SelfModel(_)) {
                let _ = cmd_tx.send(AppCommand::RequestSelfModel);
            }
        }
        AppEvent::BackgroundTask { kind, active: on } => match kind {
            BackgroundKind::Reflection => screen.set_reflecting(on),
            BackgroundKind::Consolidation => screen.set_consolidating(on),
        },
        AppEvent::Error(message) => screen.push_error(&message),
    }
}

/// Транслирует намерение чата в команду оркестратору (или открывает экран
/// настроек/списка чатов). Возвращает `true` для [`ChatIntent::Quit`].
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
        ChatIntent::OpenSettings => {
            if let Some((config, profiles)) = screen.settings_snapshot() {
                let mut settings = SettingsScreen::new(config, profiles);
                // Начальный снимок статусов серверов (чипы в секции «Модель/сервер»);
                // дальше их обновляет `apply_event` из события `ServerStatus`.
                settings.set_server_statuses(screen.server_statuses());
                *active = ActiveScreen::Settings(Box::new(settings));
            }
            return false;
        }
        // Список чатов открывается из снимка, который чат держит актуальным.
        ChatIntent::OpenChatList => {
            *active = ActiveScreen::ChatList(Box::new(ChatListScreen::new(
                screen.chat_summaries(),
                screen.active_chat(),
                screen.palette(),
            )));
            return false;
        }
        // Просмотр модели себя: данными владеет оркестратор — запрашиваем снимок,
        // экран откроется по событию `SelfModelView` (см. `apply_event`).
        ChatIntent::OpenSelfModel => {
            let _ = cmd_tx.send(AppCommand::RequestSelfModel);
            return false;
        }
        // Тумблер прокрутки колесом: включаем/выключаем захват мыши терминала.
        // Это чисто терминальный side-effect (FSD: экран про терминал не знает,
        // только сообщает желаемое состояние). При включённом захвате выделение
        // текста доступно с зажатым Shift.
        ChatIntent::SetMouseCapture(on) => {
            let _ = if on {
                execute!(stdout(), EnableMouseCapture)
            } else {
                execute!(stdout(), DisableMouseCapture)
            };
            return false;
        }
    };
    let _ = cmd_tx.send(command);
    false
}

/// Транслирует намерение экрана списка чатов в команду (или управление экранами).
/// Возвращает `true` для [`ChatListIntent::Quit`] (петля завершается).
pub(super) fn dispatch_chat_list(
    intent: ChatListIntent,
    cmd_tx: &UnboundedSender<AppCommand>,
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
) -> bool {
    let command = match intent {
        // Закрытие/переход к чату возвращает базовый экран.
        ChatListIntent::Close => {
            *active = ActiveScreen::Chat;
            return false;
        }
        ChatListIntent::Quit => return true,
        ChatListIntent::Switch(id) => {
            *active = ActiveScreen::Chat;
            AppCommand::SwitchChat(id)
        }
        // Создание чата запускает поток нового чата на экране чата (там живёт
        // выбор профиля — оверлей при >1 профиле).
        ChatListIntent::NewChat => {
            match screen.request_new_chat() {
                // Один профиль: чат создаёт оркестратор (round-trip). Список
                // ОСТАВЛЯЕМ открытым до прихода `ChatActivated` нового чата —
                // иначе на время round-trip мигнул бы прежний активный чат. По
                // приходу активации `apply_event` переключит на новый чат.
                Some(ChatIntent::NewChat { profile_id }) => {
                    if let ActiveScreen::ChatList(list) = active {
                        list.set_pending_new_chat();
                    }
                    let _ = cmd_tx.send(AppCommand::NewChat { profile_id });
                }
                // >1 профиля: `request_new_chat` открыл оверлей выбора профиля в
                // экране чата — показываем чат, чтобы оверлей был виден.
                _ => *active = ActiveScreen::Chat,
            }
            return false;
        }
        ChatListIntent::Clone(id) => {
            *active = ActiveScreen::Chat;
            AppCommand::CloneChat(id)
        }
        // Копирование/удаление/переименование/авто-название не закрывают список:
        // подтверждение/ошибка прилетят в его область статуса, обновлённый набор —
        // событием `ChatList`.
        ChatListIntent::Copy(id) => AppCommand::CopyChat(id),
        ChatListIntent::Delete(id) => AppCommand::DeleteChat(id),
        ChatListIntent::Rename { id, title } => AppCommand::RenameChat { id, title },
        ChatListIntent::AutoRename(id) => AppCommand::AutoRenameChat(id),
    };
    let _ = cmd_tx.send(command);
    false
}

/// Транслирует намерение экрана настроек в команду (или закрывает его).
/// Возвращает `true` для [`SettingsIntent::Quit`] (петля завершается).
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
    };
    let _ = cmd_tx.send(command);
    false
}

/// Транслирует намерение экрана просмотра модели себя: закрытие возвращает к чату,
/// `Quit` завершает петлю (`true`). Команд оркестратору не шлёт (read-only).
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
        // Правка: команда оркестратору; экран остаётся открытым и обновится по
        // ответному `SelfModelView` (см. `apply_event`).
        SelfModelIntent::Edit(edit) => {
            let _ = cmd_tx.send(AppCommand::UpdateSelfModel(edit));
            false
        }
    }
}
