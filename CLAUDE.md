# CLAUDE.md — ориентир по проекту mindfork-rs

Консольное (TUI) приложение ИИ-чата на Rust. Локальные модели **Gemma 3/4** и
**Qwen 3.5/3.6** через **xinfer** (OpenAI-совместимый сервер). UI на **ratatui**.
Платформы: Windows + Linux. Архитектура — **Feature-Sliced Design (FSD)**.

## Главные документы (читать перед работой)
- **[spec.md](spec.md)** — полная инженерная спецификация (доменная модель, движок,
  инструменты, профили, UI, тестирование). Источник истины по «что» и «почему».
- **[plan.md](plan.md)** — пошаговый план по этапам M0–M9 (задачи, тесты, DoD).
- **[docs/xinfer-contract.md](docs/xinfer-contract.md)** — зафиксированный по
  исходникам контракт xinfer (эндпоинты, поля запроса/стрима, семплинг, EOS,
  эмбеддинги). Сверяться при любой работе со слоем движка.

## Ключевые архитектурные решения
- **xinfer = локальный HTTP-сервер** (managed-подпроцесс или external), приложение —
  HTTP-клиент. Встраивание (rlib) сознательно НЕ используется (тянет candle/CUDA).
- **Agentic-loop клиентский** (в оркестраторе). Инструменты возвращают результат +
  эффекты; оркестратор (единственный владелец `Chat`) их применяет — без локов.
- **Семплинг ограничен тем, что есть в xinfer**: temperature, top_k, top_p,
  frequency/presence_penalty, max_tokens, thinking, reasoning_effort. НЕТ min_p,
  repetition_penalty, seed-на-запрос, DRY/mirostat. (См. contract §7.)
- **EOS**: остановка по token-id на сервере; поле `stop` НЕ отправляем (анти-самообрыв).
- **«Мысли» (CoT)**: `delta.reasoning_content` (managed-сервер запускается с
  `XINFER_STREAM_AS_REASONING_CONTENT=1`); fallback — парсинг `<think>` из content.
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
- `src/shared/api/` — движок за трейтом `EngineBackend` (xinfer-клиент, супервайзер, парсер мыслей, mock).
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
Запуск против реального xinfer (M1-smoke, contract §9):
```
xinfer --m Qwen/Qwen3-0.6B --server --port 8000      # терминал 1
$env:MINDFORK_XINFER_URL="http://127.0.0.1:8000/v1"  # терминал 2 (PowerShell)
cargo run
cargo test -- --ignored                              # смоук-тесты
```
Env для выбора бэкенда: `MINDFORK_XINFER_URL` (external) ИЛИ `MINDFORK_XINFER_BIN`
(+ `MINDFORK_MODEL`, `MINDFORK_XINFER_PORT`, `MINDFORK_ISQ`) для managed.

## Статус (на 2026-06-14)
Сделано **M0, M1, M2, M3, M4, M5, M6** (в `main`) и **весь M7** (в ветке `m7-web-python`).
**184 теста зелёные, 4 `#[ignore]`.** Чат-цикл, профили с изоляцией, инструменты
с клиентским agentic-loop, саб-агент, web-поиск и Python под выключателями.
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

### Отложено за пределы M3
- **Сворачивание/выделение per-message** и tool-блоки в ленте — сейчас «мысли»
  сворачиваются глобально (`Ctrl+T`); выделение сообщений и tool-блоки — на M5.
- **Словари спелл-чека не входят в репозиторий** (`.gitignore`; в git только
  `dictionaries/.gitkeep`): положить Hunspell-пары `*.aff`+`*.dic` (`en_US.*`,
  `en_GB.*`, `ru_RU.*`) в `dictionaries/` корня проекта. `build.rs` сам копирует
  их рядом с бинарником (`target/<profile>/dictionaries/`) при сборке. Открытый
  `[R]`: качество ru_RU на реальном словаре (по факту — работает).
- Снять `#![allow(dead_code)]` из `main.rs` — **после M5** (часть API: notes/RAG/db,
  ещё не имеет потребителей; сейчас снятие сломает `clippy -D warnings`).

## Подводные камни
- **TUI «висит» при headless-запуске** (без TTY) — это нормально; чистый выход
  по `q`/`Esc`/`Ctrl+C` проверяется юнит-тестами `map_key`. Для живой проверки нужен
  настоящий терминал.
- `#![allow(dead_code)]` в `main.rs` — временный (часть API опережает потребителей:
  notes/RAG/db появятся на M5), убрать после M5.
- Данные приложения портативны: лежат рядом с бинарником (в dev — `target/debug/`).
