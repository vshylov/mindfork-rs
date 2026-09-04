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
//! With no pool known (`sessions = 1`, a cloud, an external server that
//! answers no `/props`) the type is the bare semaphore it replaced, bit for
//! bit: the count bounds, nothing is priced, nothing waits for room.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use tokio::sync::{Notify, Semaphore, SemaphorePermit};
use tokio_util::sync::CancellationToken;

/// The budget: the permit count and, when a pool is known, the token sum.
pub struct SessionBudget {
    permits: Semaphore,
    /// The KV pool the open streams share, in tokens. `None`: no guard.
    pool: Option<u64>,
    /// The sum of the reservations of the streams currently open. A `std`
    /// mutex, never held across an await.
    in_flight: Mutex<u64>,
    /// Woken (all waiters) whenever a reservation is released.
    room: Notify,
    /// The estimator's correction (research §4.3): the latest ratio of an exact
    /// `usage.prompt_tokens` to the estimate of the same request, as `f64`
    /// bits; `0` — none recorded yet, read as `1.0`.
    density: AtomicU64,
}

/// A stream's place in the budget: one permit and its token reservation,
/// both released when it is dropped — the sum first (so a waiter woken by it
/// finds the permit free too), then the permit.
pub struct Reservation<'a> {
    budget: &'a SessionBudget,
    tokens: u64,
    _permit: SemaphorePermit<'a>,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if self.tokens > 0 {
            let mut open = self.budget.open();
            *open = open.saturating_sub(self.tokens);
            drop(open);
            self.budget.room.notify_waiters();
        }
    }
}

impl std::fmt::Debug for SessionBudget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionBudget")
            .field("sessions", &self.permits.available_permits())
            .field("pool", &self.pool)
            .field("in_flight", &*self.open())
            .field("density", &self.density())
            .finish()
    }
}

impl SessionBudget {
    /// `sessions` permits (never below one — a turn must be able to stream)
    /// over a pool of `pool` tokens, or over no pool at all (`None` or `0`).
    pub fn new(sessions: u32, pool: Option<u64>) -> Self {
        Self {
            permits: Semaphore::new(sessions.max(1) as usize),
            pool: pool.filter(|&n| n > 0),
            in_flight: Mutex::new(0),
            room: Notify::new(),
            density: AtomicU64::new(0),
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

    fn open(&self) -> std::sync::MutexGuard<'_, u64> {
        self.in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The estimator's correction factor: the latest exact-to-estimate ratio
    /// recorded, floored at `1.0` (an estimate that over-counts costs a wait
    /// that could have run; one that under-counts costs the failure the type
    /// exists to prevent — research R6). `1.0` until a round has reported.
    pub fn density(&self) -> f64 {
        let bits = self.density.load(Ordering::Relaxed);
        if bits == 0 {
            return 1.0;
        }
        f64::from_bits(bits).max(1.0)
    }

    /// Records a round's exact prompt size next to the estimate made for the
    /// same request. Any loop of the turn may record; the latest wins — the
    /// tokenizer's density on this conversation's kind of text.
    pub fn record_usage(&self, estimate: u64, exact: u64) {
        if estimate == 0 || exact == 0 {
            return;
        }
        let ratio = (exact as f64 / estimate as f64).max(1.0);
        self.density.store(ratio.to_bits(), Ordering::Relaxed);
    }

    /// What a stream reserves (research §4.2): the calibrated `estimate` of its
    /// prompt, floored by `floor` (the loop's last exact size plus what it
    /// generated — a lower bound the server has vouched for; `0` for a first
    /// round), plus `reply_cap`. With no cap the reservation is the whole
    /// pool: the stream is admitted alone. Meaningless (and unused) without a
    /// pool.
    pub fn price(&self, estimate: u64, floor: u64, reply_cap: Option<u64>) -> u64 {
        let prompt = ((estimate as f64 * self.density()).round() as u64).max(floor);
        match (self.pool, reply_cap) {
            (Some(pool), None) => pool,
            (_, cap) => prompt.saturating_add(cap.unwrap_or(0)),
        }
    }

    /// Takes a permit and, under a pool, waits until `need` tokens fit next to
    /// the open reservations — or no stream is open at all. `None` when the
    /// turn is cancelled while waiting, for either. The reservation is the
    /// caller's to drop, which it does the moment its stream ends, so a
    /// round's tools never hold one.
    pub async fn acquire(&self, need: u64, cancel: &CancellationToken) -> Option<Reservation<'_>> {
        let permit = tokio::select! {
            biased;
            _ = cancel.cancelled() => return None,
            // A closed semaphore cannot happen here (nothing closes it), and
            // reads as "cancelled" rather than as a panic.
            permit = self.permits.acquire() => permit.ok()?,
        };
        let Some(pool) = self.pool else {
            return Some(Reservation {
                budget: self,
                tokens: 0,
                _permit: permit,
            });
        };
        let mut waited = false;
        loop {
            // Registered before the check, so a release between the check and
            // the wait is not missed.
            let notified = self.room.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut open = self.open();
                if *open == 0 || open.saturating_add(need) <= pool {
                    *open += need;
                    return Some(Reservation {
                        budget: self,
                        tokens: need,
                        _permit: permit,
                    });
                }
                if !waited {
                    tracing::info!(
                        need,
                        in_flight = *open,
                        pool,
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

    /// Pricing: the density corrects the estimate and never below 1.0, the
    /// floor wins when larger, and no cap means the whole pool.
    #[test]
    fn pricing_calibrates_floors_and_reserves_the_pool_without_a_cap() {
        let budget = SessionBudget::new(2, Some(1000));
        assert_eq!(budget.density(), 1.0);
        assert_eq!(budget.price(100, 0, Some(50)), 150);
        budget.record_usage(100, 220);
        assert_eq!(budget.density(), 2.2);
        assert_eq!(budget.price(100, 0, Some(50)), 270);
        // An estimate that over-counted: the ratio is floored, never trusted
        // to shrink a reservation.
        budget.record_usage(100, 80);
        assert_eq!(budget.density(), 1.0);
        assert_eq!(budget.price(100, 300, Some(50)), 350, "the floor wins");
        assert_eq!(budget.price(100, 0, None), 1000, "no cap: the pool");
        // A zero on either side records nothing.
        budget.record_usage(0, 500);
        budget.record_usage(500, 0);
        assert_eq!(budget.density(), 1.0);
        // No pool: the reply half is what it is; the figure is unused anyway.
        assert_eq!(SessionBudget::new(1, None).price(100, 0, None), 100);
    }
}
