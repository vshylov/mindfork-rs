# Исследование: Python-песочница на Wasmer (WASIX) + вынос в отдельную библиотеку

**Статус:** исследование (2026-07-11) + **Фаза 0 (спайк) пройдена на Windows —
вердикт GO** (см. §9). **Решение по поставке (2026-07-11): вариант B — бандлить
готовый `wasmer` CLI сайдкаром** за контрактом `shared/sandbox.rs` (embed V8 в
собственную dll отклонён из-за LLVM/libclang + статик-V8 в нашей сборке, §9.7);
полный embed-спайк отложен как неактуальный. Заказ: у `python_exec` должно быть два
режима — **локальный интерпретатор** (как сейчас) и **изолированная среда** на Wasmer
с предустановленными пакетами (numpy, requests и т.п.) для относительно сложных задач;
**по умолчанию — режим Wasmer**. Код изолированной среды не должен жить в основном
исполняемом файле — отдельный **артефакт рядом с бинарником** (по решению — сам
`wasmer.exe`/`wasmer`, а не dll; дух заказа «ноль Wasmer в основном exe, отдельный
файл рядом» сохранён).

Связанные документы: [spec §9.3, §13.2](../../spec.md) (текущий `python_exec` и его
позиция безопасности), [ADR 0004](../decisions/0004-engine-contract-multi-provider.md)
(паттерн «новая реализация за существующим трейтом/контрактом»),
[docs/install.md](../install.md).

---

## 1. Задача и текущее положение

Сегодня `python_exec` ([features/tools/python.rs](../../src/features/tools/python.rs)) —
это запуск **системного** Python отдельным процессом (`python -c <code>`, stdin
закрыт, stdout/stderr в pipe, таймаут 10 с через `kill_on_drop`, UTF-8 через
`PYTHONIOENCODING`/`PYTHONUTF8`). Изоляции нет вообще: код получает полный доступ к
ФС/сети/процессам от имени пользователя. Именно поэтому глобальный выключатель
`tools.python_enabled` **выключен по умолчанию** (spec §13.2 — «на Windows нет
OS-песочницы»), а сам инструмент требует установленного на машине Python.

Чего хотим:

1. **Режим Wasmer (дефолт)** — Python внутри WebAssembly-песочницы: без доступа к
   хост-ФС, сеть — по явному разрешению, предустановленные пакеты (numpy, pandas,
   requests, …), не требует Python на машине.
2. **Режим Local** — прежнее поведение (системный интерпретатор, `python_path`).
3. **Песочница — в отдельной dll/so** рядом с бинарником: тяжёлый wasm-рантайм не
   раздувает основной exe и подгружается только когда нужен.

---

## 2. Ландшафт «Python в WASM» (сверено по вебу, июль 2026)

### 2.1 Wasmer + WASIX — единственный путь, дающий и numpy, и сеть

**WASIX** — расширение WASI preview1 от Wasmer (треды, полноценные сокеты, fork,
setjmp/longjmp, с 7.0 — динамическая линковка). Единственный рантайм с поддержкой
WASIX — сам Wasmer ([wasix.org](https://wasix.org/docs/)).

Состояние на июль 2026:

- **Wasmer 7.0 (30.01.2026)** — переломный релиз: **динамическая линковка
  (dlopen/dlsym) в WASIX** + libffi (ctypes). До этого работал только чистый
  интерпретатор; теперь работают нативные C-расширения — numpy, pydantic и т.д.
  Плюс экспериментальный **async API** (полный asyncio; SQLAlchemy, greenlet),
  бэкенды singlepass/cranelift/LLVM, ускорена компиляция LLVM (python.wasm ~90 с →
  ~10 с). ([блог Wasmer 7](https://wasmer.io/posts/wasmer-7),
  [heise](https://www.heise.de/en/news/WebAssembly-Wasmer-7-0-brings-experimental-async-support-for-Python-11163325.html),
  [InfoWorld](https://www.infoworld.com/article/4125985/wasmer-beefs-up-python-support.html))
- **CPython 3.13** (форк [wasix-org/cpython](https://github.com/wasix-org/cpython));
  3.12 — legacy-сборка «только pure-python». 3.14 в роадмапе. Распространяется
  пакетом **`python/python`** в реестре Wasmer (формат webc):
  `wasmer run python/python --dir=. -- script.py`.
  ([Python on the Edge](https://wasmer.io/posts/python-on-the-edge-powered-by-webassembly))
- **Индекс нативных WASIX-колёс** — <https://pythonindex.wasix.org/> (pip-совместимый
  `/simple`): ~64 пакета, среди них **numpy 2.4.0.dev0, pandas 2.3.2,
  cryptography 45.0.4, pillow 11.3.0, matplotlib 3.10.6, aiohttp 3.13.2**. В сборке
  окружения — OpenSSL 3.5.1, zlib, libffi, sqlite, libjpeg/png/webp и др.
  ([wasix-org/build-scripts](https://github.com/wasix-org/build-scripts); репозиторий
  14.04.2026 архивирован в пользу преемника
  [wasinix](https://github.com/wasix-org/wasinix) — инфраструктура сборки жива, но в
  движении). **scipy пока нет**; polars/PyTorch/curl_cffi анонсированы как «скоро».
- **requests** в индексе отсутствует, потому что он **чистый Python** (как и urllib3,
  certifi, idna, charset-normalizer) — ставится обычным колесом с PyPI. Ему нужны
  сокеты (есть в WASIX) и `ssl` (OpenSSL в сборке есть; наличие aiohttp с TLS в
  индексе — хороший знак). Подтвердить живым прогоном (§6, Фаза 0).
- **Сеть — явный opt-in рантайма.** WASIX-сокеты не проброшены в хост по умолчанию:
  в CLI — флаг `--net`, при embedding — своя реализация `virtual-net` (host
  passthrough включаем сами). То есть песочница **по построению без сети**, пока мы
  её не дали. ([Wasmer sandbox post](https://wasmer.io/posts/edgejs-safe-nodejs-using-wasm-sandbox))
- **ФС — полностью виртуальная**: гость видит только то, что мы примонтировали
  (webc-ФС пакета + явные preopen/mapdir). Историческая уязвимость CLI ≤4.2.3
  ([GHSA-4mq4-7rw3-vm5j](https://github.com/wasmerio/wasmer/security/advisories/GHSA-4mq4-7rw3-vm5j) —
  cwd монтировался по умолчанию) embedding'а с явными preopen не касается; наша
  политика — **ноль хост-каталогов по умолчанию** (§5.5).

### 2.2 Альтернативы (рассмотрены и отклонены)

| Вариант | Почему нет |
|---|---|
| **Официальный CPython WASI** (tier 2, wasmtime) | WASI p1: нет сокетов, тредов, dlopen → нет ни requests, ни numpy. C-расширения — только экспериментально ([wasi-wheels](https://github.com/dicej/wasi-wheels) unmaintained; [numpy не заводится](https://github.com/dicej/wasi-wheels/issues/4) на обычном wasmtime; [numpy#25859](https://github.com/numpy/numpy/issues/25859) — WASI-сборка numpy не приоритет) |
| **Pyodide** | Самый зрелый набор пакетов, но это emscripten: [работает только в браузере/Node](https://github.com/pyodide/pyodide/issues/558), [в wasmtime/wasmer не запускается](https://github.com/pyodide/pyodide/discussions/5145). Тащить JS-движок в TUI-приложение — нет |
| **py2wasm** (Nuitka→wasm) | Компилятор *приложений* в wasm, не интерпретатор для произвольного кода инструмента |
| **RustPython / MicroPython (wasm)** | Нет CPython C-API → нет numpy/pandas; неполная стдлиба |
| **wasmtime как рантайм** | Лучшее в индустрии прерывание (epoch/fuel), но WASIX он не исполняет — а без WASIX нет ни сокетов, ни готовых нативных колёс |
| **Контейнеры (docker/podman/WSL)** | Тяжёлая внешняя зависимость и администрирование; против духа портативного TUI «положил рядом и работает» |

**Вывод:** под связку требований «numpy + requests + изоляция + Windows/Linux +
встраиваемость в Rust» Wasmer/WASIX сегодня безальтернативен.

### 2.3 Embedding из Rust

- Крейты: `wasmer` 7.x + **`wasmer-wasix` 0.702.0** (30.06.2026) + `webc` 12
  ([crates.io](https://crates.io/crates/wasmer-wasix), [lib.rs](https://lib.rs/crates/wasmer-wasix)).
  Ключевые фичи: `sys-thread` (пул потоков WASIX — python.wasm многопоточный),
  бэкенды `cranelift`/`llvm`/`singlepass`, `host-vnet` (проброс сети; включаем по
  тумблеру), `host-fs` (нам **не** нужен — ФС только виртуальная).
- API: `WasiEnvBuilder` / runner для webc-пакетов (`BinaryPackage`), захват
  вывода — `Pipe::channel()` в `stdout`/`stderr`, монтирование — virtual-fs
  (webc-fs пакета + наши каталоги), сеть — реализация `virtual-net`.
  Windows-хост заявлен (фича `windows-sys`; CLI официально собирается под Windows),
  но связку «python + динлинковка на Windows-хосте» надо подтвердить смоуком.
- **Кэш компиляции:** первый запуск компилирует python.wasm (десятки МБ; cranelift —
  секунды, LLVM — ~10 с) → обязательно `Module::serialize`/`deserialize`
  (или `wasmer-cache`) в `data/sandbox/cache/` — дальше загрузка почти мгновенная.
- **Прерывание — слабое место Wasmer.** Аналога epoch-interruption wasmtime в ядре
  Wasmer нет ([старый запрос #337](https://github.com/wasmerio/wasmer/issues/337),
  [обсуждение](https://users.rust-lang.org/t/interrupt-wasmer-wasmtime/60331));
  штатные пути: (а) `terminate` WASIX-процесса — сигнал доставляется на
  syscall-границах, чистый CPU-цикл (`while True: pass`) может не прерваться;
  (б) metering middleware (`wasmer-middlewares`) — детерминированный лимит
  инструкций, но накладные расходы и непроверенная совместимость с динлинковкой.
  Подробно — §4.1.

---

## 3. Предлагаемая архитектура

> **ПРАВКА ПОСЛЕ ФАЗЫ 0 (решение 2026-07-11):** выбран **вариант B — сайдкар
> `wasmer` CLI** (§9.7), а не embed V8 в cdylib. Поэтому §3.1 ниже
> (**cdylib со встроенным рантаймом**) — **отклонённая альтернатива A**, оставлена
> для истории. Актуальная поставка — **§3.1-B**. Разделы §3.2–§3.5 (раскладка на
> диске, конфиг/инструмент/UI, ассеты, политика песочницы) **остаются в силе** —
> они не зависят от способа исполнения (трейт `shared/sandbox.rs` их прячет).

### 3.1-B Сайдкар: бандленный `wasmer` за контрактом `shared/sandbox.rs` (ВЫБРАНО)

Рядом с приложением кладётся готовый бинарь **`wasmer.exe` / `wasmer`** (v7.2+, со
встроенным V8+WASIX — проверено, §9). Основной exe **не содержит ни байта Wasmer**;
песочница — отдельный процесс, запускаемый по требованию (как текущий Local-режим
шелит `python`, только теперь — свой `wasmer run` с политикой изоляции).

- **Контракт `shared/sandbox.rs`** (за трейтом, mock в тестах — паттерн
  `EngineBackend`): поиск бинаря рядом с exe (override `MINDFORK_SANDBOX_WASMER`),
  запуск `tokio::process` с `--v8`, монтированием ассетов (`--volume HOST:/sp`,
  `PYTHONPATH=/site-packages`), сетью по тумблеру (`--net`), таймаутом и **kill по
  таймауту** (`kill_on_drop` + `taskkill /T` на Windows — доказанно чистый, §9.1).
  Код инструмента передаётся **файлом** (пишем во временный смонтированный каталог и
  запускаем `python /w/job.py`) — так избегаем экранирования `-c` (см. §9.4).
- **Враппер кода** (генерируем мы): в начало — **шим `setsockopt`** (§9.3, иначе
  requests/urllib падают) + установка `PYTHONPATH`/`sys.path` при необходимости.
- **Прерывание — процессным kill** (главный плюс B): чистый, ~360 мс, без висящих
  процессов (§9.1) — снимает риск №1 (§4.1) целиком, metering/воркер-эскалация не
  нужны.
- **Изоляция сбоев**: паника/OOM рантайма живут в сайдкар-процессе — TUI не падает.
- **Вес поставки**: `wasmer` бинарь (~50–100 МБ) рядом — как задел `dictionaries/`;
  качается тем же `mindfork sandbox setup` (§3.4), в репо не хранится.
- **ABI/воркер не нужны** — процесс *и есть* граница; `libloading`/cdylib/`extern "C"`
  из варианта A не вводим.

### 3.1 (A, ОТКЛОНЕНО) Отдельная библиотека `mindfork-sandbox` (cdylib)

Проект сейчас — один пакет ([Cargo.toml](../../Cargo.toml)). Переходим на
**workspace**:

```
Cargo.toml            # [workspace] members = [".", "crates/mindfork-sandbox"]
crates/mindfork-sandbox/
  Cargo.toml          # crate-type = ["cdylib"]; deps: wasmer, wasmer-wasix, webc, …
  src/lib.rs          # extern "C" ABI + рантайм песочницы
```

Основной пакет **не зависит** от sandbox-крейта — в exe не попадает ни байта
Wasmer; dll собирается отдельной целью. Каталог `target/` у workspace общий, так что
в dev `mindfork_sandbox.dll` появляется **рядом с exe автоматически**
(`target/debug/`); в релизе — просто второй файл в архиве (как `dictionaries/`).

**Почему dll, а не воркер-exe:** так заказано; плюсы — один процесс, нет IPC-схемы,
простой обмен. Минусы честно фиксируем: паника/OOM wasm-рантайма происходит в
процессе приложения (гасим `catch_unwind` + лимитом памяти Store), незавершаемый
поток утекает (§4.1). **ABI спроектирован так, что реализацию позже можно пересадить
на воркер-процесс, не меняя хост-код** — dll сама начнёт спавнить воркера, если
живые тесты покажут, что terminate недостаточно.

**C ABI** (Rust ABI нестабилен между компиляторами/версиями; обмен — JSON-строки,
это уже конвенция инструментов):

```c
uint32_t mfsb_abi_version(void);                     // = 1; несовпадение мажора → не грузимся
void*    mfsb_init(const char* config_json);         // пути к ассетам, лимиты; NULL при ошибке
char*    mfsb_exec(void* ctx, const char* req_json); // блокирующий вызов; JSON-ответ
void     mfsb_cancel(void* ctx, uint64_t job_id);    // из другого потока
void     mfsb_free(char* ptr);                       // освобождение строк ответа
void     mfsb_shutdown(void* ctx);
```

Запрос: `{job_id, code, timeout_ms, net: bool}`; ответ: `{stdout, stderr, exit_code,
timed_out, error?}`. Внутри dll: `catch_unwind` на каждой extern-границе (паника
через FFI — UB), собственный мини-рантайм задач (`wasmer-wasix` TaskManager; tokio
приложения не трогаем), логи — в `logs/sandbox.log` (stdout занят TUI — конвенция
проекта).

**Хост-сторона:** новый `shared/sandbox.rs` — поиск библиотеки рядом с бинарником
(`mindfork_sandbox.dll` / `libmindfork_sandbox.so`; override `MINDFORK_SANDBOX_LIB`),
загрузка `libloading`, проверка `mfsb_abi_version`, безопасная обёртка
`SandboxClient` **за трейтом** (mock в тестах — паттерн `EngineBackend`). Вызов из
инструмента: `spawn_blocking` + `tokio::time::timeout` + `mfsb_cancel` по таймауту.
Новая зависимость основного пакета — только `libloading` (крошечная).

### 3.2 Раскладка на диске

```
mindfork.exe
mindfork_sandbox.dll          # рядом с бинарником (как dictionaries/)
data/sandbox/                 # за shared/paths.rs (portable/system/path)
  python.webc                 # CPython 3.13 WASIX (пакет python/python)
  site-packages/              # предустановленные пакеты (§3.4), read-only mount
  cache/                      # сериализованные скомпилированные модули
```

Нет dll или ассетов → инструмент отвечает понятным текстом («песочница не
установлена: … запустите `mindfork sandbox setup`»), приложение не падает —
паттерн `UnavailableEmbedder`.

### 3.3 Конфиг, инструмент, UI

- `shared/config.rs`: `PythonMode { Wasmer, Local }` (serde kebab-case,
  **default = Wasmer**); `ToolSettings` +=
  `python_mode: PythonMode`,
  `python_net_enabled: bool` (сеть внутри песочницы),
  `python_wasm_timeout_secs: u64` (default **30** — wasm-интерпретация в ~2–5×
  медленнее нативной; прежние 10 с оставляем Local-режиму). Всё `#[serde(default)]` —
  старые `settings.json` без миграции. `python_path` остаётся (Local).
  `python_enabled` **остаётся мастер-гейтом** (см. развилку §7.1).
- `features/tools/python.rs` → диспетчер по режиму (enum-бэкенд внутри
  `PythonExec`): id инструмента **`python_exec` не меняется** (стабильный
  wire-протокол; presenter ленты уже рисует код+консоль и продолжит работать —
  формат `format_output` сохраняем). `description()` в режиме Wasmer перечисляет
  предустановленные пакеты («доступны numpy, pandas, requests, …») — модель охотнее
  и точнее пользуется инструментом; упоминает изоляцию («нет доступа к файлам
  машины»).
- `ToolConfig` += `python_mode`/`python_net`/`python_wasm_timeout`/пути к ассетам;
  реестр и так пересобирается при правках `config.tools`.
- **UI настроек** (секция «Инструменты», группа «Python»): прежний тумблер;
  режим — Choice (`Wasmer-песочница` / `Локальный интерпретатор`); mode-driven
  видимость полей (путь к интерпретатору — только Local; сеть и таймаут — только
  Wasmer) — прецедент `model_fields`; подсказки-описания. Поиск `/` и счётчики
  секций подхватываются автоматически (единый индекс).

### 3.4 Ассеты: python.webc + site-packages

- **Вариант A (рекомендуемый): `mindfork sandbox setup`** — clap-подкоманда (как
  `backup`/`import-lamellama`): скачивает `python.webc` из реестра Wasmer и колёса по
  **фиксированному lock-списку** в репо (имя/версия/URL/sha256) — WASIX-колёса с
  `pythonindex.wasix.org`, чистые — с PyPI; колесо = zip → распаковка в
  `site-packages/` **без pip и без host-Python**. Идемпотентно, проверка хешей,
  ~100–200 МБ на диске.
- Вариант B: готовый `sandbox-assets.zip` в GitHub Releases, setup лишь
  распаковывает. Проще пользователю, дороже в поддержке релизов.
- Вариант C (отвергнут): pip cross-install (`--platform wasix_wasm32 --target …`) —
  требует установленный Python, что противоречит цели «работает из коробки».
- Стартовый набор: **numpy, pandas, requests (+urllib3/certifi/idna/
  charset-normalizer), cryptography, pillow, aiohttp**. matplotlib — под вопросом
  (тяжёлый; headless-рендер в файл в песочнице малополезен, файл наружу не выходит);
  scipy — в индексе пока отсутствует.
- Гостю: `PYTHONPATH=/site-packages` (RO), рабочий каталог `/tmp` — tmpfs в памяти.

### 3.5 Политика песочницы

| Ресурс | Политика |
|---|---|
| ФС | только webc-ФС питона + RO `site-packages` + RW tmpfs `/tmp`. **Ни одного хост-каталога.** (Расширение «смонтировать `tools.fs_root`» — сознательно не в первой версии.) |
| Сеть | `python_net_enabled=false` → сокетов нет физически (нет passthrough); `true` → host passthrough. Задел: allowlist доменов через свою `virtual-net`-обёртку — вне объёма. |
| CPU/время | таймаут `python_wasm_timeout_secs` → `mfsb_cancel` → terminate WASIX-процесса; residual-риск CPU-цикла — §4.1 |
| Память | wasm32 ≤ 4 ГБ адресно; ограничить Store tunables (напр. 512 МБ–1 ГБ max memory pages) |
| Вывод | прежний `MAX_OUTPUT_CHARS` (8000) |
| Параллелизм | одна активная задача на приложение (гейт в dll) — и защита от утечки потоков, и предсказуемая нагрузка |

---

## 4. Риски и открытые вопросы

1. **Прерывание CPU-цикла — главный технический риск.** У Wasmer нет
   epoch-interruption. План по слоям: (а) terminate WASIX-процесса — покрывает
   I/O-bound и, вероятно, большинство циклов (CPython проверяет сигналы в eval-loop;
   вопрос — доставляет ли WASIX сигнал без syscall'а гостя; **проверить в Фазе 0**
   на `while True: pass`); (б) если нет — metering middleware (замерить накладные и
   совместимость с dlopen); (в) эскалация — воркер-процесс за тем же ABI (kill —
   абсолютная гарантия). До тех пор: гейт «одна задача», незавершаемый поток утекает,
   но не валит приложение. Считаем блокером GA-статуса фичи, не блокером старта.
2. **Windows-хост.** Поддержка заявлена, CLI под Windows есть, но связку
   «python.webc 3.13 + динлинковка нативных колёс на Windows» подтверждаем смоуком
   (Фаза 0). Фолбэк у пользователя всегда есть — режим Local.
3. **Молодость экосистемы.** Динлинковке полгода (7.0 — январь 2026); build-scripts
   архивирован в пользу wasinix в апреле 2026 — индекс жив, но URL/теги колёс могут
   меняться → lock-список с точными URL+sha256 (репо), setup не зависит от
   «latest».
4. **Вес и сборка.** dll с cranelift — десятки МБ; ассеты ~150 МБ; чистая сборка
   sandbox-крейта — минуты. CI: отдельный job/target, основной гейт
   (fmt/clippy/test) не замедляется. В `cargo test` основного пакета песочница
   участвует только mock'ом; живые прогоны — `#[ignore]`.
5. **Первый старт.** Компиляция python.wasm — секунды–десятки секунд → кэш модулей
   обязателен + баннер «подготовка песочницы…» (паттерн `RagProgress`) на первый
   запуск.
6. **requests/TLS.** Ожидается рабочим (OpenSSL 3.5.1 в сборке, сокеты WASIX,
   aiohttp с TLS в индексе), но это ключевое обещание фичи — проверить GET
   `https://…` в Фазе 0.
7. **Кириллица/UTF-8.** В WASIX-госте локаль UTF-8 по умолчанию (аналог
   `PYTHONUTF8` не нужен), но смоук «печать кириллицы» переносим (прецедент —
   баг cp1252 в Local-режиме).
8. **Лицензии.** Wasmer/wasmer-wasix — MIT; CPython — PSF; колёса — свои лицензии.
   Ассеты не в репо (качаются setup'ом, как словари Hunspell) — конфликтов нет.

---

## 5. Поэтапный план

- **Фаза 0 — спайк (go/no-go, без кода в main). ✅ СДЕЛАНО** (§9, вердикт GO):
  на Windows проверены python 3.13, numpy (динлинковка), requests (HTTPS 200),
  сеть-opt-in, прерывание process-kill; вскрыты «только V8», шим `setsockopt`,
  тяжесть embed → **решение: сайдкар (§9.7)**. Embedding-спайк не достраивался
  (неактуален для сайдкара). *Осталось (по желанию): контрольный прогон на Linux —
  но механика сайдкара идентична и уже проверена на Windows.*
- **Фаза 1 — каркас (сайдкар).** `shared/sandbox.rs` (`SandboxRunner` за трейтом +
  mock): поиск `wasmer` рядом с exe, запуск `tokio::process` (`--v8`, `--volume`,
  таймаут+kill), захват stdout/stderr, враппер кода (**шим `setsockopt`** + запись
  во временный файл, без `-c`-экранирования). `python.rs` — режимы **Wasmer/Local**
  (enum-диспетчер, id `python_exec` не меняется), конфиг `PythonMode`/поля,
  UI-секция «Инструменты»→«Python» (mode-driven видимость), graceful «песочница не
  установлена». Юнит-тесты: диспетчер режимов, сборка аргументов/враппера, парсинг
  вывода, ошибки; живое — `#[ignore]`.
- **Фаза 2 — ассеты и сеть.** `mindfork sandbox setup` (clap-подкоманда): скачать
  `wasmer` бинарь + `python.webc` + колёса по lock-списку (numpy с wasix-индекса,
  requests-стек с PyPI; §9.4), sha256, распаковка колёс в `site-packages/` без pip.
  Кэш скомпилированного модуля Wasmer; `python_net_enabled`; баннер первого запуска
  (паттерн `RagProgress`). `#[ignore]`-смоуки: numpy, requests (с сетью и без),
  кириллица, таймаут-kill.
- **Фаза 3 — прочность и полировка.** Лимит памяти/ресурсов сайдкара; гейт «одна
  задача»; пересмотр дефолта `python_enabled` (§7.1); доки (install.md, spec
  §9.3/§13.2, CLAUDE.md, + ADR «Python-песочница: сайдкар `wasmer`/WASIX за
  `shared/sandbox.rs`» → `docs/decisions/0005`).

---

## 6. Что даёт пользователю (сводка)

| | Local (сейчас) | Wasmer (план, дефолт) |
|---|---|---|
| Требует Python на машине | да | **нет** |
| Доступ кода к файлам машины | полный | **нет** (виртуальная ФС) |
| Сеть | полная, всегда | **выключаемая** (тумблер, физически нет сокетов) |
| numpy/pandas | если сам поставил | **предустановлены** |
| requests | если сам поставил | **предустановлен** (при вкл. сети) |
| Скорость | нативная | ~2–5× медленнее (терпимо для инструмента) |
| Старт | ~50 мс | первый — секунды (компиляция+кэш), дальше ~0.1–0.5 с |
| Место на диске | 0 | ~200–350 МБ (`wasmer` бинарь + python.webc + пакеты) |

---

## 7. Развилки (нужны решения пользователя перед Фазой 1)

1. **Дефолт `python_enabled`.** Песочница снимает исходную причину «выключен по
   умолчанию» (spec §13.2). Рекомендация: в первом PR **оставить `false`**
   (ассетов может не быть — инструмент «включён, но не готов» хуже, чем «выключен»),
   пересмотреть на `true` после обкатки Фазы 2 (когда есть setup и понятные ошибки).
2. **Дефолт сети в песочнице (`python_net_enabled`).** «За» `true`: главная ценность
   предустановленного requests; включение Python — уже осознанный opt-in. «За»
   `false`: консервативные дефолты приватности в проекте (python/fs выключены).
   Рекомендация: **`true`** — но с отдельным тумблером и явной подсказкой в UI.
3. **Поставка ассетов:** A (setup-скачивание по lock-списку) vs B (архив в релизе).
   Рекомендация: **A** (репродуцируемо, без раздувания релизов; B можно добавить позже).
4. **matplotlib в стартовом наборе:** рекомендация **нет** (вес; вывод-файл всё равно
   заперт в песочнице), добавить по запросу.
5. **Прерывание:** решается по фактам Фазы 0 (terminate-only / +metering /
   воркер-процесс за тем же ABI).

---

## 9. Результаты Фазы 0 (спайк, 2026-07-11, Windows 11 x64)

Прогон на реальной целевой машине (Windows 11 Pro, x86_64). Установлен **Wasmer
7.2.0** (winget `Wasmer.Wasmer`; `runtimes: Singlepass, Cranelift, V8; features:
wasix`). Все манипуляции — вне репозитория (scratchpad); в `main`/ветку код не
добавлялся.

### 9.1 Сводная таблица go/no-go

| Проверка | Итог | Детали |
|---|---|---|
| Запуск `python/python` | ✅ | CPython **3.13.0rc2** (WASIX-сборка wasix-org) |
| Бэкенд на Windows | ⚠️→✅ | **cranelift и singlepass падают**, работает только **V8** (см. 9.2) |
| Холодный старт | ✅ | ~3.0–3.8 с (скачивание пакета + первая компиляция) |
| Тёплый старт | ✅ | **~630–660 мс** (V8, пакет закэширован) |
| stdlib (json/hashlib/socket) | ✅ | на месте |
| `ssl` / TLS | ✅ | **OpenSSL 3.5.1**; ручной `wrap_socket` → **TLSv1.3** |
| numpy (нативные `.so`) | ✅ | **numpy 2.3.2**, matmul + `linalg.eigvals` (грузит `lapack_lite.so`); **динамическая линковка 19 `.so` работает** (~4.9 с с учётом компиляции модулей) |
| requests (весь стек) | ✅ | **requests 2.34.2 → HTTP 200** (реальный GitHub API, TLS) — с шимом (9.3) |
| Сеть — opt-in | ✅ | без `--net` — **явный понятный отказ**; с `--net` — DNS + TCP работают |
| Прерывание CPU-цикла | ✅ | `taskkill /T /F` убивает `while True: pass` за **~360 мс, без висящих процессов** |
| Embedding из Rust (wasmer+wasmer-wasix, фича `v8`) на Windows | ✅/⏳ | зависимости резолвятся (wasmer 7.2.0, wasmer-wasix 0.702.0); сборка cdylib — см. 9.5 |

### 9.2 Критично: на Windows пригоден только бэкенд V8

Компилятор **cranelift** (дефолт) **паникует** при компиляции `python.wasm`:
`unimplemented clobbers for exn abi of WindowsFastcall` + `index out of bounds` в
`cranelift-codegen` — не реализован **ABI обработки исключений** (exception-handling
proposal, которым WASIX пользуется для setjmp/longjmp/динлинковки) под Windows fastcall.
**singlepass** — `exceptions proposal not enabled` (и с `--enable-exceptions` не
поднимается). Работает **только рантайм V8** (`--v8`): у него нативная поддержка WASM
exceptions. Это меняет план embedding: dll собирается с фичей **`v8`** крейта `wasmer`
(документация подтверждает: `sys` и `v8` композируются; V8 даёт WASM exceptions+GC),
а **не** с дефолтным cranelift. Плата — V8 как статическая зависимость (вес dll,
время/сложность сборки). На Linux cranelift, вероятно, справится (проверить отдельно) —
но чтобы **не** держать два кодовых пути, разумно и там взять V8 ради единообразия
(решение — §7, доп. развилка ниже).

### 9.3 Сеть работает, но нужен шим `setsockopt`

С `--net`: **DNS и сырой TCP-connect работают**, ручной TLS-handshake даёт TLSv1.3.
Но `urllib`/`http.client` (и, значит, `requests`) падали с `[Errno 28] Invalid
argument` — виновник ровно один: `setsockopt(IPPROTO_TCP, TCP_NODELAY)` не реализован
в WASIX/V8 и бросает `EINVAL`, а `http.client.HTTPConnection.connect` его **всегда**
ставит. Так как **обёртку кода инструмента генерируем мы**, лечится крошечным шимом,
подмешиваемым перед пользовательским кодом:

```python
import socket
_o = socket.socket.setsockopt
socket.socket.setsockopt = lambda self, *a, **k: (None if <TCP_NODELAY> else _o(self,*a,**k))
```

После шима `urllib` → HTTP 200, полный `requests` → HTTP 200. Вывод: **шим
`setsockopt` (глушить неподдержанные опции сокета) — обязательная часть враппера
Wasmer-режима.**

### 9.4 Уточнения к плану

- **Ассеты Python 3.13**: numpy тянется колесом `numpy-2.3.2-cp313-cp313-wasix_wasm32.whl`
  (тег `wasix_wasm32`, стабильная 2.3.2 предпочтительнее dev-2.4.0); requests и его
  зависимости (urllib3/certifi/idna/charset_normalizer) — обычные `py3-none-any`
  колёса с PyPI (в WASIX-индексе их нет и не должно быть — чистый Python). Колесо =
  zip → распаковка без pip/host-Python подтверждена. Lock-список (§3.4, §7.3) должен
  тянуть numpy с `pythonindex.wasix.org`, а requests-стек — с PyPI.
- **Монтирование**: CLI-флаг `--mapdir GUEST:HOST` **устарел** → `--volume HOST:GUEST`
  (обратный порядок!). Для embedding это неважно (virtual-fs API), но в доках/скриптах
  setup учесть.
- **UTF-8**: не проверяли отдельно (перенесено в смоуки Фазы 2), но локаль гостя
  WASIX — UTF-8, `PYTHONUTF8`-костыль Local-режима, вероятно, не нужен.

### 9.5 Embedding-спайк (cdylib на Windows) — вскрыл тяжёлый build-тулчейн

Минимальный проект (`wasmer 7` + `wasmer-wasix 0.702`, `features=["v8"]`/`["sys","v8"]`)
**резолвится** (wasmer 7.2.0, wasmer-wasix 0.702.0), но **сборка падает**: V8-бэкенд
через `bindgen 0.72` требует **libclang** (`Unable to find libclang … set
LIBCLANG_PATH`). На машине LLVM/clang нет (ставится winget `LLVM.LLVM`, ~2.5 ГБ).
Итог — два тяжёлых требования у dll-подхода на Windows, оба следствие «только V8»
(9.2): **(1) build-time** — LLVM/libclang для биндингов V8; **(2) runtime** — статик
V8 в dll (вес, время сборки). Это не тупик, но существенно меняет цену «embed в dll»
и поднимает **развилку 7.7** (ниже): embed V8 в dll vs. **бандлить готовый
`wasmer` CLI** сайдкаром (у него V8+WASIX уже собран и работает, а kill процесса —
чистый, 9.1) за тем же контрактом `shared/sandbox.rs`. Полный embed-прогон
(«python.wasm через `WasiEnvBuilder` + `Pipe` + `terminate`») отложен до решения по
7.7 — нет смысла ставить LLVM и ждать V8-компиляцию, если выбран сайдкар.

### 9.7 Развилка 7.7: **embed V8 в dll** vs. **бандл `wasmer` CLI сайдкаром**

Оба варианта прячутся за один трейт `shared/sandbox.rs` (хост-код не различает).

| | A. Embed V8 в `mindfork_sandbox.dll` | B. Бандл `wasmer.exe`/`.so`-нет — сайдкар-процесс |
|---|---|---|
| Как заказано (dll) | да | нет (процесс, не dll) — но за тем же контрактом |
| Наш build-тулчейн | +LLVM/libclang, долгая сборка V8 | нет (берём готовый бинарь Wasmer) |
| Вес поставки | dll с V8 (десятки МБ) | `wasmer.exe` (~50–100 МБ) рядом |
| Прерывание CPU-цикла | in-process `terminate` V8 (не проверено на чистом цикле) | **process-kill — доказанно чистый (9.1)** |
| Паника/OOM рантайма | в процессе приложения (нужен `catch_unwind`+лимиты) | **изолировано в сайдкаре** (падает — не роняет TUI) |
| Обновление рантайма | пересборка dll | замена бинаря |
| Контроль ФС/сети | тонкий (virtual-fs/virtual-net API) | грубее (флаги CLI `--volume`/`--net`), но достаточно |

**Рекомендация:** учитывая 9.2/9.5 (Windows жёстко требует V8, а его embed тянет
LLVM+V8 в *нашу* сборку) и 9.1 (process-kill идеально чист), **вариант B (сайдкар с
бандленным `wasmer`)** — прагматичнее и надёжнее по прерыванию/изоляции, при этом
сохраняет дух заказа (отдельный артефакт рядом с exe, единый контракт `shared/
sandbox.rs`, ноль Wasmer в основном exe). Вариант A остаётся возможным, если
принципиально нужна именно dll.

> **РЕШЕНО (2026-07-11): вариант B.** Пользователь выбрал сайдкар-бандл `wasmer`.
> Актуальная архитектура — **§3.1-B**; §3.1-A (cdylib) отклонён. Полный embed-спайк
> (WasiEnvBuilder+Pipe+terminate) не достраивался — он неактуален для B. Механика
> сайдкара **фактически уже проверена**: все прогоны §9 (python/numpy/requests/kill)
> шли именно через `wasmer` CLI — ровно то, что будет делать `shared/sandbox.rs`.

### 9.6 Влияние на риски (§4) и развилки (§7)

- **Риск 1 (прерывание)** снижен: process-kill CPU-цикла на Windows — чистый и
  быстрый (~360 мс). Это подтверждает эскалацию «воркер-процесс за тем же ABI» как
  гарантированный фолбэк, если in-process `terminate()` V8 не остановит чистый
  CPU-цикл (проверить в спайке embedding).
- **Риск 2 (Windows)** в целом **снят** для рантайма (python/numpy/requests/сеть/kill
  работают), **но** с оговоркой 9.2: обязателен бэкенд V8 (не cranelift).
- **Риск 6 (requests/TLS)** **снят** — с шимом 9.3 работает end-to-end.
- **Новая развилка (7.6): бэкенд компиляции.** Windows вынуждает V8. Варианты:
  (а) **V8 везде** (единый код, но тяжёлая dll и на Linux); (б) V8 на Windows +
  cranelift на Linux (легче Linux-сборка, но два пути и `#[cfg]`). Рекомендация:
  **(а) V8 везде** в первой версии ради единообразия; оптимизацию Linux-сборки на
  cranelift отложить. Требует решения пользователя.
- **Развилка 7.1 (дефолт `python_enabled`)**: результаты 9.1 усиливают довод оставить
  `false` до готового `sandbox setup` — ассеты (python.webc + колёса, ~150–250 МБ)
  скачиваются отдельно, «включён, но не установлен» хуже «выключен».

---

## 8. Источники

- [Wasmer 7 (блог, 30.01.2026)](https://wasmer.io/posts/wasmer-7) · [релиз v7.0.0](https://github.com/wasmerio/wasmer/releases/tag/v7.0.0) · [Phoronix](https://www.phoronix.com/news/Wasmer-7.0-Released) · [heise](https://www.heise.de/en/news/WebAssembly-Wasmer-7-0-brings-experimental-async-support-for-Python-11163325.html) · [InfoWorld](https://www.infoworld.com/article/4125985/wasmer-beefs-up-python-support.html)
- [Python on the Edge (блог Wasmer, сент. 2025)](https://wasmer.io/posts/python-on-the-edge-powered-by-webassembly) · [Dynamic linking in WASIX](https://wasmer.io/posts/dynamic-linking-in-wasm-wasix) · [Announcing WASIX](https://wasmer.io/posts/announcing-wasix)
- [pythonindex.wasix.org](https://pythonindex.wasix.org/) (индекс WASIX-колёс) · [wasix-org/build-scripts](https://github.com/wasix-org/build-scripts) (архив; преемник [wasinix](https://github.com/wasix-org/wasinix)) · [wasix-org/cpython](https://github.com/wasix-org/cpython)
- [пакет python/python в реестре](https://wasmer.io/python/python) · [WASIX docs](https://wasix.org/docs/)
- [wasmer-wasix (crates.io)](https://crates.io/crates/wasmer-wasix) · [lib.rs](https://lib.rs/crates/wasmer-wasix) · [docs.rs](https://docs.rs/crate/wasmer-wasix/latest)
- Прерывание: [wasmer#337](https://github.com/wasmerio/wasmer/issues/337) · [rust-lang форум](https://users.rust-lang.org/t/interrupt-wasmer-wasmtime/60331) · (для сравнения: [wasmtime epochs](https://docs.wasmtime.dev/examples-interrupting-wasm.html))
- Песочница ФС: [GHSA-4mq4-7rw3-vm5j](https://github.com/wasmerio/wasmer/security/advisories/GHSA-4mq4-7rw3-vm5j) · [wasmer#4267](https://github.com/wasmerio/wasmer/issues/4267)
- Альтернативы: [Pyodide вне браузера — нет](https://github.com/pyodide/pyodide/issues/558), [обсуждение wasmtime/wasmer](https://github.com/pyodide/pyodide/discussions/5145) · [wasi-wheels (unmaintained)](https://github.com/dicej/wasi-wheels) · [numpy WASI issue](https://github.com/numpy/numpy/issues/25859) · [состояние WASI-поддержки CPython](https://snarky.ca/wasi-support-for-cpython-june-2023/)
