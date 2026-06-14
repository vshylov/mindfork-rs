//! Петля рендеринга TUI и мост к оркестратору. См. spec §4.4.1 и plan M1.
//!
//! Петля синхронная (на главном потоке): опрашивает ввод с таймаутом,
//! неблокирующе дренирует события оркестратора и перерисовывает экран.
//! Команды уходят оркестратору через `mpsc` (send синхронный). M1 — минимальный
//! чат (одно строковое поле ввода); полноценный UI (`tui-textarea`, список
//! чатов, markdown, сворачиваемые блоки) приходит на M3.

use std::time::Duration;

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

use crate::app::events::{AppCommand, AppEvent, ServerStatus};
use crate::entities::chat::ChatSummary;
use crate::entities::message::{Message, MessageRole};
use crate::shared::api::FinishReason;
use crate::widgets::chat_list::{ChatListAction, ChatListState};

/// Период опроса ввода (тик перерисовки).
const TICK: Duration = Duration::from_millis(50);

/// Роль элемента ленты.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    User,
    Assistant,
    Note,
}

/// Элемент ленты сообщений (UI-проекция).
#[derive(Debug, Clone)]
struct FeedItem {
    role: Role,
    text: String,
    thoughts: String,
    streaming: bool,
}

impl FeedItem {
    /// Проекция доменного сообщения в элемент ленты (для перестроения при
    /// активации чата). Системные сообщения в ленте не показываются.
    fn from_message(msg: &Message) -> Option<Self> {
        let role = match msg.role {
            MessageRole::User => Role::User,
            MessageRole::Assistant => Role::Assistant,
            MessageRole::Tool => Role::Note,
            MessageRole::System => return None,
        };
        Some(Self {
            role,
            text: msg.text.clone(),
            thoughts: msg.thoughts.clone().unwrap_or_default(),
            streaming: false,
        })
    }
}

/// Состояние UI (read-only-проекция, обновляется только событиями).
struct UiState {
    feed: Vec<FeedItem>,
    /// Активный чат и его заголовок.
    active_chat: Option<Uuid>,
    title: String,
    /// Список чатов (для оверлея).
    chats: Vec<ChatSummary>,
    /// Открытый оверлей списка чатов (если есть).
    overlay: Option<ChatListState>,
    input: String,
    status: ServerStatus,
    current_gen: Option<Uuid>,
    generating: bool,
    should_quit: bool,
}

impl UiState {
    fn new() -> Self {
        Self {
            feed: Vec::new(),
            active_chat: None,
            title: String::new(),
            chats: Vec::new(),
            overlay: None,
            input: String::new(),
            status: ServerStatus::Connecting,
            current_gen: None,
            generating: false,
            should_quit: false,
        }
    }

    fn apply(&mut self, event: AppEvent) {
        match event {
            AppEvent::ServerStatus(s) => self.status = s,
            AppEvent::ChatList(chats) => {
                // Если оверлей открыт — синхронизируем его снимок (после
                // переименования/удаления/клонирования).
                if let Some(overlay) = &mut self.overlay {
                    overlay.set_chats(chats.clone());
                }
                self.chats = chats;
            }
            AppEvent::ChatActivated {
                id,
                title,
                messages,
            } => {
                // Смена чата сбрасывает состояние генерации: «осиротевшие» чанки
                // прежней генерации не должны попадать в ленту нового чата.
                self.active_chat = Some(id);
                self.title = title;
                self.current_gen = None;
                self.generating = false;
                self.feed = messages.iter().filter_map(FeedItem::from_message).collect();
            }
            AppEvent::UserMessage(text) => self.feed.push(FeedItem {
                role: Role::User,
                text,
                thoughts: String::new(),
                streaming: false,
            }),
            AppEvent::GenerationStarted { generation_id } => {
                self.current_gen = Some(generation_id);
                self.generating = true;
                self.feed.push(FeedItem {
                    role: Role::Assistant,
                    text: String::new(),
                    thoughts: String::new(),
                    streaming: true,
                });
            }
            AppEvent::Chunk {
                generation_id,
                text,
            } => {
                if self.current_gen == Some(generation_id)
                    && let Some(last) = self.feed.last_mut()
                {
                    last.text.push_str(&text);
                }
            }
            AppEvent::Thoughts {
                generation_id,
                text,
            } => {
                if self.current_gen == Some(generation_id)
                    && let Some(last) = self.feed.last_mut()
                {
                    last.thoughts.push_str(&text);
                }
            }
            AppEvent::Finished {
                generation_id,
                reason,
            } => {
                if self.current_gen == Some(generation_id) {
                    if let Some(last) = self.feed.last_mut() {
                        last.streaming = false;
                    }
                    self.generating = false;
                    self.current_gen = None;
                    if reason == FinishReason::Cancelled {
                        self.push_note("(генерация отменена)");
                    }
                }
            }
            AppEvent::Error(msg) => self.push_note(&format!("⚠ {msg}")),
        }
    }

    fn push_note(&mut self, text: &str) {
        self.feed.push(FeedItem {
            role: Role::Note,
            text: text.to_string(),
            thoughts: String::new(),
            streaming: false,
        });
    }
}

/// Инициализирует терминал, запускает петлю и восстанавливает терминал на выходе
/// (в т.ч. при панике — `ratatui::init` ставит panic hook).
pub fn run(cmd_tx: UnboundedSender<AppCommand>, evt_rx: UnboundedReceiver<AppEvent>) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal, &cmd_tx, evt_rx);
    ratatui::restore();
    // Просим оркестратор остановиться (на случай выхода не по Quit-команде).
    let _ = cmd_tx.send(AppCommand::Quit);
    result
}

fn run_loop(
    terminal: &mut DefaultTerminal,
    cmd_tx: &UnboundedSender<AppCommand>,
    mut evt_rx: UnboundedReceiver<AppEvent>,
) -> Result<()> {
    let mut ui = UiState::new();
    while !ui.should_quit {
        while let Ok(event) = evt_rx.try_recv() {
            ui.apply(event);
        }
        terminal.draw(|frame| draw(frame, &ui))?;
        if event::poll(TICK)?
            && let Event::Key(key) = event::read()?
        {
            handle_key(key, &mut ui, cmd_tx);
        }
    }
    Ok(())
}

fn handle_key(key: KeyEvent, ui: &mut UiState, cmd_tx: &UnboundedSender<AppCommand>) {
    if key.kind != KeyEventKind::Press {
        return;
    }
    // Открытый оверлей перехватывает все клавиши.
    if ui.overlay.is_some() {
        handle_overlay_key(key, ui, cmd_tx);
        return;
    }
    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => ui.should_quit = true,
        (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
            let _ = cmd_tx.send(AppCommand::NewChat);
        }
        (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
            ui.overlay = Some(ChatListState::new(ui.chats.clone(), ui.active_chat));
        }
        (KeyCode::Esc, _) => {
            if ui.generating {
                let _ = cmd_tx.send(AppCommand::Cancel);
            } else {
                ui.should_quit = true;
            }
        }
        (KeyCode::Enter, _) => {
            let text = ui.input.trim().to_string();
            if !text.is_empty() && !ui.generating {
                let _ = cmd_tx.send(AppCommand::SendMessage(text));
                ui.input.clear();
            }
        }
        (KeyCode::Backspace, _) => {
            ui.input.pop();
        }
        (KeyCode::Char(c), m) if m == KeyModifiers::NONE || m == KeyModifiers::SHIFT => {
            ui.input.push(c);
        }
        _ => {}
    }
}

/// Маршрутизирует клавишу в открытый оверлей списка чатов и исполняет действие.
fn handle_overlay_key(key: KeyEvent, ui: &mut UiState, cmd_tx: &UnboundedSender<AppCommand>) {
    let Some(overlay) = ui.overlay.as_mut() else {
        return;
    };
    match overlay.on_key(key) {
        ChatListAction::None => {}
        ChatListAction::Close => ui.overlay = None,
        ChatListAction::Switch(id) => {
            let _ = cmd_tx.send(AppCommand::SwitchChat(id));
            ui.overlay = None;
        }
        ChatListAction::New => {
            let _ = cmd_tx.send(AppCommand::NewChat);
            ui.overlay = None;
        }
        ChatListAction::Clone(id) => {
            let _ = cmd_tx.send(AppCommand::CloneChat(id));
            ui.overlay = None;
        }
        // Удаление/переименование не закрывают оверлей: обновлённый список
        // прилетит событием `ChatList` и синхронизирует снимок.
        ChatListAction::Delete(id) => {
            let _ = cmd_tx.send(AppCommand::DeleteChat(id));
        }
        ChatListAction::Rename { id, title } => {
            let _ = cmd_tx.send(AppCommand::RenameChat { id, title });
        }
    }
}

fn draw(frame: &mut Frame, ui: &UiState) {
    let [feed_area, input_area, status_area] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    // --- лента (показываем хвост, помещающийся в область) ---
    let mut lines = feed_lines(ui);
    let height = feed_area.height.saturating_sub(2) as usize; // минус рамка
    if lines.len() > height {
        lines.drain(..lines.len() - height);
    }
    let title = if ui.title.is_empty() {
        " mindfork-rs ".to_string()
    } else {
        format!(" {} ", ui.title)
    };
    let feed = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    frame.render_widget(feed, feed_area);

    // --- поле ввода ---
    let input_title = if ui.generating {
        " ввод (генерация… Esc — отмена) "
    } else {
        " ввод (Enter — отправить, Esc — выход) "
    };
    let input = Paragraph::new(ui.input.as_str())
        .block(Block::default().borders(Borders::ALL).title(input_title))
        .wrap(Wrap { trim: false });
    frame.render_widget(input, input_area);

    // --- статус-бар ---
    frame.render_widget(Line::from(status_spans(ui)), status_area);

    // --- оверлей списка чатов (поверх всего) ---
    if let Some(overlay) = &ui.overlay {
        overlay.render(frame, frame.area(), ui.active_chat);
    }
}

fn feed_lines(ui: &UiState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if ui.feed.is_empty() {
        lines.push(Line::from("Начните диалог — введите сообщение ниже.").dim());
        return lines;
    }
    for item in &ui.feed {
        match item.role {
            Role::User => lines.push(Line::from("Вы:".to_string()).bold().cyan()),
            Role::Assistant => lines.push(Line::from("Ассистент:".to_string()).bold().green()),
            Role::Note => {}
        }
        if !item.thoughts.is_empty() {
            for t in item.thoughts.lines() {
                lines.push(Line::from(format!("  💭 {t}")).dim().italic());
            }
        }
        let body = if item.text.is_empty() && item.streaming {
            "…".to_string()
        } else {
            item.text.clone()
        };
        for line in body.split('\n') {
            let span = if item.role == Role::Note {
                Span::from(line.to_string()).dim()
            } else {
                Span::from(line.to_string())
            };
            lines.push(Line::from(span));
        }
        lines.push(Line::from(""));
    }
    lines
}

fn status_spans(ui: &UiState) -> Vec<Span<'static>> {
    let (label, style) = match &ui.status {
        ServerStatus::NotConfigured => ("сервер не настроен".to_string(), Style::new().yellow()),
        ServerStatus::Connecting => ("подключение…".to_string(), Style::new().yellow()),
        ServerStatus::Ready => ("готов".to_string(), Style::new().green()),
        ServerStatus::Disconnected(why) => (format!("нет связи: {why}"), Style::new().red()),
    };
    let mut spans = vec![Span::from("сервер: "), Span::styled(label, style)];
    if ui.generating {
        spans.push(Span::from("  •  ").dim());
        spans.push(Span::from("генерация…").magenta());
    }
    spans.push(Span::from("  •  Ctrl+L чаты · Ctrl+N новый · Ctrl+C выход").dim());
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gen_id() -> Uuid {
        Uuid::new_v4()
    }

    #[test]
    fn applies_streaming_sequence() {
        let mut ui = UiState::new();
        let id = gen_id();
        ui.apply(AppEvent::UserMessage("hi".into()));
        ui.apply(AppEvent::GenerationStarted { generation_id: id });
        ui.apply(AppEvent::Thoughts {
            generation_id: id,
            text: "hmm".into(),
        });
        ui.apply(AppEvent::Chunk {
            generation_id: id,
            text: "Hel".into(),
        });
        ui.apply(AppEvent::Chunk {
            generation_id: id,
            text: "lo".into(),
        });
        ui.apply(AppEvent::Finished {
            generation_id: id,
            reason: FinishReason::Stop,
        });

        assert_eq!(ui.feed.len(), 2);
        assert_eq!(ui.feed[0].role, Role::User);
        assert_eq!(ui.feed[1].role, Role::Assistant);
        assert_eq!(ui.feed[1].text, "Hello");
        assert_eq!(ui.feed[1].thoughts, "hmm");
        assert!(!ui.feed[1].streaming);
        assert!(!ui.generating);
    }

    #[test]
    fn ignores_chunks_from_stale_generation() {
        let mut ui = UiState::new();
        let current = gen_id();
        let stale = gen_id();
        ui.apply(AppEvent::GenerationStarted {
            generation_id: current,
        });
        ui.apply(AppEvent::Chunk {
            generation_id: stale,
            text: "ghost".into(),
        });
        ui.apply(AppEvent::Chunk {
            generation_id: current,
            text: "real".into(),
        });
        assert_eq!(ui.feed[0].text, "real");
    }

    #[test]
    fn cancelled_finish_adds_note() {
        let mut ui = UiState::new();
        let id = gen_id();
        ui.apply(AppEvent::GenerationStarted { generation_id: id });
        ui.apply(AppEvent::Finished {
            generation_id: id,
            reason: FinishReason::Cancelled,
        });
        assert!(ui.feed.iter().any(|i| i.role == Role::Note));
        assert!(!ui.generating);
    }
}
