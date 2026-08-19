//! A JSON Schema validator small enough to be worth trusting.
//!
//! The same subset mem's `tests/json_contract.rs` reads — `type`, `enum`,
//! `required`, `properties`, `items`, `additionalProperties: false` and `$ref`
//! to a sibling file — because a contract that needs an unapproved crate to
//! check is not a contract anybody will check.
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

pub fn schema_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas")
}

pub fn load(name: &str) -> Value {
    let path = schema_dir().join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()))
}

/// Every way `value` fails `name`, rather than only the first: a failing
/// contract should say everything that drifted in one run.
pub fn problems(name: &str, value: &Value) -> Vec<String> {
    let mut found = Vec::new();
    check(&load(name), value, name, &mut found);
    found
}

pub fn assert_valid(name: &str, value: &Value) {
    let found = problems(name, value);
    assert!(found.is_empty(), "{name}: {found:#?}\nin {value:#}");
}

fn check(schema: &Value, value: &Value, at: &str, problems: &mut Vec<String>) {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        check(&load(reference), value, at, problems);
    }
    if let Some(types) = schema.get("type") {
        let wanted: Vec<&str> = match types {
            Value::String(one) => vec![one.as_str()],
            Value::Array(many) => many.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        let actual = match value {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        let ok = wanted.contains(&actual) || (actual == "integer" && wanted.contains(&"number"));
        if !ok {
            problems.push(format!("{at}: expected {wanted:?}, got {actual} ({value})"));
        }
    }
    if let Some(Value::Array(allowed)) = schema.get("enum")
        && !allowed.contains(value)
    {
        problems.push(format!("{at}: {value} is not one of {allowed:?}"));
    }
    if let Some(Value::Array(required)) = schema.get("required") {
        for key in required.iter().filter_map(Value::as_str) {
            if value.get(key).is_none() {
                problems.push(format!("{at}: missing required key '{key}'"));
            }
        }
    }
    if let Some(Value::Object(properties)) = schema.get("properties")
        && let Some(object) = value.as_object()
    {
        for (key, sub) in properties {
            if let Some(found) = object.get(key) {
                check(sub, found, &format!("{at}.{key}"), problems);
            }
        }
        if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
            let known: BTreeSet<&String> = properties.keys().collect();
            for key in object.keys() {
                if !known.contains(key) {
                    problems.push(format!("{at}: unexpected key '{key}'"));
                }
            }
        }
    }
    if let Some(sub) = schema.get("items")
        && let Some(array) = value.as_array()
    {
        for (n, item) in array.iter().enumerate() {
            check(sub, item, &format!("{at}[{n}]"), problems);
        }
    }
}
