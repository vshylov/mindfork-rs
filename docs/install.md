# Установка и запуск mindfork-rs

Консольное (TUI) приложение ИИ-чата. Платформы: **Windows** и **Linux**.
Все данные — **рядом с бинарником** (портативно): отдельная папка/флешка
самодостаточна.

## 1. Сборка из исходников

Нужен **Rust** (edition 2024, свежий стабильный toolchain).

```bash
cargo build --release        # бинарник в target/release/
```

Релизный профиль включает LTO и `strip` (меньше размер, выше скорость). Профиль
сохраняет `panic = unwind` — это нужно для гарантированного восстановления
терминала при панике.

Проверки перед коммитом (зелёные на обеих платформах):

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## 2. Где лежат данные (портативно)

Рядом с исполняемым файлом создаются:

| Путь | Назначение |
|---|---|
| `settings.json` | глобальная конфигурация (модель/сервер, семплинг, инструменты, интерфейс) |
| `profiles.json` | профили ИИ-собеседника |
| `chats/{id}.json` | чаты (+ `.bak` — бэкап при перезаписи) |
| `data.db` | заметки и RAG (SQLite + sqlite-vec), изоляция по профилю |
| `dictionaries/` | словари спелл-чека (Hunspell) |
| `personal_dictionary.txt` | персональный словарь |
| `logs/` | логи (stdout занят TUI) |

В dev-сборке это `target/debug/` рядом с бинарником.

## 3. Движок инференса (llama.cpp `llama-server`)

Приложение — **HTTP-клиент** к локальному **OpenAI-совместимому** серверу. Протокол
универсален, поэтому в external-режиме подойдёт любой такой сервер (llama.cpp
`llama-server`, vLLM, LM Studio, Ollama …). Рекомендуемый и проверенный бэкенд —
**llama.cpp `llama-server`** (готовые сборки под Windows/CUDA). Вся цепочка
(стриминг, остановка по EOS, tool-calling, «мысли») проверена на Gemma 4 E4B-it.

> Исходно проектировался под `xinfer`, но он оказался слишком сырым (бессвязный
> вывод на Gemma 4, плохо собирается под Windows). Контракт протокола (он же
> OpenAI, на котором говорит и llama.cpp) — в [docs/xinfer-contract.md](xinfer-contract.md).

Два режима (настраиваются на экране настроек, `Ctrl+P`, секция «Модель/сервер»):

- **managed** — приложение само запускает дочерний `llama-server` (путь к бинарнику
  + GGUF-модель `-m`, `-ngl`, `-c`, `--jinja`, host/порт). Смена модели в настройках
  **перезапускает** сервер.
- **external** — подключение к уже запущенному серверу по URL.

Пример ручного запуска (external):

```bash
llama-server -m google_gemma-4-E4B-it-Q4_1.gguf \
  --host 0.0.0.0 --port 8000 \
  -ngl 99 -c 8192 \
  --jinja          # встроенный chat-template модели — обязателен для корректного
                   # формата Gemma и для tool-calling
```

«Мысли» (`reasoning_content`) у `llama-server` включаются флагом `--reasoning-format`
(например `auto`) для thinking-моделей; иначе mindfork подхватывает `<think>…</think>`
из текста фолбэком.

### Быстрый старт через переменные окружения (dev)

Env имеет приоритет над `settings.json` (удобно для смоук-прогонов), затрагивает
только серверы инференса/эмбеддингов:

```powershell
# external chat-сервер (любой OpenAI-совместимый)
$env:MINDFORK_ENGINE_URL = "http://127.0.0.1:8000/v1"
# или managed llama-server:
$env:MINDFORK_LLAMA_BIN = "C:\path\to\llama-server.exe"
$env:MINDFORK_MODEL     = "C:\GGUF\google_gemma-4-E4B-it-Q4_1.gguf"
$env:MINDFORK_NGL       = "99"     # GPU-слои (опц.)
$env:MINDFORK_CTX       = "8192"   # контекст (опц.)
$env:MINDFORK_PORT      = "8000"   # опц.
```

Эмбеддинги для RAG — **выделенный** сервер (ADR 0002):
`MINDFORK_EMBED_URL` (external) либо `MINDFORK_EMBED_BIN` + `MINDFORK_EMBED_MODEL`
+ `MINDFORK_EMBED_PORT` (managed). Если не настроен — RAG отдаёт ошибку, но
приложение не падает.

## 4. Словари спелл-чека

Положите Hunspell-пары `*.aff` + `*.dic` в `dictionaries/` рядом с бинарником
(`en_US.*`, `en_GB.*`, `ru_RU.*`). `build.rs` копирует словари из корня проекта
рядом с бинарником при сборке. Нет каталога → спелл-чек просто выключен. Выбор
активных словарей — на экране настроек (секция «Интерфейс»).

## 5. Импорт из LameLLaMA (.NET)

Одноразовый идемпотентный импорт профилей и чатов:

```bash
mindfork-rs --import-lamellama "C:\path\to\LameLLama.Desktop\bin\...\net10.0"
```

Импортируются: `Settings.json` (`Configurations` → профили; глобальный семплинг и
настройки интерфейса) и `Conversations/*.json` (→ чаты; `*.deleted` пропускаются).
Не переносятся: имперсонация, KV-кэш, неподдержанные параметры семплинга
(`min_p`, `repeat_penalty`, seed, mirostat, …). Команда не запускает TUI и
завершает процесс; повторный запуск не создаёт дубликатов.

## 6. Запуск

```bash
cargo run            # dev
./target/release/mindfork-rs   # релиз
```

Нужен **настоящий терминал** (TUI). В headless-окружении приложение «висит» —
это нормально. Базовые клавиши: `F1` — справка, `Ctrl+P` — настройки, `Ctrl+L` —
список чатов, `Ctrl+N` — новый чат, `Esc` — отмена/закрыть, `Ctrl+C` — выход.
Ctrl-шорткаты работают при любой раскладке клавиатуры (в т.ч. русской).

## 7. Смоук-тесты на живой модели

Юнит-тесты сервер не требуют. Сценарии против реального сервера помечены
`#[ignore]` и запускаются вручную с заданным `MINDFORK_ENGINE_URL`:

```powershell
$env:MINDFORK_ENGINE_URL = "http://127.0.0.1:8000/v1"
cargo test ignored_smoke -- --ignored --nocapture --test-threads=1
```

Покрывают: стриминг/финиш, **анти-самообрыв на тексте EOS** (`<|im_end|>` Qwen и
`<end_of_turn>` Gemma), **tool-calling** (`finish_reason=tool_calls` + разбор
`delta.tool_calls`) и **«мысли»** (`reasoning_content` → `Thoughts`). Проверено
зелёным на `google_gemma-4-E4B-it-Q4_1.gguf` через `llama-server`.
