# i18n — инструкция по продолжению (Ярус 2: перевод инструментов)

> Временный handoff-док. Удалить, когда Ярус 2 завершён (или влить остаток в
> `docs/i18n.md`). Полный дизайн направления — [`docs/i18n.md`](i18n.md).

## Где мы сейчас

- **PR #143** (ветка `feat/i18n-tools`, от `main`): Ярус 1 (язык профиля + горячее ядро) +
  `defaults.json` + **инфраструктура Яруса 2** (трейт `Tool` принимает `&Locale`).
- Все 35 инструментов уже получили параметр `loc` в `description`/`parameters`, но **пока
  игнорируют его** (`_loc`) и возвращают русские строки. `schemas_for(enabled, loc)` и
  call sites (generation/reflection/consolidation) уже передают язык профиля.
- **`ToolContext.loc`** доступен в каждом `invoke` — им локализуются **результаты**.
- ru-бандл (`locales/ru.json`) и en-бандл (`locales/en.json`) уже есть; ключи горячего
  ядра переведены. Механизм: `loc.t("key")` / `loc.tf("key", &[("name", val)])`.

**Задача Яруса 2:** перевести сами тексты инструментов — по группам, каждая группа —
отдельный коммит/PR с ревью en-формулировок пользователем.

## Пошаговый рецепт перевода одного инструмента

Для каждого инструмента (напр. `note_save` в `src/features/tools/notes/save.rs`):

1. **description**: `fn description(&self, _loc: …)` → `fn description(&self, loc: …)`;
   тело `"<русский текст>".into()` → `loc.t("tool.note_save.desc").into()`.
2. **parameters**: `_loc` → `loc`; каждое `"description": "<русский>"` в JSON-схеме →
   `"description": loc.t("tool.note_save.param.<имя_поля>")`. (Имена полей и `type` —
   **не** переводятся, это wire-протокол.)
3. **результаты** (`invoke`): каждую русскую строку `ToolOutcome::text("…")` /
   `format!("…")` → `ctx.loc.t(...)` / `ctx.loc.tf(...)`. Динамические части — через
   плейсхолдеры `{n}`/`{text}`.
4. **бандлы**: добавить ключи в `locales/ru.json` (**дословно** текущий русский —
   так существующие ru-ассерты и живые смоуки не ломаются) и `locales/en.json`
   (перевод; **деликатные тексты — на ревью пользователя**).
5. **тесты**: ассерты на русский текст переориентировать на `ru()`-локаль (helper уже
   есть в тест-модулях notes/self_model, см. `ru()` в `rename_chat.rs`/`reflection.rs`);
   где полезно — добавить per-locale проверку (цикл по `Lang::ALL`, ассерт на
   `loc.t(key)`). Тестовые данные-фикстуры (тексты заметок, имена) — **не** в бандл.

### Конвенция ключей

`tool.<id>.desc`, `tool.<id>.param.<field>`, `tool.<id>.result.<что>`,
`tool.<id>.gate.<что>`. Для блоков-результатов, общих между инструментами (напр.
«Связанные заметки»), — `notes.block.<имя>`. Массив строк в JSON склеивается **одним
пробелом** (для длинных `\`-склеенных промптов); реальный `\n` — явным `\n` в строке.

## Порядок групп (из docs/i18n.md)

### 2a — notes + self_model (самое ценное и связанное)
Файлы: `src/features/tools/notes/{save,recall,edit,graph,cite,overview}.rs` (9
инструментов: note_save/recall/revise/supersede/merge/link/neighbors/consolidate_notes/
cite_source) + `src/features/tools/self_model.rs` (5: get_self_model/reflect/
update_self_model/update_user_model/add_insight).

**Кросс-ссылки — синхронизировать вместе (иначе рассинхрон имён блоков):**
- Заголовки блоков в `notes/overview.rs` («Похожие пары», «Обзор наблюдений для
  консолидации», …) **цитируются** промптами `reflect_system_message` (reflection.rs) и
  `prompt.consolidate.system`, а также рубрикой `reflect` (self_model.rs). Держи заголовок
  и его упоминание на **одном ключе** (или проверь совпадение подстрокой в per-locale
  тесте).
- **Ворота `add_insight`** живут в `notes/save.rs::self_note_similar` («Похожие
  наблюдения … перепиши через note_revise…»), а сам инструмент `add_insight` — в
  self_model.rs. Переводить вместе.
- Блоки «Связанные заметки»/«Ссылки на источники» — `notes/recall.rs` (`related_block`/
  `cited_sources_block`), показываются и в `note_recall`, и в `get_self_model`
  (`render_self_read`).

### 2b — rag + web + fetch
`src/features/tools/{rag,web,fetch}.rs`. `fetch.rs` — промпт саммаризации (system +
задача с/без focus) + описание. `web.rs` — блок «Содержимое:». `rag.rs` — rag_add/
rag_search результаты.

### 2c — introspection + fs + python + utilities + control + subagent
`src/features/tools/{introspection,fs,python,calc,datetime,subagent,control}.rs`.
**Осторожно:** `present.rs::parse_console` **парсит** вывод `python::format_output` по
русским меткам («код возврата:», «stdout:», «stderr:»). Если локализуешь метки в
`python.rs::format_output_parts`, синхронно правь парсер `parse_console` (или ключи
меток общие) — иначе консольная карточка в ленте сломается. Проверить тестом.

## Тесты и гейты (уже работают, доверяй им)

- **`en_bundle_has_no_cyrillic`** (`shared/i18n.rs`) — бандл-wide: если оставишь русский
  в en-переводе любого ключа, тест упадёт. Прямой прокси go-критерия «нет кириллицы».
- **`key_sets_match_across_languages`** / **`placeholder_sets_match_across_languages`** —
  ru и en обязаны иметь одинаковый набор ключей и плейсхолдеров. Добавил ключ в ru —
  добавь в en.
- Существующие тесты инструментов (notes/tests.rs, self_model.rs tests) ассертят русские
  подстроки → переориентируй на `ru()`-локаль (или оставь литералами — они пинят ru-бандл).

## Как гонять (проверка + живой прогон)

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test   # обязательный гейт

# Живой прогон (сервера пользователя; env — в одной команде, shell state не персистится!):
export MINDFORK_ENGINE_URL="http://192.168.1.20:8000/v1"   # Gemma 4 31B q4
export MINDFORK_EMBED_URL="http://192.168.1.20:8001/v1"    # bge-m3
cargo test i18n_en_profile_title_e2e_live self_model_gate_e2e_live \
  -- --ignored --nocapture --test-threads=1
```
Живой смоук Яруса 2: зеркало `self_model_gate_e2e_live` на en-профиле — ворота
`add_insight` должны показать похожее наблюдение **по-английски**, модель интегрировать.
Создание en-профиля в тесте — см. `i18n_en_profile_title_e2e_live` (CreateProfile →
UpdateProfile{language:En} → NewChat → ход).

## Финализация (AGENTS.md §4)

- CLAUDE.md — запись в журнале (что переведено, число тестов, живой GO) + счётчик.
- docs/i18n.md — обновить статус группы; когда все группы готовы — снять «Ярус 2 не начат».
- Удалить этот файл (или влить остаток в docs/i18n.md).
- Коммит-трейлер с фактической моделью; PR — по шаблону, раздел «Модели».

## Ось B (UI) — НЕ Ярус 2

Локализация интерфейса (настройки, статус-бар, справка, ошибки — тексты для человека) —
**отдельное будущее направление** на том же `shared/i18n` (ключи `ui.*`), независимо от
языка агентов. Не смешивать с Ярусом 2.
