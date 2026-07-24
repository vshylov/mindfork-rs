//! Note tests. See mod.rs.

use super::super::testkit::ctx_with_storage;
use super::*;
use uuid::Uuid;

/// The reference locale (ru) — direct overview calls in tests pin the ru bundle.
fn ru() -> &'static crate::shared::i18n::Locale {
    crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
}

/// Is there no Cyrillic in the string (a proxy for "translated to en").
fn no_cyrillic(s: &str) -> bool {
    !s.chars()
        .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c))
}

#[test]
fn note_tool_descriptions_are_localized() {
    // Every note tool returns DIFFERENT text on ru/en (catches a forgotten
    // `_loc`), and the en description has no Cyrillic. §3.5 docs/history/i18n.md.
    use crate::shared::i18n::{Lang, locale};
    let (ru, en) = (locale(Lang::Ru), locale(Lang::En));
    let pairs: Vec<(String, String)> = vec![
        (NoteSave.description(ru), NoteSave.description(en)),
        (NoteRecall.description(ru), NoteRecall.description(en)),
        (NoteRevise.description(ru), NoteRevise.description(en)),
        (NoteSupersede.description(ru), NoteSupersede.description(en)),
        (NoteMerge.description(ru), NoteMerge.description(en)),
        (NoteLink.description(ru), NoteLink.description(en)),
        (NoteNeighbors.description(ru), NoteNeighbors.description(en)),
        (
            ConsolidateNotes.description(ru),
            ConsolidateNotes.description(en),
        ),
        (
            NoteCiteSource.description(ru),
            NoteCiteSource.description(en),
        ),
    ];
    for (r, e) in pairs {
        assert_ne!(r, e, "description not localized (loc forgotten?): {r}");
        assert!(no_cyrillic(&e), "Cyrillic in en description: {e}");
    }
}

#[tokio::test]
async fn note_recall_result_localized_for_all_langs() {
    // The empty result and the "found" header render in every built-in language.
    use crate::shared::i18n::{Lang, locale};
    for &lang in Lang::ALL {
        let profile = Uuid::new_v4();
        let (_d, _s, mut ctx) = ctx_with_storage(profile);
        ctx.loc = locale(lang);
        let empty = NoteRecall
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(
            empty.result,
            locale(lang).t("tool.note_recall.result.empty")
        );
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "hello world"}))
            .await
            .unwrap();
        let out = NoteRecall
            .invoke(&ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert!(out.result.contains("hello world"), "{lang:?}");
        assert!(
            out.result
                .contains(&locale(lang).tf("tool.note_recall.result.header", &[("n", "1")])),
            "{lang:?}: {}",
            out.result
        );
    }
}

#[tokio::test]
async fn save_then_recall_isolated_by_profile() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);

    NoteSave
        .invoke(
            &ctx,
            serde_json::json!({"content": "любит чай", "tags": ["pref"]}),
        )
        .await
        .unwrap();
    // The note was actually recorded under this profile.
    assert_eq!(
        storage
            .db()
            .note_list(profile, None, &[], None)
            .unwrap()
            .len(),
        1
    );

    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({"query": "чай"}))
        .await
        .unwrap();
    assert!(out.result.contains("любит чай"));

    // A different profile doesn't see the note.
    let (_d2, _s2, other) = ctx_with_storage(Uuid::new_v4());
    // A different profile — different storage: checking isolation at the filter level
    let empty = NoteRecall
        .invoke(&other, serde_json::json!({}))
        .await
        .unwrap();
    assert!(empty.result.contains("не найдены"));
}

#[tokio::test]
async fn recall_by_tag() {
    let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "a", "tags": ["x"]}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "b", "tags": ["y"]}))
        .await
        .unwrap();
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({"tags": ["x"]}))
        .await
        .unwrap();
    assert!(out.result.contains("a"));
    assert!(!out.result.contains("- b"));
}

#[tokio::test]
async fn save_rejects_empty_content() {
    let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
    assert!(
        NoteSave
            .invoke(&ctx, serde_json::json!({"content": "  "}))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn recall_excludes_self_notes() {
    // Tier 1 "narrative as notes": self-notes (@self) don't surface in user-facing
    // note_recall — neither the substring path nor the semantic one.
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "любит чай"}))
        .await
        .unwrap();
    storage
        .db()
        .note_insert(&Note::new(
            profile,
            "сам люблю чай",
            vec![SELF_NOTE_TAG.to_string()],
        ))
        .unwrap();

    // The substring path (no query).
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({}))
        .await
        .unwrap();
    assert!(out.result.contains("любит чай"));
    assert!(!out.result.contains("сам люблю чай"));

    // The semantic path (there's a query + an embedder).
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({"query": "чай"}))
        .await
        .unwrap();
    assert!(out.result.contains("любит чай"));
    assert!(!out.result.contains("сам люблю чай"));
}

#[tokio::test]
async fn recall_includes_self_notes_when_enabled() {
    // Tier 3, Path 2: with recall_includes_self, self-notes (@self) DO enter
    // general note_recall marked [about self] (both branches: substring and
    // semantic); the internal @self tag is hidden from the output.
    let profile = Uuid::new_v4();
    let (_d, storage, mut ctx) = ctx_with_storage(profile);
    ctx.recall_includes_self = true;
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "любит чай"}))
        .await
        .unwrap();
    storage
        .db()
        .note_insert(&Note::new(
            profile,
            "сам люблю чай",
            vec![SELF_NOTE_TAG.to_string()],
        ))
        .unwrap();

    // The substring path (no query).
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({}))
        .await
        .unwrap();
    assert!(out.result.contains("любит чай"));
    assert!(out.result.contains("[о себе] сам люблю чай"));
    // The internal @self tag is hidden from the tag display (the marker replaces it).
    assert!(!out.result.contains("@self"));

    // The semantic path (query + embedder).
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({"query": "чай"}))
        .await
        .unwrap();
    assert!(out.result.contains("[о себе] сам люблю чай"));
}

#[tokio::test]
async fn recall_shows_note_ids() {
    // Tier 3: note_recall prints note ids — otherwise the model can't reference
    // them in note_link/note_revise (including cross-organ).
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "любит чай"}))
        .await
        .unwrap();
    let id = storage.db().note_list(profile, None, &[], None).unwrap()[0].id;
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({}))
        .await
        .unwrap();
    assert!(out.result.contains(&format!("(id={id})")));
}

#[tokio::test]
async fn note_cite_source_links_and_recall_shows_it() {
    // Tier 3, Path 3: a note cites a RAG source; note_recall shows the "Source
    // citations" block.
    use crate::entities::rag::RagDocument;
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    storage
        .db()
        .rag_insert(&RagDocument::new(
            profile,
            "spec.md",
            "текст про X",
            vec![1.0, 0.0],
        ))
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "вывод про X"}))
        .await
        .unwrap();
    let id = storage.db().note_list(profile, None, &[], None).unwrap()[0].id;

    // An unknown source — a clear refusal, nothing created.
    let out = NoteCiteSource
        .invoke(
            &ctx,
            serde_json::json!({"note_id": id.to_string(), "source": "нет.md"}),
        )
        .await
        .unwrap();
    assert!(out.result.contains("не найден в базе знаний"));

    // An existing source — the link is created; a repeat — it already existed.
    let out = NoteCiteSource
        .invoke(
            &ctx,
            serde_json::json!({"note_id": id.to_string(), "source": "spec.md"}),
        )
        .await
        .unwrap();
    assert!(out.result.contains("Связь с источником создана"));
    let out = NoteCiteSource
        .invoke(
            &ctx,
            serde_json::json!({"note_id": id.to_string(), "source": "spec.md"}),
        )
        .await
        .unwrap();
    assert!(out.result.contains("уже существовала"));

    // note_recall shows the source citation.
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({}))
        .await
        .unwrap();
    assert!(out.result.contains("Ссылки на источники"));
    assert!(out.result.contains("spec.md"));
}

#[tokio::test]
async fn recall_surfaces_cross_organ_self_neighbor_marked() {
    // Tier 3 (cross-organ links): a user note EXPLICITLY linked to an "about
    // self" observation shows it in the "Related notes" block marked [about
    // self] — but regular search still doesn't pull in self-notes.
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(
            &ctx,
            serde_json::json!({"content": "пользователь любит краткость"}),
        )
        .await
        .unwrap();
    storage
        .db()
        .note_insert(&Note::new(
            profile,
            "я склонен к многословию",
            vec![SELF_NOTE_TAG.to_string()],
        ))
        .unwrap();
    let user_id = id_by_content(&storage, profile, "пользователь любит краткость");
    let self_id = id_by_content(&storage, profile, "я склонен к многословию");
    storage
        .db()
        .note_link_insert(profile, user_id, self_id, "contradicts")
        .unwrap();

    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({"query": "краткость"}))
        .await
        .unwrap();
    // The primary output — only the user note (self is hidden from search).
    assert!(out.result.contains("пользователь любит краткость"));
    // But the linked observation surfaces in the related-links block marked [about self].
    assert!(out.result.contains("Связанные заметки"));
    assert!(out.result.contains("[о себе]"));
    assert!(out.result.contains("я склонен к многословию"));
}

#[tokio::test]
async fn self_related_block_surfaces_cross_organ_user_note_marked() {
    // Tier 3: reading the "self-model" shows a user note that neighbors an
    // observation, marked [note] (a self↔user edge).
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    storage
        .db()
        .note_insert(&Note::new(
            profile,
            "я склонен к многословию",
            vec![SELF_NOTE_TAG.to_string()],
        ))
        .unwrap();
    NoteSave
        .invoke(
            &ctx,
            serde_json::json!({"content": "пользователь любит краткость"}),
        )
        .await
        .unwrap();
    let self_id = id_by_content(&storage, profile, "я склонен к многословию");
    let user_id = id_by_content(&storage, profile, "пользователь любит краткость");
    storage
        .db()
        .note_link_insert(profile, self_id, user_id, "contradicts")
        .unwrap();

    let block = self_related_block(&ctx, &[self_id]).expect("expected a links block");
    assert!(block.contains("[заметка]"));
    assert!(block.contains("пользователь любит краткость"));
    assert!(block.contains("contradicts"));
}

#[tokio::test]
async fn save_gate_excludes_self_notes() {
    // The note_save gate doesn't show semantically close self-notes — a regular
    // save shouldn't run into "self-model" observations.
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    storage
        .db()
        .note_insert(&Note::new(
            profile,
            "aaaa bbbb",
            vec![SELF_NOTE_TAG.to_string()],
        ))
        .unwrap();
    let out = NoteSave
        .invoke(&ctx, serde_json::json!({"content": "aaab"}))
        .await
        .unwrap();
    // The only close note is self → the "Similar notes" block doesn't appear.
    assert!(!out.result.contains("Похожие заметки"));
    assert!(!out.result.contains("aaaa bbbb"));
}

#[tokio::test]
async fn consolidation_overview_excludes_self_notes() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "обычная одна"}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "обычная два"}))
        .await
        .unwrap();
    storage
        .db()
        .note_insert(&Note::new(
            profile,
            "наблюдение о себе",
            vec![SELF_NOTE_TAG.to_string()],
        ))
        .unwrap();

    let overview = build_consolidation_overview(&storage, profile, ru());
    // The self-note isn't in the active count and isn't in the overview's lists.
    assert!(overview.contains("Активных заметок: 2"));
    assert!(!overview.contains("наблюдение о себе"));
}

#[tokio::test]
async fn self_consolidation_overview_covers_self_only() {
    // Tier 3: the self-consolidation overview over observations (@self) — similar
    // pairs, contradicts, no links; user notes are excluded; None when < 2.
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    assert!(build_self_consolidation_overview(&storage, profile, ru()).is_none());
    create_note(&ctx, "aaaa bbbb".into(), vec![SELF_NOTE_TAG.to_string()])
        .await
        .unwrap();
    // 1 observation → still None.
    assert!(build_self_consolidation_overview(&storage, profile, ru()).is_none());
    create_note(&ctx, "aaab".into(), vec![SELF_NOTE_TAG.to_string()])
        .await
        .unwrap();
    create_note(&ctx, "wwww".into(), vec![SELF_NOTE_TAG.to_string()])
        .await
        .unwrap();
    // A user note shouldn't make it into the observation overview.
    NoteSave
        .invoke(
            &ctx,
            serde_json::json!({"content": "aaaa пользовательская"}),
        )
        .await
        .unwrap();

    let ov = build_self_consolidation_overview(&storage, profile, ru()).unwrap();
    assert!(ov.contains("Обзор наблюдений"));
    assert!(ov.contains("Наблюдений: 3")); // @self only
    assert!(!ov.contains("пользовательская"));
    // A similar pair among observations (aaaa bbbb ↔ aaab, cosine ≈ 0.89 ≥ 0.85).
    assert!(ov.contains("aaaa bbbb"));
    assert!(ov.contains("aaab"));

    // A contradicts link among observations — the overview shows it.
    let selves = storage
        .db()
        .note_list(profile, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap();
    storage
        .db()
        .note_link_insert(profile, selves[0].id, selves[1].id, "contradicts")
        .unwrap();
    let ov = build_self_consolidation_overview(&storage, profile, ru()).unwrap();
    assert!(ov.contains("Связи contradicts среди наблюдений: 1"));
}

#[tokio::test]
async fn summary_obs_overlap_surfaces_match_not_unrelated() {
    // A2: a self-description (summary) paragraph matching an observation is
    // surfaced as a pair; an unrelated paragraph/observation isn't. Observation
    // vectors are set manually, so the test is robust to the threshold (a match =
    // cosine 1.0, non-matches = 0.0).
    use crate::entities::self_model::SelfModel;
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);

    // Summary paragraphs (by blank line): P_match (all "a") + unrelated P_unrel
    // (all "b"); both ≥ 40 characters. MockEmbedder — a bag of characters, so
    // their embeddings are orthogonal.
    let p_match = "a".repeat(50);
    let p_unrel = "b".repeat(50);
    let mut model = SelfModel::new(profile);
    model.summary = format!("{p_match}\n\n{p_unrel}");
    storage.db().self_model_upsert(&model).unwrap();

    // The observation matching P_match: its vector = P_match's embedding (cosine = 1.0).
    let match_vec = ctx
        .embedder
        .embed(vec![p_match.clone()])
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let obs_match = Note::new(profile, "MARKER_MATCH", vec![SELF_NOTE_TAG.to_string()]);
    storage.db().note_insert(&obs_match).unwrap();
    storage
        .db()
        .note_vector_upsert(obs_match.id, profile, &match_vec)
        .unwrap();

    // An unrelated observation: a vector on an unused dimension (cosine = 0 with both).
    let mut other_vec = vec![0.0_f32; match_vec.len()];
    other_vec[5] = 1.0;
    let obs_other = Note::new(profile, "MARKER_OTHER", vec![SELF_NOTE_TAG.to_string()]);
    storage.db().note_insert(&obs_other).unwrap();
    storage
        .db()
        .note_vector_upsert(obs_other.id, profile, &other_vec)
        .unwrap();

    let out = summary_observation_overlaps(&storage, ctx.embedder.as_ref(), profile, ru())
        .await
        .expect("expected a summary↔observation match section");
    assert!(out.contains("совпадающие с наблюдениями")); // the section header
    assert!(out.contains("MARKER_MATCH"));
    assert!(out.contains(&obs_match.id.to_string())); // the observation's full id
    // The unrelated observation and unrelated paragraph don't surface.
    assert!(!out.contains("MARKER_OTHER"));
    assert!(!out.contains(&"b".repeat(10)));
    // Exactly one pair (one "\n- …" item line under the header).
    assert_eq!(out.matches("\n- ").count(), 1);
}

#[tokio::test]
async fn summary_obs_overlap_soft_degrades() {
    // A2, graceful degradation: no observations / an empty summary / an embedder
    // with a mismatched vector count → no section (None), no panic.
    use crate::entities::self_model::SelfModel;
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);

    // There's a summary, but no observations → None.
    let mut model = SelfModel::new(profile);
    model.summary = "a".repeat(50);
    storage.db().self_model_upsert(&model).unwrap();
    assert!(
        summary_observation_overlaps(&storage, ctx.embedder.as_ref(), profile, ru())
            .await
            .is_none()
    );

    // Add an observation with a vector — now there's something to compare.
    let match_vec = ctx
        .embedder
        .embed(vec!["a".repeat(50)])
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let obs = Note::new(profile, "obs", vec![SELF_NOTE_TAG.to_string()]);
    storage.db().note_insert(&obs).unwrap();
    storage
        .db()
        .note_vector_upsert(obs.id, profile, &match_vec)
        .unwrap();

    // The embedder returns the wrong number of vectors → None (a mismatch).
    struct BadCountEmbedder;
    #[async_trait::async_trait]
    impl crate::shared::api::Embedder for BadCountEmbedder {
        async fn embed(&self, _texts: Vec<String>) -> anyhow::Result<Vec<Vec<f32>>> {
            Ok(Vec::new()) // 0 vectors for any input — a mismatch
        }
    }
    assert!(
        summary_observation_overlaps(&storage, &BadCountEmbedder, profile, ru())
            .await
            .is_none()
    );

    // An empty summary → None (even with observations present and a working embedder).
    let mut empty = SelfModel::new(profile);
    empty.summary = String::new();
    storage.db().self_model_upsert(&empty).unwrap();
    assert!(
        summary_observation_overlaps(&storage, ctx.embedder.as_ref(), profile, ru())
            .await
            .is_none()
    );
}

#[test]
fn migrate_self_narrative_moves_and_is_idempotent() {
    use crate::entities::self_model::{NarrativeSegment, SelfModel};
    use chrono::{Duration, Utc};
    let profile = Uuid::new_v4();
    let (_d, storage, _ctx) = ctx_with_storage(profile);
    // An "old" model with a narrative blob (as before Tier 1).
    let mut m = SelfModel::new(profile);
    let old = Utc::now() - Duration::days(3);
    m.narrative = vec![
        NarrativeSegment {
            id: Uuid::new_v4(),
            text: "старое наблюдение".into(),
            created_at: old,
        },
        NarrativeSegment {
            id: Uuid::new_v4(),
            text: "ещё одно".into(),
            created_at: Utc::now(),
        },
    ];
    storage.db().self_model_upsert(&m).unwrap();

    migrate_self_narrative(&storage, profile);

    // The blob's narrative is cleared, observations became @self notes (created_at preserved).
    assert!(
        storage
            .db()
            .self_model_get(profile)
            .unwrap()
            .unwrap()
            .narrative
            .is_empty()
    );
    let self_notes = storage
        .db()
        .note_list(profile, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap();
    assert_eq!(self_notes.len(), 2);
    assert!(
        self_notes
            .iter()
            .any(|n| n.content == "старое наблюдение" && n.created_at == old)
    );

    // A repeat pass — a no-op (the narrative is empty, no duplicates).
    migrate_self_narrative(&storage, profile);
    assert_eq!(
        storage
            .db()
            .note_list(profile, None, &[SELF_NOTE_TAG.to_string()], None)
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn self_notes_relevant_ranks_and_filters() {
    // Tier 2: relevance-based injection — self_notes_relevant ranks self-notes by
    // closeness to the query and doesn't return regular notes.
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    create_note(
        &ctx,
        "aaaa про краткость".into(),
        vec![SELF_NOTE_TAG.to_string()],
    )
    .await
    .unwrap();
    create_note(
        &ctx,
        "wwww про погоду".into(),
        vec![SELF_NOTE_TAG.to_string()],
    )
    .await
    .unwrap();
    // A regular note (not @self) — shouldn't make it into the observation selection.
    create_note(&ctx, "aaaa обычная".into(), vec![])
        .await
        .unwrap();

    let rel = self_notes_relevant(&storage, ctx.embedder.as_ref(), profile, "aaaa", 3).await;
    assert!(!rel.is_empty());
    assert!(rel[0].content.contains("краткость")); // closest to "aaaa"
    assert!(rel.iter().all(|n| n.content != "aaaa обычная")); // @self only
    // An empty query → empty (the caller's graceful degradation to recency).
    assert!(
        self_notes_relevant(&storage, ctx.embedder.as_ref(), profile, "  ", 3)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn save_surfaces_similar_notes_as_gate() {
    // MockEmbedder(16) — a bag of characters: texts sharing letters are close.
    let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "aaaa bbbb"}))
        .await
        .unwrap();
    // The second note is close by characters → the gate must show the first one.
    let out = NoteSave
        .invoke(&ctx, serde_json::json!({"content": "aaab"}))
        .await
        .unwrap();
    assert!(out.result.contains("Похожие заметки"));
    assert!(out.result.contains("aaaa bbbb"));
}

#[tokio::test]
async fn recall_semantic_finds_non_substring_match() {
    let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "aaaa"}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "wwww"}))
        .await
        .unwrap();
    // The query "aaab" isn't a substring of any note, but is semantically closer
    // to "aaaa" → the semantic path finds it.
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({"query": "aaab", "limit": 1}))
        .await
        .unwrap();
    assert!(out.result.contains("aaaa"));
    assert!(!out.result.contains("wwww"));
}

#[tokio::test]
async fn save_gate_surfaces_legacy_note_without_vector() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    // An "old" note with no vector (inserted directly — as before the feature/on import).
    storage
        .db()
        .note_insert(&Note::new(profile, "aaaa bbbb", vec![]))
        .unwrap();
    assert_eq!(
        storage.db().notes_missing_vectors(profile).unwrap().len(),
        1
    );

    // Save a similar one — the gate must show the old one (backfill in note_save).
    let out = NoteSave
        .invoke(&ctx, serde_json::json!({"content": "aaab"}))
        .await
        .unwrap();
    assert!(out.result.contains("Похожие заметки"));
    assert!(out.result.contains("aaaa bbbb"));
    // Backfill indexed the old note.
    assert_eq!(
        storage.db().notes_missing_vectors(profile).unwrap().len(),
        0
    );
}

#[tokio::test]
async fn recall_backfills_legacy_notes_without_vectors() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    // "Old" notes with no vectors (inserted directly — as before the feature/on import).
    storage
        .db()
        .note_insert(&Note::new(profile, "aaaa", vec![]))
        .unwrap();
    storage
        .db()
        .note_insert(&Note::new(profile, "wwww", vec![]))
        .unwrap();
    assert_eq!(
        storage.db().notes_missing_vectors(profile).unwrap().len(),
        2
    );

    // Semantic recall pulls in vectors and finds a non-substring match.
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({"query": "aaab", "limit": 1}))
        .await
        .unwrap();
    assert!(out.result.contains("aaaa"));
    // Backfill was performed — no more notes without vectors.
    assert_eq!(
        storage.db().notes_missing_vectors(profile).unwrap().len(),
        0
    );
}

#[tokio::test]
async fn revise_rewrites_in_place() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "старое"}))
        .await
        .unwrap();
    let id = storage.db().note_list(profile, None, &[], None).unwrap()[0].id;

    let out = NoteRevise
        .invoke(
            &ctx,
            serde_json::json!({"id": id.to_string(), "content": "новое"}),
        )
        .await
        .unwrap();
    assert!(out.result.contains("переписана"));
    // Content is replaced in place (no new note added).
    let notes = storage.db().note_list(profile, None, &[], None).unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].content, "новое");
}

#[tokio::test]
async fn revise_bad_and_missing_id() {
    let (_d, _s, ctx) = ctx_with_storage(Uuid::new_v4());
    // An invalid uuid → an error.
    assert!(
        NoteRevise
            .invoke(&ctx, serde_json::json!({"id": "not-uuid", "content": "x"}))
            .await
            .is_err()
    );
    // Valid but nonexistent → clear text, not a panic.
    let out = NoteRevise
        .invoke(
            &ctx,
            serde_json::json!({"id": Uuid::new_v4().to_string(), "content": "x"}),
        )
        .await
        .unwrap();
    assert!(out.result.contains("не найдена"));
}

/// A note's id by content (for graph tests).
fn id_by_content(storage: &crate::shared::storage::Storage, profile: Uuid, content: &str) -> Uuid {
    storage
        .db()
        .note_list(profile, None, &[], None)
        .unwrap()
        .into_iter()
        .find(|n| n.content == content)
        .unwrap()
        .id
}

#[tokio::test]
async fn link_then_neighbors() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "альфа"}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "бета"}))
        .await
        .unwrap();
    let a = id_by_content(&storage, profile, "альфа");
    let b = id_by_content(&storage, profile, "бета");

    let out = NoteLink
            .invoke(
                &ctx,
                serde_json::json!({"from_id": a.to_string(), "to_id": b.to_string(), "relation": "refines"}),
            )
            .await
            .unwrap();
    assert!(out.result.contains("Связь создана"));

    // Repeating the same link — an honest "already existed" answer (no duplicate
    // in the graph).
    let dup = NoteLink
            .invoke(
                &ctx,
                serde_json::json!({"from_id": a.to_string(), "to_id": b.to_string(), "relation": "refines"}),
            )
            .await
            .unwrap();
    assert!(dup.result.contains("уже существовала"));

    let nb = NoteNeighbors
        .invoke(&ctx, serde_json::json!({"id": a.to_string()}))
        .await
        .unwrap();
    assert!(nb.result.contains("бета"));
    assert!(nb.result.contains("refines"));

    // An unknown relation type and a self-link → errors.
    assert!(
            NoteLink
                .invoke(
                    &ctx,
                    serde_json::json!({"from_id": a.to_string(), "to_id": b.to_string(), "relation": "foo"}),
                )
                .await
                .is_err()
        );
    assert!(
            NoteLink
                .invoke(
                    &ctx,
                    serde_json::json!({"from_id": a.to_string(), "to_id": a.to_string(), "relation": "relates"}),
                )
                .await
                .is_err()
        );
}

#[tokio::test]
async fn revise_warns_when_note_has_links() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "узел"}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "другой"}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "одиночка"}))
        .await
        .unwrap();
    let a = id_by_content(&storage, profile, "узел");
    let b = id_by_content(&storage, profile, "другой");
    let lone = id_by_content(&storage, profile, "одиночка");
    NoteLink
            .invoke(
                &ctx,
                serde_json::json!({"from_id": b.to_string(), "to_id": a.to_string(), "relation": "contradicts"}),
            )
            .await
            .unwrap();

    // Revising a node with a link warns about note_supersede.
    let out = NoteRevise
        .invoke(
            &ctx,
            serde_json::json!({"id": a.to_string(), "content": "узел v2"}),
        )
        .await
        .unwrap();
    assert!(out.result.contains("переписана"));
    assert!(out.result.contains("note_supersede"));

    // A node with no links — no warning.
    let out2 = NoteRevise
        .invoke(
            &ctx,
            serde_json::json!({"id": lone.to_string(), "content": "одиночка v2"}),
        )
        .await
        .unwrap();
    assert!(!out2.result.contains("note_supersede"));
}

#[tokio::test]
async fn supersede_hides_old_shows_new() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "первая версия"}))
        .await
        .unwrap();
    let old = id_by_content(&storage, profile, "первая версия");

    let out = NoteSupersede
        .invoke(
            &ctx,
            serde_json::json!({"old_id": old.to_string(), "content": "вторая версия"}),
        )
        .await
        .unwrap();
    assert!(out.result.contains("замещена"));

    let active = storage.db().note_list(profile, None, &[], None).unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].content, "вторая версия");
    assert!(!storage.db().note_is_active(profile, old).unwrap());
}

#[tokio::test]
async fn merge_consolidates_sources() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "кусок один"}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "кусок два"}))
        .await
        .unwrap();
    let ids: Vec<String> = storage
        .db()
        .note_list(profile, None, &[], None)
        .unwrap()
        .into_iter()
        .map(|n| n.id.to_string())
        .collect();

    let out = NoteMerge
        .invoke(
            &ctx,
            serde_json::json!({"ids": ids, "content": "единая заметка"}),
        )
        .await
        .unwrap();
    assert!(out.result.contains("Объединено заметок: 2"));

    let active = storage.db().note_list(profile, None, &[], None).unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].content, "единая заметка");

    // Fewer than two existing ones → an error.
    assert!(
        NoteMerge
            .invoke(
                &ctx,
                serde_json::json!({"ids": [Uuid::new_v4().to_string()], "content": "x"}),
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn supersede_preserves_tags() {
    // Superseding a self-note preserves the @self tag — the new version stays
    // hidden from user-facing recall (otherwise it would "fall out" into the output).
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    storage
        .db()
        .note_insert(&Note::new(
            profile,
            "версия 1",
            vec![SELF_NOTE_TAG.to_string()],
        ))
        .unwrap();
    let old = storage
        .db()
        .note_list(profile, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap()[0]
        .id;
    NoteSupersede
        .invoke(
            &ctx,
            serde_json::json!({"old_id": old.to_string(), "content": "версия 2"}),
        )
        .await
        .unwrap();
    let self_notes = storage
        .db()
        .note_list(profile, None, &[SELF_NOTE_TAG.to_string()], None)
        .unwrap();
    assert_eq!(self_notes.len(), 1);
    assert_eq!(self_notes[0].content, "версия 2");
    // And doesn't surface in regular recall.
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({}))
        .await
        .unwrap();
    assert!(out.result.contains("не найдены"));
}

#[tokio::test]
async fn merge_unions_tags() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    storage
        .db()
        .note_insert(&Note::new(profile, "aaa", vec!["x".into()]))
        .unwrap();
    storage
        .db()
        .note_insert(&Note::new(profile, "bbb", vec!["y".into()]))
        .unwrap();
    let ids: Vec<String> = storage
        .db()
        .note_list(profile, None, &[], None)
        .unwrap()
        .iter()
        .map(|n| n.id.to_string())
        .collect();
    NoteMerge
        .invoke(&ctx, serde_json::json!({"ids": ids, "content": "ccc"}))
        .await
        .unwrap();
    let merged = storage.db().note_list(profile, None, &[], None).unwrap();
    assert_eq!(merged.len(), 1);
    assert!(merged[0].tags.contains(&"x".to_string()));
    assert!(merged[0].tags.contains(&"y".to_string()));
}

#[tokio::test]
async fn recall_spreads_to_linked_notes() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "aaaa"}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "zzzz"}))
        .await
        .unwrap();
    let a = id_by_content(&storage, profile, "aaaa");
    let z = id_by_content(&storage, profile, "zzzz");
    NoteLink
            .invoke(
                &ctx,
                serde_json::json!({"from_id": a.to_string(), "to_id": z.to_string(), "relation": "relates"}),
            )
            .await
            .unwrap();

    // The query is close to "aaaa"; "zzzz" isn't similar, but is linked → makes
    // it into "Related".
    let out = NoteRecall
        .invoke(&ctx, serde_json::json!({"query": "aaab", "limit": 1}))
        .await
        .unwrap();
    assert!(out.result.contains("aaaa"));
    assert!(out.result.contains("Связанные заметки"));
    assert!(out.result.contains("zzzz"));
}

#[tokio::test]
async fn consolidate_notes_reports_dups_and_dangling() {
    let profile = Uuid::new_v4();
    let (_d, _s, ctx) = ctx_with_storage(profile);
    // Two near-duplicates (shared characters → a high cosine on MockEmbedder).
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "aaaa bbbb"}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "aaaa bbbbb"}))
        .await
        .unwrap();
    // An unrelated, dissimilar note.
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "zzzz"}))
        .await
        .unwrap();

    let out = ConsolidateNotes
        .invoke(&ctx, serde_json::json!({}))
        .await
        .unwrap();
    assert!(out.result.contains("Обзор базы знаний"));
    // A similar pair was found (at least one).
    assert!(out.result.contains("Похожие пары (возможные дубли"));
    assert!(out.result.contains("aaaa bbbb"));
    assert!(out.result.contains("без связей"));
    assert!(out.result.contains("note_merge"));
}

#[tokio::test]
async fn merge_transfers_links_to_new_note() {
    let profile = Uuid::new_v4();
    let (_d, storage, ctx) = ctx_with_storage(profile);
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "часть один"}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "часть два"}))
        .await
        .unwrap();
    NoteSave
        .invoke(&ctx, serde_json::json!({"content": "третья"}))
        .await
        .unwrap();
    // Ids taken BEFORE merging (afterward the sources are superseded and vanish
    // from the list).
    let s1 = id_by_content(&storage, profile, "часть один");
    let s2 = id_by_content(&storage, profile, "часть два");
    let other = id_by_content(&storage, profile, "третья");
    NoteLink
            .invoke(
                &ctx,
                serde_json::json!({"from_id": s1.to_string(), "to_id": other.to_string(), "relation": "relates"}),
            )
            .await
            .unwrap();

    NoteMerge
        .invoke(
            &ctx,
            serde_json::json!({"ids": [s1.to_string(), s2.to_string()], "content": "единая"}),
        )
        .await
        .unwrap();

    // The source's link is transferred onto the merged note: "third"'s neighbor
    // is "merged".
    let nb = NoteNeighbors
        .invoke(&ctx, serde_json::json!({"id": other.to_string()}))
        .await
        .unwrap();
    assert!(nb.result.contains("единая"));
    assert!(!nb.result.contains("часть один"));
}
