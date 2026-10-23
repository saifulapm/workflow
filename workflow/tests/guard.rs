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

fn guard(cwd: &std::path::Path, tool: &str, command: &str) -> Output {
    let input = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": tool,
        "tool_input": { "command": command },
        "cwd": cwd,
    });
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_workflow"))
        .args(["guard", "pnpm"])
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
