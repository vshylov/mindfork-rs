//! Shared HTTP wiring for the engine clients: one `reqwest::Client` builder and a
//! `send` that honours cancellation.
//!
//! Both exist because of the same gap. Every engine client built a bare
//! `reqwest::Client::new()` — which has **no timeout of any kind** — and awaited
//! `send()` *outside* the `select!` on the cancellation token, so the token only
//! took effect once SSE events were already flowing. A host that accepts a
//! connection and then answers nothing (a firewall drop, a dead VPN, a wrong
//! port) therefore left the turn generating forever, with `Esc` moving it to
//! `Cancelling` and no way out. See docs/research/cloud-retry-backoff.md §1.3.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::contract::{ChatChunk, ChatStream, FinishReason};
use super::error::EngineError;

/// How long to wait for a TCP/TLS connection to be established.
///
/// Deliberately **only** a connect timeout. An SSE stream legitimately stays open
/// for minutes, and a local `llama-server` prefilling a long prompt can be silent
/// for tens of seconds before the first token, so a total or idle timeout would
/// cut healthy generations — which is why none was ever set. What this bounds is
/// the one case with no healthy interpretation: nobody is answering at all.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The `reqwest::Client` every engine client uses.
pub fn engine_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        // The builder only fails when the TLS backend cannot be initialised, and
        // `Client::new()` panics on exactly that — so this is the previous
        // behaviour rather than a silent downgrade.
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Sends a request while honouring `cancel`.
///
/// `Ok(None)` means the token fired before a response arrived — the caller
/// answers with [`cancelled_stream`], never with an error: a cancellation is the
/// user's own doing, and it must not read as a failed turn (nor, once the retry
/// decorator lands, be retried).
pub async fn send_cancellable(
    rb: reqwest::RequestBuilder,
    cancel: &CancellationToken,
) -> Result<Option<reqwest::Response>, EngineError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Ok(None),
        sent = rb.send() => match sent {
            Ok(response) => Ok(Some(response)),
            Err(err) => Err(EngineError::transport(&err)),
        },
    }
}

/// A stream that only says "cancelled" — the answer when the token fired before
/// the response arrived.
pub fn cancelled_stream() -> ChatStream {
    Box::pin(futures_util::stream::once(async {
        ChatChunk::Finished(FinishReason::Cancelled)
    }))
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;

    use super::*;

    #[tokio::test]
    async fn cancelled_stream_yields_one_cancelled_finish() {
        let mut s = cancelled_stream();
        assert_eq!(
            s.next().await,
            Some(ChatChunk::Finished(FinishReason::Cancelled))
        );
        assert_eq!(s.next().await, None);
    }

    /// A URL nothing listens on: bind a port, learn it, then drop the listener.
    /// Deliberately local — a connect to a *closed* local port is refused at once,
    /// while an unroutable address would make every test below wait out
    /// [`CONNECT_TIMEOUT`].
    fn dead_url() -> String {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        format!("http://127.0.0.1:{port}/v1/chat/completions")
    }

    /// An already-cancelled token must win over the send, and must not touch the
    /// network at all: `select!` is `biased`, so the cancel arm is polled first and
    /// the send future is dropped unpolled.
    #[tokio::test]
    async fn a_cancelled_token_short_circuits_the_send() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let client = engine_client();
        let result = send_cancellable(client.get(dead_url()), &cancel).await;
        assert!(matches!(result, Ok(None)));
    }

    /// A refused connection becomes an [`EngineError`] that is transient and
    /// carries a reason — the reason in particular, since wrapping the failure in
    /// `.with_context(|| format!("POST {url}"))` used to hide it behind the URL.
    #[tokio::test]
    async fn a_refused_connection_is_a_transient_error_with_a_reason() {
        let client = engine_client();
        let cancel = CancellationToken::new();
        let err = send_cancellable(client.get(dead_url()), &cancel)
            .await
            .expect_err("nothing is listening on that port");
        assert!(err.is_transient(), "a transport failure is worth a retry");
        assert_eq!(err.status, None);
        assert!(
            err.message.len() > "error sending request".len(),
            "the cause must reach the message, not only the log: {}",
            err.message
        );
    }
}
