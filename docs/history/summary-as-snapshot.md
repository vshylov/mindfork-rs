# Plan: self-model summary as a snapshot, not a chronicle (anti-bloat)

> **Status: implemented** (stages 1–4, branch `feat/summary-as-snapshot`). The live
> run `summary_gate_e2e_live` and related SelfModel smokes were GO on Gemma 4 31B + bge-m3.
> Historical document, not a source of truth. Outcome — in CLAUDE.md (post-M9 log) and
> [architecture.md §9](../architecture.md).

An actionable plan from a 2026-07-05 analysis. User observation: "the
self-model tends to bloat and fill up with somewhat chaotic information."
A live profile confirms this: `summary` ≈ 4.5–5k characters of dense essay
against a default injection ceiling of `prompt_cap = 1200`, and the same
insight ("consistent skew") is written into the description **twice** —
LLM-driven integration without gates duplicates itself.

Structure — as in [refinements.md](refinements.md): stages = separate PRs,
each with code sketches, tests, and a scope estimate. Same philosophy as
[notes-connectivity](notes-connectivity.md) / [narrative-as-notes](narrative-as-notes.md):
**integration instead of accumulation**, **gates, not bans** (judgment stays
with the model), "current snapshot + biography-as-scar."

## Diagnosis

Anti-bloat machinery has been built for every organ of the self-model
**except `summary`** — and that's exactly where everything piles up:

| Organ | Bloat/drift protection |
|---|---|
| observations (@self notes) | near-duplicate gate (`self_note_similar`), `note_revise`/`note_supersede`, graph, self-consolidation overview |
| goals | lifecycle by `#id` + closed-goal ceiling (`fold_closed_goals` → scars) |
| traits/interests | merge (add_/remove_) + semantic gate 0.72 + scar-`note` |
| **summary** | **nothing: no target, no gate, no consolidation, no size feedback** |

Five root causes (from the code):

1. **`POLICY_CORE` routes event-like conclusions into summary.** The rule
   splits material on a "durable → `update_self_model`, fleeting →
   `add_insight`" axis ([self_model.rs:46](../../src/features/tools/self_model.rs)).
   But a philosophical insight ("resolved question X," "understood boundary
   Y") is precisely *durable* — and legitimately gets integrated into the
   description under the letter of the rule. The correct axis is different:
   **current state** (who I am, values, working style — in summary) vs.
   **event-conclusion** (what and when I understood — into observations,
   *even if it's durable*). Observations lose nothing by this: they surface
   in the injection by relevance (Tier 2), get linked, and get consolidated.
2. **Integration without gates duplicates itself.** `update_self_model.summary`
   is a wholesale replace with the instruction "integrate the prior with the
   new"; in practice that's "old text + new paragraph," monotonic growth. No
   one catches a near-duplicate inside summary (for observations, the gate
   would catch it).
3. **Bloat silently breaks the injection.** `render_for_prompt` assembles the
   block in the order summary → goals → user → observations and truncates
   **the whole thing** with one final `truncate_chars`
   ([self_model.rs:471](../../src/entities/self_model.rs)).
   When summary > `prompt_cap`, the system prompt gets only the first ~1200
   characters of the essay (cut off mid-word), and active goals, the user
   model, and observations don't make it in **at all** — all of Tier 2's
   relevant-injection machinery runs for nothing.
4. **Updating from a truncated view risks losing the tail.** summary is
   replaced wholesale; if the model "integrates" based on the truncated
   injection rather than a prior `get_self_model`, the tail is lost or
   rewritten from memory (drift). The protocol doesn't explicitly require
   "read the whole thing first."
5. **The echo reinforces growth.** After every edit, `update_self_model`/
   `update_user_model` return the full `render_full`
   ([self_model.rs:396](../../src/features/tools/self_model.rs)) — with a
   bloated summary that's thousands of tokens on every call, and the model
   "anchors" on the essay genre each time.

## Frame (what we're NOT doing — decisions already fixed)

- **Hard truncation of data** — no. Only soft gates/hints; judgment stays
  with the model (the philosophy of the `note_save`/`add_insight`/traits
  gates).
- **Structuring summary into fields** — no (structured traits with a
  lifecycle were already rejected, architecture.md §9.9; summary's form
  stays free text, we discipline the *genre*, not the schema).
- **Moving the injection out of `system`** — no (the prefix-cache trade-off
  was accepted 2026-07-03, refinements.md).
- **No migrations**: the model is a JSON blob + `#[serde(default)]`; a new
  settings field goes through the container's `#[serde(default)]` (like the
  section's other fields).

## Principles (in the project's spirit)

- **Each stage is a separate PR** with a green gate (`cargo fmt`,
  `clippy --all-targets -- -D warnings`, `cargo test`); after merge — update
  the CLAUDE.md log and architecture.md §9.
- **Invariants stay untouched**: the orchestrator remains the sole owner of
  `Chat`; SelfModel mutations are DB-only (no `ChatEffect`); isolation by
  `profile_id`; FSD.
- **Pure functions for logic** — testable without the engine/tokio; live
  verification is an `#[ignore]` smoke (modeled on the narrative-as-notes GO
  smokes).

Recommended order: 1 → 2 → 3 → 4. Stage 3 is independent of 1–2 (can go
earlier); stage 4 relies on the size line from stage 2.

---

## Stage 1 — Genre boundary: "summary is a snapshot, events go into observations" (text only)

**Problem.** Diagnosis item 1: `POLICY_CORE` routes by a durable/fleeting
axis; event-like conclusions, even durable ones, belong in observations.
Nowhere does it say "keep summary short" or "read the whole thing before
editing."

### Step 1.1 — rewrite `POLICY_CORE`

The text stays connected prose (it gets spliced into the middle of sentences
in `maintenance_protocol()` and `reflect_system_message()`); the size target
is qualitative ("keep it short"); the numeric one arrives in stage 2 as a
data-aware note (can't embed it into a `const`). Sketch:

```rust
pub const POLICY_CORE: &str = "Куда что писать. summary (update_self_model) — \
     компактный рабочий снимок: кто ты, что ценишь, как работаешь; держи его \
     кратким, при правке интегрируй и СОКРАЩАЙ, а не только дописывай, а перед \
     правкой прочти его целиком через get_self_model (в промпте он может быть \
     усечён). Событийные выводы — что и когда ты понял(а), разрешённые вопросы, \
     эпизоды, противоречия — записывай add_insight, даже если они устойчивы: \
     наблюдение не теряется (всплывает по релевантности к теме), связывается и \
     консолидируется, а описание себя не раздувается. Мимолётное (настроение, \
     разовая реакция) — тоже в add_insight, не в модель собеседника. Цели веди по \
     #id — закрывай выполненные и неактуальные, а не только ставь новые. \
     Черты/интересы собеседника — update_user_model (add_/remove_, не перетирая \
     прежнее). Если наблюдение почти повторяет прежнее (add_insight покажет \
     похожие) — перепиши то через note_revise или замести note_supersede, а не \
     плоди почти-дубль. Точность важнее угодливости: фиксируй то, что верно, а не \
     что польстит.";
```

*(Note: this is the actual, live Russian text of the `POLICY_CORE` constant,
delivered in the profile's agent-scaffold language — axis A — and is not
translated; see the CLAUDE.md log entry for its English gist.)*

Both consumers (`maintenance_protocol()`, `reflect_system_message()`) update
automatically — the rules still don't live in two places (stage 6 of the
refinement plan stays intact).

### Step 1.2 — tool descriptions and the rubric

- `UpdateSelfModel::description()`: "summary is a compact snapshot (who you
  are, what you value, how you work); integrate and shorten rather than just
  append; event-like conclusions go into add_insight, not here."
- `AddInsight::description()`: add — "durable event-like conclusions (what
  and when you understood) go here too: the observation surfaces by
  relevance and doesn't bloat the self description."
- The `Reflect` rubric (interactive, deliberately not sourced from
  `POLICY_CORE`): a new question — "Has the self description grown too
  large? Move event-like content out of it into observations (add_insight),
  keep the gist in summary."

### Tests

- Update string assertions: `maintenance_protocol_wraps_policy_core`,
  `reflect_system_message_composes_from_policy_core` (the key phrase about
  "agreeableness" is preserved — the assertions stay green in spirit).
- New assertions on genre markers: "snapshot," "shorten," "even if durable"
  in `POLICY_CORE`; "add_insight" in `update_self_model`'s description; the
  new question in `reflect`'s output.

**Scope:** ~0.25 day. Files: `features/tools/self_model.rs` (+ its tests),
`app/orchestrator/reflection.rs` (tests only).

---

## Stage 2 — Soft gates on summary size

**Problem.** Diagnosis items 2/5: neither the model nor the user can see that
the description has grown too large. Needs feedback — modeled on patterns
that already worked (the `note` reminder, the trait gate, the former
`narrative_fill_hint`).

### Step 2.1 — config and params

```rust
// shared/config.rs
/// Ориентир размера описания себя (символы): сверх него инструменты и протокол
/// начинают мягко предлагать сократить summary (это ворота, не потолок — данные
/// не усекаются).
pub const DEFAULT_SELF_MODEL_SUMMARY_TARGET: usize = 1000;
// SelfModelSettings += pub summary_target_chars: usize (контейнерный serde(default))
```

`SelfModelParams` += `summary_target_chars` (санитизация `.max(200)` в
`from_settings`).

### Шаг 2.2 — чистый хелпер подсказки

```rust
// entities/self_model.rs
impl SelfModel {
    /// Подсказка о разросшемся описании: `None`, пока summary в пределах ориентира.
    /// Аналог бывшего narrative_fill_hint, но для summary (единственного органа
    /// без обратной связи о размере).
    pub fn summary_fill_hint(&self, target: usize) -> Option<String> {
        let n = self.summary.chars().count();
        (n > target).then(|| format!(
            "Описание себя разрослось: {n} симв. при ориентире ≤ {target} — при \
             ближайшей правке сократи его до сути, событийные выводы вынеси в \
             наблюдения (add_insight)."
        ))
    }
}
```

### Шаг 2.3 — проводка (три точки показа)

1. **Чтение** — `render_self_read` (features/tools/self_model.rs) дописывает
   подсказку в конец: её видят `get_self_model`, `reflect` **и авто-рефлексия**
   (ей велено начинать с `get_self_model` — отдельной проводки в дайджест не
   нужно).
2. **Инъекция** — `inject_self_model` (orchestrator/generation.rs): при включённом
   протоколе ведения после `maintenance_protocol()` добавляется data-aware
   приписка (протокол становится конкретным, когда описание реально разрослось):

   ```rust
   if maintenance_protocol {
       parts.push(crate::features::tools::self_model::maintenance_protocol());
       if let Some(hint) = m.summary_fill_hint(params.summary_target_chars) {
           parts.push(format!("({hint})"));
       }
   }
   ```
3. **Эхо правки** — `update_self_model`: если summary менялся (флаг из closure),
   к результату добавляется строка размера — всегда, не только при превышении
   (дешёвая обратная связь):

   ```rust
   msg.push_str(&format!(
       "\nОписание: {} симв. (ориентир ≤ {}).",
       model.summary.chars().count(), params.summary_target_chars
   ));
   ```

### Шаг 2.4 — UI настроек

Поле «Модель себя: ориентир описания (симв.)» в секции «Инструменты»
(`FieldId::SmSummaryTarget`, рядом с остальными `Sm*`), подсказка в
`field_description`: «Сверх ориентира инструменты мягко предлагают сократить
описание; данные не усекаются».

### Тесты

- `SelfModelParams::from_settings` — санитизация нового поля; дефолт конфига.
- `summary_fill_hint`: `None` в пределах ориентира, текст с числами сверх.
- `get_self_model`/`reflect` показывают подсказку при превышении и не показывают
  без него (существующие тесты чтения остаются зелёными — подсказки нет).
- `inject_self_model` (чистая): приписка при превышении + включённом протоколе;
  отсутствие при выключенном протоколе/нормальном размере.
- Эхо `update_self_model` содержит «Описание: N симв.» при правке summary.
- **Живой `#[ignore]`-смоук** `summary_gate_e2e_live` (по образцу GO-смоуков):
  программно сеем раздутый summary (> ориентира), просим модель отрефлексировать —
  ожидание: она сокращает summary (`update_self_model` с более коротким) и/или
  выносит событийное в `add_insight` (проверка по БД). Критерий GO — как у прежних
  зондов: поведение воспроизводится, ворота читаются моделью.

**Объём:** ~0.5–1 день. Файлы: `shared/config.rs`, `entities/self_model.rs`,
`features/tools/self_model.rs`, `app/orchestrator/generation.rs`,
`screens/settings.rs`.

---

## Этап 3 — Посекционные бюджеты рендера инъекции

**Проблема.** П.3 диагноза: одно финальное усечение всего блока → раздутый summary
вытесняет из промпта цели, собеседника и наблюдения. Правится **рендер** (данные
не трогаются), поэтому этап независим от 1–2 и полезен даже при их успехе
(страховка на будущее раздувание).

### Шаг 3.1 — бюджет summary-секции

В `render_for_prompt` summary-секция получает не более **половины** бюджета;
остальным секциям гарантирован остаток. Финальное усечение всего блока остаётся
страховкой:

```rust
if !self.summary.trim().is_empty() {
    out.push_str("О себе: ");
    // Не более половины бюджета: раздутое описание не должно вытеснять из
    // инъекции цели/собеседника/наблюдения (посекционный бюджет).
    out.push_str(&truncate_chars(self.summary.trim(), max_chars / 2));
    out.push('\n');
}
```

Порядок секций не меняется (стабильность промпта). `render_full` не трогается
(полное чтение — без усечений, это зафиксированный урок живого теста).

### Шаг 3.2 (полировка, опционально) — усечение по границе слова

`truncate_chars` режет на полуслове («…» посреди слова читается как повреждённая
память). Хелпер `truncate_chars_word`: откат к последнему пробелу в пределах
лимита (fallback — посимвольно, если пробела нет); применить к summary-секции и
финальному усечению.

### Тесты

- `bloated_summary_does_not_starve_sections`: summary 5k + активная цель + черты +
  наблюдения → в `render_for_prompt(1200, …)` присутствуют «Активные цели:»,
  «О собеседнике:», «Недавние наблюдения:», блок ≤ 1200 символов, summary-секция
  оканчивается «…».
- Небольшой summary — поведение байт-в-байт прежнее (без «…»).
- Существующий `render_truncates_to_cap` остаётся зелёным.
- (при 3.2) усечение не рвёт слово.

**Объём:** ~0.5 дня. Файлы: `entities/self_model.rs` (+ тесты).

---

## Этап 4 — Диета эха правок

**Проблема.** П.5 диагноза: `update_self_model`/`update_user_model` возвращают
полный `render_full` после каждой правки — расход токенов растёт вместе с моделью,
а модель «заякоривается» на жанре эссе. Полное чтение остаётся за
`get_self_model`/`reflect`; эхо правки должно отражать **изменённое**.

### Шаг 4.1 — `update_self_model`

Эхо собирается из дельт (всё уже доступно в/после closure):

- строка размера summary из этапа 2 (если менялся);
- добавленные цели **с их `#id`** (модель должна уметь закрыть их позже):
  в closure после `m.add_goal(g)` — сравнить `goals.len()` до/после (пустые
  отбрасываются) и забрать `m.goals.last()`; понадобится публичный
  `Goal::short_id()` (обёртка над приватным `short_hex`);
- закрытые/неактуальные: «Закрыты: #a1b2c3; неактуальны: #…»;
- свёртка: «Старые закрытые цели свёрнуты в наблюдения: K»;
- нерезолвленные ручки — как сейчас.

Полный `render_full` из эха убирается.

### Шаг 4.2 — `update_user_model`

Эхо: итоговые списки правленого органа (компактны по построению) вместо всей
модели — «Черты теперь: …», «Интересы теперь: …», динамика (если менялась);
подтверждение шрама с текстом («Пояснение сохранено наблюдением: „…“»). Ворота
родственных черт и напоминание про `note` — без изменений.

### Тесты

- Эхо `update_self_model` содержит `#id` добавленной цели и **не** содержит текст
  summary (маркер-строка из теста).
- Эхо `update_user_model` показывает итоговые списки; существующие ассерты
  (`replacing_nonempty_dynamic_without_note_nudges` — текст `note` в эхе;
  «Родственные черты»; «без пояснения») остаются зелёными за счёт явных
  подтверждений.
- Живые смоуки не зависят от формата эха (проверяют БД) — ревизия по месту.

**Объём:** ~0.5 дня. Файлы: `features/tools/self_model.rs`,
`entities/self_model.rs` (`Goal::short_id`).

---

## Разовая чистка существующей модели (операционный шаг, без кода)

Уже раздутый summary живого профиля пункты 1–4 сами не сожмут — они предотвращают
зарастание. Два пути:

1. **Силами модели** (предпочтительно, после этапов 1–2): в чате профиля вызвать
   `reflect` / попросить прямо — «прочитай get_self_model и сверни описание:
   суть оставь, событийные выводы перенеси в add_insight». Ворота похожих покажут
   пересечения переносимого с существующими наблюдениями — это штатно (модель
   сольёт через `note_revise`/`note_merge`).
2. **Вручную** через `F3` (многострочный редактор summary).

## Вне объёма (задел)

- **Авто-консолидация модели себя по таймеру** — уже в направлениях развития
  (architecture.md §9.9); ворота этапа 2 дадут ей конкретный сигнал «summary
  разросся». Отдельный PR после проверки этапов на живой модели.
- **Семантическое сравнение summary ↔ наблюдения** в обзоре self-консолидации
  («абзац summary похож на наблюдение X — вынеси/сшей») — требует сегментации
  summary и эмбеддингов абзацев; вернуться, если жанровой границы + ворот
  окажется мало.
- **Старение `current_interests`** (интересы «текущие», но ничто их не выводит) —
  наблюдение зафиксировано; лечится теми же воротами/рефлексией, отдельной
  механики пока не заводим.

## Порядок и итоговый объём

| Этап | Суть | Объём |
|---|---|---|
| 1 | жанровая граница в текстах (POLICY_CORE, описания, рубрика) | ~0.25 дня |
| 2 | ворота размера summary (конфиг + подсказка + 3 точки показа + UI + живой смоук) | ~0.5–1 день |
| 3 | посекционные бюджеты инъекции (+ усечение по слову) | ~0.5 дня |
| 4 | диета эха правок | ~0.5 дня |

Итого ~2 дня. Критерий успеха на живом профиле: summary держится около ориентира;
в инъекции каждого хода присутствуют цели/собеседник/наблюдения; событийные выводы
оседают @self-заметками (и всплывают по релевантности), а не абзацами эссе.
