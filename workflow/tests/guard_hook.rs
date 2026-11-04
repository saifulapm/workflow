//! The hook entry every session runs before a Bash or Monitor call, as
//! `cloud/settings-hooks.json` writes it, and the input a Monitor call hands
//! it. `fixtures/monitor-pretooluse.json` is a real Monitor call's PreToolUse
//! input (Claude Code 2.1.316, `echo monitor-probe`), its ids and folders
//! scrubbed: Claude Code names it a Bash call.

#[path = "../src/scratch.rs"]
mod scratch;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use scratch::Scratch;
use serde_json::Value;

fn monitor_input(command: &str) -> String {
    let text = include_str!("fixtures/monitor-pretooluse.json");
    let mut input: Value = serde_json::from_str(text).unwrap();
    input["tool_input"]["command"] = command.into();
    input.to_string()
}

fn feed(mut command: Command, input: &str) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn guard_bash(input: &str) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_workflow"));
    command.args(["guard", "bash"]).env("HOME", "/nonexistent");
    feed(command, input)
}

#[test]
fn a_monitor_call_is_refused_with_the_bash_message() {
    let bash = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": "pkill -f x" },
        "cwd": "/repo",
    })
    .to_string();
    let from_bash = guard_bash(&bash);
    let from_monitor = guard_bash(&monitor_input("pkill -f x"));
    assert_eq!(from_monitor.status.code(), Some(2), "{from_monitor:?}");
    assert!(!from_monitor.stderr.is_empty());
    assert_eq!(from_monitor.stderr, from_bash.stderr);
    let probe = guard_bash(&monitor_input("echo monitor-probe"));
    assert_eq!(probe.status.code(), Some(0), "{probe:?}");
}

/// The command of the one PreToolUse entry that matches Bash.
fn hook_entry() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../cloud/settings-hooks.json");
    let hooks: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let entries: Vec<&Value> = hooks["PreToolUse"]
        .as_array()
        .expect("a PreToolUse list")
        .iter()
        .filter(|e| e["matcher"] == "Bash")
        .collect();
    assert_eq!(entries.len(), 1, "{hooks}");
    entries[0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The entry run by sh with `bin` first on a PATH that has no workflow of
/// its own.
fn run_entry(bin: &Path, input: &str) -> Output {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", &hook_entry()])
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("HOME", "/nonexistent");
    feed(command, input)
}

#[test]
fn the_hook_entry_runs_the_guard() {
    let w = Scratch::new("guard-hook-real");
    let bin = w.mkdir("bin");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_workflow"), bin.join("workflow")).unwrap();
    let out = run_entry(&bin, &monitor_input("pkill -f x"));
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("kill -TERM"));
    let out = run_entry(&bin, &monitor_input("git status"));
    assert_eq!(out.status.code(), Some(0), "{out:?}");
}

#[test]
fn the_hook_entry_lets_every_call_run_with_no_workflow_or_an_old_one() {
    assert!(
        !Path::new("/usr/bin/workflow").exists() && !Path::new("/bin/workflow").exists(),
        "the test's PATH must hold no workflow"
    );
    let w = Scratch::new("guard-hook-missing");
    let empty = w.mkdir("empty");
    let out = run_entry(&empty, &monitor_input("pkill -f x"));
    assert_eq!(out.status.code(), Some(0), "{out:?}");

    // A workflow from before guard bash: its help lists pnpm alone, and clap
    // refuses `guard bash` with exit 2, which would block every call.
    let old = w.mkdir("old");
    w.script(
        "old/workflow",
        "if [ \"$1 $2\" = 'guard --help' ]; then\n\
           echo '  <CHECK>  pnpm: refuse pnpm [possible values: pnpm]'; exit 0\n\
         fi\n\
         echo \"error: invalid value '$2'\" >&2; exit 2",
    );
    let out = run_entry(&old, &monitor_input("pkill -f x"));
    assert_eq!(out.status.code(), Some(0), "{out:?}");
}
