//! Keeping the chat-content search index (`cache.db`) in step with the chat
//! files, and answering content queries. See
//! docs/research/chat-content-search.md §3 (sync) and §7a (stage 1 design).
//!
//! **The writer is already single**, which is what makes this small: the
//! orchestrator is the sole writer of chats (architecture §1), so everything
//! that happens inside the running app has an exact hook — index the chat right
//! after it is saved ([`Orchestrator::index_saved_chat`], called from
//! `flush_saves`). The 800 ms save debounce already coalesces a burst of
//! streaming updates into one write, so it coalesces indexing too, and the
//! message-level diff inside [`CacheDb::index_chat`] reduces that write to a
//! single row.
//!
//! What is left is everything that happens **outside** the app — `mindfork
//! import`, `restore`, a hand-edited file, data synced from another machine, or
//! simply a `cache.db` that was deleted. Those are covered by
//! [`spawn_reconcile`] at startup: a stat-only walk of `chats/` costs ~0.3 ms on
//! the real corpus, so it runs unconditionally on every launch.
//!
//! Everything here is **best effort**. The index is derived data: a failure is
//! logged and the app carries on — a search that misses a chat is a nuisance, a
//! save that fails because of the index would be a bug.

use std::collections::HashMap;
use std::sync::Arc;

use uuid::Uuid;

use crate::app::events::AppEvent;
use crate::entities::chat::Chat;
use crate::entities::message::MessageRole;
use crate::features::chat_search::{self, SearchGroup, SearchHit};
use crate::features::chat_search_sort::SortMode;
use crate::shared::storage::Storage;
use crate::shared::storage::cache::{IndexedMessage, MessageHit};

use super::Orchestrator;

impl Orchestrator {
    /// Answers a content query from the chat list (`Ctrl+F`).
    ///
    /// Escaping happens **here**, not in the widget and not in `CacheDb`:
    /// `shared/storage` may not depend on `features` (FSD, research §7a), and
    /// keeping the rule in one place is what guarantees raw input never reaches
    /// `MATCH` — where ordinary text like `C++` or `cost-benefit` is a syntax
    /// error (research §4).
    ///
    /// A query that cannot search — nothing survived trigram's 3-character
    /// floor, or the search itself failed — answers `None`, i.e. "do not
    /// filter". A failure is logged, never raised as an `AppEvent::Error`: a
    /// half-typed query is not an error the user should be shown a popup about.
    pub(super) fn handle_search_chats(&self, query: String) {
        let chat_ids = match crate::features::chat_search::to_fts_query(&query) {
            None => None,
            Some(fts) => match self.storage.cache().search_chats(&fts) {
                Ok(ids) => Some(ids),
                Err(err) => {
                    tracing::warn!(query = %query, error = %format!("{err:#}"),
                        "chat content search failed");
                    None
                }
            },
        };
        let _ = self
            .evt_tx
            .send(AppEvent::ChatSearchResults { query, chat_ids });
    }

    /// Answers a message-level content query (`Ctrl+G` in the chat list).
    ///
    /// Escaping happens here for the same reason as in
    /// [`Self::handle_search_chats`], and an unsearchable query answers with no
    /// groups — the screen shows its empty state rather than an error popup.
    ///
    /// Grouping is done **here** and not in `CacheDb` because it needs what the
    /// orchestrator owns and the index does not: chat titles, the chat list's
    /// order, and the real position of a message inside its chat. See
    /// docs/history/chat-search-stage2.md §4 (fork S2).
    pub(super) fn handle_search_messages(&self, query: String, sort: SortMode) {
        let (groups, total) = self.message_search(&query, sort);
        let _ = self.evt_tx.send(AppEvent::MessageSearchResults {
            query,
            groups,
            total,
        });
    }

    /// The search itself (split out so it is testable without the loop).
    fn message_search(&self, query: &str, sort: SortMode) -> (Vec<SearchGroup>, usize) {
        let Some(fts) = chat_search::to_fts_query(query) else {
            return (Vec::new(), 0);
        };
        let cache = self.storage.cache();
        let hits = match cache.search_messages(&fts, chat_search::HIT_CAP) {
            Ok(hits) => hits,
            Err(err) => {
                tracing::warn!(query = %query, error = %format!("{err:#}"),
                    "message content search failed");
                return (Vec::new(), 0);
            }
        };
        // Only pay for the count when the cap actually bit — otherwise the
        // rows we have *are* the total.
        let total = if hits.len() < chat_search::HIT_CAP {
            hits.len()
        } else {
            cache.count_matching_messages(&fts).unwrap_or(hits.len())
        };
        (self.group_hits(hits, query, sort), total)
    }

    /// Buckets hits into chats **in the order the chat list is currently showing
    /// them** (fork S2 — the list's `Tab` toggle carries over, rather than the
    /// results quietly using a different order), and orders each chat's hits by their real
    /// position in the conversation.
    ///
    /// A hit whose chat we do not have — deleted or hidden since it was indexed
    /// — is dropped: the index is derived data and may lag by a moment, and
    /// showing a result that cannot be opened is worse than showing one fewer.
    fn group_hits(&self, hits: Vec<MessageHit>, query: &str, sort: SortMode) -> Vec<SearchGroup> {
        let mut by_chat: HashMap<Uuid, Vec<MessageHit>> = HashMap::new();
        for hit in hits {
            by_chat.entry(hit.chat_id).or_default().push(hit);
        }

        let mut chats: Vec<&Chat> = self
            .chats
            .iter()
            .filter(|c| !c.is_hidden && by_chat.contains_key(&c.id))
            .collect();
        chats.sort_by_key(|c| {
            std::cmp::Reverse(match sort {
                SortMode::Created => c.created_at,
                SortMode::Modified => c.modified_at,
            })
        });

        chats
            .into_iter()
            .map(|chat| {
                let order = message_order(chat);
                let mut hits = by_chat.remove(&chat.id).unwrap_or_default();
                hits.sort_by_key(|h| order.get(&h.message_id).copied().unwrap_or(usize::MAX));
                SearchGroup {
                    chat_id: chat.id,
                    title: chat.title.clone(),
                    hits: hits
                        .into_iter()
                        .map(|h| SearchHit {
                            message_id: h.message_id,
                            role: h.role,
                            ts: h.ts,
                            snippet: chat_search::build_snippet(
                                &h.text,
                                query,
                                chat_search::SNIPPET_BUDGET_CHARS,
                            ),
                        })
                        .collect(),
                }
            })
            .collect()
    }

    /// The earliest message of `chat` matching `query`, in real chat order —
    /// what `Enter` in the chat list's content mode opens the chat at. `None`
    /// when the query is unsearchable, the search fails, or nothing in this
    /// chat matches (then the chat opens at its tail, as a plain switch does).
    pub(super) fn first_match_in_chat(&self, chat_id: Uuid, query: &str) -> Option<Uuid> {
        let fts = chat_search::to_fts_query(query)?;
        let ids = self
            .storage
            .cache()
            .matching_messages_in_chat(&fts, chat_id)
            .inspect_err(|err| {
                tracing::warn!(chat = %chat_id, error = %format!("{err:#}"),
                    "resolving the first match in a chat failed");
            })
            .ok()?;
        let chat = self.chats.iter().find(|c| c.id == chat_id)?;
        let order = message_order(chat);
        ids.into_iter()
            .filter_map(|id| order.get(&id).map(|pos| (*pos, id)))
            .min()
            .map(|(_, id)| id)
    }

    /// Brings one chat's index in line right after it was written to disk.
    ///
    /// Stats the file we have just written so the index records the same
    /// `(mtime_ms, size)` the next startup reconciliation will compare against —
    /// otherwise every launch would re-parse every chat.
    pub(super) fn index_saved_chat(&self, chat: &Chat) {
        if chat.is_hidden {
            self.forget_chat_index(chat.id);
            return;
        }
        let Some(info) = self.storage.json().chat_file_info(chat.id) else {
            tracing::warn!(chat = %chat.id, "could not stat a just-saved chat file for the search index");
            return;
        };
        let messages = indexed_messages(chat);
        if let Err(err) =
            self.storage
                .cache()
                .index_chat(chat.id, info.mtime_ms, info.size, &messages)
        {
            tracing::warn!(chat = %chat.id, error = %format!("{err:#}"),
                "failed to update the chat search index");
        }
    }

    /// Drops a chat from the index (it was hidden or deleted).
    pub(super) fn forget_chat_index(&self, chat_id: Uuid) {
        if let Err(err) = self.storage.cache().forget_chat(chat_id) {
            tracing::warn!(chat = %chat_id, error = %format!("{err:#}"),
                "failed to drop a chat from the search index");
        }
    }
}

/// The messages of a chat as the index stores them.
///
/// Stage 1 indexes `message.text` only (fork F3): `thoughts` and tool-call
/// JSON would inflate the index and match on words the user never wrote.
/// Messages with nothing to index are skipped — a whitespace-only message
/// (a cancelled stream, a tool turn) carries no searchable content.
fn indexed_messages(chat: &Chat) -> Vec<IndexedMessage> {
    chat.messages
        .iter()
        .filter(|m| !m.text.trim().is_empty())
        .map(|m| IndexedMessage {
            id: m.id,
            role: role_str(m.role).to_string(),
            ts: m.timestamp.to_rfc3339(),
            text: m.text.clone(),
        })
        .collect()
}

/// `message_id → position in the conversation`, the only authority on the order
/// hits are shown in: the index's rowids only approximate it, since a message
/// whose text changed is deleted and re-inserted with a fresh one.
fn message_order(chat: &Chat) -> HashMap<Uuid, usize> {
    chat.messages
        .iter()
        .enumerate()
        .map(|(i, m)| (m.id, i))
        .collect()
}

/// The role as the index stores it. Unused by stage 1's chat-list filter;
/// stage 2's message-level screen shows it (see [`IndexedMessage`]).
fn role_str(role: MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

/// Reconciles the index against `chats/` in the background (research §3).
///
/// Runs in `spawn_blocking`: this is synchronous file + SQLite I/O and must not
/// occupy the async runtime. Writes **per chat** (one transaction each), so
/// search stays usable while the pass runs — results are simply "everything
/// indexed so far", and a first run on an empty index takes ~350 ms.
///
/// One chat's failure is logged and skipped, never aborting the pass: a single
/// corrupt file must not cost the index of all the others.
pub(super) fn spawn_reconcile(storage: Arc<Storage>) {
    tokio::task::spawn_blocking(move || reconcile(&storage));
}

/// The reconciliation pass itself (sync — see [`spawn_reconcile`]). Returns
/// `(indexed, forgotten, unchanged)` for the summary log and for tests.
fn reconcile(storage: &Storage) -> (usize, usize, usize) {
    // Bookkeeping **first**, then the directory. The order matters for the
    // "indexed but the file is gone" pass below: a chat created between the two
    // reads must not look like one that vanished. Read this way it appears in
    // `files` but not in `indexed` (harmless — the guarded write below skips it,
    // the app having already indexed it); read the other way round it would
    // appear in `indexed` but not in `files`, and be forgotten.
    let indexed = match storage.cache().indexed_state() {
        Ok(state) => state,
        Err(err) => {
            tracing::warn!(error = %format!("{err:#}"), "search index: cannot read its bookkeeping");
            return (0, 0, 0);
        }
    };
    let files = match storage.json().chat_files() {
        Ok(files) => files,
        Err(err) => {
            tracing::warn!(error = %format!("{err:#}"), "search index: cannot list chat files");
            return (0, 0, 0);
        }
    };

    let (mut reindexed, mut forgotten, mut unchanged) = (0, 0, 0);
    for file in &files {
        let was = indexed.get(&file.id).copied();
        // Unchanged since we indexed it — the whole point of the stat walk.
        if was == Some((file.mtime_ms, file.size)) {
            unchanged += 1;
            continue;
        }
        match storage.json().load_chat(file.id) {
            // A hidden chat is dropped rather than indexed: the chat list never
            // shows it, so neither should search. But it is recorded with *no
            // messages* instead of being forgotten — `forget_chat` also drops the
            // bookkeeping, so the next pass would find no record, parse the file
            // again, and drop it again, on every startup forever. Soft delete is
            // the only delete here (spec §12.3), so that set only grows: measured
            // on the real corpus, 43 of 171 chats were re-parsed every pass.
            // Indexing an empty message set removes any rows it already had and
            // records the file state, so the next pass skips it on the stat alone.
            Ok(Some(chat)) if chat.is_hidden => match write_indexed(storage, file, was, &[]) {
                Ok(true) => forgotten += 1,
                Ok(false) => unchanged += 1,
                Err(err) => tracing::warn!(chat = %file.id, error = %format!("{err:#}"),
                        "search index: failed to drop a hidden chat"),
            },
            Ok(Some(chat)) => match write_indexed(storage, file, was, &indexed_messages(&chat)) {
                Ok(true) => reindexed += 1,
                Ok(false) => {
                    unchanged += 1;
                    tracing::debug!(chat = %file.id,
                        "search index: a fresher write won, leaving this chat alone");
                }
                Err(err) => tracing::warn!(chat = %file.id, error = %format!("{err:#}"),
                    "search index: failed to index a chat"),
            },
            // The file vanished between the walk and the read — the "file gone"
            // branch below will pick it up on the next run.
            Ok(None) => {}
            Err(err) => tracing::warn!(chat = %file.id, error = %format!("{err:#}"),
                "search index: skipped an unreadable chat file"),
        }
    }

    // Indexed, but the file is gone (deleted outside the app, or a restore that
    // rolled the data back).
    for id in indexed.keys() {
        if !files.iter().any(|f| f.id == *id) && forget(storage, *id) {
            forgotten += 1;
        }
    }

    tracing::info!(
        indexed = reindexed,
        forgotten,
        unchanged,
        "chat search index reconciled"
    );
    (reindexed, forgotten, unchanged)
}

/// The reconciliation's write step, factored out because the guard on it is the
/// whole correctness argument of the pass.
///
/// `was` is the bookkeeping the pass saw **before** it read the file. Between
/// that read and this write the app may have saved and indexed the very same
/// chat, and our older snapshot must not win — so this is deliberately
/// [`CacheDb::index_chat_if_unchanged`] and never a plain
/// [`CacheDb::index_chat`]. Returns whether the write happened.
fn write_indexed(
    storage: &Storage,
    file: &crate::shared::storage::json::ChatFileInfo,
    was: Option<(i64, u64)>,
    messages: &[IndexedMessage],
) -> anyhow::Result<bool> {
    storage
        .cache()
        .index_chat_if_unchanged(file.id, was, file.mtime_ms, file.size, messages)
}

/// `forget_chat` with the failure logged; `true` if it succeeded (so the caller
/// counts only what actually left the index).
fn forget(storage: &Storage, id: Uuid) -> bool {
    match storage.cache().forget_chat(id) {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!(chat = %id, error = %format!("{err:#}"),
                "search index: failed to forget a chat");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::message::Message;
    use crate::entities::profile::Profile;
    use crate::shared::paths::Paths;

    fn storage() -> (tempfile::TempDir, Storage) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(Paths::with_root(dir.path())).unwrap();
        (dir, storage)
    }

    fn chat_with(texts: &[&str]) -> Chat {
        let profile = Profile::new("P", "sys");
        let mut chat = Chat::from_profile(&profile, "заголовок");
        for t in texts {
            chat.push_message(Message::user(*t));
        }
        chat
    }

    #[test]
    fn reconcile_indexes_new_files_and_skips_unchanged_ones() {
        // The property the design rests on: a second pass over untouched files
        // re-indexes nothing, so startup costs a stat walk rather than a parse.
        let (_d, storage) = storage();
        let chat = chat_with(&["содержимое про кошек"]);
        storage.json().save_chat(&chat).unwrap();

        let (indexed, forgotten, unchanged) = reconcile(&storage);
        assert_eq!((indexed, forgotten, unchanged), (1, 0, 0));
        assert_eq!(
            storage.cache().search_chats("\"кош\"").unwrap(),
            vec![chat.id]
        );

        let (indexed, forgotten, unchanged) = reconcile(&storage);
        assert_eq!(
            (indexed, forgotten, unchanged),
            (0, 0, 1),
            "an unchanged file must not be re-parsed"
        );
    }

    #[test]
    fn reconcile_reindexes_a_changed_file_and_forgets_a_deleted_one() {
        let (_d, storage) = storage();
        let mut chat = chat_with(&["первая версия текста"]);
        storage.json().save_chat(&chat).unwrap();
        reconcile(&storage);

        // Edited outside the app (import/restore/sync — the case the startup
        // pass exists for).
        chat.messages.clear();
        chat.push_message(Message::user("вторая версия текста"));
        storage.json().save_chat(&chat).unwrap();
        let (indexed, ..) = reconcile(&storage);
        assert_eq!(indexed, 1);
        assert_eq!(
            storage.cache().search_chats("\"вторая\"").unwrap(),
            vec![chat.id]
        );
        assert!(
            storage
                .cache()
                .search_chats("\"первая\"")
                .unwrap()
                .is_empty()
        );

        // The file is gone — so is its index entry.
        std::fs::remove_file(_d.path().join("chats").join(format!("{}.json", chat.id))).unwrap();
        let (_, forgotten, _) = reconcile(&storage);
        assert_eq!(forgotten, 1);
        assert!(
            storage
                .cache()
                .search_chats("\"вторая\"")
                .unwrap()
                .is_empty()
        );
        assert!(storage.cache().indexed_state().unwrap().is_empty());
    }

    #[test]
    fn reconcile_drops_a_hidden_chat() {
        let (_d, storage) = storage();
        let chat = chat_with(&["секретное содержимое"]);
        storage.json().save_chat(&chat).unwrap();
        reconcile(&storage);
        assert!(
            !storage
                .cache()
                .search_chats("\"секрет\"")
                .unwrap()
                .is_empty()
        );

        storage.json().hide_chat(chat.id).unwrap();
        let (_, forgotten, _) = reconcile(&storage);
        assert_eq!(forgotten, 1);
        assert!(
            storage
                .cache()
                .search_chats("\"секрет\"")
                .unwrap()
                .is_empty(),
            "a hidden chat must not show up in results"
        );
    }

    /// A hidden chat must be *recorded* as processed, not forgotten — otherwise
    /// every later pass finds no bookkeeping for it, parses the file again and
    /// drops it again. Soft delete is the only delete (spec §12.3), so that set
    /// only grows; the real corpus had 43 of 171 chats re-parsed on every single
    /// startup before this. Synthetic single-pass tests cannot see it, so this
    /// asserts on the *second* pass.
    #[test]
    fn a_hidden_chat_is_not_re_examined_on_every_pass() {
        let (_d, storage) = storage();
        let chat = chat_with(&["секретное содержимое"]);
        storage.json().save_chat(&chat).unwrap();
        reconcile(&storage);
        storage.json().hide_chat(chat.id).unwrap();
        assert_eq!(reconcile(&storage).1, 1, "the pass that drops it");

        // The pass after that must skip it on the stat alone.
        let (indexed, forgotten, unchanged) = reconcile(&storage);
        assert_eq!(
            (indexed, forgotten, unchanged),
            (0, 0, 1),
            "a hidden chat must count as unchanged, not be dropped again"
        );
        assert!(
            storage
                .cache()
                .indexed_state()
                .unwrap()
                .contains_key(&chat.id),
            "and it must stay in the bookkeeping — that is what stops the re-parse"
        );
    }

    #[test]
    fn reconcile_survives_a_corrupt_chat_file() {
        // One broken JSON file must not cost the index of all the others.
        let (_d, storage) = storage();
        let good = chat_with(&["исправное содержимое"]);
        storage.json().save_chat(&good).unwrap();
        let broken = Uuid::new_v4();
        std::fs::write(
            _d.path().join("chats").join(format!("{broken}.json")),
            "{ не json",
        )
        .unwrap();

        let (indexed, ..) = reconcile(&storage);
        assert_eq!(indexed, 1, "the healthy chat is still indexed");
        assert_eq!(
            storage.cache().search_chats("\"исправ\"").unwrap(),
            vec![good.id]
        );
    }

    #[test]
    fn the_pass_does_not_clobber_a_write_that_landed_while_it_read() {
        // The observed bug, in the order it actually happened: the pass stats
        // and reads a chat while it is still empty; the app then saves the real
        // conversation and indexes it; the pass only gets round to writing
        // afterwards. Its stale, empty snapshot must not win — unguarded it
        // does, and the chat disappears from search until the next launch.
        //
        // Driven through `write_indexed` rather than `reconcile`, because the
        // two halves have to be interleaved and a single synchronous pass
        // cannot be: `reconcile` reads its bookkeeping and writes in one go.
        let (_d, storage) = storage();
        let mut chat = chat_with(&[]);
        storage.json().save_chat(&chat).unwrap();

        // What the pass saw when it looked: nothing indexed, an empty file.
        let seen_by_the_pass = storage
            .cache()
            .indexed_state()
            .unwrap()
            .get(&chat.id)
            .copied();
        let file_as_read = storage.json().chat_file_info(chat.id).unwrap();
        let messages_as_read = indexed_messages(&chat);
        assert!(messages_as_read.is_empty());

        // Meanwhile the app saves the real conversation and indexes it.
        chat.push_message(Message::user("живая переписка"));
        storage.json().save_chat(&chat).unwrap();
        let now = storage.json().chat_file_info(chat.id).unwrap();
        storage
            .cache()
            .index_chat(chat.id, now.mtime_ms, now.size, &indexed_messages(&chat))
            .unwrap();

        // Only now does the pass write.
        let wrote =
            write_indexed(&storage, &file_as_read, seen_by_the_pass, &messages_as_read).unwrap();
        assert!(!wrote, "the stale snapshot must be refused");
        assert_eq!(
            storage.cache().search_chats("\"живая\"").unwrap(),
            vec![chat.id],
            "the live index survived the reconciliation"
        );
    }

    #[test]
    fn the_pass_writes_when_nothing_moved_underneath_it() {
        // The other half — the guard must not turn the pass into a no-op.
        let (_d, storage) = storage();
        let chat = chat_with(&["содержимое для индексации"]);
        storage.json().save_chat(&chat).unwrap();

        let seen = storage
            .cache()
            .indexed_state()
            .unwrap()
            .get(&chat.id)
            .copied();
        let file = storage.json().chat_file_info(chat.id).unwrap();
        let wrote = write_indexed(&storage, &file, seen, &indexed_messages(&chat)).unwrap();
        assert!(wrote);
        assert_eq!(
            storage.cache().search_chats("\"индексац\"").unwrap(),
            vec![chat.id]
        );
    }

    #[test]
    fn only_message_text_is_indexed() {
        // Fork F3: `thoughts` and tool-call JSON stay out of the index — they
        // would match on words the user never wrote.
        let profile = Profile::new("P", "sys");
        let mut chat = Chat::from_profile(&profile, "t");
        let mut msg = Message::assistant("видимый ответ");
        msg.thoughts = Some("скрытые рассуждения".into());
        chat.push_message(msg);
        chat.push_message(Message::assistant("   "));

        let indexed = indexed_messages(&chat);
        assert_eq!(indexed.len(), 1, "a blank message carries nothing to index");
        assert_eq!(indexed[0].text, "видимый ответ");
        assert_eq!(indexed[0].role, "assistant");
        assert!(!indexed[0].ts.is_empty());
    }
}
