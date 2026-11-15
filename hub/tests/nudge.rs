//! A cloud thread that parked on its questions wakes once Saiful has
//! answered every one: the mini's hub sends the thread one message with
//! `claude -p --cloud <session>`, so he types nothing on claude.ai. A
//! fixture `mem` and `claude` stand in.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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
             show\\ *) cat \"{d}/show-$2.json\" 2>/dev/null || echo '{{\"items\":[{{\"machine\":\"mac-mini-m2\"}}]}}' ;;\n\
             esac"
        ),
    );
    fixture_bin(
        &bin,
        "claude",
        &format!(
            "printf '%s\\n' \"${{CLAUDE_CONFIG_DIR-default}}\" >> '{d}/logins'\n\
             {{ printf '%s\\n' \"$*\"; cat; printf '\\n<END>\\n'; }} >> '{d}/sent'\n\
             if [ -e '{d}/claude.fail' ]; then\n\
               echo '{{\"ok\":false,\"error\":\"session not found\"}}'; exit 1\n\
             fi\n\
             echo '{{\"ok\":true,\"session_id\":\"{SESSION}\"}}'"
        ),
    );
    bin
}

/// The calls the fake claude finished recording; one it is still writing
/// has no `<END>` yet and is left out.
fn sent(dir: &TempDir) -> Vec<String> {
    let text = std::fs::read_to_string(dir.join("sent")).unwrap_or_default();
    let done = text
        .rfind("\n<END>\n")
        .map_or(0, |at| at + "\n<END>\n".len());
    text[..done]
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
    logins(&home);
    let socket = dir.join("local.sock");
    // The hub's own login is neither account's.
    let own = home.join("hub-own-login");
    Hub::spawn_env(
        &home,
        &[bin],
        &["local", "--port", "0"],
        &[
            ("HUB_PEER_TOKEN", PEER_TOKEN),
            ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
            ("WORKFLOW_MACHINE", "mini"),
            ("HUB_NUDGE_CHECK_MS", "100"),
            ("CLAUDE_CONFIG_DIR", own.to_str().unwrap()),
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
    // Oldest first, each his answer as the relay types it.
    let q1 = text.find("Saiful answered your question #Q1 (\"Empty file prints?\"): \"0\".");
    let q2 = text.find(
        "Saiful answered your question #Q2 (\"Do blank lines count?\"): \"no, skip blank lines\".",
    );
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
    logins(&home);
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
fn only_a_park_on_a_question_gets_a_nudge() {
    // A hold, a 6-hour stop or a local-only park is not waiting on an
    // answer, however its questions stand.
    for handoff in [
        "parked: Saiful holds w20-nudge (#Q1). Branch cloud/cloud_walk-w20-nudge",
        "parked: 6-hour stop, at t2 on cloud/cloud_walk-w20-nudge",
        "parked: local-only t14 waits on a device: w20-nudge",
    ] {
        let dir = TempDir::new("nudge-not-question");
        let bin = bins(&dir);
        set_run(&dir, FIRST, PARKED_AT, "w20-nudge");
        std::fs::write(dir.join("handoff.txt"), format!("{handoff}\n")).unwrap();
        answered(&dir, Some("0"), Some("yes"));
        let _hub = spawn(&dir, &bin, "mac-mini-m2");
        std::thread::sleep(Duration::from_millis(800));
        assert!(sent(&dir).is_empty(), "{handoff}: {:?}", sent(&dir));
    }
}

#[test]
fn a_park_on_a_blocked_step_gets_its_nudge_once_answered() {
    let dir = TempDir::new("nudge-blocked");
    let bin = bins(&dir);
    set_run(&dir, FIRST, PARKED_AT, "w20-nudge");
    std::fs::write(
        dir.join("handoff.txt"),
        "parked: blocked: mem project set land-ci ci.yml (#Q1)\n",
    )
    .unwrap();
    // His answer, from a device: the one thing that may clear the block.
    let row = asked_by(
        &dir,
        "2026-11-13T18:09:00Z",
        "Q1",
        ASKED,
        Some("Yes"),
        "nuc",
    );
    set_questions(&dir, &[row]);
    let _hub = spawn(&dir, &bin, "mac-mini-m2");
    wait_for("the nudge", Duration::from_secs(5), || {
        !sent(&dir).is_empty()
    });
}

#[test]
fn an_answer_just_before_the_park_still_gets_its_nudge() {
    // It landed after the thread's last wait-answer and before its park
    // handoff was stamped, so the thread parked without reading it.
    let dir = TempDir::new("nudge-race");
    let bin = bins(&dir);
    set_run(&dir, FIRST, PARKED_AT, "w20-nudge");
    let just_before = "2026-11-13T18:39:58Z";
    set_questions(
        &dir,
        &[
            answered_at(
                "2026-11-13T18:10:00Z",
                "Q2",
                "Blank lines?",
                Some("yes"),
                just_before,
            ),
            answered_at(
                "2026-11-13T18:09:00Z",
                "Q1",
                "Empty file?",
                Some("0"),
                "2026-11-13T18:20:00Z",
            ),
        ],
    );
    let _hub = spawn(&dir, &bin, "mac-mini-m2");
    wait_for("the nudge", Duration::from_secs(5), || {
        !sent(&dir).is_empty()
    });
}

#[test]
fn the_nudge_waits_a_moment_after_the_last_answer() {
    // He may answer and then type in the thread himself: the nudge holds
    // off for HUB_NUDGE_SETTLE_MS after the newest answer.
    let dir = TempDir::new("nudge-settle");
    let bin = bins(&dir);
    set_run(&dir, FIRST, PARKED_AT, "w20-nudge");
    let now = jiff::Timestamp::now().to_string();
    set_questions(
        &dir,
        &[
            answered_at(
                "2026-11-13T18:10:00Z",
                "Q2",
                "Blank lines?",
                Some("yes"),
                &now,
            ),
            question("2026-11-13T18:09:00Z", "Q1", "Empty file?", Some("0")),
        ],
    );
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), "mac-mini-m2").unwrap();
    logins(&home);
    let socket = dir.join("local.sock");
    let _hub = Hub::spawn_env(
        &home,
        &[&bin],
        &["local", "--port", "0"],
        &[
            ("HUB_PEER_TOKEN", PEER_TOKEN),
            ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
            ("WORKFLOW_MACHINE", "mini"),
            ("HUB_NUDGE_CHECK_MS", "100"),
            ("HUB_NUDGE_SETTLE_MS", "2000"),
        ],
    );
    std::thread::sleep(Duration::from_millis(1000));
    assert!(sent(&dir).is_empty(), "too soon: {:?}", sent(&dir));
    wait_for("the settled nudge", Duration::from_secs(5), || {
        !sent(&dir).is_empty()
    });
}

/// The mini's claude logins: the default one in ~/.claude, and the work
/// account's in a folder of its own, which dots' registry names.
fn logins(home: &Path) -> PathBuf {
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(home.join(".claude/.credentials.json"), "{}").unwrap();
    let work = home.join("state/dots/claude-accounts/work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join(".credentials.json"), "{}").unwrap();
    std::fs::write(
        home.join("state/dots/claude-accounts.json"),
        json!({"active": "work", "accounts": [
            {"id": "main", "home": home.join(".claude")},
            {"id": "work", "home": work},
        ]})
        .to_string(),
    )
    .unwrap();
    work
}

/// The CLAUDE_CONFIG_DIR each call of the fake claude saw, `default` for none.
fn logins_used(dir: &TempDir) -> Vec<String> {
    std::fs::read_to_string(dir.join("logins"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// Names the claude.ai account the run's record holds.
fn set_account(dir: &TempDir, account: Option<&str>) {
    let path = dir.join("projects.json");
    let mut projects: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    projects["projects"][0]["runner"]["account"] = account.map_or(Value::Null, Value::from);
    std::fs::write(&path, projects.to_string()).unwrap();
}

#[test]
fn the_nudge_goes_out_with_the_login_of_the_runs_own_account() {
    // The hub's own login may be either; the run's account decides.
    for (account, work) in [(Some("work"), true), (Some("main"), false), (None, false)] {
        let dir = TempDir::new("nudge-account");
        let bin = bins(&dir);
        set_run(&dir, FIRST, PARKED_AT, "w20-nudge");
        set_account(&dir, account);
        answered(&dir, Some("0"), Some("yes"));
        let _hub = spawn(&dir, &bin, "mac-mini-m2");
        wait_for("the nudge", Duration::from_secs(5), || {
            !sent(&dir).is_empty()
        });
        let want = if work {
            dir.join("home/state/dots/claude-accounts/work")
                .display()
                .to_string()
        } else {
            "default".to_string()
        };
        assert_eq!(logins_used(&dir), [want], "{account:?}");
    }
}

/// A Slack Web API that records the text of each chat.postMessage.
fn fake_slack() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let api = format!("http://{}/api", listener.local_addr().unwrap());
    let posts = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&posts);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                let header = header.trim_end().to_ascii_lowercase();
                if header.is_empty() {
                    break;
                }
                if let Some(value) = header.strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap_or_default();
            recorded
                .lock()
                .unwrap()
                .push(body["text"].as_str().unwrap_or_default().to_string());
            let reply = r#"{"ok":true,"channel":"D0DM","ts":"1700000000.000100"}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    (api, posts)
}

#[test]
fn a_run_whose_account_has_no_login_here_gets_no_send_and_one_ring() {
    let dir = TempDir::new("nudge-no-login");
    let bin = bins(&dir);
    set_run(&dir, FIRST, PARKED_AT, "w20-nudge");
    set_account(&dir, Some("work"));
    answered(&dir, Some("0"), Some("yes"));
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), "mac-mini-m2").unwrap();
    // The work account's folder is there, logged out.
    let work = logins(&home);
    std::fs::remove_file(work.join(".credentials.json")).unwrap();
    let (api, posts) = fake_slack();
    let socket = dir.join("local.sock");
    let _hub = Hub::spawn_env(
        &home,
        &[&bin],
        &["local", "--port", "0"],
        &[
            ("HUB_PEER_TOKEN", PEER_TOKEN),
            ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
            ("WORKFLOW_MACHINE", "mini"),
            ("HUB_NUDGE_CHECK_MS", "100"),
            ("SLACK_BOT_TOKEN", "xoxb-test"),
            ("SLACK_APP_TOKEN", "xapp-test"),
            ("SLACK_USER_ID", "U0SAIFUL"),
            ("SLACK_API_URL", &api),
        ],
    );
    let ring = "no claude login for the work account on mac-mini-m2";
    wait_for("the ring", Duration::from_secs(5), || {
        posts.lock().unwrap().iter().any(|text| text == ring)
    });
    std::thread::sleep(Duration::from_millis(800));
    let rings = posts
        .lock()
        .unwrap()
        .iter()
        .filter(|text| *text == ring)
        .count();
    assert_eq!(rings, 1, "{:?}", posts.lock().unwrap());
    assert!(sent(&dir).is_empty(), "{:?}", sent(&dir));
}

/// A run of `cloud_walk` live in the cloud since FIRST, with no park.
fn live_run(dir: &TempDir) {
    set_run(dir, FIRST, "2026-11-13T17:00:00Z", "w20-nudge");
}

/// A question the thread asked from a cloud machine at `at`, answered at
/// 18:45 by `by` when `answer` is given.
fn asked_by(
    dir: &TempDir,
    at: &str,
    short: &str,
    body: &str,
    answer: Option<&str>,
    by: &str,
) -> Value {
    let mut row = question(at, short, body, answer);
    row["machine"] = json!("cloud-vm-2");
    if let Some(id) = row["answer_id"].as_str() {
        std::fs::write(
            dir.join(&format!("show-{id}.json")),
            json!({"items": [{"id": id, "kind": "answer", "machine": by}]}).to_string(),
        )
        .unwrap();
    }
    row
}

const ASKED: &str = "May I run: mem project set verify \"bash ./test.sh\"?";

#[test]
fn a_devices_answer_to_a_live_run_is_typed_into_its_thread_once() {
    for by in ["macbook-m2", "hub-web"] {
        let dir = TempDir::new("relay-device");
        let bin = bins(&dir);
        live_run(&dir);
        let row = asked_by(
            &dir,
            "2026-11-13T18:09:00Z",
            "AAAA1111",
            ASKED,
            Some("Yes"),
            by,
        );
        set_questions(&dir, &[row]);
        let hub = spawn(&dir, &bin, "mac-mini-m2");
        wait_for("the relay", Duration::from_secs(5), || {
            !sent(&dir).is_empty()
        });
        let call = &sent(&dir)[0];
        let (args, text) = call.split_once('\n').unwrap();
        assert_eq!(args, format!("-p --cloud {SESSION} --output-format json"));
        assert_eq!(
            text,
            "Saiful answered your question #AAAA1111 (\"May I run: mem project set verify \
             \"bash ./test.sh\"?\"): \"Yes\". His hub typed this for him.",
            "{by}"
        );
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(sent(&dir).len(), 1, "{by}: {:?}", sent(&dir));
        // A restarted hub knows the answer went in.
        drop(hub);
        let _hub = spawn(&dir, &bin, "mac-mini-m2");
        std::thread::sleep(Duration::from_millis(800));
        assert_eq!(
            sent(&dir).len(),
            1,
            "{by} after a restart: {:?}",
            sent(&dir)
        );
    }
}

#[test]
fn every_answer_goes_in_its_own_message_in_the_order_asked_no_and_hold_too() {
    let dir = TempDir::new("relay-order");
    let bin = bins(&dir);
    live_run(&dir);
    let body = "May I run:\nrm -r \"scratch dir\"?";
    let rows = [
        asked_by(
            &dir,
            "2026-11-13T18:10:00Z",
            "BBBB2222",
            "Keep going?",
            Some("hold"),
            "nuc",
        ),
        asked_by(
            &dir,
            "2026-11-13T18:09:00Z",
            "AAAA1111",
            body,
            Some("No"),
            "mac-mini-m2",
        ),
    ];
    set_questions(&dir, &rows);
    let _hub = spawn(&dir, &bin, "mac-mini-m2");
    wait_for("both relays", Duration::from_secs(5), || {
        sent(&dir).len() >= 2
    });
    let texts: Vec<String> = sent(&dir)
        .iter()
        .map(|call| call.split_once('\n').unwrap().1.to_string())
        .collect();
    assert_eq!(
        texts,
        [
            "Saiful answered your question #AAAA1111 (\"May I run:\nrm -r \"scratch dir\"?\"): \
             \"No\". His hub typed this for him.",
            "Saiful answered your question #BBBB2222 (\"Keep going?\"): \"hold\". His hub \
             typed this for him.",
        ]
    );
}

#[test]
fn no_answer_a_cloud_machine_wrote_is_typed_in() {
    // The thread's own `mem answer`, or any write from a cloud machine,
    // is not his word.
    for by in ["cloud-vm-2", "cloud", ""] {
        let dir = TempDir::new("relay-cloud");
        let bin = bins(&dir);
        live_run(&dir);
        let row = asked_by(
            &dir,
            "2026-11-13T18:09:00Z",
            "AAAA1111",
            ASKED,
            Some("Yes"),
            by,
        );
        set_questions(&dir, &[row]);
        let _hub = spawn(&dir, &bin, "mac-mini-m2");
        std::thread::sleep(Duration::from_millis(800));
        assert!(sent(&dir).is_empty(), "{by:?}: {:?}", sent(&dir));
    }
}

#[test]
fn a_device_agents_question_or_a_landed_run_gets_no_relay() {
    // A device's agent asked it: not the thread's question.
    let dir = TempDir::new("relay-device-asked");
    let bin = bins(&dir);
    live_run(&dir);
    let mut row = asked_by(
        &dir,
        "2026-11-13T18:09:00Z",
        "AAAA1111",
        ASKED,
        Some("Yes"),
        "nuc",
    );
    row["machine"] = json!("mac-mini-m2");
    set_questions(&dir, &[row]);
    let _hub = spawn(&dir, &bin, "mac-mini-m2");
    std::thread::sleep(Duration::from_millis(800));
    assert!(sent(&dir).is_empty(), "{:?}", sent(&dir));

    // The run's milestone landed: the open one is the next.
    let dir = TempDir::new("relay-landed");
    let bin = bins(&dir);
    set_run(&dir, FIRST, "2026-11-13T17:00:00Z", "w21-next");
    let row = asked_by(
        &dir,
        "2026-11-13T18:09:00Z",
        "AAAA1111",
        ASKED,
        Some("Yes"),
        "nuc",
    );
    set_questions(&dir, &[row]);
    let _hub = spawn(&dir, &bin, "mac-mini-m2");
    std::thread::sleep(Duration::from_millis(800));
    assert!(sent(&dir).is_empty(), "{:?}", sent(&dir));
}

#[test]
fn an_answer_from_before_the_runs_latest_record_is_not_typed_in_again() {
    // It came while the run was parked: its prompt or its nudge had it.
    let dir = TempDir::new("relay-before");
    let bin = bins(&dir);
    set_run(
        &dir,
        "2026-11-13T18:50:00Z",
        "2026-11-13T18:40:00Z",
        "w20-nudge",
    );
    let row = asked_by(
        &dir,
        "2026-11-13T18:09:00Z",
        "AAAA1111",
        ASKED,
        Some("Yes"),
        "nuc",
    );
    set_questions(&dir, &[row]);
    let _hub = spawn(&dir, &bin, "mac-mini-m2");
    std::thread::sleep(Duration::from_millis(800));
    assert!(sent(&dir).is_empty(), "{:?}", sent(&dir));
}

#[test]
fn a_parked_runs_device_answer_comes_once_in_the_relays_words() {
    // Parked on its question: the nudge wakes it, its first line his
    // answer as the relay types it, and no relay comes besides.
    let dir = TempDir::new("relay-parked");
    let bin = bins(&dir);
    set_run(&dir, FIRST, PARKED_AT, "w20-nudge");
    let row = asked_by(
        &dir,
        "2026-11-13T18:09:00Z",
        "Q1",
        ASKED,
        Some("Yes"),
        "macbook-m2",
    );
    set_questions(&dir, &[row]);
    let _hub = spawn(&dir, &bin, "mac-mini-m2");
    wait_for("the nudge", Duration::from_secs(5), || {
        !sent(&dir).is_empty()
    });
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(sent(&dir).len(), 1, "{:?}", sent(&dir));
    let text = sent(&dir)[0].split_once('\n').unwrap().1.to_string();
    assert!(
        text.starts_with(
            "Saiful answered your question #Q1 (\"May I run: mem project set verify \
             \"bash ./test.sh\"?\"): \"Yes\". His hub typed this for him.\n"
        ),
        "{text}"
    );
    assert!(!text.contains("typed nothing"), "{text}");
}

#[test]
fn a_park_on_a_blocked_step_is_not_nudged_again_with_the_same_answers() {
    // Still blocked after his answer went in: another message with the
    // same answer would only park it again, and again.
    let dir = TempDir::new("nudge-blocked-again");
    let bin = bins(&dir);
    set_run(&dir, FIRST, PARKED_AT, "w20-nudge");
    std::fs::write(
        dir.join("handoff.txt"),
        "parked: blocked: mem project set verify \"true\" (#Q1)\n",
    )
    .unwrap();
    let row = asked_by(
        &dir,
        "2026-11-13T18:09:00Z",
        "Q1",
        ASKED,
        Some("Yes"),
        "nuc",
    );
    set_questions(&dir, &[row]);
    let _hub = spawn(&dir, &bin, "mac-mini-m2");
    wait_for("the nudge", Duration::from_secs(5), || {
        !sent(&dir).is_empty()
    });
    // It woke, tried, and parked on the same step again.
    set_run(
        &dir,
        "2026-11-13T19:00:00Z",
        "2026-11-13T19:05:00Z",
        "w20-nudge",
    );
    std::fs::write(
        dir.join("handoff.txt"),
        "parked: blocked: mem project set verify \"true\" (#Q1)\n",
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(sent(&dir).len(), 1, "{:?}", sent(&dir));
}

#[test]
fn a_parked_nudge_leaves_out_an_answer_the_thread_wrote_itself() {
    let dir = TempDir::new("nudge-own-answer");
    let bin = bins(&dir);
    set_run(&dir, FIRST, PARKED_AT, "w20-nudge");
    let row = asked_by(
        &dir,
        "2026-11-13T18:09:00Z",
        "Q1",
        ASKED,
        Some("Yes"),
        "cloud-vm-2",
    );
    set_questions(&dir, &[row]);
    let _hub = spawn(&dir, &bin, "mac-mini-m2");
    wait_for("the nudge", Duration::from_secs(5), || {
        !sent(&dir).is_empty()
    });
    let text = sent(&dir)[0].split_once('\n').unwrap().1.to_string();
    assert!(!text.contains("May I run"), "{text}");
    assert!(!text.contains("Yes"), "{text}");
}

#[test]
fn an_answer_typed_in_while_live_is_not_typed_again_by_a_later_nudge() {
    let dir = TempDir::new("nudge-typed-before");
    let bin = bins(&dir);
    live_run(&dir);
    let q1 = asked_by(
        &dir,
        "2026-11-13T18:09:00Z",
        "Q1",
        ASKED,
        Some("Yes"),
        "nuc",
    );
    set_questions(&dir, std::slice::from_ref(&q1));
    let _hub = spawn(&dir, &bin, "mac-mini-m2");
    wait_for("the relay", Duration::from_secs(5), || {
        !sent(&dir).is_empty()
    });
    // Then it parked on a second question, which he answered.
    set_run(&dir, FIRST, PARKED_AT, "w20-nudge");
    std::fs::write(
        dir.join("handoff.txt"),
        "parked: question #Q2 waits on Saiful: which word\n",
    )
    .unwrap();
    let q2 = asked_by(
        &dir,
        "2026-11-13T18:10:00Z",
        "Q2",
        "Which word?",
        Some("count"),
        "nuc",
    );
    set_questions(&dir, &[q2, q1]);
    wait_for("the nudge", Duration::from_secs(5), || {
        sent(&dir).len() >= 2
    });
    let text = sent(&dir)[1].split_once('\n').unwrap().1.to_string();
    assert!(
        text.starts_with("Saiful answered your question #Q2 (\"Which word?\"): \"count\"."),
        "{text}"
    );
    assert!(!text.contains("#Q1"), "{text}");
}

#[test]
fn a_relay_that_did_not_go_in_still_lets_a_blocked_park_be_woken() {
    let dir = TempDir::new("relay-failed-blocked");
    let bin = bins(&dir);
    live_run(&dir);
    let row = asked_by(
        &dir,
        "2026-11-13T18:09:00Z",
        "Q1",
        ASKED,
        Some("Yes"),
        "nuc",
    );
    set_questions(&dir, &[row]);
    std::fs::write(dir.join("claude.fail"), "").unwrap();
    let _hub = spawn(&dir, &bin, "mac-mini-m2");
    wait_for("three tries", Duration::from_secs(5), || {
        sent(&dir).len() >= 3
    });
    std::thread::sleep(Duration::from_millis(500));
    std::fs::remove_file(dir.join("claude.fail")).unwrap();
    // The thread never saw it, tried, was blocked, and parked.
    set_run(&dir, FIRST, PARKED_AT, "w20-nudge");
    std::fs::write(
        dir.join("handoff.txt"),
        "parked: blocked: mem project set verify \"true\" (#Q1)\n",
    )
    .unwrap();
    wait_for("the nudge", Duration::from_secs(5), || {
        sent(&dir).len() >= 4
    });
    assert!(
        sent(&dir)[3].contains("Saiful answered your question #Q1"),
        "{:?}",
        sent(&dir)
    );
}
