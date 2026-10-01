//! Sampling parameters (see spec §8). Fields are serialized into the
//! `/v1/chat/completions` request body. Besides the standard OpenAI fields
//! (`temperature`, `top_p`, `frequency_penalty`, …), this holds llama.cpp
//! `llama-server` extensions (`min_p`, `top_n_sigma`, DRY, XTC, `repeat_penalty`,
//! `seed`, mirostat) — `llama-server` accepts them straight in the request body;
//! a server that doesn't understand a field simply ignores it (a strict
//! third-party OpenAI server might reject it — hence the extensions stay `None`
//! until the user sets them).

use serde::{Deserialize, Serialize};

use crate::shared::config::CloudProvider;

/// The reasoning-effort level: one scale for every provider, translated per wire
/// (spec §8.1). `None` is a switch — do not reason; the rest are depths, lowest
/// first. No model takes the whole scale — ten OpenAI models take five different
/// parts of it — so a value a model does not have becomes the nearest it has
/// (`shared::api::effort`, docs/research/effort-tiers.md). Variant order = the UI
/// cycle order **and** the scale's own: the derived `Ord` is what a ceiling
/// compares with.
///
/// **A new variant is a new stored value.** The effort is written into
/// `settings.json`, a profile's defaults and every chat file (the override and
/// each reply's `metadata.sampling`), and it is a strict enum: an older binary
/// fails the parse of the whole file on a value it does not know — for a chat
/// file that means the chat is missing from its list. So a variant arrives
/// with a schema step (`shared::storage::schema`, where a test names the
/// schema each variant arrived in and does not compile for one it has not been
/// told about); `Max` arrived in 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl ReasoningEffort {
    /// The string representation for the HTTP field `reasoning_effort` / `reasoning.effort`.
    pub fn as_wire(self) -> &'static str {
        match self {
            ReasoningEffort::None => "none",
            ReasoningEffort::Minimal => "minimal",
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
            ReasoningEffort::XHigh => "xhigh",
            ReasoningEffort::Max => "max",
        }
    }
}

/// Reply verbosity (OpenAI Responses `text.verbosity`): controls reply length
/// separately from temperature. OpenAI Responses only; other backends ignore it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verbosity {
    Low,
    Medium,
    High,
}

impl Verbosity {
    /// The string representation for the HTTP field `text.verbosity`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Verbosity::Low => "low",
            Verbosity::Medium => "medium",
            Verbosity::High => "high",
        }
    }
}

/// Sampling configuration for a generation request.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SamplingConfig {
    pub temperature: Option<f32>,
    /// Dynamic temperature (llama.cpp `dynatemp_range`): the width of the ± range
    /// around `temperature`, adapted per-token by the distribution's entropy
    /// (confident positions — colder, ambiguous ones — hotter).
    /// `0.0` = off (regular static temperature).
    pub dynatemp_range: Option<f32>,
    /// The dynamic-temperature curve exponent (llama.cpp
    /// `dynatemp_exponent`, server default `1.0`).
    pub dynatemp_exponent: Option<f32>,
    pub top_k: Option<i64>,
    pub top_p: Option<f32>,
    /// min-p (llama.cpp): cuts off tokens with probability below a fraction of the maximum.
    pub min_p: Option<f32>,
    /// top-n-sigma (llama.cpp `top_n_sigma`): cutoff by number of σ from the max
    /// logit (`-1` = off).
    pub top_n_sigma: Option<f32>,
    /// locally typical sampling (llama.cpp `typical_p`, `1.0` = off).
    pub typical_p: Option<f32>,
    /// adaptive-p (llama.cpp `adaptive_target`, PR #17927): the target entropy
    /// tokens are chosen around; a negative value = off (valid range `≤ 1.0`).
    /// Checked against `server-schema.cpp` (a flat request-body key). A new
    /// sampler — verify behavior against a live model.
    pub adaptive_target: Option<f32>,
    /// adaptive-p (llama.cpp `adaptive_decay`): EMA decay of the target's
    /// adaptation (hard range `0.0`..`0.99`; lower — more reactive, higher — more
    /// stable).
    pub adaptive_decay: Option<f32>,
    pub frequency_penalty: Option<f32>,
    pub presence_penalty: Option<f32>,
    /// Penalty for repeating a token sequence (llama.cpp `repeat_penalty`,
    /// `1.0` = off). Separate from the OpenAI presence/frequency penalties.
    pub repeat_penalty: Option<f32>,
    /// How many recent tokens to account for `repeat_penalty` (llama.cpp
    /// `repeat_last_n`; `0` = off, `-1` = the whole context).
    pub repeat_last_n: Option<i64>,
    /// DRY: the penalty multiplier (llama.cpp `dry_multiplier`, `0.0` = off).
    pub dry_multiplier: Option<f32>,
    /// DRY: the exponent base (llama.cpp `dry_base`).
    pub dry_base: Option<f32>,
    /// DRY: the allowed repeat length before a penalty kicks in (llama.cpp `dry_allowed_length`).
    pub dry_allowed_length: Option<i64>,
    /// DRY: how many recent tokens to scan (llama.cpp `dry_penalty_last_n`;
    /// `0` = off, `-1` = the whole context).
    pub dry_penalty_last_n: Option<i64>,
    /// DRY: "breakers" — strings that reset repeat tracking (llama.cpp
    /// `dry_sequence_breakers`). `None`/empty — the server defaults
    /// (`\n`, `:`, `"`, `*`). Sent only when non-empty.
    pub dry_sequence_breakers: Option<Vec<String>>,
    /// XTC: the sampler's application probability (llama.cpp `xtc_probability`,
    /// `0.0` = off).
    pub xtc_probability: Option<f32>,
    /// XTC: the probability threshold (llama.cpp `xtc_threshold`).
    pub xtc_threshold: Option<f32>,
    /// Mirostat: the mode (llama.cpp `mirostat`; `0` = off, `1`/`2` = versions).
    pub mirostat: Option<i64>,
    /// Mirostat: the target entropy τ (llama.cpp `mirostat_tau`).
    pub mirostat_tau: Option<f32>,
    /// Mirostat: the learning rate η (llama.cpp `mirostat_eta`).
    pub mirostat_eta: Option<f32>,
    pub max_tokens: Option<usize>,
    /// The RNG seed per request (llama.cpp `seed`; `-1` = random). `None` — the
    /// field isn't sent (the server picks its own).
    pub seed: Option<i64>,
    /// The sampler application order (llama.cpp `samplers`): sampler names in the
    /// desired order (e.g. `["penalties","dry","top_k","top_p","min_p",
    /// "temperature"]`). `None` — the server's default order. **Important:**
    /// a sampler not listed in a non-empty list is disabled — the list must be
    /// complete. Sent only when non-empty.
    pub samplers: Option<Vec<String>>,
    /// Enable reasoning ("thoughts", `<think>`/`reasoning_content`).
    pub thinking: Option<bool>,
    pub reasoning_effort: Option<ReasoningEffort>,
    /// The "thoughts" token budget (llama.cpp `reasoning_budget`): `0` —
    /// **fully disable** thinking, even for models with reasoning "baked into"
    /// the template (Gemma `peg-gemma4`, Qwen), `-1` — unlimited. `None` —
    /// the field isn't sent (the server's default behavior). See spec §8.
    pub reasoning_budget: Option<i64>,
    /// Reply verbosity (OpenAI Responses `text.verbosity`). `None` — the field
    /// isn't sent (the provider's default). Other backends ignore it.
    pub verbosity: Option<Verbosity>,
}

impl SamplingConfig {
    /// A copy with every field **unavailable** in the engine mode of provider
    /// `provider` zeroed out (`None`) (see [`supported_sampling_fields`]). Used for
    /// the `Message.metadata` snapshot: the engine wouldn't have accepted an
    /// unavailable field anyway, so it shouldn't land in "what was applied". List
    /// fields (`samplers`/`dry_sequence_breakers`) are handled as regular keys.
    pub fn retain_supported(
        &self,
        provider: Option<CloudProvider>,
        endpoint: Option<&[String]>,
    ) -> SamplingConfig {
        let supported = available_sampling_fields(provider, endpoint);
        // Serializing our own type doesn't fail; on the unexpected, return as-is.
        let Ok(serde_json::Value::Object(mut map)) = serde_json::to_value(self) else {
            return self.clone();
        };
        map.retain(|k, _| supported.contains(&k.as_str()));
        serde_json::from_value(serde_json::Value::Object(map)).unwrap_or_else(|_| self.clone())
    }
}

/// Resolves the actual sampling by priority (spec §8.3):
/// `Chat.sampling_override` → `Profile.default_sampling` → global.
///
/// Resolution is **whole-config** (not per-field): the first level that's set
/// wins. A snapshot of the result is saved into `Message.metadata`.
pub fn resolve(
    chat_override: Option<&SamplingConfig>,
    profile_default: Option<&SamplingConfig>,
    global: &SamplingConfig,
) -> SamplingConfig {
    chat_override
        .or(profile_default)
        .cloned()
        .unwrap_or_else(|| global.clone())
}

/// Names of the sampling JSON fields the model can control via the
/// `set_sampling` tool (order = the tool's JSON-schema order). The internal
/// `reasoning_budget` is **not** included here — it's not editable by the model.
pub const SETTABLE_SAMPLING_FIELDS: &[&str] = &[
    "temperature",
    "dynatemp_range",
    "dynatemp_exponent",
    "top_k",
    "top_p",
    "min_p",
    "top_n_sigma",
    "typical_p",
    "adaptive_target",
    "adaptive_decay",
    "frequency_penalty",
    "presence_penalty",
    "repeat_penalty",
    "repeat_last_n",
    "dry_multiplier",
    "dry_base",
    "dry_allowed_length",
    "dry_penalty_last_n",
    "dry_sequence_breakers",
    "xtc_probability",
    "xtc_threshold",
    "mirostat",
    "mirostat_tau",
    "mirostat_eta",
    "max_tokens",
    "seed",
    "samplers",
    "thinking",
    "reasoning_effort",
    "verbosity",
];

/// Names of the sampling fields the engine of the given mode actually accepts —
/// a mirror of the wire dialect (`shared/api/openai/wire::restrict_to_strict` and
/// `anthropic/wire`). `None` provider = local/external `llama.cpp`: accepts all
/// fields (it ignores extensions rather than rejecting them). The cloud is strict:
///
/// - **OpenAI** — `max_tokens` + reasoning (`thinking`/`reasoning_effort`) + `verbosity`.
///   The mode goes through the **Responses API** (`ResponsesClient`), which has no
///   `temperature`/`top_p`/`seed`/penalties (reasoning models reject them), but has
///   reasoning summaries and `text.verbosity`. See ADR 0004, docs/research/openai-responses-client.md;
/// - **Gemini** (native `generateContent`, [`GeminiClient`](crate::shared::api::gemini::GeminiClient))
///   — `temperature`/`top_p`/`top_k`/`max_tokens`/`seed`/`frequency_penalty`/
///   `presence_penalty` **+ reasoning** (`thinking`/`reasoning_effort`): the native API
///   accepts `top_k` (unlike the former compat path) and gives "thoughts" summaries
///   + `thinkingLevel`/`thinkingBudget`. No `verbosity` (that's OpenAI-Responses-specific);
/// - **Claude** — `max_tokens` + reasoning (`thinking`/`reasoning_effort`):
///   4.x models have "locked in" sampling (reject `temperature`/`top_p`/`top_k`),
///   but support extended thinking (`{type:"adaptive"}` + `output_config.effort`).
///   `reasoning_budget` isn't included here — 4.x models reject `budget_tokens`;
/// - **Grok** (xAI, plain Chat Completions via [`OpenAiClient`](crate::shared::api::OpenAiClient))
///   — `temperature`/`top_p`/`max_tokens`/`seed` + reasoning. Verified live against
///   `api.x.ai` (docs/research/grok-xai-provider.md §2.5): the penalties are a hard
///   `400` ("Model grok-4.5 does not support parameter presencePenalty"), while
///   `top_k`/`min_p`/`repeat_penalty` and every llama.cpp extension are **silently
///   dropped** — they aren't in xAI's request schema, so offering them would be a
///   lie rather than an error. No `verbosity` (OpenAI-Responses-specific);
/// - **OpenRouter** — what the gateway's own schema reads, measured by sending
///   each field with a value of the wrong type
///   (docs/research/openrouter-mode.md §3.5). This list is the **ceiling**, and
///   the one a model's catalogue entry narrows ([`available_sampling_fields`]):
///   which of these a given model takes is the entry's to say.
///
/// This is the single source of truth for the settings UI (`cloud_supported_param`)
/// and the `get_sampling`/`set_sampling` tools (show/change only what's available).
/// See ADR 0004.
/// The catalogue spelling(s) that would mean "this endpoint takes our field", for
/// the fields where an endpoint publishes a `supported_parameters` list
/// ([docs/history/gateway-capabilities.md](../../docs/history/gateway-capabilities.md)).
///
/// Two vocabularies, and they are not the same one: their `repetition_penalty` is
/// our `repeat_penalty` — the very mismatch that made that knob look set and do
/// nothing through a gateway — and their single `reasoning` covers both switches
/// we spell separately. A field with **no** catalogue name is not a mapping we
/// forgot: it is a llama.cpp extension no gateway publishes, and the whole point
/// of the narrowing is that such a field disappears from the offer once the
/// endpoint has spoken.
fn catalogue_aliases(field: &str) -> &'static [&'static str] {
    match field {
        // Same word in both vocabularies.
        "temperature" => &["temperature"],
        "top_k" => &["top_k"],
        "top_p" => &["top_p"],
        "min_p" => &["min_p"],
        "max_tokens" => &["max_tokens"],
        "seed" => &["seed"],
        "frequency_penalty" => &["frequency_penalty"],
        "presence_penalty" => &["presence_penalty"],
        "verbosity" => &["verbosity"],
        // Different word, same knob.
        "repeat_penalty" => &["repetition_penalty"],
        // One catalogue entry, two switches of ours: a gateway that publishes
        // `reasoning` takes a reasoning request, however we spell it.
        "thinking" | "reasoning_effort" => &["reasoning", "include_reasoning"],
        // llama.cpp's own: dynatemp, adaptive-p, typical-p, top-n-sigma, DRY,
        // XTC, mirostat, the sampler order, `repeat_last_n`.
        _ => &[],
    }
}

/// The sampling fields actually on offer: the mode's static set, narrowed by what
/// the **endpoint** published about the configured model, when it published
/// anything.
///
/// `endpoint` is the catalogue's own list
/// ([`ModelCapabilities::sampling_fields`](crate::shared::api::contract::ModelCapabilities)).
/// `None` — it said nothing, so nothing narrows and this is
/// [`supported_sampling_fields`] exactly, which is what every local
/// `llama-server` and every cloud gets. A **cloud** provider ignores the list
/// outright: its table is a compile-time fact about that protocol (ADR 0004), not
/// something a gateway's catalogue has standing to trim. The one exception is
/// the cloud that **is** a gateway: behind OpenRouter there is a different
/// model per slug, and the catalogue is the only thing that knows which.
pub fn available_sampling_fields(
    provider: Option<CloudProvider>,
    endpoint: Option<&[String]>,
) -> Vec<&'static str> {
    let base = supported_sampling_fields(provider);
    let Some(published) = endpoint
        .filter(|_| matches!(provider, None | Some(CloudProvider::OpenRouter)))
        .filter(|p| !p.is_empty())
    else {
        return base.to_vec();
    };
    base.iter()
        .copied()
        .filter(|field| {
            catalogue_aliases(field)
                .iter()
                .any(|alias| published.iter().any(|p| p == alias))
        })
        .collect()
}

pub fn supported_sampling_fields(provider: Option<CloudProvider>) -> &'static [&'static str] {
    match provider {
        None => SETTABLE_SAMPLING_FIELDS,
        Some(CloudProvider::OpenAi) => &["max_tokens", "thinking", "reasoning_effort", "verbosity"],
        Some(CloudProvider::Gemini) => &[
            "temperature",
            "top_p",
            "top_k",
            "max_tokens",
            "seed",
            "frequency_penalty",
            "presence_penalty",
            "thinking",
            "reasoning_effort",
        ],
        Some(CloudProvider::Claude) => &["max_tokens", "thinking", "reasoning_effort"],
        Some(CloudProvider::Grok) => &[
            "temperature",
            "top_p",
            "max_tokens",
            "seed",
            "thinking",
            "reasoning_effort",
        ],
        Some(CloudProvider::OpenRouter) => &[
            "temperature",
            "top_k",
            "top_p",
            "min_p",
            "max_tokens",
            "seed",
            "frequency_penalty",
            "presence_penalty",
            "repeat_penalty",
            "thinking",
            "reasoning_effort",
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalogue OpenRouter actually answered for `deepseek/deepseek-r1`,
    /// copied from the measurement rather than invented
    /// ([openrouter-external.md](../../docs/research/openrouter-external.md) §8.1, M5).
    fn r1_catalogue() -> Vec<String> {
        [
            "frequency_penalty",
            "include_reasoning",
            "max_tokens",
            "presence_penalty",
            "reasoning",
            "repetition_penalty",
            "response_format",
            "seed",
            "stop",
            "temperature",
            "tool_choice",
            "tools",
            "top_k",
            "top_p",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    /// What a gateway's published list does to the offer: the OpenAI-shaped
    /// fields survive, `repeat_penalty` survives **under the other name**, the
    /// reasoning switches survive on one entry, and every llama.cpp extension
    /// goes — which is the defect this narrowing exists for, since those were
    /// offered, set, sent and silently dropped.
    #[test]
    fn a_published_catalogue_narrows_the_offer_to_what_it_lists() {
        let fields = available_sampling_fields(None, Some(&r1_catalogue()));
        for kept in [
            "temperature",
            "top_p",
            "top_k",
            "max_tokens",
            "seed",
            "frequency_penalty",
            "presence_penalty",
            "repeat_penalty",
            "thinking",
            "reasoning_effort",
        ] {
            assert!(
                fields.contains(&kept),
                "{kept} is published, under some name"
            );
        }
        for dropped in [
            "min_p",
            "typical_p",
            "top_n_sigma",
            "dynatemp_range",
            "adaptive_target",
            "mirostat",
            "dry_multiplier",
            "xtc_probability",
            "samplers",
            "repeat_last_n",
        ] {
            assert!(
                !fields.contains(&dropped),
                "{dropped} is not in the catalogue and must stop being offered"
            );
        }
    }

    /// Silence never narrows: no list, an empty list, and a cloud provider (whose
    /// table is a fact about the protocol, not the endpoint's to trim) all leave
    /// the answer exactly as it was before this existed.
    #[test]
    fn silence_and_the_clouds_are_left_alone() {
        assert_eq!(
            available_sampling_fields(None, None),
            SETTABLE_SAMPLING_FIELDS.to_vec(),
            "no list — every field, as before"
        );
        assert_eq!(
            available_sampling_fields(None, Some(&[])),
            SETTABLE_SAMPLING_FIELDS.to_vec(),
            "an empty list is silence, not a claim that nothing is taken"
        );
        let openai = available_sampling_fields(Some(CloudProvider::OpenAi), Some(&r1_catalogue()));
        assert_eq!(
            openai,
            supported_sampling_fields(Some(CloudProvider::OpenAi)).to_vec(),
            "a cloud's table is not a gateway catalogue's business"
        );
    }

    #[test]
    fn default_is_all_none() {
        let s = SamplingConfig::default();
        assert!(s.temperature.is_none());
        assert!(s.max_tokens.is_none());
        assert!(s.reasoning_effort.is_none());
    }

    #[test]
    fn reasoning_effort_wire_strings() {
        assert_eq!(ReasoningEffort::Medium.as_wire(), "medium");
        assert_eq!(ReasoningEffort::None.as_wire(), "none");
        assert_eq!(ReasoningEffort::Max.as_wire(), "max");
        // The stored spelling is the wire's, and an older config reads as before.
        assert_eq!(
            serde_json::from_str::<ReasoningEffort>("\"max\"").unwrap(),
            ReasoningEffort::Max
        );
        assert_eq!(
            serde_json::to_string(&ReasoningEffort::XHigh).unwrap(),
            "\"xhigh\""
        );
        // The variants' order is the scale's: what a ceiling compares with.
        assert!(ReasoningEffort::Max > ReasoningEffort::XHigh);
        assert!(ReasoningEffort::Minimal > ReasoningEffort::None);
    }

    #[test]
    fn serde_roundtrip() {
        let s = SamplingConfig {
            temperature: Some(0.7),
            top_k: Some(40),
            thinking: Some(true),
            reasoning_effort: Some(ReasoningEffort::High),
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: SamplingConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn supported_fields_mirror_wire_dialect() {
        // Local (None) — the whole configurable set.
        assert_eq!(supported_sampling_fields(None), SETTABLE_SAMPLING_FIELDS);
        // OpenAI — Responses API: max_tokens + reasoning + verbosity; no
        // temperature/top_p/seed/penalties (reasoning models reject them).
        let openai = supported_sampling_fields(Some(CloudProvider::OpenAi));
        assert!(openai.contains(&"max_tokens"));
        assert!(openai.contains(&"thinking"));
        assert!(openai.contains(&"reasoning_effort"));
        assert!(openai.contains(&"verbosity"));
        assert!(!openai.contains(&"seed"));
        assert!(!openai.contains(&"temperature"));
        assert!(!openai.contains(&"top_p"));
        assert!(!openai.contains(&"top_k"));
        // Gemini (native generateContent): temperature/top_p/top_k/penalties/seed/
        // max_tokens + reasoning (thinking/reasoning_effort), but no verbosity.
        let gemini = supported_sampling_fields(Some(CloudProvider::Gemini));
        assert!(gemini.contains(&"temperature"));
        assert!(gemini.contains(&"top_p"));
        assert!(gemini.contains(&"top_k"));
        assert!(gemini.contains(&"seed"));
        assert!(gemini.contains(&"thinking"));
        assert!(gemini.contains(&"reasoning_effort"));
        assert!(!gemini.contains(&"verbosity"));
        // Claude — max_tokens + reasoning (thinking/reasoning_effort), but not top_k.
        let claude = supported_sampling_fields(Some(CloudProvider::Claude));
        assert!(claude.contains(&"max_tokens"));
        assert!(claude.contains(&"thinking"));
        assert!(claude.contains(&"reasoning_effort"));
        assert!(!claude.contains(&"top_k"));
        // Grok (xAI Chat Completions): temperature/top_p/max_tokens/seed + reasoning.
        // The penalties are excluded because xAI answers `400` for them — the one
        // provider where offering a field would break the request rather than be
        // ignored (docs/research/grok-xai-provider.md §2.5).
        let grok = supported_sampling_fields(Some(CloudProvider::Grok));
        assert!(grok.contains(&"temperature"));
        assert!(grok.contains(&"top_p"));
        assert!(grok.contains(&"seed"));
        assert!(grok.contains(&"max_tokens"));
        assert!(grok.contains(&"reasoning_effort"));
        assert!(!grok.contains(&"presence_penalty"));
        assert!(!grok.contains(&"frequency_penalty"));
        // Not in xAI's request schema — silently dropped, so don't offer them.
        assert!(!grok.contains(&"top_k"));
        assert!(!grok.contains(&"min_p"));
        assert!(!grok.contains(&"verbosity"));
        // The cloud subsets are indeed subsets of the full set.
        for f in openai.iter().chain(gemini).chain(claude).chain(grok) {
            assert!(SETTABLE_SAMPLING_FIELDS.contains(f));
        }
    }

    #[test]
    fn retain_supported_drops_fields_by_mode() {
        let s = SamplingConfig {
            temperature: Some(0.7),
            top_k: Some(40),
            min_p: Some(0.05),
            max_tokens: Some(256),
            thinking: Some(true),
            ..Default::default()
        };
        // Local (None) — llama.cpp accepts everything configurable: fields are kept.
        let local = s.retain_supported(None, None);
        assert_eq!(local.temperature, Some(0.7));
        assert_eq!(local.top_k, Some(40));
        assert_eq!(local.min_p, Some(0.05));
        assert_eq!(local.thinking, Some(true));
        // OpenAI (Responses): max_tokens + thinking remain; temperature/top_k/min_p
        // are zeroed (absent from the Responses API).
        let openai = s.retain_supported(Some(CloudProvider::OpenAi), None);
        assert_eq!(openai.max_tokens, Some(256));
        assert_eq!(openai.thinking, Some(true));
        assert_eq!(openai.temperature, None);
        assert_eq!(openai.top_k, None);
        assert_eq!(openai.min_p, None);
        // Gemini (native) accepts temperature/top_k/thinking; min_p (a llama.cpp
        // extension) is zeroed.
        let gemini = s.retain_supported(Some(CloudProvider::Gemini), None);
        assert_eq!(gemini.temperature, Some(0.7));
        assert_eq!(gemini.max_tokens, Some(256));
        assert_eq!(gemini.top_k, Some(40));
        assert_eq!(gemini.thinking, Some(true));
        assert_eq!(gemini.min_p, None);
        // Claude — only max_tokens + reasoning: temperature/top_k are zeroed,
        // thinking is kept.
        let claude = s.retain_supported(Some(CloudProvider::Claude), None);
        assert_eq!(claude.max_tokens, Some(256));
        assert_eq!(claude.thinking, Some(true));
        assert_eq!(claude.temperature, None);
        assert_eq!(claude.top_k, None);
        // Grok keeps temperature/max_tokens/thinking; top_k and min_p are zeroed —
        // xAI would drop them silently, and a knob that does nothing is worse than
        // one that isn't offered.
        let grok = s.retain_supported(Some(CloudProvider::Grok), None);
        assert_eq!(grok.temperature, Some(0.7));
        assert_eq!(grok.max_tokens, Some(256));
        assert_eq!(grok.thinking, Some(true));
        assert_eq!(grok.top_k, None);
        assert_eq!(grok.min_p, None);
    }

    /// The penalties are the one field group that turns a Grok request into a hard
    /// `400`, so they must not survive the filter.
    #[test]
    fn retain_supported_drops_penalties_for_grok() {
        let s = SamplingConfig {
            presence_penalty: Some(0.5),
            frequency_penalty: Some(0.5),
            temperature: Some(0.7),
            ..Default::default()
        };
        let grok = s.retain_supported(Some(CloudProvider::Grok), None);
        assert_eq!(grok.presence_penalty, None);
        assert_eq!(grok.frequency_penalty, None);
        assert_eq!(grok.temperature, Some(0.7));
    }

    #[test]
    fn resolve_follows_priority() {
        let global = SamplingConfig {
            temperature: Some(0.1),
            ..Default::default()
        };
        let profile = SamplingConfig {
            temperature: Some(0.5),
            ..Default::default()
        };
        let chat = SamplingConfig {
            temperature: Some(0.9),
            ..Default::default()
        };

        // Every combination of overrides (spec §8.3).
        assert_eq!(resolve(Some(&chat), Some(&profile), &global), chat);
        assert_eq!(resolve(None, Some(&profile), &global), profile);
        assert_eq!(resolve(None, None, &global), global);
        // Chat takes priority over the profile, the profile — over the global.
        assert_eq!(resolve(Some(&chat), None, &global), chat);
    }

    // ---------- the OpenRouter mode (docs/research/openrouter-mode.md) ----------

    const GATEWAY: Option<CloudProvider> = Some(CloudProvider::OpenRouter);

    /// What the gateway's own schema reads, of the fields this application
    /// names (docs/research/openrouter-mode.md §3.5, measured by sending each
    /// with a value of the wrong type) — `repeat_penalty` among them, since it
    /// travels as the `repetition_penalty` the schema has.
    const READ_BY_THE_GATEWAY: &str = "temperature top_k top_p min_p max_tokens seed \
        frequency_penalty presence_penalty repeat_penalty thinking reasoning_effort";

    fn words(list: &str) -> Vec<&str> {
        list.split_whitespace().collect()
    }

    fn published(list: &str) -> Vec<String> {
        list.split_whitespace().map(str::to_string).collect()
    }

    /// The mode's ceiling is a partition of everything that can be set: a field
    /// the gateway reads is offered, and a field that is llama.cpp's own — every
    /// one answered `200` to a wrong type, which is to say dropped — is not.
    /// Asserted over the whole settable list, so a field added later has to be
    /// put on one side here instead of being offered by accident.
    #[test]
    fn the_gateway_is_offered_what_its_schema_reads_and_nothing_else() {
        let read = words(READ_BY_THE_GATEWAY);
        let offered = supported_sampling_fields(GATEWAY);
        let misplaced: Vec<&str> = SETTABLE_SAMPLING_FIELDS
            .iter()
            .copied()
            .filter(|field| offered.contains(field) != read.contains(field))
            .collect();
        assert!(misplaced.is_empty(), "{misplaced:?} in {offered:?}");
        assert_eq!(
            offered.len(),
            read.len(),
            "nothing offered twice: {offered:?}"
        );
    }

    /// Behind the gateway there is a different model per slug, so its catalogue
    /// entry narrows the ceiling — the one cloud where that holds. Each row is
    /// what an entry publishes and what is then offered; the two alias rows are
    /// defect D1's other half: **their** word for our knob has to keep it, and
    /// ours, which no gateway publishes, keeps nothing. One literal, not tuple
    /// rows (lessons §2).
    const NARROWED_BY_AN_ENTRY: &str = "
        include_reasoning max_tokens reasoning temperature tools top_k top_p \
            => temperature top_k top_p max_tokens thinking reasoning_effort | Claude Haiku 4.5, as measured
        repetition_penalty => repeat_penalty           | their word for the penalty keeps ours
        repeat_penalty     =>                          | our word is not a claim of theirs
        reasoning          => thinking reasoning_effort | one entry, both switches
        include_reasoning  => thinking reasoning_effort | ...under either of its names
        tools response_format stop =>                  | an entry that lists nothing of ours
    ";

    #[test]
    fn a_models_catalogue_entry_narrows_what_the_gateway_is_offered() {
        let mut wrong = Vec::new();
        for row in NARROWED_BY_AN_ENTRY
            .lines()
            .filter(|l| !l.trim().is_empty())
        {
            let (columns, why) = row.split_once('|').unwrap();
            let (entry, kept) = columns.split_once("=>").unwrap();
            let got = available_sampling_fields(GATEWAY, Some(&published(entry)));
            if got != words(kept) {
                wrong.push(format!("{}: {got:?}", why.trim()));
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    /// Silence is never a claim: an entry with no list, and one whose list is
    /// empty, leave the mode's ceiling exactly as it is — an empty list read as
    /// "takes nothing" would empty the sampling section for such a model.
    #[test]
    fn a_gateway_entry_that_lists_nothing_narrows_nothing() {
        let ceiling = supported_sampling_fields(GATEWAY).to_vec();
        assert_eq!(available_sampling_fields(GATEWAY, None), ceiling);
        assert_eq!(available_sampling_fields(GATEWAY, Some(&[])), ceiling);
    }

    /// The exception is the gateway's alone. Every other cloud's table is a
    /// fact about its protocol (ADR 0004), so a list that would cut it to one
    /// field — or to none — cuts nothing, provider by provider.
    #[test]
    fn no_other_cloud_lets_a_catalogue_trim_its_table() {
        for provider in CloudProvider::ALL {
            if Some(provider) == GATEWAY {
                continue;
            }
            for entry in ["max_tokens", "tools"] {
                assert_eq!(
                    available_sampling_fields(Some(provider), Some(&published(entry))),
                    supported_sampling_fields(Some(provider)).to_vec(),
                    "{provider:?}, an entry listing {entry}"
                );
            }
        }
    }

    /// The snapshot stored on a message is "what was applied", so through the
    /// gateway it holds what the gateway reads and, once the model's entry has
    /// spoken, what that model takes — never a llama.cpp field the dialect left
    /// out of the request.
    #[test]
    fn what_was_applied_through_the_gateway_is_what_it_reads() {
        let set = SamplingConfig {
            temperature: Some(0.7),
            min_p: Some(0.05),
            repeat_penalty: Some(1.15),
            typical_p: Some(0.9),
            mirostat: Some(2),
            samplers: Some(vec!["top_k".into()]),
            thinking: Some(true),
            verbosity: Some(Verbosity::Low),
            ..Default::default()
        };
        let ceiling = SamplingConfig {
            temperature: Some(0.7),
            min_p: Some(0.05),
            repeat_penalty: Some(1.15),
            thinking: Some(true),
            ..Default::default()
        };
        assert_eq!(set.retain_supported(GATEWAY, None), ceiling);

        let entry = published("temperature reasoning tools");
        let narrowed = SamplingConfig {
            temperature: Some(0.7),
            thinking: Some(true),
            ..Default::default()
        };
        assert_eq!(set.retain_supported(GATEWAY, Some(&entry)), narrowed);
    }
}
