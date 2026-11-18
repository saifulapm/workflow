//! `workflow guard pnpm` through the real binary, fed the JSON a Claude Code
//! PreToolUse hook gets on stdin. A linked worktree is a folder whose `.git`
//! is a file; its node_modules is the main checkout's when it is a symlink.

#[path = "../src/scratch.rs"]
mod scratch;

use std::io::Write;
use std::process::{Output, Stdio};

use scratch::Scratch;
use serde_json::json;

/// A main checkout with its own node_modules, and a linked worktree of it
/// whose node_modules links back to that one, as Claude Code makes them.
fn world(tag: &str) -> Scratch {
    let work = Scratch::new(tag);
    work.mkdir("main/.git");
    work.mkdir("main/node_modules/.bin");
    work.put("wt/.git", "gitdir: ../main/.git/worktrees/wt\n");
    work.mkdir("wt/apps/shop");
    std::os::unix::fs::symlink(work.at("main/node_modules"), work.at("wt/node_modules")).unwrap();
    work
}

/// mem is one that does not exist, so no test writes to the real store.
fn guard(cwd: &std::path::Path, tool: &str, command: &str) -> Output {
    run_guard(cwd, tool, command, std::path::Path::new("/nonexistent/mem"))
}

fn run_guard(cwd: &std::path::Path, tool: &str, command: &str, mem: &std::path::Path) -> Output {
    let input = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": tool,
        "tool_input": { "command": command },
        "cwd": cwd,
    });
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_workflow"))
        .args(["guard", "pnpm"])
        .env("WORKFLOW_MEM", mem)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

/// Exit 2 with the guard's own reason: clap's usage error exits 2 as well.
fn refused(out: &Output) -> bool {
    out.status.code() == Some(2)
        && String::from_utf8_lossy(&out.stderr).contains("node_modules/.bin/")
}

#[test]
fn pnpm_in_a_linked_worktree_over_a_linked_node_modules_is_refused() {
    let w = world("guard-refused");
    for command in [
        "pnpm install",
        "pnpm test",
        "cd apps/shop && pnpm vitest run",
        "FORCE_COLOR=0 pnpm i",
        "timeout 600 pnpm exec tsc -b",
        "/usr/bin/pnpm add zod",
        "npx pnpm i",
        "corepack pnpm install",
        "nohup pnpm i",
        "sudo pnpm i",
        "ls | xargs pnpm i",
        "{ pnpm i; }",
        "if true; then pnpm i; fi",
        "for d in a b; do pnpm i; done",
        "! pnpm i",
        // Backticks inside double quotes still run their command in bash.
        "git commit -m \"Verified against `pnpm dev`\"",
    ] {
        let out = guard(&w.at("wt"), "Bash", command);
        assert!(refused(&out), "{command}: {out:?}");
    }
}

#[test]
fn a_subfolder_of_the_worktree_is_guarded_too() {
    let w = world("guard-subfolder");
    assert!(refused(&guard(&w.at("wt/apps/shop"), "Bash", "pnpm test")));
}

#[test]
fn tools_by_path_and_pnpm_as_a_word_pass() {
    let w = world("guard-passes");
    for command in [
        "node_modules/.bin/vitest run app/cart.test.ts",
        "grep -n pnpm package.json",
        "git log --oneline -3",
        // pnpm inside quoted text is a word, not a command.
        r#"git commit -m "Fix the cart (pnpm test was red)""#,
        r#"git commit -m "Speed up CI; pnpm cache""#,
        "git commit -m \"Hold the bar\n\npnpm test passes again\"",
        r#"rg -n "(pnpm|npm) install" README.md"#,
        "echo 'a && pnpm i'",
        "git commit -F - <<'EOF'\nKeep the cart\n\npnpm test is green\nEOF",
    ] {
        let out = guard(&w.at("wt"), "Bash", command);
        assert_eq!(out.status.code(), Some(0), "{command}: {out:?}");
    }
}

#[test]
fn the_main_checkout_and_a_worktree_with_its_own_node_modules_pass() {
    let w = world("guard-own");
    assert_eq!(
        guard(&w.at("main"), "Bash", "pnpm install").status.code(),
        Some(0)
    );
    w.put("own/.git", "gitdir: ../main/.git/worktrees/own\n");
    w.mkdir("own/node_modules");
    assert_eq!(
        guard(&w.at("own"), "Bash", "pnpm install").status.code(),
        Some(0)
    );
}

#[test]
fn other_tools_and_unreadable_input_pass() {
    let w = world("guard-other");
    assert_eq!(
        guard(&w.at("wt"), "Read", "pnpm install").status.code(),
        Some(0)
    );
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_workflow"))
        .args(["guard", "pnpm"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
}

/// A mem that writes the folder it runs in and its words to mem.log in
/// `w`, then runs `then` (an exit code or a sleep).
fn fake_mem(w: &Scratch, then: &str) -> std::path::PathBuf {
    let log = w.at("mem.log");
    w.script(
        "bin/mem",
        &format!(
            "printf '%s|%s\\n' \"$(pwd -P)\" \"$*\" >>'{}'\n{then}",
            log.display()
        ),
    )
}

fn guard_with_mem(cwd: &std::path::Path, mem: &std::path::Path, command: &str) -> Output {
    run_guard(cwd, "Bash", command, mem)
}

#[test]
fn a_refusal_writes_one_line_to_the_project_of_the_cwd() {
    let w = world("guard-record");
    let mem = fake_mem(&w, "exit 0");
    assert!(refused(&guard_with_mem(&w.at("wt"), &mem, "pnpm test")));
    let wt = w.at("wt").canonicalize().unwrap();
    assert_eq!(
        w.read("mem.log"),
        format!("{}|log guard pnpm refused: pnpm test\n", wt.display())
    );
}

#[test]
fn a_refused_command_is_logged_masked_and_cut_to_120_characters() {
    let w = world("guard-record-cut");
    let mem = fake_mem(&w, "exit 0");
    let long = format!("pnpm test {}", "x".repeat(290));
    assert!(refused(&guard_with_mem(&w.at("wt"), &mem, &long)));
    // Built at run time, so the source holds no token shape.
    let token = format!(
        "GITHUB_TOKEN=ghp_{} pnpm i",
        "0123456789abcdefghijklmnopqrstuvwxyz"
    );
    assert!(refused(&guard_with_mem(&w.at("wt"), &mem, &token)));
    let log = w.read("mem.log");
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "{log}");
    assert!(
        lines[0].ends_with(&format!("|log guard pnpm refused: {}", &long[..120])),
        "{log}"
    );
    assert!(
        lines[1].ends_with("|log guard pnpm refused: GITHUB_TOKEN=ghp_*** pnpm i"),
        "{log}"
    );
    assert!(!log.contains("ghp_0123"), "{log}");
}

#[test]
fn an_allowed_command_writes_nothing() {
    let w = world("guard-record-allowed");
    let mem = fake_mem(&w, "exit 0");
    let out = guard_with_mem(&w.at("wt"), &mem, "node_modules/.bin/vitest run");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(!w.has("mem.log"));
}

#[test]
fn a_failing_or_slow_mem_never_changes_the_answer() {
    let w = world("guard-record-mem-fails");
    let failing = fake_mem(&w, "echo 'mem: store down' >&2; exit 1");
    let out = guard_with_mem(&w.at("wt"), &failing, "pnpm test");
    assert!(refused(&out), "{out:?}");
    assert!(!String::from_utf8_lossy(&out.stderr).contains("store down"));
    assert!(out.stdout.is_empty(), "{out:?}");

    let slow = fake_mem(&w, "sleep 10");
    let started = std::time::Instant::now();
    assert!(refused(&guard_with_mem(&w.at("wt"), &slow, "pnpm test")));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));

    let missing = w.at("no-such-mem");
    assert!(refused(&guard_with_mem(&w.at("wt"), &missing, "pnpm test")));
}
