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

/// guard bash refused `command` naming exactly `lines`.
#[track_caller]
fn names(cwd: &Path, command: &str, lines: &[&str]) {
    let out = bash(cwd, command);
    assert_eq!(out.status.code(), Some(2), "{command:?}: {out:?}");
    assert_eq!(
        stderr(&out),
        format!("{HEAD}\n{}\n", lines.join("\n")),
        "{command:?}"
    );
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
        }),
    );
    let site = w.path().join("apps/site");
    for script in ["build", "npm", "vars", "sub", "or", "cd"] {
        says_today(&site, &format!("pnpm {script}"));
    }
}
