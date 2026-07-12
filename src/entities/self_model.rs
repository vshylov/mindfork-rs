//! «Модель себя» агента — пер-профильное представление о себе, целях и
//! собеседнике. Живёт в SQLite (как заметки/RAG), изолирована по `profile_id`.
//! Минимальный MVP-зонд: свободный текст + цели + модель пользователя, без
//! числовых «сил убеждений». См. [docs/history/self-model-mvp.md](../../docs/history/self-model-mvp.md).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::shared::config::SelfModelSettings;
use crate::shared::i18n::Locale;

/// Параметры рендера/хранения «модели себя» (из `config.self_model`). Передаются в
/// методы сущности вместо захардкоженных констант, чтобы пользователь мог
/// регулировать размеры нарратива и объём инъекции в промпт. Аналог
/// [`ChunkParams`](crate::features::tools::rag::ChunkParams) для RAG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelfModelParams {
    /// Сколько недавних наблюдений (self-заметок) поднимать при полном чтении модели
    /// (`get_self_model`/`reflect`). Наблюдения переехали в заметки — FIFO-потолка
    /// хранения больше нет; параметр лишь ограничивает объём чтения. Историческое имя
    /// поля сохранено ради совместимости `settings.json`.
    pub max_narrative: usize,
    /// Сколько свежих наблюдений идёт в системный промпт (инъекция).
    pub narrative_in_prompt: usize,
    /// Потолок символов рендера модели в системный промпт.
    pub prompt_cap: usize,
    /// Сколько закрытых целей держать в структуре (старейшие сверх — в нарратив-шрам).
    pub max_closed_goals: usize,
    /// Ориентир размера описания себя (summary): сверх него [`SelfModel::summary_fill_hint`]
    /// возвращает мягкую подсказку сократить. Ворота, не потолок.
    pub summary_target_chars: usize,
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
            max_closed_goals: s.max_closed_goals.max(1),
            summary_target_chars: s.summary_target_chars.max(200),
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
    /// Когда цель ушла из `Active` (для возраста закрытой цели и свёртки старых
    /// закрытых). `None` у активных и у старых записей (без миграции —
    /// `#[serde(default)]`). Возраст закрытой считается от неё, иначе — от `created_at`.
    #[serde(default)]
    pub closed_at: Option<DateTime<Utc>>,
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

/// Публичный короткий id для эха инструментов (первые 6 hex UUID, тот же формат, что
/// `#id` в чтениях `render_full`). Обёртка над [`short_hex`] — чтобы дельта-эхо правок
/// (этап 4, docs/summary-as-snapshot.md) называло цели теми же ручками, по которым их
/// закрывают.
pub fn short_id(id: &Uuid) -> String {
    short_hex(id)
}

/// Проставляет/снимает `closed_at` цели по её текущему статусу: при уходе из
/// `Active` — ставит момент (если ещё не стоял), при возврате в `Active` — снимает.
fn stamp_closed(g: &mut Goal) {
    match g.status {
        GoalStatus::Active => g.closed_at = None,
        _ => {
            if g.closed_at.is_none() {
                g.closed_at = Some(Utc::now());
            }
        }
    }
}

/// Грубая человекочитаемая метка возраста записи (сутки-гранулярность). Внутри
/// одного дня текст стабилен — значит инъекция «модели себя» в системный промпт не
/// меняется от хода к ходу (prefix cache локальной модели страдает не чаще раза в
/// день, сверх реальных правок модели). Отрицательная разница (часы вперёд из-за
/// рассинхронизации) трактуется как «сегодня».
fn age_label(at: DateTime<Utc>, now: DateTime<Utc>, loc: &Locale) -> String {
    let days = (now - at).num_days();
    match days {
        d if d <= 0 => loc.t("selfmodel.age.today").to_string(),
        1 => loc.t("selfmodel.age.yesterday").to_string(),
        2..=6 => loc.tf("selfmodel.age.days", &[("n", &days.to_string())]),
        7..=30 => loc.tf("selfmodel.age.weeks", &[("n", &(days / 7).to_string())]),
        31..=364 => loc.tf("selfmodel.age.months", &[("n", &(days / 30).to_string())]),
        _ => loc.tf("selfmodel.age.years", &[("n", &(days / 365).to_string())]),
    }
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

    /// Пуста ли **структурная** часть модели: пустое описание, нет активных целей
    /// (рендерятся только они) и пустая модель собеседника. Нарратив здесь **не
    /// учитывается** — он переехал в заметки (`@self`, см. docs/history/narrative-as-notes.md)
    /// и передаётся в рендер параметром `recent`; наличие наблюдений проверяет
    /// вызывающий (`is_empty() && recent.is_empty()`). Завершённые цели сами по себе
    /// «пустой» модель не делают информативной.
    pub fn is_empty(&self) -> bool {
        self.summary.trim().is_empty()
            && self.active_goals().next().is_none()
            && self.user_model.is_empty()
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
            closed_at: None,
        });
    }

    /// Переводит цель в статус (по id). Возвращает `true`, если цель найдена.
    /// Проставляет/снимает `closed_at` при уходе из `Active`/реактивации.
    pub fn set_goal_status(&mut self, id: Uuid, status: GoalStatus) -> bool {
        if let Some(g) = self.goals.iter_mut().find(|g| g.id == id) {
            g.status = status;
            stamp_closed(g);
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
            stamp_closed(g);
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

    /// Сворачивает старые закрытые цели в «шрам»-наблюдения и убирает их из `goals`,
    /// оставляя не более `keep` самых свежих закрытых (по `closed_at`/`created_at`).
    /// **Возвращает** тексты шрамов («[архив цели] …») — вызывающий записывает их как
    /// self-заметки (наблюдения переехали в заметки, см. docs/history/narrative-as-notes.md).
    /// Активные цели не трогает. Это интеграция, а не потеря: закрытая цель уходит
    /// наблюдением-шрамом, а не молча удаляется.
    pub fn fold_closed_goals(&mut self, keep: usize, loc: &Locale) -> Vec<String> {
        let freshness = |g: &Goal| g.closed_at.unwrap_or(g.created_at);
        let mut closed: Vec<(Uuid, DateTime<Utc>)> = self
            .goals
            .iter()
            .filter(|g| g.status != GoalStatus::Active)
            .map(|g| (g.id, freshness(g)))
            .collect();
        if closed.len() <= keep {
            return Vec::new();
        }
        closed.sort_by_key(|(_, at)| std::cmp::Reverse(*at)); // новые первыми
        let fold_ids: std::collections::HashSet<Uuid> =
            closed[keep..].iter().map(|(id, _)| *id).collect();

        let scars: Vec<String> = self
            .goals
            .iter()
            .filter(|g| fold_ids.contains(&g.id))
            .map(|g| {
                let verb = loc.t(match g.status {
                    GoalStatus::Completed => "selfmodel.status.completed",
                    GoalStatus::Abandoned => "selfmodel.status.abandoned",
                    GoalStatus::Active => "selfmodel.status.active",
                });
                loc.tf(
                    "selfmodel.goal_archive",
                    &[("verb", verb), ("text", g.description.trim())],
                )
            })
            .collect();
        self.goals.retain(|g| !fold_ids.contains(&g.id));
        scars
    }

    /// Мягкая подсказка о разросшемся описании себя: `None`, пока `summary` в
    /// пределах ориентира `target`; иначе текст с текущим размером и ориентиром,
    /// направляющий вынести событийное в наблюдения. Прямой аналог бывшего
    /// `narrative_fill_hint`, но для `summary` — единственного органа, у которого не
    /// было обратной связи о размере. Ворота, а не потолок: ничего не усекает и не
    /// блокирует. См. docs/summary-as-snapshot.md (этап 2).
    pub fn summary_fill_hint(&self, target: usize, loc: &Locale) -> Option<String> {
        let n = self.summary.chars().count();
        (n > target).then(|| {
            loc.tf(
                "selfmodel.summary_hint",
                &[("n", &n.to_string()), ("target", &target.to_string())],
            )
        })
    }

    /// Компактный человекочитаемый блок для инъекции в системный промпт.
    /// `None`, если структурная часть пуста **и** нет наблюдений. Наблюдения
    /// (`recent` — self-заметки, новейшие первыми, готовит вызывающий) идут в блок в
    /// количестве `narrative_in_prompt` самых свежих; результат усекается до
    /// `max_chars` символов. `now` — точка отсчёта для меток возраста (сутки-
    /// гранулярность, стабильно в пределах дня — см. [`age_label`]).
    pub fn render_for_prompt(
        &self,
        max_chars: usize,
        narrative_in_prompt: usize,
        now: DateTime<Utc>,
        recent: &[NarrativeSegment],
        loc: &Locale,
    ) -> Option<String> {
        if self.is_empty() && recent.is_empty() {
            return None;
        }
        let mut out = format!("{}\n", loc.t("selfmodel.render.header"));
        if !self.summary.trim().is_empty() {
            out.push_str(loc.t("selfmodel.render.about"));
            // Посекционный бюджет (этап 3): описание — не более половины лимита, чтобы
            // раздутый summary не вытеснял из инъекции цели/собеседника/наблюдения.
            // Финальное усечение всего блока ниже остаётся страховкой.
            // См. docs/summary-as-snapshot.md.
            out.push_str(&truncate_chars_word(self.summary.trim(), max_chars / 2));
            out.push('\n');
        }
        let active: Vec<&Goal> = self.active_goals().collect();
        if !active.is_empty() {
            out.push_str(loc.t("selfmodel.render.goals_active"));
            out.push('\n');
            for g in active {
                out.push_str(&loc.tf(
                    "selfmodel.item.goal_prompt",
                    &[
                        ("desc", g.description.trim()),
                        ("age", &age_label(g.created_at, now, loc)),
                    ],
                ));
                out.push('\n');
            }
        }
        render_user_model(&mut out, &self.user_model, loc);
        if !recent.is_empty() && narrative_in_prompt > 0 {
            out.push_str(loc.t("selfmodel.render.observations_recent"));
            out.push('\n');
            for seg in recent.iter().take(narrative_in_prompt) {
                out.push_str(&loc.tf(
                    "selfmodel.item.obs_prompt",
                    &[
                        ("age", &age_label(seg.created_at, now, loc)),
                        ("text", seg.text.trim()),
                    ],
                ));
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

    /// Полное человекочитаемое чтение модели — для инструментов (`get_self_model`,
    /// `reflect`, эхо после правок). В отличие от [`Self::render_for_prompt`]
    /// (компактная инъекция) **ничего не усекает**, показывает все наблюдения
    /// (`recent` — self-заметки, новейшие первыми, готовит вызывающий) и цели со
    /// статусом. Цели — с коротким `#id` (их ведёт `update_self_model` резолвером);
    /// наблюдения — с **полным** id (они заметки, их переписывает/замещает
    /// note_revise/note_supersede по полному id). Пустую модель помечает явно.
    /// См. docs/history/self-model-mvp.md, docs/history/narrative-as-notes.md.
    pub fn render_full(
        &self,
        now: DateTime<Utc>,
        recent: &[NarrativeSegment],
        loc: &Locale,
    ) -> String {
        let header = loc.t("selfmodel.render.header");
        let mut out = format!("{header}\n");
        if !self.summary.trim().is_empty() {
            out.push_str(loc.t("selfmodel.render.about"));
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
            out.push_str(loc.t("selfmodel.render.goals_ref"));
            out.push('\n');
            let active_st = loc.t("selfmodel.status.active");
            for g in &active {
                out.push_str(&loc.tf(
                    "selfmodel.item.goal_full",
                    &[
                        ("id", &short_hex(&g.id)),
                        ("status", active_st),
                        ("age", &age_label(g.created_at, now, loc)),
                        ("text", g.description.trim()),
                    ],
                ));
                out.push('\n');
            }
            // Недавние закрытые — компактно, новейшие первыми. Возраст — от момента
            // закрытия (`closed_at`), иначе от создания.
            for g in closed.iter().rev().take(CLOSED_GOALS_SHOWN) {
                let st = loc.t(match g.status {
                    GoalStatus::Completed => "selfmodel.status.completed",
                    GoalStatus::Abandoned => "selfmodel.status.stale",
                    GoalStatus::Active => "selfmodel.status.active",
                });
                out.push_str(&loc.tf(
                    "selfmodel.item.goal_full",
                    &[
                        ("id", &short_hex(&g.id)),
                        ("status", st),
                        (
                            "age",
                            &age_label(g.closed_at.unwrap_or(g.created_at), now, loc),
                        ),
                        ("text", g.description.trim()),
                    ],
                ));
                out.push('\n');
            }
        }
        render_user_model(&mut out, &self.user_model, loc);
        // Наблюдения (self-заметки) целиком, новейшее первым — без усечения. Полный
        // id: наблюдение переписывается/замещается note-инструментами по полному id.
        if !recent.is_empty() {
            out.push_str(&loc.tf(
                "selfmodel.render.observations",
                &[("n", &recent.len().to_string())],
            ));
            out.push('\n');
            for seg in recent {
                out.push_str(&loc.tf(
                    "selfmodel.item.obs_full",
                    &[
                        ("id", &seg.id.to_string()),
                        ("age", &age_label(seg.created_at, now, loc)),
                        ("text", seg.text.trim()),
                    ],
                ));
                out.push('\n');
            }
        }
        let body = out.trim_end();
        if body == header {
            return loc.t("selfmodel.render.empty").to_string();
        }
        body.to_string()
    }
}

/// Блок «О собеседнике» — общий для [`SelfModel::render_for_prompt`] и
/// [`SelfModel::render_full`] (в обоих байт-в-байт). Разделители `, ` и `;` —
/// пунктуация, языко-нейтральны и остаются в коде; локализуются лишь метки-подписи.
fn render_user_model(out: &mut String, u: &UserModel, loc: &Locale) {
    if u.is_empty() {
        return;
    }
    out.push_str(loc.t("selfmodel.render.user"));
    if !u.perceived_traits.is_empty() {
        out.push_str(loc.t("selfmodel.render.user.traits"));
        out.push_str(&u.perceived_traits.join(", "));
        out.push(';');
    }
    if !u.current_interests.is_empty() {
        out.push_str(loc.t("selfmodel.render.user.interests"));
        out.push_str(&u.current_interests.join(", "));
        out.push(';');
    }
    if !u.relationship_dynamic.trim().is_empty() {
        out.push_str(loc.t("selfmodel.render.user.relationship"));
        out.push_str(u.relationship_dynamic.trim());
    }
    out.push('\n');
}

impl UserModel {
    pub fn is_empty(&self) -> bool {
        self.perceived_traits.is_empty()
            && self.current_interests.is_empty()
            && self.relationship_dynamic.trim().is_empty()
    }

    /// Компактная подсказка для имперсонации (`Ctrl+U`): агент пишет реплику **за**
    /// человека, а `UserModel` — модель этого человека, поэтому подмешивание её в
    /// системный промпт имперсонации делает голос точнее. `None`, если модель пуста;
    /// результат усечён до `max_chars` символов.
    pub fn render_for_impersonation(&self, max_chars: usize) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut out = String::from("Известное о человеке, за которого ты пишешь:");
        if !self.perceived_traits.is_empty() {
            out.push_str(" черты — ");
            out.push_str(&self.perceived_traits.join(", "));
            out.push(';');
        }
        if !self.current_interests.is_empty() {
            out.push_str(" интересы — ");
            out.push_str(&self.current_interests.join(", "));
            out.push(';');
        }
        if !self.relationship_dynamic.trim().is_empty() {
            out.push_str(" отношения с собеседником — ");
            out.push_str(self.relationship_dynamic.trim());
        }
        Some(truncate_chars(out.trim_end_matches([';', ' ']), max_chars))
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

/// Усечение по границе слова: как [`truncate_chars`], но откатывается к последнему
/// пробелу в пределах лимита, чтобы не рвать слово посреди («…» внутри слова читается
/// как повреждённая память). Если пробела нет (одно длинное слово) — режет по символу.
/// Результат, как и у [`truncate_chars`], не длиннее `max_chars` символов.
fn truncate_chars_word(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let take = max_chars.saturating_sub(1);
    let head: String = s.chars().take(take).collect();
    // rfind даёт байтовый индекс пробела (на границе символа — валиден для среза).
    let base = match head.rfind(char::is_whitespace) {
        Some(idx) => head[..idx].trim_end(),
        None => head.as_str(),
    };
    // Откат съел всё (лидирующий пробел) — падаем обратно на посимвольный head.
    let base = if base.is_empty() { head.as_str() } else { base };
    format!("{base}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Дефолтные параметры рендера/хранения для тестов.
    fn p() -> SelfModelParams {
        SelfModelParams::default()
    }

    /// Точка отсчёта времени для тестов рендера (совпадает с моментом создания
    /// записей → свежие показываются как «сегодня»).
    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    /// Референсная локаль (ru) для тестов рендера: ассерты на русские подстроки
    /// одновременно пинят содержимое ru-бандла. Per-language проверки — ниже
    /// (`render_localized_for_all_langs`, `age_label_localized_for_all_langs`).
    fn loc() -> &'static Locale {
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
    }

    /// Наблюдение (self-заметка) для тестов рендера: id + текст + «сейчас».
    /// Наблюдения переехали в заметки и передаются в рендер параметром `recent`.
    fn seg(text: &str) -> NarrativeSegment {
        NarrativeSegment {
            id: Uuid::new_v4(),
            text: text.into(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn empty_model_renders_none() {
        let m = SelfModel::new(Uuid::new_v4());
        assert!(m.is_empty());
        // Пусто и структурно, и по наблюдениям → None.
        assert!(
            m.render_for_prompt(p().prompt_cap, p().narrative_in_prompt, now(), &[], loc())
                .is_none()
        );
    }

    #[test]
    fn render_includes_recent_observations() {
        // Только наблюдения (recent), структурная часть пуста → модель информативна.
        let m = SelfModel::new(Uuid::new_v4());
        let recent = [seg("заметил напряжение между «кратко» и «полно»")];
        let r = m
            .render_for_prompt(
                p().prompt_cap,
                p().narrative_in_prompt,
                now(),
                &recent,
                loc(),
            )
            .unwrap();
        assert!(r.contains("Недавние наблюдения:"));
        assert!(r.contains("напряжение"));
    }

    #[test]
    fn render_for_prompt_takes_freshest_n() {
        // recent — новейшие первыми; в промпт идут только narrative_in_prompt свежих.
        let params = SelfModelParams::from_settings(&SelfModelSettings {
            narrative_in_prompt: 2,
            prompt_cap: 1000,
            ..SelfModelSettings::default()
        });
        let m = SelfModel::new(Uuid::new_v4());
        let recent: Vec<NarrativeSegment> =
            (0..10).rev().map(|i| seg(&format!("инсайт {i}"))).collect();
        let r = m
            .render_for_prompt(
                params.prompt_cap,
                params.narrative_in_prompt,
                now(),
                &recent,
                loc(),
            )
            .unwrap();
        assert_eq!(r.matches("инсайт ").count(), 2);
        // Первые два (новейшие) — «инсайт 9», «инсайт 8».
        assert!(r.contains("инсайт 9"));
        assert!(r.contains("инсайт 8"));
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
            .render_for_prompt(p().prompt_cap, p().narrative_in_prompt, now(), &[], loc())
            .unwrap();
        assert!(r.contains("О себе: ценю честность"));
        assert!(r.contains("разобраться в коде"));
        assert!(r.contains("черты: любопытный"));
        assert!(r.contains("интересы: Rust"));
        assert!(r.contains("отношения: доверительные"));
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
    fn summary_target_sanitized_to_floor() {
        // Крошечный ориентир → пол 200 (защита от бессмысленно малого значения).
        let params = SelfModelParams::from_settings(&SelfModelSettings {
            summary_target_chars: 10,
            ..SelfModelSettings::default()
        });
        assert_eq!(params.summary_target_chars, 200);
    }

    #[test]
    fn summary_fill_hint_only_over_target() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "к".repeat(50);
        // В пределах ориентира — подсказки нет.
        assert!(m.summary_fill_hint(100, loc()).is_none());
        // Сверх ориентира — подсказка с числами.
        m.summary = "к".repeat(150);
        let hint = m.summary_fill_hint(100, loc()).unwrap();
        assert!(hint.contains("150"));
        assert!(hint.contains("100"));
        assert!(hint.contains("add_insight"));
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
            m.render_for_prompt(p().prompt_cap, p().narrative_in_prompt, now(), &[], loc())
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

        // DeleteInsight в apply_edit работает над полем `narrative` (в проде
        // оркестратор перехватывает его и удаляет self-заметку; поле оставлено для
        // реконструкции снимка `F3` и совместимости). Наполняем поле напрямую.
        m.narrative.push(seg("наблюдение"));
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
    fn bloated_summary_does_not_starve_sections() {
        // Этап 3: раздутое описание не вытесняет из инъекции цели/собеседника/наблюдения
        // (посекционный бюджет: summary ≤ половины лимита).
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "слово ".repeat(400); // ~2400 симв., много слов
        m.add_goal("активная цель");
        m.user_model.perceived_traits = vec!["внимательный".into()];
        let recent = [seg("свежее наблюдение о стиле")];

        let r = m.render_for_prompt(1200, 3, now(), &recent, loc()).unwrap();
        // Все секции присутствуют, несмотря на раздутое описание.
        assert!(r.contains("Активные цели:"), "цели вытеснены: {r}");
        assert!(r.contains("О собеседнике:"), "собеседник вытеснен: {r}");
        assert!(
            r.contains("Недавние наблюдения:"),
            "наблюдения вытеснены: {r}"
        );
        // Блок в пределах лимита; описание усечено (сверх половины бюджета).
        assert!(r.chars().count() <= 1200);
        assert!(r.contains("О себе: "));
    }

    #[test]
    fn small_summary_not_truncated() {
        // Небольшое описание проходит без «…» (поведение прежнее для нераздутых моделей).
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "ценю ясность и краткость".into();
        let r = m
            .render_for_prompt(1200, p().narrative_in_prompt, now(), &[], loc())
            .unwrap();
        assert!(r.contains("О себе: ценю ясность и краткость"));
        assert!(!r.contains('…'));
    }

    #[test]
    fn truncate_word_does_not_split_word() {
        // Усечение по границе слова не рвёт слово посреди.
        let s = "первое второе третье четвёртое пятое";
        let out = truncate_chars_word(s, 20);
        assert!(out.ends_with('…'));
        assert!(out.chars().count() <= 20);
        // Обрезка на границе слова: без «…» результат — префикс из целых слов.
        let body = out.trim_end_matches('…');
        assert!(s.starts_with(body.trim_end()));
        assert!(!body.trim_end().is_empty());
        // Одно длинное слово без пробелов — падаем на посимвольное усечение.
        let long = "я".repeat(50);
        let out = truncate_chars_word(&long, 10);
        assert_eq!(out.chars().count(), 10);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn render_truncates_to_cap() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "я".repeat(500);
        let r = m
            .render_for_prompt(50, p().narrative_in_prompt, now(), &[], loc())
            .unwrap();
        assert_eq!(r.chars().count(), 50);
        assert!(r.ends_with('…'));
    }

    #[test]
    fn render_full_shows_goal_ids_and_full_observation_ids() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.summary = "я".repeat(500);
        m.add_goal("активная цель");
        m.add_goal("завершённая цель");
        let done = m.goals[1].id;
        m.set_goal_status(done, GoalStatus::Completed);
        // Много наблюдений (recent, новейшие первыми) — полное чтение показывает все.
        let recent: Vec<NarrativeSegment> =
            (0..12).rev().map(|i| seg(&format!("инсайт {i}"))).collect();

        let full = m.render_full(now(), &recent, loc());
        assert!(!full.ends_with('…'), "полное чтение не усекается");
        // Активная цель — с коротким #id, статусом и меткой возраста («сегодня»).
        let short = short_hex(&m.goals[0].id);
        assert!(full.contains(&format!("#{short} (активна · сегодня) активная цель")));
        // Завершённая тоже видна (жизненный цикл), с возрастом от закрытия.
        assert!(full.contains("(выполнена · сегодня) завершённая цель"));
        // Все наблюдения (не только narrative_in_prompt=3), с ПОЛНЫМ id (для
        // note_revise/note_supersede).
        assert_eq!(full.matches("инсайт ").count(), 12);
        assert!(full.contains("Наблюдения (12"));
        assert!(full.contains(&format!("(id={})", recent[0].id)));
        // Полное описание себя целиком (не обрезано до prompt_cap).
        assert!(full.contains(&"я".repeat(500)));
    }

    #[test]
    fn render_full_on_empty_marks_empty() {
        let m = SelfModel::new(Uuid::new_v4());
        assert_eq!(m.render_full(now(), &[], loc()), "(модель себя пока пуста)");
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

    #[test]
    fn user_model_render_for_impersonation() {
        // Пустая → None.
        assert!(UserModel::default().render_for_impersonation(500).is_none());
        let u = UserModel {
            perceived_traits: vec!["скептик".into(), "любопытный".into()],
            current_interests: vec!["Rust".into()],
            relationship_dynamic: "доверительные, на равных".into(),
        };
        let r = u.render_for_impersonation(500).unwrap();
        assert!(r.contains("за которого ты пишешь"));
        assert!(r.contains("черты — скептик, любопытный"));
        assert!(r.contains("интересы — Rust"));
        assert!(r.contains("отношения с собеседником — доверительные, на равных"));
    }

    /// Per-language покрытие (§3.5 docs/i18n.md): рендер под КАЖДЫМ вшитым языком —
    /// секции/наблюдения помечены заголовками из бандла того же языка, плейсхолдеры
    /// подставлены (ловит битый/неполный перевод и дыры `{…}` на конкретном языке).
    #[test]
    fn render_localized_for_all_langs() {
        for &lang in crate::shared::i18n::Lang::ALL {
            let l = crate::shared::i18n::locale(lang);
            let mut m = SelfModel::new(Uuid::new_v4());
            m.summary = "s".into();
            m.add_goal("g");
            m.user_model.perceived_traits = vec!["t".into()];
            let recent = [seg("obs")];
            let r = m
                .render_for_prompt(p().prompt_cap, p().narrative_in_prompt, now(), &recent, l)
                .unwrap();
            assert!(r.contains(l.t("selfmodel.render.header")), "{lang:?}: {r}");
            assert!(r.contains(l.t("selfmodel.render.goals_active")), "{lang:?}");
            assert!(r.contains(l.t("selfmodel.render.user")), "{lang:?}");
            assert!(
                r.contains(l.t("selfmodel.render.observations_recent")),
                "{lang:?}"
            );
            // Возраст «сегодня» подставлен без остатка `{…}`.
            assert!(!r.contains('{'), "{lang:?}: остался плейсхолдер: {r}");
            // Полное чтение под тем же языком — свой заголовок, без усечения.
            let full = m.render_full(now(), &recent, l);
            assert!(full.contains(l.t("selfmodel.render.goals_ref")), "{lang:?}");
            assert!(!full.contains('{'), "{lang:?}: плейсхолдер в full: {full}");
        }
    }

    /// Per-language метки возраста: каждый корзинный вариант подставляет `{n}` и не
    /// оставляет плейсхолдера — на всех вшитых языках.
    #[test]
    fn age_label_localized_for_all_langs() {
        use chrono::Duration;
        let base = Utc::now();
        for &lang in crate::shared::i18n::Lang::ALL {
            let l = crate::shared::i18n::locale(lang);
            for d in [0i64, 1, 3, 10, 40, 400] {
                let s = age_label(base - Duration::days(d), base, l);
                assert!(!s.contains('{'), "{lang:?} d={d}: {s}");
                assert!(!s.is_empty());
            }
        }
    }

    #[test]
    fn age_label_buckets() {
        use chrono::Duration;
        let base = Utc::now();
        let ago = |d: i64| base - Duration::days(d);
        assert_eq!(age_label(base, base, loc()), "сегодня");
        assert_eq!(age_label(ago(1), base, loc()), "вчера");
        assert_eq!(age_label(ago(3), base, loc()), "3 дн.");
        assert_eq!(age_label(ago(6), base, loc()), "6 дн.");
        assert_eq!(age_label(ago(7), base, loc()), "1 нед.");
        assert_eq!(age_label(ago(20), base, loc()), "2 нед.");
        assert_eq!(age_label(ago(31), base, loc()), "1 мес.");
        assert_eq!(age_label(ago(200), base, loc()), "6 мес.");
        assert_eq!(age_label(ago(365), base, loc()), "1 г.");
        assert_eq!(age_label(ago(800), base, loc()), "2 г.");
        // Время «из будущего» (рассинхронизация часов) → «сегодня», не паника.
        assert_eq!(age_label(base + Duration::hours(5), base, loc()), "сегодня");
    }

    #[test]
    fn set_goal_status_stamps_and_clears_closed_at() {
        let mut m = SelfModel::new(Uuid::new_v4());
        m.add_goal("цель");
        let id = m.goals[0].id;
        assert!(m.goals[0].closed_at.is_none()); // активная — без отметки
        m.set_goal_status(id, GoalStatus::Completed);
        let closed = m.goals[0].closed_at;
        assert!(closed.is_some()); // закрытие проставило момент
        // Повторное закрытие (в другой статус) момент не сдвигает.
        m.set_goal_status(id, GoalStatus::Abandoned);
        assert_eq!(m.goals[0].closed_at, closed);
        // Реактивация снимает отметку.
        m.set_goal_status(id, GoalStatus::Active);
        assert!(m.goals[0].closed_at.is_none());
    }

    #[test]
    fn fold_closed_goals_returns_scars_beyond_keep() {
        use chrono::Duration;
        let mut m = SelfModel::new(Uuid::new_v4());
        // Пять закрытых целей с разным временем закрытия + одна активная.
        for i in 0..5 {
            m.add_goal(format!("закрытая {i}"));
        }
        m.add_goal("активная");
        // Закрываем первые пять, проставляя разные closed_at (старые — раньше).
        for i in 0..5 {
            let id = m.goals[i].id;
            m.set_goal_status(id, GoalStatus::Completed);
            m.goals[i].closed_at = Some(Utc::now() - Duration::days((5 - i) as i64));
        }
        // Держим 2 самых свежих закрытых, остальные 3 → возвращаются шрамами (их
        // вызывающий запишет как self-заметки).
        let scars = m.fold_closed_goals(2, loc());
        assert_eq!(scars.len(), 3);
        // Активная не тронута; всего целей: 2 закрытых + 1 активная.
        assert_eq!(m.goals.len(), 3);
        assert_eq!(m.active_goals().count(), 1);
        // Шрамы — записи «[архив цели]»; самая старая закрытая среди них.
        assert!(scars.iter().all(|s| s.starts_with("[архив цели]")));
        assert!(scars.iter().any(|s| s.contains("закрытая 0")));
        // Меньше keep закрытых → пусто.
        assert!(m.fold_closed_goals(2, loc()).is_empty());
    }
}
