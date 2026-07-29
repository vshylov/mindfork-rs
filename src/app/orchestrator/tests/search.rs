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
