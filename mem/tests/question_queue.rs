//! The question queue (spec §7b, AC12).

mod common;

use std::time::{Duration, Instant};

use common::{World, code, stderr, stdout};

/// The queue tests need the notification and sync seams stubbed and a poll
/// interval short enough for a test.
fn ask_env(
    w: &World,
    cwd: &std::path::Path,
    args: &[&str],
    notify_log: Option<&std::path::Path>,
) -> std::process::Output {
    let mut cmd = mem_cmd(w, cwd, args);
    match notify_log {
        // A stub standing in for notify-send: it appends the arguments it was
        // called with to a file the test can read.
        Some(path) => {
            cmd.env("MEM_NOTIFY_CMD", path.display().to_string());
        }
        None => {
            cmd.env("MEM_NOTIFY_CMD", "true");
        }
    }
    cmd.output().expect("run mem")
}

fn mem_cmd(w: &World, cwd: &std::path::Path, args: &[&str]) -> std::process::Command {
    let dirs = w.dirs();
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_mem"));
    cmd.current_dir(cwd)
        .args(args)
        .env("XDG_DATA_HOME", dirs.data)
        .env("XDG_CACHE_HOME", dirs.cache)
        .env("XDG_STATE_HOME", dirs.state)
        .env("XDG_CONFIG_HOME", dirs.config)
        .env("MEM_SYNC_CMD", "true")
        .env("MEM_POLL_MS", "50")
        .env_remove("WORKFLOW_TASK")
        .env_remove("MEM_PROJECT")
        .env_remove("CARGO_TARGET_DIR");
    cmd
}

/// A stand-in for notify-send that records the arguments it was given.
/// A stand-in that records any notification mem tries to send. Since hub's
/// doorbell became the one bell, the record must stay empty.
fn notify_stub(w: &World) -> std::path::PathBuf {
    let script = w.dir.join("notify-stub.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {}\n",
            w.dir.join("notified.txt").display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    script
}

#[test]
fn ask_returns_immediately_and_rings_no_bell_of_its_own() {
    let w = World::new("q-ask");
    let repo = w.repo("thing", None);
    let log = notify_stub(&w);

    let started = Instant::now();
    let out = ask_env(
        &w,
        &repo,
        &["ask", "deploy on friday?", "--options", "yes,no", "--json"],
        Some(&log),
    );
    let elapsed = started.elapsed();
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        elapsed < Duration::from_secs(2),
        "ask must not block: {elapsed:?}"
    );

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let short = v["short_id"].as_str().unwrap().to_string();
    assert_eq!(v["options"], serde_json::json!(["yes", "no"]));

    // hub's doorbell is the one bell; mem ringing too was the double
    // notification, and mem's carried the question text besides.
    let notified = std::fs::read_to_string(w.dir.join("notified.txt")).unwrap_or_default();
    assert!(
        notified.is_empty(),
        "ask must not send its own notification: {notified:?}"
    );
    let _ = &short;

    // The question is listed and still pending.
    let out = ask_env(&w, &repo, &["questions", "--pending"], None);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).starts_with('?'), "{}", stdout(&out));
    assert!(stdout(&out).contains("deploy on friday?"));
}

#[test]
fn waiting_times_out_with_exit_four() {
    let w = World::new("q-timeout");
    let repo = w.repo("thing", None);
    let v: serde_json::Value = serde_json::from_slice(
        &ask_env(&w, &repo, &["ask", "still waiting?", "--json"], None).stdout,
    )
    .unwrap();
    let id = v["short_id"].as_str().unwrap().to_string();

    let started = Instant::now();
    let out = ask_env(
        &w,
        &repo,
        &["questions", "--wait", &id, "--timeout", "1s"],
        None,
    );
    assert_eq!(code(&out), 4, "{}", stderr(&out));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(
        stderr(&out).contains("mem handoff"),
        "the caller is told to park: {}",
        stderr(&out)
    );
}

#[test]
fn an_answer_arriving_during_a_wait_ends_it_with_the_text() {
    let w = World::new("q-answer");
    let repo = w.repo("thing", None);
    let v: serde_json::Value = serde_json::from_slice(
        &ask_env(&w, &repo, &["ask", "redis or postgres?", "--json"], None).stdout,
    )
    .unwrap();
    let id = v["short_id"].as_str().unwrap().to_string();

    let answerer = {
        let dirs = w.dirs();
        let repo = repo.clone();
        let id = id.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            std::process::Command::new(env!("CARGO_BIN_EXE_mem"))
                .current_dir(&repo)
                .args(["answer", &id, "redis, and pin the decision"])
                .env("XDG_DATA_HOME", dirs.data)
                .env("XDG_CACHE_HOME", dirs.cache)
                .env("XDG_STATE_HOME", dirs.state)
                .env("XDG_CONFIG_HOME", dirs.config)
                .env("MEM_SYNC_CMD", "true")
                .env_remove("MEM_PROJECT")
                .output()
                .expect("answer")
        })
    };

    let out = ask_env(
        &w,
        &repo,
        &["questions", "--wait", &id, "--timeout", "30s"],
        None,
    );
    let answered = answerer.join().unwrap();
    assert_eq!(
        answered.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&answered.stderr)
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).contains("redis, and pin the decision"),
        "{}",
        stdout(&out)
    );

    // Once answered it drops out of the pending list.
    let out = ask_env(&w, &repo, &["questions", "--pending"], None);
    assert_eq!(code(&out), 1, "no pending questions left");
}

#[test]
fn answering_an_unknown_or_wrong_kind_of_id_is_exit_one() {
    let w = World::new("q-unknown");
    let repo = w.repo("thing", None);
    assert_eq!(
        code(&ask_env(&w, &repo, &["answer", "ZZZZZZZZ", "text"], None)),
        1
    );
    assert_eq!(
        code(&ask_env(&w, &repo, &["answer", "not-an-id", "text"], None)),
        1
    );

    let v: serde_json::Value =
        serde_json::from_slice(&ask_env(&w, &repo, &["save", "a fact", "--json"], None).stdout)
            .unwrap();
    let out = ask_env(
        &w,
        &repo,
        &["answer", v["short_id"].as_str().unwrap(), "text"],
        None,
    );
    assert_eq!(code(&out), 1, "a fact is not a question");
    assert!(stderr(&out).contains("not a question"));

    // An answer needs something to say.
    let v: serde_json::Value =
        serde_json::from_slice(&ask_env(&w, &repo, &["ask", "well?", "--json"], None).stdout)
            .unwrap();
    assert_eq!(
        code(&ask_env(
            &w,
            &repo,
            &["answer", v["short_id"].as_str().unwrap()],
            None
        )),
        2
    );
    assert_eq!(
        code(&ask_env(
            &w,
            &repo,
            &["answer", v["short_id"].as_str().unwrap(), "--option", "yes"],
            None
        )),
        0
    );
}

#[test]
fn a_wait_on_an_unknown_id_or_an_over_long_timeout_is_rejected() {
    let w = World::new("q-args");
    let repo = w.repo("thing", None);
    assert_eq!(
        code(&ask_env(
            &w,
            &repo,
            &["questions", "--wait", "ZZZZZZZZ", "--timeout", "1s"],
            None
        )),
        1
    );
    assert_eq!(
        code(&ask_env(
            &w,
            &repo,
            &["questions", "--wait", "ZZZZZZZZ", "--timeout", "later"],
            None
        )),
        2
    );
    // Anything longer than the documented ceiling is clamped, not refused.
    assert_eq!(
        mem::questions::parse_timeout("4h").unwrap(),
        mem::questions::MAX_WAIT
    );
    assert_eq!(
        mem::questions::parse_timeout("30s").unwrap(),
        Duration::from_secs(30)
    );
    assert!(mem::questions::parse_timeout("5x").is_none());
}

#[test]
fn a_wait_watches_the_questions_own_project_not_the_working_directory() {
    let w = World::new("q-wait-elsewhere");
    let repo = w.repo("thing", None);
    let v: serde_json::Value = serde_json::from_slice(
        &ask_env(
            &w,
            &repo,
            &["ask", "which directory is watched?", "--json"],
            None,
        )
        .stdout,
    )
    .unwrap();
    let id = v["short_id"].as_str().unwrap().to_string();

    let answerer = {
        let dirs = w.dirs();
        let repo = repo.clone();
        let id = id.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            std::process::Command::new(env!("CARGO_BIN_EXE_mem"))
                .current_dir(&repo)
                .args(["answer", &id, "the question's own items dir"])
                .env("XDG_DATA_HOME", dirs.data)
                .env("XDG_CACHE_HOME", dirs.cache)
                .env("XDG_STATE_HOME", dirs.state)
                .env("XDG_CONFIG_HOME", dirs.config)
                .env("MEM_SYNC_CMD", "true")
                .env_remove("MEM_PROJECT")
                .output()
                .expect("answer")
        })
    };

    // The wait runs from a directory belonging to no project at all, which is
    // the ordinary case — an orchestrator waits from wherever it happens to be.
    // Polling this checkout's items dir would mean never seeing the answer land.
    let loose = w.plain_dir("loose");
    let out = ask_env(
        &w,
        &loose,
        &["questions", "--wait", &id, "--timeout", "30s"],
        None,
    );
    let answered = answerer.join().unwrap();
    assert_eq!(
        answered.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&answered.stderr)
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).contains("the question's own items dir"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn a_question_and_its_answer_live_in_the_same_project_tree() {
    let w = World::new("q-colocated");
    let repo = w.repo("thing", None);
    let v: serde_json::Value = serde_json::from_slice(
        &ask_env(&w, &repo, &["ask", "where do answers go?", "--json"], None).stdout,
    )
    .unwrap();
    let short = v["short_id"].as_str().unwrap().to_string();
    // Answering from outside the checkout still files it with the question.
    let loose = w.plain_dir("loose");
    assert_eq!(
        code(&ask_env(
            &w,
            &loose,
            &["answer", &short, "next to it"],
            None
        )),
        0
    );

    let id = mem::project::Registry::load(&w.store()).projects[0]
        .id
        .clone();
    let dir = w.store().project_items(&id);
    let kinds: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| mem::store::read_item(&e.unwrap().path()).ok())
        .map(|i| i.meta.kind.to_string())
        .collect();
    assert!(
        kinds.contains(&"question".to_string()) && kinds.contains(&"answer".to_string()),
        "{kinds:?}"
    );
}

/// The listing JSON must say which questions are answered: the text mode has
/// its ✓/? column, and a robot reading `--json` deserves the same fact.
#[test]
fn the_listing_json_carries_the_answered_flag() {
    let w = World::new("q-answered-json");
    let repo = w.repo("thing", None);

    let asked: serde_json::Value = serde_json::from_slice(
        &ask_env(&w, &repo, &["ask", "is this answered?", "--json"], None).stdout,
    )
    .unwrap();
    let id = asked["short_id"].as_str().unwrap().to_string();
    ask_env(&w, &repo, &["ask", "and this one?", "--json"], None);

    let out = ask_env(&w, &repo, &["answer", &id, "yes it is"], None);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let out = ask_env(&w, &repo, &["questions", "--json"], None);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let rows = v["questions"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{v}");
    for row in rows {
        let want = row["short_id"] == serde_json::json!(id);
        assert_eq!(
            row["answered"],
            serde_json::json!(want),
            "answered must mirror the text mode's mark: {row}"
        );
    }

    // The pending queue never contains an answered question, so there the
    // field is always false — but it is still present, so one parser serves
    // both listings.
    let out = ask_env(&w, &repo, &["questions", "--pending", "--json"], None);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let rows = v["questions"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{v}");
    assert_eq!(rows[0]["answered"], serde_json::json!(false), "{v}");
}

/// A worker asks from its task worktree, and the question is the
/// orchestrator's: tagged with the task, kept off the hub's listing, and
/// carried with its answer in the JSON a run reads.
#[test]
fn a_workers_question_is_the_orchestrators_and_names_its_task() {
    let w = World::new("q-worker");
    let repo = w.repo("thing", None);
    common::run_git(
        &repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "base",
        ],
    );
    // Where `workflow run` puts a task's worktree, under the mem state root.
    let wt = w.dirs().workflow_worktrees().join("thing/cart-v2/t3");
    std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
    common::run_git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            wt.to_str().unwrap(),
            "-b",
            "cart-v2/t3",
        ],
    );

    let out = ask_env(
        &w,
        &wt,
        &["ask", "may I widen Files by src/main.rs?", "--json"],
        None,
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["audience"], "orchestrator", "{v}");
    let id = v["short_id"].as_str().unwrap().to_string();

    // The hub's view: nothing for a person here.
    let out = ask_env(
        &w,
        &repo,
        &[
            "questions",
            "--pending",
            "--all-projects",
            "--for",
            "human",
            "--json",
        ],
        None,
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["questions"], serde_json::json!([]), "{v}");

    // The orchestrator's view: the question, the task that asked it, the body.
    let out = ask_env(
        &w,
        &repo,
        &["questions", "--pending", "--for", "orchestrator", "--json"],
        None,
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let q = &v["questions"][0];
    assert_eq!(q["task"], "cart-v2/t3", "{q}");
    assert_eq!(q["audience"], "orchestrator");
    assert_eq!(q["body"], "may I widen Files by src/main.rs?");
    assert_eq!(q["answered"], false);
    assert!(q["answer"].is_null());
    let text = stdout(&ask_env(&w, &repo, &["questions", "--pending"], None));
    assert!(text.contains("[cart-v2/t3] may I widen"), "{text}");

    // Answered, the same listing carries the answer for the next attempt.
    let out = ask_env(&w, &repo, &["answer", &id, "yes, widen"], None);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = ask_env(
        &w,
        &repo,
        &["questions", "--for", "orchestrator", "--json"],
        None,
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["questions"][0]["answer"], "yes, widen", "{v}");
    assert_eq!(v["questions"][0]["answered"], true);

    // `--for human` from a worktree is a worker escalating on purpose.
    let out = ask_env(
        &w,
        &wt,
        &["ask", "delete the prod table?", "--for", "human", "--json"],
        None,
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["audience"].is_null(), "{v}");
    let out = ask_env(
        &w,
        &repo,
        &["questions", "--pending", "--for", "human", "--json"],
        None,
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["questions"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["questions"][0]["title"], "delete the prod table?");
}

/// The environment says so too, for a backend whose worker does not stand in
/// a worktree under the state root.
#[test]
fn workflow_task_in_the_environment_addresses_the_orchestrator() {
    let w = World::new("q-env");
    let repo = w.repo("thing", None);
    let out = common::mem_env(
        &w,
        &repo,
        &["ask", "which base?", "--json"],
        &[("MEM_SYNC_CMD", "true"), ("WORKFLOW_TASK", "cart-v2/t1")],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["audience"], "orchestrator", "{v}");
    let out = ask_env(&w, &repo, &["questions", "--pending", "--json"], None);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["questions"][0]["task"], "cart-v2/t1", "{v}");
    // And a plain checkout with nothing set is a person's question.
    let out = ask_env(&w, &repo, &["ask", "ship it?", "--json"], None);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["audience"].is_null(), "{v}");
}

/// A task worktree's own path outranks `WORKFLOW_TASK`: the environment
/// cannot spoof another task's question just because it names one.
#[test]
fn the_worktree_path_outranks_workflow_task_in_the_environment() {
    let w = World::new("q-worktree-wins");
    let repo = w.repo("thing", None);
    common::run_git(
        &repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "base",
        ],
    );
    let wt = w.dirs().workflow_worktrees().join("thing/cart-v2/t7");
    std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
    common::run_git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            wt.to_str().unwrap(),
            "-b",
            "cart-v2/t7",
        ],
    );

    let out = common::mem_env(
        &w,
        &wt,
        &["ask", "which base?", "--json"],
        &[("MEM_SYNC_CMD", "true"), ("WORKFLOW_TASK", "other/t1")],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["audience"], "orchestrator", "{v}");

    let out = ask_env(&w, &repo, &["questions", "--pending", "--json"], None);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["questions"][0]["task"], "cart-v2/t7", "{v}");
}

/// The `_integration` worktree speaks for no single task, so it defers to
/// `WORKFLOW_TASK` like a process outside the worktree root would.
#[test]
fn an_integration_worktree_defers_to_workflow_task_in_the_environment() {
    let w = World::new("q-integration");
    let repo = w.repo("thing", None);
    common::run_git(
        &repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "base",
        ],
    );
    let wt = w
        .dirs()
        .workflow_worktrees()
        .join("thing/cart-v2/_integration");
    std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
    common::run_git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            wt.to_str().unwrap(),
            "-b",
            "cart-v2/_integration",
        ],
    );

    let out = common::mem_env(
        &w,
        &wt,
        &["ask", "which base?", "--json"],
        &[("MEM_SYNC_CMD", "true"), ("WORKFLOW_TASK", "cart-v2/t9")],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["audience"], "orchestrator", "{v}");

    let out = ask_env(&w, &repo, &["questions", "--pending", "--json"], None);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["questions"][0]["task"], "cart-v2/t9", "{v}");
}

/// A recommendation is one field on the question item: ask records it, and
/// both listings carry it with the options, so the hub can show them under
/// the question without reading the file itself.
#[test]
fn a_recommendation_rides_with_the_options_in_both_listings() {
    let w = World::new("q-recommend");
    let repo = w.repo("thing", None);

    let out = ask_env(
        &w,
        &repo,
        &[
            "ask",
            "ship on friday?",
            "--options",
            "yes,no",
            "--recommend",
            "yes, the gate is green",
            "--json",
        ],
        None,
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let asked: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(asked["recommend"], "yes, the gate is green", "{asked}");
    let id = asked["short_id"].as_str().unwrap().to_string();

    let out = ask_env(&w, &repo, &["ask", "and this one?", "--json"], None);
    let bare: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(bare["recommend"].is_null(), "{bare}");

    let check = |w: &World| {
        let out = ask_env(w, &repo, &["questions", "--json"], None);
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let rows = v["questions"].as_array().unwrap();
        assert_eq!(rows.len(), 2, "{v}");
        for row in rows {
            if row["short_id"] == serde_json::json!(id) {
                assert_eq!(row["options"], serde_json::json!(["yes", "no"]), "{row}");
                assert_eq!(row["recommend"], "yes, the gate is green", "{row}");
            } else {
                assert_eq!(row["options"], serde_json::json!([]), "{row}");
                assert!(row["recommend"].is_null(), "{row}");
            }
        }
    };
    check(&w);

    let out = ask_env(&w, &repo, &["questions"], None);
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    let at = lines
        .iter()
        .position(|l| l.contains(&format!("#{id}")))
        .unwrap();
    assert_eq!(
        lines[at + 1],
        "    recommended: yes, the gate is green",
        "{text}"
    );

    // The field lives in the item file, so a rebuilt index loses nothing.
    let out = ask_env(&w, &repo, &["reindex"], None);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    check(&w);
}

/// mem run by a session: `MEM_SESSION_ID` outranks whatever id the test
/// process itself inherited.
fn as_session(
    w: &World,
    cwd: &std::path::Path,
    session: &str,
    args: &[&str],
) -> std::process::Output {
    let mut cmd = mem_cmd(w, cwd, args);
    cmd.env("MEM_NOTIFY_CMD", "true")
        .env("MEM_SESSION_ID", session);
    cmd.output().expect("run mem")
}

fn asked_short(out: &std::process::Output) -> String {
    assert_eq!(code(out), 0, "{}", stderr(out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["short_id"].as_str().unwrap().to_string()
}

/// The session that asked is kept in mem's machine-local session file, so a
/// session can list its own questions with their answers, across projects.
#[test]
fn a_session_lists_the_questions_it_asked() {
    let w = World::new("q-asked-by");
    let repo = w.repo("thing", None);
    let other = w.repo("other", None);

    let first = asked_short(&as_session(
        &w,
        &repo,
        "s1",
        &["ask", "first from s1?", "--json"],
    ));
    let second = asked_short(&as_session(
        &w,
        &other,
        "s1",
        &["ask", "second from s1?", "--json"],
    ));
    let theirs = asked_short(&as_session(&w, &repo, "s2", &["ask", "from s2?", "--json"]));
    // A question for the orchestrator reaches the session another way.
    let routed = asked_short(&as_session(
        &w,
        &repo,
        "s1",
        &["ask", "for the run?", "--for", "orchestrator", "--json"],
    ));

    let out = ask_env(&w, &repo, &["answer", &first, "yes, go"], None);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let out = ask_env(
        &w,
        &repo,
        &["questions", "--asked-by", "s1", "--json"],
        None,
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let rows = v["questions"].as_array().unwrap();
    let shorts: Vec<&str> = rows
        .iter()
        .map(|r| r["short_id"].as_str().unwrap())
        .collect();
    assert_eq!(shorts, [first.as_str(), second.as_str()], "{v}");
    assert!(!shorts.contains(&theirs.as_str()) && !shorts.contains(&routed.as_str()));
    assert_eq!(rows[0]["title"], "first from s1?", "{v}");
    assert_eq!(rows[0]["answered"], true, "{v}");
    assert_eq!(rows[0]["answer"], "yes, go", "{v}");
    assert_eq!(rows[1]["answered"], false, "{v}");
    assert!(rows[1]["answer"].is_null(), "{v}");

    let out = ask_env(
        &w,
        &repo,
        &["questions", "--asked-by", "s1", "--pending", "--json"],
        None,
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["questions"][0]["short_id"], second.as_str(), "{v}");
    assert_eq!(v["questions"].as_array().unwrap().len(), 1, "{v}");

    let out = ask_env(
        &w,
        &repo,
        &["questions", "--asked-by", "s2", "--json"],
        None,
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["questions"].as_array().unwrap().len(), 1, "{v}");

    let out = ask_env(
        &w,
        &repo,
        &["questions", "--asked-by", "nobody", "--json"],
        None,
    );
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["questions"], serde_json::json!([]), "{v}");

    let out = ask_env(
        &w,
        &repo,
        &["questions", "--asked-by", "s1", "--wait", &second],
        None,
    );
    assert_eq!(code(&out), 2, "{}", stderr(&out));
}

/// A session file written before mem kept the asked list still reads, and
/// the first question adds the list to it.
#[test]
fn a_session_file_from_before_the_asked_list_still_reads() {
    let w = World::new("q-asked-old");
    let repo = w.repo("thing", None);
    let sessions = w.dirs().sessions_dir();
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join("s3"),
        r#"{"writes":4,"batches":2,"nudged":true,"last":"2026-10-01T00:00:00Z"}"#,
    )
    .unwrap();

    let short = asked_short(&as_session(
        &w,
        &repo,
        "s3",
        &["ask", "still read?", "--json"],
    ));
    let out = ask_env(
        &w,
        &repo,
        &["questions", "--asked-by", "s3", "--json"],
        None,
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["questions"][0]["short_id"], short.as_str(), "{v}");

    let kept: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sessions.join("s3")).unwrap()).unwrap();
    assert_eq!(kept["writes"], 5, "{kept}");
    assert_eq!(kept["nudged"], true, "{kept}");
}
