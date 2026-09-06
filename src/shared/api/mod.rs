//! Inference engine layer (`shared/api`). A provider-agnostic contract
//! ([`EngineBackend`]/[`Embedder`] in [`contract`]) and its implementations, grouped
//! by family: [`openai`] (local/external `llama-server`, OpenAI cloud via
//! Responses), [`anthropic`] (Claude, Messages API), [`gemini`] (Gemini, native
//! generateContent), [`managed`] (launching a child `llama-server`).
//! See spec §6 and [ADR 0004](../../../docs/decisions/0004-engine-contract-multi-provider.md).

pub mod anthropic;
pub mod contract;
pub mod error;
pub mod gemini;
pub mod http;
pub mod managed;
pub mod openai;
pub mod retry;
pub mod thoughts;

// Production since the demo mode (`mindfork demo`) boots on it; tests were
// the original and remain the main consumer.
pub mod mock;

// Stage-0 live probe for `/continue` (docs/research/continue-generation.md §7).
#[cfg(test)]
mod continue_probe;

// Stage-0 live probe for the two-agent dialogue
// (docs/research/two-agent-dialogue.md §5).
#[cfg(test)]
mod dialogue_probe;

pub use anthropic::AnthropicClient;
pub use contract::{
    ApiImage, ApiMessage, ApiToolCall, ChatChunk, ChatRequest, EmbedRole, Embedder, EngineBackend,
    FinishReason, ThinkingAccumulator, ThinkingBlock, ThinkingRef, ToolCallAccumulator, ToolSchema,
    UnavailableEmbedder, VisionSupport,
};
pub use gemini::GeminiClient;
pub use managed::{ManagedConfig, ServerHandle, wait_until_ready};
pub use openai::{OpenAiClient, ResponsesClient};

/// A client to a live OpenAI-compatible server for the `#[ignore]` smokes,
/// named by a pair of env variables: the URL and an **optional** Bearer key.
///
/// `None` when the URL variable is unset — the smoke skips, as before. An unset
/// or empty key variable sends no `Authorization` header, i.e. byte-for-byte the
/// previous behaviour against a local `llama-server`; setting it lets the same
/// smokes run against an authenticated server (a hosted endpoint, a proxy). See
/// [docs/history/remote-e2e-hf.md](../../../docs/history/remote-e2e-hf.md) §5.
#[cfg(test)]
pub(crate) fn live_client(url_var: &str, key_var: &str) -> Option<OpenAiClient> {
    let url = std::env::var(url_var).ok()?;
    Some(OpenAiClient::new(url).with_api_key(std::env::var(key_var).ok()))
}

/// The `Authorization` header for a smoke that speaks to the engine **directly**
/// rather than through [`OpenAiClient`] — a probe reading a field the client
/// drops (`timings`), or one composing a request body by hand.
///
/// The same rule as [`live_client`]: no key variable, no header, i.e. byte for
/// byte the previous behaviour against a local `llama-server`. It exists because
/// the first gpt-oss dispatch found four such smokes answering `401` on the
/// rented gate — they had been written against an unauthenticated LAN stand and
/// had never met an authenticated server (docs/research/e2e-gpt-oss-120b.md).
#[cfg(test)]
pub(crate) fn live_bearer(rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match std::env::var("MINDFORK_ENGINE_KEY") {
        Ok(k) if !k.is_empty() => rb.bearer_auth(k),
        _ => rb,
    }
}

/// The stack under test has **no vision projector to give**, and the run says so
/// in writing.
///
/// Three smokes need a projector and *fail loudly rather than skip* against a
/// text-only server, deliberately: a vision smoke that quietly passes on a blind
/// model is worse than none ([docs/lessons.md](../../../docs/lessons.md) §9).
/// That rule assumes the stack could have been given one — the gate's two chat
/// models both ship a projector in the same repository as their weights, so a
/// blind run there is a misconfiguration, which is exactly what the rule is
/// meant to catch.
///
/// A gate model that has no projector in existence (`gpt-oss-120b` is text-only)
/// falls outside the assumption rather than outside the rule. So it is declared:
/// `MINDFORK_LIVE_TEXT_ONLY=1` turns those three smokes into skips and changes
/// nothing else. It is set by the model record that knows it is blind, never by
/// the standard gate, and every skip prints the variable's name — a skip the run
/// has to ask for, by name, in the log, is not the silent pass the rule guards
/// against. See docs/research/e2e-gpt-oss-120b.md §7 (fork F3).
#[cfg(test)]
pub(crate) fn live_text_only() -> bool {
    // Any non-empty value that is not an explicit denial: this is read from a CI
    // workflow's environment block, where "true" and "1" are equally idiomatic
    // and an accidental "0" should not silently disable three smokes.
    match std::env::var("MINDFORK_LIVE_TEXT_ONLY") {
        Ok(v) => !matches!(v.trim(), "" | "0" | "false"),
        Err(_) => false,
    }
}

/// The stack under test serves a model whose weights are **split across several
/// files**, and the run says so.
///
/// The declaration is what makes the check worth anything. A smoke that merely
/// asserted "the reported name has no part number" would pass on every
/// single-file stack without ever meeting a split one — which is where the
/// gate stood until `gpt-oss-120b` joined it: [`crate::shared::gguf`] parses,
/// rebuilds and strips the `-00001-of-00002` tail, and every assertion about it
/// was a unit test over strings.
///
/// So `MINDFORK_LIVE_SPLIT_MODEL=1` says "this server is holding a split model",
/// and the smoke that reads it **fails rather than skips** if the server turns
/// out to report a plain file — the same discipline as the vision smokes
/// ([docs/lessons.md](../../../docs/lessons.md) §9), for the same reason: a
/// declaration that is quietly wrong is worse than no declaration.
/// See docs/research/e2e-gpt-oss-120b.md §6 (T2).
#[cfg(test)]
pub(crate) fn live_split_model() -> bool {
    match std::env::var("MINDFORK_LIVE_SPLIT_MODEL") {
        Ok(v) => !matches!(v.trim(), "" | "0" | "false"),
        Err(_) => false,
    }
}

/// The fixture every provider's vision smoke sends: a 512×512 blue field with a large
/// white square in the middle, as base64 png (spec §9.10).
///
/// Generated rather than committed, and geometric rather than photographic, for the same
/// reason the compaction smokes plant an invented code: the assertion has to be something
/// no model could answer from pretraining, and something two humans would describe the
/// same way. One helper for all four smokes so the four wire formats are compared on
/// identical bytes — if one provider disagrees, the difference is the format, not the
/// picture.
#[cfg(test)]
pub(crate) fn blue_square_png_base64() -> String {
    use base64::Engine as _;
    let buf = image::ImageBuffer::from_fn(512, 512, |x, y| {
        if (160..352).contains(&x) && (160..352).contains(&y) {
            image::Rgb([255u8, 255, 255])
        } else {
            image::Rgb([20u8, 60, 200])
        }
    });
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(buf)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .expect("encoding the fixture png cannot fail");
    base64::engine::general_purpose::STANDARD.encode(&bytes)
}

/// What every vision smoke asks, and what its answer has to contain. Kept next to the
/// fixture so a change to the picture cannot drift away from the question about it.
#[cfg(test)]
pub(crate) const VISION_PROMPT: &str = "What is the background colour of this image, and what shape is in the centre? \
     Answer in a few words.";

/// The fixture the **tool-result** image smokes send: a green field with a large white
/// circle, as base64 png (spec §9.10).
///
/// Deliberately a *different* shape and colour from [`blue_square_png_base64`], and the
/// reason is recorded in docs/research/mcp-tool-images.md §2.1: with a blue-square
/// fixture every arm answered "blue background, white square" — including a control that
/// was sent **no image at all**. The model was answering the question, not the picture.
/// A fixture a plausible guess does not match is what makes these smokes able to fail.
#[cfg(test)]
pub(crate) fn green_circle_png_base64() -> String {
    use base64::Engine as _;
    let buf = image::ImageBuffer::from_fn(256, 256, |x, y| {
        let (dx, dy) = (x as i64 - 128, y as i64 - 128);
        if dx * dx + dy * dy < 60 * 60 {
            image::Rgb([255u8, 255, 255])
        } else {
            image::Rgb([30u8, 160, 60])
        }
    });
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(buf)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .expect("encoding the fixture png cannot fail");
    base64::engine::general_purpose::STANDARD.encode(&bytes)
}

/// What a tool-result image smoke asks about [`green_circle_png_base64`].
#[cfg(test)]
pub(crate) const TOOL_VISION_PROMPT: &str = "What is the background colour of the screenshot, and what shape is in the centre? \
     Answer briefly.";

/// Asserts an answer really describes [`green_circle_png_base64`] — and, for the control
/// arm, that it does **not**.
///
/// The control is the whole point: a model that cannot see the image still answers
/// confidently (measured: "light blue background, white five-pointed star"), so a smoke
/// without one passes on a feature that never worked.
#[cfg(test)]
pub(crate) fn assert_sees_green_circle(answer: &str, expected: bool, label: &str) {
    let lower = answer.to_lowercase();
    let saw = lower.contains("green") && lower.contains("circle");
    assert_eq!(
        saw, expected,
        "{label}: expected sees_green_circle={expected}, got {answer:?}"
    );
}

/// Asserts a vision answer really describes [`blue_square_png_base64`].
///
/// Both halves matter: a model that sees nothing still tends to produce a fluent
/// sentence, and one that only guesses "blue" from the prompt's wording would miss the
/// shape. Requiring both is what makes the smoke fail when an image never arrives.
#[cfg(test)]
pub(crate) fn assert_sees_blue_square(answer: &str, provider: &str) {
    let lower = answer.to_lowercase();
    assert!(
        lower.contains("blue"),
        "{provider}: the background colour is missing from the answer: {answer:?}"
    );
    assert!(
        lower.contains("square"),
        "{provider}: the centred shape is missing from the answer: {answer:?}"
    );
}
