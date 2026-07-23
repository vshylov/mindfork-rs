# RU → EN glossary (source-language migration)

Canonical translations for the RU → EN migration. **Every translator (me and
each subagent) uses these exact renderings** so the same concept is never
translated two different ways across 240 files. When a term here conflicts with
a "nicer" ad-hoc choice, this file wins. This is migration tooling — removed (or
demoted to a CONTRIBUTING terminology note) once the migration lands.

## Style
- American English, present tense, terse — match the existing terse comment
  voice, do **not** pad it into prose.
- Keep the exact structure of the source: line breaks, bullet nesting, code
  fences, tables, links, anchors. Translate prose only.
- Preserve verbatim: identifiers, type/field/fn names, file paths, `spec §8.3`
  style cross-refs, CLI/protocol tokens, JSON keys, numbers, `#[attrs]`, macros.
- Keep the em-dash "— " explanatory style where it reads naturally; a plain
  ": " or " —" is fine when it reads better in English.
- Doc-comment first line stays a one-line summary (Rust convention).

## Domain nouns (the load-bearing ones — never vary)
| RU | EN |
|---|---|
| оркестратор | orchestrator |
| движок (инференса) | engine |
| супервайзер | supervisor |
| лента (сообщений) | message feed / feed |
| поле ввода | input box |
| экран настроек | settings screen |
| список чатов | chat list |
| оверлей / попап | overlay / popup |
| профиль | profile |
| чат | chat |
| обмен (сообщениями) | exchange |
| черновик | draft |
| заметка / заметки | note / notes |
| связность заметок | notes connectivity |
| модель себя | self-model |
| модель собеседника / user_model | user model |
| нарратив | narrative |
| наблюдение (@self) | observation |
| цель (self-model) | goal |
| рефлексия | reflection |
| консолидация / «сон» | consolidation / sleep |
| саморефлексия | self-reflection |
| имперсонация | impersonation |
| затравка (seed) | seed |
| мысли / рассуждения (CoT) | thoughts / reasoning |
| подпись мыслей | thought signature |
| семплинг | sampling |
| эмбеддинги / эмбеддер | embeddings / embedder |
| песочница | sandbox |
| сайдкар | sidecar |
| словарь (спелл-чек) | dictionary |
| спелл-чек | spellcheck |
| тема (оформления) | theme |
| палитра | palette |
| рейл (роли) | rail |
| пилюля (бейдж статуса/мыслей) | pill (a rounded status/label badge — NOT a list bullet) |
| маркер списка | list marker / bullet |
| чип (статуса) | chip |
| глиф | glyph |
| скроллбар | scrollbar |
| локаль / бандл локали | locale / locale bundle |
| язык интерфейса | interface language |
| язык (служебного) каркаса | agent-scaffold language |
| ось A / ось B | axis A / axis B |
| ворота (дублей / совместимости / черт) | gate (duplicate / compatibility / trait) |
| нудж | nudge |
| шрам (ревизии) | scar |
| дайджест | digest |
| ватермарк | watermark |
| каденция | cadence |
| склейка (чанков) | stitching |
| чанк / чанкинг | chunk / chunking |
| протокол ведения | maintenance protocol |
| обзор консолидации | consolidation overview |
| системное сообщение | system message |
| системный промпт | system prompt |
| промпт | prompt |
| счётчик токенов | token counter |
| статус-бар | status bar |
| каталог (инструментов) | catalog |
| реестр (инструментов) | registry |
| гейт (запуска/вызова) | gate |
| снимок (состояния) | snapshot |
| ярус | tier |
| этап (направления) | stage |
| направление (работы) | track / direction |
| развилка | decision point / fork |
| задел | groundwork / future work |
| зонд | probe / spike |
| источник (RAG) | source |
| каркас (служебный) | scaffold |

## Verbs / adjectives / recurring phrases
| RU | EN |
|---|---|
| по умолчанию | by default / default |
| на лету | live / on the fly |
| в бою | in production |
| мягкая деградация | graceful degradation |
| жёсткий фолбэк | hard fallback |
| мягкие ворота | soft gate |
| живой прогон / смоук | live run / live smoke |
| на живой модели | against a live model |
| без миграции | without migration |
| источник истины | source of truth |
| единственный владелец `Chat` | sole owner of `Chat` |
| жизненный цикл | lifecycle |
| перенос слов | word wrap |
| перенос строки | line break / newline |
| буфер обмена | clipboard |
| мягкое удаление | soft delete |
| каскад (удаления) | cascade |
| изоляция по `profile_id` | isolation by `profile_id` |
| предполётная проверка | preflight check |
| раскладко-независимый | layout-independent |
| режим совместимости (терминала) | compatibility mode |
| широкий глиф | wide glyph |
| вне объёма | out of scope |
| за трейтом | behind a trait |
| фоновая задача | background task |
| отменяемый / отмена | cancellable / cancellation |
| дебаунс | debounce |
| гонка (данных) | race |
| сделано | done |
| зелёные / чисты | green / clean |
| N юнит-тестов зелёные | N unit tests green |
| `N` смоуков `#[ignore]` | N `#[ignore]` smokes |
| решение пользователя | user's decision |
| по образцу X | modeled on X |
| в духе проекта | in the project's spirit |

## Do NOT translate (keep verbatim)
- Rust identifiers, type/field/method/module names, macros, attributes.
- CLI subcommands & flags: `backup`, `restore`, `import`, `sandbox setup`,
  `--no-mmap`, etc. (protocol surface — translating them breaks usage).
- JSON keys, config field names, locale keys (`ui.err.*`), tool ids (`mcp__*`,
  `note_save`), env var names (`MINDFORK_ENGINE_URL`).
- `ru.json` / `en.json` values; Hunspell dictionaries.
- In-test assertion strings that check `ru` locale output (they stay Russian).
- Wire/protocol names: `llama-server`, `generateContent`, `/v1/messages`,
  reasoning field names, etc.
- Illustrative `ru`-bundle snippets embedded in i18n docs (translate the prose
  around them; leave the quoted Russian example as an example, noting it is one).

## Production string literals — routing (Phase 5)
Classify each remaining non-test Russian string literal:
- **Non-user-facing** (tracing logs, `build.rs` `cargo:warning`, internal
  diagnostics) → translate inline to English.
- **User-facing** (reaches the UI / the model) → **move into the locale
  bundles** instead of translating inline: add the key to `ru.json` (keep the
  Russian) and `en.json` (English), wire through `loc`/`ctx.loc` on the right
  axis — axis A (agent/profile locale) for text sent to the model (e.g.
  self-model prompt fragments), axis B (interface locale) for UI text. The
  existing i18n gate tests (key/placeholder parity, no-Cyrillic-in-`en`) enforce
  correctness.
