//! `workflow guard bash` through the real binary, fed the JSON a Claude Code
//! PreToolUse hook gets on stdin. Each rule's lesson command is refused with
//! its safe form named, and the safe form runs.

#[path = "../src/scratch.rs"]
mod scratch;

use std::io::Write;
use std::path::Path;
use std::process::{Output, Stdio};

use scratch::Scratch;
use serde_json::json;

/// HOME is `cwd`, which holds no ~/.dotfiles unless a test puts one there.
fn guard(cwd: &Path, command: &str) -> Output {
    guard_home(cwd, cwd, command)
}

fn guard_home(cwd: &Path, home: &Path, command: &str) -> Output {
    let input = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": command },
        "cwd": cwd,
    });
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_workflow"))
        .args(["guard", "bash"])
        .env("HOME", home)
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

/// Exit 2 with `says` in the reason, which fits on two lines.
fn refused(out: &Output, says: &str) -> bool {
    let reason = String::from_utf8_lossy(&out.stderr);
    out.status.code() == Some(2) && reason.contains(says) && reason.trim_end().lines().count() <= 2
}

fn passes(out: &Output) -> bool {
    out.status.code() == Some(0) && out.stderr.is_empty()
}

#[test]
fn a_commit_or_deploy_chained_after_a_test_or_lint_is_refused() {
    let w = Scratch::new("guard-bash-chain");
    for command in [
        "node_modules/.bin/vitest run; git commit -am x",
        "cargo test && git commit -am x",
        "oxlint src | grep error; git commit -am x",
        "node_modules/.bin/vitest run && pnpm exec wrangler deploy",
        "vitest run\npnpm exec wrangler deploy",
        // A command split over lines is one command line.
        "cargo test \\\n  && git commit -am x",
        "cargo test &&\ngit commit -am x",
        "timeout 600 pnpm test; git -C app commit -m x",
        "(cd apps/web && node_modules/.bin/tsc -b) && git commit -am x",
        "cargo clippy -- -D warnings && npx wrangler deploy",
        "pnpm run lint && pnpm run deploy",
        // The commit after a heredoc's body still runs.
        "cat <<'EOF' >notes\ngit commit -am x\nEOF\ncargo test; git commit -am x",
    ] {
        let out = guard(w.path(), command);
        assert!(
            refused(&out, "tests in one call, commit in the next"),
            "{command:?}: {out:?}"
        );
    }
}

#[test]
fn a_command_after_a_piped_git_merge_is_refused() {
    let w = Scratch::new("guard-bash-merge");
    for command in [
        "git merge --ff-only b 2>&1 | tail -1 && git worktree remove w",
        "git merge --ff-only b | grep -v Updating; git branch -d b",
    ] {
        let out = guard(w.path(), command);
        assert!(
            refused(&out, "git merge in one call"),
            "{command:?}: {out:?}"
        );
    }
}

#[test]
fn a_test_a_commit_or_a_merge_alone_runs() {
    let w = Scratch::new("guard-bash-alone");
    for command in [
        "git commit -am x",
        "cargo test",
        "cargo test -q --manifest-path workflow/Cargo.toml --test guard 2>&1 | tail -5",
        r#"git commit -m "fix vitest run""#,
        r#"git commit -m "make vitest run faster""#,
        "git commit -m 'cargo test && git commit'",
        "git commit -F - <<'EOF'\nKeep the cart\n\ncargo test; git commit is fine to say\nEOF",
        "git commit -am x && cargo test",
        "git merge --ff-only b && git worktree remove w",
        "git merge --ff-only b 2>&1 | tail -1",
        "git add -A && git commit -m x",
        "pnpm exec wrangler deploy",
        "echo vitest; echo commit",
    ] {
        let out = guard(w.path(), command);
        assert!(passes(&out), "{command:?}: {out:?}");
    }
}

#[test]
fn input_the_guard_cannot_read_passes() {
    let w = Scratch::new("guard-bash-unread");
    // An unclosed quote or subshell is no command line the guard can read.
    for command in [
        "cargo test; git commit -m \"x",
        "cargo test; git commit -am x)",
        "(cargo test; git commit -am x",
    ] {
        let out = guard(w.path(), command);
        assert!(passes(&out), "{command:?}: {out:?}");
    }
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_workflow"))
        .args(["guard", "bash"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(passes(&out), "{out:?}");
}

#[test]
fn guard_help_lists_bash_among_its_checks() {
    // The hook entry runs `workflow guard bash` only when this lists it, so
    // a workflow too old to know it passes instead of failing every call.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_workflow"))
        .args(["guard", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(
        help.lines()
            .any(|l| l.contains("possible values:") && l.contains("bash")),
        "{help}"
    );
}

#[test]
fn pkill_and_pgrep_by_full_command_line_are_refused() {
    let w = Scratch::new("guard-bash-pkill");
    for command in [
        r#"pkill -f "wrangler tail eventsmith""#,
        "pgrep -f vitest",
        "pkill -9 -f vitest",
        "pgrep -af 'cf dev'",
        "pkill --full vitest",
        "sudo pkill -f vitest",
    ] {
        let out = guard(w.path(), command);
        assert!(
            refused(&out, "kill -TERM -- -<pgid>"),
            "{command:?}: {out:?}"
        );
        assert!(
            refused(&out, "ss -ltnp | grep :<port>"),
            "{command:?}: {out:?}"
        );
    }
}

#[test]
fn pkill_by_name_and_kill_by_group_run() {
    let w = Scratch::new("guard-bash-kill");
    for command in [
        "pkill vitest",
        "kill -TERM -- -1234",
        "pgrep -l vitest",
        "ss -ltnp | grep :8787",
        r#"git commit -m "stop using pkill -f""#,
    ] {
        let out = guard(w.path(), command);
        assert!(passes(&out), "{command:?}: {out:?}");
    }
}

#[test]
fn a_bare_playwright_cli_attach_is_refused_where_the_wrapper_exists() {
    let w = Scratch::new("guard-bash-attach");
    w.put(".dotfiles/bin/playwright-cli", "#!/bin/sh\n");
    for command in [
        "playwright-cli attach --extension=chromium",
        "pnpm exec playwright-cli attach --extension=chromium",
        "PATH=$HOME/.local/share/pnpm/bin:$PATH playwright-cli attach --extension=chromium",
    ] {
        let out = guard(w.path(), command);
        assert!(
            refused(&out, "~/.dotfiles/bin/playwright-cli attach"),
            "{command:?}: {out:?}"
        );
    }
    for command in [
        "~/.dotfiles/bin/playwright-cli attach --extension=chromium",
        "$HOME/.dotfiles/bin/playwright-cli attach --extension=chromium",
        "playwright-cli snapshot",
        "playwright-cli open https://example.com",
    ] {
        let out = guard(w.path(), command);
        assert!(passes(&out), "{command:?}: {out:?}");
    }
}

#[test]
fn a_bare_playwright_cli_attach_runs_where_there_is_no_wrapper() {
    // A cloud session has no ~/.dotfiles.
    let w = Scratch::new("guard-bash-attach-cloud");
    let out = guard(w.path(), "playwright-cli attach --extension=chromium");
    assert!(passes(&out), "{out:?}");
}
