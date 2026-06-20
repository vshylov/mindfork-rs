//! Параметры семплинга (см. spec §8). Поля сериализуются в тело запроса
//! `/v1/chat/completions`. Помимо стандартных OpenAI-полей (`temperature`,
//! `top_p`, `frequency_penalty`, …) здесь есть расширения llama.cpp `llama-server`
//! (`min_p`, `top_n_sigma`, DRY, XTC, `repeat_penalty`, `seed`, mirostat) — их
//! `llama-server` принимает прямо в теле запроса; сервер, не понимающий поле,
//! его просто игнорирует (а строгий сторонний OpenAI-сервер может отклонить —
//! поэтому расширения остаются `None`, пока пользователь их не задаст).

use serde::{Deserialize, Serialize};

/// Уровень reasoning-усилия (OpenAI-совместимый).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Low,
    Medium,
    High,
}

impl ReasoningEffort {
    /// Строковое представление для HTTP-поля `reasoning_effort`.
    pub fn as_wire(self) -> &'static str {
        match self {
            ReasoningEffort::None => "none",
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
        }
    }
}

/// Конфигурация семплинга для запроса генерации.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SamplingConfig {
    pub temperature: Option<f32>,
    pub top_k: Option<i64>,
    pub top_p: Option<f32>,
    /// min-p (llama.cpp): отсекает токены с вероятностью ниже доли от максимальной.
    pub min_p: Option<f32>,
    /// top-n-sigma (llama.cpp `top_n_sigma`): отсев по числу σ от макс. логита
    /// (`-1` = выключено).
    pub top_n_sigma: Option<f32>,
    /// locally typical sampling (llama.cpp `typical_p`, `1.0` = выключено).
    pub typical_p: Option<f32>,
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
    /// XTC: вероятность применения сэмплера (llama.cpp `xtc_probability`,
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
    /// Включить reasoning («мысли», `<think>`/`reasoning_content`).
    pub thinking: Option<bool>,
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Бюджет «мыслей» в токенах (llama.cpp `reasoning_budget`): `0` —
    /// **полностью выключить** thinking даже для моделей со «вшитым» в шаблон
    /// reasoning (Gemma `peg-gemma4`, Qwen), `-1` — без ограничения. `None` —
    /// поле не отправляется (поведение сервера по умолчанию). См. spec §8.
    pub reasoning_budget: Option<i64>,
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
