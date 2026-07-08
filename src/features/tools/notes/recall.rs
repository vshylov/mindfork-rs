//! Заметки — note_recall + семантический путь, связанные блоки, форматирование. Часть модуля [`super`]; разбито из монолита
//! notes.rs (см. docs/refactoring-god-objects.md, этап 4).

use super::*;

/// `note_recall` — ищет заметки профиля по тексту/тегам.
pub struct NoteRecall;

#[async_trait::async_trait]
impl Tool for NoteRecall {
    fn id(&self) -> ToolId {
        NOTE_RECALL_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Memory
    }
    fn ui_label(&self) -> &'static str {
        "найти заметки"
    }
    fn description(&self) -> String {
        "Найти ранее сохранённые заметки по тексту и/или тегам.".into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Подстрока для поиска по содержимому"},
                "tags": {"type": "array", "items": {"type": "string"}},
                "limit": {"type": "integer", "minimum": 1}
            }
        })
    }
    async fn invoke(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<ToolOutcome> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let tags = parse_tags(&args);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);

        // Семантический путь: есть запрос и доступен эмбеддер. Иначе (нет запроса,
        // эмбеддер недоступен или нет векторов у заметок) — откат на подстроку/теги.
        // Оба пути исключают self-заметки (@self) из пользовательской выдачи.
        let notes = match query {
            Some(q) => match semantic_recall(ctx, q, &tags, limit).await {
                Some(n) => n,
                None => list_user_notes(ctx, query, &tags, limit)?,
            },
            None => list_user_notes(ctx, None, &tags, limit)?,
        };

        let mut outcome = format_notes(&notes);
        // Spreading activation: подмешиваем связанные по графу заметки (Ярус 2),
        // чтобы припоминание поднимало кластер, а не одиночные атомы.
        if let Some(block) = related_block(ctx, &notes) {
            outcome.result.push_str(&block);
        }
        // Ссылки заметок на RAG-источники (Ярус 3, Путь 3): показываем, на что опирается
        // заметка (связывание органов памяти).
        let ids: Vec<Uuid> = notes.iter().map(|n| n.id).collect();
        if let Some(block) = cited_sources_block(ctx, &ids) {
            outcome.result.push_str(&block);
        }
        Ok(outcome)
    }
}

/// Подмешиваемый блок «Связанные заметки»: соседи топ-хитов по графу (обе стороны),
/// без уже показанных и без замещённых. `None`, если связей нет. Чистое чтение БД.
///
/// **Кросс-органные связи (Ярус 3):** сосед-наблюдение «о себе» (`@self`), явно
/// связанный моделью с пользовательской заметкой, **показывается** с пометкой
/// `[о себе]`. Это НЕ реверс сокрытия Яруса 1: обычный поиск/spreading по-прежнему
/// не тащит self-заметки — всплывает лишь **намеренно созданное** моделью ребро
/// между органами. См. docs/history/narrative-as-notes.md (Ярус 3, кросс-органные связи).
pub(crate) fn related_block(ctx: &ToolContext, hits: &[Note]) -> Option<String> {
    let mut seen: std::collections::HashSet<Uuid> = hits.iter().map(|n| n.id).collect();
    let mut lines: Vec<String> = Vec::new();
    for hit in hits.iter().take(3) {
        let nb = ctx
            .storage
            .db()
            .note_neighbors(ctx.profile_id, hit.id, None)
            .unwrap_or_default();
        for (note, relation, outgoing) in nb {
            if !seen.insert(note.id) {
                continue;
            }
            let arrow = if outgoing { "→" } else { "←" };
            // Кросс-органный сосед (наблюдение «о себе») помечается — так модель видит
            // связь заметки с наблюдением, не смешивая органы в общей выдаче.
            let mark = if is_self_note(&note) {
                "[о себе] "
            } else {
                ""
            };
            lines.push(format!(
                "- {arrow}{relation} {mark}(id={}) {}",
                note.id, note.content
            ));
            if lines.len() >= RELATED_IN_RECALL {
                break;
            }
        }
        if lines.len() >= RELATED_IN_RECALL {
            break;
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(format!("\nСвязанные заметки:\n{}", lines.join("\n")))
    }
}

/// Блок «Ссылки на источники»: для показанных заметок (по id) перечисляет RAG-источники,
/// на которые они ссылаются (Ярус 3, Путь 3 — связывание органов памяти). `None`, если
/// ссылок нет. Чистое чтение БД. `pub(crate)` — используется и `note_recall`, и чтением
/// «модели себя» (`self_model::render_self_read`).
pub(crate) fn cited_sources_block(ctx: &ToolContext, note_ids: &[Uuid]) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for id in note_ids {
        let srcs = ctx
            .storage
            .db()
            .note_cited_sources(ctx.profile_id, *id)
            .unwrap_or_default();
        for s in srcs {
            lines.push(format!("- (id={id}) → источник «{s}»"));
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(format!("\nСсылки на источники:\n{}", lines.join("\n")))
    }
}

/// Substring/тег-выборка заметок для `note_recall`: читаем без лимита, **отбрасываем
/// self-заметки** (кроме случая `ctx.recall_includes_self` — Ярус 3, Путь 2), затем
/// усечение — иначе self-заметки заняли бы слоты лимита и вытеснили пользовательские
/// из выдачи.
pub(crate) fn list_user_notes(
    ctx: &ToolContext,
    query: Option<&str>,
    tags: &[String],
    limit: Option<usize>,
) -> Result<Vec<Note>> {
    let mut notes = ctx
        .storage
        .db()
        .note_list(ctx.profile_id, query, tags, None)?;
    if !ctx.recall_includes_self {
        notes.retain(|n| !is_self_note(n));
    }
    if let Some(l) = limit {
        notes.truncate(l);
    }
    Ok(notes)
}

/// Семантический поиск заметок по эмбеддингу запроса. `None`, если эмбеддер
/// недоступен (мягкая деградация — вызывающий откатится на подстроку) или выдача
/// пуста (например, у заметок ещё нет векторов). Теги применяются фильтром поверх
/// ранжирования.
pub(crate) async fn semantic_recall(
    ctx: &ToolContext,
    query: &str,
    tags: &[String],
    limit: Option<usize>,
) -> Option<Vec<Note>> {
    // Бэкфилл: дотянуть эмбеддинги заметок без векторов (старые/импортированные),
    // иначе семантический поиск их не увидит.
    ensure_note_vectors(&ctx.storage, ctx.embedder.as_ref(), ctx.profile_id).await;
    let emb = ctx
        .embedder
        .embed(vec![query.to_string()])
        .await
        .ok()?
        .into_iter()
        .next()?;
    let want = limit.unwrap_or(DEFAULT_RECALL);
    // Берём запас кандидатов: их прорежают фильтр по тегам и исключение self-заметок.
    let cand = want.max(30);
    let hits = ctx
        .storage
        .db()
        .note_search_semantic(ctx.profile_id, &emb, cand)
        .ok()?;
    let mut notes: Vec<Note> = hits
        .into_iter()
        .map(|(n, _)| n)
        // self-заметки исключаются, кроме `recall_includes_self` (Ярус 3, Путь 2).
        .filter(|n| {
            (ctx.recall_includes_self || !is_self_note(n))
                && (tags.is_empty() || tags.iter().all(|t| n.tags.contains(t)))
        })
        .collect();
    notes.truncate(want);
    if notes.is_empty() { None } else { Some(notes) }
}

/// Форматирует список заметок в текстовый результат инструмента. Показывает **id**
/// каждой заметки — чтобы модель могла ссылаться на неё в `note_link`/`note_revise`/
/// `note_supersede` (в т.ч. кросс-органно: связать пользовательскую заметку с
/// наблюдением «о себе», Ярус 3). Раньше id не выводился, и заметки из recall были
/// неадресуемы, хотя описание `note_link` обещало «id из note_recall».
///
/// Наблюдения «о себе» (`@self`, попадают в выдачу лишь при `recall_includes_self` —
/// Ярус 3, Путь 2) помечаются префиксом `[о себе]`, а служебный тег `@self` из
/// показа тегов убирается (пометка его заменяет).
pub(crate) fn format_notes(notes: &[Note]) -> ToolOutcome {
    if notes.is_empty() {
        return ToolOutcome::text("Заметки не найдены.");
    }
    let mut out = format!("Найдено заметок: {}\n", notes.len());
    for n in notes {
        let mark = if is_self_note(n) {
            "[о себе] "
        } else {
            ""
        };
        out.push_str(&format!("- (id={}) {mark}{}", n.id, n.content));
        // Служебный тег @self скрываем — его роль играет пометка [о себе].
        let tags: Vec<&str> = n
            .tags
            .iter()
            .filter(|t| t.as_str() != SELF_NOTE_TAG)
            .map(String::as_str)
            .collect();
        if !tags.is_empty() {
            out.push_str(&format!("  [{}]", tags.join(", ")));
        }
        out.push('\n');
    }
    ToolOutcome::text(out.trim_end().to_string())
}
