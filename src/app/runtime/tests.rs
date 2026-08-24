//! Runtime tests (input batching, chunk_batch). See mod.rs.

use super::*;
use crate::entities::chat::FeedView;
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
    crate::entities::chat::ChatSummary::fixture(title)
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
            parent: None,
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
            secrets_present: Vec::new(),
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
        FeedView::default(),
        None,
        None,
    );
    let mut term = Terminal::new(TestBackend::new(60, 12)).unwrap();
    term.draw(|f| screen.render(f)).unwrap();
    let dump = format!("{:?}", term.backend().buffer());
    assert!(dump.contains("GAIA"), "{dump}");
}

/// The transcript's persona bubble is built by `activate_chat` from the child
/// view, so the view must be applied **before** it (its documented contract).
/// The screen-level tests call the pair by hand in the right order — only an
/// event-level test can catch the dispatch applying them backwards: a
/// transcript opening without its persona, and the chat opened *next*
/// inheriting one that is not its own.
#[test]
fn a_transcript_opens_with_its_persona_and_the_next_chat_does_not_inherit_it() {
    use crate::app::events::ChildView;
    use crate::entities::message::Message;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut screen = ChatScreen::new();
    let mut clip = None;
    let mut back = None;
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut active = ActiveScreen::Chat;
    let dump = |screen: &mut ChatScreen| {
        let mut term = Terminal::new(TestBackend::new(60, 20)).unwrap();
        term.draw(|f| screen.render(f)).unwrap();
        format!("{:?}", term.backend().buffer())
    };

    // Opening a transcript straight from a chat (the view was `None` so far).
    let parent = uuid::Uuid::new_v4();
    apply_event(
        &mut screen,
        &mut active,
        &mut back,
        &mut clip,
        &cmd_tx,
        AppEvent::ChatActivated {
            id: uuid::Uuid::new_v4(),
            title: "Critic".into(),
            messages: vec![Message::user("the task")],
            draft: String::new(),
            feed_view: FeedView::default(),
            focus: None,
            compaction: None,
            child: Some(ChildView {
                parent,
                parent_title: "Space".into(),
                system_message: "Critic persona".into(),
            }),
            live_turn: None,
        },
    );
    assert!(screen.read_only());
    // The persona text alone, not the role header: the header is a locale
    // string, and how the bubble is headed is the screen tests' business —
    // this test is only about the event applying the view in time.
    let on_transcript = dump(&mut screen);
    assert!(
        on_transcript.contains("Critic persona"),
        "the persona must head the transcript's feed: {on_transcript}"
    );

    // Back to the parent: its feed must not inherit the transcript's persona.
    apply_event(
        &mut screen,
        &mut active,
        &mut back,
        &mut clip,
        &cmd_tx,
        chat_activated(parent),
    );
    assert!(!screen.read_only());
    let on_parent = dump(&mut screen);
    assert!(
        !on_parent.contains("Critic persona"),
        "a chat never shows a system bubble: {on_parent}"
    );
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
            parent: None,
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
        feed_view: FeedView::default(),
        focus: None,
        compaction: None,
        child: None,
        live_turn: None,
    }
}

/// Opens the results, moves the selection down one, and opens that hit — the
/// real path, key presses included. Returns the chat it jumped into.
#[cfg(test)]
fn jump_to_second_hit(
    screen: &mut ChatScreen,
    active: &mut ActiveScreen,
    back: &mut Option<Back>,
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
    assert_eq!(
        ret.chat(),
        chat,
        "the stash remembers where the jump landed"
    );
    match ret {
        Back::Search { screen, .. } => {
            assert_eq!(screen.selected(), 1, "with the selection intact")
        }
        Back::Link { .. } => panic!("a hit stashes the results, not a chat"),
    }
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
    let mut back = Some(Back::Search {
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
    let bar = |screen: &mut ChatScreen, back: &Option<Back>| {
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

// ---------- following a `chat://` reference (spec §11.3, fork F6) ----------

/// The runtime loop's state, owned in one place: `dispatch` and `apply_event`
/// each take four or five of these, and spelling them out per test is what made
/// the reference tests near-copies of one another (docs/lessons.md §2 — the
/// duplication gate's fifth recurrence, this time in fixtures written here).
struct Harness {
    screen: ChatScreen,
    active: ActiveScreen,
    help: HelpOverlay,
    back: Option<Back>,
    clip: Option<arboard::Clipboard>,
    cmd_tx: UnboundedSender<AppCommand>,
    cmd_rx: tokio::sync::mpsc::UnboundedReceiver<AppCommand>,
}

impl Harness {
    fn new() -> Self {
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            screen: ChatScreen::new(),
            active: ActiveScreen::Chat,
            help: HelpOverlay::new(),
            back: None,
            clip: None,
            cmd_tx,
            cmd_rx,
        }
    }

    /// One event through the input pipeline (a single-event batch); returns
    /// `true` when quitting was requested.
    fn feed(&mut self, ev: Event) -> bool {
        process_input_batch(
            vec![ev],
            &mut self.screen,
            &mut self.active,
            &mut self.help,
            &mut self.back,
            &self.cmd_tx,
            &mut self.clip,
        )
    }

    fn apply(&mut self, event: AppEvent) {
        apply_event(
            &mut self.screen,
            &mut self.active,
            &mut self.back,
            &mut self.clip,
            &self.cmd_tx,
            event,
        );
    }

    fn dispatch(&mut self, intent: ChatIntent) {
        dispatch(
            intent,
            &self.cmd_tx,
            &self.screen,
            &mut self.active,
            &mut self.back,
        );
    }

    /// The next command the loop sent, if any (the queue is drained as read).
    fn next_command(&mut self) -> Option<AppCommand> {
        self.cmd_rx.try_recv().ok()
    }

    fn drain_commands(&mut self) {
        while self.cmd_rx.try_recv().is_ok() {}
    }

    /// Opens the results and jumps into the second hit, then applies the
    /// activation that jump asks for. Returns the chat it landed in.
    fn arrive_from_a_search_hit(&mut self) -> uuid::Uuid {
        let chat = jump_to_second_hit(
            &mut self.screen,
            &mut self.active,
            &mut self.back,
            &mut self.clip,
            &self.cmd_tx,
        );
        self.apply(chat_activated(chat));
        chat
    }

    /// Reads `origin`, follows a `chat://` reference to `target`, and applies
    /// the activation the switch asks for — the state every test below starts
    /// from. Returns `(origin, target)`.
    fn follow_a_reference(&mut self) -> (uuid::Uuid, uuid::Uuid) {
        let (origin, target) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        self.apply(chat_activated(origin));
        self.dispatch(ChatIntent::OpenChatLink(target));
        self.apply(chat_activated(target));
        (origin, target)
    }
}

/// `Esc` after following a reference goes back to the conversation it was
/// followed **from**, not to the chat list — the user drilled down exactly as
/// they do from a search hit. The next `Esc` goes on to the list, as it always
/// did.
#[test]
fn esc_after_following_a_reference_returns_to_the_previous_chat() {
    let mut h = Harness::new();
    let (origin, target) = h.follow_a_reference();

    assert!(
        matches!(h.next_command(), Some(AppCommand::SwitchChat(id)) if id == target),
        "the reference is followed by an ordinary switch"
    );
    assert!(
        h.back.is_some(),
        "the switch's own activation is not leaving"
    );
    assert_eq!(
        esc_target(&h.back),
        EscTarget::PreviousChat,
        "and the bar says so"
    );

    h.dispatch(ChatIntent::OpenChatList);
    assert!(
        matches!(h.next_command(), Some(AppCommand::SwitchChat(id)) if id == origin),
        "back to where the reference was followed from"
    );
    assert!(
        matches!(h.active, ActiveScreen::Chat),
        "we stay in the chat"
    );
    assert!(h.back.is_none(), "one step deep, and consumed");
    assert_eq!(esc_target(&h.back), EscTarget::ChatList);

    // The next `Esc` opens the chat list, exactly as it always did.
    h.dispatch(ChatIntent::OpenChatList);
    assert!(matches!(h.active, ActiveScreen::ChatList(_)));
}

/// Leaving by an ordinary route drops the way back — the same funnel and the
/// same rule the search half obeys.
#[test]
fn an_ordinary_chat_switch_drops_the_reference_way_back() {
    let mut h = Harness::new();
    let (_, target) = h.follow_a_reference();

    // Re-activating the *same* chat (a regeneration, `Ctrl+E`) is not leaving.
    h.apply(chat_activated(target));
    assert!(h.back.is_some());

    // Picking a third chat in the list is.
    h.apply(chat_activated(uuid::Uuid::new_v4()));
    assert!(h.back.is_none(), "no longer where we came from");
}

/// Following a reference out of a chat opened from a search hit replaces the
/// stash rather than keeping both: the most recent step down is the one `Esc`
/// undoes. Before this the hits were simply discarded by the switch, so this is
/// strictly more than was there.
#[test]
fn following_a_reference_replaces_a_stashed_result_screen() {
    let mut h = Harness::new();
    let from_hit = h.arrive_from_a_search_hit();
    assert!(matches!(h.back, Some(Back::Search { .. })));
    h.drain_commands();

    let target = uuid::Uuid::new_v4();
    h.dispatch(ChatIntent::OpenChatLink(target));
    match &h.back {
        Some(Back::Link { origin, chat }) => {
            assert_eq!(*origin, from_hit, "back to the chat the hit opened");
            assert_eq!(*chat, target);
        }
        _ => panic!("the reference owns the way back now"),
    }
}

/// A way back is for someone *looking* at the chat they drilled into. Working
/// in it — sending, regenerating, taking back an exchange, compacting,
/// attaching a file — means they have arrived, and `Esc` goes back to meaning
/// "the chat list".
#[test]
fn working_in_the_chat_you_arrived_at_drops_the_way_back() {
    for intent in [
        ChatIntent::Send("привет".into()),
        ChatIntent::RegenerateLast,
        ChatIntent::DeleteLastExchange,
        ChatIntent::Impersonate {
            seed: String::new(),
        },
        ChatIntent::Compact,
        ChatIntent::FileAttach {
            path: "заметки.txt".into(),
        },
        ChatIntent::FileRemove {
            target: "#1".into(),
        },
    ] {
        let mut h = Harness::new();
        h.follow_a_reference();
        assert!(h.back.is_some(), "{intent:?}: still just looking");

        h.dispatch(intent.clone());
        assert!(h.back.is_none(), "{intent:?} must drop the way back");
        assert_eq!(esc_target(&h.back), EscTarget::ChatList);
    }
}

/// …and the boundary: reading and looking are not arriving. Staging an image is
/// turn-scoped and never stored, so it is on this side too. The draft is absent
/// because it never reaches `dispatch` at all — the loop polls
/// `take_dirty_draft` and sends `SetDraft` itself, so typing without sending
/// cannot drop the way back by construction.
#[test]
fn reading_and_looking_keep_the_way_back() {
    for intent in [
        ChatIntent::SetFeedView(FeedView::default()),
        ChatIntent::FileList,
        ChatIntent::ImageAttach {
            path: "снимок.png".into(),
        },
        ChatIntent::ImageList,
        ChatIntent::Cancel,
    ] {
        let mut h = Harness::new();
        h.follow_a_reference();

        h.dispatch(intent.clone());
        assert!(h.back.is_some(), "{intent:?} must keep the way back");
    }
}

/// A typed command's intent must reach the orchestrator **after** the draft
/// flush that empties the box. The loop's own flush runs at the top of the
/// *next* iteration — one step behind the intent — and every handler that read
/// `chat.draft` in between saw the spent command: `/takeback` glued it onto the
/// restored message, `/regen` re-loaded it into the box via `ChatActivated`,
/// `/clone` copied it into the clone, and `/new` left it as the old chat's
/// saved draft. The pre-dispatch flush in `handle_key_event` is the one seam
/// that closes the class (docs/history/commands-stage3.md §2); this test pins the
/// order, so re-ordering the flush after the dispatch goes red.
#[test]
fn a_command_intent_is_preceded_by_the_empty_draft_flush() {
    let mut h = Harness::new();
    h.apply(chat_activated(uuid::Uuid::new_v4()));
    h.drain_commands();

    // One event per batch: a burst of key events in a single batch is taken
    // for a paste and coalesced into text, which never sends.
    for c in "/takeback".chars() {
        h.feed(Event::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        )));
    }
    h.feed(key(KeyCode::Enter));

    let mut got = Vec::new();
    while let Some(cmd) = h.next_command() {
        got.push(cmd);
    }
    let delete = got
        .iter()
        .position(|c| matches!(c, AppCommand::DeleteLastExchange))
        .expect("the takeback command was dispatched");
    assert!(delete > 0, "nothing preceded the intent: {got:?}");
    assert!(
        matches!(&got[delete - 1], AppCommand::SetDraft(d) if d.is_empty()),
        "the empty draft flush must land right before the intent: {got:?}"
    );
}

/// The same rule holds for the search half — one back-stack, one meaning of
/// "you have arrived", so the results are not restored under someone who has
/// started working in the chat a hit opened.
#[test]
fn working_after_a_search_jump_drops_the_results_too() {
    let mut h = Harness::new();
    h.arrive_from_a_search_hit();
    assert!(matches!(h.back, Some(Back::Search { .. })));

    h.dispatch(ChatIntent::Send("продолжим здесь".into()));
    assert!(h.back.is_none());

    // `Esc` now opens the chat list, as it does from any ordinary chat.
    h.dispatch(ChatIntent::OpenChatList);
    assert!(matches!(h.active, ActiveScreen::ChatList(_)));
}

/// `F1` opens the help over any screen — routed above the active screen's
/// handler, which keeps no `F1` arm of its own — and while the dialog is open
/// the keys belong to it (spec §11.7, docs/help-hotkeys-context.md stage 2).
#[test]
fn f1_opens_the_help_over_any_screen_and_owns_the_keys() {
    let mut h = Harness::new();
    // From the chat: the remembered (default) tab, chat context.
    h.feed(key(KeyCode::F(1)));
    let state = h.help.open.as_ref().expect("the dialog opened");
    assert_eq!(
        (state.tab, state.context),
        (HelpTab::Hotkeys, HelpContext::Chat)
    );
    // While open, keys go to the dialog: a character neither closes it nor
    // lands in the chat's input box (the draft never goes dirty).
    h.feed(key(KeyCode::Char('x')));
    assert!(
        h.help.open.is_some(),
        "a stray key must not close the dialog"
    );
    assert!(
        h.screen.take_dirty_draft().is_none(),
        "input leaked under the dialog"
    );
    // Esc closes it, back to the covered screen.
    h.feed(key(KeyCode::Esc));
    assert!(h.help.open.is_none());

    // From a non-chat screen: "Shortcuts" is forced and the context is that
    // screen's — the anchor itself is consumed at render and pinned in
    // `widgets::help_dialog::tests::a_non_chat_open_lands_on_its_section`.
    h.active = ActiveScreen::SelfModel(Box::new(SelfModelScreen::new(
        None,
        Palette::default(),
        h.screen.loc(),
    )));
    h.feed(key(KeyCode::F(1)));
    let state = h.help.open.as_ref().expect("the dialog opened over F3");
    assert_eq!(
        (state.tab, state.context),
        (HelpTab::Hotkeys, HelpContext::SelfModel)
    );
}

/// The dialog remembers its tab between chat-side opens, and the quit keys
/// punch through it — the behavior the chat guaranteed while it owned the
/// dialog, now pinned at the runtime that took it over.
#[test]
fn the_help_remembers_its_tab_and_quit_punches_through() {
    let mut h = Harness::new();
    h.feed(key(KeyCode::F(1)));
    h.feed(key(KeyCode::Tab)); // Hotkeys → Commands
    h.feed(key(KeyCode::Esc));
    assert!(h.help.open.is_none());
    h.feed(key(KeyCode::F(1)));
    assert_eq!(h.help.open.as_ref().unwrap().tab, HelpTab::Commands);
    assert!(
        h.feed(Event::Key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::CONTROL
        ))),
        "Ctrl+Q must quit through the open dialog"
    );
}

/// The chat's `?` (and `/help`, which reports the same intent) reaches the
/// overlay as `ChatIntent::OpenHelp` — one dialog, opened with the chat's
/// context.
#[test]
fn the_chats_question_mark_opens_the_overlay() {
    let mut h = Harness::new();
    h.feed(key(KeyCode::Char('?')));
    let state = h.help.open.as_ref().expect("`?` opened the dialog");
    assert_eq!(state.context, HelpContext::Chat);
}

/// While the dialog is open a paste (and a mouse event — the same match arm)
/// acts on nothing: there is no paste target, and the wheel would scroll a
/// screen the dialog covers — the rule the chat applied while it owned it.
#[test]
fn a_paste_is_inert_under_the_open_help() {
    let mut h = Harness::new();
    h.feed(key(KeyCode::F(1)));
    h.feed(Event::Paste("stray".into()));
    h.feed(key(KeyCode::Esc));
    assert!(
        h.screen.take_dirty_draft().is_none(),
        "the paste leaked into the input box under the dialog"
    );
}

// ---- the composed "Shortcuts" sections (docs/help-hotkeys-context.md §6) ----
//
// The section tables live next to the key handlers they document; the app
// composes them, so the whole-tab gates live here — the one layer that can
// see every screen's table at once. The commands tab's twin gates stay in
// `widgets::help_dialog`, next to its own data.

use crate::widgets::help_dialog::{HELP_MIN_WIDTH, hotkeys_tab, render_help};

/// The `ru` locale — what the ported gates ran under while the chat owned the
/// dialog.
fn ru() -> &'static crate::shared::i18n::Locale {
    crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
}

/// Every screen owns exactly one section, and exactly one section is the
/// context-free "Everywhere" block. [`help_context`]'s exhaustive match makes
/// the compiler ask about a new screen; this closes the loop to the table.
#[test]
fn help_sections_cover_every_context() {
    for context in [
        HelpContext::Chat,
        HelpContext::ChatList,
        HelpContext::Settings,
        HelpContext::SelfModel,
        HelpContext::Changes,
        HelpContext::Search,
    ] {
        assert_eq!(
            HELP_SECTIONS
                .iter()
                .filter(|s| s.context == Some(context))
                .count(),
            1,
            "{context:?} must own exactly one section"
        );
    }
    assert_eq!(
        HELP_SECTIONS.iter().filter(|s| s.context.is_none()).count(),
        1,
        "one \"Everywhere\" section"
    );
}

/// The hotkeys half of `widgets::help_dialog`'s width gate: no row of the
/// sectioned tab — headers included — may run past the dialog in ANY bundled
/// locale at either bound of the adaptive width range (docs/lessons.md §7).
#[test]
fn the_hotkeys_sections_fit_the_dialog_in_every_locale() {
    use crate::widgets::help_dialog::HELP_MAX_WIDTH;
    let palette = Palette::default();
    for w in [HELP_MIN_WIDTH as usize, HELP_MAX_WIDTH as usize] {
        for &lang in crate::shared::i18n::Lang::ALL {
            let loc = crate::shared::i18n::locale(lang);
            let (lines, _) = hotkeys_tab(&HELP_SECTIONS, HelpContext::Chat, &palette, loc, w);
            for line in lines {
                let width: usize = line.spans.iter().map(|s| span_cols(s)).sum();
                assert!(
                    width <= w,
                    "hotkeys row is {width} columns wide, the dialog is {w} ({lang:?}): {}",
                    line.spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                );
            }
        }
    }
}

/// A span's width in terminal columns (the widget's private helper, restated
/// for the gates that moved up here with the composed sections).
fn span_cols(span: &ratatui::text::Span<'_>) -> usize {
    crate::shared::wrap::display_width(&span.content.chars().collect::<Vec<_>>())
}

/// The hotkeys half of the one-column gate: every description — and every
/// wrapped continuation — starts in the same column across ALL sections;
/// headers end in a rule, not a description, and are skipped.
#[test]
fn hotkeys_descriptions_share_one_column() {
    use crate::widgets::help_dialog::HELP_MAX_WIDTH;
    let palette = Palette::default();
    for &lang in crate::shared::i18n::Lang::ALL {
        let loc = crate::shared::i18n::locale(lang);
        let (lines, _) = hotkeys_tab(
            &HELP_SECTIONS,
            HelpContext::Chat,
            &palette,
            loc,
            HELP_MAX_WIDTH as usize,
        );
        let starts: Vec<usize> = lines
            .iter()
            .filter(|l| l.spans.iter().any(|s| !s.content.trim().is_empty()))
            .filter(|l| !l.spans.iter().any(|s| s.content.contains('─')))
            .map(|l| l.spans[..l.spans.len() - 1].iter().map(span_cols).sum())
            .collect();
        assert!(!starts.is_empty(), "no rows rendered ({lang:?})");
        assert!(
            starts.iter().all(|s| s == &starts[0]),
            "descriptions start at {starts:?} ({lang:?}) — not one column"
        );
    }
}

/// The sections' half of the opener contract (the commands tab's half lives
/// in `widgets::help_dialog`): every opener names exactly one row of its own
/// section, never the first; rendered, the tab's blanks are the leading
/// spacer, one break per later section and one per opener — and each opener's
/// break sits immediately before its row, looked up inside its own section's
/// slice, since key labels repeat across sections ("Esc", "Enter") while
/// headers do not.
#[test]
fn section_openers_open_real_rows() {
    let palette = Palette::default();
    let loc = ru();
    let is_blank = |l: &ratatui::text::Line<'_>| -> bool {
        l.spans.iter().all(|s| s.content.trim().is_empty())
    };

    for section in HELP_SECTIONS {
        assert!(
            !section.rows.is_empty(),
            "{}: an empty section",
            section.title
        );
        for opener in section.openers {
            assert_eq!(
                section.rows.iter().filter(|(k, _)| k == opener).count(),
                1,
                "{}: opener {opener:?} must name exactly one row",
                section.title
            );
        }
        assert!(
            !section.openers.contains(&section.rows[0].0),
            "{}: the first row cannot open a group",
            section.title
        );
    }

    let (lines, _) = hotkeys_tab(
        &HELP_SECTIONS,
        HelpContext::Chat,
        &palette,
        loc,
        HELP_MIN_WIDTH as usize,
    );
    let openers_total: usize = HELP_SECTIONS.iter().map(|s| s.openers.len()).sum();
    assert_eq!(
        lines.iter().filter(|l| is_blank(l)).count(),
        1 + (HELP_SECTIONS.len() - 1) + openers_total,
        "the leading spacer + section breaks + openers"
    );

    let header_at: Vec<usize> = HELP_SECTIONS
        .iter()
        .map(|s| {
            lines
                .iter()
                .position(|l| l.spans.iter().any(|sp| sp.content.trim() == loc.t(s.title)))
                .unwrap_or_else(|| panic!("{}: header is not rendered", s.title))
        })
        .collect();
    for (i, section) in HELP_SECTIONS.iter().enumerate() {
        let end = header_at.get(i + 1).copied().unwrap_or(lines.len());
        for opener in section.openers {
            let label = loc.t(opener);
            let at = lines[header_at[i]..end]
                .iter()
                .position(|l| l.spans.iter().any(|s| s.content.trim() == label))
                .map(|p| header_at[i] + p)
                .unwrap_or_else(|| panic!("{}: opener {opener:?} is not rendered", section.title));
            assert!(
                is_blank(&lines[at - 1]),
                "{}: no blank line before opener {opener:?}",
                section.title
            );
        }
    }
}

/// Fork F1: the section for the screen the dialog was opened from — and only
/// it — carries the "you are here" marker, next to its own title. Pinned for
/// two contexts so the marker provably follows the context.
#[test]
fn the_invoking_screens_section_is_marked() {
    let palette = Palette::default();
    for &lang in crate::shared::i18n::Lang::ALL {
        let loc = crate::shared::i18n::locale(lang);
        let here = loc.t("ui.help.here");
        for (context, title) in [
            (HelpContext::Chat, "ui.help.sec.chat"),
            (HelpContext::Settings, "ui.help.sec.settings"),
        ] {
            let (lines, anchor) = hotkeys_tab(
                &HELP_SECTIONS,
                context,
                &palette,
                loc,
                HELP_MIN_WIDTH as usize,
            );
            let marked: Vec<&ratatui::text::Line<'_>> = lines
                .iter()
                .filter(|l| l.spans.iter().any(|s| s.content.contains(here)))
                .collect();
            assert_eq!(marked.len(), 1, "one marker per dialog ({lang:?})");
            assert!(
                marked[0]
                    .spans
                    .iter()
                    .any(|s| s.content.trim() == loc.t(title)),
                "the marker must sit on the {title} header ({lang:?}): {:?}",
                marked[0]
            );
            // …and the anchor points exactly at that header row.
            assert!(
                std::ptr::eq(&lines[anchor], marked[0]),
                "the anchor must be the marked header's row"
            );
        }
    }
}

/// Fork F2, rendered end to end over the real sections: opened from a
/// non-chat screen the dialog lands on "Shortcuts" with that section's marked
/// header as the top content row, and the anchor is consumed once — the
/// user's own scrolling then sticks.
#[test]
fn a_non_chat_open_lands_on_its_section() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let loc = ru();
    let palette = Palette::default();
    let mut state = HelpState::open_at(HelpContext::Settings);
    let render = |state: &mut HelpState| -> String {
        let mut term = Terminal::new(TestBackend::new(90, 40)).unwrap();
        term.draw(|f| render_help(f, state, &HELP_SECTIONS, &palette, loc))
            .unwrap();
        let buf = term.backend().buffer();
        let mut out = String::new();
        for y in buf.area.top()..buf.area.bottom() {
            for x in buf.area.left()..buf.area.right() {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    };
    let text = render(&mut state);
    assert!(state.scroll > 0, "the anchor scrolled the tab");
    assert!(
        text.contains(loc.t("ui.help.sec.settings")),
        "the settings header is not visible: {text}"
    );
    assert!(
        text.contains(loc.t("ui.help.here")),
        "the marker is not visible: {text}"
    );
    assert!(
        !text.contains(loc.t("ui.help.sec.everywhere")),
        "the sections above the anchor must be scrolled past: {text}"
    );
    // One-shot: the user's scroll survives the next frame.
    state.scroll = 3;
    let _ = render(&mut state);
    assert_eq!(state.scroll, 3, "the anchor re-fired on a later render");

    // The tab shows the real sections' content — a chat row — while the
    // commands stay on their own tab (their gate lives with the widget).
    let mut top = HelpState::open(HelpTab::Hotkeys);
    let hotkeys = render(&mut top);
    assert!(
        hotkeys.contains("отправить сообщение"),
        "missing a key description: {hotkeys}"
    );
    assert!(
        !hotkeys.contains("/rag add"),
        "commands must not be on the hotkeys tab"
    );
}
