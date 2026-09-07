//! The shared "silent" agentic loop for background tasks (self-model auto-reflection
//! and notes auto-consolidation). Both tasks are a mini agentic loop with no UI streaming:
//! stream → a call accumulator → executing allowed tools → the next
//! round; tolerant of `Thoughts`/`ThoughtsSignature`/`Usage` (ignored). This body used to
//! be duplicated verbatim in `reflection.rs` and `consolidation.rs` (differing only in
//! limits and the log label) — now it lives here once. The main generation loop is deliberately
//! **not** touched: it has UI streaming, control-flow tools, Anthropic thinking
//! signatures, usage, effects — its complexity doesn't pay for a shared sink right now.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::events::BackgroundKind;
use crate::entities::profile::ToolId;
use crate::features::tools::{ToolContext, ToolRegistry};
use crate::shared::api::contract::ChatStream;
use crate::shared::api::{
    ApiMessage, ApiToolCall, ChatChunk, ChatRequest, Embedder, EngineBackend, FinishReason,
    ToolCallAccumulator,
};
use crate::shared::i18n::Locale;
use crate::shared::session_budget::{Reservation, SessionBudget};
use crate::shared::storage::Storage;

/// An async layer over the background task's digest (§A2): a semantic comparison of
/// the self-description's (`summary`) paragraphs with observations (`@self`). Computed **in
/// the task**, before the loop — embedding summary paragraphs isn't available in the orchestrator's
/// synchronous handler. See docs/history/self-model-consolidation.md §A2.
pub(super) struct SummarySemantics {
    pub embedder: Arc<dyn Embedder>,
    pub storage: Arc<Storage>,
    pub profile_id: Uuid,
    pub loc: &'static Locale,
}

/// Is it time to run the periodic background task: the feature is enabled (`every > 0`) and
/// enough replies have accumulated. A pure function — testable. Shared by reflection and
/// consolidation.
pub(super) fn due(count: u32, every: usize) -> bool {
    every > 0 && (count as usize) >= every
}

/// Launch parameters for the silent background task.
pub(super) struct SilentLoop {
    pub backend: Arc<dyn EngineBackend>,
    pub registry: Arc<ToolRegistry>,
    pub ctx: ToolContext,
    pub request: ChatRequest,
    /// Allowed tools (a guard against calling something outside the task's set).
    pub allowed: Vec<ToolId>,
    pub cancel: CancellationToken,
    /// A backstop against looping (the round count).
    pub max_rounds: u32,
    /// The time limit for the whole task.
    pub timeout: Duration,
    /// A label for diagnostic logs ("auto-reflection"/"auto-consolidation").
    pub label: &'static str,
    /// The profile (for logs).
    pub profile_id: Uuid,
    /// The task kind — goes into `done_tx` along with the outcome (the loop handles it in one branch).
    pub kind: BackgroundKind,
    /// A single outcome channel: `(kind, Ok(()))` on success, `(kind, Err(reason))` on
    /// an error/timeout.
    pub done_tx: UnboundedSender<(BackgroundKind, Result<(), String>)>,
    /// An optional async layer over the digest, computed in the task BEFORE the loop
    /// (embedding summary paragraphs isn't available in the synchronous handler): the result
    /// is appended to the request's first user message. See
    /// docs/history/self-model-consolidation.md §A2.
    pub summary_semantics: Option<SummarySemantics>,
}

/// Starts the silent background task: a mini agentic loop under a timeout. On completion
/// sends the outcome into `done_tx` (clear the "running …" flag and provide observability: a run of
/// failures → one UI error). Tools write directly into `Storage`; the chat/feed aren't touched.
pub(super) fn spawn_silent_loop(spawn: SilentLoop) {
    let SilentLoop {
        backend,
        registry,
        ctx,
        mut request,
        allowed,
        cancel,
        max_rounds,
        timeout,
        label,
        profile_id,
        kind,
        done_tx,
        summary_semantics,
    } = spawn;

    tokio::spawn(async move {
        // A2: the async layer over the digest (summary↔observation semantics) — compute it BEFORE
        // the loop and append to the first user message (embedding summary paragraphs in a
        // synchronous handler isn't available). See docs/history/self-model-consolidation.md §A2.
        if let Some(ss) = &summary_semantics
            && let Some(section) = crate::features::tools::notes::summary_observation_overlaps(
                &ss.storage,
                ss.embedder.as_ref(),
                ss.profile_id,
                ss.loc,
            )
            .await
            && let Some(first) = request.messages.first_mut()
        {
            first.content.push_str("\n\n");
            first.content.push_str(&section);
        }
        let run = run_rounds(
            &backend,
            &registry,
            &ctx,
            &mut request,
            &allowed,
            &cancel,
            max_rounds,
            super::background::lane_label(kind),
        );
        let outcome: Result<(), String> = match tokio::time::timeout(timeout, run).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => {
                tracing::warn!(%profile_id, "{label}: error: {e}");
                Err(e.to_string())
            }
            Err(_) => {
                cancel.cancel();
                tracing::warn!(%profile_id, "{label}: time limit exceeded");
                Err(ctx.loc.t("loop.time_limit_exceeded").to_string())
            }
        };
        let _ = done_tx.send((kind, outcome));
    });
}

/// The mini agentic loop's body: rounds of stream→calls→execution up to `max_rounds` or
/// the first round with no calls. Only allows tools from `allowed`.
///
/// Every round streams under the **silent lane** of the app's session budget
/// (`ctx.sessions`; docs/research/silent-tasks-budget.md §4.1–§4.2): a
/// permit the silent tasks share one of, and under a pool a reservation —
/// the calibrated estimate of the request, floored by the last round's exact
/// size plus what it generated, plus the reply cap — held for the stream and
/// dropped before the round's tools run, as a turn's loop does. A wait
/// cancelled (the app is quitting) ends the task quietly: nothing ran, so
/// nothing failed.
#[allow(clippy::too_many_arguments)]
async fn run_rounds(
    backend: &Arc<dyn EngineBackend>,
    registry: &Arc<ToolRegistry>,
    ctx: &ToolContext,
    request: &mut ChatRequest,
    allowed: &[ToolId],
    cancel: &CancellationToken,
    max_rounds: u32,
    lane: &'static str,
) -> Result<(), anyhow::Error> {
    let mut round: u32 = 0;
    let mut last_exact: u64 = 0;
    loop {
        let estimate = super::generation::estimate_prompt_tokens(request);
        let (text, calls, reason, usage) = {
            let Ok(_lane) = lane_reservation(
                ctx.sessions.as_deref(),
                request,
                estimate,
                last_exact,
                cancel,
                lane,
            )
            .await
            else {
                return Ok(());
            };
            let stream = backend.chat_stream(request.clone(), cancel.clone()).await?;
            read_round(stream).await
        };
        if let Some(u) = usage {
            if let Some(budget) = ctx.sessions.as_deref() {
                budget.record_usage(estimate, u.prompt_tokens as u64);
            }
            last_exact = u.prompt_tokens as u64 + u.completion_tokens as u64;
        }
        // A round with no calls, or the limit was reached — the task is done.
        if reason != FinishReason::ToolCalls || calls.is_empty() || round >= max_rounds {
            break;
        }
        round += 1;
        request.messages.push(ApiMessage::assistant_tool_calls(
            text.clone(),
            calls.clone(),
        ));
        for call in &calls {
            let args: serde_json::Value =
                serde_json::from_str(&call.arguments).unwrap_or_else(|_| serde_json::json!({}));
            let result = invoke_allowed(registry, ctx, allowed, call, args).await;
            request.messages.push(ApiMessage::tool(&call.id, &result));
        }
    }
    Ok(())
}

/// The wait for the silent lane ended without a permit: the app is quitting,
/// the round never streamed, and the task ends quietly (`run_rounds`).
struct Cancelled;

/// The round's place on the budget's silent lane (spec §6.3): `None` where
/// the engine has no session budget, otherwise the reservation — the
/// request's calibrated `estimate` floored by the last round's exact size
/// (`floor`), plus the reply cap — held until dropped.
async fn lane_reservation<'a>(
    budget: Option<&'a SessionBudget>,
    request: &ChatRequest,
    estimate: u64,
    floor: u64,
    cancel: &CancellationToken,
    lane: &'static str,
) -> Result<Option<Reservation<'a>>, Cancelled> {
    let Some(budget) = budget else {
        return Ok(None);
    };
    let need = budget.price(
        estimate,
        floor,
        request.sampling.max_tokens.map(|m| m as u64),
    );
    budget
        .acquire_silent(need, cancel, lane)
        .await
        .map(Some)
        .ok_or(Cancelled)
}

/// Consumes one round's stream into its text, accumulated tool calls, finish
/// reason and the server's exact `usage` when it sent one (the budget's floor
/// and calibration read it); `Thoughts`/`ThoughtsSignature` are tolerated and
/// ignored (a silent task has no UI to stream them to).
async fn read_round(
    mut stream: ChatStream,
) -> (
    String,
    Vec<ApiToolCall>,
    FinishReason,
    Option<crate::shared::api::contract::TokenUsage>,
) {
    let mut acc = ToolCallAccumulator::default();
    let mut text = String::new();
    let mut reason = FinishReason::Stop;
    let mut usage = None;
    while let Some(chunk) = stream.next().await {
        match chunk {
            ChatChunk::ToolCall(d) => acc.push(d),
            ChatChunk::Text(t) => text.push_str(&t),
            ChatChunk::Usage(u) => usage = Some(u),
            ChatChunk::Finished(r) => {
                reason = r;
                break;
            }
            // A background turn: the retry is worth a log line (a flaky provider is
            // otherwise invisible here) but has nothing to show — these turns have no
            // chip of their own.
            ChatChunk::Retry {
                attempt,
                max,
                delay,
            } => {
                tracing::info!(
                    attempt,
                    max,
                    ?delay,
                    "retrying a a background tool-loop turn"
                );
            }
            ChatChunk::Error { message, .. } => {
                tracing::warn!(error = %message, "engine error in a background tool loop");
            }
            ChatChunk::Thoughts(_) | ChatChunk::ThoughtsSignature(_) => {}
        }
    }
    (text, acc.finish(), reason, usage)
}

/// One call's result: the invocation when the tool is in the task's allowed
/// set, otherwise a localized refusal; an invocation error becomes result text
/// (the model reads it), never a panic.
async fn invoke_allowed(
    registry: &Arc<ToolRegistry>,
    ctx: &ToolContext,
    allowed: &[ToolId],
    call: &ApiToolCall,
    args: serde_json::Value,
) -> String {
    let allowed_has = |name: &str| allowed.iter().any(|t| t == name);
    if allowed_has(&call.name) {
        match registry.invoke(&call.name, ctx, args).await {
            Ok(o) => o.result,
            Err(e) => ctx.loc.tf(
                "loop.tool_error",
                &[("name", &call.name), ("err", &e.to_string())],
            ),
        }
    } else {
        ctx.loc.tf("loop.tool_not_allowed", &[("name", &call.name)])
    }
}

#[cfg(test)]
mod tests {
    use super::due;

    #[test]
    fn due_respects_threshold_and_disabled() {
        assert!(!due(5, 0)); // disabled
        assert!(!due(1, 3));
        assert!(!due(2, 3));
        assert!(due(3, 3)); // threshold reached
        assert!(due(4, 3)); // and above
    }
}
