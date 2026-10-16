//! A parked session resumes on its own machine once a person has answered
//! every question it asked: the runner's local hub runs `amx resume` with
//! the answers as its first message, or `workflow go` when amx has lost the
//! session. A fixture `mem`, `amx` and `systemd-run` stand in.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{Hub, TempDir, fixture_bin, fixture_mem, wait_for};
use serde_json::{Value, json};

const PEER_TOKEN: &str = "peer-token-for-tests-0123456789abcdef";
const SESSION: &str = "walk01-m1";

fn bins(dir: &TempDir) -> PathBuf {
    let bin = dir.join("bin");
    fixture_mem(
        &bin,
        &format!(
            "case \"$1 $2\" in\n\
             'projects --json') cat '{projects}' ;;\n\
             'questions --json') cat '{questions}' ;;\n\
             esac",
            projects = dir.join("projects.json").display(),
            questions = dir.join("questions.json").display(),
        ),
    );
    fixture_bin(
        &bin,
        "amx",
        &format!(
            "[ -e '{d}/amx.fail' ] && exit 1\n[ \"$1 $2\" = 'ls --json' ] && cat '{d}/amx.json'",
            d = dir.path().display()
        ),
    );
    fixture_bin(
        &bin,
        "systemd-run",
        &format!(
            "printf '%s\\n<END>\\n' \"$*\" >> '{}'",
            dir.join("ran").display()
        ),
    );
    bin
}

fn ran(dir: &TempDir) -> Vec<String> {
    std::fs::read_to_string(dir.join("ran"))
        .unwrap_or_default()
        .split("\n<END>\n")
        .filter(|call| !call.is_empty())
        .map(str::to_string)
        .collect()
}

fn set_runner(dir: &TempDir, machine: &str, id: &str) {
    std::fs::write(
        dir.join("projects.json"),
        json!({"projects": [{"name": "walk01", "checkouts": [],
            "runner": {"machine": machine, "id": id, "milestone": null, "since": "2026-10-16T00:00:00Z"}}]})
        .to_string(),
    )
    .unwrap();
}

fn set_amx(dir: &TempDir, state: Option<&str>) {
    let rows = match state {
        Some(state) => {
            json!([{"id": SESSION, "state": state, "ended": if state == "running" { 0 } else { 9 }, "created": 5}])
        }
        None => json!([]),
    };
    std::fs::write(dir.join("amx.json"), rows.to_string()).unwrap();
}

/// A question the session asked, answered when `answer` is given.
fn question(id: &str, short: &str, body: &str, answer: Option<&str>) -> Value {
    json!({"id": id, "short_id": short, "body": body, "session": SESSION,
           "audience": "human", "answered": answer.is_some(), "answer": answer,
           "answer_by": answer.map(|_| "slack")})
}

fn set_questions(dir: &TempDir, rows: &[Value]) {
    std::fs::write(
        dir.join("questions.json"),
        json!({ "questions": rows }).to_string(),
    )
    .unwrap();
}

fn spawn(dir: &TempDir, bin: &Path) -> Hub {
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let socket = dir.join("local.sock");
    Hub::spawn_env(
        &home,
        &[bin],
        &["local", "--port", "0"],
        &[
            ("HUB_PEER_TOKEN", PEER_TOKEN),
            ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
            ("WORKFLOW_MACHINE", "mini"),
            ("HUB_RESUME_CHECK_MS", "100"),
        ],
    )
}

/// Started with a parked session whose questions are open.
fn parked(dir: &TempDir) -> Hub {
    let bin = bins(dir);
    set_runner(dir, "mini", SESSION);
    set_amx(dir, Some("stopped"));
    set_questions(
        dir,
        &[
            question("01Q2", "Q2", "Which colour?", None),
            question("01Q1", "Q1", "Ship it?", None),
        ],
    );
    let hub = spawn(dir, &bin);
    // A few rounds pass with nothing to do.
    std::thread::sleep(Duration::from_millis(500));
    hub
}

#[test]
fn the_last_answer_resumes_the_session_with_every_answer_once() {
    let dir = TempDir::new("parked-last");
    let _hub = parked(&dir);
    set_questions(
        &dir,
        &[
            question("01Q2", "Q2", "Which colour?", None),
            question("01Q1", "Q1", "Ship it?", Some("yes")),
        ],
    );
    std::thread::sleep(Duration::from_millis(600));
    assert!(ran(&dir).is_empty(), "one answer of two: {:?}", ran(&dir));

    set_questions(
        &dir,
        &[
            question("01Q2", "Q2", "Which colour?", Some("teal")),
            question("01Q1", "Q1", "Ship it?", Some("yes")),
        ],
    );
    let at = Instant::now();
    wait_for("the resume", Duration::from_secs(5), || {
        !ran(&dir).is_empty()
    });
    assert!(at.elapsed() < Duration::from_secs(5));
    let line = &ran(&dir)[0];
    assert!(
        line.starts_with("--user --scope --quiet amx resume walk01-m1 "),
        "{line}"
    );
    // Oldest first, each with its answer.
    let q1 = line.find("#Q1 Ship it?\nAnswer: yes");
    let q2 = line.find("#Q2 Which colour?\nAnswer: teal");
    assert!(q1.is_some() && q2.is_some() && q1 < q2, "{line}");

    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(ran(&dir).len(), 1, "{:?}", ran(&dir));
}

#[test]
fn a_session_amx_lost_starts_the_project_instead() {
    let dir = TempDir::new("parked-lost");
    let _hub = parked(&dir);
    set_amx(&dir, None);
    set_questions(
        &dir,
        &[
            question("01Q2", "Q2", "Which colour?", Some("teal")),
            question("01Q1", "Q1", "Ship it?", Some("yes")),
        ],
    );
    wait_for("the start", Duration::from_secs(5), || {
        !ran(&dir).is_empty()
    });
    assert_eq!(ran(&dir), ["--user --scope --quiet workflow go walk01"]);
}

#[test]
fn a_live_session_reads_its_answers_itself() {
    let dir = TempDir::new("parked-live");
    let _hub = parked(&dir);
    set_amx(&dir, Some("running"));
    set_questions(
        &dir,
        &[
            question("01Q2", "Q2", "Which colour?", Some("teal")),
            question("01Q1", "Q1", "Ship it?", Some("yes")),
        ],
    );
    std::thread::sleep(Duration::from_millis(800));
    assert!(ran(&dir).is_empty(), "{:?}", ran(&dir));
}

/// Answers given before the hub started belong to an earlier park, which
/// was resumed then: the first round only takes note of them.
#[test]
fn answers_from_before_the_hub_started_resume_nothing() {
    let dir = TempDir::new("parked-seed");
    let bin = bins(&dir);
    set_runner(&dir, "mini", SESSION);
    set_amx(&dir, Some("stopped"));
    set_questions(&dir, &[question("01Q1", "Q1", "Ship it?", Some("yes"))]);
    let _hub = spawn(&dir, &bin);
    std::thread::sleep(Duration::from_millis(800));
    assert!(ran(&dir).is_empty(), "{:?}", ran(&dir));

    // A new park after that resumes with the new answer alone.
    set_questions(
        &dir,
        &[
            question("01Q3", "Q3", "And docs?", Some("later")),
            question("01Q1", "Q1", "Ship it?", Some("yes")),
        ],
    );
    wait_for("the resume", Duration::from_secs(5), || {
        !ran(&dir).is_empty()
    });
    let line = &ran(&dir)[0];
    assert!(line.contains("#Q3 And docs?\nAnswer: later"), "{line}");
    assert!(!line.contains("#Q1"), "{line}");
}

#[test]
fn another_machines_runner_or_an_older_session_is_left_alone() {
    let dir = TempDir::new("parked-other");
    let _hub = parked(&dir);
    set_runner(&dir, "nuc", SESSION);
    set_questions(
        &dir,
        &[
            question("01Q2", "Q2", "Which colour?", Some("teal")),
            question("01Q1", "Q1", "Ship it?", Some("yes")),
        ],
    );
    std::thread::sleep(Duration::from_millis(800));
    assert!(ran(&dir).is_empty(), "nuc's runner: {:?}", ran(&dir));
    set_runner(&dir, "mini", "walk01-m2");
    std::thread::sleep(Duration::from_millis(800));
    assert!(ran(&dir).is_empty(), "a newer runner: {:?}", ran(&dir));
}

#[test]
fn an_open_orchestrator_question_holds_nothing_back() {
    let dir = TempDir::new("parked-orch");
    let _hub = parked(&dir);
    let mut orch = question("01Q0", "Q0", "Implementer asks", None);
    orch["audience"] = json!("orchestrator");
    set_questions(
        &dir,
        &[
            question("01Q2", "Q2", "Which colour?", Some("teal")),
            question("01Q1", "Q1", "Ship it?", Some("yes")),
            orch,
        ],
    );
    wait_for("the resume", Duration::from_secs(5), || {
        !ran(&dir).is_empty()
    });
    assert!(!ran(&dir)[0].contains("Implementer"), "{:?}", ran(&dir));
}

/// amx that cannot answer is no sign the session is gone: the answers wait
/// for amx, and the session resumes with them once it answers again.
#[test]
fn an_amx_that_cannot_answer_holds_the_resume_until_it_can() {
    let dir = TempDir::new("parked-amx-down");
    let _hub = parked(&dir);
    std::fs::write(dir.join("amx.fail"), "").unwrap();
    set_questions(
        &dir,
        &[
            question("01Q2", "Q2", "Which colour?", Some("teal")),
            question("01Q1", "Q1", "Ship it?", Some("yes")),
        ],
    );
    std::thread::sleep(Duration::from_millis(800));
    assert!(
        ran(&dir).is_empty(),
        "nothing starts while amx is down: {:?}",
        ran(&dir)
    );
    std::fs::remove_file(dir.join("amx.fail")).unwrap();
    wait_for("the resume", Duration::from_secs(5), || {
        !ran(&dir).is_empty()
    });
    let line = &ran(&dir)[0];
    assert!(
        line.starts_with("--user --scope --quiet amx resume walk01-m1 "),
        "{line}"
    );
    assert!(line.contains("#Q2 Which colour?\nAnswer: teal"), "{line}");
}
