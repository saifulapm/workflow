//! `workflow route` through the real binary, with a script in the place of
//! curl (`$WORKFLOW_CURL`) that records its argv and body and answers with
//! the body in `reply` and the code in `status`.

#[path = "../src/scratch.rs"]
mod scratch;

use std::process::Output;

use scratch::Scratch;
use serde_json::json;

const HUB: &str = "https://dulali.taila27604.ts.net";

fn world(tag: &str, status: &str, reply: &str) -> Scratch {
    let work = Scratch::new(tag);
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
        json!({"route":"device","machine":"mini","waiting":0,"hub":HUB})
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
