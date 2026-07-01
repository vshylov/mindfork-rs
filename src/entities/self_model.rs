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

/// Сколько недавних завершённых/неактуальных целей показывать в полном чтении
/// (`render_full`) — виден жизненный цикл, но список не растёт бесконечно.
const CLOSED_GOALS_SHOWN: usize = 5;

/// Результат разрешения ссылки на цель по «ручке» (короткий `#id` или полный UUID).
/// См. [`SelfModel::match_goal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalMatch {
    /// Однозначно найдена цель.
    One(Uuid),
    /// Совпадений нет.
    None,
    /// Префикс неоднозначен (совпало несколько целей).
    Ambiguous,
}

/// Короткий человекочитаемый id цели: первые 6 hex-символов UUID. Показывается в
/// чтениях и принимается в `complete_goals`/`abandon_goals` (полный UUID тоже). На
/// десятке целей коллизия практически невозможна, а `match_goal` всё равно ловит
/// неоднозначность.
fn short_hex(id: &Uuid) -> String {
    id.simple().to_string()[..6].to_string()
}

/// Разрешает «ручку» (полный UUID или короткий hex-префикс, с ведущим `#` или без;
/// регистронезависимо) среди набора id. Общая логика для целей и наблюдений.
fn resolve_handle(handle: &str, ids: &[Uuid]) -> GoalMatch {
    let h = handle.trim().trim_start_matches('#').to_lowercase();
    if h.is_empty() {
        return GoalMatch::None;
    }
    // Полный UUID (с дефисами или без).
    if let Ok(u) = Uuid::parse_str(&h) {
        return if ids.contains(&u) {
            GoalMatch::One(u)
        } else {
            GoalMatch::None
        };
    }
    // Иначе — префикс hex-представления id (первые символы `simple()`).
    let mut found: Option<Uuid> = None;
    for id in ids {
        if id.simple().to_string().starts_with(&h) {
            if found.is_some() {
                return GoalMatch::Ambiguous;
            }
            found = Some(*id);
        }
    }
    found.map_or(GoalMatch::None, GoalMatch::One)
}

/// Ручная правка «модели себя» из UI-редактора (`F3`). Применяется сущностью
/// ([`SelfModel::apply_edit`]); сохраняет оркестратор. Контракт UI↔оркестратор.
#[derive(Debug, Clone, PartialEq)]
pub enum SelfModelEdit {
    /// Заменить краткое описание себя.
    SetSummary(String),
    /// Добавить новую активную цель.
    AddGoal(String),
    /// Изменить текст цели по id.
    SetGoalText { id: Uuid, text: String },
    /// Переключить статус цели (Active→Completed→Abandoned→Active).
    CycleGoalStatus(Uuid),
    /// Удалить цель по id.
    DeleteGoal(Uuid),
    /// Заменить список воспринимаемых черт собеседника.
    SetTraits(Vec<String>),
    /// Заменить список текущих интересов собеседника.
    SetInterests(Vec<String>),
    /// Заменить описание динамики отношений.
    SetRelationship(String),
    /// Удалить инсайт нарратива по id.
    DeleteInsight(Uuid),
    /// Очистить всю модель (описание/цели/собеседник/нарратив).
    Clear,
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

    /// Циклически переключает статус цели Active→Completed→Abandoned→Active (по id).
    /// Возвращает `true`, если цель найдена.
    pub fn cycle_goal_status(&mut self, id: Uuid) -> bool {
        if let Some(g) = self.goals.iter_mut().find(|g| g.id == id) {
            g.status = match g.status {
                GoalStatus::Active => GoalStatus::Completed,
                GoalStatus::Completed => GoalStatus::Abandoned,
                GoalStatus::Abandoned => GoalStatus::Active,
            };
            true
        } else {
            false
        }
    }

    /// Применяет ручную правку из UI-редактора (`F3`). Возвращает `true`, если
    /// модель изменилась (оркестратору — стоит ли сохранять). Чистая логика —
    /// тестируема без оркестратора. Списки черт/интересов заменяются целиком.
    pub fn apply_edit(&mut self, edit: SelfModelEdit) -> bool {
        match edit {
            SelfModelEdit::SetSummary(s) => {
                let s = s.trim().to_string();
                if self.summary == s {
                    return false;
                }
                self.summary = s;
                true
            }
            SelfModelEdit::AddGoal(desc) => {
                let before = self.goals.len();
                self.add_goal(desc);
                self.goals.len() != before
            }
            SelfModelEdit::SetGoalText { id, text } => {
                let text = text.trim().to_string();
                if let Some(g) = self.goals.iter_mut().find(|g| g.id == id) {
                    if text.is_empty() || g.description == text {
                        return false;
                    }
                    g.description = text;
                    true
                } else {
                    false
                }
            }
            SelfModelEdit::CycleGoalStatus(id) => self.cycle_goal_status(id),
            SelfModelEdit::DeleteGoal(id) => {
                let before = self.goals.len();
                self.goals.retain(|g| g.id != id);
                self.goals.len() != before
            }
            SelfModelEdit::SetTraits(v) => {
                if self.user_model.perceived_traits == v {
                    return false;
                }
                self.user_model.perceived_traits = v;
                true
            }
            SelfModelEdit::SetInterests(v) => {
                if self.user_model.current_interests == v {
                    return false;
                }
                self.user_model.current_interests = v;
                true
            }
            SelfModelEdit::SetRelationship(s) => {
                let s = s.trim().to_string();
                if self.user_model.relationship_dynamic == s {
                    return false;
                }
                self.user_model.relationship_dynamic = s;
                true
            }
            SelfModelEdit::DeleteInsight(id) => {
                let before = self.narrative.len();
                self.narrative.retain(|n| n.id != id);
                self.narrative.len() != before
            }
            SelfModelEdit::Clear => {
                if self.is_empty() && self.summary.is_empty() && self.goals.is_empty() {
                    return false;
                }
                self.summary.clear();
                self.goals.clear();
                self.user_model = UserModel::default();
                self.narrative.clear();
                true
            }
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

    /// Убирает инсайты нарратива по id (консолидация: сырые/устаревшие/дубли
    /// вычищаются после того, как устойчивое свёрнуто в summary/черты/сводный
    /// инсайт). Возвращает число удалённых.
    pub fn remove_insights(&mut self, ids: &[Uuid]) -> usize {
        let before = self.narrative.len();
        self.narrative.retain(|n| !ids.contains(&n.id));
        before - self.narrative.len()
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

    /// Разрешает ссылку на цель по «ручке»: полный UUID или короткий hex-префикс
    /// (с ведущим `#` или без). Регистронезависимо. Ищет только среди целей модели.
    pub fn match_goal(&self, handle: &str) -> GoalMatch {
        let ids: Vec<Uuid> = self.goals.iter().map(|g| g.id).collect();
        resolve_handle(handle, &ids)
    }

    /// Разрешает ссылку на инсайт нарратива по «ручке» (полный UUID или короткий
    /// hex-префикс). Для консолидации нарратива (`consolidate_narrative`).
    pub fn match_insight(&self, handle: &str) -> GoalMatch {
        let ids: Vec<Uuid> = self.narrative.iter().map(|n| n.id).collect();
        resolve_handle(handle, &ids)
    }

    /// Полное человекочитаемое чтение модели — для инструментов (`get_self_model`,
    /// `reflect`, эхо после правок). В отличие от [`Self::render_for_prompt`]
    /// (компактная инъекция в системный промпт) **ничего не усекает**, показывает
    /// весь нарратив и цели с коротким id и статусом — так модель, читающая себя, не
    /// видит «…» и получает id, необходимые для complete/abandon. Пустую модель
    /// помечает явно. См. docs/self-model-mvp.md.
    pub fn render_full(&self) -> String {
        let mut out = String::from("[Твоя модель себя]\n");
        if !self.summary.trim().is_empty() {
            out.push_str("О себе: ");
            out.push_str(self.summary.trim());
            out.push('\n');
        }
        let active: Vec<&Goal> = self.active_goals().collect();
        let closed: Vec<&Goal> = self
            .goals
            .iter()
            .filter(|g| g.status != GoalStatus::Active)
            .collect();
        if !active.is_empty() || !closed.is_empty() {
            out.push_str("Цели (ссылайся по #id):\n");
            for g in &active {
                out.push_str(&format!(
                    "- #{} (активна) {}\n",
                    short_hex(&g.id),
                    g.description.trim()
                ));
            }
            // Недавние закрытые — компактно, новейшие первыми.
            for g in closed.iter().rev().take(CLOSED_GOALS_SHOWN) {
                let st = match g.status {
                    GoalStatus::Completed => "выполнена",
                    GoalStatus::Abandoned => "неактуальна",
                    GoalStatus::Active => "активна",
                };
                out.push_str(&format!(
                    "- #{} ({st}) {}\n",
                    short_hex(&g.id),
                    g.description.trim()
                ));
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
        // Нарратив целиком (новейшее первым) — без усечения, с #id для консолидации.
        if !self.narrative.is_empty() {
            out.push_str(&format!(
                "Наблюдения ({}, ссылайся по #id):\n",
                self.narrative.len()
            ));
            for seg in self.narrative.iter().rev() {
                out.push_str(&format!("- #{} {}\n", short_hex(&seg.id), seg.text.trim()));
            }
        }
        let body = out.trim_end();
        if body == "[Твоя модель себя]" {
            return "(модель себя пока пуста)".to_string();
        }
        body.to_string()
    }
}

impl UserModel {
    pub fn is_empty(&self) -> bool {
        self.perceived_traits.is_empty()
            && self.current_interests.is_empty()
            && self.relationship_dynamic.trim().is_empty()
    }

    /// Добавляет черты (дедуп без учёта регистра, пустые отбрасываются). Возвращает,
    /// изменился ли список. **Merge, а не замена** — правка не перетирает прежнее.
    pub fn add_traits(&mut self, items: Vec<String>) -> bool {
        merge_into(&mut self.perceived_traits, items)
    }
    /// Убирает черты по совпадению (без учёта регистра). Возвращает, изменилось ли.
    pub fn remove_traits(&mut self, items: &[String]) -> bool {
        remove_from(&mut self.perceived_traits, items)
    }
    /// Добавляет интересы (дедуп без учёта регистра). Возвращает, изменилось ли.
    pub fn add_interests(&mut self, items: Vec<String>) -> bool {
        merge_into(&mut self.current_interests, items)
    }
    /// Убирает интересы по совпадению (без учёта регистра). Возвращает, изменилось ли.
    pub fn remove_interests(&mut self, items: &[String]) -> bool {
        remove_from(&mut self.current_interests, items)
    }
}

/// Добавляет элементы в список с дедупом без учёта регистра (Unicode). Пустые после
/// trim отбрасываются. Возвращает `true`, если что-то добавилось.
fn merge_into(list: &mut Vec<String>, items: Vec<String>) -> bool {
    let mut changed = false;
    for it in items {
        let it = it.trim().to_string();
        if it.is_empty() {
            continue;
        }
        let lc = it.to_lowercase();
        if !list.iter().any(|x| x.to_lowercase() == lc) {
            list.push(it);
            changed = true;
        }
    }
    changed
}

/// Убирает из списка элементы, совпадающие (без учёта регистра) с любым из `items`.
/// Возвращает `true`, если список изменился.
fn remove_from(list: &mut Vec<String>, items: &[String]) -> bool {
    let targets: Vec<String> = items
        .iter()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if targets.is_empty() {
        return false;
    }
    let before = list.len();
    list.retain(|x| !targets.contains(&x.to_lowercase()));
    list.len() != before
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
    fn apply_edit_covers_operations() {
        let mut m = SelfModel::new(Uuid::new_v4());
        assert!(m.apply_edit(SelfModelEdit::SetSummary("я краток".into())));
        assert_eq!(m.summary, "я краток");
        // повтор того же — без изменений
        assert!(!m.apply_edit(SelfModelEdit::SetSummary("я краток".into())));

        assert!(m.apply_edit(SelfModelEdit::AddGoal("помочь".into())));
        let gid = m.goals[0].id;
        assert!(m.apply_edit(SelfModelEdit::SetGoalText {
            id: gid,
            text: "помочь лучше".into()
        }));
        assert_eq!(m.goals[0].description, "помочь лучше");
        // цикл статуса: Active → Completed
        assert!(m.apply_edit(SelfModelEdit::CycleGoalStatus(gid)));
        assert_eq!(m.goals[0].status, GoalStatus::Completed);
        assert!(m.apply_edit(SelfModelEdit::DeleteGoal(gid)));
        assert!(m.goals.is_empty());

        assert!(m.apply_edit(SelfModelEdit::SetTraits(vec!["скептик".into()])));
        assert!(m.apply_edit(SelfModelEdit::SetInterests(vec!["Rust".into()])));
        assert!(m.apply_edit(SelfModelEdit::SetRelationship("рабочие".into())));
        assert_eq!(m.user_model.perceived_traits, vec!["скептик".to_string()]);

        m.add_insight("наблюдение", 50);
        let iid = m.narrative[0].id;
        assert!(m.apply_edit(SelfModelEdit::DeleteInsight(iid)));
        assert!(m.narrative.is_empty());

        // Clear сбрасывает всё; повторный Clear на пустой — no-op.
        assert!(m.apply_edit(SelfModelEdit::Clear));
        assert!(m.is_empty());
        assert!(!m.apply_edit(SelfModelEdit::Clear));
        // несуществующие id — no-op
        assert!(!m.apply_edit(SelfModelEdit::DeleteGoal(Uuid::new_v4())));
    }

    #[test]
    fn render_truncates_to_cap() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "я".repeat(500);
        let r = m.render_for_prompt(50, p().narrative_in_prompt).unwrap();
        assert_eq!(r.chars().count(), 50);
        assert!(r.ends_with('…'));
    }

    #[test]
    fn render_full_shows_goal_ids_and_is_not_truncated() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "я".repeat(500);
        m.add_goal("активная цель");
        m.add_goal("завершённая цель");
        let done = m.goals[1].id;
        m.set_goal_status(done, GoalStatus::Completed);
        // Много инсайтов — полное чтение показывает все и без «…».
        for i in 0..12 {
            m.add_insight(format!("инсайт {i}"), 50);
        }

        let full = m.render_full();
        assert!(!full.ends_with('…'), "полное чтение не усекается");
        // Активная цель — с коротким id и статусом.
        let short = short_hex(&m.goals[0].id);
        assert!(full.contains(&format!("#{short} (активна) активная цель")));
        // Завершённая тоже видна (жизненный цикл).
        assert!(full.contains("(выполнена) завершённая цель"));
        // Весь нарратив (не только narrative_in_prompt=3), каждый с #id.
        assert_eq!(full.matches("инсайт ").count(), 12);
        assert!(full.contains("Наблюдения (12"));
        // Полное описание себя целиком (не обрезано до prompt_cap).
        assert!(full.contains(&"я".repeat(500)));
    }

    #[test]
    fn match_insight_and_remove_insights() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.add_insight("первое", 50);
        m.add_insight("второе", 50);
        m.add_insight("третье", 50);
        let ids: Vec<Uuid> = m.narrative.iter().map(|n| n.id).collect();

        // Резолвинг инсайта по короткому #id.
        let short = short_hex(&ids[1]);
        assert_eq!(
            m.match_insight(&format!("#{short}")),
            GoalMatch::One(ids[1])
        );
        assert_eq!(m.match_insight("zzzzzz"), GoalMatch::None);

        // Удаление двух инсайтов (консолидация): остаётся один.
        let removed = m.remove_insights(&[ids[0], ids[2]]);
        assert_eq!(removed, 2);
        assert_eq!(m.narrative.len(), 1);
        assert_eq!(m.narrative[0].text, "второе");
    }

    #[test]
    fn render_full_on_empty_marks_empty() {
        let m = SelfModel::new(Uuid::new_v4());
        assert_eq!(m.render_full(), "(модель себя пока пуста)");
    }

    #[test]
    fn match_goal_by_prefix_full_and_ambiguous() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.add_goal("первая");
        let id = m.goals[0].id;
        // Полный UUID.
        assert_eq!(m.match_goal(&id.to_string()), GoalMatch::One(id));
        // Короткий hex-префикс, с ведущим '#'.
        let short = short_hex(&id);
        assert_eq!(m.match_goal(&format!("#{short}")), GoalMatch::One(id));
        // Регистронезависимо.
        assert_eq!(m.match_goal(&short.to_uppercase()), GoalMatch::One(id));
        // Несуществующий.
        assert_eq!(m.match_goal("zzzzzz"), GoalMatch::None);
        assert_eq!(m.match_goal(""), GoalMatch::None);
        // Пустой префикс (после снятия '#') совпал бы со всеми → неоднозначно.
        m.add_goal("вторая");
        assert_eq!(m.match_goal("#"), GoalMatch::None); // пусто → None, не Ambiguous
    }

    #[test]
    fn user_model_merge_add_remove() {
        let mut u = UserModel::default();
        assert!(u.add_traits(vec!["добрый".into(), "Добрый".into(), "  ".into()]));
        // Дедуп без учёта регистра + отброс пустого.
        assert_eq!(u.perceived_traits, vec!["добрый".to_string()]);
        // Новая правка не перетирает — merge.
        assert!(u.add_traits(vec!["прямолинейный".into()]));
        assert_eq!(u.perceived_traits.len(), 2);
        // Повтор уже известного — без изменений.
        assert!(!u.add_traits(vec!["добрый".into()]));
        // Удаление по совпадению без учёта регистра.
        assert!(u.remove_traits(&["ДОБРЫЙ".into()]));
        assert_eq!(u.perceived_traits, vec!["прямолинейный".to_string()]);
        // Удаление отсутствующего — no-op.
        assert!(!u.remove_traits(&["нет такого".into()]));
    }
}
