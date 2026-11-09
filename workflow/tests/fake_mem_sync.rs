//! A test's fake `mem sync --json` answers as real mem does. workflow
//! m1-project-run's fakes said "performed" with outbox 3 and left out
//! `detail`, which real mem never prints, so a path that cannot run was
//! tested green. Every fake now prints through one fixture, and this file
//! holds that fixture to mem's own schema.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mem-sync.sh");

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn fixture(args: &[&str]) -> Value {
    let out = Command::new("sh").arg(FIXTURE).args(args).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{args:?}: not JSON ({e}): {out:?}"))
}

/// What mem/schemas/sync.json asks of a document: its required keys, no
/// others, a state from its enum, and the types it names.
fn problems(doc: &Value) -> Vec<String> {
    let path = repo().join("mem/schemas/sync.json");
    let schema: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let mut found = Vec::new();
    let object = doc.as_object().expect("an object");
    for key in schema["required"].as_array().unwrap() {
        if !object.contains_key(key.as_str().unwrap()) {
            found.push(format!("missing {key}"));
        }
    }
    let properties = schema["properties"].as_object().unwrap();
    for (key, value) in object {
        let Some(property) = properties.get(key) else {
            found.push(format!("{key} is not in mem's sync.json"));
            continue;
        };
        if let Some(states) = property["enum"].as_array()
            && !states.contains(value)
        {
            found.push(format!("{key} {value} is not one of {states:?}"));
        }
        let types: Vec<&str> = match &property["type"] {
            Value::String(one) => vec![one.as_str()],
            Value::Array(many) => many.iter().filter_map(Value::as_str).collect(),
            _ => continue,
        };
        let kind = match value {
            Value::Null => "null",
            Value::String(_) => "string",
            Value::Number(n) if n.is_i64() => "integer",
            _ => "other",
        };
        if !types.contains(&kind) {
            found.push(format!("{key} {value} is not {types:?}"));
        }
    }
    // mem says performed only once nothing waits (mem/src/verbs.rs `sync`).
    if doc["sync"] == "performed" && doc["outbox"] != 0 {
        found.push(format!("performed with outbox {}", doc["outbox"]));
    }
    found
}

#[test]
fn the_fixture_answers_in_mem_s_own_shape() {
    for args in [&["0"][..], &["3"], &["0", "stuck"]] {
        let doc = fixture(args);
        let found = problems(&doc);
        assert!(found.is_empty(), "{args:?} -> {doc}: {found:?}");
    }
    assert_eq!(fixture(&["0"])["sync"], "performed");
    assert_eq!(fixture(&["3"])["sync"], "not performed");
    assert_eq!(fixture(&["0", "stuck"])["sync"], "not performed");
}

#[test]
fn the_check_refuses_what_the_old_fakes_printed() {
    let old = serde_json::json!({"sync": "performed", "outbox": 3});
    let found = problems(&old);
    assert!(found.iter().any(|p| p.contains("detail")), "{found:?}");
    assert!(found.iter().any(|p| p.contains("outbox 3")), "{found:?}");
}

/// A fake that prints its own sync document skips the fixture and the
/// check above, which is how the old mistake got in.
#[test]
fn no_test_fakes_a_sync_document_of_its_own() {
    let mut offenders = Vec::new();
    for dir in ["workflow/tests", "hub/tests", "tests"] {
        let Ok(entries) = std::fs::read_dir(repo().join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|n| n == "fake_mem_sync.rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (n, line) in text.lines().enumerate() {
                if line.contains(r#"{\"sync\""#) || line.contains(r#"{{\"sync\""#) {
                    offenders.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "print a fake sync through tests/fixtures/mem-sync.sh:\n{}",
        offenders.join("\n")
    );
}
