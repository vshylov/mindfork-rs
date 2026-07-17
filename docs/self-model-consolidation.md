# План: консолидация «модели себя» (авто-«сон» + семантика summary + старение интересов)

> Статус: **A1, A2, A3-лёгкий — сделано; направление завершено** (A3-тяжёлый —
> задел). Ветки `feat/self-model-auto-consolidate`, `feat/self-model-summary-semantics`,
> `feat/self-model-interests-aging`; журнал — CLAUDE.md пост-M9; порог A2
> `SUMMARY_OBS_SIMILARITY=0.62` откалиброван на живом bge-m3; A3-лёгкий — нудж в
> `selfmodel.policy_core`. Направление продолжает
> [summary-as-snapshot](history/summary-as-snapshot.md) («Вне объёма») и
> [refinements](history/refinements.md); закрывает три задела roadmap раздела
> «Память, модель себя, знания». При финализации документ переезжает в
> `docs/history/`.

## Нерв задачи

У всех органов «модели себя» есть механика анти-раздувания **кроме периодической**:
наблюдения (`@self`-заметки) консолидируются только через авто-рефлексию и
интерактивный `reflect`; у `summary` есть ворота размера (`summary_fill_hint`), но
никто их не отрабатывает вне хода; `current_interests` — «текущие», но ничто их не
вымывает. Направление добавляет три вещи:

1. **A1 — фоновый «сон» модели себя по таймеру** (зеркало
   `notes.auto_consolidate_every`): периодически слить дубли наблюдений, заместить
   устаревшее со «шрамом», **сжать раздутый `summary`**, связать противоречия — вне
   зависимости от свежих ходов.
2. **A2 — семантическое сравнение абзацев `summary` с наблюдениями** в обзоре
   self-консолидации: «абзац описания похож на наблюдение X → вынеси/сшей».
3. **A3 — старение `current_interests`**: убирать давно не подтверждаемые интересы.

## Что уже готово (опорные точки в коде)

- **Каркас фоновых задач построен под это.** SOLID-этап 2
  (`app/orchestrator/background.rs` — реестр слотов `BgSlot` по `BackgroundKind`;
  `app/orchestrator/tool_loop.rs::spawn_silent_loop`) прямо готовил «задачу №3
  семейства (авто-консолидация «модели себя» по таймеру)»: её добавление «больше не
  трогает `run()`/`Quit`».
- **Сиблинги для копирования:** `app/orchestrator/consolidation.rs` (заметки) и
  `app/orchestrator/reflection.rs` (модель себя) — один в один жизненный цикл
  (гейты → счётчик каденции → `build_*_overview` → `spawn_silent_loop` → `begin_bg`).
- **Данные к «сну» уже собираются:** `features/tools/notes/overview.rs::
  build_self_consolidation_overview` (похожие пары наблюдений / `contradicts` /
  «без связей»). Сигнал раздутого summary — `entities/self_model.rs::
  SelfModel::summary_fill_hint`.
- **Обратная связь наблюдаемости:** `background.rs::handle_bg_done` уже при успехе
  **рефлексии** эмитит `SelfModelChanged` (открытый `F3` обновляется) и ведёт серию
  неудач (`BACKGROUND_FAILURE_ALERT`).

## Принципы (в духе проекта)

- **DB-only, без `ChatEffect`** — все инструменты модели себя/заметок пишут напрямую
  через `ctx.storage`; инвариант «единственный владелец `Chat`» не затрагивается.
- **Opt-in, по умолчанию выкл** — как `auto_reflect_every`/`auto_consolidate_every`
  (фоновые вызовы стоят токенов).
- **Мягкая деградация** — недоступен эмбеддер / сервер не готов → тихо пропускаем,
  счётчик каденции не теряется (как в `reflection.rs`/`consolidation.rs`).
- **Интеграция, а не потеря** — устаревшее сворачивается в «шрам»-наблюдение, не
  удаляется молча (как `fold_closed_goals`, `note_supersede`).
- **Гейты, а не жёсткие правила** — решение всегда за моделью; мы даём данные и нудж.
- **Без миграций схемы** — новые поля конфига через `#[serde(default)]`.

---

## Этап A1 — фоновая авто-консолидация «модели себя» ⭐ первым

### Шаг A1.1 — вариант `BackgroundKind` и статус-бар

- `app/events.rs` — `enum BackgroundKind` += `SelfConsolidation`.
- `app/runtime/dispatch.rs` — маппинг `SelfConsolidation → screen.set_self_consolidating(on)`.
- `screens/chat/*` — поле-флаг + сеттер `set_self_consolidating` (зеркало
  `set_reflecting`/`set_consolidating`).
- `widgets/status_bar.rs` — тихий muted-чип (например `✻ сон себя`; глиф шириной 1
  колонка — компат-безопасно, как прочие фоновые чипы).

### Шаг A1.2 — обработчик `maybe_auto_self_consolidate`

Новый модуль `app/orchestrator/self_consolidation.rs` — калька с `consolidation.rs`:

- **Конфиг-гейт:** `self_model.auto_consolidate_every == 0` → выход.
- **Каденция:** счётчик `self_consolidate_counts: HashMap<chat_id, u32>` на
  `Orchestrator` (как `consolidate_counts`); `tool_loop::due(count, every)`; сброс —
  только при фактическом спавне (пропуск по гейту не теряет цикл).
- **Гейты:** профиль включил инструменты модели себя (`GET_SELF_MODEL_ID`); есть что
  консолидировать — наблюдений ≥ 2 **или** `summary` сверх `summary_target_chars`
  (через `summary_fill_hint`); `bg_running(SelfConsolidation)` == false; сервер
  `Ready` (`engines.backend_if_ready`).
- **Набор инструментов** (пересечение с профилем): `note_revise`/`note_supersede`/
  `note_merge`/`note_link`/`note_neighbors` (над `@self`-наблюдениями) +
  `update_self_model`/`update_user_model` (сжать summary / привести собеседника) +
  `get_self_model` (полные id наблюдений). `note_recall` **не даём** — он скрывает
  `@self`; полные id модель берёт из `get_self_model` (как рефлексия).
- **Дайджест:** `build_self_consolidation_overview(...)` + строка `summary_fill_hint`
  (если summary раздут). Пусто/нет сигнала → выход без сброса счётчика.
- **Запрос:** системное сообщение `prompt.self_consolidate.system` (на базе
  `self_model::policy_core(loc)` — единый источник правил, как
  `reflect_system_message`), `max_tokens ≈ 2048`, `temperature ≈ 0.3`,
  `spawn_silent_loop(SilentLoop { kind: SelfConsolidation, ... })` + `begin_bg`.

### Шаг A1.3 — триггер и наблюдаемость

- `app/orchestrator/mod.rs::handle_done` — вызвать `maybe_auto_self_consolidate`
  (рядом с `maybe_auto_reflect`/`maybe_auto_consolidate`); поле счётчика.
- `background.rs::handle_bg_done` — расширить спец-случай: при успехе
  `SelfConsolidation` тоже эмитить `SelfModelChanged` (меняет модель себя, как
  рефлексия). Ключ ошибки `ui.err.bg_self_consolidation`.

### Шаг A1.4 — конфиг и UI

- `shared/config.rs` — `SelfModelSettings.auto_consolidate_every: usize`
  (`#[serde(default)]`, дефолт 0) + константа `DEFAULT_*`.
- `screens/settings/*` — поле в секции «Память», группа «Модель себя» (рядом с
  «авто-рефлексия»), описание-подсказка + i18n-ключ.
- `locales/{ru,en}.json` — `prompt.self_consolidate.system`, метка чипа, ошибка,
  подпись поля/описание. i18n-гейты (key/placeholder parity, `no_cyrillic`,
  key-в-коде) покрывают автоматически.

### Тесты A1

- Юнит: `due`-каденция уже покрыта в `tool_loop`; гейт по summary_fill_hint.
- Интеграционные (`orchestrator/tests/`): `handle_done` спавнит при пороге; гейты
  держат при выключенной фиче / < 2 наблюдений и не раздутом summary / неготовом
  сервере; счётчик не сброшен при пропуске; успех → `SelfModelChanged`.
- Runtime: `SelfConsolidation` → чип + не открывает `F3`, а перезапрашивает снимок.
- **Живой** `#[ignore]` (Gemma 4 31B + bge-m3): при `auto_consolidate_every=1` два
  похожих наблюдения сводятся `note_merge`; раздутый summary (> target) сжимается
  `update_self_model`. Критерий go/no-go как у прежних зондов.

### Развилка A1 (для пользователя)

- **Отдельный `self_model.auto_consolidate_every`** (рекомендуется) — модель себя и
  заметки уже разведены по гейтам/данным; свой тумблер точнее.
- **Общий тумблер «сна памяти»** — переиспользовать `notes.auto_consolidate_every`.
  Проще UI, но связывает два разных органа. Не рекомендуется.

---

## Этап A2 — семантика «summary ↔ наблюдения» в обзоре

**Что.** `build_self_consolidation_overview`
(`features/tools/notes/overview.rs`) сейчас сравнивает **наблюдения между собой**
(попарный косинус) + `contradicts` + «без связей». Добавить раздел: сегментировать
`summary` на абзацы, **эмбеддить их на лету** (у summary нет хранимых векторов — как
ворота черт `self_model.rs::near_duplicate_traits`), сравнить с векторами
`@self`-наблюдений (`notes_with_vectors`, фильтр `is_self_note`) и поднять пары
«абзац summary ≈ наблюдение X» с подсказкой «вынеси/сшей».

**Нюанс (важно).** Функция сейчас **чистое чтение БД** (эмбеддер не нужен). Новый
раздел требует `Embedder`, поэтому он **опционален**: нет эмбеддера/нестыковка →
раздел пропускается (мягкая деградация, как реранкинг RAG/web). Значит сигнатуру и
вызовы (`reflect`, `reflection.rs`, а также `self_consolidation.rs` из A1) надо
расширить эмбеддером; чистая часть остаётся, семантический раздел — надстройка.

**Файлы.** `overview.rs` (сегментация абзацев по пустым строкам + раздел),
`features/tools/self_model.rs` / `app/orchestrator/reflection.rs` /
`app/orchestrator/self_consolidation.rs` (проброс эмбеддера), `locales`.

**Порог.** Откалибровать по живому bge-m3 (как `TRAIT_SIMILARITY=0.72`), не угадывать.

**Тесты.** На `MockEmbedder`: абзац, похожий на наблюдение, поднимается; непохожий —
молчит; без эмбеддера — раздел отсутствует, паники нет.

---

## Этап A3 — старение `current_interests`

`current_interests` — плоский `Vec<String>` (`entities/self_model.rs::UserModel`),
ничто их не вымывает. Две развилки:

- **A3-лёгкий (рекомендуется).** Без изменения схемы: нудж «проверь, не устарели ли
  интересы; давно не подтверждаемые — убери через `remove_interests`» в системное
  сообщение A1/рефлексии и «протокол ведения» (`policy_core`). Согласуется с
  философией проекта (интеграция силами модели) и прямой рекомендацией
  [summary-as-snapshot](history/summary-as-snapshot.md) («лечится теми же
  воротами/рефлексией»). Стоимость — тексты бандлов. Сделать в рамках A1.
- **A3-тяжёлый (задел).** `current_interests: Vec<Interest { text, updated_at }>` —
  настоящее старение по времени. Ломает плоский `Vec<String>` и трогает ~десяток
  мест: `render_user_model`, `add_interests`/`remove_interests`, `apply_edit`
  (`SetInterests`), `UserModel::render_for_impersonation`, ворота, serde-миграция
  (`#[serde(default)]` + шаг). Оправдан, только если A3-лёгкий на живой модели
  окажется недостаточным.

---

## Вне объёма (задел)

- **A3-тяжёлый** (структура интересов с временными метками) — если лёгкого мало.
- **Старение перенести в vec0** — не связано; см. [notes-vec0](notes-vec0.md).
- **Consolidation модели собеседника отдельным органом** — сейчас `user_model`
  ведётся merge-семантикой + шрамами; отдельного «сна» ему не заводим.

## Порядок и DoD

1. **A1** (авто-«сон» + A3-лёгкий нудж в тех же промптах) — один PR, флагман.
2. **A2** (семантика summary↔наблюдения) — следующий PR, поверх A1/рефлексии.

DoD этапа: `cargo fmt`/`clippy -D warnings`/`test` зелёные; живой смоук A1 — **go**;
запись в CLAUDE.md (журнал пост-M9) + CHANGELOG (рубрика по эффекту); поля настроек и
i18n-ключи на месте.
