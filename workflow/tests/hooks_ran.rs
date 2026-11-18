//! The SessionStart hooks of the cloud kit, as `cloud/settings-hooks.json`
//! holds them, run by sh with a fake mem first on PATH. A cloud session's
//! start writes `hooks ran: <session> in <project or none>` to workflow's
//! log, so mem shows whether a thread's hooks ran (#R5JGX4HC).

#[path = "../src/scratch.rs"]
mod scratch;

use std::path::Path;
use std::process::{Command, Output};

use scratch::Scratch;
use serde_json::Value;

/// Every SessionStart command, in order.
fn session_start() -> Vec<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../cloud/settings-hooks.json");
    let hooks: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    hooks["SessionStart"]
        .as_array()
        .expect("a SessionStart list")
        .iter()
        .flat_map(|e| e["hooks"].as_array().unwrap().iter())
        .map(|h| h["command"].as_str().unwrap().to_string())
        .collect()
}

/// A mem that writes each `log` call's words to mem.log, names the project
/// `cloud_walk` in a folder of that name and none elsewhere, and runs
/// `then` for every call.
fn fake_mem(w: &Scratch, then: &str) -> std::path::PathBuf {
    let log = w.at("mem.log");
    w.script(
        "bin/mem",
        &format!(
            "{then}\n\
             case \"$1\" in\n\
             log) shift; printf '%s\\n' \"$*\" >>'{log}' ;;\n\
             project) case \"$(pwd -P)\" in */cloud_walk) printf 'id    01X\\nname  cloud_walk\\n' ;; *) exit 1 ;; esac ;;\n\
             esac",
            log = log.display()
        ),
    );
    w.at("bin")
}

/// Each SessionStart command run by sh in `cwd`, the session id set when
/// given; the outputs in order.
fn start(bin: &Path, cwd: &Path, session: Option<&str>) -> Vec<Output> {
    session_start()
        .iter()
        .map(|command| {
            let mut sh = Command::new("/bin/sh");
            sh.args(["-c", command])
                .current_dir(cwd)
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .env("HOME", "/nonexistent")
                .env_remove("CLAUDE_CODE_REMOTE_SESSION_ID");
            if let Some(id) = session {
                sh.env("CLAUDE_CODE_REMOTE_SESSION_ID", id);
            }
            sh.output().unwrap()
        })
        .collect()
}

fn logged(w: &Scratch) -> Vec<String> {
    if !w.has("mem.log") {
        return Vec::new();
    }
    w.read("mem.log").lines().map(str::to_string).collect()
}

#[test]
fn a_cloud_session_start_names_its_session_and_no_project_outside_one() {
    let w = Scratch::new("hooks-ran-none");
    let bin = fake_mem(&w, "");
    let home = w.mkdir("home/user");
    // What a SessionStart hook prints enters the session's context.
    for out in start(&bin, &home, Some("cse_r1test")) {
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        assert!(out.stdout.is_empty(), "{out:?}");
    }
    assert_eq!(
        logged(&w),
        ["--project workflow hooks ran: cse_r1test in none"]
    );
}

#[test]
fn a_cloud_session_start_in_a_checkout_names_its_project() {
    let w = Scratch::new("hooks-ran-project");
    let bin = fake_mem(&w, "");
    let checkout = w.mkdir("cloud_walk");
    start(&bin, &checkout, Some("cse_r1test"));
    assert_eq!(
        logged(&w),
        ["--project workflow hooks ran: cse_r1test in cloud_walk"]
    );
}

#[test]
fn a_start_and_a_resume_write_a_line_each() {
    let w = Scratch::new("hooks-ran-twice");
    let bin = fake_mem(&w, "");
    let home = w.mkdir("home/user");
    start(&bin, &home, Some("cse_r1test"));
    start(&bin, &home, Some("cse_r1test"));
    let line = "--project workflow hooks ran: cse_r1test in none";
    assert_eq!(logged(&w), [line, line]);
}

#[test]
fn a_local_session_start_writes_no_line() {
    let w = Scratch::new("hooks-ran-local");
    let bin = fake_mem(&w, "");
    let home = w.mkdir("home/user");
    for out in start(&bin, &home, None) {
        assert_eq!(out.status.code(), Some(0), "{out:?}");
    }
    assert_eq!(logged(&w), Vec::<String>::new());
}

#[test]
fn a_failing_mem_still_lets_the_session_start() {
    let w = Scratch::new("hooks-ran-mem-fails");
    let bin = fake_mem(&w, "echo 'mem: store down' >&2; exit 1");
    let home = w.mkdir("home/user");
    for out in start(&bin, &home, Some("cse_r1test")) {
        assert_eq!(out.status.code(), Some(0), "{out:?}");
    }
}
