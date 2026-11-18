//! A pnpm refusal names what to run instead: the by-path form of the refused
//! command, through the real binary fed the JSON a Claude Code PreToolUse
//! hook gets. 22 refused lareys-trendsi implementers had pnpm in their VERIFY
//! and were told only to call the tool by path.

#[path = "../src/scratch.rs"]
mod scratch;

use std::io::Write;
use std::path::Path;
use std::process::{Output, Stdio};

use scratch::Scratch;
use serde_json::json;

/// guard bash's refusal when it cannot name the command to run instead.
const TODAY: &str = "workflow guard refused this on purpose: pnpm refuses a node_modules linked from outside this worktree, or writes into the checkout it links.\n\
Run the tool by path (node_modules/.bin/vitest), or `npm_config_verify_deps_before_run=false node scripts/verify.mjs`.\n";

/// guard bash's first line when it can.
const HEAD: &str = "workflow guard refused this on purpose: pnpm refuses a node_modules linked from outside this worktree, or writes into the checkout it links. Run each line below as a call of its own:";

/// A linked worktree whose node_modules links the trunk's, with
/// package.json files at the paths given. The trunk is kept with it.
fn worktree(tag: &str, packages: &[(&str, &str)]) -> (Scratch, Scratch) {
    let trunk = Scratch::new(&format!("{tag}-trunk"));
    std::fs::create_dir_all(trunk.path().join("node_modules")).unwrap();
    let w = Scratch::new(tag);
    std::fs::write(w.path().join(".git"), "gitdir: /elsewhere\n").unwrap();
    std::os::unix::fs::symlink(
        trunk.path().join("node_modules"),
        w.path().join("node_modules"),
    )
    .unwrap();
    for (path, text) in packages {
        let at = w.path().join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, text).unwrap();
    }
    (trunk, w)
}

fn hook(check: &str, cwd: &Path, input: serde_json::Value) -> Output {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_workflow"))
        .args(["guard", check])
        .env("WORKFLOW_MEM", "/nonexistent/mem")
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

fn bash(cwd: &Path, command: &str) -> Output {
    let input = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": command },
        "cwd": cwd,
    });
    hook("bash", cwd, input)
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// guard bash refused `command` naming exactly `lines`, and every line it
/// names passes guard bash: a refusal that names a command the guards
/// refuse too sends the agent round in a circle.
#[track_caller]
fn names(cwd: &Path, command: &str, lines: &[&str]) {
    let out = bash(cwd, command);
    assert_eq!(out.status.code(), Some(2), "{command:?}: {out:?}");
    assert_eq!(
        stderr(&out),
        format!("{HEAD}\n{}\n", lines.join("\n")),
        "{command:?}"
    );
    for line in lines {
        passes(cwd, line);
    }
}

/// guard bash lets `command` through.
#[track_caller]
fn passes(cwd: &Path, command: &str) {
    let out = bash(cwd, command);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{command:?} is refused: {}",
        stderr(&out)
    );
}

#[test]
fn turbo_by_path_is_a_line_no_refusal_may_name() {
    // r2's first by-path refusal named this for `pnpm typecheck` with the
    // script `turbo run typecheck`; guard bash's turbo rule refuses it.
    let (_t, w) = worktree("by-path-turbo", &[]);
    let out = bash(w.path(), "node_modules/.bin/turbo run typecheck");
    assert_eq!(out.status.code(), Some(2), "{out:?}");
}

/// guard bash refused `command` with today's message.
#[track_caller]
fn says_today(cwd: &Path, command: &str) {
    let out = bash(cwd, command);
    assert_eq!(out.status.code(), Some(2), "{command:?}: {out:?}");
    assert_eq!(stderr(&out), TODAY, "{command:?}");
}

const TSC: &str = r#"{"name": "app", "scripts": {"typecheck": "tsc --noEmit"}}"#;

#[test]
fn pnpm_exec_names_the_tool_by_path() {
    let (_t, w) = worktree("by-path-exec", &[]);
    names(
        w.path(),
        "pnpm exec vitest run test/a.test.ts",
        &["node_modules/.bin/vitest run test/a.test.ts"],
    );
}

#[test]
fn a_package_script_names_its_own_command_by_path() {
    let (_t, w) = worktree("by-path-script", &[("package.json", TSC)]);
    for command in ["pnpm typecheck", "pnpm run typecheck"] {
        names(w.path(), command, &["node_modules/.bin/tsc --noEmit"]);
    }
}

#[test]
fn a_script_no_package_has_or_a_pnpm_command_keeps_todays_message() {
    let (_t, w) = worktree("by-path-none", &[("package.json", TSC)]);
    for command in ["pnpm nosuch", "pnpm install", "pnpm i", "pnpm add zod"] {
        says_today(w.path(), command);
    }
}

#[test]
fn pnpm_as_a_word_inside_quotes_passes() {
    let (_t, w) = worktree("by-path-quoted", &[("package.json", TSC)]);
    let out = bash(
        w.path(),
        r#"git commit -m "Fix the cart (pnpm test was red)""#,
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
}

#[test]
fn the_package_folder_comes_from_c_filter_or_cd() {
    let (_t, w) = worktree(
        "by-path-folder",
        &[
            ("package.json", r#"{"name": "root"}"#),
            ("apps/admin/package.json", TSC),
            (
                "packages/core/package.json",
                r#"{"name": "@lareys/core", "scripts": {"typecheck": "tsc --noEmit"}}"#,
            ),
            // A package of the same name under node_modules is no workspace package.
            (
                "node_modules/.pnpm/x/package.json",
                r#"{"name": "@lareys/core", "scripts": {"typecheck": "tsc -b"}}"#,
            ),
        ],
    );
    let admin = &["cd apps/admin && node_modules/.bin/tsc --noEmit"];
    names(w.path(), "pnpm -C apps/admin typecheck", admin);
    names(w.path(), "pnpm --dir=apps/admin run typecheck", admin);
    names(w.path(), "cd apps/admin && pnpm typecheck", admin);
    names(
        w.path(),
        "pnpm --filter @lareys/core typecheck",
        &["cd packages/core && node_modules/.bin/tsc --noEmit"],
    );
    // From inside the package, no cd.
    names(
        &w.path().join("apps/admin"),
        "pnpm typecheck",
        &["node_modules/.bin/tsc --noEmit"],
    );
    for command in [
        "pnpm --filter @lareys/nosuch typecheck",
        "pnpm -r typecheck",
    ] {
        says_today(w.path(), command);
    }
}

fn scripts(tag: &str, scripts: serde_json::Value) -> (Scratch, Scratch) {
    let package = json!({ "name": "app", "scripts": scripts }).to_string();
    worktree(tag, &[("apps/site/package.json", &package)])
}

#[test]
fn a_script_of_two_tools_names_one_line_each() {
    let (_t, w) = scripts(
        "by-path-two",
        json!({ "typecheck": "astro check && tsc -p test", "check": "rm -rf dist; node scripts/a.mjs" }),
    );
    names(
        w.path(),
        "cd apps/site && pnpm typecheck",
        &[
            "cd apps/site && node_modules/.bin/astro check",
            "cd apps/site && node_modules/.bin/tsc -p test",
        ],
    );
    // A program the system or node has is no package's tool.
    names(
        w.path(),
        "pnpm -C apps/site check",
        &[
            "cd apps/site && rm -rf dist",
            "cd apps/site && node scripts/a.mjs",
        ],
    );
}

#[test]
fn an_env_prefix_arguments_and_a_pipe_tail_are_kept() {
    let (_t, w) = scripts(
        "by-path-keep",
        json!({ "test": "NODE_ENV=test vitest run", "both": "tsc -b && vitest run", "unit": "vitest run" }),
    );
    let site = w.path().join("apps/site");
    names(
        &site,
        "pnpm test",
        &["NODE_ENV=test node_modules/.bin/vitest run"],
    );
    names(
        &site,
        "pnpm unit 2>&1 | tail -5",
        &["node_modules/.bin/vitest run 2>&1 | tail -5"],
    );
    names(
        &site,
        "pnpm unit 2>&1 > out.txt",
        &["node_modules/.bin/vitest run 2>&1 > out.txt"],
    );
    // pnpm hands its arguments to the script's last command; a redirection
    // and the pipe go with every line.
    names(
        &site,
        "timeout 600 pnpm both test/a.test.ts > out.txt | grep -v skip",
        &[
            "timeout 600 node_modules/.bin/tsc -b > out.txt | grep -v skip",
            "timeout 600 node_modules/.bin/vitest run test/a.test.ts > out.txt | grep -v skip",
        ],
    );
    names(
        &site,
        "pnpm exec vitest run -t 'the cart'",
        &["node_modules/.bin/vitest run -t 'the cart'"],
    );
}

#[test]
fn a_script_it_cannot_put_by_path_keeps_todays_message() {
    let (_t, w) = scripts(
        "by-path-cannot",
        json!({
            "build": "pnpm run build && vitest",
            "npm": "npm run x",
            "vars": "vitest run $FILES",
            "sub": "tsc && (cd e2e && playwright test)",
            "or": "tsc || true",
            "cd": "cd e2e && playwright test",
            // turbo runs pnpm install over the linked node_modules (palmshop m20).
            "turbo": "turbo run typecheck",
        }),
    );
    let site = w.path().join("apps/site");
    for script in ["build", "npm", "vars", "sub", "or", "cd", "turbo"] {
        says_today(&site, &format!("pnpm {script}"));
    }
}

fn guard_pnpm(cwd: &Path, command: &str) -> Output {
    let input = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": command },
        "cwd": cwd,
    });
    hook("pnpm", cwd, input)
}

#[test]
fn guard_pnpm_names_the_same_lines() {
    let (_t, w) = scripts(
        "by-path-guard-pnpm",
        json!({ "typecheck": "astro check && tsc -p test" }),
    );
    let out = guard_pnpm(w.path(), "cd apps/site && pnpm typecheck");
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert_eq!(
        stderr(&out),
        "workflow guard: pnpm refused. This worktree's node_modules is the trunk checkout's, linked in, and pnpm would rewrite it. Run each line below as a call of its own:\n\
         cd apps/site && node_modules/.bin/astro check\n\
         cd apps/site && node_modules/.bin/tsc -p test\n"
    );
    let out = guard_pnpm(w.path(), "pnpm nosuch");
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert_eq!(
        stderr(&out),
        "workflow guard: pnpm refused. This worktree's node_modules is the trunk checkout's, linked in, and pnpm would rewrite it. Call the tool by path instead, as in node_modules/.bin/vitest run <file>.\n"
    );
    let out = guard_pnpm(
        w.path(),
        r#"git commit -m "Fix the cart (pnpm test was red)""#,
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
}

fn agent(cwd: &Path, subagent: &str, prompt: &str) -> Output {
    let input = json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Agent",
        "tool_input": {
            "subagent_type": subagent,
            "description": "t3 build the composer",
            "prompt": prompt,
        },
        "cwd": cwd,
    });
    hook("agent", cwd, input)
}

/// A brief in the orchestrate skill's shape, with `verify` under VERIFY.
fn brief(context: &str, verify: &str) -> String {
    format!(
        "GOAL        Build the composer\n\
         SCOPE       apps/site\n\
         CONTEXT     {context}\n\
         ACCEPTANCE  a failing test first\n\
         VERIFY      {verify}\n\
         STOP        anything outside apps/site\n"
    )
}

const AGENT_HEAD: &str = "workflow guard refused this on purpose: this implementer brief's VERIFY runs pnpm, which its worktree's linked node_modules refuses. Write these lines in its place, each a call of its own:";

#[test]
fn an_implementer_brief_whose_verify_runs_pnpm_is_refused_naming_the_lines() {
    let trunk = Scratch::new("by-path-agent");
    trunk.put(
        "apps/site/package.json",
        r#"{"name": "site", "scripts": {"typecheck": "astro check && tsc -p test", "lint": "oxlint src"}}"#,
    );
    let lint = "\n            node_modules/.bin/oxlint src";
    let refused = agent(
        trunk.path(),
        "implementer",
        &brief("the site", &format!("cd apps/site && pnpm typecheck{lint}")),
    );
    assert_eq!(refused.status.code(), Some(2), "{refused:?}");
    assert_eq!(
        stderr(&refused),
        format!(
            "{AGENT_HEAD}\n\
             cd apps/site && node_modules/.bin/astro check\n\
             cd apps/site && node_modules/.bin/tsc -p test\n"
        )
    );

    // Two pnpm lines, as a list with backticks, name both forms.
    let two = agent(
        trunk.path(),
        "implementer",
        &brief(
            "the site",
            "\n- `cd apps/site && pnpm typecheck`\n- `pnpm -C apps/site lint`",
        ),
    );
    assert_eq!(two.status.code(), Some(2), "{two:?}");
    assert_eq!(
        stderr(&two),
        format!(
            "{AGENT_HEAD}\n\
             cd apps/site && node_modules/.bin/astro check\n\
             cd apps/site && node_modules/.bin/tsc -p test\n\
             cd apps/site && node_modules/.bin/oxlint src\n"
        )
    );

    for (subagent, prompt) in [
        (
            "implementer",
            brief(
                "the site",
                &format!(
                    "cd apps/site && node_modules/.bin/astro check\n            \
                     cd apps/site && node_modules/.bin/tsc -p test{lint}"
                ),
            ),
        ),
        ("scout", brief("the site", "cd apps/site && pnpm typecheck")),
        (
            "implementer",
            brief(
                "pnpm typecheck",
                &format!("cd apps/site && node_modules/.bin/tsc -p test{lint}"),
            ),
        ),
    ] {
        let out = agent(trunk.path(), subagent, &prompt);
        assert_eq!(out.status.code(), Some(0), "{prompt}: {out:?}");
    }
}

#[test]
fn a_verify_pnpm_line_with_no_by_path_form_is_still_refused() {
    let trunk = Scratch::new("by-path-agent-none");
    let out = agent(
        trunk.path(),
        "implementer",
        &brief("x", "pnpm nosuch\n            node_modules/.bin/oxlint ."),
    );
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert_eq!(
        stderr(&out),
        "workflow guard refused this on purpose: this implementer brief's VERIFY runs pnpm, which its worktree's linked node_modules refuses.\n\
         Call each tool by path instead, as in node_modules/.bin/vitest run <file>, one line each.\n"
    );
}
