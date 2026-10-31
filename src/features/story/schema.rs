//! The schemas of the agents' replies, and the validator that holds every
//! reply to its schema (docs/research/fabula-engine.md §8, *Structured
//! output*).
//!
//! - **The dialect by construction.** A [`Schema`] can express only what the
//!   dialect allows: every object closed with all its properties required, a
//!   value that may be absent as a union with null, a tagged union as closed
//!   objects told apart by a one-value `enum`. There is nothing to write a
//!   `$ref`, a bound or a pattern with. [`super::dialect::check`] holds the
//!   printed text to the dialect on its own, independent of how it was built.
//! - **Printed in declaration order.** `serde_json` is built without
//!   `preserve_order`, so a `Value` keeps its keys sorted, and a schema handed
//!   over as one would reach the model with `action` before `private_thought`,
//!   while the order of the properties is the order a model writes them in
//!   (spec §8, *Property order is generation order*). [`Schema::to_json`]
//!   writes the text itself, in the order declared.
//! - **The validator is the contract.** [`Schema::validate`] runs on every
//!   reply, whatever produced it, before serde sees it: serde reads a stray
//!   field on a variant without fields in silence, even under
//!   `deny_unknown_fields` (docs/lessons.md §8), and the validator refuses it.
//!   `enum` values match without regard to case and come back in the
//!   schema's spelling, since Anthropic does not guarantee the case.
//! - **Derived from the types.** [`StorySchema`] gives each reply type its
//!   schema, with the canon's keys as enums; the tests hold every value of a
//!   type to its schema and every instance of a schema to its type.

use std::fmt;

use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::{Map, Value};

use crate::entities::story::{Action, Canon, CharacterKey, LocationKey, Proposal, Visibility};

/// A reply's schema, in the dialect of spec §8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Schema {
    /// Free text.
    String,
    /// One of a fixed set of strings, matched without regard to case.
    Enum(Vec<String>),
    /// A whole number.
    Integer,
    /// True or false.
    Boolean,
    /// A list of values of one schema.
    Array(Box<Schema>),
    /// A closed object: every property required, in the order a model writes
    /// them.
    Object(Vec<Property>),
    /// A value of the inner schema, or null.
    Nullable(Box<Schema>),
    /// Closed objects told apart by the value of their `tag` property.
    Tagged {
        /// The property naming the variant, written first.
        tag: String,
        /// The variants, each with its own properties.
        variants: Vec<Variant>,
    },
}

/// A property of an object or of a variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    /// Its name.
    pub name: String,
    /// What the model is told it holds.
    pub description: Option<String>,
    /// Its schema.
    pub schema: Schema,
}

impl Property {
    /// A property with no description.
    pub fn new(name: &str, schema: Schema) -> Self {
        Self {
            name: name.to_owned(),
            description: None,
            schema,
        }
    }

    /// A property with a description for the model.
    pub fn described(name: &str, description: &str, schema: Schema) -> Self {
        Self {
            name: name.to_owned(),
            description: Some(description.to_owned()),
            schema,
        }
    }
}

/// A variant of a tagged union.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    /// The tag's value for it.
    pub name: String,
    /// What the model is told it means.
    pub description: Option<String>,
    /// Its properties after the tag.
    pub properties: Vec<Property>,
}

impl Variant {
    /// A variant with a description and its properties.
    pub fn new(name: &str, description: &str, properties: Vec<Property>) -> Self {
        Self {
            name: name.to_owned(),
            description: Some(description.to_owned()),
            properties,
        }
    }
}

/// A place where a reply breaks its schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Where, as `$.action.line`.
    pub path: String,
    /// What is wrong there.
    pub problem: String,
}

impl Violation {
    fn new(path: &str, problem: impl Into<String>) -> Self {
        Self {
            path: path.to_owned(),
            problem: problem.into(),
        }
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.problem)
    }
}

/// The violations as one line each, for a log or for the repair's message.
pub fn describe(violations: &[Violation]) -> String {
    violations
        .iter()
        .map(|v| format!("- {v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

impl Schema {
    /// The schema as the JSON text a model or a provider receives, its
    /// properties in the order declared.
    pub fn to_json(&self) -> String {
        serde_json::to_string(&Node::bare(self)).expect("a schema always serializes")
    }

    /// The same as a `Value`, whose keys are sorted: for checks that do not
    /// depend on the order.
    pub fn to_value(&self) -> Value {
        serde_json::to_value(Node::bare(self)).expect("a schema always serializes")
    }

    /// Checks `value` against the schema. On success, the value as the schema
    /// spells it: an `enum` value matched in another case comes back in the
    /// schema's. Every violation is reported, not only the first.
    pub fn validate(&self, value: &Value) -> Result<Value, Vec<Violation>> {
        let mut found = Vec::new();
        let checked = self.check(value, "$", &mut found);
        if found.is_empty() {
            Ok(checked)
        } else {
            Err(found)
        }
    }

    fn check(&self, value: &Value, path: &str, found: &mut Vec<Violation>) -> Value {
        match self {
            Schema::String => {
                expect(value.is_string(), "text", value, path, found);
                value.clone()
            }
            Schema::Enum(values) => check_enum(values, value, path, found),
            Schema::Integer => {
                expect(
                    value.is_i64() || value.is_u64(),
                    "a whole number",
                    value,
                    path,
                    found,
                );
                value.clone()
            }
            Schema::Boolean => {
                expect(value.is_boolean(), "true or false", value, path, found);
                value.clone()
            }
            Schema::Array(items) => match value.as_array() {
                Some(list) => Value::Array(
                    list.iter()
                        .enumerate()
                        .map(|(i, item)| items.check(item, &format!("{path}[{i}]"), found))
                        .collect(),
                ),
                None => {
                    expect(false, "a list", value, path, found);
                    value.clone()
                }
            },
            Schema::Object(properties) => check_object(properties, None, value, path, found),
            Schema::Nullable(inner) => {
                if value.is_null() {
                    Value::Null
                } else {
                    inner.check(value, path, found)
                }
            }
            Schema::Tagged { tag, variants } => check_tagged(tag, variants, value, path, found),
        }
    }
}

fn expect(ok: bool, wanted: &str, value: &Value, path: &str, found: &mut Vec<Violation>) {
    if !ok {
        found.push(Violation::new(
            path,
            format!("expected {wanted}, found {}", kind(value)),
        ));
    }
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "true or false",
        Value::Number(_) => "a number",
        Value::String(_) => "text",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

/// Two strings equal without regard to case.
pub(super) fn same_word(a: &str, b: &str) -> bool {
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}

fn one_of(values: &[String]) -> String {
    values
        .iter()
        .map(|v| format!("\"{v}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

fn check_enum(values: &[String], value: &Value, path: &str, found: &mut Vec<Violation>) -> Value {
    let Some(text) = value.as_str() else {
        expect(false, "text", value, path, found);
        return value.clone();
    };
    match values.iter().find(|v| same_word(v, text)) {
        Some(spelled) => Value::String(spelled.clone()),
        None => {
            found.push(Violation::new(
                path,
                format!("\"{text}\" is not one of {}", one_of(values)),
            ));
            value.clone()
        }
    }
}

fn check_object(
    properties: &[Property],
    tag: Option<(&str, &str)>,
    value: &Value,
    path: &str,
    found: &mut Vec<Violation>,
) -> Value {
    let Some(object) = value.as_object() else {
        expect(false, "an object", value, path, found);
        return value.clone();
    };
    let mut checked = Map::new();
    if let Some((tag, name)) = tag {
        checked.insert(tag.to_owned(), Value::String(name.to_owned()));
    }
    for property in properties {
        let at = format!("{path}.{}", property.name);
        match object.get(&property.name) {
            Some(v) => {
                let v = property.schema.check(v, &at, found);
                checked.insert(property.name.clone(), v);
            }
            None => found.push(Violation::new(&at, "missing")),
        }
    }
    for key in object.keys() {
        let known =
            properties.iter().any(|p| &p.name == key) || tag.is_some_and(|(tag, _)| tag == key);
        if !known {
            found.push(Violation::new(
                &format!("{path}.{key}"),
                "is not in the schema",
            ));
        }
    }
    Value::Object(checked)
}

fn check_tagged(
    tag: &str,
    variants: &[Variant],
    value: &Value,
    path: &str,
    found: &mut Vec<Violation>,
) -> Value {
    if !value.is_object() {
        expect(false, "an object", value, path, found);
        return value.clone();
    }
    let names: Vec<String> = variants.iter().map(|v| v.name.clone()).collect();
    let at = format!("{path}.{tag}");
    let Some(name) = value.get(tag).and_then(Value::as_str) else {
        found.push(Violation::new(
            &at,
            format!("missing; one of {}", one_of(&names)),
        ));
        return value.clone();
    };
    match variants.iter().find(|v| same_word(&v.name, name)) {
        Some(variant) => check_object(
            &variant.properties,
            Some((tag, &variant.name)),
            value,
            path,
            found,
        ),
        None => {
            found.push(Violation::new(
                &at,
                format!("\"{name}\" is not one of {}", one_of(&names)),
            ));
            value.clone()
        }
    }
}

/// The first JSON object in a reply, read leniently (spec §8, *The
/// fallback*): the whole reply when it is one, else the first object that
/// parses from an opening brace, which steps over code fences and prose
/// before or after it. `None` when the reply holds no object.
pub fn extract_object(reply: &str) -> Option<Value> {
    /// A reply with more opening braces than this before its object is not
    /// worth reading further.
    const MAX_STARTS: usize = 64;
    if let Ok(value @ Value::Object(_)) = serde_json::from_str::<Value>(reply.trim()) {
        return Some(value);
    }
    reply
        .match_indices('{')
        .take(MAX_STARTS)
        .find_map(|(at, _)| {
            match serde_json::Deserializer::from_str(&reply[at..])
                .into_iter::<Value>()
                .next()
            {
                Some(Ok(value @ Value::Object(_))) => Some(value),
                _ => None,
            }
        })
}

// ---------------------------------------------------------------------------
// The printed form.

/// A schema node with the description its property gives it.
struct Node<'a> {
    schema: &'a Schema,
    description: Option<&'a str>,
}

impl<'a> Node<'a> {
    fn bare(schema: &'a Schema) -> Self {
        Self {
            schema,
            description: None,
        }
    }

    fn of(property: &'a Property) -> Self {
        Self {
            schema: &property.schema,
            description: property.description.as_deref(),
        }
    }
}

impl Serialize for Node<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(None)?;
        let simple = |map: &mut S::SerializeMap, ty: &str| -> Result<(), S::Error> {
            map.serialize_entry("type", ty)?;
            if let Some(d) = self.description {
                map.serialize_entry("description", d)?;
            }
            Ok(())
        };
        match self.schema {
            Schema::String => simple(&mut map, "string")?,
            Schema::Enum(values) => {
                simple(&mut map, "string")?;
                map.serialize_entry("enum", values)?;
            }
            Schema::Integer => simple(&mut map, "integer")?,
            Schema::Boolean => simple(&mut map, "boolean")?,
            Schema::Array(items) => {
                simple(&mut map, "array")?;
                map.serialize_entry("items", &Node::bare(items))?;
            }
            Schema::Object(properties) => {
                simple(&mut map, "object")?;
                write_closed(&mut map, None, properties)?;
            }
            Schema::Nullable(_) | Schema::Tagged { .. } => {
                if let Some(d) = self.description {
                    map.serialize_entry("description", d)?;
                }
                map.serialize_entry("anyOf", &Branches(self.schema))?;
            }
        }
        map.end()
    }
}

/// An object's `properties`, `required` and `additionalProperties`, with the
/// tag of a variant first.
fn write_closed<M: SerializeMap>(
    map: &mut M,
    tag: Option<(&str, &str)>,
    properties: &[Property],
) -> Result<(), M::Error> {
    map.serialize_entry("properties", &Properties { tag, properties })?;
    let names: Vec<&str> = tag
        .map(|(tag, _)| tag)
        .into_iter()
        .chain(properties.iter().map(|p| p.name.as_str()))
        .collect();
    map.serialize_entry("required", &names)?;
    map.serialize_entry("additionalProperties", &false)
}

struct Properties<'a> {
    tag: Option<(&'a str, &'a str)>,
    properties: &'a [Property],
}

impl Serialize for Properties<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.properties.len() + 1))?;
        if let Some((tag, name)) = self.tag {
            map.serialize_entry(tag, &TagValue(name))?;
        }
        for property in self.properties {
            map.serialize_entry(&property.name, &Node::of(property))?;
        }
        map.end()
    }
}

/// A variant's tag: a string with one allowed value.
struct TagValue<'a>(&'a str);

impl Serialize for TagValue<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(2))?;
        map.serialize_entry("type", "string")?;
        map.serialize_entry("enum", &[self.0])?;
        map.end()
    }
}

/// The branches of a union. A nullable tagged union is flattened into one
/// list, its variants and then null, so no union sits directly inside another.
struct Branches<'a>(&'a Schema);

/// One branch of a printed union.
enum Branch<'a> {
    Plain(&'a Schema),
    Variant(&'a str, &'a Variant),
    Null,
}

fn branches(schema: &Schema) -> Vec<Branch<'_>> {
    match schema {
        Schema::Tagged { tag, variants } => variants
            .iter()
            .map(|v| Branch::Variant(tag.as_str(), v))
            .collect(),
        Schema::Nullable(inner) => {
            let mut list = match inner.as_ref() {
                Schema::Tagged { .. } | Schema::Nullable(_) => branches(inner),
                other => vec![Branch::Plain(other)],
            };
            list.retain(|b| !matches!(b, Branch::Null));
            list.push(Branch::Null);
            list
        }
        other => vec![Branch::Plain(other)],
    }
}

impl Serialize for Branches<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let list = branches(self.0);
        let mut seq = s.serialize_seq(Some(list.len()))?;
        for branch in &list {
            seq.serialize_element(branch)?;
        }
        seq.end()
    }
}

impl Serialize for Branch<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Branch::Plain(schema) => Node::bare(schema).serialize(s),
            Branch::Null => {
                let mut map = s.serialize_map(Some(1))?;
                map.serialize_entry("type", "null")?;
                map.end()
            }
            Branch::Variant(tag, variant) => {
                let mut map = s.serialize_map(None)?;
                map.serialize_entry("type", "object")?;
                if let Some(d) = &variant.description {
                    map.serialize_entry("description", d)?;
                }
                write_closed(&mut map, Some((tag, &variant.name)), &variant.properties)?;
                map.end()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The schemas of the reply types.

/// The keys a canon fixes, which a reply's schema lists as enums (spec §8,
/// *One schema dialect*). The validators still check who is present where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonKeys {
    /// The cast, in the canon's order.
    pub characters: Vec<String>,
    /// The map, in the canon's order.
    pub locations: Vec<String>,
}

impl CanonKeys {
    /// The keys of `canon`.
    pub fn of(canon: &Canon) -> Self {
        Self {
            characters: canon
                .characters
                .iter()
                .map(|c| c.key.as_str().to_owned())
                .collect(),
            locations: canon
                .locations
                .iter()
                .map(|l| l.key.as_str().to_owned())
                .collect(),
        }
    }
}

/// A reply type's schema, derived from the type and the canon's keys. A
/// role's schema changes only with the canon (spec §8, *Schemas are stable
/// per story*).
pub trait StorySchema {
    /// The schema of this type's values.
    fn schema(keys: &CanonKeys) -> Schema;
}

impl StorySchema for String {
    fn schema(_: &CanonKeys) -> Schema {
        Schema::String
    }
}

impl<T: StorySchema> StorySchema for Option<T> {
    fn schema(keys: &CanonKeys) -> Schema {
        Schema::Nullable(Box::new(T::schema(keys)))
    }
}

impl<T: StorySchema> StorySchema for Vec<T> {
    fn schema(keys: &CanonKeys) -> Schema {
        Schema::Array(Box::new(T::schema(keys)))
    }
}

impl StorySchema for CharacterKey {
    fn schema(keys: &CanonKeys) -> Schema {
        Schema::Enum(keys.characters.clone())
    }
}

impl StorySchema for LocationKey {
    fn schema(keys: &CanonKeys) -> Schema {
        Schema::Enum(keys.locations.clone())
    }
}

/// The free-text fields are story content, and their descriptions say so: a
/// field that reads as the model's own reasoning is refused by Claude 5.x as
/// `reasoning_extraction` (spec §8).
impl StorySchema for Proposal {
    fn schema(keys: &CanonKeys) -> Schema {
        Schema::Object(vec![
            Property::described(
                "private_thought",
                "What the character thinks and feels at this moment, in their own inner \
                 voice. Story content that no other character perceives.",
                String::schema(keys),
            ),
            Property::described(
                "action",
                "What the character does now.",
                Action::schema(keys),
            ),
            Property::described(
                "targets",
                "The characters the action is aimed at; empty when it is aimed at no one.",
                Vec::<CharacterKey>::schema(keys),
            ),
            Property::described(
                "visibility",
                "Who can perceive the action.",
                Visibility::schema(keys),
            ),
        ])
    }
}

impl StorySchema for Action {
    fn schema(keys: &CanonKeys) -> Schema {
        Schema::Tagged {
            tag: "type".to_owned(),
            variants: vec![
                Variant::new(
                    "speak",
                    "Says something aloud.",
                    vec![Property::described(
                        "line",
                        "The words spoken, in the character's voice.",
                        String::schema(keys),
                    )],
                ),
                Variant::new(
                    "move",
                    "Leaves for another place.",
                    vec![Property::described(
                        "to",
                        "Where the character goes.",
                        LocationKey::schema(keys),
                    )],
                ),
                Variant::new(
                    "act",
                    "Does something, perhaps saying something at the same time.",
                    vec![
                        Property::described(
                            "verb",
                            "The deed, such as \"slams\" or \"pockets\".",
                            String::schema(keys),
                        ),
                        Property::described(
                            "object",
                            "What it is done to, or null.",
                            Option::<String>::schema(keys),
                        ),
                        Property::described(
                            "line",
                            "What is said meanwhile, or null.",
                            Option::<String>::schema(keys),
                        ),
                    ],
                ),
                Variant::new("wait", "Does nothing for now.", Vec::new()),
            ],
        }
    }
}

impl StorySchema for Visibility {
    fn schema(keys: &CanonKeys) -> Schema {
        Schema::Tagged {
            tag: "type".to_owned(),
            variants: vec![
                Variant::new("public", "Everyone present can perceive it.", Vec::new()),
                Variant::new(
                    "whisper",
                    "Only the named characters perceive it; they must be present.",
                    vec![Property::described(
                        "to",
                        "Who hears it.",
                        Vec::<CharacterKey>::schema(keys),
                    )],
                ),
                Variant::new("hidden", "No one else perceives it.", Vec::new()),
            ],
        }
    }
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
