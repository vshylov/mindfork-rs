//! Заметки — consolidate_notes + обзоры консолидации (пользовательские / @self). Часть модуля [`super`]; разбито из монолита
//! notes.rs (см. docs/history/refactoring-god-objects.md, этап 4).

use super::*;

/// Строит обзор базы знаний для консолидации: похожие пары (возможные дубли по
/// косинусу), связи `contradicts`, заметки без связей. Только данные (без рубрики) —
/// используется и инструментом `consolidate_notes`, и фоновой авто-консолидацией.
/// Изоляция по `profile_id`. Чистое чтение БД (эмбеддер не нужен — вектора уже в БД).
pub(crate) fn build_consolidation_overview(
    storage: &crate::shared::storage::Storage,
    profile_id: Uuid,
    loc: &crate::shared::i18n::Locale,
) -> String {
    // Консолидация — только над пользовательскими заметками: self-заметки (@self)
    // исключаем, чтобы «сон» не сливал память о себе с памятью о собеседнике.
    let mut active = storage
        .db()
        .note_list(profile_id, None, &[], None)
        .unwrap_or_default();
    active.retain(|n| !is_self_note(n));
    if active.is_empty() {
        return loc.t("notes.overview.empty").to_string();
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
        "{}\n{}\n",
        loc.t("notes.overview.header"),
        loc.tf(
            "notes.overview.active",
            &[
                ("n", &active.len().to_string()),
                ("d", &dangling.len().to_string())
            ]
        )
    );

    out.push('\n');
    out.push_str(&loc.tf(
        "notes.overview.pairs",
        &[
            ("sim", &CONSOLIDATE_SIMILARITY.to_string()),
            ("n", &pairs.len().to_string()),
        ],
    ));
    out.push('\n');
    for (s, a, b) in pairs.iter().take(CONSOLIDATE_LIST_CAP) {
        out.push_str(&format!(
            "- {s:.2} (id={}) {} ↔ (id={}) {}\n",
            a.id,
            clip(&a.content, 60),
            b.id,
            clip(&b.content, 60)
        ));
    }

    out.push('\n');
    out.push_str(&loc.tf(
        "notes.overview.contradicts",
        &[("n", &contradicts.len().to_string())],
    ));
    out.push('\n');
    for (f, t, _) in contradicts.iter().take(CONSOLIDATE_LIST_CAP) {
        out.push_str(&format!("- (id={f}) ↔ (id={t})\n"));
    }

    out.push('\n');
    out.push_str(&loc.tf(
        "notes.overview.dangling",
        &[("n", &dangling.len().to_string())],
    ));
    out.push('\n');
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
    loc: &crate::shared::i18n::Locale,
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
        "{}\n{}\n",
        loc.t("notes.self_overview.header"),
        loc.tf(
            "notes.self_overview.count",
            &[
                ("n", &active.len().to_string()),
                ("d", &dangling.len().to_string())
            ]
        )
    );
    out.push('\n');
    out.push_str(&loc.tf(
        "notes.overview.pairs",
        &[
            ("sim", &CONSOLIDATE_SIMILARITY.to_string()),
            ("n", &pairs.len().to_string()),
        ],
    ));
    out.push('\n');
    for (s, a, b) in pairs.iter().take(CONSOLIDATE_LIST_CAP) {
        out.push_str(&format!(
            "- {s:.2} (id={}) {} ↔ (id={}) {}\n",
            a.id,
            clip(&a.content, 60),
            b.id,
            clip(&b.content, 60)
        ));
    }
    out.push('\n');
    out.push_str(&loc.tf(
        "notes.self_overview.contradicts",
        &[("n", &contradicts.len().to_string())],
    ));
    out.push('\n');
    for (f, t, _) in contradicts.iter().take(CONSOLIDATE_LIST_CAP) {
        out.push_str(&format!("- (id={f}) ↔ (id={t})\n"));
    }
    out.push('\n');
    out.push_str(&loc.tf(
        "notes.self_overview.dangling",
        &[("n", &dangling.len().to_string())],
    ));
    out.push('\n');
    for n in dangling.iter().take(CONSOLIDATE_LIST_CAP) {
        out.push_str(&format!("- (id={}) {}\n", n.id, clip(&n.content, 60)));
    }
    Some(out.trim_end().to_string())
}

/// Порог косинусной близости, при котором абзац описания себя (`summary`) считается
/// семантически совпадающим с наблюдением (`@self`-заметкой) — раздел A2 обзора
/// self-консолидации. **Откалибровано на живом bge-m3** (как `TRAIT_SIMILARITY` в
/// `self_model.rs`, смоук `summary_obs_calibration_e2e_live`): перефразы «абзац ↔
/// наблюдение» дали 0.69–0.80, несвязанные пары — 0.48–0.51; чистый разрыв 0.51→0.69.
/// Порог 0.62 (в разрыве, с запасом в обе стороны) ловит все перефразы и отсекает
/// несвязанные. Абзацы длиннее коротких черт, поэтому перефразы чуть ниже, чем у
/// ворот черт (0.73–0.83). См. docs/history/self-model-consolidation.md §A2.
const SUMMARY_OBS_SIMILARITY: f32 = 0.62;

/// Минимальная длина абзаца `summary` (в символах) для участия в сравнении: более
/// короткий фрагмент слишком мал для осмысленного совпадения.
const SUMMARY_PARAGRAPH_MIN_CHARS: usize = 40;

/// Семантическое совпадение абзацев описания себя (`summary`) с наблюдениями
/// (`@self`-заметками) — раздел A2 обзора self-консолидации. У наблюдений вектора уже
/// в БД, а у `summary` их нет (свободный текст) — поэтому абзацы эмбеддятся **на лету**
/// одним запросом (прямое зеркало ворот черт `self_model::near_duplicate_traits`).
/// Возвращает секцию с парами «абзац ≈ наблюдение X → вынеси/сшей», или `None`, если
/// совпадений нет / нет наблюдений / пустой summary. **Мягкая деградация**: эмбеддер
/// недоступен, вернул пусто или нестыковку числа векторов → `None` (как реранкинг
/// RAG/web). Изоляция по `profile_id`. См. docs/history/self-model-consolidation.md §A2.
pub(crate) async fn summary_observation_overlaps(
    storage: &crate::shared::storage::Storage,
    embedder: &dyn crate::shared::api::Embedder,
    profile_id: Uuid,
    loc: &crate::shared::i18n::Locale,
) -> Option<String> {
    // Описание себя (summary) профиля.
    let model = storage.db().self_model_get(profile_id).ok().flatten()?;
    let summary = model.summary.trim();
    if summary.is_empty() {
        return None;
    }
    // Абзацы summary (по пустым строкам), отбрасывая слишком короткие.
    let paragraphs: Vec<String> = summary
        .split("\n\n")
        .map(|p| p.trim().to_string())
        .filter(|p| p.chars().count() >= SUMMARY_PARAGRAPH_MIN_CHARS)
        .collect();
    if paragraphs.is_empty() {
        return None;
    }
    // Наблюдения (@self) с хранимыми векторами — с ними и сравниваем.
    let mut obs = storage
        .db()
        .notes_with_vectors(profile_id)
        .unwrap_or_default();
    obs.retain(|(n, _)| is_self_note(n));
    if obs.is_empty() {
        return None;
    }
    // Эмбеддим абзацы одним запросом (у summary нет хранимых векторов — на лету).
    // Мягкая деградация: ошибка/нестыковка числа векторов → секции нет.
    let Ok(vecs) = embedder.embed(paragraphs.clone()).await else {
        return None;
    };
    if vecs.len() != paragraphs.len() {
        return None;
    }
    let mut lines: Vec<String> = Vec::new();
    for (p, pv) in paragraphs.iter().zip(&vecs) {
        // Ближайшее наблюдение выше порога (одно на абзац — не шумим).
        let mut best: Option<(f32, &Note)> = None;
        for (n, nv) in &obs {
            let s = cosine(pv, nv);
            if s >= SUMMARY_OBS_SIMILARITY && best.map(|(bs, _)| s > bs).unwrap_or(true) {
                best = Some((s, n));
            }
        }
        if let Some((_, n)) = best {
            lines.push(format!(
                "- {} ≈ (id={}) {}",
                clip(p, 60),
                n.id,
                clip(&n.content, 60)
            ));
        }
    }
    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "{}\n{}",
        loc.t("notes.self_overview.summary_obs"),
        lines.join("\n")
    ))
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
    fn description(&self, loc: &crate::shared::i18n::Locale) -> String {
        loc.t("tool.consolidate_notes.desc").into()
    }
    fn parameters(&self, _loc: &crate::shared::i18n::Locale) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn invoke(&self, ctx: &ToolContext, _args: serde_json::Value) -> Result<ToolOutcome> {
        let overview = build_consolidation_overview(&ctx.storage, ctx.profile_id, ctx.loc);
        let out = format!(
            "{overview}\n\n{}",
            ctx.loc.t("tool.consolidate_notes.rubric")
        );
        Ok(ToolOutcome::text(out))
    }
}
