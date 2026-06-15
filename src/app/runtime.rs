//! Петля рендеринга TUI и мост к оркестратору. См. spec §4.4.1, §11.
//!
//! Петля синхронная (на главном потоке): опрашивает ввод с таймаутом,
//! неблокирующе дренирует события оркестратора и перерисовывает [`ChatScreen`].
//! `app` — единственный, кто знает обе стороны контракта: входящие [`AppEvent`]
//! применяются к экрану мутаторами, исходящие [`ChatIntent`] транслируются в
//! [`AppCommand`]. Сам экран про `app`/каналы не знает (FSD, зависимости вниз).

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::app::events::{AppCommand, AppEvent};
use crate::features::spellcheck::{SpellChecker, dict};
use crate::screens::chat::{ChatIntent, ChatScreen};
use crate::screens::settings::{SettingsIntent, SettingsScreen};

/// Период опроса ввода (тик перерисовки).
const TICK: Duration = Duration::from_millis(50);

/// Инициализирует терминал, запускает петлю и восстанавливает терминал на выходе
/// (в т.ч. при панике — `ratatui::init` ставит panic hook). Словари спелл-чека
/// `app` грузит сам в фоне по настройкам (`dict_dir`/`personal`) и перегружает при
/// их изменении.
pub fn run(
    cmd_tx: UnboundedSender<AppCommand>,
    evt_rx: UnboundedReceiver<AppEvent>,
    dict_dir: PathBuf,
    personal: PathBuf,
) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal, &cmd_tx, evt_rx, dict_dir, personal);
    ratatui::restore();
    // Просим оркестратор остановиться (на случай выхода не по Quit-команде).
    let _ = cmd_tx.send(AppCommand::Quit);
    result
}

/// Состояние фоновой (пере)загрузки словарей спелл-чека. Перезагрузка запускается
/// при изменении `interface.spellcheck_enabled`/`selected_dictionaries` (событие
/// `Settings`); `generation` отбрасывает устаревшие результаты. См. spec §11.6.
struct SpellLoader {
    dict_dir: PathBuf,
    personal: PathBuf,
    tx: Sender<(u64, SpellChecker)>,
    rx: Receiver<(u64, SpellChecker)>,
    /// Последние применённые настройки `(включён, словари)` (None — ещё не грузили).
    applied: Option<(bool, Vec<String>)>,
    /// Номер последней запущенной загрузки (применяем только её результат).
    generation: u64,
}

impl SpellLoader {
    fn new(dict_dir: PathBuf, personal: PathBuf) -> Self {
        let (tx, rx) = channel();
        Self {
            dict_dir,
            personal,
            tx,
            rx,
            applied: None,
            generation: 0,
        }
    }

    /// Если настройки спелл-чека изменились — запускает фоновую (пере)загрузку.
    fn maybe_reload(&mut self, enabled: bool, selected: &[String]) {
        let changed = self
            .applied
            .as_ref()
            .is_none_or(|(e, s)| *e != enabled || s.as_slice() != selected);
        if !changed {
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        let (dir, personal, tx) = (
            self.dict_dir.clone(),
            self.personal.clone(),
            self.tx.clone(),
        );
        let selected = selected.to_vec();
        let sel_for_thread = selected.clone();
        std::thread::spawn(move || {
            let checker = dict::load(&dir, &personal, enabled, &sel_for_thread);
            let _ = tx.send((generation, checker));
        });
        self.applied = Some((enabled, selected));
    }

    /// Готовый чекер последней загрузки (устаревшие отбрасываются), если есть.
    fn poll(&self) -> Option<SpellChecker> {
        let mut latest = None;
        while let Ok((generation, checker)) = self.rx.try_recv() {
            if generation == self.generation {
                latest = Some(checker);
            }
        }
        latest
    }
}

fn run_loop(
    terminal: &mut DefaultTerminal,
    cmd_tx: &UnboundedSender<AppCommand>,
    mut evt_rx: UnboundedReceiver<AppEvent>,
    dict_dir: PathBuf,
    personal: PathBuf,
) -> Result<()> {
    let mut screen = ChatScreen::new();
    // Экран настроек открывается поверх чата (Ctrl+P). События продолжают
    // применяться к чату (генерация не прерывается).
    let mut settings: Option<SettingsScreen> = None;
    let mut spell = SpellLoader::new(dict_dir, personal);
    let mut quit = false;
    while !quit {
        while let Ok(event) = evt_rx.try_recv() {
            apply_event(&mut screen, &mut settings, event);
        }
        // Настройки спелл-чека получены/изменились — (пере)грузим словари в фоне.
        if let Some((enabled, selected)) = screen.spell_config() {
            spell.maybe_reload(enabled, selected);
        }
        // Готовая (пере)загрузка — подключаем чекер (отключённый ничего не флагует).
        if let Some(checker) = spell.poll() {
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
        AppEvent::ChatRenamed { id, title } => screen.rename_chat(id, title),
        AppEvent::ChatListError(message) => screen.set_overlay_error(message),
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
        ChatIntent::RegenerateLast => AppCommand::RegenerateLast,
        ChatIntent::DeleteLastExchange => AppCommand::DeleteLastExchange,
        ChatIntent::Cancel => AppCommand::Cancel,
        ChatIntent::NewChat { profile_id } => AppCommand::NewChat { profile_id },
        ChatIntent::SwitchChat(id) => AppCommand::SwitchChat(id),
        ChatIntent::CloneChat(id) => AppCommand::CloneChat(id),
        ChatIntent::DeleteChat(id) => AppCommand::DeleteChat(id),
        ChatIntent::RenameChat { id, title } => AppCommand::RenameChat { id, title },
        ChatIntent::AutoRenameChat(id) => AppCommand::AutoRenameChat(id),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spell_loader_reloads_only_on_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut loader = SpellLoader::new(dir.path().to_path_buf(), dir.path().join("p.txt"));

        loader.maybe_reload(true, &[]);
        assert_eq!(loader.generation, 1);
        loader.maybe_reload(true, &[]); // те же настройки — без перезагрузки
        assert_eq!(loader.generation, 1);
        loader.maybe_reload(false, &[]); // выключили — перезагрузка
        assert_eq!(loader.generation, 2);
        loader.maybe_reload(true, &["en_US".to_string()]); // другой выбор — перезагрузка
        assert_eq!(loader.generation, 3);

        // Дожидаемся готового чекера последней загрузки (пустой каталог → выключен).
        let mut got = None;
        for _ in 0..300 {
            if let Some(c) = loader.poll() {
                got = Some(c);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(got.is_some(), "фоновая загрузка не завершилась");
        assert!(!got.unwrap().is_enabled());
    }
}
