//! `workflow cloud-switch` through the real binary, with scripts in the place
//! of mem (`$WORKFLOW_MEM`) and pass.

#[path = "../src/scratch.rs"]
mod scratch;

use std::process::{Command, Output};

use scratch::Scratch;

const STORE: &str = "wss://example.test/store/share";
const SECRET: &str = "SECRET-MARKER-9f3a";

fn now_minus(hours: i64) -> String {
    let out = Command::new("date")
        .args([
            "-u",
            "-d",
            &format!("{hours} hours ago"),
            "+%Y-%m-%dT%H:%M:%SZ",
        ])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// One project `gamma` on milestone `m2-next`. `extra` is more row JSON (a
/// cloud runner, a handoff time); `handoff` the text mem answers. A fake
/// pass on PATH logs any call and prints the secret marker.
fn world(tag: &str, extra: &str, handoff: Option<&str>) -> Scratch {
    let work = Scratch::new(tag);
    work.put(
        "projects.json",
        &format!(r#"{{"projects":[{{"name":"gamma","milestone":"m2-next"{extra}}}]}}"#),
    );
    if let Some(text) = handoff {
        work.put("handoff-gamma.md", text);
    }
    work.script(
        "mem",
        r#"
dir=$(dirname "$0")
case "$1" in
projects) cat "$dir/projects.json" ;;
handoff) [ -f "$dir/handoff-gamma.md" ] && cat "$dir/handoff-gamma.md" || exit 1 ;;
esac
"#,
    );
    let log = work.at("pass.log");
    work.script(
        "bin/pass",
        &format!(r#"echo "$*" >>"{}"; echo {SECRET}"#, log.display()),
    );
    work
}

fn cloud_runner(hours: i64) -> String {
    format!(
        r#","runner":{{"machine":"cloud","id":"session_01Abc","since":"{}","milestone":"m2-next"}}"#,
        now_minus(hours)
    )
}

fn switch(work: &Scratch, args: &[&str]) -> Output {
    let path = format!(
        "{}:{}",
        work.at("bin").display(),
        std::env::var("PATH").unwrap()
    );
    Command::new(env!("CARGO_BIN_EXE_workflow"))
        .arg("cloud-switch")
        .args(args)
        .env("WORKFLOW_MEM", work.at("mem"))
        .env("PATH", path)
        .current_dir(work.path())
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

#[test]
fn a_live_cloud_run_refuses_and_is_named() {
    let w = world("cs-live", &cloud_runner(1), None);
    let out = switch(&w, &["--store", STORE]);
    assert_eq!(out.status.code(), Some(2));
    let said = text(&out.stderr);
    assert!(said.contains("gamma"), "{said}");
    assert!(said.contains("m2-next"), "{said}");
    assert!(said.contains("session_01Abc"), "{said}");
    assert_eq!(text(&out.stdout), "");
}

#[test]
fn a_parked_cloud_run_refuses_too() {
    let extra = format!(r#"{},"handoff_at":"{}""#, cloud_runner(3), now_minus(1));
    let w = world("cs-parked", &extra, Some("parked: 6-hour stop\n"));
    let out = switch(&w, &["--store", STORE]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("session_01Abc"));
}

#[test]
fn with_no_running_cloud_run_it_prints_the_steps_and_no_secret() {
    // An ended run (8 hours, no handoff) does not count.
    let w = world("cs-steps", &cloud_runner(8), None);
    let out = switch(&w, &["--store", STORE]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let said = text(&out.stdout);
    for want in [
        "HUB_STORE_URL",
        STORE,
        "among the allowed hosts: example.test",
        "HUB_STORE_NAME",
        "main",
        "HUB_STORE_TOKEN",
        "hub share mint 'session_*' 'cloud-*'",
        "pass insert -m workflow/hub-store-cloud",
        "pass show workflow/hub-store-cloud",
        "HUB_CLOUD_TOKEN",
        "pass show workflow/hub-cloud-token",
        "HUB_PEER_TOKEN_SHA256",
        "pass show workflow/hub-peer-token | head -n1 | tr -d '\\n' | sha256sum",
        "HUB_PEER_TOKEN",
        "MEM_REMOTE",
        "workflow cloud-setup",
    ] {
        assert!(said.contains(want), "{want}\n{said}");
    }
    assert!(
        !said.contains("HUB_STORE_TOKEN to the output of: pass show workflow/hubweb-store-token"),
        "{said}"
    );
    assert!(!said.contains(SECRET));
    assert!(!text(&out.stderr).contains(SECRET));
    assert!(!w.has("pass.log"), "pass ran");
}

#[test]
fn a_bad_store_url_refuses_before_anything() {
    let w = world("cs-bad", &cloud_runner(1), None);
    for url in [
        "https://example.test/store/share",
        "wss://example.test/store",
        "wss:///store/share",
    ] {
        let out = switch(&w, &["--store", url]);
        assert_eq!(out.status.code(), Some(2), "{url}");
        assert!(text(&out.stderr).contains("--store"), "{url}");
        assert!(!text(&out.stderr).contains("session_01Abc"), "{url}");
    }
}
