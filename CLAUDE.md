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
Сделано **M0, M1, M2** (в `main`) и **весь M3** (в ветке `m3-ui`).
**133 теста зелёные, 2 `#[ignore]`.** Полный чат-цикл в TUI готов.
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
