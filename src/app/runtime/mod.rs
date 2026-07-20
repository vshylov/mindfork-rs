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
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers,
};
#[cfg(unix)]
use ratatui::crossterm::event::{
    EnableBracketedPaste, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
#[cfg(unix)]
use ratatui::crossterm::terminal::supports_keyboard_enhancement;
use ratatui::crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
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

    /// Обновляет палитру и локаль интерфейса открытого overlay-экрана (список чатов /
    /// модель себя) при смене темы/режима совместимости/языка UI. Экран настроек
    /// обновляется отдельно (`refresh` шире палитры), чат — своей базой
    /// (`ChatScreen::set_settings`). Каноничное место перечисления экранов для
    /// broadcast темы (палитра + локаль). См. docs/i18n-ui.md §3.3.
    fn set_theme(&mut self, palette: Palette, loc: &'static crate::shared::i18n::Locale) {
        match self {
            ActiveScreen::ChatList(list) => {
                list.set_palette(palette);
                list.set_loc(loc);
            }
            ActiveScreen::SelfModel(view) => {
                view.set_palette(palette);
                view.set_loc(loc);
            }
            ActiveScreen::Chat | ActiveScreen::Settings(_) => {}
        }
    }

    /// Направляет вставку из буфера в целевой экран: настройки/список/модель себя —
    /// в свои поля; базовый чат — в поле ввода. Каноничное место маршрутизации
    /// вставки по экранам. `chat` — базовый экран (нужен для варианта `Chat`).
    fn handle_paste(&mut self, chat: &mut ChatScreen, text: &str) {
        match self {
            ActiveScreen::Settings(settings) => settings.handle_paste(text),
            ActiveScreen::Chat => chat.handle_paste(text),
            ActiveScreen::ChatList(list) => list.handle_paste(text),
            ActiveScreen::SelfModel(view) => view.handle_paste(text),
        }
    }
}

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
    bundled_dict_dir: Option<PathBuf>,
    personal: PathBuf,
) -> Result<()> {
    let mut terminal = ratatui::init();
    // На unix включаем bracketed paste: crossterm отдаёт вставку из буфера ОДНИМ
    // событием `Event::Paste` (целиком, переводы строк — текстом, не Enter). На
    // Windows этого режима у crossterm нет (ввод читается через Console API), там
    // вставка приходит пачкой обычных key-событий — её собираем в петле
    // (`process_input_batch`), поэтому включать тут нечего. См. spec §11.5.
    //
    // Здесь же (unix) включаем kitty keyboard protocol на уровне «disambiguate»:
    // legacy-кодировка терминала шлёт для `Shift+Enter` и `Enter` один и тот же CR,
    // поэтому перенос строки в поле ввода на «голом» unix-терминале был недоступен.
    // С `DISAMBIGUATE_ESCAPE_CODES` терминал сообщает модификаторы у спец-клавиш
    // (Enter/стрелки/…), и `Shift+Enter` становится отличим от `Enter` (а `Shift`+
    // стрелки — от голых стрелок, что оживляет выделение с клавиатуры). Пушим только
    // если терминал поддерживает протокол (иначе no-op); снимаем на выходе и в
    // panic-hook. Печатный ввод и одиночный `Shift`+символ этот флаг не трогает
    // (текст идёт как есть), поэтому раскладко-независимый разбор Ctrl-шорткатов
    // (`shared::keys`) и ввод `?`/эмодзи не регрессируют. На Windows не нужно —
    // Console API и так сообщает модификаторы. `Alt+Enter` в поле ввода — запасной
    // перенос строки для терминалов без этого протокола (см. spec §11.5, п.11 аудита).
    #[cfg(unix)]
    {
        let _ = execute!(stdout(), EnableBracketedPaste);
        if supports_keyboard_enhancement().unwrap_or(false) {
            let _ = execute!(
                stdout(),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            );
        }
    }
    // Захват мыши по умолчанию ВЫКЛЮЧЕН: тогда работает нативное выделение текста
    // мышью. Прокрутка ленты колесом включается тумблером (`Ctrl+W`) — он шлёт
    // `EnableMouseCapture`/`DisableMouseCapture` (см. `dispatch`). Дополняем
    // panic-hook ratatui выключением мыши и bracketed paste: иначе после паники с
    // включёнными режимами терминал продолжит слать escape-коды в шелл. Здесь же
    // снимаем синхронизированный вывод (DEC 2026, см. петлю): паника внутри
    // `terminal.draw` случается между `?2026h` и `?2026l`, и без снятия терминал
    // держал бы кадр замороженным (сообщение паники не видно) до своего таймаута.
    // DECRST невзведённого режима — no-op, лишний `?2026l` безвреден.
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(
            stdout(),
            EndSynchronizedUpdate,
            DisableMouseCapture,
            DisableBracketedPaste
        );
        // Снимаем kitty-протокол, если пушили (unix); безвредно при пустом стеке.
        #[cfg(unix)]
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
        prev_hook(info);
    }));
    let result = run_loop(
        &mut terminal,
        &cmd_tx,
        evt_rx,
        dict_dir,
        bundled_dict_dir,
        personal,
    );
    // Снимаем режимы на выходе (безвредно, если уже выключены).
    let _ = execute!(
        stdout(),
        EndSynchronizedUpdate,
        DisableMouseCapture,
        DisableBracketedPaste
    );
    #[cfg(unix)]
    let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
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
    /// Резервный каталог словарей рядом с бинарником (П1) — источник при не-портативном
    /// режиме хранения, когда словарей в корне данных нет.
    bundled_dir: Option<PathBuf>,
    personal: PathBuf,
    tx: Sender<(u64, SpellChecker)>,
    rx: Receiver<(u64, SpellChecker)>,
    /// Последние применённые настройки `(включён, словари)` (None — ещё не грузили).
    applied: Option<(bool, Vec<String>)>,
    /// Номер последней запущенной загрузки (применяем только её результат).
    generation: u64,
}

impl SpellLoader {
    fn new(dict_dir: PathBuf, bundled_dir: Option<PathBuf>, personal: PathBuf) -> Self {
        let (tx, rx) = channel();
        Self {
            dict_dir,
            bundled_dir,
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
        let (dir, bundled, personal, tx) = (
            self.dict_dir.clone(),
            self.bundled_dir.clone(),
            self.personal.clone(),
            self.tx.clone(),
        );
        let selected = selected.to_vec();
        let sel_for_thread = selected.clone();
        std::thread::spawn(move || {
            let checker = dict::load(
                &dir,
                bundled.as_deref(),
                &personal,
                enabled,
                &sel_for_thread,
            );
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
    bundled_dict_dir: Option<PathBuf>,
    personal: PathBuf,
) -> Result<()> {
    let mut screen = ChatScreen::new();
    // Поверх чата может быть открыт список чатов (Esc) или настройки (Ctrl+P).
    // События оркестратора продолжают применяться к чату (генерация не прерывается).
    let mut active = ActiveScreen::Chat;
    // Буфер обмена создаётся лениво при первом копировании (на headless-Linux без
    // X11/Wayland конструктор может упасть — тогда показываем ошибку, не паникуем).
    let mut clipboard: Option<arboard::Clipboard> = None;
    let mut spell = SpellLoader::new(dict_dir, bundled_dict_dir, personal);
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
            // Кадр обёрнут в синхронизированный вывод (DEC private mode 2026):
            // `?2026h` до отрисовки, `?2026l` после — терминал буферизует всё
            // между ними и применяет кадр АТОМАРНО. Без этого аппаратный курсор
            // был виден на промежуточных состояниях записи: ratatui пишет diff
            // при видимом курсоре (курсор терминала = позиция записи) и
            // возвращает его в поле ввода отдельными записями ПОСЛЕ diff'а
            // (`show_cursor`/`set_cursor_position` у CrosstermBackend — это
            // `execute!` с немедленным flush; крупный diff вдобавок дробится
            // маленьким буфером stdout). Windows Terminal рендерит асинхронно и
            // успевал показать курсор на последней записанной ячейке diff'а: при
            // генерации это счётчик токенов (нижние строки статус-бара пишутся
            // последними), при индексации RAG — спиннер баннера. Курсор «прыгал»
            // между полем ввода и этими ячейками с частотой кадров (~20/с).
            //
            // Терминалы без поддержки 2026 (conhost компат-режима) игнорируют
            // незнакомый приватный режим — мягкая деградация (прыжок остаётся,
            // как раньше). Ошибка draw пробрасывается ПОСЛЕ снятия режима, чтобы
            // терминал не остался в буферизации. См. spec §4.4.1.
            let _ = execute!(stdout(), BeginSynchronizedUpdate);
            let drawn = match &mut active {
                ActiveScreen::Chat => {
                    // Широкие эмодзи требуют ПОЛНОЙ перерисовки в двух случаях:
                    // прокрутка ленты со «съезжающими» VS16-кластерами (`🕸️`/`🗂️`) и
                    // работа с попапом эмодзи (`Ctrl+B`) — там глиф уходит с места, а
                    // его хвостовую ячейку diff не трогает (см.
                    // `ChatScreen::request_full_redraw`). В обоих случаях некоторые
                    // терминалы (Command Prompt/conhost)
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
                    // (`take_full_redraw` — прокрутка ленты с VS16 либо действие в
                    // попапе эмодзи); на чистом тексте прокрутка не перерисовывает
                    // всё. См. spec §11.3, §11.5.
                    if screen.take_full_redraw() {
                        for cell in terminal.current_buffer_mut().content.iter_mut() {
                            cell.set_symbol("\u{0}");
                        }
                        terminal.swap_buffers();
                    }
                    terminal.draw(|frame| screen.render(frame))
                }
                ActiveScreen::ChatList(list) => terminal.draw(|frame| list.render(frame)),
                ActiveScreen::Settings(settings) => terminal.draw(|frame| settings.render(frame)),
                ActiveScreen::SelfModel(view) => terminal.draw(|frame| view.render(frame)),
            };
            let _ = execute!(stdout(), EndSynchronizedUpdate);
            drawn?;
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

// ---------- подмодули (разбор god-object: docs/history/refactoring-god-objects.md, этап 7) ----------

mod clipboard;
mod dispatch;
mod input;

// Внутренняя проводка: run_loop зовёт батчинг ввода (input), применение событий и
// диспетчеризацию (dispatch), буфер обмена (clipboard). Внешняя поверхность — run.
use self::{clipboard::*, dispatch::*, input::*};

#[cfg(test)]
mod tests;
