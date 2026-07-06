//! Тесты runtime (батчинг ввода, chunk_batch). См. mod.rs.

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
