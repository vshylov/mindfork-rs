# План рефакторинга: разбор god-object'ов (2026-07)

Дизайн-план направления. Статус: **в работе** (этап 1 сделан). По завершении всех
этапов — переезжает в `docs/history/` (как refinements.md и др.).

**Прогресс:** этап 1 (`screens/settings/`) — **сделан** (ветка
`refactor/settings-module-split`); детали — в конце документа.

## 1. Контекст и диагноз

Архитектура в целом здорова: слои FSD соблюдены (`screens`/`widgets` не знают про
`app`), инвариант «единственный владелец `Chat`» цел, оркестратор уже расслоён
(Фазы 1–3: каталог `orchestrator/` + `EngineManager`/`SaveQueue`/`RestartQueue`/
`GenState`). Долг сконцентрирован не в архитектуре, а в **гранулярности файлов**:
несколько модулей выросли в файлы-монолиты с impl-блоками по 800–2000 строк,
куда каждая новая фича дописывает «ещё один метод». Это уже даёт классические
симптомы god-object: любая правка идёт через один файл, ревью-диффы тонут,
навигация по 4–5k строк медленная.

Замер (2026-07-07, `main`, всего строк / из них код до `mod tests`):

| Файл | Всего | Код | Тесты | Симптом |
|---|---:|---:|---:|---|
| `screens/settings.rs` | 4727 | ~3790 | ~940 | один `impl SettingsScreen` ≈ 2040 строк + 1100 строк свободных хелперов; 6+ ответственностей |
| `app/orchestrator/tests.rs` | 3207 | — | 3207 | тест-монолит: ~150 тестов всех фич оркестратора в одном файле |
| `screens/chat.rs` | 2445 | ~1585 | ~860 | `impl ChatScreen` ≈ 1110 строк; экран + 4 попапа + имперсонация + RAG-баннер + орфография |
| `features/tools/notes.rs` | 2114 | ~1230 | ~880 | 9 инструментов + подсистема self-заметок + обзоры консолидации + утилиты |
| `shared/markdown.rs` | 1863 | ~1600 | ~265 | три независимые подсистемы: walker/Writer, таблицы, LaTeX-конвертер |
| `shared/storage/db.rs` | 1537 | ~1075 | ~460 | один `impl Db` ≈ 820 строк, ~45 методов, 4 домена (notes/граф/self_model/RAG) |
| `widgets/input_box.rs` | 1496 | ~940 | ~560 | один целостный виджет, но impl ≈ 770 строк |
| `app/runtime.rs` | 1050 | ~860 | ~190 | петля + батчинг вставки + буфер обмена + 4 dispatch-функции |

Горячесть подтверждается журналом: три последних PR (#117–#119) — все в
`screens/settings.rs`; направления памяти (notes/self-model) постоянно правят
`notes.rs` и `db.rs`.

**Не god-object'ы** (не трогаем): `Orchestrator` (уже расслоён, 16 полей),
`shared/config.rs` (плоские типы данных — много, но без логики),
`entities/self_model.rs` и `features/tools/self_model.rs` (на границе, под
наблюдением), `widgets/message_feed.rs`/`chat_list.rs`, `features/tools/web.rs`
(< ~700 строк кода).

## 2. Метод: проверенный плейбук оркестратора

Разбор `app/orchestrator.rs` (≈1.8k кода + 1.5k тестов → каталог из 15 файлов)
уже проведён в этом репозитории и дал рабочие правила. Все этапы ниже — **тот же
чисто механический перенос**, без изменения типов, полей, каналов и поведения:

1. **Файл → каталог-модуль.** `foo.rs` → `foo/mod.rs` + подфайлы. Публичный путь
   модуля не меняется (`crate::screens::settings::SettingsScreen` остаётся);
   наружу — re-export'ы из `mod.rs`.
2. **Struct и контракт остаются в `mod.rs`.** Методы переезжают отдельными
   `impl X`-блоками в файлы по ответственностям (Rust позволяет много impl-блоков
   одного типа в разных файлах модуля).
3. **Видимость.** Приватные поля struct'а из `mod.rs` видны дочерним модулям
   (правило видимости Rust: потомки видят приватное предка) — менять поля на
   `pub` не нужно. Методы, зовущиеся из другого файла, — `pub(super)` точечно.
4. **Импорты точечные**, без `use super::*` вне тестов (иначе висячие импорты
   под `-D warnings`).
5. **Тесты.** Где тесты по-доменные (notes, db, markdown) — распределяются в
   `mod tests` своих подфайлов (конвенция «тесты рядом с кодом»); где тестируется
   экран целиком через `handle_key`/`render` (settings, chat) — единый
   `tests.rs`-подмодуль (прецедент оркестратора). Общие фикстуры — `#[cfg(test)]
   mod testkit` в `mod.rs`. Имена тест-функций не меняются (на них ссылается
   журнал CLAUDE.md).
6. **Гейты на каждый PR**: `cargo fmt`, `cargo clippy --all-targets -- -D
   warnings`, `cargo test` — зелёные; число тестов не уменьшилось (сейчас 794).
7. **Один этап — один PR**, сплит не смешивается с функциональными правками.
   Коммит переноса отдельно от мелких правок видимости — дифф читается как
   перемещение.
8. **Доки**: обновить карту модулей architecture.md §3 + журнальную запись
   CLAUDE.md (конвенция каждого PR).

Ориентиры-пороги («жёлтая зона», вход в план при нарушении + ожидаемом churn):
**~1000 строк кода** на файл, **~500 строк** на impl-блок.

## 3. Этапы

Каждый этап независим; порядок — по (размер × горячесть), но его можно менять.
Оценка усилий: 0.5–1 сессия на этап, settings — 1–2.

### Этап 1 — `screens/settings.rs` → `screens/settings/` (приоритет 1)

Самый большой файл репозитория и самый горячий (редизайн в 6 этапов + три
последних PR). Ответственности уже хорошо расслаиваются по границам существующих
методов:

```
screens/settings/
├─ mod.rs           SettingsIntent, Section/Subsection/ModelTab/Focus, struct
│                   SettingsScreen, new/refresh/set_server_statuses, верхний
│                   handle_key-диспетчер, move_section, palette()        (~350)
├─ catalog.rs       «модель формы»: FieldId, FieldKind/FieldRow, NumKind,
│                   SamplingParam + SAMPLING_PARAMS, каталоги секций
│                   (model_fields_for/sampling_fields_for/tool_fields/
│                   memory_fields/interface_fields/profile_fields_for),
│                   row/grouped/text_row/num_row/sampling_row, managed_rows/
│                   cloud_rows + ManagedFieldIds, label-функции, tool_catalog,
│                   is_subsection/is_profile_field                       (~950)
├─ descriptions.rs  field_description (≈190 строк), gate_hint            (~230)
├─ apply.rs         мутации: toggle_field/toggle_profile_tool/cycle_field/
│                   cycle_sampling_field/apply_text/apply_profile_text/
│                   apply_sampling_text/save_config/save_profile,
│                   reset_field/default_fields, валидация (field_num_kind/
│                   field_validation_error), cycle_* (mode/imp/theme/opt_bool/
│                   reasoning), gate_disabled/sampling_cloud_provider,
│                   parse_opt*/parse_list/parse_breakers/decode/encode   (~750)
├─ editor.rs        Editor, handle_editor_key/handle_paste/field_seed,
│                   multiline_popup_height                               (~150)
├─ choice.rs        ChoiceState, choice_menu/open_choice/apply_choice/
│                   handle_choice_key, SERVER_MODES/IMP_MODES/THEMES,
│                   index_menu/flash_menu/spec_menu/sampling_choice_menu (~200)
├─ search.rs        SearchHit/SearchState, build_search_index/collect_hits/
│                   open_search/search_filter/jump_to_selected/
│                   handle_search_key                                    (~250)
├─ render.rs        render/render_menu/render_fields/render_choice/
│                   render_search, tab_strip_line/header_line/
│                   render_field_line/server_status_chip/value_text,
│                   section_label_col/label_width/truncate_to_width/
│                   span_width/centered_rect*                            (~800)
└─ tests.rs         все тесты экрана (через handle_key/render)           (~940)
```

Внешняя поверхность минимальна: снаружи модуль импортирует только
`app/runtime.rs` (`SettingsIntent`, `SettingsScreen`) — re-export в `mod.rs`.

Нюанс: `catalog.rs`/`apply.rs`/`render.rs` связаны через `FieldId` — это
нормально (id — общий словарь формы). Резать `FieldId` по секциям **не** нужно.

### Этап 2 — `screens/chat.rs` → `screens/chat/` (приоритет 1)

`ChatScreen` — второй по горячести: каждая UI-фича проходит через него.
Состояния попапов уже вынесены в структуры (`SuggestPopup`,
`ImpersonationState`, `RagBanner`) — осталось разложить методы:

```
screens/chat/
├─ mod.rs            ChatIntent, struct ChatScreen, new + сеттеры/геттеры
│                    снимков (set_settings/set_server_status/set_chat_list/
│                    set_profile_list/spellchecker/background-флаги)     (~330)
├─ feed.rs           проекция AppEvent в ленту: activate_chat/rename_chat/
│                    push_user_message/begin_generation/push_chunk/
│                    push_thoughts/push_tool_call/continue_assistant/
│                    rewrite_assistant/set_token_usage/finish_generation/
│                    push_error/push_note/restore_input/take_feed_scrolled/
│                    feed_msg_has_vs16                                   (~350)
├─ impersonation.rs  ImpersonationState + begin/push/finish/is_impersonating (~120)
├─ rag.rs            RagBanner + set_rag_progress/is_rag_active/
│                    format_rag_sources                                  (~130)
├─ input.rs          handle_key/handle_paste/handle_mouse,
│                    mark_input_changed/take_dirty_draft/input_is_command/
│                    maybe_recheck_spelling, request_new_chat/
│                    handle_profile_overlay_key, trigger_destructive     (~420)
├─ popups.rs         SuggestItem/SuggestPopup, ConfirmAction, HELP_KEYS,
│                    open_suggestions/handle_suggest_key/apply_suggestion,
│                    handle_confirm_key, handle_emoji_key,
│                    render_help/render_suggest/render_confirm           (~380)
├─ render.rs         render, model_meta, visual_line_count, centered_rect (~220)
└─ tests.rs          все тесты экрана                                    (~860)
```

Нюанс: `handle_key` — маршрутизатор модальностей (справка → confirm → подсказки
→ эмодзи → оверлей профиля → обычный ввод). При сплите **порядок веток не
менять** — он и есть контракт модальности.

### Этап 3 — `app/orchestrator/tests.rs` → `app/orchestrator/tests/` (дёшево, риск ~0)

3.2k строк, ~150 тестов всех фич в одном файле — замедляет каждую правку
оркестратора. Разложить зеркально сорс-модулям:

```
app/orchestrator/tests/
├─ mod.rs            общие фикстуры (testkit: spawn_orch, окружение,
│                    MockSupervisor-обвязка) + mod-объявления
├─ generation.rs · chats.rs · profiles.rs · settings.rs · title.rs
├─ impersonation.rs · rag.rs · reflection.rs · consolidation.rs · self_model.rs
└─ live.rs           все #[ignore] e2e_live-смоуки (их запускают пачкой)
```

Путь модуля не меняется (`mod tests;` в `orchestrator/mod.rs` уже есть).
Чистый cut-paste; единственная работа — растащить общие хелперы в `mod.rs`.

### Этап 4 — `features/tools/notes.rs` → `features/tools/notes/` (приоритет 2)

Самый горячий из features (направления памяти продолжатся: vec0-задел,
нуджи цитирования). Сейчас в одном файле: 9 инструментов, подсистема
self-заметок, обзоры консолидации, чистые утилиты.

```
features/tools/notes/
├─ mod.rs            ID-константы, пороги (CONSOLIDATE_SIMILARITY и др.),
│                    SELF_NOTE_TAG, is_self_note, parse_id/parse_tags/clip,
│                    pub-use всех инструментов и pub(crate)-хелперов
├─ save.rs           NoteSave, create_note, ворота похожести, ensure_note_vectors
├─ recall.rs         NoteRecall, list_user_notes, семантический путь,
│                    related_block, cited_sources_block, format_notes
├─ edit.rs           NoteRevise, NoteSupersede, NoteMerge
├─ graph.rs          NoteLink, NoteNeighbors
├─ cite.rs           NoteCiteSource
├─ overview.rs       ConsolidateNotes, build_consolidation_overview,
│                    build_self_consolidation_overview
├─ self_notes.rs     self_notes_recent/self_notes_relevant/self_related_block/
│                    migrate_self_narrative, cosine
└─ (тесты — в mod tests соответствующих подфайлов; общие фикстуры-ctx —
    #[cfg(test)] testkit в mod.rs)
```

Внешняя поверхность широкая (оркестратор, `self_model.rs`, `rag.rs`,
`tools/mod.rs`, `meta.rs` зовут `notes::*` — константы, `is_self_note`,
`create_note`, `self_notes_*`, обзоры) — всё сохраняется re-export'ами из
`mod.rs`, внешние `use` не трогаются.

### Этап 5 — `shared/storage/db.rs` → `shared/storage/db/` (приоритет 2)

Один `impl Db` на ~45 методов и 4 домена. Разрез по доменам данных:

```
shared/storage/db/
├─ mod.rs            struct Db, open/open_in_memory/from_conn,
│                    register_sqlite_vec, migrate() (схема), ensure_vec_table/
│                    vec_dim, общие хелперы (row_to_note/parse_uuid/parse_dt/
│                    cosine/norm_path)
├─ notes.rs          note_insert/list/get/update/delete/is_active +
│                    note_vector_upsert/note_search_semantic/
│                    notes_missing_vectors/notes_with_vectors
├─ graph.rs          note_link_insert/count/neighbors/links_all/links_retarget,
│                    note_supersede_mark, note_cite_source_insert/
│                    note_cited_sources/notes_citing_source
├─ self_model.rs     self_model_get/upsert/update + *_conn-хелперы
└─ rag.rs            rag_insert/search/count/delete_*/source_*/stored_sources/
│                    list_sources/dimension/other_profiles_have_docs/
│                    reset_vectors + delete_matching/delete_sources_matching +
│                    rag_source_exists
```

Схему (`migrate()`) оставить одним куском в `mod.rs` — по ней видно всю БД
сразу. Тесты распределить по доменным подфайлам.

### Этап 6 — `shared/markdown.rs` → `shared/markdown/` (приоритет 3)

Холодный сейчас, но швы идеальные — три независимые подсистемы:

```
shared/markdown/
├─ mod.rs      render/render_with + стили (heading/code/link/blockquote)
├─ writer.rs   Writer (walker по событиям pulldown-cmark), heading_number
├─ code.rs     syntect: resolve_syntax/canonical_lang/code_theme/
│              build_code_theme/gray/scope_item/to_syn
├─ table.rs    TableBuilder, render_table/fit_columns/border_line/render_row/
│              pad_cell/clip_line, MIN_COL/MAX_MIN
└─ latex.rs    normalize_delimiters + latex_to_unicode + таблицы команд
               (~570 строк — самый большой изолируемый кусок)
```

### Этап 7 — `app/runtime.rs` → `app/runtime/` (приоритет 3)

Ещё не критично (~860 кода), но швы чёткие и файл растёт с каждым экраном:

```
app/runtime/
├─ mod.rs        run/run_loop, ActiveScreen, SpellLoader, TICK
├─ input.rs      PASTE_BURST/PASTE_GAP, collect_press/paste_char, Chunk/
│                chunk_batch/process_input_batch, reconcile_paste/
│                paste_projection_matches (вся Windows-вставка)
├─ clipboard.rs  read_clipboard_text/write_clipboard (arboard-слот)
└─ dispatch.rs   apply_event + dispatch/dispatch_chat_list/dispatch_settings/
                 dispatch_self_model (Intent → AppCommand)
```

### Этап 8 (опционально, по мере роста)

- **`widgets/input_box.rs`** — целостный виджет; резать только если продолжит
  расти: `input_box/{mod,edit,nav,render}.rs` (правки текста / визуальная
  навигация / отрисовка).
- **`shared/config.rs`** — плоские типы; при росте — `config/{engine,tools,
  memory,interface}.rs` с re-export'ами.
- **Наблюдательный список** (пересмотреть при следующем замере):
  `entities/self_model.rs` (~720 кода), `features/tools/self_model.rs` (~655),
  `features/tools/web.rs` (~620), `app/orchestrator/generation.rs` (~700),
  `widgets/message_feed.rs`, `widgets/chat_list.rs`, `screens/self_model.rs`.

## 4. Что сознательно НЕ делаем (и почему)

- **Крейт-сплит** — отклонён ещё в ADR 0004 для `shared/api`; та же логика для
  остального: один бинарный крейт, границы — модулями.
- **Декларативная таблица дескрипторов полей настроек** (свернуть пять
  match-сайтов по `FieldId` — каталог/apply/toggle/описания/валидация — в одну
  таблицу с геттерами/сеттерами-замыканиями). Причина роста settings.rs — O(полей)
  в пяти местах, и таблица бы это вылечила, **но**: это переписывание с реальным
  риском регрессий, а текущий match-подход прост и проверяется компилятором на
  полноту. Вернуться, только если после сплита `catalog.rs`+`apply.rs` продолжат
  расти быстрее прочих.
- **Modal-enum для попапов `ChatScreen`** (`enum Modal { Help | Confirm | … }`
  вместо независимых `Option`-полей) — сделал бы инвариант «одна модалка за раз»
  верным по построению, но это поведенческая правка, не механический перенос.
  Рассмотреть отдельным PR **после** этапа 2, если при сплите вскроются
  двусмысленности приоритета модальностей.
- **Вливание основной петли генерации в `tool_loop`** — уже решено «нет»
  (этап 6 доводки модели себя): стриминг/control-flow/thinking-подписи не
  окупают общий сток.
- **Дальнейшее дробление `Orchestrator`** — уже расслоён (Фазы 1–3), 16 полей,
  инвариант владения `Chat` цел. Не трогаем.

## 5. Definition of Done (всего направления)

- Ни одного файла > ~1600 строк всего; в затронутых модулях ≤ ~1000 строк кода
  на файл и ≤ ~500 строк на impl-блок.
- Публичные пути модулей не изменились (внешние `use` без правок, кроме
  свободных функций, ставших методами-соседями, — таких быть не должно).
- 794+ юнит-тестов зелёные после каждого этапа, число не уменьшилось;
  `#[ignore]`-смоуки не тронуты.
- `cargo fmt` / `cargo clippy --all-targets -- -D warnings` чисты.
- architecture.md §3 (карта модулей) и CLAUDE.md (журнал) обновлены на каждом
  этапе.

## 6. Выполнено

### Этап 1 — `screens/settings.rs` → `screens/settings/` (сделано)

Монолит 4966 строк разбит на 8 файлов чисто механическим переносом (byte-exact
слайсы по диапазонам строк — нулевой риск транскрипции; поведение/типы/поля/
контракт `SettingsIntent` не менялись). Итог (строк): `mod.rs` 650 (типы форм +
enum'ы секций + `struct SettingsScreen` + декларации подмодулей), `catalog.rs`
565 (конструктор + построители полей секций/подсекций + гейты), `apply.rs` 675
(обработка клавиш + редактор + тумблеры/циклы + сохранение), `render.rs` 530
(отрисовка), `helpers.rs` 1130 (свободные функции — построители строк, описания,
парсеры, циклы enum-значений), `choice.rs` 150, `search.rs` 155, `tests.rs` 1175.
Прежний `impl SettingsScreen` (~2040 строк) разложен на 4 impl-блока по файлам
(≤ ~660 строк каждый).

Ключевые правила видимости (закрепляют плейбук §2 для UI-экранов):
- Подмодули берут элементы `mod.rs` через `use super::*;` — glob подтягивает и
  приватные `use`-импорты родителя (ratatui/uuid/crate::…), поэтому внешние
  импорты в подмодулях не дублируются и в `mod.rs` не «висят» (все «использованы»
  транзитивно → ноль warning'ов).
- Свободные функции `helpers.rs` помечены `pub(super)` (иначе не видны
  сиблингам); потребители делают `use super::helpers::*;`.
- Приватные методы `impl SettingsScreen`, вызываемые из другого файла, помечены
  `pub(super)` (метод-приватность в Rust — по модулю определения).

**Отклонения от проекта §3:** `descriptions.rs` и `editor.rs` не выделялись —
`field_description`/`gate_hint` остались в `helpers.rs`, а редактор поля
(`handle_editor_key`/`field_seed`) — в `apply.rs` (тесно связан с обработкой
клавиш). `helpers.rs` оставлен единым модулем свободных функций (1130 строк, но
это независимые чистые функции, не запутанный impl-блок) — дальнейшее дробление
по ответственностям (catalog/apply/render/descriptions) отложено как
низкоприоритетное. Гейты зелёные: **805 тестов** (0 упавших, 26 `#[ignore]`),
clippy `-D warnings` чист, `cargo fmt --check` чист.
