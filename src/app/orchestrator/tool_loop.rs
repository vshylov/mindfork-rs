//! The shared "silent" agentic loop for background tasks (self-model auto-reflection
//! and notes auto-consolidation). Both tasks are a mini agentic loop with no UI streaming:
//! stream → a call accumulator → executing allowed tools → the next
//! round; tolerant of `Thoughts`/`ThoughtsSignature`/`Usage` (ignored). This body used to
//! be duplicated verbatim in `reflection.rs` and `consolidation.rs` (differing only in
//! limits and the log label) — now it lives here once. The main generation loop is deliberately
//! **not** touched: it has UI streaming, control-flow tools, Anthropic thinking
//! signatures, usage, effects — its complexity doesn't pay for a shared sink right now.

use std::sync::Arc;
use std::time::{Duration, Instant};

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
use crate::shared::session_budget::{Reservation, SILENT_YIELDS_MAX, SessionBudget};
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
    /// The time limit for the task's streaming and tools; its waits for the
    /// silent lane and for room are outside it (silent-preemption §4.5).
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
            timeout,
        );
        let outcome: Result<(), String> = match run.await {
            Ok(RoundsEnd::Done) => Ok(()),
            Ok(RoundsEnd::TimedOut) => {
                cancel.cancel();
                tracing::warn!(%profile_id, "{label}: time limit exceeded");
                Err(ctx.loc.t("loop.time_limit_exceeded").to_string())
            }
            Err(e) => {
                tracing::warn!(%profile_id, "{label}: error: {e}");
                Err(e.to_string())
            }
        };
        let _ = done_tx.send((kind, outcome));
    });
}

/// How a run of rounds ended: the task's work is done (a round with no
/// calls, or the round limit), or its clock ran out.
pub(super) enum RoundsEnd {
    Done,
    TimedOut,
}

/// One round's stream: its text, accumulated tool calls, finish reason and
/// the server's exact `usage` when it sent one.
type RoundOut = (
    String,
    Vec<ApiToolCall>,
    FinishReason,
    Option<crate::shared::api::contract::TokenUsage>,
);

/// How one round's stream ended under the lane and the clock
/// ([`stream_round`]).
enum Streamed {
    /// The round streamed to its end.
    Round(RoundOut),
    /// Displaced by an interactive stream: the round is made again.
    Displaced,
    /// The wait for the lane was cancelled: the app is quitting.
    Cancelled,
    /// The task's clock ran out while streaming.
    TimedOut,
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
///
/// A round whose stream was **displaced** by an interactive one
/// (docs/research/silent-preemption.md §4.4) is made again with the same
/// request — the messages are pushed only once a round completes — up to
/// [`SILENT_YIELDS_MAX`] times, after which the round holds. `clock` is the
/// task's time over its streaming and its tools; the waits for the lane and
/// for room are outside it (§4.5), so a task queued behind a long roll, or
/// displaced by a turn, is not timed out for the queue.
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
    clock: Duration,
) -> Result<RoundsEnd, anyhow::Error> {
    let mut round: u32 = 0;
    let mut last_exact: u64 = 0;
    let mut yields: u32 = 0;
    let mut left = clock;
    loop {
        let estimate = super::generation::estimate_prompt_tokens(request);
        let streamed = stream_round(
            backend,
            ctx,
            request,
            estimate,
            last_exact,
            cancel,
            lane,
            yields < SILENT_YIELDS_MAX,
            &mut left,
        )
        .await?;
        let (text, calls, reason, usage) = match streamed {
            Streamed::Round(out) => out,
            Streamed::Displaced => {
                yields += 1;
                tracing::info!(
                    lane,
                    yields,
                    "a silent round was displaced by an interactive stream; made again"
                );
                continue;
            }
            Streamed::Cancelled => return Ok(RoundsEnd::Done),
            Streamed::TimedOut => return Ok(RoundsEnd::TimedOut),
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
        if !run_tools(registry, ctx, allowed, request, &calls, &mut left).await {
            return Ok(RoundsEnd::TimedOut);
        }
    }
    Ok(RoundsEnd::Done)
}

/// One round's stream: the lane's reservation (a wait outside the clock),
/// the stream on the reservation's token under what is `left` of the clock,
/// and the reading of how it ended. The reservation is dropped with this
/// call, before the round's tools run.
#[allow(clippy::too_many_arguments)]
async fn stream_round(
    backend: &Arc<dyn EngineBackend>,
    ctx: &ToolContext,
    request: &ChatRequest,
    estimate: u64,
    floor: u64,
    cancel: &CancellationToken,
    lane: &'static str,
    yields: bool,
    left: &mut Duration,
) -> Result<Streamed, anyhow::Error> {
    let Ok(held) = lane_reservation(
        ctx.sessions.as_deref(),
        request,
        estimate,
        floor,
        cancel,
        lane,
        yields,
    )
    .await
    else {
        return Ok(Streamed::Cancelled);
    };
    let token = held
        .as_ref()
        .map_or_else(|| cancel.clone(), Reservation::stream_token);
    let started = Instant::now();
    let streamed = async {
        let stream = backend.chat_stream(request.clone(), token.clone()).await?;
        Ok::<RoundOut, anyhow::Error>(read_round(stream).await)
    };
    let Ok(out) = tokio::time::timeout(*left, streamed).await else {
        return Ok(Streamed::TimedOut);
    };
    let out = out?;
    *left = left.saturating_sub(started.elapsed());
    if out.2 == FinishReason::Cancelled && held.as_ref().is_some_and(Reservation::displaced) {
        return Ok(Streamed::Displaced);
    }
    Ok(Streamed::Round(out))
}

/// The round's calls in the model's order, under what is `left` of the
/// task's clock; `false` when the clock ran out.
async fn run_tools(
    registry: &Arc<ToolRegistry>,
    ctx: &ToolContext,
    allowed: &[ToolId],
    request: &mut ChatRequest,
    calls: &[ApiToolCall],
    left: &mut Duration,
) -> bool {
    let started = Instant::now();
    let tools = async {
        for call in calls {
            let args: serde_json::Value =
                serde_json::from_str(&call.arguments).unwrap_or_else(|_| serde_json::json!({}));
            let result = invoke_allowed(registry, ctx, allowed, call, args).await;
            request.messages.push(ApiMessage::tool(&call.id, &result));
        }
    };
    if tokio::time::timeout(*left, tools).await.is_err() {
        return false;
    }
    *left = left.saturating_sub(started.elapsed());
    true
}

/// The wait for the silent lane ended without a permit: the app is quitting,
/// the round never streamed, and the task ends quietly (`run_rounds`).
struct Cancelled;

/// The round's place on the budget's silent lane (spec §6.3): `None` where
/// the engine has no session budget, otherwise the reservation — the
/// request's calibrated `estimate` floored by the last round's exact size
/// (`floor`), plus the reply cap — held until dropped; `yields` says whether
/// an interactive waiter may displace its stream (silent-preemption §4.3).
async fn lane_reservation<'a>(
    budget: Option<&'a SessionBudget>,
    request: &ChatRequest,
    estimate: u64,
    floor: u64,
    cancel: &CancellationToken,
    lane: &'static str,
    yields: bool,
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
        .acquire_silent(need, cancel, lane, yields)
        .await
        .map(Some)
        .ok_or(Cancelled)
}

/// Consumes one round's stream into its text, accumulated tool calls, finish
/// reason and the server's exact `usage` when it sent one (the budget's floor
/// and calibration read it); `Thoughts`/`ThoughtsSignature` are tolerated and
/// ignored (a silent task has no UI to stream them to).
async fn read_round(mut stream: ChatStream) -> RoundOut {
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
