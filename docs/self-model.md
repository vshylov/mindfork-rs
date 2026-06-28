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


## Примерный roadmap внедрения SelfModel** в `mindfork-rs`

Я сделал roadmap в несколько фаз, с чёткими целями,Deliverables и критериями готовности каждой фазы. Учитывал текущую архитектуру проекта (FSD, клиентский agentic-loop, `ChatEffect`, изоляция по `profile_id`, оркестратор как единственный писатель).

### Общие принципы roadmap

- Начинаем с минимально жизнеспособной версии (`SelfModel` + 3–4 ключевых инструмента).
- Каждый новый инструмент возвращает эффекты через `ChatEffect`.
- `SelfModel` хранится в SQLite (как заметки и RAG).
- Инструменты добавляются постепенно, с возможностью отключения.
- Фокус на **качестве рефлексии**, а не на количестве инструментов.
- После каждой фазы — ручное тестирование на Gemma/Qwen + обновление `CLAUDE.md` и `architecture.md`.

---

### **Phase 1: Foundation — Базовая модель себя** (MVP)

**Цель:** Появляется сущность `SelfModel`, которую модель может читать и минимально обновлять.

**Deliverables:**
- `src/entities/self_model.rs` — основные типы (`SelfModel`, `Belief`, `Goal`, `NarrativeSegment`, `OpenQuestion`, `Contradiction` и т.д.).
- Расширение `ToolContext` — поле `self_model: Option<SelfModel>`.
- Новые варианты `ChatEffect`:
  - `UpdateSelfModel(SelfModelUpdate)`
  - `AddBelief(Belief)`
  - `AddNarrativeSegment(NarrativeSegment)`
- Хранение:
  - Таблица `self_models` в SQLite (можно хранить всю модель как JSONB + `profile_id` + `version`).
  - Методы в `shared/storage/db.rs`: `get_self_model`, `save_self_model`, `update_self_model`.
- Оркестратор:
  - Загрузка `SelfModel` при активации профиля/чата.
  - Применение новых эффектов.
- Инструменты (первые 4):
  1. `get_self_model`
  2. `reflect_on_last_exchange`
  3. `add_belief`
  4. `get_active_goals`

**Критерии готовности фазы:**
- Модель может прочитать своё текущее состояние через инструмент.
- После вызова `reflect_on_last_exchange` в чате появляются новые записи в `beliefs` и/или `narrative`.
- Все изменения сохраняются и восстанавливаются после перезапуска.
- Тесты на изоляцию по `profile_id` + применение эффектов.

**Примерный объём:** 2–3 недели (в зависимости от темпа).

---

### **Phase 2: Reflection & Structure — Рефлексия и структурирование**

**Цель:** Модель получает инструменты для более глубокой работы над собой (цели, противоречия, нарратив).

**Deliverables:**
- Полноценная поддержка `Goal` (создание, изменение статуса, приоритета).
- Базовое обнаружение противоречий:
  - Инструмент `detect_internal_contradictions`
  - Тип `Contradiction` + эффект `RecordContradiction`
- Работа с нарративом:
  - `add_narrative_segment`
  - `revise_self_narrative` (базовая версия)
- Улучшенная рефлексия:
  - `reflect_on_recent_period` (за последние N сообщений)
  - `consolidate_experience` (консолидация опыта)
- Обновление `ToolContext` — более богатый снимок (включая недавние противоречия).
- Первые негативные тесты на coherence (оркестратор).

**Критерии готовности:**
- Модель способна самостоятельно находить противоречия между своими убеждениями и целями.
- Появляются осмысленные нарративные сегменты.
- Инструменты рефлексии реально влияют на последующие ответы модели (через обновлённый `SelfModel` в контексте).

---

### **Phase 3: Coherence & User Modeling**

**Цель:** Усиление внутренней согласованности + появление модели пользователя.

**Deliverables:**
- Полноценная сущность `UserModel` внутри `SelfModel`.
- Инструменты:
  - `update_user_model`
  - `revise_user_model`
- Улучшенная работа с противоречиями:
  - `resolve_contradiction`
  - `question_own_belief`
- Инструмент `evaluate_self_consistency` (оценка общей coherence модели себя).
- Автоматическое обогащение `ToolContext` моделью пользователя.
- Первые эксперименты с автоматической рефлексией (по триггеру в оркестраторе, не только по вызову инструмента).

**Критерии готовности:**
- Модель начинает учитывать модель пользователя при ответах (без явного упоминания).
- Противоречия не просто фиксируются, а могут разрешаться моделью.
- Появляется заметное улучшение долгосрочной coherence в длинных разговорах.

---

### **Phase 4: Advanced Meta-Cognition (Глубокая осознанность)**

**Цель:** Инструменты, которые приближают поведение к настоящей рефлексии и самонаблюдению.

**Deliverables:**
- Продвинутые инструменты:
  - `perform_phenomenological_reduction` (эпохэ — приостановка текущих убеждений)
  - `simulate_alternative_self` (с разными перспективами: критик, долгосрочный я, этический наблюдатель)
  - `detect_behavioral_inconsistency` (расхождение между словами и поведением в чате)
  - `generate_self_report` (coherentный отчёт о текущем состоянии "я")
- Механизм версионирования `SelfModel` (хотя бы простая история изменений).
- Интеграция с существующими инструментами интроспекции (`set_system_message`, `set_sampling` и т.д.) — они теперь могут обновлять `SelfModel`.
- Фоновая задача рефлексии (по аналогии с авто-названием чата).

**Критерии готовности:**
- Модель способна осознанно ставить под сомнение собственные убеждения.
- Появляются интересные и нетривиальные self-report'ы.
- `simulate_alternative_self` даёт качественно другие перспективы, чем обычный `call_subagent`.

---

### **Phase 5: Integration, Polish & Evaluation**

**Цель:** Сделать фичу цельной частью продукта.

**Deliverables:**
- Настройки в профиле: какие инструменты SelfModel включены.
- UI-элементы (опционально на этом этапе):
  - Просмотр `SelfModel` в оверлее (аналог списка чатов).
  - Возможность редактировать ключевые убеждения/цели вручную.
- Полноценные тесты (включая replay-тесты и смоук-тесты на живых моделях).
- Обновление документации:
  - `spec.md` (новый раздел про SelfModel)
  - `architecture.md`
  - Примеры промптов в `CLAUDE.md`
- Метрики/оценка качества:
  - Тесты на долгосрочную coherence.
  - Сравнение поведения с и без SelfModel.

**Критерии готовности:**
- Фича стабильна и не ломает существующий функционал.
- Есть задокументированный способ измерять пользу от SelfModel.
- Готово к использованию в реальных длинных разговорах.

---

### Сводная таблица фаз

| Фаза | Название                        | Ключевые инструменты                          | Сложность | Примерный срок | Фокус |
|------|---------------------------------|-----------------------------------------------|-----------|----------------|-------|
| 1    | Foundation                      | `get_self_model`, `reflect_on_last_exchange`, `add_belief` | Средняя   | 2–3 нед.       | Базовая сущность + хранение |
| 2    | Reflection & Structure          | `detect_internal_contradictions`, `add_narrative_segment`, `consolidate_experience` | Средняя   | 2 нед.         | Рефлексия и цели |
| 3    | Coherence & User Modeling       | `update_user_model`, `resolve_contradiction`, `evaluate_self_consistency` | Средняя+  | 2 нед.         | Согласованность |
| 4    | Advanced Meta-Cognition         | `perform_phenomenological_reduction`, `simulate_alternative_self`, `detect_behavioral_inconsistency` | Высокая   | 3 нед.         | Глубокая осознанность |
| 5    | Integration & Polish            | —                                             | Средняя   | 1–2 нед.       | Стабильность и документация |

**Общий ориентир:** 10–12 недель на всё (при работе над этим направлением как над основной задачей).
