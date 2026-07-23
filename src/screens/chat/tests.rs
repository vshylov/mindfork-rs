//! Тесты экрана чата (через handle_key/render). См. mod.rs.

use super::popups::HELP_KEYS;
use super::*;
use crate::entities::message::MessageRole;

fn gen_id() -> Uuid {
    Uuid::new_v4()
}

/// Снимок статусов с готовым чат-сервером (эмбеддинги/имперсонация не настроены).
fn ready_statuses() -> ServerStatuses {
    ServerStatuses {
        chat: ServerStatus::Ready,
        embed: ServerStatus::NotConfigured,
        impersonation: ServerStatus::NotConfigured,
    }
}

#[test]
fn chunk_after_midgen_note_goes_to_new_assistant_bubble() {
    // Регрессия: пометка (AppEvent::Error о лимите раундов) посреди генерации
    // делала `last` заметкой, и последующий стрим финального ответа дописывался в
    // неё (простой текст, без markdown). Теперь чанк открывает новый пузырь.
    let mut s = ChatScreen::new();
    let id = gen_id();
    s.push_user_message("собери отзывы".into());
    s.begin_generation(id);
    // Ассистент вызвал инструмент (пузырь ассистента пуст по тексту)...
    s.push_tool_call(id, "web_search".into(), "{}".into(), "результаты".into());
    // ...достигнут лимит — в ленту уходит пометка.
    s.push_error("Достигнут лимит раундов инструментов (8) — свожу итог.");
    // Форс-синтез стримит финальный ответ.
    s.push_chunk(id, "## Итог\n\n**Вывод**.");
    s.finish_generation(id, FinishReason::Stop);

    // Заметка отдельным элементом; финальный текст — в пузыре ассистента (markdown),
    // а не в заметке.
    let notes: Vec<_> = s.feed.iter().filter(|m| m.role == FeedRole::Note).collect();
    assert_eq!(notes.len(), 1, "ровно одна заметка о лимите");
    assert!(
        !notes[0].text.contains("## Итог"),
        "финальный текст не должен попасть в заметку: {:?}",
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

    // Live: текст раунда 1 → вызов инструмента → текст раунда 2 (финал).
    let mut s = ChatScreen::new();
    let id = gen_id();
    s.begin_generation(id);
    s.push_chunk(id, "Ищу погоду.");
    s.push_tool_call(
        id,
        "web_search".into(),
        "{\"q\":\"погода\"}".into(),
        "ясно".into(),
    );
    s.push_chunk(id, "Сейчас ясно.");
    s.finish_generation(id, FinishReason::Stop);

    let live = s.feed.last().unwrap().clone();
    assert_eq!(live.text, "Ищу погоду.\n\nСейчас ясно.");
    assert_eq!(live.tools.len(), 1);
    assert_eq!(live.tools[0].text_offset, "Ищу погоду.".len());

    // Reload: те же раунды как доменные сообщения (assistant+tool / assistant).
    let mut r1 = Message::assistant("Ищу погоду.");
    r1.tool_calls = vec![ToolCallRecord {
        thought_signature: None,
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

    // Один слитый блок ассистента, тот же текст и то же смещение вызова.
    assert_eq!(reload.len(), 1);
    assert_eq!(reload[0].text, live.text);
    assert_eq!(reload[0].tools.len(), 1);
    assert_eq!(reload[0].tools[0].text_offset, live.tools[0].text_offset);
}

#[test]
fn live_followup_makes_two_bubbles_matching_reload() {
    use crate::entities::message::{Message, ToolCallRecord};

    // Live: текст 1 → followup → текст 2.
    let mut s = ChatScreen::new();
    let id = gen_id();
    s.begin_generation(id);
    s.push_chunk(id, "Первое сообщение.");
    s.continue_assistant(id);
    s.push_chunk(id, "Второе сообщение.");
    s.finish_generation(id, FinishReason::Stop);

    // Два отдельных пузыря ассистента.
    let bubbles: Vec<&FeedMessage> = s
        .feed
        .iter()
        .filter(|m| m.role == FeedRole::Assistant)
        .collect();
    assert_eq!(bubbles.len(), 2);
    assert_eq!(bubbles[0].text, "Первое сообщение.");
    assert_eq!(bubbles[1].text, "Второе сообщение.");

    // Reload: A1 (с управляющим вызовом) → tool → A2 (new_bubble).
    let mut a1 = Message::assistant("Первое сообщение.");
    a1.tool_calls = vec![ToolCallRecord {
        thought_signature: None,
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
    // Live: частичный неверный текст → rewrite → переписанный ответ.
    let mut s = ChatScreen::new();
    let id = gen_id();
    s.begin_generation(id);
    s.push_chunk(id, "Непра");
    s.push_chunk(id, "вильный ответ.");
    s.rewrite_assistant(id);
    s.push_chunk(id, "Правильный ответ.");
    s.finish_generation(id, FinishReason::Stop);

    // Ровно один пузырь ассистента с переписанным текстом (частичный отброшен).
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
    s.begin_generation(prev); // как будто шла генерация
    s.set_token_usage(prev, 42, Some(123), true, Some(7)); // счётчик токенов прежнего чата
    let id = gen_id();
    let messages = vec![
        Message::new(MessageRole::System, "sys"),
        Message::user("привет"),
        Message::assistant("здравствуйте"),
    ];
    s.activate_chat(id, "Чат".into(), &messages, "");
    assert_eq!(s.active_chat, Some(id));
    assert!(!s.generating);
    assert!(s.current_gen.is_none());
    // счётчик токенов прежнего чата сброшен (иначе висел бы в статус-баре)
    assert_eq!(s.gen_tokens, 0);
    assert!(s.gen_context.is_none());
    assert!(!s.gen_context_exact);
    // системное сообщение не попадает в ленту
    assert_eq!(s.feed.len(), 2);
}

#[test]
fn feed_scroll_requests_clear_only_with_vs16_emoji() {
    // Прокрутка ленты с чистым текстом не требует полной перерисовки (не мигает),
    // а с VS16-эмодзи (`🗂️`) — требует (стирает «висячий» артефакт на conhost).
    let id = gen_id();

    let mut clean = ChatScreen::new();
    clean.activate_chat(id, "Чат".into(), &[Message::assistant("обычный текст")], "");
    clean.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
    assert!(
        !clean.take_feed_scrolled(),
        "на чистом тексте полная перерисовка не нужна"
    );
    // флаг забран однократно
    assert!(!clean.take_feed_scrolled());

    let mut emoji = ChatScreen::new();
    emoji.activate_chat(id, "Чат".into(), &[Message::assistant("## 🗂️ Хэш")], "");
    emoji.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    assert!(
        emoji.take_feed_scrolled(),
        "с VS16-эмодзи нужна полная перерисовка"
    );
    // без новой прокрутки повторно не запрашиваем
    assert!(!emoji.take_feed_scrolled());

    // Петля забирает флаг обобщённым `take_full_redraw` — источник ленты входит в него
    // (иначе фикс VS16-артефакта тихо отвалился бы при добавлении второго источника).
    emoji.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
    assert!(
        emoji.take_full_redraw(),
        "прокрутка ленты с VS16 входит в take_full_redraw"
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
    // поле очищено
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
    // Shift+Enter — перенос строки, не отправка.
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
        None
    );
    for c in "cd".chars() {
        s.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    // Alt+Enter — тот же перенос (запасной вариант для терминалов без kitty-протокола).
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)),
        None
    );
    for c in "ef".chars() {
        s.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    assert_eq!(s.input.text(), "ab\ncd\nef");
    // Голый Enter по-прежнему отправляет весь многострочный ввод.
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(ChatIntent::Send("ab\ncd\nef".into()))
    );
}

#[test]
fn activate_chat_loads_draft_without_marking_dirty() {
    let mut s = ChatScreen::new();
    // Активация чата с сохранённым черновиком загружает его в поле ввода…
    s.activate_chat(gen_id(), "Чат".into(), &[], "недописанный текст");
    assert_eq!(s.input.text(), "недописанный текст");
    // …но не помечает черновик «грязным» (иначе тут же отправили бы его обратно).
    assert_eq!(s.take_dirty_draft(), None);
    // Переключение на чат без черновика очищает поле ввода.
    s.activate_chat(gen_id(), "Новый".into(), &[], "");
    assert!(s.input.is_empty());
    assert_eq!(s.take_dirty_draft(), None);
}

#[test]
fn typing_marks_draft_dirty_and_take_returns_text_once() {
    let mut s = ChatScreen::new();
    type_str(&mut s, "черновик");
    // Первый забор отдаёт набранный текст…
    assert_eq!(s.take_dirty_draft(), Some("черновик".into()));
    // …повторный — None, пока ввод снова не изменится.
    assert_eq!(s.take_dirty_draft(), None);
}

#[test]
fn sending_clears_draft_to_empty() {
    let mut s = ChatScreen::new();
    s.set_server_status(ready_statuses());
    type_str(&mut s, "вопрос");
    let _ = s.take_dirty_draft(); // забрали черновик при наборе
    let intent = s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(intent, Some(ChatIntent::Send("вопрос".into())));
    // Отправка очистила поле — черновик стал пустым (UI отправит SetDraft("")).
    assert_eq!(s.take_dirty_draft(), Some(String::new()));
}

#[test]
fn restore_input_sets_when_empty_and_prepends_when_not() {
    let mut s = ChatScreen::new();
    // Пустое поле — просто заполняется.
    s.restore_input("вопрос".into());
    assert_eq!(s.input.text(), "вопрос");
    // Непустое — текст добавляется в начало, существующий ввод сохраняется.
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
    // Во время генерации — подавляется.
    s.begin_generation(gen_id());
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)),
        None
    );
}

#[test]
fn impersonation_stream_then_stop_commits_text_to_input() {
    let mut s = ChatScreen::new();
    type_str(&mut s, "Я "); // затравка
    let id = gen_id();
    s.begin_impersonation(id);
    assert!(s.is_impersonating());
    s.push_impersonation_chunk(id, "хочу узнать про Rust");
    s.finish_impersonation(id, FinishReason::Stop);
    assert!(!s.is_impersonating());
    // Текст реплики (затравка + сгенерированное) — в поле ввода.
    assert_eq!(s.input.text(), "Я хочу узнать про Rust");
}

#[test]
fn impersonation_cancel_keeps_seed_and_discards_generated() {
    let mut s = ChatScreen::new();
    type_str(&mut s, "черновик");
    let id = gen_id();
    s.begin_impersonation(id);
    s.push_impersonation_chunk(id, " дополнение");
    // Esc во время имперсонации — намерение отмены.
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Some(ChatIntent::CancelImpersonation)
    );
    // Отмена (Cancelled) отбрасывает сгенерированное — поле сохраняет затравку.
    s.finish_impersonation(id, FinishReason::Cancelled);
    assert!(!s.is_impersonating());
    assert_eq!(s.input.text(), "черновик");
}

#[test]
fn impersonation_timeout_keeps_partial_text() {
    // Таймаут имперсонации приходит как `Length` (не `Cancelled`) — обрезанная
    // реплика должна сохраниться в поле, а не исчезнуть.
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
    // Обычная клавиша не печатается в поле (поле скрыто).
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
        None
    );
    // Ctrl+Q / F10 всё ещё выходят (выход переехал с Ctrl+C).
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

/// Включает подтверждение `Ctrl+R`/`Ctrl+E` через снимок настроек.
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
    // Первое нажатие не отдаёт намерение — открывает попап подтверждения.
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
        None
    );
    assert_eq!(s.confirm, Some(ConfirmAction::Regenerate));
    // Enter подтверждает и закрывает попап.
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
    // Esc отменяет — попап закрыт, намерения нет.
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
    // Произвольная клавиша не закрывает попап и не печатается в поле ввода.
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
    // По умолчанию (без снимка настроек) подтверждение выключено.
    let mut s = ChatScreen::new();
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
        Some(ChatIntent::RegenerateLast)
    );
    assert_eq!(s.confirm, None);
}

#[test]
fn esc_opens_chat_list_else_cancels_generation() {
    let mut s = ChatScreen::new();
    // Без генерации Esc просит открыть экран списка чатов (его создаёт `app`).
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Some(ChatIntent::OpenChatList)
    );
    // Во время генерации Esc сперва отменяет её.
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
    // Ctrl+C без выделения — no-op (освобождён под копирование, не выход).
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
        .on_key(KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL)); // курсор в начало
    for _ in 0..5 {
        s.input
            .on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)); // выделить "hello"
    }
    // Ctrl+C → намерение записать выделение в буфер; выделение снято, текст цел.
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Some(ChatIntent::CopyToClipboard("hello".into()))
    );
    assert!(!s.input.has_selection());
    assert_eq!(s.input.text(), "hello world");
    // Выделяем следующие 5 символов (" worl") и вырезаем — текст укорачивается.
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
    // Открывается на вкладке «Горячие клавиши».
    assert_eq!(s.help.as_ref().unwrap().tab, HelpTab::Hotkeys);
    // Прочая клавиша не закрывает диалог (в нём вкладки/навигация) и не печатается.
    let intent = s.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert_eq!(intent, None);
    assert!(s.help.is_some(), "прочая клавиша не закрывает диалог");
    assert!(s.input.is_empty(), "ввод не печатался при открытом диалоге");
    // Esc закрывает.
    s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(s.help.is_none());
}

#[test]
fn question_mark_opens_help_only_when_input_empty() {
    let mut s = ChatScreen::new();
    // Пустой ввод → `?` открывает диалог.
    s.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    assert!(s.help.is_some());
    s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)); // закрыть
    // Непустой ввод → `?` печатается, диалог не открывается.
    type_str(&mut s, "abc");
    s.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    assert!(s.help.is_none());
    assert_eq!(s.input.text(), "abc?");
}

#[test]
fn help_navigation_scrolls_and_switches_tabs() {
    let mut s = ChatScreen::new();
    s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    // ↑↓/PgUp/PgDn прокручивают активную вкладку, не закрывая диалог.
    s.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    s.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    assert!(s.help.is_some(), "прокрутка не должна закрывать диалог");
    assert_eq!(s.help.as_ref().unwrap().scroll, 1 + PAGE_SCROLL);
    s.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(s.help.as_ref().unwrap().scroll, PAGE_SCROLL);
    s.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
    assert_eq!(s.help.as_ref().unwrap().scroll, 0);
    // Прокрутим и переключим вкладку → прокрутка сбрасывается (Tab — следующая,
    // порядок About/Hotkeys/Commands/License/Components: следующая за Hotkeys — Commands).
    s.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    s.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let h = s.help.as_ref().unwrap();
    assert_eq!(h.tab, HelpTab::Commands);
    assert_eq!(h.scroll, 0, "смена вкладки сбрасывает прокрутку");
    // ← возвращает на предыдущую вкладку.
    s.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(s.help.as_ref().unwrap().tab, HelpTab::Hotkeys);
    // Esc закрывает; повторное открытие — на той же вкладке (запоминается), с нулём
    // прокрутки. Здесь вернулись на Hotkeys, значит и откроется на Hotkeys.
    s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(s.help.is_none());
    s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    let h = s.help.as_ref().unwrap();
    assert_eq!((h.tab, h.scroll), (HelpTab::Hotkeys, 0));
}

/// Диалог справки запоминает последнюю выбранную вкладку и открывается на ней.
#[test]
fn help_remembers_last_tab() {
    let mut s = ChatScreen::new();
    // Открываем (Hotkeys по умолчанию), переключаемся на «Компоненты», закрываем.
    s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    // About ← Hotkeys ← ... : два `←` от Hotkeys → Components (по кругу: Hotkeys→About→Components).
    s.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    s.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(s.help.as_ref().unwrap().tab, HelpTab::Components);
    s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(s.help.is_none());
    // Повторное открытие — снова на «Компонентах».
    s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert_eq!(s.help.as_ref().unwrap().tab, HelpTab::Components);
}

#[test]
fn help_scroll_clamps_and_draws_scrollbar_on_short_terminal() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut s = ChatScreen::new();
    s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    s.help.as_mut().unwrap().scroll = 10_000; // «перекручено» — рендер клампит
    let mut term = Terminal::new(TestBackend::new(90, 12)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
    // Список «Горячих клавиш» не помещается в невысокий диалог → прокрутка клампится
    // к максимуму (заведомо меньше запрошенного) и рисуется бегунок скроллбара.
    assert!(
        s.help.as_ref().unwrap().scroll < HELP_KEYS.len(),
        "прокрутка клампится к максимуму"
    );
    let buf = term.backend().buffer();
    let mut thumb = false;
    for y in buf.area.top()..buf.area.bottom() {
        for x in buf.area.left()..buf.area.right() {
            thumb |= buf[(x, y)].symbol() == "█";
        }
    }
    assert!(
        thumb,
        "на коротком терминале у справки есть бегунок скроллбара"
    );
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
    // Без снимка настроек — Ctrl+P ничего не делает.
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
    // При русской раскладке физические клавиши дают кириллицу: Ctrl+з (физ. P),
    // Ctrl+й (физ. Q) — шорткаты обязаны срабатывать.
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
        "Ctrl+з (физ. P) открывает настройки"
    );
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('й'), KeyModifiers::CONTROL)),
        Some(ChatIntent::Quit),
        "Ctrl+й (физ. Q) — выход"
    );
}

#[test]
fn rename_chat_updates_title_bar_of_active_chat() {
    let mut s = ChatScreen::new();
    let id = gen_id();
    s.activate_chat(id, "Старое".into(), &[], "");
    s.rename_chat(id, "Новое".into());
    assert_eq!(s.title, "Новое");
    // Чужой чат не трогает шапку активного.
    s.rename_chat(gen_id(), "Постороннее".into());
    assert_eq!(s.title, "Новое");
}

#[test]
fn f5_copies_active_chat_in_main_window() {
    let mut s = ChatScreen::new();
    // Без активного чата F5 — no-op.
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
        None
    );
    let id = gen_id();
    s.activate_chat(id, "Чат".into(), &[], "");
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
        Some(ChatIntent::CopyChat(id))
    );
}

#[test]
fn late_list_op_results_fall_to_feed() {
    // Когда экран списка закрыт, поздние результаты операций списка (копия/
    // авто-название) `app` кладёт заметкой в ленту через push_note/push_error.
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
    // Рендерим, чтобы поле ввода запомнило свою область (last_area).
    let mut term = Terminal::new(TestBackend::new(50, 16)).unwrap();
    term.draw(|f| s.render(f)).unwrap();
    let area = s.input.last_area_for_test().expect("поле отрисовано");
    // Клик в начало поля — курсор туда, выделения ещё нет.
    s.handle_mouse(mouse_at(
        MouseEventKind::Down(MouseButton::Left),
        area.x,
        area.y,
    ));
    assert_eq!(s.input.cursor(), (0, 0));
    assert!(!s.input.has_selection());
    // Драг вправо на 5 колонок растит выделение "hello".
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
    // Клик в ленту (верх экрана, вне поля ввода) — курсор поля не двигается.
    s.handle_mouse(mouse_at(MouseEventKind::Down(MouseButton::Left), 0, 0));
    assert_eq!(s.input.cursor(), (0, 5)); // курсор остался в конце текста
    assert!(!s.input.has_selection());
}

#[test]
fn mouse_wheel_up_scrolls_feed_and_disables_follow() {
    let mut s = ChatScreen::new();
    assert!(s.feed_view.is_following());
    s.handle_mouse(wheel(MouseEventKind::ScrollUp));
    assert!(
        !s.feed_view.is_following(),
        "прокрутка вверх отключает следование за хвостом"
    );
}

#[test]
fn ctrl_w_toggles_mouse_capture_intent() {
    let mut s = ChatScreen::new();
    // По умолчанию захват выключен → первое нажатие включает (true).
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)),
        Some(ChatIntent::SetMouseCapture(true))
    );
    // Второе — выключает (false).
    assert_eq!(
        s.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)),
        Some(ChatIntent::SetMouseCapture(false))
    );
    // Работает и при русской раскладке: Ctrl+ц (физ. W).
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
        "при открытой справке колесо не трогает ленту"
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
    // Правка ЛЕВЕЕ VS16-эмодзи в поле ввода двигает его на место чужого символа:
    // diff шлёт хвостовую половину, бэкенд печатает её без `MoveTo` и ряд едет
    // вправо (ratatui#2651, механика — в `shared::ui`). Поэтому поле с таким глифом
    // рисуется полной перерисовкой; на обычном тексте — нет.
    let mut s = ChatScreen::new();
    type_str(&mut s, "hello");
    assert!(
        !s.take_full_redraw(),
        "обычный текст полной перерисовки не требует"
    );

    type_str(&mut s, "\u{2764}\u{FE0F}");
    assert!(s.take_full_redraw(), "поле с ❤️ требует полной перерисовки");

    // И последующие правки тоже — глиф всё ещё в поле.
    type_str(&mut s, "x");
    assert!(s.take_full_redraw(), "правка при живом глифе — тоже");

    // Очистка поля снимает требование.
    s.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    s.take_full_redraw();
    type_str(&mut s, "plain");
    assert!(
        !s.take_full_redraw(),
        "после очистки обычный текст перерисовки не требует"
    );
}

#[test]
fn ctrl_g_opens_suggestions_for_misspelled_word() {
    let mut s = ChatScreen::new();
    s.set_spellchecker(mk_checker());
    type_str(&mut s, "helo"); // курсор в конце слова с ошибкой
    s.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
    assert!(s.suggest.is_some());
    // последний пункт — «добавить в словарь»
    let items = &s.suggest.as_ref().unwrap().items;
    assert_eq!(items.last(), Some(&SuggestItem::AddToDictionary));
}

#[test]
fn applying_suggestion_replaces_word() {
    let mut s = ChatScreen::new();
    s.set_spellchecker(mk_checker());
    type_str(&mut s, "helo");
    s.open_suggestions();
    // первый пункт — подсказка «hello»; Enter применяет
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
    // переходим на «добавить в словарь» и применяем
    for _ in 0..last {
        s.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(s.suggest.is_none());
    // слово теперь в персональном словаре — больше не ошибка
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
    // Выделение в попапе орфографии — как в списке чатов, настройках и «модели
    // себя»: мягкая подложка `keycap_bg` + зелёный рейл `▌`, а не инверсия строки.
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
        .expect("у выделенной строки попапа должен быть рейл `▌`");
    let cell = &buf[rail];
    assert_eq!(cell.style().fg, Some(palette.success), "рейл — зелёный");
    assert_eq!(
        cell.style().bg,
        Some(palette.keycap_bg),
        "выделение — подложка keycap_bg"
    );
    // Инверсии в выделенной строке быть не должно (реверс свапал бы fg↔bg поспаново).
    let (_, row) = rail;
    for x in 0..buf.area.width {
        assert!(
            !buf[(x, row)]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "выделенная строка не должна инвертироваться"
        );
    }
}

#[test]
fn ctrl_b_opens_emoji_picker_and_enter_inserts_at_cursor() {
    let mut s = ChatScreen::new();
    type_str(&mut s, "ab");
    // курсор между 'a' и 'b'
    s.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    // Ctrl+B открывает попап (в т.ч. при русской раскладке: Ctrl+и → физ. B).
    s.handle_key(KeyEvent::new(KeyCode::Char('и'), KeyModifiers::CONTROL));
    assert!(s.emoji.is_some());
    // Enter вставляет первый эмодзи на месте курсора и закрывает попап.
    let intent = s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(intent, None);
    assert!(s.emoji.is_none());
    assert_eq!(s.input.text(), "a😀b");
}

#[test]
fn emoji_picker_remembers_last_selection() {
    let mut s = ChatScreen::new();
    // Открываем, сдвигаем выделение и вставляем.
    s.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    s.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    s.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(s.emoji.is_none());
    // Повторное открытие восстанавливает прежнее выделение (индекс 2).
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
    assert!(s.input.is_empty(), "при отмене эмодзи не вставлен");
}

#[test]
fn risky_glyph_detector_covers_emoji_classes_but_not_plain_text() {
    use super::feed::is_risky_glyph;
    // Классы риска: VS16, ZWJ, тон кожи, supplementary-пиктограммы, BMP-эмодзи ширины 2.
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
        assert!(is_risky_glyph(c), "{c:?} должен считаться рискованным");
    }
    // Обычный текст, пунктуация, типографика и CJK — нет (иначе полная перерисовка
    // гонялась бы на каждый чанк китайского/японского текста без всякой пользы).
    for c in ['a', 'я', ' ', '·', '—', '→', '│', '█', '中', 'あ'] {
        assert!(!is_risky_glyph(c), "{c:?} рискованным быть не должен");
    }
}

#[test]
fn feed_content_change_requests_full_redraw_only_with_risky_glyphs() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    // Артефакты на legacy-терминалах появлялись при ИЗМЕНЕНИИ содержимого (стрим,
    // добавленная заметка), а «чинила» их только прокрутка — она была единственным
    // триггером полной перерисовки. Теперь триггером стало и изменение ленты.
    let draw = |s: &mut ChatScreen, term: &mut Terminal<TestBackend>| {
        term.draw(|f| s.render(f)).unwrap();
    };

    // Чистый текст: стрим не требует полной перерисовки.
    let mut plain = ChatScreen::new();
    let mut term = Terminal::new(TestBackend::new(60, 16)).unwrap();
    let id = gen_id();
    plain.begin_generation(id);
    plain.push_chunk(id, "обычный текст");
    draw(&mut plain, &mut term);
    assert!(
        !plain.take_full_redraw(),
        "на чистом тексте изменение ленты перерисовку не требует"
    );

    // Эмодзи в ленте: стрим требует.
    let mut emoji = ChatScreen::new();
    let id = gen_id();
    emoji.begin_generation(id);
    emoji.push_chunk(id, "смотри: 😀");
    assert!(
        emoji.take_full_redraw(),
        "изменение ленты с эмодзи заказывает перерисовку — ДО отрисовки, чтобы          артефакт не мелькнул даже на кадр"
    );
    assert!(!emoji.take_full_redraw(), "флаг забирается однократно");

    // Повторная отрисовка без изменений — запрос не возобновляется (иначе петля
    // перерисовывала бы экран целиком вечно).
    draw(&mut emoji, &mut term);
    assert!(
        !emoji.take_full_redraw(),
        "без изменения содержимого перерисовка не нужна"
    );

    // Заметка в ленту (F5 «Переписка скопирована…») — тоже изменение содержимого.
    emoji.push_note("Переписка скопирована в буфер обмена");
    draw(&mut emoji, &mut term);
    assert!(
        emoji.take_full_redraw(),
        "добавленная заметка заказывает перерисовку"
    );
}

#[test]
fn every_feed_mutator_marks_content_change() {
    // Гейт на подход «флаг ставят мутаторы»: забытый вызов `mark_feed_changed` вернул
    // бы мелькающий артефакт на legacy-терминалах. Лента с самого начала содержит
    // эмодзи, поэтому ЛЮБОЕ изменение обязано заказать полную перерисовку.
    let mut s = ChatScreen::new();
    let id = gen_id();

    s.activate_chat(id, "Чат".into(), &[Message::assistant("привет 😀")], "");
    assert!(s.take_full_redraw(), "activate_chat");

    s.push_user_message("вопрос".into());
    assert!(s.take_full_redraw(), "push_user_message");

    s.begin_generation(id);
    assert!(s.take_full_redraw(), "begin_generation");

    s.push_chunk(id, "ответ");
    assert!(s.take_full_redraw(), "push_chunk");

    s.push_thoughts(id, "мысль");
    assert!(s.take_full_redraw(), "push_thoughts");

    s.push_tool_call(id, "web_search".into(), "{}".into(), "ок".into());
    assert!(s.take_full_redraw(), "push_tool_call");

    s.continue_assistant(id);
    assert!(s.take_full_redraw(), "continue_assistant");

    s.rewrite_assistant(id);
    assert!(s.take_full_redraw(), "rewrite_assistant");

    s.push_note("заметка");
    assert!(s.take_full_redraw(), "push_note");

    s.push_error("ошибка");
    assert!(s.take_full_redraw(), "push_error");

    // Сворачивание «мыслей» (`Ctrl+T`) перекраивает все блоки — тоже изменение.
    s.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
    assert!(s.take_full_redraw(), "Ctrl+T (сворачивание мыслей)");
}

#[test]
fn suggest_popup_actions_request_full_redraw() {
    // Тот же класс, что у попапа эмодзи: пункт «➕ добавить в словарь» несёт широкий
    // глиф. При ЗАКРЫТИИ попапа его хвостовую ячейку diff шлёт только у стилизованной
    // строки, поэтому страхуемся полной перерисовкой; при сдвиге выделения она пользы
    // не даёт (сентинел пропускает хвост широкого глифа, ratatui#2651).
    let mut s = ChatScreen::new();
    s.set_spellchecker(mk_checker());
    type_str(&mut s, "helo");
    s.open_suggestions();
    assert!(s.suggest.is_some(), "попап подсказок открылся");
    s.take_full_redraw(); // сбросить флаг от набора текста

    // Сдвиг выделения перерисовки НЕ требует: глиф остаётся широким, сентинел обязан
    // пропускать его хвост (ratatui#2651) — подсветку снимает перепечатка глифа.
    s.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert!(
        !s.take_full_redraw(),
        "сдвиг выделения перерисовку не требует"
    );

    // Клавиша, которая попап не меняет, перерисовку не просит.
    s.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert!(
        !s.take_full_redraw(),
        "no-op клавиша перерисовку не требует"
    );

    // Закрытие отменой (`Esc`).
    s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(s.suggest.is_none());
    assert!(s.take_full_redraw(), "отмена закрывает попап → перерисовка");

    // Закрытие применением подсказки (`Enter`).
    s.open_suggestions();
    s.take_full_redraw();
    s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(s.suggest.is_none());
    assert!(
        s.take_full_redraw(),
        "применение закрывает попап → перерисовка"
    );
}

#[test]
fn emoji_picker_actions_request_full_redraw() {
    // Широкий глиф эмодзи оставляет на conhost «висячую» хвостовую половину, когда
    // ИСЧЕЗАЕТ с экрана: хвосты нестилизованных глифов сетки diff не шлёт (канарейка
    // на границу апстрим-фикса — в `widgets::emoji_picker`). Поэтому полную
    // перерисовку просит закрытие попапа — но не сдвиг выделения, где глиф остаётся
    // широким и сентинел его хвост пропускает (ratatui#2651).
    let mut s = ChatScreen::new();
    assert!(!s.take_full_redraw(), "без попапа перерисовка не нужна");

    // Открытие само по себе перерисовки не требует — глиф ниоткуда не уходит.
    s.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    assert!(
        !s.take_full_redraw(),
        "открытие попапа перерисовку не требует"
    );

    // Сдвиг выделения перерисовки НЕ требует: глиф остаётся широким, сентинел обязан
    // пропускать его хвост (ratatui#2651) — подложку снимает перепечатка глифа.
    s.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert!(
        !s.take_full_redraw(),
        "сдвиг выделения перерисовку не требует"
    );

    // Закрытие вставкой (`Enter`).
    s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(s.emoji.is_none());
    assert!(
        s.take_full_redraw(),
        "вставка закрывает попап → перерисовка"
    );
    assert!(!s.take_full_redraw(), "флаг забирается однократно");

    // Закрытие отменой (`Esc`).
    s.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    s.take_full_redraw();
    s.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(s.emoji.is_none());
    assert!(s.take_full_redraw(), "отмена закрывает попап → перерисовка");
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
    assert!(s.input.is_empty(), "поле очищено после команды");
}

#[test]
fn invalid_rag_command_shows_note_and_does_not_send() {
    let mut s = ChatScreen::new();
    s.set_server_status(ready_statuses());
    type_str(&mut s, "/rag");
    let intent = s.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(intent, None, "ошибочная команда не отправляется");
    assert!(s.feed.iter().any(|m| m.role == FeedRole::Note));
}

#[test]
fn rag_progress_banner_lifecycle() {
    let mut s = ChatScreen::new();
    assert!(!s.is_rag_active());
    s.set_rag_progress(RagProgress::Started { total: 3 });
    assert!(s.is_rag_active());
    // Файл только начат (chunks_total=0) — суффикса чанков в баннере нет.
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
        "без chunks_total суффикса чанков быть не должно: {:?}",
        s.rag.as_ref().unwrap().text
    );
    // Прогресс по чанкам (chunks_total>0) — баннер несёт счётчик чанков.
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
        "баннер должен показывать чанки 16/42: {banner:?}"
    );
    s.set_rag_progress(RagProgress::Finished {
        files: 3,
        chunks: 9,
        errors: 0,
        cancelled: false,
    });
    assert!(!s.is_rag_active(), "по завершении баннер гаснет");
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
    // Обычный текст с ошибкой → проверяется (есть подчёркивания).
    type_str(&mut s, "helo");
    s.last_edit = None; // снять дебаунс, чтобы перепроверка прошла сразу
    assert!(s.maybe_recheck_spelling());
    assert!(
        !s.input.misspelled_is_empty(),
        "обычный текст проверяется орфографией"
    );
    // Делаем из строки команду — орфография снимается.
    s.input.clear();
    type_str(&mut s, "/rag add helo");
    s.last_edit = None;
    assert!(s.input_is_command());
    assert!(s.maybe_recheck_spelling());
    assert!(
        s.input.misspelled_is_empty(),
        "команда не проверяется орфографией"
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
    // Ноль — понятная заметка «ничего не найдено».
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
    // Пустая база — понятная заметка.
    s.set_rag_progress(RagProgress::Listed { sources: vec![] });
    assert!(
        s.feed
            .iter()
            .any(|m| m.role == FeedRole::Note && m.text.contains("база знаний пуста"))
    );
    // С источниками — счётчик чанков и имя источника.
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

/// Лockup в шапке справки рисуется, когда высота терминала позволяет показать и
/// знак, и весь список клавиш; знак прижат влево по полю списка (docs/branding.md §5).
#[test]
fn help_shows_logo_when_terminal_is_tall() {
    use crate::widgets::logo::{LOCKUP_ROWS, LOGO_COLS};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    const ORANGE: Color = Color::Rgb(0xc2, 0x5a, 0x27);

    let mut s = ChatScreen::new();
    s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    // Запас: рамка (2) + все клавиши + блок лockup'а (отбивки сверху и снизу).
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
    // Ствол глифа фирменного цвета есть в каждой его строке, плюс «fork» в слове.
    assert!(
        orange.len() > LOCKUP_ROWS as usize,
        "фирменного цвета слишком мало — знак не нарисован (найдено {})",
        orange.len()
    );
    // Левая рамка попапа: скруглённый угол (палитра по умолчанию — Auto). Панели
    // самого чата рисуются во всю ширину, т.е. их углы — в колонке 0; попап
    // центрирован и вставлен внутрь, поэтому его угол — самый правый из найденных.
    let corner = (buf.area.top()..buf.area.bottom())
        .flat_map(|y| (buf.area.left()..buf.area.right()).map(move |x| (x, y)))
        .filter(|&(x, y)| buf[(x, y)].symbol() == "╭")
        .max_by_key(|&(x, _)| x)
        .expect("рамка попапа");
    // Знак прижат влево: ствол глифа (колонки 4–5 его чернил) стоит ровно на поле
    // списка клавиш — рамка + два пробела. При центрировании он уехал бы вправо.
    let left = orange.iter().map(|(x, _)| *x).min().unwrap();
    assert_eq!(
        left,
        corner.0 + 1 + 2 + 4,
        "ствол глифа не на левом поле списка клавиш"
    );
    // «fork» вордмарка — справа от глифа, за его правым краем.
    let right = orange.iter().map(|(x, _)| *x).max().unwrap();
    assert!(
        right > corner.0 + 1 + 2 + LOGO_COLS,
        "«fork» вордмарка не нарисован справа от глифа"
    );
}

/// На невысоком терминале логотип не рисуется вовсе — список клавиш не сдвигается
/// и не требует лишней прокрутки (жёсткая деградация, docs/branding.md §5).
#[test]
fn help_hides_logo_when_terminal_is_short() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    const ORANGE: Color = Color::Rgb(0xc2, 0x5a, 0x27);

    let mut s = ChatScreen::new();
    s.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    // Диалог высотой 13 (внутри 11 рядов) не вмещает лockup с отбивками → он не
    // рисуется, вкладки не сдвигаются вниз.
    let mut term = Terminal::new(TestBackend::new(90, 13)).unwrap();
    term.draw(|f| s.render(f)).unwrap();

    let buf = term.backend().buffer();
    for y in buf.area.top()..buf.area.bottom() {
        for x in buf.area.left()..buf.area.right() {
            let c = &buf[(x, y)];
            assert_ne!(c.style().fg, Some(ORANGE), "логотип не должен рисоваться");
            assert_ne!(c.style().bg, Some(ORANGE), "логотип не должен рисоваться");
        }
    }
}
/// Каждая вкладка диалога справки рисует своё характерное содержимое, а таб-стрип —
/// все четыре вкладки.
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

    // Таб-стрип на любой вкладке несёт все пять ярлыков.
    let about = text_for(HelpTab::About);
    for label in [
        "О программе",
        "Горячие клавиши",
        "Команды",
        "Лицензия",
        "Компоненты",
    ] {
        assert!(about.contains(label), "нет ярлыка вкладки «{label}»");
    }
    // «О программе»: бренд-имя, автор, версия, ссылки.
    assert!(about.contains("Vladimir Shylov"), "нет автора");
    assert!(about.contains(env!("CARGO_PKG_VERSION")), "нет версии");
    assert!(about.contains("https://mindfork.io"), "нет ссылки на сайт");
    assert!(
        about.contains("https://crates.io/crates/mindfork"),
        "нет ссылки на крейт"
    );

    // «Горячие клавиши»: подпись из HELP_KEYS, но НЕ команды (они на своей вкладке).
    let hotkeys = text_for(HelpTab::Hotkeys);
    assert!(
        hotkeys.contains("отправить сообщение"),
        "нет описания клавиши"
    );
    assert!(
        !hotkeys.contains("/rag add"),
        "команды не должны быть на вкладке горячих клавиш"
    );

    // «Команды»: команды поля ввода.
    let commands = text_for(HelpTab::Commands);
    assert!(commands.contains("/rag add"), "нет команды /rag add");
    assert!(commands.contains("/tts"), "нет команды /tts");

    // «Лицензия»: текст MIT.
    let license = text_for(HelpTab::License);
    assert!(license.contains("MIT License"), "нет заголовка лицензии");
    assert!(license.contains("WARRANTY"), "нет тела лицензии");

    // «Компоненты»: имя, версия и лицензия (берём из начала списка — он длинный и
    // прокручивается, дальние крейты за пределами экрана).
    let components = text_for(HelpTab::Components);
    assert!(components.contains("ansi-to-tui"), "нет компонента");
    assert!(components.contains("8.0.1"), "нет версии компонента");
    assert!(
        components.contains("Zlib OR Apache-2.0 OR MIT"),
        "нет лицензии компонента"
    );
}
