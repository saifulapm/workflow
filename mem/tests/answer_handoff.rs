//! `mem answer` hands the answer to this machine's local hub first, and
//! falls back to writing it here when the hub does not take it.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use common::{World, code, mem_env, stderr, stdout};

fn read_body(stream: &mut UnixStream) -> String {
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
        if let Some(v) = header.to_ascii_lowercase().strip_prefix("content-length:") {
            length = v.trim().parse().unwrap();
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    format!("{}\n{}", line.trim(), String::from_utf8(body).unwrap())
}

/// A fake hub that replies `status` and `body` to every request, and hands
/// each request (request line, newline, body) down the channel.
fn fake_hub(socket: &Path, status: &'static str, body: &'static str) -> mpsc::Receiver<String> {
    std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
    let listener = UnixListener::bind(socket).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let seen = read_body(&mut stream);
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
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

fn ask(w: &World, repo: &Path) -> String {
    let out = mem_env(w, repo, &["ask", "probe?", "--json"], &[]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["short_id"].as_str().unwrap().to_string()
}

fn request(seen: &mpsc::Receiver<String>) -> (String, serde_json::Value) {
    let raw = seen.recv_timeout(Duration::from_secs(5)).unwrap();
    let (line, body) = raw.split_once('\n').unwrap();
    (line.to_string(), serde_json::from_str(body).unwrap())
}

fn unanswered(w: &World, repo: &Path, id: &str) -> bool {
    let out = mem_env(w, repo, &["questions", "--pending"], &[]);
    stdout(&out).contains(id)
}

/// The evidence item for the question exists, with the file's bytes.
fn assert_evidence(w: &World, repo: &Path, id: &str, bytes: &[u8]) {
    let out = mem_env(w, repo, &["evidence", "list"], &[]);
    let listing = stdout(&out);
    let line = listing
        .lines()
        .find(|l| l.contains(&format!("  #{id}  ")))
        .unwrap_or_else(|| panic!("no evidence for #{id}: {listing}"));
    let evidence = line
        .trim_start_matches('#')
        .split_whitespace()
        .next()
        .unwrap();
    let out = mem_env(w, repo, &["evidence", "cat", evidence], &[]);
    assert_eq!(out.stdout, bytes);
}

#[test]
fn a_hub_that_takes_the_answer_gets_it_and_mem_writes_nothing() {
    let w = World::new("ah-200");
    let repo = w.repo("thing", None);
    let id = ask(&w, &repo);
    let seen = fake_hub(&socket_of(&w), "200 OK", "ok");

    let out = mem_env(&w, &repo, &["answer", &id, "yes"], &[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!("answered #{id} (the hub passed it on)\n")
    );
    let (line, body) = request(&seen);
    assert_eq!(line, "POST /answer HTTP/1.1");
    assert_eq!(
        body,
        serde_json::json!({"question": id, "answer": "yes", "by": "cli"})
    );
    assert!(unanswered(&w, &repo, &id));
}

#[test]
fn by_and_the_screenshot_paths_travel_with_the_answer_in_order() {
    let w = World::new("ah-shot");
    let repo = w.repo("thing", None);
    let id = ask(&w, &repo);
    let first = w.dir.join("first.png");
    let second = w.dir.join("second.png");
    std::fs::write(&first, b"png1").unwrap();
    std::fs::write(&second, b"png2").unwrap();
    let seen = fake_hub(&socket_of(&w), "200 OK", "ok");

    let out = mem_env(
        &w,
        &repo,
        &[
            "answer",
            "--by",
            "popup",
            "--screenshot",
            first.to_str().unwrap(),
            "--screenshot",
            second.to_str().unwrap(),
            &id,
            "yes",
        ],
        &[],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let (_, body) = request(&seen);
    assert_eq!(
        body,
        serde_json::json!({
            "question": id, "answer": "yes", "by": "popup",
            "screenshots": [first.to_str().unwrap(), second.to_str().unwrap()],
        })
    );
}

/// Written here, each image becomes its own evidence item.
#[test]
fn every_screenshot_of_an_answer_written_here_is_filed() {
    let w = World::new("ah-shots-here");
    let repo = w.repo("thing", None);
    let id = ask(&w, &repo);
    let first = w.dir.join("first.png");
    let second = w.dir.join("second.png");
    std::fs::write(&first, b"FIRST").unwrap();
    std::fs::write(&second, b"SECOND").unwrap();

    let out = mem_env(
        &w,
        &repo,
        &[
            "answer",
            "--screenshot",
            first.to_str().unwrap(),
            "--screenshot",
            second.to_str().unwrap(),
            &id,
            "yes",
        ],
        &[("MEM_ANSWER_DIRECT", "1")],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let listing = stdout(&mem_env(&w, &repo, &["evidence", "list"], &[]));
    let ids: Vec<&str> = listing
        .lines()
        .filter(|l| l.contains(&format!("  #{id}  ")))
        .filter_map(|l| l.trim_start_matches('#').split_whitespace().next())
        .collect();
    assert_eq!(ids.len(), 2, "{listing}");
    let mut bytes: Vec<Vec<u8>> = ids
        .iter()
        .map(|e| mem_env(&w, &repo, &["evidence", "cat", e], &[]).stdout)
        .collect();
    bytes.sort();
    assert_eq!(bytes, [b"FIRST".to_vec(), b"SECOND".to_vec()]);
}

#[test]
fn a_hash_in_front_of_the_id_is_dropped() {
    let w = World::new("ah-hash");
    let repo = w.repo("thing", None);
    let id = ask(&w, &repo);
    let seen = fake_hub(&socket_of(&w), "200 OK", "ok");

    let out = mem_env(&w, &repo, &["answer", &format!("#{id}"), "yes"], &[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(request(&seen).1["question"], id.as_str());
}

#[test]
fn a_standing_answer_is_reported_and_nothing_is_written() {
    let w = World::new("ah-409");
    let repo = w.repo("thing", None);
    let id = ask(&w, &repo);
    fake_hub(&socket_of(&w), "409 Conflict", "no");

    let out = mem_env(&w, &repo, &["answer", &id, "yes"], &[]);
    assert_eq!(code(&out), 1);
    assert_eq!(
        stderr(&out),
        format!("mem: #{id} already has an answer: no\n")
    );
    assert!(unanswered(&w, &repo, &id));
}

#[test]
fn a_refusal_is_printed_and_nothing_is_written() {
    let w = World::new("ah-403");
    let repo = w.repo("thing", None);
    let id = ask(&w, &repo);
    fake_hub(
        &socket_of(&w),
        "403 Forbidden",
        "a review waits for the human",
    );

    let out = mem_env(&w, &repo, &["answer", &id, "yes"], &[]);
    assert_eq!(code(&out), 1);
    assert_eq!(stderr(&out), "mem: a review waits for the human\n");
    assert!(unanswered(&w, &repo, &id));
}

/// The asking machine could not be reached, or may have taken it after all:
/// a second answer written here would make two, so mem says so instead.
#[test]
fn an_answer_the_hub_could_not_pass_on_is_reported_and_nothing_is_written() {
    let w = World::new("ah-502");
    let repo = w.repo("thing", None);
    let id = ask(&w, &repo);
    fake_hub(
        &socket_of(&w),
        "502 Bad Gateway",
        "the asking machine could not be reached",
    );

    let out = mem_env(&w, &repo, &["answer", &id, "yes"], &[]);
    assert_eq!(code(&out), 1);
    assert_eq!(
        stderr(&out),
        format!("mem: #{id} was not answered: the asking machine could not be reached\n")
    );
    assert!(unanswered(&w, &repo, &id));
}

#[test]
fn a_hub_that_does_not_know_the_question_leaves_the_write_to_mem() {
    let w = World::new("ah-404");
    let repo = w.repo("thing", None);
    let id = ask(&w, &repo);
    let shot = w.dir.join("shot.png");
    std::fs::write(&shot, b"PNGBYTES").unwrap();
    fake_hub(&socket_of(&w), "404 Not Found", "no such question");

    let out = mem_env(
        &w,
        &repo,
        &["answer", "--screenshot", shot.to_str().unwrap(), &id, "yes"],
        &[],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(!unanswered(&w, &repo, &id));
    assert_evidence(&w, &repo, &id, b"PNGBYTES");
}

#[test]
fn with_no_hub_the_answer_is_written_here_with_its_screenshot() {
    let w = World::new("ah-nohub");
    let repo = w.repo("thing", None);
    let id = ask(&w, &repo);
    let shot = w.dir.join("shot.png");
    std::fs::write(&shot, b"PNGBYTES").unwrap();

    let out = mem_env(
        &w,
        &repo,
        &["answer", "--screenshot", shot.to_str().unwrap(), &id, "yes"],
        &[],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(!unanswered(&w, &repo, &id));
    assert_evidence(&w, &repo, &id, b"PNGBYTES");
}

#[test]
fn a_direct_answer_never_calls_the_hub() {
    let w = World::new("ah-direct");
    let repo = w.repo("thing", None);
    let id = ask(&w, &repo);
    let seen = fake_hub(&socket_of(&w), "200 OK", "ok");

    let out = mem_env(
        &w,
        &repo,
        &["answer", "--by", "popup", &id, "yes"],
        &[("MEM_ANSWER_DIRECT", "1")],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(seen.recv_timeout(Duration::from_millis(300)).is_err());
    assert!(!unanswered(&w, &repo, &id));
}

#[test]
fn a_missing_screenshot_is_a_usage_error_before_anything_is_sent_or_written() {
    let w = World::new("ah-missing");
    let repo = w.repo("thing", None);
    let id = ask(&w, &repo);
    let seen = fake_hub(&socket_of(&w), "200 OK", "ok");

    let out = mem_env(
        &w,
        &repo,
        &[
            "answer",
            "--screenshot",
            "/nonexistent/shot.png",
            &id,
            "yes",
        ],
        &[],
    );
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(seen.recv_timeout(Duration::from_millis(300)).is_err());
    assert!(unanswered(&w, &repo, &id));
}
