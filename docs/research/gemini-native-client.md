# Исследование: полноценный клиент режима `gemini` (нативный generateContent)

**Статус:** исследование (2026-07-10). Аналог
[docs/research/openai-responses-client.md](openai-responses-client.md) для Gemini.
Развивает [ADR 0004](../decisions/0004-engine-contract-multi-provider.md) (в котором
нативный Gemini был явно помечен «вне объёма — берётся через OpenAI-compat»). Вывод:
нативный клиент **реализуем и укладывается за трейт `EngineBackend`**, как Anthropic и
OpenAI Responses, но у Gemini есть один структурный нюанс (**подписи мыслей — per-part и
обязательны для Gemini 3 при tool-use**), которого нет у других провайдеров; он решает,
персистить ли подпись в доменном `Message`.

## 1. Проблема

Режим `ServerMode::Gemini` сегодня ходит в **OpenAI-совместимый Chat Completions**
(`…/v1beta/openai/chat/completions`) тем же `OpenAiClient` с `WireDialect::Gemini`.
Диалект строгий (`restrict_to_strict`) — **вычищает** reasoning-сигналы (`thinking`,
`reasoning_effort`, `reasoning_budget`) и расширения llama.cpp, поэтому:

- «мыслей» (CoT) в режиме `gemini` нет — `ChatChunk::Thoughts` не приходит;
- `reasoning_effort` не отправляется — глубиной рассуждения управлять нельзя;
- `supported_sampling_fields(Gemini)` = `temperature`/`top_p`/`frequency_penalty`/
  `presence_penalty`/`seed`/`max_tokens` (даже `top_k`, который Gemini **принимает
  нативно**, вырезан диалектом — это ограничение compat-протокола, не Gemini).

У Claude (Фаза 2) и OpenAI (Responses) «мысли» и `reasoning_effort` есть — асимметрия
видна пользователю прямо в секции «Семплинг».

## 2. Что выяснено про API (сверено по docs, июль 2026)

### 2.1 У Gemini ДВА нативных API

- **`generateContent` / `streamGenerateContent`** — зрелый, стабильный, хорошо
  документированный REST (`…/v1beta/models/{model}:streamGenerateContent`). Это аналог
  Chat Completions по «поколению»: `contents[]` + `generationConfig`. **Рекомендуемая
  цель.**
- **Interactions API** — новый (аналог OpenAI Responses): `generation_config.
  thinking_level`/`thinking_summaries`, «шаги мыслей» с `signature`+`summary`. Свежий,
  меньше документирован, есть отдельный гайд миграции. **Пока не берём** — незрелость
  ради того же результата.

Берём `generateContent` (мысли через `thinkingConfig.includeThoughts`, стриминг через
`:streamGenerateContent?alt=sse`).

### 2.2 Reasoning живёт в `generationConfig.thinkingConfig`

- **Резюме мыслей** (не сырой CoT): `thinkingConfig.includeThoughts: true` → в ответе
  появляются части `{"text": "...", "thought": true}` (при стриминге — инкрементально).
  Это ровно наш `ChatChunk::Thoughts`.
- **Глубина** зависит от поколения модели (ловушка, см. §3):
  - **Gemini 3.x** — `thinkingConfig.thinkingLevel`: `minimal`|`low`|`medium`|`high`;
  - **Gemini 2.5** — `thinkingConfig.thinkingBudget` (токены): `0` выключает (кроме
    2.5 Pro — минимум 128, выключить нельзя), `-1` = динамически, иначе диапазон
    (2.5 Flash `0–24576`, Pro `128–32768`).
- `reasoning_effort` как отдельного поля в **нативном** API нет — есть `thinkingLevel`/
  `thinkingBudget`. Наш `ReasoningEffort` (`none/minimal/low/medium/high/xhigh`) мапится
  на `thinkingLevel` (3.x) или `thinkingBudget` (2.5).
- **Токены мыслей**: `usageMetadata.thoughtsTokenCount` (уже входят в биллинг вывода) —
  ложатся в наш `TokenUsage.reasoning_tokens`.

### 2.3 Подписи мыслей — per-part и обязательны для Gemini 3 (ключевой нюанс)

Это то, что отличает Gemini от Anthropic (одна подпись на ход) и OpenAI (один
reasoning-элемент на ход):

- Подпись `thoughtSignature` — **зашифрованный опаковый токен, привязанный к
  конкретной части** (`functionCall` или `text`), а не к ходу целиком.
- **Gemini 3: обязательна.** Если в `contents` встречается `functionCall`-часть без
  `thoughtSignature`, API возвращает `400`: *«Function call … in the N content block is
  missing a thought_signature»*. Gemini 2.5 — опционально (без 400).
- **Параллельные вызовы**: подпись только у **первой** `functionCall`-части хода.
  **Последовательные** (в разных раундах хода): у каждой своя.
- Правило: получил подпись — верни её **в той же части** при отправке истории.

```jsonc
// ответ модели: подпись — сосед functionCall
{ "functionCall": { "name": "calc", "args": {"x": 1} },
  "thoughtSignature": "<opaque>" }
```

**Почему это критично для нас.** `message_to_api` (`orchestrator/request.rs`)
пересобирает историю из персистентного `Chat` **на каждой генерации**: прошлые
assistant-ходы с tool-вызовами (`ApiMessage::assistant_tool_calls`) отправляются заново.
У Anthropic это безопасно (сервер сам отбрасывает старые thinking и не требует подписи на
исторических `tool_use`); у **Gemini 3** исторические `functionCall` без подписи → `400`.
Значит для Gemini 3 подпись, скорее всего, **надо персистить** в доменном `Message`
(в отличие от Anthropic/OpenAI, где она живёт только в памяти хода). Это единственная
правка вне слоя движка, ради которой Gemini сложнее двух предыдущих провайдеров.

### 2.4 Формат протокола (нативный generateContent)

| | Chat Completions (наш `OpenAiClient`) | Gemini generateContent |
|---|---|---|
| Endpoint | `/v1/chat/completions` | `…/v1beta/models/{model}:streamGenerateContent?alt=sse` |
| Auth | `Authorization: Bearer` | `x-goog-api-key: <key>` |
| Системное | `messages[0].role="system"` | top-level `systemInstruction:{parts:[{text}]}` |
| История | `messages[]` (роли system/user/assistant/tool) | `contents:[{role:"user"|"model", parts:[…]}]` — **только `user`/`model`** |
| Результат инструмента | `role:"tool"`, `tool_call_id` | часть `{functionResponse:{name, response:{…}}}` в **`role:"user"`**-контенте |
| Вызов инструмента | `assistant.tool_calls[]` (`id`+`arguments`-строка) | часть `{functionCall:{name, args:{…}}}` (+ сосед `thoughtSignature`); **args — объект**, `id` **нет** |
| Схема инструмента | `{type:"function", function:{name,description,parameters}}` | `tools:[{functionDeclarations:[{name,description,parameters}]}]` (OpenAPI-подмножество) |
| Лимит токенов | `max_tokens` | `generationConfig.maxOutputTokens` |
| Reasoning | (нет в compat) | `generationConfig.thinkingConfig.{thinkingLevel\|thinkingBudget, includeThoughts}` |
| Причина остановки | `finish_reason` | `candidates[].finishReason` (`STOP`/`MAX_TOKENS`/`SAFETY`/…) |
| Usage | `prompt_tokens`/`completion_tokens` | `usageMetadata.{promptTokenCount, candidatesTokenCount, thoughtsTokenCount, totalTokenCount}` |

**SSE**: `:streamGenerateContent?alt=sse` даёт строки `data: {…}`, где каждый объект —
частичный `GenerateContentResponse` (`candidates[0].content.parts[]` с дельтами). Текст
и части-мысли (`thought:true`) стримятся инкрементально; `functionCall`-часть обычно
приходит целиком, `thoughtSignature` — на ней. Причину завершения выводит клиент по
`finishReason` + наличию `functionCall`-частей.

**Нет `id`/`call_id` у вызовов** — сопоставление `functionCall`↔`functionResponse` идёт
**по имени и порядку** (в отличие от OpenAI/Anthropic с `tool_call_id`). Наши
`ApiToolCall.id`/`ApiMessage.tool_call_id` для Gemini-wire не нужны; парность держится
позиционно. Для agentic-loop это прозрачно (мы сами храним `id`), но wire его не шлёт.

## 3. Ловушки

1. **`thinkingLevel` (3.x) ≠ `thinkingBudget` (2.5).** Клиент не знает поколение модели
   без конфига. Варианты: (а) инференс по имени модели (`gemini-3*`→level,
   `gemini-2.5*`→budget); (б) слать `thinkingLevel` по умолчанию (целясь во флагман
   3.x) + документировать; (в) поле выбора в конфиге. Целясь в актуальный флагман,
   разумный дефолт — `thinkingLevel`; на 2.5 при промахе — понятная ошибка API.
2. **Полностью выключить мысли нельзя на 3.x и 2.5 Pro.** `reasoning_budget==0`
   (импперсонация/авто-название) на Gemini 3 → максимум `thinkingLevel:"minimal"`, на
   2.5 Pro → минимум 128 токенов. Как и у OpenAI (`effort:none` не у всех) — просто
   **игнорируем текст мыслей** downstream (импперсонация/title уже так делают).
3. **`maxOutputTokens` включает токены мыслей** — тот же класс бага, что «мысли съели
   бюджет». Нужен щедрый дефолт или предупреждение; `Finished(Length)` по
   `finishReason:"MAX_TOKENS"`.
4. **Схема инструмента — OpenAPI-подмножество, Gemini придирчив.** Он не принимает
   часть JSON-Schema (`$schema`, произвольные `additionalProperties`, некоторые
   форматы). Наши схемы генерятся под OpenAI — вероятна **санитизация** (снять
   `$schema`/неподдержанное) в `gemini/wire.rs`. Требует проверки на живых схемах
   инструментов проекта.
5. **`role:"model"`, не `"assistant"`; нет ролей `system`/`tool`.** system → top-level;
   `tool`-результат → `functionResponse`-часть внутри `role:"user"`. Наш `ApiRole`
   маппится в wire, как у Anthropic (там роль `tool` тоже сворачивается в user).
6. **Подписи (см. §2.3).** Без персиста — риск `400` на Gemini 3 при реплее истории с
   tool-вызовами. Решение — персистить подпись per-tool-call (см. §4.2).
7. **Эмбеддинги** остаются на OpenAI-compat (`OpenAiClient`, `…/v1beta/openai/
   embeddings`) — как OpenAI оставил `/v1/embeddings`. `cloud_embed_setup` не трогаем.
   Нюанс: base URL для нативного чата (`…/v1beta`) и compat-эмбеддингов
   (`…/v1beta/openai`) **разные** — развести в супервайзере/конфиге (см. §4.4).
8. **Base URL.** `CloudProvider::Gemini.base_url()` сейчас `…/v1beta/openai` (для
   compat). Нативный `GeminiClient` берёт `…/v1beta` и строит путь
   `/models/{model}:streamGenerateContent`.

## 4. Архитектура: куда это ложится

Нативный Gemini — **другой протокол**, значит новая реализация `EngineBackend`, ровно
как `AnthropicClient` и `ResponsesClient`. Слои выше движка (оркестратор, agentic-loop,
инструменты, UI) не трогаются — **фича помещается за трейт** (главный вывод, как и в
исследовании OpenAI).

Раскладка: `shared/api/gemini/{client.rs, wire.rs}` (рядом с `openai/`, `anthropic/`).
`shared/api/mod.rs` реэкспортит `GeminiClient`.

### 4.1 Развилка: нативный клиент (A) или расширить compat-диалект (B)?

| Вариант | Плюсы | Минусы |
|---|---|---|
| **A. Нативный `GeminiClient` (generateContent)** | «Полноценность»: мысли, `reasoning_effort`, `top_k` назад, `thoughtsTokenCount`, подписи «как надо»; стабильный документированный протокол; симметрия с Claude/Responses | Отдельный wire-слой (роли/parts/подписи/санитизация схем); персист подписи для Gemini 3 |
| B. Расширить `WireDialect::Gemini` (compat) | Дёшево: не чистить reasoning, слать `reasoning_effort` + `extra_body.google.thinking_config` | Compat **в бете**, недодокументирован по возврату резюме/подписей; **нестабилен** (Gemini 3 Preview отвергает `reasoning_effort:"medium"`); подписи через compat мутны → риск `400` на tool-use с Gemini 3. Инструменты у нас центральны — риск неприемлем |

**Рекомендация — A** (как и с OpenAI: полноценный клиент за трейт, а не хрупкая надстройка
над compat). B годился бы как быстрый временный зонд «мыслей без tool-use», но не как цель.

### 4.2 Правки контракта (главное отличие от OpenAI/Anthropic)

Подпись у Gemini — **per-tool-call**, а не одна на ход, поэтому существующий
`ThinkingRef`/`ThinkingBlock` (один на ход) **не подходит**. Естественнее:

- **`ApiToolCall.thought_signature: Option<String>`** и **`ToolCallDelta.
  thought_signature`** — Gemini-клиент проставляет при разборе `functionCall`-части;
  прочие бэкенды оставляют `None` (как `thinking`). `ToolCallAccumulator` копит подпись
  вместе с вызовом.
- **`ToolCallRecord.thought_signature: Option<String>`** (доменный `entities/message.rs`,
  `#[serde(default, skip_serializing_if=Option::is_none)]` → без миграции) — **персист**
  ради реплея истории на Gemini 3. Подпись опаковая/зашифрованная — хранить безопасно.
  `record_to_api`/обратный маппинг протягивают её. Прочие провайдеры поле не читают.
- Существующий `ThinkingBlock`/`ThoughtsSignature(ThinkingRef)` для Gemini **не нужен**
  (мысли-как-текст стримятся частями `thought:true`; подпись едет на вызове). Трогать его
  не надо — Gemini его просто не использует.

**Альтернатива (проще, но менее верно):** только Gemini 2.5 (подписи опциональны → не
персистим, ветка как Anthropic). Отбрасывает флагман 3.x — не рекомендуется.

### 4.3 Правки семплинга

- `supported_sampling_fields(Some(Gemini))` → `["temperature", "top_p", "top_k",
  "max_tokens", "seed", "frequency_penalty", "presence_penalty", "thinking",
  "reasoning_effort"]`. Отличия от текущего compat-набора: **+`top_k`** (Gemini
  принимает нативно), **+`thinking`/`reasoning_effort`**. **Нет `verbosity`** (это
  OpenAI-Responses-специфика). Это **единственная** правка для UI настроек и
  `get/set_sampling` — всё выводится из неё (как было с Claude/OpenAI).
- Маппинг в wire: `thinking:Some(true)`+`includeThoughts:true`; `reasoning_effort`→
  `thinkingLevel` (3.x) / `thinkingBudget` (2.5); `reasoning_budget==Some(0)`→
  минимальный уровень/`thinkingBudget:0` (см. ловушку 2); `max_tokens`→`maxOutputTokens`;
  `temperature`/`top_p`/`top_k`/`seed`/`frequency_penalty`/`presence_penalty`→
  `generationConfig.*`.

### 4.4 Супервайзер и очистка `WireDialect`

- `cloud_chat_setup`: ветка `CloudProvider::Gemini` строит `GeminiClient` вместо
  `OpenAiClient::with_dialect(WireDialect::Gemini)`.
- После этого **`WireDialect::Gemini` становится мёртвым** (его единственный потребитель —
  Gemini-облако). А `WireDialect::OpenAi` уже удалён (Responses). Останется единственный
  вариант `LlamaCpp` → **`WireDialect` можно убрать целиком** вместе с `is_strict`/
  `restrict_to_strict`; `openai/wire.rs` худеет, `OpenAiClient` остаётся для External +
  эмбеддингов. Приятная симметричная зачистка (как удаление `OpenAi`-диалекта в Responses).
- `cloud_embed_setup(Gemini)` **не трогаем** — эмбеддинги идут через compat
  (`…/v1beta/openai/embeddings`). Развести base URL: чат-клиент — `…/v1beta`, эмбеддер —
  `…/v1beta/openai` (либо `GeminiClient` сам строит путь от `…/v1beta`, а `base_url()`
  оставить для эмбеддера; либо два аксессора).

## 5. Что ещё даёт нативный Gemini (сверх «мыслей» и effort)

- **`top_k` назад** (compat его резал) — бесплатно из §4.3.
- **`thoughtsTokenCount`** — в счётчик токенов (у нас уже есть `reasoning_tokens`).
- **`stopSequences`, `responseMimeType`/`responseSchema` (structured output),
  `candidateCount`, `safetySettings`** — задел, не в объёме.
- **Мультимодальность** (изображения/аудио во `parts`) — крупное отдельное направление,
  вне объёма.
- **Google-инструменты** (`googleSearch`, `codeExecution`) — серверные аналоги наших
  `web_search`/`python_exec`; конфликтуют с клиентским agentic-loop (как у OpenAI). Вне
  объёма, задел.

## 6. Оценка объёма

По образцу Anthropic/Responses (`wire.rs` ~500–580 строк + `client.rs` ~400, с тестами и
смоуками):

- **Фаза A** (ядро, без подписей): `gemini/wire.rs` (сборка `contents`/`systemInstruction`/
  `tools.functionDeclarations`/`generationConfig.thinkingConfig`; санитизация схем; разбор
  SSE-частей — текст, `thought:true`, `functionCall`, `finishReason`, `usageMetadata`) +
  `gemini/client.rs` (`EngineBackend`, `x-goog-api-key`, вывод `FinishReason`, usage) +
  правки семплинга/супервайзера. Смоуки: генерация; поток `Thoughts` при `includeThoughts`;
  `reasoning_effort`/`thinkingLevel` принимается; tool-call без подписей (Gemini 2.5 или
  один раунд).
- **Фаза B** (подписи для Gemini 3): `thought_signature` в `ApiToolCall`/`ToolCallDelta`/
  `ToolCallRecord` (+персист), проброс в agentic-loop, эхо подписи на `functionCall`-части
  в `build_contents`. Смоук: два раунда tool-use на **Gemini 3** без `400` (проверяет
  наличие подписи в теле второго запроса) + реплей истории на следующей генерации.
- **Фаза C** (опц.): точная санитизация схем под придирки Gemini; выбор `thinkingLevel`
  vs `thinkingBudget` в UI/по имени модели.

Реалистично: A+B — один PR, сопоставимый с Claude/Responses, **плюс ~30 строк персиста
подписи** (уникальная для Gemini добавка). Ripple вне `shared/api` — контракт (per-call
подпись + персист), семплинг, супервайзер, generation.rs.

**Риск-профиль**: средний. Слои выше `EngineBackend` не меняются; уникальный риск —
подписи Gemini 3 (нужна живая проверка, §7). Откат = вернуть ветку супервайзера на
`OpenAiClient` + Gemini-диалект (при этом придётся временно вернуть `WireDialect::Gemini`,
если его уже удалили — учесть в порядке коммитов).

## 7. Открытые вопросы (требуют живого ключа)

1. **Область валидации подписи на Gemini 3.** Требует ли API подпись у **всех**
   исторических `functionCall` (тогда персист обязателен), или только у самого свежего
   хода, ответ на который сейчас отправляется (тогда, возможно, хватит подписи-в-памяти,
   как у Anthropic)? Это определяет, нужен ли персист в доменном `Message` — **главный
   вопрос**. Увидеть `400` глазами.
2. **`thinkingLevel` vs `thinkingBudget` по имени модели.** Принимает ли 3.x
   `thinkingBudget` (и наоборот)? Нужен ли инференс поколения, или один параметр
   универсален.
3. **Санитизация схем инструментов.** Какие именно ключи JSON-Schema наших инструментов
   Gemini отвергает (`$schema`, `additionalProperties`, форматы) — увидеть на реальных
   схемах проекта.
4. **Формат `thoughtSignature` при параллельных вызовах** (только первая часть) —
   подтвердить, что accumulator корректно кладёт подпись на нужный вызов.
5. **`role` у `functionResponse`** — подтвердить `user` (а не `function`); и что несколько
   `functionResponse` подряд можно слать одной `user`-частью (как склейка у Anthropic).
6. **Приходит ли резюме мыслей без верификации организации** (у OpenAI резюме придержаны
   до верификации — проверить, есть ли у Google аналогичный гейт; по докам — нет, но
   увидеть на живом ключе).

Смоуки — по образцу Anthropic/OpenAI: `#[ignore]`, ключ из `MINDFORK_GEMINI_KEY`, модель
из `MINDFORK_GEMINI_MODEL`.

## 8. План реализации (детальный)

Три фазы + опц. C. **A** доводится и мержится отдельно (мысли/effort без tool-use
подписей — уже полезно), **B** добавляет подписи для Gemini 3, **C** — полировка.
Порядок коммитов и точные правки:

### Фаза A — ядро нативного клиента (мысли + reasoning, без подписей) — СДЕЛАНА

> **Статус (2026-07-10):** реализовано на ветке `feat/gemini-native-client`.
> `shared/api/gemini/{wire,client,mod}.rs` (нативный `GeminiClient`); режим `gemini`
> переключён с OpenAI-compat на нативный `generateContent`; `WireDialect` удалён целиком
> (осиротел — `openai/wire.rs` худеет); `supported_sampling_fields(Gemini)` += `top_k`/
> `thinking`/`reasoning_effort`, − `verbosity`; `CloudProvider::chat_base_url()` (нативный
> `…/v1beta`, эмбеддинги остаются на compat `…/v1beta/openai`). Инференс поколения по имени
> модели (`gemini-3*`→`thinkingLevel`, иначе `thinkingBudget`) внесён уже в Фазу A. **13
> unit-тестов + 3 `#[ignore]`-смоука** (`MINDFORK_GEMINI_KEY`); fmt/clippy/test зелёные.
> Живой прогон — за пользователем (нужен ключ). Далее — Фаза B (подписи Gemini 3).

1. **`shared/api/gemini/wire.rs`** (новый; образец — `openai/responses/wire.rs`):
   - `build_request(req, model, stream) -> GenRequest`:
     - `systemInstruction: {parts:[{text}]}` из `req.system` (skip если пусто);
     - `contents: Vec<Value>` из `build_contents(req)` (см. ниже);
     - `tools: [{functionDeclarations: [{name, description, parameters: sanitize(schema)}]}]`
       (skip пустых); `sanitize_schema` снимает `$schema`/неподдержанное (Фаза C
       уточняет — на A достаточно снять `$schema` и `additionalProperties` с корня);
     - `generationConfig`: `maxOutputTokens`←`max_tokens`, `temperature`/`topP`←`top_p`/
       `topK`←`top_k`/`seed`/`frequencyPenalty`/`presencePenalty` (все `skip_if None`),
       `thinkingConfig` (см. ниже);
     - `thinkingConfig`: `includeThoughts:true` при `thinking==Some(true)&&!force_off`;
       глубина — `mapping(reasoning_effort)` (по умолчанию `thinkingLevel`; §3.1);
       `force_off` (`reasoning_budget==Some(0)`) → минимальный уровень, `includeThoughts:false`.
   - `build_contents(req)`: роль `User→"user"`, `Assistant→"model"`, `Tool→"user"` c
     частью `functionResponse:{name, response:{...}}`. Склейка соседних одной роли
     (как Anthropic). `functionCall`-часть: `{functionCall:{name, args}}` — `args`
     из строки в объект (не-объект → `{}`, как Anthropic). System пропускается.
     Часть-подпись `thoughtSignature` — **Фаза B** (на A не ставим).
   - Serde-типы разбора SSE: `GenResponse { candidates:[{content:{parts:[Part]},
     finishReason}], usageMetadata }`, `Part { text?, thought?:bool, functionCall?,
     thoughtSignature? }`, `usageMetadata { promptTokenCount, candidatesTokenCount,
     thoughtsTokenCount, totalTokenCount }`. Тесты сборки/разбора — как в
     `responses/wire.rs`.
2. **`shared/api/gemini/client.rs`** (новый; образец — `responses/client.rs`):
   - `GeminiClient { http, base_url, api_key, model }`; `chat_stream`:
     `POST {base}/models/{model}:streamGenerateContent?alt=sse`, заголовок
     `x-goog-api-key` (не `bearer_auth`); тело ошибки не глотаем; SSE через
     `eventsource()`.
   - Разбор частей потока → `ChatChunk`: `text`(без `thought`)→`Text`;
     `text`+`thought:true`→`Thoughts`; `functionCall`→`ToolCall(ToolCallDelta{index,
     id:name-based или пусто, name, arguments: args.to_string()})` (у Gemini `args` —
     объект целиком в одном чанке; `index` = порядковый номер part); `finishReason`
     → `Finished` (`STOP`+были вызовы→`ToolCalls`; `MAX_TOKENS`→`Length`; иначе `Stop`);
     `usageMetadata`→`Usage(TokenUsage{prompt=promptTokenCount, completion=
     candidatesTokenCount, reasoning=thoughtsTokenCount})`. Отмена — `select! cancel`.
     - **Нюанс id**: у Gemini нет `call_id`. Клиент синтезирует стабильный `id` (напр.
       `format!("{name}-{index}")`) — он нужен только нашей внутренней парности
       `functionCall↔functionResponse`; в wire (Фаза A `build_contents`) `id` не
       сериализуется (парность у Gemini позиционная).
   - Смоуки `#[ignore]` (ключ `MINDFORK_GEMINI_KEY`, модель `MINDFORK_GEMINI_MODEL`,
     дефолт `gemini-2.5-flash`): `simple_generation`; `thinking_streams_thoughts`
     (includeThoughts→Thoughts); tool-call один раунд.
3. **`shared/api/gemini/mod.rs`** + реэкспорт `GeminiClient` в `shared/api/mod.rs`.
4. **Семплинг** (`entities/sampling.rs`): `supported_sampling_fields(Some(Gemini))` →
   `["temperature","top_p","top_k","max_tokens","seed","frequency_penalty",
   "presence_penalty","thinking","reasoning_effort"]`. Обновить тест
   `supported_fields_mirror_wire_dialect` (Gemini теперь имеет `top_k`+reasoning, без
   `verbosity`). Проверить `retain_supported`-тест.
5. **Супервайзер** (`app/supervisor.rs`): `cloud_chat_setup` ветка
   `CloudProvider::Gemini` → `GeminiClient::new(base, key, model)` вместо
   `OpenAiClient::…with_dialect(Gemini)`. Base для чата — `…/v1beta` (не `…/openai`):
   либо новый аксессор `CloudProvider::Gemini.chat_base_url()`, либо `GeminiClient`
   строит путь от общего `…/v1beta`. `cloud_embed_setup(Gemini)` **не трогать**
   (эмбеддинги через compat `…/v1beta/openai`). Импперсонация (`cloud_chat_setup`)
   получает Gemini автоматически.
6. **Удаление `WireDialect::Gemini`**: диалект осиротел → убрать вариант,
   `is_strict`/`restrict_to_strict` и (раз остаётся один `LlamaCpp`) при желании
   **весь `WireDialect`**. Осторожно с порядком: сделать в этом же PR **после** п.5,
   иначе откат сложнее (см. §6). Обновить `openai/wire.rs`, реэкспорт в `mod.rs`,
   тест `gemini_dialect_keeps_temp_and_strips_extensions` (удалить/заменить).
7. **UI** (`screens/settings/`): проверить, что секция «Семплинг» для Gemini теперь
   показывает `top_k`/`thinking`/`reasoning_effort` и прячет `verbosity` — всё
   выводится из `supported_sampling_fields`, кода UI править не нужно (как было с
   Claude/OpenAI). Прогнать settings-тесты.
   → **Гейт A**: `cargo fmt`/`clippy -D warnings`/`test` зелёные; живой смоук
   (генерация + мысли + один tool-раунд) на Gemini 2.5/3.

### Фаза B — подписи мыслей (Gemini 3, tool-use round-trip)

Отличие Gemini от Anthropic/OpenAI: подпись **per-tool-call**, не одна на ход →
существующий `ThinkingRef`/`ThinkingBlock` не переиспользуем, а добавляем поле в вызов.

1. **Контракт** (`shared/api/contract.rs`):
   - `ApiToolCall.thought_signature: Option<String>` (обновить `ApiToolCall {…}`
     литералы: `record_to_api`, тесты, accumulator);
   - `ToolCallDelta.thought_signature: Option<String>`;
   - `ToolCallAccumulator::push` копит `thought_signature` в нужный вызов по `index`
     (если `Some`). Прочие бэкенды поле не выставляют → `None`.
2. **Домен** (`entities/message.rs`): `ToolCallRecord.thought_signature:
   Option<String>` (`#[serde(default, skip_serializing_if=Option::is_none)]` → без
   миграции). **Персист** ради реплея истории на Gemini 3 (см. §2.3, §7-1).
3. **Gemini client** (`gemini/client.rs`): при разборе `functionCall`-части с
   `thoughtSignature` — класть её в `ToolCallDelta.thought_signature` того же `index`.
   (У параллельных вызовов подпись только у первого — accumulator это переживает,
   остальные `None`.)
4. **Gemini wire** (`gemini/wire.rs`, `build_contents`): для `functionCall`-части
   ставить сосед `thoughtSignature` из `ApiToolCall.thought_signature` (skip если
   `None`). Тест: подпись едет на нужной части.
5. **generation.rs**: при построении `records` копировать `call.thought_signature` в
   `ToolCallRecord` (одна строка в цикле, стр. ~542). `assistant_tool_calls(out.text,
   out.calls)` уже несёт подписи в `out.calls` (accumulator) → wire их переотправит в
   том же ходе. Round-level `thinking_ref`/`.with_thinking` **не трогаем** (это для
   Anthropic/OpenAI; Gemini его не использует).
6. **request.rs** (`record_to_api`): протянуть `thought_signature` из
   `ToolCallRecord` в `ApiToolCall` — так исторические вызовы на **следующей**
   генерации несут подпись (закрывает 400 Gemini 3 на реплее).
   - **Текст-part подписи** (чисто-reasoning ход без вызова) — **не персистим**
     (наш реплей assistant-текста их не несёт); по докам жёсткое требование — только
     `functionCall`-части. Отметить known-limitation.
   - Смоук `#[ignore]` `tool_use_round_trips_signature` на **Gemini 3**: раунд-1 даёт
     вызов+подпись; раунд-2 переотправляет подпись на `functionCall` + результат — без
     `400`. Плюс проверка реплея (третий запрос с историей из шага-2 — без `400`).
   → **Гейт B**: как A + живой tool-round-trip на Gemini 3.

### Фаза C — полировка (опц.)

- **Санитизация схем**: точный набор ключей, которые Gemini отвергает (`$schema`,
  вложенные `additionalProperties`, форматы `date-time`/…) — по результату живого
  прогона реальных схем инструментов проекта.
- **`thinkingLevel` vs `thinkingBudget`**: инференс поколения по имени модели
  (`gemini-3*`→level, `gemini-2.5*`→budget) либо явный выбор — если живой прогон
  покажет, что один параметр не универсален.
- Обновить `CLAUDE.md` (журнал), `ADR 0004` (Gemini native вместо «вне объёма»),
  `architecture.md §9` при необходимости.

### Ripple вне `shared/api/gemini/` (сводка)

`contract.rs` (+2 поля, accumulator), `entities/message.rs` (+1 поле персист),
`entities/sampling.rs` (supported-набор), `app/supervisor.rs` (ветка + base URL),
`openai/wire.rs`+`mod.rs` (удаление `WireDialect`), `orchestrator/request.rs`
(+подпись), `orchestrator/generation.rs` (+1 строка records). UI/оркестратор/
agentic-loop структурно не меняются. ~40 строк + новый модуль ~900 строк с тестами.

### Развилка, требующая живого ключа (решить до Фазы B)

Персист подписи (шаг B-2/B-6) — на случай, если Gemini 3 валидирует подпись у **всех**
исторических `functionCall`. Если живой прогон покажет, что хватает подписи только у
**текущего** хода (как Anthropic), персист (B-2/B-6) можно снять, оставив подпись лишь
в памяти хода (проще). **По умолчанию проектируем с персистом** (безопаснее);
подтвердить на `MINDFORK_GEMINI_KEY` перед финализацией B.

---

Источники: [thinking (generateContent)](https://ai.google.dev/gemini-api/docs/generate-content/thinking),
[thought signatures](https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures),
[generateContent reference](https://ai.google.dev/api/generate-content),
[OpenAI compatibility](https://ai.google.dev/gemini-api/docs/openai.md.txt),
[Interactions API thinking](https://ai.google.dev/gemini-api/docs/thinking).
