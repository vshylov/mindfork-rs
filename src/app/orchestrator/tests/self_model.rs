//! Orchestrator tests — the self-model: injection, F3 edits, signals. Part of the [`super`]
//! module (fixtures in mod.rs). See docs/history/refactoring-god-objects.md, stage 3.

use super::*;
use crate::entities::self_model::ObservationPick;
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
            ObservationPick::Nearest,
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
        ObservationPick::Nearest,
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
            ObservationPick::Nearest,
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
        ObservationPick::Nearest,
        crate::shared::i18n::locale(crate::shared::i18n::Lang::Ru),
    )
    .unwrap();
    assert!(only_obs.contains("Наблюдения, ближайшие к теме разговора, и последнее:"));
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
            ObservationPick::Nearest,
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
        ObservationPick::Nearest,
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
        ObservationPick::Nearest,
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
        ObservationPick::Nearest,
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
        ObservationPick::Nearest,
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
        ObservationPick::Nearest,
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
    let (recent, pick) =
        injection_recent(&storage, &embedder, profile, true, "xxxx", &params).await;
    assert!(
        recent.iter().any(|s| s.text.contains("xxxx старое")),
        "the relevant old observation should be surfaced: {recent:?}"
    );
    assert_eq!(pick, ObservationPick::Nearest);
    // A query about "yyyy" → the irrelevant old X isn't surfaced.
    let (recent, _) = injection_recent(&storage, &embedder, profile, true, "yyyy", &params).await;
    assert!(!recent.iter().any(|s| s.text.contains("xxxx")));
    // Injection disabled → empty.
    assert!(
        injection_recent(&storage, &embedder, profile, false, "xxxx", &params)
            .await
            .0
            .is_empty()
    );
}

#[tokio::test]
async fn a_follow_up_on_another_subject_leaves_the_system_prompt_as_it_was() {
    // F1 (e1), docs/research/prompt-caching.md §2.2: the observations are picked by
    // the chat's opening, so the self-model block — the end of `system`, ahead of
    // the whole conversation — is byte-identical on the next turn, and the
    // provider's cache holds. Picked by the latest message (the control), the
    // follow-up would have changed it.
    use super::super::generation::{inject_self_model, injection_recent};
    use crate::app::orchestrator::request::observation_query;
    use crate::entities::note::Note;
    use crate::entities::self_model::SelfModelParams;
    use crate::features::tools::notes::SELF_NOTE_TAG;
    use chrono::{Duration, Utc};

    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::open(Paths::with_root(dir.path())).unwrap();
    let embedder = MockEmbedder::new(16);
    let profile = Uuid::new_v4();
    let now = Utc::now();
    // Two old observations on two subjects.
    for (content, days) in [
        ("xxxx about the first subject", 10),
        ("yyyy about the second", 11),
    ] {
        let at = now - Duration::days(days);
        let note = Note {
            id: Uuid::new_v4(),
            profile_id: profile,
            content: content.into(),
            tags: vec![SELF_NOTE_TAG.to_string()],
            created_at: at,
            updated_at: at,
        };
        storage.db().note_insert(&note).unwrap();
        let emb = embedder
            .embed(vec![content.into()], EmbedRole::Passage)
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
    // Both observations in the prompt, in the order of relevance — so the query
    // decides which one leads the section. (With one, the guaranteed freshest
    // observation would displace the pick and hide the difference.)
    let params = SelfModelParams {
        narrative_in_prompt: 2,
        ..SelfModelParams::default()
    };
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
    let system_for = async |query: &str| {
        let (recent, pick) =
            injection_recent(&storage, &embedder, profile, true, query, &params).await;
        inject_self_model(
            Some("You are X.".into()),
            None,
            true,
            false,
            &params,
            now,
            &recent,
            pick,
            loc,
        )
        .expect("a block with an observation")
    };
    let nearest = "Observations nearest this conversation, then the latest:";
    let latest = "\nLatest observations:\n";

    let p = Profile::new("X", "You are X.");
    let mut chat = Chat::from_profile(&p, "c");
    chat.push_message(Message::user("xxxx"));
    let first = system_for(&observation_query(&chat, true)).await;
    chat.push_message(Message::assistant("ok"));
    chat.push_message(Message::user("yyyy"));
    let second = system_for(&observation_query(&chat, true)).await;
    assert_eq!(first, second, "the follow-up changed the system prompt");
    let lead = |s: &str| s.find("xxxx about").unwrap() < s.find("yyyy about").unwrap();
    assert!(lead(&first), "the opening's observation leads: {first}");
    assert!(first.contains(nearest), "picked by the opening: {first}");
    // The control: picked by the latest message, the second turn's head differs.
    assert_ne!(first, system_for("yyyy").await);

    // An opening near no observation (F (b)): the latest ones, under their own
    // header, on every turn — a follow-up that would clear the floor moves
    // neither the arm nor the header.
    let mut chat = Chat::from_profile(&p, "c");
    chat.push_message(Message::user("qqqq"));
    let first = system_for(&observation_query(&chat, true)).await;
    chat.push_message(Message::assistant("ok"));
    chat.push_message(Message::user("yyyy"));
    let second = system_for(&observation_query(&chat, true)).await;
    assert_eq!(first, second, "the follow-up changed the system prompt");
    assert!(
        first.contains(latest) && !first.contains(nearest),
        "{first}"
    );
    assert!(lead(&first), "the latest observation leads: {first}");
    // The control: picked by the follow-up, it would have taken the other arm.
    assert!(system_for("yyyy").await.contains(nearest));
}

/// Embeds every text as the same vector, the query's, so a test gives each
/// observation its cosine to the query directly ([`observations_at`]).
struct QueryVector;

#[async_trait::async_trait]
impl crate::shared::api::Embedder for QueryVector {
    async fn embed(&self, texts: Vec<String>, _role: EmbedRole) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
    }
}

/// Seeds `@self` observations, each at a cosine to [`QueryVector`]'s query and an
/// age in days, into a store under `dir`.
fn observations_at(
    dir: &std::path::Path,
    profile: Uuid,
    seeds: &[(&str, f32, i64)],
) -> crate::shared::storage::Storage {
    use crate::entities::note::Note;
    use crate::features::tools::notes::SELF_NOTE_TAG;
    let storage = Storage::open(Paths::with_root(dir)).unwrap();
    let now = chrono::Utc::now();
    for (content, cosine, days) in seeds {
        let at = now - chrono::Duration::days(*days);
        let note = Note {
            id: Uuid::new_v4(),
            profile_id: profile,
            content: (*content).into(),
            tags: vec![SELF_NOTE_TAG.to_string()],
            created_at: at,
            updated_at: at,
        };
        storage.db().note_insert(&note).unwrap();
        let vector = vec![*cosine, (1.0 - cosine * cosine).sqrt()];
        storage
            .db()
            .note_vector_upsert(note.id, profile, &vector)
            .unwrap();
    }
    storage
}

/// F (b): an opening whose nearest observation is under the floor takes the
/// latest observations for the whole chat, under their own header; one above it
/// keeps the pick by relevance, and the picks after the lead need not clear the
/// floor (docs/history/self-model-observations.md §6).
#[tokio::test]
async fn an_opening_under_the_floor_takes_the_latest_observations() {
    use super::super::generation::{inject_self_model, injection_recent};
    use crate::entities::self_model::SelfModelParams;
    let params = SelfModelParams::default();
    let loc = crate::shared::i18n::locale(crate::shared::i18n::Lang::En);
    let profile = Uuid::new_v4();
    let pick_for = async |lead: f32| {
        let dir = tempfile::tempdir().unwrap();
        let storage = observations_at(
            dir.path(),
            profile,
            &[
                ("old lead", lead, 30),
                ("old second", 0.40, 40),
                ("freshest", 0.10, 1),
                ("second freshest", 0.05, 2),
            ],
        );
        let (recent, pick) =
            injection_recent(&storage, &QueryVector, profile, true, "an opening", &params).await;
        let texts: Vec<String> = recent.iter().map(|s| s.text.clone()).collect();
        let system = inject_self_model(
            None,
            None,
            true,
            false,
            &params,
            chrono::Utc::now(),
            &recent,
            pick,
            loc,
        )
        .unwrap();
        (texts, pick, system)
    };

    let (texts, pick, system) = pick_for(0.45).await;
    assert_eq!(pick, ObservationPick::Latest);
    assert_eq!(texts[..2], ["freshest", "second freshest"], "{texts:?}");
    assert!(system.contains("\nLatest observations:\n"), "{system}");
    assert!(!system.contains("nearest"), "{system}");

    let (texts, pick, system) = pick_for(0.55).await;
    assert_eq!(pick, ObservationPick::Nearest);
    // The second pick is under the floor and kept: the floor is on the lead only.
    assert_eq!(texts, ["old lead", "old second", "freshest"]);
    assert!(
        system.contains("Observations nearest this conversation, then the latest:"),
        "{system}"
    );
}

/// The floor is a position in bge-m3's scale, read in the embedder's own: on a
/// model whose cosines run higher (multilingual-e5's measured range) the same
/// lead falls under it.
#[tokio::test]
async fn the_floor_is_read_in_the_embedders_scale() {
    use super::super::generation::injection_recent;
    use crate::entities::self_model::SelfModelParams;
    use crate::shared::embed_calibration::Calibration;
    let params = SelfModelParams::default();
    let profile = Uuid::new_v4();
    let dir = tempfile::tempdir().unwrap();
    let storage = observations_at(
        dir.path(),
        profile,
        &[("old lead", 0.70, 30), ("freshest", 0.10, 1)],
    );
    let pick = async || {
        injection_recent(&storage, &QueryVector, profile, true, "an opening", &params)
            .await
            .1
    };
    assert_eq!(
        pick().await,
        ObservationPick::Nearest,
        "0.70 on bge-m3's scale"
    );
    storage
        .db()
        .set_embed_calibration(&Calibration {
            unrelated: 0.7897,
            paraphrase: 0.9456,
        })
        .unwrap();
    // 0.50 maps to about 0.82 there.
    assert_eq!(pick().await, ObservationPick::Latest, "0.70 on e5's scale");
}

/// One place in the prompt holds the guaranteed freshest observation whatever
/// the relevance pick, so it is the latest one and says so.
#[tokio::test]
async fn one_place_takes_the_latest_observation() {
    use super::super::generation::injection_recent;
    use crate::entities::self_model::SelfModelParams;
    let params = SelfModelParams {
        narrative_in_prompt: 1,
        ..SelfModelParams::default()
    };
    let profile = Uuid::new_v4();
    let dir = tempfile::tempdir().unwrap();
    let storage = observations_at(
        dir.path(),
        profile,
        &[("old lead", 0.90, 30), ("freshest", 0.10, 1)],
    );
    let (recent, pick) =
        injection_recent(&storage, &QueryVector, profile, true, "an opening", &params).await;
    assert_eq!(pick, ObservationPick::Latest);
    assert_eq!(recent[0].text, "freshest");
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
        .self_model_update(
            pid,
            &crate::entities::self_model_history::VersionAuthor::Person,
            |m| {
                m.summary = "о себе".into();
                true
            },
        )
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
    // The blob is gone, and its history with it (spec §17.9).
    assert!(orch.storage.db().self_model_get(pid).unwrap().is_none());
    assert!(
        orch.storage
            .db()
            .self_model_history(pid)
            .unwrap()
            .is_empty()
    );
}

/// An edit on the `F3` screen is a version of the person's.
#[test]
fn f3_edit_is_recorded_as_the_persons_version() {
    use crate::entities::self_model::SelfModelEdit;
    use crate::entities::self_model_history::VersionAuthor;
    let (_d, orch, pid) = orch_with_active_profile();
    orch.handle_update_self_model(SelfModelEdit::SetSummary("I am Sol.".into()));
    let history = orch.storage.db().self_model_history(pid).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].author, VersionAuthor::Person);
    assert_eq!(history[0].model.summary, "I am Sol.");
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
        images_withheld: 0,
        anchor: None,
        woken: false,
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

/// The history's answer (spec §17.9): every version oldest first, the titles
/// of the chats a model wrote in, and the profile's model history.
#[test]
fn history_request_answers_versions_titles_and_models() {
    use crate::entities::profile::LlmChange;
    use crate::entities::self_model_history::VersionAuthor;
    use crate::shared::config::ServerMode;
    let (_d, mut orch, mut rx) = bare_orch_rx();
    let profile = Profile::new("P", "sys");
    let pid = profile.id;
    let chat = Chat::from_profile(&profile, "Chess openings");
    let chat_id = chat.id;
    orch.profiles.push(profile);
    orch.chats.push(chat);
    orch.active_id = Some(chat_id);
    let db = orch.storage.db();
    let turn = VersionAuthor::Model {
        name: Some("gemma-4-31b".into()),
        mode: ServerMode::External,
        chat_id: Some(chat_id),
        background: false,
    };
    db.self_model_update(pid, &turn, |m| {
        m.summary = "I am Sol.".into();
        true
    })
    .unwrap();
    let gone = VersionAuthor::Model {
        name: Some("gemma-4-31b".into()),
        mode: ServerMode::External,
        chat_id: Some(Uuid::new_v4()),
        background: true,
    };
    db.self_model_update(pid, &gone, |m| {
        m.add_goal("learn chess");
        true
    })
    .unwrap();
    db.llm_history_note(
        pid,
        &LlmChange {
            changed_at: chrono::Utc::now(),
            model: "gemma-4-31b".into(),
            mode: ServerMode::External,
        },
    )
    .unwrap();

    orch.handle_request_self_model_history();
    let view = loop {
        match rx.try_recv().expect("no SelfModelHistory event") {
            AppEvent::SelfModelHistory(view) => break view,
            _ => continue,
        }
    };
    let authors: Vec<&VersionAuthor> = view.versions.iter().map(|v| &v.author).collect();
    assert_eq!(authors, vec![&turn, &gone]);
    // Only a chat that still exists has a title to show.
    assert_eq!(view.chat_titles.len(), 1);
    assert_eq!(view.chat_titles[&chat_id], "Chess openings");
    assert_eq!(view.llm.len(), 1);
}
