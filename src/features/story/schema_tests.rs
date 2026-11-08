//! The schema dialect and its validator, held to a reply type of the tests'
//! own: an object with a tagged union, a list of enum values and a value that
//! may be absent — every shape a reply's schema takes.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{Property, Schema, Variant, describe, extract_object};
use crate::features::story::dialect;

/// A reply as a model would write one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sample {
    thought: String,
    #[serde(rename = "move")]
    act: Act,
    targets: Vec<String>,
    note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Act {
    Speak { line: String },
    Go { to: String },
    Wait,
}

fn sample_schema() -> Schema {
    Schema::Object(vec![
        Property::new("thought", Schema::String),
        Property::new(
            "move",
            Schema::Tagged {
                tag: "type".to_owned(),
                variants: vec![
                    Variant::new(
                        "speak",
                        "A line said aloud.",
                        vec![Property::new("line", Schema::String)],
                    ),
                    Variant::new(
                        "go",
                        "A walk to a place.",
                        vec![Property::new(
                            "to",
                            Schema::Enum(vec!["harbour".to_owned(), "inn".to_owned()]),
                        )],
                    ),
                    Variant::new("wait", "Nothing done.", Vec::new()),
                ],
            },
        ),
        Property::new(
            "targets",
            Schema::Array(Box::new(Schema::Enum(
                ["mara", "silas", "ines"].map(str::to_owned).to_vec(),
            ))),
        ),
        Property::new("note", Schema::Nullable(Box::new(Schema::String))),
    ])
}

/// One instance per value of every enum and per branch of every union, the
/// other properties at their first choice: every shape the schema allows.
fn instances(schema: &Schema) -> Vec<Value> {
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

#[test]
fn the_sample_schema_keeps_the_dialect() {
    let schema = sample_schema();
    assert_eq!(dialect::check(&schema.to_value()), Vec::new());
    // The printed text reads back as the same value: nothing is lost in the
    // hand-written serialization.
    let text: Value = serde_json::from_str(&schema.to_json()).unwrap();
    assert_eq!(text, schema.to_value());
}

/// The order of the properties is the order a model writes them in (spec
/// §8), and a variant's tag comes before its fields. A `Value` would have
/// sorted them.
#[test]
fn the_printed_schema_keeps_the_declared_order() {
    let text = sample_schema().to_json();
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle} in {text}"))
    };
    assert!(at("\"thought\"") < at("\"move\""));
    assert!(at("\"move\"") < at("\"targets\""));
    assert!(at("\"targets\"") < at("\"note\""));
    assert!(text.contains(
        r#""properties":{"type":{"type":"string","enum":["speak"]},"line":{"type":"string""#
    ));
    assert!(text.contains(r#""required":["thought","move","targets","note"]"#));
    assert!(text.contains(r#""enum":["mara","silas","ines"]"#));
}

#[test]
fn every_instance_of_the_schema_reads_as_its_type() {
    let schema = sample_schema();
    let all = instances(&schema);
    // 3 moves, 3 + 1 targets lists, a note or none.
    assert!(all.len() >= 6, "{}", all.len());
    for instance in all {
        let checked = schema
            .validate(&instance)
            .unwrap_or_else(|v| panic!("{instance}: {}", describe(&v)));
        let parsed: Sample = serde_json::from_value(checked.clone())
            .unwrap_or_else(|e| panic!("{checked} does not read as a Sample: {e}"));
        assert_eq!(serde_json::to_value(parsed).unwrap(), checked);
    }
}

/// A stray field on a variant without fields is what serde reads in silence,
/// even under `deny_unknown_fields` (docs/lessons.md §8); the validator
/// refuses it.
#[test]
fn the_validator_refuses_what_serde_reads_in_silence() {
    let stray = json!({
        "thought": "",
        "move": {"type": "wait", "to": "inn"},
        "targets": [],
        "note": null
    });
    let found = sample_schema().validate(&stray).unwrap_err();
    assert_eq!(describe(&found), "- $.move.to: is not in the schema");
}

#[test]
fn the_validator_reports_every_violation_at_once() {
    let schema = sample_schema();
    let reply = json!({
        "move": {"type": "speak", "line": 3},
        "targets": "ines",
        "note": null,
        "mood": "grim"
    });
    let found = describe(&schema.validate(&reply).unwrap_err());
    assert_eq!(
        found,
        "- $.thought: missing\n\
         - $.move.line: expected text, found a number\n\
         - $.targets: expected a list, found text\n\
         - $.mood: is not in the schema"
    );
    let cases = [
        (
            json!({"type": "go", "to": "the moon"}),
            "is not one of \"harbour\"",
        ),
        (
            json!({"line": "hi"}),
            "$.move.type: missing; one of \"speak\"",
        ),
        (json!("speak"), "$.move: expected an object, found text"),
        (json!({"type": 7}), "$.move.type: missing"),
    ];
    for (act, expected) in cases {
        let reply = json!({"thought": "", "move": act, "targets": [], "note": null});
        let found = describe(&schema.validate(&reply).unwrap_err());
        assert!(found.contains(expected), "{found}");
    }
    let found = describe(&schema.validate(&json!([])).unwrap_err());
    assert_eq!(found, "- $: expected an object, found a list");
}

#[test]
fn enum_values_match_without_regard_to_case_and_come_back_spelled() {
    let reply = json!({
        "thought": "",
        "move": {"type": "GO", "to": "INN"},
        "targets": ["Ines"],
        "note": null
    });
    let checked = sample_schema().validate(&reply).unwrap();
    let parsed: Sample = serde_json::from_value(checked).unwrap();
    assert_eq!(parsed.act, Act::Go { to: "inn".into() });
    assert_eq!(parsed.targets, ["ines"]);
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
