//! Tests for the chat screen (via handle_key/render). See mod.rs.

use super::popups::{HELP_KEYS, KEY_GROUP_OPENERS};
use super::*;
use crate::entities::message::MessageRole;
use crate::features::chat_search::FeedFocus;

fn gen_id() -> Uuid {
    Uuid::new_v4()
}

/// A jump request onto `message` with nothing to highlight (the shape most of
/// these tests care about — they assert the scroll/marker, not the highlight).
fn focus_on(message: Uuid) -> Option<FeedFocus> {
    Some(FeedFocus {
        message,
        query: String::new(),
    })
}

/// A status snapshot with a ready chat server (embeddings/impersonation not configured).
fn ready_statuses() -> ServerStatuses {
    ServerStatuses {
        chat: ServerStatus::Ready,
        embed: ServerStatus::NotConfigured,
        impersonation: ServerStatus::NotConfigured,
    }
}

#[test]
fn chunk_after_midgen_note_goes_to_new_assistant_bubble() {
    // Regression: a note (AppEvent::Error about the round limit) mid-generation
    // used to make `last` a note, and the subsequent stream of the final reply got
    // appended into it (plain text, no markdown). Now a chunk opens a new bubble.
    let mut s = ChatScreen::new();
    let id = gen_id();
    s.push_user_message("собери отзывы".into());
    s.begin_generation(id);
    // The assistant called a tool (the assistant bubble is text-empty)...
    s.push_tool_call(id, "web_search".into(), "{}".into(), "результаты".into(), 0);
    // ...the limit is reached — a note goes into the feed.
    s.push_error("Достигнут лимит раундов инструментов (8) — свожу итог.");
    // The forced synthesis streams the final reply.
    s.push_chunk(id, "## Итог\n\n**Вывод**.");
    s.finish_generation(id, FinishReason::Stop);

    // The note is a separate element; the final text is in the assistant bubble (markdown),
    // not in the note.
    let notes: Vec<_> = s.feed.iter().filter(|m| m.role == FeedRole::Note).collect();
    assert_eq!(notes.len(), 1, "expected exactly one note about the limit");
    assert!(
        !notes[0].text.contains("## Итог"),
        "the final text must not end up in the note: {:?}",
        notes[0].text
    );
    let last = s.feed.last().unwrap();
    assert_eq!(last.role, FeedRole::Assistant);
    assert_eq!(last.text, "## Итог\n\n**Вывод**.");
    assert!(!last.streaming);
}

#[test]
fn streaming_sequence_builds_feed() {
    let mut s = ChatScreen::new();
    let id = gen_id();
    s.push_user_message("hi".into());
    s.begin_generation(id);
    s.push_thoughts(id, "hmm");
    s.push_chunk(id, "Hel");
    s.push_chunk(id, "lo");
    s.finish_generation(id, FinishReason::Stop);

    assert_eq!(s.feed.len(), 2);
    assert_eq!(s.feed[0].role, FeedRole::User);
    assert_eq!(s.feed[1].role, FeedRole::Assistant);
    assert_eq!(s.feed[1].text, "Hello");
    assert_eq!(s.feed[1].thoughts, "hmm");
    assert!(!s.feed[1].streaming);
    assert!(!s.generating);
}

#[test]
fn live_stream_with_tool_matches_reload() {
    use crate::entities::message::{Message, ToolCallRecord};

    // Live: round-1 text → a tool call → round-2 text (final).
    let mut s = ChatScreen::new();
    let id = gen_id();
    s.begin_generation(id);
    s.push_chunk(id, "Ищу погоду.");
    s.push_tool_call(
        id,
        "web_search".into(),
        "{\"q\":\"погода\"}".into(),
        "ясно".into(),
        0,
    );
    s.push_chunk(id, "Сейчас ясно.");
    s.finish_generation(id, FinishReason::Stop);

    let live = s.feed.last().unwrap().clone();
    assert_eq!(live.text, "Ищу погоду.\n\nСейчас ясно.");
    assert_eq!(live.tools.len(), 1);
    assert_eq!(live.tools[0].text_offset, "Ищу погоду.".len());

    // Reload: the same rounds as domain messages (assistant+tool / assistant).
    let mut r1 = Message::assistant("Ищу погоду.");
    r1.tool_calls = vec![ToolCallRecord {
        thought_signature: None,
        images: 0,
        id: "c1".into(),
        name: "web_search".into(),
        arguments: serde_json::json!({"q": "погода"}),
        result: Some("ясно".into()),
    }];
    let tool_msg = {
        let mut m = Message::new(MessageRole::Tool, "ясно");
        m.tool_call_id = Some("c1".into());
        m.tool_name = Some("web_search".into());
        m
    };
    let r2 = Message::assistant("Сейчас ясно.");
    let reload = FeedMessage::from_messages(&[r1, tool_msg, r2]);

    // One merged assistant block, the same text and the same call offset.
    assert_eq!(reload.len(), 1);
    assert_eq!(reload[0].text, live.text);
    assert_eq!(reload[0].tools.len(), 1);
    assert_eq!(reload[0].tools[0].text_offset, live.tools[0].text_offset);
}

#[test]
fn live_followup_makes_two_bubbles_matching_reload() {
    use crate::entities::message::{Message, ToolCallRecord};

    // Live: text 1 → followup → text 2.
    let mut s = ChatScreen::new();
    let id = gen_id();
    s.begin_generation(id);
    s.push_chunk(id, "Первое сообщение.");
    s.continue_assistant(id);
    s.push_chunk(id, "Второе сообщение.");
    s.finish_generation(id, FinishReason::Stop);

    // Two separate assistant bubbles.
    let bubbles: Vec<&FeedMessage> = s
        .feed
        .iter()
        .filter(|m| m.role == FeedRole::Assistant)
        .collect();
    assert_eq!(bubbles.len(), 2);
    assert_eq!(bubbles[0].text, "Первое сообщение.");
    assert_eq!(bubbles[1].text, "Второе сообщение.");

    // Reload: A1 (with a control call) → tool → A2 (new_bubble).
    let mut a1 = Message::assistant("Первое сообщение.");
    a1.tool_calls = vec![ToolCallRecord {
        thought_signature: None,
        images: 0,
        id: "c1".into(),
        name: "send_followup_message".into(),
        arguments: serde_json::json!({}),
        result: Some("ок".into()),
    }];
    let tool_msg = {
        let mut m = Message::new(MessageRole::Tool, "ок");
        m.tool_call_id = Some("c1".into());
        m.tool_name = Some("send_followup_message".into());
        m
    };
    let mut a2 = Message::assistant("Второе сообщение.");
    a2.new_bubble = true;
    let reload = FeedMessage::from_messages(&[a1, tool_msg, a2]);
    assert_eq!(reload.len(), 2);
    assert_eq!(reload[0].text, bubbles[0].text);
    assert_eq!(reload[1].text, bubbles[1].text);
}

#[test]
fn live_rewrite_discards_partial_text() {
    // Live: partial incorrect text → rewrite → the rewritten reply.
    let mut s = ChatScreen::new();
    let id = gen_id();
    s.begin_generation(id);
    s.push_chunk(id, "Непра");
    s.push_chunk(id, "вильный ответ.");
    s.rewrite_assistant(id);
    s.push_chunk(id, "Правильный ответ.");
    s.finish_generation(id, FinishReason::Stop);

    // Exactly one assistant bubble with the rewritten text (the partial one is discarded).
    let bubbles: Vec<&FeedMessage> = s
        .feed
        .iter()
        .filter(|m| m.role == FeedRole::Assistant)
        .collect();
    assert_eq!(bubbles.len(), 1);
    assert_eq!(bubbles[0].text, "Правильный ответ.");
}

#[test]
fn ignores_chunks_from_stale_generation() {
    let mut s = ChatScreen::new();
    let current = gen_id();
    let stale = gen_id();
    s.begin_generation(current);
    s.push_chunk(stale, "ghost");
    s.push_chunk(current, "real");
    assert_eq!(s.feed[0].text, "real");
}

#[test]
fn cancelled_finish_adds_note() {
    let mut s = ChatScreen::new();
    let id = gen_id();
    s.begin_generation(id);
    s.finish_generation(id, FinishReason::Cancelled);
    assert!(s.feed.iter().any(|i| i.role == FeedRole::Note));
    assert!(!s.generating);
}

#[test]
fn activate_chat_rebuilds_feed_and_resets_gen() {
    let mut s = ChatScreen::new();
    let prev = gen_id();
    s.begin_generation(prev); // as if a generation was running
    s.set_token_usage(prev, 42, Some(123), true, Some(7)); // the previous chat's token counter
    let id = gen_id();
    let messages = vec![
        Message::new(MessageRole::System, "sys"),
        Message::user("привет"),
        Message::assistant("здравствуйте"),
    ];
    s.activate_chat(
        id,
        "Чат".into(),
        &messages,
        "",
        FeedView::default(),
        None,
        None,
    );
    assert_eq!(s.active_chat, Some(id));
    assert!(!s.generating);
    assert!(s.current_gen.is_none());
    // the previous chat's token counter is cleared (otherwise it would linger in the status bar)
    assert_eq!(s.gen_tokens, 0);
    assert!(s.gen_context.is_none());
    assert!(!s.gen_context_exact);
    // the system message doesn't land in the feed
    assert_eq!(s.feed.len(), 2);
}

/// A jump from a search hit (`AppCommand::OpenChatAt`): the feed opens on the
/// requested message instead of the tail, and the message is marked.
/// See docs/history/chat-search-stage2.md §3.
#[test]
fn activate_chat_with_focus_puts_the_feed_on_that_message() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut s = ChatScreen::new();
    let messages: Vec<Message> = (0..12)
        .map(|i| Message::user(format!("реплика-{i}")))
        .collect();
    let target = messages[8].id;
    s.activate_chat(
        gen_id(),
        "Чат".into(),
        &messages,
        "",
        FeedView::default(),
        focus_on(target),
        None,
    );
    assert_eq!(s.feed_view.anchor(), Some(8));
    assert_eq!(s.feed_view.marker(), Some(8));
    // The jump itself is applied by the next render — the row only exists there
    // (§1.1), so `follow` is still on until a frame is drawn.
    let mut term = Terminal::new(TestBackend::new(40, 14)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
    assert!(!s.feed_view.is_following(), "a jump leaves the tail");
    let dump = format!("{:?}", term.backend().buffer());
    assert!(dump.contains("реплика-8"), "{dump}");

    // Without a focus — the usual tail.
    let mut s = ChatScreen::new();
    s.activate_chat(
        gen_id(),
        "Чат".into(),
        &messages,
        "",
        FeedView::default(),
        None,
        None,
    );
    assert_eq!(s.feed_view.anchor(), None);
    assert_eq!(s.feed_view.marker(), None);
    term.draw(|f| s.render(f)).unwrap();
    assert!(s.feed_view.is_following());
    let dump = format!("{:?}", term.backend().buffer());
    assert!(dump.contains("реплика-11"), "the tail: {dump}");
}

/// The query rides the jump all the way into the feed, so the searched word is
/// highlighted inside the message the view lands on (fork S3(b)); an ordinary
/// activation — a plain switch, a new chat, bootstrap — highlights nothing.
#[test]
fn activate_chat_carries_the_highlight_query_only_on_a_jump() {
    let mut s = ChatScreen::new();
    let messages: Vec<Message> = (0..4)
        .map(|i| Message::user(format!("реплика-{i} про маркер")))
        .collect();
    s.activate_chat(
        gen_id(),
        "Чат".into(),
        &messages,
        "",
        FeedView::default(),
        Some(FeedFocus {
            message: messages[2].id,
            query: "маркер".into(),
        }),
        None,
    );
    assert_eq!(s.feed_view.marker(), Some(2));
    assert_eq!(s.feed_view.highlight(), Some("маркер"));

    // A plain activation must also clear the previous jump's query, or it would
    // light up whatever now sits at that index.
    s.activate_chat(
        gen_id(),
        "Чат".into(),
        &messages,
        "",
        FeedView::default(),
        None,
        None,
    );
    assert_eq!(s.feed_view.marker(), None);
    assert_eq!(s.feed_view.highlight(), None);
}

/// A focus request must never survive into the wrong chat: an id the newly
/// activated chat doesn't contain falls back to the tail, and both the anchor
/// and the marker left over from the previous chat are dropped. (The marker
/// outlives a manual scroll on purpose, so a chat switch is the one place that
/// has to clear it explicitly — this is the test that pins it.)
#[test]
fn focus_from_another_chat_falls_back_to_the_tail() {
    let mut s = ChatScreen::new();
    let first: Vec<Message> = (0..12)
        .map(|i| Message::user(format!("первый-{i}")))
        .collect();
    let stale = first[8].id;
    s.activate_chat(
        gen_id(),
        "Первый".into(),
        &first,
        "",
        FeedView::default(),
        focus_on(stale),
        None,
    );
    assert_eq!(s.feed_view.marker(), Some(8));

    let second: Vec<Message> = (0..12)
        .map(|i| Message::user(format!("второй-{i}")))
        .collect();
    s.activate_chat(
        gen_id(),
        "Второй".into(),
        &second,
        "",
        FeedView::default(),
        focus_on(stale),
        None,
    );
    assert_eq!(
        s.feed_view.anchor(),
        None,
        "an id from another chat must not anchor anything here"
    );
    assert_eq!(
        s.feed_view.marker(),
        None,
        "and the previous chat's marker must not leak in"
    );
    assert!(
        s.feed_view.is_following(),
        "and the view falls back to the tail"
    );
}

/// The yank split (docs/history/chat-search-stage2.md §1.3, §4): after the user has
/// scrolled away, content that arrives on its own leaves the view alone, while
/// user-initiated content still goes to the tail.
#[test]
fn arriving_content_does_not_yank_a_scrolled_away_reader() {
    let id = gen_id();
    let mut s = ChatScreen::new();
    s.push_user_message("вопрос".into());
    s.begin_generation(id);
    s.feed_view.scroll_up(5); // the user scrolled up to read
    assert!(!s.feed_view.is_following());

    // Arrives on its own — the position is kept.
    s.push_tool_call(id, "web_search".into(), "{}".into(), "ок".into(), 0);
    assert!(!s.feed_view.is_following(), "a tool card must not yank");
    s.push_note("заметка");
    assert!(!s.feed_view.is_following(), "a note must not yank");
    s.continue_assistant(id);
    assert!(!s.feed_view.is_following(), "a followup must not yank");
    s.rewrite_assistant(id);
    assert!(!s.feed_view.is_following(), "a rewrite must not yank");

    // User-initiated — back to the tail.
    s.push_user_message("ещё".into());
    assert!(s.feed_view.is_following(), "sending goes to the tail");

    // While already following, all of them keep following.
    let id2 = gen_id();
    s.begin_generation(id2);
    assert!(s.feed_view.is_following());
    s.push_tool_call(id2, "web_search".into(), "{}".into(), "ок".into(), 0);
    s.push_note("ещё заметка");
    assert!(s.feed_view.is_following());
}

#[test]
fn feed_scroll_requests_clear_only_with_vs16_emoji() {
    // Scrolling a feed with plain text doesn't require a full redraw (no flicker),
    // while with a VS16 emoji (`🗂️`) it does (wipes a "hanging" artifact on conhost).
    let id = gen_id();

    let mut clean = ChatScreen::new();
    clean.activate_chat(
        id,
        "Чат".into(),
        &[Message::assistant("обычный текст")],
        "",
        FeedView::default(),
        None,
        None,
    );
    clean.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
    assert!(
        !clean.take_feed_scrolled(),
        "plain text needs no full redraw"
    );
    // the flag is taken exactly once
    assert!(!clean.take_feed_scrolled());

    let mut emoji = ChatScreen::new();
    emoji.activate_chat(
        id,
        "Чат".into(),
        &[Message::assistant("## 🗂️ Хэш")],
        "",
        FeedView::default(),
        None,
        None,
    );
    emoji.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    assert!(
        emoji.take_feed_scrolled(),
        "a VS16 emoji needs a full redraw"
    );
    // no repeated request without a new scroll
    assert!(!emoji.take_feed_scrolled());

    // The loop takes the flag via the generalized `take_full_redraw` — the feed source is
    // included (otherwise the VS16-artifact fix would silently break when a second source is added).
    emoji.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
    assert!(
        emoji.take_full_redraw(),
        "scrolling the feed with VS16 is part of take_full_redraw"
    );
    assert!(!emoji.take_full_redraw());
}

#[test]
fn enter_sends_when_idle_and_nonempty() {
    let mut s = ChatScreen::new();
    s.set_server_status(ready_statuses());
    for c in "привет".chars() {
        s.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let intent = s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(intent, Some(ChatIntent::Send("привет".into())));
    // the field is cleared
    assert!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .is_none()
    );
}

#[test]
fn shift_and_alt_enter_insert_newline_not_send() {
    let mut s = ChatScreen::new();
    s.set_server_status(ready_statuses());
    for c in "ab".chars() {
        s.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    // Shift+Enter — a line break, not a send.
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
        None
    );
    for c in "cd".chars() {
        s.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    // Alt+Enter — the same break (a fallback for terminals with no kitty protocol).
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)),
        None
    );
    for c in "ef".chars() {
        s.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    assert_eq!(s.input.text(), "ab\ncd\nef");
    // A bare Enter still sends the whole multiline input.
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(ChatIntent::Send("ab\ncd\nef".into()))
    );
}

#[test]
fn activate_chat_loads_draft_without_marking_dirty() {
    let mut s = ChatScreen::new();
    // Activating a chat with a saved draft loads it into the input box...
    s.activate_chat(
        gen_id(),
        "Чат".into(),
        &[],
        "недописанный текст",
        FeedView::default(),
        None,
        None,
    );
    assert_eq!(s.input.text(), "недописанный текст");
    // ...but doesn't mark the draft "dirty" (otherwise it would be sent right back).
    assert_eq!(s.take_dirty_draft(), None);
    // Switching to a chat with no draft clears the input box.
    s.activate_chat(
        gen_id(),
        "Новый".into(),
        &[],
        "",
        FeedView::default(),
        None,
        None,
    );
    assert!(s.input.is_empty());
    assert_eq!(s.take_dirty_draft(), None);
}

#[test]
fn typing_marks_draft_dirty_and_take_returns_text_once() {
    let mut s = ChatScreen::new();
    type_str(&mut s, "черновик");
    // The first take returns the typed text...
    assert_eq!(s.take_dirty_draft(), Some("черновик".into()));
    // ...a repeat — None, until the input changes again.
    assert_eq!(s.take_dirty_draft(), None);
}

#[test]
fn sending_clears_draft_to_empty() {
    let mut s = ChatScreen::new();
    s.set_server_status(ready_statuses());
    type_str(&mut s, "вопрос");
    let _ = s.take_dirty_draft(); // took the draft while typing
    let intent = s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(intent, Some(ChatIntent::Send("вопрос".into())));
    // Sending cleared the field — the draft became empty (the UI will send SetDraft("")).
    assert_eq!(s.take_dirty_draft(), Some(String::new()));
}

#[test]
fn restore_input_sets_when_empty_and_prepends_when_not() {
    let mut s = ChatScreen::new();
    // An empty field is simply filled.
    s.restore_input("вопрос".into());
    assert_eq!(s.input.text(), "вопрос");
    // Non-empty — the text is prepended, the existing input is kept.
    s.input.clear();
    type_str(&mut s, "хвост");
    s.restore_input("голова ".into());
    assert_eq!(s.input.text(), "голова хвост");
}

#[test]
fn ctrl_u_emits_impersonate_with_input_seed() {
    let mut s = ChatScreen::new();
    type_str(&mut s, "начало");
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)),
        Some(ChatIntent::Impersonate {
            seed: "начало".into()
        })
    );
    // Suppressed during generation.
    s.begin_generation(gen_id());
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)),
        None
    );
}

#[test]
fn impersonation_stream_then_stop_commits_text_to_input() {
    let mut s = ChatScreen::new();
    type_str(&mut s, "Я "); // the seed
    let id = gen_id();
    s.begin_impersonation(id);
    assert!(s.is_impersonating());
    s.push_impersonation_chunk(id, "хочу узнать про Rust");
    s.finish_impersonation(id, FinishReason::Stop);
    assert!(!s.is_impersonating());
    // The reply text (seed + generated) — in the input box.
    assert_eq!(s.input.text(), "Я хочу узнать про Rust");
}

#[test]
fn impersonation_cancel_keeps_seed_and_discards_generated() {
    let mut s = ChatScreen::new();
    type_str(&mut s, "черновик");
    let id = gen_id();
    s.begin_impersonation(id);
    s.push_impersonation_chunk(id, " дополнение");
    // Esc during impersonation — the cancel intent.
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Some(ChatIntent::CancelImpersonation)
    );
    // Cancellation (Cancelled) discards the generated part — the field keeps the seed.
    s.finish_impersonation(id, FinishReason::Cancelled);
    assert!(!s.is_impersonating());
    assert_eq!(s.input.text(), "черновик");
}

#[test]
fn impersonation_timeout_keeps_partial_text() {
    // An impersonation timeout arrives as `Length` (not `Cancelled`) — the truncated
    // reply must be kept in the field, not vanish.
    let mut s = ChatScreen::new();
    type_str(&mut s, "Я ");
    let id = gen_id();
    s.begin_impersonation(id);
    s.push_impersonation_chunk(id, "хочу узнать про");
    s.finish_impersonation(id, FinishReason::Length);
    assert!(!s.is_impersonating());
    assert_eq!(s.input.text(), "Я хочу узнать про");
}

#[test]
fn keys_ignored_during_impersonation_except_cancel_quit() {
    let mut s = ChatScreen::new();
    s.begin_impersonation(gen_id());
    // A regular key isn't typed into the field (the field is hidden).
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
        None
    );
    // Ctrl+Q / F10 still quit (quit moved off Ctrl+C).
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
        Some(ChatIntent::Quit)
    );
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::F(10), KeyModifiers::NONE)),
        Some(ChatIntent::Quit)
    );
}

#[test]
fn render_during_impersonation_does_not_panic() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut s = ChatScreen::new();
    let id = gen_id();
    s.begin_impersonation(id);
    s.push_impersonation_chunk(id, "текст реплики");
    let mut term = Terminal::new(TestBackend::new(50, 16)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
}

#[test]
fn ctrl_r_and_e_emit_intents_when_idle() {
    let mut s = ChatScreen::new();
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
        Some(ChatIntent::RegenerateLast)
    );
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)),
        Some(ChatIntent::DeleteLastExchange)
    );
}

#[test]
fn ctrl_r_and_e_suppressed_while_generating() {
    let mut s = ChatScreen::new();
    s.begin_generation(gen_id());
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
        None
    );
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)),
        None
    );
}

/// Turns on `Ctrl+R`/`Ctrl+E` confirmation via a settings snapshot.
fn with_confirm() -> ChatScreen {
    let mut s = ChatScreen::new();
    let mut cfg = AppConfig::default();
    cfg.interface.confirm_destructive_keys = true;
    s.set_settings(cfg, Vec::new(), Vec::new(), Default::default(), Vec::new());
    s
}

#[test]
fn ctrl_r_with_confirm_opens_popup_then_enter_confirms() {
    let mut s = with_confirm();
    // The first press doesn't yield an intent — it opens the confirmation popup.
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
        None
    );
    assert_eq!(s.confirm, Some(ConfirmAction::Regenerate));
    // Enter confirms and closes the popup.
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(ChatIntent::RegenerateLast)
    );
    assert_eq!(s.confirm, None);
}

#[test]
fn ctrl_e_with_confirm_esc_cancels() {
    let mut s = with_confirm();
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)),
        None
    );
    assert_eq!(s.confirm, Some(ConfirmAction::DeleteExchange));
    // Esc cancels — the popup closes, no intent.
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        None
    );
    assert_eq!(s.confirm, None);
}

#[test]
fn confirm_popup_ignores_other_keys() {
    let mut s = with_confirm();
    s.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    // An arbitrary key doesn't close the popup and isn't typed into the input box.
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
        None
    );
    assert_eq!(s.confirm, Some(ConfirmAction::Regenerate));
    assert!(s.input.is_empty());
}

#[test]
fn ctrl_q_breaks_through_confirm_popup_to_quit() {
    let mut s = with_confirm();
    s.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    assert_eq!(s.confirm, Some(ConfirmAction::Regenerate));
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
        Some(ChatIntent::Quit)
    );
    assert_eq!(s.confirm, None);
}

#[test]
fn ctrl_r_and_e_emit_directly_without_confirm() {
    // By default (with no settings snapshot) confirmation is off.
    let mut s = ChatScreen::new();
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
        Some(ChatIntent::RegenerateLast)
    );
    assert_eq!(s.confirm, None);
}

// ---------- the dangerous-tool confirmation popup (spec §9.8) ----------

/// The ids the loop parked the call under. They must come back untouched — a
/// reply landing on the wrong turn or the wrong call is what the orchestrator
/// drops on the other side.
const TOOL_CALL_ID: &str = "call-7";

/// A screen with the dangerous-tool popup open, as [`ChatScreen::request_tool_confirm`]
/// opens it. Deliberately **not** generating: the tests below pin the popup's own
/// logic, while `tool_confirm_is_answerable_while_generating` pins the routing
/// order that makes it usable at all.
fn with_tool_confirm() -> (ChatScreen, Uuid) {
    let mut s = ChatScreen::new();
    let id = gen_id();
    s.request_tool_confirm(
        id,
        TOOL_CALL_ID.into(),
        "python_exec".into(),
        r#"{"code":"print(1)"}"#.into(),
    );
    (s, id)
}

/// The intent the popup must emit for `decision`, carrying the ids back.
fn confirmed(id: Uuid, decision: ToolDecision) -> Option<ChatIntent> {
    Some(ChatIntent::ConfirmTool {
        generation_id: id,
        call_id: TOOL_CALL_ID.into(),
        decision,
    })
}

#[test]
fn tool_confirm_enter_allows_the_call_and_closes_the_popup() {
    let (mut s, id) = with_tool_confirm();
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        confirmed(id, ToolDecision::Allow)
    );
    assert_eq!(s.tool_confirm, None);
}

#[test]
fn tool_confirm_a_allows_the_tool_for_the_rest_of_the_turn() {
    let (mut s, id) = with_tool_confirm();
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
        confirmed(id, ToolDecision::AllowForTurn)
    );
    assert_eq!(s.tool_confirm, None);
}

/// Layout-independent, like every other letter shortcut: physical A is `ф` on a
/// Russian layout (see shared::keys).
#[test]
fn tool_confirm_a_works_under_a_cyrillic_layout() {
    let (mut s, id) = with_tool_confirm();
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('ф'), KeyModifiers::NONE)),
        confirmed(id, ToolDecision::AllowForTurn),
        "ф (physical A) — allow for the turn"
    );
}

/// Declining is **not** cancelling the turn: the loop carries on with the
/// refusal, and a second `Esc` — now that the popup is gone — cancels as it
/// always did.
#[test]
fn tool_confirm_esc_declines_without_cancelling_the_turn() {
    let (mut s, id) = with_tool_confirm();
    s.begin_generation(id);
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        confirmed(id, ToolDecision::Deny)
    );
    assert_eq!(s.tool_confirm, None);
    // Only now does Esc mean "cancel".
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Some(ChatIntent::Cancel)
    );
}

#[test]
fn tool_confirm_ignores_other_keys_and_stays_open() {
    let (mut s, _) = with_tool_confirm();
    for key in [
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
    ] {
        assert_eq!(s.handle_key(key), None, "{key:?} must be ignored");
        assert!(s.tool_confirm.is_some(), "the popup stays open");
    }
    assert!(s.input.is_empty(), "and nothing is typed into the message");
}

#[test]
fn ctrl_q_and_f10_break_through_the_tool_confirm_popup() {
    let (mut s, _) = with_tool_confirm();
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
        Some(ChatIntent::Quit)
    );
    assert_eq!(s.tool_confirm, None);

    let (mut s, _) = with_tool_confirm();
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::F(10), KeyModifiers::NONE)),
        Some(ChatIntent::Quit)
    );
    assert_eq!(s.tool_confirm, None);
}

/// The point of the feature: the popup is open **during** the turn — the loop is
/// parked on it. Without the routing order in `handle_key` (the popup checked
/// before the generation gate) `Enter` would be swallowed and `Esc` would cancel
/// the turn instead of answering.
#[test]
fn tool_confirm_is_answerable_while_generating() {
    let (mut s, id) = with_tool_confirm();
    s.begin_generation(id);
    assert!(s.generating);
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        confirmed(id, ToolDecision::Allow)
    );
    assert!(s.generating, "answering doesn't end the turn");
}

/// The call is formatted through `features::tools::present` — the same
/// formatting the feed uses afterwards (fork F7) — so the user decides on
/// readable code, not on a JSON blob.
#[test]
fn tool_confirm_popup_shows_the_call_as_code_with_the_three_options() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut s = ChatScreen::new();
    let id = gen_id();
    s.begin_generation(id);
    s.request_tool_confirm(
        id,
        TOOL_CALL_ID.into(),
        "python_exec".into(),
        r#"{"code": "print(1)\nprint(2)"}"#.into(),
    );
    let mut term = Terminal::new(TestBackend::new(90, 20)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
    // Rows joined by hand rather than `format!("{:?}", buffer)`: the buffer's
    // Debug prints rows inside quotes **without escaping** the ones in the
    // content, so a `"` assertion against it can never match — and the "no raw
    // JSON" check below is exactly such an assertion.
    let buf = term.backend().buffer();
    let mut text = String::new();
    for y in buf.area.top()..buf.area.bottom() {
        for x in buf.area.left()..buf.area.right() {
            text.push_str(buf[(x, y)].symbol());
        }
        text.push('\n');
    }

    assert!(text.contains("python_exec"), "the tool name: {text}");
    // The code arrives as lines — each statement on a row of its own, and no
    // trace of the JSON it was extracted from.
    assert!(text.contains("print(1)"), "{text}");
    assert!(text.contains("print(2)"), "{text}");
    assert!(
        !text
            .lines()
            .any(|l| l.contains("print(1)") && l.contains("print(2)")),
        "the two statements belong on separate lines: {text}"
    );
    assert!(
        !text.contains("\"code\""),
        "the raw JSON must not be shown: {text}"
    );
    // The footer spells out all three answers.
    for option in [
        "Enter — выполнить",
        "A — разрешить до конца хода",
        "Esc — отклонить",
    ] {
        assert!(text.contains(option), "missing the \"{option}\" option");
    }
}

#[test]
fn esc_opens_chat_list_else_cancels_generation() {
    let mut s = ChatScreen::new();
    // With no generation running, Esc asks to open the chat-list screen (`app` creates it).
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Some(ChatIntent::OpenChatList)
    );
    // During generation, Esc first cancels it.
    s.begin_generation(gen_id());
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Some(ChatIntent::Cancel)
    );
}

#[test]
fn ctrl_q_and_f10_quit_from_chat() {
    let mut s = ChatScreen::new();
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
        Some(ChatIntent::Quit)
    );
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::F(10), KeyModifiers::NONE)),
        Some(ChatIntent::Quit)
    );
    // Ctrl+C with no selection — a no-op (freed up for copying, not quitting).
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        None
    );
}

#[test]
fn ctrl_c_copies_selection_ctrl_x_cuts() {
    let mut s = ChatScreen::new();
    s.input.set_text("hello world");
    s.input
        .on_key(KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL)); // cursor to the start
    for _ in 0..5 {
        s.input
            .on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)); // select "hello"
    }
    // Ctrl+C → an intent to write the selection to the clipboard; the selection clears, the text is intact.
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Some(ChatIntent::CopyToClipboard("hello".into()))
    );
    assert!(!s.input.has_selection());
    assert_eq!(s.input.text(), "hello world");
    // Select the next 5 characters (" worl") and cut them — the text shrinks.
    for _ in 0..5 {
        s.input
            .on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    }
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)),
        Some(ChatIntent::CopyToClipboard(" worl".into()))
    );
    assert_eq!(s.input.text(), "hellod");
}

#[test]
fn f1_opens_help_and_esc_closes() {
    let mut s = ChatScreen::new();
    assert!(s.help.is_none());
    s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(s.help.is_some());
    // Opens on the "Hotkeys" tab.
    assert_eq!(s.help.as_ref().unwrap().tab, HelpTab::Hotkeys);
    // Any other key doesn't close the dialog (it has tabs/navigation) and isn't typed.
    let intent = s.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert_eq!(intent, None);
    assert!(s.help.is_some(), "another key must not close the dialog");
    assert!(
        s.input.is_empty(),
        "input must not be typed while the dialog is open"
    );
    // Esc closes it.
    s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(s.help.is_none());
}

#[test]
fn question_mark_opens_help_only_when_input_empty() {
    let mut s = ChatScreen::new();
    // Empty input → `?` opens the dialog.
    s.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    assert!(s.help.is_some());
    s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)); // close it
    // Non-empty input → `?` is typed, the dialog doesn't open.
    type_str(&mut s, "abc");
    s.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    assert!(s.help.is_none());
    assert_eq!(s.input.text(), "abc?");
}

#[test]
fn help_navigation_scrolls_and_switches_tabs() {
    let mut s = ChatScreen::new();
    s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    // ↑↓/PgUp/PgDn scroll the active tab without closing the dialog.
    s.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    s.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    assert!(s.help.is_some(), "scrolling must not close the dialog");
    assert_eq!(s.help.as_ref().unwrap().scroll, 1 + PAGE_SCROLL);
    s.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(s.help.as_ref().unwrap().scroll, PAGE_SCROLL);
    s.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
    assert_eq!(s.help.as_ref().unwrap().scroll, 0);
    // Scroll a bit and switch tabs → scroll resets (Tab — next tab,
    // order About/Hotkeys/Commands/License/Components: the one after Hotkeys is Commands).
    s.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    s.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let h = s.help.as_ref().unwrap();
    assert_eq!(h.tab, HelpTab::Commands);
    assert_eq!(h.scroll, 0, "switching tabs resets scroll");
    // ← goes back to the previous tab.
    s.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(s.help.as_ref().unwrap().tab, HelpTab::Hotkeys);
    // Esc closes it; reopening — on the same tab (remembered), with scroll at
    // zero. Here we came back to Hotkeys, so it reopens on Hotkeys.
    s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(s.help.is_none());
    s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    let h = s.help.as_ref().unwrap();
    assert_eq!((h.tab, h.scroll), (HelpTab::Hotkeys, 0));
}

/// The help dialog remembers the last-selected tab and opens on it.
#[test]
fn help_remembers_last_tab() {
    let mut s = ChatScreen::new();
    // Open it (Hotkeys by default), switch to "Components", close it.
    s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    // About ← Hotkeys ← ... : two `←` from Hotkeys → Components (wrapping: Hotkeys→About→Components).
    s.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    s.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(s.help.as_ref().unwrap().tab, HelpTab::Components);
    s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(s.help.is_none());
    // Reopening — on "Components" again.
    s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert_eq!(s.help.as_ref().unwrap().tab, HelpTab::Components);
}

#[test]
fn help_scroll_clamps_and_draws_scrollbar_on_short_terminal() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut s = ChatScreen::new();
    s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    s.help.as_mut().unwrap().scroll = 10_000; // "over-scrolled" — the render clamps it
    let mut term = Terminal::new(TestBackend::new(90, 12)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
    // The "Hotkeys" list doesn't fit in a short dialog → scroll clamps to
    // the max (well below what was requested) and the scrollbar thumb is drawn.
    // The bound: entries + one blank per group opener comfortably exceeds the
    // tab's line count minus the view (a couple of rows also wrap).
    assert!(
        s.help.as_ref().unwrap().scroll < HELP_KEYS.len() + KEY_GROUP_OPENERS.len(),
        "scroll clamps to the maximum"
    );
    let buf = term.backend().buffer();
    let mut thumb = false;
    for y in buf.area.top()..buf.area.bottom() {
        for x in buf.area.left()..buf.area.right() {
            thumb |= buf[(x, y)].symbol() == "█";
        }
    }
    assert!(thumb, "on a short terminal help has a scrollbar thumb");
}

#[test]
fn settings_event_updates_theme_palette() {
    use crate::shared::config::Theme;
    let mut s = ChatScreen::new();
    assert_eq!(s.palette, Palette::for_theme(Theme::Auto));
    let mut cfg = AppConfig::default();
    cfg.interface.theme = Theme::Dark;
    s.set_settings(cfg, vec![], Vec::new(), Default::default(), Vec::new());
    assert_eq!(s.palette, Palette::for_theme(Theme::Dark));
}

#[test]
fn ctrl_p_opens_settings_only_with_snapshot() {
    let mut s = ChatScreen::new();
    // Without a settings snapshot — Ctrl+P does nothing.
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)),
        None
    );
    s.set_settings(
        AppConfig::default(),
        vec![Profile::new("P", "sys")],
        Vec::new(),
        Default::default(),
        Vec::new(),
    );
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)),
        Some(ChatIntent::OpenSettings)
    );
}

#[test]
fn ctrl_shortcuts_work_under_cyrillic_layout() {
    // Under a Cyrillic layout physical keys report Cyrillic characters: Ctrl+з (physical P),
    // Ctrl+й (physical Q) — the shortcuts must still fire.
    let mut s = ChatScreen::new();
    s.set_settings(
        AppConfig::default(),
        vec![Profile::new("P", "sys")],
        Vec::new(),
        Default::default(),
        Vec::new(),
    );
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('з'), KeyModifiers::CONTROL)),
        Some(ChatIntent::OpenSettings),
        "Ctrl+з (physical P) opens settings"
    );
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('й'), KeyModifiers::CONTROL)),
        Some(ChatIntent::Quit),
        "Ctrl+й (physical Q) — quit"
    );
}

#[test]
fn rename_chat_updates_title_bar_of_active_chat() {
    let mut s = ChatScreen::new();
    let id = gen_id();
    s.activate_chat(
        id,
        "Старое".into(),
        &[],
        "",
        FeedView::default(),
        None,
        None,
    );
    s.rename_chat(id, "Новое".into());
    assert_eq!(s.title, "Новое");
    // A foreign chat doesn't touch the active chat's header.
    s.rename_chat(gen_id(), "Постороннее".into());
    assert_eq!(s.title, "Новое");
}

#[test]
fn f5_copies_active_chat_in_main_window() {
    let mut s = ChatScreen::new();
    // With no active chat, F5 — a no-op.
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
        None
    );
    let id = gen_id();
    s.activate_chat(id, "Чат".into(), &[], "", FeedView::default(), None, None);
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
        Some(ChatIntent::CopyChat(id))
    );
}

#[test]
fn late_list_op_results_fall_to_feed() {
    // When the list screen is closed, late results of list operations (copy/
    // auto-title) `app` puts into the feed as a note via push_note/push_error.
    let mut s = ChatScreen::new();
    s.push_note("Переписка скопирована в буфер обмена");
    s.push_error("не удалось");
    assert_eq!(
        s.feed.iter().filter(|m| m.role == FeedRole::Note).count(),
        2
    );
}

fn wheel(kind: MouseEventKind) -> MouseEvent {
    MouseEvent {
        kind,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    }
}

fn mouse_at(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

#[test]
fn mouse_click_and_drag_in_input_build_selection() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::MouseButton;
    let mut s = ChatScreen::new();
    type_str(&mut s, "hello world");
    // Render so the input box remembers its area (last_area).
    let mut term = Terminal::new(TestBackend::new(50, 16)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
    let area = s
        .input
        .last_area_for_test()
        .expect("the field has been rendered");
    // A click at the start of the field — the cursor goes there, no selection yet.
    s.handle_mouse(mouse_at(
        MouseEventKind::Down(MouseButton::Left),
        area.x,
        area.y,
    ));
    assert_eq!(s.input.cursor(), (0, 0));
    assert!(!s.input.has_selection());
    // Dragging right by 5 columns grows the "hello" selection.
    s.handle_mouse(mouse_at(
        MouseEventKind::Drag(MouseButton::Left),
        area.x + 5,
        area.y,
    ));
    assert!(s.input.has_selection());
    assert_eq!(s.input.selected_text().as_deref(), Some("hello"));
}

#[test]
fn mouse_click_outside_input_does_not_move_cursor() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::MouseButton;
    let mut s = ChatScreen::new();
    type_str(&mut s, "hello");
    let mut term = Terminal::new(TestBackend::new(50, 16)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
    // A click into the feed (top of the screen, outside the input box) — the field's cursor doesn't move.
    s.handle_mouse(mouse_at(MouseEventKind::Down(MouseButton::Left), 0, 0));
    assert_eq!(s.input.cursor(), (0, 5)); // the cursor stayed at the end of the text
    assert!(!s.input.has_selection());
}

#[test]
fn mouse_wheel_up_scrolls_feed_and_disables_follow() {
    let mut s = ChatScreen::new();
    assert!(s.feed_view.is_following());
    s.handle_mouse(wheel(MouseEventKind::ScrollUp));
    assert!(
        !s.feed_view.is_following(),
        "scrolling up disables tail-following"
    );
}

#[test]
fn ctrl_t_and_ctrl_o_report_the_new_collapse_state() {
    // Both toggles are independent and both report the whole view back, so the
    // orchestrator can store it on the chat (spec §11.3, docs/feed-collapse.md).
    let mut s = ChatScreen::new();
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)),
        Some(ChatIntent::SetFeedView(FeedView {
            thoughts: true,
            tools: false
        }))
    );
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)),
        Some(ChatIntent::SetFeedView(FeedView {
            thoughts: true,
            tools: true
        }))
    );
    // ...and back, one at a time.
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)),
        Some(ChatIntent::SetFeedView(FeedView {
            thoughts: false,
            tools: true
        }))
    );
    // Layout-independent: Ctrl+щ is the physical O.
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('щ'), KeyModifiers::CONTROL)),
        Some(ChatIntent::SetFeedView(FeedView::default()))
    );
}

#[test]
fn activating_a_chat_applies_its_stored_collapse_state() {
    // The state belongs to the chat, so switching to one that had things
    // expanded shows them expanded — without a keypress.
    let mut s = ChatScreen::new();
    let expanded = FeedView {
        thoughts: true,
        tools: true,
    };
    s.activate_chat(gen_id(), "Чат".into(), &[], "", expanded, None, None);
    assert_eq!(s.feed_view.view(), expanded);
    // ...and switching to a chat that never expanded anything collapses again.
    s.activate_chat(
        gen_id(),
        "Другой".into(),
        &[],
        "",
        FeedView::default(),
        None,
        None,
    );
    assert_eq!(s.feed_view.view(), FeedView::default());
}

#[test]
fn ctrl_w_toggles_mouse_capture_intent() {
    let mut s = ChatScreen::new();
    // Capture is off by default → the first press turns it on (true).
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)),
        Some(ChatIntent::SetMouseCapture(true))
    );
    // The second one — turns it off (false).
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)),
        Some(ChatIntent::SetMouseCapture(false))
    );
    // Also works under a Cyrillic layout: Ctrl+ц (physical W).
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('ц'), KeyModifiers::CONTROL)),
        Some(ChatIntent::SetMouseCapture(true))
    );
}

#[test]
fn mouse_wheel_ignored_while_overlay_open() {
    let mut s = ChatScreen::new();
    s.help = Some(HelpState::open(DEFAULT_HELP_TAB));
    s.handle_mouse(wheel(MouseEventKind::ScrollUp));
    assert!(
        s.feed_view.is_following(),
        "with help open, the wheel doesn't touch the feed"
    );
}

fn mk_checker() -> SpellChecker {
    let dict = spellbook::Dictionary::new("SET UTF-8\n", "2\nhello\nworld\n").unwrap();
    SpellChecker::new(vec![dict], std::collections::HashSet::new(), None)
}

fn type_str(s: &mut ChatScreen, text: &str) {
    for c in text.chars() {
        s.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

#[test]
fn input_with_risky_glyph_requests_full_redraw() {
    // An edit LEFT of a VS16 emoji in the input box moves it onto a foreign symbol's spot:
    // the diff sends the trailing half, the backend prints it with no `MoveTo` and the row drifts
    // right (ratatui#2651, mechanics in `shared::ui`). So a field with such a glyph
    // is drawn with a full redraw; with plain text — it isn't.
    let mut s = ChatScreen::new();
    type_str(&mut s, "hello");
    assert!(!s.take_full_redraw(), "plain text needs no full redraw");

    type_str(&mut s, "\u{2764}\u{FE0F}");
    assert!(s.take_full_redraw(), "a field with ❤️ needs a full redraw");

    // And subsequent edits do too — the glyph is still in the field.
    type_str(&mut s, "x");
    assert!(
        s.take_full_redraw(),
        "an edit while the glyph is live — too"
    );

    // Clearing the field lifts the requirement.
    s.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    s.take_full_redraw();
    type_str(&mut s, "plain");
    assert!(
        !s.take_full_redraw(),
        "after clearing, plain text needs no redraw"
    );
}

#[test]
fn ctrl_g_opens_suggestions_for_misspelled_word() {
    let mut s = ChatScreen::new();
    s.set_spellchecker(mk_checker());
    type_str(&mut s, "helo"); // cursor at the end of the misspelled word
    s.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
    assert!(s.suggest.is_some());
    // the last item — "add to dictionary"
    let items = &s.suggest.as_ref().unwrap().items;
    assert_eq!(items.last(), Some(&SuggestItem::AddToDictionary));
}

#[test]
fn applying_suggestion_replaces_word() {
    let mut s = ChatScreen::new();
    s.set_spellchecker(mk_checker());
    type_str(&mut s, "helo");
    s.open_suggestions();
    // the first item — the "hello" suggestion; Enter applies it
    assert_eq!(
        s.suggest.as_ref().unwrap().items.first(),
        Some(&SuggestItem::Replace("hello".into()))
    );
    s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(s.suggest.is_none());
    assert_eq!(s.input.text(), "hello");
}

#[test]
fn add_to_dictionary_clears_the_error() {
    let mut s = ChatScreen::new();
    s.set_spellchecker(mk_checker());
    type_str(&mut s, "helo");
    s.open_suggestions();
    let last = s.suggest.as_ref().unwrap().items.len() - 1;
    // navigate to "add to dictionary" and apply it
    for _ in 0..last {
        s.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(s.suggest.is_none());
    // the word is now in the personal dictionary — no longer an error
    assert!(
        s.spell
            .as_ref()
            .unwrap()
            .misspelled_word_at("helo", 2)
            .is_none()
    );
}

#[test]
fn esc_closes_suggestions_without_quitting() {
    let mut s = ChatScreen::new();
    s.set_spellchecker(mk_checker());
    type_str(&mut s, "helo");
    s.open_suggestions();
    assert!(s.suggest.is_some());
    let intent = s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(intent, None);
    assert!(s.suggest.is_none());
}

#[test]
fn render_with_suggestions_does_not_panic() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut s = ChatScreen::new();
    s.set_spellchecker(mk_checker());
    type_str(&mut s, "helo");
    s.open_suggestions();
    let mut term = Terminal::new(TestBackend::new(50, 16)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
}

#[test]
fn suggest_popup_selection_matches_other_lists() {
    // Selection in the spellcheck popup — like in the chat list, settings, and the "self-
    // model" screen: a soft `keycap_bg` backdrop + a green `▌` rail, not a reversed line.
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Modifier;
    let mut s = ChatScreen::new();
    s.set_spellchecker(mk_checker());
    type_str(&mut s, "helo");
    s.open_suggestions();
    let palette = s.palette;
    let mut term = Terminal::new(TestBackend::new(50, 16)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
    let buf = term.backend().buffer();

    let rail = (0..buf.area.height)
        .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
        .find(|&(x, y)| buf[(x, y)].symbol() == "▌")
        .expect("the popup's selected line must have a `▌` rail");
    let cell = &buf[rail];
    assert_eq!(cell.style().fg, Some(palette.success), "the rail is green");
    assert_eq!(
        cell.style().bg,
        Some(palette.keycap_bg),
        "the selection is a keycap_bg backdrop"
    );
    // The selected line must not be reversed (reverse video would swap fg↔bg per span).
    let (_, row) = rail;
    for x in 0..buf.area.width {
        assert!(
            !buf[(x, row)]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "the selected line must not be reversed"
        );
    }
}

#[test]
fn ctrl_b_opens_emoji_picker_and_enter_inserts_at_cursor() {
    let mut s = ChatScreen::new();
    type_str(&mut s, "ab");
    // cursor between 'a' and 'b'
    s.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    // Ctrl+B opens the popup (also under a Cyrillic layout: Ctrl+и → physical B).
    s.handle_key(KeyEvent::new(KeyCode::Char('и'), KeyModifiers::CONTROL));
    assert!(s.emoji.is_some());
    // Enter inserts the first emoji at the cursor and closes the popup.
    let intent = s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(intent, None);
    assert!(s.emoji.is_none());
    assert_eq!(s.input.text(), "a😀b");
}

#[test]
fn emoji_picker_remembers_last_selection() {
    let mut s = ChatScreen::new();
    // Open it, shift the selection, and insert.
    s.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    s.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    s.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(s.emoji.is_none());
    // Reopening restores the previous selection (index 2).
    s.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    assert_eq!(s.emoji.as_ref().unwrap().selected(), 2);
}

#[test]
fn esc_closes_emoji_picker_without_quitting() {
    let mut s = ChatScreen::new();
    s.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    assert!(s.emoji.is_some());
    let intent = s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(intent, None);
    assert!(s.emoji.is_none());
    assert!(s.input.is_empty(), "no emoji is inserted on cancel");
}

#[test]
fn risky_glyph_detector_covers_emoji_classes_but_not_plain_text() {
    use super::feed::is_risky_glyph;
    // Risk classes: VS16, ZWJ, skin tone, supplementary-plane pictographs, width-2 BMP emoji.
    for c in [
        '\u{FE0F}',
        '\u{200D}',
        '\u{1F3FD}',
        '😀',
        '🔥',
        '✅',
        '⭐',
        '✨',
    ] {
        assert!(is_risky_glyph(c), "{c:?} must be considered risky");
    }
    // Plain text, punctuation, typography, and CJK — not risky (otherwise a full redraw
    // would run for every chunk of Chinese/Japanese text for no benefit).
    for c in ['a', 'я', ' ', '·', '—', '→', '│', '█', '中', 'あ'] {
        assert!(!is_risky_glyph(c), "{c:?} must not be considered risky");
    }
}

#[test]
fn feed_content_change_requests_full_redraw_only_with_risky_glyphs() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    // Artifacts on legacy terminals appeared on a content CHANGE (streaming,
    // a note added), and only scrolling used to "fix" them — it was the sole
    // trigger for a full redraw. Now a feed change is a trigger too.
    let draw = |s: &mut ChatScreen, term: &mut Terminal<TestBackend>| {
        term.draw(|f| s.render(f)).unwrap();
    };

    // Plain text: streaming doesn't require a full redraw.
    let mut plain = ChatScreen::new();
    let mut term = Terminal::new(TestBackend::new(60, 16)).unwrap();
    let id = gen_id();
    plain.begin_generation(id);
    plain.push_chunk(id, "обычный текст");
    draw(&mut plain, &mut term);
    assert!(
        !plain.take_full_redraw(),
        "with plain text a feed change needs no redraw"
    );

    // Emoji in the feed: streaming requires it.
    let mut emoji = ChatScreen::new();
    let id = gen_id();
    emoji.begin_generation(id);
    emoji.push_chunk(id, "смотри: 😀");
    assert!(
        emoji.take_full_redraw(),
        "a feed change with emoji requests a redraw — BEFORE rendering, so the artifact doesn't flash even for one frame"
    );
    assert!(!emoji.take_full_redraw(), "the flag is taken exactly once");

    // Redrawing again with no changes — the request isn't reissued (otherwise the loop
    // would redraw the whole screen forever).
    draw(&mut emoji, &mut term);
    assert!(
        !emoji.take_full_redraw(),
        "with no content change a redraw isn't needed"
    );

    // A note added to the feed (F5 "Conversation copied...") is also a content change.
    emoji.push_note("Переписка скопирована в буфер обмена");
    draw(&mut emoji, &mut term);
    assert!(emoji.take_full_redraw(), "an added note requests a redraw");
}

#[test]
fn every_feed_mutator_marks_content_change() {
    // A gate for the "mutators set the flag" approach: a forgotten `mark_feed_changed` call
    // would bring back a flashing artifact on legacy terminals. The feed contains an
    // emoji from the very start, so ANY change must request a full redraw.
    let mut s = ChatScreen::new();
    let id = gen_id();

    s.activate_chat(
        id,
        "Чат".into(),
        &[Message::assistant("привет 😀")],
        "",
        FeedView::default(),
        None,
        None,
    );
    assert!(s.take_full_redraw(), "activate_chat");

    s.push_user_message("вопрос".into());
    assert!(s.take_full_redraw(), "push_user_message");

    s.begin_generation(id);
    assert!(s.take_full_redraw(), "begin_generation");

    s.push_chunk(id, "ответ");
    assert!(s.take_full_redraw(), "push_chunk");

    s.push_thoughts(id, "мысль");
    assert!(s.take_full_redraw(), "push_thoughts");

    s.push_tool_call(id, "web_search".into(), "{}".into(), "ок".into(), 0);
    assert!(s.take_full_redraw(), "push_tool_call");

    s.continue_assistant(id);
    assert!(s.take_full_redraw(), "continue_assistant");

    s.rewrite_assistant(id);
    assert!(s.take_full_redraw(), "rewrite_assistant");

    s.push_note("заметка");
    assert!(s.take_full_redraw(), "push_note");

    s.push_error("ошибка");
    assert!(s.take_full_redraw(), "push_error");

    // Collapsing "thoughts" (`Ctrl+T`) recomputes all blocks — also a change.
    s.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
    assert!(s.take_full_redraw(), "Ctrl+T (collapsing thoughts)");

    // A compaction adds a divider above a block — a content change like any other.
    s.set_compaction(Some((gen_id(), "ранее обсудили X".into())));
    assert!(s.take_full_redraw(), "set_compaction");
}

#[test]
fn suggest_popup_actions_request_full_redraw() {
    // The same class as the emoji popup: the "➕ add to dictionary" item carries a wide
    // glyph. On CLOSING the popup, the diff sends its trailing cell only for a styled
    // line, so we take out insurance via a full redraw; on a selection shift it wouldn't
    // help anyway (the sentinel skips a wide glyph's tail, ratatui#2651).
    let mut s = ChatScreen::new();
    s.set_spellchecker(mk_checker());
    type_str(&mut s, "helo");
    s.open_suggestions();
    assert!(s.suggest.is_some(), "the suggestion popup opened");
    s.take_full_redraw(); // reset the flag left over from typing

    // A selection shift does NOT require a redraw: the glyph stays wide, the sentinel is
    // required to skip its tail (ratatui#2651) — reprinting the glyph clears the highlight.
    s.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert!(!s.take_full_redraw(), "a selection shift needs no redraw");

    // A key that doesn't change the popup doesn't request a redraw.
    s.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert!(!s.take_full_redraw(), "a no-op key needs no redraw");

    // Closing via cancel (`Esc`).
    s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(s.suggest.is_none());
    assert!(
        s.take_full_redraw(),
        "cancelling closes the popup → a redraw"
    );

    // Closing by applying a suggestion (`Enter`).
    s.open_suggestions();
    s.take_full_redraw();
    s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(s.suggest.is_none());
    assert!(
        s.take_full_redraw(),
        "applying it closes the popup → a redraw"
    );
}

#[test]
fn emoji_picker_actions_request_full_redraw() {
    // A wide emoji glyph leaves a "hanging" trailing half on conhost when it
    // DISAPPEARS from the screen: the diff doesn't send trailing cells of unstyled grid
    // glyphs (a canary pinning the boundary of the upstream fix — in `widgets::emoji_picker`).
    // So a full redraw is requested by closing the popup — but not by a selection
    // shift, where the glyph stays wide and the sentinel skips its tail (ratatui#2651).
    let mut s = ChatScreen::new();
    assert!(
        !s.take_full_redraw(),
        "with no popup, a redraw isn't needed"
    );

    // Opening it by itself doesn't require a redraw — the glyph doesn't leave anywhere.
    s.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    assert!(!s.take_full_redraw(), "opening the popup needs no redraw");

    // A selection shift does NOT require a redraw: the glyph stays wide, the sentinel is
    // required to skip its tail (ratatui#2651) — reprinting the glyph clears the backdrop.
    s.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert!(!s.take_full_redraw(), "a selection shift needs no redraw");

    // Closing via insertion (`Enter`).
    s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(s.emoji.is_none());
    assert!(
        s.take_full_redraw(),
        "inserting closes the popup → a redraw"
    );
    assert!(!s.take_full_redraw(), "the flag is taken exactly once");

    // Closing via cancel (`Esc`).
    s.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    s.take_full_redraw();
    s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(s.emoji.is_none());
    assert!(
        s.take_full_redraw(),
        "cancelling closes the popup → a redraw"
    );
}

#[test]
fn render_with_emoji_picker_does_not_panic() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut s = ChatScreen::new();
    s.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    let mut term = Terminal::new(TestBackend::new(50, 16)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
}

#[test]
fn rag_command_intercepted_on_enter() {
    let mut s = ChatScreen::new();
    s.set_server_status(ready_statuses());
    type_str(&mut s, "/rag add d:\\docs -r");
    let intent = s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        intent,
        Some(ChatIntent::RagAdd {
            path: "d:\\docs".into(),
            recursive: true,
        })
    );
    assert!(s.input.is_empty(), "the field is cleared after the command");
}

#[test]
fn invalid_rag_command_shows_note_and_does_not_send() {
    let mut s = ChatScreen::new();
    s.set_server_status(ready_statuses());
    type_str(&mut s, "/rag");
    let intent = s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(intent, None, "an invalid command isn't sent");
    assert!(s.feed.iter().any(|m| m.role == FeedRole::Note));
}

#[test]
fn file_command_intercepted_on_enter() {
    let mut s = ChatScreen::new();
    s.set_server_status(ready_statuses());
    // A path with spaces (quoted) — the whole thing is one argument.
    type_str(&mut s, "/file attach \"d:\\my docs\\notes.md\"");
    let intent = s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        intent,
        Some(ChatIntent::FileAttach {
            path: "d:\\my docs\\notes.md".into(),
        })
    );
    assert!(s.input.is_empty(), "the field is cleared after the command");

    type_str(&mut s, "/file remove #2");
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(ChatIntent::FileRemove {
            target: "#2".into()
        })
    );
    type_str(&mut s, "/file list");
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(ChatIntent::FileList)
    );
}

#[test]
fn invalid_file_command_shows_note_and_does_not_send() {
    let mut s = ChatScreen::new();
    s.set_server_status(ready_statuses());
    type_str(&mut s, "/file attach");
    let intent = s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(intent, None, "an invalid command isn't sent as a message");
    // Errors go into the feed as a service note (there is no separate error role).
    assert!(s.feed.iter().any(|m| m.role == FeedRole::Note));
}

#[test]
fn attachment_chip_shows_count_and_standing_cost() {
    use crate::entities::attachment::{AttachMode, AttachmentInfo};
    let mut s = ChatScreen::new();
    assert_eq!(s.attachments_hint(), None, "no chip without attachments");

    s.set_attachments(vec![
        AttachmentInfo {
            name: "a.md".into(),
            bytes: 4096,
            est_tokens: 1200,
            prompt_tokens: 1200,
            mode: AttachMode::Inline,
        },
        // A by-reference file contributes only its excerpt, so it must not be
        // counted at full weight in the standing cost.
        AttachmentInfo {
            name: "big.log".into(),
            bytes: 900_000,
            est_tokens: 200_000,
            // Only the excerpt is actually re-sent every turn.
            prompt_tokens: 300,
            mode: AttachMode::ByReference,
        },
    ]);
    let hint = s.attachments_hint().expect("a chip with attachments");
    assert!(hint.contains('2'), "the count of files: {hint}");
    // The real per-request cost: the inline file in full (1200) plus the
    // by-reference excerpt (300) — not its whole 200k, and not zero either.
    assert!(hint.contains("1.5k"), "the standing cost: {hint}");
    assert!(
        !hint.contains("200k"),
        "a by-reference file must not be counted at full weight: {hint}"
    );
}

#[test]
fn file_list_note_numbers_items_for_removal() {
    use crate::entities::attachment::{AttachMode, AttachmentInfo};
    use crate::features::file_command::FileProgress;
    let mut s = ChatScreen::new();
    s.set_file_progress(FileProgress::Listed {
        items: vec![AttachmentInfo {
            name: "notes.md".into(),
            bytes: 2048,
            est_tokens: 400,
            prompt_tokens: 400,
            mode: AttachMode::Inline,
        }],
    });
    let note = s
        .feed
        .iter()
        .find(|m| m.role == FeedRole::Note)
        .expect("a note in the feed");
    // The `#N` handle is what `/file remove #N` accepts.
    assert!(note.text.contains("#1"), "{}", note.text);
    assert!(note.text.contains("notes.md"), "{}", note.text);
}

/// A staged-image card for the tests — `est_tokens` is what the chip and the
/// listing actually add up, so it is set explicitly rather than derived.
fn image_info(
    name: &str,
    bytes: usize,
    est_tokens: usize,
) -> crate::entities::message_image::ImageInfo {
    crate::entities::message_image::ImageInfo {
        name: name.into(),
        mime: "image/png".into(),
        width: 800,
        height: 600,
        bytes,
        est_tokens,
    }
}

#[test]
fn image_command_intercepted_on_enter() {
    let mut s = ChatScreen::new();
    s.set_server_status(ready_statuses());
    // A path with spaces (quoted) — the whole thing is one argument.
    type_str(&mut s, "/image attach \"d:\\my pics\\chart 1.png\"");
    let intent = s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        intent,
        Some(ChatIntent::ImageAttach {
            path: "d:\\my pics\\chart 1.png".into(),
        })
    );
    assert!(s.input.is_empty(), "the field is cleared after the command");
    assert!(
        !s.feed.iter().any(|m| m.role == FeedRole::User),
        "a command must not go out as a message"
    );

    type_str(&mut s, "/image remove #2");
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(ChatIntent::ImageRemove {
            target: "#2".into()
        })
    );
    type_str(&mut s, "/image list");
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(ChatIntent::ImageList)
    );
    assert!(!s.feed.iter().any(|m| m.role == FeedRole::User));
}

/// The two ways to paste an image differ in exactly one thing, and it is the thing that
/// matters: `Ctrl+V` must still paste **text** when the clipboard holds no image (that is
/// what the help overlay has always promised the key does), while a typed `/image paste`
/// must not silently turn into a text paste — the user asked for an image.
#[test]
fn both_paste_routes_produce_an_intent_and_differ_only_in_the_text_fallback() {
    let mut s = ChatScreen::new();
    s.set_server_status(ready_statuses());

    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL)),
        Some(ChatIntent::PasteImage {
            text_fallback: true
        }),
        "Ctrl+V keeps working as a text paste when there is no image"
    );

    type_str(&mut s, "/image paste");
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(ChatIntent::PasteImage {
            text_fallback: false
        })
    );
    assert!(s.input.is_empty(), "the field is cleared after the command");
    assert!(
        !s.feed.iter().any(|m| m.role == FeedRole::User),
        "a command must not go out as a message"
    );
}

/// `/image paste` is a command like the others, so the input box must highlight it and
/// skip spellcheck on it — otherwise it reads as a misspelled sentence while being typed.
#[test]
fn image_paste_is_recognized_as_a_command_while_typing() {
    let mut s = ChatScreen::new();
    s.set_server_status(ready_statuses());
    type_str(&mut s, "/image paste");
    assert!(s.input_is_command());
}

#[test]
fn invalid_image_command_shows_note_and_does_not_send() {
    let mut s = ChatScreen::new();
    s.set_server_status(ready_statuses());
    type_str(&mut s, "/image attach");
    let intent = s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(intent, None, "an invalid command isn't sent as a message");
    // Errors go into the feed as a service note (there is no separate error role).
    assert!(s.feed.iter().any(|m| m.role == FeedRole::Note));
    assert!(!s.feed.iter().any(|m| m.role == FeedRole::User));
}

#[test]
fn image_input_is_recognized_as_command() {
    let mut s = ChatScreen::new();
    type_str(&mut s, "/image attach a.png");
    assert!(
        s.input_is_command(),
        "the input box highlights it and skips spellcheck"
    );
    // A malformed one is recognized too — it's still a command, not prose.
    s.input.clear();
    type_str(&mut s, "/image");
    assert!(s.input_is_command());
    // Prose that merely mentions the word isn't.
    s.input.clear();
    type_str(&mut s, "look at the image below");
    assert!(!s.input_is_command());
}

#[test]
fn staged_image_chip_shows_count_and_standing_cost() {
    let mut s = ChatScreen::new();
    assert_eq!(
        s.staged_images_hint(),
        None,
        "no chip without staged images"
    );

    s.set_staged_images(vec![
        image_info("chart.png", 120_000, 1200),
        image_info("photo.jpg", 300_000, 300),
    ]);
    let hint = s.staged_images_hint().expect("a chip with staged images");
    assert!(hint.contains('2'), "the count of images: {hint}");
    // The cost of the next turn: both images together (1200 + 300).
    assert!(hint.contains("1.5k"), "the cost of the next send: {hint}");
    // Sending clears the staging — the chip goes with it (the orchestrator
    // reports an empty set), unlike the attachments chip which stays.
    s.set_staged_images(Vec::new());
    assert_eq!(s.staged_images_hint(), None, "the chip goes with the send");
}

#[test]
fn image_list_note_numbers_items_for_removal() {
    use crate::features::image_command::ImageProgress;
    let mut s = ChatScreen::new();
    s.set_image_progress(ImageProgress::Listed {
        items: vec![
            image_info("chart.png", 120_000, 1200),
            image_info("photo.jpg", 4096, 400),
        ],
    });
    let note = s
        .feed
        .iter()
        .find(|m| m.role == FeedRole::Note)
        .expect("a note in the feed");
    // The `#N` handles are what `/image remove #N` accepts, in listing order.
    assert!(note.text.contains("#1 chart.png"), "{}", note.text);
    assert!(note.text.contains("#2 photo.jpg"), "{}", note.text);
    let first = note.text.find("#1").unwrap();
    let second = note.text.find("#2").unwrap();
    assert!(
        first < second,
        "numbering follows the listing: {}",
        note.text
    );
}

#[test]
fn empty_image_list_says_sent_images_are_not_staged() {
    use crate::features::image_command::ImageProgress;
    let mut s = ChatScreen::new();
    s.set_image_progress(ImageProgress::Listed { items: Vec::new() });
    let note = s
        .feed
        .iter()
        .find(|m| m.role == FeedRole::Note)
        .expect("a note in the feed");
    // The empty listing is not "you have no images" — it has to distinguish
    // staged-for-the-next-message from already sent, or `/image remove` looks
    // broken to whoever wants an image out of the conversation.
    assert_eq!(note.text, s.loc.t("ui.image.list_empty"), "{}", note.text);
}

#[test]
fn rag_progress_banner_lifecycle() {
    let mut s = ChatScreen::new();
    assert!(!s.is_rag_active());
    s.set_rag_progress(RagProgress::Started { total: 3 });
    assert!(s.is_rag_active());
    // The file has just started (chunks_total=0) — no chunk suffix in the banner.
    s.set_rag_progress(RagProgress::Indexing {
        index: 1,
        total: 3,
        name: "a.txt".into(),
        dir: "d:\\docs".into(),
        chunks_done: 0,
        chunks_total: 0,
    });
    assert!(s.is_rag_active());
    assert!(
        !s.rag.as_ref().unwrap().text.contains("чанки"),
        "with no chunks_total there must be no chunk suffix: {:?}",
        s.rag.as_ref().unwrap().text
    );
    // Chunk progress (chunks_total>0) — the banner carries the chunk counter.
    s.set_rag_progress(RagProgress::Indexing {
        index: 1,
        total: 3,
        name: "a.txt".into(),
        dir: "d:\\docs".into(),
        chunks_done: 16,
        chunks_total: 42,
    });
    let banner = s.rag.as_ref().unwrap().text.clone();
    assert!(
        banner.contains("16") && banner.contains("42"),
        "the banner must show chunks 16/42: {banner:?}"
    );
    s.set_rag_progress(RagProgress::Finished {
        files: 3,
        chunks: 9,
        errors: 0,
        cancelled: false,
    });
    assert!(!s.is_rag_active(), "the banner clears on completion");
    assert!(
        s.feed
            .iter()
            .any(|m| m.role == FeedRole::Note && m.text.contains("завершена"))
    );
}

#[test]
fn command_input_is_not_spellchecked() {
    let mut s = ChatScreen::new();
    s.set_spellchecker(mk_checker());
    // Plain text with an error → checked (underlines are present).
    type_str(&mut s, "helo");
    s.last_edit = None; // lift the debounce so the recheck runs immediately
    assert!(s.maybe_recheck_spelling());
    assert!(!s.input.misspelled_is_empty(), "plain text is spellchecked");
    // Turn the line into a command — spellcheck is lifted.
    s.input.clear();
    type_str(&mut s, "/rag add helo");
    s.last_edit = None;
    assert!(s.input_is_command());
    assert!(s.maybe_recheck_spelling());
    assert!(
        s.input.misspelled_is_empty(),
        "a command isn't spellchecked"
    );
}

#[test]
fn rag_remove_command_intercepted_on_enter() {
    let mut s = ChatScreen::new();
    type_str(&mut s, "/rag remove d:\\dir\\file.txt");
    let intent = s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        intent,
        Some(ChatIntent::RagDelete {
            path: "d:\\dir\\file.txt".into(),
        })
    );
    assert!(s.input.is_empty());
}

#[test]
fn rag_removed_progress_pushes_note() {
    let mut s = ChatScreen::new();
    s.set_rag_progress(RagProgress::Removed { chunks: 5 });
    assert!(
        s.feed
            .iter()
            .any(|m| m.role == FeedRole::Note && m.text.contains("удалено фрагментов: 5"))
    );
    // Zero — a clear "nothing found" note.
    s.set_rag_progress(RagProgress::Removed { chunks: 0 });
    assert!(
        s.feed
            .iter()
            .any(|m| m.role == FeedRole::Note && m.text.contains("ничего не найдено"))
    );
}

#[test]
fn rag_list_and_rebuild_commands_intercepted_on_enter() {
    let mut s = ChatScreen::new();
    type_str(&mut s, "/rag list");
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(ChatIntent::RagList)
    );
    assert!(s.input.is_empty());

    type_str(&mut s, "/rag rebuild");
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(ChatIntent::RagRebuild)
    );
}

#[test]
fn rag_listed_progress_pushes_note() {
    use crate::entities::rag::RagSourceInfo;
    let mut s = ChatScreen::new();
    // An empty knowledge base — a clear note.
    s.set_rag_progress(RagProgress::Listed { sources: vec![] });
    assert!(
        s.feed
            .iter()
            .any(|m| m.role == FeedRole::Note && m.text.contains("база знаний пуста"))
    );
    // With sources — a chunk counter and the source name.
    s.set_rag_progress(RagProgress::Listed {
        sources: vec![RagSourceInfo {
            source: "spec.md".into(),
            chunks: 42,
            created_at: chrono::Utc::now(),
        }],
    });
    assert!(
        s.feed.iter().any(|m| m.role == FeedRole::Note
            && m.text.contains("spec.md")
            && m.text.contains("42"))
    );
}

#[test]
fn rag_failure_pushes_error_note() {
    let mut s = ChatScreen::new();
    s.set_rag_progress(RagProgress::Failed("эмбеддер недоступен".into()));
    assert!(!s.is_rag_active());
    assert!(
        s.feed
            .iter()
            .any(|m| m.role == FeedRole::Note && m.text.contains("эмбеддер недоступен"))
    );
}

#[test]
fn render_does_not_panic() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut s = ChatScreen::new();
    s.push_user_message("привет".into());
    let id = gen_id();
    s.begin_generation(id);
    s.push_chunk(id, "# Ответ\n\nтекст");
    let mut term = Terminal::new(TestBackend::new(50, 16)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
}

/// The lockup in the help dialog's header is drawn when the terminal height allows both
/// the mark and the full key list to fit; the mark is left-aligned to the list margin (docs/branding.md §5).
#[test]
fn help_shows_logo_when_terminal_is_tall() {
    use crate::widgets::logo::{LOCKUP_ROWS, LOGO_COLS};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    const ORANGE: Color = Color::Rgb(0xc2, 0x5a, 0x27);

    let mut s = ChatScreen::new();
    s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    // Margin: the border (2) + all the keys + the lockup block (top and bottom breathing room).
    let tall = HELP_KEYS.len() as u16 + 2 + LOCKUP_ROWS + 2;
    let mut term = Terminal::new(TestBackend::new(90, tall)).unwrap();
    term.draw(|f| s.render(f)).unwrap();

    let buf = term.backend().buffer();
    let mut orange: Vec<(u16, u16)> = Vec::new();
    for y in buf.area.top()..buf.area.bottom() {
        for x in buf.area.left()..buf.area.right() {
            let c = &buf[(x, y)];
            if c.style().fg == Some(ORANGE) || c.style().bg == Some(ORANGE) {
                orange.push((x, y));
            }
        }
    }
    // The glyph's brand-color stem is in every one of its rows, plus "fork" in the word.
    assert!(
        orange.len() > LOCKUP_ROWS as usize,
        "too little brand color — the mark isn't drawn (found {})",
        orange.len()
    );
    // The popup's left border: a rounded corner (default palette — Auto). The chat's
    // own panels are drawn full-width, i.e. their corners sit in column 0; the popup
    // is centered and inset, so its corner is the rightmost one found.
    let corner = (buf.area.top()..buf.area.bottom())
        .flat_map(|y| (buf.area.left()..buf.area.right()).map(move |x| (x, y)))
        .filter(|&(x, y)| buf[(x, y)].symbol() == "╭")
        .max_by_key(|&(x, _)| x)
        .expect("the popup's border");
    // The mark is left-aligned: the glyph's stem (columns 4-5 of its ink) sits exactly on the
    // key list's margin — the border + two spaces. Centering would have shifted it right.
    let left = orange.iter().map(|(x, _)| *x).min().unwrap();
    assert_eq!(
        left,
        corner.0 + 1 + 2 + 4,
        "the glyph's stem is not on the key list's left margin"
    );
    // The wordmark's "fork" — to the right of the glyph, past its right edge.
    let right = orange.iter().map(|(x, _)| *x).max().unwrap();
    assert!(
        right > corner.0 + 1 + 2 + LOGO_COLS,
        "the wordmark's \"fork\" is not drawn to the right of the glyph"
    );
}

/// On a short terminal the logo isn't drawn at all — the key list doesn't shift
/// and doesn't need extra scrolling (a hard degradation, docs/branding.md §5).
#[test]
fn help_hides_logo_when_terminal_is_short() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    const ORANGE: Color = Color::Rgb(0xc2, 0x5a, 0x27);

    let mut s = ChatScreen::new();
    s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    // A dialog height of 13 (11 rows inside) doesn't fit the lockup with breathing room → it
    // isn't drawn, the tabs don't shift down.
    let mut term = Terminal::new(TestBackend::new(90, 13)).unwrap();
    term.draw(|f| s.render(f)).unwrap();

    let buf = term.backend().buffer();
    for y in buf.area.top()..buf.area.bottom() {
        for x in buf.area.left()..buf.area.right() {
            let c = &buf[(x, y)];
            assert_ne!(c.style().fg, Some(ORANGE), "the logo must not be drawn");
            assert_ne!(c.style().bg, Some(ORANGE), "the logo must not be drawn");
        }
    }
}
/// The tab strip is one line and the dialog has a fixed width, so a tab label
/// that is a few columns too long in *some* locale silently truncates the last
/// tab — and the tab that gets cut is the rightmost one, which nobody looking
/// at the developer's locale would notice. Adding the "Disclaimer" tab pushed
/// the `ru` strip six columns over the edge, and the fix was to shorten the
/// hotkeys label in `locales/ru.json` — so the budget is checked for every
/// bundled locale rather than left to luck.
#[test]
fn the_help_tab_strip_fits_the_dialog_in_every_locale() {
    use super::popups::{HELP_MIN_WIDTH, help_tab_strip};
    use crate::shared::i18n::{Lang, locale};

    // The budget is the dialog's MINIMUM width: the strip has to fit the
    // smallest window the adaptive sizing ever grants.
    let palette = Palette::default();
    for lang in Lang::ALL {
        let strip = help_tab_strip(HelpTab::About, &palette, locale(*lang));
        let chars: Vec<char> = strip.spans.iter().flat_map(|s| s.content.chars()).collect();
        let width = crate::shared::wrap::display_width(&chars);
        assert!(
            width <= HELP_MIN_WIDTH as usize,
            "the {} tab strip is {width} columns wide, the dialog is {HELP_MIN_WIDTH}",
            lang.code()
        );
    }
}

/// A level-1 markdown heading is accent + bold + **underlined**, and the writer
/// puts that on the `Line` rather than on its spans — so the "Disclaimer" tab's
/// left indent inherited it and the underline visibly ran out to the left of the
/// heading's text. The style belongs on the content spans; the indent stays
/// blank. Reported from a real screenshot.
#[test]
fn the_disclaimer_indent_does_not_inherit_the_heading_style() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Modifier;

    let mut s = ChatScreen::new();
    s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    s.help.as_mut().unwrap().tab = HelpTab::Disclaimer;
    let mut term = Terminal::new(TestBackend::new(90, 40)).unwrap();
    term.draw(|f| s.render(f)).unwrap();

    let buf = term.backend().buffer();
    // Find the heading row and the column its `#` marker starts at.
    let (y, x) = (buf.area.top()..buf.area.bottom())
        .find_map(|y| {
            (buf.area.left()..buf.area.right())
                .find(|&x| buf[(x, y)].symbol() == "#")
                .map(|x| (y, x))
        })
        .expect("the disclaimer heading is not on screen");
    assert!(
        buf[(x, y)]
            .style()
            .add_modifier
            .contains(Modifier::UNDERLINED),
        "the heading itself lost its underline"
    );
    for dx in 1..=2 {
        let cell = &buf[(x - dx, y)];
        assert_eq!(cell.symbol(), " ", "the indent is not blank");
        assert!(
            !cell.style().add_modifier.contains(Modifier::UNDERLINED),
            "the indent column {dx} left of the heading is underlined"
        );
    }
}

/// Every tab of the help dialog draws its own distinctive content, and the tab strip
/// carries all six tabs.
#[test]
fn help_tabs_render_distinct_content() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let text_for = |tab: HelpTab| -> String {
        let mut s = ChatScreen::new();
        s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        s.help.as_mut().unwrap().tab = tab;
        let mut term = Terminal::new(TestBackend::new(90, 40)).unwrap();
        term.draw(|f| s.render(f)).unwrap();
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

    // The tab strip carries all six labels on any tab.
    let about = text_for(HelpTab::About);
    for label in [
        "О программе",
        "Клавиши",
        "Команды",
        "Лицензия",
        "Дисклеймер",
        "Компоненты",
    ] {
        assert!(about.contains(label), "missing the \"{label}\" tab label");
    }
    // "About": the brand name, author, version, links.
    assert!(about.contains("Vladimir Shylov"), "missing the author");
    assert!(
        about.contains(env!("CARGO_PKG_VERSION")),
        "missing the version"
    );
    assert!(
        about.contains("https://mindfork.io"),
        "missing the site link"
    );
    assert!(
        about.contains("https://crates.io/crates/mindfork"),
        "missing the crate link"
    );

    // "Hotkeys": a label from HELP_KEYS, but NOT commands (they're on their own tab).
    let hotkeys = text_for(HelpTab::Hotkeys);
    assert!(
        hotkeys.contains("отправить сообщение"),
        "missing a key description"
    );
    assert!(
        !hotkeys.contains("/rag add"),
        "commands must not be on the hotkeys tab"
    );

    // "Commands": input-box commands.
    let commands = text_for(HelpTab::Commands);
    assert!(
        commands.contains("/rag add"),
        "missing the /rag add command"
    );
    assert!(commands.contains("/tts"), "missing the /tts command");
    // Attachments are listed FIRST — they're the commands used while writing a
    // message (docs/file-attachments.md §4.8).
    assert!(
        commands.contains("/file attach"),
        "missing the /file attach command"
    );
    assert!(
        commands.find("/file attach") < commands.find("/rag add"),
        "the /file commands must come before /rag: {commands}"
    );

    // "License": the MIT text.
    let license = text_for(HelpTab::License);
    assert!(
        license.contains("MIT License"),
        "missing the license header"
    );
    assert!(license.contains("WARRANTY"), "missing the license body");
    // The disclaimer is a separate tab, not a tail on the license: the MIT text
    // must stay pure (see `credits::LICENSE_TEXT`).
    assert!(
        !license.contains("Disclaimer"),
        "the disclaimer leaked into the license tab"
    );

    // "Disclaimer": the model-output notice, rendered through our own markdown
    // renderer — headings keep their styled `#` prefix (that is the renderer's
    // house style), but emphasis markers are consumed, which is what tells us
    // the text went through the renderer rather than being dumped verbatim.
    let disclaimer = text_for(HelpTab::Disclaimer);
    assert!(
        disclaimer.contains("Disclaimer"),
        "missing the disclaimer heading"
    );
    assert!(
        disclaimer.contains("mindfork is a client"),
        "missing the disclaimer body"
    );
    assert!(
        !disclaimer.contains("**"),
        "raw markdown emphasis markers on screen — the renderer was bypassed"
    );

    // "Components": name, version, and license (taken from the start of the list — it's
    // long and scrolls, distant crates are off-screen).
    let components = text_for(HelpTab::Components);
    assert!(components.contains("ansi-to-tui"), "missing the component");
    assert!(
        components.contains("8.0.1"),
        "missing the component version"
    );
    assert!(
        components.contains("Zlib OR Apache-2.0 OR MIT"),
        "missing the component license"
    );
}

/// The vendored syntax grammars are third-party data we redistribute, so the
/// "Components" tab must name them and their licences — like the crates above.
/// They sit at the end of a long list, so this scrolls to the bottom rather
/// than reading the first screen (see `syntaxes/SOURCES.md`).
#[test]
fn components_tab_lists_the_vendored_grammars() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut s = ChatScreen::new();
    s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    let help = s.help.as_mut().unwrap();
    help.tab = HelpTab::Components;
    help.scroll = usize::MAX / 2; // render() clamps to the last page
    let mut term = Terminal::new(TestBackend::new(100, 40)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
    let buf = term.backend().buffer();
    let mut out = String::new();
    for y in buf.area.top()..buf.area.bottom() {
        for x in buf.area.left()..buf.area.right() {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }

    assert!(
        out.contains("грамматики"),
        "missing the grammars section header: {out}"
    );
    // The last row of the manifest — whichever it is, it must be on the last page.
    let (lang, repo, licence) = *crate::shared::credits::GRAMMARS
        .last()
        .expect("the manifest lists grammars");
    assert!(out.contains(lang), "missing the grammar {lang}: {out}");
    assert!(out.contains(repo), "missing its upstream {repo}");
    assert!(out.contains(licence), "missing its licence {licence}");
}

/// In-feed search must never touch the message being written — that is the whole
/// reason it lives in its own field (docs/history/in-feed-search.md §3, fork F3).
#[test]
fn ctrl_f_opens_feed_search_and_esc_closes_it_leaving_the_message_alone() {
    let mut s = ChatScreen::new();
    type_str(&mut s, "недописанное сообщение");

    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL)),
        None
    );
    assert!(s.search.is_some(), "Ctrl+F opens the search field");
    type_str(&mut s, "маркер");
    assert_eq!(
        s.input.text(),
        "недописанное сообщение",
        "typing a query must not reach the message box"
    );

    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        None
    );
    assert!(s.search.is_none(), "Esc closes it");
    assert_eq!(
        s.input.text(),
        "недописанное сообщение",
        "and leaves the message"
    );
}

/// Layout-independent, like every other Ctrl shortcut: physical F is `Ctrl+а`
/// on a Russian layout.
#[test]
fn ctrl_f_opens_feed_search_under_a_cyrillic_layout() {
    let mut s = ChatScreen::new();
    s.handle_key(KeyEvent::new(KeyCode::Char('а'), KeyModifiers::CONTROL));
    assert!(s.search.is_some());
}

/// **The trap that slow manual testing cannot see** (§1.5): a run of characters
/// arrives as one coalesced paste, and without its own target it would land in
/// the message the user was writing.
#[test]
fn fast_typing_reaches_the_search_field_not_the_message() {
    let mut s = ChatScreen::new();
    type_str(&mut s, "черновик");
    s.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));

    s.handle_paste("быстрый набор");
    assert_eq!(
        s.search.as_ref().map(|f| f.text()),
        Some("быстрый набор".to_string()),
        "a coalesced paste belongs to the search field while it is open"
    );
    assert_eq!(s.input.text(), "черновик", "and must not touch the message");
}

/// `Ctrl+E`/`Ctrl+R`/a rewrite round/a cross-chat jump all funnel through
/// `activate_chat`, which renumbers the feed — so a search left open would point
/// at messages that moved (§1.5).
#[test]
fn activating_a_chat_closes_the_search() {
    let mut s = ChatScreen::new();
    s.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
    type_str(&mut s, "маркер");
    assert!(s.search.is_some());

    s.activate_chat(
        gen_id(),
        "Чат".into(),
        &[],
        "",
        FeedView::default(),
        None,
        None,
    );
    assert!(
        s.search.is_none(),
        "the search must not survive a feed rebuild"
    );
}

/// Reopening resumes the query, so a second `Ctrl+F` continues rather than
/// starting over (fork F5) — but only within the same chat.
#[test]
fn reopening_the_search_resumes_the_query() {
    let mut s = ChatScreen::new();
    s.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
    type_str(&mut s, "маркер");
    s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    s.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
    assert_eq!(
        s.search.as_ref().map(|f| f.text()),
        Some("маркер".to_string())
    );
}

// ---------- history compaction (spec §6.7) ----------

/// The divider's localized label — how the boundary is found in a rendered feed.
fn compacted_label() -> &'static str {
    crate::shared::i18n::locale(crate::shared::i18n::Lang::default()).t("ui.feed.compacted")
}

/// Renders the screen and returns its rows as strings.
fn screen_rows(s: &mut ChatScreen, w: u16, h: u16) -> Vec<String> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
    let buf = term.backend().buffer().clone();
    let area = buf.area;
    (area.top()..area.bottom())
        .map(|y| {
            (area.left()..area.right())
                .map(|x| buf[(x, y)].symbol())
                .collect()
        })
        .collect()
}

/// Activation carries the chat's boundary into the feed: the compaction is a
/// per-chat rendering input, so switching chats has to bring it along or a
/// summary would linger from the previous one.
#[test]
fn activation_carries_the_compaction_boundary() {
    let messages: Vec<Message> = (0..4)
        .map(|i| Message::user(format!("реплика-{i}")))
        .collect();
    let boundary = messages[2].id;
    let mut s = ChatScreen::new();
    s.activate_chat(
        gen_id(),
        "Чат".into(),
        &messages,
        "",
        FeedView::default(),
        None,
        Some((boundary, "ранее обсудили X".into())),
    );
    let rows = screen_rows(&mut s, 60, 24);
    assert!(
        rows.iter().any(|r| r.contains(compacted_label())),
        "the divider must be drawn for the activated chat: {rows:?}"
    );

    // Switching to a chat with nothing folded takes it away again.
    s.activate_chat(
        gen_id(),
        "Другой".into(),
        &messages,
        "",
        FeedView::default(),
        None,
        None,
    );
    let rows = screen_rows(&mut s, 60, 24);
    assert!(
        !rows.iter().any(|r| r.contains(compacted_label())),
        "the previous chat's boundary must not linger: {rows:?}"
    );
}

/// A live compaction (`AppEvent::Compacted`) reaches the feed without a
/// reactivation — the boundary appears on the chat the user is looking at.
#[test]
fn a_live_compaction_reaches_the_feed() {
    let messages: Vec<Message> = (0..4)
        .map(|i| Message::user(format!("реплика-{i}")))
        .collect();
    let boundary = messages[2].id;
    let mut s = ChatScreen::new();
    s.activate_chat(
        gen_id(),
        "Чат".into(),
        &messages,
        "",
        FeedView::default(),
        None,
        None,
    );
    assert!(
        !screen_rows(&mut s, 60, 24)
            .iter()
            .any(|r| r.contains(compacted_label()))
    );

    s.set_compaction(Some((boundary, "ранее обсудили X".into())));
    let rows = screen_rows(&mut s, 60, 24);
    assert!(
        rows.iter().any(|r| r.contains(compacted_label())),
        "the divider must appear without reactivating the chat: {rows:?}"
    );
}

/// The quiet background indicator joins the same `·`-separated list the other
/// background tasks use — they can run at the same time, so it must compose
/// rather than replace.
#[test]
fn compaction_shows_a_quiet_background_indicator() {
    let mut s = ChatScreen::new();
    assert_eq!(s.background_hint(), None);

    s.set_compacting(true);
    let hint = s.background_hint().expect("the indicator must be shown");
    assert_eq!(hint, s.loc().t("ui.chat.bg.compact"));

    // Composes with a concurrent task rather than replacing it.
    s.set_reflecting(true);
    let hint = s.background_hint().unwrap();
    assert!(hint.contains(s.loc().t("ui.chat.bg.reflect")), "{hint}");
    assert!(hint.contains(s.loc().t("ui.chat.bg.compact")), "{hint}");
    assert!(hint.contains(" · "), "{hint}");

    s.set_compacting(false);
    let hint = s.background_hint().unwrap();
    assert!(!hint.contains(s.loc().t("ui.chat.bg.compact")), "{hint}");
}

/// The retry chip (spec §6.8): it carries the numbers, it composes with the
/// background tasks, and — the part that matters — it cannot outlive the wait it
/// describes.
#[test]
fn the_retry_chip_shows_the_numbers_and_is_cleared_by_what_ends_the_wait() {
    let mut s = ChatScreen::new();
    let gen_id = Uuid::new_v4();
    s.begin_generation(gen_id);
    assert_eq!(s.background_hint(), None);

    s.set_retrying(gen_id, 2, 3, 4);
    let hint = s.background_hint().expect("the chip must be shown");
    for part in ["2", "3", "4"] {
        assert!(
            hint.contains(part),
            "the numbers must reach the chip: {hint}"
        );
    }

    // Composes with a concurrent background task rather than replacing it.
    s.set_reflecting(true);
    let hint = s.background_hint().unwrap();
    assert!(hint.contains(s.loc().t("ui.chat.bg.reflect")), "{hint}");
    assert!(hint.contains(" · "), "{hint}");

    // Content ends the wait, so the chip goes.
    s.push_chunk(gen_id, "answer");
    let hint = s.background_hint().unwrap();
    assert!(
        !hint.contains('4'),
        "content must clear the chip, leaving only the background task: {hint}"
    );

    // And so does the turn finishing, from the other direction.
    s.set_retrying(gen_id, 3, 3, 2);
    assert!(s.background_hint().unwrap().contains('3'));
    s.finish_generation(gen_id, FinishReason::Error);
    let hint = s.background_hint().unwrap();
    assert!(
        !hint.contains('3'),
        "a finished turn must not leave a chip behind: {hint}"
    );
}

/// A chip from a turn the user has already cancelled must not appear on the next
/// one — the same staleness rule every streamed event follows (spec §4.4).
#[test]
fn a_retry_chip_from_a_stale_generation_is_dropped() {
    let mut s = ChatScreen::new();
    let current = Uuid::new_v4();
    s.begin_generation(current);

    s.set_retrying(Uuid::new_v4(), 2, 3, 9);
    assert_eq!(
        s.background_hint(),
        None,
        "a chip for another generation must be ignored"
    );
}

// ---------- the quit commands (`/exit`, `/quit`, spec §11.7) ----------

/// Both spellings quit from the input box — the third route out, for terminals
/// that keep `Ctrl+Q` and `F10` for themselves.
#[test]
fn exit_commands_quit_from_the_input_box() {
    for cmd in crate::features::exit_command::ALIASES {
        let mut s = ChatScreen::new();
        type_str(&mut s, cmd);
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(ChatIntent::Quit),
            "{cmd}"
        );
        // The command must not survive as the chat's draft: it is cleared here,
        // and `app::runtime::run_loop` flushes this last state on its way out.
        assert!(s.input.is_empty(), "{cmd} left text in the box");
        assert!(
            s.take_dirty_draft().is_some_and(|d| d.is_empty()),
            "{cmd} must hand an empty draft back for saving"
        );
    }
}

/// Quitting during generation works, exactly as `Ctrl+Q`/`F10` do — the command
/// is checked before the `generating` gate that holds back ordinary messages.
#[test]
fn exit_command_quits_while_generating() {
    let mut s = ChatScreen::new();
    s.begin_generation(gen_id());
    type_str(&mut s, "/exit");
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(ChatIntent::Quit)
    );

    // The control: an ordinary message IS held back while generating, so the
    // assertion above is about the command and not about a missing gate.
    let mut s = ChatScreen::new();
    s.begin_generation(gen_id());
    type_str(&mut s, "/exit please");
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        None
    );
}

/// A mistyped quit leaves a note instead of quitting — and instead of going out
/// to the model, which is what an unrecognized `/…` line would do.
#[test]
fn a_mistyped_quit_command_notes_and_does_not_send() {
    let mut s = ChatScreen::new();
    type_str(&mut s, "/quit now");
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        None,
        "a malformed command is neither sent nor obeyed"
    );
    let note = s
        .feed
        .iter()
        .rev()
        .find(|m| m.role == FeedRole::Note)
        .expect("a note explaining the syntax");
    // The note has to close the door (docs/lessons.md §4): it names the route
    // that does work, not only the mistake.
    assert!(note.text.contains("/quit"), "{}", note.text);
    assert!(note.text.contains("Ctrl+Q"), "{}", note.text);
}

/// The words themselves are ordinary things to say to a model, and a longer
/// command that merely starts with one is not a quit command either.
#[test]
fn text_that_only_resembles_a_quit_command_is_sent() {
    for text in ["exit", "how do I quit vim?", "/exits"] {
        let mut s = ChatScreen::new();
        type_str(&mut s, text);
        assert_eq!(
            s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(ChatIntent::Send(text.to_string())),
            "{text}"
        );
    }
}

// ---------- `chat://` references (spec §11.3) ----------

/// A chat card for the address book the screen keeps.
fn card(id: Uuid, profile: Uuid, title: &str) -> ChatSummary {
    ChatSummary {
        id,
        profile_id: profile,
        title: title.into(),
        created_at: chrono::Utc::now(),
        modified_at: chrono::Utc::now(),
        message_count: 1,
    }
}

/// Puts `s` on `here` with `others` also in the list, all of one profile, and
/// an assistant reply citing every id in `cited`.
fn screen_citing(here: Uuid, others: &[Uuid], cited: &[Uuid]) -> ChatScreen {
    let profile = Uuid::new_v4();
    let mut s = ChatScreen::new();
    let mut list = vec![card(here, profile, "Текущий")];
    list.extend(
        others
            .iter()
            .enumerate()
            .map(|(i, id)| card(*id, profile, &format!("Прошлый {i}"))),
    );
    let text = cited
        .iter()
        .map(|id| crate::features::chat_links::uri(*id))
        .collect::<Vec<_>>()
        .join(" и ");
    s.activate_chat(
        here,
        "Текущий".into(),
        &[Message::assistant(format!("см. {text}"))],
        "",
        FeedView::default(),
        None,
        None,
    );
    // After activation, as it arrives in practice: the orchestrator emits the
    // list on its own schedule.
    s.set_chat_list(list);
    s
}

/// The cache the picker reads is only built by a render, so every test that
/// asks for references draws a frame first — the same constraint the jump obeys.
fn draw(s: &mut ChatScreen) {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut term = Terminal::new(TestBackend::new(60, 16)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// `Ctrl+L` opens the picker on the cited conversation, and `Enter` follows it.
#[test]
fn ctrl_l_follows_a_cited_conversation() {
    let (here, other) = (gen_id(), gen_id());
    let mut s = screen_citing(here, &[other], &[other]);
    draw(&mut s);

    assert_eq!(s.handle_key(ctrl('l')), None, "the picker opens in place");
    assert!(s.chat_links.is_some());
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(ChatIntent::OpenChatLink(other))
    );
    assert!(s.chat_links.is_none(), "the picker closes on a pick");
}

/// Nothing to follow is not an empty list — it says what a reference looks
/// like (docs/lessons.md §4).
#[test]
fn ctrl_l_with_no_references_explains_instead_of_opening() {
    let mut s = screen_citing(gen_id(), &[], &[]);
    draw(&mut s);

    assert_eq!(s.handle_key(ctrl('l')), None);
    assert!(s.chat_links.is_none(), "an empty popup is not opened");
    let note = s
        .feed
        .iter()
        .rev()
        .find(|m| m.role == FeedRole::Note)
        .expect("a note about references");
    assert!(note.text.contains("chat://"), "{}", note.text);
}

/// A reference back to the open conversation is listed (the model wrote it)
/// but following it says where you already are instead of switching.
#[test]
fn a_reference_to_the_open_chat_says_so() {
    let here = gen_id();
    let mut s = screen_citing(here, &[], &[here]);
    draw(&mut s);

    s.handle_key(ctrl('l'));
    assert!(s.chat_links.is_some(), "the open chat is still offered");
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        None,
        "no switch to the chat we are on"
    );
    let note = s
        .feed
        .iter()
        .rev()
        .find(|m| m.role == FeedRole::Note)
        .expect("a note saying we are already here");
    assert!(!note.text.is_empty());
}

/// The profile boundary (spec §9.5) holds in the feed too: an address for
/// another companion's conversation resolves to nothing, so it is neither
/// drawn as a link nor offered.
#[test]
fn another_profiles_conversation_is_not_a_reference() {
    let (here, foreign) = (gen_id(), gen_id());
    let mut s = ChatScreen::new();
    s.activate_chat(
        here,
        "Текущий".into(),
        &[Message::assistant(format!(
            "см. {}",
            crate::features::chat_links::uri(foreign)
        ))],
        "",
        FeedView::default(),
        None,
        None,
    );
    s.set_chat_list(vec![
        card(here, Uuid::new_v4(), "Текущий"),
        card(foreign, Uuid::new_v4(), "Чужой профиль"),
    ]);
    draw(&mut s);

    assert!(s.feed_view.chat_links().is_empty());
    assert_eq!(s.handle_key(ctrl('l')), None);
    assert!(s.chat_links.is_none());
}

/// Switching chats must not leave the previous conversation's picker open.
#[test]
fn switching_chats_closes_the_picker() {
    let (here, other) = (gen_id(), gen_id());
    let mut s = screen_citing(here, &[other], &[other]);
    draw(&mut s);
    s.handle_key(ctrl('l'));
    assert!(s.chat_links.is_some());

    s.activate_chat(
        other,
        "Прошлый".into(),
        &[Message::user("привет")],
        "",
        FeedView::default(),
        None,
        None,
    );
    assert!(s.chat_links.is_none());
}

/// Stage 2 (spec §11.3): a left click on a drawn `chat://` address follows it,
/// and a click a column past it is ordinary feed again.
#[test]
fn a_click_on_a_chat_reference_opens_that_conversation() {
    use ratatui::crossterm::event::MouseButton;
    let (here, other) = (gen_id(), gen_id());
    let mut s = screen_citing(here, &[other], &[other]);
    draw(&mut s);
    let at = s
        .feed_view
        .link_hit_for_test(0)
        .expect("the address is drawn");

    assert_eq!(
        s.handle_mouse(mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            at.0,
            at.1
        )),
        Some(ChatIntent::OpenChatLink(other))
    );
    assert_eq!(
        s.handle_mouse(mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            at.2,
            at.1
        )),
        None,
        "the cell past the address is not the address"
    );
}

/// Clicking a reference back to the open conversation says so rather than
/// switching — the same rule (and wording) the picker follows.
#[test]
fn clicking_a_reference_to_the_open_chat_says_so() {
    use ratatui::crossterm::event::MouseButton;
    let here = gen_id();
    let mut s = screen_citing(here, &[], &[here]);
    draw(&mut s);
    let at = s
        .feed_view
        .link_hit_for_test(0)
        .expect("the address is drawn");

    assert_eq!(
        s.handle_mouse(mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            at.0,
            at.1
        )),
        None
    );
    assert!(
        s.feed
            .iter()
            .rev()
            .any(|m| m.role == FeedRole::Note && !m.text.is_empty()),
        "a note instead of a silent no-op"
    );
}

/// The click must not reach the feed while an overlay owns the screen — and the
/// reference picker is now one of them.
#[test]
fn a_click_is_ignored_while_the_picker_is_open() {
    use ratatui::crossterm::event::MouseButton;
    let (here, other) = (gen_id(), gen_id());
    let mut s = screen_citing(here, &[other], &[other]);
    draw(&mut s);
    let at = s
        .feed_view
        .link_hit_for_test(0)
        .expect("the address is drawn");
    s.handle_key(ctrl('l'));
    assert!(s.chat_links.is_some());

    assert_eq!(
        s.handle_mouse(mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            at.0,
            at.1
        )),
        None
    );
}

// ---------- typed routes for the chords (docs/research/command-only-control.md) ----------

use crate::features::ui_command::{self, Arity, UiCommand};

/// A chat screen driven the way a user drives it — type a line, press `Enter` —
/// with the state the typed routes need: an open chat, two profiles and a
/// settings snapshot.
///
/// A harness rather than a prologue per test: six tests spelling out the same
/// four setup lines is the sliding self-duplication the gate keeps catching
/// (docs/lessons.md §2, where the previous recurrence was exactly this shape).
struct Cmd {
    s: ChatScreen,
    chat: Uuid,
}

impl Cmd {
    /// A furnished screen: the common case. The ids are **fixed** rather than
    /// fresh, so two screens built here are genuinely identical — the
    /// command-versus-chord comparison below is between two of them, and random
    /// ids would make every intent that carries one differ.
    fn new() -> Self {
        let mut s = ChatScreen::new();
        let chat = Uuid::from_u128(1);
        s.activate_chat(
            chat,
            "Про космос".into(),
            &[Message::user("привет")],
            "",
            FeedView::default(),
            None,
            None,
        );
        s.set_profile_list(vec![
            ProfileSummary {
                id: Uuid::from_u128(2),
                name: "Гайя".into(),
            },
            ProfileSummary {
                id: Uuid::from_u128(3),
                name: "Гелиос".into(),
            },
        ]);
        s.set_settings(
            AppConfig::default(),
            vec![Profile::new("P", "sys")],
            Vec::new(),
            Default::default(),
            Vec::new(),
        );
        Self { s, chat }
    }

    /// A bare screen: no chat, no profiles, no settings snapshot yet — the state
    /// the app is in for the first moments after launch.
    fn bare() -> Self {
        Self {
            s: ChatScreen::new(),
            chat: Uuid::nil(),
        }
    }

    /// Types a command line and presses `Enter`.
    fn run(&mut self, line: &str) -> Option<ChatIntent> {
        type_str(&mut self.s, line);
        self.s
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    }

    fn ctrl(&mut self, c: char) -> Option<ChatIntent> {
        self.s
            .handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn key(&mut self, code: KeyCode) -> Option<ChatIntent> {
        self.s.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// The last note in the feed (the answer a refused command leaves).
    fn last_note(&self) -> String {
        self.s
            .feed
            .iter()
            .rev()
            .find(|m| m.role == FeedRole::Note)
            .map(|m| m.text.clone())
            .unwrap_or_default()
    }
}

/// The heart of the track: a command reaches the action through the very
/// handler its chord uses, so the two cannot grow two behaviours. Each pair is
/// run on two identical screens and must produce the same intent — and, where
/// the effect is in-screen rather than an intent, the same visible state.
#[test]
fn every_command_agrees_with_the_chord_it_mirrors() {
    let ctrl_pairs = [
        ("/settings", 'p'),
        ("/regen", 'r'),
        ("/takeback", 'e'),
        ("/new", 'n'),
        ("/thoughts", 't'),
        ("/toolcalls", 'o'),
        ("/mouse", 'w'),
        ("/links", 'l'),
    ];
    for (line, chord) in ctrl_pairs {
        let (mut typed, mut pressed) = (Cmd::new(), Cmd::new());
        assert_eq!(
            typed.run(line),
            pressed.ctrl(chord),
            "{line} and Ctrl+{chord} disagree"
        );
    }
    // The function keys, and `Esc` for the chat list.
    for (line, code) in [
        ("/self", KeyCode::F(3)),
        ("/copy", KeyCode::F(5)),
        ("/chats", KeyCode::Esc),
    ] {
        let (mut typed, mut pressed) = (Cmd::new(), Cmd::new());
        assert_eq!(
            typed.run(line),
            pressed.key(code),
            "{line} and {code:?} disagree"
        );
    }
    // In-screen effects have no intent to compare, so compare the state each
    // route leaves behind.
    let (mut typed, mut pressed) = (Cmd::new(), Cmd::new());
    assert_eq!(typed.run("/help"), None);
    pressed.key(KeyCode::F(1));
    assert_eq!(typed.s.help.is_some(), pressed.s.help.is_some());
    let (mut typed, mut pressed) = (Cmd::new(), Cmd::new());
    assert_eq!(typed.run("/emoji"), None);
    pressed.ctrl('b');
    assert_eq!(typed.s.emoji.is_some(), pressed.s.emoji.is_some());
    let (mut typed, mut pressed) = (Cmd::new(), Cmd::new());
    assert_eq!(typed.run("/find"), None);
    pressed.ctrl('f');
    assert_eq!(typed.s.search.is_some(), pressed.s.search.is_some());
}

/// Every command clears the input box and hands an **empty** draft back, or the
/// next launch would greet the user with the command they typed (the `/exit`
/// trap, journal ui-input.md). Driven off the registry, so a new row is covered
/// the moment it is added.
#[test]
fn every_command_clears_the_box_and_the_draft() {
    for spec in ui_command::COMMANDS {
        let mut c = Cmd::new();
        let line = match spec.arity {
            Arity::Required => format!("{} что-нибудь", spec.aliases[0]),
            _ => spec.aliases[0].to_string(),
        };
        c.run(&line);
        // `/rename` bare deliberately hands the current title back for editing;
        // it is the one command whose answer IS text in the box.
        if spec.command == UiCommand::Rename {
            continue;
        }
        assert!(c.s.input.is_empty(), "{line} left text in the box");
        assert!(
            c.s.take_dirty_draft().is_some_and(|d| d.is_empty()),
            "{line} must hand an empty draft back for saving"
        );
    }
}

/// Every registry command is highlighted while it is being typed — including a
/// malformed one, which is a command and not prose. What the box shows and what
/// `Enter` does come from the same parser, so they cannot disagree.
#[test]
fn commands_are_highlighted_as_they_are_typed() {
    for spec in ui_command::COMMANDS {
        for alias in spec.aliases {
            let mut c = Cmd::new();
            type_str(&mut c.s, alias);
            assert!(c.s.input_is_command(), "{alias} is not highlighted");
            // …and with a stray argument, which is still a command.
            type_str(&mut c.s, " nonsense");
            assert!(
                c.s.input_is_command(),
                "{alias} with an argument is not highlighted"
            );
        }
    }
    // A longer word that merely starts with a command name is prose, exactly as
    // it is on `Enter`.
    for text in ["/settings2", "/newest", "how do I stop this?"] {
        let mut c = Cmd::new();
        type_str(&mut c.s, text);
        assert!(!c.s.input_is_command(), "{text} must stay plain text");
    }
}

/// A command blocked by state answers; a chord in the same state is silent. That
/// asymmetry is the point (docs/lessons.md §4) — and each note has to name a
/// route out, so the answer is not a dead end.
#[test]
fn a_blocked_command_says_so_and_names_a_route() {
    // Nothing is generating: `/stop` has nothing to cancel.
    let mut c = Cmd::new();
    assert_eq!(c.run("/stop"), None);
    assert!(!c.last_note().is_empty(), "/stop said nothing");

    // A turn is running: the destructive pair and impersonation are held back —
    // by the command, with an answer, where the chord just no-ops.
    for line in ["/regen", "/retry", "/takeback", "/impersonate"] {
        let mut c = Cmd::new();
        c.s.begin_generation(gen_id());
        assert_eq!(c.run(line), None, "{line} must not fire mid-turn");
        let note = c.last_note();
        assert!(note.contains(line), "{line}: the note must name it: {note}");
        assert!(
            note.contains("/stop"),
            "{line}: the note must name the way out: {note}"
        );
    }

    // No chat open yet: the chat-scoped commands say so instead of doing nothing.
    for line in ["/copy", "/clone", "/rename Новое"] {
        let mut c = Cmd::bare();
        assert_eq!(c.run(line), None, "{line} without a chat");
        assert!(!c.last_note().is_empty(), "{line} said nothing");
    }

    // The settings snapshot has not arrived yet (the first moments after launch).
    let mut c = Cmd::bare();
    assert_eq!(c.run("/settings"), None);
    assert!(!c.last_note().is_empty(), "/settings said nothing");
}

/// `/stop` is the explicit half of `Esc`: it cancels a running turn, while
/// `/chats` is the other half and opens the list whatever the turn is doing.
#[test]
fn stop_cancels_a_turn_and_chats_never_does() {
    let mut c = Cmd::new();
    c.s.begin_generation(gen_id());
    assert_eq!(c.run("/stop"), Some(ChatIntent::Cancel));

    let mut c = Cmd::new();
    c.s.begin_generation(gen_id());
    assert_eq!(c.run("/chats"), Some(ChatIntent::OpenChatList));
    // The control: `Esc` in that same state cancels instead — which is the
    // overload the two commands exist to resolve.
    let mut c = Cmd::new();
    c.s.begin_generation(gen_id());
    assert_eq!(c.key(KeyCode::Esc), Some(ChatIntent::Cancel));
}

/// The confirmation popup belongs to the operation, not to the key: with
/// `confirm_destructive_keys` on, a typed `/regen` opens it too, and the intent
/// is only issued on `Enter`.
#[test]
fn a_typed_takeback_honours_the_confirmation_setting() {
    for (line, expected) in [
        ("/regen", ChatIntent::RegenerateLast),
        ("/takeback", ChatIntent::DeleteLastExchange),
    ] {
        let mut c = Cmd::new();
        let mut cfg = AppConfig::default();
        cfg.interface.confirm_destructive_keys = true;
        c.s.set_settings(cfg, Vec::new(), Vec::new(), Default::default(), Vec::new());
        assert_eq!(c.run(line), None, "{line} must ask first");
        assert!(c.s.confirm.is_some(), "{line} did not open the popup");
        assert_eq!(
            c.key(KeyCode::Enter),
            Some(expected),
            "{line} was not confirmed"
        );
    }
}

/// `/search <text>` goes straight to the message-level results screen — the
/// two-step journey (`Esc`, `Ctrl+F`, `Ctrl+G`) collapsed into one line. Bare, it
/// teaches its own syntax rather than opening an empty screen (fork F4).
#[test]
fn search_asks_the_orchestrator_and_bare_search_teaches_the_syntax() {
    let mut c = Cmd::new();
    assert_eq!(
        c.run("/search про космос"),
        Some(ChatIntent::SearchMessages {
            query: "про космос".into()
        })
    );

    let mut c = Cmd::new();
    assert_eq!(c.run("/search"), None);
    assert!(
        c.last_note().contains("/search"),
        "the usage line: {}",
        c.last_note()
    );
}

/// `/find <text>` seeds the in-feed query, so one line opens the search *and*
/// lands on a match.
#[test]
fn find_with_text_seeds_the_query() {
    let mut c = Cmd::new();
    assert_eq!(c.run("/find привет"), None);
    assert!(c.s.search.is_some(), "the search field is not open");
    assert_eq!(c.s.search_last, "привет");
}

/// `/rename <title>` renames the open chat. Bare, it hands the current title
/// back as an editable command line — the box is where a command is typed, so
/// the title is edited there rather than in a popup of its own.
#[test]
fn rename_takes_a_title_or_offers_the_current_one() {
    let mut c = Cmd::new();
    let chat = c.chat;
    assert_eq!(
        c.run("/rename Космос и мы"),
        Some(ChatIntent::RenameChat {
            id: chat,
            title: "Космос и мы".into()
        })
    );

    let mut c = Cmd::new();
    assert_eq!(c.run("/rename"), None);
    assert_eq!(c.s.input.text(), "/rename Про космос");
    // Pressing Enter on what it offered renames to the same title — a no-op
    // rename, not a crash and not a message going out to the model.
    let chat = c.chat;
    assert_eq!(
        c.s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(ChatIntent::RenameChat {
            id: chat,
            title: "Про космос".into()
        })
    );
}

/// A title longer than the chat list's own ceiling is trimmed to it, not stored
/// whole — the two routes to a title must agree on its bounds.
#[test]
fn a_renamed_title_obeys_the_length_ceiling() {
    let mut c = Cmd::new();
    let long = "я".repeat(crate::features::rename_chat::MAX_TITLE_LEN + 40);
    let Some(ChatIntent::RenameChat { title, .. }) = c.run(&format!("/rename {long}")) else {
        panic!("expected a rename");
    };
    assert_eq!(
        title.chars().count(),
        crate::features::rename_chat::MAX_TITLE_LEN
    );
}

/// `/new <profile>`: an exact name, then an unambiguous prefix (fork F3). A miss
/// and an ambiguity both name the candidates and the bare-`/new` route.
#[test]
fn new_chat_resolves_a_profile_name_or_says_why_not() {
    let mut c = Cmd::new();
    let expected = c.s.profiles[0].id;
    assert_eq!(
        c.run("/new гайя"), // exact, case-insensitively
        Some(ChatIntent::NewChat {
            profile_id: Some(expected)
        })
    );

    let mut c = Cmd::new();
    let helios = c.s.profiles[1].id;
    assert_eq!(
        c.run("/new гел"), // an unambiguous prefix
        Some(ChatIntent::NewChat {
            profile_id: Some(helios)
        })
    );

    // Ambiguous: both profile names share their first letter, so a one-letter
    // prefix matches them both — the note lists the candidates.
    let mut c = Cmd::new();
    assert_eq!(c.run("/new г"), None);
    let note = c.last_note();
    assert!(note.contains("Гайя") && note.contains("Гелиос"), "{note}");
    assert!(note.contains("/new"), "the route back: {note}");

    // A miss: the note lists what there is.
    let mut c = Cmd::new();
    assert_eq!(c.run("/new Прометей"), None);
    let note = c.last_note();
    assert!(note.contains("Гайя"), "{note}");

    // Bare `/new` with several profiles opens the picker, exactly as `Ctrl+N`.
    let mut c = Cmd::new();
    assert_eq!(c.run("/new"), None);
    assert!(c.s.profile_overlay.is_some(), "the picker did not open");
}

/// The registry does not swallow ordinary text: a message that merely looks like
/// a command still goes out to the model.
#[test]
fn text_that_only_resembles_a_command_is_sent() {
    for text in ["/settings2", "how do I /stop it?", "стоп"] {
        let mut c = Cmd::new();
        assert_eq!(
            c.run(text),
            Some(ChatIntent::Send(text.into())),
            "{text} must go out as a message"
        );
    }
}

/// Every registry command is answered by the screen — no row can be added
/// without deciding what it does. A command that neither returns an intent nor
/// changes anything visible would be exactly the silence this track exists to
/// remove.
#[test]
fn no_command_is_a_silent_no_op() {
    for spec in ui_command::COMMANDS {
        let mut c = Cmd::new();
        let line = match spec.arity {
            Arity::Required => format!("{} космос", spec.aliases[0]),
            _ => spec.aliases[0].to_string(),
        };
        let intent = c.run(&line);
        let visible = c.s.help.is_some()
            || c.s.emoji.is_some()
            || c.s.search.is_some()
            || c.s.profile_overlay.is_some()
            || c.s.chat_links.is_some()
            || !c.last_note().is_empty()
            || !c.s.input.is_empty();
        assert!(
            intent.is_some() || visible,
            "{:?} ({line}) did nothing at all",
            spec.command
        );
    }
}

/// A modal owns the keyboard while it is open, so a command typed "through" one
/// cannot fire. This pins the routing order rather than the commands themselves.
#[test]
fn commands_do_not_fire_through_a_modal() {
    let mut c = Cmd::new();
    c.s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(c.s.help.is_some());
    assert_eq!(c.run("/settings"), None, "a modal must keep the keyboard");
    assert!(c.s.help.is_some(), "the dialog closed unexpectedly");
}

/// The registry's `UiCommand` variants are all reachable from the table — a
/// variant nobody can type would be dead weight, and one claimed twice would
/// shadow the second row.
#[test]
fn every_ui_command_variant_has_exactly_one_row() {
    let mut seen: Vec<UiCommand> = Vec::new();
    for spec in ui_command::COMMANDS {
        assert!(
            !seen.contains(&spec.command),
            "{:?} has two rows",
            spec.command
        );
        seen.push(spec.command);
    }
    assert_eq!(seen.len(), ui_command::COMMANDS.len());
}

// ---------- stage 2: the screens' own leftovers (/profile, /self clear) ----------

/// `/profile list` answers from the snapshot the screen already holds, and marks
/// the open chat's own profile — the question behind it is usually "which one am
/// I talking to?".
#[test]
fn profile_list_names_them_and_marks_the_active_one() {
    let mut c = Cmd::new();
    // The open chat belongs to the first profile.
    c.s.set_chat_list(vec![ChatSummary {
        id: c.chat,
        profile_id: Uuid::from_u128(2),
        title: "Про космос".into(),
        created_at: chrono::Utc::now(),
        modified_at: chrono::Utc::now(),
        message_count: 1,
    }]);
    assert_eq!(c.run("/profile list"), None);
    let note = c.last_note();
    assert!(note.contains("Гайя") && note.contains("Гелиос"), "{note}");
    let marker = c.s.palette.glyphs().title_marker;
    assert!(
        note.contains(&format!("{marker} Гайя")),
        "the open chat's profile is not marked: {note}"
    );
    assert!(
        !note.contains(&format!("{marker} Гелиос")),
        "only one profile is the active one: {note}"
    );
}

/// `/profile new` creates through the same command the settings screen's
/// `Ctrl+N` sends, with the same default name; a given name is used verbatim.
/// The persona stays empty either way — that is the settings screen's job, and
/// the orchestrator's notice is what says so.
#[test]
fn profile_new_matches_the_settings_screen_default() {
    let mut c = Cmd::new();
    let default_name = c.s.loc.t("ui.settings.new_profile_name").to_string();
    assert_eq!(
        c.run("/profile new"),
        Some(ChatIntent::CreateProfile { name: default_name })
    );

    let mut c = Cmd::new();
    assert_eq!(
        c.run("/profile new Дневной помощник"),
        Some(ChatIntent::CreateProfile {
            name: "Дневной помощник".into()
        })
    );
}

/// Deleting a profile **always** asks, unlike the key it mirrors: a typed name
/// can resolve to a profile the user did not picture, where the settings screen
/// shows the row it is about to delete. The question names the profile and how
/// many conversations go with it.
#[test]
fn profile_delete_always_confirms_and_names_what_goes() {
    let mut c = Cmd::new();
    c.s.set_chat_list(vec![
        ChatSummary {
            id: c.chat,
            profile_id: Uuid::from_u128(2),
            title: "Про космос".into(),
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            message_count: 1,
        },
        ChatSummary {
            id: Uuid::from_u128(9),
            profile_id: Uuid::from_u128(2),
            title: "Второй".into(),
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            message_count: 1,
        },
    ]);
    // The setting that gates the two chat-level keys is OFF here — this popup
    // is not governed by it.
    assert!(!c.s.confirm_destructive);
    assert_eq!(c.run("/profile delete Гайя"), None, "it must ask first");
    let text = confirm_popup_text(&mut c.s);
    assert!(text.contains("Гайя"), "the profile is named: {text}");
    assert!(text.contains('2'), "the chat count is stated: {text}");
    // Answering yes sends the same command `Ctrl+D` sends.
    assert_eq!(
        c.key(KeyCode::Enter),
        Some(ChatIntent::DeleteProfile(Uuid::from_u128(2)))
    );

    // …and `Esc` answers no: nothing is sent and the popup closes.
    let mut c = Cmd::new();
    c.run("/profile delete Гайя");
    assert_eq!(c.key(KeyCode::Esc), None);
    assert!(c.s.confirm.is_none(), "the popup stayed open");
}

/// The last profile cannot go — there would be nothing to create chats from. The
/// orchestrator refuses too, but asking a question whose "yes" is then declined
/// is a worse way to say it.
#[test]
fn deleting_the_only_profile_is_refused_before_it_asks() {
    let mut c = Cmd::new();
    c.s.set_profile_list(vec![ProfileSummary {
        id: Uuid::from_u128(2),
        name: "Гайя".into(),
    }]);
    assert_eq!(c.run("/profile delete Гайя"), None);
    assert!(c.s.confirm.is_none(), "it must not ask");
    assert!(!c.last_note().is_empty(), "it said nothing");
    assert!(
        c.last_note().contains("/profile new"),
        "the note must name the way out: {}",
        c.last_note()
    );
}

/// A name that matches nothing or several profiles is answered by the **shared**
/// resolver, so `/profile delete` and `/new` agree on the prefix rule — but each
/// names the route that fits the command that was typed.
#[test]
fn profile_delete_uses_the_shared_name_resolver() {
    let mut c = Cmd::new();
    assert_eq!(c.run("/profile delete гел"), None, "an unambiguous prefix");
    assert!(
        matches!(&c.s.confirm, Some(ConfirmAction::DeleteProfile { name, .. }) if name == "Гелиос"),
        "the prefix must resolve like /new does"
    );

    let mut c = Cmd::new();
    assert_eq!(c.run("/profile delete г"), None);
    assert!(c.s.confirm.is_none(), "an ambiguous name must not ask");
    let note = c.last_note();
    assert!(note.contains("Гайя") && note.contains("Гелиос"), "{note}");
    assert!(
        note.contains("/profile list"),
        "the route fits the typed command: {note}"
    );

    // The other caller keeps its own route.
    let mut c = Cmd::new();
    c.run("/new г");
    assert!(
        c.last_note().contains("/new"),
        "/new keeps the picker route: {}",
        c.last_note()
    );
}

/// `/self clear` mirrors `Ctrl+K` twice in the self-model screen — a
/// confirmation there, a confirmation here — and reaches the same edit.
#[test]
fn self_clear_confirms_and_sends_the_same_edit() {
    let mut c = Cmd::new();
    assert_eq!(c.run("/self clear"), None, "it must ask first");
    assert!(c.s.confirm.is_some());
    assert_eq!(c.key(KeyCode::Enter), Some(ChatIntent::ClearSelfModel));

    // Bare `/self` still opens the screen — the subcommand did not shadow it.
    let mut c = Cmd::new();
    assert_eq!(c.run("/self"), Some(ChatIntent::OpenSelfModel));

    // An unknown word is reported by the parser, with the usage line.
    let mut c = Cmd::new();
    assert_eq!(c.run("/self wipe"), None);
    assert!(
        c.last_note().contains("/self"),
        "the usage line: {}",
        c.last_note()
    );

    // The model belongs to the active profile: with no chat open the
    // orchestrator would drop the edit, so the command says so instead.
    let mut c = Cmd::bare();
    assert_eq!(c.run("/self clear"), None);
    assert!(c.s.confirm.is_none(), "nothing to clear, nothing to ask");
    assert!(!c.last_note().is_empty(), "/self clear said nothing");
}

/// The stage-2 commands are highlighted while typed and never leak to the model,
/// exactly like the registry's.
#[test]
fn profile_commands_are_highlighted_and_never_sent() {
    for text in [
        "/profile",
        "/profile list",
        "/profile new Гайя",
        "/profile delete Гайя",
        "/profile renam x",
    ] {
        let mut c = Cmd::new();
        type_str(&mut c.s, text);
        assert!(c.s.input_is_command(), "{text} is not highlighted");
        let mut c = Cmd::new();
        assert!(
            !matches!(c.run(text), Some(ChatIntent::Send(_))),
            "{text} must not go out as a message"
        );
    }
    // Near-words stay prose.
    for text in ["/profiles", "/profile-new"] {
        let mut c = Cmd::new();
        type_str(&mut c.s, text);
        assert!(!c.s.input_is_command(), "{text} must stay plain text");
    }
}

/// The confirmation popup, rendered to text — the question has to be readable,
/// not merely present (the popup wraps at 56 columns).
fn confirm_popup_text(s: &mut ChatScreen) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
    let buf = term.backend().buffer();
    let mut out = String::new();
    for y in buf.area.top()..buf.area.bottom() {
        for x in buf.area.left()..buf.area.right() {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}
