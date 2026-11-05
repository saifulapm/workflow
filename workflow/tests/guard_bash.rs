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

/// Exit 2 with `says` in the reason, which fits on two lines and says the
/// refusal is meant. The same reason goes out as a deny decision on stdout:
/// Claude Code then shows it without the hook's command in front, and the
/// exit 2 still blocks a Claude Code that cannot read the decision.
fn refused(out: &Output, says: &str) -> bool {
    let reason = String::from_utf8_lossy(&out.stderr);
    let decision: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    let output = &decision["hookSpecificOutput"];
    out.status.code() == Some(2)
        && reason.starts_with("workflow guard refused this on purpose: ")
        && reason.contains(says)
        && reason.trim_end().lines().count() <= 2
        && output["hookEventName"] == "PreToolUse"
        && output["permissionDecision"] == "deny"
        && output["permissionDecisionReason"] == reason.trim_end()
}

fn passes(out: &Output) -> bool {
    out.status.code() == Some(0) && out.stderr.is_empty() && out.stdout.is_empty()
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
        "ruff check . && git commit -am x",
        "biome ci && git commit -am x",
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
        // A lookup and a formatter run no check.
        "command -v vitest >/dev/null && git commit -am x",
        "ruff format . && git commit -am fmt",
        "npx biome format --write . && git commit -am fmt",
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
        // A `)` inside backticks closes no subshell bash would open.
        "echo `)`",
        "echo `)` (cd /repo/.claude/worktrees/a && ls",
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
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_workflow"))
        .args(["guard", "bash"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"not json").unwrap();
    let out = child.wait_with_output().unwrap();
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

#[test]
fn a_top_level_cd_into_an_agent_worktree_is_refused() {
    let repo = Path::new("/repo");
    for command in [
        "cd /repo/.claude/worktrees/agent-a1 && node_modules/.bin/vitest run",
        "cd .claude/worktrees/agent-a1",
        r#"cd "/repo/.claude/worktrees/agent-a1/apps/web"; ls"#,
        "pushd /repo/.claude/worktrees/agent-a1",
        "git status && cd /repo/.claude/worktrees/agent-a1",
        "cd /repo && cd .claude/worktrees/agent-a1",
    ] {
        let out = guard(repo, command);
        assert!(
            refused(&out, "(cd <worktree> && <command>)"),
            "{command:?}: {out:?}"
        );
        assert!(refused(&out, "git -C"), "{command:?}: {out:?}");
    }
}

#[test]
fn a_subshell_cd_and_a_cd_inside_the_own_worktree_run() {
    let repo = Path::new("/repo");
    for command in [
        "(cd /repo/.claude/worktrees/agent-a1 && node_modules/.bin/vitest run)",
        "echo $(cd /repo/.claude/worktrees/agent-a1 && pwd)",
        "cd apps/web && node_modules/.bin/vitest run",
        "git -C /repo/.claude/worktrees/agent-a1 log -1",
        r#"cd "$w""#,
        "cd -",
        "cd",
        "cd /repo/.claude",
    ] {
        let out = guard(repo, command);
        assert!(passes(&out), "{command:?}: {out:?}");
    }
    // An implementer moving inside its own worktree is never refused.
    let own = Path::new("/repo/.claude/worktrees/agent-a1");
    for command in [
        "cd /repo/.claude/worktrees/agent-a1/apps/web && node_modules/.bin/vitest run",
        "cd apps/web",
        "cd /repo/.claude/worktrees/agent-a1",
    ] {
        let out = guard(own, command);
        assert!(passes(&out), "{command:?}: {out:?}");
    }
    let nested = Path::new("/repo/.claude/worktrees/agent-a1/apps/web");
    let out = guard(nested, "cd ../../packages/ui");
    assert!(passes(&out), "{out:?}");
}

#[test]
fn a_cd_from_one_agent_worktree_into_another_is_refused() {
    let own = Path::new("/repo/.claude/worktrees/agent-a1");
    let out = guard(own, "cd /repo/.claude/worktrees/agent-b2");
    assert!(refused(&out, "git -C"), "{out:?}");
}
