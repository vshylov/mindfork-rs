//! Петля рендеринга TUI и мост к оркестратору. См. spec §4.4.1, §11.
//!
//! Петля синхронная (на главном потоке): опрашивает ввод с таймаутом,
//! неблокирующе дренирует события оркестратора и перерисовывает [`ChatScreen`].
//! `app` — единственный, кто знает обе стороны контракта: входящие [`AppEvent`]
//! применяются к экрану мутаторами, исходящие [`ChatIntent`] транслируются в
//! [`AppCommand`]. Сам экран про `app`/каналы не знает (FSD, зависимости вниз).

use std::io::stdout;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use anyhow::Result;
use ratatui::DefaultTerminal;
#[cfg(unix)]
use ratatui::crossterm::event::EnableBracketedPaste;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers,
};
use ratatui::crossterm::execute;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::app::events::{AppCommand, AppEvent};
use crate::features::spellcheck::{SpellChecker, dict};
use crate::screens::chat::{ChatIntent, ChatScreen};
use crate::screens::settings::{SettingsIntent, SettingsScreen};

/// Период опроса ввода (тик перерисовки).
const TICK: Duration = Duration::from_millis(50);

/// Порог «всплеска» событий за один дренаж, при котором подозреваем вставку и
/// добираем её хвост (см. `run_loop`). Человек не набирает столько за один
/// zero-timeout-дренаж — несколько событий разом означают вставку.
const PASTE_BURST: usize = 2;

/// Пауза-детектор конца вставки. На Windows крупная вставка приходит несколькими
/// порциями через консольный буфер, и петля дренирует их за разные итерации — на
/// стыке порций серия символов рвалась бы, а одиночный `Enter` на стыке уезжал бы
/// как отправка. Пока события идут с зазором < этого порога — считаем их одной
/// вставкой; реальный ввод человека имеет паузы много больше (>100мс реакции).
const PASTE_GAP: Duration = Duration::from_millis(20);

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
    // На unix включаем bracketed paste: crossterm отдаёт вставку из буфера ОДНИМ
    // событием `Event::Paste` (целиком, переводы строк — текстом, не Enter). На
    // Windows этого режима у crossterm нет (ввод читается через Console API), там
    // вставка приходит пачкой обычных key-событий — её собираем в петле
    // (`process_input_batch`), поэтому включать тут нечего. См. spec §11.5.
    #[cfg(unix)]
    let _ = execute!(stdout(), EnableBracketedPaste);
    // Захват мыши по умолчанию ВЫКЛЮЧЕН: тогда работает нативное выделение текста
    // мышью. Прокрутка ленты колесом включается тумблером (`Ctrl+W`) — он шлёт
    // `EnableMouseCapture`/`DisableMouseCapture` (см. `dispatch`). Дополняем
    // panic-hook ratatui выключением мыши и bracketed paste: иначе после паники с
    // включёнными режимами терминал продолжит слать escape-коды в шелл.
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
        prev_hook(info);
    }));
    let result = run_loop(&mut terminal, &cmd_tx, evt_rx, dict_dir, personal);
    // Снимаем режимы на выходе (безвредно, если уже выключены).
    let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
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
    // Буфер обмена создаётся лениво при первом копировании (на headless-Linux без
    // X11/Wayland конструктор может упасть — тогда показываем ошибку, не паникуем).
    let mut clipboard: Option<arboard::Clipboard> = None;
    let mut spell = SpellLoader::new(dict_dir, personal);
    let mut quit = false;
    // Перерисовываем ТОЛЬКО при изменениях (флаг `dirty`), а не на каждый тик.
    // Иначе `terminal.draw` зовётся ~20 раз/сек и каждый раз переставляет курсор
    // (`frame.set_cursor_position`), а терминал (особенно Windows Terminal)
    // сбрасывает фазу мигания на каждое перемещение курсора → курсор мигает чаще
    // и неровно, хотя CPU ~0% (diff буфера пустой). Анимаций по таймеру в рендере
    // нет, поэтому простаивающие тики перерисовки не нужны. См. spec §11.
    let mut dirty = true;
    while !quit {
        while let Ok(event) = evt_rx.try_recv() {
            apply_event(&mut screen, &mut settings, &mut clipboard, event);
            dirty = true;
        }
        // Настройки спелл-чека получены/изменились — (пере)грузим словари в фоне.
        if let Some((enabled, selected)) = screen.spell_config() {
            spell.maybe_reload(enabled, selected);
        }
        // Готовая (пере)загрузка — подключаем чекер (отключённый ничего не флагует).
        if let Some(checker) = spell.poll() {
            screen.set_spellchecker(checker);
            dirty = true;
        }
        // Дебаунс-перепроверка орфографии: петля крутится каждый тик (таймаут
        // `poll`), даже когда не рисует, поэтому здесь и обеспечивается пробуждение
        // по истечении дебаунса. Перерисовываем только когда подсветка реально
        // пересчитана. На экране настроек ввод чата не активен — пропускаем.
        if settings.is_none() && screen.maybe_recheck_spelling() {
            dirty = true;
        }
        // Пока идёт фоновая индексация RAG — перерисовываем каждый тик для анимации
        // спиннера (вне индексации простаивающие тики не рисуют — см. `dirty`).
        if settings.is_none() && screen.is_rag_active() {
            dirty = true;
        }
        if dirty {
            if let Some(settings_screen) = &mut settings {
                terminal.draw(|frame| settings_screen.render(frame))?;
            } else {
                terminal.draw(|frame| screen.render(frame))?;
            }
            dirty = false;
        }
        if event::poll(TICK)? {
            // Любое терминальное событие (ввод, скролл, ресайз) может изменить вид.
            dirty = true;
            // Дренируем ВСЕ доступные сейчас события разом. На Windows вставка из
            // буфера приходит пачкой обычных key-событий (Event::Paste там нет —
            // см. выше). Без батчинга это перерисовка на символ (тормоза), а Enter
            // внутри текста = отправка. Пачку коалесим в `process_input_batch`.
            let mut batch = Vec::new();
            collect_press(&mut batch, event::read()?);
            while event::poll(Duration::ZERO)? {
                collect_press(&mut batch, event::read()?);
            }
            // Похоже на вставку (всплеск событий за один дренаж) — добираем её хвост
            // с короткой паузой-детектором (`PASTE_GAP`), чтобы крупная вставка из
            // нескольких консольных порций собралась в ОДНУ пачку. Иначе на стыке
            // порций серия рвётся и одиночный `Enter` уезжает как отправка (Windows).
            if batch.len() >= PASTE_BURST {
                while event::poll(PASTE_GAP)? {
                    collect_press(&mut batch, event::read()?);
                }
            }
            if process_input_batch(batch, &mut screen, &mut settings, cmd_tx) {
                quit = true;
            }
        }
    }
    Ok(())
}

/// Применяет событие оркестратора к экрану чата (read-only-проекция). Снимок
/// настроек при открытом экране настроек дополнительно обновляет его рабочую
/// копию (отражает создание/удаление профилей).
fn apply_event(
    screen: &mut ChatScreen,
    settings: &mut Option<SettingsScreen>,
    clipboard: &mut Option<arboard::Clipboard>,
    event: AppEvent,
) {
    match event {
        AppEvent::ServerStatus(status) => screen.set_server_status(status),
        AppEvent::ChatList(chats) => screen.set_chat_list(chats),
        AppEvent::ChatRenamed { id, title } => screen.rename_chat(id, title),
        AppEvent::ChatListError(message) => screen.set_overlay_error(message),
        // Запись в буфер обмена — side-effect UI-слоя; подтверждение/ошибку шлём в
        // область статуса оверлея списка чатов (его и открывали для копирования).
        AppEvent::CopyToClipboard(text) => match write_clipboard(clipboard, &text) {
            Ok(()) => screen.set_overlay_notice("Переписка скопирована в буфер обмена".into()),
            Err(err) => {
                screen.set_overlay_error(format!("Не удалось скопировать в буфер обмена: {err}"))
            }
        },
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
        AppEvent::RagProgress(progress) => screen.set_rag_progress(progress),
        AppEvent::Error(message) => screen.push_error(&message),
    }
}

/// Пишет текст в системный буфер обмена, создавая клиент лениво и переиспользуя
/// его. Возвращает текст ошибки (вместо паники), если буфер недоступен — на
/// headless-Linux без X11/Wayland конструктор `arboard` может упасть.
fn write_clipboard(slot: &mut Option<arboard::Clipboard>, text: &str) -> Result<(), String> {
    if slot.is_none() {
        *slot = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
    }
    // `unwrap` безопасен: только что гарантировали `Some`.
    slot.as_mut()
        .unwrap()
        .set_text(text.to_string())
        .map_err(|e| e.to_string())
}

/// Кладёт событие в пачку, отбрасывая key-события «отпускания»/повтора: приложение
/// их и так игнорирует (`handle_key` берёт только `Press`), а при коалесинге вставки
/// они разрывали бы серию символов между нажатиями (на Windows вставка идёт как
/// пары down/up). Мышь/ресайз/`Paste` пропускаются как есть.
fn collect_press(batch: &mut Vec<Event>, ev: Event) {
    if let Event::Key(k) = &ev
        && k.kind != KeyEventKind::Press
    {
        return;
    }
    batch.push(ev);
}

/// Символ для коалесинга вставки: печатная клавиша / Enter / Tab без Ctrl/Alt.
/// `Enter → '\r'` (CRLF из буфера затем схлопывается в `InputBox::insert_str`),
/// `Tab → '\t'`. Возвращает `None` для всего остального (стрелки, Ctrl-шорткаты,
/// функц. клавиши) — оно разрывает серию вставки.
fn paste_char(key: &KeyEvent) -> Option<char> {
    if key.kind != KeyEventKind::Press
        || key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return None;
    }
    match key.code {
        KeyCode::Char(c) => Some(c),
        KeyCode::Enter => Some('\r'),
        KeyCode::Tab => Some('\t'),
        _ => None,
    }
}

/// Кусок пачки ввода: либо собранная вставка (серия текстовых клавиш ≥2), либо
/// одиночное событие (обычная клавиша/мышь/unix-`Paste`).
enum Chunk {
    Paste(String),
    Event(Event),
}

/// Разбивает пачку событий на куски, коалесируя подряд идущие текстовые клавиши
/// (см. [`paste_char`]) в одну вставку, если их ≥2. Чистая функция — тестируема без
/// экрана/терминала. Серия длиной 1 (обычный ввод символа / одиночный Enter)
/// остаётся одиночным событием, чтобы Enter работал как отправка.
fn chunk_batch(batch: Vec<Event>) -> Vec<Chunk> {
    let mut out = Vec::new();
    let mut iter = batch.into_iter().peekable();
    while let Some(ev) = iter.next() {
        if let Event::Key(key) = &ev
            && let Some(first) = paste_char(key)
        {
            let mut run = String::new();
            run.push(first);
            while let Some(Event::Key(k)) = iter.peek() {
                match paste_char(k) {
                    Some(c) => {
                        run.push(c);
                        iter.next();
                    }
                    None => break,
                }
            }
            if run.chars().count() >= 2 {
                out.push(Chunk::Paste(run));
                continue;
            }
            // Серия из одной клавиши — отдаём обычным событием (ниже).
        }
        out.push(Chunk::Event(ev));
    }
    out
}

/// Обрабатывает пачку терминальных событий за один проход. Коалесированные вставки
/// идут в активный редактор (экран настроек) или в поле ввода чата текстом — без
/// отправки, даже если содержат переводы строк. Одиночные события — обычным путём.
/// Возвращает `true`, если запрошен выход.
fn process_input_batch(
    batch: Vec<Event>,
    screen: &mut ChatScreen,
    settings: &mut Option<SettingsScreen>,
    cmd_tx: &UnboundedSender<AppCommand>,
) -> bool {
    let mut quit = false;
    for chunk in chunk_batch(batch) {
        match chunk {
            // Вставка из буфера: на экране настроек — в активный редактор поля,
            // иначе — в поле ввода чата (никогда не отправляет сообщение).
            Chunk::Paste(text) | Chunk::Event(Event::Paste(text)) => {
                if let Some(settings_screen) = settings.as_mut() {
                    settings_screen.handle_paste(&text);
                } else {
                    screen.handle_paste(&text);
                }
            }
            Chunk::Event(Event::Key(key)) => {
                if let Some(settings_screen) = settings.as_mut() {
                    // Два стейтмента (не collapsible): сперва снимаем намерение, чтобы
                    // отпустить заимствование `settings` до `dispatch_settings`.
                    let intent = settings_screen.handle_key(key);
                    if let Some(intent) = intent {
                        dispatch_settings(intent, cmd_tx, settings);
                    }
                } else if let Some(intent) = screen.handle_key(key)
                    && dispatch(intent, cmd_tx, screen, settings)
                {
                    quit = true;
                }
            }
            // Колесо мыши прокручивает ленту чата. На экране настроек (своя
            // навигация) прокрутку игнорируем.
            Chunk::Event(Event::Mouse(mouse)) if settings.is_none() => screen.handle_mouse(mouse),
            Chunk::Event(_) => {}
        }
    }
    quit
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
        ChatIntent::CopyChat(id) => AppCommand::CopyChat(id),
        ChatIntent::DeleteChat(id) => AppCommand::DeleteChat(id),
        ChatIntent::RagAdd { path, recursive } => AppCommand::RagAdd { path, recursive },
        ChatIntent::RagDelete { path } => AppCommand::RagDelete { path },
        ChatIntent::RenameChat { id, title } => AppCommand::RenameChat { id, title },
        ChatIntent::AutoRenameChat(id) => AppCommand::AutoRenameChat(id),
        ChatIntent::OpenSettings => {
            if let Some((config, profiles)) = screen.settings_snapshot() {
                *settings = Some(SettingsScreen::new(config, profiles));
            }
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

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn chunk_batch_coalesces_text_run_with_newline() {
        // Серия «a Enter b» (как пришла бы вставка "a\nb" на Windows) → одна вставка;
        // Enter внутри серии становится переводом строки ('\r' схлопнется при вставке).
        let batch = vec![
            key(KeyCode::Char('a')),
            key(KeyCode::Enter),
            key(KeyCode::Char('b')),
        ];
        let chunks = chunk_batch(batch);
        assert_eq!(chunks.len(), 1);
        match &chunks[0] {
            Chunk::Paste(s) => assert_eq!(s, "a\rb"),
            _ => panic!("ожидалась коалесированная вставка"),
        }
    }

    #[test]
    fn chunk_batch_keeps_multiple_newlines_in_one_paste() {
        // Серия с несколькими Enter внутри (многострочная вставка) → одна вставка,
        // все Enter — переводы строк, ни один не уезжает отправкой.
        let batch = vec![
            key(KeyCode::Char('a')),
            key(KeyCode::Enter),
            key(KeyCode::Char('b')),
            key(KeyCode::Enter),
            key(KeyCode::Char('c')),
        ];
        let chunks = chunk_batch(batch);
        assert_eq!(chunks.len(), 1);
        assert!(matches!(&chunks[0], Chunk::Paste(s) if s == "a\rb\rc"));
    }

    #[test]
    fn chunk_batch_single_enter_stays_event() {
        // Одиночный Enter — это отправка, НЕ вставка.
        let chunks = chunk_batch(vec![key(KeyCode::Enter)]);
        assert_eq!(chunks.len(), 1);
        assert!(matches!(chunks[0], Chunk::Event(_)));
    }

    #[test]
    fn chunk_batch_single_char_stays_event() {
        let chunks = chunk_batch(vec![key(KeyCode::Char('x'))]);
        assert_eq!(chunks.len(), 1);
        assert!(matches!(chunks[0], Chunk::Event(_)));
    }

    #[test]
    fn chunk_batch_splits_run_on_arrow() {
        // Стрелка разрывает серию: "ab" (вставка) + стрелка (событие) + "cd" (вставка).
        let batch = vec![
            key(KeyCode::Char('a')),
            key(KeyCode::Char('b')),
            key(KeyCode::Left),
            key(KeyCode::Char('c')),
            key(KeyCode::Char('d')),
        ];
        let chunks = chunk_batch(batch);
        assert_eq!(chunks.len(), 3);
        assert!(matches!(&chunks[0], Chunk::Paste(s) if s == "ab"));
        assert!(matches!(chunks[1], Chunk::Event(_)));
        assert!(matches!(&chunks[2], Chunk::Paste(s) if s == "cd"));
    }

    #[test]
    fn paste_char_maps_and_filters() {
        assert_eq!(
            paste_char(&KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
            Some('q')
        );
        assert_eq!(
            paste_char(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some('\r')
        );
        assert_eq!(
            paste_char(&KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Some('\t')
        );
        // Ctrl/Alt-комбинации не часть вставки (Ctrl+V, Alt+… — это шорткаты).
        assert_eq!(
            paste_char(&KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(
            paste_char(&KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            None
        );
    }

    #[test]
    fn collect_press_drops_release_events() {
        let mut batch = Vec::new();
        collect_press(&mut batch, key(KeyCode::Char('a')));
        // «Отпускание» клавиши отбрасывается (иначе разрывало бы серию вставки).
        let release = Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ));
        collect_press(&mut batch, release);
        assert_eq!(batch.len(), 1);
    }

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
