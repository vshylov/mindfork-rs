# План: SelfModel MVP-зонд (урезанный Phase 1)

Документ — реализуемый план минимальной версии «модели себя» агента, выжимка из
банка идей [self-model.md](self-model.md). Цель — проверить **поведенческую**
пользу (вспоминает ли локальная модель факты о пользователе и свои цели между
чатами), а не построить полную мета-когнитивную машинерию.

## Ключевое отступление от исходного документа (осознанное)

Документ моделировал `SelfModel` через `ChatEffect`, как состояние `Chat`. В
реальной архитектуре пер-профильные данные (заметки, RAG) пишутся инструментами
**напрямую** в SQLite (`ctx.storage.db().note_insert(...)`; `Storage` —
потокобезопасный `Arc` + внутренний мьютекс). `ChatEffect` существует только для
мутаций `Chat` (его единолично владеет оркестратор, и его нет в БД во время хода).
`SelfModel` — пер-профильные данные, как заметки, поэтому:

- **Новые варианты `ChatEffect` не нужны** — инструменты-мутаторы пишут `SelfModel`
  напрямую, как `note_save`.
- Инвариант «единственный владелец `Chat`» не затрагивается (`SelfModel` — не `Chat`).

## Принципы

- `SelfModel` живёт в SQLite, изоляция по `profile_id` (как notes/RAG), один ряд на
  профиль.
- Оркестратор читает `SelfModel` в начале хода: кладёт снимок в `ToolContext` и
  **компактно** подмешивает в `system`-промпт (вот где появляется польза).
- Инструменты **опциональны** (как `send_followup_message`): в `all_tool_ids`, но не
  в `default_tool_ids` — существующим профилям не навязываются, видны как тумблеры в
  настройках профиля.
- Минимум структуры: свободный текст + цели + модель пользователя. Никаких
  `strength: f32`/`severity` (провоцируют бессмысленные числа).

## Шаг 1 — Сущность `src/entities/self_model.rs`

```rust
pub struct SelfModel {
    pub profile_id: Uuid,
    pub version: u64,
    pub summary: String,             // свободный текст «о себе»
    pub goals: Vec<Goal>,
    pub user_model: UserModel,
    pub updated_at: DateTime<Utc>,
}
pub struct Goal { pub id: Uuid, pub description: String, pub status: GoalStatus }
pub enum GoalStatus { Active, Completed, Abandoned }
pub struct UserModel {            // всё Vec<String>/String, без id
    pub perceived_traits: Vec<String>,
    pub current_interests: Vec<String>,
    pub relationship_dynamic: String,
}
```

Методы: `new(profile_id)` (пустая), `is_empty()`,
`render_for_prompt(max_chars) -> Option<String>` (компактный блок «[Модель себя]
О себе: … / Активные цели: … / О собеседнике: …», с усечением ради 8k-контекста).
serde, `#[serde(default)]` на новых полях по конвенции.

## Шаг 2 — Хранилище `src/shared/storage/db.rs`

В `migrate()` добавить (рядом с `notes`/`rag_sources`, `CREATE TABLE IF NOT EXISTS`
→ миграция не нужна):

```sql
CREATE TABLE IF NOT EXISTS self_models (
    profile_id  TEXT PRIMARY KEY,
    data        TEXT NOT NULL,    -- JSON всей модели
    version     INTEGER NOT NULL,
    updated_at  TEXT NOT NULL
);
```

Методы: `self_model_get(profile_id) -> Result<Option<SelfModel>>`,
`self_model_upsert(&SelfModel)` (INSERT OR REPLACE, version+1). Изоляция — через PK
`profile_id`. Тест: `self_model_isolated_by_profile` + upsert→get round-trip.

## Шаг 3 — `ToolContext` (`features/tools/mod.rs`)

- Добавить поле `pub self_model: Option<SelfModel>` (снимок на начало хода).
- В `testkit::ctx_with_storage` добавить `self_model: None`.

## Шаг 4 — Инструменты `src/features/tools/self_model.rs` (4 шт., по шаблону `notes.rs`)

1. **`get_self_model`** — читает `ctx.self_model` (или из БД), возвращает `render`.
   Без записи.
2. **`reflect`** — возвращает текущую модель + короткую рубрику-нуджет («что
   изменилось о тебе / о собеседнике / о твоих целях?»). Чистый текст, без записи —
   точка входа рефлексии; дальше модель сама зовёт update-инструменты.
3. **`update_self_model`** — args: `summary?`, `add_goals: [string]?`,
   `complete_goals: [uuid]?`, `abandon_goals: [uuid]?`. Мёрджит в хранимую модель,
   пишет `self_model_upsert`. (Управление целями свёрнуто сюда, чтобы не плодить
   `set_goal`/`revise_goal`/`abandon_goal`.)
4. **`update_user_model`** — args: `perceived_traits?`, `current_interests?`,
   `relationship_dynamic?`. Мёрдж + upsert.

Все читают/пишут под `ctx.profile_id`. Ошибки — текстом в результат, не паника
(контракт `Tool`).

## Шаг 5 — Реестр и гейтинг (`features/tools/mod.rs`)

- Константы id; `standard_registry` — `reg.register(...)` всех четырёх.
- В `all_tool_ids()` добавить четыре id (в `default_tool_ids` — **нет**).
- `effective_tool_ids`: проходят через `_ => true` (DB-only, безопасны, глобальный
  выключатель не нужен).
- Тумблеры профиля в `screens/settings.rs::tool_catalog` подхватят их автоматически
  (он строится из `all_tool_ids`).

## Шаг 6 — Оркестратор (`app/orchestrator/generation.rs`, `start_generation`)

- Загрузить снимок: `let self_model = self.storage.db().self_model_get(profile_id).ok().flatten();`
- Положить в `ToolContext { …, self_model: self_model.clone() }`.
- **Инъекция в промпт** (после `build_request`): если модель непустая *и* профиль
  имеет включённым `get_self_model` (opt-in), дописать `render_for_prompt(CAP)` в
  `request.system`. Небольшой хелпер; `ChatRequest.system: Option<String>`.
- `handle_done` — **не трогаем** (эффектов нет).
- Снимок в `ToolContext` намеренно может устаревать в пределах хода (как у notes) —
  следующий ход перечитывает из БД.

## Шаг 7 — Тесты

- entity: merge целей (add/complete/abandon), `render_for_prompt`, усечение,
  `is_empty`.
- db: изоляция + round-trip (Шаг 2).
- tools: `get` рендерит; `update_self_model` мёрджит и персистит;
  `update_user_model`; `reflect` возвращает текущее+рубрику. Через
  `testkit::ctx_with_storage`.
- mod: `all_tool_ids` содержит четыре, `default_tool_ids` — нет.
- orchestrator (фокусный): инъекция в `request.system` при непустой модели +
  включённом инструменте; отсутствие инъекции при пустой/выключенной.

## Шаг 8 — Оценка зонда (ради этого всё и делается)

Определить **до** мержа: 2–3 длинных мульти-сессионных диалога, прогон с фичей
off/on, сравнить — вспоминает ли модель факты о пользователе и свои цели между
чатами. Зафиксировать критерий go/no-go (продолжать ли к Tier 2).

## Явно вне зоны MVP

`beliefs` с числами, `contradictions`/detect/resolve, нарратив, версионная история,
авто-рефлексия по таймеру, вся Phase 4, UI-оверлей просмотра. Если зонд взлетит —
Tier 2 отдельным заходом.

## Объём

~1.5–2 дня на код+тесты (паттерны уже есть: сущность ≈ `note.rs`, инструменты ≈
`notes.rs`, БД ≈ методы notes). После — обновить `CLAUDE.md`/`architecture.md`
(новая группа инструментов, поле `ToolContext`, таблица БД).
</content>
</invoke>
