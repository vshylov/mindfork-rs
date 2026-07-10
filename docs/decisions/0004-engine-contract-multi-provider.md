# ADR 0004 — Контракт движка и мульти-провайдерный инференс (без крейт-сплита)

**Статус:** принято (2026-06-26). Определяет границы `shared/api` перед добавлением
облачных провайдеров инференса (Claude/Anthropic, OpenAI, Gemini). Развивает
[ADR 0002](0002-embeddings-dedicated-server.md) (разделение `EngineBackend`/`Embedder`).

**Контекст:** появилась задача — инференс не только через локальный
OpenAI-совместимый сервер (llama.cpp `llama-server`), но и через облачные API
лидеров: **platform.claude.com** (Anthropic), **platform.openai.com** (OpenAI) и
**aistudio.google.com** (Gemini). Нативная поддержка Gemini пока **не требуется** —
Gemini берётся через его OpenAI-совместимый endpoint
(`generativelanguage.googleapis.com/v1beta/openai/`), то есть тем же путём, что и
OpenAI.

Анализ показал: ключевой шов — трейт `EngineBackend` (`shared/api/backend.rs`) —
уже изолирует транспорт, и всё, что выше него (оркестратор, клиентский
agentic-loop, инструменты, UI, трёхуровневый семплинг), **провайдеро-независимо**.
Менять эти слои не нужно. Но сам слой `shared/api` неоднороден и протекает: в нём
смешаны generic-контракт, llama.cpp-специфика клиента и управление дочерним
процессом.

Возник второй вопрос: не вынести ли движок в отдельные крейты ради изоляции.

## Решение

### 1. Крейты не плодим

`mindfork-rs` остаётся **единым бинарным крейтом**. Крейт-сплит — это *упаковка*,
а не изоляция; он окупается лишь при конкретном триггере (внешний потребитель,
публикация библиотеки, время сборки, принудительная проверка границ компилятором),
ни одного из которых сейчас нет. Сплит дал бы workspace, лишние `Cargo.toml`,
раздувание `pub`-поверхности (то, что было `pub(crate)`, пришлось бы открыть —
местами это *ослабляет* инкапсуляцию) и координацию версий — за почти нулевую
отдачу. Границы FSD держатся дисциплиной модулей, как и прежде.

**Изоляция и крейт-сплит — разные задачи.** Делаем первое; второе остаётся дешёвой
обратимой опцией на будущее: если правильно прочертить границы модулями и трейтом
сейчас, выделение крейтов (когда/если появится триггер) станет почти механическим.

### 2. Изоляцию `shared/api` улучшаем — вместе с фичей, не отдельно

Перед добавлением провайдеров `shared/api` переразлагается **по природе
ответственности**, а не «всё в кучу». Целевая раскладка:

```
shared/api/
├─ contract/      трейты EngineBackend/Embedder + generic ChatRequest/ChatChunk/ApiMessage/ToolSchema
├─ openai/        wire + client семейства OpenAI (сам OpenAI, Gemini-compat, llama.cpp llama-server)
├─ anthropic/     AnthropicClient + его wire (/v1/messages)
├─ managed/       server.rs — запуск дочернего llama-server (это НЕ «клиент»)
└─ thoughts.rs    общий потоковый парсер «мыслей»
```

Это те же шаги, что нужны для мульти-провайдера, поэтому изоляция приходит
**вместе с фичей**, а не как отдельный рефактор «ради красоты».

### 3. Развязываем семплинг от generic-контракта

Главная протечка: `ChatRequest` несёт `entities::sampling::SamplingConfig`, полный
llama.cpp-расширений (`dynatemp_*`, `dry_*`, `top_n_sigma`, `mirostat`, `samplers`,
…). Для OpenAI это прямой источник `400` (строгий сервер отвергает незнакомые поля),
для Anthropic — мёртвый груз.

Решение: **каждый клиент сам отвечает за свой wire** и фильтрует, что он умеет
слать. Generic-контракт продолжает нести `SamplingConfig` как **намерение
пользователя**, но клиент трактует его как «лучшее усилие»: OpenAI-клиент шлёт
общеподдержанное (`temperature`/`top_p`/`top_k`/`max_tokens`/`seed`/penalties) и
расширения только для llama.cpp-цели; Anthropic-клиент — лишь `temperature`/
`top_p`/`top_k`, остальное игнорирует. Так UI и `set_sampling` не меняются, а
несовместимость не доходит до сети. (Альтернатива — выделить «extension bag» для
движко-специфичных полей — оставлена на потом, если фильтрации окажется мало.)

### 4. Сквозные пробелы, общие для всех облаков (Фаза 0)

Не зависят от выбора провайдера, делаются один раз:

- **Поле `model` в запросе.** Сейчас `wire.rs` его не шлёт (`llama-server` берёт
  загруженную модель). Все облака требуют имя модели в теле — нужно протянуть
  `model_name` из конфига в `ChatRequest`.
- **Аутентификация.** `OpenAiClient` не умеет ключей. Добавляем API-ключ (Bearer
  для OpenAI/Gemini-compat, `x-api-key` + `anthropic-version` для Anthropic).
- **Секреты не на диск.** Ключ читается из **env-переменной**, не пишется в
  `settings.json` (там он лёг бы открытым текстом). Конфиг хранит лишь *имя*
  env-переменной / признак «ключ из окружения».
- **Понятие провайдера.** `ServerMode { Managed | External }` дополняется облачным
  вариантом (External + ключ + протокол), либо вводится поле `protocol`/`provider`
  рядом с `url`. `probe()` по `/health` для облаков вернёт `404` → код уже трактует
  это как «жив, готов» (`client.rs`), так что readiness-гейт срабатывает без правок.

### 5. Эмбеддинги для RAG при облачном движке

Трейт `Embedder` уже отделён ([ADR 0002](0002-embeddings-dedicated-server.md)),
поэтому источник эмбеддингов выбирается независимо от chat-движка. Важно:
**у Anthropic нет embeddings API** — при движке-Anthropic RAG использует отдельный
эмбеддер (локальный `llama-server --embeddings`, OpenAI `/v1/embeddings` или
`UnavailableEmbedder`). Механизм уже есть, доп. работы по архитектуре не требует.

## План реализации

1. **Фаза 0 (фундамент):** поле `model` сквозь контракт; auth + ключ из env;
   провайдер в конфиге; провайдеро-зависимая фильтрация семплинга; раскладка
   `shared/api` по подмодулям (п. 2). Разблокирует OpenAI и Gemini-compat.
2. **Фаза 1:** OpenAI + Gemini (через OpenAI-compat endpoint) — преимущественно
   конфиг/UI-проводка и тесты (клиент уже говорит на Chat Completions).
3. **Фаза 2:** `AnthropicClient` + `anthropic` wire как отдельная реализация
   `EngineBackend` (`/v1/messages`: `tool_use`/`tool_result`-блоки, событийный SSE
   `content_block_delta`/`thinking_delta`, `max_tokens` обязателен). Сверять модели/
   поля через skill `claude-api`.
4. **Нативный Gemini — вне объёма** (берётся через OpenAI-compat).

### Статус реализации

- **Фаза 0** — сделана: поле `model` (инъектит бэкенд), Bearer-ключ из env, `WireDialect`
  с провайдеро-зависимой фильтрацией, провайдер в конфиге, mode-driven UI.
- **Фаза 1** — сделана вместе с Фазой 0: OpenAI и Gemini (OpenAI-compat) работают вживую.
- **Фаза 2** — сделана: `AnthropicClient` + `anthropic`-wire в модуле
  `shared/api/anthropic/` (реализация `EngineBackend`); `Claude` проведён через конфиг/
  супервайзер/настройки.
- **CoT (extended thinking) для Claude** — сделано поверх Фазы 2: `wire::build_request`
  шлёт `thinking:{type:"adaptive", display:"summarized"}` (+ `output_config.effort`) при
  включённом `thinking`; `budget_tokens`/`reasoning_budget` не шлём (модели 4.x их
  отвергают). Парс `signature_delta` → `ChatChunk::ThoughtsSignature`. **При tool-use**
  Anthropic требует возвращать thinking-блок с подписью в assistant-ходе того же хода —
  agentic-loop крепит `ApiMessage.thinking` (текст+подпись) к ходу с вызовами,
  `build_messages` ставит `AntBlock::Thinking` первым. Подпись только в памяти хода
  (между ходами авто-отбрасывается сервером — не персистится). `supported_sampling_
  fields(Claude)` расширен на `thinking`/`reasoning_effort`. Проверено живыми
  `#[ignore]`-смоуками против Anthropic API (Phase A: «мысли»+подпись; Phase B:
  round-trip подписи с tool-use без `400`). Известный задел — `redacted_thinking`.
- **OpenAI Responses API — сделано** (2026-07-10, docs/research/openai-responses-client.md):
  режим `openai` переведён с Chat Completions на Responses (`POST /v1/responses`) — новая
  реализация `EngineBackend` (`shared/api/openai/responses/`, `ResponsesClient`). Даёт
  резюме рассуждений (`reasoning.summary:"auto"` → `ChatChunk::Thoughts`), глубину
  (`reasoning.effort`, `ReasoningEffort` расширен `Minimal`/`XHigh`), многословность
  (`text.verbosity`, новое поле `SamplingConfig.verbosity`). Tool-use round-trip —
  reasoning-элемент (`id`+`encrypted_content`) переотправляется перед своим
  `function_call` (аналог подписи thinking Anthropic): `ThoughtsSignature(String)` →
  `ThoughtsSignature(ThinkingRef{id,signature})`, `ThinkingBlock.id`. `store:false`,
  `include:["reasoning.encrypted_content"]`, `strict:false`. Диалект `WireDialect::OpenAi`
  удалён (Gemini остаётся на Chat Completions). `supported_sampling_fields(OpenAi)` =
  `max_tokens`+reasoning+verbosity. «Прокси с ключом» (прежний `openai`+url-override)
  закрыт `ExternalSettings.api_key_env`. Nativ Gemini через Responses — задел.
- **Раскладка `shared/api` (§2) — выполнена** (после Фазы 2, ради симметрии с
  `anthropic/`): `backend.rs` → `contract.rs` (провайдеро-агностичный контракт);
  `client.rs`+`wire.rs` → `openai/` (с приватным `wire`, re-export `OpenAiClient`/
  `WireDialect`); `server.rs` → `managed.rs`; `anthropic/` уже был. Публичная
  поверхность не изменилась (re-export из `shared/api/mod.rs`); внешние ссылки
  `shared::api::backend::*` → `::contract::*`. Перемещения через `git mv` (история
  сохранена). Тесты/clippy/fmt зелёные.

## UX настроек (экран `screens/settings.rs`)

В настройках уже три «движковых» конфига со своими селекторами режима: чат-движок
ассистента и движок имперсонации (секция «Модель/сервер», подсекции Ассистент/
Имперсонация) и эмбеддинги (секция «Инструменты»). Добавление облаков **не должно**
плодить поля — наоборот, экран де-загромождается.

### Видимость полей по режиму (ключевой приём)

Сейчас `model_fields` показывает **все** поля независимо от режима (в External видны
бесполезные `Бинарник`/`-ngl`/`--jinja`/…). Переходим на **mode-driven visibility**:
режим — всегда первое поле, ниже — только релевантные ему. Тогда облачный режим — это
2–3 поля против 8 у managed, и каждый режим выглядит проще, хотя возможностей больше.

| Режим | Видимые поля |
|---|---|
| Managed (локальный `llama-server`) | binary, model (`-m`), ngl, ctx, jinja, no_mmap, host, port |
| External (свой OpenAI-URL) | url, model (опц.), api-ключ (имя env-переменной, опц. — для прокси/шлюза с авторизацией) |
| OpenAI / Claude / Gemini (облако) | model, api-ключ (имя env-переменной), base URL (опц., переопределение) |
| Shared (только имперсонация) | — (переиспользует движок ассистента) |

### Таксономия режима — плоская

`Mode` расширяется провайдерами как равноправными вариантами одного `←/→`-цикла:
`Managed | External | OpenAI | Claude | Gemini` (у имперсонации добавляется `Shared`).
Принято плоско (а не «Облако» + под-выбор провайдера): один селектор, нет
вложенности, единый мысленный вопрос «откуда инференс». Минус — цикл из 5–6 пунктов —
снимается mode-driven visibility (поля под каждый режим короткие) и подсказкой
`field_description` под селектором.

### Секреты — имя env-переменной, не ключ

Облачные режимы показывают поле «API-ключ» как **имя env-переменной** (напр.
`ANTHROPIC_API_KEY`), а не сам секрет. В `settings.json` пишется только имя; ключ
читается из окружения при старте/запросе (см. п. 4). Если переменная не задана —
предупреждение под секцией (тот же механизм, что подсказки полей).

### Эмбеддинги и имперсонация

- **Эмбеддинги**: облачные есть только у OpenAI/Gemini (у Anthropic нет). Чтобы не
  вводить отдельный «провайдер» для RAG, облачные эмбеддинги уложены в расширенный
  External: `url + api-ключ(env) + model`. Новый селектор не добавляется.
- **Имперсонация**: `Shared` работает с облаком без изменений (переиспускает движок
  ассистента, доп. полей нет); прочие режимы — те же, что у ассистента.

### Форма конфига (Фаза 0)

`EngineSettings`/`ImpersonationEngineSettings` получают поля под облако:
`model_name: Option<String>`, `api_key_env: Option<String>`, опц. `base_url`-override;
`ServerMode` дополняется облачными вариантами. Всё под `#[serde(default)]` — старые
`settings.json` читаются без миграции (инвариант проекта).

## Последствия

- **Плюс:** мульти-провайдер достигается добавлением реализаций трейта; слои выше
  `EngineBackend` не трогаются. Изоляция `shared/api` улучшается естественно, под
  фичу. Крейт-сплит остаётся дешёвой опцией.
- **Плюс:** развязка семплинга устраняет класс ошибок `400` на строгих серверах.
- **Минус:** generic `ChatRequest` по-прежнему формально знает про `SamplingConfig`
  (полная развязка через extension bag отложена) — приемлемо, т.к. трактовка
  «лучшее усилие» локализована в клиентах.
- **Минус:** Anthropic требует отдельного wire-слоя (другой протокол) — это
  осознанная средняя по объёму работа, изолированная за трейтом.
- **Риск:** часть UI-полей семплинга бессмысленна для облачных моделей; их стоит
  скрывать под выбранный провайдер либо молча не слать (решается в UI Фазы 1/2).
