//! Orchestrator tests — chat content search (`cache.db`). Part of the [`super`]
//! module (fixtures in mod.rs). See docs/research/chat-content-search.md.
//!
//! The reconciliation logic itself is unit-tested synchronously in
//! `orchestrator/search.rs`; these tests pin the wiring through the **real
//! `run` loop**: that a saved chat reaches the index, that the index survives a
//! restart, and what `SearchChats` answers.
//!
//! Two phases rather than a sleep: the post-save index update rides the 800 ms
//! save debounce, and `Quit` flushes it deterministically — so session 1 writes
//! and session 2 asks. Same shape as `remembers_and_restores_last_opened_chat`.

use super::*;
use crate::features::chat_search_sort::SortMode;

/// Runs one turn against a scripted engine, so the chat gains real messages.
fn scripted(reply: &str) -> Arc<dyn EngineBackend> {
    Arc::new(MockBackend::scripted(vec![
        ChatChunk::Text(reply.into()),
        ChatChunk::Finished(crate::shared::api::FinishReason::Stop),
    ]))
}

/// Sends a message, waits for the reply, then quits — flushing the save (and
/// with it the index update) before the loop returns. Returns the chat's id.
async fn record_a_conversation(root: &std::path::Path, user: &str, assistant: &str) -> Uuid {
    let (cmd_tx, mut evt_rx, handle) =
        spawn_orch_at(root, Some(scripted(assistant)), AppConfig::default());
    let activated = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let chat_id = match activated {
        AppEvent::ChatActivated { id, .. } => id,
        _ => unreachable!(),
    };
    cmd_tx.send(AppCommand::SendMessage(user.into())).unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Finished { .. }))
        .await
        .unwrap();
    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
    chat_id
}

/// Asks the running orchestrator a content query and returns what it answered.
async fn search(
    cmd_tx: &UnboundedSender<AppCommand>,
    evt_rx: &mut UnboundedReceiver<AppEvent>,
    query: &str,
) -> Option<Vec<Uuid>> {
    cmd_tx.send(AppCommand::SearchChats(query.into())).unwrap();
    let ev = wait_for(evt_rx, |e| matches!(e, AppEvent::ChatSearchResults { .. }))
        .await
        .unwrap();
    match ev {
        AppEvent::ChatSearchResults {
            query: echoed,
            chat_ids,
        } => {
            assert_eq!(echoed, query, "the query must be echoed back verbatim");
            chat_ids
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn a_saved_conversation_becomes_searchable_by_content() {
    // The end-to-end property: text the user never put in a *title* is findable.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let chat_id = record_a_conversation(&root, "расскажи про кристаллографию", "конечно").await;

    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(&root, None, AppConfig::default());
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // The user's message…
    assert_eq!(
        search(&cmd_tx, &mut evt_rx, "кристалл").await,
        Some(vec![chat_id])
    );
    // …and the assistant's reply are both in the index.
    assert_eq!(
        search(&cmd_tx, &mut evt_rx, "конечно").await,
        Some(vec![chat_id])
    );
    // Only message text is indexed (fork F3): the chat's own title — the one
    // bootstrap gave it, `defaults.chat_title` — is not in the index. Content
    // search and the title filter stay separate mechanisms rather than one
    // blurred into the other. Taken from the bundle rather than spelled out, so
    // the test does not pin a translatable string.
    let bootstrap_title =
        crate::shared::i18n::locale(crate::shared::i18n::Lang::default()).t("defaults.chat_title");
    assert_eq!(
        search(&cmd_tx, &mut evt_rx, bootstrap_title).await,
        Some(Vec::new())
    );

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn a_query_matching_nothing_returns_an_empty_list() {
    // Empty is NOT the same answer as "don't filter": the list must end up
    // empty, not unfiltered.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    record_a_conversation(&root, "про кошек", "мяу").await;

    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(&root, None, AppConfig::default());
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    assert_eq!(
        search(&cmd_tx, &mut evt_rx, "бетономешалка").await,
        Some(Vec::new())
    );

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn a_too_short_query_does_not_filter() {
    // Trigram cannot match fewer than 3 characters (research §5), so such a
    // query answers `None` — "show everything" — rather than an empty list.
    // Escaping happens in the orchestrator: `C++` would be an FTS5 syntax error
    // raw, and here it is simply too short after the floor drops nothing…
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    assert_eq!(search(&cmd_tx, &mut evt_rx, "ab").await, None);
    assert_eq!(search(&cmd_tx, &mut evt_rx, "   ").await, None);
    // …while `C++` survives the floor and must come back as a *search*, not an
    // error — that is what the escaping buys (research §4).
    assert_eq!(search(&cmd_tx, &mut evt_rx, "C++").await, Some(Vec::new()));

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

// ---- stage 2b: message-level search ----

/// Puts a chat into the orchestrator, saves it and indexes it exactly as a save
/// would. Returns the chat and its message ids.
fn indexed_chat(orch: &mut Orchestrator, title: &str, texts: &[&str]) -> (Uuid, Vec<Uuid>) {
    let profile = Profile::new("P", "sys");
    let mut chat = Chat::from_profile(&profile, title);
    for t in texts {
        chat.push_message(Message::user(*t));
    }
    let ids: Vec<Uuid> = chat.messages.iter().map(|m| m.id).collect();
    orch.storage.json().save_chat(&chat).unwrap();
    orch.index_saved_chat(&chat);
    let id = chat.id;
    orch.chats.push(chat);
    (id, ids)
}

/// The reply to a message-level query, taken off the event channel.
fn message_search(
    orch: &mut Orchestrator,
    rx: &mut UnboundedReceiver<AppEvent>,
    query: &str,
) -> (Vec<crate::features::chat_search::SearchGroup>, usize) {
    message_search_sorted(orch, rx, query, SortMode::Modified)
}

/// The same, for the tests that care which way the chat list is sorted.
fn message_search_sorted(
    orch: &mut Orchestrator,
    rx: &mut UnboundedReceiver<AppEvent>,
    query: &str,
    sort: SortMode,
) -> (Vec<crate::features::chat_search::SearchGroup>, usize) {
    orch.handle_search_messages(query.into(), sort);
    loop {
        match rx.try_recv().expect("no MessageSearchResults arrived") {
            AppEvent::MessageSearchResults {
                query: echoed,
                groups,
                total,
            } => {
                assert_eq!(echoed, query, "the query must be echoed back verbatim");
                return (groups, total);
            }
            _ => continue,
        }
    }
}

/// Fork S2: hits are grouped by chat, chats in the chat list's order (most
/// recently modified first), messages in **chat** order — none of which the
/// index can answer on its own, which is why grouping lives in the orchestrator.
#[test]
fn message_search_groups_by_chat_in_list_order_with_titles() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let (older, older_msgs) = indexed_chat(
        &mut orch,
        "Старый чат",
        &["первое про маркер", "без совпадения", "второе про маркер"],
    );
    let (newer, _) = indexed_chat(&mut orch, "Свежий чат", &["ещё раз маркер"]);
    // A chat present in the index but not in the orchestrator (deleted between
    // indexing and now) must be dropped rather than shown as unopenable.
    orch.storage
        .cache()
        .index_chat(
            Uuid::new_v4(),
            1,
            1,
            &[crate::shared::storage::cache::IndexedMessage {
                id: Uuid::new_v4(),
                role: "user".into(),
                ts: "2026-07-29T10:00:00+00:00".into(),
                text: "призрачный маркер".into(),
            }],
        )
        .unwrap();

    // Make the index order differ from chat order, which is the only way this
    // test can tell the two apart: editing a message deletes and re-inserts its
    // row, so the *first* message ends up with the *highest* rowid.
    let chat = orch.chats.iter_mut().find(|c| c.id == older).unwrap();
    chat.messages[0].text = "первое про маркер, переписанное".into();
    let chat = chat.clone();
    orch.storage.json().save_chat(&chat).unwrap();
    orch.index_saved_chat(&chat);

    // Make the chat ordering explicit rather than relying on creation timing.
    let now = chrono::Utc::now();
    for chat in &mut orch.chats {
        chat.modified_at = if chat.id == newer {
            now
        } else {
            now - chrono::Duration::hours(1)
        };
    }

    let (groups, total) = message_search(&mut orch, &mut rx, "маркер");
    assert_eq!(groups.len(), 2, "the unknown chat's hit is dropped");
    assert_eq!(groups[0].chat_id, newer, "most recently modified first");
    assert_eq!(
        groups[0].title, "Свежий чат",
        "the title comes from the chat"
    );
    assert_eq!(groups[1].chat_id, older);
    assert_eq!(groups[1].title, "Старый чат");

    // Within a chat: **chat** order (not the index's), and only the matching
    // messages.
    let hits: Vec<Uuid> = groups[1].hits.iter().map(|h| h.message_id).collect();
    assert_eq!(
        hits,
        vec![older_msgs[0], older_msgs[2]],
        "hits must follow the conversation, not the index's rowids"
    );
    // Each hit carries what the screen shows, snippet highlights included.
    let first = &groups[1].hits[0];
    assert_eq!(first.role, "user");
    assert!(!first.ts.is_empty());
    assert!(first.snippet.text.contains("маркер"), "{:?}", first.snippet);
    assert_eq!(
        first.snippet.matches.len(),
        1,
        "the match must be located for highlighting"
    );
    // `total` counts the index, which still holds the ghost chat's hit.
    assert_eq!(total, 4);
}

/// The cap truncates the hits, never the count — that is what lets the screen
/// say "showing N of M" instead of quietly dropping results.
#[test]
fn message_search_reports_the_true_total_when_the_cap_bites() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let cap = crate::features::chat_search::HIT_CAP;
    let texts: Vec<String> = (0..cap + 5)
        .map(|i| format!("совпадение номер {i}"))
        .collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    indexed_chat(&mut orch, "Большой чат", &refs);

    let (groups, total) = message_search(&mut orch, &mut rx, "совпадение");
    let shown: usize = groups.iter().map(|g| g.hits.len()).sum();
    assert_eq!(shown, cap, "the hits are capped");
    assert_eq!(total, cap + 5, "the total stays honest");
}

/// An unsearchable query answers with nothing at all — no groups, no error
/// popup. (Unlike the chat-list filter, where "nothing searchable" means "show
/// everything": here there is no list to leave unfiltered.)
#[test]
fn an_unsearchable_message_query_yields_no_groups() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    indexed_chat(&mut orch, "Чат", &["какое-то содержимое"]);

    for query in ["ab", "   ", ""] {
        let (groups, total) = message_search(&mut orch, &mut rx, query);
        assert!(groups.is_empty(), "query {query:?}");
        assert_eq!(total, 0, "query {query:?}");
    }
    // And a query that matches nothing is empty too, not "everything".
    let (groups, total) = message_search(&mut orch, &mut rx, "бетономешалка");
    assert!(groups.is_empty());
    assert_eq!(total, 0);
}

/// `Enter` in content mode opens a chat at its **first** match in conversation
/// order. The index cannot answer this: a message whose text changed is deleted
/// and re-inserted with a fresh rowid, so index order is not chat order — which
/// is exactly what this test arranges.
#[test]
fn first_match_in_chat_uses_chat_order_not_index_order() {
    let (_d, mut orch, _rx) = bare_orch_rx();
    let (chat_id, msgs) = indexed_chat(
        &mut orch,
        "Чат",
        &["первое про маркер", "без", "третье про маркер"],
    );

    // Edit the *first* message, as a re-save would: it leaves and re-enters the
    // index, taking the highest rowid — behind the third message.
    let chat = orch.chats.iter_mut().find(|c| c.id == chat_id).unwrap();
    chat.messages[0].text = "первое про маркер, переписанное".into();
    let chat = chat.clone();
    orch.storage.json().save_chat(&chat).unwrap();
    orch.index_saved_chat(&chat);

    assert_eq!(
        orch.first_match_in_chat(chat_id, "маркер"),
        Some(msgs[0]),
        "the earliest message in the conversation, not the lowest rowid"
    );
    // Nothing matching, an unsearchable query, or an unknown chat — no jump,
    // which the caller turns into a plain "open at the tail".
    assert_eq!(orch.first_match_in_chat(chat_id, "бетономешалка"), None);
    assert_eq!(orch.first_match_in_chat(chat_id, "ab"), None);
    assert_eq!(orch.first_match_in_chat(Uuid::new_v4(), "маркер"), None);
}

/// The whole `Enter`-in-content-mode path through the real loop: the chat is
/// activated **with the focus on its first match**, which is what stage 2a's
/// jump consumes.
#[tokio::test]
async fn open_chat_at_first_match_activates_with_the_focus() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let chat_id = record_a_conversation(&root, "вопрос про кристаллографию", "ответ").await;

    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(&root, None, AppConfig::default());
    let activated = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    let user_msg = match activated {
        AppEvent::ChatActivated { messages, .. } => messages[0].id,
        _ => unreachable!(),
    };

    // A second chat to jump *from*, so this exercises a real cross-chat open
    // rather than only moving the feed of the chat already on screen.
    cmd_tx
        .send(AppCommand::NewChat { profile_id: None })
        .unwrap();
    let other = match wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id != chat_id),
    )
    .await
    .unwrap()
    {
        AppEvent::ChatActivated { id, .. } => id,
        _ => unreachable!(),
    };

    cmd_tx
        .send(AppCommand::OpenChatAtFirstMatch {
            chat: chat_id,
            query: "кристалл".into(),
        })
        .unwrap();
    let ev = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    assert!(
        matches!(ev, AppEvent::ChatActivated { id, focus, .. }
            if id == chat_id && focus == Some(user_msg)),
        "the chat must open on the message that matched"
    );

    // Nothing matching → the chat still opens, just at its tail. (From the
    // other chat again: a focus-less jump onto the chat already open is a
    // deliberate no-op — there is nothing to move.)
    cmd_tx.send(AppCommand::SwitchChat(other)).unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatActivated { id, .. } if *id == other),
    )
    .await
    .unwrap();
    cmd_tx
        .send(AppCommand::OpenChatAtFirstMatch {
            chat: chat_id,
            query: "бетономешалка".into(),
        })
        .unwrap();
    let ev = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    assert!(matches!(
        ev,
        AppEvent::ChatActivated { id, focus: None, .. } if id == chat_id
    ));

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn a_deleted_chat_drops_out_of_results() {
    // A hidden chat never appears in the list, so it must not appear in search
    // results either — including within the same session, before any
    // reconciliation gets a chance to notice.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let chat_id = record_a_conversation(&root, "секретное содержимое", "ага").await;

    let (cmd_tx, mut evt_rx, handle) = spawn_orch_at(&root, None, AppConfig::default());
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();
    assert_eq!(
        search(&cmd_tx, &mut evt_rx, "секретное").await,
        Some(vec![chat_id])
    );

    cmd_tx.send(AppCommand::DeleteChat(chat_id)).unwrap();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatList(_)))
        .await
        .unwrap();

    assert_eq!(
        search(&cmd_tx, &mut evt_rx, "секретное").await,
        Some(Vec::new()),
        "a deleted chat must leave the index at once"
    );

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();
}

/// Fork S2 promised "chats ordered by your existing sort", so the chat list's
/// `Tab` toggle has to carry over into the results. It did not at first — the
/// screen hardcoded `modified_at` — and nothing caught it, because the default
/// toggle position *is* `Modified`. This asserts the other position.
#[test]
fn message_search_follows_the_chat_lists_sort_toggle() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    // Created early, modified late; and the reverse. The two sort modes must
    // therefore put them in opposite orders.
    let (old_new, _) = indexed_chat(&mut orch, "Создан раньше", &["про маркер"]);
    let (new_old, _) = indexed_chat(&mut orch, "Создан позже", &["тоже маркер"]);
    {
        let a = orch.chats.iter_mut().find(|c| c.id == old_new).unwrap();
        a.created_at = "2020-01-01T00:00:00Z".parse().unwrap();
        a.modified_at = "2030-01-01T00:00:00Z".parse().unwrap();
        let b = orch.chats.iter_mut().find(|c| c.id == new_old).unwrap();
        b.created_at = "2029-01-01T00:00:00Z".parse().unwrap();
        b.modified_at = "2021-01-01T00:00:00Z".parse().unwrap();
    }

    let (by_modified, _) = message_search_sorted(&mut orch, &mut rx, "маркер", SortMode::Modified);
    let (by_created, _) = message_search_sorted(&mut orch, &mut rx, "маркер", SortMode::Created);

    assert_eq!(by_modified[0].chat_id, old_new, "newest modification first");
    assert_eq!(by_created[0].chat_id, new_old, "newest creation first");
}
