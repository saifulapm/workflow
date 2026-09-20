//! Four papercuts ebdify m1's workers and orchestrator hit (m1-lessons
//! ruling 12): a kind cannot be listed without inventing a query, one task's
//! block cannot be read without the whole plan, `mem list` is nobody's verb
//! and says nothing useful, and a `#`-prefixed id has always been taken.

mod common;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use common::{World, code, mem, stderr, stdout};

fn repo(w: &World) -> std::path::PathBuf {
    w.repo("app", Some("git@github.com:me/app.git"))
}

/// `mem` with text on stdin, the environment `common::mem_env` sets.
fn mem_stdin(w: &World, cwd: &Path, args: &[&str], input: &str) -> std::process::Output {
    let dirs = w.dirs();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mem"));
    cmd.current_dir(cwd)
        .args(args)
        .env("XDG_DATA_HOME", &dirs.data)
        .env("XDG_CACHE_HOME", &dirs.cache)
        .env("XDG_STATE_HOME", &dirs.state)
        .env("XDG_CONFIG_HOME", &dirs.config)
        .env("PI_CODING_AGENT_DIR", w.pi_agent_dir())
        .env("WORKFLOW_BIN", "/nonexistent/workflow")
        .env_remove("MEM_SESSION_ID")
        .env_remove("PI_SESSION_ID")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("WORKFLOW_TASK")
        .env_remove("CARGO_TARGET_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn mem");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().expect("mem")
}

#[test]
fn a_kind_lists_without_a_query() {
    let w = World::new("papercuts-kind");
    let dir = repo(&w);
    let out = mem(
        &w,
        &dir,
        &[
            "save",
            "--kind",
            "ruling",
            "cents never floats - money - a rounding bug",
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = mem(
        &w,
        &dir,
        &["save", "--kind", "fact", "a fact that is not a ruling"],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let out = mem(&w, &dir, &["search", "--kind", "ruling"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).contains("cents never floats"),
        "{}",
        stdout(&out)
    );
    assert!(
        !stdout(&out).contains("a fact that is not"),
        "{}",
        stdout(&out)
    );

    let out = mem(&w, &dir, &["search"]);
    assert_eq!(
        code(&out),
        2,
        "no query and no kind is usage: {}",
        stderr(&out)
    );
    assert!(stderr(&out).contains("--kind <kind>"), "{}", stderr(&out));
}

#[test]
fn one_task_block_is_read_alone() {
    let w = World::new("papercuts-task");
    let dir = repo(&w);
    let plan = "# plan: p\n\n## Spec\n\nx\n\n- [ ] t1 First\n      Files: a.rs\n      Verify: true\n- [x] t2 Second  [after: t1]\n      Files: b.rs\n      Verify: true\n      Done: b is done\n";
    let out = mem_stdin(&w, &dir, &["plan", "--stdin"], plan);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let out = mem(&w, &dir, &["plan", "--task", "t2"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "- [x] t2 Second  [after: t1]\n      Files: b.rs\n      Verify: true\n      Done: b is done\n"
    );
    let out = mem(&w, &dir, &["plan", "--task", "t9"]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("no task 't9'"), "{}", stderr(&out));
}

#[test]
fn list_says_which_verbs_list() {
    let w = World::new("papercuts-list");
    let dir = w.plain_dir("anywhere");
    let out = mem(&w, &dir, &["list"]);
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).contains("`mem log` lists recent entries"),
        "{}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("`mem search --kind <kind>`"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_hash_prefixed_id_resolves() {
    let w = World::new("papercuts-hash");
    let dir = repo(&w);
    let out = mem(
        &w,
        &dir,
        &["save", "--kind", "fact", "found by its short id"],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    // `#ABCDEFGH fact`: the short id is the first word.
    let first = stdout(&out)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    assert!(first.starts_with('#'), "{}", stdout(&out));
    let out = mem(&w, &dir, &["show", &first]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).contains("found by its short id"),
        "{}",
        stdout(&out)
    );
}
