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
}
