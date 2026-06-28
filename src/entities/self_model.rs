//! «Модель себя» агента — пер-профильное представление о себе, целях и
//! собеседнике. Живёт в SQLite (как заметки/RAG), изолирована по `profile_id`.
//! Минимальный MVP-зонд: свободный текст + цели + модель пользователя, без
//! числовых «сил убеждений». См. [docs/self-model-mvp.md](../../docs/self-model-mvp.md).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::shared::config::SelfModelSettings;

/// Параметры рендера/хранения «модели себя» (из `config.self_model`). Передаются в
/// методы сущности вместо захардкоженных констант, чтобы пользователь мог
/// регулировать размеры нарратива и объём инъекции в промпт. Аналог
/// [`ChunkParams`](crate::features::tools::rag::ChunkParams) для RAG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelfModelParams {
    /// Потолок хранения инсайтов (старые вытесняются).
    pub max_narrative: usize,
    /// Сколько свежих инсайтов идёт в системный промпт.
    pub narrative_in_prompt: usize,
    /// Потолок символов рендера модели в системный промпт.
    pub prompt_cap: usize,
}

impl Default for SelfModelParams {
    fn default() -> Self {
        Self::from_settings(&SelfModelSettings::default())
    }
}

impl SelfModelParams {
    /// Строит параметры из настроек, санитизируя значения (защита от нулей и
    /// несогласованности: хотя бы 1 инсайт хранится, в промпт не больше, чем хранится,
    /// читаемый минимум символов промпта).
    pub fn from_settings(s: &SelfModelSettings) -> Self {
        let max_narrative = s.max_narrative.max(1);
        Self {
            max_narrative,
            narrative_in_prompt: s.narrative_in_prompt.min(max_narrative),
            prompt_cap: s.prompt_cap.max(100),
        }
    }
}

/// Представление агента о себе (один экземпляр на профиль).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelfModel {
    pub profile_id: Uuid,
    /// Растёт при каждом сохранении — грубый индикатор «насколько менялся».
    #[serde(default)]
    pub version: u64,
    /// Свободный текст «о себе» (кто я, что ценю, как себя веду).
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub goals: Vec<Goal>,
    #[serde(default)]
    pub user_model: UserModel,
    /// Нарратив «я во времени»: короткие инсайты/наблюдения (включая замеченные
    /// противоречия — простой прозой, без отдельного типа). Append-only с потолком.
    #[serde(default)]
    pub narrative: Vec<NarrativeSegment>,
    pub updated_at: DateTime<Utc>,
}

/// Фрагмент нарратива: короткое наблюдение/инсайт агента с отметкой времени.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NarrativeSegment {
    pub id: Uuid,
    pub text: String,
    pub created_at: DateTime<Utc>,
}

/// Долгосрочная цель/намерение агента.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Goal {
    pub id: Uuid,
    pub description: String,
    pub status: GoalStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalStatus {
    Active,
    Completed,
    Abandoned,
}

/// Представление агента о собеседнике (свободные списки/текст, без id).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UserModel {
    #[serde(default)]
    pub perceived_traits: Vec<String>,
    #[serde(default)]
    pub current_interests: Vec<String>,
    #[serde(default)]
    pub relationship_dynamic: String,
}

impl SelfModel {
    /// Пустая модель для профиля.
    pub fn new(profile_id: Uuid) -> Self {
        Self {
            profile_id,
            version: 0,
            summary: String::new(),
            goals: Vec::new(),
            user_model: UserModel::default(),
            narrative: Vec::new(),
            updated_at: Utc::now(),
        }
    }

    /// Нечего показывать/инъектить: пустое описание, нет активных целей (рендерятся
    /// только они) и пустая модель собеседника. Завершённые/неактуальные цели сами
    /// по себе «пустой» модель не делают информативной.
    pub fn is_empty(&self) -> bool {
        self.summary.trim().is_empty()
            && self.active_goals().next().is_none()
            && self.user_model.is_empty()
            && self.narrative.is_empty()
    }

    /// Активные цели (для рендера/чтения).
    pub fn active_goals(&self) -> impl Iterator<Item = &Goal> {
        self.goals.iter().filter(|g| g.status == GoalStatus::Active)
    }

    /// Добавляет новую активную цель.
    pub fn add_goal(&mut self, description: impl Into<String>) {
        let description = description.into().trim().to_string();
        if description.is_empty() {
            return;
        }
        self.goals.push(Goal {
            id: Uuid::new_v4(),
            description,
            status: GoalStatus::Active,
            created_at: Utc::now(),
        });
    }

    /// Переводит цель в статус (по id). Возвращает `true`, если цель найдена.
    pub fn set_goal_status(&mut self, id: Uuid, status: GoalStatus) -> bool {
        if let Some(g) = self.goals.iter_mut().find(|g| g.id == id) {
            g.status = status;
            true
        } else {
            false
        }
    }

    /// Добавляет инсайт в нарратив (append-only, с обрезкой до `max_narrative`
    /// самых свежих). Пустой текст игнорируется.
    pub fn add_insight(&mut self, text: impl Into<String>, max_narrative: usize) {
        let text = text.into().trim().to_string();
        if text.is_empty() {
            return;
        }
        self.narrative.push(NarrativeSegment {
            id: Uuid::new_v4(),
            text,
            created_at: Utc::now(),
        });
        let max_narrative = max_narrative.max(1);
        if self.narrative.len() > max_narrative {
            let drop = self.narrative.len() - max_narrative;
            self.narrative.drain(0..drop);
        }
    }

    /// Компактный человекочитаемый блок для инъекции в системный промпт.
    /// `None`, если модель пуста. В нарратив идут `narrative_in_prompt` свежих
    /// инсайтов; результат усекается до `max_chars` символов.
    pub fn render_for_prompt(
        &self,
        max_chars: usize,
        narrative_in_prompt: usize,
    ) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut out = String::from("[Твоя модель себя]\n");
        if !self.summary.trim().is_empty() {
            out.push_str("О себе: ");
            out.push_str(self.summary.trim());
            out.push('\n');
        }
        let active: Vec<&Goal> = self.active_goals().collect();
        if !active.is_empty() {
            out.push_str("Активные цели:\n");
            for g in active {
                out.push_str("- ");
                out.push_str(g.description.trim());
                out.push('\n');
            }
        }
        let u = &self.user_model;
        if !u.is_empty() {
            out.push_str("О собеседнике:");
            if !u.perceived_traits.is_empty() {
                out.push_str(" черты: ");
                out.push_str(&u.perceived_traits.join(", "));
                out.push(';');
            }
            if !u.current_interests.is_empty() {
                out.push_str(" интересы: ");
                out.push_str(&u.current_interests.join(", "));
                out.push(';');
            }
            if !u.relationship_dynamic.trim().is_empty() {
                out.push_str(" отношения: ");
                out.push_str(u.relationship_dynamic.trim());
            }
            out.push('\n');
        }
        if !self.narrative.is_empty() && narrative_in_prompt > 0 {
            out.push_str("Недавние наблюдения:\n");
            for seg in self.narrative.iter().rev().take(narrative_in_prompt) {
                out.push_str("- ");
                out.push_str(seg.text.trim());
                out.push('\n');
            }
        }
        Some(truncate_chars(out.trim_end(), max_chars))
    }
}

impl UserModel {
    pub fn is_empty(&self) -> bool {
        self.perceived_traits.is_empty()
            && self.current_interests.is_empty()
            && self.relationship_dynamic.trim().is_empty()
    }
}

/// Усечение по символам (не байтам — кириллица) с многоточием.
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let take = max_chars.saturating_sub(1);
    let mut out: String = s.chars().take(take).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Дефолтные параметры рендера/хранения для тестов.
    fn p() -> SelfModelParams {
        SelfModelParams::default()
    }

    #[test]
    fn empty_model_renders_none() {
        let m = SelfModel::new(Uuid::new_v4());
        assert!(m.is_empty());
        assert!(
            m.render_for_prompt(p().prompt_cap, p().narrative_in_prompt)
                .is_none()
        );
    }

    #[test]
    fn add_and_complete_goals() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.add_goal("помочь с рефакторингом");
        m.add_goal("  "); // пустую игнорируем
        assert_eq!(m.goals.len(), 1);
        assert_eq!(m.active_goals().count(), 1);

        let id = m.goals[0].id;
        assert!(m.set_goal_status(id, GoalStatus::Completed));
        assert_eq!(m.active_goals().count(), 0);
        // несуществующая цель
        assert!(!m.set_goal_status(Uuid::new_v4(), GoalStatus::Abandoned));
    }

    #[test]
    fn render_includes_sections() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "ценю честность".into();
        m.add_goal("разобраться в коде");
        m.user_model.perceived_traits = vec!["любопытный".into()];
        m.user_model.current_interests = vec!["Rust".into()];
        m.user_model.relationship_dynamic = "доверительные".into();

        let r = m
            .render_for_prompt(p().prompt_cap, p().narrative_in_prompt)
            .unwrap();
        assert!(r.contains("О себе: ценю честность"));
        assert!(r.contains("разобраться в коде"));
        assert!(r.contains("черты: любопытный"));
        assert!(r.contains("интересы: Rust"));
        assert!(r.contains("отношения: доверительные"));
    }

    #[test]
    fn insights_append_cap_and_render() {
        let params = p();
        let mut m = SelfModel::new(Uuid::new_v4());
        // Только нарратив → модель уже информативна (рендерится).
        m.add_insight(
            "заметил напряжение между «кратко» и «полно»",
            params.max_narrative,
        );
        m.add_insight("   ", params.max_narrative); // пустой игнорируется
        assert!(!m.is_empty());
        assert_eq!(m.narrative.len(), 1);
        let r = m
            .render_for_prompt(params.prompt_cap, params.narrative_in_prompt)
            .unwrap();
        assert!(r.contains("Недавние наблюдения:"));
        assert!(r.contains("напряжение"));

        // Потолок: держим самые свежие max_narrative.
        for i in 0..params.max_narrative + 10 {
            m.add_insight(format!("инсайт {i}"), params.max_narrative);
        }
        assert_eq!(m.narrative.len(), params.max_narrative);
        // Самый старый из добавленных в цикле вытеснен, последний — присутствует.
        let last = format!("инсайт {}", params.max_narrative + 9);
        assert!(m.narrative.iter().any(|s| s.text == last));
        assert!(!m.narrative.iter().any(|s| s.text == "инсайт 0"));

        // В промпт идут только narrative_in_prompt свежих (новейший — первым).
        let r = m
            .render_for_prompt(params.prompt_cap, params.narrative_in_prompt)
            .unwrap();
        assert_eq!(r.matches("- инсайт ").count(), params.narrative_in_prompt);
        assert!(r.contains(&last));
    }

    #[test]
    fn custom_params_limit_storage_and_injection() {
        // Параметры из настроек: хранить 5, в промпт — 2.
        let params = SelfModelParams::from_settings(&SelfModelSettings {
            max_narrative: 5,
            narrative_in_prompt: 2,
            prompt_cap: 1000,
            ..SelfModelSettings::default()
        });
        let mut m = SelfModel::new(Uuid::new_v4());
        for i in 0..10 {
            m.add_insight(format!("инсайт {i}"), params.max_narrative);
        }
        assert_eq!(m.narrative.len(), 5);
        let r = m
            .render_for_prompt(params.prompt_cap, params.narrative_in_prompt)
            .unwrap();
        assert_eq!(r.matches("- инсайт ").count(), 2);
    }

    #[test]
    fn params_sanitize_inconsistent_settings() {
        // Нулевой max → минимум 1; in_prompt не больше max; крошечный cap → пол.
        let params = SelfModelParams::from_settings(&SelfModelSettings {
            max_narrative: 0,
            narrative_in_prompt: 99,
            prompt_cap: 1,
            ..SelfModelSettings::default()
        });
        assert_eq!(params.max_narrative, 1);
        assert_eq!(params.narrative_in_prompt, 1);
        assert_eq!(params.prompt_cap, 100);
    }

    #[test]
    fn completed_goals_not_rendered() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.add_goal("старая цель");
        let id = m.goals[0].id;
        m.set_goal_status(id, GoalStatus::Completed);
        // только завершённая цель → модель «пуста» для рендера
        assert!(m.is_empty());
        assert!(
            m.render_for_prompt(p().prompt_cap, p().narrative_in_prompt)
                .is_none()
        );
    }

    #[test]
    fn render_truncates_to_cap() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "я".repeat(500);
        let r = m.render_for_prompt(50, p().narrative_in_prompt).unwrap();
        assert_eq!(r.chars().count(), 50);
        assert!(r.ends_with('…'));
    }
}
