//! Automatic retry with backoff for transient provider failures — stage 2 of
//! [docs/research/cloud-retry-backoff.md](../../../docs/research/cloud-retry-backoff.md).
//!
//! A decorator over [`EngineBackend`], not a change to the clients: one
//! implementation serves all five providers and all five call sites, and the
//! clients stay dumb about policy (fork F1(a)). It is applied to the **cloud and
//! external** backends only (F2(a)) — a managed child that died reloads for
//! minutes, and there the supervisor's health monitor and relaunch budget are the
//! honest recovery mechanism, not a three-attempt burst.
//!
//! ## What may be retried, and what may not
//!
//! Only a failure that arrives **before the first content chunk** (F3(a)). Once
//! `Text`/`Thoughts`/`ToolCall`/`ThoughtsSignature` has been yielded the turn is
//! *committed*: the user has seen the answer start, no provider we speak to can
//! resume a broken stream, and splicing a regenerated answer under a half-rendered
//! one would be a lie — so the failure is surfaced and the partial reply kept, as
//! it already is. Because a tool call counts as content, a round that produced
//! calls is never replayed **by construction**, which is what keeps tool effects
//! from double-firing. Retrying happens at this one layer and nowhere else (the
//! Google SRE rule): the orchestrator and the agentic loop re-issue nothing.
//!
//! ## Why the first attempt runs eagerly
//!
//! [`RetryBackend::chat_stream`] awaits attempt 1 before returning, so a failure
//! that will *not* be retried keeps the exact shape it had before this decorator
//! existed — an `Err` for a pre-stream failure, an `Error` chunk for an in-stream
//! one. Only once a wait is unavoidable does it return the stream and continue
//! inside it, which is what lets the UI show the wait ([`ChatChunk::Retry`]) while
//! it happens.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_stream::stream;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use super::contract::{
    ChatChunk, ChatRequest, ChatStream, EngineBackend, FinishReason, VisionSupport,
};
use super::error::EngineError;

/// How many attempts a turn gets in total, the initial one included.
///
/// Three — i.e. two retries — is what both first-party SDKs default to
/// (openai-python and the Anthropic SDKs), and the Google SRE book's reasoning
/// for a small cap applies directly: a request that has already landed on
/// overloaded capacity three times is unlikely to be helped by a fourth, while
/// the amplification it adds is charged to a provider that is already shedding
/// load.
pub const MAX_ATTEMPTS: u32 = 3;

/// The wait after the first failure; each subsequent one multiplies by
/// [`BACKOFF_FACTOR`]. So the waits are ~1 s and ~2 s, and a turn that fails
/// three times has spent ~3 s of waiting — small enough that a user watching a
/// TUI reads it as slowness rather than as a hang.
pub const BASE_DELAY: Duration = Duration::from_secs(1);

/// See [`BASE_DELAY`].
pub const BACKOFF_FACTOR: u32 = 2;

/// How much of a computed delay jitter may remove (a quarter).
///
/// Downward only, which is the shape openai-python uses (`1 - 0.25 * rand`):
/// spreading retries earlier cannot push a client past a deadline it was already
/// told about, and it still breaks up the thundering herd of many clients
/// retrying on the same beat.
pub const JITTER: f64 = 0.25;

/// The longest `Retry-After` we will honour by waiting.
///
/// Providers are explicit that an earlier retry fails, so the header is obeyed
/// rather than second-guessed — but only within reason. Beyond this the request is
/// treated as **not** retryable and the failure is surfaced at once: a provider
/// asking for minutes is describing a quota rather than a blip, its body says so,
/// and a TUI frozen on a spinner is worse than an error a user can read. The
/// reference SDK caps its own trust at 120 s; this is an interactive client, so it
/// caps lower.
pub const RETRY_AFTER_CAP: Duration = Duration::from_secs(30);

/// The retry policy. Deliberately constants rather than settings (fork F6(a)) —
/// the same taste the health monitor's cadence is kept at; if a setup ever needs
/// them tunable they belong in the "Model" section next to the engine fields.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub factor: u32,
    pub jitter: f64,
    pub retry_after_cap: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: MAX_ATTEMPTS,
            base_delay: BASE_DELAY,
            factor: BACKOFF_FACTOR,
            jitter: JITTER,
            retry_after_cap: RETRY_AFTER_CAP,
        }
    }
}

/// What to do after an attempt failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    /// Wait this long, then try again.
    Wait(Duration),
    /// Report the failure.
    GiveUp,
}

impl RetryPolicy {
    /// Decides what happens after `attempt` failed.
    fn decide(self, attempt: u32, transient: bool, retry_after: Option<Duration>) -> Decision {
        if !transient || attempt >= self.max_attempts {
            return Decision::GiveUp;
        }
        match retry_after {
            // The provider named a delay: honour it, or refuse the wait entirely
            // when it is longer than an interactive client should hide.
            Some(asked) if asked > self.retry_after_cap => Decision::GiveUp,
            Some(asked) => Decision::Wait(asked),
            None => Decision::Wait(self.backoff(attempt)),
        }
    }

    /// `base * factor^(attempt-1)`, jittered downward.
    fn backoff(self, attempt: u32) -> Duration {
        let steps = attempt.saturating_sub(1);
        let scale = self.factor.saturating_pow(steps);
        let base = self.base_delay.saturating_mul(scale.max(1));
        base.mul_f64(1.0 - self.jitter * unit_random())
    }
}

/// A number in `[0, 1)` for jitter.
///
/// Deliberately not a `rand` dependency: the only requirement is that many
/// clients do not pick the same instant, and the wall clock's sub-millisecond
/// digits already satisfy that. Reading the clock also keeps this usable under
/// `tokio::time::pause`, which freezes timers but not `SystemTime`.
fn unit_random() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    f64::from(nanos % 1_000_000) / 1_000_000.0
}

/// Why an attempt failed, and in which of the two shapes it has to be reported.
enum Failure {
    /// No stream was ever opened — reportable as `Err`, exactly as before.
    Pre {
        error: anyhow::Error,
        transient: bool,
        retry_after: Option<Duration>,
    },
    /// The failure arrived inside the stream — reportable as chunks.
    InStream { message: String, transient: bool },
}

impl Failure {
    fn transient(&self) -> bool {
        match self {
            Failure::Pre { transient, .. } | Failure::InStream { transient, .. } => *transient,
        }
    }

    fn retry_after(&self) -> Option<Duration> {
        match self {
            Failure::Pre { retry_after, .. } => *retry_after,
            Failure::InStream { .. } => None,
        }
    }

    /// The chunks that report this failure from **inside** a stream (which is where
    /// we are once any waiting has happened).
    fn into_chunks(self) -> [ChatChunk; 2] {
        match self {
            Failure::Pre {
                error, transient, ..
            } => ChatChunk::failure(error.to_string(), transient),
            Failure::InStream { message, transient } => ChatChunk::failure(message, transient),
        }
    }
}

/// The outcome of one attempt.
enum Attempt {
    /// The turn is under way (content arrived, or it ended without failing) —
    /// `head` is what was already taken off the stream, `rest` is the remainder.
    Started {
        head: Vec<ChatChunk>,
        rest: ChatStream,
    },
    /// It failed. `head` holds the non-committing chunks seen first (a `Usage`,
    /// typically): dropped if we retry, replayed if we give up.
    Failed {
        head: Vec<ChatChunk>,
        failure: Failure,
    },
}

/// Runs one attempt, reading the stream only as far as the commit point.
async fn run_attempt(
    inner: &Arc<dyn EngineBackend>,
    req: ChatRequest,
    cancel: &CancellationToken,
) -> Attempt {
    let mut stream = match inner.chat_stream(req, cancel.clone()).await {
        Ok(stream) => stream,
        Err(error) => {
            // The verdict comes from the typed error. An untyped one is treated as
            // permanent on purpose: without a status there is nothing to justify
            // replaying a request the provider may have rejected on its merits.
            let (transient, retry_after) = match error.downcast_ref::<EngineError>() {
                Some(typed) => (typed.is_transient(), typed.retry_after),
                None => (false, None),
            };
            return Attempt::Failed {
                head: Vec::new(),
                failure: Failure::Pre {
                    error,
                    transient,
                    retry_after,
                },
            };
        }
    };

    let mut head = Vec::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            ChatChunk::Error { message, transient } => {
                return Attempt::Failed {
                    head,
                    failure: Failure::InStream { message, transient },
                };
            }
            // The commit point. `Usage` deliberately does not commit: Anthropic
            // reports it in `message_start`, before a single token of content, and
            // re-emitting it after a retry is harmless (the counter is overwritten)
            // whereas treating it as content would make every Anthropic turn
            // unretryable.
            chunk @ (ChatChunk::Text(_)
            | ChatChunk::Thoughts(_)
            | ChatChunk::ThoughtsSignature(_)
            | ChatChunk::ToolCall(_)
            | ChatChunk::Finished(_)) => {
                head.push(chunk);
                return Attempt::Started { head, rest: stream };
            }
            chunk @ (ChatChunk::Usage(_) | ChatChunk::Retry { .. }) => head.push(chunk),
        }
    }
    // A stream that ended without a terminator: nothing failed, so hand over what
    // there is rather than inventing a retry.
    Attempt::Started { head, rest: stream }
}

/// Prepends already-consumed chunks back onto the remainder of a stream.
fn resume(head: Vec<ChatChunk>, rest: ChatStream) -> ChatStream {
    Box::pin(futures_util::stream::iter(head).chain(rest))
}

/// An [`EngineBackend`] that retries transient failures. See the module docs.
pub struct RetryBackend {
    inner: Arc<dyn EngineBackend>,
    policy: RetryPolicy,
}

impl RetryBackend {
    /// Wraps `inner` with the default policy.
    pub fn wrap(inner: Arc<dyn EngineBackend>) -> Arc<dyn EngineBackend> {
        Arc::new(Self {
            inner,
            policy: RetryPolicy::default(),
        })
    }

    /// Wraps `inner` with an explicit policy (tests).
    #[cfg(test)]
    pub fn with_policy(inner: Arc<dyn EngineBackend>, policy: RetryPolicy) -> Self {
        Self { inner, policy }
    }
}

#[async_trait::async_trait]
impl EngineBackend for RetryBackend {
    async fn chat_stream(&self, req: ChatRequest, cancel: CancellationToken) -> Result<ChatStream> {
        // Attempt 1 eagerly — see the module docs on why.
        let (head, failure) = match run_attempt(&self.inner, req.clone(), &cancel).await {
            Attempt::Started { head, rest } => return Ok(resume(head, rest)),
            Attempt::Failed { head, failure } => (head, failure),
        };
        let mut delay = match self
            .policy
            .decide(1, failure.transient(), failure.retry_after())
        {
            Decision::Wait(delay) => delay,
            // Nothing will be retried, so report exactly what the inner backend
            // reported, in its own shape.
            Decision::GiveUp => {
                return match failure {
                    Failure::Pre { error, .. } => Err(error),
                    in_stream => Ok(resume(
                        [head, in_stream.into_chunks().to_vec()].concat(),
                        Box::pin(futures_util::stream::empty()),
                    )),
                };
            }
        };

        let inner = self.inner.clone();
        let policy = self.policy;
        Ok(Box::pin(stream! {
            let mut attempt = 1u32;
            loop {
                attempt += 1;
                yield ChatChunk::Retry { attempt, max: policy.max_attempts, delay };
                // Interruptible: a user who pressed `Esc` must not be made to wait
                // out a backoff, and neither must a turn whose chat was switched
                // away. Same shape as the readiness poll in `managed.rs`.
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        yield ChatChunk::Finished(FinishReason::Cancelled);
                        return;
                    }
                    _ = tokio::time::sleep(delay) => {}
                }
                match run_attempt(&inner, req.clone(), &cancel).await {
                    Attempt::Started { head, rest } => {
                        for chunk in head { yield chunk; }
                        let mut rest = rest;
                        while let Some(chunk) = rest.next().await { yield chunk; }
                        return;
                    }
                    Attempt::Failed { head, failure } => {
                        match policy.decide(attempt, failure.transient(), failure.retry_after()) {
                            Decision::Wait(next) => delay = next,
                            Decision::GiveUp => {
                                for chunk in head { yield chunk; }
                                for chunk in failure.into_chunks() { yield chunk; }
                                return;
                            }
                        }
                    }
                }
            }
        }))
    }

    /// Delegated, and load-bearing: this is where auto-compaction learns the
    /// engine's context window (spec §6.7), so a decorator that answered `None`
    /// would silently switch the automatic trigger off for every wrapped backend.
    async fn context_budget(&self) -> Option<u32> {
        self.inner.context_budget().await
    }

    /// Delegated for the same reason, and it is the same hole: every cloud backend
    /// is wrapped in this decorator, so a `RetryBackend` that fell back to the
    /// trait's default would answer `Unknown` for a Claude backend that knows
    /// perfectly well it takes images — a silent capability regression of exactly
    /// the shape docs/lessons.md §9 records.
    async fn vision(&self) -> VisionSupport {
        self.inner.vision().await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::shared::api::contract::{TokenUsage, ToolCallDelta};
    use crate::shared::api::error::EngineErrorKind;

    /// What one attempt against the scripted backend does.
    enum Outcome {
        /// No stream ever opens — the `Err` shape.
        Pre(EngineError),
        /// A stream opens and yields these.
        Chunks(Vec<ChatChunk>),
    }

    /// An inner backend that plays a scripted outcome per attempt and **counts the
    /// attempts**. The count is the load-bearing assertion throughout: "retried"
    /// and "did not retry" are indistinguishable from the chunks alone when the
    /// outcome matches, and asserting `is_ok()` on a path built to degrade
    /// gracefully proves nothing (docs/lessons.md §2).
    struct Scripted {
        outcomes: Mutex<VecDeque<Outcome>>,
        calls: AtomicUsize,
        budget: Option<u32>,
        vision: VisionSupport,
    }

    impl Scripted {
        fn new(outcomes: Vec<Outcome>) -> Arc<Self> {
            Arc::new(Self {
                outcomes: Mutex::new(outcomes.into()),
                calls: AtomicUsize::new(0),
                budget: None,
                vision: VisionSupport::Unknown,
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl EngineBackend for Scripted {
        async fn chat_stream(
            &self,
            _req: ChatRequest,
            _cancel: CancellationToken,
        ) -> Result<ChatStream> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            // Past the end of the script, keep failing transiently: a test that
            // under-scripts must not accidentally succeed.
            let outcome = self
                .outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Outcome::Pre(err(503, None)));
            match outcome {
                Outcome::Pre(e) => Err(e.into()),
                Outcome::Chunks(chunks) => Ok(Box::pin(futures_util::stream::iter(chunks))),
            }
        }

        async fn context_budget(&self) -> Option<u32> {
            self.budget
        }

        async fn vision(&self) -> VisionSupport {
            self.vision
        }
    }

    fn err(status: u16, retry_after: Option<Duration>) -> EngineError {
        EngineError {
            kind: EngineErrorKind::Status,
            status: Some(status),
            retry_after,
            message: format!("engine returned status {status}"),
        }
    }

    fn req() -> ChatRequest {
        ChatRequest {
            system: None,
            messages: Vec::new(),
            sampling: Default::default(),
            tools: Vec::new(),
        }
    }

    fn text() -> Vec<ChatChunk> {
        vec![
            ChatChunk::Text("hello".into()),
            ChatChunk::Finished(FinishReason::Stop),
        ]
    }

    async fn drain(backend: &dyn EngineBackend) -> Result<Vec<ChatChunk>> {
        let mut stream = backend.chat_stream(req(), CancellationToken::new()).await?;
        let mut out = Vec::new();
        while let Some(c) = stream.next().await {
            out.push(c);
        }
        Ok(out)
    }

    fn retries(chunks: &[ChatChunk]) -> Vec<(u32, Duration)> {
        chunks
            .iter()
            .filter_map(|c| match c {
                ChatChunk::Retry { attempt, delay, .. } => Some((*attempt, *delay)),
                _ => None,
            })
            .collect()
    }

    fn has_error(chunks: &[ChatChunk]) -> bool {
        chunks.iter().any(|c| matches!(c, ChatChunk::Error { .. }))
    }

    #[tokio::test(start_paused = true)]
    async fn a_transient_failure_is_retried_and_the_next_attempt_wins() {
        let inner = Scripted::new(vec![Outcome::Pre(err(429, None)), Outcome::Chunks(text())]);
        let backend = RetryBackend::wrap(inner.clone());
        let chunks = drain(backend.as_ref()).await.unwrap();

        assert_eq!(inner.calls(), 2, "the request must be re-issued once");
        assert_eq!(retries(&chunks).len(), 1, "the wait must be announced");
        assert_eq!(
            retries(&chunks)[0].0,
            2,
            "the chip names the attempt about to start"
        );
        assert!(chunks.contains(&ChatChunk::Text("hello".into())));
        assert!(
            !has_error(&chunks),
            "a recovered turn reports no error: {chunks:?}"
        );
    }

    /// The contract stage 1 established has to survive the decorator: a failure
    /// that will not be retried is still an `Err`, with its type and message intact.
    #[tokio::test(start_paused = true)]
    async fn a_permanent_failure_is_not_retried_and_stays_an_err() {
        let inner = Scripted::new(vec![Outcome::Pre(err(400, None))]);
        let backend = RetryBackend::wrap(inner.clone());
        let result = backend.chat_stream(req(), CancellationToken::new()).await;

        let e = match result {
            Ok(_) => panic!("a 400 must not be turned into a stream"),
            Err(e) => e,
        };
        assert_eq!(inner.calls(), 1, "a 400 must be sent exactly once");
        assert!(e.to_string().contains("400"), "{e}");
        assert!(
            e.downcast_ref::<EngineError>().is_some(),
            "the typed error must pass through unchanged"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_in_stream_failure_before_content_is_retried() {
        let inner = Scripted::new(vec![
            Outcome::Chunks(vec![
                // Anthropic reports usage before any content — this must not count
                // as commitment, or none of its turns could ever be retried.
                ChatChunk::Usage(TokenUsage::default()),
                ChatChunk::Error {
                    message: "overloaded_error".into(),
                    transient: true,
                },
                ChatChunk::Finished(FinishReason::Error),
            ]),
            Outcome::Chunks(text()),
        ]);
        let backend = RetryBackend::wrap(inner.clone());
        let chunks = drain(backend.as_ref()).await.unwrap();

        assert_eq!(inner.calls(), 2);
        assert!(chunks.contains(&ChatChunk::Text("hello".into())));
        assert!(
            !has_error(&chunks),
            "the failed attempt's error must not reach the user: {chunks:?}"
        );
    }

    /// The safety property the whole design rests on: once content is out, nothing
    /// is replayed.
    #[tokio::test(start_paused = true)]
    async fn text_commits_the_turn_so_a_later_failure_is_surfaced_not_retried() {
        let inner = Scripted::new(vec![Outcome::Chunks(vec![
            ChatChunk::Text("half an ans".into()),
            ChatChunk::Error {
                message: "overloaded_error".into(),
                transient: true,
            },
            ChatChunk::Finished(FinishReason::Error),
        ])]);
        let backend = RetryBackend::wrap(inner.clone());
        let chunks = drain(backend.as_ref()).await.unwrap();

        assert_eq!(
            inner.calls(),
            1,
            "a rendered answer must never be re-requested"
        );
        assert!(has_error(&chunks));
        assert_eq!(
            chunks.last(),
            Some(&ChatChunk::Finished(FinishReason::Error))
        );
    }

    /// A tool call is content, and that is what keeps a retry from re-running tools.
    #[tokio::test(start_paused = true)]
    async fn a_tool_call_commits_the_turn() {
        let inner = Scripted::new(vec![Outcome::Chunks(vec![
            ChatChunk::ToolCall(ToolCallDelta {
                index: 0,
                id: Some("c1".into()),
                name: Some("fs_write".into()),
                arguments: "{}".into(),
                thought_signature: None,
            }),
            ChatChunk::Error {
                message: "overloaded_error".into(),
                transient: true,
            },
            ChatChunk::Finished(FinishReason::Error),
        ])]);
        let backend = RetryBackend::wrap(inner.clone());
        let chunks = drain(backend.as_ref()).await.unwrap();

        assert_eq!(
            inner.calls(),
            1,
            "replaying a round that emitted a tool call would re-run the tool"
        );
        assert!(chunks.iter().any(|c| matches!(c, ChatChunk::ToolCall(_))));
    }

    #[tokio::test(start_paused = true)]
    async fn the_attempt_budget_is_spent_then_the_failure_is_reported() {
        // Always transient, so only the budget can stop it.
        let inner = Scripted::new(vec![
            Outcome::Pre(err(503, None)),
            Outcome::Pre(err(503, None)),
            Outcome::Pre(err(503, None)),
            Outcome::Chunks(text()), // must never be reached
        ]);
        let backend = RetryBackend::wrap(inner.clone());
        let chunks = drain(backend.as_ref()).await.unwrap();

        assert_eq!(inner.calls(), MAX_ATTEMPTS as usize, "exactly the budget");
        assert_eq!(retries(&chunks).len(), 2, "two waits for three attempts");
        let reported = chunks.iter().find_map(|c| match c {
            ChatChunk::Error { message, .. } => Some(message.clone()),
            _ => None,
        });
        assert!(
            reported.is_some_and(|m| m.contains("503")),
            "the last failure's own text is what gets reported: {chunks:?}"
        );
        assert_eq!(
            chunks.last(),
            Some(&ChatChunk::Finished(FinishReason::Error))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_retry_after_within_the_cap_is_honoured_exactly() {
        let asked = Duration::from_secs(5);
        let inner = Scripted::new(vec![
            Outcome::Pre(err(429, Some(asked))),
            Outcome::Chunks(text()),
        ]);
        let backend = RetryBackend::wrap(inner.clone());
        let chunks = drain(backend.as_ref()).await.unwrap();

        assert_eq!(inner.calls(), 2);
        // Not jittered: the provider says an earlier retry fails, so the wait is
        // taken as given rather than shortened.
        assert_eq!(retries(&chunks)[0].1, asked);
    }

    #[tokio::test(start_paused = true)]
    async fn a_retry_after_beyond_the_cap_fails_fast_instead_of_waiting() {
        let inner = Scripted::new(vec![Outcome::Pre(err(
            429,
            Some(RETRY_AFTER_CAP + Duration::from_secs(1)),
        ))]);
        let backend = RetryBackend::wrap(inner.clone());
        let result = backend.chat_stream(req(), CancellationToken::new()).await;

        assert!(result.is_err(), "a quota-length wait is not hidden");
        assert_eq!(inner.calls(), 1, "and nothing is retried");
    }

    /// A user who pressed `Esc` must not be held for the backoff.
    #[tokio::test(start_paused = true)]
    async fn cancelling_during_the_backoff_ends_the_turn_at_once() {
        let inner = Scripted::new(vec![
            Outcome::Pre(err(503, None)),
            Outcome::Chunks(text()), // must never be reached
        ]);
        let backend = RetryBackend::wrap(inner.clone());
        let cancel = CancellationToken::new();
        let mut stream = backend.chat_stream(req(), cancel.clone()).await.unwrap();

        // The wait is announced first; cancelling now must be honoured before the
        // sleep completes — the `select!` is biased on the token.
        let first = stream.next().await;
        assert!(matches!(first, Some(ChatChunk::Retry { .. })), "{first:?}");
        cancel.cancel();
        assert_eq!(
            stream.next().await,
            Some(ChatChunk::Finished(FinishReason::Cancelled))
        );
        assert_eq!(inner.calls(), 1, "the second attempt must not be made");
    }

    /// Guards a trap the module docs call out: auto-compaction learns the context
    /// window through this call, so a decorator that forgot to delegate would
    /// silently switch the automatic trigger off for every wrapped backend.
    #[tokio::test]
    async fn the_context_budget_is_delegated() {
        let mut scripted = Scripted::new(vec![]);
        Arc::get_mut(&mut scripted).unwrap().budget = Some(16384);
        let backend = RetryBackend::wrap(scripted);
        assert_eq!(backend.context_budget().await, Some(16384));
    }

    /// The sibling of the test above, and the same trap: every cloud backend is
    /// wrapped in this decorator, so a forgotten delegation would report `Unknown`
    /// for a backend that answers `Supported` — image attachments would silently
    /// lose their capability check on all four providers (docs/lessons.md §9).
    #[tokio::test]
    async fn vision_is_delegated() {
        // The default is `Unknown`, so a delegation test has to assert on a value
        // the decorator could not have produced by falling through.
        let mut scripted = Scripted::new(vec![]);
        Arc::get_mut(&mut scripted).unwrap().vision = VisionSupport::Supported;
        let backend = RetryBackend::wrap(scripted);
        assert_eq!(backend.vision().await, VisionSupport::Supported);

        let mut scripted = Scripted::new(vec![]);
        Arc::get_mut(&mut scripted).unwrap().vision = VisionSupport::Unsupported;
        let backend = RetryBackend::wrap(scripted);
        assert_eq!(backend.vision().await, VisionSupport::Unsupported);
    }

    /// An explicit policy is what lets a test pin the shape without waiting on the
    /// real constants.
    #[tokio::test(start_paused = true)]
    async fn a_single_attempt_policy_never_retries() {
        let inner = Scripted::new(vec![Outcome::Pre(err(503, None))]);
        let policy = RetryPolicy {
            max_attempts: 1,
            ..RetryPolicy::default()
        };
        let backend = RetryBackend::with_policy(inner.clone(), policy);
        let result = backend.chat_stream(req(), CancellationToken::new()).await;

        assert!(result.is_err());
        assert_eq!(inner.calls(), 1, "a budget of one means one request");
    }

    #[test]
    fn the_policy_decides_by_transience_then_budget_then_the_header() {
        let p = RetryPolicy::default();
        // Permanent: never, whatever the attempt.
        assert_eq!(p.decide(1, false, None), Decision::GiveUp);
        // Transient with budget left → wait; budget spent → stop.
        assert!(matches!(p.decide(1, true, None), Decision::Wait(_)));
        assert!(matches!(p.decide(2, true, None), Decision::Wait(_)));
        assert_eq!(p.decide(MAX_ATTEMPTS, true, None), Decision::GiveUp);
        // The header wins over the backoff, and the cap wins over the header.
        assert_eq!(
            p.decide(1, true, Some(Duration::from_secs(7))),
            Decision::Wait(Duration::from_secs(7))
        );
        assert_eq!(
            p.decide(1, true, Some(RETRY_AFTER_CAP + Duration::from_millis(1))),
            Decision::GiveUp
        );
        // Exactly at the cap is still honoured — the boundary is inclusive.
        assert_eq!(
            p.decide(1, true, Some(RETRY_AFTER_CAP)),
            Decision::Wait(RETRY_AFTER_CAP)
        );
    }

    #[test]
    fn the_backoff_grows_and_jitter_only_shortens_it() {
        let p = RetryPolicy::default();
        // Repeated because the jitter is drawn from the clock: one sample could
        // pass a bound it usually violates.
        for _ in 0..64 {
            let first = p.backoff(1);
            let second = p.backoff(2);
            assert!(
                first <= BASE_DELAY && first >= BASE_DELAY.mul_f64(1.0 - JITTER),
                "{first:?}"
            );
            let doubled = BASE_DELAY * BACKOFF_FACTOR;
            assert!(
                second <= doubled && second >= doubled.mul_f64(1.0 - JITTER),
                "{second:?}"
            );
            assert!(
                second > first,
                "the wait must grow: {first:?} -> {second:?}"
            );
        }
    }
}
