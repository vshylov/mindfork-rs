use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{
    BranchReading, ContinuityReply, FindingKind, NextScene, OutlineReply, PickChoice, PickReply,
    PlanReply, ReaderReply, TriageReply,
};
use crate::entities::story::NoteKind;
use crate::entities::story::fixture_canon;
use crate::features::story::dialect;
use crate::features::story::schema::describe;
use crate::features::story::schema::tests::instances;
use crate::features::story::schema::{CanonKeys, Schema, StorySchema};

fn keys() -> CanonKeys {
    CanonKeys::of(&fixture_canon())
}

fn labels(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

/// Every instance the schema allows reads as `T` and writes back the same,
/// and the schema keeps the dialect.
fn holds<T: DeserializeOwned + Serialize>(schema: &Schema) -> usize {
    assert_eq!(dialect::check(&schema.to_value()), Vec::new());
    let all = instances(schema);
    for instance in &all {
        let checked = schema
            .validate(instance)
            .unwrap_or_else(|v| panic!("{instance}: {}", describe(&v)));
        let parsed: T = serde_json::from_value(checked.clone())
            .unwrap_or_else(|e| panic!("{checked} does not read as its type: {e}"));
        assert_eq!(serde_json::to_value(parsed).unwrap(), checked);
    }
    all.len()
}

#[test]
fn every_author_and_reviewer_schema_keeps_the_dialect_and_reads_as_its_type() {
    let keys = keys();
    assert!(holds::<OutlineReply>(&OutlineReply::schema(&keys)) >= 1);
    assert!(holds::<PlanReply>(&PlanReply::schema_for(&keys, false)) >= 1);
    assert!(holds::<PlanReply>(&PlanReply::schema_for(&keys, true)) >= 2);
    let branches = labels(&["k0", "k2"]);
    assert!(holds::<PickReply>(&PickReply::schema_for(&keys, &branches, true)) >= 2);
    assert!(holds::<PickReply>(&PickReply::schema_for(&keys, &branches, false)) >= 1);
    assert!(holds::<ReaderReply>(&ReaderReply::schema(&keys)) >= 1);
    assert!(holds::<ContinuityReply>(&ContinuityReply::schema(&keys)) >= 1);
    let notes = labels(&["n1", "n2", "n3"]);
    assert!(holds::<TriageReply>(&TriageReply::schema_for(&keys, &notes)) >= 1);
}

#[test]
fn the_deciding_fields_come_last() {
    let keys = keys();
    let order = |text: &str, names: &[&str]| {
        let at: Vec<usize> = names
            .iter()
            .map(|n| {
                text.find(&format!("\"{n}\""))
                    .unwrap_or_else(|| panic!("{n} in {text}"))
            })
            .collect();
        assert!(at.windows(2).all(|w| w[0] < w[1]), "{names:?} in {text}");
    };
    let plan = PlanReply::schema_for(&keys, true).to_json();
    order(
        &plan,
        &["intent", "location", "present", "just_happened", "pressure"],
    );
    order(&plan, &["next", "assumptions"]);
    let pick = PickReply::schema_for(&keys, &labels(&["k0", "k1"]), true).to_json();
    order(&pick, &["readings", "human_note", "choice", "assumptions"]);
    order(
        &pick,
        &["note", "tension", "surprise", "truth", "consequences"],
    );
    let continuity = ContinuityReply::schema(&keys).to_json();
    order(
        &continuity,
        &["anchor", "evidence", "note", "kind", "severity"],
    );
    let reader = ReaderReply::schema(&keys).to_json();
    order(&reader, &["anchor", "note", "severity"]);
    let triage = TriageReply::schema_for(&keys, &labels(&["n1"])).to_json();
    order(&triage, &["note", "critique", "decision"]);
}

fn property_names(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(props) = value.get("properties").and_then(Value::as_object) {
        out.extend(props.keys().cloned());
        for v in props.values() {
            out.extend(property_names(v));
        }
    }
    for key in ["items", "anyOf"] {
        match value.get(key) {
            Some(Value::Array(list)) => list.iter().for_each(|v| out.extend(property_names(v))),
            Some(v) => out.extend(property_names(v)),
            None => {}
        }
    }
    out
}

#[test]
fn no_field_is_named_like_the_models_reasoning() {
    let keys = keys();
    let branches = labels(&["k0"]);
    for schema in [
        OutlineReply::schema(&keys),
        PlanReply::schema_for(&keys, true),
        PickReply::schema_for(&keys, &branches, true),
        ReaderReply::schema(&keys),
        ContinuityReply::schema(&keys),
        TriageReply::schema_for(&keys, &labels(&["n1"])),
    ] {
        let names = property_names(&schema.to_value());
        for banned in ["reason", "thinking", "thought", "trace", "rationale"] {
            assert!(
                names.iter().all(|n| !n.contains(banned)),
                "{banned} in {names:?}"
            );
        }
    }
}

#[test]
fn a_calls_options_shape_its_schema() {
    let keys = keys();
    let first = PlanReply::schema_for(&keys, false).to_json();
    assert!(
        !first.contains(r#""enum":["end"]"#),
        "the first scene cannot end"
    );
    assert!(
        PlanReply::schema_for(&keys, true)
            .to_json()
            .contains(r#""enum":["end"]"#)
    );
    let ended = json!({"next": {"type": "end"}, "assumptions": []});
    assert!(
        PlanReply::schema_for(&keys, false)
            .validate(&ended)
            .is_err()
    );
    assert!(PlanReply::schema_for(&keys, true).validate(&ended).is_ok());

    let shown = labels(&["k0", "k2"]);
    let last = PickReply::schema_for(&keys, &shown, false).to_json();
    assert!(!last.contains(r#""enum":["rerun"]"#), "no rerun is left");
    assert!(
        last.contains(r#""enum":["k0","k2"]"#),
        "only the branches shown"
    );
    let pick = |branch: &str| {
        json!({
            "readings": [],
            "human_note": "n",
            "choice": {"type": "branch", "branch": branch},
            "assumptions": [],
        })
    };
    let schema = PickReply::schema_for(&keys, &shown, true);
    assert!(schema.validate(&pick("k2")).is_ok());
    assert!(schema.validate(&pick("k1")).is_err(), "k1 did not play");

    let triage = TriageReply::schema_for(&keys, &labels(&["n1", "n2"]));
    let entry = |note: &str| json!({"decisions": [{"note": note, "critique": "c", "decision": "accepted"}]});
    assert!(triage.validate(&entry("n2")).is_ok());
    assert!(triage.validate(&entry("n3")).is_err());
}

#[test]
fn a_readings_scores_are_held_to_one_to_five() {
    let reading = BranchReading {
        branch: "k0".into(),
        note: "n".into(),
        tension: 9,
        surprise: 0,
        truth: 3,
        consequences: -4,
    };
    assert_eq!(reading.total(), 5 + 1 + 3 + 1);
}

#[test]
fn a_draft_and_a_rerun_carry_the_same_scene() {
    let value = json!({
        "intent": "Ines doubts.",
        "location": "harbour",
        "present": ["mara", "ines"],
        "just_happened": "The ferry is cancelled.",
        "pressure": "The gate closes.",
    });
    let mut planned = value.clone();
    planned["type"] = json!("scene");
    let next: NextScene = serde_json::from_value(planned).unwrap();
    let mut rerun = value.clone();
    rerun["type"] = json!("rerun");
    let choice: PickChoice = serde_json::from_value(rerun).unwrap();
    let draft = next.draft().unwrap();
    assert_eq!(Some(draft.clone()), choice.rerun());
    assert_eq!(draft.setup().present.len(), 2);
    assert_eq!(NextScene::End.draft(), None);
    let kept = PickChoice::Branch {
        branch: "k1".into(),
    };
    assert_eq!(kept.rerun(), None);
    assert_eq!(FindingKind::Timeline.note_kind(), NoteKind::Timeline);
}
