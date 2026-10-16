//! `workflow route` through the real binary, with a script in the place of
//! curl (`$WORKFLOW_CURL`) that records its argv and body and answers with
//! the body in `reply` and the code in `status`.

#[path = "../src/scratch.rs"]
mod scratch;

use std::process::Output;
use std::sync::RwLock;

use scratch::Scratch;
use serde_json::json;

const HUB: &str = "https://dulali.taila27604.ts.net";

/// A child forked while another test's fixture is still open for writing
/// holds that file open, and its exec then fails with "Text file busy". So
/// a fixture is written under the write lock and every spawn waits for it.
static FORKS: RwLock<()> = RwLock::new(());

fn world(tag: &str, status: &str, reply: &str) -> Scratch {
    let work = Scratch::new(tag);
    let _writing = FORKS.write().unwrap_or_else(|e| e.into_inner());
    work.script(
        "curl",
        r#"
dir=$(dirname "$0")
for a in "$@"; do printf '%s\n' "$a"; done >"$dir/curl.argv"
cat >"$dir/curl.stdin"
cat "$dir/reply"
printf '\n%s' "$(cat "$dir/status")"
"#,
    );
    work.put("reply", reply);
    work.put("status", status);
    work
}

fn paired(work: &Scratch) {
    work.put("state/workflow/agent-token", "tok123");
    work.put("state/workflow/agent-hub", HUB);
    work.mkdir("run");
}

fn route(work: &Scratch, args: &[&str]) -> Output {
    let _forking = FORKS.read().unwrap_or_else(|e| e.into_inner());
    std::process::Command::new(env!("CARGO_BIN_EXE_workflow"))
        .arg("route")
        .args(args)
        .env("HOME", work.at("home"))
        .env("XDG_STATE_HOME", work.at("state"))
        .env("XDG_RUNTIME_DIR", work.at("run"))
        .env("WORKFLOW_CURL", work.at("curl"))
        .env("WORKFLOW_MACHINE", "mini")
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn route_device_posts_this_machine_and_writes_the_file() {
    let w = world(
        "route-device",
        "200",
        r#"{"route":"device","machine":"mini","waiting":0}"#,
    );
    paired(&w);
    let out = route(&w, &["device"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(stdout(&out), "route: device (mini)\n");
    let argv = w.read("curl.argv");
    let argv: Vec<&str> = argv.lines().collect();
    assert!(argv.contains(&"Authorization: Bearer tok123"), "{argv:?}");
    assert!(argv.contains(&"Content-Type: application/json"), "{argv:?}");
    assert_eq!(argv.last(), Some(&format!("{HUB}/api/route").as_str()));
    let sent: serde_json::Value = serde_json::from_str(&w.read("curl.stdin")).unwrap();
    assert_eq!(sent, json!({"route":"device","machine":"mini"}));
    let file: serde_json::Value = serde_json::from_str(&w.read("run/workflow-route.json")).unwrap();
    assert_eq!(
        file,
        json!({"route":"device","machine":"mini","waiting":0,"hub":HUB,"self":"mini"})
    );
}

#[test]
fn route_phone_posts_only_the_route() {
    let w = world(
        "route-phone",
        "200",
        r#"{"route":"phone","machine":null,"waiting":0}"#,
    );
    paired(&w);
    let out = route(&w, &["phone"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(stdout(&out), "route: phone\n");
    let sent: serde_json::Value = serde_json::from_str(&w.read("curl.stdin")).unwrap();
    assert_eq!(sent, json!({"route":"phone"}));
}

#[test]
fn route_alone_reads_the_hub() {
    let w = world(
        "route-get",
        "200",
        r#"{"route":"device","machine":"mac-mini-m2","waiting":1}"#,
    );
    paired(&w);
    let out = route(&w, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(stdout(&out), "route: device (mac-mini-m2)\n");
    let argv = w.read("curl.argv");
    assert!(!argv.contains("POST"), "{argv}");
    assert_eq!(w.read("curl.stdin"), "");
}

#[test]
fn a_409_prints_the_hubs_error_and_fails() {
    let w = world("route-409", "409", r#"{"error":"mini is offline"}"#);
    paired(&w);
    let out = route(&w, &["device"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("mini is offline"),
        "{out:?}"
    );
    assert!(!w.has("run/workflow-route.json"));
}

#[test]
fn a_401_says_to_pair_again() {
    let w = world("route-401", "401", "");
    paired(&w);
    let out = route(&w, &["phone"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("workflow agent pair <link>"),
        "{out:?}"
    );
}

#[test]
fn route_before_pairing_exits_2() {
    let w = world("route-unpaired", "200", "{}");
    let out = route(&w, &["phone"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(!w.has("curl.argv"));
}

#[test]
fn route_dulali_posts_only_the_route_and_writes_the_file() {
    let w = world(
        "route-dulali",
        "200",
        r#"{"route":"dulali","machine":null,"waiting":0}"#,
    );
    paired(&w);
    let out = route(&w, &["dulali"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(stdout(&out), "route: dulali\n");
    let sent: serde_json::Value = serde_json::from_str(&w.read("curl.stdin")).unwrap();
    assert_eq!(sent, json!({"route":"dulali"}));
    let file: serde_json::Value = serde_json::from_str(&w.read("run/workflow-route.json")).unwrap();
    assert_eq!(file["route"], "dulali");
}

#[test]
fn an_unknown_route_names_all_three_choices() {
    let w = world("route-bogus", "200", "");
    paired(&w);
    let out = route(&w, &["bogus"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("phone, device, dulali"), "{err}");
}

/// A local hub on this machine's socket that takes one request and keeps it.
fn local_hub(work: &Scratch) -> std::os::unix::net::UnixListener {
    work.mkdir("state/hub");
    let listener = std::os::unix::net::UnixListener::bind(work.at("state/hub/local.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    listener
}

/// The request the local hub got, if any, answering it 200.
fn taken(listener: &std::os::unix::net::UnixListener) -> Option<String> {
    use std::io::{Read, Write};
    let (mut stream, _) = listener.accept().ok()?;
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();
    let mut got = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = stream.read(&mut buf).unwrap_or(0);
        got.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&got).to_string();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let want = head
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                })
                .unwrap_or(0);
            if body.len() >= want || n == 0 {
                break;
            }
        }
        if n == 0 {
            break;
        }
    }
    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
    Some(String::from_utf8_lossy(&got).to_string())
}

#[test]
fn route_device_and_phone_also_set_this_machines_local_hub() {
    for (kind, reply) in [
        (
            "device",
            r#"{"route":"device","machine":"mini","waiting":0}"#,
        ),
        ("phone", r#"{"route":"phone","machine":null,"waiting":0}"#),
    ] {
        let w = world(&format!("route-local-{kind}"), "200", reply);
        paired(&w);
        let hub = local_hub(&w);
        let asking = {
            let hub = hub.try_clone().unwrap();
            std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                loop {
                    if let Some(got) = taken(&hub) {
                        return Some(got);
                    }
                    if std::time::Instant::now() > deadline {
                        return None;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            })
        };
        let out = route(&w, &[kind]);
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        let got = asking.join().unwrap().expect("the local hub was told");
        assert!(got.starts_with("POST /route "), "{got}");
        let body = got.split_once("\r\n\r\n").unwrap().1;
        let sent: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(sent, json!({ "route": kind }));
    }
}

#[test]
fn route_alone_leaves_the_local_hub_alone() {
    let w = world(
        "route-local-get",
        "200",
        r#"{"route":"phone","machine":null,"waiting":0}"#,
    );
    paired(&w);
    let hub = local_hub(&w);
    let out = route(&w, &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(taken(&hub).is_none());
}
