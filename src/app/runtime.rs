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
    let mut quit = false;
    while !quit {
        while let Ok(event) = evt_rx.try_recv() {
            apply_event(&mut screen, event);
        }
        // Словари загрузились в фоне — подключаем спелл-чек.
        if let Ok(checker) = spell_rx.try_recv() {
            screen.set_spellchecker(checker);
        }
        terminal.draw(|frame| screen.render(frame))?;
        if event::poll(TICK)?
            && let Event::Key(key) = event::read()?
            && let Some(intent) = screen.handle_key(key)
        {
            quit = dispatch(intent, cmd_tx);
        }
    }
    Ok(())
}

/// Применяет событие оркестратора к экрану (read-only-проекция).
fn apply_event(screen: &mut ChatScreen, event: AppEvent) {
    match event {
        AppEvent::ServerStatus(status) => screen.set_server_status(status),
        AppEvent::ChatList(chats) => screen.set_chat_list(chats),
        AppEvent::ProfileList(profiles) => screen.set_profile_list(profiles),
        // Снимок настроек применяется экраном настроек (проводка — ниже в M8).
        AppEvent::Settings { config, profiles } => screen.set_settings(*config, profiles),
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

/// Транслирует намерение экрана в команду оркестратору. Возвращает `true` для
/// [`ChatIntent::Quit`] (петля завершается).
fn dispatch(intent: ChatIntent, cmd_tx: &UnboundedSender<AppCommand>) -> bool {
    let command = match intent {
        ChatIntent::Quit => return true,
        ChatIntent::Send(text) => AppCommand::SendMessage(text),
        ChatIntent::Cancel => AppCommand::Cancel,
        ChatIntent::NewChat { profile_id } => AppCommand::NewChat { profile_id },
        ChatIntent::SwitchChat(id) => AppCommand::SwitchChat(id),
        ChatIntent::CloneChat(id) => AppCommand::CloneChat(id),
        ChatIntent::DeleteChat(id) => AppCommand::DeleteChat(id),
        ChatIntent::RenameChat { id, title } => AppCommand::RenameChat { id, title },
    };
    let _ = cmd_tx.send(command);
    false
}
