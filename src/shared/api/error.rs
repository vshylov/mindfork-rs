//! A typed engine error — what the transport and the provider actually said, in a
//! shape the layers above can **decide** on rather than only display.
//!
//! Every client used to end a failed request with `anyhow::bail!("… status
//! {status}: {body}")`, so the status code and the provider's `Retry-After`
//! survived only as substrings of a sentence. Anything wanting to act on them —
//! the retry decorator (stage 2 of docs/research/cloud-retry-backoff.md) above
//! all — would have had to re-parse prose. [`EngineError`] keeps the decision
//! inputs as fields while rendering **byte-for-byte the same text as before**, so
//! the message the user reads, the overflow markers
//! ([`is_context_overflow`](crate::features::compaction::is_context_overflow))
//! and every test that pins the wording are untouched.
//!
//! This is also where the two classifications live, in one place each: which HTTP
//! statuses are worth another attempt ([`EngineError::is_transient`]) and which
//! provider error *names* are ([`stream_error_transient`]).

use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::HeaderMap;

/// What the subject of the error message is called. The wording is
/// per-client and predates this module — kept identical so error text (and the
/// tests reading it) does not change.
pub const SUBJECT_ENGINE: &str = "engine";
/// See [`SUBJECT_ENGINE`].
pub const SUBJECT_ANTHROPIC: &str = "engine (Anthropic)";
/// See [`SUBJECT_ENGINE`].
pub const SUBJECT_GEMINI: &str = "engine (Gemini)";
/// See [`SUBJECT_ENGINE`].
pub const SUBJECT_RESPONSES: &str = "engine (OpenAI Responses)";
/// See [`SUBJECT_ENGINE`].
pub const SUBJECT_EMBEDDER: &str = "embedder";

/// HTTP statuses that mean "the provider shed this request; another attempt can
/// succeed".
///
/// The set converges across every provider we speak to (verified 2026-08-12,
/// docs/research/cloud-retry-backoff.md §2): `429` rate limits, `500`/`502`/
/// `503`/`504` server-side failures, `408` request timeout, and Anthropic's
/// dedicated `529 overloaded_error`. Everything else — every other 4xx — is a
/// statement about the *request*, which a retry cannot change.
///
/// Two absences are deliberate. `400` covers llama.cpp's
/// `exceed_context_size_error`, whose answer is compaction (spec §6.7), not
/// another attempt. `402`/`403` and OpenAI's quota-flavoured `429` bodies are
/// billing, and "retrying billing, spend, or quota errors won't restore API
/// access" — telling those apart from a rate limit needs the body, and that
/// discrimination was deliberately left out of v1 (fork F5).
pub const RETRYABLE_STATUSES: &[u16] = &[408, 429, 500, 502, 503, 504, 529];

/// Provider error *names* that mean the same thing as a [`RETRYABLE_STATUSES`]
/// code, for failures that arrive **inside** an already-open stream, where there
/// is no status line left to read.
///
/// Anthropic sends its 529 as an in-stream `overloaded_error`; llama.cpp and
/// OpenAI-compatible proxies put an OpenAI-shaped error object in the stream;
/// Gemini repeats its canonical `status` (`UNAVAILABLE`/`RESOURCE_EXHAUSTED`/
/// `INTERNAL`). `exceed_context_size_error` is deliberately **not** here, for the
/// reason given on [`RETRYABLE_STATUSES`].
const TRANSIENT_ERROR_NAMES: &[&str] = &[
    // Anthropic (docs/en/api/errors): 500 and 529 respectively.
    "api_error",
    "overloaded_error",
    "rate_limit_error",
    "timeout_error",
    // llama.cpp / OpenAI-compatible.
    "unavailable_error",
    "server_error",
    "rate_limit_exceeded",
    "service_unavailable",
    // Google's canonical codes (google.rpc.Code names).
    "internal",
    "internal_error",
    "unavailable",
    "resource_exhausted",
];

/// How a request failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineErrorKind {
    /// No HTTP response at all — DNS, connect, TLS, a connection that died before
    /// the status line. Always worth another attempt.
    Transport,
    /// The provider answered, with a non-2xx status.
    Status,
}

/// A failed engine request.
///
/// [`Display`](std::fmt::Display) is the whole user-facing text, so
/// `err.to_string()` at a call site reads exactly as it did before this type
/// existed; the fields are what a caller decides on.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct EngineError {
    pub kind: EngineErrorKind,
    /// The HTTP status, when there was a response.
    pub status: Option<u16>,
    /// How long the provider asked us to wait before retrying, when it said so.
    ///
    /// Sources differ by provider and are all read here: a `retry-after` header
    /// in seconds (OpenAI, Anthropic), OpenAI's millisecond variant
    /// `retry-after-ms`, or — Gemini having no header at all — a
    /// `google.rpc.RetryInfo` hint inside the 429 body. Nothing in stage 1 reads
    /// this field; the retry decorator is its consumer.
    pub retry_after: Option<Duration>,
    /// The full message, rendered once at construction.
    pub message: String,
}

impl EngineError {
    /// The request never produced a response.
    ///
    /// The message is the **whole cause chain**: `reqwest::Error`'s own `Display`
    /// names the request but keeps *why* it failed in its source, and since the
    /// old call sites wrapped it in `.with_context(|| format!("POST {url}"))` —
    /// and `anyhow`'s `Display` prints only the outermost context — a refused
    /// connection used to reach the feed as a bare URL with no reason attached.
    pub fn transport(err: &reqwest::Error) -> Self {
        Self {
            kind: EngineErrorKind::Transport,
            status: None,
            retry_after: None,
            message: chain_text(err),
        }
    }

    /// A non-2xx response. `subject` is one of the `SUBJECT_*` constants; `body`
    /// is the response body, already read.
    pub fn status(subject: &str, status: StatusCode, headers: &HeaderMap, body: &str) -> Self {
        // Don't swallow the error body: providers put the reason in JSON
        // (`{"error":{"message":…}}`), and without it "error status" is useless.
        // Long bodies are truncated — the same 500 characters as before.
        let detail: String = body.trim().chars().take(500).collect();
        let message = if detail.is_empty() {
            format!("{subject} returned status {status}")
        } else {
            format!("{subject} returned status {status}: {detail}")
        };
        Self {
            kind: EngineErrorKind::Status,
            status: Some(status.as_u16()),
            retry_after: retry_after_from_headers(headers).or_else(|| retry_after_from_body(body)),
            message,
        }
    }

    /// Is another attempt worth making?
    pub fn is_transient(&self) -> bool {
        match self.kind {
            EngineErrorKind::Transport => true,
            EngineErrorKind::Status => self.status.is_some_and(|s| RETRYABLE_STATUSES.contains(&s)),
        }
    }
}

/// Reads a response's status, returning the response itself when it is a success.
///
/// One helper for all four clients, which each carried their own copy of this
/// block. Consumes the response on the failure path (the body has to be read to
/// be reported).
pub async fn check_status(
    subject: &str,
    response: reqwest::Response,
) -> Result<reqwest::Response, EngineError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let headers = response.headers().clone();
    let body = response.text().await.unwrap_or_default();
    let err = EngineError::status(subject, status, &headers, &body);
    tracing::warn!(
        %subject,
        %status,
        retry_after = ?err.retry_after,
        transient = err.is_transient(),
        body = %err.message,
        "engine returned an error status"
    );
    Err(err)
}

/// Would another attempt help, judging by a provider's own error name and code?
///
/// For failures that arrive inside an open stream — see [`TRANSIENT_ERROR_NAMES`].
/// Matching is exact on the lowercased name, so `exceed_context_size_error` can
/// never be read as a transient `*_error`.
pub fn stream_error_transient(name: &str, status: Option<u16>) -> bool {
    if status.is_some_and(|s| RETRYABLE_STATUSES.contains(&s)) {
        return true;
    }
    let name = name.trim().to_ascii_lowercase();
    TRANSIENT_ERROR_NAMES.contains(&name.as_str())
}

/// The text shown for an in-stream error, from the provider's error *name* and its
/// prose message — either of which can be absent.
///
/// The name is the load-bearing half: `overloaded_error` says precisely what
/// happened, while the message is often empty. A message that named neither would
/// be the defect this whole change exists to fix, so the last arm still says
/// something.
pub fn stream_error_text(name: &str, message: &str) -> String {
    match (name.trim(), message.trim()) {
        ("", "") => "unknown engine error".to_string(),
        ("", msg) => msg.to_string(),
        (name, "") => name.to_string(),
        (name, msg) => format!("{name}: {msg}"),
    }
}

/// An error and every cause under it, on one line.
///
/// Duplicates are skipped: `reqwest::Error`'s `Display` sometimes already quotes
/// its source, and repeating it reads as two failures.
pub fn chain_text(err: &dyn std::error::Error) -> String {
    let mut out = err.to_string();
    let mut cur = err.source();
    while let Some(e) = cur {
        let s = e.to_string();
        if !out.contains(&s) {
            out.push_str(": ");
            out.push_str(&s);
        }
        cur = e.source();
    }
    out
}

/// `Retry-After` as the providers actually send it.
///
/// OpenAI and Anthropic both send the standard header in **seconds** (Anthropic's
/// docs are explicit that an earlier retry will fail); OpenAI additionally sends
/// `retry-after-ms`, which its own SDK prefers, so we do too. The RFC's
/// HTTP-date form is not produced by any provider we speak to and is therefore
/// read as "no hint" rather than parsed.
fn retry_after_from_headers(headers: &HeaderMap) -> Option<Duration> {
    if let Some(ms) = header_number(headers, "retry-after-ms") {
        return Some(Duration::from_secs_f64(ms / 1000.0));
    }
    header_number(headers, "retry-after").map(Duration::from_secs_f64)
}

/// A header parsed as a non-negative number, or `None` if it is missing, not
/// ASCII, or not a number (an HTTP-date lands here).
fn header_number(headers: &HeaderMap, name: &str) -> Option<f64> {
    let raw = headers.get(name)?.to_str().ok()?;
    let value: f64 = raw.trim().parse().ok()?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

/// Gemini's retry hint, which lives in the body rather than a header.
///
/// A 429 carries `error.details[]` with a `google.rpc.RetryInfo` whose
/// `retryDelay` is a protobuf duration (`"7s"`). It is **undocumented** — but
/// Google's own gemini-cli parses it, and it is the only hint Gemini gives, so it
/// is read best-effort: anything unexpected is "no hint", never an error.
fn retry_after_from_body(body: &str) -> Option<Duration> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let details = value.get("error")?.get("details")?.as_array()?;
    details
        .iter()
        .find_map(|d| parse_proto_duration(d.get("retryDelay")?.as_str()?))
}

/// A protobuf `Duration` in JSON: decimal seconds with an `s` suffix.
fn parse_proto_duration(raw: &str) -> Option<Duration> {
    let secs: f64 = raw.trim().strip_suffix('s')?.parse().ok()?;
    (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    /// The whole point of keeping `Display`: the text must be what the previous
    /// per-client `bail!`s produced, or the overflow markers and the locale
    /// messages built on them shift under us.
    #[test]
    fn status_message_matches_the_pre_existing_wording() {
        let err = EngineError::status(
            SUBJECT_ENGINE,
            StatusCode::TOO_MANY_REQUESTS,
            &HeaderMap::new(),
            "  {\"error\":{\"message\":\"slow down\"}}  ",
        );
        assert_eq!(
            err.to_string(),
            "engine returned status 429 Too Many Requests: {\"error\":{\"message\":\"slow down\"}}"
        );
        // An empty body drops the colon entirely, as before.
        let bare = EngineError::status(
            SUBJECT_ANTHROPIC,
            StatusCode::BAD_GATEWAY,
            &HeaderMap::new(),
            "   ",
        );
        assert_eq!(
            bare.to_string(),
            "engine (Anthropic) returned status 502 Bad Gateway"
        );
    }

    #[test]
    fn long_bodies_are_truncated_to_500_chars() {
        let body = "x".repeat(900);
        let err = EngineError::status(
            SUBJECT_GEMINI,
            StatusCode::INTERNAL_SERVER_ERROR,
            &HeaderMap::new(),
            &body,
        );
        // The message is the prefix plus exactly 500 body characters.
        assert_eq!(err.message.matches('x').count(), 500);
    }

    #[test]
    fn transient_statuses_are_exactly_the_documented_set() {
        for status in [408u16, 429, 500, 502, 503, 504, 529] {
            let err = EngineError {
                kind: EngineErrorKind::Status,
                status: Some(status),
                retry_after: None,
                message: String::new(),
            };
            assert!(err.is_transient(), "{status} must be retryable");
        }
        // A request-level verdict cannot be retried away — 400 in particular is
        // llama.cpp's context overflow, whose answer is compaction.
        for status in [400u16, 401, 402, 403, 404, 405, 413, 422] {
            let err = EngineError {
                kind: EngineErrorKind::Status,
                status: Some(status),
                retry_after: None,
                message: String::new(),
            };
            assert!(!err.is_transient(), "{status} must not be retryable");
        }
    }

    #[test]
    fn transport_failures_are_always_transient() {
        let err = EngineError {
            kind: EngineErrorKind::Transport,
            status: None,
            retry_after: None,
            message: String::new(),
        };
        assert!(err.is_transient());
    }

    #[test]
    fn retry_after_seconds_header_is_read() {
        let err = EngineError::status(
            SUBJECT_ENGINE,
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[("retry-after", "7")]),
            "",
        );
        assert_eq!(err.retry_after, Some(Duration::from_secs(7)));
    }

    /// OpenAI sends both; the millisecond variant is the precise one and its own
    /// SDK prefers it.
    #[test]
    fn millisecond_header_wins_over_seconds() {
        let err = EngineError::status(
            SUBJECT_ENGINE,
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[("retry-after", "9"), ("retry-after-ms", "1500")]),
            "",
        );
        assert_eq!(err.retry_after, Some(Duration::from_millis(1500)));
    }

    #[test]
    fn http_date_retry_after_reads_as_no_hint() {
        let err = EngineError::status(
            SUBJECT_ENGINE,
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[("retry-after", "Wed, 12 Aug 2026 07:28:00 GMT")]),
            "",
        );
        assert_eq!(err.retry_after, None);
    }

    /// Gemini has no header — the hint is a `google.rpc.RetryInfo` in the body.
    #[test]
    fn gemini_retry_info_in_the_body_is_read() {
        let body = r#"{"error":{"code":429,"message":"quota","status":"RESOURCE_EXHAUSTED",
            "details":[{"@type":"type.googleapis.com/google.rpc.QuotaFailure"},
                       {"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"7.5s"}]}}"#;
        let err = EngineError::status(
            SUBJECT_GEMINI,
            StatusCode::TOO_MANY_REQUESTS,
            &HeaderMap::new(),
            body,
        );
        assert_eq!(err.retry_after, Some(Duration::from_secs_f64(7.5)));
        assert!(err.is_transient());
    }

    #[test]
    fn a_body_without_retry_info_yields_no_hint() {
        let err = EngineError::status(
            SUBJECT_GEMINI,
            StatusCode::BAD_REQUEST,
            &HeaderMap::new(),
            r#"{"error":{"code":400,"message":"bad","status":"INVALID_ARGUMENT"}}"#,
        );
        assert_eq!(err.retry_after, None);
        assert!(!err.is_transient());
    }

    #[test]
    fn stream_error_names_are_matched_exactly() {
        assert!(stream_error_transient("overloaded_error", None));
        assert!(stream_error_transient("  API_Error  ", None));
        assert!(stream_error_transient("UNAVAILABLE", None));
        // Names that are *not* on the list, including one that also ends in
        // `_error` and one whose answer is compaction rather than a retry.
        assert!(!stream_error_transient("exceed_context_size_error", None));
        assert!(!stream_error_transient("invalid_request_error", None));
        assert!(!stream_error_transient("", None));
        // The input is an error **name**, and matching it exactly is what enforces
        // that: hand this the composed text from `stream_error_text` — the shape a
        // call site could plausibly confuse it with — and it answers "not
        // transient" rather than finding the name inside the sentence. Failing
        // closed surfaces the error instead of silently retrying on a misread
        // input, and this case is the one that makes the exactness observable at
        // all (mutation-tested: loosening it to `contains` survives every other
        // fixture here).
        assert!(!stream_error_transient(
            &stream_error_text("overloaded_error", "Overloaded"),
            None
        ));
        // A numeric code decides on its own when the name says nothing useful.
        assert!(stream_error_transient("weird", Some(503)));
        assert!(!stream_error_transient("weird", Some(400)));
    }

    #[test]
    fn stream_error_text_always_says_something() {
        assert_eq!(
            stream_error_text("overloaded_error", ""),
            "overloaded_error"
        );
        assert_eq!(stream_error_text("", "Overloaded"), "Overloaded");
        assert_eq!(
            stream_error_text("overloaded_error", "Overloaded"),
            "overloaded_error: Overloaded"
        );
        assert_eq!(stream_error_text("  ", " "), "unknown engine error");
    }

    #[test]
    fn chain_text_appends_causes_without_repeating_them() {
        #[derive(Debug)]
        struct Inner;
        impl std::fmt::Display for Inner {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "connection refused")
            }
        }
        impl std::error::Error for Inner {}

        #[derive(Debug)]
        struct Outer(Inner, &'static str);
        impl std::fmt::Display for Outer {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.1)
            }
        }
        impl std::error::Error for Outer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }

        assert_eq!(
            chain_text(&Outer(Inner, "error sending request")),
            "error sending request: connection refused"
        );
        // Already quoted by the outer Display → not appended twice.
        assert_eq!(
            chain_text(&Outer(Inner, "failed: connection refused")),
            "failed: connection refused"
        );
    }
}
