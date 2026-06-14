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
Ветка `main` содержит всё. Сделано **M0, M1, M2**. **53 теста зелёные, 2 `#[ignore]`.**
- **M0** — каркас FSD, TUI-петля с восстановлением терминала, single-instance, логирование.
- **M1** — `shared/api` (xinfer-клиент со стримингом/отменой, супервайзер, парсер
  мыслей, mock), оркестратор (автомат + generation_id), мост tokio↔TUI, минимальный
  чат-UI. История пока в памяти.
- **M2** — `entities` (все доменные типы), `shared/config`, `shared/storage`
  (JSON атомарно + SQLite/sqlite-vec, изоляция по профилю, мягкое удаление + каскад).
  Storage пока НЕ подключён к оркестратору/UI.

### Следующий этап — M3 (UI чата), см. plan.md §M3
Крупный UI-этап. Основные задачи:
- Подключить `Storage` к оркестратору: мульти-чат, загрузка/сохранение, дебаунс.
- `widgets/`: список чатов (поиск, 2 сортировки, переименование, `Ctrl+L`), лента
  (markdown, сворачиваемые CoT и tool-блоки, скролл), ввод на `tui-textarea`, статус-бар.
- `shared/markdown.rs`: рендер markdown + unicode-аппроксимация LaTeX.
- `features/spellcheck/`: `spellbook` (en_US/en_GB/ru_RU) + сегментация + подсказки.
- Снять `#![allow(dead_code)]` из `main.rs`, когда слои свяжутся.

**Открытые `[R]` для M3** (решить ранним прототипом, зафиксировать ADR в коде/доке):
- `tui-textarea` vs `ratatui-textarea` (совместимость с ratatui 0.30).
- Покрытие `ratatui-markdown` (таблицы/подсветка/сворачивание) vs `tui-markdown`.
- Способ отрисовки спелл-чека в TUI (нет API подчёркивания произвольных диапазонов).
- Качество `spellbook` на ru_RU.

## Подводные камни
- **TUI «висит» при headless-запуске** (без TTY) — это нормально; чистый выход
  по `q`/`Esc`/`Ctrl+C` проверяется юнит-тестами `map_key`. Для живой проверки нужен
  настоящий терминал.
- `#![allow(dead_code)]` в `main.rs` — временный (часть API опережает потребителей),
  убрать на M3.
- Данные приложения портативны: лежат рядом с бинарником (в dev — `target/debug/`).
