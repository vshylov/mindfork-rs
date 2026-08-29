//! Orchestrator tests — the self-model: injection, F3 edits, signals. Part of the [`super`]
//! module (fixtures in mod.rs). See docs/history/refactoring-god-objects.md, stage 3.

use super::*;
use crate::shared::api::EmbedRole;

#[test]
fn inject_self_model_respects_flag_and_emptiness() {
    use super::super::generation::inject_self_model;
    use crate::entities::self_model::{NarrativeSegment, SelfModel, SelfModelParams};

    let pp = SelfModelParams::default();
    let mut m = SelfModel::new(Uuid::new_v4());
    m.summary = "ценю ясность".into();
    let now = chrono::Utc::now();
    let seg = |t: &str| NarrativeSegment {
        id: Uuid::new_v4(),
        text: t.into(),
        created_at: now,
    };

    // Off → the system message doesn't change (the protocol isn't mixed in either).
    assert_eq!(
        inject_self_model(
            Some("S".into()),
            Some(&m),
            false,
            true,
            &pp,
            now,
            &[],
            crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
        ),
        Some("S".into())
    );
    // On, protocol off, a non-empty model → the block is appended, no protocol.
    let out = inject_self_model(
        Some("S".into()),
        Some(&m),
        true,
        false,
        &pp,
        now,
        &[],
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
    )
    .unwrap();
    assert!(out.starts_with("S\n\n"));
    assert!(out.contains("ценю ясность"));
    assert!(!out.contains("угодливости"));
    // On, protocol off, no model, no observations → no change.
    assert_eq!(
        inject_self_model(
            Some("S".into()),
            None,
            true,
            false,
            &pp,
            now,
            &[],
            crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
        ),
        Some("S".into())
    );
    // No model, but there are observations (self-notes) → injection still happens.
    let obs = [seg("заметил склонность к краткости")];
    let only_obs = inject_self_model(
        None,
        None,
        true,
        false,
        &pp,
        now,
        &obs,
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
    )
    .unwrap();
    assert!(only_obs.contains("Недавние наблюдения:"));
    assert!(only_obs.contains("склонность к краткости"));
    // An empty model + empty observations, system=None → nothing to mix in → None.
    let empty = SelfModel::new(Uuid::new_v4());
    assert_eq!(
        inject_self_model(
            None,
            Some(&empty),
            true,
            false,
            &pp,
            now,
            &[],
            crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru)
        ),
        None
    );
    // Empty system + a non-empty model (protocol off) → the block becomes the system message.
    let only = inject_self_model(
        None,
        Some(&m),
        true,
        false,
        &pp,
        now,
        &[],
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
    )
    .unwrap();
    assert!(only.contains("О себе: ценю ясность"));

    // Protocol on + an empty model → the protocol is mixed in anyway (bootstrap).
    let boot = inject_self_model(
        Some("S".into()),
        Some(&empty),
        true,
        true,
        &pp,
        now,
        &[],
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
    )
    .unwrap();
    assert!(boot.starts_with("S\n\n"));
    assert!(boot.contains("угодливости"));
    // Protocol on + a non-empty model → both the render and the protocol.
    let both = inject_self_model(
        None,
        Some(&m),
        true,
        true,
        &pp,
        now,
        &[],
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
    )
    .unwrap();
    assert!(both.contains("ценю ясность"));
    assert!(both.contains("угодливости"));
}

#[test]
fn inject_self_model_appends_summary_hint_when_bloated() {
    // Stage 2: with the protocol enabled and a bloated description (beyond the target), a
    // data-aware hint to shrink it is appended to the injection; with the protocol disabled,
    // there's no hint even with a bloated description.
    use super::super::generation::inject_self_model;
    use crate::entities::self_model::{SelfModel, SelfModelParams};
    use crate::shared::config::SelfModelSettings;

    // A target of 5 is sanitized up to the floor of 200 — the description is taken longer than 200 chars.
    let params = SelfModelParams::from_settings(&SelfModelSettings {
        summary_target_chars: 5,
        ..SelfModelSettings::default()
    });
    let mut m = SelfModel::new(Uuid::new_v4());
    m.summary = "я".repeat(250);
    let now = chrono::Utc::now();

    // Protocol on → the hint is present.
    let with = inject_self_model(
        None,
        Some(&m),
        true,
        true,
        &params,
        now,
        &[],
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
    )
    .unwrap();
    assert!(with.contains("Описание себя разрослось"));
    // Protocol off → no protocol and no hint (only the model's render).
    let without = inject_self_model(
        None,
        Some(&m),
        true,
        false,
        &params,
        now,
        &[],
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
    )
    .unwrap();
    assert!(!without.contains("Описание себя разрослось"));
}

#[test]
fn blend_self_notes_prioritizes_relevant_and_guarantees_freshest() {
    use super::super::generation::blend_self_notes;
    use crate::entities::note::Note;
    let p = Uuid::new_v4();
    let mk = |c: &str| Note::new(p, c, vec![]);
    let (r1, r2) = (mk("релевантное 1"), mk("релевантное 2"));
    let (f0, f1) = (mk("самое свежее"), mk("свежее 1"));
    let relevant = vec![r1.clone(), r2.clone()];
    let fresh = vec![f0.clone(), f1.clone()];

    // n=3: 2 relevant + the guaranteed freshest one (f1 doesn't fit).
    let out = blend_self_notes(relevant.clone(), &fresh, 3);
    let ids: Vec<_> = out.iter().map(|n| n.id).collect();
    assert_eq!(out.len(), 3);
    assert!(ids.contains(&r1.id) && ids.contains(&r2.id));
    assert!(
        ids.contains(&f0.id),
        "the freshest observation must be guaranteed to be included"
    );
    assert!(!ids.contains(&f1.id));

    // n=2 with 2 relevant: the last relevant one is crowded out by the freshest.
    let out = blend_self_notes(relevant, &fresh, 2);
    let ids: Vec<_> = out.iter().map(|n| n.id).collect();
    assert_eq!(out.len(), 2);
    assert!(ids.contains(&r1.id));
    assert!(ids.contains(&f0.id));
    assert!(!ids.contains(&r2.id));

    // Dedup: if the freshest is already among the relevant ones — it isn't duplicated.
    let out = blend_self_notes(vec![f0.clone(), r1.clone()], &fresh, 3);
    assert_eq!(out.iter().filter(|n| n.id == f0.id).count(), 1);
}

#[tokio::test]
async fn injection_recent_surfaces_relevant_over_fresh() {
    // Tier 2: relevance-based injection surfaces an OLD but request-relevant
    // observation — one that pure recency would have lost.
    use super::super::generation::injection_recent;
    use crate::entities::note::Note;
    use crate::entities::self_model::SelfModelParams;
    use crate::features::tools::notes::SELF_NOTE_TAG;
    use chrono::{Duration, Utc};

    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(Paths::with_root(dir.path())).unwrap();
    let embedder = MockEmbedder::new(16);
    let profile = Uuid::new_v4();

    // X is old (10 days ago), topic "xxxx". Then 4 fresh Y's (topic "yyyy"),
    // crowding X out of recency (narrative_in_prompt=3).
    let now = Utc::now();
    let mut seeds: Vec<(String, chrono::DateTime<Utc>)> =
        vec![("xxxx старое наблюдение".into(), now - Duration::days(10))];
    for i in 0..4 {
        seeds.push((format!("yyyy свежее {i}"), now));
    }
    for (content, at) in &seeds {
        let note = Note {
            id: Uuid::new_v4(),
            profile_id: profile,
            content: content.clone(),
            tags: vec![SELF_NOTE_TAG.to_string()],
            created_at: *at,
            updated_at: *at,
        };
        storage.db().note_insert(&note).unwrap();
        let emb = embedder
            .embed(vec![content.clone()], EmbedRole::Passage)
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        storage
            .db()
            .note_vector_upsert(note.id, profile, &emb)
            .unwrap();
    }
    let params = SelfModelParams::default();

    // A query about "xxxx" → the old relevant observation is surfaced (though not the freshest).
    let recent = injection_recent(&storage, &embedder, profile, true, "xxxx", &params).await;
    assert!(
        recent.iter().any(|s| s.text.contains("xxxx старое")),
        "the relevant old observation should be surfaced: {recent:?}"
    );
    // A query about "yyyy" → the irrelevant old X isn't surfaced.
    let recent = injection_recent(&storage, &embedder, profile, true, "yyyy", &params).await;
    assert!(!recent.iter().any(|s| s.text.contains("xxxx")));
    // Injection disabled → empty.
    assert!(
        injection_recent(&storage, &embedder, profile, false, "xxxx", &params)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn update_self_model_persists_and_reemits() {
    use crate::entities::self_model::SelfModelEdit;
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let root = _d.path().to_path_buf();
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // An edit from the UI editor (no model yet — the orchestrator creates it on the spot).
    cmd_tx
        .send(AppCommand::UpdateSelfModel(SelfModelEdit::SetSummary(
            "ценю ясность".into(),
        )))
        .unwrap();

    // Re-emitting the snapshot reflects the edit.
    let ev = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::SelfModelView { .. }))
        .await
        .unwrap();
    match ev {
        AppEvent::SelfModelView { model, .. } => {
            assert_eq!(model.expect("expected a model").summary, "ценю ясность");
        }
        _ => unreachable!(),
    }

    cmd_tx.send(AppCommand::Quit).unwrap();
    handle.await.unwrap();

    // Persistence: the write is visible after a restart.
    let reopened = Storage::open(Paths::with_root(&root)).unwrap();
    let pid = reopened.json().load_profiles().unwrap()[0].id;
    let stored = reopened.db().self_model_get(pid).unwrap().unwrap();
    assert_eq!(stored.summary, "ценю ясность");
}

#[test]
fn f3_delete_insight_removes_self_note() {
    use crate::entities::self_model::SelfModelEdit;
    use crate::features::tools::notes::SELF_NOTE_TAG;
    let (_d, orch, pid) = orch_with_active_profile();
    // An observation is a self-note (@self).
    let note = crate::entities::note::Note::new(pid, "наблюдение", vec![SELF_NOTE_TAG.to_string()]);
    let nid = note.id;
    orch.storage.db().note_insert(&note).unwrap();

    // F3 "delete observation" → deleting the self-note (not editing the blob).
    orch.handle_update_self_model(SelfModelEdit::DeleteInsight(nid));
    assert!(
        orch.storage
            .db()
            .note_list(pid, None, &[SELF_NOTE_TAG.to_string()], None)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn f3_clear_removes_self_notes_and_blob() {
    use crate::entities::self_model::SelfModelEdit;
    use crate::features::tools::notes::SELF_NOTE_TAG;
    let (_d, orch, pid) = orch_with_active_profile();
    // An observation note + a non-empty model blob.
    orch.storage
        .db()
        .note_insert(&crate::entities::note::Note::new(
            pid,
            "наблюдение",
            vec![SELF_NOTE_TAG.to_string()],
        ))
        .unwrap();
    orch.storage
        .db()
        .self_model_update(pid, |m| {
            m.summary = "о себе".into();
            true
        })
        .unwrap();

    orch.handle_update_self_model(SelfModelEdit::Clear);
    // Self-notes are wiped, the blob is cleared.
    assert!(
        orch.storage
            .db()
            .note_list(pid, None, &[SELF_NOTE_TAG.to_string()], None)
            .unwrap()
            .is_empty()
    );
    let stored = orch.storage.db().self_model_get(pid).unwrap();
    assert!(stored.map(|m| m.summary.is_empty()).unwrap_or(true));
}

#[tokio::test]
async fn handle_done_signals_self_model_changed_on_self_model_tool_call() {
    use crate::entities::message::ToolCallRecord;
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let profile = Profile::new("P", "sys");
    let mut chat = Chat::from_profile(&profile, "t");
    chat.push_message(Message::user("привет"));
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);
    let gen_id = Uuid::new_v4();
    orch.gen_state
        .begin(gen_id, tokio_util::sync::CancellationToken::new());

    // An assistant reply with a self-model tool call → SelfModelChanged.
    let mut msg = Message::assistant("готово");
    msg.tool_calls = vec![ToolCallRecord {
        thought_signature: None,
        images: 0,
        id: "c1".into(),
        name: "update_self_model".into(),
        arguments: serde_json::json!({}),
        result: Some("ok".into()),
        subagent: None,
    }];
    orch.handle_done(super::super::generation::GenResult {
        usage: None,
        continuation: None,
        id: gen_id,
        chat_id,
        messages: vec![msg],
        effects: vec![],
        deleted: vec![],
    });
    let mut changed = false;
    while let Ok(e) = rx.try_recv() {
        if matches!(e, AppEvent::SelfModelChanged) {
            changed = true;
        }
    }
    assert!(changed);
}
