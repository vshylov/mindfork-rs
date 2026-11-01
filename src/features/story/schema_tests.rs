use serde_json::{Map, Value, json};

use super::{CanonKeys, Property, Schema, StorySchema, Variant, describe, extract_object};
use crate::entities::story::{Action, CharacterKey, Proposal, Visibility, fixture_canon};
use crate::features::story::dialect;

fn keys() -> CanonKeys {
    CanonKeys::of(&fixture_canon())
}

fn proposal_schema() -> Schema {
    Proposal::schema(&keys())
}

/// One instance per value of every enum and per branch of every union, the
/// other properties at their first choice: every shape the schema allows.
pub(crate) fn instances(schema: &Schema) -> Vec<Value> {
    match schema {
        Schema::String => vec![json!("x")],
        Schema::Enum(values) => values.iter().map(|v| json!(v)).collect(),
        Schema::Integer => vec![json!(1)],
        Schema::Boolean => vec![json!(true), json!(false)],
        Schema::Array(items) => vec![json!([]), Value::Array(instances(items))],
        Schema::Nullable(inner) => {
            let mut list = instances(inner);
            list.push(Value::Null);
            list
        }
        Schema::Object(properties) => object_instances(properties, None),
        Schema::Tagged { tag, variants } => variants
            .iter()
            .flat_map(|v| object_instances(&v.properties, Some((tag, &v.name))))
            .collect(),
    }
}

fn object_instances(properties: &[Property], tag: Option<(&str, &str)>) -> Vec<Value> {
    let options: Vec<Vec<Value>> = properties.iter().map(|p| instances(&p.schema)).collect();
    let build = |choice: &[usize]| {
        let mut object = Map::new();
        if let Some((tag, name)) = tag {
            object.insert(tag.to_owned(), json!(name));
        }
        for (i, property) in properties.iter().enumerate() {
            object.insert(property.name.clone(), options[i][choice[i]].clone());
        }
        Value::Object(object)
    };
    let mut list = vec![build(&vec![0; properties.len()])];
    for (i, choices) in options.iter().enumerate() {
        for j in 1..choices.len() {
            let mut choice = vec![0; properties.len()];
            choice[i] = j;
            list.push(build(&choice));
        }
    }
    list
}

fn proposal(action: Action, visibility: Visibility) -> Proposal {
    Proposal {
        private_thought: "The gate shuts soon.".to_owned(),
        action,
        targets: vec!["ines".into()],
        visibility,
    }
}

fn samples() -> Vec<Proposal> {
    let actions = [
        Action::Speak {
            line: "Not tonight.".to_owned(),
        },
        Action::Move { to: "inn".into() },
        Action::Act {
            verb: "slams".to_owned(),
            object: Some("the ledger".to_owned()),
            line: None,
        },
        Action::Act {
            verb: "shrugs".to_owned(),
            object: None,
            line: Some("Storm's early.".to_owned()),
        },
        Action::Wait,
    ];
    let visibilities = [
        Visibility::Public,
        Visibility::Whisper {
            to: vec!["silas".into()],
        },
        Visibility::Hidden,
    ];
    actions
        .iter()
        .flat_map(|a| visibilities.iter().map(|v| proposal(a.clone(), v.clone())))
        .collect()
}

#[test]
fn the_proposal_schema_keeps_the_dialect() {
    let schema = proposal_schema();
    assert_eq!(dialect::check(&schema.to_value()), Vec::new());
    // The printed text reads back as the same value: nothing is lost in the
    // hand-written serialization.
    let text: Value = serde_json::from_str(&schema.to_json()).unwrap();
    assert_eq!(text, schema.to_value());
}

/// The order of the properties is the order a model writes them in (spec
/// §8): the thought before the action, and a variant's tag before its
/// fields. A `Value` would have sorted them.
#[test]
fn the_printed_schema_keeps_the_declared_order() {
    let text = proposal_schema().to_json();
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle} in {text}"))
    };
    assert!(at("\"private_thought\"") < at("\"action\""));
    assert!(at("\"action\"") < at("\"targets\""));
    assert!(at("\"targets\"") < at("\"visibility\""));
    assert!(text.contains(
        r#""properties":{"type":{"type":"string","enum":["speak"]},"line":{"type":"string""#
    ));
    assert!(text.contains(r#""required":["private_thought","action","targets","visibility"]"#));
    assert!(text.contains(r#""required":["type","verb","object","line"]"#));
    // The cast and the map are enums, in the canon's order.
    assert!(text.contains(r#""enum":["mara","silas","ines"]"#));
    assert!(text.contains(r#""enum":["harbour","inn","lighthouse","chapel","warehouse"]"#));
}

#[test]
fn every_value_of_the_reply_type_meets_its_schema() {
    let schema = proposal_schema();
    for sample in samples() {
        let value = serde_json::to_value(&sample).unwrap();
        assert_eq!(
            schema.validate(&value),
            Ok(value.clone()),
            "{value} should meet the schema"
        );
        assert_eq!(serde_json::from_value::<Proposal>(value).unwrap(), sample);
    }
}

#[test]
fn every_instance_of_the_schema_reads_as_its_type() {
    let schema = proposal_schema();
    let all = instances(&schema);
    // 4 actions, 3 visibilities, 3 + 1 targets lists, the act's nullables.
    assert!(all.len() >= 10, "{}", all.len());
    for instance in all {
        let checked = schema
            .validate(&instance)
            .unwrap_or_else(|v| panic!("{instance}: {}", describe(&v)));
        let parsed: Proposal = serde_json::from_value(checked.clone())
            .unwrap_or_else(|e| panic!("{checked} does not read as a Proposal: {e}"));
        assert_eq!(serde_json::to_value(parsed).unwrap(), checked);
    }
}

#[test]
fn the_validator_refuses_what_serde_reads_in_silence() {
    let schema = proposal_schema();
    // A stray field on a variant without fields: serde reads it as `Public`
    // under `deny_unknown_fields` (entities::story::event's tests).
    let stray = json!({
        "private_thought": "",
        "action": {"type": "wait"},
        "targets": [],
        "visibility": {"type": "public", "to": ["ines"]}
    });
    let found = schema.validate(&stray).unwrap_err();
    assert_eq!(describe(&found), "- $.visibility.to: is not in the schema");
}

#[test]
fn the_validator_reports_every_violation_at_once() {
    let schema = proposal_schema();
    let reply = json!({
        "action": {"type": "speak", "line": 3},
        "targets": "ines",
        "visibility": {"type": "shout"},
        "mood": "grim"
    });
    let found = describe(&schema.validate(&reply).unwrap_err());
    assert_eq!(
        found,
        "- $.private_thought: missing\n\
         - $.action.line: expected text, found a number\n\
         - $.targets: expected a list, found text\n\
         - $.visibility.type: \"shout\" is not one of \"public\", \"whisper\", \"hidden\"\n\
         - $.mood: is not in the schema"
    );
    let cases = [
        (
            json!({"type": "move", "to": "the moon"}),
            "is not one of \"harbour\"",
        ),
        (
            json!({"line": "hi"}),
            "$.action.type: missing; one of \"speak\"",
        ),
        (json!("speak"), "$.action: expected an object, found text"),
        (json!({"type": 7}), "$.action.type: missing"),
    ];
    for (action, expected) in cases {
        let reply = json!({
            "private_thought": "", "action": action, "targets": [],
            "visibility": {"type": "hidden"}
        });
        let found = describe(&schema.validate(&reply).unwrap_err());
        assert!(found.contains(expected), "{found}");
    }
    let found = describe(&schema.validate(&json!([])).unwrap_err());
    assert_eq!(found, "- $: expected an object, found a list");
}

#[test]
fn enum_values_match_without_regard_to_case_and_come_back_spelled() {
    let reply = json!({
        "private_thought": "",
        "action": {"type": "Move", "to": "INN"},
        "targets": ["Ines"],
        "visibility": {"type": "WHISPER", "to": ["Silas"]}
    });
    let checked = proposal_schema().validate(&reply).unwrap();
    let parsed: Proposal = serde_json::from_value(checked).unwrap();
    assert_eq!(parsed.action, Action::Move { to: "inn".into() });
    assert_eq!(parsed.targets, [CharacterKey::from("ines")]);
    assert_eq!(
        parsed.visibility,
        Visibility::Whisper {
            to: vec!["silas".into()]
        }
    );
}

#[test]
fn whole_numbers_and_booleans_are_checked_by_kind() {
    let schema = Schema::Object(vec![
        Property::new("n", Schema::Integer),
        Property::new("over", Schema::Boolean),
    ]);
    assert!(schema.validate(&json!({"n": 3, "over": false})).is_ok());
    let found = describe(
        &schema
            .validate(&json!({"n": 3.5, "over": "no"}))
            .unwrap_err(),
    );
    assert_eq!(
        found,
        "- $.n: expected a whole number, found a number\n\
         - $.over: expected true or false, found text"
    );
    let found = describe(
        &schema
            .validate(&json!({"n": null, "over": true}))
            .unwrap_err(),
    );
    assert_eq!(found, "- $.n: expected a whole number, found null");
    assert_eq!(dialect::check(&schema.to_value()), Vec::new());
    assert!(schema.to_json().contains(r#"{"type":"integer"}"#));
}

/// A tagged union that may be absent prints as one union, its variants and
/// then null, and never as a union inside a union.
#[test]
fn a_nullable_union_prints_flat() {
    let tagged = Schema::Tagged {
        tag: "kind".to_owned(),
        variants: vec![
            Variant::new("a", "First.", Vec::new()),
            Variant::new("b", "Second.", Vec::new()),
        ],
    };
    let schema = Schema::Object(vec![
        Property::described("x", "Maybe one.", Schema::Nullable(Box::new(tagged))),
        Property::new(
            "y",
            Schema::Nullable(Box::new(Schema::Nullable(Box::new(Schema::String)))),
        ),
    ]);
    assert_eq!(dialect::check(&schema.to_value()), Vec::new());
    let value = schema.to_value();
    let x = &value["properties"]["x"];
    assert_eq!(x["description"], "Maybe one.");
    assert_eq!(x["anyOf"].as_array().unwrap().len(), 3);
    assert_eq!(x["anyOf"][2], json!({"type": "null"}));
    assert_eq!(
        value["properties"]["y"]["anyOf"],
        json!([{"type": "string"}, {"type": "null"}])
    );
    assert_eq!(
        schema.validate(&json!({"x": null, "y": null})),
        Ok(json!({"x": null, "y": null}))
    );
    assert_eq!(
        schema.validate(&json!({"x": {"kind": "B"}, "y": "z"})),
        Ok(json!({"x": {"kind": "b"}, "y": "z"}))
    );
}

#[test]
fn the_reply_is_read_leniently() {
    let object = json!({"a": 1, "b": "}"});
    let cases = [
        r#"{"a": 1, "b": "}"}"#.to_owned(),
        "  \n{\"a\": 1, \"b\": \"}\"}\n".to_owned(),
        "```json\n{\"a\": 1, \"b\": \"}\"}\n```".to_owned(),
        "Here it is:\n```\n{\"a\": 1, \"b\": \"}\"}\n```\nHope that helps.".to_owned(),
        "Sure. {\"a\": 1, \"b\": \"}\"} And that's all.".to_owned(),
        "{not json} then {\"a\": 1, \"b\": \"}\"}".to_owned(),
    ];
    for reply in cases {
        assert_eq!(extract_object(&reply), Some(object.clone()), "{reply}");
    }
    assert_eq!(extract_object("no object here"), None);
    assert_eq!(extract_object("[1, 2]"), None);
    assert_eq!(extract_object(""), None);
    assert_eq!(extract_object("{\"a\": "), None);
}
