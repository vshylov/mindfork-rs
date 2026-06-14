//! Параметры семплинга. Содержит **только** поля, поддержанные xinfer по HTTP
//! (см. docs/xinfer-contract.md §3.1, §7). `min_p`, `repetition_penalty`,
//! `seed`-на-запрос, DRY/mirostat/typical в xinfer отсутствуют и здесь не
//! представлены.

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
    pub frequency_penalty: Option<f32>,
    pub presence_penalty: Option<f32>,
    pub max_tokens: Option<usize>,
    /// Включить reasoning («мысли», `<think>`/`reasoning_content`).
    pub thinking: Option<bool>,
    pub reasoning_effort: Option<ReasoningEffort>,
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
