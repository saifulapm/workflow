//! `workflow guard bash` through the real binary, fed the JSON a Claude Code
//! PreToolUse hook gets on stdin. Each rule's lesson command is refused with
//! its safe form named, and the safe form runs.

#[path = "../src/scratch.rs"]
mod scratch;

use std::io::Write;
use std::path::{Path, PathBuf};
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
    // The scratch folder is under /tmp, where a cargo build is refused.
    let guard = |_: &Path, command: &str| guard_home(Path::new("/repo"), w.path(), command);
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
fn pkill_by_exact_name_and_killall_are_refused() {
    // A walker's cleanup, `pkill -x chromium-browser`, ran on the host and
    // would have closed Saiful's own chromium with the walk's.
    let w = Scratch::new("guard-bash-exact");
    for command in [
        "pkill -x chromium-browser",
        "pkill -9 -x niri",
        "pkill --exact chromium-browser",
        "killall chromium-browser",
        "sudo killall -9 niri",
    ] {
        let out = guard(w.path(), command);
        assert!(refused(&out, "by the pid"), "{command:?}: {out:?}");
        assert!(
            refused(&out, "kill -TERM -- -<pgid>"),
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
        "pgrep -x chromium-browser",
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
        "playwright-cli snapshot --filename=page.yml",
        "playwright-cli open https://example.com",
    ] {
        let out = guard(w.path(), command);
        assert!(passes(&out), "{command:?}: {out:?}");
    }
}

#[test]
fn a_snapshot_read_into_the_transcript_is_refused() {
    let w = Scratch::new("guard-bash-snapshot-refused");
    for command in [
        "playwright-cli snapshot",
        "playwright-cli -s=x snapshot",
        "~/.dotfiles/bin/playwright-cli -s=x snapshot",
        "grep -i secret .playwright-cli/page-2026.yml",
        "cat saved-snapshot.yml",
        "head -n 20 .playwright-cli/page-1.yaml",
        "grep -rn secret .playwright-cli",
        "rg -i \"client secret\" .playwright-cli/",
        "cd .playwright-cli && grep secret page-1.yml",
        "playwright-cli -s=x find \"Client secret\"",
        "more .playwright-cli/page-3.yml",
        "jq . .playwright-cli/page-3.yml",
    ] {
        let out = guard(w.path(), command);
        assert!(refused(&out, "workflow mask"), "{command:?}: {out:?}");
    }
}

#[test]
fn a_snapshot_through_workflow_mask_or_to_a_file_passes() {
    let w = Scratch::new("guard-bash-snapshot-passes");
    for command in [
        "playwright-cli -s=x snapshot --filename=/some/file.yml",
        "playwright-cli -s=x snapshot --filename /some/file.yml",
        "playwright-cli -s=x snapshot | workflow mask",
        "grep -n Secret .playwright-cli/page-1.yml | workflow mask",
        "cat saved-snapshot.yml | workflow mask",
        "cat config.yml",
        "echo snapshot",
        "git log -- .playwright-cli/x.yml",
        "playwright-cli --raw snapshot > before.yml",
        "playwright-cli -s=x find \"Client secret\" | workflow mask",
        "ls .playwright-cli",
    ] {
        let out = guard(w.path(), command);
        assert!(passes(&out), "{command:?}: {out:?}");
    }
}

#[test]
fn playwright_cli_close_all_is_refused() {
    let w = Scratch::new("guard-bash-close-all");
    for command in [
        "playwright-cli close-all",
        "~/.dotfiles/bin/playwright-cli close-all",
        "playwright-cli -s=walk close-all",
    ] {
        let out = guard(w.path(), command);
        assert!(
            refused(&out, "playwright-cli -s=<name> close"),
            "{command:?}: {out:?}"
        );
    }
}

#[test]
fn playwright_cli_closes_one_session_and_echo_may_say_close_all() {
    let w = Scratch::new("guard-bash-close-one");
    for command in [
        "playwright-cli -s=walk close",
        "playwright-cli close",
        "echo close-all",
    ] {
        let out = guard(w.path(), command);
        assert!(passes(&out), "{command:?}: {out:?}");
    }
}

#[test]
fn a_run_code_fill_type_or_insert_text_outside_a_try_is_refused() {
    let w = Scratch::new("guard-bash-fill");
    w.put("fill.js", "async page => { await page.fill('#a', 'x') }");
    for command in [
        r#"playwright-cli -s=w run-code 'async page => { await page.getByRole("textbox").fill(process.env.X) }'"#,
        r#"playwright-cli run-code --raw 'async page => { await page.keyboard.insertText("x") }'"#,
        r#"playwright-cli run-code 'async page => { try { await a.fill("x") } catch (e) {} await b.type("y") }'"#,
        "playwright-cli run-code --filename fill.js",
        "playwright-cli run-code --filename=fill.js",
    ] {
        let out = guard(w.path(), command);
        assert!(
            refused(&out, "try { ... } catch (e) { return e.name }"),
            "{command:?}: {out:?}"
        );
    }
}

#[test]
fn a_run_code_fill_inside_a_try_or_a_string_or_no_fill_runs() {
    let w = Scratch::new("guard-bash-fill-safe");
    for command in [
        r##"playwright-cli run-code 'async page => { try { await page.locator("#a").fill("x") } catch (e) { return e.name } }'"##,
        r#"playwright-cli run-code 'async page => { return await page.title() }'"#,
        r#"playwright-cli run-code 'async page => { await page.click("text=fill(") }'"#,
        "playwright-cli run-code --filename missing.js",
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

/// A worktree or its build under /tmp filled the tmpfs quota and died on
/// EDQUOT, ten times, the last workflow m3-mem-fixes linking serde_derive.
#[test]
fn a_worktree_or_a_cargo_build_under_tmp_is_refused() {
    let repo = Path::new("/home/me/Sites/github/workflow");
    let in_tmp = Path::new("/tmp/claude-1000/x/repo");
    for (cwd, command) in [
        (repo, "git worktree add /tmp/claude-1000/x/wt HEAD"),
        (repo, "git worktree add -b t1 /tmp/claude-1000/x/wt HEAD"),
        (repo, "git -C /tmp/claude-1000/x worktree add wt HEAD"),
        (in_tmp, "git worktree add ../wt HEAD"),
        (in_tmp, "cargo test"),
        (in_tmp, "cargo +nightly build --release"),
        (repo, "CARGO_TARGET_DIR=/tmp/t cargo build"),
        (repo, "env CARGO_TARGET_DIR=/tmp/t cargo test -q"),
        (repo, "export CARGO_TARGET_DIR=/tmp/t; cargo build"),
        (repo, "cargo build --target-dir /tmp/t"),
        (repo, "cargo build --target-dir=/tmp/t"),
        (in_tmp, "cargo install --path . --root ~/.local"),
        (repo, "cargo test --manifest-path /tmp/x/Cargo.toml"),
        (repo, "cd /tmp/x && cargo build"),
        (repo, "(cd /tmp/x && cargo test)"),
        (repo, "git worktree add --orphan -b t /tmp/x/wt"),
        (repo, "git worktree add --orphan /tmp/x/wt"),
        // cargo takes the flag over the variable.
        (
            repo,
            "CARGO_TARGET_DIR=/home/me/.cache/w/target cargo build --target-dir /tmp/t",
        ),
    ] {
        let out = guard_home(cwd, Path::new("/home/me"), command);
        assert!(refused(&out, "~/.cache/"), "{cwd:?} {command:?}: {out:?}");
    }
}

#[test]
fn reads_copies_and_builds_outside_tmp_run() {
    let repo = Path::new("/home/me/Sites/github/workflow");
    let in_tmp = Path::new("/tmp/claude-1000/x");
    for (cwd, command) in [
        (repo, "git worktree add ~/.cache/workflow-m1/wt HEAD"),
        (repo, "git worktree add .claude/worktrees/a HEAD"),
        (repo, "git worktree remove /tmp/x/wt"),
        (repo, "cargo test"),
        (repo, "cargo build --target-dir ~/.cache/workflow-m1/target"),
        (repo, "ls /tmp"),
        (repo, "cp a /tmp/b"),
        (repo, "cargo test > /tmp/x/log 2>&1"),
        (repo, "(cd /tmp/x && ls); cargo build"),
        (in_tmp, "ls && cat notes.md"),
        (in_tmp, "cargo --version"),
        (in_tmp, "cd ~/Sites/github/workflow && cargo test"),
        // The build lands beside the manifest, or in the target dir given.
        (
            in_tmp,
            "cargo test --manifest-path /home/me/Sites/github/workflow/workflow/Cargo.toml",
        ),
        (in_tmp, "cargo build --target-dir ~/.cache/w/target"),
        (
            repo,
            "CARGO_TARGET_DIR=/tmp/t cargo build --target-dir /home/me/.cache/w/target",
        ),
    ] {
        let out = guard_home(cwd, Path::new("/home/me"), command);
        assert!(passes(&out), "{cwd:?} {command:?}: {out:?}");
    }
}

/// The hook's input as a subagent's call carries it: `agent_type` names
/// the subagent.
fn guard_as(cwd: &Path, agent: &str, command: &str) -> Output {
    let input = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": command },
        "cwd": cwd,
        "agent_id": "agent-a1",
        "agent_type": agent,
    });
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_workflow"))
        .args(["guard", "bash"])
        .env("HOME", cwd)
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

/// A walker's dev server on the trunk reloaded the orchestrator's review
/// fixes mid-walk (lareys-trendsi m3-bf-bag, ebdfy m26), so a dogfooder
/// starts one only where HEAD is detached: a worktree at the green sha.
#[test]
fn a_dogfooders_dev_server_on_a_branch_checkout_is_refused() {
    let w = Scratch::new("guard-bash-dev");
    w.put("trunk/.git/HEAD", "ref: refs/heads/main\n");
    w.mkdir("trunk/apps/web");
    w.put(
        "walk/.git",
        &format!("gitdir: {}\n", w.at("gitdir").display()),
    );
    w.put("gitdir/HEAD", "0123456789abcdef0123456789abcdef01234567\n");
    let trunk = w.at("trunk");
    for command in [
        "pnpm dev",
        "setsid pnpm dev > ~/.cache/dev.log 2>&1 &",
        "cd apps/web && pnpm exec astro dev --port 4321",
        "npx wrangler dev",
        "cf dev",
        "npm run dev",
        "pnpm --filter web dev",
        "nohup node_modules/.bin/astro dev &",
    ] {
        let out = guard_as(&trunk, "dogfooder", command);
        assert!(refused(&out, "git worktree add"), "{command}: {out:?}");
    }
    // A detached worktree, another agent, or no dev server pass.
    for (dir, agent, command) in [
        (w.at("walk"), "dogfooder", "setsid pnpm dev &"),
        (
            w.at("walk"),
            "dogfooder",
            "cd /x && ls; pnpm exec wrangler dev",
        ),
        (trunk.clone(), "implementer", "pnpm dev"),
        (trunk.clone(), "dogfooder", "pnpm dev:store --name Demo"),
        (trunk.clone(), "dogfooder", "pnpm test"),
        (w.at("nogit"), "dogfooder", "pnpm dev"),
    ] {
        let out = guard_as(&dir, agent, command);
        assert!(passes(&out), "{command} in {dir:?}: {out:?}");
    }
    // The orchestrator's own call carries no agent_type.
    assert!(passes(&guard(&trunk, "pnpm dev")));
}

/// The dev server's folder is the one its package manager or tmux is
/// pointed at, and a command handed to a shell or to tmux is judged as if
/// typed (#YPXPMQHY): the dogfood skill launches servers in tmux.
#[test]
fn a_dogfooders_dev_server_reached_through_a_flag_a_shell_or_tmux_is_judged_too() {
    let w = Scratch::new("guard-bash-dev-routes");
    w.put("trunk/.git/HEAD", "ref: refs/heads/main\n");
    w.put(
        "walk/.git",
        &format!("gitdir: {}\n", w.at("gitdir").display()),
    );
    w.put("gitdir/HEAD", "0123456789abcdef0123456789abcdef01234567\n");
    let (trunk, walk) = (w.at("trunk"), w.at("walk"));
    let (t, k) = (trunk.display(), walk.display());
    for (dir, command) in [
        (&walk, format!("pnpm -C {t} dev")),
        (&walk, format!("pnpm --dir {t} dev")),
        (&walk, format!("pnpm --dir={t} dev")),
        (&walk, format!("npm --prefix {t} run dev")),
        (&walk, format!("yarn --cwd {t} dev")),
        (&walk, format!("bun --cwd={t} dev")),
        (&walk, format!("pnpm -C{t} dev")),
        (&walk, format!("env -C {t} pnpm dev")),
        (&walk, format!("env --chdir={t} pnpm dev")),
        (&trunk, "bash -c 'pnpm dev'".to_string()),
        (&trunk, "sh -c \"cd sub && npx astro dev\"".to_string()),
        (&trunk, "tmux new -d 'pnpm dev'".to_string()),
        (
            &trunk,
            "tmux new-session -d -s walk -x 120 -y 40 pnpm dev".to_string(),
        ),
        (&walk, format!("tmux new -d -c {t} 'pnpm dev'")),
        // Back out of the subshell, the line is in the trunk again.
        (&trunk, format!("(cd {k} && ls); pnpm dev")),
    ] {
        let out = guard_as(dir, "dogfooder", &command);
        assert!(refused(&out, "git worktree add"), "{command}: {out:?}");
    }
    for (dir, command) in [
        (&trunk, format!("pnpm -C {k} dev")),
        (&trunk, format!("env -C {k} pnpm dev")),
        (&trunk, format!("tmux new -d -c {k} 'pnpm dev'")),
        (&trunk, format!("bash -c 'cd {k} && pnpm dev'")),
        // A subshell's cd moves only the subshell, as the cd rule advises.
        (&trunk, format!("(cd {k} && setsid pnpm dev &)")),
        // A pane's folder is not on the line, so keys sent to it pass.
        (
            &trunk,
            "tmux send-keys -t walk 'pnpm dev' Enter".to_string(),
        ),
        (&trunk, "tmux new -d -s walk".to_string()),
    ] {
        let out = guard_as(dir, "dogfooder", &command);
        assert!(passes(&out), "{command}: {out:?}");
    }
}

/// A process whose argv holds `words` and whose cwd is `cwd`, the way a
/// running test suite looks from /proc. It dies with the test, panic or not.
struct Suite(std::process::Child);

impl Suite {
    fn run(cwd: &Path, words: &str) -> Suite {
        let child = std::process::Command::new("sh")
            .args(["-c", &format!("sleep 30; : {words}")])
            .current_dir(cwd)
            .spawn()
            .unwrap();
        Suite(child)
    }
}

impl Drop for Suite {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn git(cwd: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(args)
        .current_dir(cwd)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn repo(w: &Scratch) -> PathBuf {
    let repo = w.mkdir("repo");
    git(&repo, &["init", "-q"]);
    repo
}

const HELD: &str = "moves HEAD under the tests";

#[test]
fn a_commit_or_merge_while_the_checkouts_suite_runs_is_refused() {
    for words in ["cargo test -q", "bash tests/run.sh"] {
        let w = Scratch::new("guard-bash-suite");
        let repo = repo(&w);
        let elsewhere = w.mkdir("elsewhere");
        let _suite = Suite::run(&repo, words);
        for command in ["git commit -am x", "git merge --ff-only b"] {
            let out = guard(&repo, command);
            assert!(refused(&out, HELD), "{words} {command:?}: {out:?}");
        }
        let by_dir = format!("git -C {} commit -m x", repo.display());
        let out = guard(&elsewhere, &by_dir);
        assert!(refused(&out, HELD), "{words} {by_dir:?}: {out:?}");
        for command in ["git status", "git log"] {
            let out = guard(&repo, command);
            assert!(passes(&out), "{words} {command:?}: {out:?}");
        }
    }
}

#[test]
fn a_suite_in_one_worktree_does_not_hold_back_a_commit_in_the_other() {
    let w = Scratch::new("guard-bash-suite-worktree");
    let repo = repo(&w);
    git(
        &repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let linked = repo.join(".claude/worktrees/w");
    git(
        &repo,
        &["worktree", "add", "-q", linked.to_str().unwrap(), "-b", "w"],
    );
    {
        let _suite = Suite::run(&repo, "cargo test -q");
        let out = guard(&linked, "git commit -am x");
        assert!(passes(&out), "{out:?}");
    }
    let _suite = Suite::run(&linked, "cargo test -q");
    let out = guard(&repo, "git commit -am x");
    assert!(passes(&out), "{out:?}");
}

#[test]
fn a_commit_passes_when_no_suite_runs() {
    let w = Scratch::new("guard-bash-suite-idle");
    let repo = repo(&w);
    let out = guard(&repo, "git commit -am x");
    assert!(passes(&out), "{out:?}");
}
