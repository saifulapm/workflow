//! A cloud run whose thread has written nothing to mem for 30 minutes, with
//! no question of its own open and no park, rings Saiful once on Slack from
//! the mini, with the thread's link; again only after the run wrote in
//! between. A fixture `mem` and a fake Slack stand in.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Hub, TempDir, fixture_mem, wait_for};
use serde_json::{Value, json};

const PEER_TOKEN: &str = "peer-token-for-tests-0123456789abcdef";
const SESSION: &str = "session_01QUIETTESTQUIETTESTQUIET";
const RING: &str = "cloud_walk w23-quiet: its thread has written nothing for 30 minutes \
                    and has no open question: https://claude.ai/code/session_01QUIETTESTQUIETTESTQUIET";

fn bins(dir: &TempDir) -> PathBuf {
    let bin = dir.join("bin");
    let d = dir.path().display();
    fixture_mem(
        &bin,
        &format!(
            "case \"$1 $2\" in\n\
             'projects --json') cat '{d}/projects.json' ;;\n\
             'questions --json') cat '{d}/questions.json' 2>/dev/null || echo '{{\"questions\":[]}}' ;;\n\
             'handoff --project') cat '{d}/handoff.txt' ;;\n\
             esac"
        ),
    );
    bin
}

fn ago(minutes: i64) -> String {
    (jiff::Timestamp::now() - jiff::SignedDuration::from_mins(minutes)).to_string()
}

/// cloud_walk's run of w23-quiet in the cloud, recorded `since` minutes ago,
/// its thread's newest write `wrote` minutes ago.
fn set_run(dir: &TempDir, since: i64, wrote: i64) {
    let since = ago(since);
    std::fs::write(
        dir.join("projects.json"),
        json!({"projects": [{"name": "cloud_walk", "checkouts": [], "milestone": "w23-quiet",
            "handoff_at": ago(600), "cloud_write_at": ago(wrote),
            "runner": {"machine": "cloud", "id": SESSION, "milestone": "w23-quiet",
                       "account": "work", "first": since, "since": since, "earlier": []}}]})
        .to_string(),
    )
    .unwrap();
}

fn edit(dir: &TempDir, change: impl FnOnce(&mut Value)) {
    let path = dir.join("projects.json");
    let mut projects: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    change(&mut projects["projects"][0]);
    std::fs::write(&path, projects.to_string()).unwrap();
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

fn spawn(dir: &TempDir, bin: &Path, machine: &str, api: &str) -> Hub {
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
            ("SLACK_BOT_TOKEN", "xoxb-test"),
            ("SLACK_APP_TOKEN", "xapp-test"),
            ("SLACK_USER_ID", "U0SAIFUL"),
            ("SLACK_API_URL", api),
        ],
    )
}

fn rings(posts: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    posts
        .lock()
        .unwrap()
        .iter()
        .filter(|text| text.contains("has written nothing"))
        .cloned()
        .collect()
}

#[test]
fn a_cloud_run_quiet_for_30_minutes_rings_once_with_its_threads_link() {
    let dir = TempDir::new("quiet-rings");
    let bin = bins(&dir);
    set_run(&dir, 60, 31);
    let (api, posts) = fake_slack();
    let _hub = spawn(&dir, &bin, "mac-mini-m2", &api);
    wait_for("the ring", Duration::from_secs(5), || {
        !rings(&posts).is_empty()
    });
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(rings(&posts), [RING]);
}

#[test]
fn a_run_quiet_for_29_minutes_gets_no_ring() {
    let dir = TempDir::new("quiet-29");
    let bin = bins(&dir);
    set_run(&dir, 60, 29);
    let (api, posts) = fake_slack();
    let _hub = spawn(&dir, &bin, "mac-mini-m2", &api);
    std::thread::sleep(Duration::from_millis(1000));
    assert!(rings(&posts).is_empty(), "{:?}", posts.lock().unwrap());
}

#[test]
fn an_open_question_of_its_own_a_park_or_a_landing_keeps_a_run_from_ringing() {
    for case in ["question", "park", "landed", "nuc"] {
        let dir = TempDir::new("quiet-none");
        let bin = bins(&dir);
        set_run(&dir, 60, 45);
        let mut machine = "mac-mini-m2";
        match case {
            // Asked 40 minutes ago by the thread, and waiting in wait-answer.
            "question" => std::fs::write(
                dir.join("questions.json"),
                json!({"questions": [{"id": ulid(&ago(40), "Q1"), "short_id": "Q1",
                    "machine": "cloud-runner-abc", "audience": "saiful",
                    "answered": false, "answer_id": null}]})
                .to_string(),
            )
            .unwrap(),
            // Parked at minute 29 of its quiet.
            "park" => {
                edit(&dir, |p| p["handoff_at"] = json!(ago(16)));
                std::fs::write(
                    dir.join("handoff.txt"),
                    "parked: question #Q1 waits on Saiful: w23-quiet\n",
                )
                .unwrap();
            }
            "landed" => edit(&dir, |p| p["milestone"] = json!("w24-next")),
            // Only the mini rings.
            _ => machine = "nuc",
        }
        let (api, posts) = fake_slack();
        let _hub = spawn(&dir, &bin, machine, &api);
        std::thread::sleep(Duration::from_millis(1000));
        assert!(
            rings(&posts).is_empty(),
            "{case}: {:?}",
            posts.lock().unwrap()
        );
    }
}

/// One ring for each quiet spell: none more while it lasts, a hub restart
/// included, and one again once the run wrote and went quiet again.
#[test]
fn a_quiet_spell_rings_once_and_a_new_one_after_a_write_rings_again() {
    let dir = TempDir::new("quiet-spells");
    let bin = bins(&dir);
    // Quiet for three hours.
    set_run(&dir, 240, 180);
    let (api, posts) = fake_slack();
    let hub = spawn(&dir, &bin, "mac-mini-m2", &api);
    wait_for("the ring", Duration::from_secs(5), || {
        !rings(&posts).is_empty()
    });
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(rings(&posts).len(), 1, "{:?}", rings(&posts));

    drop(hub);
    let hub = spawn(&dir, &bin, "mac-mini-m2", &api);
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(
        rings(&posts).len(),
        1,
        "after a restart: {:?}",
        rings(&posts)
    );

    // It wrote 40 minutes ago, after the first ring's spell began.
    edit(&dir, |p| p["cloud_write_at"] = json!(ago(40)));
    wait_for("the second ring", Duration::from_secs(5), || {
        rings(&posts).len() == 2
    });
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(rings(&posts).len(), 2, "{:?}", rings(&posts));
    drop(hub);
}
