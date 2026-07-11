//! Runtime — батчинг ввода и вставка из буфера (Windows-путь): коалесинг клавиш, Chunk, сверка с буфером. Часть модуля [`super`]; разбито из монолита
//! runtime.rs (см. docs/history/refactoring-god-objects.md, этап 7).

use super::*;

/// Порог «всплеска» событий за один дренаж, при котором подозреваем вставку и
/// добираем её хвост (см. `run_loop`). Человек не набирает столько за один
/// zero-timeout-дренаж — несколько событий разом означают вставку.
pub(super) const PASTE_BURST: usize = 2;

/// Пауза-детектор конца вставки. На Windows крупная вставка приходит несколькими
/// порциями через консольный буфер, и петля дренирует их за разные итерации — на
/// стыке порций серия символов рвалась бы, а одиночный `Enter` на стыке уезжал бы
/// как отправка. Пока события идут с зазором < этого порога — считаем их одной
/// вставкой; реальный ввод человека имеет паузы много больше (>100мс реакции).
pub(super) const PASTE_GAP: Duration = Duration::from_millis(20);

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
pub(super) fn reconcile_paste(
    reconstructed: String,
    clipboard: &mut Option<arboard::Clipboard>,
) -> String {
    match read_clipboard_text(clipboard) {
        Some(clip) if paste_projection_matches(&clip, &reconstructed) => clip,
        _ => reconstructed,
    }
}

/// На не-Windows вставка приходит корректным UTF-8 (`Event::Paste`) — сверка не нужна.
#[cfg(not(windows))]
pub(super) fn reconcile_paste(
    reconstructed: String,
    _clipboard: &mut Option<arboard::Clipboard>,
) -> String {
    reconstructed
}

/// Совпадает ли буфер обмена с реконструкцией вставки по «BMP-проекции» (чистая
/// функция, тестируема без буфера/терминала). Из буфера выбрасываются символы, которые
/// консоль Windows теряет (кодпойнты > U+FFFF — суррогатные пары), затем обе стороны
/// нормализуются по переводам строк/табам (как [`InputBox::insert_str`]). См.
/// [`reconcile_paste`].
#[cfg(windows)]
pub(super) fn paste_projection_matches(clipboard: &str, reconstructed: &str) -> bool {
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

/// Кладёт событие в пачку, отбрасывая key-события «отпускания»/повтора: приложение
/// их и так игнорирует (`handle_key` берёт только `Press`), а при коалесинге вставки
/// они разрывали бы серию символов между нажатиями (на Windows вставка идёт как
/// пары down/up). Мышь/ресайз/`Paste` пропускаются как есть.
pub(super) fn collect_press(batch: &mut Vec<Event>, ev: Event) {
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
pub(super) fn paste_char(key: &KeyEvent) -> Option<char> {
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
pub(super) enum Chunk {
    Paste(String),
    Event(Event),
}

/// Разбивает пачку событий на куски, коалесируя подряд идущие текстовые клавиши
/// (см. [`paste_char`]) в одну вставку, если их ≥2. Чистая функция — тестируема без
/// экрана/терминала. Серия длиной 1 (обычный ввод символа / одиночный Enter)
/// остаётся одиночным событием, чтобы Enter работал как отправка.
pub(super) fn chunk_batch(batch: Vec<Event>) -> Vec<Chunk> {
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
pub(super) fn process_input_batch(
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
                // `Chunk::Paste` — реконструкция из key-событий (Windows); сверяем её с
                // буфером, чтобы восстановить эмодзи (см. [`reconcile_paste`]).
                // Маршрутизацию по экранам держит `ActiveScreen::handle_paste`.
                let text = reconcile_paste(text, clipboard);
                active.handle_paste(screen, &text);
            }
            Chunk::Event(Event::Key(key)) => {
                // Снимаем намерение из активного экрана (борроу заканчивается на
                // owned-значении `AnyIntent`), затем диспетчеризуем одним владением —
                // иначе конфликт заимствований `active`/`screen`.
                let intent = match active {
                    ActiveScreen::Chat => screen.handle_key(key).map(AnyIntent::Chat),
                    ActiveScreen::ChatList(list) => list.handle_key(key).map(AnyIntent::List),
                    ActiveScreen::Settings(settings) => {
                        settings.handle_key(key).map(AnyIntent::Settings)
                    }
                    ActiveScreen::SelfModel(view) => view.handle_key(key).map(AnyIntent::SelfModel),
                };
                match intent {
                    // Копирование выделения в буфер обмена (`Ctrl+C`/`Ctrl+X`) —
                    // side-effect UI-слоя: текст уже у нас, в оркестратор не идём.
                    // Слот `arboard` есть тут (в `dispatch` его нет). Успех молчалив,
                    // сбой (headless-Linux без X11) — заметкой в ленту.
                    Some(AnyIntent::Chat(ChatIntent::CopyToClipboard(text))) => {
                        if let Err(e) = write_clipboard(clipboard, &text) {
                            screen
                                .push_error(&format!("Не удалось скопировать в буфер обмена: {e}"));
                        }
                    }
                    Some(intent) => {
                        if dispatch_any(intent, cmd_tx, screen, active) {
                            quit = true;
                        }
                    }
                    None => {}
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
