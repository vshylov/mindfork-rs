# Архитектура mindfork-rs

Документ описывает **как устроен** `mindfork-rs` — консольное (TUI) приложение
ИИ-чата на Rust. Это карта кода и потоков данных: слои, модули, контракты,
жизненные циклы и инварианты. Дополняет, но не заменяет источники истины:

- **[spec.md](../spec.md)** — инженерная спецификация («что» и «почему»);
- **[CLAUDE.md](../CLAUDE.md)** — ориентир и журнал реализованного (M0–M9 + пост-M9);
- **[docs/decisions/](decisions/)** — ADR (зафиксированные технические решения),
  включая [0004](decisions/0004-engine-contract-multi-provider.md) — границы
  `shared/api` и мульти-провайдерный инференс без крейт-сплита;
- **[docs/install.md](install.md)** — установка/запуск, движок, env.

> Терминология: **движок** = провайдер инференса за трейтом `EngineBackend` —
> локальный OpenAI-совместимый сервер (llama.cpp `llama-server`) **или** облако
> (OpenAI / Gemini / Anthropic, [ADR 0004](decisions/0004-engine-contract-multi-provider.md));
> **приложение** = `mindfork-rs`, HTTP-клиент движка. Agentic-loop **клиентский** —
> его исполняет оркестратор приложения.

---

## 1. Картина в целом

`mindfork-rs` — единый бинарный крейт на Rust (edition 2024), организованный по
**Feature-Sliced Design (FSD)**. Приложение само по себе **не содержит ML-стека**:
инференс делает внешний движок — локальный процесс `llama-server` (managed-подпроцесс
или external) либо облачный API (OpenAI / Gemini / Anthropic), — а приложение
общается с ним по HTTP (SSE-стриминг, `/v1/embeddings` для RAG; см. §6 и ADR 0004).

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
(`ChatIntent`/`ChatListIntent`/`SettingsIntent`; виджет списка отдаёт экрану
`ChatListAction`), а `app/runtime.rs` транслирует его в `AppCommand`. Аналогично терминальные side-effect'ы (захват мыши, запись в
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
│  │  │                     владелец `Chat` остался один — см. ниже §11):
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
│  │  ├─ reflection.rs      авто-рефлексия «модели себя» (окно/ватермарк, сигналы)
│  │  ├─ consolidation.rs   авто-консолидация заметок («сон»)
│  │  ├─ tool_loop.rs       общий «тихий» agentic-loop фоновых задач (рефлексия/консолидация)
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
│  ├─ chat_list.rs          ChatListScreen: полноэкранный список чатов (Esc), → ChatListIntent
│  └─ settings.rs           SettingsScreen: секции/поля, подсекции Ассистент/Имперсонация
│
├─ widgets/                 составные UI-блоки (FSD "widgets")
│  ├─ message_feed.rs       лента: markdown, мысли, инлайн tool-блоки, скролл, перенос
│  ├─ input_box.rs          свой multiline-ввод (ADR 0001): курсор, перенос, спелл-чек,
│  │                        однострочный режим (поля настроек), визуальная навигация
│  ├─ chat_list.rs          виджет списка чатов (поиск/сортировка/F2/F5); обёрнут ChatListScreen
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
│  │  ├─ fetch.rs           fetch_url (загрузка страницы + саммаризация через движок)
│  │  ├─ calc.rs            calculate (свой вычислитель математических выражений)
│  │  ├─ datetime.rs        current_time (дата/время, chrono)
│  │  ├─ fs.rs              fs_read/fs_write/fs_list (файлы; гейт fs_enabled + песочница)
│  │  └─ subagent.rs        call_subagent (без истории/инструментов, запрет вложенности)
│  ├─ spellcheck/           check, segment, dict, mod — Hunspell + сегментатор + личн. словарь
│  ├─ profiles.rs           чистые операции над профилями (sanitize_name, ProfileEdit)
│  ├─ chat_search_sort.rs   фильтр/сортировка списка чатов
│  ├─ rename_chat.rs        авто-название (digest, чистка), переименование
│  ├─ chat_export.rs        format_conversation (копирование переписки)
│  ├─ rag_command.rs        парсер /rag add|remove|list|rebuild
│  ├─ rag_ingest.rs         scan, read_text, RagProgress (типы прогресса индексации)
│  ├─ backup.rs             резервное копирование/восстановление данных (zip, транзакц.)
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
   ├─ api/                  слой движка инференса (контракт + реализации по семействам, ADR 0004)
   │  ├─ contract.rs        EngineBackend, Embedder, ChatRequest/Chunk, ToolCallAccumulator (агностичный)
   │  ├─ openai/            OpenAI-протокол: client.rs (reqwest+SSE, probe /health, embed) + wire.rs (WireDialect)
   │  ├─ anthropic/         Anthropic Messages API: client.rs + wire.rs (Claude, /v1/messages)
   │  ├─ managed.rs         ServerHandle (managed-процесс llama-server), ManagedConfig, wait_until_ready
   │  ├─ thoughts.rs        потоковый парсер <think> (fallback к reasoning_content)
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
   ├─ paths.rs             расположение данных: портативно / ОС-папка / путь (location.json)
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
`Thoughts`, `TokenUsage`, `ToolCall`, `AssistantContinue`/`AssistantRewrite`
(управляющие инструменты беседы — §8), `Finished`,
`Impersonation{Started,Chunk,Finished}`, `RagProgress`, `SelfModelView`,
`SelfModelChanged` (лёгкий сигнал «модель себя изменилась» — открытый экран `F3`
перезапрашивает снимок; §9.7), `BackgroundTask{kind,active}` (тихий индикатор
фоновой рефлексии/консолидации в статус-баре), `Error`.

`TokenUsage { completion, context, context_exact }` — live-счётчик токенов: ответ
(`completion`, накопительно по раундам agentic-loop) и переписка/промпт (`context`).
Источник двойной: live-приближение по числу потоковых дельт + точное число из блока
`usage` сервера (его просим через `stream_options.include_usage=true`; приходит
финальным чанком как `ChatChunk::Usage`). До прихода `usage` переписка показывается
клиентской оценкой (`shared/tokens.rs`, эвристика «байты UTF-8 / 4»), помеченной `~`;
точное `prompt_tokens` её заменяет. Статус-бар показывает сумму одним числом.

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
        LLM-->>ORCH: Text / Thoughts / ToolCall / Usage deltas
        ORCH->>UI: Chunk / Thoughts / TokenUsage (по generation_id)
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
- **«Мысли» (CoT).** llama.cpp: `delta.reasoning_content` (`llama-server
  --reasoning-format`) → `ChatChunk::Thoughts`; запасной путь — потоковый парсер
  `<think>…</think>` (`shared/api/thoughts.rs`), склеивающий теги на границе чанка.
  **Anthropic (Claude):** `wire::build_request` шлёт `thinking:{type:"adaptive",
  display:"summarized"}` при `sampling.thinking==Some(true)` (+ `output_config.effort`
  из `reasoning_effort`); `budget_tokens`/`reasoning_budget` **не шлём** — модели 4.x
  их отвергают (`400`). `thinking_delta`→`Thoughts`, `signature_delta`→
  `ChatChunk::ThoughtsSignature`. **При tool-use** Anthropic требует возвращать
  thinking-блок **с подписью** в assistant-ходе с `tool_use` того же хода (иначе
  `400`): agentic-loop копит подпись раунда и крепит `ApiMessage.thinking`
  (`with_thinking`) к ходу с вызовами; `build_messages` ставит `AntBlock::Thinking`
  первым. Подпись живёт только в памяти хода (между ходами Anthropic авто-отбрасывает
  старые thinking → не персистится). `supported_sampling_fields(Claude)` =
  `max_tokens`+`thinking`+`reasoning_effort`.
- **EOS.** Остановка строго по token-id спец-токенов модели (на стороне сервера);
  строковые `stop` по тексту EOS приложение **не отправляет** (анти-самообрыв).
- **Регенерация** усекает историю по последнее сообщение пользователя
  включительно и перезапускает `start_generation`; **удаление последнего обмена**
  снимает последний user+assistant и возвращает текст пользователя в ввод.

---

## 6. Слой движка (`shared/api`)

Транспорт спрятан за двумя трейтами — это даёт замену движка, мульти-провайдерность
и mock в тестах. Модуль разложен по семействам ([ADR 0004](decisions/0004-engine-contract-multi-provider.md)):
**`contract`** (провайдеро-агностичные трейты и типы), **`openai`** (OpenAI-протокол:
локальный/external `llama-server`, облако OpenAI/Gemini), **`anthropic`** (Claude,
Messages API), **`managed`** (запуск дочернего `llama-server`).

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
        openai/: reqwest + SSE
        +probe() /health
        Bearer-ключ, WireDialect
    }
    class AnthropicClient {
        anthropic/: /v1/messages
        x-api-key, событийный SSE
    }
    class UnavailableEmbedder {
        RAG не настроен → ошибка
    }
    class MockBackend {
        #[cfg(test)]
    }
    EngineBackend <|.. OpenAiClient
    EngineBackend <|.. AnthropicClient
    EngineBackend <|.. MockBackend
    Embedder <|.. OpenAiClient
    Embedder <|.. UnavailableEmbedder
```

Провайдер выбирается в настройках единым селектором режима (`managed`/`external`/
`openai`/`gemini`/`claude`); API-ключ хранится **именем env-переменной** (секрет не
на диске). Поле `model` для облака обязательно — его подставляет сам бэкенд (доменный
`ChatRequest` модель не несёт). **Диалект сэмплинга** ([`openai::WireDialect`]):
`LlamaCpp` шлёт все расширения, `OpenAi`/`Gemini` чистят незнакомое (иначе `400`;
OpenAI требует `max_completion_tokens` вместо `max_tokens`), `AnthropicClient` из
сэмплинга шлёт `max_tokens` + extended thinking (`thinking`/`reasoning_effort` →
adaptive; Claude 4.x отвергает temperature/top_p/top_k и `budget_tokens` — см. «Мысли
(CoT)» выше). У Anthropic нет embeddings — `Embedder` он не реализует (RAG берёт
отдельный, ADR 0002).

- **`ChatRequest`** = `system` + `messages` (user/assistant/tool, включая
  `tool_calls` и tool-результаты) + `sampling` + `tools`. История append-only →
  локальный сервер переиспользует prefix cache. Каждый бэкенд транслирует в свой
  wire-формат (OpenAI Chat Completions либо Anthropic Messages: system → top-level,
  tool-результаты → `tool_result`-блоки в user, склейка соседних ролей).
- **`ChatChunk`** = `Text` | `Thoughts` | `ThoughtsSignature` | `ToolCall(ToolCallDelta)` |
  `Usage(TokenUsage)` | `Finished`. `ToolCallAccumulator` собирает разрезанные по
  чанкам вызовы по `index`; `Usage` (`prompt_tokens`/`completion_tokens`) приходит
  финальным чанком при `stream_options.include_usage=true` — счётчик токенов.
  `ThoughtsSignature` эмитит только Anthropic (подпись thinking-блока для переотправки
  при tool-use); прочие бэкенды его не шлют.
- **`ServerHandle`** (`managed.rs`) владеет дочерним `llama-server`: `Child` отдан
  **монитор-задаче** (`spawn_monitor`), которая `select!`-ит между его выходом (взвод
  `exited`-токена) и сигналом `kill` (взводится в `Drop` хэндла → `start_kill`;
  `kill_on_drop` оставлен подстраховкой). `build_args` собирает CLI (`-m`, `-ngl`,
  `-c`, `--jinja`, `--no-mmap`, `--flash-attn`; спекулятивное декодирование
  `--spec-type` + черновые `-md`/`-ngld`/`--spec-draft-n-max`/`-n-min`; для
  эмбеддингов — `--embeddings -ub <ctx> -b <ctx>`). Опциональные флаги добавляются
  только когда заданы (незаданное → дефолт llama.cpp); `FlashAttn`/`SpecType` —
  enum'ы в `shared/config`, в `ManagedConfig` приходят примитивами (как
  `reasoning_format`), enum→строку конвертирует супервайзер.
  **Предполёт:** если `model_path` (или черновой `-md`) задан, но файла нет — `bail!` до `spawn`.
  **Ранний выход:** если файл валиден, но процесс умирает уже *в ходе* загрузки
  (битый GGUF, OOM), монитор взводит `exited`, а `wait_until_ready(..., exited)`
  прекращает поллинг сразу с понятной ошибкой — не ждёт таймаут (иначе зависание в
  «подключение…» до `MANAGED_READY_TIMEOUT=600с`).

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
        NV["note_vectors (note_id PK)<br/>эмбеддинг заметки, косинус в Rust"]
        NL["note_links (граф связей)<br/>from/to/relation, изоляция по profile_id"]
        NSUP["note_superseded (note_id PK)<br/>«шрам» замещения, скрыт из выдачи"]
        RAGD["rag_documents (profile_id)"]
        VEC["rag_vectors vec0 (rowid)"]
        SELF["self_models (profile_id PK)"]
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
    CTX["ToolContext (снимок на начало хода)<br/>profile_id, chat_id, system_message,<br/>effective_sampling, last_user_message_at,<br/>storage: Arc&lt;Storage&gt;, engine, embedder, self_model_params"]
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
| Память/знания  | `note_save` (эмбеддит + ворота совместимости), `note_recall` (семантический поиск + spreading activation по графу, откат на подстроку; **скрывает self-заметки** `@self`), `note_revise` (правка на месте), `note_link`/`note_neighbors` (типизированный граф связей), `note_supersede`/`note_merge` (замещение со «шрамом» / слияние с переносом связей; **наследуют теги**, в т.ч. `@self`), `consolidate_notes` (обзор для консолидации), `rag_add`, `rag_search`. Связность заметок (накопление → интеграция) + авто-«сон»: см. [docs/notes-connectivity.md](notes-connectivity.md). Наблюдения «модели себя» — обычные заметки с тегом `@self` ([docs/narrative-as-notes.md](narrative-as-notes.md), §9) |
| Интроспекция   | `get_sampling`, `set_sampling`, `get_system_message`, `set_system_message`, `get_last_user_message_time` |
| Внешние        | `web_search` (мульти-провайдер + анти-бот), `fetch_url` (загрузка+саммаризация), `python_exec` (subprocess) — гейтятся `web_enabled`/`python_enabled` |
| Файлы          | `fs_read`, `fs_write`, `fs_list` — гейтятся `fs_enabled`, опциональная песочница `fs_root` |
| Утилиты        | `calculate` (свой вычислитель выражений), `current_time` (chrono) — без I/O, не гейтятся |
| Осознанность   | `call_subagent` (без истории/инструментов, запрет вложенности) |
| Управление беседой | `send_followup_message` / `rewrite_current_message` — **control-flow** (опц., по умолч. выкл): распознаются agentic-loop'ом, а не `Tool::invoke` |
| Модель себя    | `get_self_model`, `reflect`, `update_self_model`, `update_user_model`, `add_insight` — **опц., по умолч. выкл**: пер-профильная «модель себя» в SQLite (описание + цели + модель собеседника), пишут напрямую через `storage` (не через `ChatEffect`). Наблюдения («нарратив») переехали в заметки `@self` — их консолидируют note-инструменты (`consolidate_narrative` удалён). **Подробно — §9** |

Особенности реализации:

- **`call_subagent`** — независимый одно-ходовый запрос через `ctx.engine`:
  заданное `system`, единственное `user`-сообщение, `tools: []` (запрет
  рекурсии), лимит токенов и таймаут.
- **Управляющие инструменты беседы** (`send_followup_message` / `rewrite_current_message`,
  `features/tools/control.rs`) — не обычные инструменты, а **control-flow**: их
  распознаёт сам agentic-loop (`generation.rs`), а `Tool`-реализации нужны лишь для
  схемы/регистрации/гейтинга. `send_followup_message` начинает второе сообщение
  отдельным пузырём (флаг `Message.new_bubble`); `rewrite_current_message` отбрасывает
  начатый раунд в `Chat.deleted` и пишет ответ заново. Опциональны (нет в
  `default_tool_ids`, есть в каталоге `all_tool_ids` — тумблеры профиля). Live-стрим
  ↔ перезагрузка синхронизируются событиями `AppEvent::AssistantContinue`/
  `AssistantRewrite`. См. spec §9.3.3.
- **`web_search`** — фоллбэк по провайдерам (DDG lite → DDG html → Mojeek →
  Ecosia); распознаёт анти-бот троттлинг (HTTP 202/403/429) и переключает
  провайдера, а не парсит пустую выдачу.
- **RAG** — умный чанкинг с перекрытием (`chunk_text`/`chunk_markdown`, размеры
  конфигурируемы через `config.rag`/`ChunkParams`), при извлечении — склейка соседних
  чанков по дословному перекрытию (`stitch_hits`). Команды `/rag add|remove|list|
  rebuild`: загрузка/удаление файлов (фоновая индексация, отменяемая), просмотр
  источников (счётчик чанков+дата) и реиндексация. Исходный текст источников хранится
  в `rag_sources` → `/rag rebuild` перечанковывает/переэмбеддивает без файлов на диске
  (смена размеров чанка или embedding-модели; при смене размерности вектора таблица
  векторов пересоздаётся, если базу не делят другие профили). Всё с изоляцией по
  профилю.
- **Модель себя (SelfModel)** — пер-профильная «модель себя» агента (описание + цели +
  модель собеседника + нарратив) в SQLite; мутаторы пишут напрямую через `ctx.storage`
  (без `ChatEffect`), оркестратор подмешивает снимок в системный промпт. Это отдельный
  сквозной механизм — **подробно, с диаграммами, в §9** (доменная модель, цикл
  чтение/запись, семантика инструментов, протокол ведения, авто-рефлексия, направления
  развития).

---

## 9. Модель себя (SelfModel)

**«Модель себя»** — пер-профильное, персистентное представление агента о себе, своих
целях и собеседнике, живущее **между чатами**. Цель — дать модели **непрерывность и
идентичность**: чтобы во втором чате того же профиля она помнила, кто её собеседник, к
чему стремится и что о себе поняла. Механизм вырос из банка идей
([docs/self-model.md](self-model.md)) через реализуемый зонд
([docs/self-model-mvp.md](self-model-mvp.md)) и два раунда правок по живому
тестированию на Opus 4.8 (журнал — в [CLAUDE.md](../CLAUDE.md)).

Ключевая архитектурная позиция: **модель себя — пер-профильные данные в SQLite (как
notes/RAG), а не состояние `Chat`.** Поэтому инструменты-мутаторы пишут её **напрямую**
через `ctx.storage` (не через `ChatEffect`), и инвариант «единственный владелец `Chat`»
(§11) не затрагивается — новых вариантов `ChatEffect` нет. Вся группа **опциональна**
(по умолчанию выключена).

### 9.1 Доменная модель (`entities/self_model.rs`)

```mermaid
classDiagram
    class SelfModel {
        Uuid profile_id
        u64 version
        String summary
        Vec~Goal~ goals
        UserModel user_model
        Vec~NarrativeSegment~ narrative
        +render_for_prompt() компактно
        +render_full() детально
        +match_goal() резолвер
        +match_insight() резолвер
    }
    class Goal {
        Uuid id
        String description
        GoalStatus status
        Option~DateTime~ closed_at
    }
    class GoalStatus {
        <<enum>>
        Active
        Completed
        Abandoned
    }
    class UserModel {
        Vec~String~ perceived_traits
        Vec~String~ current_interests
        String relationship_dynamic
        +add_traits() merge
        +remove_traits() merge
    }
    class NarrativeSegment {
        Uuid id
        String text
        DateTime created_at
    }
    SelfModel "1" *-- "0..*" Goal
    SelfModel "1" *-- "1" UserModel
    SelfModel "1" *-- "0..*" NarrativeSegment
    Goal --> GoalStatus
```

Четыре органа, у каждого своя роль:

- **`summary`** — свободный текст «о себе»: связное самоописание (кто я, что ценю, как
  себя веду). Правится **интеграцией** (уточняется), а не перезаписью с нуля.
- **`goals`** — долгосрочные намерения с **жизненным циклом** (`Active` →
  `Completed`/`Abandoned`). Закрытые **не удаляются** — хранятся как «шрам»
  достигнутого/оставленного и показываются в полном чтении.
- **`user_model`** — устойчивая, интегрированная модель собеседника:
  `perceived_traits`/`current_interests` (списки, правятся **merge**: add/remove с
  дедупом) + `relationship_dynamic` (свободный текст; **замена непустой** динамики —
  существенный пересмотр, просит `note`-шрам, как удаление черт). Это **снимок текущих
  выводов**, не летопись настроения. Подмешивается и в **имперсонацию** (`Ctrl+U`,
  `UserModel::render_for_impersonation`): агент пишет реплику *за* человека, а это
  буквально модель того человека — тот же opt-in-гейт (`get_self_model`).
- **`narrative`** — «биография я во времени»: короткие инсайты/наблюдения (включая
  замеченные противоречия и **шрамы ревизий** — прозой). Именно наблюдения, а не
  структурные поля, хранят «что и почему менялось». **Наблюдения переехали в заметки**
  (тег `@self`, Ярус 1 [narrative-as-notes.md](narrative-as-notes.md)): поле
  `narrative` в блобе оставлено лишь для одноразового бэкфилла и реконструкции снимка
  `F3`; запись/чтение идут через заметки (эмбеддинги, ворота дублей, замещение, «сон»).

**Два режима рендера** (важное различие, выведенное живым тестом): оба принимают
наблюдения параметром `recent: &[NarrativeSegment]` (self-заметки готовит вызывающий —
`notes::self_notes_recent`; рендер перестал быть чистым по нарративу).

- `render_for_prompt(cap, n, now, recent)` — **компактный, усечённый** (по `prompt_cap`),
  только активные цели, `n` свежих наблюдений. Для **пассивной инъекции** в системный
  промпт (экономия окна контекста).
- `render_full(now, recent)` — **без усечения**: цели с коротким `#id` и статусом (+
  недавние закрытые), все наблюдения с **полным** id (наблюдения — заметки, их
  переписывает/замещает `note_revise`/`note_supersede` по полному id). Для **чтения
  инструментами** (`get_self_model`/`reflect`/эхо правок).

Оба режима показывают **метки возраста** (`age_label(at, now)`, сутки-гранулярность:
сегодня/вчера/N дн./нед./мес./г.) у целей (закрытые — от `Goal.closed_at`) и наблюдений.
FIFO-потолок нарратива исчез (наблюдения — заметки, роста блоба нет); остался потолок
закрытых целей — свёртка старейших в наблюдение-шрам «[архив цели] …»
(`fold_closed_goals` возвращает шрамы, вызывающий пишет их @self-заметками;
`SelfModelSettings.max_closed_goals`).

**Короткие `#id`** (`short_hex` = первые 6 hex UUID) и общий резолвер `resolve_handle`
(за `match_goal`/`match_insight`: принимает `#id`-префикс или полный UUID, ловит
неоднозначность) — низкое трение для модели: ей не нужно копировать 36-символьные UUID.

### 9.2 Хранение (`shared/storage/db.rs`)

Одна таблица, JSON-блоб всей модели:

```sql
CREATE TABLE IF NOT EXISTS self_models (
    profile_id TEXT PRIMARY KEY,   -- изоляция по профилю (как notes/RAG)
    data       TEXT NOT NULL,      -- serde JSON всей SelfModel
    version    INTEGER NOT NULL,   -- растёт при upsert (счётчик ведёт хранилище)
    updated_at TEXT NOT NULL
);
```

`self_model_get(profile_id)` / `self_model_upsert(&model)` (INSERT OR REPLACE,
`version`+1). **Миграций нет:** модель — JSON-блоб + `#[serde(default)]` на новых полях,
поэтому эволюция формы (новые поля/инструменты) читает старые записи без миграции —
форму хранения не меняли ни в одном ярусе.

### 9.3 Цикл «чтение ↔ запись» за ход

```mermaid
flowchart TB
    subgraph READ["Чтение — пассивно, каждый ход"]
        SNAP["start_generation:<br/>self_model_get(profile_id) → снимок в ToolContext"]
        INJ["inject_self_model:<br/>render_for_prompt (компактно) + протокол ведения"]
        SYS["system-промпт хода"]
        SNAP --> INJ --> SYS
    end
    subgraph WRITE["Запись — активно, инструментами"]
        RD["get_self_model / reflect (чтение)"]
        WR["update_self_model / update_user_model<br/>add_insight / consolidate_narrative (запись)"]
    end
    DB[("self_models (SQLite)<br/>изоляция по profile_id")]
    SYS -.->|"агент видит себя, решает вызвать"| RD
    SYS -.-> WR
    WR -->|"пишут напрямую через ctx.storage"| DB
    DB -.->|"следующий ход перечитывает"| SNAP
    DB -.->|"мутатор перечитывает свежее в ходе"| RD
```

Два независимых пути обращения:

- **Чтение — пассивное, каждый ход.** «Модель себя» **компактно** подмешивается в
  `system`-промпт через `generation::inject_self_model` — но только если профиль
  **включил** `get_self_model` (opt-in-гейт). Так агент «видит себя» на каждом ходу без
  явного вызова инструмента. Наблюдения (self-заметки) идут в промпт **по релевантности
  к последней реплике** пользователя (эмбеддинг реплики → близкие наблюдения) **плюс
  гарантия свежайшего** — старое, но относящееся к теме наблюдение всплывает, когда
  тема возвращается (Ярус 2, [narrative-as-notes.md](narrative-as-notes.md)):
  `notes::self_notes_relevant` + `blend_self_notes` → `injection_recent`; мягкая
  деградация к свежести без эмбеддера. Инъекция **делается в async-задаче генерации**
  (`spawn_generation`, а не в sync `start_generation`) — релевантность требует
  async-эмбеддинга, а command-handler синхронный.
- **Запись — активная, инструментами.** Мутаторы пишут напрямую в `self_models` под
  `ctx.profile_id`. Снимок в `ToolContext` намеренно может устареть в пределах хода (как
  у notes) — мутаторы перечитывают свежее из БД, а следующий ход берёт новый снимок.

**Запись атомарна** (`Db::self_model_update(profile_id, |m| -> bool)`): чтение-правка-
запись под **одним** захватом мьютекса соединения. Это закрывает гонку load-modify-write
между тремя параллельными писателями — инструментами хода, **фоновой авто-рефлексией**
(§9.6, работает одновременно с пользователем) и ручной правкой `F3`; иначе рефлексия
могла записать свою версию поверх только что сохранённой правки `F3`. Публичные
`get`/`upsert` делегируют приватным `*_conn`-хелперам; closure `mutate` не может звать
методы `Db` (нереентерабельный `Mutex` → дедлок) — это чистая правка значения.

**Трейд-офф prefix cache (осознанный).** Инъекция «модели себя» живёт в начале
`system`-промпта, поэтому каждое обновление модели меняет `system` следующего хода и
локальный `llama-server` пере-обрабатывает контекст с нуля. Это **принятая цена** за
возможности механизма (решение 2026-07-03): перенос блока в конец истории ради
сохранения кэша не делаем. Смягчение: метки возраста в рендере имеют **сутки-
гранулярность** (`age_label`), поэтому сам по себе ход времени не дёргает промпт чаще
раза в день — инвалидация происходит только на реальных правках модели.

### 9.4 Инструменты и их семантика

Шесть инструментов (все **опциональны**: в `all_tool_ids`, не в `default_tool_ids`;
DB-only → проходят `effective_tool_ids` через `_ => true`; тумблеры в профиле). Их
семантика — не «CRUD над полями», а **два выведенных принципа**: *интеграция вместо
накопления* (от связности заметок) и *текущий снимок + шрам в нарративе*.

| Инструмент | Роль | Ключевая семантика |
|---|---|---|
| `get_self_model` | чтение | `render_full` + блок «Связи наблюдений» (`render_self_read`) — модель целиком, цели с `#id`, наблюдения с полным id, **рёбра графа наблюдений** (Ярус 2), без усечения |
| `reflect` | чтение + рубрика | текущая модель (+ связи) + **обзор self-консолидации** (похожие пары / `contradicts` / без связей при ≥2 наблюдений) + вопросы (цели по `#id`, merge, консолидация наблюдений через note_revise/supersede, **связать соотносящиеся note_link**) |
| `update_self_model` | описание + цели | `summary` — **интеграция**; цели — жизненный цикл по `#id` (complete/abandon), не только add |
| `update_user_model` | собеседник | списки — **merge** (add_/remove_, дедуп); `note` → **шрам** @self-заметкой; напоминание при удалении черт **или замене непустой динамики** без `note`; **ворота родственных черт** (Ярус 2/C): близкая по теме черта уже есть → показать, модель решает «дубль (слить) или противоречие (add_insight)»; эмбеддинг черт на лету, порог 0.72 (калибровка bge-m3), зеркало ворот `add_insight` |
| `add_insight` | наблюдение | `create_note(@self)` + **ворота** (похожие наблюдения → перепиши через note_revise/supersede, а не плоди дубль) |

Наблюдения-заметки консолидируют **note-инструменты** (`note_revise`/`note_supersede`/
`note_merge` — замещение со «шрамом»); прежний `consolidate_narrative` удалён (Ярус 1,
[narrative-as-notes.md](narrative-as-notes.md)). **Граф над наблюдениями** (Ярус 2):
`note_link`/`note_neighbors` связывают соотносящиеся наблюдения (`contradicts`/`refines`/
`relates`); связи показываются в `get_self_model`/`reflect` (`self_related_block`).
**Кросс-органные связи** (Ярус 3, Путь 1): наблюдение «о себе» можно связать с
пользовательской заметкой «о собеседнике» — такое ребро **показывается** в чтении
«модели себя» (`[заметка]`) и в `note_recall` (`[о себе]`); авто-рефлексии даны
`note_link`/`note_neighbors` + `note_recall` (id заметок) для кросс-связывания
(`REFLECT_TOOL_IDS`). **Связь с RAG** (Ярус 3, Путь 3): `note_cite_source(note_id,
source)` привязывает заметку/наблюдение к источнику базы знаний (по **имени** источника —
стабильно к `/rag rebuild`); двунаправленно — `note_recall`/`get_self_model` показывают
«Ссылки на источники», `rag_search` — «Заметки со ссылкой на эти источники».

**Обзор self-консолидации** (Ярус 3, отложен в Ярусе 2 до подтверждения пользы
связывания — GO): `notes::build_self_consolidation_overview(storage, profile_id)` —
чистое чтение БД над **только** `@self`-наблюдениями (зеркало
`build_consolidation_overview` для пользовательских заметок): похожие пары (дубли по
косинусу ≥ `CONSOLIDATE_SIMILARITY`), связи `contradicts` среди наблюдений, наблюдения
без связей; `None` при < 2 наблюдений. Подмешивается в `reflect` (между моделью и
рубрикой) и в дайджест авто-рефлексии — «сон» памяти «о себе» получает **конкретные
данные**, а не только рубрику; `reflect_system_message` нуджит сливать похожие пары,
проверять `contradicts`, связывать несвязанные. Изоляция по `profile_id`, без миграций.

Почему именно так (уроки живого теста, см. CLAUDE.md):

- **`get_self_model` не усекает** — раньше отдавал ту же компактную инъекцию с «…», и
  модель на «…» жаловалась.
- **Цели ведутся по `#id`** — раньше id не показывались нигде, и модель физически не
  могла закрыть цель (write-once → «заполнила один раз и забыла»).
- **`update_user_model` мёржит** — раньше заменял списки целиком, и «настроение»
  перетирало накопленное («добрейший» ↔ «беспощадный»).
- **Шрам — в нарративе, а не на черте** — черты плоские (текущий снимок), а «биография
  изменения» (что/почему) живёт в нарративе; `note` при удалении/замене черты оставляет
  след, `remove_*` без `note` получает напоминание.
- **Консолидация** лечит зеркальную болезнь накопления — раздувание/дрейф: устойчивое
  поднимается в `summary`/черты, сырые дубли сворачиваются. Зеркало
  `note_merge`/`consolidate_notes`, но **без графа** (нарратив ≈ self-directed notes).

Жизненный цикл целей — «шрам» во внутренней идиоме модели себя (закрытые хранятся):

```mermaid
stateDiagram-v2
    [*] --> Active: add_goals
    Active --> Completed: complete_goals по id
    Active --> Abandoned: abandon_goals по id
    Completed --> Active: реактивация (правка F3)
    note right of Completed
        Закрытые НЕ удаляются —
        видны в render_full (жизненный цикл)
    end note
```

### 9.5 Протокол ведения и предсказуемость независимо от персоны

Наблюдение живого теста: спонтанное использование SelfModel-инструментов **зависит от
персоны** профиля (добрая-осознавшая — переиспользует и льстит; «экспериментальный ИИ» —
недоиспользует без напоминаний). Два рычага отвязывают поведение от персоны:

1. **Нейтральный к персоне «протокол ведения»** (`self_model::maintenance_protocol()`)
   подмешивается в системный промпт **поверх** персоны профиля: когда фиксировать
   изменения, «мимолётное — в наблюдения», и ключевое **«точность важнее угодливости»**
   (прямой контр-приём против лести доброй персоны). Тумблер
   `config.self_model.maintenance_protocol` (по умолчанию **вкл**); подмешивается даже
   при пустой модели (bootstrap первой записи).
2. **Фоновая авто-рефлексия** (§9.6) — детерминированный пишущий путь, не зависящий от
   того, вспомнит ли модель вызвать инструмент сама.

**Единый источник правил** (этап 6 доводки): формулировки протокола ведения и системного
сообщения авто-рефлексии собираются из одной константы `self_model::POLICY_CORE` (раньше
дублировались и уже слегка разъехались). `maintenance_protocol()` = `POLICY_CORE` в
обрамлении «ты сам ведёшь», `reflection::reflect_system_message()` = преамбула рефлексии +
`POLICY_CORE` + пояснение о поведенческих сигналах. Интерактивная рубрика инструмента
`reflect` намеренно **не** отсюда — иной жанр (вопросы, а не императив), покрывает те же
темы.

### 9.6 Авто-рефлексия (фоновый мини agentic-loop)

```mermaid
sequenceDiagram
    participant ORCH as Оркестратор (handle_done)
    participant TASK as Фоновая задача рефлексии
    participant LLM as Движок
    participant DB as self_models (SQLite)
    Note over ORCH: каждые N ответов (auto_reflect_every ≥ 1)<br/>гейты: профиль включил модель себя, сервер Ready, одна за раз
    ORCH->>TASK: spawn(digest переписки + REFLECT_SYSTEM_MESSAGE)
    loop до REFLECT_MAX_ROUNDS=6 раундов, таймаут 120с
        TASK->>LLM: chat_stream(tools = SelfModel)
        LLM-->>TASK: ToolCall(update_self_model / … / consolidate_narrative)
        TASK->>DB: инструмент пишет напрямую
    end
    TASK-->>ORCH: reflect_done (снять флаг «идёт рефлексия»)
    Note over ORCH,DB: чат/лента НЕ трогаются — рефлексия молчалива
```

`orchestrator/reflection.rs`: каждые `config.self_model.auto_reflect_every` ответов
ассистента (0 = **выкл по умолчанию** — фоновые вызовы облачного API стоят токенов)
фоновая задача просит модель **самой** пересмотреть недавний разговор (дайджест
переписки) и обновить «модель себя». В отличие от авто-названия (одноходовый запрос без
инструментов) это **мини agentic-loop**: модели даны SelfModel-инструменты
(`REFLECT_TOOL_IDS`, включая `consolidate_narrative`), и петля **исполняет** её вызовы,
пишущие в `Storage`. Чат/лента **не трогаются** — рефлексия молчалива. Гейты: профиль
включил инструменты модели себя, сервер `Ready`, одна рефлексия за раз, переписки
достаточно.

Само тело мини agentic-loop (стрим → аккумулятор вызовов → исполнение → раунд) —
**общее** для рефлексии и авто-консолидации заметок: `tool_loop::spawn_silent_loop`
(`orchestrator/tool_loop.rs`, этап 6 доводки). Раньше оно дублировалось дословно в двух
модулях; теперь оба строят `SilentLoop { backend, request, allowed, max_rounds, timeout,
label, done_tx, … }` и различаются лишь параметрами (набор инструментов, лимиты, метка
лога) и системным сообщением. Общий и предикат каденции `tool_loop::due`. Основную петлю
генерации (§5) сознательно **не** влили — у неё стриминг в UI, control-flow-инструменты,
thinking-подписи Anthropic, usage, эффекты; её сложность не окупает общий сток.

**Каденция — ватермарк, а не in-memory счётчик** (этап 3 доводки,
[refinements.md](refinements.md)). `Chat.reflected_upto: Option<usize>` — индекс-
водораздел: сколько первых сообщений уже охвачено рефлексией. `reflect_window` считает
ответы ассистента **только в окне `messages[wm..]`** (кламп ватермарка к длине истории →
устойчив к усечению `Ctrl+R`/`Ctrl+E`), и дайджест строится по тому же окну — иначе
каждый цикл перечитывал бы уже отрефлексированное и плодил дубли инсайтов. Ватермарк
(`reflected_upto`/`reflected_at`, `#[serde(default)]` → без миграции, живут с чатом →
переживают рестарт) продвигается **только при фактическом спавне** — пропуск по гейту
(сервер не готов, рефлексия уже идёт) не теряет накопленный цикл. `modified_at` при этом
не трогается (рефлексия не поднимает чат в списке). Авто-консолидация заметок (§ниже)
осталась на in-memory счётчике, но с тем же исправлением: сброс — только при спавне.

**Поведенческие сигналы собеседника в дайджест** (этап 4): `Ctrl+R` (перегенерация =
«ответ не устроил»), `Ctrl+E` (удаление обмена) и rewrite-раунды уже архивируются в
`Chat.deleted` — теперь с меткой `DeletedCause`. `behavior_markers(chat, since)` (где
`since` = прежний `reflected_at`) считает эти удаления за окно и дописывает к дайджесту
блок «Поведенческие сигналы собеседника: …» — рефлексия получает **реальное поведение**,
а не одни самоописания (перегенерация/удаление приписываются собеседнику, rewrite —
собственному поведению агента; записи без `cause` — старые — не считаются).

**Наблюдаемость фоновых задач** (этап 5): рефлексия и консолидация — молчаливые
фоновые задачи, поэтому их отказы легко не заметить (протухший облачный ключ → фича
месяцами «не работает»). Внутренний done-канал несёт **исход** (`Result<(), String>`);
оркестратор считает подряд идущие неудачи и на пороге (`BACKGROUND_FAILURE_ALERT=3`)
**один раз** эмитит `AppEvent::Error`, дальше молчит до первого успеха (сброс) —
наблюдаемость без спама; ошибки логируются на уровне `warn` с `profile_id`. Пока задача
идёт, `BackgroundTask{active}` рисует тихий индикатор в статус-баре (`✻ рефлексия`/
`✻ сон заметок`). После **успешной** рефлексии — `SelfModelChanged` (открытый `F3`
перезапросит снимок; §9.7); то же — из `handle_done`, если модель правила себя своими
инструментами в ходу (детект по `self_model::is_self_model_tool`).

### 9.7 UI: просмотр и правка (`F3`)

Экран «Модель себя» (`screens/self_model.rs`, `ActiveScreen::SelfModel`, см. §10) —
просмотр и **ручная правка** описания/целей/собеседника/нарратива. Данными владеет
оркестратор, поэтому `F3` не открывает экран сразу: `OpenSelfModel` →
`AppCommand::RequestSelfModel` → ответ `AppEvent::SelfModelView` (снимок модели активного
профиля). Правки: `SelfModelIntent::Edit` → `AppCommand::UpdateSelfModel` → оркестратор
применяет (`SelfModel::apply_edit`), сохраняет и **переэмитит** `SelfModelView` (открытый
экран обновляется на месте). Ручная правка списков заменяет их целиком — там человек (в
отличие от merge-семантики инструментов). Когда модель меняется **не** через этот экран
(фоновая рефлексия, инструменты хода), приходит лёгкий `SelfModelChanged` — runtime, если
экран `F3` открыт, шлёт `RequestSelfModel` и обновляет его свежим снимком; при закрытом
экране сигнал игнорируется (не открывает его, в отличие от `SelfModelView`). Размеры/
инъекция/протокол/авто-рефлексия — поля секции «Инструменты» настроек (`Sm*`).

### 9.8 Инварианты

- **Единственный владелец `Chat` не затронут:** мутаторы пишут пер-профильные данные
  напрямую через `storage`, `ChatEffect` не участвует.
- **Изоляция по `profile_id`** — как notes/RAG (PK таблицы).
- **Миграций нет** — JSON-блоб + `#[serde(default)]`.
- **Opt-in** — вся группа выключена по умолчанию; инъекция и авто-рефлексия гейтятся
  включённостью `get_self_model` в профиле.
- **Снимок в ходе может устаревать** — источник истины БД; мутатор перечитывает свежее,
  следующий ход берёт новый снимок.

### 9.9 Направления развития

```mermaid
flowchart LR
    Y1["Ярус 1 (зонд)<br/>сущность + 5 инструментов + инъекция<br/>summary / goals / user_model / narrative"]
    Y2["Ярус 2 (по живому тесту)<br/>render_full + #id целей + merge user_model<br/>протокол ведения + шрам ревизии + consolidate_narrative"]
    F1["Авто-консолидация по таймеру<br/>(как notes.auto_consolidate_every)"]
    F2["Унификация: narrative → notes (@self)<br/>Ярус 1 сделан; далее ↔ RAG"]
    F3["Структурные черты с жизненным циклом<br/>(при необходимости; нужна миграция)"]
    Y1 --> Y2
    Y2 --> F1
    Y2 --> F2
    Y2 -.->|отклонено сейчас| F3
```

Две философии, на которых стоит механизм и которые задают вектор:

- **Интеграция вместо накопления** (родственно
  [notes-connectivity](notes-connectivity.md)): и перезапись (теряет прошлое), и чистое
  накопление (раздувание/дрейф) — болезни; ответ — merge + шрам + консолидация.
- **Текущий снимок + биография-шрам:** структурные поля = рабочий снимок выводов;
  нарратив = биография, помнящая, что менялась.

Ближайшие развилки (осознанно отложены):

- **Авто-консолидация модели себя по таймеру** — как `notes.auto_consolidate_every`:
  детерминированный «сон», сворачивающий раздувание без явного вызова. Сейчас
  консолидация ручная / через авто-рефлексию — последняя уже получает **обзор
  self-консолидации** (`build_self_consolidation_overview`: похожие пары / `contradicts` /
  без связей), так что «сон» идёт с конкретными данными; отдельного таймера пока нет.
- **Унификация органов памяти** — нарратив был **вторым, слабым экземпляром заметок**
  (append-only, FIFO-потолок, без графа). **Ярус 1 сделан**: наблюдения переехали в
  заметки `@self` ([narrative-as-notes.md](narrative-as-notes.md)) — получили эмбеддинги,
  ворота дублей, замещение со «шрамом», «сон». **Ярус 2 сделан** (структура): инъекция
  наблюдений **по релевантности** к реплике (`injection_recent`), **граф** над
  наблюдениями (`note_link`/`note_neighbors` + блок «Связи наблюдений», граф-смоук —
  GO), **семантические ворота родственных черт** `user_model` (шаг C, порог 0.72). **Ярус
  3 сделан целиком** (Пути 1–3, все GO). **Путь 1** (кросс-органные связи): рёбра
  self-заметка ↔ пользовательская заметка **показываются** в выдаче с пометками
  `[о себе]`/`[заметка]`, а `note_recall` выводит id (адресуемость); механика графа была
  кросс-органна и раньше, Ярус 3 снял фильтры в выдаче. **Путь 2** (смешение выдачи):
  тумблер `notes.recall_includes_self` (по умолчанию выкл) показывает self-заметки в общем
  `note_recall` с пометкой `[о себе]` — на живом тесте загрязнения нет (модель по пометке
  чисто разделяет «о себе»/«о собеседнике»). **Путь 3** (RAG ↔ заметки): заметка/наблюдение
  ссылается на **источник** RAG (`note_cite_source`, по имени источника — стабильно к
  переиндексации), поиск работает через оба органа (`note_recall`/`get_self_model` →
  «Ссылки на источники»; `rag_search` → «Заметки со ссылкой на эти источники»). Органы
  остаются РАЗДЕЛЬНЫМИ по хранению — связаны явными рёбрами/ссылками. Направление
  «нарратив как заметки» **завершено**.
- **Структурные черты с жизненным циклом** (как у целей: `active`/`retired` + причина) —
  рассматривались и **отклонены** в пользу «плоские черты + шрам в нарративе»: изменение
  черты по форме — маленькая история, ей естественнее прозой. Вернуться стоит, только
  если появится потребность в машинном обходе истории черт (потребует миграции
  `Vec<String>` → `Vec<Trait>`).
- **Сознательно НЕ делаем** (наследие банка идей): `beliefs` с числовыми `strength`,
  `contradictions` с `severity`/detect/resolve — провоцируют бессмысленные числа;
  противоречия живут прозой в нарративе.

---

## 10. UI: экраны, виджеты, рендеринг

`runtime.rs` держит один базовый `ChatScreen` (лента/генерация/ввод) и enum
`ActiveScreen { Chat | ChatList | Settings | SelfModel }` — экран, открытый поверх
чата. Открытый список/настройки/просмотр получают ввод и рисуются вместо чата;
событие `OpenChatList`/`OpenSettings` от чата создаёт их, `Close` (Esc) — возвращает
к `Chat`. Список чатов держит снимок актуальным через `AppEvent::ChatList` (его
`app` применяет и к чату, и к открытому списку). **«Модель себя»** (`F3`,
просмотр+правка) — данными владеет оркестратор, поэтому `OpenSelfModel` не открывает
экран сразу, а шлёт `AppCommand::RequestSelfModel`; экран создаётся по ответному
событию `AppEvent::SelfModelView` (снимок модели активного профиля). Правки экран
отдаёт `SelfModelIntent::Edit` → `AppCommand::UpdateSelfModel`; оркестратор применяет,
сохраняет и **переэмитит** `SelfModelView` — открытый экран обновляется на месте
(`set_model`, выделение сохраняется). См. [docs/self-model-mvp.md](self-model-mvp.md).

```mermaid
flowchart TB
    RT["runtime.rs (петля)<br/>ChatScreen + enum ActiveScreen"]
    subgraph SC["screens (отдают Intent, не AppCommand)"]
        CHAT["ChatScreen<br/>handle_key→ChatIntent, мутаторы-проекция AppEvent"]
        CLS["ChatListScreen<br/>обёртка chat_list → ChatListIntent"]
        SET["SettingsScreen<br/>секции/поля → SettingsIntent"]
    end
    subgraph WG["widgets"]
        FEED["message_feed"]
        INP["input_box"]
        CL["chat_list (виджет списка)"]
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
    RT --> CLS
    RT --> SET
    CHAT --> FEED & INP & SB & PL & IP
    CLS --> CL
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

## 11. Конкурентность и владение

```mermaid
flowchart TB
    subgraph MAIN["Главный поток"]
        L["render loop (синхронный)"]
    end
    subgraph RUNTIME["tokio runtime"]
        O["задача оркестратора (1 шт.)"]
        G["задача генерации (на время хода)"]
        T["фоновые задачи: авто-название, имперсонация, RAG-индексация, авто-рефлексия, авто-консолидация заметок"]
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

## 12. Точка входа, конфигурация, сборка

- **`main.rs`** — тонкая: грузит `AppConfig`, сеет его env-переменными
  (`apply_env_overrides` — dev-workflow), проверяет single-instance, инициализирует
  логирование (в файл) и `tokio`, поднимает `ratatui`, запускает оркестратор и
  петлю. CLI на `clap` (подкоманды выполняются без TUI и выходят): `import-lamellama
  <dir>` (импортёр), `backup [-o FILE] [-c 0..9]` и `restore <archive>` (резервное
  копирование/восстановление, `features/backup.rs`; берут single-instance, чтобы не
  конкурировать с работающим приложением за `data.db`).
- **Конфигурация** (`shared/config.rs`): `AppConfig` с секциями `EngineSettings`,
  `EmbedSettings`, `ToolSettings`, `InterfaceSettings`, `ImpersonationEngineSettings`,
  `impersonation_sampling`, глобальный семплинг. Все поля под `#[serde(default)]` —
  старые `settings.json` читаются без миграции. Выбор бэкенда — через настройки или
  env (`MINDFORK_ENGINE_URL` external / `MINDFORK_LLAMA_BIN` + `MINDFORK_MODEL`…
  managed; аналогично `MINDFORK_EMBED_*`).
  - **Конфиг движка вложенный по режимам/провайдерам:** `EngineSettings`/
    `ImpersonationEngineSettings`/`EmbedSettings` несут `mode` + под-секции
    `managed: ManagedSettings`, `external: ExternalSettings` и по `CloudSettings` на
    каждого облачного провайдера (`openai`/`gemini`/`claude`; у эмбеддингов managed —
    `ManagedEmbedSettings`). У каждого режима свои поля, поэтому переключение режима/
    провайдера не теряет чужих значений. Аксессоры `cloud()`/`cloud_mut()` отдают
    активную облачную под-структуру по `mode`. Экран настроек показывает поля только
    выбранного режима и скрывает неподдержанные облаком параметры сэмплинга (фильтр
    `cloud_supported_param`; значения сохраняются для локальных моделей). Шапка чата
    (`screens/chat::model_meta`) показывает модель активного режима. См. ADR 0004.
    `ManagedSettings` дополнительно несёт `flash_attn: FlashAttn` (`--flash-attn`) и
    поля спекулятивного декодирования (`spec_type: SpecType` + `draft_model`/
    `draft_gpu_layers`/`draft_n_max`/`draft_n_min`); черновые поля в UI видны только
    для типов `draft-*` — для MTP-моделей (`mtp-gemma-…`) это `draft-mtp`.
- **Расположение данных** (`shared/paths.rs`): по умолчанию **портативно** — в
  подкаталоге `data/` рядом с бинарником (в dev — `target/debug/data/`; подкаталог
  отделяет данные от служебных файлов/кэшей сборки): `settings.json`,
  `profiles.json`, `chats/`, `data.db`, `personal_dictionary.txt`, `dictionaries/`,
  `backups/`, `logs/`. Файл-маркер `location.json` рядом с бинарником (всегда вне
  `data/`; `DataLocation`: `portable`/`system`/`path`) переключает корень в
  стандартную ОС-папку (крейт `directories`) или произвольный каталог; нет маркера →
  портативный режим. **Резервное копирование** (`features/backup.rs`): zip с
  настраиваемым сжатием (chats/dictionaries/data.db/profiles/settings/personal +
  `*.bak` + `fs_root`, если внутри корня); восстановление транзакционно (валидация →
  pre-restore копия в `backups/` → очистка → распаковка → откат при сбое).
- **Зависимости** (см. [Cargo.toml](../Cargo.toml)): `ratatui` 0.30 + `crossterm`,
  `tokio`, `reqwest` (rustls), `rusqlite` (bundled) + `sqlite-vec`,
  `pulldown-cmark` + `syntect` + `ansi-to-tui`, `spellbook`, `arboard`, `scraper`,
  `serde`/`serde_json`, `uuid`, `chrono`, `tracing`, `clap` (CLI), `zip` +
  `directories` (резервное копирование и режим хранения). Приложение **не зависит от
  ML-стека** — это ключевое упрощение сборки.
- **Релиз**: `[profile.release]` с LTO/strip/`opt-level=3`, `panic=unwind`.

---

## 13. Тестируемость

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
