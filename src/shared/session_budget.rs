//! The turn's session budget — how many request streams may be open at once,
//! and how much of the engine's one KV pool they may occupy together
//! (spec §6.3; docs/research/parallel-subagents.md §4.2 for the count,
//! docs/research/admission-by-budget.md §4 for the tokens).
//!
//! A permit is a *count*: with `sessions = 2` two streams may be open. Under
//! llama.cpp's unified pool (`-np N --kv-unified`, the shape a managed server
//! above one session is launched in) two open streams also share one context
//! pool, and when they outgrow it *together* the server ends every processing
//! slot at once — "Context size has been exceeded." to both, measured in the
//! research's §3.2–§3.3. So next to the permit a stream **reserves** what it
//! will occupy: its prompt (a calibrated estimate, floored by the exact size
//! the server last reported for that loop) plus its reply cap. A stream whose
//! reservation does not fit next to the ones already open **waits** — with its
//! permit, cancellably — until one of them ends; a stream that is alone is
//! admitted whatever its size (the server, not the app, decides whether a
//! single conversation fits its window, exactly as before this type existed),
//! which is also what makes the wait always end.
//!
//! With no pool known (a cloud, an external server that answers no `/props`
//! or reports one slot) the type is the bare semaphore it replaced, bit for
//! bit: the count bounds, nothing is priced, nothing waits for room.
//!
//! Beside the interactive lane — the turns, the runs, the scenes, a tool's
//! summary — sits the **silent lane** (docs/research/silent-tasks-budget.md
//! §4.1): one permit for the app's own background requests (the title, the
//! silent loops, the compaction roll, impersonation on the shared engine)
//! over the *same* pool sum. So the silent tasks take turns among
//! themselves, and a silent stream never overfills the pool beside an
//! interactive one. The lane records the label of the request it is
//! streaming, so the tasks screen can say which task waits.
//!
//! The reverse wait — an interactive stream that does not fit beside an open
//! silent one — ends by **displacement** (docs/research/silent-preemption.md
//! §4): a silent reservation carries a child token its holder streams on, and
//! an interactive waiter that would fit once that reservation is gone cancels
//! it, counts itself as *displacing* — the silent lane takes no room while
//! such a waiter is pending, so the displaced task's retry cannot slip back in
//! ahead of it — and is admitted when the reservation drops. The holder reads
//! the displacement off its reservation and makes the same request again;
//! after [`SILENT_YIELDS_MAX`] displacements it asks for a reservation that
//! does not yield, and the interactive waiter waits that one round out, as it
//! waited every round before this existed. Impersonation and a summary inside
//! a silent loop never yield. Without a pool there is no reservation and no
//! displacement.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use tokio::sync::{Notify, Semaphore, SemaphorePermit};
use tokio_util::sync::CancellationToken;

/// How many times one silent task yields its stream to an interactive waiter
/// before it holds (docs/research/silent-preemption.md §4.4, fork F5): a
/// bound on the rounds wasted and on how long a long agentic turn can keep a
/// task from finishing — its fourth attempt streams to its end, and the turn
/// waits that one round.
pub const SILENT_YIELDS_MAX: u32 = 3;

/// The silent stream open right now, as the interactive lane sees it.
struct SilentOpen {
    label: &'static str,
    /// The child token the holder streams on — what a displacement cancels.
    token: CancellationToken,
    /// Its reservation, in tokens (`0` with no pool).
    tokens: u64,
    /// Whether it yields to an interactive waiter (research §4.3).
    yields: bool,
}

/// The budget: the permit count and, when a pool is known, the token sum.
pub struct SessionBudget {
    permits: Semaphore,
    /// The silent lane's one permit (research §4.1, fork F5).
    silent_permits: Semaphore,
    /// The silent request streaming right now (`None` — the lane is idle, or
    /// its holder is still waiting for room). A `std` mutex, never held
    /// across an await.
    silent_open: Mutex<Option<SilentOpen>>,
    /// The KV pool the open streams share, in tokens. `None`: no guard.
    pool: Option<u64>,
    /// The sum of the reservations of the streams currently open. A `std`
    /// mutex, never held across an await.
    in_flight: Mutex<u64>,
    /// Woken (all waiters) whenever a reservation is released, and whenever a
    /// displacing waiter stops being one.
    room: Notify,
    /// The estimator's correction (research §4.3), one per [`Shape`]: the
    /// latest ratio of an exact `usage.prompt_tokens` to the estimate of the
    /// same request of that kind, as `f64` bits; `0` — none recorded yet,
    /// read as `1.0`.
    density: [AtomicU64; Shape::COUNT],
    /// Interactive waiters that displaced a silent stream and are not
    /// admitted yet (silent-preemption §4.2): while one is pending, the
    /// silent lane takes no room.
    displacing: AtomicUsize,
}

/// The kind of request a reservation is for — the population its
/// exact-to-estimate ratio is kept with (docs/research/title-impersonation-usage.md
/// §3.1). Measured, the populations differ by a third: a turn carrying a
/// tool result runs at 1.34, the title and impersonation at 0.6, and under
/// one ratio the second erased the first's correction. Each kind prices
/// with the latest ratio of its own kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// The turn's own rounds.
    Turn,
    /// A child run's rounds — a sub-agent's or a dialogue's, a persona's
    /// prompt and the turn's tools.
    Run,
    /// A silent loop's rounds — reflection, the consolidations.
    Loop,
    /// The compaction roll.
    Roll,
    /// The automatic title.
    Title,
    /// Impersonation on the shared engine.
    Impersonation,
    /// The page summary inside a tool — priced, never recorded.
    Summary,
}

impl Shape {
    /// How many kinds there are — the ratio array's size.
    pub const COUNT: usize = 7;
}

/// A stream's place in the budget: one permit and its token reservation,
/// both released when it is dropped — the sum first (so a waiter woken by it
/// finds the permit free too), then the permit.
pub struct Reservation<'a> {
    budget: &'a SessionBudget,
    tokens: u64,
    /// The silent lane's label this reservation holds, if it is a silent one.
    label: Option<&'static str>,
    /// The holder's own token.
    cancel: CancellationToken,
    /// A silent reservation's stream token: a child of `cancel`, cancelled by
    /// a displacement as well.
    child: Option<CancellationToken>,
    _permit: SemaphorePermit<'a>,
}

impl Reservation<'_> {
    /// The token to stream on: for a silent reservation the child a
    /// displacement cancels, otherwise the holder's own.
    pub fn stream_token(&self) -> CancellationToken {
        self.child.clone().unwrap_or_else(|| self.cancel.clone())
    }

    /// Whether the stream was displaced by an interactive waiter: the child
    /// fired while the holder's own token did not (silent-preemption §4.1).
    pub fn displaced(&self) -> bool {
        self.child.as_ref().is_some_and(|c| c.is_cancelled()) && !self.cancel.is_cancelled()
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if let Some(label) = self.label {
            let mut silent = self.budget.silent();
            if silent.as_ref().is_some_and(|s| s.label == label) {
                *silent = None;
            }
        }
        if self.tokens > 0 {
            let mut open = self.budget.open();
            *open = open.saturating_sub(self.tokens);
            drop(open);
            self.budget.room.notify_waiters();
        }
    }
}

/// An interactive waiter's mark while it displaces a silent stream
/// (silent-preemption §4.2): counted at the cancel, uncounted — with a
/// wake-up for the silent lane — when the waiter is admitted or gives up.
struct Displacing<'a>(&'a SessionBudget);

impl<'a> Displacing<'a> {
    fn new(budget: &'a SessionBudget) -> Self {
        budget.displacing.fetch_add(1, Ordering::SeqCst);
        Self(budget)
    }
}

impl Drop for Displacing<'_> {
    fn drop(&mut self) {
        self.0.displacing.fetch_sub(1, Ordering::SeqCst);
        self.0.room.notify_waiters();
    }
}

impl std::fmt::Debug for SessionBudget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionBudget")
            .field("sessions", &self.permits.available_permits())
            .field("pool", &self.pool)
            .field("in_flight", &*self.open())
            .field(
                "density",
                &self
                    .density
                    .iter()
                    .map(|d| match d.load(Ordering::Relaxed) {
                        0 => 1.0,
                        bits => f64::from_bits(bits).max(1.0),
                    })
                    .collect::<Vec<_>>(),
            )
            .field("displacing", &self.displacing.load(Ordering::SeqCst))
            .finish()
    }
}

impl SessionBudget {
    /// `sessions` permits (never below one — a turn must be able to stream)
    /// over a pool of `pool` tokens, or over no pool at all (`None` or `0`).
    pub fn new(sessions: u32, pool: Option<u64>) -> Self {
        Self {
            permits: Semaphore::new(sessions.max(1) as usize),
            silent_permits: Semaphore::new(1),
            silent_open: Mutex::new(None),
            pool: pool.filter(|&n| n > 0),
            in_flight: Mutex::new(0),
            room: Notify::new(),
            density: std::array::from_fn(|_| AtomicU64::new(0)),
            displacing: AtomicUsize::new(0),
        }
    }

    /// The pool the reservations are measured against, when one is known.
    #[cfg(test)]
    pub fn pool(&self) -> Option<u64> {
        self.pool
    }

    /// Permits not currently held.
    #[cfg(test)]
    pub fn available_sessions(&self) -> usize {
        self.permits.available_permits()
    }

    /// The sum of the open reservations.
    #[cfg(test)]
    pub fn in_flight(&self) -> u64 {
        *self.open()
    }

    /// Interactive waiters displacing a silent stream right now.
    #[cfg(test)]
    pub fn displacing(&self) -> usize {
        self.displacing.load(Ordering::SeqCst)
    }

    fn open(&self) -> std::sync::MutexGuard<'_, u64> {
        self.in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn silent(&self) -> std::sync::MutexGuard<'_, Option<SilentOpen>> {
        self.silent_open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The label of the silent request streaming right now, if any — what
    /// the tasks screen reads to tell a task that waits from the one that
    /// runs (spec §11.10). `None` while the lane's holder waits for room.
    pub fn silent_streaming(&self) -> Option<&'static str> {
        self.silent().as_ref().map(|s| s.label)
    }

    /// The estimator's correction factor for requests of `shape`: the latest
    /// exact-to-estimate ratio recorded for that kind, floored at `1.0` (an
    /// estimate that over-counts costs a wait that could have run; one that
    /// under-counts costs the failure the type exists to prevent — research
    /// R6). `1.0` until a request of the kind has reported.
    pub fn density(&self, shape: Shape) -> f64 {
        let bits = self.density[shape as usize].load(Ordering::Relaxed);
        if bits == 0 {
            return 1.0;
        }
        f64::from_bits(bits).max(1.0)
    }

    /// Records a request's exact prompt size next to the estimate made for
    /// it, under its kind. Any request of the kind may record; the latest of
    /// the kind wins — the tokenizer's density on that kind's text — and no
    /// kind touches another's (title-impersonation-usage §3.1).
    pub fn record_usage(&self, shape: Shape, estimate: u64, exact: u64) {
        if estimate == 0 || exact == 0 {
            return;
        }
        let ratio = (exact as f64 / estimate as f64).max(1.0);
        self.density[shape as usize].store(ratio.to_bits(), Ordering::Relaxed);
    }

    /// What a stream reserves (research §4.2): the calibrated `estimate` of its
    /// prompt, floored by `floor` (the loop's last exact size plus what it
    /// generated — a lower bound the server has vouched for; `0` for a first
    /// round), plus `reply_cap`. With no cap the reservation is the whole
    /// pool: the stream is admitted alone. Meaningless (and unused) without a
    /// pool.
    pub fn price(&self, shape: Shape, estimate: u64, floor: u64, reply_cap: Option<u64>) -> u64 {
        let prompt = ((estimate as f64 * self.density(shape)).round() as u64).max(floor);
        match (self.pool, reply_cap) {
            (Some(pool), None) => pool,
            (_, cap) => prompt.saturating_add(cap.unwrap_or(0)),
        }
    }

    /// Takes a permit and, under a pool, waits until `need` tokens fit next to
    /// the open reservations — or no stream is open at all. `None` when the
    /// turn is cancelled while waiting, for either. The reservation is the
    /// caller's to drop, which it does the moment its stream ends, so a
    /// round's tools never hold one. A silent stream in the way is displaced
    /// when its room would let this one in (silent-preemption §4.2).
    pub async fn acquire(&self, need: u64, cancel: &CancellationToken) -> Option<Reservation<'_>> {
        self.acquire_in(&self.permits, need, cancel, None, false)
            .await
    }

    /// The silent lane's [`Self::acquire`] (research §4.1): the lane's one
    /// permit — so the app's own background requests take turns among
    /// themselves — then the same wait for room under the pool. `label`
    /// names the request for [`Self::silent_streaming`]; `yields` says
    /// whether an interactive waiter may displace the stream
    /// (silent-preemption §4.3) — the holder then streams on
    /// [`Reservation::stream_token`] and reads [`Reservation::displaced`].
    pub async fn acquire_silent(
        &self,
        need: u64,
        cancel: &CancellationToken,
        label: &'static str,
        yields: bool,
    ) -> Option<Reservation<'_>> {
        self.acquire_in(&self.silent_permits, need, cancel, Some(label), yields)
            .await
    }

    async fn acquire_in<'a>(
        &'a self,
        permits: &'a Semaphore,
        need: u64,
        cancel: &CancellationToken,
        label: Option<&'static str>,
        yields: bool,
    ) -> Option<Reservation<'a>> {
        let permit = tokio::select! {
            biased;
            _ = cancel.cancelled() => return None,
            // A closed semaphore cannot happen here (nothing closes it), and
            // reads as "cancelled" rather than as a panic.
            permit = permits.acquire() => permit.ok()?,
        };
        let child = label.map(|_| cancel.child_token());
        let Some(pool) = self.pool else {
            self.admit_silent(label, child.as_ref(), 0, yields);
            return Some(Reservation {
                budget: self,
                tokens: 0,
                label,
                cancel: cancel.clone(),
                child,
                _permit: permit,
            });
        };
        let mut waited = false;
        let mut displacing: Option<Displacing<'_>> = None;
        loop {
            // Registered before the check, so a release between the check and
            // the wait is not missed.
            let notified = self.room.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut open = self.open();
                let fits = *open == 0 || open.saturating_add(need) <= pool;
                // A silent request behind a displacing waiter takes no room
                // until that waiter is in (silent-preemption §4.2).
                let deferred = label.is_some() && self.displacing.load(Ordering::SeqCst) > 0;
                if fits && !deferred {
                    *open += need;
                    drop(open);
                    self.admit_silent(label, child.as_ref(), need, yields);
                    // Uncounted after the reservation is in, so the displaced
                    // task's retry finds the room taken.
                    drop(displacing);
                    return Some(Reservation {
                        budget: self,
                        tokens: need,
                        label,
                        cancel: cancel.clone(),
                        child,
                        _permit: permit,
                    });
                }
                if label.is_none()
                    && !fits
                    && displacing.is_none()
                    && self.displace(*open, need, pool)
                {
                    displacing = Some(Displacing::new(self));
                }
                if !waited {
                    tracing::info!(
                        need,
                        in_flight = *open,
                        pool,
                        lane = label.unwrap_or("interactive"),
                        "stream waits for room in the KV pool"
                    );
                    waited = true;
                }
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return None,
                _ = &mut notified => {}
            }
        }
    }

    /// Records the silent stream admitted, for the tasks screen and for the
    /// interactive lane's displacement.
    fn admit_silent(
        &self,
        label: Option<&'static str>,
        child: Option<&CancellationToken>,
        tokens: u64,
        yields: bool,
    ) {
        if let (Some(label), Some(token)) = (label, child) {
            *self.silent() = Some(SilentOpen {
                label,
                token: token.clone(),
                tokens,
                yields,
            });
        }
    }

    /// Displaces the silent stream open (silent-preemption §4.2) when it
    /// yields and its room would let a waiter of `need` in beside the rest
    /// of `open`: its token is cancelled once, and the answer is whether
    /// the waiter should count itself as displacing — also when another
    /// waiter cancelled the same stream first.
    fn displace(&self, open: u64, need: u64, pool: u64) -> bool {
        let silent = self.silent();
        let Some(s) = silent.as_ref() else {
            return false;
        };
        if !s.yields || s.tokens == 0 {
            return false;
        }
        let without = open.saturating_sub(s.tokens);
        if without != 0 && without.saturating_add(need) > pool {
            return false;
        }
        if !s.token.is_cancelled() {
            s.token.cancel();
            tracing::info!(
                need,
                in_flight = open,
                pool,
                displaced = s.label,
                "an interactive stream displaces the silent one"
            );
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// A future that has not resolved within a short wait — "it is waiting".
    async fn pending<T>(fut: impl std::future::Future<Output = T>) -> bool {
        tokio::time::timeout(Duration::from_millis(60), fut)
            .await
            .is_err()
    }

    /// A permit is held while the stream runs and returned when it is dropped —
    /// the whole invariant a `sessions` budget of one rests on.
    #[tokio::test]
    async fn a_session_is_taken_for_the_stream_and_returned_after() {
        let budget = SessionBudget::new(1, None);
        let cancel = CancellationToken::new();
        let r = budget.acquire(10, &cancel).await;
        assert!(r.is_some());
        assert_eq!(budget.available_sessions(), 0);
        drop(r);
        assert_eq!(budget.available_sessions(), 1);
    }

    /// A loop waiting for a session stops waiting when its turn is cancelled:
    /// `Esc` must not hang on a sibling's stream.
    #[tokio::test]
    async fn cancellation_ends_the_wait_for_a_session() {
        let budget = SessionBudget::new(1, None);
        let held = budget.acquire(0, &CancellationToken::new()).await;
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(budget.acquire(0, &cancel).await.is_none());
        drop(held);
        assert!(budget.acquire(0, &CancellationToken::new()).await.is_some());
    }

    /// Two reservations that fit the pool together are both admitted.
    #[tokio::test]
    async fn reservations_that_fit_together_run_together() {
        let budget = SessionBudget::new(2, Some(1000));
        let cancel = CancellationToken::new();
        let a = budget.acquire(400, &cancel).await.unwrap();
        let b = budget.acquire(400, &cancel).await.unwrap();
        assert_eq!(budget.in_flight(), 800);
        drop(a);
        assert_eq!(budget.in_flight(), 400);
        drop(b);
        assert_eq!((budget.in_flight(), budget.available_sessions()), (0, 2));
    }

    /// A third that would not fit waits — with a permit free — and is admitted
    /// when one of the two ends.
    #[tokio::test]
    async fn a_reservation_that_does_not_fit_waits_for_room() {
        let budget = SessionBudget::new(3, Some(1000));
        let cancel = CancellationToken::new();
        let a = budget.acquire(400, &cancel).await.unwrap();
        let _b = budget.acquire(400, &cancel).await.unwrap();
        let mut c = std::pin::pin!(budget.acquire(400, &cancel));
        assert!(pending(&mut c).await, "no room: 800 + 400 > 1000");
        assert_eq!(
            budget.available_sessions(),
            0,
            "the waiter holds its permit"
        );
        drop(a);
        let c = c.await.expect("admitted once a sibling released");
        assert_eq!(budget.in_flight(), 800);
        drop(c);
    }

    /// A reservation larger than the pool is admitted when alone — the server
    /// decides whether one conversation fits — and waits when it is not.
    #[tokio::test]
    async fn larger_than_the_pool_is_admitted_alone_and_waits_otherwise() {
        let budget = SessionBudget::new(2, Some(1000));
        let cancel = CancellationToken::new();
        let big = budget
            .acquire(5000, &cancel)
            .await
            .expect("alone: admitted");
        let mut small = std::pin::pin!(budget.acquire(1, &cancel));
        assert!(pending(&mut small).await, "nothing fits next to 5000");
        drop(big);
        let small = small.await.unwrap();
        let mut big2 = std::pin::pin!(budget.acquire(5000, &cancel));
        assert!(pending(&mut big2).await, "not alone: waits");
        drop(small);
        assert!(big2.await.is_some());
    }

    /// A waiter cancelled while waiting for room leaves the sum untouched and
    /// its permit returned.
    #[tokio::test]
    async fn a_cancelled_waiter_leaves_no_reservation_behind() {
        let budget = SessionBudget::new(2, Some(1000));
        let calm = CancellationToken::new();
        let _a = budget.acquire(900, &calm).await.unwrap();
        let cancel = CancellationToken::new();
        let mut w = std::pin::pin!(budget.acquire(400, &cancel));
        assert!(pending(&mut w).await);
        cancel.cancel();
        assert!(w.await.is_none());
        assert_eq!(budget.in_flight(), 900);
        assert_eq!(budget.available_sessions(), 1);
    }

    /// No pool: never a wait for room, however large the reservations — the
    /// count alone bounds, as before the pool existed.
    #[tokio::test]
    async fn without_a_pool_only_the_count_bounds() {
        let budget = SessionBudget::new(2, None);
        let cancel = CancellationToken::new();
        let _a = budget.acquire(u64::MAX / 2, &cancel).await.unwrap();
        let _b = budget.acquire(u64::MAX / 2, &cancel).await.unwrap();
        assert_eq!(budget.in_flight(), 0);
        let mut c = std::pin::pin!(budget.acquire(1, &cancel));
        assert!(pending(&mut c).await, "two permits, both held");
        assert_eq!(SessionBudget::new(2, Some(0)).pool(), None);
    }

    /// The permit count still bounds under a pool with room to spare.
    #[tokio::test]
    async fn the_count_bounds_under_a_roomy_pool() {
        let budget = SessionBudget::new(1, Some(100_000));
        let cancel = CancellationToken::new();
        let _a = budget.acquire(1, &cancel).await.unwrap();
        let mut b = std::pin::pin!(budget.acquire(1, &cancel));
        assert!(pending(&mut b).await);
    }

    /// The silent lane is one permit wide: a second silent request waits
    /// for the first to end, whatever the interactive count says.
    #[tokio::test]
    async fn the_silent_lane_is_one_stream_wide() {
        let budget = SessionBudget::new(4, None);
        let cancel = CancellationToken::new();
        let a = budget
            .acquire_silent(10, &cancel, "reflection", false)
            .await
            .unwrap();
        assert_eq!(budget.silent_streaming(), Some("reflection"));
        let mut b = std::pin::pin!(budget.acquire_silent(10, &cancel, "compaction", false));
        assert!(pending(&mut b).await, "one silent stream at a time");
        assert_eq!(
            budget.available_sessions(),
            4,
            "the interactive lane is untouched"
        );
        drop(a);
        let b = b.await.expect("admitted once the first ended");
        assert_eq!(budget.silent_streaming(), Some("compaction"));
        drop(b);
        assert_eq!(budget.silent_streaming(), None);
    }

    /// Under a pool the two lanes share one sum: a silent reservation counts,
    /// an interactive stream that does not fit beside it waits for it, and
    /// one that fits is admitted at once.
    #[tokio::test]
    async fn the_lanes_share_the_pool() {
        let budget = SessionBudget::new(2, Some(1000));
        let cancel = CancellationToken::new();
        let silent = budget
            .acquire_silent(700, &cancel, "compaction", false)
            .await
            .unwrap();
        assert_eq!(budget.in_flight(), 700);
        let mut big = std::pin::pin!(budget.acquire(400, &cancel));
        assert!(pending(&mut big).await, "700 + 400 > 1000: the turn waits");
        let small = budget.acquire(200, &cancel).await.expect("fits beside it");
        assert_eq!(budget.in_flight(), 900);
        drop(small);
        drop(silent);
        let big = big.await.expect("admitted once the silent round ended");
        assert_eq!(budget.in_flight(), 400);
        // …and the reverse: a silent request that does not fit beside an
        // interactive stream waits, with its silent permit.
        let mut quiet = std::pin::pin!(budget.acquire_silent(700, &cancel, "reflection", false));
        assert!(pending(&mut quiet).await);
        assert_eq!(
            budget.silent_streaming(),
            None,
            "waiting for room is not streaming"
        );
        drop(big);
        assert!(quiet.await.is_some());
    }

    /// A silent request alone is admitted whatever its size — the roll's
    /// digest plus its cap can exceed a small pool, and the server decides.
    #[tokio::test]
    async fn a_silent_stream_alone_is_admitted_whatever_its_size() {
        let budget = SessionBudget::new(1, Some(1000));
        let cancel = CancellationToken::new();
        let big = budget
            .acquire_silent(5000, &cancel, "compaction", false)
            .await
            .expect("alone: admitted");
        let mut turn = std::pin::pin!(budget.acquire(1, &cancel));
        assert!(pending(&mut turn).await, "nothing fits beside it");
        drop(big);
        assert!(turn.await.is_some());
    }

    /// A silent waiter cancelled while waiting leaves no reservation and no
    /// label behind, and returns its permit.
    #[tokio::test]
    async fn a_cancelled_silent_waiter_leaves_nothing_behind() {
        let budget = SessionBudget::new(1, Some(1000));
        let calm = CancellationToken::new();
        let _turn = budget.acquire(900, &calm).await.unwrap();
        let cancel = CancellationToken::new();
        let mut w = std::pin::pin!(budget.acquire_silent(400, &cancel, "title", false));
        assert!(pending(&mut w).await);
        cancel.cancel();
        assert!(w.await.is_none());
        assert_eq!(budget.in_flight(), 900);
        assert_eq!(budget.silent_streaming(), None);
        assert!(
            budget
                .acquire_silent(1, &CancellationToken::new(), "title", false)
                .await
                .is_some(),
            "the silent permit came back"
        );
    }

    /// Pricing: the density corrects the estimate and never below 1.0, the
    /// floor wins when larger, and no cap means the whole pool.
    #[test]
    fn pricing_calibrates_floors_and_reserves_the_pool_without_a_cap() {
        let _ = Shape::COUNT;
        let budget = SessionBudget::new(2, Some(1000));
        assert_eq!(budget.density(Shape::Turn), 1.0);
        assert_eq!(budget.price(Shape::Turn, 100, 0, Some(50)), 150);
        budget.record_usage(Shape::Turn, 100, 220);
        assert_eq!(budget.density(Shape::Turn), 2.2);
        assert_eq!(budget.price(Shape::Turn, 100, 0, Some(50)), 270);
        // An estimate that over-counted: the ratio is floored, never trusted
        // to shrink a reservation.
        budget.record_usage(Shape::Turn, 100, 80);
        assert_eq!(budget.density(Shape::Turn), 1.0);
        assert_eq!(
            budget.price(Shape::Turn, 100, 300, Some(50)),
            350,
            "the floor wins"
        );
        assert_eq!(
            budget.price(Shape::Turn, 100, 0, None),
            1000,
            "no cap: the pool"
        );
        // A zero on either side records nothing.
        budget.record_usage(Shape::Turn, 0, 500);
        budget.record_usage(Shape::Turn, 500, 0);
        assert_eq!(budget.density(Shape::Turn), 1.0);
        // No pool: the reply half is what it is; the figure is unused anyway.
        assert_eq!(
            SessionBudget::new(1, None).price(Shape::Turn, 100, 0, None),
            100
        );
    }

    /// A yielding silent stream is displaced by an interactive waiter that
    /// would fit once its reservation is gone (silent-preemption §4.2): the
    /// child token the holder streams on is cancelled, the holder's own is
    /// not, the reservation reads displaced, and the waiter is admitted the
    /// moment the reservation drops.
    #[tokio::test]
    async fn an_interactive_waiter_displaces_the_silent_stream_it_would_then_fit_beside() {
        let budget = SessionBudget::new(1, Some(1000));
        let calm = CancellationToken::new();
        let roll = budget
            .acquire_silent(600, &calm, "compaction", true)
            .await
            .unwrap();
        let token = roll.stream_token();
        let mut turn = std::pin::pin!(budget.acquire(600, &calm));
        assert!(pending(&mut turn).await, "no room beside the roll");
        assert!(token.is_cancelled(), "the roll was told to yield");
        assert!(roll.displaced());
        assert!(!calm.is_cancelled(), "the holder's own token is untouched");
        assert_eq!(budget.displacing(), 1);
        drop(roll);
        let turn = turn
            .await
            .expect("admitted once the roll's reservation dropped");
        assert_eq!(budget.displacing(), 0, "uncounted at admission");
        assert_eq!(budget.in_flight(), 600);
        drop(turn);
    }

    /// A waiter that would still not fit without the silent stream displaces
    /// nothing (§4.2's "when it helps"): above one session another
    /// interactive stream is what is in the way.
    #[tokio::test]
    async fn a_waiter_that_would_still_not_fit_displaces_nothing() {
        let budget = SessionBudget::new(2, Some(1000));
        let calm = CancellationToken::new();
        let a = budget.acquire(500, &calm).await.unwrap();
        let quiet = budget
            .acquire_silent(300, &calm, "reflection", true)
            .await
            .unwrap();
        let token = quiet.stream_token();
        let mut b = std::pin::pin!(budget.acquire(600, &calm));
        assert!(pending(&mut b).await, "500 + 300 + 600 > 1000");
        assert!(
            !token.is_cancelled(),
            "500 + 600 > 1000 even without the reflection"
        );
        assert_eq!(budget.displacing(), 0);
        drop(a);
        let b = b
            .await
            .expect("300 + 600 fit: admitted beside the reflection");
        assert!(!token.is_cancelled());
        assert!(!quiet.displaced());
        drop(b);
        drop(quiet);
    }

    /// A silent stream asked for without yielding — impersonation, a summary
    /// inside a silent loop, a task past its yields — is never displaced: the
    /// waiter waits it out.
    #[tokio::test]
    async fn a_holding_silent_stream_is_never_displaced() {
        let budget = SessionBudget::new(1, Some(1000));
        let calm = CancellationToken::new();
        let preview = budget
            .acquire_silent(600, &calm, "impersonation", false)
            .await
            .unwrap();
        let token = preview.stream_token();
        let mut turn = std::pin::pin!(budget.acquire(600, &calm));
        assert!(pending(&mut turn).await);
        assert!(!token.is_cancelled());
        assert!(!preview.displaced());
        assert_eq!(budget.displacing(), 0);
        drop(preview);
        assert!(turn.await.is_some());
    }

    /// A silent waiter displaces nothing: the silent tasks yield, never the
    /// reverse (the lane's R2).
    #[tokio::test]
    async fn a_silent_waiter_displaces_nothing() {
        let budget = SessionBudget::new(1, Some(1000));
        let calm = CancellationToken::new();
        let turn = budget.acquire(600, &calm).await.unwrap();
        let mut quiet = std::pin::pin!(budget.acquire_silent(600, &calm, "title", true));
        assert!(pending(&mut quiet).await);
        assert!(!turn.stream_token().is_cancelled());
        assert_eq!(budget.displacing(), 0);
        drop(turn);
        assert!(quiet.await.is_some());
    }

    /// The waiter that displaced a stream is admitted before the displaced
    /// task's retry (§4.2): the retry takes the lane's permit but no room
    /// while the waiter is pending, and finds the room taken once it is in.
    #[tokio::test]
    async fn the_displacing_waiter_is_admitted_before_the_displaced_streams_retry() {
        let budget = SessionBudget::new(1, Some(1000));
        let calm = CancellationToken::new();
        let roll = budget
            .acquire_silent(600, &calm, "compaction", true)
            .await
            .unwrap();
        let mut turn = std::pin::pin!(budget.acquire(600, &calm));
        assert!(pending(&mut turn).await);
        assert!(roll.displaced());
        drop(roll);
        // The retry, made before the waiter has been polled again.
        let mut retry = std::pin::pin!(budget.acquire_silent(600, &calm, "compaction", true));
        assert!(
            pending(&mut retry).await,
            "deferred behind the displacing waiter"
        );
        assert_eq!(budget.in_flight(), 0);
        let turn = turn.await.expect("the waiter first");
        assert!(
            pending(&mut retry).await,
            "600 + 600 > 1000: the retry waits for the turn"
        );
        drop(turn);
        let retry = retry.await.expect("then the retry");
        assert!(!retry.displaced());
        drop(retry);
    }

    /// A displacing waiter that gives up (its turn cancelled) uncounts
    /// itself, so the displaced task's retry is not held back for nothing.
    #[tokio::test]
    async fn a_displacing_waiter_that_gives_up_uncounts_itself() {
        let budget = SessionBudget::new(1, Some(1000));
        let calm = CancellationToken::new();
        let roll = budget
            .acquire_silent(600, &calm, "compaction", true)
            .await
            .unwrap();
        let cancel = CancellationToken::new();
        let mut turn = std::pin::pin!(budget.acquire(600, &cancel));
        assert!(pending(&mut turn).await);
        assert_eq!(budget.displacing(), 1);
        cancel.cancel();
        assert!(turn.await.is_none());
        assert_eq!(budget.displacing(), 0);
        drop(roll);
        let retry = budget.acquire_silent(600, &calm, "compaction", true).await;
        assert!(retry.is_some(), "nothing holds the retry back");
    }

    /// No pool: nothing is reserved, nothing waits, nothing is displaced —
    /// the clouds and the one-slot server are untouched (R5).
    #[tokio::test]
    async fn without_a_pool_nothing_is_displaced() {
        let budget = SessionBudget::new(2, None);
        let calm = CancellationToken::new();
        let roll = budget
            .acquire_silent(600, &calm, "compaction", true)
            .await
            .unwrap();
        let turn = budget
            .acquire(600, &calm)
            .await
            .expect("no wait without a pool");
        assert!(!roll.stream_token().is_cancelled());
        assert!(!roll.displaced());
        assert_eq!(budget.displacing(), 0);
        drop(turn);
        drop(roll);
    }

    /// The holder's own cancellation (the app quitting, a timeout) reads as
    /// what it is, not as a displacement: the child fires with its parent.
    #[tokio::test]
    async fn a_holder_that_cancelled_itself_was_not_displaced() {
        let budget = SessionBudget::new(1, Some(1000));
        let cancel = CancellationToken::new();
        let roll = budget
            .acquire_silent(600, &cancel, "compaction", true)
            .await
            .unwrap();
        let token = roll.stream_token();
        cancel.cancel();
        assert!(token.is_cancelled(), "the child follows its parent");
        assert!(!roll.displaced());
    }

    /// Each kind of request prices with the latest ratio of its own kind
    /// (title-impersonation-usage §3.1): a turn's 1.34 — a tool result's
    /// JSON, measured — survives a title's 0.65, which the floor stores as
    /// 1.0 in the title's own slot.
    #[test]
    fn a_kinds_ratio_is_its_own() {
        let budget = SessionBudget::new(2, Some(100_000));
        budget.record_usage(Shape::Turn, 11_058, 14_767);
        budget.record_usage(Shape::Title, 294, 192);
        assert!((budget.density(Shape::Turn) - 1.3354).abs() < 1e-3);
        assert_eq!(budget.density(Shape::Title), 1.0, "over-counted: floored");
        assert_eq!(budget.density(Shape::Roll), 1.0, "nothing recorded");
        assert_eq!(budget.price(Shape::Turn, 1000, 0, Some(0)), 1335);
        assert_eq!(budget.price(Shape::Title, 1000, 0, Some(0)), 1000);
        assert_eq!(budget.price(Shape::Impersonation, 1000, 0, Some(0)), 1000);
    }
}
