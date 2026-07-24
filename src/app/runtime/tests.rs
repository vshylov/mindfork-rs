//! Runtime tests (input batching, chunk_batch). See mod.rs.

use super::*;

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

#[cfg(windows)]
#[test]
fn paste_projection_matches_restores_supplementary_emoji() {
    // The clipboard "hello 😊 world", with the console's lost emoji, reconstructs as
    // "hello  world" (😊 > U+FFFF dropped out) — the projection matches → we take the clipboard.
    assert!(paste_projection_matches("hello 😊 world", "hello  world"));
    // ❤ (U+2764) and the U+FE0F selector — BMP, pass through and stay in the reconstruction.
    assert!(paste_projection_matches("ok ❤\u{FE0F}", "ok ❤\u{FE0F}"));
    // Line breaks are normalized (the clipboard's \n ↔ the reconstruction's \r from Enter).
    assert!(paste_projection_matches("a😊\nb", "a\rb"));
}

#[cfg(windows)]
#[test]
fn paste_projection_rejects_unrelated_clipboard() {
    // A stale/unrelated clipboard doesn't match the reconstruction → we DON'T substitute it.
    assert!(!paste_projection_matches("совсем другое", "hello  world"));
    // An empty reconstruction never matches (no signal that it's the same paste).
    assert!(!paste_projection_matches("😊", ""));
}

#[test]
fn chunk_batch_coalesces_text_run_with_newline() {
    // A run "a Enter b" (as a paste of "a\nb" would arrive on Windows) → one paste;
    // Enter inside the run becomes a line break ('\r' collapses on insertion).
    let batch = vec![
        key(KeyCode::Char('a')),
        key(KeyCode::Enter),
        key(KeyCode::Char('b')),
    ];
    let chunks = chunk_batch(batch);
    assert_eq!(chunks.len(), 1);
    match &chunks[0] {
        Chunk::Paste(s) => assert_eq!(s, "a\rb"),
        _ => panic!("expected a coalesced paste"),
    }
}

#[test]
fn chunk_batch_keeps_multiple_newlines_in_one_paste() {
    // A run with several Enters inside (a multiline paste) → one paste,
    // every Enter is a line break, none of them slips through as a send.
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
    // A single Enter is a send, NOT a paste.
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
    // An arrow breaks the run: "ab" (a paste) + an arrow (an event) + "cd" (a paste).
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
    // Ctrl/Alt combinations aren't part of a paste (Ctrl+V, Alt+… are shortcuts).
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
    // A key "release" is dropped (otherwise it would break the paste run).
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
    let mut loader = SpellLoader::new(dir.path().to_path_buf(), None, dir.path().join("p.txt"));

    loader.maybe_reload(true, &[]);
    assert_eq!(loader.generation, 1);
    loader.maybe_reload(true, &[]); // the same settings — no reload
    assert_eq!(loader.generation, 1);
    loader.maybe_reload(false, &[]); // turned off — a reload
    assert_eq!(loader.generation, 2);
    loader.maybe_reload(true, &["en_US".to_string()]); // a different selection — a reload
    assert_eq!(loader.generation, 3);

    // Wait for the last load's ready checker (an empty directory → disabled).
    let mut got = None;
    for _ in 0..300 {
        if let Some(c) = loader.poll() {
            got = Some(c);
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(got.is_some(), "the background load didn't finish");
    assert!(!got.unwrap().is_enabled());
}

#[test]
fn open_self_model_requests_snapshot_without_opening() {
    // `OpenSelfModel` sends a request to the orchestrator and does NOT open the screen right away —
    // it opens on the reply `SelfModelView` event.
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

    // The `F3` screen is closed → SelfModelChanged sends nothing.
    let mut active = ActiveScreen::Chat;
    apply_event(
        &mut screen,
        &mut active,
        &mut clip,
        &cmd_tx,
        AppEvent::SelfModelChanged,
    );
    assert!(cmd_rx.try_recv().is_err());

    // The `F3` screen is open → re-request the snapshot.
    let mut active = ActiveScreen::SelfModel(Box::new(SelfModelScreen::new(
        None,
        screen.palette(),
        screen.loc(),
    )));
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

    // BackgroundTask sets the indicator flag on the chat screen (doesn't panic).
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
