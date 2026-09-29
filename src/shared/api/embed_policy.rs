//! What stands between the application and an embedding endpoint: requests of a
//! bounded size, and — for a cloud — another attempt when the failure is one
//! that passes. Stage 2 of docs/research/openrouter-mode.md (fork F8), built for
//! every embedder rather than for the gateway's.
//!
//! Two decorators over [`Embedder`], for the reason [`RetryBackend`] is one over
//! `EngineBackend`: the clients stay dumb about policy, and no call site can
//! forget it. The supervisor stacks them, outermost first:
//!
//! ```text
//! BatchedEmbedder { RetryEmbedder { the client } }
//! ```
//!
//! so a long input is split first and each part is retried by itself — a part
//! that already answered is not asked for again.
//!
//! [`RetryBackend`]: super::retry::RetryBackend

use std::sync::Arc;

use anyhow::Result;

use super::contract::{EmbedRole, Embedder};
use super::error::EngineError;
use super::retry::{Decision, RetryPolicy};

/// How many inputs one request carries at most.
///
/// Measured on 2026-09-29, by sending more until the endpoint refused: Gemini's
/// OpenAI-compatible endpoint takes **100** (`400 "at most 100 requests can be
/// in one batch"`), and so do two of the gateway's 33 embedding models, which
/// are Gemini's; DeepInfra, behind the gateway's `baai/bge-m3`, takes 1024;
/// OpenAI 2048. A `llama-server` has no count of its own. So the number has to
/// stay under a hundred, and it stays well under: at the default chunk size a
/// request of 64 is some tens of thousands of tokens, which is inside every
/// per-request token ceiling a provider publishes as well.
///
/// The application's own ingest batches are 16 and 32 and pass through whole.
/// What this splits is the callers that send everything they have in one
/// request — `rag_add`, which embeds every chunk of a text at once and was
/// refused by Gemini for any text of more than a hundred chunks.
pub const MAX_INPUTS: usize = 64;

// Whoever raises the number meets the measurement it rests on before the
// build does anything else: Gemini refuses the hundred-and-first.
const _: () = assert!(MAX_INPUTS < 100);

/// An [`Embedder`] whose requests carry at most [`MAX_INPUTS`] texts. A longer
/// input is sent in parts, one after another, and answered as one list in the
/// order it was given.
pub struct BatchedEmbedder {
    inner: Arc<dyn Embedder>,
    cap: usize,
}

impl BatchedEmbedder {
    /// Wraps `inner` with the cap every embedder gets.
    pub fn wrap(inner: Arc<dyn Embedder>) -> Arc<dyn Embedder> {
        Arc::new(Self {
            inner,
            cap: MAX_INPUTS,
        })
    }

    /// Wraps `inner` with an explicit cap (tests).
    #[cfg(test)]
    fn with_cap(inner: Arc<dyn Embedder>, cap: usize) -> Self {
        Self { inner, cap }
    }

    /// One request, held to its promise: as many vectors as there were texts.
    ///
    /// Checked here rather than left to the callers because of what a part is
    /// for. Vectors are matched to their texts by position; a part that came
    /// back one short would shift every vector after it onto its neighbour's
    /// text, and nothing downstream could tell.
    async fn part(&self, texts: Vec<String>, role: EmbedRole) -> Result<Vec<Vec<f32>>> {
        let asked = texts.len();
        let vectors = self.inner.embed(texts, role).await?;
        anyhow::ensure!(
            vectors.len() == asked,
            "the embedder answered {} vectors for {asked} inputs",
            vectors.len()
        );
        Ok(vectors)
    }
}

#[async_trait::async_trait]
impl Embedder for BatchedEmbedder {
    async fn embed(&self, texts: Vec<String>, role: EmbedRole) -> Result<Vec<Vec<f32>>> {
        if texts.len() <= self.cap {
            return self.part(texts, role).await;
        }
        let mut vectors = Vec::with_capacity(texts.len());
        let mut rest = texts.into_iter().peekable();
        while rest.peek().is_some() {
            let part: Vec<String> = rest.by_ref().take(self.cap).collect();
            // All or nothing, as one request is: a caller that got half the
            // vectors would have to know which half.
            vectors.extend(self.part(part, role).await?);
        }
        Ok(vectors)
    }
}

/// An [`Embedder`] that makes another attempt after a failure that passes — a
/// rate limit, an overloaded provider, a connection that died — under the policy
/// a chat turn is retried by ([`RetryPolicy`]).
///
/// It is simpler than its chat counterpart in one way that matters: an
/// embedding request has no commit point. It answers whole or fails whole, so
/// nothing is ever replayed under a reader's eyes and every transient failure
/// may be retried.
///
/// For the clouds only. A server the user runs is either up or down, and a
/// retry against one that is down buys three seconds of waiting per call — for
/// memory tools that embed several times in one turn, a wait the user would
/// read as the application hanging on a server they have not started.
pub struct RetryEmbedder {
    inner: Arc<dyn Embedder>,
    policy: RetryPolicy,
}

impl RetryEmbedder {
    /// Wraps `inner` with the default policy.
    pub fn wrap(inner: Arc<dyn Embedder>) -> Arc<dyn Embedder> {
        Arc::new(Self {
            inner,
            policy: RetryPolicy::default(),
        })
    }

    /// Wraps `inner` with an explicit policy (tests).
    #[cfg(test)]
    fn with_policy(inner: Arc<dyn Embedder>, policy: RetryPolicy) -> Self {
        Self { inner, policy }
    }
}

#[async_trait::async_trait]
impl Embedder for RetryEmbedder {
    async fn embed(&self, texts: Vec<String>, role: EmbedRole) -> Result<Vec<Vec<f32>>> {
        let mut attempt = 1u32;
        let mut outcome = self.inner.embed(texts.clone(), role).await;
        while let Err(error) = &outcome {
            // The verdict comes from the typed error, as for a chat turn: an
            // untyped failure — a body that did not parse — is a statement about
            // the answer, and asking again would get the same one.
            let (transient, retry_after) = match error.downcast_ref::<EngineError>() {
                Some(typed) => (typed.is_transient(), typed.retry_after),
                None => (false, None),
            };
            let Decision::Wait(delay) = self.policy.decide(attempt, transient, retry_after) else {
                break;
            };
            tracing::warn!(
                attempt,
                max = self.policy.max_attempts,
                wait_ms = delay.as_millis() as u64,
                error = %error,
                "the embedding request failed; trying again"
            );
            tokio::time::sleep(delay).await;
            attempt += 1;
            outcome = self.inner.embed(texts.clone(), role).await;
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::time::Duration;

    use super::*;
    use crate::shared::api::error::EngineErrorKind;

    /// What one request to the scripted embedder does.
    enum Step {
        /// Answers a vector per text — the text's length, so that a vector can be
        /// traced back to the text it was made from.
        Answer,
        /// Answers one vector fewer than it was asked for.
        Short,
        Fail(EngineError),
        /// A failure that carries no verdict.
        Untyped,
    }

    /// An embedder that plays a scripted step per request and keeps **what it
    /// was asked**: the sizes of the requests are the assertion a split is
    /// judged by, and their count the one a retry is.
    struct Scripted {
        steps: Mutex<VecDeque<Step>>,
        asked: Mutex<Vec<Vec<String>>>,
    }

    impl Scripted {
        fn new(steps: Vec<Step>) -> Arc<Self> {
            Arc::new(Self {
                steps: Mutex::new(steps.into()),
                asked: Mutex::new(Vec::new()),
            })
        }

        fn sizes(&self) -> Vec<usize> {
            self.asked.lock().unwrap().iter().map(Vec::len).collect()
        }
    }

    #[async_trait::async_trait]
    impl Embedder for Scripted {
        async fn embed(&self, texts: Vec<String>, _role: EmbedRole) -> Result<Vec<Vec<f32>>> {
            self.asked.lock().unwrap().push(texts.clone());
            // Past the end of the script every request is answered: a test that
            // scripts failures says how many it means.
            let step = self.steps.lock().unwrap().pop_front();
            let vector = |t: &String| vec![t.len() as f32];
            match step.unwrap_or(Step::Answer) {
                Step::Answer => Ok(texts.iter().map(vector).collect()),
                Step::Short => Ok(texts.iter().skip(1).map(vector).collect()),
                Step::Fail(error) => Err(error.into()),
                Step::Untyped => anyhow::bail!("decoding embeddings response"),
            }
        }
    }

    fn status(code: u16, retry_after: Option<Duration>) -> EngineError {
        EngineError {
            kind: EngineErrorKind::Status,
            status: Some(code),
            retry_after,
            message: format!("embedding server returned status {code}"),
        }
    }

    /// Texts of lengths 1, 2, 3, … — each one's vector names it.
    fn texts(n: usize) -> Vec<String> {
        (1..=n).map(|len| "x".repeat(len)).collect()
    }

    fn lengths(vectors: &[Vec<f32>]) -> Vec<usize> {
        vectors.iter().map(|v| v[0] as usize).collect()
    }

    /// The whole of the split: requests of at most the cap, every text sent
    /// once, and the answer in the order the texts were given — the last part
    /// being whatever is left.
    #[tokio::test]
    async fn a_long_input_is_sent_in_parts_and_answered_in_order() {
        let inner = Scripted::new(vec![]);
        let capped = BatchedEmbedder::with_cap(inner.clone(), 4);
        let vectors = capped.embed(texts(10), EmbedRole::Passage).await.unwrap();
        assert_eq!(inner.sizes(), [4, 4, 2]);
        assert_eq!(lengths(&vectors), (1..=10).collect::<Vec<_>>());

        // Exactly the cap, and one more: the boundary on both sides.
        for (n, sizes) in [(4, vec![4]), (5, vec![4, 1]), (8, vec![4, 4]), (1, vec![1])] {
            let inner = Scripted::new(vec![]);
            let capped = BatchedEmbedder::with_cap(inner.clone(), 4);
            let vectors = capped.embed(texts(n), EmbedRole::Query).await.unwrap();
            assert_eq!(inner.sizes(), sizes, "{n} texts");
            assert_eq!(lengths(&vectors), (1..=n).collect::<Vec<_>>(), "{n} texts");
        }
    }

    /// Nothing to embed is still one request: what the endpoint says about an
    /// empty input is its to say, and a decorator that answered for it would
    /// hide a server that is down.
    #[tokio::test]
    async fn an_empty_input_is_passed_on_as_it_is() {
        let inner = Scripted::new(vec![]);
        let capped = BatchedEmbedder::with_cap(inner.clone(), 4);
        assert!(
            capped
                .embed(vec![], EmbedRole::Passage)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(inner.sizes(), [0]);
    }

    /// The cap every embedder gets is the constant, and not a number of the
    /// constructor's own.
    #[tokio::test]
    async fn every_embedder_gets_the_measured_cap() {
        let inner = Scripted::new(vec![]);
        let capped = BatchedEmbedder::wrap(inner.clone());
        capped
            .embed(texts(MAX_INPUTS + 1), EmbedRole::Passage)
            .await
            .unwrap();
        assert_eq!(inner.sizes(), [MAX_INPUTS, 1]);
    }

    /// A part that comes back short is an error, not a shorter answer: every
    /// vector after it would belong to the text before. In a single request as
    /// much as in a part.
    #[tokio::test]
    async fn an_answer_with_a_vector_missing_is_refused() {
        let inner = Scripted::new(vec![Step::Answer, Step::Short]);
        let capped = BatchedEmbedder::with_cap(inner.clone(), 4);
        let refused = capped
            .embed(texts(10), EmbedRole::Passage)
            .await
            .unwrap_err();
        assert_eq!(
            refused.to_string(),
            "the embedder answered 3 vectors for 4 inputs"
        );
        assert_eq!(inner.sizes(), [4, 4], "and the third part is not asked for");

        let inner = Scripted::new(vec![Step::Short]);
        let capped = BatchedEmbedder::with_cap(inner, 4);
        assert!(capped.embed(texts(3), EmbedRole::Passage).await.is_err());
    }

    /// A part that fails fails the whole, with the endpoint's own words.
    #[tokio::test]
    async fn a_failed_part_fails_the_request() {
        let inner = Scripted::new(vec![Step::Answer, Step::Fail(status(400, None))]);
        let capped = BatchedEmbedder::with_cap(inner.clone(), 4);
        let failed = capped
            .embed(texts(10), EmbedRole::Passage)
            .await
            .unwrap_err();
        assert_eq!(failed.to_string(), "embedding server returned status 400");
        assert_eq!(inner.sizes(), [4, 4]);
    }

    fn quick() -> RetryPolicy {
        RetryPolicy {
            jitter: 0.0,
            ..RetryPolicy::default()
        }
    }

    /// A failure that passes is followed by another attempt, with the same
    /// texts, after the policy's wait — and the answer is the one that came.
    #[tokio::test(start_paused = true)]
    async fn a_passing_failure_is_retried_with_the_same_texts() {
        for code in [429, 503, 502, 529] {
            let inner = Scripted::new(vec![Step::Fail(status(code, None))]);
            let retrying = RetryEmbedder::with_policy(inner.clone(), quick());
            let started = tokio::time::Instant::now();
            let vectors = retrying.embed(texts(3), EmbedRole::Query).await.unwrap();
            assert_eq!(lengths(&vectors), [1, 2, 3]);
            assert_eq!(inner.sizes(), [3, 3], "{code}");
            assert_eq!(started.elapsed(), Duration::from_secs(1), "{code}");
        }
    }

    /// No answer at all — the connection refused, or dead before the status
    /// line — passes as well.
    #[tokio::test(start_paused = true)]
    async fn no_answer_is_retried() {
        let lost = EngineError {
            kind: EngineErrorKind::Transport,
            status: None,
            retry_after: None,
            message: "connection closed".into(),
        };
        let inner = Scripted::new(vec![Step::Fail(lost)]);
        let retrying = RetryEmbedder::with_policy(inner.clone(), quick());
        assert!(retrying.embed(texts(2), EmbedRole::Passage).await.is_ok());
        assert_eq!(inner.sizes(), [2, 2]);
    }

    /// Three attempts, then the last failure as it was said.
    #[tokio::test(start_paused = true)]
    async fn the_attempts_are_counted_and_the_last_failure_is_the_one_reported() {
        let inner = Scripted::new(vec![
            Step::Fail(status(429, None)),
            Step::Fail(status(503, None)),
            Step::Fail(status(502, None)),
            Step::Answer,
        ]);
        let retrying = RetryEmbedder::with_policy(inner.clone(), quick());
        let started = tokio::time::Instant::now();
        let failed = retrying
            .embed(texts(1), EmbedRole::Passage)
            .await
            .unwrap_err();
        assert_eq!(failed.to_string(), "embedding server returned status 502");
        assert_eq!(inner.sizes().len(), 3, "and not a fourth");
        assert_eq!(started.elapsed(), Duration::from_secs(1 + 2));
    }

    /// A refusal is a statement about the request: a wrong key, an input over
    /// the model's window, a batch over the provider's count. Asked again it
    /// would be refused again.
    #[tokio::test(start_paused = true)]
    async fn a_refusal_is_not_retried() {
        for code in [400, 401, 403, 404, 422] {
            let inner = Scripted::new(vec![Step::Fail(status(code, None))]);
            let retrying = RetryEmbedder::with_policy(inner.clone(), quick());
            assert!(retrying.embed(texts(1), EmbedRole::Passage).await.is_err());
            assert_eq!(inner.sizes().len(), 1, "{code}");
        }
        let inner = Scripted::new(vec![Step::Untyped]);
        let retrying = RetryEmbedder::with_policy(inner.clone(), quick());
        assert!(retrying.embed(texts(1), EmbedRole::Passage).await.is_err());
        assert_eq!(inner.sizes().len(), 1, "a failure without a verdict");
    }

    /// The provider's own wait is the one kept — and one longer than the policy
    /// will hide is not waited out at all.
    #[tokio::test(start_paused = true)]
    async fn the_providers_wait_is_honoured_within_reason() {
        let asked = Duration::from_secs(7);
        let inner = Scripted::new(vec![Step::Fail(status(429, Some(asked)))]);
        let retrying = RetryEmbedder::with_policy(inner.clone(), quick());
        let started = tokio::time::Instant::now();
        assert!(retrying.embed(texts(1), EmbedRole::Passage).await.is_ok());
        assert_eq!(started.elapsed(), asked);

        let minutes = Duration::from_secs(600);
        let inner = Scripted::new(vec![Step::Fail(status(429, Some(minutes)))]);
        let retrying = RetryEmbedder::with_policy(inner.clone(), quick());
        let started = tokio::time::Instant::now();
        assert!(retrying.embed(texts(1), EmbedRole::Passage).await.is_err());
        assert_eq!(inner.sizes().len(), 1);
        assert_eq!(started.elapsed(), Duration::ZERO);
    }

    /// The two together, in the order the supervisor stacks them: the split
    /// outside, so a part that failed is asked for again by itself and the
    /// parts that answered are not.
    #[tokio::test(start_paused = true)]
    async fn a_part_is_retried_by_itself() {
        let inner = Scripted::new(vec![Step::Answer, Step::Fail(status(429, None))]);
        let retrying: Arc<dyn Embedder> =
            Arc::new(RetryEmbedder::with_policy(inner.clone(), quick()));
        let stacked = BatchedEmbedder::with_cap(retrying, 4);
        let vectors = stacked.embed(texts(10), EmbedRole::Passage).await.unwrap();
        assert_eq!(inner.sizes(), [4, 4, 4, 2]);
        assert_eq!(lengths(&vectors), (1..=10).collect::<Vec<_>>());
        let asked = inner.asked.lock().unwrap();
        assert_eq!(asked[1], asked[2], "the second part, twice");
    }
}
