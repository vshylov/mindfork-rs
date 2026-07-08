# План рефакторинга: точечные SOLID-улучшения (2026-07)

Дизайн-план направления. Статус: **не начат**.

Происхождение — оценка соблюдения SOLID по всей кодовой базе (2026-07-08):
архитектура здорова (транспорт за трейтами, FSD, единые источники истины,
god-object'ы разобраны — [refactoring-god-objects.md](refactoring-god-objects.md)),
остаточный долг **точечный** и сосредоточен на четырёх осях, по которым проект
активно растёт. План — четыре независимых этапа, каждый = отдельный PR.

Базовая линия: **808 юнит-тестов зелёные, 26 `#[ignore]`-смоуков** (main,
2026-07-08).

## 1. Контекст и диагноз

| # | Ось | Принцип | Симптом | Очаг |
|---|-----|---------|---------|------|
| 1 | Сборка `ToolContext` | ISP, shotgun surgery | 11 полей литералом в **8 местах**; новое поле = правка всех (прожито в этапе 5.4 доводки модели себя) | `features/tools/mod.rs:43` + 3 продакшн- и 5 тест-сайтов |
| 2 | Фоновые задачи оркестратора | SRP, OCP по растущей оси | триплет полей + канал + ветка `select!` + обработчик **на каждую** задачу; roadmap уже содержит следующую (авто-консолидация модели себя) | `app/orchestrator/mod.rs` (поля, `run()`, `Quit`), `reflection.rs`, `consolidation.rs` |
| 3 | Поля экрана настроек | SRP, OCP | аспекты одного поля размазаны по **5 match-сайтам** в 4 файлах (~80 вариантов `FieldId`, 311 использований) | `screens/settings/{mod,catalog,apply,helpers,choice}.rs` |
| 4 | Мелкие точечные | ISP, SRP | 10-аргументные сигнатуры статус-бара; поимённые перечисления экранов в runtime; 4 копипаст-блока диспетчеризации intent'ов | `widgets/status_bar.rs`, `app/runtime/{dispatch,input,mod}.rs`, `screens/chat/mod.rs` |

Диагноз без излишеств: **системной перестройки не требуется**. Замкнутые enum +
exhaustive match — идиоматичный для Rust выбор (компилятор проводит по всем
местам правки), поэтому часть «нарушений OCP» — осознанная цена, а не долг.
Лечим только оси, где ripple реально мешает при каждом расширении.

## 2. Метод и правила (общие для всех этапов)

Тот же плейбук, что у разбора god-object'ов
([refactoring-god-objects.md §2](refactoring-god-objects.md)); дополнения — под
характер этих этапов (здесь не перенос кода, а **введение малых типов/хелперов
без изменения поведения**):

1. **Поведение не меняется.** Ни новых фич, ни изменения текстов сообщений/
   событий/логов (тексты ошибок фоновых задач — байт-в-байт: на них смотрят
   тесты и журнал). Диффы читаются как «поля сгруппированы / сайты сведены к
   хелперу».
2. **Инварианты целы**: единственный владелец `Chat` — оркестратор; FSD
   (`screens`/`widgets` не знают `app`); никаких новых каналов, кроме явно
   заявленной замены двух каналов одним (этап 2).
3. **Гейты на каждый PR**: `cargo fmt`, `cargo clippy --all-targets -- -D
   warnings`, `cargo test` — зелёные; число тестов **не уменьшилось** (808+).
   Имена существующих тест-функций не меняются; правятся только конструкции
   внутри (например, сборка `ToolContext` через новый конструктор).
4. **Один этап — один PR**, ветка от `main`. Этапы независимы (кроме 3.2 после
   3.1) — порядок можно менять.
5. **Доки**: architecture.md (затронутые §) + журнальная запись CLAUDE.md на
   каждом этапе; по завершении направления — отметка здесь.

---

## 3. Этап 1 — `ToolContext`: пучки зависимостей + конструктор (ISP / ripple)

**Цель:** новое поле контекста инструментов правит **один файл**, а не 8;
у сайтов сборки исчезает 11-строчный литерал.

### Текущее состояние

`ToolContext` — 11 плоских полей ([tools/mod.rs:43-69](../src/features/tools/mod.rs)):
идентичность хода (`profile_id`, `chat_id`), снимок чата (`system_message`,
`effective_sampling`, `last_user_message_at`), разделяемые зависимости
(`storage`, `engine`, `embedder`) и конфиг-параметры (`chunk_params`,
`self_model_params`, `recall_includes_self`). Собирается **сырым литералом**:

- продакшн: [generation.rs:231](../src/app/orchestrator/generation.rs),
  [reflection.rs:238](../src/app/orchestrator/reflection.rs),
  [consolidation.rs:125](../src/app/orchestrator/consolidation.rs);
- тесты: `tools/mod.rs:390` (testkit), `web.rs:952`, `subagent.rs:181`,
  `rag.rs:789`, `fetch.rs:261`.

Конфиг-параметры — растущая часть (три штуки добавлены тремя разными фичами);
каждая новая правила все 8 сайтов. ISP-послабление (у `calculate`/`current_time`
есть `storage`/`engine`, которые им не нужны) — **не лечим**: сегрегация
контекста по группам инструментов не окупится.

### Целевое состояние

Три строительных блока + конструктор; **плоские публичные поля `ToolContext`
сохраняются** → код инструментов (`ctx.storage`, `ctx.chunk_params`, …) не
тронут вообще:

```rust
/// Долгоживущие разделяемые зависимости (пучок Arc; меняется при рестарте
/// серверов, не от хода к ходу).
#[derive(Clone)]
pub struct ToolDeps {
    pub storage: Arc<Storage>,
    pub engine: Arc<dyn EngineBackend>,
    pub embedder: Arc<dyn Embedder>,
}

/// Параметры инструментов из конфига (снимок на ход).
#[derive(Clone)]
pub struct ToolParams {
    pub chunk_params: rag::ChunkParams,
    pub self_model_params: SelfModelParams,
    pub recall_includes_self: bool,
}
impl ToolParams {
    pub fn from_config(cfg: &AppConfig) -> Self { … } // единственное место маппинга
}

/// Снимок хода (что видит инструмент о текущем чате).
pub struct TurnInfo {
    pub profile_id: Uuid,
    pub chat_id: Uuid,
    pub system_message: String,
    pub effective_sampling: SamplingConfig,
    pub last_user_message_at: Option<DateTime<Utc>>,
}

impl ToolContext {
    /// Разворачивает блоки в прежние плоские поля.
    pub fn new(deps: ToolDeps, params: ToolParams, turn: TurnInfo) -> Self { … }
}
```

FSD чист: `features/tools` уже импортирует `shared::config` (`ToolConfig`
ссылается на `DEFAULT_SUBAGENT_*`), так что `ToolParams::from_config(&AppConfig)`
законен.

### Шаги

1. `tools/mod.rs`: добавить `ToolDeps`/`ToolParams`/`TurnInfo` +
   `ToolContext::new` (поля структуры не меняются).
2. Оркестратор: хелпер `fn tool_deps(&self, backend: Arc<dyn EngineBackend>) ->
   ToolDeps` (в `orchestrator/mod.rs`, рядом с прочими общими хелперами);
   три продакшн-сайта переводятся на
   `ToolContext::new(self.tool_deps(…), ToolParams::from_config(&self.config), TurnInfo { … })`.
3. testkit: `ctx_with_storage` строит через `new`; добавить
   `ctx_with_backends(profile_id, engine, embedder)` для четырёх сайтов с
   кастомным движком/эмбеддером (web/subagent/rag/fetch) — их литералы уходят.
4. Проверка ripple: «примерочная» правка (добавить фиктивное поле в `ToolParams`,
   убедиться, что компилятор требует правку только `tools/mod.rs`) — до коммита
   откатывается.

### Что НЕ делаем

- Не вкладываем пучки в `ToolContext` полями (`ctx.deps.storage`) — тронуло бы
  все ~15 файлов инструментов ради нулевой функциональной выгоды.
- Не трогаем `#[allow(dead_code)] chat_id` и семантику снимка.
- Не вводим трейт-сегрегацию контекста по группам инструментов.

**Риски:** минимальные (конструктор + замена литералов). **Объём:** ~0.5 сессии.

**DoD:** 808+ тестов зелёные; `rg "ToolContext \{" src` находит **только**
`ToolContext::new` (единственный литерал в конструкторе); файлы инструментов
(кроме тестовых конструкций) не изменены.

---

## 4. Этап 2 — фоновые задачи: единый done-канал + реестр слотов (SRP / OCP)

**Цель:** добавление тихой фоновой задачи №3 (в roadmap уже заявлена
авто-консолидация модели себя по таймеру — architecture.md §9.9) не трогает
каркас `run()`, ветку `Quit` и не плодит поля/каналы/обработчики.

### Текущее состояние

Семейство «тихих» фоновых задач (мини agentic-loop без UI: рефлексия +
консолидация, общий раннер `tool_loop::spawn_silent_loop`) обслуживается
**копипастой жизненного цикла**:

- поля `Orchestrator` ([mod.rs:232-280](../src/app/orchestrator/mod.rs)):
  `reflect_cancel`/`reflect_done_tx`/`reflect_failures` +
  `consolidate_cancel`/`consolidate_done_tx`/`consolidate_failures` (6 шт.);
- два канала (`reflect_done`, `consolidate_done`) и две ветки `select!` в
  `run()` ([mod.rs:186-195](../src/app/orchestrator/mod.rs));
- ветка `Quit` вручную перечисляет все токены отмены
  ([mod.rs:341-357](../src/app/orchestrator/mod.rs));
- почти идентичные обработчики `handle_reflect_done`
  ([reflection.rs:290-310](../src/app/orchestrator/reflection.rs)) и
  `handle_consolidate_done`
  ([consolidation.rs:175-192](../src/app/orchestrator/consolidation.rs)):
  снять cancel → погасить индикатор → при `Ok` сброс серии (+у рефлексии
  `SelfModelChanged`) → при `Err` серия и одна ошибка на пороге
  `BACKGROUND_FAILURE_ALERT`.

Этап 6 доводки модели себя отклонил «полевую группировку» как косметику — тогда
задач было две и ripple не рос. Триггер пересмотра, зафиксированный в оценке:
**третья задача семейства** (уже в roadmap). Этот этап — подготовка каркаса;
саму новую задачу **не добавляем** (рефактор не смешивается с фичей).

### Целевое состояние

Ключ реестра — существующий `BackgroundKind` (`app/events.rs:197`, никакого
нового enum'а). Новый модуль `app/orchestrator/background.rs` (~80 строк):

```rust
/// Слот тихой фоновой задачи: токен активного запуска + серия неудач.
/// Серия живёт дольше запуска (переживает завершения) — потому слот, не задача.
#[derive(Default)]
pub(super) struct BgSlot {
    cancel: Option<CancellationToken>, // Some — задача идёт (одна за раз)
    failures: u32,
}

impl Orchestrator {
    pub(super) fn bg_running(&self, kind: BackgroundKind) -> bool { … }
    /// Фиксирует запуск: слот.cancel = Some + BackgroundTask{active:true}.
    pub(super) fn begin_bg(&mut self, kind: BackgroundKind, cancel: CancellationToken) { … }
    /// Общий обработчик исхода (бывшие handle_reflect_done/handle_consolidate_done).
    pub(super) fn handle_bg_done(&mut self, kind: BackgroundKind, result: Result<(), String>) { … }
    pub(super) fn cancel_all_bg(&self) { … } // для ветки Quit
}

fn kind_label(kind: BackgroundKind) -> &'static str {
    // "Авто-рефлексия" / "Авто-консолидация" — тексты ошибок собираются
    // из label БАЙТ-В-БАЙТ с текущими (на них смотрят тесты).
}
```

- Поля `Orchestrator`: 6 → 2 (`bg: HashMap<BackgroundKind, BgSlot>` +
  `bg_done_tx: UnboundedSender<(BackgroundKind, Result<(), String>)>`).
- `run()`: два канала и две ветки `select!` → один канал и одна ветка
  (`orch.handle_bg_done(kind, res)`).
- `SilentLoop` получает поле `kind: BackgroundKind`; `done_tx` шлёт
  `(kind, outcome)` ([tool_loop.rs:49](../src/app/orchestrator/tool_loop.rs)).
- Kind-специфика — **в одном месте** (`handle_bg_done`): `Reflection` при `Ok`
  дополнительно шлёт `SelfModelChanged`; `Consolidation` — нет (текущее
  поведение, см. коммент в consolidation.rs:173).
- Гейты «уже идёт» в `maybe_auto_reflect`/`maybe_auto_consolidate` →
  `self.bg_running(kind)`; спавн-хвосты (установка cancel + индикатор) →
  `self.begin_bg(kind, cancel)`.
- `Quit`: перечисление reflect/consolidate токенов → `cancel_all_bg()`
  (gen/rag/imp остаются как есть).

### Что НЕ делаем (границы семейства)

- **Имперсонация** — не в семействе: свой протокол done-канала
  (`(Uuid, FinishReason)`), стриминг в UI, `imp_gen`. Не трогаем.
- **RAG-индексация** — не в семействе: done-канала нет (прогресс идёт
  `AppEvent::RagProgress` напрямую), только `rag_cancel`. Не трогаем.
- **Авто-название** (`title_tx`) — свой результат с id чата. Не трогаем.
- `consolidate_counts` (каденция по чату) — данные каденции, не жизненный цикл
  задачи; остаётся полем как есть (рефлексия ведёт каденцию ватермарком в
  `Chat` — унифицировать их **нельзя** без изменения поведения).

**Риски:** средние — задет каркас `run()`; смягчение: тексты
ошибок/событий байт-в-байт, поведение проверяют существующие интеграционные
тесты (серия из 3 неудач → одна ошибка; успех сбрасывает и шлёт
`SelfModelChanged`; ватермарк двигается только при спавне). Фикстуры
`bare_orch*` в `tests/mod.rs` конструируют `Orchestrator` литералом — правка
полей в **одном** месте. **Объём:** ~1 сессия.

**DoD:** 808+ тестов зелёные без переименований; поля `reflect_*`/
`consolidate_cancel|_done_tx|_failures` удалены; в `run()` одна bg-ветка;
`rg "BACKGROUND_FAILURE_ALERT" src` — единственный потребитель
`handle_bg_done`.

---

## 5. Этап 3 — настройки: дескрипторы полей (SRP / OCP; поэтапно)

Крупнейший shotgun-surgery узел: добавление одного поля настроек сегодня правит
до **пяти match-сайтов в четырёх файлах** — строка каталога
([catalog.rs](../src/screens/settings/catalog.rs)), `apply_text` (~50 арм,
[apply.rs:415-633](../src/screens/settings/apply.rs)) или
`toggle_field`/`cycle_field` (apply.rs:256-387), `field_description` (~40 арм,
[helpers.rs:21-208](../src/screens/settings/helpers.rs)), валидация
(`field_num_kind`), плюс дефолт для `Del`-сброса и маркера `•` через
`default_fields`.

План god-object'ов (§4) сознательно отложил дескрипторную таблицу с критерием
возврата: «если после сплита catalog.rs+apply.rs продолжат расти быстрее
прочих». Журнал это подтверждает (поток PR в настройки не иссяк) — этап
исполняет отложенную опцию, **но по шагам с отдельной ценностью**, чтобы можно
было остановиться после любого.

Важный прецедент в этом же экране: **семплинг уже устроен дескрипторно** —
`SamplingParam` несёт `label`/`field_name`/`num_kind`/`group`/`description`
([settings/mod.rs:247-427](../src/screens/settings/mod.rs)), значения ходят через
единые `sampling_row`/`apply_sampling_text` (helpers.rs). 28 параметров ×
2 подсекции обслуживаются одной таблицей — паттерн в кодовой базе доказан.

### Шаг 3.1 — описание поля переезжает в `FieldRow` (дёшево, самоценно)

- `FieldRow` получает `description: Option<&'static str>` + builder-метод
  `fn describe(self, d: &'static str) -> FieldRow`.
- Тексты из `field_description` переезжают к местам постройки строк:
  секционные — в catalog.rs, общие для пар Ассистент/Имперсонация — в
  `managed_rows`/`cloud_rows` (helpers.rs), где **одна** строка описания
  автоматически накрывает оба `FieldId` пары (сейчас это дублируется армами
  `XNoMmap | IxNoMmap => …`); семплинг — `sampling_row` подставляет
  `p.description()` (источник не трогаем).
- Потребители: нижняя панель ([render.rs](../src/screens/settings/render.rs)) и
  ловушка поиска ([search.rs](../src/screens/settings/search.rs)) читают
  `row.description` вместо вызова `field_description(id)`; сам 190-строчный
  match удаляется.
- Тесты, зовущие `field_description` напрямую, переводятся на чтение строки
  каталога (имена тестов не меняются).

**Итог шага:** подпись + группа + описание поля живут в **одном** месте.
Риск низкий. Объём ~0.5 сессии.

### Шаг 3.2 — доступ к значению через спецификацию поля (ядро)

Свернуть оставшиеся четыре match-сайта (**toggle/cycle/apply_text/валидация** +
производные `reset_field`/маркер `•`) в **одну таблицу** — честная формулировка:
пять match'ей по `FieldId` → один.

```rust
/// Доступ к значению config-поля. fn-указатели (не замыкания) — 'static, без
/// капчуринга; маршрутизация по режиму (external vs cloud_mut) живёт ВНУТРИ
/// сеттера — ему доступен весь AppConfig.
enum Access {
    Toggle { get: fn(&AppConfig) -> bool,   set: fn(&mut AppConfig, bool) },
    Text   { get: fn(&AppConfig) -> String, set: fn(&mut AppConfig, &str) },
    Choice { get: fn(&AppConfig) -> String, cycle: fn(&mut AppConfig, i32),
             options: fn(&AppConfig) -> (Vec<String>, usize) },
}

struct FieldSpec {
    label: &'static str,
    description: Option<&'static str>,
    num: Option<NumKind>, // валидация редактора
    access: Access,
}

/// ЕДИНСТВЕННЫЙ match по FieldId (таблица). Возвращает None для полей вне
/// охвата (профильные, селекторы подсекций, семплинг — см. границы).
fn field_spec(id: FieldId) -> Option<FieldSpec> { … }
```

Потребители после шага:

- `toggle_field`/`cycle_field`/`apply_text` → «если есть spec — применить через
  `access` и `save_config()`; иначе прежний путь» (профили/семплинг/навигация);
- `field_validation_error` → `spec.num`;
- `reset_field` → `set(cfg, get(&AppConfig::default()))` — текущая связка
  «`default_fields` + повтор навигации» упрощается;
- маркер `•` «изменено» → `get(cfg) != get(&default)`;
- построители каталога → `spec_row(&self.config, id)` (label/описание/значение
  из spec), порядок и **mode-driven видимость остаются императивными** в
  catalog.rs — это осознанная логика показа, не свойство поля;
- попап Choice ([choice.rs](../src/screens/settings/choice.rs)) → `options` из
  spec (шаг 3.3, можно отделить).

**Границы охвата (важно для механичности):**

- **Только config-поля.** Вне охвата: профильные (`PName`/`PSystem`/
  `PGreeting`/`PImpSystem`/`PSelect`/`PTool(idx)` — работают над
  `profiles[profile_idx]`, а не `AppConfig`; их 6 видов, прежний путь
  остаётся), селекторы подсекций (`ModelSub`/`SamplingSub`/`ProfileSub` —
  навигация), `IDicts` при желании — в охвате (сеттер парсит список).
- **Семплинг не трогаем**: `S(p)`/`IS(p)` уже дескрипторны через
  `SamplingParam`; заворачивать таблицу в таблицу — лишняя косвенность.
- Пары Ассистент/Имперсонация (`X*`/`Ix*`) — отдельные спеки с общими
  const-текстами (fn-указатели не капчурят «какой движок», поэтому по спеке на
  FieldId; описания уже общие после 3.1).

**Порядок внедрения** — семействами, каждое — зелёный коммит: (a) Интерфейс +
Инструменты + Память (простые прямые поля — обкатка паттерна); (b) движки
X*/Ix*/E* (маршрутизация external/cloud внутри сеттеров); (c) `reset_field`/`•`
на `get`-сравнение; (d) 3.3 — options Choice-полей.

**Компромисс, фиксируем осознанно:** уходит exhaustive-проверка компилятором
пяти match'ей («забыть поле» теперь = «забыть строку в одной таблице» — ровно
тот же риск, что сегодня «забыть строку каталога»). Взамен — поле целиком
читается в одном месте. Сеть безопасности — существующие ~134 settings-теста
(tests.rs, 1244 строки) + гейт `all_labels_fit_alignment_cap`.

**Критерий отката/остановки:** если после шага (a) таблица читается хуже
прежних match'ей или диффы тестов разрастаются — остановиться на 3.1 (он
самоценен) и зафиксировать решение здесь.

**Риски:** средне-высокие (самый тяжёлый узел UI, 5118 строк); митигируется
пошаговостью и тестами. **Объём:** 3.1 — ~0.5 сессии; 3.2 — 1–2 сессии;
3.3 — ~0.5.

**DoD (полный этап):** `field_description`/`toggle_field`-config-армы/
`cycle_field`-config-армы/config-ветки `apply_text` удалены; по `FieldId`
остаются **два** структурных match'а (таблица `field_spec` + построители
каталога) вместо шести; 808+ тестов зелёные.

---

## 6. Этап 4 — мелкие точечные улучшения (ISP / SRP)

Четыре независимых мини-правки; 4a+4b — один PR, 4c/4d — опционально.

### 4a. View-model статус-бара

`status_bar::render`/`height` несут по **10 аргументов**
(`#[allow(clippy::too_many_arguments)]`,
[status_bar.rs:36-89](../src/widgets/status_bar.rs)); каждый новый индикатор
(последними были `background`-чипы рефлексии/консолидации) расширяет обе
сигнатуры и все вызовы.

```rust
/// Снимок состояния для строки статуса (экран собирает в одном месте).
pub struct StatusModel<'a> {
    pub statuses: &'a ServerStatuses,
    pub generating: bool,
    pub tokens: u64,
    pub context: Option<u64>,
    pub context_exact: bool,
    pub mouse_scroll: bool,
    pub background: Option<&'a str>,
}
pub fn render(frame: &mut Frame, area: Rect, model: &StatusModel, palette: &Palette)
pub fn height(width: usize, model: &StatusModel, palette: &Palette) -> u16
```

`ChatScreen` собирает модель одним приватным хелпером (`status_model()` в
`chat/render.rs`) — новый индикатор = поле + заполнение + отрисовка, без churn
сигнатур. Оба `#[allow(too_many_arguments)]` снимаются. Тесты status_bar
переводятся на литерал модели (механика). Объём: ~0.3 сессии.

### 4b. Каноничные broadcast-хелперы `ActiveScreen` + единый диспетчер intent'ов

Сейчас добавление экрана правит 6–8 разрозненных мест; сводим перечисления
экранов к **одному каноничному месту** — методам на самом `ActiveScreen`
(`app/runtime/mod.rs`, рядом с enum):

- `ActiveScreen::set_palette(&mut self, palette: Palette)` — сворачивает
  поимённый палитровый блок [dispatch.rs:62-77](../src/app/runtime/dispatch.rs)
  (арм `Settings` сохраняет свой `refresh` — у него семантика шире палитры);
- `ActiveScreen::handle_paste(&mut self, chat: &mut ChatScreen, text: &str)` —
  сворачивает роутинг вставки [input.rs:166-175](../src/app/runtime/input.rs);
- локальный `enum AnyIntent { Chat(..), List(..), Settings(..), SelfModel(..) }`
  + одна `dispatch_any(intent, cmd_tx, screen, active) -> bool` — вместо
  «снять 4 Option + 4 почти одинаковых if-блока»
  ([input.rs:180-209](../src/app/runtime/input.rs); текущая форма — обход
  конфликта заимствований, `dispatch_any` решает его одним владением);
- сюда же 4d: вынос side-effect'а буфера обмена из `apply_event`
  ([dispatch.rs:43-58](../src/app/runtime/dispatch.rs)) в приватный хелпер
  `deliver_clipboard(screen, active, clipboard, text)` — `apply_event` перестаёт
  знать про `arboard`.

`match` по экранам не исчезает (это и не цель — enum-диспетчеризация здесь
идиоматична), но новый экран добавляет ветки в **предсказуемых каноничных
местах** одного модуля. Объём: ~0.4 сессии.

### 4c. (Опционально) группировка полей `ChatScreen`

~40 полей ([chat/mod.rs:174-249](../src/screens/chat/mod.rs)) — реализация
разбита по подмодулям, состояние единое. По прецеденту Фазы 3 оркестратора
(`EngineManager`/`SaveQueue`) сгруппировать две когезивные тройки:

- `TokenCounters { tokens, context, context_exact }` (поля `gen_*`);
- `SpellState { checker, dirty, last_edit }` (поля `spell`/`spell_dirty`/
  `last_edit`; `draft_dirty` — не сюда, это черновик).

Попапы **не** группируем и modal-enum **не** вводим — уже отложено планом
god-object'ов §4 как поведенческая правка. Ценность 4c — читаемость; делать,
только если попутно правится `chat/` (не отдельным PR ради него самого).

**Риски этапа 4:** низкие (механика + тесты как сеть). **Объём:** 4a+4b(+4d) —
~0.5–1 сессия одним PR; 4c — ~0.5 попутно.

**DoD:** сигнатуры статус-бара ≤4 параметров без `allow`; в `dispatch.rs`/
`input.rs` нет поимённых перечислений экранов вне методов `ActiveScreen`/
`dispatch_any`; 808+ тестов зелёные.

---

## 7. Что сознательно НЕ делаем (границы направления)

Зафиксировано оценкой 2026-07-08 — это компромиссы, а не долг:

- **`Storage` за трейтом / репозитории-абстракции** — конкретный `Arc<Storage>`
  осознан: тесты быстрые (`:memory:`), второй реализации не предвидится, трейт
  на ~47 методов был бы header-interface. Шов на будущее уже есть (`db/` разбит
  по доменам).
- **`trait Screen`/`trait Widget`** — enum-диспетчеризация экранов идиоматична
  и прозрачна; трейт размыл бы разные снимки/интенты экранов.
- **Слияние `ChatIntent` ↔ `AppCommand`** — дублирование 1:1 есть цена
  FSD-инварианта «screens не знают app», слияние сломало бы слоистость.
- **`ProviderProfile`-таблица** (централизация capability-знания провайдеров) —
  ось уже обслужена `supported_sampling_fields`/`WireDialect`/`cloud()`;
  таблица окупится только при провайдере №4+.
- **Extension bag для семплинга** — отложен ADR 0004, статус не меняем.
- **Вливание петли генерации в `tool_loop`** — решено «нет» (этап 6 доводки):
  стриминг/control-flow/thinking-подписи не окупают общий сток.
- **Modal-enum попапов `ChatScreen`** — отложено планом god-object'ов §4
  (поведенческая правка).
- **Крейт-сплит** — отклонён ADR 0004.

## 8. Порядок, независимость, оценка усилий

Этапы независимы (3.2 — после 3.1). Рекомендуемый порядок — по value/cost:

| Порядок | Этап | Объём | Риск |
|---|------|-------|------|
| 1 | Этап 1 — `ToolContext` | ~0.5 сессии | низкий |
| 2 | Этап 4a+4b(+4d) — статус-бар + runtime | ~0.5–1 | низкий |
| 3 | Этап 2 — фоновые задачи | ~1 | средний |
| 4 | Этап 3.1 — описания в `FieldRow` | ~0.5 | низкий |
| 5 | Этап 3.2 (+3.3) — спецификации полей | 1–2 | средне-высокий |
| — | Этап 4c — группировка `ChatScreen` | ~0.5 | низкий (попутно) |

После этапов 1–2 и 4 стоит сделать паузу и замер: если поток правок в
настройки продолжается — идти в 3.2; если иссяк — остановиться на 3.1.

## 9. Definition of Done (всего направления)

- Новое поле `ToolContext` правит один файл; новая тихая фоновая задача не
  трогает `run()`/`Quit`/обработчик исходов; новое config-поле настроек
  описывается в ≤2 местах (таблица + построитель секции); новый индикатор
  статус-бара не меняет сигнатур.
- Поведение не изменилось: тексты событий/ошибок/логов байт-в-байт; число
  тестов не уменьшилось (808+), имена тест-функций сохранены;
  `#[ignore]`-смоуки не тронуты.
- `cargo fmt` / `cargo clippy --all-targets -- -D warnings` чисты после
  каждого этапа.
- architecture.md (§3 карта модулей, §8 инструменты, §11 конкурентность — по
  затронутому) и CLAUDE.md (журнал) обновлены на каждом этапе; статус этапов
  отмечен в этом документе.
