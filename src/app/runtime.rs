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

use crate::app::events::{AppCommand, AppEvent, BackgroundKind};
use crate::features::spellcheck::{SpellChecker, dict};
use crate::screens::chat::{ChatIntent, ChatScreen};
use crate::screens::chat_list::{ChatListIntent, ChatListScreen};
use crate::screens::self_model::{SelfModelIntent, SelfModelScreen};
use crate::screens::settings::{SettingsIntent, SettingsScreen};
use crate::shared::theme::Palette;

/// Экран, открытый поверх чата. `ChatScreen` всегда существует как база (лента,
/// генерация, поле ввода); поверх него может быть открыт список чатов (`Esc`) или
/// настройки (`Ctrl+P`). См. архитектуру UI (architecture.md §9): три экрана.
enum ActiveScreen {
    /// Только чат — наложенного экрана нет.
    Chat,
    /// Полноэкранный список чатов. Боксируем — экраны крупные, держать их инлайн в
    /// enum-варианте раздувает каждое значение (clippy::large_enum_variant).
    ChatList(Box<ChatListScreen>),
    /// Экран настроек.
    Settings(Box<SettingsScreen>),
    /// Экран просмотра «модели себя» (read-only, `F3`).
    SelfModel(Box<SelfModelScreen>),
}

impl ActiveScreen {
    /// На переднем плане сам чат (а не список/настройки)? Гейтит работу, которая
    /// относится только к чату: перепроверку орфографии, анимацию спиннеров,
    /// прокрутку колесом.
    fn is_chat(&self) -> bool {
        matches!(self, ActiveScreen::Chat)
    }
}

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
    // Поверх чата может быть открыт список чатов (Esc) или настройки (Ctrl+P).
    // События оркестратора продолжают применяться к чату (генерация не прерывается).
    let mut active = ActiveScreen::Chat;
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
            apply_event(&mut screen, &mut active, &mut clipboard, cmd_tx, event);
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
        if active.is_chat() && screen.maybe_recheck_spelling() {
            dirty = true;
        }
        // На экране списка чатов поле переименования (`F2`) тоже проверяется
        // орфографией — чекер одалживаем у экрана чата (владельца). См. spec §11.5.
        if let ActiveScreen::ChatList(list) = &mut active
            && let Some(spell) = screen.spellchecker()
            && list.recheck_spelling(spell)
        {
            dirty = true;
        }
        // Пока идёт фоновая индексация RAG или имперсонация — перерисовываем каждый
        // тик для анимации спиннера (вне них простаивающие тики не рисуют — `dirty`).
        if active.is_chat() && (screen.is_rag_active() || screen.is_impersonating()) {
            dirty = true;
        }
        // Черновик поля ввода изменился — сохраняем его в активном чате (оркестратор
        // пишет на диск с дебаунсом). Перерисовку это не требует. См. spec §11.7.
        if let Some(draft) = screen.take_dirty_draft() {
            let _ = cmd_tx.send(AppCommand::SetDraft(draft));
        }
        if dirty {
            match &mut active {
                ActiveScreen::Chat => {
                    // Прокрутка ленты с «съезжающими» VS16-эмодзи (`🕸️`/`🗂️`) требует
                    // ПОЛНОЙ перерисовки: некоторые терминалы (Command Prompt/conhost)
                    // рисуют такой кластер шире модели ratatui (контент уезжает, и
                    // поячеечный diff не достаёт до уехавшего символа → «висячий»
                    // артефакт). Нужно переписать КАЖДУЮ ячейку явно — включая пробелы
                    // в пустых местах — чтобы затереть артефакт на месте.
                    //
                    // Раньше для этого звался `terminal.clear()`, но он шлёт escape-
                    // очистку экрана `ESC[2J` — экран на миг гаснет (мигание). Обойти
                    // это просто сбросом заднего буфера НЕЛЬЗЯ: тогда diff сравнивает
                    // «пусто → кадр» и пропускает ячейки-пробелы (они равны пустому
                    // заднему буферу) — пустые места НЕ перерисовываются, и старый
                    // контент/артефакт остаётся виден.
                    //
                    // Поэтому делаем задний буфер ОТЛИЧНЫМ от любой реальной ячейки:
                    // заполняем текущий буфер символом-сентинелом "\0" (его не бывает в
                    // реальном контенте) и переносим его в задний буфер через
                    // `swap_buffers()` — БЕЗ вывода на экран (flush не зовём). Тогда
                    // ближайший `draw` сдиффит «\0 → реальный кадр»: отличается каждая
                    // ячейка (и пробелы тоже), поэтому ratatui перепишет ВЕСЬ экран по
                    // ячейкам — без `ESC[2J` (без мигания) и с пробелами в пустых
                    // местах. Внутренний swap в `draw` восстанавливает инвариант
                    // «задний буфер = экран». Сам "\0" на экран не попадает.
                    //
                    // Делаем это только когда артефакт реально возможен
                    // (`take_feed_scrolled` — есть VS16 и была прокрутка); на чистом
                    // тексте прокрутка не перерисовывает всё. См. spec §11.3.
                    if screen.take_feed_scrolled() {
                        for cell in terminal.current_buffer_mut().content.iter_mut() {
                            cell.set_symbol("\u{0}");
                        }
                        terminal.swap_buffers();
                    }
                    terminal.draw(|frame| screen.render(frame))?
                }
                ActiveScreen::ChatList(list) => terminal.draw(|frame| list.render(frame))?,
                ActiveScreen::Settings(settings) => {
                    terminal.draw(|frame| settings.render(frame))?
                }
                ActiveScreen::SelfModel(view) => terminal.draw(|frame| view.render(frame))?,
            };
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
            if process_input_batch(batch, &mut screen, &mut active, cmd_tx, &mut clipboard) {
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
    active: &mut ActiveScreen,
    clipboard: &mut Option<arboard::Clipboard>,
    cmd_tx: &UnboundedSender<AppCommand>,
    event: AppEvent,
) {
    match event {
        AppEvent::ServerStatus(status) => screen.set_server_status(status),
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
                // Тема могла смениться — обновим палитру открытых экранов.
                ActiveScreen::ChatList(list) => {
                    list.set_palette(Palette::for_theme(config.interface.theme))
                }
                ActiveScreen::SelfModel(view) => {
                    view.set_palette(Palette::for_theme(config.interface.theme))
                }
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

/// Сверяет реконструированную из key-событий вставку с буфером обмена и, если это та
/// же вставка, возвращает её полную версию из буфера (с восстановленными эмодзи).
///
/// **Зачем (Windows):** при вставке из буфера обмена консоль Windows доставляет текст
/// как обычные key-события, а crossterm 0.29 **теряет** символы supplementary-плоскости
/// (эмодзи вроде 😊, U+1F60A): они кодируются UTF-16 суррогатной парой, а записи
/// key-down/key-up консоли ломают сборку пары в crossterm — символ пропадает ещё до
/// нашего слоя. BMP-символы (буквы, ❤ U+2764, селектор U+FE0F) проходят. Поэтому
/// реконструкция = вставка без supplementary-эмодзи.
///
/// Чтобы вернуть эмодзи, читаем буфер обмена и сверяем: если выбросить из него ровно те
/// символы, что теряет консоль (кодпойнты > U+FFFF), и нормализовать переводы строк/
/// табы, совпадает ли он с реконструкцией? Совпал → это та же вставка, отдаём полный
/// текст буфера. Не совпал (буфер устарел/не та вставка/недоступен) → реконструкцию
/// (без эмодзи, но без риска вставить чужое). На не-Windows — тождественно (там приходит
/// корректная bracketed-вставка `Event::Paste`).
#[cfg(windows)]
fn reconcile_paste(reconstructed: String, clipboard: &mut Option<arboard::Clipboard>) -> String {
    match read_clipboard_text(clipboard) {
        Some(clip) if paste_projection_matches(&clip, &reconstructed) => clip,
        _ => reconstructed,
    }
}

/// На не-Windows вставка приходит корректным UTF-8 (`Event::Paste`) — сверка не нужна.
#[cfg(not(windows))]
fn reconcile_paste(reconstructed: String, _clipboard: &mut Option<arboard::Clipboard>) -> String {
    reconstructed
}

/// Совпадает ли буфер обмена с реконструкцией вставки по «BMP-проекции» (чистая
/// функция, тестируема без буфера/терминала). Из буфера выбрасываются символы, которые
/// консоль Windows теряет (кодпойнты > U+FFFF — суррогатные пары), затем обе стороны
/// нормализуются по переводам строк/табам (как [`InputBox::insert_str`]). См.
/// [`reconcile_paste`].
#[cfg(windows)]
fn paste_projection_matches(clipboard: &str, reconstructed: &str) -> bool {
    fn normalize(s: &str) -> String {
        s.replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\t', "    ")
    }
    let projected: String = clipboard
        .chars()
        .filter(|c| (*c as u32) <= 0xFFFF)
        .collect();
    !reconstructed.is_empty() && normalize(&projected) == normalize(reconstructed)
}

/// Читает текст системного буфера обмена (лениво создавая клиент). `None`, если буфер
/// недоступен/пуст/не текстовый. Используется только на Windows для восстановления
/// эмодзи во вставке (см. [`reconcile_paste`]).
#[cfg(windows)]
fn read_clipboard_text(slot: &mut Option<arboard::Clipboard>) -> Option<String> {
    if slot.is_none() {
        *slot = arboard::Clipboard::new().ok();
    }
    slot.as_mut().and_then(|c| c.get_text().ok())
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
    active: &mut ActiveScreen,
    cmd_tx: &UnboundedSender<AppCommand>,
    clipboard: &mut Option<arboard::Clipboard>,
) -> bool {
    let mut quit = false;
    for chunk in chunk_batch(batch) {
        match chunk {
            // Вставка из буфера: на экране настроек — в активный редактор поля; в
            // чате — в поле ввода (никогда не отправляет); в списке цели вставки нет.
            // `Chunk::Paste` — реконструкция из key-событий (Windows); сверяем её с
            // буфером обмена, чтобы восстановить потерянные crossterm эмодзи
            // supplementary-плоскости (см. [`reconcile_paste`]). `Event::Paste` —
            // настоящая bracketed-вставка (unix), уже корректный UTF-8.
            Chunk::Paste(text) | Chunk::Event(Event::Paste(text)) => {
                let text = reconcile_paste(text, clipboard);
                match active {
                    ActiveScreen::Settings(settings) => settings.handle_paste(&text),
                    ActiveScreen::Chat => screen.handle_paste(&text),
                    // В списке цель вставки — поле переименования (`F2`), если открыто.
                    ActiveScreen::ChatList(list) => list.handle_paste(&text),
                    // В редакторе модели себя — в активное поле правки, если открыто.
                    ActiveScreen::SelfModel(view) => view.handle_paste(&text),
                }
            }
            Chunk::Event(Event::Key(key)) => {
                // Снимаем намерение из активного экрана (борроу заканчивается на
                // owned-значении), затем диспетчеризуем — иначе конфликт заимствований.
                let mut chat_intent = None;
                let mut list_intent = None;
                let mut settings_intent = None;
                let mut self_model_intent = None;
                match active {
                    ActiveScreen::Chat => chat_intent = screen.handle_key(key),
                    ActiveScreen::ChatList(list) => list_intent = list.handle_key(key),
                    ActiveScreen::Settings(settings) => settings_intent = settings.handle_key(key),
                    ActiveScreen::SelfModel(view) => self_model_intent = view.handle_key(key),
                }
                if let Some(intent) = chat_intent
                    && dispatch(intent, cmd_tx, screen, active)
                {
                    quit = true;
                }
                if let Some(intent) = list_intent
                    && dispatch_chat_list(intent, cmd_tx, screen, active)
                {
                    quit = true;
                }
                if let Some(intent) = settings_intent {
                    dispatch_settings(intent, cmd_tx, active);
                }
                if let Some(intent) = self_model_intent
                    && dispatch_self_model(intent, cmd_tx, active)
                {
                    quit = true;
                }
            }
            // Колесо мыши прокручивает ленту чата. На списке/настройках (своя
            // навигация) прокрутку игнорируем.
            Chunk::Event(Event::Mouse(mouse)) if active.is_chat() => screen.handle_mouse(mouse),
            Chunk::Event(_) => {}
        }
    }
    quit
}

/// Транслирует намерение чата в команду оркестратору (или открывает экран
/// настроек/списка чатов). Возвращает `true` для [`ChatIntent::Quit`].
fn dispatch(
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
                *active = ActiveScreen::Settings(Box::new(SettingsScreen::new(config, profiles)));
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
fn dispatch_chat_list(
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
fn dispatch_settings(
    intent: SettingsIntent,
    cmd_tx: &UnboundedSender<AppCommand>,
    active: &mut ActiveScreen,
) {
    let command = match intent {
        SettingsIntent::Close => {
            *active = ActiveScreen::Chat;
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

/// Транслирует намерение экрана просмотра модели себя: закрытие возвращает к чату,
/// `Quit` завершает петлю (`true`). Команд оркестратору не шлёт (read-only).
fn dispatch_self_model(
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

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[cfg(windows)]
    #[test]
    fn paste_projection_matches_restores_supplementary_emoji() {
        // Буфер «hello 😊 world» при потерянном консолью эмодзи реконструируется как
        // «hello  world» (😊 > U+FFFF выпал) — проекция совпадает → берём буфер.
        assert!(paste_projection_matches("hello 😊 world", "hello  world"));
        // ❤ (U+2764) и селектор U+FE0F — BMP, проходят и остаются в реконструкции.
        assert!(paste_projection_matches("ok ❤\u{FE0F}", "ok ❤\u{FE0F}"));
        // Переводы строк нормализуются (буфер \n ↔ реконструкция \r от Enter).
        assert!(paste_projection_matches("a😊\nb", "a\rb"));
    }

    #[cfg(windows)]
    #[test]
    fn paste_projection_rejects_unrelated_clipboard() {
        // Устаревший/чужой буфер не совпадает с реконструкцией → НЕ подставляем его.
        assert!(!paste_projection_matches("совсем другое", "hello  world"));
        // Пустая реконструкция никогда не матчится (нет сигнала, что это та же вставка).
        assert!(!paste_projection_matches("😊", ""));
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

    #[test]
    fn open_self_model_requests_snapshot_without_opening() {
        // `OpenSelfModel` шлёт запрос оркестратору и НЕ открывает экран сразу —
        // он откроется по ответному событию `SelfModelView`.
        let screen = ChatScreen::new();
        let mut active = ActiveScreen::Chat;
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let quit = dispatch(ChatIntent::OpenSelfModel, &cmd_tx, &screen, &mut active);
        assert!(!quit);
        assert!(matches!(active, ActiveScreen::Chat));
        assert!(matches!(
            cmd_rx.try_recv(),
            Ok(AppCommand::RequestSelfModel)
        ));
    }

    #[test]
    fn self_model_view_event_opens_screen() {
        let mut screen = ChatScreen::new();
        let mut active = ActiveScreen::Chat;
        let mut clip = None;
        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut m = crate::entities::self_model::SelfModel::new(uuid::Uuid::new_v4());
        m.summary = "о себе".into();
        apply_event(
            &mut screen,
            &mut active,
            &mut clip,
            &cmd_tx,
            AppEvent::SelfModelView(Box::new(Some(m))),
        );
        assert!(matches!(active, ActiveScreen::SelfModel(_)));
    }

    #[test]
    fn self_model_changed_refreshes_open_screen_only() {
        let mut screen = ChatScreen::new();
        let mut clip = None;
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel();

        // Экран `F3` закрыт → SelfModelChanged ничего не шлёт.
        let mut active = ActiveScreen::Chat;
        apply_event(
            &mut screen,
            &mut active,
            &mut clip,
            &cmd_tx,
            AppEvent::SelfModelChanged,
        );
        assert!(cmd_rx.try_recv().is_err());

        // Экран `F3` открыт → перезапрос снимка.
        let mut active =
            ActiveScreen::SelfModel(Box::new(SelfModelScreen::new(None, screen.palette())));
        apply_event(
            &mut screen,
            &mut active,
            &mut clip,
            &cmd_tx,
            AppEvent::SelfModelChanged,
        );
        assert!(matches!(
            cmd_rx.try_recv(),
            Ok(AppCommand::RequestSelfModel)
        ));

        // BackgroundTask ставит флаг индикатора у экрана чата (не паникует).
        apply_event(
            &mut screen,
            &mut active,
            &mut clip,
            &cmd_tx,
            AppEvent::BackgroundTask {
                kind: BackgroundKind::Reflection,
                active: true,
            },
        );
    }
}
