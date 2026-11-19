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

/// Like [`indexed_chat`], with one sub-agent transcript hung on the last
/// message's tool-call record. Returns the chat id, the transcript id and the
/// transcript's message ids.
fn indexed_chat_with_run(
    orch: &mut Orchestrator,
    title: &str,
    own: &[&str],
    run_title: &str,
    run_texts: &[&str],
) -> (Uuid, Uuid, Vec<Uuid>) {
    let profile = Profile::new("P", "sys");
    let mut chat = Chat::from_profile(&profile, title);
    for t in own {
        chat.push_message(Message::user(*t));
    }
    let run = crate::entities::subagent::SubagentRun::fixture(run_title, run_texts);
    let run_id = run.id;
    let run_msgs: Vec<Uuid> = run.messages.iter().map(|m| m.id).collect();
    let mut carrier = Message::assistant("delegated");
    carrier.tool_calls = vec![run.on_record()];
    chat.push_message(carrier);
    orch.storage.json().save_chat(&chat).unwrap();
    orch.index_saved_chat(&chat);
    let id = chat.id;
    orch.chats.push(chat);
    (id, run_id, run_msgs)
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
                sub_id: None,
                role: "user".into(),
                ts: "2026-07-29T10:00:00+00:00".into(),
                text: "призрачный маркер".into(),
                parked: false,
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

/// Sub-agent transcripts in the results (spec §11.2.1, research §3.9): a
/// transcript's hits form a group of their own right after its parent's,
/// carrying `parent`; a parent none of whose own messages match still heads
/// the group, with no hits; and the `Ctrl+F` id set names the transcript by
/// its own id — and *not* the parent, so the list's membership rule can
/// dim it.
#[test]
fn message_search_nests_a_transcripts_hits_under_its_parent() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let (parent, run, run_msgs) = indexed_chat_with_run(
        &mut orch,
        "Родитель",
        &["свой маркер", "и тайна"],
        "Критик",
        &["маркер в задании", "маркер в ответе", "тайна внутри"],
    );
    let (_plain, _) = indexed_chat(&mut orch, "Обычный", &["маркер тут"]);

    // Both levels match: the parent's group, then the transcript's.
    let (groups, total) = message_search(&mut orch, &mut rx, "маркер");
    assert_eq!(total, 4);
    let shape: Vec<(Uuid, Option<Uuid>, usize)> = groups
        .iter()
        .map(|g| (g.chat_id, g.parent, g.hits.len()))
        .collect();
    let i = shape.iter().position(|g| g.0 == parent).unwrap();
    assert_eq!(shape[i], (parent, None, 1));
    assert_eq!(shape[i + 1], (run, Some(parent), 2), "{shape:?}");
    assert_eq!(groups[i + 1].title, "Критик");
    let hits: Vec<Uuid> = groups[i + 1].hits.iter().map(|h| h.message_id).collect();
    assert_eq!(hits, vec![run_msgs[0], run_msgs[1]], "in the run's order");

    // Only the transcript matches: the parent still heads it, with 0 hits.
    let (groups, total) = message_search(&mut orch, &mut rx, "внутри");
    assert_eq!(total, 1);
    assert_eq!(groups.len(), 2, "{groups:?}");
    assert_eq!((groups[0].chat_id, groups[0].hits.len()), (parent, 0));
    assert_eq!((groups[1].chat_id, groups[1].parent), (run, Some(parent)));

    // The id set for the list filter.
    let ids = |orch: &Orchestrator, rx: &mut UnboundedReceiver<AppEvent>, q: &str| {
        orch.handle_search_chats(q.into());
        loop {
            if let AppEvent::ChatSearchResults { chat_ids, .. } = rx.try_recv().unwrap() {
                return chat_ids.unwrap();
            }
        }
    };
    assert_eq!(
        ids(&orch, &mut rx, "внутри"),
        vec![run],
        "the transcript stands for itself"
    );
    let mut both = ids(&orch, &mut rx, "тайна");
    both.sort();
    let mut want = vec![parent, run];
    want.sort();
    assert_eq!(both, want);
}

/// `Enter` on a row in content mode: a transcript row opens the transcript on
/// *its* first match, a parent row on the parent's own — a match inside a
/// transcript never drags the parent to a message it does not have.
#[test]
fn first_match_in_chat_resolves_each_level_separately() {
    let (_d, mut orch, _rx) = bare_orch_rx();
    let (parent, run, run_msgs) = indexed_chat_with_run(
        &mut orch,
        "Родитель",
        &["без совпадения", "свой маркер"],
        "Критик",
        &["маркер в задании"],
    );
    let own_hit = orch.chats.iter().find(|c| c.id == parent).unwrap().messages[1].id;
    assert_eq!(orch.first_match_in_chat(run, "маркер"), Some(run_msgs[0]));
    assert_eq!(orch.first_match_in_chat(parent, "маркер"), Some(own_hit));
    assert_eq!(orch.first_match_in_chat(parent, "задании"), None);
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
    match ev {
        AppEvent::ChatActivated { id, focus, .. } => {
            assert_eq!(id, chat_id, "the chat must open");
            let focus = focus.expect("on the message that matched");
            assert_eq!(focus.message, user_msg);
            // The query travels too, so the feed can highlight it inside that
            // message rather than only marking the bubble (fork S3(b)).
            assert_eq!(focus.query, "кристалл");
        }
        _ => unreachable!(),
    }

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

// ---- search in parked branches (docs/research/chat-branches.md §10.3) ----

use crate::entities::chat_branch::{Branch, ParkCause};

/// What a test needs to name in a chat with parked tails.
struct Parked {
    chat: Uuid,
    /// The redone reply, which names the password and is parked.
    old_reply: Uuid,
    /// The reply that replaced it — parked too once the chat is forked above.
    new_reply: Uuid,
}

/// `q1 a1 q2 a2 q3 a3` a minute apart, with the last reply redone: the old
/// one, which names the password, is variant 1 of message #6 and is parked;
/// the new one says it does not remember. With `forked`, the chat is then
/// forked at its first reply and asked something else, so both replies are in
/// tails — the old one in a tail inside the forked one.
fn parked_chat(orch: &mut Orchestrator, title: &str, forked: bool) -> Parked {
    let at = |minutes: i64| {
        chrono::Utc::now() - chrono::Duration::hours(2) + chrono::Duration::minutes(minutes)
    };
    let said = |mut m: Message, minutes: i64| {
        m.timestamp = at(minutes);
        m
    };
    let mut chat = Chat::from_profile(&Profile::new("P", "sys"), title);
    for (i, m) in [
        Message::user("вопрос один"),
        Message::assistant("ответ один"),
        Message::user("вопрос два"),
        Message::assistant("ответ два"),
        Message::user("а пароль от лодки?"),
        Message::assistant("пароль — ЗАНЗИБАР-42"),
    ]
    .into_iter()
    .enumerate()
    {
        chat.push_message(said(m, i as i64));
    }
    let old_reply = chat.messages[5].id;
    assert!(chat.park_tail(5, ParkCause::Regenerate, at(10)));
    chat.push_message(said(Message::assistant("пароля я не помню"), 11));
    let new_reply = chat.messages[5].id;
    if forked {
        assert!(chat.fork_at(2, at(20)));
        chat.push_message(said(Message::user("начнём иначе"), 21));
    }
    orch.storage.json().save_chat(&chat).unwrap();
    orch.index_saved_chat(&chat);
    let id = chat.id;
    orch.chats.push(chat);
    Parked {
        chat: id,
        old_reply,
        new_reply,
    }
}

/// The ids a content query keeps in the chat list.
fn list_filter(
    orch: &Orchestrator,
    rx: &mut UnboundedReceiver<AppEvent>,
    query: &str,
) -> Vec<Uuid> {
    orch.handle_search_chats(query.into());
    loop {
        if let AppEvent::ChatSearchResults { chat_ids, .. } = rx.try_recv().unwrap() {
            return chat_ids.unwrap();
        }
    }
}

/// What an open sent down: the activation's chat and focus, and the notice
/// that followed it, if one did.
fn opened(
    rx: &mut UnboundedReceiver<AppEvent>,
) -> (Uuid, Option<crate::app::events::FeedFocus>, Option<String>) {
    let mut activated = None;
    let mut notice = None;
    while let Ok(event) = rx.try_recv() {
        match event {
            AppEvent::ChatActivated { id, focus, .. } => activated = Some((id, focus)),
            AppEvent::Notice(text) => notice = Some(text),
            _ => {}
        }
    }
    let (id, focus) = activated.expect("the open was answered with an activation");
    (id, focus, notice)
}

/// The setting is off unless the person switched it on: a word said only in a
/// parked tail finds nothing, on the screen and in the list. On, the tail's
/// hits are a group of their own under the chat's — which heads it even with
/// no hit of its own — titled by the address `/branch` lists the tail under.
#[test]
fn a_word_said_only_in_a_branch_is_found_when_the_person_asks_for_it() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let boat = parked_chat(&mut orch, "Лодка", false);
    assert!(
        !orch.config.interface.search_parked_branches,
        "off by default"
    );

    let (groups, total) = message_search(&mut orch, &mut rx, "занзибар");
    assert!(groups.is_empty() && total == 0, "{groups:?}");
    assert!(list_filter(&orch, &mut rx, "занзибар").is_empty());

    orch.config.interface.search_parked_branches = true;
    let (groups, total) = message_search(&mut orch, &mut rx, "занзибар");
    assert_eq!(total, 1);
    assert_eq!(groups.len(), 2, "{groups:?}");
    assert_eq!(
        (groups[0].chat_id, groups[0].parent, groups[0].hits.len()),
        (boat.chat, None, 0),
        "the chat heads its branch"
    );
    assert!(groups[0].branch.is_none());
    let branch = &groups[1];
    assert_eq!(
        (branch.chat_id, branch.parent),
        (boat.chat, Some(boat.chat))
    );
    assert_eq!(branch.title, "6/1", "the address `/branch` shows");
    let tag = branch.branch.expect("a branch's group says so");
    assert_eq!((tag.cause, tag.detached), (ParkCause::Regenerate, false));
    let hits: Vec<Uuid> = branch.hits.iter().map(|h| h.message_id).collect();
    assert_eq!(hits, vec![boat.old_reply]);
    assert_eq!(list_filter(&orch, &mut rx, "занзибар"), vec![boat.chat]);

    // A word on the line and in the tail: a hit in each, the chat listed once.
    let (groups, total) = message_search(&mut orch, &mut rx, "парол");
    assert_eq!(total, 3, "the question, the line's reply, the parked one");
    assert_eq!(groups[0].hits.len(), 2);
    assert_eq!(groups[1].hits.len(), 1);
    assert_eq!(list_filter(&orch, &mut rx, "парол"), vec![boat.chat]);

    // And the address figures are `Chat::branch_view`'s.
    let chat = orch.chats.iter().find(|c| c.id == boat.chat).unwrap();
    let view = chat.branch_view();
    assert_eq!((view.parked[0].number, view.parked[0].variant), (6, 1));
}

/// Tails come in `/branch`'s order, each under its own address: a tail that
/// continues from the line, the tail inside it, and a detached one last.
#[test]
fn nested_and_detached_tails_are_groups_under_their_addresses() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    orch.config.interface.search_parked_branches = true;
    let boat = parked_chat(&mut orch, "Лодка", true);
    {
        let chat = orch.chats.iter_mut().find(|c| c.id == boat.chat).unwrap();
        chat.branches.push(Branch {
            after: Some(Uuid::new_v4()),
            messages: vec![Message::user("потерянный пароль")],
            parked_at: chrono::Utc::now(),
            cause: ParkCause::Fork,
            compaction: None,
            reflected_upto: None,
        });
        let chat = chat.clone();
        orch.storage.json().save_chat(&chat).unwrap();
        orch.index_saved_chat(&chat);
    }

    // The word for "password": the question and the reply that replaced the
    // redone one are in the forked tail, the redone reply in the tail inside
    // it, and the detached tail says it too. Nothing on the line does.
    let (groups, total) = message_search(&mut orch, &mut rx, "парол");
    assert_eq!(total, 4);
    let shape: Vec<(&str, usize, bool)> = groups
        .iter()
        .map(|g| {
            (
                g.title.as_str(),
                g.hits.len(),
                g.branch.is_some_and(|t| t.detached),
            )
        })
        .collect();
    assert_eq!(
        shape,
        [
            ("Лодка", 0, false),
            ("3/1", 2, false),
            ("3/1 > 6/1", 1, false),
            ("0/1", 1, true),
        ]
    );
    assert_eq!(groups[1].branch.map(|t| t.cause), Some(ParkCause::Fork));
    // A tail's hits are in the order they were said.
    let chat = orch.chats.iter().find(|c| c.id == boat.chat).unwrap();
    let outer = chat
        .branches
        .iter()
        .find(|b| b.messages.iter().any(|m| m.id == boat.new_reply))
        .unwrap();
    let question = outer.messages[2].id;
    let hits: Vec<Uuid> = groups[1].hits.iter().map(|h| h.message_id).collect();
    assert_eq!(hits, vec![question, boat.new_reply]);
}

/// `Enter` on a hit in a parked branch (the owner's H3): the chat opens on
/// its line, the feed where the branch leaves it, a note names the branch by
/// its address — and **nothing is switched**: the line and the tails are what
/// they were.
#[test]
fn a_hit_in_a_parked_branch_opens_the_chat_on_its_line_and_switches_nothing() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    orch.config.interface.language = crate::shared::i18n::Lang::En;
    let boat = parked_chat(&mut orch, "Лодка", false);
    let before = orch
        .chats
        .iter()
        .find(|c| c.id == boat.chat)
        .cloned()
        .unwrap();

    orch.handle_open_chat_at(boat.chat, boat.old_reply, "занзибар".into());
    let (id, focus, notice) = opened(&mut rx);
    assert_eq!(id, boat.chat);
    let focus = focus.expect("the feed is put where the branch leaves the line");
    assert_eq!(
        focus.message, boat.new_reply,
        "the line's own variant of that place — the bubble with the mark"
    );
    assert_eq!(focus.query, "", "the match is not in the message looked at");
    assert_eq!(
        notice.as_deref(),
        Some(
            "The match is in parked branch 6/1: variant 1 of message #6. Nothing was \
             switched. /branch 6 1 brings it in front of you."
        )
    );
    let after = orch.chats.iter().find(|c| c.id == boat.chat).unwrap();
    assert_eq!(after.messages, before.messages, "the line did not move");
    assert_eq!(after.branches, before.branches, "nor did a tail");

    // A hit on the line opens on its message, with the query and no note.
    orch.handle_open_chat_at(boat.chat, boat.new_reply, "помню".into());
    let (_, focus, notice) = opened(&mut rx);
    let focus = focus.unwrap();
    assert_eq!(
        (focus.message, focus.query.as_str()),
        (boat.new_reply, "помню")
    );
    assert_eq!(notice, None);
}

/// A tail inside another is looked at where the **outer** tail leaves the
/// line, and the note sends the person to the list — `/branch N K` does not
/// reach it. A detached tail has no place: the chat opens all the same — the
/// screen that asked is waiting for it, the open chat included — and the note
/// says why the branch cannot come back.
#[test]
fn a_hit_in_a_nested_or_a_detached_tail_says_how_it_is_reached() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    orch.config.interface.language = crate::shared::i18n::Lang::En;
    let boat = parked_chat(&mut orch, "Лодка", true);
    let lost = Uuid::new_v4();
    {
        let chat = orch.chats.iter_mut().find(|c| c.id == boat.chat).unwrap();
        let mut m = Message::user("потерянный пароль");
        m.id = lost;
        chat.branches.push(Branch {
            after: Some(Uuid::new_v4()),
            messages: vec![m],
            parked_at: chrono::Utc::now(),
            cause: ParkCause::Fork,
            compaction: None,
            reflected_upto: None,
        });
    }
    let standing = orch
        .chats
        .iter()
        .find(|c| c.id == boat.chat)
        .unwrap()
        .messages[2]
        .id;

    // The redone reply is in the tail inside the forked one.
    orch.handle_open_chat_at(boat.chat, boat.old_reply, "занзибар".into());
    let (_, focus, notice) = opened(&mut rx);
    assert_eq!(focus.map(|f| f.message), Some(standing));
    let notice = notice.unwrap();
    assert!(
        notice.starts_with("The match is in parked branch 3/1 > 6/1, which continues from inside"),
        "{notice}"
    );
    assert!(notice.contains("the list /branch opens"), "{notice}");

    // The reply that replaced it is in the forked tail itself.
    orch.handle_open_chat_at(boat.chat, boat.new_reply, "помню".into());
    let (_, focus, notice) = opened(&mut rx);
    assert_eq!(focus.map(|f| f.message), Some(standing));
    assert!(notice.unwrap().contains("/branch 3 1 brings it"));

    // The detached tail — asked for while this very chat is the open one.
    assert_eq!(orch.active_id, Some(boat.chat));
    orch.handle_open_chat_at(boat.chat, lost, "потерянный".into());
    let (id, focus, notice) = opened(&mut rx);
    assert_eq!((id, focus), (boat.chat, None));
    let notice = notice.unwrap();
    assert!(notice.starts_with("The match is in branch 0/1"), "{notice}");
    assert!(notice.contains("cannot be brought back"), "{notice}");
    // And from another chat.
    let (other, _) = indexed_chat(&mut orch, "Другой", &["ничего"]);
    orch.handle_switch(other);
    while rx.try_recv().is_ok() {}
    orch.handle_open_chat_at(boat.chat, lost, "потерянный".into());
    let (id, focus, notice) = opened(&mut rx);
    assert_eq!((id, focus), (boat.chat, None));
    assert!(notice.is_some());
}

/// `Enter` in the chat list on a chat the filter kept for a match in a
/// parked branch: with nothing on the line to land on, the chat opens where
/// that branch leaves it and the note says so. A match on the line still
/// wins, and with the setting off the list never kept the chat at all — an
/// open then lands at the tail, as it always did.
#[test]
fn the_lists_enter_on_a_chat_matched_only_in_a_branch_says_where() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    orch.config.interface.language = crate::shared::i18n::Lang::En;
    let boat = parked_chat(&mut orch, "Лодка", false);
    let (other, _) = indexed_chat(&mut orch, "Другой", &["ничего"]);

    orch.handle_open_chat_at_first_match(boat.chat, "занзибар");
    let (id, focus, notice) = opened(&mut rx);
    assert_eq!(
        (id, focus, notice),
        (boat.chat, None, None),
        "off: the tail"
    );

    orch.config.interface.search_parked_branches = true;
    orch.handle_switch(other);
    while rx.try_recv().is_ok() {}
    orch.handle_open_chat_at_first_match(boat.chat, "занзибар");
    let (id, focus, notice) = opened(&mut rx);
    assert_eq!(id, boat.chat);
    assert_eq!(focus.map(|f| f.message), Some(boat.new_reply));
    assert!(
        notice
            .unwrap()
            .starts_with("The match is in parked branch 6/1"),
        "the list's open names the branch"
    );

    // The word for "password" is on the line too: the line's first match, as
    // ever.
    let question = orch
        .chats
        .iter()
        .find(|c| c.id == boat.chat)
        .unwrap()
        .messages[4]
        .id;
    orch.handle_open_chat_at_first_match(boat.chat, "парол");
    let (_, focus, notice) = opened(&mut rx);
    let focus = focus.unwrap();
    assert_eq!((focus.message, focus.query.as_str()), (question, "парол"));
    assert_eq!(notice, None);
}

/// Results are a snapshot: the person may switch a branch and come back to
/// them. A hit is opened by where its message stands **then** — the reply
/// that was parked when it was found opens as a message of the line once its
/// branch is on it, and the one it displaced is then the parked one.
#[test]
fn a_hit_is_opened_by_where_its_message_stands_now() {
    let (_d, mut orch, mut rx) = bare_orch_rx();
    orch.config.interface.language = crate::shared::i18n::Lang::En;
    let boat = parked_chat(&mut orch, "Лодка", false);
    {
        let chat = orch.chats.iter_mut().find(|c| c.id == boat.chat).unwrap();
        chat.switch_to(boat.old_reply, chrono::Utc::now()).unwrap();
    }

    orch.handle_open_chat_at(boat.chat, boat.old_reply, "занзибар".into());
    let (_, focus, notice) = opened(&mut rx);
    let focus = focus.unwrap();
    assert_eq!(
        (focus.message, focus.query.as_str()),
        (boat.old_reply, "занзибар")
    );
    assert_eq!(notice, None);

    orch.handle_open_chat_at(boat.chat, boat.new_reply, "помню".into());
    let (_, focus, notice) = opened(&mut rx);
    assert_eq!(focus.map(|f| f.message), Some(boat.old_reply));
    assert!(
        notice.unwrap().contains("parked branch 6/2"),
        "by time: variant 2"
    );
}
