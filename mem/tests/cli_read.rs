//! The read verbs through the binary: exit codes and empty states.

mod common;

use common::{World, code, item, mem, put, stderr, stdout};
use mem::item::Kind;

const P: &str = "01K2AAAAAAAAAAAAAAAAAAAAAA";

#[test]
fn read_verbs_on_a_fresh_machine_are_helpful_not_fatal() {
    let w = World::new("cli-empty");
    let dir = w.plain_dir("nowhere");

    let out = mem(&w, &dir, &["projects"]);
    assert_eq!(code(&out), 0, "{}", common::stderr(&out));
    assert!(stdout(&out).contains("no projects yet"));

    let out = mem(&w, &dir, &["search", "anything"]);
    assert_eq!(code(&out), 1, "a query that finds nothing is exit 1");
    assert!(stdout(&out).is_empty());

    let out = mem(&w, &dir, &["show", "01K2YR1VC0AB3DE4FG5HJ6KM7N"]);
    assert_eq!(code(&out), 1);
    let out = mem(&w, &dir, &["projects", "--json"]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out).trim(), r#"{"projects":[]}"#);
}

#[test]
fn search_prints_ranked_lines_and_show_prints_the_file() {
    let w = World::new("cli-search");
    w.project(P, "thing");
    let store = w.store();
    let it = item(Kind::Fact, "sessions use redis", "clitoken body here");
    let path = put(&store, Some(P), &it);
    let dir = w.plain_dir("cwd");

    let out = mem(&w, &dir, &["search", "clitoken", "--scope", "all"]);
    assert_eq!(code(&out), 0, "{}", common::stderr(&out));
    let line = stdout(&out);
    assert!(
        line.starts_with(&format!("#{}", it.meta.short_id())),
        "{line}"
    );
    assert!(line.contains("1.00"), "{line}");
    assert!(line.trim_end().len() <= 80);

    let out = mem(&w, &dir, &["show", &it.meta.short_id()]);
    assert_eq!(code(&out), 0);
    assert_eq!(
        out.stdout,
        std::fs::read(&path).unwrap(),
        "show prints the file"
    );

    let out = mem(&w, &dir, &["show", &it.meta.id, "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["items"][0]["id"], serde_json::json!(it.meta.id));
    assert_eq!(
        v["items"][0]["body"],
        serde_json::json!("clitoken body here")
    );
}

#[test]
fn an_ambiguous_short_id_exits_seven_with_candidates() {
    let w = World::new("cli-ambiguous");
    w.project(P, "thing");
    let store = w.store();
    // Two ids that differ only above the suffix: exactly what a sync merge can
    // produce, since files are never renamed after they are written.
    for prefix in ["01K2YR1VC0", "01K2ZZ1VC0"] {
        let mut it = item(Kind::Fact, "collides", "body");
        it.meta.id = format!("{prefix}AB3DE4FG5HJ6KM7N");
        put(&store, Some(P), &it);
    }
    let dir = w.plain_dir("cwd");
    let out = mem(&w, &dir, &["show", "5HJ6KM7N"]);
    assert_eq!(code(&out), 7);
    let err = common::stderr(&out);
    assert!(err.contains("ambiguous"), "{err}");
    assert!(err.contains("01K2YR1VC0AB3DE4FG5HJ6KM7N"), "{err}");
}

#[test]
fn a_malformed_id_and_an_unknown_id_both_exit_one() {
    let w = World::new("cli-badid");
    let dir = w.plain_dir("cwd");
    assert_eq!(code(&mem(&w, &dir, &["show", "not-an-id"])), 1);
    assert_eq!(code(&mem(&w, &dir, &["show", "01K2YR"])), 1);
    assert_eq!(code(&mem(&w, &dir, &["show", "5HJ6KM7N"])), 1);
}

#[test]
fn no_command_is_a_usage_error() {
    let w = World::new("cli-usage");
    let dir = w.plain_dir("cwd");
    assert_eq!(code(&mem(&w, &dir, &[])), 2);
    assert_eq!(code(&mem(&w, &dir, &["nosuchverb"])), 2);
}

#[test]
fn project_current_answers_only_for_a_registered_checkout() {
    let w = World::new("cli-project-current");
    let repo = w.repo("thing", Some("git@github.com:me/thing.git"));

    // Unregistered: exit 1, nothing on stdout, and nothing created.
    let out = mem(&w, &repo, &["project", "current"]);
    assert_eq!(code(&out), 1, "an unregistered checkout is exit 1");
    assert!(stdout(&out).is_empty());
    assert!(
        !w.store().projects_dir().exists(),
        "a read verb never registers"
    );
    assert_eq!(code(&mem(&w, &repo, &["project", "current", "--json"])), 1);

    // A write registers it; then the verb answers with id, name and root.
    assert_eq!(code(&mem(&w, &repo, &["save", "a fact"])), 0);
    let id = mem::project::Registry::load(&w.store()).projects[0]
        .id
        .clone();

    let out = mem(&w, &repo, &["project", "current"]);
    assert_eq!(code(&out), 0, "{}", common::stderr(&out));
    let text = stdout(&out);
    assert!(text.contains(&id), "{text}");
    assert!(text.contains("thing"), "{text}");

    let out = mem(&w, &repo, &["project", "current", "--json"]);
    assert_eq!(code(&out), 0);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["id"], serde_json::json!(id));
    assert_eq!(v["name"], serde_json::json!("thing"));
    assert_eq!(
        std::fs::canonicalize(v["root"].as_str().unwrap()).unwrap(),
        std::fs::canonicalize(&repo).unwrap(),
        "root is the checkout the question was asked from"
    );

    // From a subdirectory the answer is the same, root included.
    let sub = repo.join("src/deep");
    std::fs::create_dir_all(&sub).unwrap();
    let v: serde_json::Value =
        serde_json::from_slice(&mem(&w, &sub, &["project", "current", "--json"]).stdout)
            .expect("json");
    assert_eq!(v["id"], serde_json::json!(id));
    assert_eq!(
        std::fs::canonicalize(v["root"].as_str().unwrap()).unwrap(),
        std::fs::canonicalize(&repo).unwrap()
    );

    // Outside a checkout there is no project to name.
    assert_eq!(
        code(&mem(&w, &w.plain_dir("loose"), &["project", "current"])),
        1
    );
}

#[test]
fn a_read_verb_leaves_no_footprint_in_the_repository() {
    let w = World::new("cli-footprint");
    let repo = w.repo("thing", Some("git@github.com:me/thing.git"));
    for args in [
        vec!["search", "anything"],
        vec!["projects"],
        vec!["show", "5HJ6KM7N"],
    ] {
        mem(&w, &repo, &args);
    }
    let out = std::process::Command::new("git")
        .current_dir(&repo)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(
        out.stdout.is_empty(),
        "a read verb dirtied the repo: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(!repo.join(".mem").exists());
    assert!(!repo.join(".focus").exists());
}

/// Older binaries on other machines share this store, so what they wrote has
/// to keep reading cleanly here: project.toml keys this mem no longer sets or
/// reads, a status.md, a roadmap status it no longer accepts, and a question
/// that carries the task that asked it.
#[test]
fn a_store_an_older_mem_wrote_still_reads_cleanly() {
    let w = World::new("cli-older-store");
    w.project(P, "thing");
    let store = w.store();
    let toml = store.project_toml(P);
    let mut text = std::fs::read_to_string(&toml).unwrap();
    text.push_str(
        "runner = \"mini\"\nrunner_since = \"2026-10-01T00:00:00Z\"\npaused = \"mini 2026-10-03\"\n\
         slots = \"2\"\ndev = \"just dev\"\npreview = \"http://localhost:5173\"\n\
         surface = \"web\"\ndogfood_machine = \"nuc\"\nreview_paths = \"app/**\"\n\
         model = \"sonnet\"\neffort = \"max\"\nplan_status = \"running\"\n\
         roadmap_status = \"maintenance\"\n",
    );
    std::fs::write(&toml, &text).unwrap();
    std::fs::write(store.project_dir(P).join("status.md"), "m2 under way\n").unwrap();
    std::fs::write(
        store.roadmap_path(P),
        "# roadmap: v2\n\n- [ ] m1-auth Sign-in\n",
    )
    .unwrap();
    let mut q = item(Kind::Question, "retry t2?", "retry t2?");
    q.meta.id = "01K2YR1VC0AB3DE4FG5HJ6KM7N".to_string();
    q.meta.audience = Some("orchestrator".to_string());
    q.meta.task = Some("m2/t2".to_string());
    put(&store, Some(P), &q);
    let dir = w.plain_dir("cwd");
    let read = |args: &[&str]| {
        let mut all = vec!["--project", "thing"];
        all.extend(args);
        let out = mem(&w, &dir, &all);
        assert_eq!(code(&out), 0, "{args:?}: {}", stderr(&out));
        stdout(&out)
    };
    let json =
        |args: &[&str]| -> serde_json::Value { serde_json::from_str(&read(args)).expect("json") };

    read(&["reindex", "--full"]);
    let row = &json(&["projects", "--json"])["projects"][0];
    assert_eq!(row["roadmap_status"], "maintenance", "{row}");
    let current = json(&["project", "current", "--json"]);
    assert_eq!(current["name"], "thing", "{current}");
    for gone in [
        "runner",
        "paused",
        "slots",
        "model",
        "effort",
        "review_paths",
    ] {
        assert!(current.get(gone).is_none(), "{gone}: {current}");
    }
    assert_eq!(read(&["roadmap", "--status"]), "maintenance\n");
    let questions = json(&["questions", "--json"]);
    assert_eq!(questions["questions"][0]["task"], "m2/t2", "{questions}");
    assert_eq!(questions["questions"][0]["audience"], "orchestrator");
    let listing = read(&["questions"]);
    assert!(listing.contains("[orchestrator] retry t2?"), "{listing}");
    let full = read(&["context", "--full"]);
    assert!(full.contains("[orchestrator] retry t2?"), "{full}");
    assert!(read(&["show", "5HJ6KM7N"]).contains("task = \"m2/t2\""));
    let context = read(&["context"]);
    assert!(context.starts_with("project: thing\n"), "{context}");
    assert!(!context.contains("m2 under way"), "{context}");
    let doctor = read(&["doctor"]);
    assert!(!doctor.contains("status.md"), "{doctor}");
    assert!(!doctor.contains("project.toml"), "{doctor}");
    assert_eq!(
        std::fs::read_to_string(&toml).unwrap(),
        text,
        "a read writes nothing"
    );
}
