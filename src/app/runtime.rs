//! Петля рендеринга TUI и обработка ввода. См. spec §4.4.1 и plan M0.
//!
//! На M0 это синхронная петля-заглушка: рисует приветственный экран и
//! завершается по `q` / `Esc` / `Ctrl+C`. На M1 сюда добавляется мост
//! tokio ↔ TUI (каналы команд/событий) и реальные экраны.

use std::time::Duration;

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Alignment;
use ratatui::style::Stylize;
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::events::AppCommand;

/// Период опроса ввода, когда событий нет (тик перерисовки).
const TICK: Duration = Duration::from_millis(100);

/// Инициализирует терминал, запускает петлю и гарантированно восстанавливает
/// терминал на выходе (в т.ч. при панике — `ratatui::init` ставит panic hook).
pub fn run() -> Result<()> {
    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal);
    ratatui::restore();
    result
}

fn run_loop(terminal: &mut DefaultTerminal) -> Result<()> {
    let mut should_quit = false;
    while !should_quit {
        terminal.draw(draw)?;
        if event::poll(TICK)?
            && let Event::Key(key) = event::read()?
            && let Some(AppCommand::Quit) = map_key(key)
        {
            should_quit = true;
        }
    }
    Ok(())
}

/// Преобразует нажатие клавиши в команду приложения (на M0 — только выход).
fn map_key(key: KeyEvent) -> Option<AppCommand> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), KeyModifiers::NONE) => Some(AppCommand::Quit),
        (KeyCode::Esc, _) => Some(AppCommand::Quit),
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(AppCommand::Quit),
        _ => None,
    }
}

fn draw(frame: &mut Frame) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" mindfork-rs ");
    let text = Text::from(vec![
        Line::from(""),
        Line::from("mindfork-rs — консольный ИИ-чат").centered(),
        Line::from("каркас M0").dim().centered(),
        Line::from(""),
        Line::from("q / Esc / Ctrl+C — выход").dim().centered(),
    ]);
    let paragraph = Paragraph::new(text)
        .block(block)
        .alignment(Alignment::Center);
    frame.render_widget(paragraph, frame.area());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn quit_keys_map_to_quit() {
        assert!(matches!(
            map_key(press(KeyCode::Char('q'), KeyModifiers::NONE)),
            Some(AppCommand::Quit)
        ));
        assert!(matches!(
            map_key(press(KeyCode::Esc, KeyModifiers::NONE)),
            Some(AppCommand::Quit)
        ));
        assert!(matches!(
            map_key(press(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(AppCommand::Quit)
        ));
    }

    #[test]
    fn other_keys_are_ignored() {
        assert!(map_key(press(KeyCode::Char('a'), KeyModifiers::NONE)).is_none());
        assert!(map_key(press(KeyCode::Char('q'), KeyModifiers::CONTROL)).is_none());
    }

    #[test]
    fn non_press_events_are_ignored() {
        let mut ev = press(KeyCode::Char('q'), KeyModifiers::NONE);
        ev.kind = KeyEventKind::Release;
        assert!(map_key(ev).is_none());
    }
}
