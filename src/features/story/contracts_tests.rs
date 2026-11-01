use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{
    CanonKeys, InitiativeRuling, IntentVerdict, Property, ReactionsRuling, ReflectionReply, Schema,
    StorySchema,
};
use crate::entities::story::{Canon, Effect, Severity, fixture_canon};
use crate::features::story::dialect;
use crate::features::story::schema::describe;
use crate::features::story::schema::tests::instances;

fn keys() -> CanonKeys {
    CanonKeys::of(&fixture_canon())
}

/// Every instance the schema allows reads as `T` and writes back the same,
/// and the schema keeps the dialect.
fn holds<T: DeserializeOwned + Serialize>(schema: &Schema) -> usize {
    // A union is never a root (spec §8): one is checked as a property.
    let root = match schema {
        Schema::Object(_) => schema.clone(),
        _ => Schema::Object(vec![Property::new("value", schema.clone())]),
    };
    assert_eq!(dialect::check(&root.to_value()), Vec::new());
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
fn every_reply_schema_keeps_the_dialect_and_reads_as_its_type() {
    let keys = keys();
    assert!(holds::<ReflectionReply>(&ReflectionReply::schema(&keys)) >= 2);
    // Six effects, each with its enums; the take's null holder.
    assert!(holds::<Effect>(&Effect::schema(&keys)) > 20);
    // A list's instance holds every instance of its items, so the rulings'
    // lists carry every effect.
    assert!(holds::<InitiativeRuling>(&InitiativeRuling::schema(&keys)) >= 5);
    assert!(holds::<ReactionsRuling>(&ReactionsRuling::schema(&keys)) >= 2);
}

#[test]
fn the_rulings_put_the_cause_before_the_verdict_and_the_event_after_both() {
    let text = InitiativeRuling::schema(&keys()).to_json();
    let at = |needle: &str| text.find(needle).unwrap_or_else(|| panic!("{needle}"));
    assert!(at("\"cause\"") < at("\"verdict\""));
    assert!(at("\"rulings\"") < at("\"initiator\""));
    assert!(at("\"initiator\"") < at("\"observable\""));
    assert!(at("\"observable\"") < at("\"effects\""));
    assert!(at("\"effects\"") < at("\"world_event\""));
    assert!(at("\"world_event\"") < at("\"scene_over\""));
    let text = ReactionsRuling::schema(&keys()).to_json();
    assert!(text.contains(r#""required":["character","cause","verdict","observable","effects"]"#));
    // No field asks for the model's own reasoning (spec §8).
    for schema in [
        InitiativeRuling::schema(&keys()),
        ReactionsRuling::schema(&keys()),
        ReflectionReply::schema(&keys()),
    ] {
        let value = schema.to_value();
        let names = property_names(&value);
        for banned in ["reason", "thinking", "trace", "rationale"] {
            assert!(
                names.iter().all(|n| !n.contains(banned)),
                "{banned} in {names:?}"
            );
        }
    }
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
fn the_world_master_never_places_and_a_canon_without_items_names_none() {
    let text = Effect::schema(&keys()).to_json();
    assert!(
        !text.contains(r#""enum":["place"]"#),
        "a placement is the opening's"
    );
    for kind in ["move", "give", "take", "drop", "reveal", "injure"] {
        assert!(text.contains(&format!(r#""enum":["{kind}"]"#)), "{kind}");
    }
    assert!(text.contains(r#""enum":["lantern","crate","ledger","warehouse_key","logbook"]"#));
    assert!(text.contains(r#""enum":["bribe","rifles","signal"]"#));

    let mut bare: Canon = fixture_canon();
    for location in &mut bare.locations {
        location.items.clear();
    }
    for character in &mut bare.characters {
        character.start.holds.clear();
        character.secrets.clear();
    }
    let schema = Effect::schema(&CanonKeys::of(&bare));
    let text = schema.to_json();
    for kind in ["give", "take", "drop", "reveal"] {
        assert!(!text.contains(&format!(r#""enum":["{kind}"]"#)), "{kind}");
    }
    assert!(holds::<Effect>(&schema) > 3);
    // The ruling built on it keeps the dialect too.
    assert_eq!(
        dialect::check(&InitiativeRuling::schema(&CanonKeys::of(&bare)).to_value()),
        Vec::new()
    );
}

#[test]
fn verdicts_and_severities_read_in_any_case() {
    let ruling = serde_json::json!({
        "rulings": [{"character": "Mara", "cause": "x", "verdict": "FEASIBLE"}],
        "initiator": "MARA",
        "observable": "Mara nods.",
        "effects": [{"type": "Injure", "character": "silas", "severity": "Minor"}],
        "world_event": null,
        "scene_over": false
    });
    let checked = InitiativeRuling::schema(&keys()).validate(&ruling).unwrap();
    let parsed: InitiativeRuling = serde_json::from_value(checked).unwrap();
    assert_eq!(parsed.rulings[0].verdict, IntentVerdict::Feasible);
    assert_eq!(parsed.initiator, Some("mara".into()));
    assert_eq!(
        parsed.effects,
        [Effect::Injure {
            character: "silas".into(),
            severity: Severity::Minor
        }]
    );
}
