//! The reasoning-effort scale across providers: the ladder of depths, what a
//! model's refusal says it takes, and the nearest of those to what was asked.
//!
//! One scale is chosen in the settings and translated per wire (spec §8.1), and
//! no two models take the same part of it — measured on 2026-10-01, ten OpenAI
//! models take five different sets (docs/research/effort-tiers.md §2). Nothing
//! publishes a model's set ahead of the request; the refusal does, in its own
//! words. So a client asks as chosen, and a value the model refuses becomes the
//! nearest it has ([`nearest`]), remembered for that client ([`EffortMemo`]).

use std::sync::{Mutex, PoisonError};

use tokio_util::sync::CancellationToken;

use super::error::EngineError;

/// The request to turn reasoning **off** — a switch, not a depth, and the one
/// value of the scale a model may simply not have.
pub(crate) const NONE: &str = "none";

/// The depths, lowest first. [`NONE`] is not among them. Also the vocabulary a
/// gateway's catalogue lists a model's efforts in.
pub(crate) const LADDER: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];

/// The values a refusal lists as supported, in the scale's order — [`NONE`]
/// first when it is among them. Empty when the refusal lists nothing this
/// module can read.
///
/// Two providers, two spellings, both measured:
///
/// - OpenAI: *"Unsupported value: 'minimal' is not supported with the
///   'gpt-6.1-sol' model. Supported values are: 'low', 'medium', 'high',
///   'xhigh', and 'max'."*
/// - Anthropic: *"This model does not support effort level 'xhigh'. Supported
///   levels: high, low, max, medium."*
///
/// Only the sentence after the **last** "supported values/levels" is read, up
/// to its full stop: the refused value is quoted earlier in the same message,
/// and a scan of the whole text would list the very value that was refused.
pub(crate) fn listed(message: &str) -> Vec<&'static str> {
    let lower = message.to_lowercase();
    let Some(at) = ["supported values", "supported levels"]
        .iter()
        .filter_map(|marker| lower.rfind(marker).map(|at| at + marker.len()))
        .max()
    else {
        return Vec::new();
    };
    let sentence = lower[at..].split('.').next().unwrap_or_default();
    let words: Vec<&str> = sentence
        .split(|c: char| !c.is_ascii_alphanumeric())
        .collect();
    std::iter::once(NONE)
        .chain(LADDER)
        .filter(|value| words.contains(value))
        .collect()
}

/// What to ask for in place of `wish`, given what the model [`listed`] — or
/// `None` when it lists nothing that can stand in.
///
/// - A **depth** becomes the nearest depth on the ladder, the lower one on a
///   tie: never more reasoning than was asked for where less is as near. It
///   never becomes [`NONE`] — "reason a little" is not "do not reason".
/// - [`NONE`] becomes the lowest depth listed: the closest thing to "off" a
///   model that cannot be switched off has. Measured on `gpt-6.1-sol` with the
///   title's body: `low` reasons 0 tokens in 5 of 5, where the field left out
///   means the model's default `medium`, 29–96.
pub(crate) fn nearest(wish: &str, listed: &[&'static str]) -> Option<&'static str> {
    let depths = LADDER
        .iter()
        .enumerate()
        .filter(|(_, d)| listed.contains(d));
    if wish == NONE {
        return depths.map(|(_, d)| *d).next();
    }
    let asked = LADDER.iter().position(|d| *d == wish)?;
    depths
        .min_by_key(|(at, _)| (at.abs_diff(asked), *at))
        .map(|(_, d)| *d)
}

/// What a client has learned about its model: a value it refused, and what was
/// accepted in its place — `None` in the pair for "no effort at all".
///
/// Per client, and never cleared: a client is one model's, and a changed model
/// gets a new client. Written only once the substitute was **accepted**, so a
/// refusal read wrongly costs one round trip and teaches the client nothing.
#[derive(Debug, Default)]
pub(crate) struct EffortMemo(Mutex<Vec<(&'static str, Option<&'static str>)>>);

impl EffortMemo {
    /// What to send in place of `wish`: `None` — nothing is known, send it as
    /// it is; `Some(instead)` — send that (`Some(None)`: no effort at all).
    pub(crate) fn instead_of(&self, wish: &str) -> Option<Option<&'static str>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|(refused, _)| *refused == wish)
            .map(|(_, instead)| *instead)
    }

    /// Records that `wish` was refused and `instead` accepted. The first answer
    /// for a value stands.
    pub(crate) fn learn(&self, wish: &'static str, instead: Option<&'static str>) {
        let mut known = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if !known.iter().any(|(refused, _)| *refused == wish) {
            known.push((wish, instead));
        }
    }
}

/// A request body that carries one value of the effort scale — in whatever
/// field and spelling its wire has.
pub(crate) trait CarriesEffort {
    /// The value the body carries, when it carries one.
    fn effort(&self) -> Option<&'static str>;
    /// Asks for `instead` in place of that value; `None` — for no effort at all.
    fn respell_effort(&mut self, instead: Option<&'static str>);
}

/// A client whose wire carries a value of the effort scale, and whose models
/// may refuse it: what [`send_asking_again`] needs of one.
pub(crate) trait EffortWire: Sync {
    type Body: CarriesEffort + Send + Sync;

    /// The model's name, for the log.
    fn model(&self) -> &str;

    /// What this client has learned about its model.
    fn efforts(&self) -> &EffortMemo;

    /// One attempt at the request. `Ok(None)` — the cancellation token fired
    /// before a response arrived.
    fn send(
        &self,
        body: &Self::Body,
        cancel: &CancellationToken,
    ) -> impl Future<Output = Result<Option<reqwest::Response>, EngineError>> + Send;

    /// What a refusal says about `wish`, the value that was sent: `None` — it
    /// is not a refusal of that value, or there is nothing to offer in its
    /// place, and the error is the turn's; `Some(instead)` — ask once more with
    /// that (`Some(None)`: with no effort at all).
    fn answer(&self, wish: &'static str, refused: &EngineError) -> Option<Option<&'static str>>;
}

/// Sends `body`, and answers the one refusal worth answering rather than
/// reporting: the model does not have the effort value the body carries.
///
/// What the client already knows is applied before the first request. A
/// refusal goes to [`EffortWire::answer`]; on `Some(instead)` the request is
/// made **once** more with it. What was sent instead is remembered only once it
/// was **accepted**: a refusal read wrongly costs one round trip, the second
/// error surfaces as it came, and the next turn starts from the value as chosen
/// again.
///
/// One place for that rule because three wires need it — Responses, Gemini and
/// Anthropic each refuse values of their own
/// (docs/research/effort-tiers.md §2).
pub(crate) async fn send_asking_again<W: EffortWire>(
    wire: &W,
    body: &mut W::Body,
    cancel: &CancellationToken,
) -> Result<Option<reqwest::Response>, EngineError> {
    let memo = wire.efforts();
    if let Some(instead) = body.effort().and_then(|wish| memo.instead_of(wish)) {
        body.respell_effort(instead);
    }
    let sent = body.effort();
    let first = wire.send(body, cancel).await;
    let (Some(wish), Err(refused)) = (sent, &first) else {
        return first;
    };
    let Some(instead) = wire
        .answer(wish, refused)
        .filter(|instead| *instead != Some(wish))
    else {
        return first;
    };
    tracing::info!(
        model = wire.model(),
        refused = wish,
        ?instead,
        "the model does not take this reasoning effort; asking again with the nearest it has"
    );
    body.respell_effort(instead);
    let second = wire.send(body, cancel).await;
    if matches!(second, Ok(Some(_))) {
        memo.learn(wish, instead);
    }
    second
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refusals as the providers wrote them on 2026-10-01.
    const OPENAI_NONE: &str = "Unsupported value: 'none' is not supported with the 'gpt-6.1-sol' model. Supported values are: 'low', 'medium', 'high', 'xhigh', and 'max'.";
    const OPENAI_MINIMAL: &str = "Unsupported value: 'minimal' is not supported with the 'gpt-5.5' model. Supported values are: 'none', 'low', 'medium', 'high', and 'xhigh'.";
    const OPENAI_O4: &str = "Unsupported value: 'xhigh' is not supported with the 'o4-mini' model. Supported values are: 'low', 'medium', and 'high'.";
    const ANTHROPIC_XHIGH: &str = "This model does not support effort level 'xhigh'. Supported levels: high, low, max, medium.";

    #[test]
    fn the_list_is_read_in_both_spellings_and_in_the_scales_order() {
        assert_eq!(
            listed(OPENAI_NONE),
            ["low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(
            listed(OPENAI_MINIMAL),
            ["none", "low", "medium", "high", "xhigh"]
        );
        assert_eq!(listed(ANTHROPIC_XHIGH), ["low", "medium", "high", "max"]);
    }

    /// The defect a scan of the whole message would be: the refused value is
    /// quoted in it, and so is the model's name.
    #[test]
    fn the_refused_value_is_not_read_as_listed() {
        assert!(!listed(OPENAI_MINIMAL).contains(&"minimal"));
        assert!(!listed(ANTHROPIC_XHIGH).contains(&"xhigh"));
    }

    /// The body as the client holds it: the message inside its JSON envelope,
    /// whose other fields name the parameter and are not values on offer.
    #[test]
    fn the_envelope_around_the_message_adds_nothing() {
        let body = format!(
            r#"engine (OpenAI Responses) returned status 400 Bad Request: {{"error": {{"message": "{OPENAI_NONE}", "type": "invalid_request_error", "param": "reasoning.effort", "code": "unsupported_value"}}}}"#
        );
        assert_eq!(listed(&body), ["low", "medium", "high", "xhigh", "max"]);
    }

    #[test]
    fn a_refusal_that_lists_nothing_reads_as_nothing() {
        assert!(listed("Invalid reasoning effort.").is_empty());
        assert!(listed("Unsupported value: 'none' with the 'gpt-x' model.").is_empty());
        assert!(listed("the budget is too low").is_empty());
    }

    #[test]
    fn a_depth_becomes_the_nearest_listed_the_lower_on_a_tie() {
        // `minimal` on a model that starts at `low`.
        assert_eq!(nearest("minimal", &listed(OPENAI_NONE)), Some("low"));
        // `minimal` where `none` is listed too: a depth never becomes the switch.
        assert_eq!(nearest("minimal", &listed(OPENAI_MINIMAL)), Some("low"));
        // `max` above a model's top.
        assert_eq!(nearest("max", &listed(OPENAI_MINIMAL)), Some("xhigh"));
        assert_eq!(nearest("xhigh", &listed(OPENAI_O4)), Some("high"));
        // `xhigh` between `high` and `max`: as near either way, so the lower.
        assert_eq!(nearest("xhigh", &listed(ANTHROPIC_XHIGH)), Some("high"));
    }

    #[test]
    fn none_becomes_the_lowest_depth_listed() {
        assert_eq!(nearest(NONE, &listed(OPENAI_NONE)), Some("low"));
        assert_eq!(nearest(NONE, &["medium", "high"]), Some("medium"));
    }

    #[test]
    fn nothing_listed_offers_nothing() {
        assert_eq!(nearest(NONE, &[]), None);
        assert_eq!(nearest("minimal", &[]), None);
        // A list of the switch alone has no depth to stand in for a depth.
        assert_eq!(nearest("high", &[NONE]), None);
        // A word that is not on the scale has no place to be near.
        assert_eq!(nearest("ultra", &["low", "high"]), None);
    }

    #[test]
    fn the_memo_keeps_the_first_answer_per_value() {
        let memo = EffortMemo::default();
        assert_eq!(memo.instead_of("minimal"), None);
        memo.learn("minimal", Some("low"));
        memo.learn(NONE, None);
        memo.learn("minimal", Some("high"));
        assert_eq!(memo.instead_of("minimal"), Some(Some("low")));
        assert_eq!(memo.instead_of(NONE), Some(None));
        assert_eq!(memo.instead_of("max"), None);
    }
}
