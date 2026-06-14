//! Петля рендеринга TUI и мост к оркестратору. См. spec §4.4.1, §11.
//!
//! Петля синхронная (на главном потоке): опрашивает ввод с таймаутом,
//! неблокирующе дренирует события оркестратора и перерисовывает [`ChatScreen`].
//! `app` — единственный, кто знает обе стороны контракта: входящие [`AppEvent`]
//! применяются к экрану мутаторами, исходящие [`ChatIntent`] транслируются в
//! [`AppCommand`]. Сам экран про `app`/каналы не знает (FSD, зависимости вниз).

use std::sync::mpsc::Receiver;
use std::time::Duration;

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::app::events::{AppCommand, AppEvent};
use crate::features::spellcheck::SpellChecker;
use crate::screens::chat::{ChatIntent, ChatScreen};
use crate::screens::settings::{SettingsIntent, SettingsScreen};

/// Период опроса ввода (тик перерисовки).
const TICK: Duration = Duration::from_millis(50);

/// Инициализирует терминал, запускает петлю и восстанавливает терминал на выходе
/// (в т.ч. при панике — `ratatui::init` ставит panic hook). `spell_rx` доставляет
/// спелл-чекер по готовности фоновой загрузки словарей.
pub fn run(
    cmd_tx: UnboundedSender<AppCommand>,
    evt_rx: UnboundedReceiver<AppEvent>,
    spell_rx: Receiver<SpellChecker>,
) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal, &cmd_tx, evt_rx, spell_rx);
    ratatui::restore();
    // Просим оркестратор остановиться (на случай выхода не по Quit-команде).
    let _ = cmd_tx.send(AppCommand::Quit);
    result
}

fn run_loop(
    terminal: &mut DefaultTerminal,
    cmd_tx: &UnboundedSender<AppCommand>,
    mut evt_rx: UnboundedReceiver<AppEvent>,
    spell_rx: Receiver<SpellChecker>,
) -> Result<()> {
    let mut screen = ChatScreen::new();
    // Экран настроек открывается поверх чата (Ctrl+,). События продолжают
    // применяться к чату (генерация не прерывается).
    let mut settings: Option<SettingsScreen> = None;
    let mut quit = false;
    while !quit {
        while let Ok(event) = evt_rx.try_recv() {
            apply_event(&mut screen, &mut settings, event);
        }
        // Словари загрузились в фоне — подключаем спелл-чек.
        if let Ok(checker) = spell_rx.try_recv() {
            screen.set_spellchecker(checker);
        }
        if let Some(settings_screen) = &mut settings {
            terminal.draw(|frame| settings_screen.render(frame))?;
        } else {
            terminal.draw(|frame| screen.render(frame))?;
        }
        if event::poll(TICK)?
            && let Event::Key(key) = event::read()?
        {
            if let Some(settings_screen) = &mut settings {
                if let Some(intent) = settings_screen.handle_key(key) {
                    dispatch_settings(intent, cmd_tx, &mut settings);
                }
            } else if let Some(intent) = screen.handle_key(key) {
                quit = dispatch(intent, cmd_tx, &screen, &mut settings);
            }
        }
    }
    Ok(())
}

/// Применяет событие оркестратора к экрану чата (read-only-проекция). Снимок
/// настроек при открытом экране настроек дополнительно обновляет его рабочую
/// копию (отражает создание/удаление профилей).
fn apply_event(screen: &mut ChatScreen, settings: &mut Option<SettingsScreen>, event: AppEvent) {
    match event {
        AppEvent::ServerStatus(status) => screen.set_server_status(status),
        AppEvent::ChatList(chats) => screen.set_chat_list(chats),
        AppEvent::ProfileList(profiles) => screen.set_profile_list(profiles),
        AppEvent::Settings { config, profiles } => {
            if let Some(settings_screen) = settings {
                settings_screen.refresh((*config).clone(), profiles.clone());
            }
            screen.set_settings(*config, profiles);
        }
        AppEvent::ChatActivated {
            id,
            title,
            messages,
        } => screen.activate_chat(id, title, &messages),
        AppEvent::UserMessage(text) => screen.push_user_message(text),
        AppEvent::GenerationStarted { generation_id } => screen.begin_generation(generation_id),
        AppEvent::Chunk {
            generation_id,
            text,
        } => screen.push_chunk(generation_id, &text),
        AppEvent::Thoughts {
            generation_id,
            text,
        } => screen.push_thoughts(generation_id, &text),
        AppEvent::ToolCall {
            generation_id,
            name,
            arguments,
            result,
        } => screen.push_tool_call(generation_id, name, arguments, result),
        AppEvent::Finished {
            generation_id,
            reason,
        } => screen.finish_generation(generation_id, reason),
        AppEvent::Error(message) => screen.push_error(&message),
    }
}

/// Транслирует намерение чата в команду оркестратору (или открывает настройки).
/// Возвращает `true` для [`ChatIntent::Quit`] (петля завершается).
fn dispatch(
    intent: ChatIntent,
    cmd_tx: &UnboundedSender<AppCommand>,
    screen: &ChatScreen,
    settings: &mut Option<SettingsScreen>,
) -> bool {
    let command = match intent {
        ChatIntent::Quit => return true,
        ChatIntent::Send(text) => AppCommand::SendMessage(text),
        ChatIntent::Cancel => AppCommand::Cancel,
        ChatIntent::NewChat { profile_id } => AppCommand::NewChat { profile_id },
        ChatIntent::SwitchChat(id) => AppCommand::SwitchChat(id),
        ChatIntent::CloneChat(id) => AppCommand::CloneChat(id),
        ChatIntent::DeleteChat(id) => AppCommand::DeleteChat(id),
        ChatIntent::RenameChat { id, title } => AppCommand::RenameChat { id, title },
        ChatIntent::OpenSettings => {
            if let Some((config, profiles)) = screen.settings_snapshot() {
                *settings = Some(SettingsScreen::new(config, profiles));
            }
            return false;
        }
    };
    let _ = cmd_tx.send(command);
    false
}

/// Транслирует намерение экрана настроек в команду (или закрывает его).
fn dispatch_settings(
    intent: SettingsIntent,
    cmd_tx: &UnboundedSender<AppCommand>,
    settings: &mut Option<SettingsScreen>,
) {
    let command = match intent {
        SettingsIntent::Close => {
            *settings = None;
            return;
        }
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
}
