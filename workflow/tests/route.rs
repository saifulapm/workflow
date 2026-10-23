//! `workflow route` through the real binary, against a stand-in for this
//! machine's local hub on its socket: it keeps the request it got and
//! answers with the status and body the test gives it.

#[path = "../src/scratch.rs"]
mod scratch;

use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::process::Output;
use std::sync::RwLock;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use scratch::Scratch;
use serde_json::json;

/// A child forked while another test's fixture is still open for writing
/// holds that file open, and its exec then fails with "Text file busy". So
/// a fixture is written under the write lock and every spawn waits for it.
static FORKS: RwLock<()> = RwLock::new(());

fn world(tag: &str) -> Scratch {
    let work = Scratch::new(tag);
    let _writing = FORKS.write().unwrap_or_else(|e| e.into_inner());
    work.mkdir("state/hub");
    work
}

fn route(work: &Scratch, args: &[&str]) -> Output {
    let _forking = FORKS.read().unwrap_or_else(|e| e.into_inner());
    std::process::Command::new(env!("CARGO_BIN_EXE_workflow"))
        .arg("route")
        .args(args)
        .env("HOME", work.at("home"))
        .env("XDG_STATE_HOME", work.at("state"))
        .env_remove("HUB_LOCAL_SOCKET")
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// A local hub on `work`'s socket that takes one request, answers it
/// `status` with `body`, and hands back what it got.
fn local_hub(work: &Scratch, status: &str, body: &str) -> JoinHandle<Option<String>> {
    let listener = UnixListener::bind(work.at("state/hub/local.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let answer = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(_) => return None,
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = stream.read(&mut buf).unwrap_or(0);
            got.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&got).to_string();
            if let Some((head, sent)) = text.split_once("\r\n\r\n") {
                let want = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                if sent.len() >= want {
                    break;
                }
            }
            if n == 0 {
                break;
            }
        }
        let _ = stream.write_all(answer.as_bytes());
        Some(String::from_utf8_lossy(&got).to_string())
    })
}

fn body(request: &str) -> serde_json::Value {
    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[test]
fn route_device_tells_the_local_hub_and_prints_its_machine() {
    let w = world("route-device");
    let hub = local_hub(
        &w,
        "200",
        r#"{"route":"device","machine":"mini.tail.ts.net","at":5}"#,
    );
    let out = route(&w, &["device"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let got = hub.join().unwrap().expect("the local hub was told");
    assert!(got.starts_with("POST /route "), "{got}");
    assert_eq!(body(&got), json!({"route": "device"}));
    assert_eq!(stdout(&out), "route: device (mini.tail.ts.net)\n");
}

#[test]
fn route_phone_tells_the_local_hub() {
    let w = world("route-phone");
    let hub = local_hub(&w, "200", r#"{"route":"phone","machine":null,"at":5}"#);
    let out = route(&w, &["phone"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let got = hub.join().unwrap().expect("the local hub was told");
    assert_eq!(body(&got), json!({"route": "phone"}));
    assert_eq!(stdout(&out), "route: phone\n");
}

#[test]
fn route_alone_reads_the_local_hub() {
    let w = world("route-get");
    let hub = local_hub(
        &w,
        "200",
        r#"{"route":"device","machine":"nuc.tail.ts.net","at":5}"#,
    );
    let out = route(&w, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let got = hub.join().unwrap().expect("the local hub was asked");
    assert!(got.starts_with("GET /route "), "{got}");
    assert_eq!(stdout(&out), "route: device (nuc.tail.ts.net)\n");
}

#[test]
fn a_refusal_prints_the_local_hubs_words_and_fails() {
    let w = world("route-refused");
    let hub = local_hub(&w, "400", "route to phone or device: bad");
    let out = route(&w, &["phone"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    hub.join().unwrap();
    assert!(
        stderr(&out).contains("route to phone or device: bad"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn no_local_hub_fails_and_says_so() {
    let w = world("route-nohub");
    let out = route(&w, &["device"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        stderr(&out).contains("the local hub did not answer"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn dulali_is_no_longer_a_route() {
    let w = world("route-dulali");
    let out = route(&w, &["dulali"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let err = stderr(&out);
    assert!(
        err.contains("phone, device") && !err.contains("dulali]"),
        "{err}"
    );
}
