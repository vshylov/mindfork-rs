//! The schema dialect's check (docs/research/fabula-engine.md §8, *One schema
//! dialect*): the intersection of what the enforcing providers compile. It
//! reads a schema as the JSON a provider would receive and names every rule
//! the schema breaks, whatever built it. The tests hold every schema the
//! engine derives to it; `story_create` holds the canon it receives to the
//! same rules (docs/fabula.md §4, PR 9).
//!
//! The rules: the root is a closed object; every object is closed
//! (`additionalProperties: false`) and lists every property as required; a
//! value that may be absent is a union with null; a tagged union is an `anyOf`
//! inside a property, each branch a closed object whose tag is a one-value
//! `enum`; nothing else — no `$ref`, `oneOf`, `allOf`, `not`, conditionals,
//! `const`, `pattern`, `format`, bounds or type lists.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::{Map, Value};

use super::schema::same_word;

/// The keywords a node may carry.
const KEYWORDS: [&str; 8] = [
    "type",
    "description",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "enum",
    "anyOf",
];

/// The types a node may name.
const TYPES: [&str; 7] = [
    "string", "integer", "number", "boolean", "array", "object", "null",
];

/// Anthropic compiles at most this many union-typed parameters in one request
/// (spec §8).
pub const MAX_UNIONS: usize = 16;

/// A rule a schema breaks, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialectProblem {
    /// Where, as `$.action|0.line`: a property by its name, a list's items as
    /// `[]`, a union's branch by its index after `|`.
    pub path: String,
    /// The rule broken.
    pub rule: String,
}

impl fmt::Display for DialectProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.rule)
    }
}

/// Every rule of the dialect `schema` breaks; empty when it keeps them all.
pub fn check(schema: &Value) -> Vec<DialectProblem> {
    let mut walk = Walk::default();
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        walk.problem("$", "the root must be a closed object");
    }
    walk.node(schema, "$");
    if walk.unions > MAX_UNIONS {
        let unions = walk.unions;
        walk.problem("$", format!("{unions} unions; at most {MAX_UNIONS}"));
    }
    walk.found
}

#[derive(Default)]
struct Walk {
    found: Vec<DialectProblem>,
    unions: usize,
}

impl Walk {
    fn problem(&mut self, path: &str, rule: impl Into<String>) {
        self.found.push(DialectProblem {
            path: path.to_owned(),
            rule: rule.into(),
        });
    }

    fn node(&mut self, schema: &Value, path: &str) {
        let Some(node) = schema.as_object() else {
            self.problem(path, "a schema must be an object");
            return;
        };
        for key in node.keys() {
            if !KEYWORDS.contains(&key.as_str()) {
                self.problem(path, format!("`{key}` is outside the dialect"));
            }
        }
        if node.get("description").is_some_and(|d| !d.is_string()) {
            self.problem(path, "a description must be text");
        }
        match (node.get("type"), node.get("anyOf")) {
            (Some(_), Some(_)) => self.problem(path, "a node has a type or a union, not both"),
            (None, None) => self.problem(path, "a node needs a type or a union"),
            (None, Some(branches)) => {
                self.misplaced(
                    node,
                    path,
                    &[
                        "properties",
                        "required",
                        "additionalProperties",
                        "items",
                        "enum",
                    ],
                );
                self.union(branches, path);
            }
            (Some(ty), None) => self.typed(node, ty, path),
        }
    }

    fn typed(&mut self, node: &Map<String, Value>, ty: &Value, path: &str) {
        let Some(ty) = ty.as_str() else {
            self.problem(
                path,
                "`type` names one type; a value that may be absent is a union with null",
            );
            return;
        };
        match ty {
            "object" => self.object(node, path),
            "array" => {
                self.misplaced(
                    node,
                    path,
                    &["properties", "required", "additionalProperties", "enum"],
                );
                match node.get("items") {
                    Some(items) => self.node(items, &format!("{path}[]")),
                    None => self.problem(path, "a list needs `items`"),
                }
            }
            "string" => {
                self.misplaced(
                    node,
                    path,
                    &["properties", "required", "additionalProperties", "items"],
                );
                if let Some(values) = node.get("enum") {
                    self.enumeration(values, path);
                }
            }
            _ if TYPES.contains(&ty) => self.misplaced(
                node,
                path,
                &[
                    "properties",
                    "required",
                    "additionalProperties",
                    "items",
                    "enum",
                ],
            ),
            other => self.problem(path, format!("`{other}` is not a type")),
        }
    }

    fn misplaced(&mut self, node: &Map<String, Value>, path: &str, keys: &[&str]) {
        for key in keys {
            if node.contains_key(*key) {
                self.problem(path, format!("`{key}` does not belong on this node"));
            }
        }
    }

    fn object(&mut self, node: &Map<String, Value>, path: &str) {
        self.misplaced(node, path, &["items", "enum"]);
        if node.get("additionalProperties") != Some(&Value::Bool(false)) {
            self.problem(path, "an object is closed: `additionalProperties: false`");
        }
        let Some(properties) = node.get("properties").and_then(Value::as_object) else {
            self.problem(path, "an object needs `properties`");
            return;
        };
        let required: Vec<&str> = match node.get("required").and_then(Value::as_array) {
            Some(list) => list.iter().filter_map(Value::as_str).collect(),
            None => {
                self.problem(path, "an object lists its properties in `required`");
                Vec::new()
            }
        };
        for name in properties.keys() {
            match required.iter().filter(|r| **r == name).count() {
                0 => self.problem(
                    &format!("{path}.{name}"),
                    "every property is required; a value that may be absent is a union with null",
                ),
                1 => {}
                _ => self.problem(&format!("{path}.{name}"), "required twice"),
            }
        }
        for name in &required {
            if !properties.contains_key(*name) {
                self.problem(
                    path,
                    format!("`required` names `{name}`, which is not a property"),
                );
            }
        }
        for (name, property) in properties {
            self.node(property, &format!("{path}.{name}"));
        }
    }

    fn enumeration(&mut self, values: &Value, path: &str) {
        let Some(values) = values.as_array() else {
            self.problem(path, "`enum` must be a list");
            return;
        };
        if values.is_empty() {
            self.problem(path, "an `enum` needs a value");
        }
        let mut seen: Vec<&str> = Vec::new();
        for value in values {
            match value.as_str() {
                None => self.problem(path, "an `enum` lists text only"),
                Some(text) if seen.iter().any(|s| same_word(s, text)) => self.problem(
                    path,
                    format!("\"{text}\" is listed twice; values match without regard to case"),
                ),
                Some(text) => seen.push(text),
            }
        }
    }

    fn union(&mut self, branches: &Value, path: &str) {
        self.unions += 1;
        if path == "$" {
            self.problem(path, "a union sits inside a property, never at the root");
        }
        let Some(branches) = branches.as_array() else {
            self.problem(path, "`anyOf` must be a list");
            return;
        };
        let null = |b: &&Value| b.get("type").and_then(Value::as_str) == Some("null");
        let nulls = branches.iter().filter(null).count();
        let rest: Vec<&Value> = branches.iter().filter(|b| !null(b)).collect();
        if branches.len() < 2 {
            self.problem(path, "a union needs two branches");
        }
        if nulls > 1 {
            self.problem(path, "a union has one null branch at most");
        }
        match rest.as_slice() {
            [] => {}
            [only] if nulls == 1 => {
                if only.get("anyOf").is_some() {
                    self.problem(path, "a union inside a union; list its branches in one");
                }
            }
            _ => self.tagged(&rest, path),
        }
        for (i, branch) in branches.iter().enumerate() {
            self.node(branch, &format!("{path}|{i}"));
        }
    }

    /// The branches other than null are closed objects, told apart by one
    /// property that holds a different one-value `enum` in each.
    fn tagged(&mut self, branches: &[&Value], path: &str) {
        let tags: Vec<BTreeMap<&str, &str>> = branches.iter().map(|b| one_value_enums(b)).collect();
        let shared = tags
            .first()
            .and_then(|first| {
                first
                    .keys()
                    .find(|key| tags.iter().all(|t| t.contains_key(*key)))
            })
            .copied();
        let Some(tag) = shared else {
            self.problem(
                path,
                "the branches of a union are closed objects that share a tag: \
                 a property holding a one-value `enum` in each",
            );
            return;
        };
        let mut seen: Vec<&str> = Vec::new();
        for t in &tags {
            let value = t[tag];
            if seen.iter().any(|s| same_word(s, value)) {
                self.problem(path, format!("two branches share the tag \"{value}\""));
            }
            seen.push(value);
        }
    }
}

/// The properties of an object branch whose schema is a string with one
/// allowed value, and that value.
fn one_value_enums(branch: &Value) -> BTreeMap<&str, &str> {
    let mut found = BTreeMap::new();
    if branch.get("type").and_then(Value::as_str) != Some("object") {
        return found;
    }
    let Some(properties) = branch.get("properties").and_then(Value::as_object) else {
        return found;
    };
    for (name, property) in properties {
        let one = property
            .get("enum")
            .and_then(Value::as_array)
            .filter(|values| values.len() == 1)
            .and_then(|values| values[0].as_str());
        if property.get("type").and_then(Value::as_str) == Some("string")
            && let Some(value) = one
        {
            found.insert(name.as_str(), value);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{MAX_UNIONS, check};

    fn rules(schema: serde_json::Value) -> Vec<String> {
        check(&schema).into_iter().map(|p| p.to_string()).collect()
    }

    fn closed(properties: serde_json::Value) -> serde_json::Value {
        let names: Vec<String> = properties.as_object().unwrap().keys().cloned().collect();
        json!({"type": "object", "properties": properties, "required": names,
               "additionalProperties": false})
    }

    #[test]
    fn a_closed_object_with_a_tagged_union_and_a_nullable_keeps_the_dialect() {
        let schema = closed(json!({
            "name": {"type": "string", "description": "Who."},
            "count": {"type": "integer"},
            "maybe": {"anyOf": [{"type": "string"}, {"type": "null"}]},
            "act": {"anyOf": [
                {"type": "object", "properties": {"type": {"type": "string", "enum": ["a"]}},
                 "required": ["type"], "additionalProperties": false},
                {"type": "object", "properties": {"type": {"type": "string", "enum": ["b"]},
                    "x": {"type": "array", "items": {"type": "string", "enum": ["p", "q"]}}},
                 "required": ["type", "x"], "additionalProperties": false},
                {"type": "null"}
            ]}
        }));
        assert_eq!(rules(schema), Vec::<String>::new());
    }

    #[test]
    fn what_the_dialect_leaves_out_is_named() {
        let cases = [
            (json!({"$ref": "#/defs/x"}), "`$ref` is outside the dialect"),
            (json!({"oneOf": []}), "`oneOf` is outside the dialect"),
            (
                json!({"type": "string", "const": "x"}),
                "`const` is outside",
            ),
            (
                json!({"type": "string", "pattern": "^a"}),
                "`pattern` is outside",
            ),
            (
                json!({"type": "string", "format": "date"}),
                "`format` is outside",
            ),
            (
                json!({"type": "string", "maxLength": 3}),
                "`maxLength` is outside",
            ),
            (
                json!({"type": "integer", "minimum": 0}),
                "`minimum` is outside",
            ),
            (
                json!({"type": "array", "items": {"type": "string"}, "minItems": 1}),
                "`minItems`",
            ),
            (json!({"type": ["string", "null"]}), "names one type"),
            (json!({"type": "date"}), "`date` is not a type"),
            (json!({"type": "array"}), "a list needs `items`"),
            (json!({"type": "string", "enum": []}), "needs a value"),
            (
                json!({"type": "string", "enum": ["Mara", "mara"]}),
                "listed twice",
            ),
            (json!({"type": "string", "enum": [1]}), "lists text only"),
            (
                json!({"type": "string", "items": {"type": "string"}}),
                "`items` does not belong",
            ),
            (json!({}), "needs a type or a union"),
            (json!({"type": "string", "anyOf": []}), "not both"),
            (
                json!({"type": "string", "description": 3}),
                "a description must be text",
            ),
            (json!(7), "a schema must be an object"),
        ];
        for (property, rule) in cases {
            let found = rules(closed(json!({ "p": property })));
            assert!(
                found.iter().any(|f| f.contains(rule)),
                "{property} should break \"{rule}\": {found:?}"
            );
        }
    }

    #[test]
    fn an_object_is_closed_and_requires_every_property() {
        let open = json!({"type": "object", "properties": {"a": {"type": "string"}},
                          "required": ["a"]});
        assert!(rules(open)[0].contains("additionalProperties: false"));
        let optional = json!({"type": "object", "properties": {"a": {"type": "string"},
                              "b": {"type": "string"}}, "required": ["a"],
                              "additionalProperties": false});
        assert_eq!(
            rules(optional),
            ["$.b: every property is required; a value that may be absent is a union with null"]
        );
        let stray = json!({"type": "object", "properties": {}, "required": ["ghost"],
                           "additionalProperties": false});
        assert!(rules(stray)[0].contains("`ghost`, which is not a property"));
        let twice = json!({"type": "object", "properties": {"a": {"type": "string"}},
                           "required": ["a", "a"], "additionalProperties": false});
        assert_eq!(rules(twice), ["$.a: required twice"]);
        let bare = json!({"type": "object", "additionalProperties": false});
        assert!(rules(bare).iter().any(|r| r.contains("needs `properties`")));
        let unlisted = json!({"type": "object", "properties": {}, "additionalProperties": false});
        assert!(rules(unlisted).iter().any(|r| r.contains("in `required`")));
    }

    #[test]
    fn the_root_is_a_closed_object_and_never_a_union() {
        assert!(rules(json!({"type": "string"}))[0].contains("the root must be"));
        let union = json!({"anyOf": [{"type": "string"}, {"type": "null"}]});
        let found = rules(union);
        assert!(
            found.iter().any(|r| r.contains("never at the root")),
            "{found:?}"
        );
    }

    #[test]
    fn a_union_is_a_nullable_or_tagged_closed_objects() {
        let object = |tag: &str| {
            json!({"type": "object", "properties": {"type": {"type": "string", "enum": [tag]}},
                   "required": ["type"], "additionalProperties": false})
        };
        let cases = [
            (json!({"anyOf": [{"type": "string"}]}), "needs two branches"),
            (
                json!({"anyOf": [{"type": "null"}, {"type": "null"}]}),
                "one null branch",
            ),
            (
                json!({"anyOf": [{"type": "string"}, {"type": "integer"}]}),
                "share a tag",
            ),
            (
                json!({"anyOf": [object("a"), object("A")]}),
                "share the tag \"A\"",
            ),
            (
                json!({"anyOf": [{"anyOf": [object("a"), object("b")]}, {"type": "null"}]}),
                "a union inside a union",
            ),
            (
                json!({"anyOf": {"type": "string"}}),
                "`anyOf` must be a list",
            ),
            (
                json!({"anyOf": [object("a"), object("b")], "enum": ["a"]}),
                "`enum` does not",
            ),
        ];
        for (property, rule) in cases {
            let found = rules(closed(json!({ "p": property })));
            assert!(
                found.iter().any(|f| f.contains(rule)),
                "{property} should break \"{rule}\": {found:?}"
            );
        }
        let tagged = closed(json!({"p": {"anyOf": [object("a"), object("b")]}}));
        assert_eq!(rules(tagged), Vec::<String>::new());
    }

    #[test]
    fn more_unions_than_anthropic_compiles_are_refused() {
        let nullable = json!({"anyOf": [{"type": "string"}, {"type": "null"}]});
        let at_most: serde_json::Map<String, serde_json::Value> = (0..MAX_UNIONS)
            .map(|i| (format!("p{i}"), nullable.clone()))
            .collect();
        assert_eq!(
            rules(closed(serde_json::Value::Object(at_most.clone()))),
            Vec::<String>::new()
        );
        let mut over = at_most;
        over.insert("extra".to_owned(), nullable);
        assert_eq!(
            rules(closed(serde_json::Value::Object(over))),
            [format!(
                "$: {} unions; at most {MAX_UNIONS}",
                MAX_UNIONS + 1
            )]
        );
    }
}
