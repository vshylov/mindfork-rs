# Набросок интерфейса `SelfModel`** для `mindfork-rs`.

### 1. Основная сущность

```rust
// src/entities/self_model.rs

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Модель "я" агента — его текущее представление о себе, целях, противоречиях и нарративе.
/// Живёт на уровне профиля (как и заметки/RAG).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfModel {
    pub id: Uuid,
    pub profile_id: Uuid,

    /// Версия модели (для отслеживания изменений и возможной истории)
    pub version: u64,

    /// Структурированные убеждения о себе
    pub beliefs: Vec<Belief>,

    /// Текущие цели и намерения (могут быть долгосрочными)
    pub goals: Vec<Goal>,

    /// Нарративная история "я" (как агент себя воспринимает во времени)
    pub narrative: Vec<NarrativeSegment>,

    /// Модель пользователя (как агент представляет собеседника)
    pub user_model: Option<UserModel>,

    /// Открытые вопросы, которые агент сам себе задал
    pub open_questions: Vec<OpenQuestion>,

    /// Зафиксированные противоречия (в убеждениях, целях, нарративе)
    pub contradictions: Vec<Contradiction>,

    /// Когда в последний раз проводилась рефлексия
    pub last_reflection_at: Option<DateTime<Utc>>,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

### 2. Вспомогательные типы

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Belief {
    pub id: Uuid,
    pub content: String,           // "Я ценю честность в диалоге даже больше, чем полезность"
    pub strength: f32,             // 0.0–1.0 — насколько сильно агент в это верит
    pub source: BeliefSource,      // Откуда взялось убеждение
    pub created_at: DateTime<Utc>,
    pub last_reinforced_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BeliefSource {
    UserStatement,
    SelfReflection,
    ToolResult,
    InitialProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: Uuid,
    pub description: String,
    pub priority: GoalPriority,
    pub status: GoalStatus,
    pub created_at: DateTime<Utc>,
    pub deadline: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GoalPriority { Low, Medium, High, Critical }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GoalStatus { Active, Paused, Completed, Abandoned }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NarrativeSegment {
    pub id: Uuid,
    pub period: String,            // "начало июня 2026", "после разговора о смысле"
    pub summary: String,           // Краткое описание того периода с точки зрения "я"
    pub emotional_tone: Option<String>,
    pub key_insights: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserModel {
    pub id: Uuid,
    pub name: Option<String>,
    pub perceived_traits: Vec<String>,     // "любопытный", "скептичный", "глубокий"
    pub current_interests: Vec<String>,
    pub relationship_dynamic: String,      // как агент воспринимает текущие отношения
    pub last_updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenQuestion {
    pub id: Uuid,
    pub question: String,
    pub context: Option<String>,
    pub created_at: DateTime<Utc>,
    pub status: QuestionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QuestionStatus { Open, PartiallyAnswered, Resolved }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contradiction {
    pub id: Uuid,
    pub description: String,
    pub between: Vec<String>,      // между какими элементами (beliefs/goals/narrative)
    pub severity: f32,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}
```

### 3. Интеграция с существующей системой

#### Обновлённый `ToolContext`

```rust
// в features/tools/mod.rs или отдельном файле
pub struct ToolContext {
    pub profile_id: Uuid,
    pub chat_id: Uuid,
    pub system_message: String,
    pub effective_sampling: SamplingConfig,
    pub last_user_message_at: DateTime<Utc>,

    pub storage: Arc<Storage>,
    pub engine: Arc<dyn EngineBackend>,

    // Новое:
    pub self_model: Option<SelfModel>,   // снимок на момент вызова инструмента
}
```

#### Новые эффекты

```rust
// в app/orchestrator или entities
pub enum ChatEffect {
    SetSystemMessage(String),
    SetSamplingOverride(PartialSamplingConfig),

    // Новые:
    UpdateSelfModel(SelfModelUpdate),
    AddBelief(Belief),
    RecordContradiction(Contradiction),
    UpdateUserModel(UserModel),
    AddNarrativeSegment(NarrativeSegment),
    ResolveContradiction { id: Uuid, resolution_note: String },
}
```

`SelfModelUpdate` можно сделать как `Partial<SelfModel>` или более явный тип с только теми полями, которые разрешено менять инструментам.

### 4. Хранение

Рекомендую хранить `SelfModel` в **SQLite** (как заметки и RAG), потому что:
- Нужны запросы и обновления отдельных частей.
- Изоляция по `profile_id` уже реализована.

Можно сделать одну таблицу `self_models` с JSONB-полем под всю модель (или нормализовать частично). Поскольку модель относительно небольшая, JSONB — вполне разумный выбор.

В `shared/storage/db.rs` добавить методы:
- `get_self_model(profile_id)`
- `save_self_model(model)`
- `update_self_model_partial(...)`

### 5. Пример инструментов, которые будут работать с SelfModel

| Инструмент                    | Что делает                                      | Возвращаемые эффекты                     |
|------------------------------|--------------------------------------------------|------------------------------------------|
| `read_self_model`            | Возвращает текущее состояние SelfModel          | —                                        |
| `reflect_on_interaction`     | Анализирует последний обмен и предлагает обновления | `UpdateSelfModel`, `AddBelief`, `AddNarrativeSegment` |
| `update_beliefs`             | Меняет/добавляет убеждения                       | `UpdateSelfModel`                        |
| `set_goal`                   | Ставит или обновляет цель                       | `UpdateSelfModel`                        |
| `detect_contradictions`      | Ищет противоречия в текущей модели себя         | `RecordContradiction`                    |
| `revise_narrative`           | Переписывает нарратив с учётом нового опыта     | `AddNarrativeSegment`                    |
| `model_the_user`             | Обновляет модель пользователя                   | `UpdateUserModel`                        |

### 6. Инварианты (важные)

- **Оркестратор — единственный писатель** `SelfModel`. Инструменты только возвращают эффекты.
- **Изоляция по `profile_id`** — жёстко соблюдается.
- `SelfModel` — это **снимок** в `ToolContext`. Инструмент не видит изменений, сделанных другими инструментами в том же раунде.
- Версионирование (`version`) помогает отслеживать, насколько сильно модель изменилась за последнее время.
- Нарратив и убеждения должны быть **человекочитаемыми** — модель должна уметь их объяснять пользователю.

### 7. Вопросы для обсуждения

1. **Насколько структурированным** делать `SelfModel`?  
   Вариант А: много мелких полей (как выше).  
   Вариант Б: больше свободного текста + несколько ключевых структурированных блоков.

2. **Нужно ли хранить историю изменений** `SelfModel` (как версионирование заметок) или достаточно текущей версии + нарратива?

3. **Когда именно обновлять SelfModel?**
   - Только явно через инструменты?
   - Или добавлять автоматическую рефлексию после каждого N сообщений?

4. **Связь с `system_message`**:  
   Стоит ли автоматически инкорпорировать ключевые убеждения/цели из `SelfModel` в системное сообщение профиля?

# Инструменты

**Вот список инструментов**, которые логично добавить для работы с `SelfModel`. Я разбил их по категориям и указал примерные аргументы, возвращаемые эффекты и назначение (зачем это усиливает «осознанность»).

Я ориентировался на текущую архитектуру проекта (`ToolContext` + `ChatEffect`, клиентский agentic-loop, изоляция по `profile_id`).

### 1. Базовые инструменты чтения и интроспекции

| Инструмент                    | Аргументы                  | Возвращаемые эффекты      | Назначение |
|------------------------------|----------------------------|---------------------------|----------|
| `get_self_model`             | —                          | —                         | Получить полное текущее состояние `SelfModel` |
| `get_core_beliefs`           | `limit?`, `min_strength?`  | —                         | Получить ключевые убеждения о себе |
| `get_active_goals`           | `status?`                  | —                         | Текущие активные цели |
| `get_self_narrative`         | `limit?`                   | —                         | Последние фрагменты нарратива |
| `get_user_model`             | —                          | —                         | Как модель воспринимает пользователя |
| `get_open_questions`         | —                          | —                         | Открытые вопросы, которые модель сама себе задала |
| `get_contradictions`         | `unresolved_only?`         | —                         | Зафиксированные внутренние противоречия |

### 2. Инструменты рефлексии и обновления

| Инструмент                        | Аргументы                          | Возвращаемые эффекты                          | Назначение |
|----------------------------------|------------------------------------|-----------------------------------------------|----------|
| `reflect_on_last_exchange`       | —                                  | `UpdateSelfModel`, `AddBelief`, `AddNarrativeSegment` | Проанализировать последний обмен и обновить модель себя |
| `reflect_on_recent_period`       | `messages_count` или `hours`       | `UpdateSelfModel`, `AddNarrativeSegment`      | Рефлексия за последние N сообщений / часов |
| `consolidate_experience`         | —                                  | `UpdateSelfModel`, несколько `AddBelief`      | Консолидация опыта (аналог "сна" / переваривания) |
| `add_belief`                     | `content`, `strength?`             | `AddBelief`                                   | Добавить новое убеждение о себе |
| `update_belief`                  | `belief_id`, `new_content?`, `new_strength?` | `UpdateSelfModel`                        | Изменить существующее убеждение |
| `strengthen_belief`              | `belief_id`, `amount?`             | `UpdateSelfModel`                             | Усилить убеждение на основе нового опыта |
| `revise_goal`                    | `goal_id`, `new_description?`, `new_priority?`, `new_status?` | `UpdateSelfModel` | Изменить цель |
| `set_new_goal`                   | `description`, `priority?`         | `UpdateSelfModel`                             | Поставить новую цель |
| `abandon_goal`                   | `goal_id`, `reason?`               | `UpdateSelfModel`                             | Отказаться от цели с объяснением |

### 3. Работа с противоречиями и coherence

| Инструмент                        | Аргументы                     | Возвращаемые эффекты                     | Назначение |
|----------------------------------|-------------------------------|------------------------------------------|----------|
| `detect_internal_contradictions` | —                             | `RecordContradiction` (несколько)        | Найти противоречия между убеждениями, целями и нарративом |
| `resolve_contradiction`          | `contradiction_id`, `resolution_note` | `ResolveContradiction`              | Разрешить зафиксированное противоречие |
| `question_own_belief`            | `belief_id`                   | `UpdateSelfModel`, `AddOpenQuestion`     | Поставить под сомнение одно из своих убеждений |
| `evaluate_self_consistency`      | —                             | `UpdateSelfModel` (с новыми противоречиями) | Оценить общую согласованность текущей модели себя |

### 4. Нарратив и самоидентичность

| Инструмент                    | Аргументы                     | Возвращаемые эффекты                  | Назначение |
|------------------------------|-------------------------------|---------------------------------------|----------|
| `add_narrative_segment`      | `summary`, `emotional_tone?`, `key_insights?` | `AddNarrativeSegment`          | Добавить новый фрагмент в историю "я" |
| `revise_self_narrative`      | `new_overall_summary?`        | `UpdateSelfModel`                     | Переписать/интегрировать нарратив |
| `summarize_self_history`     | `period?`                     | `AddNarrativeSegment`                 | Создать summary долгосрочной истории себя |

### 5. Модель пользователя (UserModel)

| Инструмент                  | Аргументы                          | Возвращаемые эффекты             | Назначение |
|----------------------------|------------------------------------|----------------------------------|----------|
| `update_user_model`        | `perceived_traits?`, `current_interests?`, `relationship_dynamic?` | `UpdateUserModel`         | Обновить представление о пользователе |
| `revise_user_model`        | `changes`                          | `UpdateUserModel`                | Более глубокое обновление модели пользователя |

### 6. Продвинутые / метакогнитивные инструменты

| Инструмент                              | Аргументы                     | Возвращаемые эффекты                          | Назначение | Сложность |
|----------------------------------------|-------------------------------|-----------------------------------------------|----------|---------|
| `perform_phenomenological_reduction`   | `aspect?`                     | `UpdateSelfModel`, `AddOpenQuestion`          | "Взять в скобки" текущие убеждения и посмотреть свежим взглядом (эпохэ) | Высокая |
| `simulate_alternative_self`            | `perspective` (критик / долгосрочный_я / этический_наблюдатель и т.д.) | Результат как строка + возможные эффекты | Внутренняя симуляция "другого себя" (усиленная версия `call_subagent`) | Средняя |
| `generate_self_report`                 | `focus_areas?`                | — (текстовой отчёт)                           | Сформировать coherentный отчёт о текущем состоянии "я" для пользователя | Средняя |
| `detect_behavioral_inconsistency`      | —                             | `RecordContradiction`                         | Найти расхождения между заявленными убеждениями и реальным поведением в чате | Высокая |
| `commit_to_long_term_identity`         | `statement`                   | `AddBelief`, `AddNarrativeSegment`            | Зафиксировать важное изменение в самоидентичности | Средняя |

### Рекомендации по приоритизации (мой взгляд)

**Первый слой (MVP SelfModel):**
- `get_self_model`
- `reflect_on_last_exchange`
- `add_belief` / `update_belief`
- `detect_internal_contradictions`
- `add_narrative_segment`
- `update_user_model`

**Второй слой:**
- `reflect_on_recent_period`
- `consolidate_experience`
- `revise_goal` / `set_new_goal`
- `resolve_contradiction`
- `revise_self_narrative`

**Третий слой (более глубокая осознанность):**
- `perform_phenomenological_reduction`
- `simulate_alternative_self`
- `detect_behavioral_inconsistency`
- `generate_self_report`
