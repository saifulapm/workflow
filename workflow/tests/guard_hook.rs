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

/// The command of the one PreToolUse entry that matches the edit tools.
fn edit_entry() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../cloud/settings-hooks.json");
    let hooks: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let entries: Vec<&Value> = hooks["PreToolUse"]
        .as_array()
        .expect("a PreToolUse list")
        .iter()
        .filter(|e| e["matcher"] == "Edit|Write|NotebookEdit")
        .collect();
    assert_eq!(entries.len(), 1, "{hooks}");
    entries[0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn the_edit_entry_refuses_an_edit_while_the_suite_runs_and_passes_an_old_workflow() {
    let w = Scratch::new("guard-hook-edit");
    let repo = w.mkdir("repo");
    let init = Command::new("git")
        .args(["-c", "core.hooksPath=/dev/null", "init", "-q"])
        .current_dir(&repo)
        .status();
    assert!(init.unwrap().success());
    let mut suite = Command::new("sh")
        .args(["-c", "sleep 30; : cargo test -q"])
        .current_dir(&repo)
        .spawn()
        .unwrap();
    let input = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Edit",
        "tool_input": { "file_path": repo.join("src/lib.rs") },
        "cwd": repo,
    })
    .to_string();
    let run = |bin: &Path| {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", &edit_entry()])
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("HOME", "/nonexistent");
        feed(command, &input)
    };
    let bin = w.mkdir("bin");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_workflow"), bin.join("workflow")).unwrap();
    let real = run(&bin);
    // A workflow from before guard edit: its help lists pnpm and bash.
    w.script(
        "old/workflow",
        "if [ \"$1 $2\" = 'guard --help' ]; then\n\
           echo '  <CHECK>  [possible values: pnpm, bash]'; exit 0\n\
         fi\n\
         echo \"error: invalid value '$2'\" >&2; exit 2",
    );
    let old = run(&w.at("old"));
    let _ = suite.kill();
    let _ = suite.wait();
    assert_eq!(real.status.code(), Some(2), "{real:?}");
    assert!(String::from_utf8_lossy(&real.stderr).contains("suite runs"));
    assert_eq!(old.status.code(), Some(0), "{old:?}");
}

fn guard_agent(subagent: &str, prompt: &str) -> Output {
    let input = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Agent",
        "tool_input": {
            "description": "t3 build",
            "subagent_type": subagent,
            "prompt": prompt,
        },
        "cwd": "/repo",
    })
    .to_string();
    let mut command = Command::new(env!("CARGO_BIN_EXE_workflow"));
    command.args(["guard", "agent"]);
    feed(command, &input)
}

fn brief(verify: &str) -> String {
    format!(
        "GOAL        a scheduled message keeps its send time\n\
         SCOPE       apps/web/app/\n\
         ACCEPTANCE  a test that stores one and reads it back\n\
         VERIFY\n{verify}\n\
         STOP        a schema change sends it back\n"
    )
}

/// palmshop m19 t3's VERIFY named no oxlint, and its commits brought four
/// lint errors the trunk then fixed, as a brief had on 2026-11-02.
#[test]
fn an_implementer_brief_whose_verify_names_no_lint_is_refused() {
    let refused = guard_agent(
        "implementer",
        &brief("node_modules/.bin/vitest run app/send.test.ts\nnode_modules/.bin/tsc --noEmit"),
    );
    assert_eq!(refused.status.code(), Some(2), "{refused:?}");
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("lint"),
        "{refused:?}"
    );
    for verify in [
        "cargo test -q --test land\ncargo clippy -q --all-targets -- -D warnings",
        "node_modules/.bin/vitest run\nnode_modules/.bin/oxlint app\nnode_modules/.bin/oxfmt --check app",
        "cd apps/web && pnpm lint",
    ] {
        let out = guard_agent("implementer", &brief(verify));
        assert_eq!(out.status.code(), Some(0), "{verify}: {out:?}");
    }
    // A lint named outside VERIFY does not count, nor does a brief with no VERIFY.
    let elsewhere = guard_agent(
        "implementer",
        &format!(
            "CONTEXT     oxlint.config.ts holds the rules\n{}",
            brief("vitest run")
        ),
    );
    assert_eq!(elsewhere.status.code(), Some(2), "{elsewhere:?}");
    let none = guard_agent("implementer", "GOAL  fix the typo in the README");
    assert_eq!(none.status.code(), Some(2), "{none:?}");
    // Any other agent's prompt is no brief.
    let scout = guard_agent("scout", "find every use of send_at");
    assert_eq!(scout.status.code(), Some(0), "{scout:?}");
}

/// The cloud kit's PreToolUse entry on the Agent tool runs `workflow guard
/// agent`, and passes every call with a workflow from before it.
#[test]
fn the_agent_entry_refuses_a_brief_without_lint_and_passes_an_old_workflow() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../cloud/settings-hooks.json");
    let hooks: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let entries: Vec<&Value> = hooks["PreToolUse"]
        .as_array()
        .expect("a PreToolUse list")
        .iter()
        .filter(|e| e["matcher"] == "Agent")
        .collect();
    assert_eq!(entries.len(), 1, "{hooks}");
    let entry = entries[0]["hooks"][0]["command"].as_str().unwrap();
    let input = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Agent",
        "tool_input": { "subagent_type": "implementer", "prompt": brief("vitest run") },
        "cwd": "/repo",
    })
    .to_string();
    let w = Scratch::new("guard-hook-agent");
    let run = |bin: &Path| {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", entry])
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("HOME", "/nonexistent");
        feed(command, &input)
    };
    let bin = w.mkdir("bin");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_workflow"), bin.join("workflow")).unwrap();
    let real = run(&bin);
    assert_eq!(real.status.code(), Some(2), "{real:?}");
    w.script(
        "old/workflow",
        "if [ \"$1 $2\" = 'guard --help' ]; then\n\
           echo '  <CHECK>  [possible values: pnpm, bash, edit]'; exit 0\n\
         fi\n\
         echo \"error: invalid value '$2'\" >&2; exit 2",
    );
    let old = run(&w.at("old"));
    assert_eq!(old.status.code(), Some(0), "{old:?}");
}
