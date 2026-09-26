//! The index board: which chat attachments are having their search index built
//! right now, how far along each is, and the feed note each one's end still owes
//! (docs/research/attachment-birth-turn.md §5, forks G1–G3).
//!
//! An attachment reaches the "start indexing" point up to three times: the loop
//! that produced it (at its round's end), a parent loop mirroring a sub-agent's
//! attachment, and the orchestrator landing the turn. The board is the one owner —
//! [`IndexBoard::begin`] is claimed once, whoever comes first. It is also what
//! `attachment_search` reads to wait for a file whose index is still being built,
//! and where the end-of-index note waits for its attachment to land, so a note
//! never lands in the middle of a streaming reply and splits it (G3).
//!
//! Shared as an `Arc` between the orchestrator and every turn's `ToolContext`; the
//! state is a map behind a `std` mutex (never held across an `.await`) plus a
//! `watch` counter that ticks on every change, which is what waiters sleep on.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::features::file_command::FileProgress;

/// One attachment's indexing, from the claim to the landing.
#[derive(Debug)]
struct Entry {
    /// `(done, total)` fragments while building; `None` once the task ended.
    running: Option<(usize, usize)>,
    /// The attachment is in its chat: a note the end produces goes out at once.
    landed: bool,
    /// The end's note, kept until the attachment lands.
    held: Option<FileProgress>,
}

/// What the landing finds for an attachment ([`IndexBoard::land`]).
#[derive(Debug, PartialEq)]
pub enum Landing {
    /// Nobody started its index — the landing does.
    Unknown,
    /// Its index is being built; the task will say when it ends.
    Running,
    /// Its index already ended; the note it held, if it had one, is the landing's
    /// to emit — after the attachment's own "attached" note.
    Ended(Option<FileProgress>),
}

/// See the module doc.
pub struct IndexBoard {
    entries: Mutex<HashMap<Uuid, Entry>>,
    tick: watch::Sender<u64>,
}

impl Default for IndexBoard {
    fn default() -> Self {
        Self::new()
    }
}

impl IndexBoard {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            tick: watch::channel(0).0,
        }
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, Entry>> {
        self.entries.lock().expect("index board poisoned")
    }

    /// Wakes every waiter. `send_modify` works with no receiver alive, which is
    /// the usual state: nothing is waiting most of the time.
    fn bump(&self) {
        self.tick.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// Claims `id` for indexing: `true` when this caller is the first and must
    /// start the task, `false` when someone already did. `landed` — the
    /// attachment is already in its chat (`/file attach`), so the end's note needs
    /// no holding.
    pub fn begin(&self, id: Uuid, landed: bool) -> bool {
        let mut entries = self.entries();
        if entries.contains_key(&id) {
            return false;
        }
        entries.insert(
            id,
            Entry {
                running: Some((0, 0)),
                landed,
                held: None,
            },
        );
        drop(entries);
        self.bump();
        true
    }

    /// The task's progress, in fragments.
    pub fn progress(&self, id: Uuid, done: usize, total: usize) {
        if let Some(e) = self.entries().get_mut(&id)
            && e.running.is_some()
        {
            e.running = Some((done, total));
        }
        self.bump();
    }

    /// The task ended. Returns the note to emit now when the attachment has
    /// landed (and forgets it), or keeps the note for the landing and returns
    /// `None`. `note` is `None` when the end has nothing to say.
    pub fn finish(&self, id: Uuid, note: Option<FileProgress>) -> Option<FileProgress> {
        let mut entries = self.entries();
        let out = match entries.get_mut(&id) {
            Some(e) if e.landed => {
                entries.remove(&id);
                note
            }
            Some(e) => {
                e.running = None;
                e.held = note;
                None
            }
            // Never claimed — a caller outside the board; say it now.
            None => note,
        };
        drop(entries);
        self.bump();
        out
    }

    /// The attachment landed in its chat. See [`Landing`].
    pub fn land(&self, id: Uuid) -> Landing {
        let mut entries = self.entries();
        match entries.get_mut(&id) {
            None => Landing::Unknown,
            Some(e) if e.running.is_some() => {
                e.landed = true;
                Landing::Running
            }
            Some(_) => Landing::Ended(entries.remove(&id).and_then(|e| e.held)),
        }
    }

    /// `(done, total)` while `id`'s index is being built; `None` otherwise
    /// (never started, or ended — the database then says whether it has rows).
    pub fn building(&self, id: Uuid) -> Option<(usize, usize)> {
        self.entries().get(&id).and_then(|e| e.running)
    }

    /// Waits until none of `ids` is being built, for at most `within`, or until
    /// `cancel` fires. `true` when they all ended; `false` on the bound or the
    /// cancellation — the caller then says how far each one got.
    pub async fn wait(&self, ids: &[Uuid], within: Duration, cancel: &CancellationToken) -> bool {
        let mut rx = self.tick.subscribe();
        let deadline = tokio::time::Instant::now() + within;
        loop {
            // Marked seen before the check, so a change between the check and
            // the `await` still wakes this waiter.
            rx.borrow_and_update();
            if ids.iter().all(|id| self.building(*id).is_none()) {
                return true;
            }
            tokio::select! {
                changed = rx.changed() => {
                    if changed.is_err() {
                        return ids.iter().all(|id| self.building(*id).is_none());
                    }
                }
                _ = tokio::time::sleep_until(deadline) => return false,
                _ = cancel.cancelled() => return false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn indexed(name: &str) -> FileProgress {
        FileProgress::Indexed {
            name: name.into(),
            chunks: 3,
        }
    }

    #[test]
    fn an_index_is_started_once_whoever_comes_first() {
        let board = IndexBoard::new();
        let id = Uuid::new_v4();
        assert!(board.begin(id, false), "the first claim starts the task");
        assert!(
            !board.begin(id, false),
            "the parent's mirror must not start it again"
        );
        // Still claimed after the task ended, until the landing collects it —
        // a late mirror of the same attachment must not index it twice.
        board.finish(id, Some(indexed("a")));
        assert!(!board.begin(id, true));
    }

    /// G3: the end of an index started inside a turn keeps its note until the
    /// attachment lands, so the note follows "attached" and never splits a reply.
    #[test]
    fn a_note_waits_for_its_attachment_to_land() {
        let board = IndexBoard::new();
        let id = Uuid::new_v4();
        board.begin(id, false);
        board.progress(id, 2, 3);
        assert_eq!(board.building(id), Some((2, 3)));
        assert_eq!(
            board.finish(id, Some(indexed("a"))),
            None,
            "held, not emitted"
        );
        assert_eq!(board.building(id), None);
        assert_eq!(board.land(id), Landing::Ended(Some(indexed("a"))));
        // Collected: the board forgets it.
        assert_eq!(board.land(id), Landing::Unknown);
    }

    /// An index still running when its turn lands says so itself when it ends.
    #[test]
    fn a_late_end_speaks_for_itself() {
        let board = IndexBoard::new();
        let id = Uuid::new_v4();
        board.begin(id, false);
        assert_eq!(board.land(id), Landing::Running);
        assert_eq!(board.finish(id, Some(indexed("a"))), Some(indexed("a")));
        assert_eq!(board.land(id), Landing::Unknown, "forgotten once said");
    }

    /// `/file attach`: the file is in its chat before the index starts.
    #[test]
    fn a_landed_attachment_is_never_held() {
        let board = IndexBoard::new();
        let id = Uuid::new_v4();
        board.begin(id, true);
        assert_eq!(board.finish(id, Some(indexed("a"))), Some(indexed("a")));
        assert_eq!(board.finish(id, None), None);
    }

    #[tokio::test]
    async fn a_waiter_wakes_when_the_index_ends() {
        let board = Arc::new(IndexBoard::new());
        let id = Uuid::new_v4();
        board.begin(id, false);
        let b = board.clone();
        let ender = tokio::spawn(async move {
            b.progress(id, 1, 2);
            tokio::task::yield_now().await;
            b.finish(id, None);
        });
        let ended = board
            .wait(&[id], Duration::from_secs(30), &CancellationToken::new())
            .await;
        ender.await.unwrap();
        assert!(ended);
    }

    /// The bound and the turn's cancellation both end the wait with `false`, and the
    /// progress is still there to report. Paused time: the bound elapses at once.
    #[tokio::test(start_paused = true)]
    async fn the_wait_is_bounded_and_cancellable() {
        let board = IndexBoard::new();
        let id = Uuid::new_v4();
        board.begin(id, false);
        board.progress(id, 5, 10);
        let cancel = CancellationToken::new();
        assert!(!board.wait(&[id], Duration::from_secs(120), &cancel).await);
        assert_eq!(board.building(id), Some((5, 10)));
        cancel.cancel();
        assert!(!board.wait(&[id], Duration::from_secs(3600), &cancel).await);
        // Nothing to wait for is an immediate yes.
        assert!(
            board
                .wait(&[Uuid::new_v4()], Duration::ZERO, &CancellationToken::new())
                .await
        );
    }
}
