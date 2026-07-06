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
- **[docs/install.md](docs/install.md)** — установка/запуск (llama.cpp `llama-server`
  managed/external, env, OpenAI-совместимый протокол, словари, импорт).
  **Актуально по движку.**
- **[docs/decisions/](docs/decisions/)** — ADR: UI-крейты под ratatui 0.30 (0001),
  выделенный embedding-сервер (0002), собственный markdown-рендерер (0003),
  контракт движка и мульти-провайдерный инференс без крейт-сплита (0004).
- **[docs/history/](docs/history/)** — архив реализованного: исходное техзадание
  (`request.md`), выполненный пошаговый план M0–M9 (`plan.md`) и **дизайн-планы
  завершённых направлений** (ниже). Историческая справка, не источник истины.
- **Дизайн-планы направлений** (банк идей + реализованный MVP-зонд; направления
  **реализованы**, планы лежат в `docs/history/`): «модель себя»
  ([self-model.md](docs/history/self-model.md), [self-model-mvp.md](docs/history/self-model-mvp.md))
  + доводка ([refinements.md](docs/history/refinements.md));
  **связность заметок** ([notes-connectivity.md](docs/history/notes-connectivity.md)) —
  сдвиг памяти от накопления к интеграции (семантический поиск, ревизия, ворота
  совместимости); **нарратив как заметки**
  ([narrative-as-notes.md](docs/history/narrative-as-notes.md)) — унификация: наблюдения
  «модели себя» переезжают в заметки с тегом `@self`; **summary как снимок**
  ([summary-as-snapshot.md](docs/history/summary-as-snapshot.md)) — анти-раздувание
  описания «модели себя» (жанровая граница, ворота размера, посекционные бюджеты
  инъекции, диета эха).

## Ключевые архитектурные решения
- **Движок = локальный OpenAI-совместимый HTTP-сервер** (managed `llama-server` или
  external любой), приложение — HTTP-клиент (`OpenAiClient`). Встраивание (rlib)
  сознательно НЕ используется (тянет candle/CUDA).
- **Agentic-loop клиентский** (в оркестраторе). Инструменты возвращают результат +
  эффекты; оркестратор (единственный владелец `Chat`) их применяет — без локов.
- **Семплинг** (`SamplingConfig`, spec §8): стандартные OpenAI-поля (temperature,
  top_k, top_p, frequency/presence_penalty, max_tokens, thinking, reasoning_effort,
  reasoning_budget) **плюс расширения llama.cpp**, которые `llama-server` принимает
  в теле запроса: dynatemp_range/dynatemp_exponent (динамическая температура), min_p,
  top_n_sigma, typical_p, adaptive_target/adaptive_decay (adaptive-p, эксперим.),
  repeat_penalty/repeat_last_n, dry_* (multiplier/base/allowed_length/penalty_last_n/
  sequence_breakers), xtc_* (probability/threshold), mirostat/mirostat_tau/mirostat_eta,
  seed, samplers (порядок семплеров). Каждое поле `Option`, шлётся только когда
  задано (`skip_serializing_if`) — незаданное не попадает в JSON; неподдержанное
  сервером поле он игнорирует (строгий сторонний OpenAI-сервер мог бы отвергнуть, но
  лишь если пользователь сам выставит расширение). Списочные поля
  (`dry_sequence_breakers`, `samplers`) — JSON-массивы, пустыми не шлются. UI — секция
  «Семплинг» (подсекции Ассистент/Имперсонация), поля через `SamplingParam`
  (`screens/settings.rs`; списки правятся текстом: samplers через «;», DRY-брейкеры
  через «,» с эскейпами \n/\t/\r); также правится инструментом `set_sampling`
  (merge всех полей).
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
llama-server -m gemma-4-it.gguf   --host 0.0.0.0 --port 8000 -ngl 99 -c 16384 --jinja        # чат
llama-server -m bge-m3-Q8_0.gguf  --host 0.0.0.0 --port 8001 -ngl 99 -c 16384 --embeddings   # эмбеддер (память/RAG)
$env:MINDFORK_ENGINE_URL="http://127.0.0.1:8000/v1"   # PowerShell; URL включает /v1
$env:MINDFORK_EMBED_URL="http://127.0.0.1:8001/v1"    # нужен воротам/графу/консолидации памяти
cargo run
# Все живые смоуки (без нужной env-переменной тихо пропускаются):
cargo test -- --ignored --nocapture --test-threads=1
# Только память/модель себя/заметки (12 e2e):  cargo test e2e_live -- --ignored --nocapture --test-threads=1
```
Env для выбора бэкенда: `MINDFORK_ENGINE_URL` (external, любой OpenAI-сервер) ИЛИ
`MINDFORK_LLAMA_BIN` (+ `MINDFORK_MODEL` GGUF, `MINDFORK_NGL`, `MINDFORK_CTX`,
`MINDFORK_PORT`) для managed `llama-server`.

## Статус (на 2026-07-06)
Сделан весь план **M0–M9** плюс обширный пост-M9 (в `main`). **794 юнит-теста
зелёные, 26 `#[ignore]`-смоуков.** Набор `#[ignore]` прогнан на живой связке
**Gemma 4 31B (q4) + bge-m3** (`llama-server`, external, `--jinja`) — 25/25 зелёные
(~370с): базовые смоуки Gemma (стриминг, EOS-анти-самообрыв, tool-calling, «мысли»,
расширения семплинга, control-инструменты) + end-to-end модели себя/заметок/нарратива
(ворота дублей, граф, авто-рефлексия/консолидация, семантические черты, кросс-органные
связи, цитирование RAG, нарратив-как-заметки). Мульти-провайдер (OpenAI/Gemini/Claude)
покрыт unit-тестами + отдельными `MINDFORK_ANTHROPIC_KEY`-смоуками.
Чат-цикл, профили с изоляцией, инструменты с клиентским agentic-loop, саб-агент,
web-поиск и Python под выключателями, экран настроек со всеми секциями и
перезапуском managed-сервера при смене модели, импорт из LameLLaMA (.NET), оверлей
помощи, релизный профиль, **темы (auto/dark/light)**, **режим совместимости со
старым терминалом** (conhost Windows 10: безопасные глифы вместо эмодзи), **спелл-чек
на лету по настройкам**, **«модель себя»/собеседника** и **связность заметок**
(нарратив как заметки). Полный журнал пост-M9 — ниже.

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
  **Заменён пост-M9** на собственный рендерер на `pulldown-cmark` (таблицы +
  delimiter-scoped LaTeX + тема) → [ADR 0003](docs/decisions/0003-own-markdown-renderer.md).
- **`features/chat_search_sort.rs`, `features/rename_chat.rs`**: чистая логика.
- **`widgets/chat_list.rs`**: оверлей `Ctrl+L` (поиск, 2 сортировки `Tab`, `F2`-переименование,
  `Ctrl+N` новый, `Ctrl+D` копия, `Del` удалить). Интегрирован в `runtime.rs`.
- **`widgets/input_box.rs`**: собственный multiline-ввод (`Vec<Vec<char>>`, курсор по
  символам, скролл; `Shift+Enter` перенос, `Enter` отправка). Заменил строку-заглушку.
- **`widgets/message_feed.rs`**: лента с markdown-рендером, сворачиваемый блок «мыслей»
  (`Ctrl+T`), скролл `PageUp/PageDown` **и колесом мыши** (`scroll_up/scroll_down`,
  тумблер захвата `Ctrl+W` — см. пост-M9 ниже) со «следованием за хвостом».
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
- **`features/tools/web.rs`**: `web_search` — **мульти-провайдер с фоллбэком**
  (`PROVIDERS`), у каждого своя инфраструктура и разметка: DDG lite
  (`a.result-link`/`td.result-snippet`) → DDG html (`a.result__a`/`a.result__snippet`)
  → **Mojeek** (`GET ?q=`, `a.title`/`p.s`) → **Ecosia** (`GET ?q=`, стабильные
  `data-test-id`; заголовок и ссылка — РАЗНЫЕ теги, парсер выравнивает href/title/
  snippet по индексу). Первый с непустой выдачей выигрывает; декодирование реального
  URL из `uddg=`. **Анти-бот троттлинг распознаётся** (`is_throttled`: DDG `HTTP 202`/
  маркер `anomaly`, Mojeek/прочие `403`/`429`) — раньше 202 проходил как «успех»
  (`error_for_status` пропускает 2xx) и парсился пустой → модель видела «нет
  результатов» на корректный запрос, а 403 всплывал фатальной ошибкой «провайдер
  недоступен». Теперь при троттлинге сразу пробуется следующий провайдер (троттл
  липкий per-IP, ретраи лишь углубляют; провайдеры режут независимо → почти всегда
  отвечает кто-то); если **все** недоступны — явная ошибка (не «нет результатов»),
  чтобы модель повторила позже. Таймаут запроса 15с.
- **Извлечение контента + реранкинг (пост-M9, сделано)**: после выдачи страницы
  результатов загружаются параллельно (`enrich_with_content` →
  `futures::join_all`, браузероподобные заголовки `Accept`/`Accept-Language` —
  часть сайтов иначе отдаёт блок-страницу), из HTML извлекается читаемый текст
  (`extract_readable` на `scraper`: абзацы/списки `<p>`/`<li>` из `<article>`/`<main>`,
  иначе из всего документа; **всё внутри `nav`/`header`/`footer`/`aside` отбрасывается**
  — `in_boilerplate` по предкам, иначе на сайтах без семантической разметки в контент
  лезло мега-меню; фрагменты < 40 симв. тоже отсеиваются; script/style не в `<p>` →
  сами; усечение до `MAX_CONTENT_CHARS=1500`). Сбои загрузки/не-2xx/пустое извлечение
  логируются в `logs/` (`tracing::debug`, для диагностики). Затем результаты
  **переупорядочиваются эмбеддингами** (`rerank_by_embeddings` через `ctx.embedder`,
  ADR 0002): запрос + `title`+контент (≤`RERANK_EMBED_CHARS=800`) каждого результата
  эмбеддятся одним запросом, сортировка по убыванию косинусной близости
  (`rerank_order`, стабильная — при равенстве остаётся порядок провайдера). Всё —
  **«лучшее усилие»**: сбой загрузки одной страницы лишь оставляет её `content`
  пустым (бот-враждебные сайты вроде Bloomberg отдают 403 — это нормально); эмбеддер
  не настроен/недоступен (RAG выключен) или вернул нестыкующееся число векторов →
  реранкинг пропускается, остаётся порядок провайдера (мягкая деградация, как у RAG).
  Вывод включает блок «Содержимое:» с извлечённым текстом.
- **Гейт `fetch_content`**: аргумент вызова `fetch_content` (`false` → быстрый путь
  «только заголовки/сниппеты», без загрузки/реранка) переопределяет дефолт из
  конфига `config.tools.web_fetch_content` (по умолчанию `true`). Дефолт прокинут
  `ToolConfig.web_fetch_content` → `WebSearch::new(bool)`; тумблер «Web: загрузка
  страниц» в секции «Инструменты» экрана настроек (с подсказкой).
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
  `mindfork import-lamellama <dir>` (изначально флаг `--import-lamellama`; переведён
  на clap-подкоманду, см. пост-M9 бэкап/restore). `Settings.json` (`Configurations` → профили
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

### Пост-M9: окно чатов на весь экран + авто-название (сделано)
- **Окно списка чатов (`Ctrl+L`) теперь во весь экран** (`widgets/chat_list.rs`):
  `render` рисует блок по всей `area` (раньше — центрированный поп-ап 60×70%);
  `centered_rect` удалён.
- **Авто-название чата (`Ctrl+R` в окне чатов)**: модель читает переписку (или её
  начало+конец, если длинная) и придумывает короткий заголовок. Чистая логика —
  `features/rename_chat.rs`: `build_conversation_digest` (помечает роли, пропускает
  системные/инструментальные/пустые, усечение середины по бюджету
  `TITLE_CONTEXT_BUDGET`), `TITLE_SYSTEM_MESSAGE`, `clean_generated_title` (срез
  кавычек + `sanitize_title`). Запрос идёт **фоновой задачей** (`spawn_title`):
  независимый одно-ходовый запрос без истории/инструментов, таймаут 30с; результат
  — во внутренний канал `title_tx` → `Orchestrator::handle_title_result`. Гейтится
  готовностью сервера (`ready_backend`); пустой чат → понятная ошибка. Контракт:
  `ChatListAction::AutoRename` → `ChatIntent::AutoRenameChat` →
  `AppCommand::AutoRenameChat`.
- **«Мысли» для заголовка глушим тремя слоями + гарантированный фолбэк.** У
  моделей со «вшитым» в GGUF thinking (Gemma `peg-gemma4`) поля
  `thinking`/`reasoning_effort` сервер игнорирует, и модель тратит **весь**
  `max_tokens` на reasoning → ответный `content` пуст («Модель не вернула
  название»; в логах llama-server `thinking = 1`, обрыв по лимиту токенов на
  reasoning). Решение: (1) новое поле `SamplingConfig.reasoning_budget=0` (llama.cpp
  built-in форматы); (2) `wire.rs` при `reasoning_budget=0` дублирует сигнал через
  `chat_template_kwargs.enable_thinking=false` (Jinja-шаблоны моделей); (3) щедрый
  `TITLE_MAX_TOKENS=2048`/таймаут 60с — если выключить не удалось, модель успевает
  **завершить** «мысли» и выдать заголовок в `content`; (4) `salvage_title_source`
  — если `content` всё же пуст, заголовок берётся из последней содержательной
  строки `reasoning_content` (`Thoughts`), так что ошибка пользователю больше не
  показывается. Для обычной генерации `reasoning_budget=None` — поля не шлются,
  поведение не меняется.
- **Отдельная область ошибок в окне списка чатов** (`AppEvent::ChatListError`):
  ошибки операций списка (авто-название/удаление/клон) идут не в ленту чата (где
  они висели за полноэкранным оверлеем до перезапуска), а в выделенную строку
  оверлея (`ChatListState.error`, цвет `palette.error`), которая **гаснет по
  первому нажатию клавиши**. `ChatScreen::set_overlay_error` направляет в оверлей,
  если он открыт, иначе (поздний ответ авто-названия при закрытом оверлее) —
  заметкой в ленту. Раньше эти ошибки шли общим `AppEvent::Error`.
- **`AppEvent::ChatRenamed { id, title }`**: обновляет шапку ленты активного чата
  без перестроения (раньше ручное переименование тоже не обновляло заголовок до
  переактивации). Эмитится из `handle_rename` и `handle_title_result`;
  `ChatScreen::rename_chat` применяет. Список/оверлей обновляются как прежде через
  `ChatList`. Шорткат раскладко-независим (`shared/keys`, физ. R = `Ctrl+к`).

### Пост-M9: правка/перегенерация последнего ответа (сделано)
- **Перегенерация (`Ctrl+R`)**: усекает историю чата по последнее сообщение
  пользователя включительно (старый ответ ассистента + tool-сообщения отбрасываются)
  и запускает генерацию заново тем же запросом. Общая часть отправки и перегенерации
  вынесена в `Orchestrator::start_generation`; лента перестраивается переэмитом
  `ChatActivated`. Команда `AppCommand::RegenerateLast`, намерение
  `ChatIntent::RegenerateLast`.
- **Удаление последнего обмена (`Ctrl+E`)**: убирает последний ответ ассистента
  вместе с вызвавшим его сообщением пользователя; текст пользователя возвращается в
  поле ввода (событие `AppEvent::RestoreInput` → `ChatScreen::restore_input`). Если
  поле непустое — текст добавляется в **начало** существующего (ввод не теряется).
  Команда `AppCommand::DeleteLastExchange`, намерение `ChatIntent::DeleteLastExchange`.
- Обе операции **гейтятся состоянием `Idle`** (во время генерации игнорируются — и на
  уровне экрана, и в оркестраторе) и являются no-op, если в чате нет сообщения
  пользователя (напр. чат только с приветствием). Шорткаты раскладко-независимы
  (`shared/keys`), добавлены в оверлей помощи (`F1`/`?`).
- **Гейт готовности сервера**: оркестратор хранит `server_status` (обновляется из
  немедленного `ChatSetup.status` и фонового probe). Отправка/перегенерация стартуют
  только в `Ready` — иначе запрос ушёл бы на ещё загружающийся managed-сервер и вернул
  `503` («engine returned an error status»). Проверка готовности в перегенерации идёт
  **до усечения истории** (иначе прежний ответ был бы снесён, а новый не пришёл бы); при
  отклонённой отправке текст возвращается в поле ввода (`RestoreInput`), а не теряется.
  Не-готовность показывается понятным сообщением («Сервер ещё подключается…»).
  `MockSupervisor` отдаёт `Ready` синхронно — тесты не зависят от гонки статуса.
- **Проба готовности учитывает загрузку модели** (`OpenAiClient::probe`): managed
  `llama-server` биндит HTTP-порт сразу, но ~секунды (крупный GGUF) отвечает `503
  Loading model` на инференс. Старая проба дёргала `/usage` и считала «готов» по
  самому факту ответа — статус `Ready` выставлялся до загрузки, и первый же запрос
  (отправка/перегенерация) падал с `503` сквозь гейт готовности. На external не
  воспроизводилось (сервер уже загружен). Фикс: проба бьёт в `/health` (в корне, вне
  `/v1`) и трактует `503` как «ещё грузится» (не готов), `200` — готов, `404`
  (сервер без `/health`) — «жив, не грузится» (готов). Теперь `server_status`
  становится `Ready` только после реальной загрузки, и гейт готовности корректно
  придерживает отправку/перегенерацию с понятным сообщением.
- Клиент больше **не глотает тело ошибки**: статус + JSON-причина
  (`{"error":{"message":…}}`) логируются в файл и попадают в текст ошибки (обрезка до
  500 симв.) — вместо бесполезного «engine returned an error status». Именно это
  тело (`503 Loading model`) и указало на корневую причину выше.

### Пост-M9: прокрутка ленты колесом мыши (сделано)
- **Колесо мыши прокручивает ленту** наравне с `PageUp/PageDown`. `ratatui::init()`
  не включает захват мыши, поэтому crossterm не доставлял события колеса — суть фикса.
  `runtime.rs` обрабатывает `Event::Mouse`, `ChatScreen::handle_mouse` маппит
  `ScrollUp/ScrollDown` → `MessageFeed::scroll_up/scroll_down` (`WHEEL_SCROLL=3`
  строки); no-op при открытом оверлее/попапе/справке.
- **Захват мыши — тумблер `Ctrl+W`** (по умолчанию **выключен**). Причина: колесо и
  нативное выделение текста делят один механизм mouse-reporting терминала —
  «только колесо, не трогая выделение» технически не отделяется. Выкл → нативное
  выделение мышью работает; вкл → колесо прокручивает ленту, выделение остаётся
  доступно с зажатым `Shift` (Windows Terminal и большинство терминалов так умеют).
  Тумблер: `ChatIntent::SetMouseCapture(bool)` → `runtime::dispatch` шлёт
  `Enable/DisableMouseCapture` (чисто терминальный side-effect; экран про терминал
  не знает, FSD). Захват **всегда снимается** на выходе и в panic-hook (терминал не
  остаётся в режиме мыши после паники). `Ctrl+M` для тумблера непригоден — терминал
  отдаёт его как `Enter`; взят `Ctrl+W` (W=wheel), раскладко-независим (`shared/keys`).
- Текущий режим виден в **статус-баре** (`мышь: прокрутка/выделение (Ctrl+W)`);
  добавлено в оверлей помощи (`F1`/`?`). В режиме «прокрутка» **слово «прокрутка»**
  подсвечивается цветом `accent` (как заголовки markdown в ленте) —
  `Palette::hint_highlight_value` красит значение после двоеточия (подпись с `:`
  остаётся приглушённой; разбиение по `:`, а не по пробелу — корректно для
  многословных значений при локализации); в режиме «выделение» описание целиком
  приглушённое.

### Пост-M9: свой markdown-рендерер (таблицы + LaTeX + тема) (сделано)
- **`shared/markdown.rs` переписан** с `tui-markdown` на собственный walker по
  событиям `pulldown-cmark` 0.13 → [ADR 0003](docs/decisions/0003-own-markdown-renderer.md).
  Причина: `tui-markdown` не поддерживал таблицы и math и игнорировал тему.
  Сигнатура: `render(input, width, palette) -> Text<'static>` (`message_feed`
  прокидывает ширину панели и палитру). Зависимость `tui-markdown` убрана;
  добавлены прямые `pulldown-cmark`, `syntect`, `ansi-to-tui`.
- **Тема**: цвета (заголовки/ссылки/маркеры списков) из `Palette` (раньше лента
  игнорировала dark/light). Подсветка блоков кода — `syntect` + `ansi-to-tui`,
  **syntect-тема строится из `Palette`** (`build_code_theme`): scope'ы → роли
  (keyword→accent, string→success, число→warning, функция→user, тип→assistant),
  текст/комментарии — серым по светлоте фона (флаг `Palette.dark`: Auto/Dark→тёмный,
  Light→светлый); именованные ANSI приводятся к RGB (Campbell). Темы кэшируются по
  палитре (`Box::leak` — палитр конечно много, `HighlightLines<'static>`). Раньше
  была захардкожена `base16-ocean.dark`, не согласованная с темой (закрыт задел
  ADR 0003).
- **LaTeX delimiter-scoped** (по образцу .NET `LaTeXConverter`): `normalize_delimiters`
  приводит `\(…\)`→`$…$`, `\[…\]`→`$$…$$` (пропуская код-спаны/блоки), парсер с
  `ENABLE_MATH` даёт `InlineMath`/`DisplayMath`, и только их содержимое идёт через
  `latex_to_unicode` — доллары снимает парсер (нет «$→$»), нет ложных срабатываний
  в прозе/коде. **Смена поведения:** «голые» команды вне `$…$` (`\alpha`, `x^2`)
  больше не конвертируются. Конвертер расширен: `\frac{a}{b}`→`a/b`, `\sqrt{x}`→
  `√(x)`, `\pmod{n}`→`(mod n)`, текстовые/шрифтовые обёртки и акценты
  (`\text/\mathrm/\mathbb/\vec/\hat/\overline/…`) → содержимое, операторные
  имена-функции (`\log/\sin/\cos/\lim/\max/\gcd/…`) → словом без `\` (главная
  пропажа — `\log` в «O(n \log n)»), размерные модификаторы скобок
  (`\left/\right/\big/\Big/…`) снимаются, spacing-команды, защита `\{ \}`, снятие
  группирующих скобок, десятки символов.
- **Таблицы** (`ENABLE_TABLES`): ячейки копятся в `TableBuilder`, на закрытии —
  box-drawing-раскладка под ширину панели. Ширины колонок — «водоналивом»
  (`fit_columns`): узкие сохраняют натуральную ширину, остаток равномерно широким
  с полом читаемого минимума (`MIN_COL`/`MAX_MIN`). Содержимое ячейки переносится
  по словам (переиспользует `shared::wrap`), высота строки = макс рядов среди
  ячеек, выравнивание из разметки, заголовок жирным. Если минимумы не вмещаются —
  натуральная ширина + горизонтальный клип с «…». Таблица гарантированно ≤ ширины
  → повторный перенос в `message_feed` безопасен. Гориз. скролл вместо клипа — задел.

### Пост-M9: перерисовка по изменениям (фикс мигания курсора) (сделано)
- **Петля `app/runtime.rs` рисует только по изменениям** (флаг `dirty`), а не на
  каждый тик. Раньше `terminal.draw` вызывался безусловно каждую итерацию (~20/сек,
  период `TICK=50мс`): при фокусе на поле ввода каждый кадр зовёт
  `frame.set_cursor_position`, и `ratatui` после `draw` всегда шлёт «показать +
  переместить курсор» даже при пустом diff буфера. Windows Terminal сбрасывает фазу
  мигания курсора на каждое перемещение → курсор мигал чаще и неровно (CPU при этом
  ~0%, т.к. diff пустой). Теперь `dirty` поднимается на: применённое событие
  оркестратора, терминальное событие (ввод/мышь/**ресайз** — раньше его «прятала»
  безусловная перерисовка, теперь обрабатывается явно), перезагрузку словаря,
  пересчёт подсветки орфографии. Анимаций по таймеру в рендере нет, так что
  простаивающие тики перерисовки не нужны.
- **Дебаунс-перепроверка орфографии вынесена в петлю.** `maybe_recheck_spelling`
  (`screens/chat.rs`) — отложенное по таймеру действие (дебаунс 300мс): раньше его
  дёргал только `render`, а `render` шёл каждый тик. С перерисовкой-по-изменениям
  после последней клавиши событий больше нет → подсветка не появлялась (видно
  только при наличии словарей). Фикс: тело петли всё равно крутится каждый тик
  (таймаут `poll`), поэтому она и вызывает `maybe_recheck_spelling` (теперь `pub`,
  возвращает `bool` — пересчитал ли), поднимая `dirty` **только** когда подсветка
  реально изменилась. Так пробуждение по истечении дебаунса обеспечивает петля, а не
  тики рендера, и лишних кадров (с перестановкой курсора) на простое нет.

### Пост-M9: managed — предполётная проверка файла модели (сделано)
- **Симптом**: в managed-режиме при несуществующем/недоступном GGUF приложение
  надолго (до таймаута `MANAGED_READY_TIMEOUT=600с`) висло в статусе «сервер:
  подключение…». Причина: `ServerHandle::launch` (`shared/api/server.rs`) делает
  `spawn` дочернего `llama-server`, а `spawn` **успешен и без файла модели** (падает
  лишь при отсутствии самого бинарника). `llama-server` затем умирает на загрузке и
  выходит, но фоновый probe `wait_until_ready` этого не замечает и впустую опрашивает
  порт до дедлайна.
- **Фикс**: предполётная проверка в `ServerHandle::launch` — если `model_path` задан,
  но `Path::is_file()` ложно, сразу `bail!("файл модели не найден или недоступен: …")`
  **до** `spawn`. Элегантно ложится в существующую архитектуру: супервайзер
  (`app/supervisor.rs`) уже маппит ошибку `launch` → `ServerStatus::Disconnected(msg)`,
  а статус-бар её показывает — никакой доппроводки. Покрыто тестами
  `launch_missing_model_file_errors_before_spawn` (без tokio-рантайма, проверка до
  spawn) и `managed_with_missing_model_is_disconnected` (реальный путь → `Disconnected`).
- **Закрыто** (см. ниже «Пост-M9: мониторинг раннего выхода `Child` в пробе»):
  процесс, умерший уже *в ходе* загрузки (битый GGUF, OOM), теперь обнаруживается
  пробой сразу, а не по таймауту.

### Пост-M9: копирование переписки чата в буфер обмена (сделано)
- **`F5` копирует всю переписку чата в системный буфер обмена** — и в окне списка
  чатов (`Ctrl+L`, копируется выделенный чат), и в основном окне чата (копируется
  активный чат). `F5` (а не `Ctrl+C`) — чтобы не путать с «выходом» по `Ctrl+C` на
  уровне экрана; функц. клавиша не зависит от раскладки (F1 — справка, F2 —
  переименование уже заняты). В основном окне `F5 → ChatIntent::CopyChat(active)`
  (оверлей перехватывает `F5` раньше, так что конфликта нет); подтверждение/ошибка
  при закрытом оверлее идут заметкой в ленту (`set_overlay_notice/_error` → `push_*`).
- **Где формируется текст**: оверлей видит только `ChatSummary` (заголовок+счётчик,
  без текста), поэтому переписку собирает оркестратор — единственный владелец `Chat`.
  Чистый форматтер `features/chat_export.rs::format_conversation(title, &messages)`:
  помечает роли (`Пользователь`/`Ассистент`), сохраняет многострочный текст,
  пропускает системные/инструментальные/пустые сообщения, заголовком ставит имя чата;
  `None` (нет содержательных сообщений) → ошибка «Нечего копировать».
- **Запись в буфер — side-effect UI-слоя** (как тумблер мыши): оркестратор эмитит
  `AppEvent::CopyToClipboard(text)`, `app/runtime.rs` пишет через `arboard` (крейт
  `arboard`, `default-features = false` — только текст, без image-data). Клиент
  создаётся лениво и переиспользуется; на headless-Linux без X11/Wayland конструктор
  может упасть — тогда показываем ошибку, не паникуем. Контракт:
  `ChatListAction::Copy → ChatIntent::CopyChat → AppCommand::CopyChat`.
- **Подтверждение/ошибка** идут в ту же область статуса оверлея, что и ошибки
  авто-названия (`ChatListState.notice` — успех зелёным `✓`, `error` — красным `⚠`,
  взаимоисключимы, гаснут по первому нажатию клавиши). Оверлей при копировании **не
  закрывается**. `ChatScreen::set_overlay_notice` направляет в оверлей, если открыт,
  иначе — заметкой в ленту. Добавлено в оверлей помощи (`F1`/`?`).

### Пост-M9: флаг `--no-mmap` + подсказки полей в настройках (сделано)
- **Настройка `--no-mmap` для managed-сервера**: новое поле `EngineSettings.no_mmap`
  (`shared/config.rs`, `#[serde(default)]` → старые `settings.json` без миграции) →
  `ManagedConfig.no_mmap` (`shared/api/server.rs`); `build_args` добавляет
  `--no-mmap` только когда включён. Прокинуто супервайзером (`app/supervisor.rs`,
  `managed_config`); тумблер в секции «Модель/сервер» экрана настроек. Грузит веса
  модели целиком в RAM вместо mmap — помогает на сетевых/медленных дисках, требует
  больше памяти (по умолчанию выключен).
- **Подсказки-описания полей настроек**: `screens/settings.rs::field_description(id)`
  возвращает человекопонятный текст для поля; `render_fields` показывает его строкой
  внизу секции (верхняя разделительная линия, `dim`, перенос по словам) при фокусе на
  поле — место резервируется **только когда у поля есть описание** (прочие секции как
  раньше). Сейчас описаны `-ngl` (GPU-слои), `--jinja`, `--no-mmap`; расширяется одной
  веткой в `field_description`.

### Пост-M9: загрузка/удаление файлов в RAG командами `/rag add|remove` (сделано)
- **Команды в поле ввода**: `/rag add <путь>` индексирует файл или директорию в базу
  знаний активного профиля; `/rag add <путь> -r` (или `--recursive`) — рекурсивный
  обход; `/rag remove <путь>` удаляет из базы файл или директорию (со всем, что под
  ней). **Слово `delete` намеренно не поддерживается** (пользователи опасались, что
  оно сотрёт сам файл на диске) — только `remove`. Пока поддержаны только
  `*.txt`/`*.md`. Разбор — чистая
  логика `features/rag_command.rs::parse` (регистронезависимо, путь с пробелами и
  кавычками, флаг в любом месте; общий `extract_path`). Экран (`screens/chat.rs`) на
  `Enter` сперва пробует разобрать команду: распознанная → `ChatIntent::RagAdd { path,
  recursive }` / `ChatIntent::RagDelete { path }` (не уходит сообщением, работает и во
  время генерации — операции фоновые/быстрые); ошибка синтаксиса → заметка-подсказка
  в ленту; иначе обычная отправка.
- **Каноничный ключ источника + идемпотентность**: `source` в БД — каноничный путь
  (`rag_ingest::canonical_source` = `fs::canonicalize` без вербатим-префикса `\\?\`),
  единый при добавлении-файлом, добавлении-папкой и удалении (устойчив к регистру/
  разделителю/относительности). **Повторное `/rag add` того же файла заменяет** его
  прежние чанки, а не плодит дубликаты: `index_file` перед вставкой зовёт
  `Db::rag_delete_by_source` (точное совпадение source).
- **Удаление** — `Db::rag_delete_under(profile_id, path)`: сносит документы по точному
  пути и всё под ним (директория), сравнение в `norm_path` устойчиво к `/`↔`\`,
  хвостовому слэшу и регистру (Windows); **не требует наличия файла на диске** (можно
  чистить записи уже удалённых файлов). Удаляет и из `rag_documents`, и из `vec0`
  (`rag_vectors`) по rowid, строго в рамках профиля (изоляция). Оркестратор
  (`handle_rag_delete`) выполняет это **на месте** (только БД, без эмбеддинга): если
  путь есть на диске — каноничный ключ, иначе — введённая строка (нормализует БД);
  результат — `RagProgress::Removed { chunks }` (0 → «ничего не найдено»).
- **Сканирование** — `features/rag_ingest.rs::scan` (std::fs, тестируемо на tempdir):
  файл-путь → он сам при поддержанном расширении; директория → все поддержанные файлы
  (рекурсивно при флаге), результат отсортирован; ошибки вложенных директорий
  пропускаются, недоступность корня — ошибка. `read_text` отбрасывает UTF-8 BOM.
- **Фоновая индексация** — `app/orchestrator/rag.rs::spawn_rag_ingest` (отдельная
  tokio-задача, не блокирует оркестратор): scan → предпроверка эмбеддера (быстрый
  отказ, если RAG не настроен) → по файлу читает/чанкует (переиспользует
  `features::tools::rag::chunk_text`, теперь `pub(crate)`)/эмбеддит/пишет с
  **изоляцией по `profile_id`** (профиль берётся из активного чата). Отменяемо
  (`rag_cancel: CancellationToken` в оркестраторе): новая индексация отменяет
  предыдущую, `Quit` — текущую. `Storage` потокобезопасен (внутренний мьютекс),
  эмбеддинг асинхронен.
- **Прогресс + спиннер**: тип `RagProgress` (`Started/Indexing/Finished/Failed`)
  определён в `features/rag_ingest.rs` — им пользуются и `app` (эмитит
  `AppEvent::RagProgress`), и `screens` (рисует), без нарушения FSD. Задача шлёт
  прогресс через `evt_tx` напрямую (без внутреннего канала — состояние `Chat` не
  трогается). `ChatScreen::set_rag_progress` ведёт баннер-строку между лентой и вводом
  (`найдено файлов: N` → `индексация file.md из dir (i/total)`) со спиннером
  (`⠋⠙⠹…`); завершение/ошибка гасят баннер и оставляют итог заметкой в ленте. Петля
  `app/runtime.rs` перерисовывает каждый тик, пока `screen.is_rag_active()` (анимация
  спиннера) — вне индексации простаивающие тики не рисуют (флаг `dirty`, см. фикс
  мигания курсора). Контракт: `AppCommand::RagAdd → handle_rag_add`. Команда добавлена
  в оверлей помощи (`F1`/`?`).
- **Подсветка команд + без орфографии**: если ввод распознаётся как команда
  (`rag_command::parse(...).is_some()`), поле ввода целиком красится цветом `warning`
  (жёлтый), а спелл-чек к нему не применяется (команды и пути файлов не слова).
  `InputBox::render` принимает флаг `command`; `ChatScreen::input_is_command` его
  вычисляет, а `maybe_recheck_spelling` для команд снимает подчёркивания и не считает.
- **Задел**: другие форматы (pdf/docx/html), извлечение читаемого текста, `/rag list`
  (показ источников), прогресс по чанкам — на будущее.

### Пост-M9: умный чанкинг RAG (перекрытие + markdown) + склейка при извлечении (сделано)
- **Чанкер переписан** (`features/tools/rag.rs::chunk_text`): вместо «абзацы +
  жёсткие окна по 800 симв.» (рвущие слова, без перекрытия, плодящие крошечные
  чанки из одной строки) — упаковщик по best practices RAG. Текст сегментируется на
  атомарные юниты (`segment_units`: абзац целиком, если влезает в цель; иначе —
  предложения `split_sentences`; слишком длинные — окна по словам `break_long`, а
  одиночное гигантское слово — по символам), затем `pack_units` пакует юниты в чанки
  до `CHUNK_TARGET_CHARS=800`, начиная каждый следующий с **хвоста предыдущего**
  (перекрытие ≤ `CHUNK_OVERLAP_CHARS=150`). Мелкие соседние абзацы группируются в
  один чанк; границы — по словам/предложениям (не середина слова); `CHUNK_MAX_CHARS=
  1200` — потолок неделимого прогона. Длины считаются в **символах** (`clen`), не
  байтах (кириллица). `rag_add` и фоновая индексация используют его же.
- **Семантический чанкинг markdown** (`chunk_markdown`, для `.md`-файлов): режет по
  ATX-заголовкам (`split_sections`, `is_atx_heading`), **защищает огороженные блоки
  кода** (``` и `~~~` — `#` внутри них не заголовок), к каждому чанку секции
  добавляет её заголовок как **смысловой якорь** (улучшает извлечение). Внутри секции
  — тот же `pack_units` с перекрытием. Документ без заголовков → обычный `chunk_text`.
  Диспетчеризация по расширению — в `orchestrator::rag::index_file` (md → `chunk_markdown`,
  иначе `chunk_text`); инструмент `rag_add` (текст без формата) — всегда `chunk_text`.
- **Склейка при извлечении** (`stitch_hits`, вызывается из `RagSearch::invoke`): так
  как перекрытие заложено при чанкинге, соседние чанки одного источника имеют
  **дословно совпадающий** хвост/начало. `stitch_hits` группирует хиты по источнику и
  итеративно склеивает любые две части с реальным перекрытием (`merge_overlap` →
  `overlap_len` ищет наибольший суффикс `a` = префикс `b`, ≥ `MIN_STITCH_OVERLAP=24`
  симв., чтобы не словить случайность) в один связный фрагмент **без дубля** — экономит
  контекст и не путает модель повтором. Учитывается повторный markdown-заголовок в
  начале второго чанка (снимается перед сопоставлением, не дублируется). Фрагменты
  упорядочены по лучшему (минимальному) расстоянию. **Без изменения схемы БД/сущностей**
  (намеренно не вводили `seq`-колонку): адъяцентность определяется по самому тексту
  перекрытия — минимальная правка, дающая ровно тот эффект, что нужен этому проекту.
- **Задел**: ранкинг/дедуп между источниками, конфигурируемые размеры чанка/перекрытия.

### Пост-M9: managed embedding-сервер — поднят физический батч (фикс «фрагментов: 0») (сделано)
- **Симптом**: индексация крупного файла (напр. `spec.md`) завершалась с «фрагментов:
  0, с ошибками: 1»; в логах — `RAG: не удалось проиндексировать файл … error=
  embeddings request returned an error status`.
- **Причина**: эмбеддинг-модели **non-causal** — весь вход должен поместиться в ОДИН
  физический батч (`n_ubatch`). По умолчанию `n_ubatch=512`, и llama-server
  приравнивает к нему `n_batch` (в логах: *«setting n_batch = n_ubatch = 512»*).
  Поэтому **любой чанк длиннее ~512 токенов отвергался целым запросом** («input is too
  large to process. increase the physical batch size»). Для кириллицы/кода 512 токенов
  — это лишь сотни символов, так что чанки `chunk_text`/`chunk_markdown` (~800–1200
  симв.) регулярно не лезли. Весь батч файла шёл одним запросом → одна большая ошибка.
- **Фикс**: managed embedding-сервер запускается с `-ub <ctx> -b <ctx>`
  (`server.rs::build_args` при `embeddings=true`, размер = `context_size`, у эмбеддера
  `DEFAULT_CONTEXT_SIZE`=8192) — физический и логический батч подняты до контекста, и
  входной чанк целиком помещается в один ubatch. Чат-серверу эти флаги не добавляются.
  Память на эмбеддинг-моделях (bge-m3 ~1.3 ГБ) растёт незначительно. **Требует
  перезапуска managed-сервера** (происходит при старте/смене настроек эмбеддера).
- **Диагностика**: `OpenAiClient::embed` больше **не глотает тело ошибки** (как и
  `chat_stream` ранее) — статус + причина из JSON логируются и попадают в текст ошибки
  (обрезка 500 симв.). Именно это (и лог llama-server) указало на корневую причину.
- **Внешний эмбеддер** (`MINDFORK_EMBED_URL`): флагами батча мы не управляем — если
  чужой сервер поднят с маленьким `n_ubatch`, крупный чанк так же отвергнется; теперь
  это видно по тексту ошибки (а не «фрагментов: 0»). Чанки ограничены `CHUNK_MAX_CHARS`
  (≤ ~1200 токенов worst-case), так что контекст эмбеддера (8192) они не превышают.

### Пост-M9: интуитивные `Esc`/`Ctrl+C` для списка чатов и выхода (сделано)
- **`Esc` переключает «список чатов ↔ текущий чат»**: в основном виде (когда не идёт
  генерация) `Esc` открывает полноэкранный оверлей списка чатов; `Esc` внутри оверлея
  закрывает его обратно. Во время генерации `Esc` сперва **отменяет её** (как раньше).
  Реализация: ветка `Esc` в `screens/chat.rs::handle_key` вместо `Quit` теперь ставит
  `self.overlay = Some(ChatListState::new(...))`; закрытие — штатный
  `ChatListAction::Close`.
- **`Ctrl+C` — выход из приложения и из чата, и из оверлея списка**: в чате уже был
  `ChatIntent::Quit`; в оверлее добавлен `ChatListAction::Quit` (ветка `'c'` в
  `chat_list.rs::on_key_search`, раскладко-независимо через `keys::physical_char`),
  `chat.rs::handle_overlay_key` маппит его в `ChatIntent::Quit`.
- **`Ctrl+L` убран** (его роль — открыть список — взял `Esc`). Обновлены оверлей
  помощи (`HELP_KEYS`), подсказка статус-бара, help-строка оверлея и доки
  (README/spec/install). Тесты переписаны на `Esc`-открытие оверлея.

### Пост-M9: быстрая многострочная вставка из буфера (сделано)
- **Симптом**: большая вставка из буфера обмена тормозила в Windows Terminal, а
  перенос строки в ней трактовался как `Enter` (= отправка сообщения). **Причина**:
  терминал слал вставку **посимвольно** как обычные нажатия, поэтому петля `run_loop`
  делала по `terminal.draw` на каждый символ (медленно), а `\n`/`\r` приходили как
  `KeyCode::Enter` → отправка.
- **Ключевой нюанс — bracketed paste на Windows НЕ работает**: `Event::Paste` у
  crossterm 0.29 эмитит **только unix-парсер** (`src/event/sys/unix/parse.rs`); на
  Windows ввод читается через Console API (`ReadConsoleInput`), и режим bracketed
  paste (`ESC[?2004h`) бесполезен — события вставки приходят обычными `KeyEvent`
  (причём перемежаются `KeyEventKind::Release`). Поэтому одного `EnableBracketedPaste`
  мало — нужен батчинг в петле.
- **Фикс — батчинг событий в `run_loop`** (`app/runtime.rs`): за один тик дренируем
  **все** доступные события (`event::poll(Duration::ZERO)` в цикле), затем
  `process_input_batch` коалесит подряд идущие «текстовые» клавиши во вставку. Это
  чинит обе беды: одна перерисовка на пачку (не на символ) и `Enter` **внутри серии**
  → перевод строки, а не отправка.
  - `collect_press` отбрасывает key-события «отпускания»/повтора (приложение их и так
    игнорирует, а они разрывали бы серию символов на Windows).
  - `paste_char`: текстовая клавиша без Ctrl/Alt → символ (`Enter→'\r'`, `Tab→'\t'`,
    `Char(c)→c`); прочее (стрелки, шорткаты) разрывает серию.
  - `chunk_batch` (чистая, тестируемая): серия текстовых клавиш длиной **≥2** →
    `Chunk::Paste(String)`; серия из 1 (обычный ввод / одиночный `Enter`) — обычное
    событие, так что `Enter`-отправка не ломается. Человек физически не наберёт 2+
    клавиши в один zero-timeout-дренаж — так что ≥2 надёжно означает вставку.
  - На unix `EnableBracketedPaste` оставлен (под `#[cfg(unix)]`) — там приходит чистый
    `Event::Paste`, который тот же `process_input_batch` обрабатывает как `Chunk`.
- **Сбор «хвоста» вставки через стыки консольных порций** (`PASTE_BURST`/`PASTE_GAP`):
  крупная вставка (тысячи символов) приходит в консольный буфер Windows **несколькими
  порциями**, и петля дренирует их за разные итерации. На стыке порций серия
  символов рвалась → если порция обрывалась ровно на одиночном `Enter`, он уезжал
  отправкой (баг: ~6900 символов прошли вставкой, затем один `Enter` сработал как
  отправка). Фикс: если за один zero-timeout-дренаж набрался всплеск (`batch.len() ≥
  PASTE_BURST=2`), добираем хвост вставки `while event::poll(PASTE_GAP=20мс)` — пока
  события идут с зазором < 20мс, считаем их одной вставкой (реальный ввод человека
  имеет паузы >>20мс). Так вся вставка собирается в ОДНУ пачку без внутренних стыков,
  и одиночных `Enter` на границах больше не возникает.
- **`InputBox::insert_str(&str)`** (`widgets/input_box.rs`): вставка в позицию курсора
  одним проходом (хвост строки отрезается, текст бьётся по `\n` на логические строки,
  хвост приклеивается к последней, курсор — в конец вставленного). `normalize_paste`:
  `\r\n`/`\r` → `\n` (так CRLF из буфера — хоть `Enter`+`\n`, хоть `\r\n` — схлопывается
  в один перевод), `\t` → 4 пробела. Без посимвольной петли → большая вставка не тормозит.
- **Маршрутизация**: вставка (коалесированная или unix-`Paste`) → на экране настроек в
  активный редактор поля (`SettingsScreen::handle_paste` → `editor.input.insert_str`,
  полезно для пути к модели), иначе — `ChatScreen::handle_paste` → поле ввода чата. В
  чате уважает модальность (справка/попап подсказок/оверлеи с однострочными полями →
  no-op) и **никогда не отправляет** сообщение, даже с переносами внутри;
  `mark_input_changed` запускает дебаунс орфографии. Добавлено в оверлей помощи
  (`F1`/`?`, `Ctrl+V`).

### Пост-M9: сохранение черновика поля ввода в файле чата (сделано)
- **Несохранённый текст поля ввода хранится в чате и восстанавливается при
  переключении**: новое поле `Chat.draft` (`entities/chat.rs`, `#[serde(default)]` →
  старые файлы чатов читаются без миграции). У нового чата пустой; при отправке
  очищается. Переключение/создание чата загружает его черновик в поле ввода (новый →
  пустое).
- **Контракт**: команда `AppCommand::SetDraft(String)` (UI → оркестратор) и поле
  `draft` в событии `AppEvent::ChatActivated`. Оркестратор — единственный писатель
  `Chat`: `handle_set_draft` пишет черновик активного чата и помечает грязным
  (`mark_dirty`, запись на диск с дебаунсом 800мс), **не трогая `modified_at`** (правка
  черновика не должна поднимать чат в списке); `handle_send` очищает `chat.draft` вместе
  с добавлением сообщения пользователя.
- **UI**: `ChatScreen::mark_input_changed` (вызывается при любой правке ввода — набор,
  вставка, восстановление, подсказки) теперь поднимает и `draft_dirty`. Петля
  `app/runtime.rs` каждый тик забирает изменённый черновик (`take_dirty_draft`) и шлёт
  `SetDraft` (без перерисовки — вид не меняется). `activate_chat` грузит `draft` в поле
  ввода через `set_text`, но **намеренно не помечает `draft_dirty`** (иначе тут же
  отправили бы тот же текст обратно тем же `SetDraft`) — перепроверку орфографии при
  этом запускает напрямую. См. spec §11.7.

### Пост-M9: сохранение удалённых обменов в файле чата (`Ctrl+E`/`Ctrl+R`) (сделано)
- **Удаление обмена (`Ctrl+E`) и перегенерация (`Ctrl+R`) необратимы в UI, но
  удалённое сохраняется на диске** ради **ручного** восстановления правкой JSON в
  редких случаях. Новый тип `DeletedExchange { deleted_at, messages, draft }` и поле
  `Chat.deleted: Vec<DeletedExchange>` (`entities/chat.rs`, `#[serde(default,
  skip_serializing_if = "Vec::is_empty")]` → старые файлы без миграции, пустая
  коллекция не засоряет JSON). Это **не** сообщение, а контейнер: удалённые
  сообщения + черновик поля ввода на момент удаления + дата.
- **Что попадает**: `Ctrl+E` (`handle_delete_last`) — сообщение пользователя и ответ
  ассистента (`split_off(idx)`) + `chat.draft` **до** возврата текста пользователя в
  поле; `Ctrl+R` (`handle_regenerate`) — ответ ассистента и tool-сообщения раунда
  (`split_off(idx+1)`) + `chat.draft`. Чистый метод `Chat::record_deleted(messages,
  draft)` проставляет `deleted_at = Utc::now()`, пустой набор игнорирует и вставляет
  запись в **начало** коллекции (свежие удаления искать быстрее), не трогая
  `modified_at` (его обновляют сами операции усечения). Восстановления через UI нет.
  См. spec §11.7.

### Пост-M9: навигация `↑/↓` по визуальным рядам перенесённой строки (сделано)
- **Стрелки `↑/↓` в поле ввода ходят по визуальным рядам**, а не по логическим
  строкам: если длинная строка перенесена на несколько рядов, `↑/↓` перемещают
  курсор между её частями (раньше прыгали через всю логическую строку на соседнюю).
  Колонка по возможности сохраняется. **Причина бага**: `move_up`/`move_down`
  (`widgets/input_box.rs`) работали по `self.row` (логическая строка), а перенос в
  визуальные ряды (`wrap::wrap_ranges`) считается только при рендере по ширине
  `view_w` — у `on_key` ширины нет.
- **Фикс**: виджет кэширует ширину последней отрисовки (`InputBox.last_width`,
  выставляется в `render`); `move_up`/`move_down` строят визуальные ряды
  (`visual_rows`) на этой ширине, берут визуальную позицию курсора (`cursor_visual`)
  и переносят её на соседний ряд через `col_for_visual` (ближайшая колонка по
  ширине в колонках, с откатом на символ назад на мягком переносе — иначе позиция
  `== end` уехала бы в начало следующего ряда, `is_soft`). До первого рендера
  (`last_width == 0`) — фолбэк на логический переход (`move_up_logical`/
  `move_down_logical`). `↑` на верхнем визуальном ряду / `↓` на нижнем — no-op.
  Рендер в петле идёт перед обработкой клавиш, так что ширина всегда актуальна.
- **`Home`/`End` — по визуальному ряду**: `Home` → начало текущего визуального ряда,
  `End` → его конец (`move_home`/`move_end`). На мягком переносе `End` встаёт на
  последнюю позицию ряда (через тот же `col_for_visual`/`is_soft`), не уезжая в начало
  следующего. До первого рендера — на всю логическую строку (фолбэк).
- **Goal-column**: серия `↑/↓` держит исходную колонку (`InputBox.goal_col`,
  в колонках) при проходе через короткие ряды — как в больших редакторах.
  Запоминается при первом вертикальном переходе, сбрасывается **любым** иным сдвигом
  курсора/правкой (`move_left`/`move_right`/`move_home`/`move_end`, `insert_char`/
  `insert_newline`/`insert_str`/`backspace`/`delete`, `replace_range`/`set_text`/
  `clear` — сброс в самих методах, чтобы покрыть и пути мимо `on_key`: вставку,
  подсказки, загрузку черновика). См. spec §11.5.

### Пост-M9: однострочные поля настроек + попап системного сообщения (сделано)
- **Симптом**: редактируемые поля экрана настроек выглядели однострочными, но под
  ними жил многострочный `InputBox` с переносом по словам: длинное значение (путь к
  GGUF, URL) заворачивалось на **невидимый** ряд (попап высотой 3 → видна 1 строка),
  а `↑/↓` ходили по визуальным рядам и `Home/End` — по визуальному ряду, а не по
  всему значению (см. пост-M9 навигацию по визуальным рядам — она и «ломала» здесь
  однострочное поведение).
- **Однострочный режим `InputBox`** (`widgets/input_box.rs`): флаг `single_line` +
  `set_single_line()` (по умолчанию выключен — чат-ввод как был, многострочный). В
  этом режиме значение **не переносится**, а скроллится **горизонтально** (`hscroll`,
  отдельный путь рендера `render_single_line` + хелпер `col_at_width`): курсор всегда
  виден, длинное значение «уезжает» влево. `↑/↓` — no-op; `Home/End` — к началу/концу
  **всего значения**; перевод строки (`insert_newline`) запрещён; переводы строк во
  `set_text`/`insert_str` (вставка из буфера) схлопываются в пробел (инвариант «одна
  логическая строка»). `clear`/`set_text` сбрасывают `hscroll`.
- **Экран настроек** (`screens/settings.rs`): редактор поля открывается в
  однострочном режиме для всех текстовых полей; **исключение — системное сообщение
  профиля** (`FieldId::PSystem`) — многострочный по смыслу. Для него редактор
  многострочный (`set_single_line(false)`), в **крупном попапе** (~80% ширины, ~60%
  высоты — `multiline_popup_height`) с переносом длинных строк; `Shift+Enter`
  вставляет перевод строки, `Enter` коммитит, `Esc` отменяет (как в чат-вводе).
  `Editor.multiline` ведёт ветвление клавиш/рендера. См. spec §11.6.

### Пост-M9: имперсонация — написание сообщения за пользователя (`Ctrl+U`) (сделано)
- **Суть** (spec §11.8): по `Ctrl+U` модель пишет следующее сообщение **от лица
  пользователя** в поле ввода. Запрос строится так: системное сообщение чата
  (персона ассистента) заменяется на **имперсонационное** из профиля
  (`Profile.impersonation_system_message`; пусто → `DEFAULT_IMPERSONATION_SYSTEM_
  MESSAGE`), роли user↔assistant в истории меняются местами (`swap_role_message`;
  system/tool/пустые отбрасываются — инструментов в режиме нет), семплинг —
  `AppConfig.impersonation_sampling`. Чистые `build_impersonation_request`/
  `swap_role_message` в `app/orchestrator/impersonation.rs`.
- **Reasoning принудительно выключен** (важно): `build_impersonation_request`
  навязывает `thinking=false`, `reasoning_effort=None` и ключевое `reasoning_budget=0`
  поверх пользовательского `impersonation_sampling`. Имперсонация отбрасывает «мысли»
  (`Thoughts` в `spawn_impersonation` игнорируются), поэтому reasoning ей не нужен.
  У моделей со «вшитым» в шаблон thinking (Gemma `peg-gemma4`, Qwen) поля
  `thinking`/`reasoning_effort` сервер игнорирует — только `reasoning_budget=0`
  (+ `chat_template_kwargs.enable_thinking=false` в `wire.rs`) реально гасит «мысли».
  Без этого модель тратила весь бюджет токенов на `reasoning_content`, а ответный
  `content` приходил пустым → предпросмотр оставался пустым (тот же класс бага, что
  у авто-названия чата, см. `title.rs`).
- **Три режима сервера** (`ImpersonationMode` в `shared/config.rs`):
  `shared` (по умолчанию) — тот же chat-сервер, что у ассистента, но с семплингом
  имперсонации (отдельный сервер не поднимается); `managed` — отдельный дочерний
  `llama-server`; `external` — отдельный удалённый сервер. Настройки —
  `AppConfig.impersonation_engine: ImpersonationEngineSettings` (поля как у
  `EngineSettings`, но режим трёхзначный). Супервайзер: `ServerSupervisor::
  apply_impersonation` (real поднимает managed/external + probe; для `shared` не
  вызывается — оркестратор берёт `backend` ассистента). Оркестратор держит
  `imp_backend`/`imp_handle`/`imp_status` и пере-поднимает при смене
  `impersonation_engine` (`apply_impersonation_settings`).
- **Поток** (фоновая задача, как авто-название): `AppCommand::Impersonate { seed }`
  → `handle_impersonate` (гейт `State::Idle` + нет активной имперсонации + готовность
  сервера через `impersonation_backend_if_ready`) → `spawn_impersonation` стримит
  `AppEvent::ImpersonationChunk`, по концу/таймауту/отмене шлёт `(id, reason)` во
  внутренний канал `imp_done` → `handle_imp_done` эмитит `ImpersonationFinished`.
  `AppCommand::CancelImpersonation` (`Esc`) отменяет токен; `Quit` тоже.
  `IMPERSONATION_TIMEOUT=600с` (страховка от зависшей задачи; щедрый — на медленном
  локальном `llama-server` только обработка промпта может занять ~минуту, генерация
  на CPU ~5 ток/с). «Мысли»/tool-вызовы в поле ввода игнорируются.
- **Таймаут ≠ отмена пользователем** (важно, чинит «реплика обрывается и исчезает»):
  отмена по `Esc` приходит как `Ok(Ok(Cancelled))` (поток внутри `run` сам ловит
  `cancel.cancelled()` и отдаёт `Finished(Cancelled)`) и отбрасывает текст; таймаут —
  это ветка `Err(_)`: серверную задачу прерываем (`cancel.cancel()`), но накопленный
  текст **сохраняем**, отдавая как `Length`. Прежний безусловный
  `if cancel.is_cancelled() { Cancelled }` сводил оба случая к отбрасыванию — из-за
  этого обрезанная таймаутом реплика исчезала.
- **UI** (`screens/chat.rs` + `widgets/impersonation_preview.rs`): на время написания
  поле ввода **скрыто**, на его месте — нередактируемый **потоковый предпросмотр** со
  спиннером (`ImpersonationState`: text начинается с уже введённого текста-затравки).
  Накопленный текст вставляется в поле ввода (`set_text`) при любом завершении,
  **кроме явной отмены** (`Cancelled` — `Esc`): тогда поле сохраняет затравку.
  Обрезанная таймаутом (`Length`) или прерванная ошибкой потока (`Error`) реплика
  **не теряется** — неполный текст полезнее пустого поля. Во время имперсонации
  обрабатываются только `Esc` (отмена) и `Ctrl+C` (выход). Петля `app/runtime.rs`
  перерисовывает каждый тик, пока `is_impersonating()` (анимация спиннера). `Ctrl+U`
  раскладко-независим (`shared/keys`), добавлен в оверлей помощи (`F1`/`?`).
- **Продолжение начатого**: если поле ввода непустое, текст уходит как `seed` —
  `build_impersonation_request` добавляет в системное сообщение просьбу продолжить
  начатое (вывести только продолжение), а предпросмотр показывает затравку +
  сгенерированное (итог = затравка + продолжение).
- **Настройки** (`screens/settings.rs`): секции «Модель/сервер», «Семплинг»,
  «Профили» получили **селектор подсекции** «Ассистент»/«Имперсонация» (`Subsection`,
  поля `model_sub`/`sampling_sub`/`profile_sub`; новый `FieldId::*Sub`). Подсекция
  имперсонации Модели — те же поля + трёхзначный режим (`IxMode`, `cycle_imp_mode`);
  Семплинга — те же поля над `impersonation_sampling`; Профиля — **только системное
  сообщение** (`PImpSystem`, многострочный редактор), без инструментов. `ProfileEdit.
  impersonation_system_message` переносит правку.

### Пост-M9: инлайн tool-блоки в ленте + перенос текста вызова (сделано)
- **Tool-блоки рисуются на месте вызова, а не в «шапке»** (`widgets/message_feed.rs`):
  у `FeedToolCall` появилось поле `text_offset` (байтовое смещение в `FeedMessage::
  text` — сколько текста ответа было сгенерировано ДО вызова). `build_lines` для
  ассистента (`push_assistant_body`) делит текст по `text_offset` вызовов на
  markdown-фрагменты и вставляет tool-блок между ними: `текст-до-вызова → 🔧 блок →
  текст-после`. Раньше `push_tools` шёл ПЕРЕД телом — все вызовы висели над ответом.
- **Аргументы и результат вызова переносятся, а не обрезаются**: убраны `truncate`/
  `.take(6)`; `push_tool` через новый `push_wrapped` (гуттер-префиксы `  🔧 `/`  │ `,
  перенос по визуальной ширине через `shared::wrap`, выравнивание продолжений)
  раскладывает длинные `arguments`/`result` на несколько рядов целиком. Лента и так
  делает второй проход `wrap::wrap_line`, но строки уже ≤ ширины → no-op.
- **Склейка раундов agentic-loop** (`FeedMessage::from_messages`): при перезагрузке
  чата подряд идущие assistant-сообщения раундов (между ними в истории tool-
  сообщения — отбрасываются) сливаются в один блок «Ассистент:» с инлайн-вызовами;
  `text_offset` каждого раунда сдвигается на накопленную длину.
  `screens/chat.rs::push_tool_call` фиксирует `text_offset = last.text.len()`;
  `activate_chat` строит ленту через `from_messages`.
- **Live-стрим байт-в-байт совпадает с перезагрузкой**: тексты раундов в live-режиме
  раньше конкатенировались без разделителя, а `from_messages` вставляет `\n\n`
  (мысли — `\n`). Флаги `pending_text_sep`/`pending_thoughts_sep` в `ChatScreen`
  взводятся при вызове инструмента и потребляются первым же текстовым/мысле-чанком
  следующего раунда, вставляя тот же разделитель (если накопленный текст непуст).
  Сбрасываются в `begin_generation`. Покрыто тестом
  `live_stream_with_tool_matches_reload` (live == `from_messages`).

### Пост-M9: семплинг «для разнообразия» — dynatemp / adaptive-p / DRY-брейкеры / порядок семплеров (сделано)
- **Четыре новых поля `SamplingConfig`** (расширения llama.cpp в теле запроса, для
  более живых и непредсказуемых ответов): **динамическая температура**
  `dynatemp_range`/`dynatemp_exponent` (температура подстраивается по энтропии
  распределения на каждом токене), **adaptive-p** `adaptive_target`/`adaptive_decay`
  (новый семплер, llama.cpp PR #17927), **DRY-брейкеры** `dry_sequence_breakers`
  (`Option<Vec<String>>`) и настраиваемый **порядок семплеров** `samplers`
  (`Option<Vec<String>>`). Каждое поле `Option`, шлётся только когда задано.
- **Списочные поля — JSON-массивы, пустыми не шлются** (`wire.rs::build_chat_request`,
  хелпер `non_empty`): пустой `samplers` сервер истолковал бы как «отключить все
  семплеры», а пустой `dry_sequence_breakers` **llama-server прямо отвергает**
  (`server-schema.cpp:238` бросает `"must be a non-empty array of strings"`) — фильтр
  это предотвращает.
- **Ключи сверены по исходникам** llama.cpp (`tools/server/server-schema.cpp`, куда в
  свежих версиях переехал разбор тела из `server.cpp`): `dynatemp_range`/`_exponent`
  (:113), `adaptive_target` (:160, плоский ключ, `≤1.0`, negative=выкл),
  `adaptive_decay` (:164, hard-диапазон `0.0–0.99`), `dry_sequence_breakers` (:234),
  `samplers` (:474, массив имён или строка). Валидные имена семплеров
  (`sampling.cpp`): `penalties`, `dry`, `top_k`, `top_p`, `top_n_sigma`, `typ_p`,
  `min_p`, `xtc`, `temperature`, `infill` (mirostat — отдельный режим, не в списке).
- **UI** (`screens/settings.rs`, секция «Семплинг»): числовые поля как обычно;
  `samplers` правится текстом через «;», `dry_sequence_breakers` — через «,» с
  эскейпами `\n`/`\t`/`\r` (однострочный редактор не даёт ввести их буквально;
  `parse_breakers`/`join_breakers`/`decode_escapes`/`encode_escapes`). Также правится
  инструментом `set_sampling` (схема + merge всех полей).
- **Тесты**: wire-сериализация новых полей + омит пустых массивов
  (`list_fields_sent_as_arrays_and_empty_omitted`), round-trip списков и эскейпов
  (`samplers_list_round_trip`, `dry_breakers_decode_and_encode_escapes`), merge в
  `set_sampling`. **Живой `#[ignore]`-смоук** `accepts_creative_sampling_extensions`
  (`client.rs`) — все четыре поля в одном запросе; проверяет **объединённый** поток
  `text`+`thoughts` (reasoning-модель Gemma кладёт ответ в `reasoning_content` —
  ловушка reasoning-бюджета). Проверено на живом `llama-server` (Gemma 4 12B).

### Пост-M9: очистка поля ввода с отменой (`Ctrl+K`) (сделано)
- **`Ctrl+K` удаляет весь текст поля ввода**; повторное нажатие **возвращает
  удалённое**, если после удаления ничего не вводилось (toggle). Логика — в самом
  виджете `InputBox` (`widgets/input_box.rs::clear_or_restore`): буфер
  `cleared: Option<String>` хранит удалённый текст; непустое поле → запоминаем и
  чистим, пустое поле с живым буфером → восстанавливаем (курсор в конец). **Любой
  ввод текста** (`insert_char`/`insert_newline`/`insert_str`/`replace_range`/
  `set_text`, а также явный `clear()` при отправке) инвалидирует буфер в `None`,
  поэтому восстановить можно лишь сразу после удаления; движения курсора и no-op
  backspace/delete по пустому полю буфер не трогают (восстановление переживает их).
- **Проводка**: `screens/chat.rs::handle_key` — ветка `'k'` в Ctrl-матче
  (раскладко-независимо через `shared/keys`, физ. K = `Ctrl+л`) зовёт
  `input.clear_or_restore()` + `mark_input_changed()` (сохранение черновика +
  перепроверка орфографии). Команды оркестратору не нужны (чисто локальное
  состояние ввода). Добавлено в оверлей помощи (`F1`/`?`) и таблицу клавиш spec
  §11.5.

### Пост-M9: автомат генерации выделен из оркестратора (`GenState`) (сделано)
- **Состояние генерации вынесено** из `app/orchestrator.rs` в отдельный модуль
  `app/gen_state.rs` — тип `GenState` (`Idle`/`Generating{id,cancel}`/
  `Cancelling{id}`). Выбор реализации — **простой enum с методами-переходами**, не
  `statig`/`rust-fsm`: состояния носят данные (`Uuid` + `CancellationToken`), что
  плохо ложится на эти фреймворки (rust-fsm — для бесстатусных FSM; statig — HSM-
  оверкилл на 3 состояния), а переходы переплетены с side-effect'ами (spawn задачи,
  отмена токена, эмит событий, запись в `Chat`), которые должны оставаться у
  оркестратора — владельца `Chat`. Зависимостей не добавляли.
- **Переходы инкапсулированы методами**, валидными по построению: `begin(id,cancel)`
  (`Idle→Generating`, no-op если занят), `request_cancel()` (`Generating→Cancelling`,
  возвращает токен — отмену дёргает оркестратор), `finish(id)` (`→Idle` только при
  совпадении `id`, анти-устаревание `Stop→Send`), `active_cancel()` (токен для `Quit`
  без перехода), `current_id()`/`is_idle()`. Тип **чистый** (без I/O) →
  юнит-тестируется без tokio-рантайма (5 тестов в `gen_state.rs`).
- **Оркестратор стал тоньше**: инвариант перехода больше не размазан по `match`-
  сайтам (`handle_send`/`handle_regenerate`/`handle_delete_last`/`handle_impersonate`
  гейтят через `is_idle()`; `Cancel`/`handle_switch` — `request_cancel()`; `Quit` —
  `active_cancel()`; `start_generation` — `begin()`; `handle_done` — `finish()`).
  Поле `state` переименовано в `gen_state` (`gen` — зарезервированное слово edition
  2024). Имперсонация (`imp_gen`) и RAG (`rag_cancel`) **сознательно** не влиты —
  это конкурентные подсостояния (RAG идёт параллельно генерации).

### Пост-M9: god-объект оркестратора расслоён по фичам (сделано)
- **`app/orchestrator.rs` (≈1.8k строк кода + ≈1.5k тестов) разбит на каталог
  `app/orchestrator/`** — чисто механический рефактор (Фазы 1+2), без изменения
  структуры/полей `Orchestrator` и без новых каналов: **инвариант «единственный
  владелец `Chat`» сохранён** (см. architecture.md §1, §10). Наружу из модуля
  по-прежнему торчат только `run`/`OrchestratorDeps` (`main.rs` не тронут).
- **Каркас — `mod.rs`** (≈450 строк): `struct Orchestrator`, петля `run()` с
  `select!`, диспетчер `handle_command`, `bootstrap` и общие хелперы (эмиттеры
  `emit_chat_list/profile_list/settings`, `chat_mut`, `effective_sampling`,
  `activate`, `mark_dirty`, `flush_saves`, `backend_if_ready`, `new_chat_value`).
  Обработчики команд — отдельными `impl Orchestrator`-блоками в файлах по фичам:
  `generation.rs` (send/regenerate/delete_last/done + задача agentic-loop),
  `chats.rs`, `profiles.rs`, `settings.rs` (+apply_*), `title.rs`, `impersonation.rs`,
  `rag.rs`; маппинг домен↔движок — `request.rs`. Все тесты (411 шт.) — в `tests.rs`.
- **Видимость**: методы, вызываемые из диспетчера/другого файла, и типы
  внутренних каналов (`GenResult`, `TitleResult`) помечены `pub(super)`; общие
  хелперы остались приватными в `mod.rs` (дочерние модули видят приватные элементы
  предка по правилу видимости Rust). Дочерние модули импортируют ровно свои
  зависимости (без `use super::*` в неtest-коде → нет «висячих» импортов под
  `-D warnings`). Фоновые задачи (`spawn_generation/_title/_impersonation/
  _rag_ingest`) — приватные free-функции в файлах своих фич.
- **Фаза 3** (выделение когезивных под-структур) — **сделана отдельным PR**, см. ниже.

### Пост-M9: Фаза 3 — выделены под-структуры EngineManager / SaveQueue (сделано)
- **`Orchestrator` ужат с 27 до 16 полей** выносом двух когезивных единиц (без
  изменения поведения, без новых каналов; инвариант «единственный владелец `Chat`»
  цел):
  - **`EngineManager`** (`app/orchestrator/engines.rs`) — жизненный цикл серверов
    инференса/эмбеддингов: владеет `backend`/`imp_backend`/`embedder`, опорами на
    managed-процессы (`*_handle`, `kill_on_drop`), статусами готовности
    (`server_status`/`imp_status`) и каналами probe (`status_tx`/`imp_status_tx`) +
    `supervisor`. Экспонирует `apply_chat` (→ возвращает немедленный статус, эмитит
    вызывающий), `apply_embed`, `apply_impersonation`, `set_chat_status`/
    `set_imp_status` (из петли probe), `backend_if_ready`,
    `impersonation_backend_if_ready(mode)`, `embedder()`. Пара к трейту
    `ServerSupervisor`. Серверная логика (~120 строк) ушла из оркестратора;
    `settings.rs` стал тоньше (81 → 57 строк) — `apply_*_settings` теперь делегируют.
  - **`SaveQueue`** (`app/orchestrator/save_queue.rs`) — дебаунс-очередь отложенного
    сохранения: `dirty: HashSet<Uuid>` + `deadline`. Методы `mark`/`forget`/
    `deadline`/`take` (+ `is_dirty` под `#[cfg(test)]`). `SAVE_DEBOUNCE` переехал
    сюда. Сама запись осталась у оркестратора (`flush_saves` зовёт `saves.take()`).
- **Видимость**: типы `pub(super)`; поля, которые проставляют тесты/петля
  (`engines.backend`, `engines.server_status`, `engines.embedder`) — `pub(super)`,
  остальные приватны. Делегирующие хелперы `Orchestrator::mark_dirty`/`flush_saves`
  сохранены (минимум правок в местах вызова); прямой `backend_if_ready` оркестратора
  убран — вызовы идут в `self.engines.backend_if_ready()`.
- **Не вошло**: состояние *генерации* имперсонации (`imp_cancel`/`imp_gen`/
  `imp_done_tx`) и `rag_cancel` — это конкурентные подсостояния хода, не серверы;
  оставлены на `Orchestrator` (как и `gen_state`). `cargo fmt`/`clippy`/`test`
  зелёные (411 passed, 7 ignored).

### Пост-M9: счётчик токенов в статус-баре при генерации (сделано)
- **Суммарный счётчик** (`токены: 1290`): токены всей переписки (промпта) **плюс**
  токены текущего/последнего ответа, одним числом. Виден **сразу при старте**
  (`~1234`, ответ ещё 0), а не «считается с 1»; во время генерации число растёт
  live (ярко, accent), после — приглушённо (итог хода) до следующей генерации.
- **Переписка — оценка `~`, затем точное число**. Точный `prompt_tokens` сервер
  присылает только в конце хода (с `usage`), поэтому до этого показывается
  клиентская **оценка** (`shared/tokens.rs::estimate_prompt`, эвристика «байты
  UTF-8 / 4»: латиница ≈4 симв./токен, кириллица ≈2 — близко к BPE Gemma/Qwen) с
  пометкой `~`. Оценку эмитит `start_generation` сразу после `GenerationStarted`
  (`TokenUsage{completion:0, context:Some(est), context_exact:false}`); приход
  `usage` заменяет её точным `prompt_tokens` (`context_exact:true`, `~` снимается).
- **Ответ — двойной источник**: (1) live-приближение по числу потоковых дельт
  (`Text`/`Thoughts`) — у llama-server одна дельта ≈ один токен, работает с любым
  сервером; (2) точный `completion_tokens` из `usage`. Запрос просит usage через
  новое поле `stream_options.include_usage=true` (`wire.rs`, только при стриминге);
  `ChatCompletionChunk.usage` → `Usage{prompt_tokens,completion_tokens}` → клиент
  эмитит `ChatChunk::Usage(TokenUsage)` (новый вариант enum) **до** разбора
  `choices` (у usage-чанка `choices` пуст) и до `Finished`.
- **Поток**: `stream_round` (`orchestrator/generation.rs`) на каждую дельту шлёт
  `AppEvent::TokenUsage{generation_id, completion, context:None, ..}` с накопительным
  счётом `base_tokens + streamed` (переписку не трогает — `context:None` сохраняет
  оценку); на `ChatChunk::Usage` — точные `completion`+`context`. Счётчик ответа
  **накопителен по раундам agentic-loop** (`total_tokens` += `RoundOutput.tokens`,
  где `tokens` = usage, иначе число дельт). `runtime.rs` →
  `ChatScreen::set_token_usage` (гейт по `generation_id`; контекст обновляется лишь
  при `Some`); поля `gen_tokens`/`gen_context`/`gen_context_exact` сбрасываются в
  `begin_generation`. `status_bar::render` рисует `токены: [~]<переписка+ответ>`
  одним числом (`~` пока переписка — оценка); скрыт при `tokens==0 &&
  context==None`.
- Внешний/строгий OpenAI-сервер `stream_options` либо поддержит (точный счёт),
  либо проигнорирует (останутся оценка переписки и live-приближение ответа) —
  деградация мягкая.

### Пост-M9: список чатов вынесен в отдельный экран (сделано)
- **Список чатов был оверлеем внутри `ChatScreen`** (`overlay: Option<ChatListState>`),
  теперь — **самостоятельный экран** `screens/chat_list.rs` (`ChatListScreen`),
  разгружая экран чата. Жёсткой привязки не было: виджет `widgets/chat_list.rs`
  (`ChatListState`) уже держал свой снимок, рисовался во весь экран и отвечал
  `ChatListAction`. Экран — **тонкая обёртка** над виджетом (FSD: `screens →
  widgets`): хранит контекст отрисовки (активный чат для метки `●`, палитру) и
  переводит `ChatListAction` → новый `ChatListIntent` (параллель `ChatIntent`/
  `SettingsIntent`). Виджет и его тесты не тронуты.
- **Три экрана через enum** (по выбору): `app/runtime.rs` держит базовый `ChatScreen`
  + `enum ActiveScreen { Chat | ChatList(Box<…>) | Settings(Box<…>) }` (варианты
  боксированы — экраны крупные, `clippy::large_enum_variant`). Заменил прежнюю пару
  «`screen` + `Option<SettingsScreen>`». Открытый список/настройки получают ввод и
  рисуются **вместо** чата (как раньше настройки); `Esc`/`Close` → `ActiveScreen::Chat`.
- **Контракт**: чат на `Esc` (не в генерации) отдаёт `ChatIntent::OpenChatList`
  (вместо локального открытия оверлея); `runtime::dispatch` создаёт `ChatListScreen`
  из снимков чата (`chat_summaries`/`active_chat`/`palette` — новые геттеры).
  `dispatch_chat_list` транслирует `ChatListIntent` в `AppCommand`/управление
  экранами: `Switch`/`Clone`/`NewChat` закрывают список, `Copy`/`Delete`/`Rename`/
  `AutoRename` оставляют открытым (как было у оверлея). `NewChat` закрывает список и
  зовёт `ChatScreen::request_new_chat()` (выбор профиля живёт в экране чата — оверлей
  при >1 профиле не дублируется).
- **Из `ChatIntent` убраны** ныне мёртвые `SwitchChat/CloneChat/DeleteChat/
  RenameChat/AutoRenameChat` (их строил только удалённый `handle_overlay_key`; теперь
  их роль у `ChatListIntent`). Остались `NewChat` (`Ctrl+N`) и `CopyChat` (`F5` в чате).
- **Маршрутизация событий** (`app/runtime.rs::apply_event` теперь берёт `&mut
  ActiveScreen`): `ChatList` применяется к чату **всегда** (актуальный снимок для
  след. открытия/`Ctrl+N`) и дополнительно к открытому списку (живое обновление);
  `ChatListError`/результат `CopyToClipboard` идут в область статуса списка, если
  открыт, иначе заметкой в ленту (`ChatScreen::push_note`/`push_error` теперь pub);
  `ChatActivated` обновляет метку активного чата в открытом списке (`set_active`);
  `Settings` при открытом списке обновляет его палитру (`set_palette`). Гейты петли
  (перепроверка орфографии, спиннеры RAG/имперсонации, колесо мыши) переведены с
  `settings.is_none()` на `active.is_chat()` — пока список/настройки открыты, эти
  относящиеся к чату вещи приостановлены (как уже было для настроек; фоновые задачи
  RAG/генерации продолжаются, просто их анимация/баннер не рисуются поверх).
- `set_overlay_error`/`set_overlay_notice` у `ChatScreen` удалены (роутинг переехал в
  runtime). 421 тест зелёный, clippy чист.

### Пост-M9: RAG — `/rag list`, конфигурируемый чанкинг, `/rag rebuild` (сделано)
- **Конфигурируемые размеры чанка/перекрытия** (`config.rag: RagSettings` —
  `chunk_target_chars`/`chunk_overlap_chars`/`chunk_max_chars`, `#[serde(default)]` →
  старые `settings.json` без миграции; дефолты = прежние константы 800/150/1200).
  Ранее захардкоженные `CHUNK_*` сняты; вместо них тип `ChunkParams`
  (`features/tools/rag.rs`, `pub`), который `chunk_text`/`chunk_markdown`/
  `segment_units` принимают параметром. `ChunkParams::from_settings` санитизирует
  ввод (нулевой target → дефолт; overlap < target; max ≥ target). Протянут в
  `ToolContext.chunk_params` (для инструмента `rag_add`, строится из `config.rag` в
  `orchestrator/generation.rs`) и в фоновые задачи RAG. UI — три текстовых поля в
  секции «Инструменты» экрана настроек (с подсказками-описаниями).
- **Хранение исходного текста источников** (`rag_sources(profile_id, source, content,
  created_at)`, PK по `(profile_id, source)`; `CREATE TABLE IF NOT EXISTS` — без
  миграции). Файловая индексация (`/rag add`) **заменяет** исходник
  (`rag_source_upsert`), инструмент `rag_add` (накапливает чанки) — **дописывает**
  (`rag_source_append`). `/rag remove` чистит и `rag_sources` (тот же предикат пути).
  Нужно для `/rag rebuild` без обращения к файлам на диске.
- **`/rag list`** — источники базы знаний активного профиля (счётчик чанков + дата
  самого раннего чанка): `Db::rag_list_sources` (`GROUP BY source`,
  `entities::rag::RagSourceInfo`); орк. `handle_rag_list` (на месте, без задачи) →
  `RagProgress::Listed { sources }` → заметка в ленте (`format_rag_sources`).
- **`/rag rebuild`** — реиндексация фоновой задачей (`spawn_rag_rebuild`): собирает
  источники из БД, резолвит текст (сохранённый → иначе чтение файла по пути для
  legacy-данных; невосстановимые считаются ошибкой), перечанковывает/переэмбеддивает
  текущими параметрами. **Смена размерности embedding-модели**: размерность вектора в
  sqlite-vec одна на всю БД, поэтому при `new_dim != current_dim` задача проверяет
  `Db::rag_other_profiles_have_docs` — если другие профили используют базу, отказ с
  понятным сообщением (не затираем чужое); иначе `Db::rag_reset_vectors`
  (drop таблицы векторов + сброс `meta.rag_dim`) и реиндекс в новой размерности. Общая
  логика файловой индексации и реиндекса вынесена в `index_source`
  (`orchestrator/rag.rs`). Markdown определяется по расширению `*.md` источника.
- **Контракт**: `RagCommand::List/Rebuild` (`features/rag_command.rs`) →
  `ChatIntent::RagList/RagRebuild` → `AppCommand::RagList/RagRebuild` → орк.
  `handle_rag_list/handle_rag_rebuild`. `reset_rag_cancel`/`active_profile_id`/
  `fail_rag` вынесены в общие хелперы `orchestrator/rag.rs`. Команды добавлены в
  оверлей помощи (`F1`/`?`). **433 теста зелёные**, clippy/fmt чисты.

### Пост-M9: мониторинг раннего выхода `Child` в пробе готовности (сделано)
- **Симптом**: при битом GGUF / нехватке памяти managed `llama-server` биндит порт,
  но **умирает уже в ходе загрузки модели** (предполётная проверка файла
  `launch_missing_model_file_errors_before_spawn` ловит лишь отсутствие файла, не
  «валидный путь, но процесс упал»). Фоновый probe (`wait_until_ready`) держал только
  `OpenAiClient` и опрашивал мёртвый порт **до таймаута** `MANAGED_READY_TIMEOUT=600с`,
  держа UI в «сервер: подключение…».
- **Причина-ограничение**: `Child` принадлежал `ServerHandle` (для `kill_on_drop`), а
  обнаружить выход можно лишь `child.wait()` — это `&mut Child` + владение, что
  конфликтует с удержанием хэндла оркестратором ради kill-on-drop.
- **Фикс** (`shared/api/server.rs`): `Child` уходит в **монитор-задачу**
  (`spawn_monitor`), которая `select!`-ит между `child.wait()` (процесс умер → взвод
  `exited: CancellationToken`) и `kill: CancellationToken` (взводится в `Drop for
  ServerHandle` → `start_kill` + `wait`). `ServerHandle` хранит оба токена и отдаёт
  `exited()` пробе. `wait_until_ready(client, timeout, exited: Option<Cancellation
  Token>)` после неуспешной пробы проверяет `exited.is_cancelled()` → ранний `bail!`
  с понятным текстом («…завершился до готовности (битый GGUF или нехватка памяти?)»),
  а в паузе между пробами `select!`-ит со сном, чтобы проснуться мгновенно при смерти
  процесса. `kill_on_drop(true)` оставлен подстраховкой (монитор владеет `Child`).
- **Проводка**: `app/supervisor.rs::spawn_probe` принимает `Option<CancellationToken>`;
  managed-режимы (chat + impersonation) передают `Some(handle.exited())`, external —
  `None` (процесса нет). Маппинг ошибки → `ServerStatus::Disconnected` уже существовал
  (статус-бар покажет причину). **435 тестов зелёные** (+2:
  `wait_until_ready_bails_on_early_exit`, `monitor_cancels_exited_when_child_dies` —
  реальный короткоживущий процесс, кросс-платформенно), clippy/fmt чисты.

### Пост-M9: новые инструменты — файлы, fetch_url, калькулятор, дата/время (сделано)
- **Четыре новых инструмента** (`features/tools/`), все по существующему контракту
  `Tool`/`ToolContext`/`ToolOutcome` (без мутации `Chat`, ошибка инструмента → текст
  результата, а не паника):
  - **Файлы** (`fs.rs`): `fs_read`/`fs_write`/`fs_list` — чтение/запись (с `append`)/
    листинг локальных файлов через `tokio::fs`. Под **новым глобальным выключателем
    `tools.fs_enabled`** (по умолчанию **выключены**, как Python: инструмент может
    прочитать/перезаписать любой файл). Опциональная **песочница `tools.fs_root`**:
    если задана, все пути канонизируются и обязаны лежать внутри корня (`FsRoot::
    resolve`: для существующих — `canonicalize`, для нового файла — `canonicalize`
    родителя + имя; `starts_with(root)` ловит выход через `..`/абсолютный путь).
    Лимиты: чтение ≤50k симв., листинг ≤500 элементов. UTF-8-lossy (бинарь читать
    нечем).
  - **`fetch_url`** (`fetch.rs`): загрузка страницы своим `reqwest`-клиентом
    (браузерные заголовки) → извлечение читаемого текста (**переиспользует
    `web::extract_readable`** — сделаны `pub(crate)`: `extract_readable`,
    `truncate_chars`, `USER_AGENT`, `ACCEPT_HTML`, `ACCEPT_LANGUAGE`) → саммаризация
    через `ctx.engine` независимым одно-ходовым запросом (как `call_subagent`: без
    истории/инструментов, лимит токенов 768, таймаут 90с). Аргумент `focus`
    фокусирует пересказ; `summarize=false` → извлечённый текст без модели; сбой
    саммаризации → мягкая деградация (отдаём извлечённый текст). Гейтится
    **`web_enabled`** (сетевой доступ, как `web_search`). Требует `http(s)://`.
  - **`calculate`** (`calc.rs`): собственный **рекурсивно-нисходящий вычислитель**
    (без новых крейтов) — `eval(&str) -> Result<f64>` (чистая, тестируемая):
    лексер + парсер `expr/term/factor/unary/atom`, приоритеты, правоассоциативный
    `^`, унарный минус, `%`, скобки, экспоненциальная запись, константы
    (`pi`/`e`/`tau`) и функции (`sqrt`/`cbrt`/`abs`/`exp`/`ln`/`log`(+основание)/
    `log2`/три­гонометрия/`atan2`/гиперболические/`floor`/`ceil`/`round`/`min`/`max`/
    `pow`). Без I/O, **не гейтится**.
  - **`current_time`** (`datetime.rs`): локальное время + UTC (`chrono::Local`/`Utc`,
    default-фичи уже включают `clock`); аргумент `format` — строка `strftime`
    (неверная спецификация ловится через `write!`+`DelayedFormat`, не паникует).
    Без I/O, **не гейтится**.
- **Сверка инструментов существующих профилей** (`features/profiles.rs::
  reconcile_tools`, новое поле `Profile.known_tools`, `#[serde(default)]`): профили
  хранят снимок `enabled_tools`, поэтому добавленные в приложение инструменты
  **не появлялись** у уже сохранённых профилей (модель видела только старый набор и
  лезла в `python_exec` за всем — симптом, который и проявился). `reconcile_tools`
  включает в профиле инструменты из `default_tool_ids`, которых он ещё «не знал»
  (нет в `known_tools`), и записывает их как известные; **выключенные пользователем
  не переоткрываются** (они в `known_tools`). Для профиля без `known_tools` (создан
  до реестра) базой известного берётся текущий `enabled_tools`, так что добавляются
  лишь действительно новые инструменты. Вызывается в `bootstrap` (для каждого
  загруженного профиля, при изменении — `upsert_profile`) и в `profiles::create`
  (новый профиль сразу получает полный текущий набор). Идемпотентна.
- **Проводка**: ID добавлены в `default_tool_ids` (значит, автоматически попадают в
  каталог тумблеров профиля `screens/settings.rs::tool_catalog`); `effective_tool_ids`
  получил **4-й параметр `fs_enabled`** и гейтит `web_search`+`fetch_url` через
  `web_enabled`, `fs_*` через `fs_enabled` (вызовы обновлены в
  `orchestrator/generation.rs`); `ToolConfig` получил `fs_root`, прокинут в
  `standard_registry` → `Fs*::new` (из `build_registry`, `orchestrator/mod.rs`).
  `ToolSettings` расширен `fs_enabled`/`fs_root` (`#[serde(default)]` → без миграции).
- **UI настроек** (секция «Инструменты»): тумблер «Доступ к файлам» (`FieldId::TFs`)
  и текст «Файлы: каталог-песочница» (`FieldId::TFsRoot`) с подсказками-описаниями
  (`field_description`). Безопасные `calculate`/`current_time` выключателей не имеют.
- **Тесты**: чистые (`calc::eval` — приоритеты/степень/функции/ошибки; `datetime`
  рендер с инъекцией момента; `fs` песочница/roundtrip/append/листинг на tempdir;
  `fetch` отказ на не-http и построение одно-ходового запроса с `focus` через
  capturing-backend). Сетевые/реальные — `#[ignore]`. **472 теста зелёные** (+37),
  clippy/fmt чисты.

### Пост-M9: редизайн TUI (палитра, рейлы ролей, пилюли, «клавиши») (сделано)
- **Цель**: кардинально освежить интерфейс по готовому макету
  (`docs/redesign/Редизайн TUI.dc.html`). Стиль: спокойные **скруглённые** рамки,
  цветные **рейлы ролей** сообщений, статус-«пилюли», тихая строка хоткеев с
  «клавишными» лейблами. Перевод макета в ratatui — не пиксель-в-пиксель, а по
  его же заметке «РЕАЛИЗАЦИЯ В RATATUI» (Block + rounded borders, рейлы `▌`,
  `Style::fg`, reversed-спаны клавиш — всё в 256-цветах).
- **`shared/theme.rs` — палитра расширена** структурными ролями поверх прежних
  (`user/assistant/tool/success/warning/error/accent`): `*_soft` (светлые варианты
  заголовков ролей), `text`, `muted`, `border`/`border_focus`, `keycap_fg`/
  `keycap_bg`. Точные оттенки **тёмной** темы — из макета (oklch → sRGB, посчитано
  при разработке). **Auto** (по умолчанию) — именованные ANSI (адаптируются
  терминалом) + нейтральные серые для структуры; **Light** — затемнённые. Хелперы:
  `panel(title, focused)` (скруглённый `Block` с титулом), `keycap(label)`,
  `hint(key, desc)`, `pill(label, color)`, `border_style(focused)`, `muted_style`.
  (Удалены неиспользуемые `warning_style`/`error_style`.) Палитра по-прежнему `Hash`
  (ключ кэша syntect-темы в `markdown.rs`).
- **`widgets/message_feed.rs`**: каждое сообщение получает **цветной гуттер-рейл**
  `▌` по роли (контент строится в ширину `width − 2`, переносится, рейл навешивается
  на каждый визуальный ряд → `prepend_rail`); заголовки ролей капсом с иконкой
  (`✦ АССИСТЕНТ` / `❯ ВЫ`, `role_header`); «мысли» свёрнуты **пилюлей**
  (`▸ мысли · N стр. · [Ctrl+T]`); tool-карточка — `⚒  имя(аргументы)` (две позиции
  после эмодзи — он шириной 2) + результат на гуттере `└`. Титул ленты: слева
  `◆ <чат>`, справа мета `модель · Nk ctx` (`ChatScreen::model_meta` из снимка
  настроек).
- **`widgets/input_box.rs`**: скруглённая рамка + цвет фокуса, колонка приглашения
  `❯` слева (ширина `PROMPT_W`; внутренняя область ввода сдвинута вправо, тест-хелпер
  `render_at` учитывает это).
- **`widgets/status_bar.rs`**: статус сервера — **пилюлей** (`● сервер: …` цветом
  статуса), разделители `│`, хоткеи через `keycap`+`hint`. Подстроки `готов`/
  `генерация`/`токены:`/`мышь:` сохранены (тесты).
- **`screens/settings.rs`**: главная панель и подсказки — через `panel`/`keycap`;
  меню секций с рейлом активной секции; значения полей раскрашены по типу
  (`render_field_line` принимает палитру: тумблер — зелёный/приглушённый, выбор —
  синий, прочерк — цвет рамки).
- **`widgets/chat_list.rs`**: полноэкранная скруглённая панель «▤ Чаты» (+счётчик
  справа); **строка поиска в рамке** с «клавишей `/`» справа; строки списка — точка
  (зелёная у активного), заголовок и счётчик, **прижатый к правому краю**
  (усечение `…`, `truncate_to_width`), выделение — мягкая подложка + зелёный рейл
  выделенной строки; **строка статуса хоткеев внизу** выложена **аккуратной сеткой**
  (`status_lines`: максимум столбцов под ширину, столбцы совпадают по вертикали;
  `Del` — красной «клавишей»). **Переименование (F2)** — отдельное поле ввода с
  **настоящим курсором** (`render_rename` + `set_cursor_position`, горизонтальный
  скролл; `Mode::Rename` хранит `buffer: Vec<char>` + `cursor`, поддержаны
  `←/→`/`Home`/`End`/`Backspace`/`Delete`/вставка в середину), на время правки строка
  поиска прячется. `profile_list.rs`/`impersonation_preview.rs`, оверлеи помощи и
  орфографии — тоже на скруглённых панелях.
- **Эмодзи-глифы шириной 2** (`⚒`/`⚙`/`⌨`) `unicode-width` считает за 1 → следующий
  пробел затирался; после них ставим **два** пробела (иначе сливаются с текстом).
- Тема по умолчанию осталась **Auto** (структура редизайна видна везде; точные
  оттенки макета — тема **тёмная** в настройках). Юнит-тесты обновлены под новый
  вид (поиск tool-блоков по имени, рейл-aware пустые строки, навигация `↑/↓` с учётом
  колонки приглашения). Гейт зелёный.

### Пост-M9: статус-бар — пилюля сверху-слева, хоткеи сеткой справа (сделано)
- **Симптом**: при переносе хоткеи шли отдельной сеткой **под** строкой состояния, и
  «`Ctrl+W  мышь: выделение`» вставало прямо под «`● сервер: готов`» — две левые
  колонки сливались и зрительно конкурировали.
- **Фикс** (`widgets/status_bar.rs`, `lines`/`grid_layout`/`right_grid`): пилюля
  статуса (сервер + генерация + счётчик токенов) остаётся слева на **верхней** строке,
  а хоткеи выкладываются **аккуратной сеткой, прижатой к правому краю**, и **делят
  верхнюю строку** с пилюлей. Когда всё влезает — одна строка: пилюля слева, хоткеи
  справа (между ними заметный зазор). Когда не влезает — хоткеи переносятся ВНИЗ
  сеткой, **столбцы которой совпадают по вертикали** (как в окне списка чатов), а
  неполная (перенесённая) строка прижата вправо — её клавиши встают **ровно под
  столбцами строки выше** (напр. `Ctrl+C выход` точно под `Ctrl+P настройки`, добитый
  до ширины столбца), так что левая/средняя часть окна пустая и не привлекает внимание.
- **Раскладка** (`grid_layout`): ячейки заполняются по строкам слева-направо/сверху-
  вниз, но неполная нижняя строка занимает **крайние правые** столбцы (`empty_lead`
  пустых столбцов в начале); ширины столбцов — максимум по фактически размещённым в
  столбце ячейкам (включая перенесённую), общая ширина блока + ведущий отступ =
  ширине → блок прижат вправо. **Выбор числа столбцов**: максимум столбцов (→ минимум
  строк), при которых пилюля делит верхнюю строку с блоком (`state_w + GAP +
  block_width(cols) ≤ width`). Узкий фолбэк (даже один столбец не влезает рядом с
  пилюлей): пилюля отдельной верхней строкой, сетка прижата вправо под ней.
  `height()`/`render()` и их вызов в `screens/chat.rs` без изменений (та же сигнатура).
  Прежний инлайн-путь и левая сетка (`palette.hotkey_grid`) в статус-баре больше не
  используются (грид-хелпер палитры остался для оверлея списка чатов). Тесты: одна
  строка с прижатыми краями; перенос с пилюлей на верхней строке и `Ctrl+C` ровно под
  столбцом `Ctrl+P` (рендер в `TestBackend`, сравнение позиций в символах).

### Пост-M9: цвет рейла роли не зависит от стиля строки (сделано)
- **Симптом**: вертикальный рейл роли (`▌`) слева от сообщения выглядел другим
  цветом (приглушённым) напротив строк-разделителей `───` и горизонтальных рамок
  таблиц.
- **Причина**: `markdown` задаёт у этих строк `.add_modifier(Modifier::DIM)` на
  **уровне строки** (`line.style`, см. `shared/markdown.rs` `rule`/`border_line`), а
  `message_feed::prepend_rail` копировал `line.style` на всю выходную строку, включая
  спан рейла. Спан рейла переопределял только `fg` (не модификаторы), поэтому
  line-level `DIM` затекал и на него.
- **Фикс** (`widgets/message_feed.rs::prepend_rail`): стиль уровня строки
  **вплавляется в спаны содержимого** (`line.style.patch(span.style)`), а `out.style`
  сбрасывается в дефолт. Содержимое выглядит идентично прежнему (тот же итоговый
  стиль спанов), но рейл стоит самостоятельным спаном со своим единственным `fg` и
  больше не наследует никаких line-level модификаторов (решение общее — не только под
  текущий `DIM`). Регрессионный тест `rail_is_not_dimmed_next_to_table_borders`.

### Пост-M9: затемнение фона под попапами (сделано)
- **Симптом**: попапы поверх экрана очищали только свою область (`Clear`), а фон за
  ними оставался на полной яркости и зрительно сливался с попапом, затрудняя
  восприятие.
- **Фикс**: хелпер `shared/ui.rs::dim_background(frame)` накладывает `Modifier::DIM`
  на **все** ячейки буфера экрана; вызывается **перед** `Clear`+рендером попапа.
  `Clear` затем сбрасывает ячейки попапа к дефолтному (неприглушённому) стилю — так
  притеняется только фон, а сам попап остаётся ярким. Эффект `DIM` терминало-зависим
  (Windows Terminal поддерживает). Лежит в `shared` (FSD: `screens → shared`), чтобы
  переиспользоваться экранами.
- **Где применено**: оверлей помощи (`F1`/`?`) и попап подсказок орфографии (`Ctrl+G`)
  в `screens/chat.rs`; крупный многострочный редактор системного сообщения/приветствия
  профиля в `screens/settings.rs` (только `editor.multiline` — компактные однострочные
  полосы редактирования полей правятся на месте и фон не притеняют). Полноэкранные
  оверлеи (выбор профиля, список чатов, настройки) фон не притеняют — они и так
  занимают весь экран.

### Пост-M9: ручное переименование чата в однострочном `InputBox` (сделано)
- **Поле переименования (`F2` в списке чатов) переведено с самодельного
  посимвольного буфера на однострочный `InputBox`** (`set_single_line`, как
  редактируемые поля настроек). Так в нём «бесплатно» появились: **спелл-чек**
  (подчёркивание ошибок), **пословная навигация/удаление** (`Ctrl+←/→`,
  `Ctrl+Backspace/Delete`), `Ctrl+Home/End`, **очистка/возврат `Ctrl+K`**, вставка
  из буфера и горизонтальный скролл длинного названия. Раньше буфер `Vec<char>`
  умел лишь посимвольное движение/правку.
- **`widgets/chat_list.rs`**: `Mode::Rename` хранит `Box<InputBox>` (+ флаг
  `spell_dirty`) вместо `Vec<char>`/`cursor`; `on_key_rename` обрабатывает только
  `Esc`/`Enter`/`Ctrl+K` (раскладко-независимо, `shared/keys`), всё прочее ведёт сам
  `InputBox`. Ручная функция `render_rename` удалена — поле рисует
  `input.render(...)` (своя скруглённая рамка + `❯` + настоящий курсор). Новые
  методы `handle_paste` и `recheck_rename_spelling(&SpellChecker) -> bool`. `render`
  стал `&mut self` (`InputBox::render` требует `&mut`).
- **Спелл-чекер не дублируется**: владелец — экран чата (`ChatScreen::spellchecker()
  -> Option<&SpellChecker>`); петля `app/runtime.rs` при открытом списке одалживает
  его экрану списка (`ChatListScreen::recheck_spelling`) — поля `screen`/`active`
  раздельны, поэтому immutable-заём чекера сосуществует с mutable-заёмом списка.
  Дебаунса нет (название короткое — пересчёт по флагу `spell_dirty` дёшев). Вставка
  из буфера для списка маршрутизируется в поле переименования
  (`process_input_batch`). **Попап подсказок (`Ctrl+G`) для переименования НЕ
  добавлялся** — только подчёркивание (по согласованию).

### Пост-M9: управляющие инструменты беседы (followup / rewrite) (сделано)
- **Два опциональных инструмента** (`features/tools/control.rs`, spec §9.3.3),
  позволяющих модели управлять структурой собственного ответа:
  - **`send_followup_message`** («написать ещё сообщение») — ассистент дописывает
    текущее сообщение, вызывает инструмент и пишет **вторую реплику** отдельным
    пузырём сразу за первой.
  - **`rewrite_current_message`** («переписать своё текущее сообщение») — если по ходу понял,
    что ответил неверно: накопленный текст текущего раунда **отбрасывается**, и
    следующий раунд пишется заново. Отброшенное (assistant + tool) уезжает в
    `Chat.deleted` (как `Ctrl+E`/`Ctrl+R`) ради ручного восстановления.
- **Это control-flow, не обычные инструменты**: их распознаёт сам клиентский
  agentic-loop (`app/orchestrator/generation.rs`), а не `Tool::invoke` (реализации
  `Tool` нужны лишь для схемы/регистрации/гейтинга; `invoke` возвращает тот же
  текст-«разрешение», что синтезирует петля). По протоколу tool-calling альтернация
  не ломается (`assistant(tool_call) → tool → assistant`), **синтетический user не
  вводится** (по согласованию — проще и надёжнее).
- **Опциональны, по умолчанию выкл**: их нет в `default_tool_ids` (→ `reconcile_tools`
  не включает их у существующих/новых профилей), но есть в новом каталоге
  `all_tool_ids` — `screens/settings.rs::tool_catalog` строит тумблеры профиля из
  него. Петля распознаёт управляющий вызов **только если он реально включён** в
  профиле (`allowed_has`), иначе — обычный отказ. `effective_tool_ids` их не
  трогает (проходят через `_ => true`).
- **Отдельный пузырь** — флаг `Message.new_bubble` (`#[serde(default,
  skip_serializing_if)]` → без миграции): `from_messages` (`widgets/message_feed.rs`)
  не склеивает помеченное сообщение с предыдущим блоком ассистента. Служебные
  tool-блоки управляющих инструментов в ленте **скрыты** (`from_message` фильтрует
  по `control::is_control_tool`).
- **Live-стрим ↔ перезагрузка**: новые события `AppEvent::AssistantContinue`
  (followup: завершить пузырь, начать новый) и `AssistantRewrite` (отбросить
  накопленный текст текущего пузыря) — `screens/chat.rs::continue_assistant`/
  `rewrite_assistant`. В петле генерации `pending_new_bubble` ставит `new_bubble` на
  следующее доменное сообщение ассистента; rewrite-раунд не поглощает его и кладёт
  отброшенное в `GenResult.deleted` → `handle_done` зовёт `chat.record_deleted`.
- Каждый вызов = раунд (`max_tool_rounds`, бэкстоп от бесконечных доп. сообщений/
  переписываний). Тесты: чистые (`control` имена/тексты; `from_messages` два пузыря
  + скрытие control-блока + обычная склейка не сломана), live (`chat.rs`: followup
  два пузыря == reload, rewrite отбрасывает частичный), интеграционные оркестратора
  (followup → 4 сообщения с `new_bubble`; rewrite → отброшенное в `deleted`).
  **511 тестов зелёные**, clippy/fmt чисты.
- **Проверено на живых Gemma 4 12B и Qwen 3.6 27B** (`llama-server`, `--jinja`): три
  `#[ignore]`-смоука зелёные на обеих — клиентский
  (`client.rs::control_tools_are_callable`: модель реально вызывает
  `send_followup_message`) и два end-to-end оркестратора через реальный
  `OpenAiClient` (`tests.rs::followup_tool_e2e_live` → история `user →
  assistant(followup) → tool → assistant(new_bubble=true)`; `rewrite_tool_e2e_live`
  → отброшенный раунд в `Chat.deleted`, финальная история чистая). Запуск:
  `MINDFORK_ENGINE_URL=…/v1 cargo test … -- --ignored --nocapture --test-threads=1`.
- **Наблюдение**: вызов `send_followup_message` обе модели делают охотно; вызов
  `rewrite_current_message` чувствительнее к промпту (Qwen на мягкую формулировку
  «сценки» просто писал неверный ответ без вызова — потребовалось явно обозначить
  вызов как обязательный шаг). Это про **поведение моделей** на контрнатуральную
  просьбу «ответь неверно, затем перепиши», а не про механизм: как только модель
  реально вызывает инструмент, отбрасывание/архив отрабатывают одинаково. В реальном
  использовании инструмент срабатывает естественно (модель сама понимает, что
  ошиблась), а его описание нейтральное.

### Пост-M9: вставка эмодзи из буфера (ширина кластеров + восстановление из буфера) (сделано)
- **Две независимые беды с эмодзи в поле ввода**, обе только в приложении (в голом
  терминале эмодзи вставляются и курсор корректен — терминал сам ведёт каретку):
  - **(B) Курсор «разъезжается» на BMP-эмодзи-кластерах** (`❤️` = `❤` U+2764 +
    селектор U+FE0F; `👍🏽` = эмодзи + модификатор тона кожи). Причина: ширину считали
    посимвольно через `unicode-width`, который даёт `❤`=1, `U+FE0F`=0 (итог 1), а
    терминал рисует кластер шириной **2** → курсор садился в середину эмодзи, текст
    после — на колонку левее. `👍🏽` мерился как 4 вместо 2.
  - **(A) Эмодзи supplementary-плоскости (`😊` U+1F60A, `🥹`) не вставляются вовсе.**
    Причина — **ограничение crossterm 0.29 на Windows**: вставка из буфера приходит
    обычными key-событиями консоли, а такие эмодзи кодируются UTF-16 суррогатной парой;
    записи key-down/key-up консоли ломают сборку пары в crossterm
    (`event/sys/windows/parse.rs`: суррогат обрабатывается без учёта `key_down`), и
    символ пропадает **до** нашего слоя. BMP-символы (буквы, `❤`, `U+FE0F`) проходят.
- **Фикс (B1) — ширина с учётом эмодзи-кластера** (`shared/wrap.rs`): новый
  `width_at(chars, i)` — каузальная (смотрит лишь на предыдущий символ) ширина символа
  `i`: селектор `U+FE0F` доводит предшествующий «текстовый» символ до 2 колонок
  (`2 − ширина_предыдущего`); модификатор тона кожи (`U+1F3FB..U+1F3FF`) даёт 0 (база
  уже 2). `display_width` теперь суммирует `width_at`, а инкрементальные потребители
  (`wrap_ranges`; `col_at_width`/`col_for_visual`/однострочный рендер в `input_box`;
  усечение с «…» в `chat_list`/`markdown`) переведены с `char_width` на `width_at` —
  иначе перенос/курсор разъехались бы с `display_width` на границе кластера. `char_width`
  оставлен как контекст-независимая база. Улучшает и ленту (`message_feed`), и таблицы.
- **Фикс (B2) — курсор/удаление по графемному кластеру** (`shared/wrap.rs`
  `prev_boundary`/`next_boundary` через `unicode-segmentation`, UAX #29; `input_box`
  `move_left`/`move_right`/`backspace`/`delete`): курсор и удаление ходили по скалярам
  Unicode, а `❤️`/`👍🏽` — это **два** скаляра (база + вариатор/модификатор). Симптомы:
  `←`/`Backspace` через `👍🏽` требовали двух нажатий (после первого оставался `👍`);
  через `❤️` курсор сначала прыгал в середину; `Delete` с начала строки оставлял
  «осиротевший» вариатор/модификатор → призрачные глифы и обрывки подчёркиваний
  орфографии. Теперь движение/удаление берут целый кластер: `prev_boundary`/
  `next_boundary` дают границу слева/справа от позиции. Одиночный скаляр-эмодзи (`😊`)
  — обычная граница ±1 (одно нажатие, как и было).
- **Фикс (A) — сверка вставки с буфером обмена** (`app/runtime.rs`, только Windows):
  `Chunk::Paste` (реконструкция из key-событий) сверяется с буфером обмена через чистую
  `paste_projection_matches`: из буфера выбрасываются кодпойнты > U+FFFF (ровно те, что
  теряет консоль), обе стороны нормализуются по `\r\n`/`\r`/`\t` — если совпало, это та
  же вставка и берётся **полный** текст буфера (с эмодзи); иначе (буфер устарел/не та
  вставка/недоступен) — реконструкция (без эмодзи, но без риска вставить чужое).
  Безопасно при любом исходе: если эмодзи в реконструкции уже есть — проекция не
  совпадёт и останется реконструкция; пустая реконструкция не матчится. `read_clipboard_
  text` лениво создаёт `arboard::Clipboard` (тот же слот, что у копирования `F5`).
  `reconcile_paste` на не-Windows тождественна (там приходит корректный `Event::Paste`).
- **Остаточное ограничение**: вставка, состоящая **только** из supplementary-эмодзи
  (без единого BMP-символа), не оставляет ни одного key-события → нет ни `Chunk::Paste`,
  ни иного триггера сверки → не вставится (нечем зацепиться). Эмодзи **внутри текста**
  (обычный случай, «привет 😊») восстанавливаются. Полное решение потребовало бы
  правки слоя чтения консоли (патч/форк crossterm либо собственный ридер `InputRecord`).

### Пост-M9: попап выбора эмодзи (`Ctrl+B`) (сделано)
- **`Ctrl+B` открывает попап-сетку популярных эмодзи** в окне чата; выбранный
  вставляется в поле ввода **на месте курсора**. Виджет `widgets/emoji_picker.rs`
  (`EmojiPickerState` + `EmojiPickerAction`) самодостаточен по FSD (`screens →
  widgets`): хранит только индекс выделения, на нажатия отвечает действием
  (`None`/`Cancel`/`Pick`). Список `EMOJIS` фиксирован (44 эмодзи, 4 ряда по
  `COLS=11`); ширина сетки выбрана так, чтобы нижняя подсказка попапа помещалась
  целиком. Навигация `←↑↓→` по сетке (с клампом), `Enter` — вставить, `Esc` —
  закрыть. Выделенная ячейка — на тёмном фоне `palette.keycap_bg` (как выбранная
  строка списка чатов; reversed давал светлый фон, затиравший цветной глиф).
- **Вставка** через `InputBox::insert_str` — безопасна для многоскалярных эмодзи
  (`❤️`, `👍🏽`; курсор/удаление по графемным кластерам уже корректны, см. выше).
- **Помнит последний выбор**: `ChatScreen.emoji_last` хранит индекс; открытие идёт
  через `EmojiPickerState::with_selected(idx)` (с клампом), закрытие (и `Enter`, и
  `Esc`) сохраняет текущее выделение обратно. Пока попап открыт — поле ввода теряет
  фокус, `handle_paste`/`handle_mouse` — no-op (как у попапа орфографии); рисуется
  поверх с затемнением фона (`dim_background`). `Ctrl+B` раскладко-независим
  (`shared/keys`, физ. B = `Ctrl+и`), добавлен в оверлей помощи (`F1`/`?`), таблицы
  клавиш README/spec §11.5. Чистые тесты виджета (навигация/кламп/`with_selected`/
  render-без-паники) и проводки экрана (вставка на месте курсора, память выбора,
  отмена).

### Пост-M9: мульти-провайдерный инференс — Фаза 0 (фундамент) (сделано)
- **Облачные провайдеры через тот же трейт `EngineBackend`** ([ADR 0004](docs/decisions/0004-engine-contract-multi-provider.md)):
  Фаза 0 разблокирует **OpenAI** (`platform.openai.com`) и **Gemini** (через
  OpenAI-совместимый endpoint). Claude (отдельный протокол `/v1/messages`) — Фаза 2.
  Слои выше движка (оркестратор, agentic-loop, инструменты, UI) **не тронуты**.
- **Плоская таксономия режимов** (`shared/config.rs`): `ServerMode` расширен
  `OpenAi`/`Gemini` (рядом с `Managed`/`External`), `ImpersonationMode` — ими же
  (плюс `Shared`). Новый `CloudProvider { OpenAi, Gemini }` с `base_url()`;
  `ServerMode::cloud_provider()`/`ImpersonationMode::cloud_provider()` → `Option`.
  `EngineSettings`/`ImpersonationEngineSettings`/`EmbedSettings` получили
  `model_name`/`api_key_env` (всё `#[serde(default)]` → старые `settings.json` без
  миграции).
- **Модель — свойство бэкенда, не запроса** (минимум правок): `ChatRequest` модель
  **не несёт** — её инъектит `OpenAiClient` в тело (`model`). Клиент получил
  `api_key`/`model`/`dialect` + билдеры `with_api_key`/`with_model`/`with_dialect`,
  шлёт `Authorization: Bearer` (метод `auth`), для эмбеддингов проставляет `model` в
  `EmbeddingRequest`. `new(url)` = локальный/external (без ключа, lenient-диалект).
- **Диалект тела запроса** (`shared/api/wire.rs`, `WireDialect { LlamaCpp, OpenAi,
  Gemini }`): `build_chat_request(req, stream, model, dialect)`. Строгие облачные
  диалекты (`OpenAi`/`Gemini`, `is_strict`) **вычищают** расширения llama.cpp и
  reasoning-сигналы (`top_k`/`min_p`/`dynatemp_*`/`dry_*`/`mirostat*`/`samplers`/
  `thinking`/`reasoning_*`/`chat_template_kwargs`) — иначе облако вернуло бы `400`
  даже на дефолтном `thinking:true` (`restrict_to_strict`). Остаются `temperature`/
  `top_p`/`frequency_penalty`/`presence_penalty`/`seed` + `tools`/`stream*`. **Лимит
  токенов различается:** OpenAI требует `max_completion_tokens` (новые модели
  отвергают `max_tokens` с `400` — реальная ошибка при прогоне), Gemini-compat —
  классический `max_tokens`; `LlamaCpp` шлёт всё как раньше. Провайдер→диалект —
  `dialect_for` в супервайзере. `WireDialect` экспортирован.
- **Супервайзер** (`app/supervisor.rs`): `apply_chat`/`apply_impersonation`/`apply_embed`
  получили облачные ветки (`cloud_chat_setup`/`cloud_embed_setup`). Ключ резолвится из
  **env-переменной по имени** (`resolve_api_key`; секрет на диск не пишется, ADR 0004);
  base URL — провайдера (override через `url`); диалект `OpenAi`. Облако не «грузит
  модель» → статус сразу `Ready` (без probe). Нет модели/ключа → `Disconnected` с
  понятным текстом (chat) / `UnavailableEmbedder` (RAG).
- **UI настроек — видимость полей по режиму** (`screens/settings.rs`): `model_fields`/
  embed-часть `tool_fields` показывают **только релевантные режиму** поля (managed →
  параметры llama-server; external → URL+модель(опц.); openai/gemini → модель+API-ключ
  (env)+base URL(опц.)) — это де-загромождает экран. `ServerMode`-цикл (`cycle_mode`,
  теперь с направлением) проходит все 4 варианта; `cycle_imp_mode` — 5. Новые
  `FieldId` (`X/Ix/E` × `ModelName`/`ApiKeyEnv`) с `apply_text` и подсказками
  (`field_description`: режимы, API-ключ-env, имя модели). API-ключ в UI — **имя
  env-переменной**, не секрет.
- **Пометка неподдерживаемого сэмплинга в облаке** (`screens/settings.rs`): в секции
  «Семплинг» при облачном провайдере подсекции параметры, не входящие в строгий
  диалект (расширения llama.cpp + `thinking`/`reasoning_effort`), помечаются **цветом
  значения** — заданное значение красится в `palette.warning` (янтарный/коричневый —
  внимание, но не тревога), видно без фокуса (`render_field_line(..., inactive)`).
  Подсвечиваются **только заданные** значения (`set`): незаданные `—` флагировать
  нечего. При фокусе внизу — развёрнутая подсказка `CLOUD_UNSUPPORTED_NOTE`
  («…значение сохранится для локальных моделей»). **Значение не трогается** (хранится в
  общем `default_sampling`/`impersonation_sampling`, заработает на локальной модели).
  Поддержанное подмножество — `cloud_supported_param`
  (temperature/top_p/frequency_penalty/presence_penalty/seed/max_tokens). Провайдер
  подсекции — `sampling_is_cloud` (для имперсонации в `shared` берётся движок
  ассистента). Поля не скрываем (значения переживают переключение на локальную модель).
  Эволюция: подсказка-при-фокусе → не видна на глаз → текстовая приписка → по просьбе
  заменена окраской значения (нагляднее, без повторов текста).
- **Тесты**: config (облачные режимы сериализуются/маппятся в провайдера, roundtrip,
  старый JSON → дефолты); wire (OpenAi-диалект вычищает расширения и шлёт `model`;
  LlamaCpp опускает `model=None`); supervisor (`resolve_api_key` env/missing; cloud
  без модели/ключа → `Disconnected`; с моделью+ключом → `Ready`+backend; cloud-embed
  без конфига → unavailable); settings (облачный режим показывает model/api-key и
  прячет llama-server-поля). **543 теста зелёные**, clippy/fmt чисты.
- **Не вошло в Фазу 0** (по плану ADR 0004): раскладка `shared/api` по подмодулям
  (`contract/openai/anthropic/managed`) — отложена к Фазе 2, когда появится
  `AnthropicClient` (до этого подмодуль `anthropic/` пуст, дробить ради одного
  семейства OpenAI преждевременно). Живые смоуки против реальных OpenAI/Gemini — по
  ключу, вне CI.

### Пост-M9: мульти-провайдерный инференс — Фаза 2 (Anthropic / Claude) (сделано)
- **Claude через отдельный протокол Messages API** ([ADR 0004](docs/decisions/0004-engine-contract-multi-provider.md)),
  как новая реализация трейта `EngineBackend` — слои выше движка не тронуты.
- **Новый модуль `shared/api/anthropic/`** (`client.rs` + `wire.rs`): `AnthropicClient`
  бьёт в `/v1/messages` (заголовки `x-api-key` + `anthropic-version`), парсит
  событийный SSE (`message_start`→usage.input_tokens; `content_block_start` tool_use→
  `ToolCall`; `content_block_delta`: `text_delta`→`Text`, `thinking_delta`→`Thoughts`,
  `input_json_delta`→`ToolCall` args; `message_delta`→`Usage`+`Finished` по
  `stop_reason`). `wire::build_request` транслирует доменный `ChatRequest`: system →
  **top-level** `system`; роли только user/assistant; tool-результаты → блоки
  `tool_result` **внутри user** (роль `tool` отсутствует); tool-вызовы → блоки
  `tool_use`; соседние сообщения одной роли **склеиваются** (Anthropic требует
  чередования); `max_tokens` обязателен (дефолт 4096); схема инструмента —
  `input_schema`. **Семплинг: только `max_tokens`** — новейшие Claude 4.x
  (opus-4-8/haiku-4-5) «зафиксировали» сэмплинг и отвергают `temperature`/`top_p`/
  `top_k` как deprecated (HTTP 400 на живом прогоне), поэтому их не шлём вовсе (UI
  помечает их у Claude как неподдержанные через `cloud_supported_param`). Тело ошибки
  не глотается (как у OpenAiClient).
  Эмбеддингов у Anthropic нет — `Embedder` не реализуется (RAG берёт отдельный, ADR 0002).
- **Конфиг**: `ServerMode`/`ImpersonationMode` += `Claude`; `CloudProvider` += `Claude`
  (`base_url`=`https://api.anthropic.com`, клиент сам добавляет `/v1/messages`).
- **Супервайзер**: `cloud_chat_setup` ветвится по протоколу провайдера — OpenAI/Gemini
  → `OpenAiClient` (+диалект), Claude → `AnthropicClient`. Эмбеддинги в режиме Claude →
  `UnavailableEmbedder` (Anthropic не умеет embeddings) с понятным логом.
- **Настройки**: `Claude` в циклах режимов (движок 5, имперсонация 6 значений), те же
  облачные поля (модель/API-ключ-env/base URL). **Пометка сэмплинга провайдеро-зависима**:
  `cloud_supported_param(provider, p)` — у Claude **`top_k` поддержан** (в отличие от
  OpenAI/Gemini), а penalties/seed — нет; `sampling_cloud_provider` отдаёт провайдера
  подсекции (shared-имперсонация наследует ассистента).
- **Claude в селекторе появился только теперь** (с реальным клиентом) — в Фазе 0 его
  намеренно не было, чтобы не шипить нерабочий пункт.
- **Тесты**: wire-трансляция (system top-level, tool_use/tool_result+склейка, input_schema,
  дефолт max_tokens, разбор всех SSE-событий), маппинг stop_reason, супервайзер (Claude
  chat → Ready+backend; Claude embed → unavailable), настройки (Claude помечает penalties/
  seed, но не top_k). **555 тестов зелёные** (+живой `#[ignore]`-смоук
  `MINDFORK_ANTHROPIC_KEY`), clippy/fmt чисты.
- **Раскладка `shared/api` по подмодулям (ADR 0004 §2) — сделана** (отдельным шагом
  после Anthropic, ради симметрии): `backend.rs`→`contract.rs`, `client.rs`+`wire.rs`→
  `openai/` (приватный `wire`, re-export `OpenAiClient`/`WireDialect`), `server.rs`→
  `managed.rs`, `anthropic/` уже был. Перемещения — `git mv` (история цела); публичная
  поверхность та же (re-export из `mod.rs`); внешние `shared::api::backend::*` →
  `::contract::*`. Итоговая структура: `contract` (трейты/типы), `openai`/`anthropic`/
  `managed` (реализации/запуск), `thoughts`, `mock`. **555 тестов**, clippy/fmt чисты.

### Пост-M9: вложенная конфигурация движка по режимам/провайдерам (сделано)
- **Конфиг движка стал вложенным** (`shared/config.rs`): раньше `EngineSettings`/
  `ImpersonationEngineSettings`/`EmbedSettings` делили **плоский** набор полей
  (`model_name`/`api_key_env`/`url`/`binary`/…), общий для всех режимов — переключение
  провайдера затирало чужие значения. Теперь у каждого режима/провайдера **своя
  под-секция**: переиспользуемые `ManagedSettings` (llama-server: binary/model/ngl/ctx/
  jinja/reasoning_format/no_mmap/host/port), `ExternalSettings` (url + опц. model_name),
  `CloudSettings` (model_name/api_key_env/url-override) — по экземпляру на каждого из
  `openai`/`gemini`/`claude`. У эмбеддингов отдельная `ManagedEmbedSettings` (меньше
  полей — host/jinja/no_mmap/ctx фиксирует супервайзер). Можно держать настроенными
  managed + OpenAI + Gemini + Claude одновременно и быстро переключаться.
- **Mode-aware аксессоры** `EngineSettings::cloud()`/`cloud_mut()` (и у impersonation/
  embed) отдают **активную** облачную под-структуру по `mode` (через
  `cloud_provider()`); общие хелперы `cloud_ref`/`cloud_mut` в config.rs. Это сохранило
  существующие `FieldId` в настройках — чтение/запись маршрутизируются по текущему режиму
  (видна лишь одна группа): `XUrl`/`XModelName` пишут `external.*` в external-режиме и
  `cloud_mut().*` в облаке; managed-поля → `managed.*`.
- **Без миграции** (решение пользователя): старый плоский `settings.json` читается через
  `#[serde(default)]` — незнакомые плоские поля движка молча игнорируются, под-секции
  дефолтные. Управляемый binary/модель/ключ нужно ввести один раз заново. Чаты/профили
  не затрагиваются. `schema_version` остался 1.
- **UI настроек** (`screens/settings.rs`): `model_fields`/embed-часть `tool_fields`
  строят поля из под-структур (хелперы `managed_rows`/`cloud_rows`); mode-driven
  visibility (как было). **Неподдерживаемые облаком параметры сэмплинга теперь
  скрываются** (фильтр `SAMPLING_PARAMS` по `cloud_supported_param`), а не красятся
  жёлтым — удалён warning-путь (`render_field_line(inactive)`, `CLOUD_UNSUPPORTED_NOTE`,
  `sampling_unsupported_in_cloud`); значения скрытых параметров сохраняются (заработают
  на локальной модели). Имперсонация в `shared` наследует фильтр провайдера ассистента.
- **Косметика шапки чата** (`screens/chat.rs::model_meta`): теперь по `engine.mode` —
  managed показывает `имя.gguf · Nk ctx`, external — `external.model_name`, облако — имя
  активной облачной модели (без ctx). Раньше в облачных режимах висели имя локального
  GGUF и его контекст.
- Прочие потребители обновлены: `supervisor.rs` (построение клиентов/`ManagedConfig` из
  под-структур; общий `managed_config(&ManagedSettings)`, хелперы
  `external_chat_setup`/`managed_chat_setup`), `main.rs::apply_env_overrides`
  (`config.engine.managed.*`/`external.url`). **556 тестов зелёные**, clippy/fmt чисты.

### Пост-M9: get_sampling/set_sampling по доступным в режиме параметрам (сделано)
- **Инструменты `get_sampling`/`set_sampling` теперь показывают/меняют только те поля
  сэмплинга, которые движок текущего режима реально принимает** — раньше схема
  `set_sampling` всегда несла полный набор (включая расширения llama.cpp), и в облаке
  модель пыталась менять `top_k`/`min_p`/… которые провайдер отвергает (`400`). Теперь
  модель видит ровно то, что применимо.
- **Единый источник истины** — `entities/sampling.rs::supported_sampling_fields(provider:
  Option<CloudProvider>) -> &[&str]` (зеркало wire-диалекта `restrict_to_strict` /
  `anthropic::wire`): `None` (локальный/external llama.cpp) — весь набор
  `SETTABLE_SAMPLING_FIELDS`; OpenAI/Gemini — `temperature`/`top_p`/`frequency_penalty`/
  `presence_penalty`/`seed`/`max_tokens`; Claude — только `max_tokens`. Им же теперь
  питается UI настроек: `screens/settings.rs::cloud_supported_param` делегирует туда
  (через новый `SamplingParam::field_name()`), убирая дублирование «что поддержано».
- **Инструменты mode-aware через `provider`** (`features/tools/introspection.rs`):
  `GetSampling::new(provider)`/`SetSampling::new(provider)` хранят провайдера chat-движка.
  `SetSampling::parameters` строит JSON-схему только из доступных полей (`field_schemas`
  — имя→схема, покрывает весь `SETTABLE_SAMPLING_FIELDS`); `invoke` **отбрасывает**
  недоступные ключи из аргументов и сообщает о них модели (а не молча применяет);
  `GetSampling::invoke` фильтрует вывод по тому же набору; **текст результата
  `set_sampling` тоже фильтруется** (`filter_to_supported`) — иначе в ленту и модели
  уезжал полный конфиг с десятками `null`-полей (сбивает с толку и модель, и
  пользователя); описания инструментов несут список доступных полей (`scope_note`).
  Провайдер берётся из `config.engine.mode.
  cloud_provider()` в `build_registry` (поле `ToolConfig.sampling_provider`); реестр
  **пересобирается и при смене режима** движка (`orchestrator/settings.rs`), не только
  `config.tools`.
- **Если в режиме нет ни одного доступного параметра — инструменты недоступны модели**:
  `effective_tool_ids` получил 5-й аргумент `sampling_provider` и отфильтровывает
  `get_sampling`/`set_sampling`, когда `supported_sampling_fields` пуст (страховочный
  путь — у всех текущих провайдеров есть хотя бы `max_tokens`). Константы
  `GET_SAMPLING_ID`/`SET_SAMPLING_ID` (re-export из `introspection`).
- **Тесты**: `supported_sampling_fields` (зеркало диалекта/подмножества); инструменты
  (облако прячет `top_k` в выводе get_sampling; Claude отбрасывает `temperature`/`top_k`
  в set_sampling и уведомляет; схема set_sampling по режиму; `field_schemas` покрывает
  весь набор); `effective_tool_ids` держит инструменты при наличии параметров; UI-фильтр
  настроек (прежний `cloud_hides_unsupported_sampling_params` — зелёный на делегации);
  результат `set_sampling` в облаке не несёт полного дампа конфига.
  **567 тестов зелёные**, clippy/fmt чисты.

### Пост-M9: FlashAttention + спекулятивное декодирование в managed-режиме (сделано)
- **Managed `llama-server` получил FlashAttention (`--flash-attn`) и спекулятивное
  декодирование (`--spec-*`)** — последнее позволяет подключать MTP-модели (например
  `mtp-gemma-4-12B-it.gguf`) через `--spec-type draft-mtp`. Только локальный managed
  (chat-сервер ассистента и сервер имперсонации); external/облако не затронуты,
  эмбеддинг-серверу неприменимо (он не генерирует токены).
- **Конфиг** (`shared/config.rs`): два enum'а — `FlashAttn { Auto, On, Off }`
  (`Auto` = флаг не передаётся, дефолт llama.cpp) и `SpecType` (`None`/`draft-simple`/
  `draft-eagle3`/`draft-mtp`/`ngram-simple`/`ngram-map-k`/`ngram-map-k4v`/`ngram-mod`/
  `ngram-cache`; serde kebab-case совпадает с CLI-значением; `as_arg()`/`label()`/
  `cycle(dir)`/`needs_draft_model()`). `ManagedSettings` расширен `flash_attn`,
  `spec_type` и черновыми полями `draft_model` (`-md`), `draft_gpu_layers` (`-ngld`),
  `draft_n_max` (`--spec-draft-n-max`), `draft_n_min` (`--spec-draft-n-min`). Всё через
  `#[serde(default)]` — старые `settings.json` без миграции.
- **Сборка аргументов** (`shared/api/managed.rs`): `ManagedConfig` несёт примитивы
  (`flash_attn: Option<String>`, `spec_type: Option<String>`, `draft_*`) — как
  `reasoning_format`; enum→строку конвертирует супервайзер (`managed_config`). `build_args`
  добавляет `--flash-attn`/`--spec-type`/`-md`/`-ngld`/`--spec-draft-n-max`/`-n-min`
  **только когда заданы** (незаданное → дефолт llama.cpp). Предполётная проверка файла
  расширена на черновую модель (`-md`): несуществующий путь → понятная ошибка до `spawn`
  (иначе `llama-server` тихо падает на её загрузке, а probe ждёт до таймаута).
- **UI настроек** (`screens/settings.rs`, секция «Модель/сервер», обе подсекции
  Ассистент/Имперсонация): FlashAttention и `--spec-type` — Choice-поля (←/→ цикл);
  черновые поля (`-md`/`-ngld`/n-max/n-min) показываются **только для типов draft-***
  (`needs_draft_model`), чтобы не загромождать секцию для ngram/none. Все поля с
  подсказками-описаниями (`field_description`). `managed_rows` отрефакторен с длинного
  списка FieldId-аргументов на структуру `ManagedFieldIds` (+ две const-инстанции
  `ASSISTANT_MANAGED_IDS`/`IMP_MANAGED_IDS`). Опциональные числовые поля парсятся через
  `parse_opt_num` (пусто → `None`/очистить, нечисло → прежнее).
- **Тесты**: config (as_arg/serde/cycle/needs_draft_model, roundtrip managed-секции с
  spec-полями); managed (флаги flash-attn/spec в `build_args`, дефолт без них,
  предполётная ошибка отсутствующего `-md`); settings (видимость flash/spec в managed,
  раскрытие черновых полей при cycle до draft-mtp, цикл flash-attn с сохранением).
  **579 тестов зелёные**, clippy/fmt чисты. Живой прогон на реальном
  `mtp-gemma-4-12B-it.gguf` — ручная проверка (нужны GPU/модель).

### Пост-M9: SelfModel MVP — «модель себя» агента (зонд, сделано)
- **Урезанный Phase 1 из банка идей** ([docs/self-model.md](docs/history/self-model.md)) по
  плану [docs/self-model-mvp.md](docs/history/self-model-mvp.md): пер-профильная «модель
  себя» агента — свободный текст о себе + цели + представление о собеседнике. Цель
  зонда — проверить **поведенческую** пользу (вспоминает ли локальная модель факты о
  пользователе и свои цели между чатами), а не строить полную мета-когнитивную
  машинерию. Сознательно **выкинуто**: `beliefs` с числовыми «силами», противоречия
  с detect/resolve, нарратив, версионная история, авто-рефлексия по таймеру, вся
  «глубокая осознанность» (Phase 4 исходного документа).
- **Ключевое отступление от исходного документа**: тот моделировал SelfModel через
  `ChatEffect` (как состояние `Chat`). В реальной архитектуре пер-профильные данные
  (заметки/RAG) пишутся инструментами **напрямую** в SQLite (`Storage` —
  потокобезопасный `Arc`+мьютекс), а `ChatEffect` существует только для мутаций
  `Chat` (его единолично владеет оркестратор). Поэтому **новых вариантов `ChatEffect`
  нет** — SelfModel-инструменты пишут через `ctx.storage`, как `note_save`; инвариант
  «единственный владелец `Chat`» не затронут.
- **Сущность** `entities/self_model.rs`: `SelfModel { profile_id, version, summary,
  goals: Vec<Goal>, user_model: UserModel }`; `Goal { id, description, status:
  Active/Completed/Abandoned }`; `UserModel { perceived_traits, current_interests,
  relationship_dynamic }`. Методы `new`/`is_empty` (учитывает только **активные**
  цели — модель из одних завершённых целей не делает блок информативным)/`add_goal`/
  `set_goal_status`/`render_for_prompt(max_chars)` (компактный блок «[Твоя модель
  себя] О себе:… / Активные цели:… / О собеседнике:…», усечение по символам ради
  8k-контекста). serde `#[serde(default)]`.
- **Хранение** `shared/storage/db.rs`: таблица `self_models(profile_id PK, data JSON,
  version, updated_at)` (`CREATE TABLE IF NOT EXISTS` → без миграции); методы
  `self_model_get`/`self_model_upsert` (INSERT OR REPLACE, версию ведёт хранилище;
  `version` хранится как `i64` — rusqlite не умеет `u64`). Изоляция по PK `profile_id`.
- **Инструменты** `features/tools/self_model.rs` (4, по шаблону `notes.rs`):
  `get_self_model` (чтение), `reflect` (возвращает текущую модель + рубрику для
  саморефлексии, без записи — «точка входа»), `update_self_model` (summary + цели:
  add/complete/abandon по id; управление целями свёрнуто в один инструмент),
  `update_user_model` (списки заменяют прежние). Мутаторы читают свежее из БД (а не из
  снимка `ctx.self_model`), чтобы видеть правки внутри хода; ошибки — текстом, не
  паника.
- **Опциональны, по умолчанию выкл** (как control-инструменты): в `all_tool_ids`, но
  **не** в `default_tool_ids` (`reconcile_tools` их не включает у существующих/новых
  профилей; тумблеры профиля строятся из `all_tool_ids`). DB-only → в
  `effective_tool_ids` проходят через `_ => true` без глобальных гейтов.
- **Интеграция** `app/orchestrator/generation.rs::start_generation`: снимок
  `self_model_get(profile_id)` кладётся в `ToolContext.self_model` и **компактно**
  подмешивается в `request.system` чистой функцией `inject_self_model(system, model,
  enabled)` — гейт: профиль включил `get_self_model` (opt-in). `ToolContext.self_model`
  пока `#[allow(dead_code)]` (инструменты читают из БД; снимок оставлен ради полноты
  снимка хода, как `chat_id`). `handle_done` не тронут (эффектов нет).
- **Тесты**: entity (merge целей, render/усечение, `is_empty` по активным целям); db
  (round-trip + изоляция по профилю + рост версии); tools (get/update/reflect,
  персист, no-op без аргументов); mod (в каталоге, не в дефолтах; проход
  `effective_tool_ids`); orchestrator (чистая `inject_self_model`: гейт/пустота/
  непустота). **593 теста зелёные**, clippy/fmt чисты.
- **Оценка зонда (сделано)**: живой `self_model_e2e_live` на Gemma 4 12B Q8_0 —
  модель в сессии 1 вызвала `update_user_model`+`update_self_model` (данные в БД), в
  сессии 2 (новый чат того же профиля) точно припомнила пользователя/цели через
  инъекцию в `system` + `get_self_model`. Критерий go/no-go — **go**, перешли к Tier 2.

### Пост-M9: SelfModel Tier 2-A — нарратив инсайтов + противоречия прозой (сделано)
- **Нарратив «я во времени»** — поле `SelfModel.narrative: Vec<NarrativeSegment>`
  (`{id, text, created_at}`, `#[serde(default)]`): короткие инсайты/наблюдения,
  append-only с потолком `MAX_NARRATIVE=50` (держим самые свежие, `add_insight`
  обрезает старые). **Противоречия влиты сюда прозой** — без отдельного типа/
  `severity`/detect-resolve-машинерии (осознанное решение Tier 2).
- **Инструмент `add_insight`** (`features/tools/self_model.rs`): записывает наблюдение
  в нарратив (включая замеченные противоречия прозой). Опционален (в `all_tool_ids`,
  не в `default_tool_ids`), пишет напрямую через `ctx.storage` (как остальные
  SelfModel-мутаторы, без `ChatEffect`). Рубрика `reflect` дополнена вопросом о
  противоречиях и упоминанием `add_insight`.
- **Рендер/инъекция**: `render_for_prompt` добавляет блок «Недавние наблюдения:» из
  свежих `NARRATIVE_IN_PROMPT=3` инсайтов (новейший первым; экономия контекста);
  `is_empty` учитывает нарратив (модель из одних инсайтов уже информативна).
- **Тесты**: entity (append/потолок/рендер свежих); tool (`add_insight` персист +
  пустой text → ошибка); каталог (`add_insight` в `all_tool_ids`, не в дефолтах).
  **595 юнит-тестов зелёные**, clippy/fmt чисты. Живой смоук `self_model_insight_e2e_live`
  (`#[ignore]`) на Gemma 4 12B: модель вызывает `add_insight`, инсайт-проза о
  противоречии («переменчивость в требованиях к объёму ответов») попадает в нарратив БД.
### Пост-M9: SelfModel Tier 2-B — экран просмотра модели себя (`F3`) (сделано)
- **Read-only экран просмотра** «модели себя» активного профиля
  (`screens/self_model.rs`, `SelfModelScreen`): описание, цели (со статусом
  ●/✓/✗), модель собеседника, нарратив (новые сверху). Открывается из чата по
  **`F3`**, закрывается `Esc`, `Ctrl+C` — выход; прокрутка `↑↓`/`PgUp`/`PgDn`/`Home`.
  Полноэкранная скруглённая панель + строка хоткеев (по образцу `chat_list`).
- **Данными владеет оркестратор** (`Storage`), поэтому экран **не** открывается сразу:
  `F3` → `ChatIntent::OpenSelfModel` → `runtime` шлёт `AppCommand::RequestSelfModel` →
  `handle_request_self_model` грузит `self_model_get(active profile)` и эмитит
  `AppEvent::SelfModelView(Box<Option<SelfModel>>)` → `runtime::apply_event` создаёт
  `ActiveScreen::SelfModel`. Новый вариант `ActiveScreen` (4-й экран); FSD соблюдён
  (`screens` не знает про `app`). Палитра экрана обновляется по `Settings` (тема).
- **Правка модели — пока только силами модели** (инструменты SelfModel); ручное
  редактирование через UI — задел Tier 3 (данные в SQLite, не в JSON).
- **Тесты**: экран (Esc/Ctrl+C-намерения раскладко-независимо, прокрутка-кламп,
  рендер пустой/заполненной модели без паники); runtime (`OpenSelfModel` шлёт
  `RequestSelfModel` и НЕ открывает экран сразу; `SelfModelView` открывает экран).
  **601 юнит-тест зелёный**, clippy/fmt чисты. Не тестируется на живой модели
  (интерактивный TUI — нужен настоящий терминал). Добавлено в оверлей помощи (`F1`/`?`).

### Пост-M9: SelfModel Tier 3-A — параметры нарратива/инъекции в настройках (сделано)
- **Размеры нарратива и объём инъекции в промпт вынесены из констант в конфиг**
  (`config.self_model: SelfModelSettings` — `max_narrative`/`narrative_in_prompt`/
  `prompt_cap`; `#[serde(default)]` → старые `settings.json` без миграции; дефолты =
  прежние 50/3/1200). Прежние захардкоженные `MAX_NARRATIVE`/`NARRATIVE_IN_PROMPT`/
  `DEFAULT_PROMPT_CAP` сняты.
- **Тип `SelfModelParams`** (`entities/self_model.rs`, аналог `ChunkParams` у RAG):
  `from_settings` санитизирует (max≥1; в промпт не больше, чем хранится; cap≥100).
  Методы сущности параметризованы: `add_insight(text, max_narrative)`,
  `render_for_prompt(prompt_cap, narrative_in_prompt)`. Протянут в `ToolContext.
  self_model_params` (строится из `config.self_model` в `start_generation`);
  инструменты (`add_insight`/`render_or_empty`) и `inject_self_model` используют его.
- **UI**: три числовых поля в секции «Инструменты» экрана настроек (рядом с RAG-
  чанкингом) с подсказками-описаниями (`field_description`); `FieldId::Sm*`.
- **Тесты**: config (дефолты в `partial_json_fills_defaults`); entity (кастомные
  параметры ограничивают хранение/инъекцию; санитизация несогласованных настроек).
  **603 юнит-теста зелёные**, clippy/fmt чисты.

### Пост-M9: SelfModel Tier 3-B — авто-рефлексия (фоновая) (сделано)
- **Авто-рефлексия** (`app/orchestrator/reflection.rs`): каждые N ответов ассистента
  в чате фоновая задача просит модель пересмотреть недавний разговор и **самой**
  обновить «модель себя». Включается `config.self_model.auto_reflect_every` (0 —
  выкл, по умолчанию). Это был отложенный пункт исходного плана («рефлексия после
  каждого N сообщений»).
- **Мини agentic-loop, не одноходовый запрос** (отличие от авто-названия): рефлексии
  даются SelfModel-инструменты (`get/update_self_model`/`update_user_model`/
  `add_insight`, пересечённые с набором профиля), и петля **исполняет** их вызовы
  (инструменты пишут напрямую в `Storage`). До `REFLECT_MAX_ROUNDS=6` раундов,
  таймаут 120с. Чат **не мутируется**, в UI ничего не стримится — рефлексия
  молчалива. Системное сообщение просит менять только изменившееся и не писать ответ
  пользователю, только вызывать инструменты.
- **Триггер** в `handle_done` (после успешного ответа): `maybe_auto_reflect`
  считает ответы (`reflect_counts: HashMap<chat_id,u32>`), при пороге сбрасывает и
  запускает. Гейты: фича включена, профиль включил `get_self_model` (как и инъекция),
  рефлексия не идёт уже (`reflect_cancel`, одна за раз), сервер `Ready`, переписки
  достаточно (есть дайджест). Чистая `due(count, every)` — тестируема.
- **Проводка**: поля `reflect_cancel`/`reflect_counts`/`reflect_done_tx` в
  `Orchestrator`; внутренний канал `reflect_done` (фон → петля снимает флаг
  `handle_reflect_done`); отмена при `Quit`. Дайджест — `rename_chat::
  build_conversation_digest` (как авто-название).
- **UI**: поле «Модель себя: авто-рефлексия (кажд. N)» в секции «Инструменты»
  настроек (`FieldId::SmAutoReflect`) с подсказкой.
- **Тесты**: `due` (порог/выключено); **604 юнит-теста зелёные**, clippy/fmt чисты.
  Живой смоук `auto_reflect_e2e_live` (`#[ignore]`, опрос БД — рефлексия без
  UI-события) на Gemma 4 12B: при `auto_reflect_every=1` после первого ответа модель
  **сама** (без явной просьбы) вызвала `update_user_model` → в БД появились черты/
  интересы собеседника.

### Пост-M9: SelfModel Tier 3-C — ручная правка модели в UI (`F3`) (сделано)
- **Экран `F3` стал редактируемым** (был read-only): правка описания себя, целей
  (добавить/переименовать/циклически менять статус `Space`/удалить `Del`), модели
  собеседника (черты/интересы списком через запятую, динамика отношений), удаление
  инсайтов нарратива (`Del`), полная очистка (`Ctrl+K` дважды — с подтверждением).
  Навигация `↑↓`/`Home`/`End`, `Enter` — правка (текстовый редактор-попап, описание
  себя многострочное), `Esc` — закрыть, `Ctrl+C` — выход.
- **Тип правки** `SelfModelEdit` (`entities/self_model.rs`) + чистый `SelfModel::
  apply_edit(edit) -> bool` (изменилось ли) и `cycle_goal_status`. Контракт UI↔
  оркестратор; списки черт/интересов **заменяются целиком**.
- **Поток** (правка не мутирует `Chat`, идёт через владельца `Storage`):
  `SelfModelIntent::Edit` → `AppCommand::UpdateSelfModel` → `handle_update_self_model`
  (загрузить/создать модель профиля → `apply_edit` → при изменении `self_model_upsert`
  → **переэмит** `AppEvent::SelfModelView`). `runtime::apply_event` обновляет **уже
  открытый** экран на месте (`set_model`, выделение сохраняется), а не пересоздаёт его.
  Вставка из буфера маршрутизируется в активный редактор поля.
- **Редактор** переиспользует `widgets::input_box::InputBox` (однострочный для
  полей/целей/списков, многострочный для описания себя) — тот же паттерн, что в
  экране настроек и переименовании чатов.
- **Тесты**: entity (`apply_edit` по всем операциям + no-op на повторе/несущ. id);
  экран (Enter→правка summary, `Space`/`Del` по цели, добавление цели + пустой no-op,
  `Ctrl+K` подтверждение/отмена, парсинг списков, рендер пустой/полной без паники);
  оркестратор (`update_self_model_persists_and_reemits` — правка сохраняется и
  переэмитится). **610 юнит-тестов зелёные**, clippy/fmt чисты. Ручное редактирование
  через UI больше не задел Tier 3 — сделано.

### Пост-M9: настраиваемое подтверждение `Ctrl+E`/`Ctrl+R` (сделано)
- **Необратимые в UI `Ctrl+R` (перегенерация) и `Ctrl+E` (удаление обмена) можно
  защитить подтверждением** — новая настройка `interface.confirm_destructive_keys`
  (`#[serde(default)]` → старые `settings.json` без миграции; по умолчанию
  **выключена**, прежнее мгновенное поведение). Тумблер «Подтверждать Ctrl+R /
  Ctrl+E» в секции «Интерфейс» экрана настроек (`FieldId::IConfirmKeys`, с подсказкой).
- **UX — единый модальный попап** (по согласованию: один общий тумблер на обе
  операции, попап Да/Нет): при включённой настройке нажатие открывает попап
  «Подтверждение» (`Enter` — да, `Esc` — нет; прочие клавиши игнорируются, попап
  остаётся открытым) вместо немедленного действия. Состояние — `ConfirmAction`
  (`Regenerate`/`DeleteExchange`) в поле `ChatScreen.confirm`; намерение
  (`RegenerateLast`/`DeleteLastExchange`) отдаётся только по `Enter` и только если не
  идёт генерация. Хелперы `trigger_destructive` (открыть попап или сразу отдать
  намерение) и `handle_confirm_key`; рендер — `render_confirm` (центрированный
  попап + `dim_background`, как справка/орфография). Флаг подхватывается из снимка
  настроек в `set_settings` (как палитра темы). См. spec §11.7.
- **Тесты** (`screens/chat.rs`): попап открывается и `Enter` подтверждает
  (`RegenerateLast`); `Esc` отменяет; прочие клавиши не закрывают попап и не
  печатаются в поле ввода; при выключенной настройке намерение отдаётся сразу.
  **614 тестов зелёные**, clippy/fmt чисты.

### Пост-M9: CoT (extended thinking) для режима Claude (сделано)
- **Claude (Anthropic) теперь умеет «мысли» (CoT)** — раньше `anthropic/wire::build_request`
  слал только `max_tokens`, поэтому reasoning не запрашивался. Приёмная часть уже была
  готова (клиент маппит `thinking_delta`→`ChatChunk::Thoughts`, лента рисует блок
  «мыслей»). Две фазы: **A** — CoT в чате без инструментов; **B** — CoT при tool-use
  (критично — инструменты центральны для приложения). См. CLAUDE.md (раздел про движок).
- **Phase A — включение thinking** (`anthropic/wire.rs`): запрос несёт
  `thinking:{type:"adaptive", display:"summarized"}` когда `sampling.thinking==Some(true)`
  (+ `output_config.effort` из `reasoning_effort`, кроме `none`). **`budget_tokens`/
  `reasoning_budget` не шлём** — модели Claude 4.x их отвергают (`400`); только adaptive,
  глубину задаёт `effort`. `display:"summarized"` нужен, чтобы текст «мыслей» приходил
  непустым (дефолт `omitted`). `supported_sampling_fields(Claude)` расширен на
  `thinking`/`reasoning_effort` (зеркало диалекта) → экран настроек и `get/set_sampling`
  показывают/правят их у Claude; `top_k`/penalties по-прежнему скрыты.
- **Phase B — подпись thinking-блока при tool-use** (главный нюанс протокола): Anthropic
  требует, чтобы assistant-ход с `tool_use` нёс **свой thinking-блок с `signature`** в
  рамках того же хода — иначе следующий запрос раунда `400`. Реализация:
  - Парс `signature_delta` (`anthropic/wire::AntDelta::SignatureDelta`) → новый чанк
    `ChatChunk::ThoughtsSignature(String)` (клиент эмитит; прочие бэкенды — нет).
  - `RoundOutput.thoughts_signature` копит подпись раунда; agentic-loop
    (`orchestrator/generation.rs`) при наличии подписи прикрепляет thinking-блок к
    `ApiMessage::assistant_tool_calls(...).with_thinking(...)` на строке push в историю
    запроса. Тип `ApiMessage.thinking: Option<ThinkingBlock>` (текст+подпись) — generic,
    использует только Anthropic-wire; `anthropic/wire::build_messages` ставит
    `AntBlock::Thinking` **первым** в assistant-ходе.
  - **Подпись живёт только в памяти одного `spawn_generation`** — НЕ персистится в
    доменном `Message`/JSON: Anthropic требует thinking только у самого свежего
    assistant-хода, а между ходами (перезагрузка/новый запрос/перегенерация) старые
    thinking авто-отбрасываются сервером. `message_to_api` (история) thinking не несёт —
    для Anthropic это корректно. Незавершённые tool-ходы из истории не пересобираются
    (перегенерация усекает по последнее user-сообщение), так что персист не нужен.
- **Контракт generic, бэкенды не ломаются**: новый вариант `ChatChunk` и поле
  `ApiMessage.thinking` игнорируются llama.cpp/OpenAI; llama.cpp-путь thinking как был
  (`reasoning_budget`/`<think>`). Exhaustive-матчи `ChatChunk` обновлены у всех
  потребителей (title/impersonation/reflection/subagent/fetch/openai-client/generation).
- **Тесты**: wire (thinking off по умолчанию; adaptive+summarized+effort при включении;
  `budget_tokens` не уходит; thinking-блок первым перед tool_use; без подписи блока нет;
  парс `signature_delta`); обновлены `supported_sampling_fields`/`set_sampling`-схема.
  **622 теста зелёные**, clippy/fmt чисты. Живые `#[ignore]`-смоуки
  (`anthropic/client.rs`, `MINDFORK_ANTHROPIC_KEY`) **проверены на реальном Anthropic
  API**: `extended_thinking_streams_thoughts_and_signature` (Phase A — приходят «мысли»
  и подпись) и `thinking_with_tool_use_round_trips_signature` (Phase B — второй раунд с
  переотправкой подписи проходит без `400`). Нюанс смоука Phase B: для гарантии
  thinking-блока нужен промпт с явным шагом рассуждения + `reasoning_effort: High` —
  на тривиальном запросе adaptive thinking рассуждение пропускает (подписи нет, что
  само по себе корректно — без «мыслей» блок и не нужен).
- **Известное ограничение**: `redacted_thinking`-блоки (редкая защитная реакция
  классификаторов) пока не обрабатываются — если такой блок придёт перед tool-use, его
  не переотправим (возможен `400`). Для обычного использования крайне редко; задел.

### Пост-M9: SelfModel — частичный показ длинного пункта на экране `F3` (сделано)
- **Симптом**: на экране «Модель себя» (`F3`) пункт списка с многострочным значением
  (длинное описание/инсайт), не помещающийся целиком в остаток высоты, **не
  отображался вовсе** — на его месте пустота, создававшая иллюзию конца списка.
- **Причина**: список рисовался виджетом `List`, который **целиком пропускает**
  многострочный элемент, не вмещающийся по высоте в остаток области (его
  `get_items_bounds` прерывается на `height + item.height() > max_height`). Окно
  списка чатов этим не страдает — там пункты однострочные.
- **Фикс** (`screens/self_model.rs`): виджет `List` заменён **ручным построчным
  рендером** визуальных рядов. Каждая логическая строка разворачивается в визуальные
  ряды (`wrap::wrap_line`), которые рисуются по одному `Paragraph`-ом в ряд области;
  хвостовой пункт, не влезающий по высоте, **обрезается по нижней границе** (виден его
  верх), а не пропускается. Добавлено персистентное поле `scroll` (первый видимый ряд)
  + чистая `adjust_scroll(scroll, sel_start, sel_height, view_h)`: держит выбранную
  строку видимой, а если она сама выше окна — прижимает к её **верху**. Подсветка
  выбранной строки прежняя — подложка на всю ширину ряда (база `Paragraph` красит всю
  область) + маркер `▌` на каждом её визуальном ряду. Навигация/выделение остались по
  логическим строкам (поведение клавиш не изменилось).
- **Тесты**: `adjust_scroll_keeps_selection_visible_and_pins_top_of_tall_item`
  (логика прокрутки) и `render_long_trailing_item_in_short_area_does_not_panic`
  (длинный инсайт в тесном окне). **625 тестов зелёные**, clippy/fmt чисты.

### Пост-M9: настраиваемый состав копии переписки (`F5`) (сделано)
- **Копирование переписки чата в буфер (`F5`) стало конфигурируемым**: новая секция
  `AppConfig.copy: CopySettings` (`copy_thoughts`/`copy_tool_calls`/`copy_tool_results`,
  `#[serde(default)]` → старые `settings.json` без миграции; все флаги по умолчанию
  **выключены** — прежнее поведение «только текст сообщений»). По выбору к блоку
  ассистента добавляются «мысли» (CoT, перед текстом), параметры вызовов инструментов
  (имя + аргументы) и их результаты.
- **`features/chat_export.rs::format_conversation` принимает `&CopySettings`**:
  опциональные блоки берутся из `Message.tool_calls` (имя/`arguments`/`result`) —
  отдельные tool-сообщения по-прежнему пропускаются (не дублируем). Tool-блок:
  заголовок `[Инструмент: имя]` (печатается, когда включён любой из двух флагов) +
  `Аргументы: {json}` / `Результат: …`. «Мысли» — блок `[Мысли]`. Сообщение
  ассистента без текста, но с tool-вызовами включается, если соответствующая опция
  включена (иначе пропускается, как раньше). Чистая логика — оркестратор
  (`chats.rs::handle_copy_chat`) передаёт `&self.config.copy`.
- **UI**: три тумблера в секции «Интерфейс» экрана настроек (`FieldId::ICopyThoughts`/
  `ICopyToolCalls`/`ICopyToolResults`) с подсказками-описаниями (`field_description`).
- **Тесты**: chat_export (по умолчанию только текст; «мысли» перед текстом; параметры/
  результаты под общим заголовком; сообщение из одних tool-вызовов); config (дефолты
  выключены). **631 тест зелёный**, clippy/fmt чисты.

### Пост-M9: связность заметок — MVP-зонд (Ярус 1, сделано)
- **Сдвиг памяти от накопления к интеграции** по плану
  [docs/notes-connectivity.md](docs/history/notes-connectivity.md). Мотив — из живого
  разговора с моделью (профиль «самоосознающий ИИ»): «инерция живёт **не в накоплении
  заметок, а в их связности**»; «личность — режим, в котором я отношусь к заметкам
  (что приму, что отвергну, что перепишу)». Раньше заметки умели только копиться
  (плоский список, поиск подстрокой, append-only). Зонд проверяет **поведенческую**
  пользу (начнёт ли модель переписывать дубль вместо записи почти-копии, найдёт ли
  семантический поиск релевантное, что подстрока пропускала) — go/no-go перед Ярусом 2
  (явный граф), как у SelfModel-зонда.
- **Три вещи MVP** (Ярус 1; граф/supersede/merge/консолидация — отложены):
  1. **Эмбеддинги на заметках + семантический `note_recall`**. Боковая таблица
     `note_vectors(note_id PK, profile_id, embedding TEXT)` (`CREATE TABLE IF NOT
     EXISTS` → без миграции; вектор — JSON-массив f32, **намеренно НЕ vec0**: заметок
     десятки–сотни, косинус считаем brute-force в Rust — `db::cosine` +
     `note_search_semantic`, изоляция `WHERE n.profile_id`). `note_recall` с запросом
     идёт семантическим путём; **мягкая деградация** (как RAG-реранкинг) — эмбеддер
     недоступен (`UnavailableEmbedder`)/нет векторов у заметок → откат на подстроку/теги
     (`semantic_recall(...) -> Option`, `None` → прежний `note_list`). Теги — фильтр
     поверх ранжирования.
  2. **`note_revise(id, content)`** — ядро интеграции: переписать заметку на месте
     (`db::note_update`, изоляция в `WHERE … AND profile_id`, `updated_at` растёт →
     всплывает в списке) + переэмбеддинг (best-effort). Чужой/несуществующий id →
     текст-ошибка, не паника. **В `default_tool_ids`** (безопасно, DB-only, центрально)
     → `reconcile_tools` включит и у существующих профилей.
  3. **Ворота совместимости в `note_save`** — после вставки эмбеддит (best-effort) →
     `note_vector_upsert` → семантически близкие существующие заметки дописываются в
     результат («Похожие заметки … перепиши через note_revise вместо новой записи»),
     так модель **на сохранении** видит дубль/конфликт и решает: оставить / переписать
     / не плодить. Это и есть «способность сказать нет».
- **Бэкфилл «старых» заметок** (`ensure_note_vectors`, `db::notes_missing_vectors`):
  заметки без вектора (созданные до фичи, импортированные, сохранённые при недоступном
  тогда эмбеддере) семантический поиск/ворота не видели — находка живого теста (модель
  звала `note_recall` и не видела ранее созданных заметок). Хелпер прозрачно (в начале
  `semantic_recall` и перед воротами `note_save`) эмбеддит недостающие заметки батчами
  (best-effort). По сути один раз на профиль — потом список пуст и вызов почти
  бесплатен (один SELECT).
- **Архитектура**: инструменты пишут **напрямую** через `ctx.storage` (как `note_save`),
  **без новых `ChatEffect`** (заметки — не состояние `Chat`); инвариант «единственный
  владелец `Chat`» не затронут. Оркестратор не менялся.
- **Тесты**: db (`note_update` только свой профиль; `note_search_semantic` ранжирует и
  изолирует; upsert заменяет вектор); tools (ворота показывают похожую при сохранении;
  семантический recall находит не-подстрочное совпадение; ревизия переписывает на
  месте; плохой/несуществующий id; бэкфилл «старых» заметок и в recall, и в воротах
  note_save); mod (`note_revise` в дефолтах). Семантические тесты — на `MockEmbedder`
  (мешок символов + L2). **642 теста зелёные**, clippy/fmt чисты. Живой прогон/оценка
  зонда на локальной модели — ручной шаг (критерий go/no-go в плане).
- **Зонд подтверждён на живой модели** (модель подтянула похожие заметки) → go,
  перешли к Ярусу 2 (ниже).

### Пост-M9: связность заметок — Ярус 2 (граф + ревизионная история, сделано)
- Продолжение Яруса 1 ([docs/notes-connectivity.md](docs/history/notes-connectivity.md)):
  связность заметок как **явная структура** + интеграция через замещение/слияние.
- **Граф связей**: таблица `note_links(profile_id, from_id, to_id, relation,
  created_at)` (PK от дублей, индексы from/to, изоляция по `profile_id`,
  `CREATE TABLE IF NOT EXISTS` → без миграции). Инструменты `note_link(from_id, to_id,
  relation)` — типы `supports`/`contradicts`/`refines`/`relates` (idempotent
  `INSERT OR IGNORE`, проверка `note_is_active` обоих концов, запрет самосвязи) и
  `note_neighbors(id, relation?)` — соседи в обе стороны (`UNION ALL` from/to) с
  направлением (→/←), исключая замещённые. `note_recall` подмешивает блок «Связанные
  заметки» — **spreading activation** (соседи топ-3 хитов, без уже показанных/
  замещённых, до `RELATED_IN_RECALL=5`), чтобы припоминание поднимало кластер.
- **Ревизионная история со «шрамом»**: таблица `note_superseded(note_id PK,
  profile_id, superseded_by, superseded_at)`. `note_supersede(old_id, content)` создаёт
  новую версию (`create_note` = insert + эмбеддинг) и помечает старую замещённой;
  `note_merge(ids[], content)` сводит ≥2 активные заметки в одну, исходные замещаются.
  Замещённые **скрыты** из `note_list`/`note_search_semantic`/`notes_missing_vectors`/
  `note_neighbors` (anti-join по `note_superseded`), но хранятся ради следа изменения и
  ручного восстановления. Простую правку на месте даёт `note_revise` (Ярус 1).
- **Архитектура без изменений**: все пять инструментов DB-only, в `default_tool_ids`
  (`reconcile_tools` включит существующим профилям), пишут напрямую через
  `ctx.storage`, без `ChatEffect`; оркестратор не тронут.
- **Тесты**: db (supersede скрывает из списка/семантики/`is_active`; связи и соседи в
  обе стороны + фильтр по типу + идемпотентность + исчезновение замещённого соседа);
  tools (link→neighbors + неизвестный тип/самосвязь; supersede прячет старую, показывает
  новую; merge сводит и требует ≥2; recall подмешивает связанную не-похожую заметку).
- **Правки по стресс-тесту на живой модели** (модель прогнала граф по краям):
  - **`note_link` — честный ответ о дубле.** Раньше повтор той же связи отвечал
    «Связь создана» оба раза (дубля в БД нет — есть PK + `INSERT OR IGNORE`, но
    сообщение вводило в заблуждение). Теперь `note_link_insert` возвращает «создано ли»
    (rowcount), инструмент отвечает «Связь уже существовала», если ничего не вставлено.
  - **`note_revise` — предупреждение целостности графа.** Правка на месте у узла с
    входящими рёбрами может сделать их неверными (создал `contradicts`, когда заметка
    отрицала X; после ревизии она X утверждает — ребро лжёт). `note_revise` теперь при
    наличии связей (`note_link_count`) добавляет предупреждение и направляет к
    `note_supersede` (сохранит замещённую версию, к которой относились связи). **Не
    запрет** — суждение остаётся за моделью (дешёвая правка у несвязанных не страдает).
  **649 тестов зелёные**, clippy/fmt чисты.
- Ярус 2 смержён (PR #86), стресс-тест на живой модели прошёл чисто → Ярус 3 (ниже).

### Пост-M9: связность заметок — Ярус 3 (консолидация / «сон», сделано)
- Завершение направления ([docs/notes-connectivity.md](docs/history/notes-connectivity.md)):
  интеграция, происходящая **вне одного чата** (исходная цель проекта).
- **`consolidate_notes`** — read-only entry-point (как `reflect` у SelfModel): обзор
  базы знаний — похожие пары (возможные дубли, попарный косинус ≥ 0.85), связи
  `contradicts`, заметки без связей — + рубрика. Логика обзора —
  `notes::build_consolidation_overview` (чистое чтение БД: `db::notes_with_vectors` +
  `db::note_links_all`). Сам ничего не меняет; дальше модель зовёт merge/supersede/
  revise/link.
- **Перенос связей при merge**: `note_merge` переносит рёбра исходных заметок на
  объединённую (`db::note_links_retarget` — дедуп по PK, самопетли отбрасываются), граф
  не осиротевает.
- **Авто-«сон»** (`app/orchestrator/consolidation.rs`, по образцу `reflection.rs`):
  каждые N ответов (`config.notes.auto_consolidate_every`, opt-in, 0=выкл) фоновая
  задача = **мини agentic-loop** с note-инструментами; ей скармливается обзор, модель
  сама консолидирует (исполняются её вызовы в `Storage`). Гейты: фича включена, профиль
  включил `note_merge`, активных заметок ≥ 2, сервер `Ready`, одна за раз; чат/лента не
  трогаются. Проводка зеркалит рефлексию: поля `consolidate_cancel`/`_counts`/`_done_tx`,
  внутренний канал, отмена при `Quit`, вызов в `handle_done`. Поле «Заметки:
  авто-консолидация (кажд. N)» в экране настроек.
- **Тесты**: db (`notes_with_vectors` только активные; `note_links_retarget` переносит/
  дедуплицирует/отбрасывает самопетли); tools (`consolidate_notes` показывает дубли/
  висячие; `note_merge` переносит связи на новую заметку); consolidation (`due`).
  **654 теста зелёные**, clippy/fmt чисты. Живая оценка авто-«сна» — ручной шаг.
- **Вне объёма (задел)**: vec0 при росте числа заметок; **связывание органов памяти**
  (notes / SelfModel-нарратив / RAG не связаны) — наблюдение живого теста, отдельное
  крупное направление.

### Пост-M9: режим хранения данных + резервное копирование/восстановление (сделано)
- **Режим хранения данных через маркер `location.json`** рядом с бинарником
  (`shared/paths.rs`, тип `DataLocation`: `portable`/`system`/`path`). По умолчанию
  (нет маркера/пустой/`portable`) — данные в подкаталоге **`data/` рядом с бинарником**
  (`PORTABLE_DATA_SUBDIR`; подкаталог отделяет данные от служебных файлов/кэшей сборки —
  в dev `target/debug/data/`, чтобы не мешались с артефактами). `system` → стандартная
  ОС-папка (крейт `directories`: Windows `%APPDATA%\mindfork-rs\data`, Linux
  `~/.local/share/mindfork-rs`); `path` → произвольный каталог (без под-папки `data/` —
  пользователь указал точное место). `Paths::discover()` читает маркер, резолвит корень
  и **создаёт его для всех режимов** (включая портативный `data/`); сам маркер
  **всегда** лежит рядом с бинарником (вне `data/`) — он про установку, не
  пользовательские данные. **Повреждённый JSON маркера — ошибка запуска** (опечатка в
  пути не должна молча увести на пустой набор данных). Логи/инстанс-гард/хранилище едут
  за корнем автоматически (`discover` → `logging::init`); `build.rs` копирует словари в
  `<profile>/data/dictionaries/`. Новый аксессор `paths.backups_dir()`. **Смена
  раскладки**: прежние портативные данные лежали прямо в каталоге бинарника — после
  перехода их нужно один раз перенести в `data/` (авто-миграция намеренно не делается).
- **Резервное копирование/восстановление** (`features/backup.rs`, CLI на `clap`):
  `mindfork backup [-o FILE] [-c 0..9]` и `mindfork restore <archive>` (без TUI,
  завершают процесс, как `import-lamellama`; берут single-instance — защита `data.db`
  от гонки с работающим приложением). `--import-lamellama` переведён на ту же
  clap-подкоманду `import-lamellama`.
- **Состав zip** (пути относительно корня): `settings.json`, `profiles.json`,
  `data.db` (+ sidecar `-wal`/`-shm`, если есть), `personal_dictionary.txt`,
  каталоги `chats/` и `dictionaries/` рекурсивно (тянут свои `*.bak`), все `*.bak`
  верхнего уровня (`settings.bak`/`profiles.bak` — у проекта `.bak` это
  `with_extension`, т.е. `settings.bak`, а не `settings.json.bak`), и каталог-
  «песочница» `tools.fs_root` — **только если** канонизированный путь внутри
  канонизированного корня (иначе пропуск). **Исключаются** `backups/`, `logs/`,
  `location.json`. Дедуп записей по имени в архиве. Степень сжатия `0..=9` (0 → store,
  иначе deflate; clap валидирует диапазон, дефолт 9). Имя по умолчанию —
  `backups/mindfork-backup-<дата>.zip` (chrono Local).
- **Восстановление транзакционно** (`restore_backup` → `RestoreOutcome`): (1)
  **валидация архива до любых разрушительных действий** (открытие zip + проверка
  `enclosed_name` каждой записи — анти-zip-slip); невалидный/повреждённый → `Err` без
  очистки. (2) Если в корне есть данные (`settings.json`|`profiles.json`|`data.db`|
  непустой `chats/`) → **авто pre-restore копия** прежних данных в `backups/`. (3)
  Очистка whitelisted-набора (тот же состав; **сохраняя** `backups/`/`logs/`/
  `location.json`; очистка по whitelist, не «снести всё» — посторонние файлы в корне
  целы) → распаковка указанного архива в корень. (4) **Если распаковка падает на
  полпути** и pre-restore копия была создана → **авто-откат**: повторная очистка +
  распаковка pre-restore (`RolledBack`); если и откат не удался → `Failed` с путём к
  pre-restore для ручного восстановления. О каждом исходе `main.rs` сообщает в
  консоль (stdout не занят TUI). `Err` рестора (до разрушений) и `Failed`/`RolledBack`
  дают ненулевой код выхода.
- **Зависимости**: `zip` (`default-features=false`, только `deflate` — без bzip2/zstd
  C-зависимостей), `clap` (derive), `directories`.
- **Тесты** (`backup.rs`, на tempdir): состав архива (ожидаемое включено, logs/
  backups/маркер исключены); `fs_root` включается только под корнем; round-trip
  restore (замена данных + pre-restore копия + удаление устаревшего чата); отказ на
  повреждённом архиве без касания данных; restore в пустой корень без pre-restore;
  store-уровень (0) даёт валидный архив; **откат при сбое распаковки** (крафт-архив с
  записью-файлом, конфликтующей с одноимённым переживающим очистку каталогом →
  `RolledBack`). `paths.rs`: дефолт-портативный, whitespace-маркер, round-trip трёх
  режимов, триминг пути, ошибка пустого пути/битого JSON, system-путь содержит имя
  приложения. **669 тестов зелёные** (+18 `#[ignore]`), clippy/fmt чисты.

### Пост-M9: прокрутка с VS16-эмодзи без мигания (сделано)
- **Полная перерисовка ленты при прокрутке с «съезжающими» VS16-эмодзи (`🕸️`/`🗂️`)
  больше не мигает.** Раньше `app/runtime.rs` звал `terminal.clear()`, который шлёт
  escape-очистку экрана `ESC[2J` — экран на миг гаснет (мигание). Цель перерисовки —
  затереть «висячие» артефакты: conhost/Command Prompt рисуют VS16-кластер шире
  модели ratatui, контент уезжает, и поячеечный diff не достаёт до уехавшего символа.
  Нужно переписать **каждую** ячейку явно, включая пробелы в пустых местах.
- **Тупик (важно)**: «облегчить» `clear()` до простого сброса заднего буфера
  (`swap_buffers`) НЕЛЬЗЯ — тогда diff сравнивает «пусто → кадр» и **пропускает
  ячейки-пробелы** (они равны пустому заднему буферу): пустые места не
  перерисовываются, старый контент/артефакт остаётся виден (визуально — «каша» из
  наложенного текста). diff эмитит только ОТЛИЧАЮЩИЕСЯ ячейки.
- **Фикс**: делаем задний буфер отличным от **любой** реальной ячейки. Заполняем
  текущий буфер символом-сентинелом `"\0"` (`current_buffer_mut().content`; такого
  символа не бывает в реальном контенте) и переносим его в задний буфер через
  `swap_buffers()` — **без** вывода на экран (flush не зовём). Тогда ближайший `draw`
  сдиффит «`\0` → реальный кадр»: отличается каждая ячейка (и пробелы тоже) → ratatui
  переписывает весь экран по ячейкам, **без** `ESC[2J` (без мигания) и с пробелами в
  пустых местах. Внутренний swap в `draw` восстанавливает инвариант «задний буфер =
  экран»; сам `"\0"` на экран не попадает. Триггер прежний (`take_feed_scrolled`:
  есть VS16 и была прокрутка). Внутристрочный сдвиг от широкого эмодзи присущ
  терминалу (одинаков с прежним `clear()`), не регресс. Поведение реального conhost
  юнит-тестами (TestBackend) не ловится — проверка на живом терминале.

### Пост-M9: компактные чипы статуса всех серверов в строке статуса (сделано)
- **Строка статуса показывала один сервер** (чат) одной пилюлей `● сервер: готов`,
  тогда как серверов уже несколько: чат, **эмбеддинги**, **имперсонация**. Теперь —
  кластер компактных **чипов** (глиф + метка), по чипу на активный сервер.
- **Модель данных**: новый снимок `ServerStatuses { chat, embed, impersonation }`
  (`shared/server.rs`); событие `AppEvent::ServerStatus` несёт его вместо одиночного
  `ServerStatus`. Оркестратор эмитит снимок при **любом** изменении любого статуса:
  фоновый probe чата (`status_rx`), probe имперсонации (`imp_status_rx` — раньше его
  статус в UI вообще не доходил), и смена настроек (`apply_chat/embed/impersonation_
  settings` через хелпер `emit_server_status`). `EngineManager` получил
  `embed_status` + метод `statuses()`; `apply_chat` больше не возвращает статус
  (читается через `statuses()`).
- **Статус эмбеддингов раньше не отслеживался** (`apply_embed` ленив, без probe):
  `EmbedSetup` получил поле `status` — `Ready` при реальном эмбеддере,
  `NotConfigured` для `UnavailableEmbedder`. Probe эмбеддингов нет → чип двухзначен
  (настроен/скрыт); реальный probe — задел.
- **Рендер** (`widgets/status_bar.rs`): глиф кодирует статус — `●` готов (success),
  `◐` подключение (warning), `✕` нет связи (error) / не настроен (warning). **Чат
  показывается всегда** и при обрыве несёт причину (`✕ чат: нет связи: <why>`) — он
  блокирует генерацию; **эмбеддинги/имперсонация** — чипами `эмб`/`имп` и **только
  когда настроены** (`NotConfigured`, в т.ч. имперсонация в `shared`, → чип скрыт),
  без текста причины (компактно). Глифы шириной 1 колонка (без эмодзи) — раскладка
  строки/сетки хоткеев не «съезжает». Хелпер `Palette::pill` удалён (его роль — у
  локальных `chat_chip`/`secondary_chip`/`chip` с разными глифами).
- **Тесты**: чипы видны только у настроенных серверов (имперсонация в `shared`/
  эмбеддинги выкл → скрыты); цвет глифа по статусу; прежние тесты раскладки/токенов/
  мыши переведены на `ServerStatuses` (ширина переноса сетки подобрана под более
  короткий чип). **671 тест зелёный**, clippy/fmt чисты.

### Пост-M9: SelfModel — ремонт по итогам живого теста (Opus 4.8 API) (сделано)
- **По следам ручного тестирования** «модели себя» на Opus 4.8 (Anthropic API)
  выявлены и устранены проблемы, из-за которых работал по сути только нарратив
  (`add_insight`). Ни новых инструментов, ни изменения схемы БД (модель — JSON-блоб
  `self_models.data`, новый config-флаг через `#[serde(default)]`) — **миграция не
  нужна**. Правки — рендер, аргументы инструментов, описания и один тумблер.
- **Троеточие в `get_self_model` (обрезка).** Причина: инструменты чтения звали тот
  же **компактный усечённый** рендер, что идёт в системный промпт
  (`render_for_prompt(prompt_cap=1200, narrative_in_prompt=3)`) — модель видела «…» и
  жаловалась. Фикс: новый `SelfModel::render_full()` (без усечения, весь нарратив,
  цели с id) — для `get_self_model`/`reflect`/эха после правок; `render_for_prompt`
  остаётся только для пассивной инъекции.
- **Цели не закрывались (write-once).** Механический блокер: `render_for_prompt`
  печатал цель как `- {описание}` **без id**, а `get_self_model` звал именно его →
  модель **никогда не видела id цели**, а `complete_goals`/`abandon_goals` требуют id.
  Фикс: `render_full` показывает цели как `- #a1b2c3 (активна) …` (короткий id =
  первые 6 hex UUID) + недавние закрытые (жизненный цикл виден); резолвер
  `SelfModel::match_goal` принимает короткий `#id` **или** полный UUID (по
  однозначному префиксу, регистронезависимо; неоднозначность/промах — понятный отчёт,
  не паника). Рубрики `reflect`/авто-рефлексии переписаны под «веди цели по #id».
- **`update_user_model` перетирал (беда «по настроению»).** Причина: списки
  заменялись целиком (`perceived_traits = […]`) — хорошее настроение → «добрейший»,
  плохое → «беспощадный», каждая правка уничтожала накопленное. Фикс (философия
  заметок «интеграция вместо накопления»): **merge-семантика** — `add_traits`/
  `remove_traits`, `add_interests`/`remove_interests` (дедуп без учёта регистра,
  Unicode) вместо замены; описание переосмыслено как *устойчивая, интегрированная*
  модель собеседника (не снимок настроения), мимолётное → `add_insight`. Ручная
  правка через `F3` (`SelfModelEdit::SetTraits`) остаётся заменой — там человек.
- **`update_self_model.summary`** — остаётся заменой (для связного самоописания
  естественно), но описание/рубрика просят **интегрировать** прежнее с новым, а не
  переписывать с нуля; текущее описание всегда на виду (инъекция + `render_full`).
- **Предсказуемость инструментов независимо от персоны.** (1) Нейтральный к персоне
  **«протокол ведения модели»** (`generation.rs::SELF_MODEL_MAINTENANCE_PROTOCOL`)
  подмешивается в системный промпт поверх персоны профиля: когда фиксировать
  изменения, «мимолётное — в наблюдения», **«точность важнее угодливости»** (прямой
  контр-приём против лести доброй персоны). Тумблер `config.self_model.
  maintenance_protocol` (`#[serde(default)]`, **по умолчанию вкл**), гейт — профиль
  включил `get_self_model`; подмешивается даже при пустой модели (bootstrap первой
  записи). (2) Фоновая **авто-рефлексия** (`reflection.rs`, `auto_reflect_every`)
  остаётся **opt-in** (0 по умолчанию — фоновые вызовы Opus API стоят токенов), но её
  системное сообщение переписано под новые семантику/цели-по-id — включив её,
  получаешь детерминированный пишущий путь, не зависящий от спонтанности модели.
- **UI**: тумблер «Модель себя: протокол ведения» в секции «Инструменты» экрана
  настроек (`FieldId::SmProtocol`, с подсказкой).
- **Тесты**: entity (`render_full` показывает id целей и не усечён; `match_goal`
  префикс/полный/неоднозначно; merge add/remove user_model); tools (merge не
  перетирает; закрытие цели по короткому `#id`; промах цели — отчёт); orchestrator
  (инъекция: протокол вкл при пустой модели, оба при непустой, выкл-поведение).
  **677 тестов зелёные** (+6), clippy/fmt чисты. Живая оценка на Opus 4.8 — следующий
  ручной шаг.

### Пост-M9: SelfModel Ярус 2 — шрам ревизии + консолидация нарратива (сделано)
- **По отзыву модели (Opus 4.8) на предыдущий фикс**: смена «перезаписи» на
  «накопление» (add_/remove_) вылечила *перетирание*, но у чистого накопления
  зеркальная болезнь — **раздувание и дрейф** (черт станет сорок, дубли/устаревшее
  топят сигнал, а `remove_traits` стирает без следа — «биография не помнит, что
  менялась»). Развилка по идентичности фичи решена: **модель себя = текущий рабочий
  снимок выводов, а шрам-биография живёт в нарративе** (не структурируем черты и не
  клонируем граф заметок — нарратив уже играет роль «self-directed notes»). Схему БД
  не трогали (JSON-блоб + новые методы сущности), миграция не нужна.
- **Шрам ревизии черт (проблема A)**: `update_user_model` получил опциональный `note`
  — что и почему изменилось; при наличии уходит в нарратив (`add_insight`), так
  изменение мнения о собеседнике оставляет **след**, а не стирается бесследно. Если
  `remove_traits`/`remove_interests` непусты, а `note` не передан — результат
  **напоминает** оставить пояснение (паттерн предупреждения `note_revise`). Черты
  остаются плоскими `Vec<String>` (без миграции).
- **Консолидация против раздувания (проблема B)**: новый инструмент
  `consolidate_narrative(remove[], add?)` — убирает наблюдения по `#id` (резолвинг как
  у целей: короткий hex-префикс или полный UUID, неоднозначность/промах → отчёт) и
  опц. добавляет одно **сводное** вместо них. Это интеграция (устойчивое поднимается в
  summary/черты, сырое сворачивается/вычищается), а не тихая потеря по FIFO-потолку —
  зеркало `note_merge`/`consolidate_notes` в идиоме модели себя, **без графа**.
  Инсайты теперь показываются в `render_full` с `#id`; сущность получила
  `match_insight`/`remove_insights` (общий резолвер `resolve_handle` с `match_goal`).
- **Опционален, DB-only**: `consolidate_narrative` в `all_tool_ids` (не в дефолтах),
  проходит `effective_tool_ids` через `_ => true`. Рубрика `reflect` и системное
  сообщение авто-рефлексии (`REFLECT_TOOL_IDS`) дополнены пунктом консолидации
  («не раздулся ли нарратив — подними устойчивое в summary/черты, сырое вычисти»).
- **Задел (не делали)**: фоновая авто-консолидация модели себя по таймеру (как
  `notes.auto_consolidate_every`) — сейчас консолидация ручная/через авто-рефлексию;
  и **унификация органов памяти** (нарратив ≈ второй экземпляр заметок) — по-прежнему
  крупное отдельное направление (см. notes-connectivity Ярус 3).
- **Тесты**: entity (`match_insight`/`remove_insights`; инсайты с `#id` в
  `render_full`); tools (ревизия черты с `note` → шрам в нарративе; удаление без
  `note` → напоминание; `consolidate_narrative` вычищает дубли + сводит + отчёт о
  промахе). **681 тест зелёный** (+4), clippy/fmt чисты. Живая проверка на Opus 4.8 —
  ручной шаг.

### Пост-M9: доводка модели себя — этап 1 (атомарная запись, фикс гонки) (сделано)
- План доводки — [docs/refinements.md](docs/history/refinements.md) (6 этапов + направление
  «нарратив как заметки»). Ветка `feat/self-model-refinements`.
- **Дефект**: все писатели «модели себя» (инструменты хода, авто-рефлексия, ручная
  правка `F3`) делали read-modify-write **тремя** вызовами (`self_model_get` → правка
  → `self_model_upsert`). Мьютекс `Db` сериализует отдельные вызовы, но не пару:
  авто-рефлексия (фоновая задача, работает параллельно с пользователем) читала модель
  → пользователь сохранял правку `F3` → рефлексия писала свою версию поверх, теряя
  правку.
- **Фикс** (`shared/storage/db.rs`): `Db::self_model_update(profile_id, |m| -> bool)`
  — SELECT + `mutate` + upsert под **одним** захватом мьютекса. Публичные
  `self_model_get`/`self_model_upsert` делегируют приватным `*_conn`-хелперам
  (`std::sync::Mutex` нереентерабелен → closure `mutate` **не может** звать методы
  `Db` — только правит значение `SelfModel`; в док-комментарии зафиксировано как
  предупреждение о дедлоке). `self_model_upsert` остался (симметричный примитив +
  тесты, `#[allow(dead_code)]`).
- **Писатели** переведены: 4 инструмента (`add_insight`/`update_self_model`/
  `update_user_model`/`consolidate_narrative`; побочные данные — unresolved-ручки,
  флаги удаления, счётчики — собираются захватом `&mut` в closure) и F3-обработчик
  (`orchestrator/mod.rs`). Читатели (`get_self_model`/`reflect`) — на `self_model_get`.
- **Тесты**: `self_model_update_is_atomic_under_concurrency` (2 потока × 50 записей →
  ровно 100 инсайтов, version=100 — при неатомарности терялись бы), no-op не пишет.
  **685 тестов зелёные** (+4), clippy/fmt чисты.

### Пост-M9: доводка модели себя — этап 2 (время, потолки нарратива/целей) (сделано)
- Этап 2 плана [refinements.md](docs/history/refinements.md): «я во времени» получает время,
  а «тихая потеря» (FIFO нарратива, безлимитный рост закрытых целей) — видимость и
  интеграцию.
- **Метки возраста** (`entities/self_model.rs`): чистый `age_label(at, now)` —
  сутки-гранулярность (сегодня/вчера/N дн./нед./мес./г.). **Гранулярность именно
  сутки — осознанно**: текст стабилен в пределах дня, поэтому инъекция «модели себя»
  в системный промпт не меняется от хода к ходу (prefix cache локальной модели
  страдает не чаще раза в день, сверх реальных правок). `render_full(now)` и
  `render_for_prompt(cap, n, now)` показывают возраст у целей (закрытые — от нового
  поля `Goal.closed_at`, `#[serde(default)]` → без миграции) и наблюдений. Экран `F3`
  добавляет дату к целям (локальная зона, как у инсайтов).
- **Вытеснение видимо**: `add_insight(text, max) -> Vec<NarrativeSegment>` возвращает
  вытесненные за потолок; инструмент `add_insight` сообщает «нарратив N/M» и **что
  ушло** (последний шанс поднять устойчивое). `narrative_fill_hint(max)` (≥80%
  заполнения) делает статичный протокол ведения **data-aware** — приписка «пора
  consolidate_narrative» подмешивается в системный промпт (`inject_self_model`) и в
  рубрику `reflect`.
- **Потолок закрытых целей**: `fold_closed_goals(keep, max_narrative)` сворачивает
  старейшие закрытые цели сверх `keep` в нарратив-шрам «[архив цели] …» и удаляет из
  структуры (та же философия «интеграция, не потеря»). Вызывается в `update_self_model`
  и F3-обработчике. Конфиг `SelfModelSettings.max_closed_goals` (default **10**,
  `#[serde(default)]`) → `SelfModelParams` (санитизация ≥1).
- **Трейд-офф prefix cache зафиксирован** (решение пользователя 2026-07-03): инъекция
  «модели себя» остаётся в `system`; потеря prefix cache при каждом обновлении —
  принятая цена за возможности. Перенос блока в конец истории **не делаем**. См.
  architecture.md §9.
- **Тесты**: entity (`age_label` корзины + время из будущего; `closed_at`
  ставится/снимается; `add_insight` возвращает вытесненное; `narrative_fill_hint`
  порог 80%; `fold_closed_goals` архивирует старейшие сверх keep); tools (`add_insight`
  отчёт о вытеснении; `update_self_model` сворачивает закрытые цели); config (дефолт
  `max_closed_goals`). **692 теста зелёные** (+7), clippy/fmt чисты.

### Пост-M9: доводка модели себя — этап 3 (каденция рефлексии по ватермарку) (сделано)
- Этап 3 плана [refinements.md](docs/history/refinements.md): авто-рефлексия перестаёт
  перечитывать один и тот же ранний материал и не теряет цикл при пропуске.
- **Три беды**: (1) дайджест рефлексии строился от **всей** истории чата → каждый
  цикл перечитывал уже отрефлексированное → дубли инсайтов, которые потом лечит
  консолидация; (2) счётчик каденции сбрасывался **до** гейтов «уже идёт»/«сервер не
  готов» — пропущенный запуск терял весь цикл (при `every=10` след. попытка через 10
  ответов); (3) счётчики in-memory (`reflect_counts`/`consolidate_counts`) терялись
  при рестарте, а данные-то per-profile.
- **Ватермарк в `Chat`** (`entities/chat.rs`): `reflected_upto: Option<usize>`
  (индекс-водораздел — сколько первых сообщений охвачено) + `reflected_at`
  (`#[serde(default, skip_serializing_if=Option::is_none)]` → старые файлы чатов без
  миграции, пустые не засоряют JSON; живут с чатом → переживают рестарт).
- **Каденция по окну** (`orchestrator/reflection.rs`): чистая
  `reflect_window(messages, reflected_upto) -> (wm, count)` — кламп ватермарка к длине
  (устойчив к усечению `Ctrl+R`/`Ctrl+E`) + счёт непустых ответов ассистента в окне
  `messages[wm..]`. `due(count, every)` как раньше. Дайджест —
  `build_conversation_digest(&messages[wm..])` (сигнатура уже принимала срез). Поле
  `reflect_counts` **удалено** из оркестратора (каденция считается от данных).
- **Сдвиг ватермарка только при спавне**: после **всех** гейтов (профиль включил
  модель себя, окно накопило `every`, дайджест непуст, рефлексия не идёт, сервер
  `Ready`) — `reflected_upto = messages.len()`, `reflected_at = now`, `mark_dirty`
  (дебаунс-сохранение). `modified_at` не трогается (рефлексия не поднимает чат в
  списке). Пропуск по любому гейту ватермарк не двигает → цикл не теряется, самоисцеляется.
- **Консолидация заметок** (`consolidation.rs`): осталась на in-memory счётчике (её
  дайджест — обзор заметок, не переписка), но **сброс счётчика перенесён после всех
  гейтов** — та же беда потери цикла закрыта.
- **Тесты**: чистые (`reflect_window` счёт от ватермарка + кламп после усечения);
  serde (старый JSON чата без полей ватермарка → дефолты, пустые не сериализуются);
  интеграционные (`maybe_auto_reflect` двигает ватермарк при готовом движке и **не**
  двигает при неготовом сервере). **697 тестов зелёные** (+5), clippy/fmt чисты.

### Пост-M9: доводка модели себя — этап 4 (модель собеседника) (сделано)
- Этап 4 плана [refinements.md](docs/history/refinements.md): соединение уже существующих
  данных о собеседнике с механизмами, которые их не использовали.
- **`user_model` → имперсонация** (4a): имперсонация (`Ctrl+U`) пишет реплику **за**
  собеседника, а `user_model` — буквально модель этого собеседника, но
  `build_impersonation_request` её не видел. Новый
  `UserModel::render_for_impersonation(cap)` подмешивается в системный промпт
  имперсонации (`build_impersonation_request(..., user_hint)`); гейт — тот же opt-in,
  что у пассивной инъекции (профиль включил `get_self_model`).
- **Поведенческие сигналы в дайджест рефлексии** (4b): `Ctrl+R` (перегенерация =
  «ответ не устроил»), `Ctrl+E` (удаление обмена), rewrite-раунды уже архивируются в
  `Chat.deleted` — сильнейшие имплицитные свидетельства о собеседнике, которых
  рефлексия не видела. Новый `DeletedCause` (`DeleteExchange`/`Regenerate`/`Rewrite`) +
  поле `DeletedExchange.cause` (`#[serde(default, skip_serializing_if)]` → без
  миграции; `record_deleted` получил параметр, 3 вызова обновлены). Чистая
  `behavior_markers(chat, since)` (`since` = прежний `reflected_at` из этапа 3) считает
  удаления за окно и дописывает к дайджесту блок «Поведенческие сигналы собеседника:…»
  (перегенерация/удаление — собеседнику, rewrite — собственному поведению агента;
  записи без `cause` не считаются). `REFLECT_SYSTEM_MESSAGE` поясняет, что маркеры —
  свидетельства (наблюдение, не осуждение).
- **Шрам при замене динамики отношений** (4c): `relationship_dynamic` — самое значимое
  поле модели собеседника, но менялось wholesale без следа (напоминание про `note`
  срабатывало только на `remove_traits`/`remove_interests`). Теперь замена **непустой**
  динамики без `note` тоже даёт напоминание-шрам (`replaced_dynamic`); первичное
  заполнение — без напоминания. Описание инструмента обновлено.
- **Тесты**: entity (`render_for_impersonation` Some/None + все поля); reflection
  (`behavior_markers` — счёт по причинам, фильтр `since`, старые без `cause` не
  считаются, собственное поведение отдельной фразой); impersonation
  (`build_impersonation_request` подмешивает `user_hint`); tools (замена непустой
  динамики без `note` → напоминание, первичное заполнение — нет, с `note` → шрам в
  нарративе). **701 тест зелёный** (+4), clippy/fmt чисты.

### Пост-M9: доводка модели себя — этап 5 (наблюдаемость фоновых задач) (сделано)
- Этап 5 плана [refinements.md](docs/history/refinements.md): рефлексия/консолидация —
  молчаливые фоновые задачи, их отказы легко не заметить; открытый `F3` после
  фоновой правки показывал устаревший снимок; мёртвое поле в контракте.
- **Исход задачи + серия неудач** (5.1): внутренние done-каналы рефлексии/
  консолидации несут `Result<(), String>` вместо `()`; ошибки логируются `warn`
  (с `profile_id`). Оркестратор считает подряд идущие неудачи (`reflect_failures`/
  `consolidate_failures`); на пороге `BACKGROUND_FAILURE_ALERT=3` **один раз**
  эмитит `AppEvent::Error`, дальше молчит до первого успеха (сброс) — наблюдаемость
  без спама.
- **Индикатор в статус-баре** (5.2): новое событие `AppEvent::BackgroundTask{kind:
  BackgroundKind, active}` (эмит при спавне/завершении). `ChatScreen` держит флажки
  (`set_reflecting`/`set_consolidating` — маппинг `BackgroundKind` делает runtime,
  чтобы `screens` не зависел от контракта `app`, FSD); `status_bar` рисует тихий
  muted-чип `✻ рефлексия`/`✻ сон заметок` (глиф шириной 1 колонка — раскладка сетки
  хоткеев не «съезжает»). Новый параметр `background: Option<&str>` в
  `render`/`height`/`lines`.
- **Свежесть открытого `F3`** (5.3): новое событие `AppEvent::SelfModelChanged` (без
  снимка). Эмитится после **успешной** рефлексии (`handle_reflect_done(Ok)`) и в
  `handle_done`, если в ходу были вызовы SelfModel-инструментов (детект по новому
  `self_model::is_self_model_tool` + `ALL_IDS`). `runtime::apply_event`: если экран
  `F3` открыт → шлёт `AppCommand::RequestSelfModel` (перезапрос свежего снимка); при
  закрытом — игнор (не открывает экран, в отличие от `SelfModelView`). Консолидация
  `SelfModelChanged` не шлёт (меняет заметки, не модель себя).
- **Чистка контракта** (5.4): удалено мёртвое поле `ToolContext.self_model`
  (`#[allow(dead_code)]`; инструменты читают из БД, рефлексия клала `None`) —
  контракт перестал давать ложное обещание «снимок доступен». Убрано из 8 мест
  конструирования (генерация, рефлексия, консолидация, testkit + 4 тест-контекста
  инструментов). Снимок для инъекции в промпт живёт локальной переменной в
  `start_generation`, не полем контекста.
- **Тесты**: runtime (`SelfModelChanged` при открытом `F3` шлёт `RequestSelfModel`,
  при закрытом — нет); оркестратор (3 неудачи подряд → одна ошибка, успех сбрасывает
  + шлёт `SelfModelChanged`; `handle_done` с self-model-вызовом → `SelfModelChanged`);
  tools (`is_self_model_tool` распознаёт группу); status_bar-тесты обновлены под новый
  параметр. **705 тестов зелёные** (+4), clippy/fmt чисты.

### Пост-M9: доводка модели себя — этап 6 (дедуп петель + политика одним источником) (сделано)
- Финальный этап плана [refinements.md](docs/history/refinements.md): механический рефактор
  без изменения поведения.
- **Общий тихий раннер** (6.1, `app/orchestrator/tool_loop.rs`): тело мини
  agentic-loop (стрим → аккумулятор вызовов → исполнение разрешённых инструментов →
  раунд, толерантность к Thoughts/ThoughtsSignature/Usage) дублировалось **дословно**
  в `reflection.rs` и `consolidation.rs` (различались лишь лимиты и метка лога). Теперь
  оно одно — `spawn_silent_loop(SilentLoop { backend, registry, ctx, request, allowed,
  cancel, max_rounds, timeout, label, profile_id, done_tx })` + приватный `run_rounds`.
  Общий спавн-хвост (таймаут + `warn`-лог + отправка исхода в done-канал). Предикат
  каденции `due` тоже переехал сюда (был в обоих модулях). Оба места стали тоньше:
  строят `SilentLoop` и зовут раннер; их `ReflectSpawn`/`ConsolidateSpawn`/
  `spawn_reflection`/`spawn_consolidation`/`due` удалены. **Основную петлю генерации
  сознательно не влили** — стриминг в UI, control-flow-инструменты, thinking-подписи
  Anthropic, usage, эффекты; её сложность не окупает общий сток (отмечено в доке модуля).
- **Политика ведения одним источником** (6.2): формулировки правил дублировались в
  `SELF_MODEL_MAINTENANCE_PROTOCOL` (`generation.rs`) и `REFLECT_SYSTEM_MESSAGE`
  (`reflection.rs`) и уже слегка разъехались. Введена канон-константа
  `self_model::POLICY_CORE` (интегрируй summary; веди цели по #id; merge user_model;
  мимолётное → add_insight; точность важнее угодливости; консолидируй нарратив). Оба
  текста собираются из неё: `self_model::maintenance_protocol()` = `POLICY_CORE` в
  обрамлении «ты сам ведёшь»; `reflection::reflect_system_message()` = преамбула +
  `POLICY_CORE` + пояснение о поведенческих сигналах (строятся в рантайме — `format!`
  не годится для `const`). Интерактивная рубрика `reflect` **намеренно** оставлена
  как есть — иной жанр (вопросы, не императив), покрывает те же темы.
- **Полевую группировку `BackgroundLoop`** (план 6.1) — **не делал**: рефлексия и
  консолидация различаются (у консолидации счётчик `consolidate_counts`, у рефлексии
  ватермарк), выигрыш косметический, а риск размазать инвариант по call-site'ам не
  окупается. Поля `*_cancel`/`*_failures`/`*_done_tx` оставлены на `Orchestrator`.
- **Тесты**: `due` — один набор (в `tool_loop`); `maintenance_protocol_wraps_policy_core`
  и `reflect_system_message_composes_from_policy_core` (композиция из `POLICY_CORE` +
  своё обрамление). Поведение петель проверяют прежние интеграционные тесты
  (`auto_reflect_advances_watermark_on_spawn` и др. — через общий раннер). **706 тестов
  зелёные**, clippy/fmt чисты. **Доводка модели себя (этапы 1–6, refinements.md) —
  завершена.**

### Пост-M9: нарратив «модели себя» как заметки — Ярус 1 (сделано)
- **Унификация органов памяти** по плану [docs/narrative-as-notes.md](docs/history/narrative-as-notes.md):
  нарратив SelfModel (`Vec<NarrativeSegment>` в JSON-блобе, FIFO-потолок, без
  семантики/дедупа/графа) переезжает в **обычные заметки** с зарезервированным тегом
  **`@self`**, получая бесплатно эмбеддинги, семантический поиск, ворота дублей,
  замещение со «шрамом» и авто-«сон». Это давно зафиксированный задел «связывание
  органов памяти» (notes-connectivity «вне объёма», architecture §9.9). Зонд, как у
  SelfModel/notes: минимальная реализация под go/no-go на живой модели.
- **Развилки подтверждены пользователем**: тег `@self` (лидирующий `@` не встречается
  в естественных тегах; коллизия редка и безобидна); self-заметки **скрыты** из
  пользовательского `note_recall` (память о себе ≠ память о собеседнике — смешение
  выдачи рискованно, полное смешение с маркером `[о себе]` отложено в Ярус 2).
- **Унификация на уровне хранилища, не выдачи** (`features/tools/notes.rs`): self-заметки
  делят таблицы/эмбеддинги/граф/консолидацию с обычными, но фильтром по тегу
  (`is_self_note`) исключены из пользовательского `note_recall` (подстрочный путь
  `list_user_notes` — читаем без лимита, отбрасываем self, затем усечение; семантический
  `semantic_recall` — фильтр + запас кандидатов; spreading activation тоже пропускает
  self), ворот `note_save` и обзора консолидации (`build_consolidation_overview` +
  гейт «≥2 заметок» авто-«сна» считают только пользовательские). DB-методы не тронуты.
- **Запись наблюдений → @self-заметки**: `add_insight` = `create_note(@self)` + **ворота**
  (ядро гипотезы: `self_note_similar` — семантически близкие наблюдения с подсказкой
  переписать через `note_revise`/`note_supersede` вместо почти-дубля); шрам ревизии
  `update_user_model.note` → @self-заметка; `fold_closed_goals` теперь **возвращает**
  шрамы (`Vec<String>`), а `update_self_model`/F3-обработчик пишут их @self-заметками.
- **Чтение наблюдений → из заметок по свежести**: `render_for_prompt`/`render_full`
  приняли параметр `recent: &[NarrativeSegment]` (готовит вызывающий — `self_notes_recent`;
  главный ripple, `render_*` перестали быть чистыми по нарративу — осознанная цена).
  Наблюдения в `render_full` — с **полным** id (их переписывает note_revise/supersede),
  цели — прежний `#id`. `inject_self_model` (оркестратор) и `get_self_model`/`reflect`
  (инструменты) читают свежие self-заметки; `is_empty` перестал учитывать нарратив.
- **Консолидация наблюдений**: `consolidate_narrative` **удалён** (сильнее — note-
  инструменты: замещение со «шрамом»); из `REFLECT_TOOL_IDS` вместо него
  `note_revise`/`note_supersede`/`note_merge` (`note_recall` не даём — он скрывает
  self-заметки; полные id модель берёт из `get_self_model`). `note_supersede`/`note_merge`
  теперь **наследуют теги** исходных (в т.ч. `@self` — self-заметка при замещении/слиянии
  не «выпадает» в пользовательскую выдачу; `Db::note_get`). `POLICY_CORE` и рубрика
  `reflect` переписаны на note-идиому; убраны entity-методы `add_insight`/`remove_insights`/
  `narrative_fill_hint`/`match_insight` (поле `narrative` оставлено для бэкфилла и
  реконструкции снимка `F3`).
- **Бэкфилл** (`migrate_self_narrative`): одноразовый идемпотентный перенос блоб-нарратива
  → @self-заметки (**атомарный drain** нарратива под мьютексом → без дублей даже при
  повторе; `created_at` сохранён; вектор эмбеддится лениво). Зовётся best-effort в
  `start_generation` перед чтением наблюдений (под тем же opt-in-гейтом `get_self_model`).
- **F3**: снимок `SelfModelView` **реконструирует** `narrative` из self-заметок (только
  для показа — снимок не персистится, запись идёт над реальной, пустой по нарративу
  моделью); экран не менялся. `SelfModelEdit::DeleteInsight` → удаление self-заметки
  (`Db::note_delete` стал профиле-скоупным + снимает вектор); `Clear` → сносит @self-заметки
  и блоб.
- **Инварианты целы**: заметки DB-only (без `ChatEffect`), изоляция по `profile_id`, без
  миграций схемы (`@self` — обычный тег; поле `narrative` — `#[serde(default)]`).
- **Тесты**: notes (recall/ворота/обзор скрывают self; supersede/merge наследуют теги;
  бэкфилл переносит и идемпотентен; note_delete изолирован + снимает вектор); self_model
  (add_insight пишет @self + ворота показывают похожее; get_self_model собирает из
  заметок; шрам черты → self-заметка; свёртка целей → self-заметки); entity (`render_*`
  над `recent`; `is_empty` без нарратива; `fold_closed_goals` возвращает шрамы);
  оркестратор (инъекция читает наблюдения; F3 DeleteInsight/Clear по заметкам).
  **711 тестов зелёные**, clippy/fmt чисты. **Оценка зонда: GO** — прогон
  `self_model_gate_e2e_live` на Gemma 4 31B + bge-m3: ворота `add_insight` показали
  похожее наблюдение, модель интегрировала (`note_merge`/`note_revise`), 3/3 прогона
  почти-дубль сведён. Мягкая деградация проверена (embed без `--embeddings` → ворота
  в пусто, ничего не падает). → Ярус 2 (ниже).

### Пост-M9: нарратив как заметки — Ярус 2 (структура: релевантность + граф) (сделано)
- Продолжение Яруса 1 ([docs/narrative-as-notes.md](docs/history/narrative-as-notes.md)):
  наблюдения-заметки получают **структуру**. Развилки подтверждены пользователем:
  объём **A+B** (C — семантические ворота черт — отложена); релевантная инъекция — в
  **system-промпт** (prefix-cache инвалидируется каждый ход, принятая цена, §9.3).
- **A. Инъекция по релевантности** (`orchestrator/generation.rs`): наблюдения в
  системный промпт подмешиваются не только по свежести, но по **релевантности к
  последней реплике** — старое, но относящееся к теме наблюдение всплывает, когда
  тема возвращается. `notes::self_notes_relevant` (бэкфилл векторов → эмбеддинг
  запроса → поиск среди @self по косинусу, top-K); `blend_self_notes` (K релевантных
  + гарантия свежайшего для непрерывности, дедуп, потолок); собраны в
  `injection_recent`. **Инъекция перенесена из sync `start_generation` в async-задачу
  `spawn_generation`** — релевантность требует async-эмбеддинга последней реплики, а
  command-handler синхронный; `GenSpawn` несёт `self_model`/params/флаги/`last_user`,
  оценка токенов эмитится после инъекции. `ensure_note_vectors` обобщён на
  `(storage, embedder, profile)`. Мягкая деградация к свежести (нет эмбеддера/реплики).
- **B. Граф над наблюдениями**: механика (`note_link`/`note_neighbors`) уже работала
  на self-заметках (они заметки, тег не фильтруется) — Ярус 2 её **использует и
  показывает**. (B1) `REFLECT_TOOL_IDS` += `note_link`/`note_neighbors`, `reflect`-
  сообщение/рубрика нуджат связывать соотносящиеся наблюдения (`contradicts`/`refines`/
  `relates`) по полному id из `get_self_model`. (B2) `notes::self_related_block` —
  блок «Связи наблюдений»: рёбра графа, касающиеся показанных наблюдений (структура
  «что с чем соотносится», которой плоский список не даёт), **только self↔self**;
  соседа вне показанного набора приводит с текстом (spreading activation); дедуп
  рёбер. `self_model::render_self_read` (= `render_full` + блок) в `get_self_model`/
  `reflect`. Пассивная инъекция граф **не** показывает (компактность промпта). Обзор
  self-консолидации отложен.
- **Не вошло**: C (семантические ворота черт `user_model` — почти-дубль черты),
  обзор self-консолидации, кросс-органные связи (self↔user↔RAG, Ярус 3), полное
  смешение выдачи (`[о себе]` в общем recall, Ярус 3) — по решению об объёме.
- **Тесты**: `self_notes_relevant` (ранжирование + фильтр @self + пустой запрос);
  `blend_self_notes` (релевантные вперёд, свежее гарантировано, дедуп, потолок);
  `injection_recent_surfaces_relevant_over_fresh` (старое релевантное поднимается над
  свежим — детерминированно на `MockEmbedder` + temp-хранилище); `get_self_model`
  показывает «Связи наблюдений» + тип связи; `REFLECT_TOOL_IDS` содержит граф-
  инструменты; `reflect_system_message` нуджит `note_link`. Живой смоук
  `self_model_graph_e2e_live` (`#[ignore]`, `spawn_orch_live`): модель связывает
  противоречащие наблюдения. **716 тестов зелёные**, 20 `#[ignore]`, clippy/fmt чисты.
- **Граф-смоук — GO** (`self_model_graph_e2e_live` на Gemma 4 31B + bge-m3): модель
  самостоятельно записала два противоречащих наблюдения (`add_insight`), увидела их с
  полными id через `get_self_model` и связала ребром `contradicts` (`note_link`) —
  ребро появилось в графе наблюдений. Релевантность/граф на живой модели используются.

### Пост-M9: нарратив как заметки — Ярус 2, шаг C (ворота родственных черт) (сделано)
- **Семантические ворота родственных черт `user_model`** — отложенный шаг C Яруса 2
  ([docs/narrative-as-notes.md](docs/history/narrative-as-notes.md)), **зеркало ворот
  `add_insight`**, но над плоским списком черт собеседника: при `add_traits` для
  каждой реально добавленной черты ищется ближайшая среди **прежних** (существовавших
  до этой правки) выше порога косинусной близости `TRAIT_SIMILARITY=0.72`; при
  наличии близкой инструмент её **показывает** и просит решить: **дубль** (слить через
  `remove_traits`) или **противоречие** (записать наблюдением `add_insight`). **Мягкие
  ворота** (не запрет — решение за моделью, обе черты сохраняются).
- **Порог 0.72 откалиброван по живому тесту, не угадан.** Исходный 0.85 (по аналогии
  с обзором консолидации заметок) на живом прогоне **пропускал настоящие перефразы**:
  bge-m3 сжимает короткие черты в узкую полосу, «любит лаконичность» ↔ «ценит
  краткость в ответах» = **0.77** (< 0.85 → ворота молчали, хотя это дубль). Калибровка
  через наш клиент: перефразы 0.73–0.83, не-родственные 0.51–0.69 → порог 0.72.
  **Ключевое наблюдение:** bge-m3 сближает черты по **измерению/теме**, не по
  направлению смысла, поэтому в полосу попадают и **антонимы** («любит краткость» ↔
  «любит длинные объяснения» = 0.71). Это не баг, а **разворот замысла**: ворота
  переформулированы с «почти-дубль» на «родственная черта — дубль или противоречие?»
  (согласуется с философией «интеграция вместо накопления» + противоречия в нарративе).
  Замер curl+awk давал испорченные значения (0.97 на всё, 1.000 на антонимах) — верны
  значения через наш `OpenAiClient` (dim=1024, реальный bge-m3).
- **Эмбеддинг черт на лету** (`self_model::near_duplicate_traits`): у черт нет
  хранимых векторов (`Vec<String>`, в отличие от заметок с `note_vectors`), поэтому
  добавленные + прежние эмбеддятся **одним запросом** и сравниваются `cosine`
  (сделан `pub(crate)` в `notes.rs` для переиспользования). **Мягкая деградация**:
  эмбеддер недоступен / нестыковка числа векторов → пусто (как `add_insight`/
  `note_save`). Гейт срабатывает только при реально добавленных чертах (пустой набор
  — без эмбеддинг-вызова).
- **Снимок прежних черт — внутри атомарной правки** (`self_model_update` closure,
  захват по `&mut existing_before_traits`), сам эмбеддинг — **после** (async/storage
  вне closure, как шрамы ревизии). Реально добавленные вычисляются вне closure
  (запрошенные ∖ прежние, регистронезависимо, без внутрибатчевых повторов).
- **Черты остаются `Vec<String>`** (без миграции схемы); описание `update_user_model`
  дополнено упоминанием ворот. **Инварианты целы**: DB-only, изоляция по `profile_id`,
  без `ChatEffect`.
- **Тесты**: `add_trait_gate_surfaces_near_duplicate` (родственная черта поднимает
  ворота + подсказка `remove_traits`), `add_trait_gate_silent_for_dissimilar`
  (неродственная молчит, обе черты сохранены). На `MockEmbedder` («aaaa bbbb» ↔ «aaab»
  cosine ≈ 0.89 > порога). Живой `#[ignore]`-смоук `trait_gate_e2e_live`
  (`spawn_orch_live`, реальный эмбеддер). **718 тестов зелёные**, 21 `#[ignore]`,
  clippy/fmt чисты.
- **Смоук — GO** (Gemma 4 31B + bge-m3): модель добавила похожую черту → ворота
  показали родственную («любит лаконичность» ≈ «ценит краткость в ответах»), модель
  ответила «Это дубль» и свела к одной черте через `remove_traits`. Ворота
  срабатывают, распознавание дубля/интеграция работают.

### Пост-M9: связывание органов памяти — Ярус 3, Путь 1 (кросс-органные связи) (сделано)
- **Исходная дальняя цель связности** ([docs/narrative-as-notes.md](docs/history/narrative-as-notes.md)):
  связать органы памяти (self-заметки ↔ пользовательские заметки ↔ RAG). Развилка
  (подтверждена пользователем) — **Путь 1: кросс-органные рёбра** (Пути 2 «смешение
  выдачи `[о себе]`-маркером» и 3 «RAG ↔ заметки» отложены).
- **Ключевое: механика графа уже была кросс-органна** — `note_link`/`note_neighbors`
  берут любые id **без фильтра тега**, так что связать self-заметку с пользовательской
  можно было и раньше; просто это нигде не всплывало. Ярус 3 (1) **показывает** такие
  рёбра в выдаче и (2) даёт модели **адресуемость** пользовательских заметок. Органы
  остаются РАЗДЕЛЬНЫМИ по хранению/поиску — всплывает лишь **намеренно созданное**
  моделью ребро (не «загрязнение» выдачи, в отличие от Пути 2).
- **Показ кросс-рёбер** (`features/tools/notes.rs`): `related_block` (пользовательский
  `note_recall`) больше не пропускает соседей-наблюдения «о себе» — показывает их с
  пометкой **`[о себе]`**; `self_related_block` (чтение «модели себя») показывает
  соседей-пользовательские заметки с пометкой **`[заметка]`**. Обычный поиск/spreading
  self-заметки по-прежнему не тащит (сокрытие Яруса 1 цело) — снят фильтр **только** для
  соседей по явному ребру.
- **Адресуемость** (`format_notes`): `note_recall` теперь выводит **id** заметок —
  иначе модель не сошлётся на пользовательскую заметку в `note_link`. Заодно закрыт
  давний разрыв: описание `note_link` обещало «id из note_recall», а id не выводился
  (заметки из recall были неадресуемы даже для обычного графа). Новая const
  `NOTE_RECALL_ID`.
- **Нудж** (`orchestrator/reflection.rs`, `self_model::Reflect`): `note_recall` добавлен
  в `REFLECT_TOOL_IDS` (даёт рефлексии id пользовательских заметок; self-заметки он
  по-прежнему скрывает); системное сообщение авто-рефлексии и рубрика интерактивного
  `reflect` предлагают связывать наблюдение «о себе» с фактом «о собеседнике» (id
  наблюдения из `get_self_model`, id заметки из `note_recall`).
- **Инварианты целы**: DB-only, изоляция по `profile_id`, без `ChatEffect`, без миграций
  схемы (граф `note_links` уже был). **Тесты**: `recall_shows_note_ids`;
  `recall_surfaces_cross_organ_self_neighbor_marked` (`[о себе]` + первичная выдача без
  self); `self_related_block_surfaces_cross_organ_user_note_marked` (`[заметка]`);
  `reflect_tools_include_graph` (+`note_recall`); `reflect_message_nudges_cross_organ_
  linking`. **722 теста зелёные**, 22 `#[ignore]`, clippy/fmt чисты.
- **Смоук — GO** (`cross_organ_link_e2e_live`, Gemma 4 31B + bge-m3): модель записала
  факт «о собеседнике» (`note_save`) и наблюдение «о себе» (`add_insight`), затем через
  `get_self_model` + `note_recall` (взяла id заметки) вызвала `note_link` → создала
  **кросс-органное ребро** `contradicts` («наблюдение о многословности ↔ заметка о
  предпочтении краткости»). Модель пользуется кросс-связями осмысленно; критерий go — **go**.
- **Отложено**: Путь 2 (`[о себе]` в общем recall, за тумблером), Путь 3 (RAG ↔
  заметки), обзор self-консолидации, vec0 при росте числа заметок.

### Пост-M9: связывание органов памяти — Ярус 3, Путь 2 (смешение выдачи) (сделано)
- **Полное смешение выдачи за тумблером** ([docs/narrative-as-notes.md](docs/history/narrative-as-notes.md),
  Путь 2, подтверждён пользователем): `config.notes.recall_includes_self`
  (`#[serde(default)]`, по умолчанию **выключено**) включает показ self-заметок
  (`@self`) в общем `note_recall` с пометкой **`[о себе]`**. Выключено = поведение
  Яруса 1 (self скрыты: память о себе ≠ память о собеседнике) — реверс сокрытия
  сознательно **за тумблером**, чтобы проверить безопасно.
- **Проводка**: `NotesSettings.recall_includes_self` → `ToolContext.recall_includes_self`
  (прокинут во всех местах конструирования — генерация/рефлексия/консолидация/testkit +
  4 тестовых ctx инструментов). Пути recall (`list_user_notes` подстрочный +
  `semantic_recall`) при включённом тумблере не отбрасывают self-заметки; `format_notes`
  помечает их `[о себе]` и **скрывает служебный тег `@self`** из показа тегов (пометка
  его заменяет). Тумблер в секции «Инструменты» экрана настроек
  (`FieldId::NotesRecallIncludesSelf`, с подсказкой).
- **Инварианты целы**: DB-only, изоляция по `profile_id`, без `ChatEffect`, без миграций;
  по умолчанию поведение не меняется. **Тесты**: `recall_includes_self_notes_when_enabled`
  (self в обеих ветках recall с пометкой, без `@self`); дефолт выключен
  (`partial_json_fills_defaults`). **723 теста зелёные**, 23 `#[ignore]`, clippy/fmt чисты.
- **Смоук — GO** (`recall_includes_self_e2e_live`, Gemma 4 31B + bge-m3): при включённом
  тумблере `note_recall` вернул и пользовательскую заметку, и наблюдение «о себе» с
  пометкой `[о себе]`, а модель в ответе **чисто разделила органы** («— О вас: любит
  краткость; — О себе: склонен к многословию») — **загрязнения нет**, пометка работает
  как задумано (go по безопасности смешения).
- **Отложено**: Путь 3 (RAG ↔ заметки), обзор self-консолидации, vec0 при росте заметок.

### Пост-M9: связывание органов памяти — Ярус 3, Путь 3 (RAG ↔ заметки) (сделано)
- **Третий орган памяти (база знаний RAG) связан с заметками/наблюдениями**
  ([docs/narrative-as-notes.md](docs/history/narrative-as-notes.md), Путь 3): заметка может
  **сослаться на источник** RAG, и поиск работает **через оба органа**. Завершает
  направление «нарратив как заметки» (Ярусы 1–3).
- **Ключевое решение: ссылка на ИМЯ источника, не на id чанка.** id чанков
  **нестабильны** — `/rag rebuild` дропает и переиндексирует документы с новыми uuid, и
  ссылка на чанк-uuid сломалась бы; имя источника (path/label) стабильно (хранится в
  `rag_sources`). Поэтому связь ведётся на `source`.
- **Схема**: таблица `note_rag_links(profile_id, note_id, source, created_at,
  PK(profile_id, note_id, source))` (`CREATE TABLE IF NOT EXISTS` → без миграции, +
  индексы по note_id/source). DB-методы: `rag_source_exists` (валидация — ссылаться
  можно лишь на существующий источник, в `rag_documents` OR `rag_sources`),
  `note_cite_source_insert` (идемпотентно, `INSERT OR IGNORE`), `note_cited_sources`
  (прямое: источники заметки), `notes_citing_source` (обратное: активные заметки
  источника, скрывает замещённые anti-join по `note_superseded`). `note_delete` чистит
  `note_rag_links`. Всё с изоляцией по `profile_id`.
- **Инструмент** `note_cite_source(note_id, source)` (`features/tools/notes.rs`):
  валидирует активность заметки (`note_is_active`) + существование источника; понятные
  текст-отказы (не паника), сообщение «создана / уже существовала». В `default_tool_ids`
  (как `note_link` — `reconcile_tools` включит существующим профилям), зарегистрирован.
  DB-only → проходит `effective_tool_ids` через `_ => true`.
- **Двунаправленная выдача** («поиск через оба органа»): `note_recall` и `get_self_model`
  (`render_self_read`) показывают блок «Ссылки на источники» (заметка→источник,
  `notes::cited_sources_block`); `rag_search` показывает блок «Заметки со ссылкой на эти
  источники» (источник→заметки, `notes_citing_source` по источникам найденных пассажей,
  дедуп по id, self-наблюдения помечены `[о себе]`).
- **Инварианты целы**: DB-only, изоляция по `profile_id`, без `ChatEffect`, без миграций.
  **Тесты**: db (`note_rag_links_bidirectional_and_isolated` — прямое/обратное + изоляция
  + чистка при удалении; `notes_citing_source_hides_superseded`); tool
  (`note_cite_source_links_and_recall_shows_it` — валидация источника/заметки,
  идемпотентность, показ в recall); rag_search (`search_surfaces_notes_citing_matched_
  source`). **727 тестов зелёные**, 24 `#[ignore]`, clippy/fmt чисты.
- **Смоук — GO** (`note_cite_source_e2e_live`, Gemma 4 31B + bge-m3): модель добавила
  документ (`rag_add`), затем в одном ходу `rag_search` → `note_save` → `note_cite_source`,
  связав вывод «Столица Франции — Париж» с источником «факты». В БД: 1 заметка ссылается
  на «факты». Полная цепочка (поиск → заметка → цитирование) отработала.
- **Отложено**: vec0 при росте числа заметок; нудж
  `note_cite_source` в рефлексии (сейчас — в обычных ходах, где доступен `rag_search`).

### Пост-M9: обзор self-консолидации в рефлексии (Ярус 3, нарратив как заметки) (сделано)
- **Отложенный в Ярусе 2 «обзор self-консолидации» включён** после подтверждения пользы
  связывания наблюдений (Ярус 3, GO): авто-рефлексия и инструмент `reflect` получают
  **конкретные данные** к «сну» памяти «о себе», а не только рубрику. Зеркало
  `build_consolidation_overview` (обзор пользовательских заметок для `consolidate_notes`),
  но над наблюдениями `@self`.
- **`build_self_consolidation_overview(storage, profile_id) -> Option<String>`**
  (`features/tools/notes.rs`): чистое чтение БД (вектора уже в БД) над **только**
  `@self`-наблюдениями (зеркально исключению self из обзора пользовательских заметок —
  «сон» наблюдений не трогает память о собеседнике). Три раздела: **похожие пары**
  (возможные дубли наблюдений, попарный косинус ≥ `CONSOLIDATE_SIMILARITY` 0.85, по
  убыванию близости), **связи `contradicts`** среди наблюдений (оба конца `@self`),
  **наблюдения без связей** (кандидаты связать). `None`, если наблюдений < 2
  (консолидировать нечего). Изоляция по `profile_id`.
- **Проводка**: `Reflect::invoke` (`features/tools/self_model.rs`) подмешивает обзор
  между «Текущей моделью себя» и рубрикой (пусто при < 2 наблюдений); авто-рефлексия
  (`app/orchestrator/reflection.rs`) дописывает обзор к дайджесту после блока-заимствования
  (`let mut digest`); `reflect_system_message` нуджит использовать блок «Обзор наблюдений
  для консолидации» (сливать похожие пары `note_merge`/`note_supersede`, проверять
  `contradicts`, связывать несвязанные `note_link`).
- **Инварианты целы**: DB-only, изоляция по `profile_id`, без `ChatEffect`, без миграций
  схемы (`@self` — обычный тег; вектора уже в `note_vectors`; граф в `note_links`).
  **Тесты**: notes (`self_consolidation_overview_covers_self_only` — покрывает только
  наблюдения, исключает пользовательские, `None` при < 2, показывает похожую пару и
  `contradicts`); self_model (`reflect_includes_self_consolidation_overview` — при ≥2
  наблюдениях reflect подмешивает обзор; правка `reflect_returns_current_and_rubric` —
  при < 2 обзора нет); reflection (assert в `reflect_message_nudges_cross_organ_linking`).
  **729 тестов зелёные**, 25 `#[ignore]`, clippy/fmt чисты.
- **Смоук — GO** (`self_consolidation_overview_e2e_live`, Gemma 4 + bge-m3): модель
  записала два похожих наблюдения, вызвала `reflect` — его результат нёс блок «Обзор
  наблюдений для консолидации» с похожей парой (реальный эмбеддер, близость **0.89 ≥
  0.85**), после чего модель свела дубли `note_merge` в одно наблюдение. Полная цепочка
  (наблюдения → reflect с обзором → сведение) отработала.
- **Отложено** (задел): vec0 при росте числа наблюдений/заметок; фоновая авто-консолидация
  модели себя по таймеру (сейчас обзор идёт через авто-рефлексию/интерактивный `reflect`).

### Пост-M9: запоминание последнего открытого чата (сделано)
- **Приложение восстанавливает последний открытый чат при следующем запуске.** Раньше
  `bootstrap` всегда активировал самый недавно изменённый чат (`chats.first()` после
  сортировки по `modified_at`) — переключение на старый чат без правок «забывалось» при
  перезапуске. Теперь активный чат помнится в настройках.
- **Поле `AppConfig.last_active_chat: Option<Uuid>`** (`shared/config.rs`, `#[serde(default)]`
  через контейнерный `#[serde(default)]` → старые `settings.json` без миграции; **не
  редактируется на экране настроек** — свойство оркестратора). Пишет `Orchestrator::
  remember_active_chat` (зовётся из `activate`) — **только при реальной смене** активного
  чата (`activate` зовётся и для перестроения ленты того же чата при перегенерации/
  удалении обмена — там записи нет); атомарная запись `settings.json`, ошибку не
  эскалируем (память — удобство). `bootstrap` выбирает активным `last_active_chat`, если
  он ещё виден, иначе — прежний fallback (самый недавний).
- **Защита round-trip настроек**: `handle_update_config` заменяет весь `self.config`
  снимком из UI, который может нести устаревший `last_active_chat` (например `None` со
  старта) — после замены восстанавливаем актуальное значение (`old.last_active_chat`),
  чтобы правка настроек не стёрла память о чате.
- **Тесты**: config (дефолт `None`); оркестратор (`remembers_and_restores_last_opened_chat`
  — двухфазный: переключение на первый чат → в `settings.json` `last_active_chat` = первый;
  второй запуск на тех же данных восстанавливает именно его, хотя второй чат изменён
  позже). **734 теста зелёные**, 25 `#[ignore]`, clippy/fmt чисты.

### Пост-M9: скроллбары (лента, ввод, настройки, справка, список чатов) (сделано)
- **Общий хелпер `shared/ui.rs::render_scrollbar`**: вертикальный скроллбар (ratatui
  `Scrollbar`) в правой колонке переданной области; **no-op, когда содержимое
  помещается** (`total ≤ viewport`) — на коротком содержимом бар не шумит. Рисуется
  **поверх правой линии рамки** панели (`area.inner(Margin::new(0, 1))` — углы
  целы), ширину у содержимого не отнимает → перенос строк/математика скролла не
  меняются. Трек — цветом рамки (параметр `focused` говорит, в каком цвете
  нарисована рамка под баром: обычная/фокусная), бегунок `█` — цветом `text`
  (читается на обеих). **Нюанс ratatui**: `ScrollbarState::content_length` — число
  **позиций** прокрутки (`total − viewport + 1`), не рядов; с `content_length =
  total` бегунок при полной прокрутке не доходил бы до низа (см. коммент в хелпере).
- **Лента чата** (`widgets/message_feed.rs`): бар на правой рамке при переполнении,
  позиция — `self.scroll` (кламп прежний).
- **Поле ввода** (`widgets/input_box.rs`, многострочный режим): бар при
  `vrows > visible_rows` (поле упёрлось в потолок высоты и прокручивается);
  однострочный режим не затронут (там горизонтальный скролл).
- **Экран настроек** (`screens/settings.rs::render_fields`): бар поверх правой
  рамки экрана (колонка `list_area.right()` — линия рамки: `fields_area` доходит
  ровно до inner панели), ниже строки титула секции; позиция — фактический
  `ListState::offset()` после рендера списка.
- **Попап помощи** (`F1`/`?`, `screens/chat.rs`): список клавиш на коротком
  терминале теперь **прокручивается** — `↑↓`/`PgUp`/`PgDn` крутят
  (`ChatScreen.help_scroll`, кламп в `render_help` — высота попапа известна только
  там), любая другая клавиша закрывает (как раньше). `List` заменён на `Paragraph`
  со `scroll`; бар на правой рамке попапа; подпись внизу при переполнении —
  «↑↓ прокрутка · Esc — закрыть».
- **Список чатов** (`widgets/chat_list.rs`): бар поверх правой рамки панели «▤ Чаты»
  на рядах списка (строка поиска и сетка хоткеев не затрагиваются); позиция —
  offset списка после рендера.
- **Тесты**: хелпер (бар только при переполнении; бегунок у верха в начале и у низа
  при полной прокрутке; нулевая область — no-op); по тесту на ленту/ввод/настройки/
  список чатов (бегунок «█» в колонке рамки появляется только при переполнении);
  справка (стрелки прокручивают и не закрывают, повторное открытие сбрасывает
  прокрутку, «перекрут» клампится при рендере, бегунок на коротком терминале).
  **744 теста зелёные**, clippy/fmt чисты.

### Пост-M9: режим совместимости со старым терминалом (сделано)
- **Тумблер `interface.terminal_compat`** (секция «Интерфейс» настроек,
  `FieldId::ICompat`, по умолчанию **выключен**; `#[serde(default)]` → старые
  `settings.json` без миграции): старые эмуляторы (conhost Windows 10 и т.п.)
  рисуют эмодзи и редкие символы Юникода квадратами-«тофу» и игнорируют `DIM` —
  режим переключает интерфейс на безопасный набор глифов. См. spec §11.6.
- **`GlyphSet`** (`shared/theme.rs`): все декоративные глифы UI собраны в одну
  структуру с двумя статиками — `UNICODE_GLYPHS` (прежний вид редизайна) и
  `COMPAT_GLYPHS`. **Ориентир компат-набора — WGL4** (базовый репертуар шрифтов
  Windows: Consolas/Lucida Console) плюс ASCII: `✦→*`, `❯→>` (заголовки ролей,
  приглашение ввода), `⚒→#` (tool-карточка, счётная ширина префикса/продолжения
  выравнена в обоих наборах), `▸/▾→►/▼` (пилюля «мыслей», «Секции»), `◆→♦`
  (титулы), `▤→≡` (список чатов), `⚙/⌨→#` (панели настроек/справки), `✓/✗/⚠→
  √/×/!` (статусы операций, цели `F3`), `◐/✕→○/×` (чипы серверов; **готовность
  `●` не заменяется** — она в WGL4), `⟳→»` (генерация), `✻→*` (фоновая задача),
  `⌕/▏→?/│` (строка поиска), `➕→+` (попап орфографии), Брайль-спиннер → ASCII
  `|/-\` (RAG-баннер, имперсонация), скруглённые рамки (`BorderType::Rounded`,
  арк-сегменты `╭╮╰╯`) → прямые. WGL4-безопасное (`▌`-рейлы, `│`/`└` гуттеры,
  box-drawing таблиц markdown, `█` скроллбара, `…`, стрелки, `‹›`, `·`, `☺`)
  сознательно не трогается. Эмодзи в **содержимом** сообщений (и попап `Ctrl+B`)
  не заменяются — это данные, не оформление.
- **Проводка через палитру** (минимальный ripple): `Palette` получила поле
  `compat: bool` (+ builder `with_compat`, метод `glyphs() -> &'static GlyphSet`)
  — палитра уже протянута во все render-функции, как флаг `dark`. `panel()` берёт
  тип рамки из набора. Виджеты/экраны читают глифы из `palette.glyphs()`; дублей
  спиннеров (const в `chat.rs`/`impersonation_preview.rs`) больше нет — кадры в
  `GlyphSet.spinner`. Пересборка палитры с флагом — `ChatScreen::set_settings`,
  `SettingsScreen::palette()` (хелпер), `runtime::apply_event` (открытые
  список/`F3`) — применяется на лету из события `Settings`.
- **`dim_background(frame, palette)`** (`shared/ui.rs`): conhost не поддерживает
  SGR `DIM`, поэтому в компат-режиме фон под попапами притеняется **цветом** — fg
  всех ячеек → `palette.muted` + снятие `BOLD` (в 16-цветном маппинге он дал бы
  «яркий» вариант и свёл притенение на нет); обычный режим — прежний `DIM`.
- **Тесты**: theme (наборы переключаются флагом; в компат-наборе нет заменяемых
  символов, спиннер ASCII; счётные ширины tool-префиксов совпадают, приглашение
  ввода — 2 колонки); ui (компат-затемнение красит fg вместо DIM); message_feed
  (компат-лента без эмодзи: `* АССИСТЕНТ`/`> ВЫ`/`# note_save`/`► мысли`);
  status_bar (компат-чипы `○/×`, `» генерация`, `* рефлексия`, `●` остаётся);
  settings (тумблер в секции «Интерфейс», сохранение + палитра рабочей копии,
  описание поля); config (дефолт выкл, roundtrip). **751 тест зелёный** (+7),
  clippy/fmt чисты.
- **Задел**: авто-детект старого терминала при первом запуске (эвристика
  `WT_SESSION`/`TERM_PROGRAM` на Windows) — сейчас включение только ручное.

### Пост-M9: метаданные сообщения — режим/модель + семплинг по доступным полям (сделано)
- **Снимок в `Message.metadata` теперь несёт режим движка и имя модели, а семплинг
  урезается до полей, доступных в этом режиме.** Раньше `finalize_message`
  (`app/orchestrator/generation.rs`) писал `MessageMetadata { sampling: полный
  effective_sampling, model: None }` — режим не фиксировался, модель всегда `None`, а
  в снимок «что применилось» попадали расширения llama.cpp, которые строгий облачный
  диалект (OpenAI/Gemini/Claude) даже не принял бы.
- **`MessageMetadata`** (`entities/message.rs`) получил поле `mode: ServerMode`
  (`#[serde(default)]` → старые сообщения читаются как `Managed`; `model:
  Option<String>` уже было). `sampling` теперь фильтруется.
- **`SamplingConfig::retain_supported(provider)`** (`entities/sampling.rs`): копия
  конфига с обнулёнными (`None`) полями, недоступными в режиме провайдера — **зеркало
  wire-диалекта** через существующий `supported_sampling_fields` (тот же источник
  истины, что у UI настроек и `get/set_sampling`; сериализационный round-trip с
  `retain` по ключам, как `filter_to_supported` в introspection). Для локального
  (`None`) — весь настраиваемый набор; облако — строгие подмножества.
- **`EngineSettings::active_model_name()`** (`shared/config.rs`): имя активной модели
  по режиму (managed — базовое имя GGUF без пути/`.gguf`; external/облако —
  `model_name`). `screens/chat.rs::model_meta` отрефакторен на него (убрано дублирование
  вывода имени модели — подпись ленты и снимок метаданных берут имя из одного места).
- **Проводка**: `GenSpawn` получил `engine_mode`/`model_name` (снимок из
  `config.engine` на старте хода), `finalize_message` их принимает и строит метаданные
  с `retain_supported(mode.cloud_provider())`.
- **Тесты**: entity (`retain_supported` роняет `top_k`/`thinking` для OpenAI, оставляет
  для локального; Claude оставляет `thinking`, роняет `temperature`); config
  (`active_model_name` по режимам, пустое имя = None); оркестратор
  (`assistant_metadata_records_mode_model_and_filtered_sampling` — облачный режим →
  метаданные несут `mode=openai`, `model`, а `top_k` из семплинга обнулён).
  **755 тестов зелёные** (+3), clippy/fmt чисты.

### Пост-M9: summary «модели себя» как снимок, а не летопись (анти-раздувание) (сделано)
- **Проблема** (наблюдение пользователя + живой профиль): `summary` «модели себя»
  разрастался в плотное эссе (~4.5–5k симв.) с дублями. Причина — механика
  анти-раздувания есть у всех органов, **кроме `summary`**: у наблюдений ворота
  дублей/замещение/граф, у целей потолок закрытых, у черт семантические ворота 0.72 и
  merge, а у `summary` — ни ориентира, ни ворот, ни консолидации, ни обратной связи о
  размере, и `POLICY_CORE` маршрутизировал туда событийные выводы (ось «устойчивое/
  мимолётное» вместо «состояние/событие»). План — четыре этапа-PR, дизайн-док
  [docs/history/summary-as-snapshot.md](docs/history/summary-as-snapshot.md).
- **Этап 1 — жанровая граница (только тексты, 0 логики):** `POLICY_CORE` (единый
  источник для протокола ведения и авто-рефлексии) переписан на ось «состояние →
  `summary`, событие-вывод → `add_insight` **даже устойчивое**» (наблюдение не теряется:
  всплывает по релевантности, связывается, консолидируется); `summary` — компактный
  снимок «кто ты, что ценишь, как работаешь», при правке «интегрируй **и СОКРАЩАЙ**» +
  шаг «прочти целиком через `get_self_model` перед правкой (в промпте усечён)». Описания
  `update_self_model`/`add_insight` и рубрика `reflect` приведены к той же границе.
- **Этап 2 — мягкие ворота размера `summary`** (ворота, не потолок — данные не
  усекаются): конфиг `self_model.summary_target_chars` (дефолт 1000, `#[serde(default)]`,
  санитизация пол 200 в `SelfModelParams`); чистый `SelfModel::summary_fill_hint(target)`
  (`None` в пределах ориентира, иначе текст с текущим размером — аналог бывшего
  `narrative_fill_hint`); **три точки показа** — чтение (`render_self_read` → его видят
  `get_self_model`/`reflect`/авто-рефлексия), data-aware приписка после
  `maintenance_protocol()` в `inject_self_model`, строка «Описание: N симв. (ориентир ≤
  M)» в эхе `update_self_model` при правке summary; поле «Модель себя: ориентир описания
  (симв.)» в секции «Инструменты» настроек (`FieldId::SmSummaryTarget`).
- **Этап 3 — посекционные бюджеты инъекции** (правится только рендер, данные не
  трогаются): `render_for_prompt` усекает секцию «О себе» до **половины** лимита
  (`max_chars/2`), гарантируя остаток целям/собеседнику/наблюдениям — раздутый summary
  больше не вытесняет их из промпта (раньше одно финальное усечение съедало всё за
  описанием); `truncate_chars_word` — усечение по границе слова (откат к последнему
  пробелу, одно длинное слово → посимвольно); финальное усечение всего блока —
  страховка (char-exact); **`render_full` (полное чтение) НЕ усекается** — прежний урок.
- **Этап 4 — диета эха правок:** `update_self_model`/`update_user_model` возвращают
  **дельты** вместо полного `render_full` (полное чтение остаётся за `get_self_model`) —
  экономит токены и снимает «заякоривание» на жанре эссе. `update_self_model`: строка
  размера + добавленные цели с `#id` + закрытые выполненными/неактуальными по `#id` +
  число свёрнутых в наблюдения (дельты собираются в атомарной правке захватом `&mut`;
  побочно уточнён учёт `changed` — пустой `add_goal` больше не помечает правку).
  `update_user_model`: компактные итоговые списки собеседника + подтверждение шрама
  (текст `note`); ворота черт и напоминание про `note` без изменений. Публичная
  `entities::self_model::short_id(&Uuid)` — ручка `#id` для эха.
- **Инварианты целы**: SelfModel-мутации DB-only (без `ChatEffect`), изоляция по
  `profile_id`, без миграций (`#[serde(default)]`); инъекция остаётся в `system`
  (трейд-офф prefix cache принят 2026-07-03).
- **Тесты**: entity (`summary_fill_hint`, санитизация, посекционный бюджет не вытесняет
  секции, усечение по слову, дельта- id); tools (эхо-дельты, подсказка при чтении,
  строка размера, жанровые маркеры POLICY_CORE/рубрики); orchestrator (data-aware
  приписка в `inject_self_model`); config (дефолт). **770 юнит-тестов зелёные** (+11),
  clippy/fmt чисты. **Живой прогон — GO** (Gemma 4 31B + bge-m3): `summary_gate_e2e_live`
  — модель увидела «разрослось: 1600 ≤ 1000», сжала `summary` 1600 → 57 симв.;
  регрессия не выявлена — `trait_gate`/`auto_reflect`/`self_model_e2e`/`self_model_gate`
  смоуки зелёные (новое эхо, ворота черт, шрам-`note`, рефлексия, ворота `add_insight`
  работают).
- **Задел**: авто-консолидация модели себя по таймеру (ворота этапа 2 дадут ей сигнал
  «summary раздут»); семантическое сравнение абзацев summary с @self-наблюдениями в
  обзоре self-консолидации; старение `current_interests` — см. дизайн-док «Вне объёма».

### Пост-M9: редизайн экрана настроек — этап 1 (IA + группы полей) (сделано)
- **Информационная архитектура экрана настроек отстала от роста числа полей** (~120):
  плоские секции без групп, «Инструменты» превратились в свалку (гейты + embedding-
  сервер + чанкинг RAG + 6 полей «модели себя» + заметки), «Инференс» из одного поля,
  подписи с префиксами-псевдогруппами («Эмбеддинги:», «RAG:», «Модель себя:»). Этап 1
  из согласованного плана: только `screens/settings.rs` (контракт `SettingsIntent` и
  `AppConfig` не тронуты, миграций нет). Ветка `feat/settings-redesign`.
- **Перегруппировка секций**: `Инференс` (1 поле) удалён — `max_tool_rounds` уехал в
  `Инструменты` (группа «Агентный цикл» с лимитами субагента); добавлена секция
  **`Память`** — из «Инструментов» переехали чанкинг RAG (группа «База знаний»),
  заметки (авто-консолидация, recall @self) и «модель себя» (6 полей). Итог 6 секций:
  Модель · Семплинг · Инструменты · Память · Профили · Интерфейс. Embedding-**сервер**
  остался в «Инструментах» (группа «Эмбеддинги (сервер)») — переезд в подсекцию
  «Модель» отложен к этапу 2 (нужен таб-стрип с 3-й подсекцией).
- **Заголовки групп** — новое поле `FieldRow.group: &'static str` (хелпер
  `grouped("Группа", vec![...])` проставляет батчу). Заголовки **не входят в
  `fields()`** (навигация поле-в-поле, без «мёртвых» шагов на заголовке) — они
  инъектируются на переходе группы **в рендере** (`render_fields`): для выбранного
  поля вычисляется его позиция среди отрисованных элементов (`select` = field_idx +
  число заголовков до него), скроллбар считает по полному числу элементов. Заголовок —
  `header_line` (имя muted+bold, продолжение линией `─` цветом рамки; `─` в WGL4 —
  без компат-замены). Managed-поля `llama-server` разложены по группам Сервер/Модель/
  Производительность/Спекулятивное декодирование; семплинг — `SamplingParam::group()`
  + переупорядоченный `SAMPLING_PARAMS` (Основные/Динам.темп/Разнообразие/Штрафы/DRY/
  Mirostat/Порядок/Рассуждения).
- **Выравнивание значений — по группе, не по секции** (`HashMap<group, max_label_w>` в
  рендере, пол `MIN_LABEL_COL=20`): одно длинное имя (напр. «Наблюдения «о себе» в
  note_recall») больше не отгоняет колонку значений всех групп секции. **Заменено** —
  единая колонка на секцию с потолком, см. ниже «Пост-M9: настройки — единая колонка
  значений на секцию».
- **Короткие подписи**: префикс уходит в заголовок группы («Эмбеддинги: бинарник» →
  «Бинарник llama-server» под «Эмбеддинги (сервер)»; «RAG: размер чанка» → «Размер
  чанка (симв.)» под «База знаний»; «Модель себя: …» → «…» под «Модель себя»;
  «инструмент: X» → «X» под «Инструменты»).
- **Фиксированная нижняя панель** (высота 4, резервируется всегда при наличии полей —
  список не «прыгает»): полное значение выбранного **длинного** текстового поля
  (пути/URL/системное сообщение; порог >32 колонок, чтобы короткие числа/host не
  дублировались) + описание-подсказка. Значения в списке **усекаются с «…»**
  (`truncate_to_width`, локальный аналог chat_list) под ширину колонки группы.
- **Счётчик полей секции** в левом меню (справа, приглушённо) — быстрая ориентация в
  объёме. Добавлены описания для полей, ставших заметными (агентный цикл, мастер-гейты
  web/python).
- **Тесты**: навигация в тестах переведена с хрупкого счётчика `Tab`/`Down` на
  устойчивые хелперы `goto_section(Section)`/`goto_field(FieldId)` (терпят
  переупорядочение секций/групп); новые тесты — состав «Памяти» (RAG/заметки/модель
  себя переехали, `max_tool_rounds` в «Инструментах»), разметка групп, усечение
  длинного значения с «…». **773 юнит-теста зелёные** (+3), clippy/fmt чисты. Живой
  прогон не требуется (чистый UI-рефактор, без движка).
- **Дальше (согласованный план)**: этап 2 — вкладки подсекций (таб-стрип, embedding →
  «Модель»), меню, контекстный футер; этап 3 — профили: инструменты группами с
  описаниями и честными гейтами (`⊘` для глобально-выключенных); этап 4 — поиск `/`;
  этап 5 — Choice-попап, валидация без закрытия редактора, `Del`-сброс, маркеры `•`
  «изменено против дефолта»; этап 6 — статусы серверов на экране + опц. дебаунс рестарта.

### Пост-M9: редизайн экрана настроек — этап 2 (вкладки подсекций + контекстный футер) (сделано)
- Продолжение этапа 1 (`feat/settings-redesign`, только `screens/settings.rs`; контракт
  `SettingsIntent`/`AppConfig` не тронуты, миграций нет).
- **Таб-стрип подсекций** вместо псевдо-поля «Подсекция ‹Ассистент›»: селектор
  подсекции (`ModelSub`/`SamplingSub`/`ProfileSub`) **остаётся полем 0 в `fields()`**
  (навигация/цикл `←→` без изменений — минимальный риск), но **в списке не рисуется**:
  `render_fields` инъектирует его как пиннед-строку под титулом (`tab_strip_line`:
  `Ассистент │ Имперсонация │ …`, активная вкладка выделена — при фокусе на стрипе
  подложкой `keycap_bg`, иначе акцентным цветом; справа `←→` при фокусе). Титул секции
  вынесен из блока `List` в отдельную шапку (`head_area` = титул + опц. таб-стрип);
  скроллбар теперь на всю высоту `list_area`. Детект поля-подсекции — `is_subsection`;
  оно исключено и из выравнивания по группам, и из строк списка.
- **Третья вкладка «Эмбеддинги» в «Модели»** — embedding-**сервер** переехал из
  «Инструментов» (зеркало трёх чипов статус-бара чат/имп/эмб). Новый тип
  `ModelTab { Assistant, Impersonation, Embeddings }` (у `model_sub`; `cycle(dir)` —
  3-позиционный с учётом направления), `Subsection` (2 варианта) остался у семплинга/
  профилей. Подписи вкладок — const `MODEL_TABS`/`SUB_TABS` (индекс = дискриминант,
  `as usize`). Поля `EMode`/`EBinary`/… и их обработчики не тронуты — сменилась лишь
  локация в UI. «Инструменты» теперь только гейты/параметры (Агентный цикл/Веб/Python/
  Файлы).
- **Профили переупорядочены**: `ProfileSub` (таб-стрип) — поле 0, затем секционные
  `PSelect` (выбор профиля) и `PName`, затем подсекционные Персона/Инструменты.
- **Контекстный футер** (`render`): базовые хоткеи + специфичные секции — в «Профилях»
  добавляются `Ctrl+N новый`/`Ctrl+D удалить`.
- **Тесты**: обновлены под `ModelTab` и переупорядочение профилей (навигация — через
  устойчивые `goto_section`/`goto_field`); новые — третья вкладка «Эмбеддинги» (поля
  в «Модели», нет в «Инструментах»), рендер таб-стрипа (вкладки видны, «Подсекция» не
  строка списка). **775 юнит-тестов зелёные** (+2), clippy/fmt чисты. Чистый UI-рефактор.

### Пост-M9: редизайн экрана настроек — этап 3 (профили: инструменты группами + гейты) (сделано)
- Продолжение (`feat/settings-redesign`). Стена из ~32 плоских тумблеров «инструмент: X»
  в профиле заменена **сгруппированным списком с описаниями и честными гейтами**.
- **Метаданные каталога вынесены в `features/tools/meta.rs`** (FSD: `screens` берёт их
  оттуда, не хардкодит): `tool_group(id)` (8 групп, порядок в `TOOL_GROUPS`),
  `tool_description(id)` (короткое 2–4-словное описание), `tool_gate(id) -> Option<ToolGate>`
  (`Web`/`Python`/`Fs` — зеркало `effective_tool_ids`). Покрыто тестом «у каждого
  инструмента из `all_tool_ids` есть группа и описание».
- **Тумблеры профиля по группам**: `profile_fields` раскладывает инструменты по
  `tool_group` (стабильная сортировка по `TOOL_GROUPS` **без изменения индексов** —
  `PTool(idx)` остаётся позицией в `tool_catalog()`, источнике истины для
  `toggle_profile_tool`; группируется лишь порядок показа). Заголовки групп —
  прежний механизм этапа 1.
- **Инлайн-описания** (`FieldRow.hint: Option<&'static str>`): короткое описание
  справа от `[x]` (`get_sampling  [x]  показать семплинг`). `render_field_line`
  дорисовывает подсказку в остатке ширины (усечение с «…»). Поле generic — прочие
  секции его не задают.
- **Честный гейт** (`FieldRow.warn`): инструмент, включённый в профиле, но выключенный
  **глобальным** выключателем (`web/python/fs_enabled`), рисуется **цветом
  предупреждения** с подсказкой «выкл. глобально: <выключатель>», а в нижней панели —
  развёрнутым пояснением «включите в секции «Инструменты»». Закрывает ловушку «[x], а
  на деле недоступен» (`SettingsScreen::gate_disabled` + `gate_hint`).
- **Счётчик «вкл/всего» в заголовке группы** (generic): группы с ≥2 тумблерами несут
  `N/M` (`header_line` расширен параметром `count`) — работает и в «Интерфейсе»
  (напр. «Копирование переписки (F5) 2/3»), и в профиле («Память и знания 11/11»).
- **Тесты**: meta (группа/описание/гейт у всех инструментов); профиль (тумблеры
  сгруппированы + непрерывны + описания; выключенный глобально помечен гейтом,
  включённый — нет); `header_line` со счётчиком и без. **780 юнит-тестов зелёные**
  (+5), clippy/fmt чисты. Чистый UI-рефактор (без движка); «опц.»-пометка групп
  (control/self_model) — оставлена заделом.

### Пост-M9: редизайн экрана настроек — этап 4 (поиск по полям `/`) (сделано)
- Продолжение (`feat/settings-redesign`). При ~140 полях по 6 секциям и 3 подсекциям
  добавлен **глобальный поиск `/`** — прыжок к полю не листая секции.
- **Индекс поиска** (`build_search_index`): перечисляет поля **всех** секций и
  **всех подсекций** (не только активной). Построители полей параметризованы по
  подсекции — `model_fields_for(ModelTab)` / `sampling_fields_for(Subsection)` /
  `profile_fields_for(Subsection)` (публичные `*_fields()` зовут их для активной);
  `ModelTab::ALL`/`Subsection::ALL`+`from_index` для перечисления. Каждый хит несёт
  координаты прыжка (`section_idx`, дискриминант подсекции, `field_idx`), крошку
  «Секция · Подсекция › Группа › Поле» и lowercase-ловушку (подпись+группа+описание+
  hint+подсекция). Mode-зависимые поля (managed/облако) индексируются по текущему
  режиму. `collect_hits` пропускает селектор подсекции.
- **Оверлей** (`SearchState { input, all, results, selected }`, поле `search:
  Option<...>`): `/` открывает (в редакторе `/` — обычный символ), ввод фильтрует
  **AND по словам-подстрокам** (`search_filter`), `↑/↓` выбор, `Enter` —
  `jump_to_selected` (ставит секцию/подсекцию/`field_idx`/фокус на полях, закрывает
  оверлей), `Esc` — отмена, `Ctrl+K` — очистка запроса. Рендер (`render_search`):
  затемнение фона, попап = строка запроса (`InputBox`) + список крошек со значением
  (приглушённо) + скроллбар; заголовок несёт счётчик «найдено/всего». Вставка из
  буфера маршрутизируется в строку поиска. `/ поиск` добавлен в контекстный футер.
- **Тесты**: `/` открывает и фильтрует; `Enter` прыгает к полю (в т.ч. **со сменой
  подсекции** — поиск поля вкладки «Эмбеддинги» переключает `model_sub`); `Esc` не
  двигает навигацию; индекс охватывает поля неактивных подсекций (>100 полей).
  **784 юнит-теста зелёные** (+4), clippy/fmt чисты. Чистый UI-рефактор.

### Пост-M9: редизайн экрана настроек — этап 5 (попап выбора, валидация, `Del`-сброс, маркер `•`) (сделано)
- Продолжение (`feat/settings-redesign`). Четыре улучшения редактирования полей.
- **Попап выбора Choice-поля** (`ChoiceState`, поле `choice`): `Enter` на Choice-поле
  открывает список всех вариантов с отметкой текущего (`↑↓`/`Enter`/`Esc`; `←/→`
  по-прежнему быстрый цикл). Критично для `--spec-type` (9 вариантов), режимов (5–6),
  тем, **выбора профиля** (`PSelect`). `choice_menu(id)` даёт (варианты, индекс) —
  переиспользует `FlashAttn::ALL`/`SpecType::ALL` (сделаны `pub`), `SERVER_MODES`/
  `IMP_MODES`/`THEMES` + `sampling_choice_menu` (thinking/reasoning); `apply_choice`
  применяет через **существующий `cycle_field`** (шагает от текущего к целевому — без
  новых сеттеров).
- **Валидация без закрытия** (`Editor.error`): невалидное числовое поле по `Enter` не
  закрывает редактор — титул краснеет глифом `⚠`/`!` + сообщение, правка сбрасывает
  ошибку, `Esc` отменяет. Классификация `field_num_kind`/`SamplingParam::num_kind`
  (Int/Float) + `field_validation_error` (мягкая i64/f64; пусто допустимо; точный
  тип/диапазон досматривает `apply_text`).
- **`Del` — сброс поля к дефолту** (`reset_field`): сравнивает значение с полем из
  **дефолтного конфига** (`default_fields` — временный `SettingsScreen` на
  `AppConfig::default()` с той же навигацией); если отличается — применяет дефолт
  через `toggle_field`/`apply_choice`/`apply_text` (уже дефолт → no-op без лишнего
  сохранения). Профильные поля (`is_profile_field`) не затрагиваются (у них нет
  config-дефолта).
- **Маркер `•`** (акцентный, 2 колонки слева, поля визуально отступлены под
  заголовком группы): `render_field_line` получил параметр `modified` —
  `render_fields` сравнивает значение с `default_fields` (строится один раз на рендер).
  Профильные поля не помечаются (дефолт-конфиг несёт те же профили → равны). `•`, `‹›`,
  `⚠`/`!` из WGL4/GlyphSet.
- Футер: `Del сброс` (при фокусе на полях). `FlashAttn`/`SpecType` получили `pub const
  ALL` (перечисление вариантов).
- **Тесты**: попап открывается/применяет выбор/`Esc` отменяет; невалидное число держит
  редактор + ошибку, правка коммитит; `field_validation_error` классифицирует
  int/float/текст; `Del` сбрасывает изменённое поле и no-op на дефолтном; маркер `•`
  появляется у изменённого поля и отсутствует в дефолте (render-проверка). **791
  юнит-тест зелёный** (+7), clippy/fmt чисты. Чистый UI-рефактор.

### Пост-M9: редизайн экрана настроек — этап 6 (чипы статусов серверов на экране) (сделано)
- Завершение направления (`feat/settings-redesign`). Единственный этап с проводкой
  вне `screens` (доставка снимка статусов, как для `Settings`/палитры — FSD цел).
- **Чип статуса сервера в заголовке секции «Модель/сервер»**, справа, контекстно по
  активной подсекции: Ассистент → `chat`, Имперсонация → `impersonation`, Эмбеддинги →
  `embed` (`● готов` / `◐ подключение…` / `✕ не настроен` / нет связи с причиной;
  глифы/цвета зеркалят `widgets::status_bar`, ширина глифа 1 колонка — компат-безопасно).
  Правишь движок → видишь `подключение… → готов` на месте, не выходя в чат (restart —
  это `Connecting`-статус, приходящий от оркестратора).
- **Проводка**: `SettingsScreen.statuses: ServerStatuses` + `set_server_statuses`;
  новый геттер `ChatScreen::server_statuses()`. Runtime: при `OpenSettings` ставит
  начальный снимок из чата; `apply_event(ServerStatus)` при открытом экране настроек
  дублирует статус в него (живое обновление). `render_field_line`-независимый чип
  правым выравниванием на строке титула (`server_status_chip`/`span_width`).
- **Тесты**: чип показывает сервер активной подсекции (чат→эмбеддинги при переключении
  вкладки), в других секциях не рисуется (render-проверка). **792 юнит-теста зелёные**
  (+1), clippy/fmt чисты.
- **Дебаунс рестарта движка — сознательно отложен** (в плане помечен «опциональным»):
  единственная по-настоящему рискованная правка (в оркестраторе — самом критичном
  concurrency-компоненте с инвариантом «единственный владелец `Chat`»; меняет давнюю
  семантику «применяется при коммите»). Чипы уже дают наблюдаемость рестарта, ради
  которой затевался этап; churn возникает лишь при быстром редактировании нескольких
  полей движка подряд и теперь виден/терпим. Делать отдельным сфокусированным PR.
  **Закрыто** — см. ниже «Пост-M9: дебаунс рестарта движка».

### Редизайн экрана настроек (этапы 1–6) — итог
Направление `feat/settings-redesign` завершено (6 коммитов). Экран настроек прошёл
путь от плоского списка ~120 полей к: **группам полей** с заголовками и посекционным
выравниванием + фикс-панелью значения/описания (этап 1); **таб-стрипу подсекций** и
переносу embedding-сервера в «Модель» + контекстному футеру (этап 2); **тумблерам
инструментов группами** с описаниями и честными `⊘`-гейтами (этап 3); **поиску `/`**
по всем секциям с прыжком к полю (этап 4); **попапу выбора Choice**, валидации без
закрытия редактора, `Del`-сбросу и маркеру `•` «изменено» (этап 5); **чипам статусов
серверов** на экране (этап 6). Контракт `SettingsIntent`/`AppConfig` не менялся,
миграций нет; +новый `features/tools/meta.rs`. Итог: **792 юнит-теста**, clippy/fmt
чисты. Задел: «опц.»-пометка опциональных групп инструментов (дебаунс рестарта
движка — сделан отдельным PR, см. ниже).

### Пост-M9: дебаунс рестарта движка при правках настроек (сделано)
- **Проблема**: экран настроек применяет правку **при коммите каждого поля**
  (`SettingsIntent::SaveConfig` → `AppCommand::UpdateConfig` →
  `handle_update_config` → немедленный `apply_chat_settings()`), поэтому серия
  «бинарник → модель → -ngl» давала три тяжёлых рестарта managed `llama-server`
  подряд. Отложенный задел этапа 6 редизайна настроек.
- **Решение**: дебаунсится **рестарт сервера**, а не сохранение конфига — конфиг
  пишется на диск и переэмитится в UI сразу (экран настроек живёт по переэмиту),
  откладывается только дорогой `apply_*` (пере)запуск. Новый модуль
  `app/orchestrator/restart_queue.rs` — `RestartQueue { chat, embed,
  impersonation: bool, deadline }` (зеркало `SaveQueue`): `mark_chat/embed/
  impersonation()` взводят флаг и продлевают дедлайн (`RESTART_DEBOUNCE = 1.2с`
  от **последней** правки), `take()` отдаёт флаги и сбрасывает. В петле `run()` —
  вторая ветка `sleep_until_opt(restart_deadline) => flush_restarts()`;
  `flush_restarts` (в `settings.rs`) зовёт `engines.apply_*` только для
  помеченных серверов + **один** `emit_server_status()`. Читает финальный
  `self.config` (заменён ещё при правке) → серия правок = один рестарт с
  итоговыми значениями. Инвариант «единственный владелец `Chat`» не тронут,
  новых каналов нет.
- **Немедленными остались**: стартовый подъём серверов (до петли, `mod.rs` —
  очередь не проходит), персист + `emit_settings()`, пересборка реестра
  инструментов (смена `tools`/провайдера — дёшево, схема get/set_sampling должна
  быть актуальна со следующего хода). На `Quit` отложенные рестарты **намеренно
  не флашатся** (серверы рвутся через Drop/kill_on_drop — поднимать процесс перед
  дропом незачем). В окне дебаунса старый сервер жив (`Ready`), затем чип
  показывает `Connecting → Ready` — как раз наблюдаемость, добавленная этапом 6.
- **Тесты**: юнит `RestartQueue` (коалесинг пометок + `take`-сброс; продление
  дедлайна каждой пометкой — на паузе виртуального времени); интеграционный
  `model_change_restarts_chat_server_debounced` (бывший
  `model_change_restarts_chat_server`): две правки движка подряд → `Settings`
  переэмичены сразу, `chat_call_count()` ещё 1 (рестарт отложен), после дедлайна —
  ровно 2 (один рестарт на серию; маркер флаша — `AppEvent::ServerStatus`).
  Детерминизм по времени — `#[tokio::test(start_paused = true)]`: в
  dev-dependencies добавлен `tokio` с фичей **`test-util`** (только тест-сборки;
  виртуальное время доматывается до дедлайна, когда все задачи idle — гонок нет).
  `MockSupervisor.chat_call_count()` уже существовал (DoD M8). **794 юнит-теста
  зелёные** (+2), clippy/fmt чисты. Доки: architecture.md §3/§6, spec §11.6.

### Пост-M9: настройки — единая колонка значений на секцию (сделано)
- **Симптом**: выравнивание значений «по группе» (этап 1 редизайна настроек) давало
  «пилу» — у каждой группы свой стоп колонки значений (в «Памяти» три группы — три
  разных колонки), а одна длинная подпись отгоняла значения своей группы далеко от
  коротких соседей («Тема ‹авто›» уезжала за «Совместимость со старым терминалом»).
- **Фикс** (`screens/settings.rs`): колонка значений считается **по всей секции** —
  `section_label_col` (max ширины подписи среди видимых полей; пол `MIN_LABEL_COL=20`,
  **потолок `LABEL_CAP=28`**; селектор подсекции исключён — он таб-стрип). Значения и
  инлайн-подсказки всех групп стоят на одной вертикали. Подпись длиннее потолка
  колонку **не отгоняет** — её значение локально встаёт сразу после подписи
  (страховка; в текущем наборе таких подписей нет), `value_w` при этом считается от
  реального конца подписи (усечение «…» не врёт). `FieldRow.group` остался только
  для заголовка группы и счётчика тумблеров.
- **Подписи-выбросы укорочены** переносом контекста в заголовок группы (приём этапа 1):
  «Копировать с X» → «С X» (глагол уже в заголовке «Копирование переписки (F5)»),
  «Совместимость со старым терминалом» → «Режим старого терминала», «Наблюдения
  «о себе» в note_recall» → ««О себе» в note_recall». Слова для поиска `/` сохранены
  в описаниях полей (`field_description` — ловушка поиска включает описание).
- **Тесты**: `section_label_col` (пол/потолок/исключение селектора);
  `all_labels_fit_alignment_cap` — **гейт на будущее**: подписи всех секций и
  подсекций (включая draft-поля спекулятивного декодирования, видимые лишь при
  `spec_type=draft-*`) обязаны влезать в `LABEL_CAP`, иначе тест требует сократить
  подпись или перенести смысл в заголовок группы;
  `value_column_is_shared_across_groups` (рендер: значения трёх групп «Инструментов»
  в одной колонке). **797 тестов зелёные** (+3), clippy/fmt чисты. Доки: spec §11.6.

### Рефакторинг god-object'ов — этап 1: `screens/settings.rs` → `screens/settings/` (сделано)
- **План направления** — [docs/refactoring-god-objects.md](docs/refactoring-god-objects.md)
  (7 этапов + опц.): разбор нескольких файлов-монолитов, выросших в god-object'ы
  (settings/chat/orchestrator-tests/notes/db/markdown/runtime). Метод — тот же
  плейбук, что и разбор оркестратора (Фазы 1–3): **чисто механический перенос**
  файла в каталог-модуль без изменения типов/полей/каналов/поведения.
- **Этап 1**: `screens/settings.rs` (4966 строк, один `impl SettingsScreen`
  ~2040 строк + ~1100 строк свободных функций) разбит на **8 файлов** каталога
  `screens/settings/` (byte-exact слайсы по диапазонам строк — нулевой риск
  транскрипции): `mod.rs` (650: `SettingsIntent`, enum'ы секций/подсекций
  `Section`/`Subsection`/`ModelTab`, типы формы `FieldId`/`FieldKind`/`FieldRow`/
  `Editor`/`Focus`/`SearchHit`/`SearchState`/`ChoiceState`, `SamplingParam` +
  `SAMPLING_PARAMS`, `struct SettingsScreen`, декларации подмодулей), `catalog.rs`
  (565: конструктор + построители полей всех секций/подсекций + гейты
  `gate_disabled`/`sampling_cloud_provider`), `apply.rs` (675: `handle_key` +
  диспетчеры + редактор поля + тумблеры/циклы + `apply_text`/`save_*`),
  `choice.rs` (150: попап Choice + `reset_field`/`default_fields`), `search.rs`
  (155: оверлей поиска `/`), `render.rs` (530: вся отрисовка), `helpers.rs` (1130:
  свободные функции — `row`/`grouped`/`sampling_row`/`managed_rows`/`cloud_rows`,
  `field_description`/`gate_hint`, `render_field_line`/`header_line`/`tab_strip_line`,
  циклы `cycle_*`/label-функции, парсеры `parse_*`/`decode/encode_escapes`),
  `tests.rs` (1175). Прежний `impl SettingsScreen` разложен на 4 impl-блока
  (≤ ~660 строк каждый).
- **Правила видимости (плейбук для UI-экранов):** (1) подмодули берут элементы
  `mod.rs` через `use super::*;` — glob подтягивает и приватные `use`-импорты
  родителя (ratatui/uuid/crate::…), поэтому внешние импорты в подмодулях не
  дублируются, а в `mod.rs` не «висят» (все использованы транзитивно → ноль
  warning'ов). (2) Свободные функции `helpers.rs` помечены `pub(super)` (иначе не
  видны сиблингам); потребители — `use super::helpers::*;`. (3) Приватные методы
  `impl SettingsScreen`, вызываемые из другого файла, помечены `pub(super)`
  (метод-приватность в Rust — по модулю определения).
- **Отклонения от проекта**: `descriptions.rs`/`editor.rs` не выделялись
  (`field_description`/`gate_hint` → `helpers.rs`; редактор поля → `apply.rs`,
  тесно связан с обработкой клавиш). `helpers.rs` оставлен единым модулем свободных
  функций — дальнейшее дробление отложено (независимые чистые функции, не
  запутанный impl-блок). Публичный путь модуля не изменился
  (`crate::screens::settings::{SettingsScreen, SettingsIntent}`), внешние `use` не
  тронуты (`app/runtime.rs`). **805 тестов зелёные** (0 упавших, 26 `#[ignore]`;
  число не изменилось — чистый перенос), clippy `-D warnings`/fmt чисты. Доки:
  architecture.md §3.
### Рефакторинг god-object'ов — этап 2: `screens/chat.rs` → `screens/chat/` (сделано)
- **Этап 2** плана разбора god-object'ов (docs/refactoring-god-objects.md — на ветке
  этапа 1; этот этап — ветка `refactor/chat-module-split` от `main`). `screens/chat.rs`
  (2616 строк, один `impl ChatScreen` ~54 метода на ~1100 строк + рендер-хелперы)
  разбит на каталог `screens/chat/` — чисто механический перенос (item-level слайсы:
  методы режутся по 4-пробельному `}`, свободные функции по col-0 закрытию;
  типы/поля/контракт `ChatIntent`/поведение не менялись).
- **Раскладка**: `mod.rs` (409: `ChatIntent`, типы попапов `ConfirmAction`/
  `SuggestPopup`/`ImpersonationState`/`RagBanner`, `struct ChatScreen`, аксессоры-
  снимки — сеттеры/геттеры настроек/списков/статусов/палитры), `input.rs` (367:
  `handle_key`/`handle_paste`/`handle_mouse` + черновик/орфография/команды +
  `trigger_destructive`/`handle_profile_overlay_key`), `popups.rs` (281: попапы
  орфографии/подтверждения/эмодзи/справки + `render_help`/`render_suggest`/
  `render_confirm`/`HELP_KEYS`), `feed.rs` (241: проекция `AppEvent` в ленту +
  `feed_msg_has_vs16`), `render.rs` (190: `render`/`model_meta` + `centered_rect`/
  `visual_line_count`), `rag.rs` (94: баннер + `format_rag_sources`),
  `impersonation.rs` (51), `tests.rs` (1034). Прежний `impl ChatScreen` разложен на
  7 impl-блоков (≤ ~360 строк каждый).
- **Правила видимости** (тот же плейбук, что этап 1): подмодули берут `mod.rs` через
  `use super::*;`; приватные методы, вызываемые межфайлово, и свободные функции
  помечены `pub(super)`. Межмодульные свободные функции — точечные `use`:
  `input.rs` → `super::feed::feed_msg_has_vs16`, `popups.rs` →
  `super::render::centered_rect`, `render.rs` → `super::popups::{render_help,
  render_suggest, render_confirm}`, `tests.rs` → `super::popups::HELP_KEYS`.
- Публичный путь модуля не изменился (`crate::screens::chat::{ChatScreen,
  ChatIntent}`), внешние `use` (`app/runtime.rs`) не тронуты. **805 тестов зелёные**
  (0 упавших, 26 `#[ignore]`; число не изменилось — чистый перенос), clippy
  `-D warnings`/fmt чисты. Доки: architecture.md §3.
### Рефакторинг god-object'ов — этап 4: `features/tools/notes.rs` → `features/tools/notes/` (сделано)
- **Этап 4** плана разбора god-object'ов (docs/refactoring-god-objects.md — на ветке
  этапа 1; этот этап — ветка `refactor/notes-module-split` от `main`).
  `features/tools/notes.rs` (2242 строки: 9 инструментов + подсистема self-заметок +
  обзоры консолидации) разбит на каталог `features/tools/notes/` — чисто механический
  перенос (item-level слайсы, поведение не менялось).
- **Раскладка**: `mod.rs` (123: ID-константы `NOTE_*_ID`, пороги, `SELF_NOTE_TAG`,
  общие хелперы `is_self_note`/`parse_id`/`parse_tags`/`clip`/`cosine`, реэкспорты),
  `recall.rs` (234: `NoteRecall` + `list_user_notes`/`semantic_recall`/`related_block`/
  `cited_sources_block`/`format_notes`), `edit.rs` (222: `NoteRevise`/`NoteSupersede`/
  `NoteMerge`), `overview.rs` (206: `ConsolidateNotes` + `build_consolidation_overview`/
  `build_self_consolidation_overview`), `save.rs` (161: `NoteSave` + `create_note`/
  `ensure_note_vectors`/`self_note_similar`), `self_notes.rs` (149: `self_notes_recent`/
  `self_notes_relevant`/`self_related_block`/`migrate_self_narrative`), `graph.rs` (115:
  `NoteLink`/`NoteNeighbors`), `cite.rs` (66: `NoteCiteSource`), `tests.rs` (1005).
- **Ключевое — сохранение внешней поверхности** (широкая: оркестратор/`self_model`/
  `rag`/`tools::mod`/`meta` зовут ~30 `notes::X`): mod.rs реэкспортирует всё
  `pub(crate) use self::{cite::*, edit::*, …}::*;`, поэтому внешние
  `use crate::features::tools::notes::{NoteSave, create_note, self_notes_recent, …}` не
  тронуты. Приватные кросс-подмодульные хелперы (`related_block`/`list_user_notes`/
  `semantic_recall`/`format_notes`/`ensure_note_vectors`) расширены до `pub(crate)`;
  подмодули берут всё через `use super::*;` (glob через реэкспорт родителя). Консты и
  мелкие общие хелперы оставлены в `mod.rs` (видны подмодулям как приватные предка).
- **Нюанс парсинга** (учтён): строчный `///`-докомментарий, заканчивающийся `;`
  (проза «…по хранению/поиску;»), ложно принимался за границу элемента и рвал
  докомментарий — в парсер добавлена защита «границей `;` не считается строка,
  начинающаяся с `//`».
- Публичный путь модуля не изменился. **805 тестов зелёные** (0 упавших, 26
  `#[ignore]`; число не изменилось — чистый перенос), clippy `-D warnings`/fmt чисты.
  Доки: architecture.md §3.
### Рефакторинг god-object'ов — этап 5: `shared/storage/db.rs` → `shared/storage/db/` (сделано)
- **Этап 5** плана разбора god-object'ов (docs/refactoring-god-objects.md — на ветке
  этапа 1; этот этап — ветка `refactor/db-module-split` от `main`).
  `shared/storage/db.rs` (1665 строк, один `impl Db` на ~41 метод, 4 домена данных)
  разбит на каталог `shared/storage/db/` по доменам — чисто механический перенос
  (item-level слайсы, поведение/схема БД не менялись).
- **Раскладка**: `mod.rs` (222: `struct Db`, `open`/`open_in_memory`/`from_conn`,
  `register_sqlite_vec`, **`migrate()` — вся схема одним куском**, `ensure_vec_table`/
  `vec_dim`, общие хелперы `row_to_note`/`parse_uuid`/`parse_dt`/`cosine`, декларации),
  `rag.rs` (321: документы/поиск/источники/размерность + удаление по пути +
  `delete_matching`/`delete_sources_matching`/`norm_path`), `notes.rs` (242: вставка/
  список/правка/удаление + эмбеддинги/семантика), `graph.rs` (218: граф связей +
  замещение + цитирование источников), `self_model.rs` (102: get/upsert/атомарный
  update), `tests.rs` (591). Прежний `impl Db` разложен на 5 impl-блоков (≤ ~320 строк).
- **Ключевое**: методы `Db` — inherent-методы (`pub`, вызываются как `db.method()`
  через фасад `Storage`), поэтому разложение `impl Db` по файлам **не требует
  реэкспортов** (метод резолвится по типу независимо от файла). Приватные поля
  `struct Db` (`conn`) видны подмодулям (потомки видят приватное предка); общие
  свободные хелперы `mod.rs` (private) подмодули берут через `use super::*;` (glob
  тянет приватные элементы родителя). Собралось с первого раза — ни правок видимости,
  ни явных импортов.
- Путь модуля `db` не изменился (`shared::storage::db::Db`). **805 тестов зелёные**
  (0 упавших, 26 `#[ignore]`; число не изменилось — чистый перенос), clippy
  `-D warnings`/fmt чисты. Доки: architecture.md §3.
### Рефакторинг god-object'ов — этап 6: `shared/markdown.rs` → `shared/markdown/` (сделано)
- **Этап 6** плана разбора god-object'ов (docs/refactoring-god-objects.md — на ветке
  этапа 1; этот этап — ветка `refactor/markdown-module-split` от `main`).
  `shared/markdown.rs` (1995 строк, три подсистемы: walker `Writer`, таблицы, LaTeX-
  конвертер + подсветка кода) разбит на каталог `shared/markdown/` — чисто механический
  перенос (item-level слайсы с виденьем, поведение не менялось).
- **Раскладка**: `mod.rs` (146: `render`/`render_with` — **вся внешняя поверхность** +
  стили из палитры `heading_style`/`code_style`/… + проводка), `latex.rs` (578: LaTeX→
  unicode — нормализация разделителей + конвертер команд, **самодостаточен**),
  `writer.rs` (418: `Writer` — walker событий pulldown-cmark → строки + `heading_number`),
  `table.rs` (312: `TableBuilder` + `render_table` + раскладка колонок), `code.rs` (176:
  подсветка syntect — синтаксис + тема из палитры), `tests.rs` (397).
- **Ключевое — `Writer` это хаб** (вызывает стили из `mod.rs`, подсветку из `code`,
  таблицы из `table`, LaTeX из `latex`; `mod.rs::render_with` строит `Writer`). Проводка:
  подсистемные элементы, используемые межфайлово, помечены `pub(super)` (включая поля
  `TableBuilder` — их строит/мутирует `Writer` и читает `render_table` — и поля
  `Writer.lines`/`soft_break_as_newline`, к которым обращается `render_with`); `mod.rs`
  сводит их приватным глобом `use self::{code::*, latex::*, table::*, writer::*};`, а
  подмодули берут всё через `use super::*;` (стили из `mod.rs` — как приватные предка).
- **Два урока clippy** (не ловятся `cargo build`, только `-D warnings`): (1) реэкспорт
  внутренней проводки должен быть **приватным** `use …::*` (не `pub(crate) use`) — иначе
  «glob import doesn't reexport anything with visibility pub(crate)», т.к. элементы
  `pub(super)`, а не `pub`; (2) `latex.rs` самодостаточен — его `use super::*` оказался
  неиспользованным и удалён.
- Внешняя поверхность (`markdown::render`/`render_with`, только `message_feed`) не
  тронута. **805 тестов зелёные** (0 упавших, 26 `#[ignore]`; число не изменилось —
  чистый перенос), clippy `-D warnings`/fmt чисты. Доки: architecture.md §3.

### Отложено за пределы M3
- **Сворачивание/выделение per-message** и tool-блоки в ленте — сейчас «мысли»
  сворачиваются глобально (`Ctrl+T`); выделение сообщений и tool-блоки — на M5.
  **Учесть тумблер мыши** (`Ctrl+W`, см. пост-M9): per-message выделение/копирование
  в ленте должно сосуществовать с захватом колеса — либо как in-app выделение
  (клики мыши идут приложению при включённом захвате), либо опираясь на нативное
  выделение терминала при выключенном (тогда копирование — обычным перетаскиванием).
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
