//! The runner's local hub rings the Slack DM once when its run stalls, with
//! a Resume button, and once when a milestone it ran lands. A fixture `mem`
//! and `amx` stand in for the store and the sessions; a fake Web API records
//! each post.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Hub, TempDir, fixture_bin, fixture_mem, wait_for};
use serde_json::{Value, json};

const PEER_TOKEN: &str = "peer-token-for-tests-0123456789abcdef";
const SESSION: &str = "walk01-w2";

/// A Slack Web API that records the body of each chat.postMessage.
fn fake_slack() -> (String, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let api = format!("http://{}/api", listener.local_addr().unwrap());
    let posts = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&posts);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            answer(stream, &recorded);
        }
    });
    (api, posts)
}

fn answer(mut stream: TcpStream, posts: &Mutex<Vec<Value>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let path = line.split(' ').nth(1).unwrap_or("").to_string();
    let mut length = 0;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap();
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    let reply = if path == "/api/chat.postMessage" {
        posts
            .lock()
            .unwrap()
            .push(serde_json::from_slice(&body).unwrap());
        r#"{"ok":true,"channel":"D0DM","ts":"1700000000.000100"}"#
    } else {
        r#"{"ok":false,"error":"unknown_method"}"#
    };
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    );
}

fn bins(dir: &TempDir) -> PathBuf {
    let bin = dir.join("bin");
    fixture_mem(
        &bin,
        &format!(
            "case \"$1 $2\" in\n\
             'projects --json') cat '{projects}' ;;\n\
             'handoff --project') cat '{handoff}' ;;\n\
             'questions --json') echo '{{\"questions\":[]}}' ;;\n\
             esac",
            projects = dir.join("projects.json").display(),
            handoff = dir.join("handoff.json").display(),
        ),
    );
    fixture_bin(
        &bin,
        "amx",
        &format!(
            "[ \"$1 $2\" = 'ls --json' ] && cat '{}'",
            dir.join("amx.json").display()
        ),
    );
    bin
}

/// walk01 on an approved roadmap, its runner `machine` running `SESSION`
/// on milestone `open` of three, `done` ticked.
fn set_project(dir: &TempDir, machine: &str, open: Option<&str>, done: u64, paused: bool) {
    let row = json!({
        "name": "walk01", "checkouts": ["/x"], "roadmap_status": "approved",
        "milestone": open, "milestones_done": done, "milestones_total": 3,
        "paused": if paused { json!("2026-10-16T00:00:00Z by hub") } else { Value::Null },
        "runner": {"machine": machine, "id": SESSION, "milestone": null,
                   "since": "2026-10-16T00:00:00Z"},
    });
    std::fs::write(
        dir.join("projects.json"),
        json!({ "projects": [row] }).to_string(),
    )
    .unwrap();
}

fn set_live(dir: &TempDir, live: bool) {
    let row = if live {
        json!({"id": SESSION, "state": "running", "ended": 0, "created": 5})
    } else {
        json!({"id": SESSION, "state": "stopped", "ended": 9, "created": 5})
    };
    std::fs::write(dir.join("amx.json"), json!([row]).to_string()).unwrap();
}

fn set_handoff(dir: &TempDir, body: &str) {
    std::fs::write(
        dir.join("handoff.json"),
        json!({ "body": body }).to_string(),
    )
    .unwrap();
}

fn spawn(dir: &TempDir, bin: &Path, api: &str, after_ms: &str) -> Hub {
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
            ("HUB_STALL_CHECK_MS", "100"),
            ("HUB_STALL_AFTER_MS", after_ms),
            ("SLACK_BOT_TOKEN", "xoxb-test"),
            ("SLACK_APP_TOKEN", "xapp-test"),
            ("SLACK_USER_ID", "U0SAIFUL"),
            ("SLACK_API_URL", api),
        ],
    )
}

fn posts_saying(posts: &Mutex<Vec<Value>>, what: &str) -> Vec<Value> {
    posts
        .lock()
        .unwrap()
        .iter()
        .filter(|p| p["text"].as_str().is_some_and(|t| t.contains(what)))
        .cloned()
        .collect()
}

/// A stalled run on this machine, the hub started on it.
fn stalled(dir: &TempDir, after_ms: &str) -> (Hub, Arc<Mutex<Vec<Value>>>) {
    let bin = bins(dir);
    set_project(dir, "mini", Some("w2"), 1, false);
    set_live(dir, false);
    set_handoff(dir, "Next: w2.");
    let (api, posts) = fake_slack();
    (spawn(dir, &bin, &api, after_ms), posts)
}

#[test]
fn a_stall_that_lasts_posts_once_with_a_resume_button() {
    let dir = TempDir::new("stall-once");
    let (_hub, posts) = stalled(&dir, "500");
    wait_for("the stall post", Duration::from_secs(5), || {
        !posts_saying(&posts, "stalled").is_empty()
    });
    // It lasts many more rounds, and still rings once.
    std::thread::sleep(Duration::from_millis(1500));
    let rung = posts_saying(&posts, "stalled");
    assert_eq!(rung.len(), 1, "{rung:?}");
    let post = &rung[0];
    assert_eq!(post["text"], "walk01 stalled on mini");
    assert_eq!(post["channel"], "U0SAIFUL");
    let button = &post["blocks"][1]["elements"][0];
    assert_eq!(button["text"]["text"], "Resume");
    let value: Value = serde_json::from_str(button["value"].as_str().unwrap()).unwrap();
    assert_eq!(
        (&value["k"], &value["m"], &value["p"]),
        (&json!("resume"), &json!("mini"), &json!("walk01"))
    );
    // The hub to pass a tap to, as peers reach it.
    assert!(
        value["h"].as_str().is_some_and(|h| !h.is_empty()),
        "{value}"
    );
}

#[test]
fn a_stall_waits_its_grace_before_ringing() {
    let dir = TempDir::new("stall-grace");
    let (_hub, posts) = stalled(&dir, "60000");
    std::thread::sleep(Duration::from_millis(1200));
    assert!(posts_saying(&posts, "stalled").is_empty());
}

#[test]
fn a_run_that_comes_back_and_stalls_again_rings_again() {
    let dir = TempDir::new("stall-again");
    let (_hub, posts) = stalled(&dir, "300");
    wait_for("the stall post", Duration::from_secs(5), || {
        !posts_saying(&posts, "stalled").is_empty()
    });
    set_live(&dir, true);
    std::thread::sleep(Duration::from_millis(500));
    set_live(&dir, false);
    wait_for("the second stall post", Duration::from_secs(5), || {
        posts_saying(&posts, "stalled").len() >= 2
    });
}

#[test]
fn a_paused_or_parked_project_or_another_machines_run_rings_no_stall() {
    for (case, machine, paused, handoff) in [
        ("paused", "mini", true, "Next: w2."),
        ("parked", "mini", false, "parked: waiting on #Q1"),
        ("nuc's", "nuc", false, "Next: w2."),
    ] {
        let dir = TempDir::new(&format!("stall-none-{}", case.trim_end_matches("'s")));
        let bin = bins(&dir);
        set_project(&dir, machine, Some("w2"), 1, paused);
        set_live(&dir, false);
        set_handoff(&dir, handoff);
        let (api, posts) = fake_slack();
        let _hub = spawn(&dir, &bin, &api, "200");
        std::thread::sleep(Duration::from_millis(1200));
        assert!(posts_saying(&posts, "stalled").is_empty(), "{case}");
    }
}

/// The hub restarted in the middle of a stall it rang already.
#[test]
fn a_stall_rung_before_a_restart_is_not_rung_again() {
    let dir = TempDir::new("stall-restart");
    let (hub, posts) = stalled(&dir, "200");
    wait_for("the stall post", Duration::from_secs(5), || {
        !posts_saying(&posts, "stalled").is_empty()
    });
    drop(hub);
    let (api, again) = fake_slack();
    let _hub = spawn(&dir, &dir.join("bin"), &api, "200");
    std::thread::sleep(Duration::from_millis(1200));
    assert!(posts_saying(&again, "stalled").is_empty());
}

#[test]
fn a_milestone_this_machine_ran_posts_once_when_it_lands() {
    let dir = TempDir::new("landed");
    let bin = bins(&dir);
    set_project(&dir, "mini", Some("w2"), 1, false);
    set_live(&dir, true);
    set_handoff(&dir, "Next: w2.");
    let (api, posts) = fake_slack();
    let _hub = spawn(&dir, &bin, &api, "60000");
    std::thread::sleep(Duration::from_millis(500));
    assert!(posts_saying(&posts, "landed").is_empty());
    // The orchestrator ticks w2, and the next one starts on the nuc.
    set_project(&dir, "nuc", Some("w3"), 2, false);
    wait_for("the landed post", Duration::from_secs(5), || {
        !posts_saying(&posts, "landed").is_empty()
    });
    std::thread::sleep(Duration::from_millis(800));
    let rung = posts_saying(&posts, "landed");
    assert_eq!(rung.len(), 1, "{rung:?}");
    assert_eq!(rung[0]["text"], "walk01: w2 landed on mini");
}

#[test]
fn the_last_milestone_landing_posts_too() {
    let dir = TempDir::new("landed-last");
    let bin = bins(&dir);
    set_project(&dir, "mini", Some("w3"), 2, false);
    set_live(&dir, true);
    set_handoff(&dir, "Next: w3.");
    let (api, posts) = fake_slack();
    let _hub = spawn(&dir, &bin, &api, "60000");
    std::thread::sleep(Duration::from_millis(500));
    set_project(&dir, "mini", None, 3, false);
    wait_for("the landed post", Duration::from_secs(5), || {
        !posts_saying(&posts, "landed").is_empty()
    });
    assert_eq!(
        posts_saying(&posts, "landed")[0]["text"],
        "walk01: w3 landed on mini"
    );
}
