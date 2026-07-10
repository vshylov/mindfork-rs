# Исследование: полноценный клиент режима `openai` (Responses API)

**Статус:** реализовано (2026-07-10). Исходно — исследование; выбран **вариант A**
(режим `openai` переведён с Chat Completions на Responses API), добавлены «мысли»
(резюме рассуждений), `reasoning_effort` (расширен до `minimal`/`xhigh`), `verbosity`;
«прокси с ключом» закрыт полем `ExternalSettings.api_key_env`. Итог в CLAUDE.md
(«OpenAI → Responses API»). Развивает
[ADR 0004](../decisions/0004-engine-contract-multi-provider.md) (Фазы 0–2).

## 1. Проблема

Режим `ServerMode::OpenAi` сегодня ходит в **Chat Completions**
(`/v1/chat/completions`) тем же `OpenAiClient`, что и llama.cpp/Gemini, отличаясь лишь
`WireDialect::OpenAi`. Диалект **вычищает** все reasoning-сигналы
(`restrict_to_strict`: `thinking`, `reasoning_effort`, `reasoning_budget`,
`chat_template_kwargs`), поэтому:

- «мыслей» (CoT) в режиме `openai` нет вообще — `ChatChunk::Thoughts` не приходит;
- `reasoning_effort` не отправляется, глубиной рассуждения управлять нельзя;
- `supported_sampling_fields(OpenAi)` = `frequency_penalty`/`presence_penalty`/`seed`/
  `max_tokens` — четыре поля, из которых для gpt-5.x реально значимо одно.

При этом у Claude (Фаза 2) «мысли» и `reasoning_effort` есть — асимметрия видна
пользователю прямо в секции «Семплинг».

## 2. Что выяснено про API (сверено по docs, июль 2026)

### 2.1 Reasoning живёт в Responses API, не в Chat Completions

- **Сырой CoT не отдаётся никогда** — ни в одном API. Доступны только **резюме
  рассуждений** (reasoning summaries). Это то же, что у Anthropic `display:"summarized"`:
  в UI мы показываем резюме, а не сырые токены. ([reasoning guide])
- Резюме приходят **только через Responses API** (`POST /v1/responses`) и только при
  явном opt-in `reasoning.summary` = `"auto"` | `"concise"` | `"detailed"` (по умолчанию
  не включаются). В Chat Completions резюме нет; сам Chat Completions для
  reasoning-моделей документация называет legacy-путём. ([reasoning guide])
- `reasoning.effort`: `none` | `minimal` | `low` | `medium` | `high` | `xhigh` (у gpt-5.6
  ещё `max`), набор **зависит от модели**; gpt-5.5 по умолчанию `medium`. Наш
  `ReasoningEffort` знает только `none/low/medium/high`.
- `temperature`/`top_p` reasoning-модели не принимают (это мы уже знаем — коммит
  `f032df8`); `frequency_penalty`/`presence_penalty` в Responses API вообще нет как
  параметров.

### 2.2 Reasoning-элементы и tool-use (ключевой нюанс, зеркало Anthropic Phase B)

При stateless-работе (`store: false`, наш случай — история чата у нас своя) нужно:

- на **каждом** запросе просить `include: ["reasoning.encrypted_content"]`;
- из ответа забирать элемент `{"type":"reasoning","id":"rs_…","encrypted_content":"gAAA…"}`
  и **возвращать его во входе следующего раунда непосредственно перед** его
  `function_call`-элементом;
- пропуск reasoning-элементов не ошибка (API их просто не увидит), но даёт **~3%
  просадки** качества на бенчмарках (OpenAI меряли на SWE-bench). ([cookbook])

Это **тот же механизм**, что уже реализован для Anthropic: `ChatChunk::ThoughtsSignature`
→ `RoundOutput.thoughts_signature` → `ApiMessage::with_thinking(ThinkingBlock)` →
wire ставит блок первым в assistant-ходе с вызовами. Разница ровно одна: у OpenAI кроме
`encrypted_content` нужен ещё `id` элемента.

### 2.3 Формат протокола

| | Chat Completions | Responses |
|---|---|---|
| Системное сообщение | `messages[0].role="system"` | top-level `instructions` |
| История | `messages[]` | `input[]` — смесь сообщений и **элементов** |
| Результат инструмента | `role:"tool"`, `tool_call_id` | `{"type":"function_call_output","call_id","output"}` |
| Вызов инструмента | `assistant.tool_calls[]` | `{"type":"function_call","call_id","name","arguments"}` |
| Схема инструмента | `{type:"function", function:{name,…}}` | `{type:"function", name, description, parameters, strict}` (плоско) |
| Лимит токенов | `max_completion_tokens` | `max_output_tokens` |
| Причина остановки | `finish_reason` | `status` + наличие `function_call` в `output` |
| Usage | `prompt_tokens`/`completion_tokens` | `input_tokens`/`output_tokens` (+`output_tokens_details.reasoning_tokens`, `input_tokens_details.cached_tokens`) |

**SSE**: `event: <имя>` + `data: {...}`, причём **внутри `data` есть поле `type`**, равное
имени события. То есть разбор — ровно как у Anthropic: `#[serde(tag = "type")]` над enum.
Порядок событий одного хода:

```
response.created → response.in_progress
response.output_item.added        (item.type = "reasoning")
response.reasoning_summary_text.delta ×N        → ChatChunk::Thoughts
response.output_item.done         (reasoning: id + encrypted_content)
response.output_item.added        (item.type = "function_call": id, call_id, name)
response.function_call_arguments.delta ×N       → ChatChunk::ToolCall(arguments)
response.output_item.done         (function_call)
response.output_text.delta ×N                   → ChatChunk::Text
response.completed                (response.usage, response.status)
```

Плюс `response.incomplete` (`incomplete_details.reason == "max_output_tokens"`),
`response.failed`, `error`.

## 3. Ловушки (то, на чём споткнёмся, если не заложить заранее)

1. **`max_output_tokens` включает reasoning-токены.** Это ровно тот же класс бага, что
   «мысли съели весь бюджет» у Gemma/авто-названия чата: при скупом лимите модель
   потратит всё на рассуждение, `output_text` придёт пустым, а статус будет
   `incomplete/max_output_tokens`. Наш `SamplingConfig.max_tokens` мапится сюда — нужен
   либо щедрый дефолт, либо предупреждение в подсказке поля. Отдельно: `Finished(Length)`
   должен эмититься по `response.incomplete`.
2. **`store` по умолчанию `true`** — OpenAI сохранит ответы у себя. Шлём `store: false`
   явно (у нас своя история, плюс приватность).
3. **`strict` у function-tool в Responses по умолчанию «пробуй строгий режим»** (в отличие
   от Chat Completions, где по умолчанию нестрогий). Наши схемы не удовлетворяют
   требованиям strict (`additionalProperties: false` + все поля в `required`) — API
   обещает мягкий откат, но полагаться не стоит: шлём `strict: false` явно.
4. **`response.completed` не несёт `finish_reason`.** `FinishReason::ToolCalls` выводится
   клиентом по факту появления `function_call`-элемента в потоке.
5. **Резюме может не прийти** — при низком `effort` или тривиальном промпте модель не
   рассуждает. Ровно как у Claude adaptive thinking (это уже описано в CLAUDE.md).
   Тесты должны это допускать.
6. **Gemini и External остаются на Chat Completions.** У OpenAI-совместимого endpoint
   Gemini нет `/responses`; у сторонних серверов — тем более. `OpenAiClient` никуда не
   девается.
7. **Эмбеддинги** (`/v1/embeddings`) остаются на `OpenAiClient` — в Responses их нет.
   `cloud_embed_setup` не трогаем.
8. `CloudProvider::OpenAi.base_url()` уже `https://api.openai.com/v1` → URL клиента
   `{base}/responses` (в отличие от Anthropic, где клиент сам дописывает `/v1/messages`).

## 4. Архитектура: куда это ложится

Responses — **другой протокол того же вендора**, значит это новая реализация
`EngineBackend`, ровно как `AnthropicClient` (ADR 0004, Фаза 2). Слои выше движка
(оркестратор, agentic-loop, инструменты, UI) не трогаются — это главный вывод
исследования: **фича полностью помещается за трейт**.

Раскладка: `shared/api/openai/responses/{client.rs, wire.rs}` (вендорная группировка
`openai/` сохраняется, внутри — деление по протоколу). Альтернатива —
sibling-модуль `shared/api/responses/` рядом с `anthropic/`; выбор косметический,
предпочтителен первый (ADR §2 группирует `openai/` по семейству).

### 4.1 Развилка: заменить транспорт или добавить режим?

| Вариант | Плюсы | Минусы |
|---|---|---|
| **A. `ServerMode::OpenAi` = Responses** (замена) | Плоская таксономия режимов не растёт; `CloudProvider::OpenAi` остаётся ключом `supported_sampling_fields`; ноль изменений конфига | Ломает тех, кто использовал режим `openai` + `url`-override как «прокси с ключом» (такой прокси говорит на Chat Completions) |
| B. Новый режим `openai-responses` | Ничего не ломает | Два пункта «OpenAI» в селекторе; дублирование облачных полей; `CloudProvider` растёт |
| C. Тумблер `CloudSettings.openai_api = chat\|responses` | Гибко | `supported_sampling_fields(provider)` перестаёт зависеть только от провайдера → сигнатура течёт в UI настроек, `get/set_sampling`, `retain_supported`, метаданные сообщения. Заметный ripple ради редкого случая |

**Рекомендация — A**, а «прокси с ключом» закрыть дешёвым дополнением:
`ExternalSettings.api_key_env: Option<String>` (`#[serde(default)]`, без миграции) —
сейчас у External вообще нет поля ключа, и это самостоятельный пробел.

### 4.2 Правки контракта (минимальные)

- `ChatChunk::ThoughtsSignature(String)` → `ThoughtsSignature(ThinkingRef)`, где
  `ThinkingRef { id: Option<String>, signature: String }`. Anthropic кладёт `id: None`,
  OpenAI — `Some("rs_…")` + `signature = encrypted_content`. Все места, где вариант
  игнорируется (`subagent`, `fetch`, `title`, `impersonation`, `tool_loop`,
  `openai/client`), матчат `ThoughtsSignature(_)` — их не придётся трогать.
- `ThinkingBlock` += `id: Option<String>`.
- `RoundOutput.thoughts_signature: Option<ThinkingRef>` (`generation.rs`, ~5 строк).

### 4.3 Правки семплинга

- `ReasoningEffort` += `Minimal`, `XHigh`, `Max` (serde lowercase; старые
  `settings.json` читаются). Маппинг `ReasoningEffort::None` → `reasoning.effort:"none"`
  (а не «не слать поле») — это даёт имперсонации и авто-названию чата честное
  выключение рассуждений, аналог `reasoning_budget=0`. `reasoning_budget == Some(0)`
  трактуем как `effort:"none"`.
- `supported_sampling_fields(Some(OpenAi))` → `["max_tokens", "thinking",
  "reasoning_effort"]` (позже `+"verbosity"`). Это **единственная** правка, нужная UI
  настроек, инструментам `get/set_sampling` и снимку `Message.metadata` — всё выводится
  из неё автоматически (как было с Claude).
- `thinking: Some(true)` → `reasoning.summary: "auto"`; иначе поле опускаем.

### 4.4 Супервайзер

`cloud_chat_setup`: ветка `CloudProvider::OpenAi` строит `ResponsesClient` вместо
`OpenAiClient::with_dialect(WireDialect::OpenAi)`. `WireDialect::OpenAi` при этом
становится мёртвым (его единственный потребитель — OpenAI-облако) → удаляем вместе с
`max_completion_tokens` и OpenAI-веткой `restrict_to_strict`; остаются `LlamaCpp` и
`Gemini`. Приятный побочный эффект: `wire.rs` худеет.

## 5. Что ещё даёт Responses (сверх «мыслей» и effort)

Помимо заявленного, «полноценность» может включать:

- **`text.verbosity`** (`low`/`medium`/`high`) — длина ответа, отдельная от температуры.
  Новое поле `SamplingConfig.verbosity` + один `SamplingParam` в UI. Дёшево.
- **Детализация usage**: `reasoning_tokens` (сколько ушло на «мысли») и `cached_tokens`.
  У нас уже есть счётчик токенов в статус-баре — сюда ложится органично.
- **`prompt_cache_key`**: стабильный ключ на чат → дешевле и быстрее кэш промпта.
  Наш системный промпт большой (модель себя, протокол ведения) — выигрыш реальный,
  но он частично съедается тем, что инъекция «модели себя» и так ломает prefix cache
  (принятый трейд-офф, architecture §9.3).
- **`service_tier`** (`flex`/`priority`) — цена/латентность.
- **Встроенные инструменты OpenAI** (`web_search`, `code_interpreter`, `file_search`) —
  серверные аналоги наших `web_search`/`python_exec`. Крупное отдельное направление:
  конфликтует с клиентским agentic-loop (их исполняет сервер, у нас — оркестратор),
  ломает единообразие tool-карточек в ленте. **Вне объёма**, но стоит записать заделом.
- `previous_response_id` / conversations — **не нужны**: у нас своя история, правка и
  перегенерация ходов; server-side state с ними конфликтует.

## 6. Оценка объёма

По образцу Anthropic (`wire.rs` 566 строк + `client.rs` 404, включая тесты и смоуки):

- **Фаза A** (ядро, без tool-use round-trip): `responses/wire.rs` (сборка `input` из
  `ApiMessage`, плоские tools, `store:false`, `include`, `reasoning`, `max_output_tokens`;
  разбор SSE-событий) + `responses/client.rs` (`EngineBackend`, вывод `FinishReason`,
  usage) + правки семплинга/супервайзера. Живые смоуки: простая генерация; поток
  `Thoughts` при `thinking=true`; `effort` принимается.
- **Фаза B** (tool-use): `ThinkingRef` с `id`, эхо reasoning-элемента перед
  `function_call` в `build_input`, правка `generation.rs`. Живой смоук: два раунда с
  инструментом (без эха — деградация, с эхом — корректно; ошибок API быть не должно ни
  там, ни там, поэтому смоук проверяет **наличие** элемента в теле второго запроса).
- **Фаза C** (опц.): `verbosity`, `reasoning_tokens` в статус-баре, `prompt_cache_key`.

Реалистично: A+B — один PR, сопоставимый с Claude Phase A/B. Ripple вне `shared/api`
— около 40 строк (контракт, семплинг, супервайзер, generation).

**Риск-профиль низкий**: слои выше `EngineBackend` не меняются; при неудаче откат =
вернуть ветку супервайзера на `OpenAiClient`.

## 7a. Итог живого прогона (gpt-5.5, 2026-07-10)

Генерация и tool-calling работают. **Резюме рассуждений не пришли** — серверный гейт,
не баг: OpenAI отдаёт `reasoning.summary` только **верифицированным организациям**
(`platform.openai.com/settings/organization/general`); неверифицированной резюме пустое
(или `400` на сам факт `reasoning.summary`). Правки клиента по итогам: `summary:"auto"`
→ `"detailed"` (надёжнее на части моделей) и разбор `response.reasoning_text.delta`
наряду с `response.reasoning_summary_text.delta`. Раз ответ пришёл без `400` —
рассуждение произошло (оплачено reasoning-токенами), просто текст резюме придержан до
верификации. Сырой CoT не отдаётся никогда (только резюме).

## 7. Открытые вопросы (требуют живого ключа)

1. Точный набор `effort` у актуальной модели по умолчанию (принимает ли `xhigh`/`max`) —
   ошибку `400` на неподдержанное значение нужно увидеть глазами, чтобы решить: скрывать
   значения в UI или отдавать текст ошибки пользователю.
2. Приходит ли `id` reasoning-элемента обязательным при эхе (или достаточно
   `encrypted_content`), и что именно API отвечает при reasoning-элементе без
   следующего за ним `function_call`.
3. Совместимость `store:false` + `include:["reasoning.encrypted_content"]` с не-reasoning
   моделями (gpt-4.1 и т.п.) — не должно ломать, но проверить.
4. Нужно ли `reasoning.context: "all_turns"` при ручном реплее истории (по докам —
   влияет только при доступе к прошлым элементам ответа; у нас их нет между ходами).

Смоуки — по образцу Anthropic: `#[ignore]`, ключ из `MINDFORK_OPENAI_KEY`, модель из
`MINDFORK_OPENAI_MODEL`.

---

Источники: [reasoning guide](https://developers.openai.com/api/docs/guides/reasoning),
[cookbook: reasoning items](https://developers.openai.com/cookbook/examples/responses_api/reasoning_items),
[streaming events](https://developers.openai.com/api/reference/resources/responses/streaming-events),
[function calling](https://developers.openai.com/api/docs/guides/function-calling).
