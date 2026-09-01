//! The write verbs (spec §7, §4): registration, CAS, budgets, session activity.

mod common;

use common::{World, code, mem, run_git, stderr, stdout};
use mem::index::{Index, Purpose};

fn json(out: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).expect("json output")
}

#[test]
fn saving_in_a_fresh_checkout_registers_it_and_stores_the_item() {
    let w = World::new("write-save");
    let repo = w.repo("thing", Some("git@github.com:me/thing.git"));

    let out = mem(
        &w,
        &repo,
        &[
            "save",
            "sessions use redis, not the database driver",
            "--json",
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v = json(&out);
    let id = v["id"].as_str().unwrap().to_string();
    assert_eq!(v["kind"], serde_json::json!("fact"));
    assert!(std::path::Path::new(v["path"].as_str().unwrap()).exists());

    // The write registered the project, and the item carries its name.
    let projects = mem::project::Registry::load(&w.store());
    assert_eq!(projects.projects.len(), 1);
    assert_eq!(projects.projects[0].name, "thing");
    let item = mem::store::read_item(std::path::Path::new(v["path"].as_str().unwrap())).unwrap();
    assert_eq!(item.meta.project.as_deref(), Some("thing"));
    assert_eq!(
        item.meta.title,
        "sessions use redis, not the database driver"
    );
    assert!(item.body_str().contains("sessions use redis"));

    // And it is findable.
    let out = mem(&w, &repo, &["search", "redis"]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains(&mem::ids::short_id(&id)));
}

#[test]
fn save_takes_kind_type_tags_and_supersedes() {
    let w = World::new("write-flags");
    let repo = w.repo("thing", None);
    let first = json(&mem(&w, &repo, &["save", "old fact", "--json"]));
    let old_path = first["path"].as_str().unwrap().to_string();
    let old_bytes = std::fs::read(&old_path).unwrap();

    let out = mem(
        &w,
        &repo,
        &[
            "save",
            "new fact",
            "--kind",
            "ruling",
            "--type",
            "decision",
            "--tags",
            "redis,sessions",
            "--supersedes",
            first["short_id"].as_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let item =
        mem::store::read_item(std::path::Path::new(json(&out)["path"].as_str().unwrap())).unwrap();
    assert_eq!(item.meta.kind.as_str(), "ruling");
    assert_eq!(item.meta.r#type.as_deref(), Some("decision"));
    assert_eq!(item.meta.tags, vec!["redis", "sessions"]);
    assert_eq!(
        item.meta.supersedes.as_deref(),
        Some(first["id"].as_str().unwrap())
    );
    assert_eq!(
        std::fs::read(&old_path).unwrap(),
        old_bytes,
        "the superseded file must not be touched"
    );

    // An unknown supersedes target is exit 1, and nothing is written.
    let before = w.store().item_paths().len();
    let out = mem(&w, &repo, &["save", "x", "--supersedes", "ZZZZZZZZ"]);
    assert_eq!(code(&out), 1);
    assert_eq!(w.store().item_paths().len(), before);
}

#[test]
fn log_writes_with_text_and_reads_without() {
    let w = World::new("write-log");
    let repo = w.repo("thing", None);
    assert_eq!(code(&mem(&w, &repo, &["log", "ran the migration"])), 0);
    assert_eq!(code(&mem(&w, &repo, &["log", "fixed the deadlock"])), 0);

    let out = mem(&w, &repo, &["log"]);
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    assert!(text.contains("ran the migration"), "{text}");
    assert!(text.contains("fixed the deadlock"), "{text}");
    assert_eq!(text.lines().count(), 2);

    assert_eq!(code(&mem(&w, &repo, &["log", "--limit", "1"])), 0);
    assert_eq!(
        stdout(&mem(&w, &repo, &["log", "--limit", "1"]))
            .lines()
            .count(),
        1
    );

    // --since takes both grammars; a future floor filters everything out.
    assert_eq!(code(&mem(&w, &repo, &["log", "--since", "2d"])), 0);
    assert_eq!(
        code(&mem(&w, &repo, &["log", "--since", "2099-01-01T00:00:00Z"])),
        1
    );
    assert_eq!(code(&mem(&w, &repo, &["log", "--since", "nonsense"])), 2);
}

#[test]
fn handoff_sets_and_prints_the_latest() {
    let w = World::new("write-handoff");
    let repo = w.repo("thing", None);
    assert_eq!(code(&mem(&w, &repo, &["handoff"])), 1, "nothing yet");

    // The sync trigger is a seam so a test never shells out to the real unit.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mem"))
        .current_dir(&repo)
        .args(["handoff", "--set", "stopped mid-migration; next: run it"])
        .env("XDG_DATA_HOME", w.dirs().data)
        .env("XDG_CACHE_HOME", w.dirs().cache)
        .env("XDG_STATE_HOME", w.dirs().state)
        .env("XDG_CONFIG_HOME", w.dirs().config)
        .env("MEM_SYNC_CMD", "true")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = mem(&w, &repo, &["handoff"]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains("stopped mid-migration"));
}

#[test]
fn status_over_cap_lands_on_disk_and_exits_six() {
    let w = World::new("write-status");
    let repo = w.repo("thing", None);
    assert_eq!(
        code(&mem(&w, &repo, &["status"])),
        1,
        "nothing recorded yet"
    );

    assert_eq!(
        code(&mem(&w, &repo, &["status", "--set", "blocked on review"])),
        0
    );
    let out = mem(&w, &repo, &["status"]);
    assert_eq!(code(&out), 0);
    assert_eq!(
        stdout(&out),
        "blocked on review\n",
        "status prints verbatim"
    );

    let long = "a status line that goes on\n".repeat(40);
    let out = mem(&w, &repo, &["status", "--set", &long]);
    assert_eq!(code(&out), 6, "accepted, but over budget");
    assert!(stderr(&out).contains("over budget"), "{}", stderr(&out));
    assert!(
        stdout(&mem(&w, &repo, &["status"])).contains("a status line that goes on"),
        "an over-cap status must still be on disk"
    );
}

#[test]
fn a_singleton_write_against_a_changed_file_is_a_conflict() {
    let w = World::new("write-cas");
    let repo = w.repo("thing", None);
    mem(&w, &repo, &["status", "--set", "first"]);
    let id = mem::project::Registry::load(&w.store()).projects[0]
        .id
        .clone();
    let path = w.store().status_path(&id);

    // A write whose CAS baseline is the file as it is now succeeds; the test
    // for the conflict path lives at the seam, where the race is reproducible.
    let seen = mem::atomic::read_mtime(&path);
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&path, b"someone else wrote this\n").unwrap();
    assert!(
        !mem::atomic::write_atomic_cas(&path, b"mine\n", seen).unwrap(),
        "a changed file must not be clobbered"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"someone else wrote this\n");
}

#[test]
fn plan_sets_prints_and_clears() {
    let w = World::new("write-plan");
    let repo = w.repo("thing", None);
    assert_eq!(code(&mem(&w, &repo, &["plan"])), 1);

    let file = w.dir.join("plan.md");
    std::fs::write(&file, "# Migrate sessions\n- [ ] run it\n").unwrap();
    assert_eq!(
        code(&mem(
            &w,
            &repo,
            &["plan", "--set-file", file.to_str().unwrap()]
        )),
        0
    );
    let out = mem(&w, &repo, &["plan"]);
    assert_eq!(stdout(&out), "# Migrate sessions\n- [ ] run it\n");
    assert_eq!(code(&out), 0);

    assert_eq!(code(&mem(&w, &repo, &["plan", "--clear"])), 0);
    assert_eq!(code(&mem(&w, &repo, &["plan"])), 1);
    assert_eq!(
        code(&mem(
            &w,
            &repo,
            &["plan", "--set-file", "/nonexistent/plan.md"]
        )),
        1
    );
}

#[test]
fn ticking_a_task_flips_one_checkbox_and_leaves_the_rest_alone() {
    let w = World::new("write-tick");
    let repo = w.repo("thing", None);
    let plan = "# plan: cart-pricing-v2\n\n\
        - [ ] t1 Extract cart pricing into a service  [after: t0]\n\
        \x20     Files: app/Services/Cart*.php\n\
        \x20     Verify: bin/php artisan test --filter=Cart\n\
        - [ ] t10 Delete the old helper\n\
        - [x] t2 Already done\n";
    let file = w.dir.join("plan.md");
    std::fs::write(&file, plan).unwrap();
    assert_eq!(
        code(&mem(
            &w,
            &repo,
            &["plan", "--set-file", file.to_str().unwrap()]
        )),
        0
    );

    let out = mem(&w, &repo, &["plan", "--tick", "t1"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let after = stdout(&mem(&w, &repo, &["plan"]));
    assert!(
        after.contains("- [x] t1 Extract cart pricing into a service  [after: t0]"),
        "{after}"
    );
    assert!(after.contains("- [ ] t10 Delete the old helper"), "{after}");
    assert!(
        after.contains("      Verify: bin/php artisan test --filter=Cart"),
        "the rest of the plan is byte-preserved: {after}"
    );
    assert_eq!(after.lines().count(), plan.lines().count());

    // Ticking it again is a no-op that still succeeds.
    let out = mem(&w, &repo, &["plan", "--tick", "t1", "--json"]);
    assert_eq!(code(&out), 0);
    assert_eq!(json(&out)["ticked"], serde_json::json!(false));
    assert_eq!(stdout(&mem(&w, &repo, &["plan"])), after);

    // An id that no task line carries is exit 1, and nothing is written.
    let out = mem(&w, &repo, &["plan", "--tick", "t99"]);
    assert_eq!(code(&out), 1);
    assert_eq!(stdout(&mem(&w, &repo, &["plan"])), after);

    // A prefix of a real id is not a match either.
    assert_eq!(code(&mem(&w, &repo, &["plan", "--tick", "t"])), 1);

    // With no plan at all there is nothing to tick.
    assert_eq!(code(&mem(&w, &repo, &["plan", "--clear"])), 0);
    assert_eq!(code(&mem(&w, &repo, &["plan", "--tick", "t1"])), 1);
}

#[test]
fn writes_are_recorded_against_the_session() {
    let w = World::new("write-session");
    let repo = w.repo("thing", None);
    let sessions = w.dirs().sessions_dir();

    assert_eq!(mem::session::read(&sessions, "s1").writes, 0);
    assert_eq!(
        code(&mem(&w, &repo, &["save", "a fact", "--session-id", "s1"])),
        0
    );
    assert_eq!(
        code(&mem(
            &w,
            &repo,
            &["log", "did a thing", "--session-id", "s1"]
        )),
        0
    );
    assert_eq!(mem::session::read(&sessions, "s1").writes, 2);
    assert_eq!(mem::session::read(&sessions, "s2").writes, 0);

    // A read verb records nothing.
    mem(&w, &repo, &["search", "fact", "--session-id", "s1"]);
    assert_eq!(mem::session::read(&sessions, "s1").writes, 2);

    // A session id may not escape the sessions directory.
    assert_eq!(
        mem::session::path(&sessions, "../../escape")
            .file_name()
            .unwrap(),
        "______escape"
    );
}

#[test]
fn a_write_outside_a_checkout_goes_to_global_scope() {
    let w = World::new("write-global");
    let loose = w.plain_dir("loose");
    let out = mem(&w, &loose, &["save", "a global fact", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let path = json(&out)["path"].as_str().unwrap().to_string();
    assert!(path.contains("/global/items/"), "{path}");
    assert!(
        !w.store().projects_dir().exists(),
        "no project was invented"
    );

    // A named project from outside a checkout still needs to exist.
    assert_eq!(
        code(&mem(&w, &loose, &["save", "x", "--project", "nope"])),
        1
    );
}

#[test]
fn a_write_verb_leaves_no_footprint_in_the_repository() {
    let w = World::new("write-footprint");
    let repo = w.repo("thing", None);
    run_git(&repo, &["config", "user.email", "t@example.com"]);
    run_git(&repo, &["config", "user.name", "T"]);
    std::fs::write(repo.join("f.txt"), b"x").unwrap();
    run_git(&repo, &["add", "f.txt"]);
    run_git(&repo, &["commit", "-qm", "first"]);

    mem(&w, &repo, &["save", "a fact"]);
    mem(&w, &repo, &["log", "an entry"]);
    mem(&w, &repo, &["status", "--set", "fine"]);

    let out = std::process::Command::new("git")
        .current_dir(&repo)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(
        out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(!repo.join(".mem").exists() && !repo.join(".focus").exists());
}

#[test]
fn an_item_written_by_the_binary_is_indexed_and_shown() {
    let w = World::new("write-roundtrip");
    let repo = w.repo("thing", None);
    let v: serde_json::Value = json(&mem(&w, &repo, &["save", "round trip fact", "--json"]));
    let index = Index::open(&w.index_path(), Purpose::Read).unwrap();
    index.reindex(&w.store(), false).unwrap();
    let row = index.get(v["id"].as_str().unwrap()).unwrap().unwrap();
    assert_eq!(row.title, "round trip fact");
    assert!(row.active);
    let out = mem(&w, &repo, &["show", v["short_id"].as_str().unwrap()]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains("round trip fact"));
}

#[test]
fn project_set_verify_records_the_command_and_project_current_reports_it() {
    let w = World::new("write-verify");
    let repo = w.repo("thing", Some("git@github.com:me/thing.git"));

    // Nothing declared yet: the field is simply absent.
    assert_eq!(code(&mem(&w, &repo, &["log", "first write"])), 0);
    let v = json(&mem(&w, &repo, &["project", "current", "--json"]));
    assert!(v.get("verify").is_none(), "{v}");

    let out = mem(&w, &repo, &["project", "set", "verify", "just test"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let v = json(&mem(&w, &repo, &["project", "current", "--json"]));
    assert_eq!(v["verify"], serde_json::json!("just test"));
    assert!(
        stdout(&mem(&w, &repo, &["project", "current"])).contains("just test"),
        "the plain rendering names the command too"
    );

    // Setting it again replaces the old command rather than adding a second one.
    assert_eq!(
        code(&mem(
            &w,
            &repo,
            &["project", "set", "verify", "make test && make lint"]
        )),
        0
    );
    let v = json(&mem(&w, &repo, &["project", "current", "--json"]));
    assert_eq!(v["verify"], serde_json::json!("make test && make lint"));

    let id = mem::project::Registry::load(&w.store()).projects[0]
        .id
        .clone();
    let text = std::fs::read_to_string(w.store().project_toml(&id)).unwrap();
    assert_eq!(
        text.matches("verify = ").count(),
        1,
        "one verify key, not two: {text}"
    );

    // An empty command is a usage error, not a verifier that runs nothing.
    let out = mem(&w, &repo, &["project", "set", "verify", "   "]);
    assert_eq!(code(&out), 2, "{}", stdout(&out));
    let v = json(&mem(&w, &repo, &["project", "current", "--json"]));
    assert_eq!(v["verify"], serde_json::json!("make test && make lint"));
}

#[test]
fn project_set_verify_keeps_keys_this_version_does_not_know_about() {
    let w = World::new("write-verify-unknown");
    let repo = w.repo("thing", None);
    assert_eq!(code(&mem(&w, &repo, &["log", "register it"])), 0);

    let id = mem::project::Registry::load(&w.store()).projects[0]
        .id
        .clone();
    let path = w.store().project_toml(&id);
    let original = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        format!("{original}future_scalar = \"keep me\"\n\n[future_table]\nnested = 1\n"),
    )
    .unwrap();

    assert_eq!(
        code(&mem(&w, &repo, &["project", "set", "verify", "cargo test"])),
        0
    );

    let after: toml::Table = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(after["verify"].as_str(), Some("cargo test"));
    assert_eq!(after["future_scalar"].as_str(), Some("keep me"));
    assert_eq!(after["future_table"]["nested"].as_integer(), Some(1));
    assert!(after.contains_key("created"), "{after:?}");
    assert_eq!(after["name"].as_str(), Some("thing"));
}

#[test]
fn project_set_review_paths_records_the_globs_and_project_current_reports_them() {
    let w = World::new("write-review-paths");
    let repo = w.repo("thing", Some("git@github.com:me/thing.git"));

    // Nothing declared yet: the field is simply absent, and the global table
    // is the whole answer.
    assert_eq!(code(&mem(&w, &repo, &["log", "first write"])), 0);
    let v = json(&mem(&w, &repo, &["project", "current", "--json"]));
    assert!(v.get("review_paths").is_none(), "{v}");

    let out = mem(
        &w,
        &repo,
        &[
            "project",
            "set",
            "review-paths",
            "packages/shopify-core/** scripts/mutate.py",
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let v = json(&mem(&w, &repo, &["project", "current", "--json"]));
    assert_eq!(
        v["review_paths"],
        serde_json::json!("packages/shopify-core/** scripts/mutate.py")
    );
    assert!(
        stdout(&mem(&w, &repo, &["project", "current"])).contains("scripts/mutate.py"),
        "the plain rendering names the globs too"
    );

    // Setting them again replaces the list rather than appending to it.
    assert_eq!(
        code(&mem(&w, &repo, &["project", "set", "review-paths", "app/**"])),
        0
    );
    let v = json(&mem(&w, &repo, &["project", "current", "--json"]));
    assert_eq!(v["review_paths"], serde_json::json!("app/**"));

    let id = mem::project::Registry::load(&w.store()).projects[0]
        .id
        .clone();
    let text = std::fs::read_to_string(w.store().project_toml(&id)).unwrap();
    assert_eq!(
        text.matches("review_paths = ").count(),
        1,
        "one review_paths key, not two: {text}"
    );

    // Nothing at all is a usage error, not a project that claims no paths.
    let out = mem(&w, &repo, &["project", "set", "review-paths", "  "]);
    assert_eq!(code(&out), 2, "{}", stdout(&out));
    let v = json(&mem(&w, &repo, &["project", "current", "--json"]));
    assert_eq!(v["review_paths"], serde_json::json!("app/**"));
}

#[test]
fn project_set_backend_records_the_choice_and_refuses_anything_else() {
    let w = World::new("write-backend");
    let repo = w.repo("thing", Some("git@github.com:me/thing.git"));

    // Nothing declared yet: the field is absent, and the caller runs whatever
    // it defaults to.
    assert_eq!(code(&mem(&w, &repo, &["log", "first write"])), 0);
    let v = json(&mem(&w, &repo, &["project", "current", "--json"]));
    assert!(v.get("backend").is_none(), "{v}");

    let out = mem(&w, &repo, &["project", "set", "backend", "amx"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let v = json(&mem(&w, &repo, &["project", "current", "--json"]));
    assert_eq!(v["backend"], serde_json::json!("amx"));
    assert!(
        stdout(&mem(&w, &repo, &["project", "current"])).contains("backend  amx"),
        "the plain rendering names it too"
    );

    // Choosing again replaces the choice rather than leaving two keys behind.
    assert_eq!(
        code(&mem(&w, &repo, &["project", "set", "backend", "claude"])),
        0
    );
    let v = json(&mem(&w, &repo, &["project", "current", "--json"]));
    assert_eq!(v["backend"], serde_json::json!("claude"));

    let id = mem::project::Registry::load(&w.store()).projects[0]
        .id
        .clone();
    let text = std::fs::read_to_string(w.store().project_toml(&id)).unwrap();
    assert_eq!(
        text.matches("backend = ").count(),
        1,
        "one backend key, not two: {text}"
    );

    // A backend nothing implements is a usage error, and the stored choice
    // survives it: a typo must not leave the project pointing at nothing.
    let out = mem(&w, &repo, &["project", "set", "backend", "gemini"]);
    assert_eq!(code(&out), 2, "{}", stdout(&out));
    let v = json(&mem(&w, &repo, &["project", "current", "--json"]));
    assert_eq!(v["backend"], serde_json::json!("claude"));
}

#[test]
fn project_add_registers_a_child_the_subdir_then_owns() {
    let w = World::new("write-child");
    let repo = w.repo("mono", Some("git@github.com:me/mono.git"));
    std::fs::create_dir_all(repo.join("apps/x")).unwrap();

    let out = mem(&w, &repo, &["project", "add", "apps/x", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v = json(&out);
    assert_eq!(v["name"], serde_json::json!("x"));
    assert_eq!(v["subdir"], serde_json::json!("apps/x"));

    let registry = mem::project::Registry::load(&w.store());
    assert_eq!(registry.projects.len(), 2, "root and child");
    let root = registry.projects.iter().find(|p| p.name == "mono").unwrap();
    let child = registry.projects.iter().find(|p| p.name == "x").unwrap();
    assert_eq!(child.parent.as_deref(), Some(root.id.as_str()));
    assert_eq!(child.subdir.as_deref(), Some("apps/x"));
    assert_eq!(child.remote, root.remote);
    assert!(w.store().project_items(&child.id).is_dir());

    // current in the subdir answers as the child, keeps the checkout root,
    // and says where the child lives.
    let v = json(&mem(&w, &repo.join("apps/x"), &["project", "current", "--json"]));
    assert_eq!(v["id"], serde_json::json!(child.id));
    assert_eq!(v["name"], serde_json::json!("x"));
    assert_eq!(v["subdir"], serde_json::json!("apps/x"));
    assert_eq!(
        v["root"],
        serde_json::json!(mem::git::canonical(&repo).to_string_lossy())
    );

    // A write from the subdir lands on the child, one from the root on the root.
    let v = json(&mem(&w, &repo.join("apps/x"), &["log", "child work", "--json"]));
    let item = mem::store::read_item(std::path::Path::new(v["path"].as_str().unwrap())).unwrap();
    assert_eq!(item.meta.project.as_deref(), Some("x"));
    let v = json(&mem(&w, &repo, &["log", "root work", "--json"]));
    let item = mem::store::read_item(std::path::Path::new(v["path"].as_str().unwrap())).unwrap();
    assert_eq!(item.meta.project.as_deref(), Some("mono"));
}

#[test]
fn project_add_takes_a_name_and_refuses_nonsense() {
    let w = World::new("write-child-refuse");
    let repo = w.repo("mono", Some("git@github.com:me/mono.git"));
    std::fs::create_dir_all(repo.join("apps/x")).unwrap();

    // A trailing slash is spelling, not a different subdir; --name overrides.
    let out = mem(
        &w,
        &repo,
        &["project", "add", "apps/x/", "--name", "renamed", "--json"],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(json(&out)["name"], serde_json::json!("renamed"));
    assert_eq!(json(&out)["subdir"], serde_json::json!("apps/x"));

    for (args, why) in [
        (vec!["project", "add", "apps/x"], "already registered"),
        (vec!["project", "add", "apps/missing"], "no such directory"),
        (vec!["project", "add", "/etc"], "absolute path"),
        (vec!["project", "add", "apps/../apps/x"], "dot-dot"),
        (vec!["project", "add", "."], "the root is not a child"),
    ] {
        let out = mem(&w, &repo, &args);
        assert_ne!(code(&out), 0, "{why}: {}", stdout(&out));
    }
    assert_eq!(
        mem::project::Registry::load(&w.store()).projects.len(),
        2,
        "refusals register nothing"
    );

    // Outside a checkout there is nothing to add to.
    let plain = w.plain_dir("loose");
    let out = mem(&w, &plain, &["project", "add", "x"]);
    assert_ne!(code(&out), 0);
}

#[test]
fn project_set_remote_records_the_normalized_url() {
    let w = World::new("write-remote");
    let repo = w.repo("thing", None);

    // Registered before its remote existed, so the store holds none.
    assert_eq!(code(&mem(&w, &repo, &["log", "first write"])), 0);

    let out = mem(
        &w,
        &repo,
        &["project", "set", "remote", "https://GitHub.com/Acme/App.git"],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let id = mem::project::Registry::load(&w.store()).projects[0]
        .id
        .clone();
    let text = std::fs::read_to_string(w.store().project_toml(&id)).unwrap();
    assert!(
        text.contains("remote = \"github.com/Acme/App\""),
        "recorded exactly as registration would have normalized it: {text}"
    );

    // An empty url is a usage error, not an empty remote.
    let out = mem(&w, &repo, &["project", "set", "remote", "   "]);
    assert_eq!(code(&out), 2, "{}", stdout(&out));
    let text = std::fs::read_to_string(w.store().project_toml(&id)).unwrap();
    assert!(text.contains("remote = \"github.com/Acme/App\""), "{text}");
}
