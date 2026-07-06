//! Заметки — подсистема self-заметок (@self): свежие/релевантные, граф, бэкфилл. Часть модуля [`super`]; разбито из монолита
//! notes.rs (см. docs/refactoring-god-objects.md, этап 4).

use super::*;

/// Свежие «заметки о себе» профиля (нарратив «модели себя»), новейшие первыми
/// (`updated_at DESC`), не более `limit`. Отдельный путь чтения self-заметок для
/// инъекции/`get_self_model`/`reflect` — пользовательский `note_recall` их скрывает.
/// См. docs/history/narrative-as-notes.md.
pub(crate) fn self_notes_recent(
    storage: &crate::shared::storage::Storage,
    profile_id: Uuid,
    limit: usize,
) -> Vec<Note> {
    let mut notes = storage
        .db()
        .note_list(profile_id, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap_or_default();
    notes.truncate(limit);
    notes
}

/// Наиболее РЕЛЕВАНТНЫЕ запросу self-заметки (инъекция по релевантности, Ярус 2):
/// бэкфилл векторов → эмбеддинг запроса → поиск среди self-заметок по косинусу,
/// top-`limit` (по убыванию близости). Пусто при пустом запросе / недоступном
/// эмбеддере (вызывающий откатится на свежесть — мягкая деградация, как Ярус 1).
/// См. docs/history/narrative-as-notes.md (Ярус 2, инъекция по релевантности).
pub(crate) async fn self_notes_relevant(
    storage: &crate::shared::storage::Storage,
    embedder: &dyn crate::shared::api::Embedder,
    profile_id: Uuid,
    query: &str,
    limit: usize,
) -> Vec<Note> {
    if query.trim().is_empty() || limit == 0 {
        return Vec::new();
    }
    ensure_note_vectors(storage, embedder, profile_id).await;
    let Ok(vecs) = embedder.embed(vec![query.to_string()]).await else {
        return Vec::new();
    };
    let Some(emb) = vecs.into_iter().next() else {
        return Vec::new();
    };
    // Запас кандидатов под фильтр @self (среди всех заметок есть и обычные).
    let Ok(hits) = storage
        .db()
        .note_search_semantic(profile_id, &emb, limit.max(20))
    else {
        return Vec::new();
    };
    hits.into_iter()
        .map(|(n, _)| n)
        .filter(is_self_note)
        .take(limit)
        .collect()
}

/// Блок «Связи наблюдений» для чтения «модели себя» (граф над self-заметками,
/// Ярус 2): **рёбра** графа, касающиеся показанных наблюдений (структура «что с чем
/// соотносится» — то, чего плоский список наблюдений не показывает). Соседа вне
/// показанного набора приводим с текстом (spreading activation). Дедуп рёбер.
/// `None`, если связей нет. Чистое чтение БД.
///
/// **Кросс-органные связи (Ярус 3):** сосед-**пользовательская** заметка (не `@self`),
/// явно связанная моделью с наблюдением, показывается с пометкой `[заметка]` — так
/// чтение «модели себя» видит, что наблюдение «о себе» соотносится с фактом «о
/// собеседнике» (self↔user ребро). Органы остаются раздельными по хранению/поиску;
/// всплывает лишь намеренно созданное ребро. См. docs/history/narrative-as-notes.md (Ярус 3).
pub(crate) fn self_related_block(ctx: &ToolContext, shown: &[Uuid]) -> Option<String> {
    let shown_set: std::collections::HashSet<Uuid> = shown.iter().copied().collect();
    let mut seen_edges: std::collections::HashSet<(Uuid, Uuid, String)> =
        std::collections::HashSet::new();
    let mut lines: Vec<String> = Vec::new();
    for id in shown {
        let nb = ctx
            .storage
            .db()
            .note_neighbors(ctx.profile_id, *id, None)
            .unwrap_or_default();
        for (note, relation, outgoing) in nb {
            // Нормализуем ребро (от→к) и дедупим (та же связь придёт с обоих концов).
            let (from, to) = if outgoing {
                (*id, note.id)
            } else {
                (note.id, *id)
            };
            if !seen_edges.insert((from, to, relation.clone())) {
                continue;
            }
            // Кросс-органный сосед (пользовательская заметка) помечается — чтение
            // «модели себя» видит связь наблюдения с фактом «о собеседнике».
            let mark = if is_self_note(&note) {
                ""
            } else {
                "[заметка] "
            };
            // Соседа вне показанного набора приводим с текстом (spreading activation).
            let tail = if shown_set.contains(&note.id) {
                format!("(id={})", note.id)
            } else {
                format!("{mark}(id={}) {}", note.id, note.content)
            };
            let arrow = if outgoing { "→" } else { "←" };
            lines.push(format!("- (id={id}) {arrow}{relation} {tail}"));
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
        Some(format!("\nСвязи наблюдений:\n{}", lines.join("\n")))
    }
}

/// Одноразовый идемпотентный перенос нарратива «модели себя» из JSON-блоба в
/// self-заметки (`@self`), с сохранением `created_at`. Нарратив **атомарно
/// вычёрпывается** (drain под захватом мьютекса БД) — повторный проход видит пусто
/// (no-op), так что дублей не будет. Вектора эмбеддятся лениво (при следующем
/// recall/воротах — `ensure_note_vectors`). Оркестратор зовёт это best-effort перед
/// чтением self-заметок. См. docs/history/narrative-as-notes.md, шаг 6.
pub(crate) fn migrate_self_narrative(storage: &crate::shared::storage::Storage, profile_id: Uuid) {
    use crate::entities::self_model::NarrativeSegment;
    let mut segments: Vec<NarrativeSegment> = Vec::new();
    let _ = storage.db().self_model_update(profile_id, |m| {
        if m.narrative.is_empty() {
            return false;
        }
        segments = std::mem::take(&mut m.narrative);
        true
    });
    for seg in segments {
        // Сохраняем исходные даты — порядок «свежих наблюдений» после переноса цел.
        let note = Note {
            id: Uuid::new_v4(),
            profile_id,
            content: seg.text,
            tags: vec![SELF_NOTE_TAG.to_string()],
            created_at: seg.created_at,
            updated_at: seg.created_at,
        };
        let _ = storage.db().note_insert(&note);
    }
}
