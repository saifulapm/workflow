//! The question queue.

mod common;

use std::time::{Duration, Instant};

use common::{World, code, stderr, stdout};

/// The queue tests need the sync seam stubbed and a poll interval short
/// enough for a test.
fn ask_env(w: &World, cwd: &std::path::Path, args: &[&str]) -> std::process::Output {
    mem_cmd(w, cwd, args).output().expect("run mem")
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
        .env_remove("MEM_PROJECT")
        .env_remove("CARGO_TARGET_DIR");
    cmd
}

#[test]
fn ask_returns_immediately() {
    let w = World::new("q-ask");
    let repo = w.repo("thing", None);

    let started = Instant::now();
    let out = ask_env(
        &w,
        &repo,
        &["ask", "deploy on friday?", "--options", "yes,no", "--json"],
    );
    let elapsed = started.elapsed();
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        elapsed < Duration::from_secs(2),
        "ask must not block: {elapsed:?}"
    );

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["options"], serde_json::json!(["yes", "no"]));

    // The question is listed and still pending.
    let out = ask_env(&w, &repo, &["questions", "--pending"]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).starts_with('?'), "{}", stdout(&out));
    assert!(stdout(&out).contains("deploy on friday?"));
}

#[test]
fn waiting_times_out_with_exit_four() {
    let w = World::new("q-timeout");
    let repo = w.repo("thing", None);
    let v: serde_json::Value =
        serde_json::from_slice(&ask_env(&w, &repo, &["ask", "still waiting?", "--json"]).stdout)
            .unwrap();
    let id = v["short_id"].as_str().unwrap().to_string();

    let started = Instant::now();
    let out = ask_env(&w, &repo, &["questions", "--wait", &id, "--timeout", "1s"]);
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
        &ask_env(&w, &repo, &["ask", "redis or postgres?", "--json"]).stdout,
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

    let out = ask_env(&w, &repo, &["questions", "--wait", &id, "--timeout", "30s"]);
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
    let out = ask_env(&w, &repo, &["questions", "--pending"]);
    assert_eq!(code(&out), 1, "no pending questions left");
}

#[test]
fn answering_an_unknown_or_wrong_kind_of_id_is_exit_one() {
    let w = World::new("q-unknown");
    let repo = w.repo("thing", None);
    assert_eq!(
        code(&ask_env(&w, &repo, &["answer", "ZZZZZZZZ", "text"])),
        1
    );
    assert_eq!(
        code(&ask_env(&w, &repo, &["answer", "not-an-id", "text"])),
        1
    );

    let v: serde_json::Value =
        serde_json::from_slice(&ask_env(&w, &repo, &["save", "a fact", "--json"]).stdout).unwrap();
    let out = ask_env(
        &w,
        &repo,
        &["answer", v["short_id"].as_str().unwrap(), "text"],
    );
    assert_eq!(code(&out), 1, "a fact is not a question");
    assert!(stderr(&out).contains("not a question"));

    // An answer needs something to say.
    let v: serde_json::Value =
        serde_json::from_slice(&ask_env(&w, &repo, &["ask", "well?", "--json"]).stdout).unwrap();
    assert_eq!(
        code(&ask_env(
            &w,
            &repo,
            &["answer", v["short_id"].as_str().unwrap()]
        )),
        2
    );
    assert_eq!(
        code(&ask_env(
            &w,
            &repo,
            &["answer", v["short_id"].as_str().unwrap(), "--option", "yes"]
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
            &["questions", "--wait", "ZZZZZZZZ", "--timeout", "1s"]
        )),
        1
    );
    assert_eq!(
        code(&ask_env(
            &w,
            &repo,
            &["questions", "--wait", "ZZZZZZZZ", "--timeout", "later"]
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
        &ask_env(&w, &repo, &["ask", "which directory is watched?", "--json"]).stdout,
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
        &ask_env(&w, &repo, &["ask", "where do answers go?", "--json"]).stdout,
    )
    .unwrap();
    let short = v["short_id"].as_str().unwrap().to_string();
    // Answering from outside the checkout still files it with the question.
    let loose = w.plain_dir("loose");
    assert_eq!(
        code(&ask_env(&w, &loose, &["answer", &short, "next to it"])),
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

    let asked: serde_json::Value =
        serde_json::from_slice(&ask_env(&w, &repo, &["ask", "is this answered?", "--json"]).stdout)
            .unwrap();
    let id = asked["short_id"].as_str().unwrap().to_string();
    ask_env(&w, &repo, &["ask", "and this one?", "--json"]);

    let out = ask_env(&w, &repo, &["answer", &id, "yes it is"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let out = ask_env(&w, &repo, &["questions", "--json"]);
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
    let out = ask_env(&w, &repo, &["questions", "--pending", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let rows = v["questions"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{v}");
    assert_eq!(rows[0]["answered"], serde_json::json!(false), "{v}");
}

/// An orchestrator's question carries its amx session, so the hub can
/// resume that very session once its questions are answered.
#[test]
fn a_question_records_the_amx_session_that_asked_it() {
    let w = World::new("q-session");
    let repo = w.repo("thing", None);
    let asked = mem_cmd(&w, &repo, &["ask", "from a run?"])
        .env("AMX_ID", "thing-m1-auth")
        .output()
        .unwrap();
    assert_eq!(code(&asked), 0, "{}", stderr(&asked));
    let typed = mem_cmd(&w, &repo, &["ask", "typed at a prompt?"])
        .env_remove("AMX_ID")
        .output()
        .unwrap();
    assert_eq!(code(&typed), 0, "{}", stderr(&typed));

    let out = ask_env(&w, &repo, &["questions", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let session = |body: &str| {
        v["questions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|q| q["body"] == body)
            .unwrap_or_else(|| panic!("{body}: {v}"))["session"]
            .clone()
    };
    assert_eq!(session("from a run?"), "thing-m1-auth", "{v}");
    assert!(session("typed at a prompt?").is_null(), "{v}");
}

/// A worker asks from its task worktree, and the question is the
/// orchestrator's: tagged with the task, kept off the hub's listing, and
/// carried with its answer in the JSON a run reads.
#[test]
fn a_question_is_a_persons_unless_it_says_otherwise() {
    let w = World::new("q-audience");
    let repo = w.repo("thing", None);
    assert_eq!(code(&ask_env(&w, &repo, &["log", "first write"])), 0);

    // Nothing about where it is asked from or what the environment holds
    // makes a question the orchestrator's: a directory where the retired
    // engine kept its worktrees, and the variable it exported, do not.
    let old_worktree = w.dirs().state.join("workflow/worktrees/thing/cart-v2/t3");
    std::fs::create_dir_all(&old_worktree).unwrap();
    for cwd in [&repo, &old_worktree] {
        let out = mem_cmd(
            &w,
            cwd,
            &["ask", "ship it?", "--project", "thing", "--json"],
        )
        .env("WORKFLOW_TASK", "cart-v2/t1")
        .output()
        .expect("run mem");
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(v["audience"].is_null(), "{v}");
    }
    let out = ask_env(&w, &repo, &["questions", "--for", "human", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["questions"].as_array().unwrap().len(), 2, "{v}");
    for q in v["questions"].as_array().unwrap() {
        assert!(q["task"].is_null(), "{q}");
    }
}

#[test]
fn an_orchestrators_question_is_listed_for_the_orchestrator_alone() {
    let w = World::new("q-orchestrator");
    let repo = w.repo("thing", None);
    let out = ask_env(
        &w,
        &repo,
        &[
            "ask",
            "may I widen Files by src/main.rs?",
            "--for",
            "orchestrator",
            "--json",
        ],
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
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["questions"], serde_json::json!([]), "{v}");

    // The orchestrator's view: the question and its body.
    let out = ask_env(
        &w,
        &repo,
        &["questions", "--pending", "--for", "orchestrator", "--json"],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let q = &v["questions"][0];
    assert!(q["task"].is_null(), "{q}");
    assert_eq!(q["audience"], "orchestrator");
    assert_eq!(q["body"], "may I widen Files by src/main.rs?");
    assert_eq!(q["answered"], false);
    assert!(q["answer"].is_null());
    let text = stdout(&ask_env(&w, &repo, &["questions", "--pending"]));
    assert!(text.contains("[orchestrator] may I widen"), "{text}");

    // Answered, the same listing carries the answer.
    let out = ask_env(&w, &repo, &["answer", &id, "yes, widen"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = ask_env(&w, &repo, &["questions", "--for", "orchestrator", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["questions"][0]["answer"], "yes, widen", "{v}");
    assert_eq!(v["questions"][0]["answered"], true);
}

/// A question only Saiful may answer carries its own audience, and still
/// counts as a person's wherever a person's questions are read.
#[test]
fn a_saiful_question_is_saifuls_and_still_a_persons() {
    let w = World::new("q-saiful");
    let repo = w.repo("thing", None);
    let out = ask_env(
        &w,
        &repo,
        &["ask", "approve the roadmap?", "--for", "saiful", "--json"],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["audience"], "saiful", "{v}");
    let out = ask_env(&w, &repo, &["ask", "which colour?", "--for", "human"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = ask_env(&w, &repo, &["ask", "widen?", "--for", "orchestrator"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let bodies = |audience: &str| -> Vec<String> {
        let out = ask_env(
            &w,
            &repo,
            &["questions", "--pending", "--for", audience, "--json"],
        );
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let mut bodies: Vec<String> = v["questions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|q| q["body"].as_str().unwrap().to_string())
            .collect();
        bodies.sort();
        bodies
    };
    assert_eq!(bodies("saiful"), ["approve the roadmap?"]);
    assert_eq!(bodies("human"), ["approve the roadmap?", "which colour?"]);
    assert_eq!(bodies("orchestrator"), ["widen?"]);

    let out = ask_env(&w, &repo, &["questions", "--for", "human", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let colour = v["questions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["body"] == "which colour?")
        .unwrap();
    assert!(colour["audience"].is_null(), "{colour}");

    let digest = stdout(&ask_env(&w, &repo, &["context"]));
    assert!(digest.contains("questions: 2 for you"), "{digest}");
}

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
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let asked: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(asked["recommend"], "yes, the gate is green", "{asked}");
    let id = asked["short_id"].as_str().unwrap().to_string();

    let out = ask_env(&w, &repo, &["ask", "and this one?", "--json"]);
    let bare: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(bare["recommend"].is_null(), "{bare}");

    let check = |w: &World| {
        let out = ask_env(w, &repo, &["questions", "--json"]);
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

    let out = ask_env(&w, &repo, &["questions"]);
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
    let out = ask_env(&w, &repo, &["reindex"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    check(&w);
}

/// Dulali answers first and Saiful replaces it later: the listing shows the
/// newer answer, who gave it, and the replaced answer stays on record.
#[test]
fn a_later_answer_that_supersedes_the_first_is_the_one_listed() {
    let w = World::new("q-supersede");
    let repo = w.repo("thing", None);
    let asked: serde_json::Value = serde_json::from_slice(
        &ask_env(
            &w,
            &repo,
            &["ask", "nag at eight?", "--options", "eight,nine", "--json"],
        )
        .stdout,
    )
    .unwrap();
    let q = asked["short_id"].as_str().unwrap().to_string();

    let out = ask_env(
        &w,
        &repo,
        &["answer", &q, "eight", "--by", "dulali", "--json"],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let first: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let first_id = first["id"].as_str().unwrap().to_string();

    let listed = |w: &World| -> serde_json::Value {
        let out = ask_env(w, &repo, &["questions", "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        v["questions"][0].clone()
    };
    let row = listed(&w);
    assert_eq!(row["answer"], "eight", "{row}");
    assert_eq!(row["answer_by"], "dulali", "{row}");
    assert_eq!(row["answer_id"], first_id.as_str(), "{row}");

    let out = ask_env(
        &w,
        &repo,
        &[
            "answer",
            &q,
            "nine",
            "--by",
            "saiful",
            "--supersedes",
            &first_id,
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let row = listed(&w);
    assert_eq!(row["answer"], "nine", "{row}");
    assert_eq!(row["answer_by"], "saiful", "{row}");

    let out = ask_env(&w, &repo, &["show", &first_id, "--json"]);
    let shown = stdout(&out);
    assert!(shown.contains("superseded_by"), "{shown}");
    assert!(!shown.contains("\"superseded_by\":null"), "{shown}");
}

/// An answer replaces only another answer to the same question.
#[test]
fn an_answer_cannot_supersede_an_item_that_is_not_its_questions_answer() {
    let w = World::new("q-supersede-wrong");
    let repo = w.repo("thing", None);
    let ask = |text: &str| -> String {
        let v: serde_json::Value =
            serde_json::from_slice(&ask_env(&w, &repo, &["ask", text, "--json"]).stdout).unwrap();
        v["short_id"].as_str().unwrap().to_string()
    };
    let a = ask("first?");
    let b = ask("second?");
    let out = ask_env(&w, &repo, &["answer", &a, "yes", "--json"]);
    let answer_a: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let answer_a = answer_a["id"].as_str().unwrap();

    let out = ask_env(&w, &repo, &["answer", &b, "no", "--supersedes", answer_a]);
    assert_ne!(code(&out), 0, "answering #{b} replaced #{a}'s answer");
    let out = ask_env(&w, &repo, &["answer", &b, "no", "--supersedes", &a]);
    assert_ne!(code(&out), 0, "an answer replaced a question");
}

#[test]
fn a_question_with_no_text_is_refused() {
    let w = World::new("q-empty");
    let repo = w.repo("thing", None);
    for text in ["", "   "] {
        let out = ask_env(&w, &repo, &["ask", text, "--options", "yes,no"]);
        assert_eq!(code(&out), 2, "{:?}: {}", text, stderr(&out));
        assert!(stderr(&out).contains("question"), "{}", stderr(&out));
    }
    let out = ask_env(&w, &repo, &["questions", "--pending"]);
    assert_eq!(code(&out), 1, "nothing was asked: {}", stdout(&out));
}

/// Another mem holding the index's write lock (a sync round, the local
/// hub's sweep) makes a reindex step aside. A question asked a moment ago is
/// then missing from the index for a little while, and a wait on it must
/// look again instead of saying there is no such question.
#[test]
fn waiting_on_a_question_the_index_has_not_caught_up_with_finds_it() {
    let w = World::new("q-wait-busy");
    let repo = w.repo("thing", None);
    // Make sure the index exists before it is locked.
    let out = ask_env(&w, &repo, &["ask", "the first?", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    ask_env(&w, &repo, &["questions", "--pending"]);

    let db = w.index_path();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let out = ask_env(&w, &repo, &["ask", "locked out?", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let asked: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let id = asked["short_id"]
        .as_str()
        .or(asked["id"].as_str())
        .unwrap()
        .to_string();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(600));
        conn.execute_batch("COMMIT").unwrap();
    });
    let out = ask_env(&w, &repo, &["questions", "--wait", &id, "--timeout", "2s"]);
    releaser.join().unwrap();
    assert!(
        !stderr(&out).contains("no question"),
        "the wait gave up on a question it had not indexed yet: {}",
        stderr(&out)
    );
    // Found, and unanswered: the wait times out.
    assert_eq!(code(&out), 4, "{}", stderr(&out));
}
