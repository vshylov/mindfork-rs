//! Which [`EmbedRole`] each embedding call site actually uses.
//!
//! This file is the executable form of the table in
//! docs/research/embedding-input-prefixes.md §5, and it exists because a wrong
//! role is **invisible**: it changes no return value, no error, no test
//! assertion anywhere else — it only quietly degrades a comparison on a model
//! that uses input prefixes, by up to 17% of a compressed model's usable range
//! (§2.3). Nothing but an explicit recording can catch that.
//!
//! The rule these tests pin, in one line: **only a search query against a stored
//! index is `Query`.** Everything stored, and everything compared against
//! something stored, is `Passage` — including the four sites that read like
//! queries but feed the calibrated similarity gates (§5.3).

use std::sync::Arc;

use serde_json::json;
use uuid::Uuid;

use super::testkit::{RoleRecorder, ctx_with_backends};
use super::{Tool, ToolContext};
use crate::shared::api::EmbedRole;
use crate::shared::api::mock::MockBackend;
use crate::shared::storage::Storage;

/// A tool context whose embedder records roles.
fn ctx() -> (
    tempfile::TempDir,
    Arc<Storage>,
    ToolContext,
    Arc<RoleRecorder>,
) {
    let rec = Arc::new(RoleRecorder::new());
    let (dir, storage, ctx) = ctx_with_backends(
        Uuid::new_v4(),
        Arc::new(MockBackend::scripted(vec![])),
        rec.clone(),
    );
    (dir, storage, ctx, rec)
}

// ---------- Query: asymmetric retrieval, query side (§5.1) ----------

#[tokio::test]
async fn rag_search_embeds_the_query_as_a_query() {
    let (_d, _s, ctx, rec) = ctx();
    // Index something first, so the search reaches the embedder at all.
    super::rag::RagAdd
        .invoke(
            &ctx,
            json!({"text": "столица Франции — Париж", "source": "f"}),
        )
        .await
        .unwrap();
    assert!(
        rec.all_were(EmbedRole::Passage),
        "indexing writes passages: {:?}",
        rec.roles()
    );

    rec.calls.lock().unwrap().clear();
    super::rag::RagSearch
        .invoke(&ctx, json!({"query": "какая столица?"}))
        .await
        .unwrap();
    assert_eq!(rec.roles(), vec![EmbedRole::Query]);
}

#[tokio::test]
async fn note_recall_embeds_the_query_as_a_query() {
    let (_d, _s, ctx, rec) = ctx();
    super::notes::NoteSave
        .invoke(&ctx, json!({"content": "пользователь ценит краткость"}))
        .await
        .unwrap();

    rec.calls.lock().unwrap().clear();
    super::notes::NoteRecall
        .invoke(&ctx, json!({"query": "что известно про ответы?"}))
        .await
        .unwrap();
    // The backfill pass may embed stored notes (Passage); the query itself must
    // be the one and only Query.
    let queries = rec
        .roles()
        .iter()
        .filter(|r| **r == EmbedRole::Query)
        .count();
    assert_eq!(queries, 1, "exactly one query: {:?}", rec.roles());
}

// ---------- Passage: stored vectors (§5.2) ----------

#[tokio::test]
async fn stored_text_is_always_embedded_as_a_passage() {
    let (_d, _s, ctx, rec) = ctx();
    super::rag::RagAdd
        .invoke(&ctx, json!({"text": "чанк базы знаний", "source": "s"}))
        .await
        .unwrap();
    super::notes::NoteSave
        .invoke(&ctx, json!({"content": "заметка"}))
        .await
        .unwrap();
    assert!(
        rec.all_were(EmbedRole::Passage),
        "every stored vector is a passage: {:?}",
        rec.roles()
    );
}

#[tokio::test]
async fn note_revise_re_embeds_as_a_passage() {
    let (_d, _s, ctx, rec) = ctx();
    let saved = super::notes::NoteSave
        .invoke(&ctx, json!({"content": "первая версия"}))
        .await
        .unwrap();
    let id = saved
        .result
        .split("id=")
        .nth(1)
        .and_then(|s| s.split(|c: char| !c.is_ascii_hexdigit() && c != '-').next())
        .unwrap()
        .to_string();

    rec.calls.lock().unwrap().clear();
    super::notes::NoteRevise
        .invoke(&ctx, json!({"id": id, "content": "вторая версия"}))
        .await
        .unwrap();
    assert!(
        rec.all_were(EmbedRole::Passage),
        "a rewritten note replaces a stored vector: {:?}",
        rec.roles()
    );
}

// ---------- Passage: the symmetric gates (§5.3) — the tricky category ----------

#[tokio::test]
async fn the_note_save_duplicate_gate_is_symmetric() {
    // The vector saved here IS the gate's probe, and it is compared against
    // other stored notes. Both sides passages, by construction.
    let (_d, _s, ctx, rec) = ctx();
    super::notes::NoteSave
        .invoke(&ctx, json!({"content": "пользователь ценит краткость"}))
        .await
        .unwrap();
    super::notes::NoteSave
        .invoke(&ctx, json!({"content": "пользователь любит лаконичность"}))
        .await
        .unwrap();
    assert!(
        rec.all_were(EmbedRole::Passage),
        "the duplicate gate compares stored notes: {:?}",
        rec.roles()
    );
}

#[tokio::test]
async fn the_add_insight_gate_probes_with_a_passage_not_a_query() {
    // `self_note_similar` reads like a query — it takes a text and searches —
    // but the other side of the comparison is the stored @self notes, and the
    // gate it feeds is one of the calibrated thresholds. A Query role here would
    // silently mis-scale it on any prefixed model.
    let (_d, _s, ctx, rec) = ctx();
    super::self_model::AddInsight
        .invoke(
            &ctx,
            json!({"text": "я склонен объяснять слишком подробно"}),
        )
        .await
        .unwrap();
    super::self_model::AddInsight
        .invoke(
            &ctx,
            json!({"text": "я часто даю больше деталей, чем нужно"}),
        )
        .await
        .unwrap();
    assert!(
        rec.all_were(EmbedRole::Passage),
        "the observation gate is passage-to-passage: {:?}",
        rec.roles()
    );
}

#[tokio::test]
async fn the_trait_gate_embeds_both_sides_alike() {
    // Symmetric by nature (new traits against prior ones) and feeds
    // TRAIT_SIMILARITY. Both sides ride in one call, so the invariant is simply
    // "not Query".
    let (_d, _s, ctx, rec) = ctx();
    super::self_model::UpdateUserModel
        .invoke(&ctx, json!({"add_traits": ["ценит краткость"]}))
        .await
        .unwrap();
    rec.calls.lock().unwrap().clear();
    super::self_model::UpdateUserModel
        .invoke(&ctx, json!({"add_traits": ["предпочитает лаконичность"]}))
        .await
        .unwrap();
    assert!(
        rec.all_were(EmbedRole::Passage),
        "trait comparison is symmetric: {:?}",
        rec.roles()
    );
}

// The one genuinely mixed site (§5.1/§5.2) — web-search reranking — is tested in
// `web.rs` itself: `rerank_by_embeddings` and `SearchResult` are private there.
