//! A cloud thread that parked on its questions wakes once Saiful has
//! answered every one: the mini's hub sends the thread one message with
//! `claude -p --cloud <session>`, so he types nothing on claude.ai. A
//! fixture `mem` and `claude` stand in.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Hub, TempDir, fixture_bin, fixture_mem, wait_for};
use serde_json::{Value, json};

const PEER_TOKEN: &str = "peer-token-for-tests-0123456789abcdef";
const SESSION: &str = "session_01NUDGETESTNUDGETESTNUDGE";
const FIRST: &str = "2026-11-13T18:00:00Z";
const PARKED_AT: &str = "2026-11-13T18:40:00Z";

fn bins(dir: &TempDir) -> PathBuf {
    let bin = dir.join("bin");
    let d = dir.path().display();
    fixture_mem(
        &bin,
        &format!(
            "case \"$1 $2\" in\n\
             'projects --json') cat '{d}/projects.json' ;;\n\
             'questions --json') cat '{d}/questions.json' ;;\n\
             'handoff --project') cat '{d}/handoff.txt' ;;\n\
             esac"
        ),
    );
    fixture_bin(
        &bin,
        "claude",
        &format!(
            "{{ printf '%s\\n' \"$*\"; cat; printf '\\n<END>\\n'; }} >> '{d}/sent'\n\
             if [ -e '{d}/claude.fail' ]; then\n\
               echo '{{\"ok\":false,\"error\":\"session not found\"}}'; exit 1\n\
             fi\n\
             echo '{{\"ok\":true,\"session_id\":\"{SESSION}\"}}'"
        ),
    );
    bin
}

fn sent(dir: &TempDir) -> Vec<String> {
    std::fs::read_to_string(dir.join("sent"))
        .unwrap_or_default()
        .split("\n<END>\n")
        .filter(|call| !call.is_empty())
        .map(str::to_string)
        .collect()
}

/// A run of `cloud_walk` in the cloud, recorded at `since`, whose handoff
/// was written at `handoff_at`; its milestone is still the open one unless
/// `open` names another.
fn set_run(dir: &TempDir, since: &str, handoff_at: &str, open: &str) {
    std::fs::write(
        dir.join("projects.json"),
        json!({"projects": [{"name": "cloud_walk", "checkouts": [], "milestone": open,
            "handoff_at": handoff_at,
            "runner": {"machine": "cloud", "id": SESSION, "milestone": "w20-nudge",
                       "first": FIRST, "since": since, "earlier": []}}]})
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("handoff.txt"),
        "parked: question #Q1 waits on Saiful: w20-nudge on cloud/cloud_walk-w20-nudge\n",
    )
    .unwrap();
}

/// A ULID made at `at`, as mem's ids are.
fn ulid(at: &str, tail: &str) -> String {
    const CROCKFORD: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut ms = at.parse::<jiff::Timestamp>().unwrap().as_millisecond();
    let mut time = [b'0'; 10];
    for slot in time.iter_mut().rev() {
        *slot = CROCKFORD[(ms % 32) as usize];
        ms /= 32;
    }
    format!("{}{tail:0>16}", String::from_utf8(time.to_vec()).unwrap())
}

/// A question the thread asked at `at`, answered after the park when
/// `answer` is given.
fn question(at: &str, short: &str, body: &str, answer: Option<&str>) -> Value {
    answered_at(at, short, body, answer, "2026-11-13T18:45:00Z")
}

fn answered_at(at: &str, short: &str, body: &str, answer: Option<&str>, when: &str) -> Value {
    json!({"id": ulid(at, short), "short_id": short, "body": body, "session": null,
           "audience": "saiful", "answered": answer.is_some(), "answer": answer,
           "answer_id": answer.map(|_| ulid(when, &format!("A{short}"))),
           "answer_by": answer.map(|_| "hub")})
}

fn set_questions(dir: &TempDir, rows: &[Value]) {
    std::fs::write(
        dir.join("questions.json"),
        json!({ "questions": rows }).to_string(),
    )
    .unwrap();
}

fn answered(dir: &TempDir, first: Option<&str>, second: Option<&str>) {
    set_questions(
        dir,
        &[
            question(
                "2026-11-13T18:10:00Z",
                "Q2",
                "Do blank lines count?",
                second,
            ),
            question("2026-11-13T18:09:00Z", "Q1", "Empty file prints?", first),
        ],
    );
}

fn spawn(dir: &TempDir, bin: &Path, machine: &str) -> Hub {
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), machine).unwrap();
    let socket = dir.join("local.sock");
    Hub::spawn_env(
        &home,
        &[bin],
        &["local", "--port", "0"],
        &[
            ("HUB_PEER_TOKEN", PEER_TOKEN),
            ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
            ("WORKFLOW_MACHINE", "mini"),
            ("HUB_NUDGE_CHECK_MS", "100"),
        ],
    )
}

#[test]
fn the_last_answer_of_a_parked_thread_sends_it_one_nudge_with_every_answer() {
    let dir = TempDir::new("nudge-last");
    let bin = bins(&dir);
    set_run(&dir, FIRST, PARKED_AT, "w20-nudge");
    answered(&dir, None, None);
    let hub = spawn(&dir, &bin, "mac-mini-m2");
    std::thread::sleep(Duration::from_millis(500));

    answered(&dir, Some("0"), None);
    std::thread::sleep(Duration::from_millis(600));
    assert!(sent(&dir).is_empty(), "one answer of two: {:?}", sent(&dir));

    answered(&dir, Some("0"), Some("no, skip blank lines"));
    wait_for("the nudge", Duration::from_secs(5), || {
        !sent(&dir).is_empty()
    });
    let call = &sent(&dir)[0];
    let (args, text) = call.split_once('\n').unwrap();
    assert_eq!(args, format!("-p --cloud {SESSION} --output-format json"));
    // Oldest first, each with its answer.
    let q1 = text.find("#Q1 Empty file prints?\nAnswer: 0");
    let q2 = text.find("#Q2 Do blank lines count?\nAnswer: no, skip blank lines");
    assert!(q1.is_some() && q2.is_some() && q1 < q2, "{text}");

    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(sent(&dir).len(), 1, "{:?}", sent(&dir));

    // A restarted hub knows this park was nudged.
    drop(hub);
    let _hub = spawn(&dir, &bin, "mac-mini-m2");
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(sent(&dir).len(), 1, "after a restart: {:?}", sent(&dir));
}

#[test]
fn a_hub_on_another_machine_sends_no_nudge() {
    let dir = TempDir::new("nudge-nuc");
    let bin = bins(&dir);
    set_run(&dir, FIRST, PARKED_AT, "w20-nudge");
    answered(&dir, Some("0"), Some("yes"));
    let _hub = spawn(&dir, &bin, "nuc");
    std::thread::sleep(Duration::from_millis(800));
    assert!(sent(&dir).is_empty(), "{:?}", sent(&dir));
}

#[test]
fn a_run_live_again_taken_up_or_landed_gets_no_nudge() {
    for (case, since, open, id) in [
        // The thread was recorded again after its park: a typed message or
        // a start line woke it.
        ("woken", "2026-11-13T18:45:00Z", "w20-nudge", SESSION),
        (
            "taken up",
            "2026-11-13T18:45:00Z",
            "w20-nudge",
            "session_01OTHERTHREADOTHERTHREAD",
        ),
        ("landed", FIRST, "w21-next", SESSION),
    ] {
        let dir = TempDir::new("nudge-none");
        let bin = bins(&dir);
        set_run(&dir, since, PARKED_AT, open);
        let text = std::fs::read_to_string(dir.join("projects.json"))
            .unwrap()
            .replace(SESSION, id);
        std::fs::write(dir.join("projects.json"), text).unwrap();
        answered(&dir, Some("0"), Some("yes"));
        let _hub = spawn(&dir, &bin, "mac-mini-m2");
        std::thread::sleep(Duration::from_millis(800));
        assert!(sent(&dir).is_empty(), "{case}: {:?}", sent(&dir));
    }
}

#[test]
fn a_nudge_that_fails_is_tried_three_times_then_said_once() {
    let dir = TempDir::new("nudge-fail");
    let bin = bins(&dir);
    set_run(&dir, FIRST, PARKED_AT, "w20-nudge");
    answered(&dir, Some("0"), Some("yes"));
    std::fs::write(dir.join("claude.fail"), "").unwrap();
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), "mac-mini-m2").unwrap();
    let log = dir.join("hub.log");
    let socket = dir.join("local.sock");
    let _hub = Hub::spawn_with_stderr(
        &home,
        &[&bin],
        &["local", "--port", "0"],
        &[
            ("HUB_PEER_TOKEN", PEER_TOKEN),
            ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
            ("WORKFLOW_MACHINE", "mini"),
            ("HUB_NUDGE_CHECK_MS", "100"),
        ],
        std::fs::File::create(&log).unwrap(),
    );
    wait_for("three tries", Duration::from_secs(5), || {
        sent(&dir).len() >= 3
    });
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(sent(&dir).len(), 3, "{:?}", sent(&dir));
    let said: Vec<String> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .filter(|line| line.starts_with("hub: nudge:"))
        .map(str::to_string)
        .collect();
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].contains(SESSION) && said[0].contains("session not found"),
        "{said:?}"
    );
}

#[test]
fn a_park_whose_answers_all_came_before_it_gets_no_nudge() {
    // The thread had every answer when it parked: what it parked on is
    // another session's question, or a hold.
    let dir = TempDir::new("nudge-before");
    let bin = bins(&dir);
    set_run(&dir, FIRST, PARKED_AT, "w20-nudge");
    let before = "2026-11-13T18:20:00Z";
    set_questions(
        &dir,
        &[
            answered_at(
                "2026-11-13T18:10:00Z",
                "Q2",
                "Blank lines?",
                Some("yes"),
                before,
            ),
            answered_at(
                "2026-11-13T18:09:00Z",
                "Q1",
                "Empty file?",
                Some("0"),
                before,
            ),
        ],
    );
    let _hub = spawn(&dir, &bin, "mac-mini-m2");
    std::thread::sleep(Duration::from_millis(800));
    assert!(sent(&dir).is_empty(), "{:?}", sent(&dir));
}
