//! Runtime tests (input batching, chunk_batch). See mod.rs.

use super::*;
use crate::features::chat_search_sort::SortMode;

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

/// Renders a chat-list screen and returns the buffer dump.
#[cfg(test)]
fn list_dump(list: &mut crate::screens::chat_list::ChatListScreen) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    term.draw(|f| list.render(f)).unwrap();
    format!("{:?}", term.backend().buffer())
}

#[cfg(test)]
fn summary(title: &str) -> crate::entities::chat::ChatSummary {
    crate::entities::chat::ChatSummary {
        id: uuid::Uuid::new_v4(),
        title: title.to_string(),
        created_at: chrono::Utc::now(),
        modified_at: chrono::Utc::now(),
        message_count: 0,
    }
}

#[test]
fn chat_search_results_reach_an_open_list_and_are_ignored_when_it_is_closed() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut screen = ChatScreen::new();
    let mut clip = None;
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let chats = vec![summary("Альфа"), summary("Бета")];

    // The list is open and in content mode: the results filter it.
    let mut active = ActiveScreen::ChatList(Box::new(ChatListScreen::new(
        chats.clone(),
        None,
        screen.palette(),
        screen.loc(),
    )));
    if let ActiveScreen::ChatList(list) = &mut active {
        // `Ctrl+F` also asks for a search — the other half of the round-trip.
        let intent = list.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
        assert_eq!(intent, Some(ChatListIntent::SearchContent(String::new())));
        dispatch_chat_list(intent.unwrap(), &cmd_tx, &mut screen, &mut active);
    }
    assert!(matches!(cmd_rx.try_recv(), Ok(AppCommand::SearchChats(q)) if q.is_empty()));

    apply_event(
        &mut screen,
        &mut active,
        &mut clip,
        &cmd_tx,
        AppEvent::ChatSearchResults {
            query: "нечто".into(),
            chat_ids: Some(vec![]),
        },
    );
    if let ActiveScreen::ChatList(list) = &mut active {
        let dump = list_dump(list);
        assert!(
            !dump.contains("Альфа"),
            "the list should be filtered: {dump}"
        );
    } else {
        panic!("the list must stay open");
    }

    // With the list closed the event is dropped — and, crucially, leaves no
    // residue: a list opened afterwards is unfiltered.
    let mut active = ActiveScreen::Chat;
    apply_event(
        &mut screen,
        &mut active,
        &mut clip,
        &cmd_tx,
        AppEvent::ChatSearchResults {
            query: "нечто".into(),
            chat_ids: Some(vec![]),
        },
    );
    assert!(matches!(active, ActiveScreen::Chat), "no screen was opened");

    let mut fresh = ChatListScreen::new(chats, None, screen.palette(), screen.loc());
    let dump = list_dump(&mut fresh);
    assert!(dump.contains("Альфа"), "{dump}");
}

// ---- stage 2b: the message-level search screen ----

#[cfg(test)]
fn message_results() -> AppEvent {
    use crate::features::chat_search::{SearchGroup, SearchHit, build_snippet};
    AppEvent::MessageSearchResults {
        query: "маркер".into(),
        groups: vec![SearchGroup {
            chat_id: uuid::Uuid::new_v4(),
            title: "Найденный чат".into(),
            hits: vec![SearchHit {
                message_id: uuid::Uuid::new_v4(),
                role: "user".into(),
                ts: "2026-07-29T10:00:00+00:00".into(),
                snippet: build_snippet("сообщение про маркер", "маркер", 160),
            }],
        }],
        total: 1,
    }
}

/// The full `Ctrl+G` round-trip: the chat list asks, the orchestrator answers,
/// and the reply — not the key press — is what opens the screen (the same shape
/// as `RequestSelfModel`/`SelfModelView`, because the index lives over there).
#[test]
fn ctrl_g_in_content_mode_opens_the_message_search_screen() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut screen = ChatScreen::new();
    let mut clip = None;
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut active = ActiveScreen::ChatList(Box::new(ChatListScreen::new(
        vec![summary("Альфа")],
        None,
        screen.palette(),
        screen.loc(),
    )));

    let ctrl = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
    if let ActiveScreen::ChatList(list) = &mut active {
        list.handle_key(ctrl('f')); // into content mode
        list.handle_key(KeyEvent::new(KeyCode::Char('м'), KeyModifiers::NONE));
        let intent = list.handle_key(ctrl('g'));
        assert_eq!(
            intent,
            Some(ChatListIntent::SearchMessages {
                query: "м".into(),
                sort: SortMode::Modified,
            })
        );
        dispatch_chat_list(intent.unwrap(), &cmd_tx, &mut screen, &mut active);
    }
    // The command goes out; the list is still on screen (nothing to show yet).
    let mut sent = Vec::new();
    while let Ok(c) = cmd_rx.try_recv() {
        sent.push(c);
    }
    assert!(
        sent.iter()
            .any(|c| matches!(c, AppCommand::SearchMessages { query, .. } if query == "м")),
        "{sent:?}"
    );
    assert!(matches!(active, ActiveScreen::ChatList(_)));

    // The reply opens it.
    apply_event(
        &mut screen,
        &mut active,
        &mut clip,
        &cmd_tx,
        message_results(),
    );
    assert!(matches!(active, ActiveScreen::Search(_)));

    // A later reply refreshes it in place rather than stacking a second one.
    apply_event(
        &mut screen,
        &mut active,
        &mut clip,
        &cmd_tx,
        message_results(),
    );
    assert!(matches!(active, ActiveScreen::Search(_)));
}

/// `Enter` on a hit is stage 2a's jump: the chat opens on that message and the
/// results close, exactly as `Switch` closes the chat list.
#[test]
fn opening_a_hit_jumps_to_the_message_and_leaves_the_results() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut screen = ChatScreen::new();
    let mut clip = None;
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut active = ActiveScreen::Chat;
    apply_event(
        &mut screen,
        &mut active,
        &mut clip,
        &cmd_tx,
        message_results(),
    );

    let (chat, message) = match &mut active {
        ActiveScreen::Search(search) => {
            let intent = search.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            match intent {
                Some(crate::screens::search::SearchIntent::OpenHit {
                    chat,
                    message,
                    query,
                }) => {
                    // The screen is the last place that knows which query these
                    // results answer, so the intent has to carry it (fork S3(b)).
                    assert_eq!(query, "маркер");
                    assert!(!dispatch_any(
                        AnyIntent::Search(crate::screens::search::SearchIntent::OpenHit {
                            chat,
                            message,
                            query
                        }),
                        &cmd_tx,
                        &mut screen,
                        &mut active
                    ));
                    (chat, message)
                }
                other => panic!("expected OpenHit, got {other:?}"),
            }
        }
        _ => panic!("the screen must be open"),
    };

    assert!(matches!(active, ActiveScreen::Chat), "the results close");
    assert!(matches!(
        cmd_rx.try_recv(),
        Ok(AppCommand::OpenChatAt { chat: c, message: m, query })
            if c == chat && m == message && query == "маркер"
    ));
}

/// `Esc` goes back to the chat list **still searching for the same query** —
/// dropping the user into a blank title-mode list would throw away the search
/// that got them here. The results are refetched by the usual round-trip.
#[test]
fn esc_from_the_results_returns_to_the_list_still_searching() {
    let mut screen = ChatScreen::new();
    let mut clip = None;
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut active = ActiveScreen::Chat;
    apply_event(
        &mut screen,
        &mut active,
        &mut clip,
        &cmd_tx,
        message_results(),
    );

    let quit = dispatch_any(
        AnyIntent::Search(crate::screens::search::SearchIntent::Close),
        &cmd_tx,
        &mut screen,
        &mut active,
    );
    assert!(!quit);
    match &mut active {
        ActiveScreen::ChatList(list) => {
            let dump = list_dump(list);
            assert!(dump.contains("маркер"), "the query must come back: {dump}");
        }
        _ => panic!("expected the chat list"),
    }
    assert!(matches!(
        cmd_rx.try_recv(),
        Ok(AppCommand::SearchChats(q)) if q == "маркер"
    ));
}

/// The silent-match-site trap (docs/history/chat-search-stage2.md §1.6): a new screen
/// is only as safe as the arms that *replace* the active one. `SelfModelView`
/// is the stealer — its `_ =>` arm would swap a results list the user is
/// reading for an unrelated snapshot — and the `Settings` broadcast is the one
/// that must reach it, or the theme and UI language would never update.
#[test]
fn the_search_screen_survives_settings_and_self_model_events() {
    let mut screen = ChatScreen::new();
    let mut clip = None;
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut active = ActiveScreen::Chat;
    apply_event(
        &mut screen,
        &mut active,
        &mut clip,
        &cmd_tx,
        message_results(),
    );
    assert!(matches!(active, ActiveScreen::Search(_)));

    // A late self-model snapshot must not replace it.
    apply_event(
        &mut screen,
        &mut active,
        &mut clip,
        &cmd_tx,
        AppEvent::SelfModelView(Box::new(None)),
    );
    assert!(
        matches!(active, ActiveScreen::Search(_)),
        "the results were stolen by an unrelated event"
    );

    // The settings broadcast reaches it: switching the UI language to English
    // must be visible on the screen.
    let mut config = crate::shared::config::AppConfig::default();
    config.interface.language = crate::shared::i18n::Lang::En;
    apply_event(
        &mut screen,
        &mut active,
        &mut clip,
        &cmd_tx,
        AppEvent::Settings {
            config: Box::new(config),
            profiles: Vec::new(),
            language_locked: Vec::new(),
            mcp: Default::default(),
            api_keys_present: Vec::new(),
        },
    );
    match &mut active {
        ActiveScreen::Search(search) => {
            use ratatui::Terminal;
            use ratatui::backend::TestBackend;
            let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
            term.draw(|f| search.render(f)).unwrap();
            let dump = format!("{:?}", term.backend().buffer());
            assert!(
                dump.contains("Matching messages"),
                "the locale broadcast must reach the screen: {dump}"
            );
        }
        _ => panic!("the screen must still be open"),
    }
}

/// The names reach the chat screen's feed even while another screen is on top
/// (the event is applied to the chat unconditionally, like `ChatList`).
#[test]
fn character_names_event_reaches_the_feed() {
    use crate::entities::message::Message;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut screen = ChatScreen::new();
    let mut clip = None;
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
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
        AppEvent::CharacterNames(crate::entities::profile::CharacterNames {
            user: "Gaia".into(),
            assistant: String::new(),
            system: String::new(),
        }),
    );

    screen.activate_chat(
        uuid::Uuid::new_v4(),
        "chat".into(),
        &[Message::user("hi")],
        "",
        None,
    );
    let mut term = Terminal::new(TestBackend::new(60, 12)).unwrap();
    term.draw(|f| screen.render(f)).unwrap();
    let dump = format!("{:?}", term.backend().buffer());
    assert!(dump.contains("GAIA"), "{dump}");
}
