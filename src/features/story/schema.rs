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
//!   over as one would reach the model with `decision` before `answers`,
//!   while the order of the properties is the order a model writes them in
//!   (spec §8, *Property order is generation order*). [`Schema::to_json`]
//!   writes the text itself, in the order declared.
//! - **The validator is the contract.** [`Schema::validate`] runs on every
//!   reply, whatever produced it, before serde sees it: serde reads a stray
//!   field on a variant without fields in silence, even under
//!   `deny_unknown_fields` (docs/lessons.md §8), and the validator refuses it.
//!   `enum` values match without regard to case and come back in the
//!   schema's spelling, since Anthropic does not guarantee the case.

use std::fmt;

use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::{Map, Value};

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

#[cfg(test)]
#[path = "schema_tests.rs"]
pub(crate) mod tests;
