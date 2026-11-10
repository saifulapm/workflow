//! `workflow device use` and bare `workflow device` through the real binary,
//! against a stand-in for this machine's local hub on its socket: it keeps
//! the request it got and answers with the status and body the test gives it.

#[path = "../src/scratch.rs"]
mod scratch;

use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::process::Output;
use std::sync::RwLock;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use scratch::Scratch;

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

fn device(work: &Scratch, args: &[&str]) -> Output {
    let _forking = FORKS.read().unwrap_or_else(|e| e.into_inner());
    std::process::Command::new(env!("CARGO_BIN_EXE_workflow"))
        .arg("device")
        .args(args)
        .env("HOME", work.at("home"))
        .env("XDG_STATE_HOME", work.at("state"))
        .env_remove("HUB_LOCAL_SOCKET")
        .env_remove("MEM_PROJECT")
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
fn use_nuc_posts_the_machine_and_prints_the_answer() {
    let w = world("use-nuc");
    let hub = local_hub(&w, "200", r#"{"device":"nuc","at":5}"#);
    let out = device(&w, &["use", "nuc"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let got = hub.join().unwrap().expect("the local hub was told");
    assert!(got.starts_with("POST /device/choice "), "{got}");
    assert_eq!(body(&got)["device"], "nuc");
    assert_eq!(stdout(&out), "device: nuc\n");
}

#[test]
fn mini_and_macbook_send_their_machine_names() {
    for (name, machine) in [("mini", "mac-mini-m2"), ("macbook", "macbook-m2")] {
        let w = world(&format!("use-{name}"));
        let hub = local_hub(&w, "200", &format!(r#"{{"device":"{machine}","at":5}}"#));
        let out = device(&w, &["use", name]);
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        let got = hub.join().unwrap().expect("the local hub was told");
        assert_eq!(body(&got)["device"], machine);
        assert_eq!(stdout(&out), format!("device: {machine}\n"));
    }
}

#[test]
fn bare_device_reads_the_choice() {
    let w = world("use-get");
    let hub = local_hub(&w, "200", r#"{"device":"nuc","at":5}"#);
    let out = device(&w, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let got = hub.join().unwrap().expect("the local hub was asked");
    assert!(got.starts_with("GET /device/choice "), "{got}");
    assert_eq!(stdout(&out), "device: nuc\n");
}

#[test]
fn a_wrong_name_or_extra_argument_is_usage_and_sends_nothing() {
    for args in [
        &["use", "phone"][..],
        &["use"],
        &["use", "nuc", "extra"],
        &["use", "mac-mini-m2"],
    ] {
        let w = world("use-usage");
        let hub = local_hub(&w, "200", r#"{"device":"nuc","at":5}"#);
        let out = device(&w, args);
        assert_eq!(out.status.code(), Some(2), "{args:?} {out:?}");
        assert_eq!(
            stderr(&out),
            "usage: workflow device use mini|nuc|macbook\n"
        );
        assert_eq!(hub.join().unwrap(), None, "{args:?}");
    }
}

#[test]
fn a_refusal_fails() {
    let w = world("use-500");
    let hub = local_hub(&w, "500", "broken");
    let out = device(&w, &["use", "nuc"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    hub.join().unwrap();
    assert!(
        stderr(&out).starts_with("workflow device: "),
        "{}",
        stderr(&out)
    );
}
