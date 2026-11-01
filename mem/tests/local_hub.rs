//! mem and its machine's local hub: `mem ask` hands the question over, and
//! `mem questions --wait` takes the answer the local hub is handed.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::{World, code, mem, mem_env, stderr, stdout};

/// One request as a fake hub saw it.
struct Seen {
    line: String,
    body: String,
}

fn read_request(stream: &mut UnixStream) -> Seen {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let mut length = 0;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        if header == "\r\n" || header.is_empty() {
            break;
        }
        let lower = header.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            length = v.trim().parse().unwrap();
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    Seen {
        line: line.trim().to_string(),
        body: String::from_utf8(body).unwrap(),
    }
}

fn reply(stream: &mut UnixStream, status: &str, body: &str) {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
}

/// A fake local hub on `socket` that answers each request with `answer`,
/// and passes what it saw back down the channel.
fn fake_hub(
    socket: &Path,
    mut answer: impl FnMut(&Seen) -> (String, String) + Send + 'static,
) -> mpsc::Receiver<Seen> {
    std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
    let listener = UnixListener::bind(socket).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let seen = read_request(&mut stream);
            let (status, body) = answer(&seen);
            reply(&mut stream, &status, &body);
            if tx.send(seen).is_err() {
                return;
            }
        }
    });
    rx
}

fn socket_of(w: &World) -> PathBuf {
    w.dirs().state.join("hub/local.sock")
}

#[test]
fn an_ask_is_handed_to_the_local_hub_with_its_text_and_options() {
    let w = World::new("lh-ask");
    let repo = w.repo("thing", None);
    let seen = fake_hub(&socket_of(&w), |_| ("200 OK".into(), "ok\n".into()));

    let out = mem_env(
        &w,
        &repo,
        &["ask", "probe?", "--options", "yes,no", "--for", "saiful"],
        &[],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let id = stdout(&out).trim().trim_start_matches('#').to_string();

    let ask = seen.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(ask.line, "POST /ask HTTP/1.1");
    let v: serde_json::Value = serde_json::from_str(&ask.body).unwrap();
    assert_eq!(v["id"], id);
    assert_eq!(v["project"], "thing");
    assert_eq!(v["text"], "probe?");
    assert_eq!(v["options"], serde_json::json!(["yes", "no"]));
    assert_eq!(v["audience"], "human");
    assert_eq!(v["recommend"], serde_json::Value::Null);
}

#[test]
fn an_ask_hands_the_local_hub_its_recommended_option() {
    let w = World::new("lh-recommend");
    let repo = w.repo("thing", None);
    let seen = fake_hub(&socket_of(&w), |_| ("200 OK".into(), "ok\n".into()));

    let out = mem_env(
        &w,
        &repo,
        &["ask", "probe?", "--options", "yes,no", "--recommend", "no"],
        &[],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let ask = seen.recv_timeout(Duration::from_secs(5)).unwrap();
    let v: serde_json::Value = serde_json::from_str(&ask.body).unwrap();
    assert_eq!(v["recommend"], "no");
}

#[test]
fn an_ask_with_no_local_hub_still_succeeds() {
    let w = World::new("lh-nohub");
    let repo = w.repo("thing", None);
    let started = Instant::now();
    let out = mem_env(&w, &repo, &["ask", "probe?"], &[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn a_wait_ends_as_soon_as_the_local_hub_has_the_answer() {
    let w = World::new("lh-wait");
    let repo = w.repo("thing", None);
    let out = mem_env(&w, &repo, &["ask", "probe?", "--options", "yes,no"], &[]);
    let id = stdout(&out).trim().trim_start_matches('#').to_string();

    // The hub holds the first wait open and has nothing; the second gets
    // the answer.
    let mut calls = 0;
    let seen = fake_hub(&socket_of(&w), move |_| {
        calls += 1;
        if calls == 1 {
            std::thread::sleep(Duration::from_millis(300));
            ("204 No Content".into(), String::new())
        } else {
            ("200 OK".into(), "yes".into())
        }
    });

    let started = Instant::now();
    // The store poll stays at its 15 s default: only the hub can end this
    // wait in time.
    let out = mem_env(
        &w,
        &repo,
        &["questions", "--wait", &id, "--timeout", "30s"],
        &[],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "yes");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    let first = seen.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(
        first.line.starts_with(&format!("GET /wait/{id}?s=")),
        "{}",
        first.line
    );
}

/// A cloud session's mem runs every verb on the mem server, which refuses a
/// wait; with a local hub, the wait is the local hub's.
#[test]
fn over_the_remote_a_wait_is_answered_by_the_local_hub_without_the_server() {
    let w = World::new("lh-remote-wait");
    let seen = fake_hub(&socket_of(&w), |_| ("200 OK".into(), "yes".into()));
    let out = mem_env(
        &w,
        w.dir.path(),
        &["questions", "--wait", "#ABCD1234", "--timeout", "30s"],
        &[("MEM_REMOTE", "http://127.0.0.1:9")],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "yes");
    let first = seen.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(
        first.line.starts_with("GET /wait/ABCD1234?s="),
        "{}",
        first.line
    );
}

/// And an ask over the remote is written on the server, then handed to the
/// local hub under the id and the project the server gave it: in a cloud
/// session the server picks the project by the git remote, so only it knows
/// the name.
#[test]
fn over_the_remote_an_ask_is_handed_to_the_local_hub_after_the_server_writes_it() {
    let w = World::new("lh-remote-ask");
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", server.local_addr().unwrap().port());
    let (sent_tx, sent_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = server.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut length = 0;
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).unwrap();
            if header == "\r\n" {
                break;
            }
            if let Some(v) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                length = v.trim().parse().unwrap();
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        sent_tx.send(String::from_utf8(body).unwrap()).unwrap();
        let printed = serde_json::json!({"id": "01M5000000000000000WXYZ5678", "short_id": "WXYZ5678", "project": "cloud_walk"}).to_string();
        let reply = serde_json::json!({"stdout": format!("{printed}\n"), "stderr": "", "code": 0})
            .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
            reply.len()
        )
        .unwrap();
    });
    let seen = fake_hub(&socket_of(&w), |_| ("200 OK".into(), "ok\n".into()));

    let out = mem_env(
        &w,
        w.dir.path(),
        &["ask", "probe?", "--options", "yes,no"],
        &[("MEM_REMOTE", &url)],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "#WXYZ5678");
    let sent: serde_json::Value = serde_json::from_str(&sent_rx.recv().unwrap()).unwrap();
    assert!(
        sent["argv"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == "--json"),
        "{sent}"
    );
    let ask = seen.recv_timeout(Duration::from_secs(5)).unwrap();
    let v: serde_json::Value = serde_json::from_str(&ask.body).unwrap();
    assert_eq!(v["id"], "WXYZ5678");
    assert_eq!(v["project"], "cloud_walk");
    assert_eq!(v["text"], "probe?");
    assert_eq!(v["options"], serde_json::json!(["yes", "no"]));
}

#[test]
fn an_asks_json_names_its_project() {
    let w = World::new("lh-ask-json");
    let repo = w.repo("thing", None);
    let out = mem_env(&w, &repo, &["ask", "probe?", "--json"], &[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["project"], "thing");
}

/// The cloud VM's hub, `self` named as it is on the tailnet.
const HOST: &str = "cloud-vm.taila27604.ts.net";

/// A fake local hub whose `GET /questions` lists `open` as asked here.
fn hub_holding(w: &World, open: &[&str]) {
    let questions: Vec<serde_json::Value> = open
        .iter()
        .map(|id| serde_json::json!({"id": id, "project": "thing", "machine": HOST, "text": "probe?", "person": true}))
        .collect();
    let doc = serde_json::json!({"self": HOST, "questions": questions}).to_string();
    let _ = fake_hub(&socket_of(w), move |_| ("200 OK".into(), doc.clone()));
}

/// `mem ask`, as session `session` asks it; the short id.
fn ask_in(w: &World, repo: &Path, session: &str) -> String {
    let out = mem_env(
        w,
        repo,
        &["ask", "probe?"],
        &[("CLAUDE_CODE_SESSION_ID", session), ("AMX_ID", "")],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    stdout(&out).trim().trim_start_matches('#').to_string()
}

fn stop_hook(w: &World, repo: &Path, session: &str) -> std::process::Output {
    let out = mem_env(w, repo, &["stop-hook", "--session-id", session], &[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    out
}

/// A cloud session's VM goes offline when its turn ends, so an answer sent
/// to it then has nowhere to land: its stop is blocked while a question it
/// asked is open, with the command that waits for the answer.
#[test]
fn a_stop_is_blocked_while_the_sessions_own_question_is_open() {
    let w = World::new("lh-stop-open");
    let repo = w.repo("thing", None);
    let id = ask_in(&w, &repo, "sess-a");
    hub_holding(&w, &[&id]);

    let out = stop_hook(&w, &repo, "sess-a");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["decision"], "block", "{v}");
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.contains(&format!("#{id}")), "{reason}");
    assert!(
        reason.contains(&format!("`mem questions --wait {id}`")),
        "{reason}"
    );
}

#[test]
fn a_stop_goes_through_once_the_hub_no_longer_holds_the_question() {
    let w = World::new("lh-stop-answered");
    let repo = w.repo("thing", None);
    ask_in(&w, &repo, "sess-a");
    hub_holding(&w, &[]);

    assert_eq!(stdout(&stop_hook(&w, &repo, "sess-a")), "");
}

#[test]
fn another_sessions_open_question_does_not_block_the_stop() {
    let w = World::new("lh-stop-other");
    let repo = w.repo("thing", None);
    let id = ask_in(&w, &repo, "sess-b");
    hub_holding(&w, &[&id]);

    assert_eq!(stdout(&stop_hook(&w, &repo, "sess-a")), "");
}

/// The 6-hour stop parks with a handoff, and that stop is meant.
#[test]
fn a_parked_handoff_after_the_question_lets_the_stop_through() {
    let w = World::new("lh-stop-parked");
    let repo = w.repo("thing", None);
    let id = ask_in(&w, &repo, "sess-a");
    let out = mem(&w, &repo, &["handoff", "--set", "parked: 6-hour stop"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    hub_holding(&w, &[&id]);

    assert_eq!(stdout(&stop_hook(&w, &repo, "sess-a")), "");
}

/// An earlier run's park is no park of this question.
#[test]
fn a_parked_handoff_older_than_the_question_still_blocks() {
    let w = World::new("lh-stop-old-park");
    let repo = w.repo("thing", None);
    let out = mem(&w, &repo, &["handoff", "--set", "parked: 6-hour stop"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let id = ask_in(&w, &repo, "sess-a");
    hub_holding(&w, &[&id]);

    let out = stop_hook(&w, &repo, "sess-a");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["decision"], "block", "{v}");
}

/// No hub means no answer can arrive either way, and a broken hook must
/// never trap a session.
#[test]
fn without_a_local_hub_the_stop_goes_through() {
    let w = World::new("lh-stop-nohub");
    let repo = w.repo("thing", None);
    ask_in(&w, &repo, "sess-a");

    assert_eq!(stdout(&stop_hook(&w, &repo, "sess-a")), "");
}
