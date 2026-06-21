# Архитектура mindfork-rs

Документ описывает **как устроен** `mindfork-rs` — консольное (TUI) приложение
ИИ-чата на Rust. Это карта кода и потоков данных: слои, модули, контракты,
жизненные циклы и инварианты. Дополняет, но не заменяет источники истины:

- **[spec.md](../spec.md)** — инженерная спецификация («что» и «почему»);
- **[CLAUDE.md](../CLAUDE.md)** — ориентир и журнал реализованного (M0–M9 + пост-M9);
- **[docs/decisions/](decisions/)** — ADR (зафиксированные технические решения);
- **[docs/install.md](install.md)** — установка/запуск, движок, env.

> Терминология: **движок** = локальный OpenAI-совместимый HTTP-сервер
> (llama.cpp `llama-server`); **приложение** = `mindfork-rs`, HTTP-клиент этого
> сервера. Agentic-loop **клиентский** — его исполняет оркестратор приложения.

---

## 1. Картина в целом

`mindfork-rs` — единый бинарный крейт на Rust (edition 2024), организованный по
**Feature-Sliced Design (FSD)**. Приложение само по себе **не содержит ML-стека**:
инференс делает внешний процесс `llama-server` (managed-подпроцесс или external),
а приложение общается с ним по HTTP (`/v1/chat/completions` SSE, `/v1/embeddings`).

Три «оси», вокруг которых построена система:

1. **Один владелец состояния.** Оркестратор (`app/orchestrator.rs`) единолично
   владеет доменным состоянием (профили, чаты, `Storage`) и единственный пишет на
   диск. UI держит только read-only-проекцию.
2. **Однонаправленный поток.** UI шлёт вверх команды `AppCommand`, оркестратор
   шлёт вниз события `AppEvent`. Никаких разделяемых мутабельных локов на `Chat` —
   значит, дедлок невозможен по построению.
3. **Транспорт за трейтом.** Всё, что зависит от сервера инференса, спрятано за
   `EngineBackend`/`Embedder` (`shared/api`) — отсюда тестируемость (mock/replay)
   и заменяемость движка.

```mermaid
flowchart TB
    subgraph TERM["Терминал (главный поток)"]
        UI["ratatui render loop<br/>(app/runtime.rs)"]
    end
    subgraph TOKIO["tokio runtime"]
        ORCH["Оркестратор<br/>(app/orchestrator.rs)<br/>владелец состояния + автомат генерации"]
        SUP["Супервайзер серверов<br/>(app/supervisor.rs)"]
        TOOLS["Инструменты<br/>(features/tools)"]
        STORE["Storage<br/>(shared/storage: JSON + SQLite)"]
    end
    subgraph EXT["Внешние процессы"]
        LLM["llama-server (chat)<br/>OpenAI /v1"]
        EMB["llama-server (embeddings)<br/>/v1/embeddings"]
    end

    UI -- "AppCommand (mpsc)" --> ORCH
    ORCH -- "AppEvent (mpsc)" --> UI
    ORCH -- "chat_stream / SSE" --> LLM
    ORCH --> TOOLS
    TOOLS -- "embed" --> EMB
    ORCH --> STORE
    ORCH --> SUP
    SUP -. "launch / probe /health" .-> LLM
    SUP -. "launch" .-> EMB
```

---

## 2. Слои FSD

Зависимости направлены **строго вниз**: `app → screens → widgets → features →
entities → shared`. Слой не импортирует «вбок» и «вверх». Кросс-срезовая
инфраструктура (движок, хранилище) живёт в `shared` за трейтами.

```mermaid
flowchart TD
    app["app/<br/>оркестратор, runtime-петля, события, супервайзер"]
    screens["screens/<br/>chat, settings — целостные экраны + Intent"]
    widgets["widgets/<br/>message_feed, input_box, chat_list, status_bar, …"]
    features["features/<br/>tools, spellcheck, profiles, migration, rag_*, …"]
    entities["entities/<br/>chat, message, profile, note, rag, sampling"]
    shared["shared/<br/>api, storage, config, markdown, wrap, theme, keys, paths, …"]

    app --> screens
    app --> widgets
    app --> features
    app --> entities
    app --> shared
    screens --> widgets
    screens --> features
    screens --> entities
    screens --> shared
    widgets --> features
    widgets --> entities
    widgets --> shared
    features --> entities
    features --> shared
    entities --> shared
```

**Ключевой инвариант FSD в коде:** `screens`/`widgets` **не импортируют `app`**.
Экран не знает про `AppCommand` — он возвращает собственное намерение
(`ChatIntent`/`SettingsIntent`/`ChatListAction`), а `app/runtime.rs` транслирует
его в `AppCommand`. Аналогично терминальные side-effect'ы (захват мыши, запись в
буфер обмена) экран не делает сам — он сигнализирует намерением, исполняет
`runtime`.

### Соответствие слоям попытки №1 (`lamellama-rs`)

| lamellama-rs (крейт) | mindfork-rs (слой/модуль)        |
|----------------------|----------------------------------|
| `llama-core`         | `entities/`                      |
| `llama-app`          | `app/orchestrator.rs` + `features/` |
| `llama-engine`       | `shared/api/` (`OpenAiClient`)   |
| `llama-tools`        | `features/tools/`                |
| `llama-storage`      | `shared/storage/`                |
| `llama-spell`        | `features/spellcheck/`           |
| `llama-ui` (`gpui`)  | `screens/` + `widgets/`          |

---

## 3. Карта модулей

```
src/
├─ main.rs                  тонкая точка входа: конфиг, env-overrides, single-instance,
│                           tokio, ratatui::init, импорт LameLLaMA (CLI-флаг)
│
├─ app/                     композиция: оркестрация, петля TUI, контракт, серверы
│  ├─ orchestrator/         владелец состояния, расслоён по фичам (god-объект разбит,
│  │  │                     владелец `Chat` остался один — см. ниже §10):
│  │  ├─ mod.rs             каркас: Orchestrator (16 полей), петля run(), диспетчер
│  │  │                     команд, общие хелперы (эмиттеры, chat_mut, mark_dirty)
│  │  ├─ engines.rs         EngineManager: жизненный цикл серверов, готовность,
│  │  │                     apply_chat/embed/impersonation, backend_if_ready
│  │  ├─ save_queue.rs      SaveQueue: дебаунс-очередь отложенного сохранения чатов
│  │  ├─ generation.rs      отправка/перегенерация/удаление обмена + задача agentic-loop
│  │  ├─ chats.rs           список чатов (new/switch/rename/clone/copy/delete) + черновик
│  │  ├─ profiles.rs        создание/правка/удаление профилей
│  │  ├─ settings.rs        конфиг + (пере)запуск серверов через супервайзер
│  │  ├─ title.rs           авто-название чата (фоновая задача)
│  │  ├─ impersonation.rs   реплика «за пользователя» (фоновая задача)
│  │  ├─ rag.rs             индексация/удаление файлов в базе знаний
│  │  └─ request.rs         маппинг доменных сообщений в формат движка
│  ├─ gen_state.rs          GenState: чистый автомат Idle/Generating/Cancelling
│  │                        (переходы begin/request_cancel/finish, без I/O)
│  ├─ events.rs             AppCommand (UI→оркестр.) и AppEvent (оркестр.→UI)
│  ├─ runtime.rs            мост tokio↔TUI: батчинг ввода, dirty-перерисовка,
│  │                        трансляция Intent→AppCommand, side-effect'ы (мышь/буфер)
│  └─ supervisor.rs         ServerSupervisor: (пере)запуск managed / подключение external
│
├─ screens/                 целостные экраны (FSD "pages"); НЕ зависят от app
│  ├─ chat.rs               ChatScreen: всё состояние UI чата, handle_key→ChatIntent
│  └─ settings.rs           SettingsScreen: секции/поля, подсекции Ассистент/Имперсонация
│
├─ widgets/                 составные UI-блоки (FSD "widgets")
│  ├─ message_feed.rs       лента: markdown, мысли, инлайн tool-блоки, скролл, перенос
│  ├─ input_box.rs          свой multiline-ввод (ADR 0001): курсор, перенос, спелл-чек,
│  │                        однострочный режим (поля настроек), визуальная навигация
│  ├─ chat_list.rs          полноэкранный оверлей списка чатов (поиск/сортировка/F2/F5)
│  ├─ status_bar.rs         модель/токены/профиль/состояние сервера/режим мыши
│  ├─ profile_list.rs       оверлей выбора профиля при создании чата
│  └─ impersonation_preview.rs  потоковый предпросмотр реплики (Ctrl+U)
│
├─ features/                пользовательские сценарии (FSD "features")
│  ├─ tools/                реестр и реализации инструментов (client-side)
│  │  ├─ mod.rs             Tool, ToolContext, ToolOutcome/ChatEffect, ToolRegistry, ToolConfig
│  │  ├─ rag.rs             rag_add/rag_search: чанкинг, эмбеддинг, kNN, склейка
│  │  ├─ notes.rs           note_save/note_recall
│  │  ├─ introspection.rs   get/set_sampling, get/set_system_message, get_last_user_message_time
│  │  ├─ python.rs          python_exec (subprocess, таймаут)
│  │  ├─ web.rs             web_search (мульти-провайдер DDG/Mojeek/Ecosia + анти-бот)
│  │  └─ subagent.rs        call_subagent (без истории/инструментов, запрет вложенности)
│  ├─ spellcheck/           check, segment, dict, mod — Hunspell + сегментатор + личн. словарь
│  ├─ profiles.rs           чистые операции над профилями (sanitize_name, ProfileEdit)
│  ├─ chat_search_sort.rs   фильтр/сортировка списка чатов
│  ├─ rename_chat.rs        авто-название (digest, чистка), переименование
│  ├─ chat_export.rs        format_conversation (копирование переписки)
│  ├─ rag_command.rs        парсер /rag add|remove
│  ├─ rag_ingest.rs         scan, read_text, RagProgress (типы прогресса индексации)
│  └─ migration.rs          импортёр LameLLaMA (.NET): профили + чаты
│
├─ entities/                доменные типы (без I/O); serde-сериализуемы
│  ├─ chat.rs               Chat, ChatSummary, CharacterNames, Chat::from_profile, draft
│  ├─ message.rs            Message, MessageRole, ToolCallRecord, MessageMetadata
│  ├─ profile.rs            Profile, ProfileSummary, ToolId
│  ├─ note.rs               Note
│  ├─ rag.rs                RagDocument / RagHit
│  └─ sampling.rs           SamplingConfig, ReasoningEffort, resolve (трёхуровневый приоритет)
│
└─ shared/                  инфраструктура и утилиты (FSD "shared")
   ├─ api/                  слой движка инференса
   │  ├─ backend.rs         EngineBackend, Embedder, ChatRequest/Chunk, ToolCallAccumulator
   │  ├─ client.rs          OpenAiClient (reqwest + SSE), probe /health, embed
   │  ├─ server.rs          ServerHandle (managed-процесс), ManagedConfig, wait_until_ready
   │  ├─ thoughts.rs        потоковый парсер <think> (fallback к reasoning_content)
   │  ├─ wire.rs            (де)сериализация OpenAI-формата (приватный)
   │  └─ mock.rs            mock-движок для тестов (#[cfg(test)])
   ├─ storage/              хранилище
   │  ├─ json.rs            атомарная запись (write-rename + .bak) конфиг/профили/чаты
   │  ├─ db.rs              SQLite + sqlite-vec: notes/RAG, изоляция по profile_id
   │  └─ mod.rs             фасад Storage (потокобезопасный)
   ├─ config.rs            AppConfig и секции (Engine/Embed/Tool/Interface/Impersonation…)
   ├─ markdown.rs          свой рендерер на pulldown-cmark (ADR 0003): таблицы + LaTeX + тема
   ├─ wrap.rs              перенос слов по колонкам (unicode-width)
   ├─ theme.rs             Palette (роли user/assistant/tool/…), auto/dark/light
   ├─ keys.rs              раскладко-независимые Ctrl-шорткаты (ЙЦУКЕН→латиница)
   ├─ server.rs            ServerStatus (статус сервера для UI)
   ├─ paths.rs             портативные пути рядом с бинарником
   ├─ instance.rs          single-instance
   └─ logging.rs           tracing в файл (stdout занят TUI)
```

---

## 4. Поток данных UI ↔ оркестратор

Сердце архитектуры — однонаправленный поток через два `mpsc`-канала. Контракт
описан в [`app/events.rs`](../src/app/events.rs).

```mermaid
flowchart LR
    subgraph MAIN["Главный поток — петля рендеринга (runtime.rs)"]
        POLL["poll input<br/>(crossterm, таймаут)"]
        BATCH["батчинг событий<br/>process_input_batch (вставка)"]
        SCREEN["ChatScreen / SettingsScreen<br/>handle_key → Intent"]
        VIEW["view-model<br/>(read-only проекция)"]
        DRAW["ratatui draw<br/>(по флагу dirty)"]
    end
    subgraph TASK["tokio — задача оркестратора"]
        LOOP["select!: cmd_rx / done / status / title / imp / rag"]
        STATE["State: Idle / Generating{id} / Cancelling{id}"]
        DOMAIN["профили, чаты, active_id, Storage"]
    end

    POLL --> BATCH --> SCREEN
    SCREEN -- "Intent→AppCommand (cmd_tx)" --> LOOP
    LOOP --> STATE
    LOOP --> DOMAIN
    LOOP -- "AppEvent (evt_tx)" --> VIEW
    VIEW --> DRAW
```

### Команды и события (контракт)

`AppCommand` (UI → оркестратор) включает: `SendMessage`, `SetDraft`,
`RegenerateLast`, `DeleteLastExchange`, `Cancel`, `Impersonate`/
`CancelImpersonation`, `NewChat`, `SwitchChat`, `RenameChat`/`AutoRenameChat`,
`CloneChat`, `CopyChat`, `DeleteChat`, `CreateProfile`/`DeleteProfile`,
`UpdateConfig`/`UpdateProfile`, `RagAdd`/`RagDelete`, `Quit`.

`AppEvent` (оркестратор → UI) включает: `ServerStatus`, `ChatList`,
`ChatRenamed`, `ChatListError`, `CopyToClipboard`, `ProfileList`, `Settings`,
`ChatActivated`, `UserMessage`, `RestoreInput`, `GenerationStarted`, `Chunk`,
`Thoughts`, `ToolCall`, `Finished`, `Impersonation{Started,Chunk,Finished}`,
`RagProgress`, `Error`.

### Инварианты потока

- **Сериализация команд.** Оркестратор — одна `tokio`-задача с `select!` по
  каналам; команды обрабатываются строго последовательно. Гонок памяти нет —
  через каналы передаются владеемые значения (клоны/снимки).
- **`generation_id`.** Каждый запуск генерации получает `Uuid`; все стрим-события
  несут его. События с неактуальным id UI отбрасывает — классическая гонка «Stop →
  сразу Send/Regenerate → долетели хвосты старого стрима» закрыта.
- **dirty-перерисовка.** `runtime.rs` рисует кадр только по изменениям
  (применённое событие, ввод/мышь/ресайз, перезагрузка словаря, пересчёт
  орфографии). На простое экран не перерисовывается — иначе `ratatui` каждый тик
  переставлял бы курсор и сбивал фазу его мигания. Тело петли при этом крутится
  каждый тик и пробуждает отложенные по дебаунсу действия (перепроверку
  орфографии, анимацию спиннеров RAG/имперсонации).
- **Батчинг ввода.** Крупная вставка из буфера на Windows приходит обычными
  `KeyEvent` посимвольно (bracketed paste у crossterm работает только на unix);
  `process_input_batch` коалесит серию текстовых клавиш (≥2) в `Chunk::Paste`,
  чиня и торможение, и ложную отправку по `Enter` внутри вставки.

---

## 5. Жизненный цикл генерации и клиентский agentic-loop

Автомат на активный чат — выделен в `GenState` ([`app/gen_state.rs`](../src/app/gen_state.rs)):
чистый тип с валидными по построению переходами (`begin`/`request_cancel`/
`finish`), оркестратор лишь вызывает их и исполняет side-effect'ы вокруг.

```mermaid
stateDiagram-v2
    [*] --> Idle
    Idle --> Generating: SendMessage / RegenerateLast / Impersonate<br/>(гейт: сервер Ready)
    Generating --> Cancelling: Cancel
    Generating --> Idle: Finished (Stop/Length/Error)
    Cancelling --> Idle: задача завершилась (частичный ответ сохранён)
    Idle --> Idle: команды списка/настроек/RAG (не зависят от генерации)
```

Сама генерация исполняется в **отдельной `tokio`-задаче**, которая стримит
`AppEvent` в UI напрямую, а итог (новые доменные сообщения + эффекты инструментов)
возвращает оркестратору через внутренний канал `done_tx` (тип `GenResult`).
Оркестратор — единственный владелец `Chat` — дописывает сообщения и применяет
эффекты сам.

**Клиентский agentic-loop** (spec §6.3): сервер не исполняет наши инструменты,
цикл ведёт оркестратор между HTTP-раундами.

```mermaid
sequenceDiagram
    participant UI
    participant ORCH as Оркестратор (gen task)
    participant LLM as llama-server
    participant REG as ToolRegistry

    UI->>ORCH: SendMessage(text)
    Note over ORCH: проверка State::Idle + сервер Ready
    ORCH->>UI: GenerationStarted{id}
    loop round < max_tool_rounds
        ORCH->>LLM: chat_stream(req, cancel)
        LLM-->>ORCH: Text / Thoughts / ToolCall deltas
        ORCH->>UI: Chunk / Thoughts (по generation_id)
        alt finish = Stop / Length / Cancelled
            ORCH->>UI: Finished{reason}
            Note over ORCH: зафиксировать ответ, выйти
        else finish = ToolCalls
            ORCH->>REG: invoke(name, args, &ToolContext)
            REG-->>ORCH: ToolOutcome{result, effects}
            ORCH->>UI: ToolCall{name,args,result}
            Note over ORCH: applied: tool-message в историю,<br/>effects применяются оркестратором
        end
    end
    ORCH-->>UI: (через done_tx) новые Message + ChatEffect
```

Детали:

- `max_tool_rounds` (по умолчанию **8**) — защита от зацикливания.
- **Гейт готовности сервера.** Отправка/регенерация/имперсонация стартуют только в
  `ServerStatus::Ready`. Проба бьёт в `/health` (вне `/v1`): `200` — готов, `503
  Loading model` — ещё грузится (не готов), `404` — сервер без `/health`, считаем
  живым. Это не даёт первому запросу уйти на ещё загружающийся managed-сервер и
  упасть на `503`. При неготовности отправки текст возвращается в поле ввода
  (`RestoreInput`).
- **«Мысли» (CoT).** Основной путь — `delta.reasoning_content` (`llama-server
  --reasoning-format`) → `ChatChunk::Thoughts`. Запасной — потоковый парсер
  `<think>…</think>` (`shared/api/thoughts.rs`), корректно склеивающий теги на
  границе чанка.
- **EOS.** Остановка строго по token-id спец-токенов модели (на стороне сервера);
  строковые `stop` по тексту EOS приложение **не отправляет** (анти-самообрыв).
- **Регенерация** усекает историю по последнее сообщение пользователя
  включительно и перезапускает `start_generation`; **удаление последнего обмена**
  снимает последний user+assistant и возвращает текст пользователя в ввод.

---

## 6. Слой движка (`shared/api`)

Транспорт спрятан за двумя трейтами — это даёт замену движка и mock в тестах.

```mermaid
classDiagram
    class EngineBackend {
        <<trait>>
        +chat_stream(req, cancel) ChatStream
    }
    class Embedder {
        <<trait>>
        +embed(texts) Vec~Vec~f32~~
    }
    class OpenAiClient {
        reqwest + SSE
        +probe() /health
    }
    class UnavailableEmbedder {
        RAG не настроен → ошибка
    }
    class MockBackend {
        #[cfg(test)]
    }
    EngineBackend <|.. OpenAiClient
    EngineBackend <|.. MockBackend
    Embedder <|.. OpenAiClient
    Embedder <|.. UnavailableEmbedder
```

- **`ChatRequest`** = `system` + `messages` (user/assistant/tool, включая
  `tool_calls` и tool-результаты) + `sampling` + `tools` (OpenAI-схемы). История
  append-only → сервер переиспользует prefix cache.
- **`ChatChunk`** = `Text` | `Thoughts` | `ToolCall(ToolCallDelta)` | `Finished`.
  `ToolCallAccumulator` собирает разрезанные по чанкам вызовы по `index`.
- **`ServerHandle`** (`server.rs`) владеет дочерним `llama-server` (`kill_on_drop`).
  `build_args` собирает CLI (`-m`, `-ngl`, `-c`, `--jinja`, `--no-mmap`; для
  эмбеддингов — `--embeddings -ub <ctx> -b <ctx>`). **Предполёт:** если
  `model_path` задан, но файла нет — `bail!` до `spawn` (иначе зависание в
  «подключение…» до таймаута).

### Управление серверами — `ServerSupervisor` (`app/supervisor.rs`)

Супервайзер живёт в `app` (композиционный клей, знающий и про `shared/config`, и
про `shared/api`). За трейтом — ради `MockSupervisor` в тестах (отдаёт `Ready`
синхронно, чтобы тесты не зависели от гонки статуса).

Со стороны оркестратора жизненный цикл серверов инкапсулирован в **`EngineManager`**
(`app/orchestrator/engines.rs`, Фаза 3): владеет движками (`backend`/`imp_backend`/
`embedder`), опорами на managed-процессы, статусами готовности и каналами probe;
экспонирует `apply_chat/embed/impersonation`, `backend_if_ready`,
`impersonation_backend_if_ready`, `embedder()`. Это убрало ~11 полей из
`Orchestrator` (27 → 16), не затронув инвариант «единственный владелец `Chat`».

```mermaid
flowchart TB
    CFG["AppConfig<br/>(EngineSettings / EmbedSettings / ImpersonationEngineSettings)"]
    SUP["ServerSupervisor (LlamaSupervisor)"]
    CFG --> SUP
    SUP -->|apply_chat| CHAT["chat backend + ServerHandle + status<br/>фоновый probe → status_tx"]
    SUP -->|apply_embed| EMB["embedder + ServerHandle (RAG ленив, без probe)"]
    SUP -->|apply_impersonation| IMP["managed/external сервер имперсонации<br/>(shared → переиспользует chat-сервер)"]
```

Два режима из настроек: **managed** (приложение запускает дочерний `llama-server`,
ждёт `/health`, при выходе — kill) и **external** (подключение к уже запущенному
любому OpenAI-совместимому серверу). Смена модели в настройках = перезапуск
сервера; смена настроек эмбеддингов — пере-поднятие embedding-сервера.

**Эмбеддинги — выделенный сервер** ([ADR 0002](decisions/0002-embeddings-dedicated-server.md)):
отдельный процесс/порт, трейт `Embedder` отделён от `EngineBackend`. Не настроен →
`UnavailableEmbedder` (RAG отдаёт понятную ошибку, не падает).

---

## 7. Доменная модель и хранение

Доменные типы (`entities/`) — без I/O, serde-сериализуемы.

```mermaid
erDiagram
    PROFILE ||--o{ CHAT : "profile_id"
    PROFILE ||--o{ NOTE : "profile_id (изоляция)"
    PROFILE ||--o{ RAG_DOCUMENT : "profile_id (изоляция)"
    CHAT ||--o{ MESSAGE : "messages[]"
    MESSAGE ||--o{ TOOL_CALL_RECORD : "tool_calls[]"

    PROFILE {
        Uuid id
        string name
        string default_system_message
        string impersonation_system_message
        Option_SamplingConfig default_sampling
        Vec_ToolId enabled_tools
        bool is_hidden
    }
    CHAT {
        Uuid id
        Uuid profile_id
        string title
        string system_message
        string draft
        Option_SamplingConfig sampling_override
        bool is_hidden
    }
    MESSAGE {
        Uuid id
        MessageRole role
        string text
        Option_String thoughts
        Option_MessageMetadata metadata
    }
    NOTE { Uuid id }
    RAG_DOCUMENT { Uuid id }
    TOOL_CALL_RECORD { string id }
```

### Двухуровневое хранение (`shared/storage`)

```mermaid
flowchart LR
    STORE["Storage (фасад, потокобезопасный)"]
    subgraph JSON["JSON (json.rs) — атомарно write-rename + .bak"]
        SET["settings.json"]
        PRO["profiles.json"]
        CHATS["chats/{id}.json"]
    end
    subgraph DB["SQLite + sqlite-vec (db.rs)"]
        NOTES["notes (profile_id)"]
        RAGD["rag_documents (profile_id)"]
        VEC["rag_vectors vec0 (rowid)"]
    end
    STORE --> JSON
    STORE --> DB
```

Инварианты хранения:

- **Изоляция по `profile_id`** — обязательный `WHERE profile_id = ?` во всех
  запросах notes/RAG (покрыто негативными тестами).
- **Мягкое удаление** (`is_hidden`) везде: скрытие профиля каскадно скрывает его
  чаты и исключает заметки/RAG из выборок. Физического удаления нет.
- **Один писатель** — оркестратор. Чаты сохраняются с дебаунсом (800 мс,
  `save_deadline` + множество `dirty`); запись атомарна (write-rename), бэкап `.bak`.
- **Размерность эмбеддингов** фиксируется по первому ответу `/v1/embeddings` и
  хранится в схеме sqlite-vec (`meta.rag_dim`).

### Трёхуровневый семплинг (`entities/sampling.rs`)

`resolve` разрешает приоритет **во время запроса** (не снимком при создании):
`Chat.sampling_override` → `Profile.default_sampling` → глобальный
`settings.json`. Снимок фактически применённого — в `Message.metadata`.

---

## 8. Система инструментов (`features/tools`)

Контракт инструмента — read-only-снимок + возврат эффектов (без локов на `Chat`).

```mermaid
flowchart TB
    REG["ToolRegistry<br/>schemas_for = профиль ∩ реестр · invoke(name,args,ctx)"]
    CTX["ToolContext (снимок на начало хода)<br/>profile_id, chat_id, system_message,<br/>effective_sampling, last_user_message_at,<br/>storage: Arc&lt;Storage&gt;, engine, embedder"]
    OUT["ToolOutcome { result: String, effects: Vec&lt;ChatEffect&gt; }"]
    EFF["ChatEffect: SetSystemMessage | SetSamplingOverride"]

    REG --> CTX
    CTX --> OUT
    OUT --> EFF
    EFF -. "применяет оркестратор (владелец Chat)" .-> ORCH["Chat"]
```

Реестр строится из `ToolConfig` (`standard_registry(&ToolConfig)`) и
пересобирается при правках `config.tools`. Эффективный набор инструментов =
`Profile.enabled_tools` ∩ глобальные выключатели (`effective_tool_ids`);
agentic-loop **гейтит и сам вызов** (выключенный инструмент отклоняется).

| Группа         | Инструменты                                                   |
|----------------|---------------------------------------------------------------|
| Память/знания  | `note_save`, `note_recall`, `rag_add`, `rag_search`           |
| Интроспекция   | `get_sampling`, `set_sampling`, `get_system_message`, `set_system_message`, `get_last_user_message_time` |
| Внешние        | `web_search` (мульти-провайдер + анти-бот), `python_exec` (subprocess) |
| Осознанность   | `call_subagent` (без истории/инструментов, запрет вложенности) |

Особенности реализации:

- **`call_subagent`** — независимый одно-ходовый запрос через `ctx.engine`:
  заданное `system`, единственное `user`-сообщение, `tools: []` (запрет
  рекурсии), лимит токенов и таймаут.
- **`web_search`** — фоллбэк по провайдерам (DDG lite → DDG html → Mojeek →
  Ecosia); распознаёт анти-бот троттлинг (HTTP 202/403/429) и переключает
  провайдера, а не парсит пустую выдачу.
- **RAG** — умный чанкинг с перекрытием (`chunk_text`/`chunk_markdown`), при
  извлечении — склейка соседних чанков по дословному перекрытию (`stitch_hits`).
  Загрузка/удаление файлов — команды `/rag add|remove` (фоновая индексация,
  отменяемая, изоляция по профилю).

---

## 9. UI: экраны, виджеты, рендеринг

```mermaid
flowchart TB
    RT["runtime.rs (петля)"]
    subgraph SC["screens (отдают Intent, не AppCommand)"]
        CHAT["ChatScreen<br/>handle_key→ChatIntent, мутаторы-проекция AppEvent"]
        SET["SettingsScreen<br/>секции/поля → SettingsIntent"]
    end
    subgraph WG["widgets"]
        FEED["message_feed"]
        INP["input_box"]
        CL["chat_list (оверлей)"]
        SB["status_bar"]
        PL["profile_list (оверлей)"]
        IP["impersonation_preview"]
    end
    subgraph SH["shared (рендер)"]
        MD["markdown (pulldown-cmark)"]
        WR["wrap (перенос по колонкам)"]
        TH["theme (Palette)"]
        KEY["keys (раскладко-независимость)"]
    end

    RT --> CHAT
    RT --> SET
    CHAT --> FEED & INP & CL & SB & PL & IP
    FEED --> MD --> WR
    FEED --> WR
    INP --> WR
    FEED & SB & INP & CL --> TH
    CHAT & SET & CL --> KEY
```

Решения, закреплённые ADR:

- **Свой `InputBox`** ([ADR 0001](decisions/0001-ui-crates-ratatui-030.md)):
  `tui-textarea`/`ratatui-textarea` несовместимы с ratatui 0.30 и не дают
  подсветки произвольных диапазонов. Свой виджет даёт полный контроль над
  `Enter`/`Shift+Enter`, скроллом, подчёркиванием орфографии (per-span
  `UNDERLINED`), визуальной навигацией `↑/↓` по рядам переноса, однострочным
  режимом для полей настроек.
- **Свой markdown-рендерер** ([ADR 0003](decisions/0003-own-markdown-renderer.md)):
  walker по событиям `pulldown-cmark` (`render(input, width, palette)`).
  Поддерживает таблицы (box-drawing, раскладка колонок «водоналивом»),
  delimiter-scoped LaTeX→unicode (только внутри `$…$`/`$$…$$`), тему (`Palette`),
  подсветку кода (`syntect` + `ansi-to-tui`).
- **Перенос слов** (`shared/wrap.rs`) считается **до** `Paragraph` (ширина в
  колонках через `unicode-width`) — это сохраняет row-based скролл/«хвост» в ленте
  и точный маппинг курсора во вводе. `Paragraph::wrap` сознательно не используется.
- **Спелл-чек** (`features/spellcheck`): Hunspell (`spellbook`) + свой сегментатор
  + личный словарь; слово верно, если принято хотя бы одним активным словарём;
  загрузка словарей — в фоне по настройкам интерфейса.
- **Раскладко-независимость** (`shared/keys.rs`): Ctrl-символ нормализуется в
  «физическую» латинскую клавишу (таблица ЙЦУКЕН), поэтому шорткаты работают и при
  кириллической раскладке.

---

## 10. Конкурентность и владение

```mermaid
flowchart TB
    subgraph MAIN["Главный поток"]
        L["render loop (синхронный)"]
    end
    subgraph RUNTIME["tokio runtime"]
        O["задача оркестратора (1 шт.)"]
        G["задача генерации (на время хода)"]
        T["фоновые задачи: авто-название, имперсонация, RAG-индексация"]
        P["фоновый probe готовности сервера"]
    end
    L <-->|"cmd_tx / evt_tx (mpsc)"| O
    O -->|"spawn + done_tx"| G
    O -->|"spawn + title_tx / imp_done_tx / evt_tx"| T
    O -->|"status_tx / imp_status_tx"| P
```

Принципы:

- **Единственный владелец `Chat`** — оркестратор. Инструменты получают
  неизменяемый снимок (`ToolContext`) и возвращают `effects` значением; никакого
  канала `ChatEffect` и реентерабельных локов — дедлок невозможен по построению
  (упрощение относительно серверного agentic-loop попытки №1).
- **Внутренние каналы** оркестратора (`done_tx`, `status_tx`, `title_tx`,
  `imp_done_tx`, `imp_status_tx`) собирают результаты фоновых задач обратно в
  главный `select!`-цикл, сохраняя единую точку записи состояния.
- **Отмена** — `CancellationToken` прерывает HTTP-стрим/фоновую задачу; частичный
  ответ фиксируется; новая задача того же рода отменяет предыдущую (RAG,
  имперсонация).
- **Восстановление терминала** — `ratatui::init` ставит panic-hook; захват мыши
  всегда снимается на выходе и при панике. `panic = "unwind"` в релизе сохранён
  намеренно ради drop-guard'ов.

---

## 11. Точка входа, конфигурация, сборка

- **`main.rs`** — тонкая: грузит `AppConfig`, сеет его env-переменными
  (`apply_env_overrides` — dev-workflow), проверяет single-instance, инициализирует
  логирование (в файл) и `tokio`, поднимает `ratatui`, запускает оркестратор и
  петлю. CLI-флаг `--import-lamellama <dir>` запускает импортёр и выходит.
- **Конфигурация** (`shared/config.rs`): `AppConfig` с секциями `EngineSettings`,
  `EmbedSettings`, `ToolSettings`, `InterfaceSettings`, `ImpersonationEngineSettings`,
  `impersonation_sampling`, глобальный семплинг. Все поля под `#[serde(default)]` —
  старые `settings.json` читаются без миграции. Выбор бэкенда — через настройки или
  env (`MINDFORK_ENGINE_URL` external / `MINDFORK_LLAMA_BIN` + `MINDFORK_MODEL`…
  managed; аналогично `MINDFORK_EMBED_*`).
- **Портативность** (`shared/paths.rs`): все данные — рядом с бинарником (в dev —
  `target/debug/`): `settings.json`, `profiles.json`, `chats/`, `data.db`,
  `personal_dictionary.txt`, `dictionaries/`, `logs/`.
- **Зависимости** (см. [Cargo.toml](../Cargo.toml)): `ratatui` 0.30 + `crossterm`,
  `tokio`, `reqwest` (rustls), `rusqlite` (bundled) + `sqlite-vec`,
  `pulldown-cmark` + `syntect` + `ansi-to-tui`, `spellbook`, `arboard`, `scraper`,
  `serde`/`serde_json`, `uuid`, `chrono`, `tracing`. Приложение **не зависит от
  ML-стека** — это ключевое упрощение сборки.
- **Релиз**: `[profile.release]` с LTO/strip/`opt-level=3`, `panic=unwind`.

---

## 12. Тестируемость

Архитектура спроектирована тестируемой без сервера/GPU/сети:

- **Движок** — за трейтом `EngineBackend`; в тестах `MockBackend` (replay потока
  чанков). Реальный сервер — `#[ignore]`-смоуки (анти-самообрыв, tool-calling,
  «мысли» — проверены на Gemma 4 и Qwen).
- **Инструменты** — на `MockEmbedder` (bag-of-chars + L2) и mock-`Storage`:
  корректность результата, возврат `effects` (а не мутация), валидность JSON-схем.
- **Хранилище** — `tempfile` + SQLite `:memory:`: изоляция по `profile_id`
  (негативные тесты), мягкое удаление/каскад, атомарность, kNN.
- **Оркестратор** — без UI и модели: автомат, гонки (spam Send, Stop→Send,
  отбрасывание по `generation_id`), agentic-loop, `max_tool_rounds`, приоритеты
  семплинга, логика «удалить последнее».
- **UI** — чистая логика: применение последовательности `AppEvent` к проекции,
  рендер markdown/LaTeX, чистые функции (`chunk_batch`, `format_conversation`,
  `rag_command::parse`, перенос слов).

Статус (CLAUDE.md): план M0–M9 + пост-M9 выполнен; **333+ тестов зелёные**, 6
`#[ignore]`-смоуков на живом сервере.

---

*Документ описывает реализацию `mindfork-rs`. Источник истины по «что/почему» —
[spec.md](../spec.md); зафиксированные решения — [ADR](decisions/); журнал
реализации и актуальный статус — [CLAUDE.md](../CLAUDE.md).*
