# Исследование: система плагинов (импортёры, инструменты, облачные интеграции)

**Статус:** исследование до решения (жанр — docs/research, AGENTS.md §1). Развилки §8
требуют подтверждения пользователем до реализации; по принятии — ADR.
**Дата:** 2026-07-16. **Ветка:** `docs/plugins-research`.
**Веб-факты** (версии крейтов, ревизии спецификаций, прецеденты) сверены по
первоисточникам 2026-07-16 четырьмя параллельными разведками; ссылки — в §9.

## 1. Задача

Запрос пользователя (2026-07-16): исследовать возможность **системы плагинов**.
Названные цели, в порядке приоритета формулировки:

1. **Импортёр LameLLaMA** — лучший кандидат на первый плагин: LameLLaMA —
   непубличная программа, и знание о её форматах нежелательно держать в монолите
   (`features/migration.rs` сегодня несёт wire-типы её `Settings.json`/`Conversations`).
2. **Инструменты как плагины** — подключать пользовательские инструменты модели
   без пересборки приложения.
3. **Интеграции с облачными платформами** — подключаемые провайдеры инференса.

**Главный вывод исследования:** «система плагинов» — это не одна технология, а
**три разные поверхности с разной природой**, и лучший дизайн для каждой — разный.
Единый in-process plugin-API (dylib «как в больших программах») — худший из
вариантов: у Rust нет стабильного ABI, а индустрия AI-приложений в 2025–2026 уже
сошлась на других механизмах (MCP для инструментов, «любой OpenAI-совместимый
endpoint» для провайдеров, документированный файл обмена для импортёров). Ниже —
обоснование и конкретный план.

## 2. Три поверхности расширения (анализ кода)

| Поверхность | Контракт сегодня | Природа | Что нужно «плагину» |
|---|---|---|---|
| Импортёры | `features/migration.rs::import_dir(dir, loc) -> ImportResult{profiles, chats, sampling, interface}` → upsert в `Storage` (CLI `import-lamellama`) | **разовая batch-операция** без TUI | прочитать чужой формат, отдать профили/чаты |
| Инструменты | трейт `Tool` (`id`/`description(loc)`/`parameters(loc)` JSON-схема/`invoke(ctx) -> ToolOutcome{result, effects}`), реестр `ToolRegistry`, статический `CATALOG`, per-profile `enabled_tools` + `reconcile_tools`, гейты `effective_tool_ids`; вызывается клиентским agentic-loop между HTTP-раундами | **долгоживущий, вызывается в горячем цикле хода**; `ToolContext` несёт `Arc<Storage>`/`engine`/`embedder` — через границу процесса **не передаваемы** | принять JSON-аргументы, вернуть текст; своя схема и описание |
| Движки | трейт `EngineBackend` (`chat_stream` — SSE-стриминг с отменой) + `Embedder`; 4 реализации (llama.cpp/OpenAI/Gemini/Anthropic); режим **external уже принимает любой OpenAI-совместимый сервер** (url + `api_key_env` + model) | **стриминговый, латентно-чувствительный**, сложный протокол (мысли/подписи/tool-calls) | говорить протокол инференса |

Следствия:

- Плагин-инструмент **не получит** `ctx.storage`/`ctx.embedder` — и не должен:
  память/RAG с изоляцией по профилю — внутреннее ядро. Плагины — это «внешние»
  инструменты (API, файлы, устройства), самодостаточные по своим зависимостям.
- Плагин-движок «за процессом» обязан говорить стриминговый протокол — то есть
  быть HTTP-сервером. Такой протокол уже существует и стандартизован де-факто
  (OpenAI-совместимый), и приложение его уже умеет (режим external).
- Импортёру не нужен долгоживущий процесс вовсе: протокол = **файл**.

Прецеденты проекта прямо поддерживают out-of-process направление: managed
`llama-server` (ADR 0004), сайдкар `wasmer` (ADR 0005 — embed в dll рассмотрен и
**отклонён** в пользу подпроцесса), выделенный embedding-сервер (ADR 0002). Весь
нужный инструментарий уже в дереве зависимостей: `tokio::process` + монитор-задачи
с `exited`-токеном, `kill_on_drop`, Job Object (windows-sys уже используется),
`serde_json`, `async-trait`.

## 3. Ландшафт механик плагинов (сверено по вебу, июль 2026)

### 3.1 In-process dylib — ОТКЛОНЕНО

- **Стабильного ABI у Rust нет и не предвидится в горизонте**: RFC crABI
  (rfcs#3470) и RFC `#[export]` (rfcs#3435) открыты с 2023 и не приняты;
  экспериментальная nightly-реализация `export_stable` (rust#134767, смержен
  2025-05-05) не входит в project goals 2025H1/H2.
- **`abi_stable` мёртв** — последний релиз 0.11.3 от 2023-10-12, ноль коммитов
  почти 3 года, PR сообщества (включая C-unwind и фикс RUSTSEC-2024-0014)
  не смержены. Живая альтернатива — `stabby` (ZettaScale, 72.1.8, июнь 2026), но
  это single-vendor решение под нужды Zenoh (лицензия EPL-2.0 OR Apache-2.0).
- **Отрицательный прецедент максимального калибра**: Bevy депрекировал (0.14) и
  удалил (0.15) `bevy_dynamic_plugin` как **unsound** («likely unsound, or at the
  very least so dangerous…», bevy#11969).
- Практика для сторонних авторов требует **lockstep тулчейна** (тот же rustc +
  флаги) либо замороженного C-ABI со всеми ограничениями (никаких String/Vec через
  границу, паника = abort, аллокации не пересекают границу, dll не выгружать).
  На Windows пользователи пересобирать плагины не будут.
- Против и проектный прецедент: ADR 0005 отклонил in-process embed (dll) в пользу
  сайдкара — по причинам изоляции сбоев и тяжести сборки.

### 3.2 WASM in-process — не для наших поверхностей (задел)

Состояние зрелое: wasmtime 46 (Tier 1 на Windows x64; component model, wasi-http,
wasi-sockets — Tier 1; WASI 0.3 с async вышел 2026-06-11), у Rust нативный таргет
`wasm32-wasip2` с 1.82; Zed — эталон плагинов на версионируемом WIT; Extism 1.30 —
turnkey-обвязка (лимиты память/fuel/таймаут, HTTP через host-функцию с
`allowed_hosts`). Но:

- **Вес рантайма**: Zellij в v0.44.0 (март 2026) ушёл с wasmtime на интерпретатор
  `wasmi` ровно ради размера бинарника и отказа от compile+cache — JIT-рантайм
  тяжёл для TUI. Нам пришлось бы вшить wasmtime/Extism в основной exe — против
  духа проекта (ADR 0005: «в основном exe ноль Wasmer»).
- **Ценность песочницы обнуляется задачей**: пользовательским инструментам нужны
  сеть и произвольный I/O — значит, всё равно раздавать capability'и; изоляция
  остаётся, но главный аргумент («безопасно исполнять чужой код») ослабевает,
  а стоимость (свой SDK и toolchain для авторов плагинов, свой ABI поверх байтов)
  остаётся.
- Экосистемы готовых инструментов под наш собственный WASM-ABI не существует —
  каждый инструмент пришлось бы писать специально под нас.

Фиксируем как **задел**: если появится потребность исполнять *недоверенные*
инструменты без установки процессов — Extism/wasmtime + WIT это решает; наш
`wasmer`-сайдкар уже даёт прецедент провизии ассетов.

### 3.3 Встраиваемые скрипты (Rhai / mlua / Steel) — не берём

Живые (Rhai 1.25.1, mlua 0.12.0, Steel 0.8.2), но: инструментам нужны сеть/файлы —
пришлось бы наращивать биндинги под всё; исполнение в нашем процессе без изоляции;
Helix не может смержить Steel-плагины с 2023 (PR #8675 — до сих пор draft).
Для «приватного кода вне монолита» скрипт годится, но impорт LameLLaMA решается
проще (файл), а инструменты — стандартнее (MCP).

### 3.4 Подпроцесс/сайдкар — ВЫБРАНО

Индустрия сходится именно здесь, и все паттерны задокументированы:

- **MCP** (stdio, NDJSON JSON-RPC 2.0) — де-факто стандарт AI-инструментов
  (Anthropic 2024-11; поддержан хостами Claude/OpenAI/Google, goose, Zed, oterm,
  gptme…). Текущая ревизия спеки **2025-11-25**.
- **LSP** (Content-Length-фрейминг) — старший брат; `rust-analyzer` вдобавок даёт
  прецедент «подпроцесс ради ABI-изоляции» (proc-macro-srv).
- **nushell** — плагины-подпроцессы с msgpack/json, регистрацией и idle-GC;
  **HashiCorp go-plugin** — канон долгоживущего сайдкара с handshake и mTLS.
- Windows-нюансы известны и решаемы: `CREATE_NO_WINDOW`; **запрет `.bat`/`.cmd`**
  как команд плагинов (CVE-2024-24576 «BatBadBut» — Rust ≥1.77.2 сам отклоняет
  неэкранируемые аргументы; `npx`-серверы на Windows документировать как
  `cmd /c npx …` либо полный путь к `.cmd` не поддерживать); **Job Object** для
  убийства дерева процессов (наш `windows-sys` уже умеет — песочница Python).
- Готовый фреймворк «go-plugin для Rust» ровно один — **MCP через официальный SDK
  `rmcp`** (2.2.0, июль 2026); вне MCP — «собери сам из tokio+serde_json», что
  проект уже дважды делал (llama-server, wasmer).

## 4. Поверхность «инструменты»: MCP-хост (рекомендация)

### 4.1 Почему MCP, а не свой протокол

- **Экосистема**: тысячи готовых серверов (файлы, git, GitHub, базы, браузер, …) и
  SDK для авторов на всех языках — свой протокол получил бы ноль готовых
  инструментов. Отрицательный прецедент: `llm-functions` (aichat) — добротный
  собственный формат, не распространившийся за пределы родного приложения.
- **Терминальные чат-клиенты уже сделали это** (oterm, gptme, mcp-client-for-ollama)
  — «MCP-клиент из конфиг-файла» стал конвенцией жанра; roadmap проекта уже
  содержит задел «MCP-клиент — естественное расширение ToolRegistry».
- Наш трейт `Tool` отображается на MCP-инструмент **без потерь**: `description` и
  JSON-схема приходят от сервера, `invoke` → `tools/call`, результат-текст →
  `ToolOutcome::text` (эффектов у внешних инструментов нет).

### 4.2 Объём протокола (тесно и стабильно)

Берём **tools-only, stdio-only** подмножество ревизии **2025-11-25** — оно
wire-стабильно с 2024-11-05:

- транспорт: подпроцесс, newline-delimited JSON-RPC 2.0, UTF-8, без вложенных
  переводов строк; **stdout сервера — только протокол**, stderr — логи
  (обязательно дренировать в наш `logs/`, иначе забьётся pipe);
- `initialize` (шлём `protocolVersion: "2025-11-25"`, `capabilities: {}`,
  `clientInfo`; принимаем встречную версию, которую умеем, иначе отключаемся) →
  `notifications/initialized`;
- `tools/list` (+ пагинация `cursor`/`nextCursor`), `tools/call`
  (`isError: true` → текст ошибки модели, не протокольная ошибка);
- `ping` (отвечаем пустым результатом), `notifications/cancelled` (шлём при
  таймауте/отмене), `notifications/tools/list_changed` (перечитать каталог);
- на неподдержанные **запросы** сервера (`sampling/createMessage`,
  `elicitation/create`, `roots/list`) отвечаем `-32601` (иначе корректный сервер
  повиснет в ожидании); неизвестные **нотификации** молча игнорируем;
- shutdown: закрыть stdin → ограниченное ожидание → SIGTERM (unix) → kill;
  на Windows — stdin-close → Job-Object/TerminateProcess.

**Не берём** (заделы): resources, prompts, sampling, roots, elicitation,
HTTP-транспорт, structuredContent-валидацию. Ревизия 2026-07-28 (RC: stateless
core, extensions) имеет stdio-совместимый путь и ≥12-месячные окна депрекации —
ничего предстраивать не нужно; вкладываться в roots/sampling/logging не стоит
(в RC депрекированы).

### 4.3 Реализация клиента: свой микро-клиент vs `rmcp` (развилка Р2)

| | Свой микро-клиент | `rmcp` 2.x |
|---|---|---|
| Объём | ~6 видов сообщений; оценочно 600–900 строк с тестами | `default-features=false, features=["client","transport-child-process"]` |
| Зависимости | **ноль новых** (tokio/serde_json/async-trait уже есть) | rmcp + rmcp-macros + process-wrap 9 + which 8 (+ хвост) |
| Соответствие спеке | сами реализуем питфоллы §4.6 (список известен) | 2.2.0 проходит официальный conformance-suite |
| Стабильность API | наша | churn реален: 1.8.0 → 2.0.0 → 2.1.0 → 2.2.0 за 3 недели (июнь–июль 2026), breaking в минорах |
| i18n/логи | полный контроль (ошибки — в бандлы, ось B) | обёртки поверх английских ошибок |
| Прецедент проекта | свой CLI-парсер (вместо clap), markdown, calc, i18n, SSE-парсеры трёх облаков | — |

**Рекомендация: свой микро-клиент.** Подмножество мало и wire-стабильно с
2024-11-05, новых зависимостей ноль, тексты ошибок локализуемы, и это ровно тот
случай, где проект исторически выбирает микро-реализацию. `rmcp` — запасной путь,
если подмножество начнёт расти (HTTP-транспорт, sampling, tasks) — переход
локализован за трейтом транспорта. Честная оговорка разведки: «genuine toss-up» —
готовая конформность rmcp ценна; решение за пользователем.

### 4.4 Интеграция в архитектуру

**Конфиг** (`shared/config.rs`, всё `#[serde(default)]` — без миграции):

```jsonc
"mcp": {
  "enabled": false,              // мастер-выключатель (по умолчанию ВЫКЛ, как python)
  "servers": [{
    "id": "github",              // slug [a-z0-9-]{1,32} — часть id инструментов
    "command": "C:/tools/github-mcp.exe",
    "args": ["--stdio"],
    // Переменная ребёнку → ИМЯ переменной-источника в окружении приложения
    // (секрет не пишется в settings.json — прецедент api_key_env; см. Р8):
    "env": { "GITHUB_TOKEN": "MINDFORK_GITHUB_PAT" },
    "enabled": true,
    "tool_timeout_secs": 60,     // per-call; startup-таймаут отдельный (дефолт 30с)
    "max_result_chars": 20000    // клип результата (прецедент Claude Code: cap 25k токенов)
  }]
}
```

**Жизненный цикл — `McpManager`** (зеркало `EngineManager`,
`app/orchestrator/mcp.rs`): спавн включённых серверов на старте/при правке
настроек (через `RestartQueue`-дебаунс, как движки); монитор-задача с
`exited`-токеном (паттерн `managed.rs`); handshake + `tools/list` → **динамический
каталог**; статус per-server (`Ready`/`Connecting`/`Disconnected(reason)`).
Рестарт-бюджет как у VS Code LSP: N падений за окно → `Disconnected` до ручного
вмешательства (без вечного рестарт-цикла).

**Регистрация инструментов**: обёртка `McpTool` реализует `Tool`
(`invoke` → `tools/call` с таймаутом; `description`/`parameters` — снимок из
`tools/list`, **не локализуются** — граница i18n, как probe-ошибки движка);
`group()` → новая `ToolGroup::Plugins`; `enabled_by_default() = false` (opt-in
per profile, `reconcile_tools` не включает — автоматически, их нет в
`default_tool_ids`). Реестр у оркестратора уже пересобирается на правки настроек —
MCP-инструменты добавляются в него по готовности сервера (ещё одна причина
пересборки), ход берёт актуальный снимок.

**Именование**: `mcp__<server>__<tool>` (конвенция Claude Code; goose —
`server__tool`). Спека (SEP-986) допускает `A-Za-z0-9_-.` до 128 символов —
нормализуем под лимиты провайдеров function-имён: `.` → `_`, итог ≤ 64 символов
(усечение + короткий hex-хвост от полного имени; точные лимиты провайдеров сверить
при реализации — исторически `^[a-zA-Z0-9_-]{1,64}$` у OpenAI/Anthropic).
Id хранится в `Profile.enabled_tools` как обычная строка — контракт профиля не
меняется.

**Точки интеграции, требующие внимания** (главные «ripples»):

1. **`CATALOG` статичен** (LazyLock от `standard_registry`) — динамические
   инструменты в него не попадают. Тумблеры профиля (`tool_catalog()` в
   `screens/settings`) и `effective_tool_ids` должны получить динамическую
   добавку: снимок MCP-каталога едет в `AppEvent::Settings` (оркестратор его
   знает), гейт — по префиксу `mcp__` + `config.mcp.enabled` (новая ветка в
   `effective_tool_ids`, не `CATALOG`-lookup).
2. **Гейт-тесты i18n не задеты**: `all_tool_descriptions_localized_to_en`
   итерирует статический `CATALOG` — MCP-инструментов там нет по построению.
3. **Отмена**: agentic-loop сейчас await'ит `registry.invoke` без `select!` с
   generation-cancel — долгий `tools/call` заблокировал бы Esc. В Фазе MCP —
   per-call таймаут обязателен; прокидывание `CancellationToken` в `ToolContext`
   (и `select!` в петле) — маленький отдельный рефактор, полезный и родным
   инструментам (`web_search` тоже не отменяем сегодня). Включить в объём.
4. **Лента**: `present.rs` уже имеет generic-ветку (Markdown/Plain) — результаты
   MCP-инструментов рендерятся ею без правок; не-текстовые блоки результата
   (image/audio/resource) в Фазе 1 — текстовая пометка `[изображение …]`.
5. **UI настроек**: секция «Плагины» (или группа в «Инструментах»): чипы статуса
   серверов (паттерн `ServerStatuses`), тумблеры enable per-server, **просмотр
   полного описания инструментов** (антидот tool-poisoning, §4.5). Добавление
   сервера — правкой `settings.json` (Р6).
6. **Контекст-бюджет — главное системное ограничение.** Схемы 5–6 серверов ≈ 15k+
   токенов; хосты вводят капы (Cursor — 40 инструментов, VS Code — 128). Для наших
   локальных 8–16k-контекстов спасает уже существующая механика: **per-profile
   opt-in каждого инструмента** (включай 3 нужных, а не 40). Плюс потолок
   инструментов на сервер (конфиг, дефолт ~25) с предупреждением. Deferred-схемы
   («tool search», как в Claude Code) — задел.

### 4.5 Безопасность (по официальным best practices + Invariant Labs)

Модель угроз: MCP-сервер = **произвольная программа с правами пользователя**
(эквивалент установки софта; песочницы в Фазе 1 нет — как и у goose/Zed/Claude
Code), плюс **tool poisoning** — инъекция инструкций через описания инструментов
(OWASP MCP03:2025), rug-pull (описание меняется после одобрения), cross-server
shadowing, отравление любых полей схемы и результатов.

Митигации в объёме Фазы MCP:

- мастер-выключатель `mcp.enabled=false` + серверы конфигурируются только
  пользователем (полная команда видна в конфиге/UI); инструменты **выключены в
  профилях по умолчанию** (двойной opt-in);
- **TOFU-пиннинг**: hash (имя+описание+схема) каталога сервера при первом
  одобрении; изменение → сервер помечается «каталог изменился, переподтвердите»
  (rug-pull-детектор; Invariant Labs / mcp-scan рекомендация);
- показ **полного** описания инструмента в UI настроек (не усечённого);
- клип результата (`max_result_chars`) и таймауты (`tool_timeout_secs`) — вход в
  промпт ограничен; stderr сервера — только в файловый лог;
- запрет `.bat`/`.cmd` команд (BatBadBut), `CREATE_NO_WINDOW`, Job Object
  (дерево процессов не переживает выход приложения);
- annotations (`readOnlyHint`/`destructiveHint`) показываем в UI, но считаем
  **недоверенными** (позиция спеки). Per-call подтверждение деструктивных вызовов
  — сознательно НЕ в первой фазе (новая модальность посреди генерации; двойного
  opt-in + TOFU достаточно для старта) — задел, вписывается в существующий попап
  подтверждения (`ConfirmAction`).

Позиция согласуется со spec.md §13.1 («умеренные требования» к prompt injection) —
но описания инструментов идут в системный промпт каждого хода, поэтому минимум
(двойной opt-in, видимость описаний, TOFU) — обязателен.

### 4.6 Известные питфоллы реализации (чек-лист для Фазы)

Из полевого опыта хостов (issue-треки VS Code/Codex/Claude Code): серверы,
пишущие мусор в stdout (баннеры/`print`) → **пропускать не-JSON строки с warn**,
а не рвать соединение; недренированный stderr блокирует сервер; `npx`/`uvx` — это
`.cmd`-шимы, на Windows без шелла — `ENOENT`; убийство только прямого потомка
оставляет сирот (нужен Job Object/process group); часть серверов не выходит по
stdin-close (эскалация до kill); id запросов уникальны и не-null, на нотификации
не отвечать; `_meta`-ключи с префиксом `modelcontextprotocol/mcp` не изобретать;
стартовая латентность серверов-«качалок» — отдельный startup-таймаут.

### 4.7 Тестирование

Транспорт за мини-трейтом (`spawn` отделён от чтения/записи) → юнит-тесты
хендшейка/фрейминга/питфоллов на `tokio::io::duplex` без процессов; функциональные
— на крошечном тестовом сервере-скрипте (готовый бинарь в тестах, как
короткоживущие процессы в тестах `managed.rs`); живой `#[ignore]`-смоук — против
реального стороннего сервера (по env-переменной с командой), плюс e2e на живой
модели (§7, зонд).

## 5. Поверхность «импортёры»: нейтральный формат + внешние конвертеры (рекомендация)

### 5.1 Почему не «плагин-процесс»

Импорт — разовая batch-операция: протоколом служит **файл**. Ровно так устроены
зрелые прецеденты: beancount 3.x вынес импортёры из ядра (beangulp: приватные
конвертеры эмитят документированный ledger-текст), KeePass (документированные
XML/CSV + Generic CSV Importer, конвертируют третьи стороны), Netscape bookmarks
HTML (30 лет универсальной границы импорта). Обратный пример — ChatGPT
`conversations.json`: недокументированное **дерево** узлов, которое каждый
потребитель заново учится сплющивать. Вывод: формат обмена должен быть **плоским
и документированным**.

### 5.2 Дизайн

- **Документированный формат** `mindfork-import.json` (в `docs/import-format.md`):

```jsonc
{
  "format": "mindfork-import", "version": 1,
  "profiles": [{
    "key": "assistant-anna",          // стабильный внешний ключ (для UUIDv5-идемпотентности)
    "name": "Anna", "language": "ru",
    "system_message": "…", "greeting": null,
    "character_names": { "user": "…", "assistant": "…" },
    "sampling": { "temperature": 0.8 }   // поддержанное подмножество; лишнее отбрасывается
  }],
  "chats": [{
    "key": "conv-123", "profile_key": "assistant-anna",
    "title": "…", "created_at": "…", "modified_at": "…",
    "system_message": "…",
    "messages": [{ "role": "user|assistant|system", "text": "…",
                    "thoughts": null, "timestamp": "…" }]
  }],
  "settings": { "sampling": {…}, "interface": {…} }   // опционально
}
```

- **Generic CLI**: `mindfork import <file.json>` — валидация (версия формата,
  downgrade-guard как у схем данных), маппинг в доменные сущности,
  **идемпотентность** через детерминированные UUIDv5 от `key` (в точности текущая
  механика `PROFILE_NAMESPACE`), upsert в `Storage`. Санитизация — как у
  `import-lamellama` (отбрасывание неподдержанного семплинга, BOM и т.п.).
- **Приватный конвертер** `lamellama2mindfork` — отдельный **приватный**
  репозиторий (язык любой; можно перенести готовые wire-типы из
  `features/migration.rs` как есть): читает каталог LameLLaMA → эмитит
  `mindfork-import.json`. Всё знание о непубличной программе покидает монолит.
- Формат заодно открывает импорт **из чего угодно** (ChatGPT-экспорт, SillyTavern
  JSONL, попытка №1) — конвертеры пишутся без участия монолита.

### 5.3 Альтернатива (отклонено): формат обмена = доменные `Profile`/`Chat` as-is

За: сериализация уже есть, версии/миграции схем уже есть (ADR 0006). Против
(решающее): внешний контракт зафиксировал бы **внутреннюю** схему целиком
(`deleted`, `reflected_upto`, `tool_calls`, метаданные…) — каждый рефактор домена
становился бы breaking-изменением для чужих конвертеров. Выделенный формат-v1 —
маленький, стабильный, ничего лишнего не обещает.

### 5.4 Судьба `import-lamellama` (развилка Р4)

Рекомендация: **удалить** из монолита в той же фазе (CHANGELOG: рубрики Удалено +
Добавлено `import`), команда `import-lamellama` подсказывает про конвертер.
Альтернатива — оставить депрекированной на 1–2 релиза (двойной код-путь).
Идемпотентность переживает переход: конвертер эмитит те же стабильные ключи
(имена конфигураций / id разговоров), UUIDv5-неймспейс переезжает в спецификацию
формата.

## 6. Поверхность «облачные интеграции»: OpenAI-совместимый endpoint = plugin-API (рекомендация)

- **Выносить `EngineBackend` за процесс = изобрести HTTP-сервер инференса.** Он
  изобретён: OpenAI-совместимый протокол; наш режим **external уже есть**
  (url + `api_key_env` + `model_name`) — это и есть точка подключения провайдеров.
- Индустрия делает ровно так: aichat (`openai-compatible`-тип клиента), LibreChat
  («custom endpoints»), Open WebUI Pipelines (плагин-граница — сам OpenAI-формат).
  Мосты-мультипликаторы: **LiteLLM proxy** (self-hosted, ядро MIT, 100+
  провайдеров за одним endpoint) и **OpenRouter** (hosted, 400+ моделей). Один
  external-слот покрывает «любую экзотику» через них.
- Провайдеры с собственным протоколом и уникальными возможностями (мысли, подписи
  рассуждений, effort) добавляются **нативной реализацией трейта в монолите** —
  как уже случилось с Responses/Gemini/Anthropic (ADR 0004). Это осознанно НЕ
  плагины: качество интеграции (стриминг «мыслей», tool-use round-trip) требует
  глубокого сцепления с контрактом.
- Что сделать в рамках направления (малый объём): **документировать паттерн** в
  install.md (рецепты LiteLLM/OpenRouter для external-режима). Опция сверх того
  (развилка Р5): **managed-custom-command** — супервайзер умеет поднимать
  *произвольную команду* как OpenAI-совместимый сайдкар (обобщение
  `ManagedConfig`: command+args вместо llama-server-специфики; probe `/health`
  уже толерантен к 404). Даёт «локальный прокси поднимается сам». Рекомендация:
  отложить до реального спроса — external + вручную запущенный прокси покрывает
  сценарий уже сегодня.

## 7. Фазовый план

Направление «плагины», этапы = отдельные ветки/PR (AGENTS.md §1–2):

- **Этап 1 — `feat/generic-import`** (самостоятельная ценность, закрывает цель №1):
  формат `mindfork-import.json` v1 + `docs/import-format.md`; CLI `mindfork import`
  (свой парсер уже умеет подкоманды; i18n-тексты в бандлы); удаление
  `import-lamellama` (по Р4) — знание о LameLLaMA уезжает в приватный конвертер
  (пишется вне этого репозитория). Тесты: маппинг/идемпотентность/валидация/
  golden-файл формата. Живой прогон движка не требуется (файловая операция);
  ручная проверка на реальном экспорте (6 профилей / 226 чатов — прецедент M9).
- **Этап 2 — `spike/mcp-client`** (зонд, go/no-go): мини-клиент stdio (initialize/
  tools list/call/ping/shutdown) + временная регистрация инструментов одного
  сервера; **критерий GO**: живая локальная модель (Gemma 4) корректно вызывает
  MCP-инструмент (например, `filesystem`-сервер) и использует результат; схемы
  1–2 серверов не разваливают 16k-контекст; Esc/таймаут не подвешивают ход.
  NO-GO-путь: остаёмся на нативных инструментах, MCP в задел (зонд дешёвый).
- **Этап 3 — `feat/mcp-host`** (после GO; вероятно, 2 PR):
  3a — клиент/`McpManager`/конфиг/гейты/отмена-таймауты/тесты;
  3b — UI (секция «Плагины», чипы статусов, тумблеры per-profile, просмотр
  описаний, TOFU-переподтверждение), i18n-хром, доки (spec §9, architecture §8,
  README, install), CHANGELOG. По завершении — ADR «Плагины: MCP-хост +
  формат обмена импорта».
- **Этап 4 (опц., по Р5)** — install.md рецепты LiteLLM/OpenRouter;
  managed-custom-command, если подтверждён спрос.
- **Заделы** (в roadmap): HTTP-транспорт MCP, resources/prompts, per-call
  подтверждение деструктивных инструментов, deferred-схемы («tool search»),
  UI-редактор серверов, WASM-песочница для недоверенных инструментов,
  server `instructions` → системный промпт.

## 8. Развилки

> **Решение пользователя (2026-07-17): все развилки Р1–Р8 приняты по
> рекомендациям.** Направление стартует с этапа 1 (`feat/generic-import`).

- **Р1. Механика инструментов-плагинов**: **MCP-хост stdio (рекомендация)** |
  собственный JSON-RPC-протокол | in-process WASM (Extism). Рекомендация — MCP:
  экосистема + конвенция жанра; свой протокол = ноль готовых инструментов.
- **Р2. Реализация MCP-клиента**: **свой микро-клиент (рекомендация)** | `rmcp`
  (`default-features=false`). См. таблицу §4.3; честно — близкий выбор.
- **Р3. Формат обмена импорта**: **выделенный `mindfork-import.json` v1
  (рекомендация)** | доменные сущности as-is (§5.3, отклонить).
- **Р4. Судьба `import-lamellama`**: **удалить в этапе 1 (рекомендация)** |
  депрекировать на 1–2 релиза. Приватный конвертер в любом случае живёт вне
  этого репозитория.
- **Р5. Облака**: **только документация паттерна external+LiteLLM/OpenRouter
  (рекомендация)** | + managed-custom-command (сайдкар-прокси поднимает
  супервайзер) | plugin-API движка (отклонить, §6).
- **Р6. Конфигурирование MCP-серверов**: **секция в `settings.json`, правка
  руками; в UI — статусы/тумблеры/просмотр описаний (рекомендация)** | отдельный
  `mcp.json` (не перезаписывается приложением, но двойной источник) | полный
  UI-редактор сразу (дорого, задел).
- **Р7. Строгость безопасности Фазы MCP**: **мастер-гейт выкл-по-умолчанию +
  двойной opt-in + TOFU-пиннинг каталога + клипы/таймауты (рекомендация)** |
  минимум без TOFU | максимум с per-call подтверждением деструктивных (отложить
  в задел — новая модальность посреди генерации).
- **Р8. Секреты/окружение серверов**: **наследовать окружение приложения + карта
  `env` с *именами* переменных-источников (прецедент `api_key_env`; сами секреты
  не в `settings.json`) (рекомендация)** | чистое окружение с явным passthrough
  (строже, но ломает PATH-зависимые серверы; честно: скрытие env от процесса,
  исполняемого с правами пользователя, — театр безопасности: файлы он и так
  прочитает).

## 9. Источники

MCP: [спека 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)
(transports / [lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) /
[tools](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) /
[security best practices](https://modelcontextprotocol.io/specification/2025-11-25/basic/security_best_practices) /
[changelog](https://modelcontextprotocol.io/specification/2025-11-25/changelog));
[RC 2026-07-28](https://blog.modelcontextprotocol.io/posts/2026-07-28-release-candidate/);
[rmcp (crates.io)](https://crates.io/crates/rmcp) /
[rust-sdk releases](https://github.com/modelcontextprotocol/rust-sdk/releases);
[Invariant Labs: tool poisoning](https://invariantlabs.ai/blog/mcp-security-notification-tool-poisoning-attacks);
[OWASP MCP Top 10 — MCP03](https://owasp.org/www-project-mcp-top-10/2025/MCP03-2025%E2%80%93Tool-Poisoning);
[CyberArk: poison everywhere](https://www.cyberark.com/resources/threat-research-blog/poison-everywhere-no-output-from-your-mcp-server-is-safe).
Хосты: [goose extensions design](https://block.github.io/goose/docs/goose-architecture/extensions-design/) /
[конфиг](https://deepwiki.com/block/goose/5.3-extension-types-and-configuration);
[Zed context servers](https://zed.dev/docs/assistant/model-context-protocol);
[oterm MCP](https://ggozad.github.io/oterm/mcp/);
[Claude Code MCP](https://code.claude.com/docs/en/mcp);
«слишком много инструментов»: [Cursor 40](https://forum.cursor.com/t/tools-limited-to-40-total/67976),
[VS Code 128](https://github.com/microsoft/vscode/issues/290356),
[15k токенов схем](https://demiliani.com/2025/09/04/model-context-protocol-and-the-too-many-tools-problem/).
Механики: [libloading](https://crates.io/crates/libloading);
[abi_stable (мёртв)](https://github.com/rodrimati1992/abi_stable_crates);
[stabby](https://github.com/ZettaScaleLabs/stabby);
[RFC crABI #3470](https://github.com/rust-lang/rfcs/pull/3470) /
[RFC #[export] #3435](https://github.com/rust-lang/rfcs/pull/3435) /
[export_stable rust#134767](https://github.com/rust-lang/rust/pull/134767);
[Bevy: dynamic plugins unsound](https://github.com/bevyengine/bevy/issues/11969);
[wasmtime tiers](https://docs.wasmtime.dev/stability-tiers.html) /
[WASI 0.3](https://bytecodealliance.org/articles/WASI-0.3);
[Extism](https://github.com/extism/extism);
[Zed extensions (WASM/WIT)](https://zed.dev/blog/zed-decoded-extensions);
[Zellij → wasmi v0.44.0](https://github.com/zellij-org/zellij/releases/tag/v0.44.0);
[LSP base protocol](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/);
[nushell plugin protocol](https://www.nushell.sh/contributor-book/plugin_protocol_reference.html);
[go-plugin](https://github.com/hashicorp/go-plugin);
[BatBadBut / Rust 1.77.2](https://blog.rust-lang.org/2024/04/09/cve-2024-24576.html);
[CREATE_NO_WINDOW](https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags);
[Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects).
Прецеденты: [llm-functions](https://github.com/sigoden/llm-functions);
[llm CLI plugin hooks](https://llm.datasette.io/en/stable/plugins/plugin-hooks.html);
[LibreChat custom endpoints](https://www.librechat.ai/docs/quick_start/custom_endpoints);
[Open WebUI Pipelines](https://docs.openwebui.com/features/extensibility/pipelines/);
[LiteLLM](https://github.com/BerriAI/litellm); [OpenRouter](https://openrouter.ai/);
[beangulp (импортёры beancount)](https://github.com/beancount/beangulp);
[KeePass import/export](https://keepass.info/help/base/importexport.html);
[спека character card v2](https://github.com/malfoyslastname/character-card-spec-v2);
[разбор ChatGPT conversations.json](https://community.openai.com/t/decoding-exported-data-by-parsing-conversations-json-and-or-chat-html/403144).
