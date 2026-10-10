//! A manual smoke set against a real OpenAI-compatible server (llama.cpp
//! `llama-server` etc.), moved out of `client.rs` unchanged. Marked `#[ignore]` — it
//! does not run in CI. Run: set `MINDFORK_ENGINE_URL=http://127.0.0.1:8000/v1` and
//! `cargo test -- --ignored`.
//!
//! Under a `tests/` directory so that coverage, which runs without `--ignored`, leaves
//! these smokes' lines out — an edit to one of them inside `client.rs` failed the new
//! code's coverage at 0 % (2026-10-10), as `llama_setup`'s and `fetch`'s had.

use super::*;
use crate::entities::sampling::SamplingConfig;
use crate::shared::api::contract::{ApiMessage, ToolCallAccumulator, ToolSchema};
use futures_util::StreamExt;

fn client_from_env() -> Option<OpenAiClient> {
    crate::shared::api::live_client("MINDFORK_ENGINE_URL", "MINDFORK_ENGINE_KEY")
}

/// A conversation whose **tool result** carries an image (spec §9.10): the model asks
/// for a screenshot, the tool returns one, and the user asks what is on it.
/// `with_image = false` is the control arm.
fn screenshot_turn(with_image: bool) -> ChatRequest {
    screenshot_turn_with(with_image, "Screenshot taken.")
}

/// The same turn with the tool result's text spelled out: the withheld-image arm
/// sends the statement the MCP adapter appends when `tools.mcp_images` is off
/// (`tool.mcp.images_off`), where the plain control arm sends nothing at all.
fn screenshot_turn_with(with_image: bool, result_text: &str) -> ChatRequest {
    let tool = ApiMessage::tool("call-1", result_text);
    let tool = if with_image {
        tool.with_images(vec![crate::shared::api::ApiImage::new(
            "image/png",
            &crate::shared::api::green_circle_png_base64(),
            None,
        )])
    } else {
        tool
    };
    ChatRequest {
        continue_final: false,
        system: None,
        // The shape a real turn has: the question is asked up front and the tool
        // result is the last message, so the model answers from it. See the Gemini
        // twin of this smoke for why a trailing user message is not just unrealistic
        // but actively misleading there.
        messages: vec![
            ApiMessage::user(crate::shared::api::TOOL_VISION_PROMPT),
            ApiMessage::assistant_tool_calls(
                "",
                vec![crate::shared::api::ApiToolCall {
                    id: "call-1".into(),
                    name: "take_screenshot".into(),
                    arguments: "{}".into(),
                    thought_signature: None,
                }],
            ),
            tool,
        ],
        // Below the app's 16384 on purpose. Qwen 3.6 thinking on the withheld arm ran
        // out at 2048 two runs of three — and at 16384 it thought on to the end of the
        // context (16 028 tokens, seven minutes) two runs of three, the same empty
        // answer: a loop, not a short budget (2026-10-10, docs/journal/ci.md). The
        // smaller cap only makes the same red cheaper.
        sampling: SamplingConfig {
            max_tokens: Some(2048),
            ..Default::default()
        },
        tools: vec![ToolSchema {
            name: "take_screenshot".into(),
            description: "Take a screenshot of the screen.".into(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
        }],
    }
}

/// An image inside a **tool result** reaches a vision-capable llama.cpp — the path an
/// MCP screenshot tool takes (spec §9.10, docs/research/mcp-tool-images.md).
///
/// Two arms on purpose. The control, with the identical conversation minus the image,
/// must **not** describe the fixture: measured, a blind model answers this question
/// confidently anyway, and a one-armed version of this test passed against a feature
/// that was doing nothing.
#[tokio::test]
#[ignore = "requires a vision-capable OpenAI-compatible server (MINDFORK_ENGINE_URL + --mmproj)"]
async fn tool_result_image_is_seen_live() {
    if crate::shared::api::live_text_only() {
        eprintln!("skip: MINDFORK_LIVE_TEXT_ONLY — this stack has no vision projector");
        return;
    }
    let Some(client) = client_from_env() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let (control, _, _) = collect(
        client
            .chat_stream(screenshot_turn(false), Default::default())
            .await
            .unwrap(),
    )
    .await;
    eprintln!("control (no image): {control}");
    crate::shared::api::assert_sees_green_circle(&control, false, "control");

    let (answer, _, _) = collect(
        client
            .chat_stream(screenshot_turn(true), Default::default())
            .await
            .unwrap(),
    )
    .await;
    eprintln!("tool-result image: {answer}");
    crate::shared::api::assert_sees_green_circle(&answer, true, "with the image");
}

/// Fork H1's live gate (docs/history/gateway-images-and-continue.md §5): the body this
/// client builds for a gateway — the tool's image re-homed into a user message —
/// **serialized by the client's own builder** and replayed pinned to the routes
/// that failed today's shape, one of each failure kind, blind arm beside it. The
/// client has no routing knob and must not grow one (§3), so the pin is the one
/// field added to its bytes. Replaying the builder's output rather than a probe's
/// reconstruction is what keeps this measuring the shipped shape (lessons §3).
#[tokio::test]
#[ignore = "requires MINDFORK_ENGINE_URL + MINDFORK_ENGINE_KEY + MINDFORK_ENGINE_MODEL on OpenRouter, and MINDFORK_LIVE_PINNED_ROUTES"]
async fn a_re_homed_tool_image_is_seen_on_the_routes_that_failed_live() {
    let (Ok(url), Ok(key), Ok(model), Ok(routes)) = (
        std::env::var("MINDFORK_ENGINE_URL"),
        std::env::var("MINDFORK_ENGINE_KEY"),
        std::env::var("MINDFORK_ENGINE_MODEL"),
        std::env::var("MINDFORK_LIVE_PINNED_ROUTES"),
    ) else {
        eprintln!(
            "skip: the gateway, its key, the model and MINDFORK_LIVE_PINNED_ROUTES are all needed"
        );
        return;
    };
    let http = reqwest::Client::new();
    for route in routes.split(',').map(str::trim).filter(|r| !r.is_empty()) {
        for with_image in [false, true] {
            let mut req = screenshot_turn(with_image);
            if let Some(messages) = wire::rehome_tool_images(&req.messages) {
                req.messages = messages;
            }
            let mut body =
                serde_json::to_value(wire::build_chat_request(&req, true, Some(&model), false))
                    .unwrap();
            body["provider"] = serde_json::json!({ "only": [route], "allow_fallbacks": false });
            let resp = http
                .post(format!("{}/chat/completions", url.trim_end_matches('/')))
                .bearer_auth(&key)
                .json(&body)
                .send()
                .await
                .expect("the gateway answers");
            let status = resp.status();
            let raw = resp.text().await.unwrap_or_default();
            assert!(status.is_success(), "{route}: HTTP {status}: {raw}");
            let answer: String = raw
                .lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .filter_map(|d| serde_json::from_str::<serde_json::Value>(d.trim()).ok())
                .filter_map(|v| {
                    v["choices"][0]["delta"]["content"]
                        .as_str()
                        .map(str::to_string)
                })
                .collect();
            eprintln!("{route}, image={with_image}: {answer}");
            if answer.is_empty() {
                eprintln!("{route}: the stream carried no text: {raw:.400}");
            }
            crate::shared::api::assert_sees_green_circle(
                &answer,
                with_image,
                &format!("{route}, image={with_image}"),
            );
        }
    }
}

/// The model says outright that it cannot see what it was asked about — the half of
/// the two withheld-image smokes that a decline has to pass.
///
/// Matched with the **apostrophes folded** to ASCII: on `gpt-oss-120b` both smokes
/// went red while the model declined exactly as asked — "I can’t see the image", with
/// U+2019 (2026-10-10, docs/research/e2e-gpt-oss-120b.md §11, fork F9). Which
/// apostrophe a model types is typography, as the dash is in the orchestrator's
/// `mentions_code`; the phrases stay the criterion, and one list serves both smokes,
/// so a decline one of them reads the other reads too.
fn says_it_cannot_see(answer: &str) -> bool {
    let low: String = answer
        .to_lowercase()
        .chars()
        .map(|c| match c {
            // The right and left single quotation marks, the modifier-letter apostrophe.
            '\u{2019}' | '\u{2018}' | '\u{02BC}' => '\'',
            other => other,
        })
        .collect();
    [
        "cannot see",
        "can't see",
        "cannot tell",
        "can't tell",
        "unable to",
        "not able to",
    ]
    .iter()
    .any(|phrase| low.contains(phrase))
}

#[test]
fn a_decline_is_read_whichever_apostrophe_the_model_typed() {
    // The two answers gpt-oss-120b gave on 2026-10-10.
    assert!(says_it_cannot_see(
        "I can\u{2019}t see the image, but based on the saved chart file you can open it"
    ));
    assert!(says_it_cannot_see(
        "I\u{2019}m not able to view the screenshot, so I can\u{2019}t tell you the colour."
    ));
    assert!(says_it_cannot_see("I cannot see it."));
    assert!(says_it_cannot_see("I'm unable to view images here."));
    // An answer that claims to know is not a decline, in any typography.
    assert!(!says_it_cannot_see(
        "The background is blue, with a white square."
    ));
    assert!(!says_it_cannot_see(
        "I can\u{2019}t wait to say it: the legend sits top left."
    ));
}

/// The blind arm once more, this time carrying the sentence the MCP adapter appends
/// when `tools.mcp_images` withheld the image (`tool.mcp.images_off`).
///
/// **Why the wording is directive and not descriptive.** Measured on Gemma 4 31B
/// (2026-09-12, five runs an arm, the fixture's own 2048-token budget): saying
/// nothing, the model described a screenshot it never received **5/5** — and the
/// descriptive wording the `loop.images_*` family uses ("You have not seen them.")
/// also went 5/5. Only adding "do not describe what they show; say that you cannot
/// see them" moved it, to 0/5. So the sentence that ships here is deliberately not
/// its siblings' shape, and this smoke is what says so. (A 256-token budget makes
/// this unreadable: a thinking model runs out inside its thoughts and returns empty
/// content, which looks like a decline and is not one.) On Qwen 3.6 the wording does
/// not hold: thinking, it deliberates without end; muted, it described the image it
/// never got 2 runs of 5 — the failure prints which (docs/journal/ci.md).
///
/// No projector is needed — nothing here sends an image.
#[tokio::test]
#[ignore = "requires MINDFORK_ENGINE_URL"]
async fn a_withheld_tool_image_is_not_described_live() {
    let Some(client) = client_from_env() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    // The control, printed rather than asserted: this is the answer the sentence has
    // to displace, and asserting a hallucination would only pin today's guess.
    let (control, _, _) = collect(
        client
            .chat_stream(screenshot_turn(false), Default::default())
            .await
            .unwrap(),
    )
    .await;
    eprintln!("control (nothing said): {control}");

    use crate::shared::i18n::{Lang, locale};
    let said = locale(Lang::En).tf("tool.mcp.images_off", &[("n", "1")]);
    let text = format!("Screenshot taken.\n{said}");
    let (answer, thoughts, finish) = collect(
        client
            .chat_stream(screenshot_turn_with(false, &text), Default::default())
            .await
            .unwrap(),
    )
    .await;
    eprintln!("withheld, and said so: {answer}");
    let low = answer.to_lowercase();
    // Two-sided on purpose. "Does not say green" would pass on a *wrong* guess —
    // which is exactly what the control produces — so the criterion is that no
    // colour is claimed at all, and that the model says outright it cannot see.
    let colours = [
        "blue", "green", "red", "white", "black", "yellow", "purple", "grey", "gray",
    ];
    assert!(
        !colours.iter().any(|c| low.contains(c)),
        "a withheld image must not be described: {answer:?}"
    );
    assert!(
        says_it_cannot_see(&answer),
        "the model has to say it cannot see the image: {answer:?} ({finish:?} after {} \
         characters of thoughts)",
        thoughts.len()
    );
}

/// The other shape of the same family: `python_exec`'s note is a **suffix on one
/// file's line** inside the result's `files:` section, not a bracketed line of its
/// own — so it gets its own arm rather than inheriting the MCP one's verdict.
///
/// The measured outcome differs too, and the assertion says so. With the shipped
/// suffix the model mostly answered by **calling the tool again** (7 of 9 runs,
/// `finish_reason: tool_calls`) — not a hallucination, but a wasted round, since the
/// second call's image is withheld for the same reason. With the directive clause it
/// declines outright 4/5 and re-calls 1/5. Either is acceptable; inventing an answer
/// about the chart is not, and that is what this asserts.
///
/// The result is assembled by the producers rather than typed: the console half by
/// `present::format_console`, the section from `keep_outputs`' own lines and join. It
/// was typed in the uncounted `stdout:` shape, which no call has returned since the
/// console's sections were counted — so the smoke measured a result the model no longer
/// gets, and would have gone on doing so through any later change to the shape.
/// Re-measured on the counted shape (Gemma 4 31B, 2026-09-13, five runs an arm): with
/// the note a plain refusal 5/5; without it, an invented answer 3/5 and a re-call 2/5.
#[tokio::test]
#[ignore = "requires MINDFORK_ENGINE_URL"]
async fn a_withheld_chart_is_not_described_live() {
    let Some(client) = client_from_env() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    use crate::features::tools::present::format_console;
    use crate::shared::i18n::{Lang, locale};
    let loc = locale(Lang::En);
    // What a call that printed `saved` and left one chart returns with images off: the
    // console, a blank line, then `files:` over the section's lines (`keep_outputs`).
    let result = format!(
        "{}\n\nfiles:\n{}\n{}{}",
        format_console(None, "saved", "", true, Some(0), loc),
        loc.tf("tool.python_exec.files.saved_in", &[("dir", "/chat/files")]),
        loc.tf(
            "tool.python_exec.files.item",
            &[
                ("name", "chart.png"),
                ("size", "24.1 KB"),
                ("mime", "image/png"),
            ],
        ),
        loc.t("tool.python_exec.files.not_shown_off"),
    );
    assert!(
        result.starts_with("stdout (1 line):\nsaved\n\nfiles:\n"),
        "the smoke has to send the shape a call returns: {result}"
    );
    let (answer, _, finish) = collect(
        client
            .chat_stream(chart_turn(&result), Default::default())
            .await
            .unwrap(),
    )
    .await;
    eprintln!("withheld chart ({finish:?}): {answer}");
    if finish == Some(FinishReason::ToolCalls) {
        // It went back to the tool instead of answering — it did not invent anything.
        return;
    }
    assert!(
        says_it_cannot_see(&answer),
        "a chart it was never shown must not be answered for: {answer:?}"
    );
}

/// A `python_exec` turn whose result is `result`: the question asks something only
/// the rendered image could answer, so any substantive answer is a claim to have
/// seen it.
fn chart_turn(result: &str) -> ChatRequest {
    ChatRequest {
        continue_final: false,
        system: None,
        messages: vec![
            ApiMessage::user(
                "Plot the monthly totals and save the chart, then tell me: does the \
                 legend overlap the plotted line? Answer briefly.",
            ),
            ApiMessage::assistant_tool_calls(
                "",
                vec![crate::shared::api::ApiToolCall {
                    id: "call-1".into(),
                    name: "python_exec".into(),
                    arguments: "{}".into(),
                    thought_signature: None,
                }],
            ),
            ApiMessage::tool("call-1", result),
        ],
        sampling: SamplingConfig {
            max_tokens: Some(8192),
            ..Default::default()
        },
        tools: vec![ToolSchema {
            name: "python_exec".into(),
            description: "Run Python.".into(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
        }],
    }
}

pub(super) async fn collect(stream: ChatStream) -> (String, String, Option<FinishReason>) {
    let mut text = String::new();
    let mut thoughts = String::new();
    let mut finish = None;
    let mut stream = stream;
    while let Some(chunk) = stream.next().await {
        match chunk {
            ChatChunk::Text(t) => text.push_str(&t),
            ChatChunk::Thoughts(t) => thoughts.push_str(&t),
            ChatChunk::ThoughtsSignature(_)
            | ChatChunk::ToolCall(_)
            | ChatChunk::Usage(_)
            | ChatChunk::Served(_) => {}
            // Say why: a smoke whose engine failed mid-stream would otherwise
            // assert on empty text with nothing in the output explaining it.
            ChatChunk::Error { message, .. } => eprintln!("engine error: {message}"),
            ChatChunk::Retry {
                attempt,
                max,
                delay,
            } => {
                eprintln!("retrying {attempt}/{max} in {delay:?}")
            }
            ChatChunk::Finished(r) => {
                finish = Some(r);
                break;
            }
        }
    }
    (text, thoughts, finish)
}

/// The base engine smoke: a stream arrives, it carries text, and the finish
/// reason is sane.
///
/// `max_tokens` is deliberately generous and thinking is left **on**, because
/// on a reasoning model the two share one budget and the reply is emitted
/// last. Measured on `Qwen3.6-27B` q4_K_M: this trivial prompt costs 357–949
/// characters of `reasoning_content` before the `pong` — so the original 64
/// were spent entirely on thinking (empty text, `finish=Length`), and even
/// 256 sits inside the worst case's noise. 1024 keeps ~4x margin over the
/// widest run observed while the smoke still exercises *both* streams, which
/// is what a base smoke is for; `emits_thoughts_for_reasoning_model` covers
/// the thoughts channel on its own.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn simple_generation() {
    let Some(client) = client_from_env() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let req = ChatRequest {
        continue_final: false,
        system: Some("You are a helpful assistant.".into()),
        messages: vec![ApiMessage::user("Reply with exactly: pong")],
        sampling: SamplingConfig {
            max_tokens: Some(1024),
            ..Default::default()
        },
        tools: vec![],
    };
    let (text, _thoughts, finish) =
        collect(client.chat_stream(req, Default::default()).await.unwrap()).await;
    assert!(!text.is_empty(), "expected non-empty response");
    assert!(matches!(
        finish,
        Some(FinishReason::Stop | FinishReason::Length)
    ));
}

/// A model too large for one file, end to end: the server reports the **part**
/// it was pointed at, and what a header may show is the *model*.
///
/// Weights over ~50 GB ship as `<name>-00001-of-00002.gguf`, …; llama.cpp is
/// handed the first part and reads the rest itself, and an un-aliased
/// `llama-server` then reports that part's whole path as its model id. That
/// is the single input [`crate::shared::gguf::display_id`] exists for, and
/// until `gpt-oss-120b` joined the live gate it had only ever been given
/// fixtures.
///
/// Both halves are asserted here because either alone is worthless: that the
/// server really did report a part (otherwise the stack is not what the run
/// declared, and the check is vacuous), and that `model_id` hands back a name
/// with the directory, the extension **and** the part number gone.
///
/// Declared, not detected — `MINDFORK_LIVE_SPLIT_MODEL=1` — and it **fails
/// rather than skips** when the declaration turns out to be false
/// (docs/research/e2e-gpt-oss-120b.md §6, T2).
///
/// Measured on `unsloth/gpt-oss-120b-GGUF` Q8_0 (two parts, 63.39 GB):
/// `/repository/Q8_0/gpt-oss-120b-Q8_0-00001-of-00002.gguf` in,
/// `gpt-oss-120b-Q8_0` out.
#[tokio::test]
#[ignore = "requires a live server holding a split GGUF (MINDFORK_LIVE_SPLIT_MODEL=1)"]
async fn a_split_model_is_named_by_the_model_not_the_part_live() {
    if !crate::shared::api::live_split_model() {
        eprintln!("skip: MINDFORK_LIVE_SPLIT_MODEL not set");
        return;
    }
    let Some(client) = client_from_env() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let listed = client
        .listed_models()
        .await
        .expect("a live server must answer GET /v1/models");
    eprintln!("listed models: {listed:?}");
    let raw = listed
        .first()
        .expect("GET /v1/models listed nothing to check");
    let shard = crate::shared::gguf::parse_shard(raw).unwrap_or_else(|| {
        panic!(
            "MINDFORK_LIVE_SPLIT_MODEL was declared, but the server reports a \
             plain file: {raw} — this stack cannot test what the run says it tests"
        )
    });
    assert_eq!(
        shard.index, 1,
        "llama.cpp must be given the first part: {raw}"
    );
    assert!(
        shard.total > 1,
        "a split model has more than one part: {raw}"
    );

    let name = client
        .model_id()
        .await
        .expect("the same server that lists a model must name it");
    eprintln!("split model reported as {raw:?}, shown as {name:?}");
    assert!(
        crate::shared::gguf::parse_shard(&format!("{name}{}", crate::shared::gguf::EXT)).is_none(),
        "the part number survived into the name a header shows: {name}"
    );
    assert!(
        !name.contains('/') && !name.contains('\\') && !name.ends_with(crate::shared::gguf::EXT),
        "a path reached the header instead of a model name: {name}"
    );
}

/// A real rejection from a real server, checked as a **typed** error rather
/// than as prose.
///
/// This is the path every non-2xx takes (`check_status`), and the properties it
/// has to hold are the ones the layers above decide on: the status survives,
/// the server's own body survives (so the overflow classifier still fires and
/// the user gets the `/compact` advice instead of a generic wrapper), and a
/// `400` is **not** reported as worth retrying — the coming retry decorator
/// would otherwise spend its whole budget on a prompt that can never fit
/// (docs/research/cloud-retry-backoff.md §5).
///
/// The prompt is deliberately far larger than any context this project runs
/// against: llama.cpp answers `400` *before* the prefill starts (measured —
/// spec §6.7), so the size costs nothing and the smoke cannot silently pass by
/// fitting.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn an_oversized_prompt_is_a_typed_non_transient_error() {
    let Some(client) = client_from_env() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let req = ChatRequest {
        continue_final: false,
        system: None,
        messages: vec![ApiMessage::user("word ".repeat(120_000))],
        sampling: SamplingConfig {
            max_tokens: Some(16),
            ..Default::default()
        },
        tools: vec![],
    };
    // `expect_err` would need `ChatStream: Debug`, which a boxed stream isn't.
    let err = match client.chat_stream(req, Default::default()).await {
        Ok(_) => panic!("a prompt this size cannot fit any context window"),
        Err(err) => err,
    };
    let typed = err
        .downcast_ref::<crate::shared::api::error::EngineError>()
        .expect("the status must reach the caller as a typed error");
    eprintln!(
        "status={:?} transient={} retry_after={:?}\nmessage={}",
        typed.status,
        typed.is_transient(),
        typed.retry_after,
        typed.message
    );
    assert_eq!(typed.status, Some(400), "the status must survive");
    assert!(
        !typed.is_transient(),
        "an oversized prompt must not be reported as retryable"
    );
    assert!(
        crate::features::compaction::is_context_overflow(&typed.message),
        "the server's body must survive so the advice can be picked: {}",
        typed.message
    );
}

/// Asks the model to print the literal EOS text and then say `DONE` —
/// generation shouldn't cut off (stopped by token-id on the server, the `stop` field
/// isn't sent; docs/xinfer-contract.md §5). `DONE` may arrive in the text or in
/// "thoughts" (a reasoning model), so both streams are checked.
async fn assert_no_self_terminate(client: &OpenAiClient, eos_text: &str) {
    let req = ChatRequest {
        continue_final: false,
        system: None,
        messages: vec![ApiMessage::user(format!(
            "Print this token literally and then say DONE: {eos_text}"
        ))],
        sampling: SamplingConfig {
            max_tokens: Some(256),
            ..Default::default()
        },
        tools: vec![],
    };
    let (text, thoughts, finish) =
        collect(client.chat_stream(req, Default::default()).await.unwrap()).await;
    let combined = format!("{thoughts}{text}");
    assert!(
        combined.contains("DONE"),
        "generation cut off early for {eos_text:?}: text={text:?} thoughts={thoughts:?}"
    );
    assert!(finish.is_some());
}

/// Anti-self-cutoff on EOS text — for both families: Qwen (`<|im_end|>`) and
/// Gemma (`<end_of_turn>`). See spec §7, docs/xinfer-contract.md §5, §9.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn does_not_self_terminate_on_eos_text() {
    let Some(client) = client_from_env() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    for eos in ["<|im_end|>", "<end_of_turn>"] {
        assert_no_self_terminate(&client, eos).await;
    }
}

/// Tool-calling: the server gets the tool schema, the model calls it —
/// `finish_reason="tool_calls"` and `delta.tool_calls` are assembled correctly.
///
/// **Thinking is off** (`reasoning_budget=0`). With it on, `Qwen3.6-27B` q4_K_M
/// emits the correct call and then does not stop — it repeats the identical
/// call (up to 15 times observed) until `max_tokens` cuts it off, so the finish
/// reason arrives as `Length` and this smoke goes red about one run in five:
/// measured 2/11 at the server's default temperature and **5/20** at the
/// orchestrator's 0.1, against **0/20** with thinking muted. Temperature is
/// therefore not the lever — the repetition rides on the thinking loop, and a
/// larger ceiling only buys more repeats.
///
/// What this costs is nothing this set was carrying alone: tool calls *with*
/// thinking on are exercised live by the orchestrator smokes
/// (`app/orchestrator/tests/live.rs`), which run the real app path — tool
/// results fed back, `thinking` left at the server's default — and stay green
/// on both model families.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn tool_call_is_emitted_and_parsed() {
    let Some(client) = client_from_env() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let tool = ToolSchema {
        name: "get_weather".into(),
        description: "Get current weather for a city".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": { "city": { "type": "string" } },
            "required": ["city"]
        }),
    };
    let req = ChatRequest {
        continue_final: false,
        system: None,
        messages: vec![ApiMessage::user(
            "Call get_weather for Paris. Respond only with the tool call.",
        )],
        sampling: SamplingConfig {
            max_tokens: Some(512),
            reasoning_budget: Some(0),
            ..Default::default()
        },
        tools: vec![tool],
    };
    let mut stream = client.chat_stream(req, Default::default()).await.unwrap();
    let mut acc = ToolCallAccumulator::default();
    let mut finish = None;
    while let Some(chunk) = stream.next().await {
        match chunk {
            ChatChunk::ToolCall(delta) => acc.push(delta),
            ChatChunk::Finished(reason) => {
                finish = Some(reason);
                break;
            }
            _ => {}
        }
    }
    let calls = acc.finish();
    assert_eq!(
        finish,
        Some(FinishReason::ToolCalls),
        "expected tool_calls finish, got {finish:?} (calls={calls:?})"
    );
    assert!(
        calls.iter().any(|c| c.name == "get_weather"),
        "get_weather call not parsed: {calls:?}"
    );
}

/// "Thoughts" (CoT): a reasoning model (or a server with `--reasoning-format`) returns
/// `reasoning_content` as a separate stream — mindfork collects it into `Thoughts`.
/// Requires a thinking model; otherwise `thoughts` will be empty (thoughts get inlined).
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server with a reasoning model"]
async fn emits_thoughts_for_reasoning_model() {
    let Some(client) = client_from_env() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let req = ChatRequest {
        continue_final: false,
        system: None,
        messages: vec![ApiMessage::user(
            "Think step by step, then answer: what is 17*23?",
        )],
        sampling: SamplingConfig {
            max_tokens: Some(512),
            thinking: Some(true),
            ..Default::default()
        },
        tools: vec![],
    };
    let (text, thoughts, finish) =
        collect(client.chat_stream(req, Default::default()).await.unwrap()).await;
    assert!(finish.is_some());
    assert!(
        !thoughts.is_empty(),
        "expected non-empty thoughts from a reasoning model: text={text:?}"
    );
}

/// The **gateway** arm of the same claim: a reasoning model reached through
/// OpenRouter (or any gateway that copies its wire) streams its reasoning as
/// `delta.reasoning`, where llama.cpp, vLLM, DeepSeek and xAI send
/// `delta.reasoning_content`. The client reads either
/// (`wire::Delta::thoughts`) — this is what proves it against a live one
/// (docs/research/openrouter-external.md §5, F1).
///
/// **Declared, never guessed**, on the pattern of `MINDFORK_LIVE_TEXT_ONLY`:
/// `MINDFORK_LIVE_GATEWAY_MODEL` names a *reasoning* model on the endpoint
/// `MINDFORK_ENGINE_URL` points at (e.g. `deepseek/deepseek-r1`), and the
/// smoke then **fails** rather than skips when no thoughts arrive. A smoke
/// that quietly passes on a stack which sent none is worse than no smoke
/// (lessons §9) — and this one exists precisely because the gateway path
/// used to end that way, silently.
///
/// The model name is not optional here as it is on a single-model
/// `llama-server`: a gateway routes on the request's `model` and answers
/// `400` without it (docs/research/external-model-name.md §2.3).
#[tokio::test]
#[ignore = "requires MINDFORK_ENGINE_URL + MINDFORK_ENGINE_KEY + MINDFORK_LIVE_GATEWAY_MODEL (a reasoning model on a gateway)"]
async fn a_gateway_streams_thoughts_under_its_own_field_name() {
    let Some(client) = client_from_env() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let Ok(model) = std::env::var("MINDFORK_LIVE_GATEWAY_MODEL") else {
        eprintln!("skip: MINDFORK_LIVE_GATEWAY_MODEL not set");
        return;
    };
    let client = client.with_model(Some(model.clone()));
    let req = ChatRequest {
        continue_final: false,
        system: None,
        messages: vec![ApiMessage::user(
            "Think step by step, then answer: what is 17*23?",
        )],
        sampling: SamplingConfig {
            // Thinking and the reply share one budget on a reasoning model,
            // and the reply is emitted last (see `simple_generation`).
            max_tokens: Some(2048),
            thinking: Some(true),
            ..Default::default()
        },
        tools: vec![],
    };
    let (text, thoughts, finish) =
        collect(client.chat_stream(req, Default::default()).await.unwrap()).await;
    println!("gateway={model} finish={finish:?}\nthoughts={thoughts}\ntext={text}");
    assert!(finish.is_some());
    assert!(
        !thoughts.is_empty(),
        "{model} was declared a reasoning model, so its thoughts must reach the app \
         under one of the two field names: text={text:?}"
    );
}

/// The catalogue, live: what a gateway publishes for the configured model is
/// what the compaction trigger measures against and what the sampling offer
/// is narrowed to (docs/history/gateway-capabilities.md §5, N1–N2).
///
/// Declared rather than guessed, on the `MINDFORK_LIVE_TEXT_ONLY` pattern:
/// `MINDFORK_LIVE_CATALOGUE=1` says "this endpoint publishes a catalogue with
/// the two keys", and the smoke then **fails** rather than skips if nothing
/// comes back — a pass against a server that publishes neither would be a
/// green light for a feature that never ran. Needs `MINDFORK_ENGINE_MODEL`:
/// there is no per-model row to look up without a model.
#[tokio::test]
#[ignore = "requires MINDFORK_ENGINE_URL + MINDFORK_ENGINE_MODEL + MINDFORK_LIVE_CATALOGUE (an endpoint that publishes one)"]
async fn a_gateways_catalogue_answers_for_the_configured_model() {
    let Some(client) = client_from_env() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    if std::env::var("MINDFORK_LIVE_CATALOGUE").is_err() {
        eprintln!("skip: MINDFORK_LIVE_CATALOGUE not set");
        return;
    }
    let model = std::env::var("MINDFORK_ENGINE_MODEL")
        .expect("MINDFORK_ENGINE_MODEL names the row to look up");
    let caps = client
        .model_capabilities()
        .await
        .expect("the endpoint was declared to publish a catalogue");
    println!("{model}: {caps:?}");
    assert!(
        caps.context_length.is_some_and(|n| n > 0),
        "a published catalogue is what gives a gateway its compaction window"
    );
    let fields = caps
        .sampling_fields
        .expect("…and the parameter list is the other half");
    // The narrowing is the point, not the raw list: what the settings screen
    // and `set_sampling` will offer after this answer.
    let offered = crate::entities::sampling::available_sampling_fields(None, Some(&fields));
    println!("offered after narrowing: {offered:?}");
    assert!(
        offered.len() < crate::entities::sampling::SETTABLE_SAMPLING_FIELDS.len(),
        "a gateway takes fewer fields than a llama.cpp: {offered:?}"
    );
}

/// The silent turns against an endpoint that **cannot** be told to stop
/// reasoning — the defect the recovery exists for, live.
///
/// `MINDFORK_LIVE_MANDATORY_REASONING_MODEL` declares such a model
/// (`deepseek/deepseek-r1` on OpenRouter is one: measured, it answers
/// `reasoning_effort: "none"` with `400 "Reasoning is mandatory for this
/// endpoint and cannot be disabled"` — docs/research/openrouter-external.md
/// §8.1, M4/M4a). The smoke then sends exactly what `title.rs` sends and
/// **fails** rather than skips if the turn does not complete: before the
/// recovery this request was the `400` itself, so a pass here is the whole
/// claim — the title, the compaction roll and impersonation work on such a
/// model again.
#[tokio::test]
#[ignore = "requires MINDFORK_ENGINE_URL + MINDFORK_LIVE_MANDATORY_REASONING_MODEL (a model that must reason)"]
async fn a_muted_turn_survives_an_endpoint_that_must_reason() {
    let Some(client) = client_from_env() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let Ok(model) = std::env::var("MINDFORK_LIVE_MANDATORY_REASONING_MODEL") else {
        eprintln!("skip: MINDFORK_LIVE_MANDATORY_REASONING_MODEL not set");
        return;
    };
    let client = client.with_model(Some(model.clone()));
    // The title turn's own shape (title.rs): muted reasoning, a short cap, a
    // moderate temperature, no tools.
    let req = ChatRequest {
        continue_final: false,
        system: Some("Give this conversation a short title. Answer with the title only.".into()),
        messages: vec![ApiMessage::user("How do database indexes work?")],
        sampling: SamplingConfig {
            max_tokens: Some(2048),
            temperature: Some(0.3),
            thinking: Some(false),
            reasoning_effort: Some(ReasoningEffort::None),
            reasoning_budget: Some(0),
            ..Default::default()
        },
        tools: vec![],
    };
    let (text, thoughts, finish) =
        collect(client.chat_stream(req, Default::default()).await.unwrap()).await;
    println!(
        "{model}: finish={finish:?}\ntitle={text}\nthoughts={} chars",
        thoughts.len()
    );
    assert!(
        finish.is_some() && !text.is_empty(),
        "the muted turn must complete on an endpoint that refuses to be muted: \
         finish={finish:?} text={text:?}"
    );
}

/// One turn of the thinking-switch smokes against the model `var` declares
/// (docs/history/gateway-thinking-switch.md §5), or `None` when the stack or the model is
/// not declared. Returns the model, the reply, the finish reason and the
/// reasoning tokens the endpoint reported — the deterministic half, since a
/// provider may summarize its thoughts to nothing (lessons §2).
async fn switch_smoke(
    var: &str,
    thinking: bool,
) -> Option<(String, String, Option<FinishReason>, u32)> {
    let Some(client) = client_from_env() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return None;
    };
    let Ok(model) = std::env::var(var) else {
        eprintln!("skip: {var} not set");
        return None;
    };
    let req = ChatRequest {
        continue_final: false,
        system: None,
        messages: vec![ApiMessage::user(
            "Which city hosted the Summer Olympics exactly 32 years before 2024? \
             Answer in one sentence.",
        )],
        sampling: SamplingConfig {
            max_tokens: Some(4096),
            thinking: Some(thinking),
            ..Default::default()
        },
        tools: vec![],
    };
    let client = client.with_model(Some(model.clone()));
    let mut stream = client.chat_stream(req, Default::default()).await.unwrap();
    let (mut text, mut reasoning, mut finish) = (String::new(), 0, None);
    while let Some(chunk) = stream.next().await {
        match chunk {
            ChatChunk::Text(t) => text.push_str(&t),
            ChatChunk::Usage(u) => reasoning = u.reasoning_tokens,
            ChatChunk::Error { message, .. } => eprintln!("engine error: {message}"),
            ChatChunk::Finished(r) => {
                finish = Some(r);
                break;
            }
            _ => {}
        }
    }
    println!(
        "{model}: thinking={thinking} finish={finish:?} reasoning_tokens={reasoning}\ntext={text}"
    );
    Some((model, text, finish, reasoning))
}

/// L1: the switch on, no effort, against a model that reasons only when asked —
/// `MINDFORK_LIVE_SWITCH_ON_MODEL`, e.g. `anthropic/claude-haiku-4.5`, which
/// reasoned zero tokens on every route while the switch went out as `thinking`.
#[tokio::test]
#[ignore = "requires MINDFORK_ENGINE_URL + MINDFORK_ENGINE_KEY + MINDFORK_LIVE_SWITCH_ON_MODEL (a gateway model that reasons only when asked)"]
async fn a_gateway_reasons_when_the_switch_is_on() {
    let Some((model, _, finish, reasoning)) =
        switch_smoke("MINDFORK_LIVE_SWITCH_ON_MODEL", true).await
    else {
        return;
    };
    assert!(finish.is_some());
    assert!(
        reasoning > 0,
        "{model} was asked to reason and reported no reasoning tokens"
    );
}

/// L2: the switch off against a model that reasons by default —
/// `MINDFORK_LIVE_SWITCH_OFF_MODEL`, e.g. `qwen/qwen3.6-27b`, which reasoned
/// anyway while "off" went out as `thinking: false`.
#[tokio::test]
#[ignore = "requires MINDFORK_ENGINE_URL + MINDFORK_ENGINE_KEY + MINDFORK_LIVE_SWITCH_OFF_MODEL (a gateway model that reasons by default)"]
async fn a_gateway_stops_reasoning_when_the_switch_is_off() {
    let Some((model, text, finish, reasoning)) =
        switch_smoke("MINDFORK_LIVE_SWITCH_OFF_MODEL", false).await
    else {
        return;
    };
    assert!(finish.is_some() && !text.is_empty(), "{model}: {text:?}");
    assert_eq!(reasoning, 0, "{model} was told not to reason");
}

/// L3: the switch off against a model that must reason, declared by the variable
/// the silent turns' smoke reads. Its catalogue says `mandatory: true`, so no
/// `enabled: false` goes out — which R1 answers with a `400` — and the ordinary
/// turn completes, reasoning as it must.
#[tokio::test]
#[ignore = "requires MINDFORK_ENGINE_URL + MINDFORK_ENGINE_KEY + MINDFORK_LIVE_MANDATORY_REASONING_MODEL (a model that must reason)"]
async fn the_switch_off_completes_on_an_endpoint_that_must_reason() {
    let Some((model, text, finish, _)) =
        switch_smoke("MINDFORK_LIVE_MANDATORY_REASONING_MODEL", false).await
    else {
        return;
    };
    assert!(
        finish.is_some() && !text.is_empty(),
        "the ordinary turn must complete on {model}: finish={finish:?} text={text:?}"
    );
}

/// Extensions "for variety": dynamic temperature, adaptive-p,
/// DRY breakers, and a custom sampler order — all in the body of one request.
/// The goal — confirm `llama-server` **accepts** these fields (doesn't respond
/// `400`/an error) and generates. The keys were checked against
/// `tools/server/server-schema.cpp` (dynatemp_range/exponent, adaptive_target/
/// decay, dry_sequence_breakers — non-empty, samplers — an array of names). If the
/// server had rejected any field, `chat_stream` would have returned a status error (the client doesn't
/// swallow the error body) and the test would fail at `.unwrap()`.
///
/// We check the **combined** stream (`text` + `thoughts`): for a reasoning model
/// (Gemma with thinking "baked in") the reply may go entirely into `reasoning_content`,
/// with `content` staying empty and `finish_reason="length"` — that's normal and has
/// nothing to do with accepting the sampling fields (see docs/journal/engine.md, the
/// reasoning-budget trap). `max_tokens` is generous so generation is visible.
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn accepts_creative_sampling_extensions() {
    let Some(client) = client_from_env() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let req = ChatRequest {
        continue_final: false,
        system: Some("You are a creative writing assistant.".into()),
        messages: vec![ApiMessage::user(
            "Write one whimsical sentence about a teapot.",
        )],
        sampling: SamplingConfig {
            temperature: Some(1.0),
            // Dynamic temperature: ±0.5 around temperature.
            dynatemp_range: Some(0.5),
            dynatemp_exponent: Some(1.0),
            // adaptive-p: a positive target enables the sampler (≤1.0).
            adaptive_target: Some(0.1),
            adaptive_decay: Some(0.9),
            // DRY with a non-empty breaker list (the server would reject an empty one).
            dry_multiplier: Some(0.8),
            dry_sequence_breakers: Some(vec!["\n".into(), ":".into()]),
            // A custom sampler order (valid names from sampling.cpp).
            samplers: Some(vec![
                "penalties".into(),
                "dry".into(),
                "top_k".into(),
                "top_p".into(),
                "min_p".into(),
                "temperature".into(),
            ]),
            max_tokens: Some(256),
            ..Default::default()
        },
        tools: vec![],
    };
    let (text, thoughts, finish) =
        collect(client.chat_stream(req, Default::default()).await.unwrap()).await;
    // A reasoning model puts the reply into "thoughts" — check both streams.
    let combined = format!("{thoughts}{text}");
    assert!(
        !combined.trim().is_empty(),
        "server accepted extensions but generated nothing: finish={finish:?}"
    );
    assert!(
        matches!(finish, Some(FinishReason::Stop | FinishReason::Length)),
        "unexpected finish reason: {finish:?}"
    );
}

/// Conversation control tools (followup/rewrite, spec §9.3.3): the live model
/// must **call** the one the instruction asks for — the name is parsed out of
/// `delta.tool_calls`. This is the feature's key unknown (will the model
/// understand the schema/description). Schemas are taken straight from the `Tool`
/// implementations (real descriptions).
///
/// **Both tools are asked for, in turn, and both schemas are offered every
/// time.** Until 2026-08-16 the pair was handed to the model and only
/// `send_followup_message` was ever asserted, so `rewrite_current_message`'s
/// description was shown and never checked — a refactor could have broken it
/// silently. Offering both in each case also makes the assertion stronger than
/// "a tool was called": the model has to pick the *right* one of two.
/// Measured on `gemma-4-31B_q4_0-it`: 0 failures in 10.
///
/// **Thinking is off** (`reasoning_budget=0`, which the wire also signals as
/// `chat_template_kwargs.enable_thinking=false` for Jinja templates). Unlike
/// [`simple_generation`], a larger ceiling does not fix this one: the prompt is
/// open-ended ("tell me a space fact, *then* call the tool"), so a reasoning
/// model deliberates without bound. Measured on `Qwen3.6-27B` q4_K_M — 1024:
/// 0/3 runs called the tool, 2048: 2/3, 4096: 3/4, with a failing run burning
/// the whole 4096-token budget on `reasoning_content` over 129 s. With thinking
/// off: 3/3 in ~1.5 s. Nothing is lost by muting it, because what this smoke
/// asks is whether the model understands the *schema*; the tool-call path
/// *through* thinking belongs to the orchestrator smokes, which run it on the
/// real app path (see [`tool_call_is_emitted_and_parsed`], muted for a
/// separate reason and for the same reference).
#[tokio::test]
#[ignore = "requires a running OpenAI-compatible server (MINDFORK_ENGINE_URL)"]
async fn control_tools_are_callable() {
    use crate::features::tools::Tool;
    use crate::features::tools::control::{RewriteCurrentMessage, SendFollowupMessage};
    let Some(client) = client_from_env() else {
        eprintln!("skip: MINDFORK_ENGINE_URL not set");
        return;
    };
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru);
    // Both schemas are offered every time, so each case also proves the model
    // picks the *right* one out of the pair rather than the only one on offer.
    let tools = vec![
        SendFollowupMessage.schema(loc),
        RewriteCurrentMessage.schema(loc),
    ];
    for (system, user, expected) in [
        (
            "Ты — дружелюбный ассистент. Ответь на сообщение пользователя \
             короткой первой репликой, а затем ОБЯЗАТЕЛЬНО вызови инструмент \
             send_followup_message, чтобы добавить вторую реплику с подробностями.",
            "Расскажи интересный факт о космосе.",
            "send_followup_message",
        ),
        (
            "Ты — ассистент. Черновик твоего ответа никуда не годится. \
             ОБЯЗАТЕЛЬНО вызови инструмент rewrite_current_message, чтобы \
             отбросить начатый ответ и написать его заново.",
            "Сколько будет два плюс два?",
            "rewrite_current_message",
        ),
    ] {
        let req = ChatRequest {
            continue_final: false,
            system: Some(system.into()),
            messages: vec![ApiMessage::user(user)],
            sampling: SamplingConfig {
                max_tokens: Some(512),
                reasoning_budget: Some(0),
                ..Default::default()
            },
            tools: tools.clone(),
        };
        let mut stream = client.chat_stream(req, Default::default()).await.unwrap();
        let mut acc = ToolCallAccumulator::default();
        let mut finish = None;
        while let Some(chunk) = stream.next().await {
            match chunk {
                ChatChunk::ToolCall(delta) => acc.push(delta),
                ChatChunk::Finished(reason) => {
                    finish = Some(reason);
                    break;
                }
                _ => {}
            }
        }
        let calls = acc.finish();
        assert!(
            calls.iter().any(|c| c.name == expected),
            "the model did not call {expected}: finish={finish:?} calls={calls:?}"
        );
    }
}
