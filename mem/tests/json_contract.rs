//! AC15, first half: every verb's `--json` validates against the committed
//! schema next to it in `mem/schemas/`.
//!
//! The validator below is deliberately small — the schemas are written to the
//! subset it understands (see `mem/schemas/README.md`), because a JSON Schema
//! crate is not in spec §12 and a contract nobody can check is not a contract.

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use common::{World, code, item, mem, mem_env, put, stderr};
use mem::item::Kind;
use serde_json::Value;

const P: &str = "01K2AAAAAAAAAAAAAAAAAAAAAA";

fn schema_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas")
}

fn load(name: &str) -> Value {
    let path = schema_dir().join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()))
}

/// Validates `value` against `schema`, collecting every complaint rather than
/// stopping at the first: a failing contract test should say everything that
/// drifted in one run.
fn check(schema: &Value, value: &Value, at: &str, problems: &mut Vec<String>) {
    if let Some(reference) = schema.get("$ref").and_then(|r| r.as_str()) {
        check(&load(reference), value, at, problems);
    }
    if let Some(types) = schema.get("type") {
        let wanted: Vec<&str> = match types {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().filter_map(|t| t.as_str()).collect(),
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
        // An integer is a number; the reverse is not true.
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
        for key in required.iter().filter_map(|k| k.as_str()) {
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
    if let Some(items) = schema.get("items")
        && let Some(array) = value.as_array()
    {
        for (n, element) in array.iter().enumerate() {
            check(items, element, &format!("{at}[{n}]"), problems);
        }
    }
}

/// Asserts one command's output against one schema, and returns the value so a
/// test can look closer.
#[track_caller]
fn validate(schema: &str, out: &std::process::Output) -> Value {
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let value: Value = serde_json::from_str(text.trim())
        .unwrap_or_else(|e| panic!("{schema}: output is not JSON ({e}): {text}"));
    let mut problems = Vec::new();
    check(
        &load(schema),
        &value,
        schema.trim_end_matches(".json"),
        &mut problems,
    );
    assert!(
        problems.is_empty(),
        "{schema}:\n  {}",
        problems.join("\n  ")
    );
    value
}

/// A store with one of everything, so the row shapes are exercised with real
/// values rather than empty arrays.
fn populated(tag: &str) -> (World, PathBuf) {
    let w = World::new(tag);
    let repo = w.repo("thing", Some("git@github.com:me/thing.git"));
    assert_eq!(
        code(&mem(
            &w,
            &repo,
            &[
                "save",
                "sessions use redis",
                "--type",
                "decision",
                "--tags",
                "redis,sessions"
            ]
        )),
        0
    );
    assert_eq!(code(&mem(&w, &repo, &["log", "ran the migration"])), 0);
    assert_eq!(
        code(&mem_env(
            &w,
            &repo,
            &["handoff", "--set", "stopped mid-migration; next: run it"],
            &[("MEM_SYNC_CMD", "true")]
        )),
        0
    );
    assert_eq!(code(&mem(&w, &repo, &["status", "--set", "on review"])), 0);
    let plan = w.dir.join("plan.md");
    std::fs::write(&plan, "# plan: migrate\n- [ ] t1 run it\n").unwrap();
    assert_eq!(
        code(&mem(
            &w,
            &repo,
            &["plan", "--set-file", plan.to_str().unwrap()]
        )),
        0
    );
    (w, repo)
}

#[test]
fn every_verb_matches_its_committed_schema() {
    let (w, repo) = populated("json-contract");
    let quiet = [("MEM_SYNC_CMD", "true"), ("MEM_NOTIFY_CMD", "true")];

    validate("context.json", &mem(&w, &repo, &["context", "--json"]));
    validate(
        "context-brief.json",
        &mem(&w, &repo, &["context", "--brief", "--json"]),
    );
    validate("projects.json", &mem(&w, &repo, &["projects", "--json"]));
    validate(
        "project-current.json",
        &mem(&w, &repo, &["project", "current", "--json"]),
    );
    // And again once a verifier is declared: the optional field is part of the
    // same contract, and a schema nobody validates with it present is a guess.
    assert_eq!(
        code(&mem(&w, &repo, &["project", "set", "verify", "just test"])),
        0
    );
    let current = validate(
        "project-current.json",
        &mem(&w, &repo, &["project", "current", "--json"]),
    );
    assert_eq!(current["verify"], serde_json::json!("just test"));
    validate("status.json", &mem(&w, &repo, &["status", "--json"]));
    validate("plan.json", &mem(&w, &repo, &["plan", "--json"]));
    validate(
        "plan-tick.json",
        &mem(&w, &repo, &["plan", "--tick", "t1", "--json"]),
    );
    validate("log.json", &mem(&w, &repo, &["log", "--json"]));
    validate("handoff.json", &mem(&w, &repo, &["handoff", "--json"]));
    validate(
        "search.json",
        &mem(&w, &repo, &["search", "redis", "--json"]),
    );
    validate(
        "session-check.json",
        &mem(&w, &repo, &["session-check", "--session-id", "s", "--json"]),
    );
    validate(
        "precompact.json",
        &mem(&w, &repo, &["precompact", "--json"]),
    );
    validate("reindex.json", &mem(&w, &repo, &["reindex", "--json"]));
    validate("snapshot.json", &mem(&w, &repo, &["snapshot", "--json"]));
    validate("prune.json", &mem(&w, &repo, &["prune", "--json"]));
    validate("doctor.json", &mem(&w, &repo, &["doctor", "--json"]));

    let written = validate(
        "written.json",
        &mem(&w, &repo, &["save", "another fact", "--json"]),
    );
    validate(
        "show.json",
        &mem(
            &w,
            &repo,
            &["show", written["short_id"].as_str().unwrap(), "--json"],
        ),
    );

    let asked = validate(
        "ask.json",
        &mem_env(
            &w,
            &repo,
            &["ask", "deploy on friday?", "--options", "yes,no", "--json"],
            &quiet,
        ),
    );
    let question = asked["short_id"].as_str().unwrap().to_string();
    validate(
        "questions.json",
        &mem(&w, &repo, &["questions", "--pending", "--json"]),
    );
    validate(
        "written.json",
        &mem_env(&w, &repo, &["answer", &question, "no", "--json"], &quiet),
    );
    validate(
        "questions-wait.json",
        &mem_env(
            &w,
            &repo,
            &[
                "questions",
                "--wait",
                &question,
                "--timeout",
                "5s",
                "--json",
            ],
            &quiet,
        ),
    );
}

#[test]
fn the_maintenance_verbs_match_theirs_too() {
    // Pruning and syncing both need a world arranged around them, so they get
    // their own test rather than a shared one that is half setup.
    let w = World::new("json-maint");
    let cwd = w.plain_dir("cwd");
    w.project(P, "thing");
    let store = w.store();
    let mut old = item(Kind::Log, "an ancient log line", "body");
    let long_ago =
        jiff::Timestamp::from_second(jiff::Timestamp::now().as_second() - 200 * 86_400).unwrap();
    old.meta.created = long_ago;
    old.meta.modified = long_ago;
    put(&store, Some(P), &old);

    let candidates = validate("prune.json", &mem(&w, &cwd, &["prune", "--json"]));
    let short = candidates["candidates"][0]["short_id"]
        .as_str()
        .expect("a stale log is a candidate")
        .to_string();
    let applied = validate(
        "prune-apply.json",
        &mem(&w, &cwd, &["prune", "--apply", &short, "--json"]),
    );
    assert_eq!(applied["archived"][0], serde_json::json!(short));

    // The sync seam moves the status file, which is how `mem sync` tells a real
    // round from qshell-sync's zero exit while its flock is held.
    let status = w.dirs().qshell_status_json();
    std::fs::create_dir_all(status.parent().unwrap()).unwrap();
    std::fs::write(
        &status,
        br#"{"version":2,"lastRun":"2026-08-18T10:00:00Z","running":false,
            "units":[{"id":"memory","ok":true,"lastRun":"2026-08-18T10:00:00Z",
                      "lastOk":"2026-08-18T10:00:00Z","error":"","errorKind":""}]}"#,
    )
    .unwrap();
    let script = w.dir.join("fake-sync");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncat > {} <<'JSON'\n{{\"version\":2,\"lastRun\":\"2026-08-18T11:00:00Z\",\
             \"running\":false,\"units\":[{{\"id\":\"memory\",\"ok\":true,\
             \"lastRun\":\"2026-08-18T11:00:00Z\",\"lastOk\":\"2026-08-18T11:00:00Z\",\
             \"error\":\"\",\"errorKind\":\"\"}}]}}\nJSON\n",
            status.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let out = mem_env(
        &w,
        &cwd,
        &["sync", "--json"],
        &[("MEM_SYNC_CMD", script.to_str().unwrap())],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let synced = validate("sync.json", &out);
    assert_eq!(synced["sync"], serde_json::json!("performed"));
}

#[test]
fn the_hook_envelopes_match_the_shapes_the_runtime_validates() {
    let w = World::new("json-hooks");
    w.project(P, "thing");
    put(
        &w.store(),
        Some(P),
        &item(Kind::Handoff, "stopped mid-migration", "next: run it"),
    );
    let cwd = w.plain_dir("cwd");

    let hook = [
        "context",
        "thing",
        "--brief",
        "--hook-json",
        "--session-id",
        "s1",
    ];
    let mut last = None;
    for _ in 1..=5 {
        last = Some(mem(&w, &cwd, &hook));
    }
    validate("hook-post-tool-batch.json", &last.unwrap());
    validate(
        "hook-stop.json",
        &mem(
            &w,
            &cwd,
            &["session-check", "--session-id", "s1", "--hook-json"],
        ),
    );
}

#[test]
fn an_empty_filtered_read_still_answers_with_a_document() {
    // Spec §7: exit 1, and an empty array on stdout — the caller for whom empty
    // is a fine answer tests the array, not the code.
    let w = World::new("json-empty");
    let repo = w.repo("thing", None);
    assert_eq!(code(&mem(&w, &repo, &["save", "a fact"])), 0);

    for (schema, args, key) in [
        (
            "search.json",
            vec!["search", "nothingmatchesthis", "--json"],
            "hits",
        ),
        (
            "log.json",
            vec!["log", "--kind", "ruling", "--type", "no-verifier", "--json"],
            "items",
        ),
        (
            "questions.json",
            vec!["questions", "--pending", "--json"],
            "questions",
        ),
    ] {
        let out = mem(&w, &repo, &args);
        assert_eq!(code(&out), 1, "{args:?} matched nothing, so exit 1");
        let value = validate(schema, &out);
        assert_eq!(
            value[key],
            serde_json::json!([]),
            "{args:?} must still print its array"
        );
    }

    // A read that resolves nothing at all prints nothing: there is no half
    // document to parse.
    let out = mem(&w, &repo, &["show", "ZZZZZZZZ", "--json"]);
    assert_eq!(code(&out), 1);
    assert!(out.stdout.is_empty());
    let out = mem(&w, &repo, &["project", "current", "--json"]);
    assert_eq!(code(&out), 0, "this checkout was registered by the save");
    assert!(!out.stdout.is_empty());
}

#[test]
fn every_schema_is_covered_by_this_test() {
    // A schema nobody validates is a document, not a contract.
    let mut files: BTreeSet<String> = std::fs::read_dir(schema_dir())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".json"))
        .collect();
    let source =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(file!())).unwrap();
    // item.json is reached through $ref from the verbs that carry rows.
    files.remove("item.json");
    let missing: Vec<&String> = files
        .iter()
        .filter(|name| !source.contains(&format!("\"{name}\"")))
        .collect();
    assert!(missing.is_empty(), "never validated: {missing:?}");
    assert!(
        source.contains("item.json"),
        "item.json must stay reachable"
    );
}
