//! H4 — the three JSON routes, their committed schemas, and AC8.
//!
//! Everything here runs the **real** `mem` against a throwaway store, and hub
//! with `cwd = $HOME`, which is not a git checkout. That last part is the whole
//! point of AC8: the obvious implementation of "recent activity" is green in
//! any test run from inside a repository and permanently empty in the delivered
//! configuration, because a systemd user unit starts in `%h`.

mod common;

use std::path::{Path, PathBuf};

use common::{
    Hub, TempDir, body_of, fixture_mem, mem_in, real_mem, schema, seed_project, status_of,
};
use serde_json::{Value, json};

/// A `mem` on PATH that is the real binary against a store under `home`, with
/// the working directory left exactly as hub set it.
fn mem_on_path(dir: &TempDir, home: &Path) -> PathBuf {
    let real =
        real_mem().expect("build mem: cargo build --release --manifest-path ../mem/Cargo.toml");
    let bin = dir.join("bin");
    fixture_mem(
        &bin,
        &format!(
            "export XDG_DATA_HOME='{home}/data' XDG_CACHE_HOME='{home}/cache'\n\
             export XDG_STATE_HOME='{home}/state' XDG_CONFIG_HOME='{home}/config'\n\
             export MEM_SYNC_CMD=true MEM_NOTIFY_CMD=true\n\
             exec '{real}' \"$@\"",
            home = home.display(),
            real = real.display(),
        ),
    );
    bin
}

/// Two registered projects, each with a log entry, and one pending question.
fn seeded(dir: &TempDir) -> (PathBuf, PathBuf) {
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let bin = mem_on_path(dir, &home);
    let real = real_mem().unwrap();

    seed_project(&real, &home, "proj-alpha", "alpha did a thing");
    seed_project(&real, &home, "proj-beta", "beta did a thing");
    let out = mem_in(
        &real,
        &home,
        &home.join("proj-alpha"),
        &["ask", "Should we use Redis?"],
    );
    assert!(out.status.success(), "{out:?}");
    (home, bin)
}

fn json_at(hub: &Hub, path: &str) -> Value {
    let response = hub.get(path);
    assert_eq!(status_of(&response), 200, "{response}");
    serde_json::from_str(body_of(&response)).unwrap_or_else(|e| panic!("{path}: {e}\n{response}"))
}

#[test]
fn ac8_recent_activity_is_not_empty_from_a_working_directory_that_is_not_a_checkout() {
    let dir = TempDir::new("api-ac8");
    let (home, bin) = seeded(&dir);
    // Hub::spawn runs the binary with `current_dir(home)`, and home is a plain
    // directory: exactly what `WorkingDirectory=%h` gives the unit.
    assert!(!home.join(".git").exists());
    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);

    let document = json_at(&hub, "/api/activity");
    let projects: Vec<&str> = document["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["project"].as_str())
        .collect();
    assert!(
        projects.contains(&"proj-alpha") && projects.contains(&"proj-beta"),
        "both projects' log entries are here: {document:#}"
    );
    assert_eq!(document["degraded"], Value::Null);
}

#[test]
fn activity_is_sorted_by_ulid_descending() {
    let dir = TempDir::new("api-sort");
    let (home, bin) = seeded(&dir);
    let real = real_mem().unwrap();

    // Four more entries in the same second, alternating projects: `created` is
    // date-only, so only the id can order these.
    for n in 0..4 {
        let project = if n % 2 == 0 {
            "proj-alpha"
        } else {
            "proj-beta"
        };
        let out = mem_in(
            &real,
            &home,
            &home.join(project),
            &["log", &format!("entry {n}")],
        );
        assert!(out.status.success());
    }

    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);
    let document = json_at(&hub, "/api/activity");
    let ids: Vec<&str> = document["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["id"].as_str())
        .collect();

    assert_eq!(ids.len(), 6, "{document:#}");
    let mut sorted = ids.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(ids, sorted, "newest first, by id");
    assert!(ids[0] > ids[5], "the span is real, not a one-element list");
}

#[test]
fn a_busy_project_does_not_hide_a_quiet_one_s_last_activity() {
    let dir = TempDir::new("api-busy");
    let (home, bin) = seeded(&dir);
    let real = real_mem().unwrap();

    // Twenty-five more entries in proj-alpha, so proj-beta's single entry is
    // nowhere near the merged top twenty.
    for n in 0..25 {
        let out = mem_in(
            &real,
            &home,
            &home.join("proj-alpha"),
            &["log", &format!("alpha entry {n}")],
        );
        assert!(out.status.success());
    }

    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);

    let activity = json_at(&hub, "/api/activity");
    let items = activity["items"].as_array().unwrap();
    assert_eq!(items.len(), 20, "the section is capped at twenty");
    assert!(
        items.iter().all(|item| item["project"] == "proj-alpha"),
        "and proj-beta is off the end of it"
    );

    // The Projects section reads each project's own log, so proj-beta still
    // reports when it was last touched rather than an em dash.
    let projects = json_at(&hub, "/api/projects");
    let beta = projects["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "proj-beta")
        .unwrap();
    assert!(beta["last_activity"].is_string(), "{beta:#}");
    assert!(beta["last_activity_age"].is_string(), "{beta:#}");
}

#[test]
fn every_document_validates_against_its_committed_schema() {
    let dir = TempDir::new("api-schema");
    let (home, bin) = seeded(&dir);
    let real = real_mem().unwrap();
    // One project with a status.md and one without, so both branches of AC7
    // are in the document being validated.
    let out = mem_in(
        &real,
        &home,
        &home.join("proj-alpha"),
        &["status", "--set", "Green. Everything builds."],
    );
    assert!(out.status.success());

    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);

    for (path, file) in [
        ("/api/questions", "questions.json"),
        ("/api/activity", "activity.json"),
        ("/api/projects", "projects.json"),
    ] {
        let document = json_at(&hub, path);
        schema::assert_valid(file, &document);
    }

    // AC7, in the shape the API reports it: absent is null, never missing.
    let projects = json_at(&hub, "/api/projects");
    let rows = projects["projects"].as_array().unwrap();
    let alpha = rows.iter().find(|p| p["name"] == "proj-alpha").unwrap();
    let beta = rows.iter().find(|p| p["name"] == "proj-beta").unwrap();
    assert_eq!(alpha["status"], "Green. Everything builds.");
    assert_eq!(beta["status"], Value::Null, "no status.md, and no error");
    assert!(beta["last_activity"].is_string(), "{beta:#}");
}

#[test]
fn a_question_document_carries_the_whole_question_and_a_real_timestamp() {
    let dir = TempDir::new("api-questions");
    let (home, bin) = seeded(&dir);
    let real = real_mem().unwrap();
    // The common case: workflow asks batched, numbered, multi-line questions,
    // and mem stores only the first line as `title`.
    let out = mem_in(
        &real,
        &home,
        &home.join("proj-beta"),
        &["ask", "Batched:\n1. redis or sqlite?\n2. ship tonight?"],
    );
    assert!(out.status.success(), "{out:?}");

    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);
    let document = json_at(&hub, "/api/questions");
    schema::assert_valid("questions.json", &document);

    let rows = document["questions"].as_array().unwrap();
    // Two, on the first read. hub serialises its own `mem` calls precisely so
    // that the doorbell's startup poll cannot leave this one answering out of
    // an index that has not seen the ask yet.
    assert_eq!(rows.len(), 2, "{document:#}");
    let batched = rows
        .iter()
        .find(|q| q["project"] == "proj-beta")
        .expect("the batched question");
    assert_eq!(batched["title"], "Batched:", "mem's own title");
    assert!(
        batched["text"]
            .as_str()
            .unwrap()
            .contains("2. ship tonight?"),
        "the whole question is here: {batched:#}"
    );
    // Derived from the ULID, so it has a time of day in it — which mem's own
    // date-only `created` cannot give.
    let asked_at = batched["asked_at"].as_str().unwrap();
    assert!(
        asked_at.contains('T') && asked_at.ends_with('Z'),
        "{asked_at}"
    );
    assert!(
        batched["age"].as_str().unwrap().ends_with('s'),
        "{batched:#}"
    );
}

#[test]
fn an_empty_store_is_empty_arrays_and_not_a_degraded_page() {
    let dir = TempDir::new("api-empty");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let bin = mem_on_path(&dir, &home);
    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);

    for (path, key, file) in [
        ("/api/questions", "questions", "questions.json"),
        ("/api/activity", "items", "activity.json"),
        ("/api/projects", "projects", "projects.json"),
    ] {
        let document = json_at(&hub, path);
        schema::assert_valid(file, &document);
        assert_eq!(document[key].as_array().unwrap().len(), 0, "{path}");
        assert_eq!(
            document["degraded"],
            Value::Null,
            "{path}: mem exits 1 on an empty read, and that is not a fault"
        );
    }
}

#[test]
fn a_missing_mem_is_the_one_thing_that_degrades_and_the_service_keeps_serving() {
    let dir = TempDir::new("api-degraded");
    let home = dir.join("home");
    // No mem anywhere on PATH.
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let hub = Hub::spawn(&home, &[&empty], &["--port", "0"]);

    let document = json_at(&hub, "/api/questions");
    schema::assert_valid("questions.json", &document);
    assert!(
        document["degraded"]
            .as_str()
            .unwrap()
            .contains("could not run mem"),
        "{document:#}"
    );
    assert!(document["questions"].as_array().unwrap().is_empty());

    // AC3: the process does not exit, and the next request still answers.
    assert_eq!(status_of(&hub.get("/api/projects")), 200);
    assert_eq!(status_of(&hub.get("/")), 200);
}

// ---------------------------------------------------------------------------
// The other direction: a schema that cannot fail is not a contract.
// ---------------------------------------------------------------------------

#[test]
fn the_schemas_reject_documents_that_have_drifted() {
    let valid = json!({
        "degraded": null,
        "questions": [{
            "id": "01M0BF1F8BY8FXGZS428J1TSD1",
            "short_id": "28J1TSD1",
            "title": "Should we use Redis?",
            "text": "Should we use Redis?",
            "project": "proj-alpha",
            "machine": "macbook",
            "asked_at": "2026-08-18T22:14:30.923Z",
            "age": "12s"
        }]
    });
    schema::assert_valid("questions.json", &valid);

    // A new key on the envelope.
    let mut drifted = valid.clone();
    drifted["extra"] = json!(1);
    assert!(!schema::problems("questions.json", &drifted).is_empty());

    // A new key on a row.
    let mut drifted = valid.clone();
    drifted["questions"][0]["options"] = json!(["yes", "no"]);
    assert!(!schema::problems("questions.json", &drifted).is_empty());

    // A missing key that the page depends on.
    let mut drifted = valid.clone();
    drifted["questions"][0]
        .as_object_mut()
        .unwrap()
        .remove("age");
    assert!(!schema::problems("questions.json", &drifted).is_empty());

    // A type that changed under us: `project` may be null, but not a number.
    let mut drifted = valid.clone();
    drifted["questions"][0]["project"] = json!(7);
    assert!(!schema::problems("questions.json", &drifted).is_empty());

    // `degraded` is a string or null, never a boolean.
    let mut drifted = valid.clone();
    drifted["degraded"] = json!(true);
    assert!(!schema::problems("questions.json", &drifted).is_empty());

    // And an unknown kind on an activity row.
    let activity = json!({
        "degraded": null,
        "items": [{
            "id": "01M0BF1F8BY8FXGZS428J1TSD1",
            "short_id": "28J1TSD1",
            "kind": "sandwich",
            "title": "alpha did a thing",
            "project": "proj-alpha",
            "machine": "macbook",
            "at": "2026-08-18T22:14:30.923Z",
            "age": "12s"
        }]
    });
    assert!(!schema::problems("activity.json", &activity).is_empty());
}

#[test]
fn every_committed_schema_is_reachable_and_parses() {
    // A `$ref` to a file nobody committed would make the validator panic at
    // the moment it was needed, which is the moment it matters least.
    for file in [
        "questions.json",
        "activity.json",
        "projects.json",
        "question.json",
        "activity-item.json",
        "project.json",
    ] {
        let schema = schema::load(file);
        assert!(schema.get("$schema").is_some(), "{file}");
        assert_eq!(
            schema["additionalProperties"],
            json!(false),
            "{file} is not closed"
        );
    }
    assert!(Path::new(&schema::schema_dir()).join("README.md").is_file());
}
