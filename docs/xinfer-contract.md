# Контракт интеграции (исходно xinfer, ADR)

> **Историческая записка.** Движок переведён с `xinfer` на **llama.cpp
> `llama-server`** (xinfer оказался слишком сырым по Gemma 4 и плохо собирался под
> Windows). Этот документ сохранён как описание **OpenAI-совместимого протокола**,
> на который опирается клиент `mindfork-rs`, — llama.cpp говорит на том же
> протоколе. Актуальная инструкция по запуску — в [install.md §3](install.md).

**Статус:** зафиксирован по исходникам.
**Источник:** репозиторий `guoqingbao/xinfer`, ветка `main`, версия крейта **0.13.1** (по `Cargo.toml`), анализ от 2026-06-14.
**Файлы-источники:** `src/api.rs`, `src/server/mod.rs`, `src/server/server.rs`, `src/server/streaming.rs`, `src/utils/config.rs`, `src/core/engine.rs`, `src/core/mod.rs`, `src/tools/mod.rs`, `src/utils/env.rs`, `src/main.rs`, `example/rust-demo*/`.

Документ фиксирует фактический API `xinfer` и **контракт, на который опирается `mindfork-rs`** (см. [spec.md](../spec.md), [plan.md §M1](../plan.md)). Закрывает `[R]`-задачу «API xinfer» из плана.

---

## 1. Решение: режим интеграции

`xinfer` существует в **двух формах** (обе подтверждены в исходниках):

1. **Встраиваемый Rust-API** (`xinfer::api`): `EngineBuilder::new(ModelRepo)…build() -> Engine`; `Engine::generate / generate_stream / start_server`; `EngineStream::{recv, recv_blocking, cancel, is_finished}`. Тянет `candle` (+опц. CUDA/Metal) в наш бинарник.
2. **Standalone HTTP-сервер** (OpenAI- и Anthropic-совместимый): `xinfer --m <id> --server [--port N] --d <gpu>`.

**Принятое решение — режим HTTP-сервер** (managed-подпроцесс по умолчанию + external). Обоснование с учётом фактов:

- Встраиваемый API даёт сверх HTTP лишь `stop_token_ids` и `ignore_eos` (плюс прямой `cancel`). Набор семплинга у встраиваемого `SamplingParams` **тот же**, что по HTTP (нет `min_p`/`repetition_penalty`/`seed` ни там, ни там). То есть встраивание почти не добавляет возможностей, но тянет ML-стек и CUDA-сборку в наш бинарник — ровно та сложность сборки, от которой уходим в попытке №2.
- HTTP-контракт стабилен, стандартен (OpenAI), изолирует сборку (наш бинарник — чистый HTTP-клиент), упрощает CI/кроссплатформенность.
- Tool-call’ы парсятся **сервером** (отдаются структурно), reasoning стримится **отдельным полем** — нам не нужно парсить `<think>`/`<tool_call>` вручную.

Встраивание остаётся теоретическим fallback’ом; слой `shared/api` (`EngineBackend`) спроектирован так, что транспорт можно сменить без затрагивания оркестратора.

---

## 2. Запуск сервера (managed-режим)

CLI-аргументы (из `Args`, `src/server/mod.rs`; `--m/--w/--f` — короткие алиасы):

| Флаг | Поле | Назначение |
|---|---|---|
| `--m <id>` | `model_id` | HF-идентификатор (safetensors) или id GGUF-репозитория |
| `--w <path>` | `weight_path` | Локальная папка с safetensors+json (HF-структура) |
| `--f <file>` | `weight_file` | Путь к GGUF-файлу (или имя файла при заданном `--m`) |
| `--isq <q>` | `isq` | On-the-fly квантизация (q2k…q8_0) |
| `--d 0,1` | `device_ids` | GPU-устройства (value_delimiter `,`) |
| `--cpu` | `cpu` | CPU-режим |
| `--dtype <t>` | `dtype` | f16/bf16/f32 |
| `--server` | `server` | **Режим сервера** (обязателен) |
| `--port <N>` | `port` | Порт (`Option<usize>`; по докам дефолт **8000**) |
| `--max_tokens N` | `max_tokens` | Дефолт 16384 |
| `--temperature/--top_k/--top_p/--frequency_penalty/--presence_penalty` | — | Дефолты семплинга сервера |
| `--seed <u64>` | `seed` | **Глобальный** seed воспроизводимости (НЕ на запрос) |
| `--enforce_parser <p>` | `enforce_parser` | Форсировать парсер tool-call: `qwen`, `qwen_coder`, `json`, … |
| `--tool_prompt <s>` | `tool_prompt` | Кастомный tool-промпт |
| `--disable_prefix_cache` | — | Отключить prefix caching (вкл по умолчанию) |
| `--prefix_cache_max_tokens N` | — | Лимит prefix-кэша |
| `--kvcache_dtype <t>` | — | `auto`/`fp8`/`turbo8`/`turbo4`/`turbo3` (TurboQuant) |
| `--kv_fraction`, `--mamba_fraction`, `--cpu_mem_fold` | — | Память KV |

**Важно — обязательные переменные окружения, которые задаёт наш супервайзер при запуске:**

- `XINFER_STREAM_AS_REASONING_CONTENT=1` — включает раздельную отдачу reasoning в поле `delta.reasoning_content` (иначе reasoning приходит инлайн в `content`). Константа в `src/utils/env.rs`: `STREAM_AS_REASONING_CONTENT_ENV`. **Без этой переменной разбор «мыслей» придётся делать парсингом `<think>` — мы её ставим всегда.**

**Готовность сервера:** маршрутов `/health` и `/v1/models` в исходниках **не обнаружено**. Реализованные эндпоинты: `/v1/chat/completions`, `/v1/embeddings`, `/v1/usage`, токенайзер (`tokenize`/`detokenize`). Готовность определяем: (а) парсингом stdout/stderr дочернего процесса (строка о старте listener’а) и/или (б) поллингом `GET /v1/usage` до успешного ответа. *(Уточнить точную строку готовности и адрес bind — 127.0.0.1 vs 0.0.0.0 — при первом запуске в M1; bind на loopback желателен из соображений приватности.)*

---

## 3. Эндпоинт чата: `POST /v1/chat/completions`

### 3.1. Тело запроса — `ChatCompletionRequest` (`src/server/mod.rs`)

| Поле | Тип | Примечание |
|---|---|---|
| `messages` | `Vec<ChatMessage>` | см. [3.2](#32-chatmessage) |
| `model` | `Option<String>` | информативно (одна модель на сервер) |
| `temperature` | `Option<f32>` | |
| `max_tokens` | `Option<usize>` | |
| `top_k` | `Option<isize>` | `-1`/отрицательное — отключить |
| `top_p` | `Option<f32>` | |
| `frequency_penalty` | `Option<f32>` | |
| `presence_penalty` | `Option<f32>` | |
| `thinking` | `Option<bool>` | алиас `enable_thinking` — включить reasoning |
| `stop` | `Option<Vec<String>>` | алиас `stop_sequences`; **по умолчанию НЕ задаём** (см. [§5](#5-обработка-eos)) |
| `stream` | `Option<bool>` | `true` для SSE |
| `stream_options` | `{ include_usage: bool }` | usage в финальном чанке |
| `session_id` | `Option<String>` | |
| `tools` | `Option<Vec<Tool>>` | OpenAI-схемы (см. [§4](#4-инструменты-tool-calling)) |
| `tool_choice` | `Option<ToolChoice>` | `auto`/`none`/`required` или конкретная функция |
| `response_format` | `Option<ResponseFormat>` | structured outputs (JSON Schema) |
| `extra_body` | `{ structured_outputs?, extra: map }` | сервер читает только `structured_outputs`; произвольные поля в `extra` **не** мапятся на семплинг |
| `structured_outputs` / `constraint` / `constraint_type` | — | грамматики llguidance |
| `reasoning_effort` | `Option<String>` | алиас `reasoning`: `"none"/"low"/"medium"/"high"` |

**НЕ поддерживаются по HTTP:** `min_p`, `repetition_penalty`, `seed` (на запрос), `logit_bias`, `dry_*`, `typical_p`, mirostat/dynatemp, `top_n_sigma`. (`extra_body.extra` — свободная map, но сервер её на семплинг не применяет.)

### 3.2. `ChatMessage`

```
{ role: String,
  content: Option<MessageContentType>,         // PureText(String) | Single(MessageContent) | Multi([...])
  tool_calls: Option<Vec<ToolCall>>,           // у assistant-хода с вызовами
  tool_call_id: Option<String>,                // у role="tool"
  reasoning_content: Option<String> }
```

- `MessageContent`: `Text{text}` | `ImageUrl{image_url}` | `ImageBase64{image_base64}` (мультимодальность; нам нужен только текст → `PureText`).
- Хелперы: `ChatMessage::text(role, content)`, `with_tool_calls(tool_calls)`, `tool_result(tool_call_id, content)`.
- **Строгая валидация порядка сервером** (`src/server/server.rs`): сообщение, отвечающее на `assistant.tool_calls`, обязано иметь `role="tool"`; `role="tool"` без предшествующего assistant с tool_calls отвергается; каждый `tool_call` обязан иметь непустой `id`; `role="tool"` не должен содержать `tool_calls`. → **Наш клиентский agentic-loop обязан соблюдать OpenAI-порядок: assistant(tool_calls) → tool(tool_call_id) для каждого вызова.**

### 3.3. Ответ — потоковый (`stream=true`)

SSE-чанки `ChatCompletionChunk { id, object, created, model, choices: [ChatChoiceChunk], usage? }`, где:

```
ChatChoiceChunk { index, delta: Delta, finish_reason: Option<String>, error: Option<[ErrorMsg]> }
Delta { role?, content?, reasoning_content?, tool_calls? : Option<Vec<PublicToolCall>> }
PublicToolCall { index?, id, type: "function", function: FunctionCall }
```

- **`delta.content`** — дельта основного текста.
- **`delta.reasoning_content`** — дельта «мыслей» (при `XINFER_STREAM_AS_REASONING_CONTENT=1`).
- **`delta.tool_calls`** — структурно распарсенные сервером вызовы (нам парсить `<tool_call>` не нужно).
- **`finish_reason`**: `"tool_calls"` — есть вызовы инструментов; `"stop"`/`"length"` — обычное завершение; `null` — промежуточный чанк. (Строка `"tool_calls"` подтверждена в `server.rs`.)
- `error` в чанке — потоковая ошибка.

### 3.4. Ответ — нестриминговый

`ChatCompletionResponse { id, object, created, model, choices: [ChatChoice], usage }`,
`ChatChoice { index, message: ChatResponseMessage, finish_reason }`,
`ChatResponseMessage { role, content?, reasoning_content?, tool_calls? }`,
`Usage { prompt/completion/total + *_details }`.

---

## 4. Инструменты (tool calling)

- Схема инструмента — OpenAI-формат. Билдеры (`src/tools`): `Tool::function(name, desc).param(name, type, desc, required).build()` или `SchemaBuilder::object().string_prop(...).integer_prop(...).build()` + `.parameters_schema(...)`.
- `ToolChoice`: `Mode("auto"|"none"|"required")` (rename_all lowercase) либо `Function{ type, function }` (форсировать конкретный инструмент).
- `ToolCall`/`FunctionCall { name, arguments }` — в ответе; `ToolResult { tool_call_id, content, is_error? }` — для возврата.
- Парсинг tool-call’ов из текста модели — **на сервере** (`--enforce_parser`, форматы: `<tool_call>{...}</tool_call>`, голый JSON, ```` ```json ````). Клиент получает уже структурный `tool_calls`.

**Следствие для нас:** agentic-loop клиентский (мы исполняем инструменты и продолжаем диалог), но **извлечение** вызовов делает сервер. Передаём `tools` (OpenAI-схемы) + `tool_choice=auto`; на `finish_reason="tool_calls"` исполняем, добавляем `assistant(tool_calls)` + `tool(tool_call_id, result)` и повторяем (до `max_tool_rounds`).

---

## 5. Обработка EOS

- Остановка на стороне сервера — по реальным EOS-токенам модели (через chat template/токенайзер). Это удовлетворяет требованию «не обрываться на тексте EOS»: литеральный `<|im_end|>`/`<end_of_turn>` как обычный текст не завершает ход.
- `stop_token_ids: Option<Vec<Vec<u32>>>` есть только во встраиваемом `SamplingParams` и помечен `#[serde(skip)]` → **по HTTP недоступен** (и не нужен: дефолтная EOS-логика сервера корректна).
- `ignore_eos: bool` — тоже только встраиваемый, по HTTP отсутствует (нам не нужен, EOS должен работать).
- **Инвариант нашего клиента:** поле `stop` в запросе оставляем пустым (никаких строковых стопов по тексту EOS). Строковые стопы — только осознанно, с предупреждением.

---

## 6. Эмбеддинги (RAG): `POST /v1/embeddings`

`EmbeddingRequest { model?, input: Single(String)|Multiple([String]), encoding_format, embedding_type: EmbeddingStrategy }`
→ `EmbeddingResponse { data: [EmbeddingData{ embedding, index }], model, usage }`.

- Эндпоинт работает на **уже запущенном сервере** (та же загруженная модель). Это потенциально снимает необходимость во втором процессе для RAG.
- **Открыто (M5):** качество эмбеддингов из chat-модели (Qwen/Gemma) vs выделенной embedding-модели; выбор `embedding_type` (стратегия пулинга) и размерности. Если качество недостаточно — поднимать выделенный embedding-сервер по требованию. (Размерность фиксируем по факту первого ответа и храним в схеме sqlite-vec.)

---

## 7. Семплинг: фактический контракт `mindfork-rs`

`entities::SamplingConfig` → поля `ChatCompletionRequest`:

| `SamplingConfig` | HTTP-поле | Статус |
|---|---|---|
| `temperature` | `temperature` | ✅ |
| `top_k` | `top_k` (isize) | ✅ |
| `top_p` | `top_p` | ✅ |
| `frequency_penalty` | `frequency_penalty` | ✅ |
| `presence_penalty` | `presence_penalty` | ✅ |
| `max_tokens` | `max_tokens` | ✅ |
| `thinking`/reasoning | `thinking` + `reasoning_effort` | ✅ (новое относительно попытки №1) |
| `min_p` | — | ❌ нет в xinfer |
| `repetition_penalty` | — | ❌ нет в xinfer |
| `seed` (на чат/запрос) | — | ❌ только глобальный `--seed` при запуске сервера |
| DRY / typical_p / mirostat / dynatemp / top_n_sigma | — | ❌ |

→ В UI настроек семплинга оставляем: **temperature, top_k, top_p, frequency_penalty, presence_penalty, max_tokens, thinking, reasoning_effort**. Поля `min_p`/`repetition_penalty`/`seed`(на запрос) **убираем** из модели и UI (или помечаем недоступными). «Случайный seed на чат» из требований **не реализуем по HTTP** — для регенерации с вариативностью полагаемся на temperature>0 (поведение документируем).

---

## 8. Маппинг на `EngineBackend` (наш слой `shared/api`)

```rust
trait EngineBackend {
    async fn chat_stream(&self, req: ChatRequest, cancel: CancellationToken)
        -> Result<BoxStream<'static, ChatChunk>>;   // POST /v1/chat/completions, stream=true
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>>; // POST /v1/embeddings
}

// ChatChunk ← Delta:
//   Delta.content          -> ChatChunk::Text
//   Delta.reasoning_content -> ChatChunk::Thoughts
//   Delta.tool_calls       -> ChatChunk::ToolCallDelta   (накапливаем по index/id)
//   finish_reason          -> ChatChunk::Finished{ Stop|ToolCalls|Length|Cancelled }
```

- Отмена: `CancellationToken` прерывает чтение SSE-стрима (HTTP-соединение закрывается). *(Проверить в M1, что закрытие соединения корректно останавливает генерацию на сервере; иначе — встраиваемый `EngineStream::cancel` как запасной путь.)*
- Клиент: `async-openai` с расширением типов под `thinking`/`reasoning_content`/`reasoning_effort` через `serde`, либо `reqwest` + собственные типы (типы из этого документа). Выбор — в M1.

---

## 9. Что проверить при первом запуске (M1 smoke-checklist)

1. Точный дефолт `--port` и адрес bind (ожидаем `127.0.0.1:8000`); при необходимости задаём `--port` явно.
2. Сигнал готовности сервера (строка stdout / поллинг `/v1/usage`).
3. `XINFER_STREAM_AS_REASONING_CONTENT=1` действительно даёт `delta.reasoning_content` на Qwen3/Gemma.
4. `finish_reason="tool_calls"` и формат `delta.tool_calls` на простом инструменте; корректность строгой валидации порядка сообщений.
5. Закрытие SSE-соединения по `CancellationToken` останавливает генерацию (нет «осиротевшей» генерации на сервере).
6. Анти-самообрыв: промпт, провоцирующий печать `<|im_end|>`/`<end_of_turn>`, не завершает ход (пустой `stop`).
7. `/v1/embeddings` на загруженной chat-модели: формат ответа, размерность, базовая адекватность (M5).
8. Поведение `--enforce_parser` для Qwen и Gemma (нужен ли разный парсер).

---

*Документ — зафиксированный контракт; обновляется при изменении API `xinfer` или по результатам M1-smoke.*
