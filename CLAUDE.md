# CLAUDE.md — ориентир по проекту mindfork-rs

Консольное (TUI) приложение ИИ-чата на Rust. Локальные модели **Gemma 3/4** и
**Qwen 3.5/3.6** через **llama.cpp `llama-server`** (OpenAI-совместимый сервер; в
external подойдёт любой такой — vLLM/LM Studio/Ollama). UI на **ratatui**.
Платформы: Windows + Linux. Архитектура — **Feature-Sliced Design (FSD)**.

> **Движок:** изначально проектировался под `xinfer`, но он оказался слишком сырым
> (бессвязный вывод на Gemma 4, плохо собирается под Windows) — переведён на
> llama.cpp. Клиент generic-OpenAI (`OpenAiClient`), managed-лаунчер — `llama-server`
> (`LlamaSupervisor`). См. [docs/install.md §3](docs/install.md).

## Главные документы (читать перед работой)
- **[spec.md](spec.md)** — полная инженерная спецификация (доменная модель, движок,
  инструменты, профили, UI, тестирование). Источник истины по «что» и «почему».
- **[plan.md](plan.md)** — пошаговый план по этапам M0–M9 (задачи, тесты, DoD).
- **[docs/install.md](docs/install.md)** — установка/запуск (llama.cpp `llama-server`
  managed/external, env, словари, импорт). **Актуально по движку.**
- **[docs/xinfer-contract.md](docs/xinfer-contract.md)** — описание OpenAI-совместимого
  протокола (эндпоинты, поля запроса/стрима, EOS, эмбеддинги). Исходно сверялся с
  xinfer; llama.cpp говорит на том же протоколе.

## Ключевые архитектурные решения
- **Движок = локальный OpenAI-совместимый HTTP-сервер** (managed `llama-server` или
  external любой), приложение — HTTP-клиент (`OpenAiClient`). Встраивание (rlib)
  сознательно НЕ используется (тянет candle/CUDA).
- **Agentic-loop клиентский** (в оркестраторе). Инструменты возвращают результат +
  эффекты; оркестратор (единственный владелец `Chat`) их применяет — без локов.
- **Семплинг**: temperature, top_k, top_p, frequency/presence_penalty, max_tokens,
  thinking, reasoning_effort (поля `SamplingConfig`). Неподдержанное сервером
  (min_p/seed/mirostat/…) просто игнорируется.
- **EOS**: остановка по token-id на сервере; поле `stop` НЕ отправляем (анти-самообрыв).
- **«Мысли» (CoT)**: `delta.reasoning_content` (`llama-server --reasoning-format`);
  fallback — парсинг `<think>` из content.
- **Хранение**: JSON (конфиг/профили/чаты, атомарная запись + .bak) + SQLite
  (заметки/RAG, sqlite-vec, изоляция по `profile_id` через partition key).
- **Мягкое удаление** везде (`is_hidden`, каскад профиль→чаты).
- **Однонаправленный поток UI↔оркестратор**: команды `AppCommand` вверх, события
  `AppEvent` вниз; `generation_id` отбрасывает устаревшие чанки; автомат
  `Idle/Generating/Cancelling`.

## Структура (FSD, зависимости строго вниз)
`app → screens → widgets → features → entities → shared`. Бинарный крейт.
- `src/app/` — оркестратор, события, TUI-петля, мост tokio↔UI.
- `src/entities/` — доменные типы (`chat`, `message`, `profile`, `note`, `rag`, `sampling`).
- `src/shared/api/` — движок за трейтом `EngineBackend` (OpenAI-клиент `OpenAiClient`, лаунчер `llama-server`, парсер мыслей, mock).
- `src/shared/storage/` — `json` + `db` (SQLite) + фасад `Storage`.
- `src/shared/` — `config`, `paths` (портативные, рядом с бинарником), `logging`, `instance`, `error`.
- `src/{screens,widgets,features}/` — пока заглушки (наполняются с M3).

## Конвенции
- Rust edition 2024. **Перед каждым коммитом**: `cargo fmt`,
  `cargo clippy --all-targets -- -D warnings`, `cargo test` — всё должно быть зелёным.
- Ошибки: `anyhow` в прикладных слоях, `thiserror` в библиотечных модулях `shared`.
- Логи — только в файл (`logs/`, stdout занят TUI). Никаких `println!`.
- Тесты пишутся рядом с кодом по ходу (`#[cfg(test)]`); реальный сервер/модель — `#[ignore]`.
- Комментарии и доки — на русском (как в существующем коде); ссылки на разделы spec.
- Коммиты заканчивать строкой: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

## Команды
```
cargo build
cargo test                         # юнит-тесты (без сервера)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo run                          # TUI (нужен НАСТОЯЩИЙ терминал — см. ниже)
```
Запуск против реального сервера (смоук, llama.cpp):
```
llama-server -m gemma-4-E4B-it.gguf --host 0.0.0.0 --port 8000 -ngl 99 -c 8192 --jinja  # терм. 1
$env:MINDFORK_ENGINE_URL="http://127.0.0.1:8000/v1"  # терминал 2 (PowerShell)
cargo run
cargo test ignored_smoke -- --ignored --test-threads=1   # смоук-тесты
```
Env для выбора бэкенда: `MINDFORK_ENGINE_URL` (external, любой OpenAI-сервер) ИЛИ
`MINDFORK_LLAMA_BIN` (+ `MINDFORK_MODEL` GGUF, `MINDFORK_NGL`, `MINDFORK_CTX`,
`MINDFORK_PORT`) для managed `llama-server`.

## Статус (на 2026-06-15)
Сделан весь план **M0–M9** (в `main`). **225 тестов зелёные, 6 `#[ignore]`.**
Чат-цикл, профили с изоляцией, инструменты с клиентским agentic-loop, саб-агент,
web-поиск и Python под выключателями, экран настроек со всеми секциями и
перезапуском managed-сервера при смене модели, импорт из LameLLaMA (.NET), оверлей
помощи, релизный профиль, **темы (auto/dark/light)**, **спелл-чек на лету по
настройкам**. **Gemma 4 проверена** на живом `llama-server` (стриминг, EOS-анти-
самообрыв, tool-calling, «мысли» — `#[ignore]`-смоуки зелёные).

> **Движок инференса:** `xinfer` оказался сырым по Gemma 4 (бессвязный вывод,
> плохо собирается под Windows). Рабочий бэкенд — **llama.cpp `llama-server`**
> (external, OpenAI-протокол, `--jinja`). См. [docs/install.md](docs/install.md) §3.
- **M0** — каркас FSD, TUI-петля с восстановлением терминала, single-instance, логирование.
- **M1** — `shared/api` (xinfer-клиент со стримингом/отменой, супервайзер, парсер
  мыслей, mock), оркестратор (автомат + generation_id), мост tokio↔TUI, минимальный
  чат-UI. История пока в памяти.
- **M2** — `entities` (все доменные типы), `shared/config`, `shared/storage`
  (JSON атомарно + SQLite/sqlite-vec, изоляция по профилю, мягкое удаление + каскад).
  Storage пока НЕ подключён к оркестратору/UI.

### M3 (UI чата) — что уже сделано
- **`[R]` UI-крейты закрыт** → [ADR 0001](docs/decisions/0001-ui-crates-ratatui-030.md):
  `tui-textarea`/`ratatui-markdown` лочат ratatui 0.29 → берём `tui-markdown` +
  `tui-scrollview`, а ввод — **собственный виджет** (это же снимает `[R]` отрисовки
  спелл-чека: рисуем подчёркивания сами).
- **`Storage` подключён к оркестратору**: мульти-чат, bootstrap (дефолтный профиль+чат),
  загрузка/сохранение с дебаунсом (800мс), новые `AppCommand`/`AppEvent`
  (`NewChat/SwitchChat/RenameChat/CloneChat/DeleteChat`, `ChatList/ChatActivated`).
  `ChatSummary` живёт в `entities/chat.rs` (нужен и `app`, и `widgets`).
- **`shared/markdown.rs`**: рендер через `tui-markdown` + unicode-аппроксимация LaTeX.
- **`features/chat_search_sort.rs`, `features/rename_chat.rs`**: чистая логика.
- **`widgets/chat_list.rs`**: оверлей `Ctrl+L` (поиск, 2 сортировки `Tab`, `F2`-переименование,
  `Ctrl+N` новый, `Ctrl+D` копия, `Del` удалить). Интегрирован в `runtime.rs`.
- **`widgets/input_box.rs`**: собственный multiline-ввод (`Vec<Vec<char>>`, курсор по
  символам, скролл; `Shift+Enter` перенос, `Enter` отправка). Заменил строку-заглушку.
- **`widgets/message_feed.rs`**: лента с markdown-рендером, сворачиваемый блок «мыслей»
  (`Ctrl+T`), скролл `PageUp/PageDown` со «следованием за хвостом».
- **Перенос слов (`shared/wrap.rs`)**: строки переносятся по словам **до** `Paragraph`
  (длинное слово рвётся по символам; ширина в колонках через `unicode-width` — кириллица
  =1, эмодзи/CJK =2). В `message_feed` это сохраняет row-based скролл/«хвост», в
  `input_box` — точный маппинг курсора `(строка,столбец)→(визуальный ряд,колонка)` и рост
  высоты поля под перенос (`visual_line_count`). `Paragraph::wrap` сознательно не
  используется (ломал бы математику скролла и позицию курсора).
- **`widgets/status_bar.rs`**: чистый виджет статуса (`ServerStatus` переехал в `shared`).
- **`screens/chat.rs`**: `ChatScreen` — всё состояние UI, мутаторы-проекция событий,
  `handle_key → ChatIntent`, `render`. `runtime.rs` теперь тонкая петля: применяет
  `AppEvent` мутаторами, транслирует `ChatIntent → AppCommand`. **FSD соблюдён**:
  `screens`/`widgets` не импортируют `app` (экран отдаёт `ChatIntent`, не `AppCommand`).

- **`features/spellcheck/`**: `spellbook` (Hunspell) + свой сегментатор слов +
  персональный словарь. `SpellChecker`: `check`/`misspellings`/`suggest`/
  `add_to_personal`. Слово верно, если принято хотя бы одним словарём. Словари
  грузятся **в фоне** из `dictionaries/` (нет каталога → чек выключен, не падает).
  В UI: подчёркивание ошибок (`UNDERLINED`/red) с дебаунсом 300мс; попап подсказок
  `Ctrl+G` (замена слова / «добавить в словарь» → `personal_dictionary.txt`).

### M4 (профили + изоляция) — что уже сделано
- **`features/profiles.rs`**: чистые операции над профилями (создание/правка/
  валидация имени `sanitize_name`, `ProfileEdit`/`apply_edit`). UI-секция — на M8.
- **`Chat::from_profile` копирует** `system_message`/`character_names` (у чата своя
  копия — правки профиля их не меняют), но **НЕ копирует** `default_sampling`.
- **Трёхуровневый семплинг** ([`sampling::resolve`], spec §8.3): разрешение
  `Chat.sampling_override → Profile.default_sampling → глобальный` **во время запроса**
  (а не снимком при создании); снимок применённого — в `Message.metadata`.
  `sampling_override` остаётся `None` до явного `set_sampling` (M5).
- **Контракт**: `AppCommand::NewChat{profile_id}`, `CreateProfile`/`DeleteProfile`;
  `AppEvent::ProfileList`. `ProfileSummary` — в `entities/profile.rs`.
- **Оркестратор**: ведёт профили, эмитит `ProfileList`, создаёт чат из выбранного
  профиля (+ приветствие), каскад `DeleteProfile → чаты` (`hide_profile_cascade`),
  защита «нельзя удалить последний профиль».
- **`widgets/profile_list.rs`**: оверлей выбора профиля. `Ctrl+N` открывает его при
  >1 профиле, иначе создаёт сразу. Проводка в `screens/chat.rs` + `runtime.rs`.
- **Изоляция** notes/RAG по `profile_id` подтверждена тестами (db + фасад `Storage`).
  Реальный `ToolContext` — deliverable M5 (здесь `profile_id` уже доступен из чата).

### M5 (инструменты + agentic-loop) — что уже сделано
- **`[R]` Эмбеддинги закрыты** → [ADR 0002](docs/decisions/0002-embeddings-dedicated-server.md):
  **выделенный** embedding-сервер (трейт `Embedder` отделён от `EngineBackend`);
  `XinferClient: Embedder`; env `MINDFORK_EMBED_URL`/`_BIN`/`_MODEL`/`_PORT`;
  если не настроен — `UnavailableEmbedder` (RAG отдаёт ошибку, не падает).
- **Движок: tool-calls** — `ChatChunk::ToolCall(ToolCallDelta)` + `ToolCallAccumulator`,
  `ChatRequest.tools`/`tool_choice=auto`, сериализация `assistant.tool_calls` в
  истории и парсинг `delta.tool_calls` (wire/client).
- **`features/tools/`**: трейт `Tool`, `ToolContext` (снимок), `ToolOutcome`+`ChatEffect`,
  `ToolRegistry` (`schemas_for` = профиль ∩ реестр, `invoke`). `standard_registry`/
  `default_tool_ids`. Инструменты: интроспекция (`get/set_sampling` с merge,
  `get/set_system_message`, `get_last_user_message_time`), заметки (`note_save`/
  `note_recall`), RAG (`rag_add`/`rag_search`: чанкинг+эмбеддинг+kNN). Всё с
  изоляцией по `profile_id`.
- **Оркестратор: клиентский agentic-loop** (spec §6.3): стрим → `ToolCalls` →
  исполнение → новый запрос, до `max_tool_rounds`; эффекты применяет оркестратор
  (владелец `Chat`) со следующего хода (§6.6). `Storage` → `Arc<Storage>`.
- **UI**: `AppEvent::ToolCall` → tool-блоки (🔧 имя/аргументы/результат) в ленте;
  tool-сообщения не дублируются (показываются как блоки внутри ответа ассистента).
- Остаётся ручная проверка `#[ignore]` полного agentic-цикла на реальной модели
  (нужен xinfer с tool-calling) и качество эмбеддингов выделенной модели.

### M6 (`call_subagent`) — что уже сделано
- **`features/tools/subagent.rs`**: `call_subagent` (spec §9.3.2) — независимый
  одно-ходовый запрос через `ctx.engine`: `system` = заданный, единственное `user`
  = `message`, **без истории, без инструментов** (`tools: []` → запрет вложенности/
  рекурсии). Лимит токенов (≤1024) и таймаут (60с, отмена по `CancellationToken`).
- Зарегистрирован в `standard_registry`/`default_tool_ids`; в UI — обычный
  tool-блок (имя/аргументы/результат). Подчиняется `max_tool_rounds` (один раунд).

### M7 (web + Python) — что уже сделано
- **`[R]` Эндпоинт DDG закрыт**: `lite.duckduckgo.com/lite` (POST `q=`) — простой
  стабильный HTML. `reqwest` теперь с `rustls`+`form`; добавлен `scraper`.
- **`features/tools/web.rs`**: `web_search` — запрос к DDG lite, парсинг
  `a.result-link`/`td.result-snippet`, декодирование реального URL из `uddg=`.
  Извлечение контента страниц/реранкинг — на будущее.
- **`features/tools/python.rs`**: `python_exec` — отдельный процесс (`python -c`),
  таймаут 10с (kill_on_drop), обрезка вывода. Путь к интерпретатору из конфига.
- **Глобальные выключатели** (`config.tools`: `web_enabled=true`, `python_enabled=
  false`, `python_path`). Эффективный набор = профиль ∩ выключатели
  (`effective_tool_ids`); agentic-loop **гейтит и вызов** (выключенный инструмент
  отклоняется, не исполняется). На Windows Python по умолчанию выключен.
- Реальные сетевой/Python прогоны — `#[ignore]` (нужны сеть/интерпретатор).

### M8 (экран настроек) — что уже сделано
- **`AppConfig` расширен** (`shared/config.rs`): `EmbedSettings` (выделенный
  embedding-сервер, ADR 0002), `ToolSettings.subagent_max_tokens/_timeout_secs`,
  `InterfaceSettings` (`Theme` auto/dark/light, `spellcheck_enabled`,
  `selected_dictionaries`). Всё через `#[serde(default)]` — старые `settings.json`
  читаются без миграции.
- **`ToolConfig`** (`features/tools`): реестр строится из конфига
  (`standard_registry(&ToolConfig)`); `CallSubagent::new(max_tokens, timeout)` —
  лимиты не захардкожены; пересобирается при правках `config.tools`.
- **Оркестратор владеет всем `AppConfig`** и серверами через
  **`ServerSupervisor`** (`app/supervisor.rs`, real `XinferSupervisor` + mock):
  `apply_chat`/`apply_embed` поднимают серверы из конфига; `ServerHandle` живут в
  оркестраторе (`kill_on_drop`). `main.rs` больше не резолвит бэкенд — грузит
  конфиг и сеет его env-переменными (`apply_env_overrides`, dev-workflow).
- **Команды/событие настроек**: `AppEvent::Settings { config, profiles }` (полный
  снимок); `AppCommand::UpdateConfig`/`UpdateProfile` — правки сохраняются
  (`save_config`/`upsert_profile`), **смена `xinfer` перезапускает сервер**, смена
  `tools` пересобирает реестр, смена `embed` пере-поднимает embedding-сервер.
  Внутренний канал `ServerStatus` (фоновый probe супервайзера → `AppEvent`).
- **`screens/settings.rs`** (`SettingsScreen` + `SettingsIntent`, вход `Ctrl+P`):
  левое меню секций (Модель/Инференс/Семплинг/Профили/Инструменты/Интерфейс) +
  правый список полей. `Tab` секция, `↑↓` поля, `Space` тумблер, `←→` enum,
  `Enter` редактор текста/числа. Правка применяется **сразу при коммите**
  (`SettingsIntent → AppCommand`). Профили: выбор `←→`, правка полей, тумблеры
  инструментов, `Ctrl+N`/`Ctrl+D` создать/удалить. **FSD строгий**: `screens` не
  импортирует `app`.
- **`runtime`**: экран настроек поверх чата; события продолжают применяться к чату
  (генерация не прерывается); переэмит `Settings` обновляет рабочую копию (create/
  delete профилей видны сразу).
- Тема/раскладка клавиш в «Интерфейсе» — пока **поля** (значения сохраняются);
  реальное применение темы (`shared/theme.rs`) и кастом-раскладки — на **M9** по плану.
  Спелл-чек вкл/выкл и выбор словарей тоже редактируются, но фактическое
  применение к загрузке словарей — M9.

### M9 (полировка) — что уже сделано
- **Импортёр LameLLaMA (.NET)** (`features/migration.rs`, spec §12.2): CLI
  `mindfork --import-lamellama <dir>`. `Settings.json` (`Configurations` → профили
  с детерминированным UUIDv5 → идемпотентно) + `Conversations/*.json` → чаты
  (исходный Id; `*.deleted` пропускаются). Семплинг с отбрасыванием неподдержанного,
  спелл-чек/тема → `config.interface`. UTF-8 BOM отбрасывается. **Проверено на
  реальных данных: 6 профилей, 226 чатов.**
- **Снят crate-wide `#![allow(dead_code)]`**: мёртвый код удалён (поля
  `State`/`GenResult`, `shared/error.rs`, неиспользуемые `is_empty`) или размечен
  точечным `#[allow(dead_code)]`/`#[cfg(test)]` с комментарием.
- **Бэкап файла чата** (атомарная запись + `.bak`) финализирован тестом (§12.3).
- **UX**: оверлей помощи по клавишам (`F1`/`?`), подсказка в статус-баре.
- **Релиз**: `[profile.release]` (LTO/strip, `panic=unwind`); `docs/install.md`
  (сборка Win/Linux, портативные данные, xinfer managed/external + env, словари,
  импорт, запуск). `cargo build --release` собирается.
- **Темы** (`shared/theme.rs`, ветка `m9-themes`): семантическая палитра `Palette`
  (роли user/assistant/tool/success/warning/error/accent) из
  `config.interface.theme` (auto = именованные ANSI; dark = яркие; light = Rgb-
  затемнённые). Протянута в виджеты с реальными цветами (`message_feed`,
  `status_bar`, `input_box`, `chat_list`); модификаторы (dim/bold/reversed)
  тема-независимы. `ChatScreen` обновляет палитру из события `Settings`.
- **Спелл-чек подчиняется настройкам**: `dict::load(enabled, selected)` грузит
  только выбранные словари (или ничего при выключенном). `app/runtime.rs` владеет
  загрузкой — `SpellLoader` перегружает словари в фоне при изменении
  `interface.spellcheck_enabled`/`selected_dictionaries` (generation-guard), так что
  тумблер и выбор словарей применяются на лету.
- **Раскладко-независимые Ctrl-шорткаты** (`shared/keys.rs`): символ нормализуется
  в «физическую» латинскую клавишу (таблица русской ЙЦУКЕН), поэтому `Ctrl+L/N/G/T/C/P`
  срабатывают и при кириллической раскладке (где crossterm отдаёт `Ctrl+д` и т.п.).
  Применено в `screens/chat.rs`, `screens/settings.rs`, `widgets/chat_list.rs`.
  **Экран настроек перенесён с `Ctrl+,` на `Ctrl+P`** (`Ctrl+,` под Windows
  перехватывает Windows Terminal).

### M9 — проверка Gemma (сделано)
- **Gemma 4 E4B-it проверена** на живом `llama-server` (llama.cpp). Добавлены/
  расширены `#[ignore]`-смоуки в [client.rs](src/shared/api/client.rs):
  анти-самообрыв для обоих семейств (`<|im_end|>` + `<end_of_turn>`), tool-calling
  (`finish_reason=tool_calls` + разбор `delta.tool_calls`), «мысли»
  (`reasoning_content` → `Thoughts`). Все зелёные. Нюанс: Gemma-reasoning «думает»
  перед вызовом инструмента — в смоуках `max_tokens` щедрый (512).
- **`xinfer` для Gemma 4 не годится** (сырой); рабочий путь — external `llama-server`.

### Отложено за пределы M3
- **Сворачивание/выделение per-message** и tool-блоки в ленте — сейчас «мысли»
  сворачиваются глобально (`Ctrl+T`); выделение сообщений и tool-блоки — на M5.
- **Словари спелл-чека не входят в репозиторий** (`.gitignore`; в git только
  `dictionaries/.gitkeep`): положить Hunspell-пары `*.aff`+`*.dic` (`en_US.*`,
  `en_GB.*`, `ru_RU.*`) в `dictionaries/` корня проекта. `build.rs` сам копирует
  их рядом с бинарником (`target/<profile>/dictionaries/`) при сборке. Открытый
  `[R]`: качество ru_RU на реальном словаре (по факту — работает).
- ~~Снять `#![allow(dead_code)]` из `main.rs`~~ — **сделано на M9**: мёртвый код
  удалён или размечен точечно (`#[allow(dead_code)]`/`#[cfg(test)]`).

## Подводные камни
- **TUI «висит» при headless-запуске** (без TTY) — это нормально; чистый выход
  по `q`/`Esc`/`Ctrl+C` проверяется юнит-тестами `map_key`. Для живой проверки нужен
  настоящий терминал.
- Точечные `#[allow(dead_code)]` (с комментарием) на осознанном API-впереди-
  потребителей: `ToolContext.chat_id`, `ApiRole::System`, `Db::note_delete/rag_count`,
  `ToolRegistry::get`. Crate-wide allow снят на M9.
- Данные приложения портативны: лежат рядом с бинарником (в dev — `target/debug/`).
