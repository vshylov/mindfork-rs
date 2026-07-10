//! Параметры семплинга (см. spec §8). Поля сериализуются в тело запроса
//! `/v1/chat/completions`. Помимо стандартных OpenAI-полей (`temperature`,
//! `top_p`, `frequency_penalty`, …) здесь есть расширения llama.cpp `llama-server`
//! (`min_p`, `top_n_sigma`, DRY, XTC, `repeat_penalty`, `seed`, mirostat) — их
//! `llama-server` принимает прямо в теле запроса; сервер, не понимающий поле,
//! его просто игнорирует (а строгий сторонний OpenAI-сервер может отклонить —
//! поэтому расширения остаются `None`, пока пользователь их не задаст).

use serde::{Deserialize, Serialize};

use crate::shared::config::CloudProvider;

/// Уровень reasoning-усилия. `Minimal`/`XHigh` — расширенные ступени OpenAI (gpt-5.x
/// Responses API); локальные модели/Anthropic понимают `low`/`medium`/`high` (крайние
/// ступени маппятся к ним при трансляции). Порядок вариантов = порядок цикла в UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

impl ReasoningEffort {
    /// Строковое представление для HTTP-поля `reasoning_effort` / `reasoning.effort`.
    pub fn as_wire(self) -> &'static str {
        match self {
            ReasoningEffort::None => "none",
            ReasoningEffort::Minimal => "minimal",
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
            ReasoningEffort::XHigh => "xhigh",
        }
    }
}

/// Многословность ответа (OpenAI Responses `text.verbosity`): регулирует длину
/// ответа отдельно от температуры. Только OpenAI-Responses; прочие бэкенды игнорируют.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verbosity {
    Low,
    Medium,
    High,
}

impl Verbosity {
    /// Строковое представление для HTTP-поля `text.verbosity`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Verbosity::Low => "low",
            Verbosity::Medium => "medium",
            Verbosity::High => "high",
        }
    }
}

/// Конфигурация семплинга для запроса генерации.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SamplingConfig {
    pub temperature: Option<f32>,
    /// Динамическая температура (llama.cpp `dynatemp_range`): ширина диапазона
    /// ± вокруг `temperature`, подстраиваемого по энтропии распределения на
    /// каждом токене (уверенные позиции — холоднее, неоднозначные — горячее).
    /// `0.0` = выключено (обычная статическая температура).
    pub dynatemp_range: Option<f32>,
    /// Показатель кривой динамической температуры (llama.cpp
    /// `dynatemp_exponent`, по умолчанию сервера `1.0`).
    pub dynatemp_exponent: Option<f32>,
    pub top_k: Option<i64>,
    pub top_p: Option<f32>,
    /// min-p (llama.cpp): отсекает токены с вероятностью ниже доли от максимальной.
    pub min_p: Option<f32>,
    /// top-n-sigma (llama.cpp `top_n_sigma`): отсев по числу σ от макс. логита
    /// (`-1` = выключено).
    pub top_n_sigma: Option<f32>,
    /// locally typical sampling (llama.cpp `typical_p`, `1.0` = выключено).
    pub typical_p: Option<f32>,
    /// adaptive-p (llama.cpp `adaptive_target`, PR #17927): целевая энтропия,
    /// около которой выбираются токены; отрицательное значение = выключено
    /// (валидный диапазон `≤ 1.0`). Сверено по `server-schema.cpp` (плоский
    /// ключ тела запроса). Семплер новый — поведение проверять на живой модели.
    pub adaptive_target: Option<f32>,
    /// adaptive-p (llama.cpp `adaptive_decay`): EMA-затухание адаптации цели
    /// (hard-диапазон `0.0`..`0.99`; меньше — реактивнее, больше — стабильнее).
    pub adaptive_decay: Option<f32>,
    pub frequency_penalty: Option<f32>,
    pub presence_penalty: Option<f32>,
    /// Штраф за повтор последовательности токенов (llama.cpp `repeat_penalty`,
    /// `1.0` = выключено). Отдельно от OpenAI-штрафов presence/frequency.
    pub repeat_penalty: Option<f32>,
    /// Сколько последних токенов учитывать для `repeat_penalty` (llama.cpp
    /// `repeat_last_n`; `0` = выключено, `-1` = весь контекст).
    pub repeat_last_n: Option<i64>,
    /// DRY: множитель штрафа (llama.cpp `dry_multiplier`, `0.0` = выключено).
    pub dry_multiplier: Option<f32>,
    /// DRY: основание экспоненты (llama.cpp `dry_base`).
    pub dry_base: Option<f32>,
    /// DRY: длина допустимого повтора до штрафа (llama.cpp `dry_allowed_length`).
    pub dry_allowed_length: Option<i64>,
    /// DRY: сколько последних токенов сканировать (llama.cpp `dry_penalty_last_n`;
    /// `0` = выключено, `-1` = весь контекст).
    pub dry_penalty_last_n: Option<i64>,
    /// DRY: «брейкеры» — строки, сбрасывающие учёт повтора (llama.cpp
    /// `dry_sequence_breakers`). `None`/пусто — серверные по умолчанию
    /// (`\n`, `:`, `"`, `*`). Отправляется только когда непуст.
    pub dry_sequence_breakers: Option<Vec<String>>,
    /// XTC: вероятность применения семплера (llama.cpp `xtc_probability`,
    /// `0.0` = выключено).
    pub xtc_probability: Option<f32>,
    /// XTC: порог вероятности (llama.cpp `xtc_threshold`).
    pub xtc_threshold: Option<f32>,
    /// Mirostat: режим (llama.cpp `mirostat`; `0` = выключено, `1`/`2` = версии).
    pub mirostat: Option<i64>,
    /// Mirostat: целевая энтропия τ (llama.cpp `mirostat_tau`).
    pub mirostat_tau: Option<f32>,
    /// Mirostat: скорость обучения η (llama.cpp `mirostat_eta`).
    pub mirostat_eta: Option<f32>,
    pub max_tokens: Option<usize>,
    /// RNG-seed на запрос (llama.cpp `seed`; `-1` = случайный). `None` — поле не
    /// отправляется (сервер выбирает сам).
    pub seed: Option<i64>,
    /// Порядок применения семплеров (llama.cpp `samplers`): имена семплеров в
    /// нужном порядке (напр. `["penalties","dry","top_k","top_p","min_p",
    /// "temperature"]`). `None` — серверный порядок по умолчанию. **Важно:**
    /// семплер, не указанный в непустом списке, отключается — список должен быть
    /// полным. Отправляется только когда непуст.
    pub samplers: Option<Vec<String>>,
    /// Включить reasoning («мысли», `<think>`/`reasoning_content`).
    pub thinking: Option<bool>,
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Бюджет «мыслей» в токенах (llama.cpp `reasoning_budget`): `0` —
    /// **полностью выключить** thinking даже для моделей со «вшитым» в шаблон
    /// reasoning (Gemma `peg-gemma4`, Qwen), `-1` — без ограничения. `None` —
    /// поле не отправляется (поведение сервера по умолчанию). См. spec §8.
    pub reasoning_budget: Option<i64>,
    /// Многословность ответа (OpenAI Responses `text.verbosity`). `None` — поле не
    /// отправляется (дефолт провайдера). Прочие бэкенды игнорируют.
    pub verbosity: Option<Verbosity>,
}

impl SamplingConfig {
    /// Копия, где обнулены (`None`) все поля, **недоступные** в режиме движка
    /// провайдера `provider` (см. [`supported_sampling_fields`]). Используется для
    /// снимка в `Message.metadata`: движок недоступное поле всё равно не принял бы,
    /// поэтому в «что применилось» оно не должно попадать. Список-поля
    /// (`samplers`/`dry_sequence_breakers`) обрабатываются как обычные ключи.
    pub fn retain_supported(&self, provider: Option<CloudProvider>) -> SamplingConfig {
        let supported = supported_sampling_fields(provider);
        // Сериализация нашего типа не падает; при неожиданности возвращаем как есть.
        let Ok(serde_json::Value::Object(mut map)) = serde_json::to_value(self) else {
            return self.clone();
        };
        map.retain(|k, _| supported.contains(&k.as_str()));
        serde_json::from_value(serde_json::Value::Object(map)).unwrap_or_else(|_| self.clone())
    }
}

/// Разрешает фактический семплинг по приоритету (spec §8.3):
/// `Chat.sampling_override` → `Profile.default_sampling` → глобальный.
///
/// Разрешение — **целиком по конфигу** (а не пофайлово): берётся первый
/// заданный уровень. Снимок результата сохраняется в `Message.metadata`.
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

/// Имена JSON-полей сэмплинга, которыми может управлять модель через инструмент
/// `set_sampling` (порядок = порядок в JSON-схеме инструмента). Внутреннее
/// `reasoning_budget` сюда **не** входит — оно не правится моделью.
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

/// Имена полей сэмплинга, которые движок данного режима реально принимает —
/// зеркало wire-диалекта (`shared/api/openai/wire::restrict_to_strict` и
/// `anthropic/wire`). `None` провайдер = локальный/external `llama.cpp`: принимает
/// все поля (расширения он игнорирует, а не отвергает). Облако строгое:
///
/// - **OpenAI** — `max_tokens` + reasoning (`thinking`/`reasoning_effort`) + `verbosity`.
///   Режим ходит в **Responses API** (`ResponsesClient`), где нет
///   `temperature`/`top_p`/`seed`/penalties (reasoning-модели их отвергают), но есть
///   резюме рассуждений и `text.verbosity`. См. ADR 0004, docs/research/openai-responses-client.md;
/// - **Gemini** (нативный `generateContent`, [`GeminiClient`](crate::shared::api::gemini::GeminiClient))
///   — `temperature`/`top_p`/`top_k`/`max_tokens`/`seed`/`frequency_penalty`/
///   `presence_penalty` **+ reasoning** (`thinking`/`reasoning_effort`): нативный API
///   принимает `top_k` (в отличие от прежнего compat) и даёт резюме «мыслей» +
///   `thinkingLevel`/`thinkingBudget`. Нет `verbosity` (это OpenAI-Responses-специфика);
/// - **Claude** — `max_tokens` + reasoning (`thinking`/`reasoning_effort`):
///   модели 4.x «зафиксировали» сэмплинг (отвергают `temperature`/`top_p`/`top_k`),
///   но поддерживают extended thinking (`{type:"adaptive"}` + `output_config.effort`).
///   `reasoning_budget` сюда не входит — `budget_tokens` модели 4.x отвергают.
///
/// Это единый источник истины для UI настроек (`cloud_supported_param`) и
/// инструментов `get_sampling`/`set_sampling` (показывать/менять только доступное).
/// См. ADR 0004.
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Локально (None) — весь настраиваемый набор.
        assert_eq!(supported_sampling_fields(None), SETTABLE_SAMPLING_FIELDS);
        // OpenAI — Responses API: max_tokens + reasoning + verbosity; нет
        // temperature/top_p/seed/penalties (reasoning-модели их отвергают).
        let openai = supported_sampling_fields(Some(CloudProvider::OpenAi));
        assert!(openai.contains(&"max_tokens"));
        assert!(openai.contains(&"thinking"));
        assert!(openai.contains(&"reasoning_effort"));
        assert!(openai.contains(&"verbosity"));
        assert!(!openai.contains(&"seed"));
        assert!(!openai.contains(&"temperature"));
        assert!(!openai.contains(&"top_p"));
        assert!(!openai.contains(&"top_k"));
        // Gemini (нативный generateContent): temperature/top_p/top_k/penalties/seed/
        // max_tokens + reasoning (thinking/reasoning_effort), но без verbosity.
        let gemini = supported_sampling_fields(Some(CloudProvider::Gemini));
        assert!(gemini.contains(&"temperature"));
        assert!(gemini.contains(&"top_p"));
        assert!(gemini.contains(&"top_k"));
        assert!(gemini.contains(&"seed"));
        assert!(gemini.contains(&"thinking"));
        assert!(gemini.contains(&"reasoning_effort"));
        assert!(!gemini.contains(&"verbosity"));
        // Claude — max_tokens + reasoning (thinking/reasoning_effort), но не top_k.
        let claude = supported_sampling_fields(Some(CloudProvider::Claude));
        assert!(claude.contains(&"max_tokens"));
        assert!(claude.contains(&"thinking"));
        assert!(claude.contains(&"reasoning_effort"));
        assert!(!claude.contains(&"top_k"));
        // Подмножества облака — действительно подмножества полного набора.
        for f in openai.iter().chain(gemini).chain(claude) {
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
        // Локально (None) — llama.cpp принимает всё настраиваемое: поля сохраняются.
        let local = s.retain_supported(None);
        assert_eq!(local.temperature, Some(0.7));
        assert_eq!(local.top_k, Some(40));
        assert_eq!(local.min_p, Some(0.05));
        assert_eq!(local.thinking, Some(true));
        // OpenAI (Responses): max_tokens + thinking остаются; temperature/top_k/min_p
        // обнуляются (их нет в Responses API).
        let openai = s.retain_supported(Some(CloudProvider::OpenAi));
        assert_eq!(openai.max_tokens, Some(256));
        assert_eq!(openai.thinking, Some(true));
        assert_eq!(openai.temperature, None);
        assert_eq!(openai.top_k, None);
        assert_eq!(openai.min_p, None);
        // Gemini (нативный) принимает temperature/top_k/thinking; min_p (расширение
        // llama.cpp) обнуляется.
        let gemini = s.retain_supported(Some(CloudProvider::Gemini));
        assert_eq!(gemini.temperature, Some(0.7));
        assert_eq!(gemini.max_tokens, Some(256));
        assert_eq!(gemini.top_k, Some(40));
        assert_eq!(gemini.thinking, Some(true));
        assert_eq!(gemini.min_p, None);
        // Claude — только max_tokens + reasoning: temperature/top_k обнуляются,
        // thinking сохраняется.
        let claude = s.retain_supported(Some(CloudProvider::Claude));
        assert_eq!(claude.max_tokens, Some(256));
        assert_eq!(claude.thinking, Some(true));
        assert_eq!(claude.temperature, None);
        assert_eq!(claude.top_k, None);
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

        // Все комбинации переопределений (spec §8.3).
        assert_eq!(resolve(Some(&chat), Some(&profile), &global), chat);
        assert_eq!(resolve(None, Some(&profile), &global), profile);
        assert_eq!(resolve(None, None, &global), global);
        // Chat имеет приоритет над профилем, профиль — над глобальным.
        assert_eq!(resolve(Some(&chat), None, &global), chat);
    }
}
