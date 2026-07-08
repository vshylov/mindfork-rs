//! Заметки — consolidate_notes + обзоры консолидации (пользовательские / @self). Часть модуля [`super`]; разбито из монолита
//! notes.rs (см. docs/refactoring-god-objects.md, этап 4).

use super::*;

/// Строит обзор базы знаний для консолидации: похожие пары (возможные дубли по
/// косинусу), связи `contradicts`, заметки без связей. Только данные (без рубрики) —
/// используется и инструментом `consolidate_notes`, и фоновой авто-консолидацией.
/// Изоляция по `profile_id`. Чистое чтение БД (эмбеддер не нужен — вектора уже в БД).
pub(crate) fn build_consolidation_overview(
    storage: &crate::shared::storage::Storage,
    profile_id: Uuid,
) -> String {
    // Консолидация — только над пользовательскими заметками: self-заметки (@self)
    // исключаем, чтобы «сон» не сливал память о себе с памятью о собеседнике.
    let mut active = storage
        .db()
        .note_list(profile_id, None, &[], None)
        .unwrap_or_default();
    active.retain(|n| !is_self_note(n));
    if active.is_empty() {
        return "База заметок пуста — консолидировать нечего.".to_string();
    }
    let mut with_vec = storage
        .db()
        .notes_with_vectors(profile_id)
        .unwrap_or_default();
    with_vec.retain(|(n, _)| !is_self_note(n));
    let links = storage.db().note_links_all(profile_id).unwrap_or_default();

    // Похожие пары (возможные дубли) по косинусу, по убыванию близости.
    let mut pairs: Vec<(f32, &Note, &Note)> = Vec::new();
    for i in 0..with_vec.len() {
        for j in (i + 1)..with_vec.len() {
            let s = cosine(&with_vec[i].1, &with_vec[j].1);
            if s >= CONSOLIDATE_SIMILARITY {
                pairs.push((s, &with_vec[i].0, &with_vec[j].0));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0));

    let contradicts: Vec<&(Uuid, Uuid, String)> = links
        .iter()
        .filter(|(_, _, r)| r == "contradicts")
        .collect();
    let linked: std::collections::HashSet<Uuid> =
        links.iter().flat_map(|(f, t, _)| [*f, *t]).collect();
    let dangling: Vec<&Note> = active.iter().filter(|n| !linked.contains(&n.id)).collect();

    let mut out = format!(
        "Обзор базы знаний для консолидации:\nАктивных заметок: {} (без связей: {}).\n",
        active.len(),
        dangling.len()
    );

    out.push_str(&format!(
        "\nПохожие пары (возможные дубли, близость ≥ {CONSOLIDATE_SIMILARITY}): {}\n",
        pairs.len()
    ));
    for (s, a, b) in pairs.iter().take(CONSOLIDATE_LIST_CAP) {
        out.push_str(&format!(
            "- {s:.2} (id={}) {} ↔ (id={}) {}\n",
            a.id,
            clip(&a.content, 60),
            b.id,
            clip(&b.content, 60)
        ));
    }

    out.push_str(&format!(
        "\nСвязи contradicts (проверь, держится ли противоречие после правок): {}\n",
        contradicts.len()
    ));
    for (f, t, _) in contradicts.iter().take(CONSOLIDATE_LIST_CAP) {
        out.push_str(&format!("- (id={f}) ↔ (id={t})\n"));
    }

    out.push_str(&format!(
        "\nЗаметки без связей (кандидаты связать): {}\n",
        dangling.len()
    ));
    for n in dangling.iter().take(CONSOLIDATE_LIST_CAP) {
        out.push_str(&format!("- (id={}) {}\n", n.id, clip(&n.content, 60)));
    }

    out.trim_end().to_string()
}

/// Обзор **наблюдений «о себе»** (self-заметок `@self`) для консолидации: похожие пары
/// (возможные дубли), связи `contradicts` среди наблюдений, наблюдения без связей.
/// Аналог [`build_consolidation_overview`], но над памятью «о себе» — для авто-рефлексии
/// и инструмента `reflect`. Обзор self-консолидации был отложен в Ярусе 2 «до
/// подтверждения пользы связывания»; связывание подтвердилось (Ярус 3, GO) — включаем.
/// `None`, если наблюдений < 2 (консолидировать нечего). Чистое чтение БД (вектора уже в
/// БД). Изоляция по `profile_id`. См. docs/history/narrative-as-notes.md.
pub(crate) fn build_self_consolidation_overview(
    storage: &crate::shared::storage::Storage,
    profile_id: Uuid,
) -> Option<String> {
    // Только наблюдения «о себе» (@self) — зеркально исключению self из обзора
    // пользовательских заметок: «сон» наблюдений не трогает память о собеседнике.
    let active = storage
        .db()
        .note_list(profile_id, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap_or_default();
    if active.len() < 2 {
        return None;
    }
    let self_ids: std::collections::HashSet<Uuid> = active.iter().map(|n| n.id).collect();
    let mut with_vec = storage
        .db()
        .notes_with_vectors(profile_id)
        .unwrap_or_default();
    with_vec.retain(|(n, _)| is_self_note(n));
    let links = storage.db().note_links_all(profile_id).unwrap_or_default();

    // Похожие пары (возможные дубли наблюдений) по косинусу, по убыванию близости.
    let mut pairs: Vec<(f32, &Note, &Note)> = Vec::new();
    for i in 0..with_vec.len() {
        for j in (i + 1)..with_vec.len() {
            let s = cosine(&with_vec[i].1, &with_vec[j].1);
            if s >= CONSOLIDATE_SIMILARITY {
                pairs.push((s, &with_vec[i].0, &with_vec[j].0));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0));

    // contradicts среди наблюдений — оба конца @self (граф наблюдений).
    let contradicts: Vec<&(Uuid, Uuid, String)> = links
        .iter()
        .filter(|(f, t, r)| r == "contradicts" && self_ids.contains(f) && self_ids.contains(t))
        .collect();
    let linked: std::collections::HashSet<Uuid> =
        links.iter().flat_map(|(f, t, _)| [*f, *t]).collect();
    let dangling: Vec<&Note> = active.iter().filter(|n| !linked.contains(&n.id)).collect();

    let mut out = format!(
        "Обзор наблюдений «о себе» для консолидации:\nНаблюдений: {} (без связей: {}).\n",
        active.len(),
        dangling.len()
    );
    out.push_str(&format!(
        "\nПохожие пары (возможные дубли, близость ≥ {CONSOLIDATE_SIMILARITY}): {}\n",
        pairs.len()
    ));
    for (s, a, b) in pairs.iter().take(CONSOLIDATE_LIST_CAP) {
        out.push_str(&format!(
            "- {s:.2} (id={}) {} ↔ (id={}) {}\n",
            a.id,
            clip(&a.content, 60),
            b.id,
            clip(&b.content, 60)
        ));
    }
    out.push_str(&format!(
        "\nСвязи contradicts среди наблюдений: {}\n",
        contradicts.len()
    ));
    for (f, t, _) in contradicts.iter().take(CONSOLIDATE_LIST_CAP) {
        out.push_str(&format!("- (id={f}) ↔ (id={t})\n"));
    }
    out.push_str(&format!(
        "\nНаблюдения без связей (кандидаты связать): {}\n",
        dangling.len()
    ));
    for n in dangling.iter().take(CONSOLIDATE_LIST_CAP) {
        out.push_str(&format!("- (id={}) {}\n", n.id, clip(&n.content, 60)));
    }
    Some(out.trim_end().to_string())
}

/// `consolidate_notes` — обзор базы знаний + рубрика для консолидации (entry-point,
/// как `reflect` у SelfModel). Ничего не меняет: дальше модель сама зовёт
/// merge/supersede/revise/link.
pub struct ConsolidateNotes;

#[async_trait::async_trait]
impl Tool for ConsolidateNotes {
    fn id(&self) -> ToolId {
        CONSOLIDATE_NOTES_ID.into()
    }
    fn group(&self) -> crate::features::tools::meta::ToolGroup {
        crate::features::tools::meta::ToolGroup::Memory
    }
    fn ui_label(&self) -> &'static str {
        "консолидация заметок"
    }
    fn description(&self) -> String {
        "Обзор базы знаний для консолидации: похожие пары (возможные дубли), связи \
         contradicts, заметки без связей — и что с этим делать. Точка входа: дальше \
         слей дубли (note_merge), перепиши/замести устаревшее (note_revise/\
         note_supersede), свяжи родственное (note_link)."
            .into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn invoke(&self, ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        let overview = build_consolidation_overview(&ctx.storage, ctx.profile_id);
        let out = format!(
            "{overview}\n\nЧто сделать (только при необходимости):\n\
             - слей явные дубли: note_merge(ids[], content);\n\
             - устаревшее перепиши (note_revise — мелкая правка) или замести \
             (note_supersede — смысловая переработка, сохранит «шрам»);\n\
             - свяжи родственные заметки: note_link (supports/contradicts/refines/relates).\n\
             Меняй только то, что действительно нужно; если всё в порядке — ничего не вызывай."
        );
        Ok(ToolOutcome::text(out))
    }
}
