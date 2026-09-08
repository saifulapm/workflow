//! The roadmap and the stored milestone plans: a plan of plans beside the plan
//! of record, and the copy that makes one of them active.

mod common;

use std::io::Write;
use std::path::Path;

use common::{World, code, mem, stderr, stdout};

fn json(out: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).expect("json output")
}

/// A `--stdin` write needs a real pipe, and a refused one exits before it
/// reads, so a closed pipe here is the behaviour under test.
fn mem_stdin(w: &World, cwd: &Path, args: &[&str], input: &[u8]) -> std::process::Output {
    let dirs = w.dirs();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_mem"))
        .current_dir(cwd)
        .args(args)
        .env("XDG_DATA_HOME", &dirs.data)
        .env("XDG_CACHE_HOME", &dirs.cache)
        .env("XDG_STATE_HOME", &dirs.state)
        .env("XDG_CONFIG_HOME", &dirs.config)
        .env_remove("MEM_SESSION_ID")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn mem");
    if let Err(e) = child.stdin.take().expect("stdin").write_all(input)
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        panic!("write stdin: {e}");
    }
    child.wait_with_output().expect("wait")
}

const ROADMAP: &str = "# roadmap: shop\n\n\
    - [ ] m1-auth Sign-in and sessions\n\
    \x20     Done: users sign in, sessions survive a restart\n\
    - [ ] m2-billing Billing  [after: m1-auth]\n\
    \x20     Done: an order is paid before it ships\n";

/// Writes a file under the world's own directory and hands back its path.
fn file(w: &World, name: &str, text: &str) -> String {
    let path = w.dir.join(name);
    std::fs::write(&path, text).unwrap();
    path.to_str().unwrap().to_string()
}

#[test]
fn the_roadmap_sets_prints_clears_and_ticks() {
    let w = World::new("roadmap-singleton");
    let repo = w.repo("shop", None);
    assert_eq!(code(&mem(&w, &repo, &["roadmap"])), 1, "nothing yet");

    let path = file(&w, "roadmap.md", ROADMAP);
    let out = mem(&w, &repo, &["roadmap", "--set-file", &path]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&mem(&w, &repo, &["roadmap"])), ROADMAP);

    // A milestone ticks by its slug, and nothing else on the page moves.
    let out = mem(&w, &repo, &["roadmap", "--tick", "m1-auth", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v = json(&out);
    assert_eq!(v["id"], serde_json::json!("m1-auth"));
    assert_eq!(v["ticked"], serde_json::json!(true));
    assert!(v["path"].as_str().unwrap().ends_with("/roadmap.md"), "{v}");

    let after = stdout(&mem(&w, &repo, &["roadmap"]));
    assert!(
        after.contains("- [x] m1-auth Sign-in and sessions"),
        "{after}"
    );
    assert!(
        after.contains("- [ ] m2-billing Billing  [after: m1-auth]"),
        "the rest is byte-preserved: {after}"
    );
    assert_eq!(after.lines().count(), ROADMAP.lines().count());

    // Ticking it again is a no-op that still succeeds, and an unknown
    // milestone is exit 1 with nothing written.
    let out = mem(&w, &repo, &["roadmap", "--tick", "m1-auth", "--json"]);
    assert_eq!(code(&out), 0);
    assert_eq!(json(&out)["ticked"], serde_json::json!(false));
    let out = mem(&w, &repo, &["roadmap", "--tick", "m9"]);
    assert_eq!(code(&out), 1);
    assert!(
        stderr(&out).contains("no milestone 'm9' in the roadmap"),
        "{}",
        stderr(&out)
    );
    assert_eq!(stdout(&mem(&w, &repo, &["roadmap"])), after);

    // The plan of record is a different file and is untouched by all of it.
    assert_eq!(code(&mem(&w, &repo, &["plan"])), 1);

    assert_eq!(code(&mem(&w, &repo, &["roadmap", "--clear"])), 0);
    let out = mem(&w, &repo, &["roadmap"]);
    assert_eq!(code(&out), 1);
    assert!(
        stderr(&out).contains("no roadmap recorded for this project"),
        "{}",
        stderr(&out)
    );
    assert_eq!(code(&mem(&w, &repo, &["roadmap", "--tick", "m1-auth"])), 1);
    assert_eq!(
        code(&mem(
            &w,
            &repo,
            &["roadmap", "--set-file", "/nonexistent.md"]
        )),
        1
    );

    // And stdin puts one back.
    let out = mem_stdin(&w, &repo, &["roadmap", "--stdin"], ROADMAP.as_bytes());
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&mem(&w, &repo, &["roadmap"])), ROADMAP);
}

#[test]
fn a_stored_plan_is_written_read_and_cleared_by_its_slug() {
    let w = World::new("roadmap-slot");
    let repo = w.repo("shop", None);
    assert_eq!(
        code(&mem(&w, &repo, &["plan", "m1-auth"])),
        1,
        "nothing yet"
    );

    let text = "# plan: m1-auth\n\n## Tasks\n\n- [ ] t1 Sign-in\n";
    let path = file(&w, "m1-auth.md", text);
    let out = mem(&w, &repo, &["plan", "m1-auth", "--set-file", &path]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let out = mem(&w, &repo, &["plan", "m1-auth"]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), text, "a stored plan prints verbatim");
    let v = json(&mem(&w, &repo, &["plan", "m1-auth", "--json"]));
    assert_eq!(v["slug"], serde_json::json!("m1-auth"));
    assert_eq!(v["text"], serde_json::json!(text));

    // The header is the slug's own: a plan filed under the wrong name is the
    // mistake that would send a run at the wrong milestone.
    let wrong = file(&w, "wrong.md", "# plan: m2-billing\n\n- [ ] t1 Bill\n");
    let out = mem(&w, &repo, &["plan", "m1-auth", "--set-file", &wrong]);
    assert_eq!(code(&out), 2, "{}", stdout(&out));
    assert!(stderr(&out).contains("# plan: m1-auth"), "{}", stderr(&out));
    let headless = file(&w, "headless.md", "- [ ] t1 Sign-in\n");
    assert_eq!(
        code(&mem(
            &w,
            &repo,
            &["plan", "m1-auth", "--set-file", &headless]
        )),
        2
    );
    assert_eq!(
        stdout(&mem(&w, &repo, &["plan", "m1-auth"])),
        text,
        "a refused write leaves the stored plan alone"
    );

    // stdin writes the same way, and is held to the same header.
    let second = "# plan: m2-billing\n\n- [ ] t1 Charge the card\n";
    let out = mem_stdin(
        &w,
        &repo,
        &["plan", "m2-billing", "--stdin"],
        second.as_bytes(),
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&mem(&w, &repo, &["plan", "m2-billing"])), second);
    let out = mem_stdin(
        &w,
        &repo,
        &["plan", "m2-billing", "--stdin"],
        b"no header\n",
    );
    assert_eq!(code(&out), 2, "{}", stdout(&out));
    assert_eq!(stdout(&mem(&w, &repo, &["plan", "m2-billing"])), second);

    // A slug that is not a slug never reaches the filesystem.
    assert_eq!(code(&mem(&w, &repo, &["plan", "../escape"])), 2);
    assert_eq!(code(&mem(&w, &repo, &["plan", "Caps"])), 2);

    // Ticking belongs to the plan of record, not to a stored one.
    assert_ne!(
        code(&mem(&w, &repo, &["plan", "m1-auth", "--tick", "t1"])),
        0
    );

    assert_eq!(code(&mem(&w, &repo, &["plan", "m1-auth", "--clear"])), 0);
    assert_eq!(code(&mem(&w, &repo, &["plan", "m1-auth"])), 1);
    assert_eq!(
        code(&mem(&w, &repo, &["plan", "m2-billing"])),
        0,
        "clearing one leaves the others"
    );
}

#[test]
fn listing_the_stored_plans_names_slug_bytes_date_and_title() {
    let w = World::new("roadmap-list");
    let repo = w.repo("shop", None);
    assert_eq!(code(&mem(&w, &repo, &["plan", "--list"])), 1, "none yet");

    for slug in ["m1-auth", "m2-billing"] {
        let text = format!("# plan: {slug}\n\n- [ ] t1 Do it\n");
        let path = file(&w, &format!("{slug}.md"), &text);
        assert_eq!(
            code(&mem(&w, &repo, &["plan", slug, "--set-file", &path])),
            0
        );
    }

    let out = mem(&w, &repo, &["plan", "--list"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    assert_eq!(text.lines().count(), 2, "{text}");
    assert!(text.contains("m1-auth"), "{text}");
    assert!(text.contains("plan: m1-auth"), "the title: {text}");
    assert!(text.contains("m2-billing"), "{text}");

    let v = json(&mem(&w, &repo, &["plan", "--list", "--json"]));
    let plans = v["plans"].as_array().unwrap();
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0]["slug"], serde_json::json!("m1-auth"));
    assert_eq!(plans[0]["title"], serde_json::json!("plan: m1-auth"));
    assert!(plans[0]["bytes"].as_u64().unwrap() > 0);
    assert_eq!(plans[0]["modified"].as_str().unwrap().len(), 10);
    assert!(plans[0]["path"].as_str().unwrap().ends_with("m1-auth.md"));
}

#[test]
fn from_copies_a_stored_plan_over_the_plan_of_record() {
    let w = World::new("roadmap-from");
    let repo = w.repo("shop", None);

    let first = "# plan: m1-auth\n\n- [ ] t1 Sign-in\n";
    let path = file(&w, "m1-auth.md", first);
    assert_eq!(
        code(&mem(&w, &repo, &["plan", "m1-auth", "--set-file", &path])),
        0
    );
    let second = "# plan: m2-billing\n\n- [ ] t1 Charge the card\n";
    let path = file(&w, "m2-billing.md", second);
    assert_eq!(
        code(&mem(
            &w,
            &repo,
            &["plan", "m2-billing", "--set-file", &path]
        )),
        0
    );

    // Nothing is active, so the first milestone becomes the plan of record.
    let out = mem(&w, &repo, &["plan", "--from", "m1-auth"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&mem(&w, &repo, &["plan"])), first);

    // The next one has to wait: an unchecked task means the run is not done.
    let out = mem(&w, &repo, &["plan", "--from", "m2-billing"]);
    assert_eq!(code(&out), 2, "{}", stdout(&out));
    assert!(
        stderr(&out).contains("t1"),
        "it names the task: {}",
        stderr(&out)
    );
    assert_eq!(stdout(&mem(&w, &repo, &["plan"])), first);

    assert_eq!(code(&mem(&w, &repo, &["plan", "--tick", "t1"])), 0);
    let out = mem(&w, &repo, &["plan", "--from", "m2-billing"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&mem(&w, &repo, &["plan"])), second);

    // The stored plan stays where it is, and an unknown one is exit 1.
    assert_eq!(stdout(&mem(&w, &repo, &["plan", "m1-auth"])), first);
    assert_eq!(code(&mem(&w, &repo, &["plan", "--from", "m9-nope"])), 1);
}

#[test]
fn from_reads_past_the_example_boxes_of_a_finished_plan() {
    let w = World::new("roadmap-from-examples");
    let repo = w.repo("shop", None);

    let second = "# plan: m2-billing\n\n- [ ] t1 Charge the card\n";
    let path = file(&w, "m2-billing.md", second);
    assert_eq!(
        code(&mem(
            &w,
            &repo,
            &["plan", "m2-billing", "--set-file", &path]
        )),
        0
    );

    // Every task is ticked; the open boxes left are a fenced example and one
    // indented under the task that explains it, so the plan is finished.
    let finished = "# plan: m1-auth\n\n\
        - [x] t1 Sign-in\n\
        \x20     Spec: a task line reads\n\n\
        \x20         - [ ] t9 an example task\n\n\
        ```\n\
        - [ ] t8 another example\n\
        ```\n";
    let path = file(&w, "record.md", finished);
    assert_eq!(code(&mem(&w, &repo, &["plan", "--set-file", &path])), 0);

    let out = mem(&w, &repo, &["plan", "--from", "m2-billing"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&mem(&w, &repo, &["plan"])), second);
}

#[test]
fn context_names_the_roadmap_and_its_next_milestone() {
    let w = World::new("roadmap-context");
    let repo = w.repo("shop", None);
    let path = file(&w, "roadmap.md", ROADMAP);
    assert_eq!(code(&mem(&w, &repo, &["roadmap", "--set-file", &path])), 0);

    let out = mem(&w, &repo, &["context"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("roadmap: # roadmap: shop"), "{text}");
    assert!(
        text.contains("roadmap: - [ ] m1-auth Sign-in and sessions"),
        "{text}"
    );
    assert!(
        !text.contains("nothing recorded"),
        "a roadmap is something recorded: {text}"
    );

    // Ticking the first milestone moves the digest on to the second.
    assert_eq!(code(&mem(&w, &repo, &["roadmap", "--tick", "m1-auth"])), 0);
    let text = stdout(&mem(&w, &repo, &["context"]));
    assert!(text.contains("roadmap: - [ ] m2-billing Billing"), "{text}");
}

#[test]
fn replacing_the_plan_of_record_keeps_the_ticks_a_run_has_made() {
    // The orchestrator answers a worker mid-run by editing the file the run
    // started from -- unticked -- and storing it over mem's copy, which by
    // then carries the merges the run has recorded. The ticks stay.
    let w = World::new("plan-keeps-ticks");
    let repo = w.repo("shop", None);
    let plan = "# plan: m18\n\n\
        - [ ] t1 The store\n\
        \x20     Files: src/store.rs\n\
        - [ ] t2 The reader\n\
        \x20     Files: src/read.rs\n\
        - [ ] t3 The writer [after: t2]\n\
        \x20     Files: src/write.rs\n";
    let out = mem_stdin(&w, &repo, &["plan", "--stdin"], plan.as_bytes());
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(code(&mem(&w, &repo, &["plan", "--tick", "t1"])), 0);
    assert_eq!(code(&mem(&w, &repo, &["plan", "--tick", "t2"])), 0);

    // The edit widens t3's Files; t1 and t2 come back unticked.
    let edited = plan.replace("Files: src/write.rs", "Files: src/write.rs src/read.rs");
    let out = mem_stdin(&w, &repo, &["plan", "--stdin"], edited.as_bytes());
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("kept the tick on t1, t2"),
        "the write says which boxes it kept: {}",
        stderr(&out)
    );
    let now = stdout(&mem(&w, &repo, &["plan"]));
    assert!(now.contains("- [x] t1 The store"), "{now}");
    assert!(now.contains("- [x] t2 The reader"), "{now}");
    assert!(now.contains("- [ ] t3 The writer [after: t2]"), "{now}");
    assert!(
        now.contains("Files: src/write.rs src/read.rs"),
        "the edit landed: {now}"
    );

    // A new plan under the same task ids is a new document: nothing carries.
    let next = "# plan: m19\n\n\
        - [ ] t1 The index\n\
        \x20     Files: src/index.rs\n\
        - [ ] t2 The search\n\
        \x20     Files: src/search.rs\n";
    let out = mem_stdin(&w, &repo, &["plan", "--stdin"], next.as_bytes());
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(!stderr(&out).contains("kept the tick"), "{}", stderr(&out));
    assert_eq!(stdout(&mem(&w, &repo, &["plan"])), next);
}
