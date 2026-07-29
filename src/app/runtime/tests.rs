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
    let mut back = None;
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let quit = dispatch(
        ChatIntent::OpenSelfModel,
        &cmd_tx,
        &screen,
        &mut active,
        &mut back,
    );
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
    let mut back = None;
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut m = crate::entities::self_model::SelfModel::new(uuid::Uuid::new_v4());
    m.summary = "о себе".into();
    apply_event(
        &mut screen,
        &mut active,
        &mut back,
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
    let mut back = None;
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel();

    // The `F3` screen is closed → SelfModelChanged sends nothing.
    let mut active = ActiveScreen::Chat;
    apply_event(
        &mut screen,
        &mut active,
        &mut back,
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
        &mut back,
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
        &mut back,
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
    let mut back = None;
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
        &mut back,
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
        &mut back,
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
    let mut back = None;
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
        &mut back,
        &mut clip,
        &cmd_tx,
        message_results(),
    );
    assert!(matches!(active, ActiveScreen::Search(_)));

    // A later reply refreshes it in place rather than stacking a second one.
    apply_event(
        &mut screen,
        &mut active,
        &mut back,
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
    let mut back = None;
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut active = ActiveScreen::Chat;
    apply_event(
        &mut screen,
        &mut active,
        &mut back,
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
                        &mut active,
                        &mut back,
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
    let mut back = None;
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut active = ActiveScreen::Chat;
    apply_event(
        &mut screen,
        &mut active,
        &mut back,
        &mut clip,
        &cmd_tx,
        message_results(),
    );

    let quit = dispatch_any(
        AnyIntent::Search(crate::screens::search::SearchIntent::Close),
        &cmd_tx,
        &mut screen,
        &mut active,
        &mut back,
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
    let mut back = None;
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut active = ActiveScreen::Chat;
    apply_event(
        &mut screen,
        &mut active,
        &mut back,
        &mut clip,
        &cmd_tx,
        message_results(),
    );
    assert!(matches!(active, ActiveScreen::Search(_)));

    // A late self-model snapshot must not replace it.
    apply_event(
        &mut screen,
        &mut active,
        &mut back,
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
        &mut back,
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
    let mut back = None;
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut active = ActiveScreen::SelfModel(Box::new(SelfModelScreen::new(
        None,
        screen.palette(),
        screen.loc(),
    )));
    apply_event(
        &mut screen,
        &mut active,
        &mut back,
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

// ---- the way back from a chat opened out of the search results ----

/// A results snapshot with **known** ids, so a test can activate the very chat a
/// jump targets — and a different one.
#[cfg(test)]
fn message_results_for(chat: uuid::Uuid, messages: &[uuid::Uuid]) -> AppEvent {
    use crate::features::chat_search::{SearchGroup, SearchHit, build_snippet};
    AppEvent::MessageSearchResults {
        query: "маркер".into(),
        groups: vec![SearchGroup {
            chat_id: chat,
            title: "Найденный чат".into(),
            hits: messages
                .iter()
                .map(|id| SearchHit {
                    message_id: *id,
                    role: "user".into(),
                    ts: "2026-07-29T10:00:00+00:00".into(),
                    snippet: build_snippet("сообщение про маркер", "маркер", 160),
                })
                .collect(),
        }],
        total: messages.len(),
    }
}

/// What the orchestrator answers with, whatever route opened the chat.
#[cfg(test)]
fn chat_activated(id: uuid::Uuid) -> AppEvent {
    AppEvent::ChatActivated {
        id,
        title: "чат".into(),
        messages: Vec::new(),
        draft: String::new(),
        focus: None,
    }
}

/// Opens the results, moves the selection down one, and opens that hit — the
/// real path, key presses included. Returns the chat it jumped into.
#[cfg(test)]
fn jump_to_second_hit(
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
    back: &mut Option<SearchReturn>,
    clip: &mut Option<arboard::Clipboard>,
    cmd_tx: &UnboundedSender<AppCommand>,
) -> uuid::Uuid {
    let chat = uuid::Uuid::new_v4();
    let hits = [uuid::Uuid::new_v4(), uuid::Uuid::new_v4()];
    apply_event(
        screen,
        active,
        back,
        clip,
        cmd_tx,
        message_results_for(chat, &hits),
    );
    let intent = match active {
        ActiveScreen::Search(search) => {
            search.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
            search.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        }
        _ => panic!("the results must be open"),
    };
    assert!(
        matches!(intent, Some(SearchIntent::OpenHit { .. })),
        "expected OpenHit, got {intent:?}"
    );
    dispatch_any(
        AnyIntent::Search(intent.unwrap()),
        cmd_tx,
        screen,
        active,
        back,
    );
    chat
}

/// Opening a hit **stashes the live screen** rather than dropping it: the
/// selection and scroll are what makes coming back worth anything, and
/// re-running the query would lose both.
#[test]
fn opening_a_hit_stashes_the_live_results_for_the_way_back() {
    let mut screen = ChatScreen::new();
    let mut clip = None;
    let mut back = None;
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut active = ActiveScreen::Chat;

    let chat = jump_to_second_hit(&mut screen, &mut active, &mut back, &mut clip, &cmd_tx);

    assert!(matches!(active, ActiveScreen::Chat), "the results give way");
    let ret = back.as_ref().expect("the results must be stashed");
    assert_eq!(ret.chat, chat, "the stash remembers where the jump landed");
    assert_eq!(ret.screen.selected(), 1, "with the selection intact");
}

/// The defect this fixes: `Esc` in a chat reached from a hit goes **one step
/// back — to the results**, selection and all, so the user can keep working
/// through the hits. The *next* `Esc` goes on to the chat list, as it always
/// did.
#[test]
fn esc_after_a_jump_returns_to_the_results_then_on_to_the_chat_list() {
    let mut screen = ChatScreen::new();
    let mut clip = None;
    let mut back = None;
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut active = ActiveScreen::Chat;

    let chat = jump_to_second_hit(&mut screen, &mut active, &mut back, &mut clip, &cmd_tx);
    // The activation the jump itself asked for — it must not read as leaving.
    apply_event(
        &mut screen,
        &mut active,
        &mut back,
        &mut clip,
        &cmd_tx,
        chat_activated(chat),
    );
    assert!(
        back.is_some(),
        "the jump's own activation kept the way back"
    );

    dispatch(
        ChatIntent::OpenChatList,
        &cmd_tx,
        &screen,
        &mut active,
        &mut back,
    );
    match &active {
        ActiveScreen::Search(search) => assert_eq!(
            search.selected(),
            1,
            "the same results came back, not a fresh search"
        ),
        _ => panic!("expected the results screen"),
    }
    assert!(
        back.is_none(),
        "the way back is one step deep, and consumed"
    );

    // And the second `Esc` behaves exactly as it did before.
    dispatch_any(
        AnyIntent::Search(SearchIntent::Close),
        &cmd_tx,
        &mut screen,
        &mut active,
        &mut back,
    );
    assert!(matches!(active, ActiveScreen::ChatList(_)));
}

/// `Esc` resolves against the stash, not against how the chat was reached — the
/// chat screen cannot know either (FSD), it only ever says "go back".
#[test]
fn esc_honours_a_stashed_result_screen() {
    let screen = ChatScreen::new();
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut active = ActiveScreen::Chat;
    let mut back = Some(SearchReturn {
        screen: Box::new(SearchScreen::new(
            "маркер".into(),
            Vec::new(),
            0,
            screen.palette(),
            screen.loc(),
        )),
        chat: uuid::Uuid::new_v4(),
    });

    dispatch(
        ChatIntent::OpenChatList,
        &cmd_tx,
        &screen,
        &mut active,
        &mut back,
    );
    assert!(matches!(active, ActiveScreen::Search(_)));
    assert!(back.is_none());
}

/// A chat reached the ordinary way has nothing behind it — `Esc` opens the chat
/// list, unchanged.
#[test]
fn esc_in_a_chat_reached_normally_opens_the_chat_list() {
    let screen = ChatScreen::new();
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut active = ActiveScreen::Chat;
    let mut back = None;

    dispatch(
        ChatIntent::OpenChatList,
        &cmd_tx,
        &screen,
        &mut active,
        &mut back,
    );
    assert!(matches!(active, ActiveScreen::ChatList(_)));
}

/// The whole risk of the feature: a stale stash must never resurrect results for
/// a chat the user reached another way. Two routes, both funnelling through
/// `ChatActivated`.
#[test]
fn a_stale_return_is_forgotten_when_another_chat_is_opened() {
    // Route 1 — picking a chat in the list (`Enter`).
    let mut screen = ChatScreen::new();
    let mut clip = None;
    let mut back = None;
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut active = ActiveScreen::Chat;
    jump_to_second_hit(&mut screen, &mut active, &mut back, &mut clip, &cmd_tx);
    let other = uuid::Uuid::new_v4();
    dispatch_chat_list(
        ChatListIntent::Switch(other),
        &cmd_tx,
        &mut screen,
        &mut active,
    );
    apply_event(
        &mut screen,
        &mut active,
        &mut back,
        &mut clip,
        &cmd_tx,
        chat_activated(other),
    );
    assert!(
        back.is_none(),
        "another chat means the results are behind us"
    );
    dispatch(
        ChatIntent::OpenChatList,
        &cmd_tx,
        &screen,
        &mut active,
        &mut back,
    );
    assert!(
        matches!(active, ActiveScreen::ChatList(_)),
        "`Esc` must not resurrect stale results"
    );

    // Route 2 — `Ctrl+N` from the chat (a brand-new chat is still another chat).
    let mut active = ActiveScreen::Chat;
    let mut back = None;
    jump_to_second_hit(&mut screen, &mut active, &mut back, &mut clip, &cmd_tx);
    let intent = screen.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
    assert_eq!(intent, Some(ChatIntent::NewChat { profile_id: None }));
    dispatch(intent.unwrap(), &cmd_tx, &screen, &mut active, &mut back);
    let fresh = uuid::Uuid::new_v4();
    apply_event(
        &mut screen,
        &mut active,
        &mut back,
        &mut clip,
        &cmd_tx,
        chat_activated(fresh),
    );
    assert!(back.is_none());
    dispatch(
        ChatIntent::OpenChatList,
        &cmd_tx,
        &screen,
        &mut active,
        &mut back,
    );
    assert!(matches!(active, ActiveScreen::ChatList(_)));
}

/// The counterpart that makes the "a *different* chat" test meaningful:
/// re-activating the **same** chat (regeneration, deleting an exchange, a repeat
/// jump) rebuilds the feed without leaving it, so the way back survives.
#[test]
fn re_activating_the_same_chat_keeps_the_way_back() {
    let mut screen = ChatScreen::new();
    let mut clip = None;
    let mut back = None;
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut active = ActiveScreen::Chat;

    let chat = jump_to_second_hit(&mut screen, &mut active, &mut back, &mut clip, &cmd_tx);
    for _ in 0..2 {
        apply_event(
            &mut screen,
            &mut active,
            &mut back,
            &mut clip,
            &cmd_tx,
            chat_activated(chat),
        );
    }
    assert!(back.is_some(), "a feed rebuild is not leaving the chat");
}

/// The status bar is the only on-screen answer to "where does `Esc` go", and
/// this is the flow where getting it wrong surprised someone. The hint is
/// **derived** from the back-stack every frame, so it cannot drift from the key:
/// stashed → the results label, cleared → the chats label.
#[test]
fn the_status_bar_esc_hint_follows_the_stashed_results() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let chats =
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru).t("ui.status.hotkey.chats");
    let results =
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru).t("ui.status.hotkey.results");

    let mut screen = ChatScreen::new();
    let mut clip = None;
    let mut back = None;
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut active = ActiveScreen::Chat;

    // What the loop does before every frame.
    let bar = |screen: &mut ChatScreen, back: &Option<SearchReturn>| {
        screen.set_esc_target(esc_target(back));
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        term.draw(|f| screen.render(f)).unwrap();
        format!("{:?}", term.backend().buffer())
    };

    jump_to_second_hit(&mut screen, &mut active, &mut back, &mut clip, &cmd_tx);
    let dump = bar(&mut screen, &back);
    assert!(dump.contains(results), "the way back must be advertised");
    assert!(!dump.contains(chats), "{dump}");

    // Leaving for another chat clears the stash — and the hint follows.
    let other = uuid::Uuid::new_v4();
    apply_event(
        &mut screen,
        &mut active,
        &mut back,
        &mut clip,
        &cmd_tx,
        chat_activated(other),
    );
    let dump = bar(&mut screen, &back);
    assert!(dump.contains(chats), "{dump}");
    assert!(
        !dump.contains(results),
        "a stale hint would point at results that are gone"
    );
}
