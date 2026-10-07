//! The runner claim over in-place writes, plan and roadmap status, and
//! `mem plan --add-task`.

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};

use common::{World, code, mem, stderr, stdout};

const PLAN: &str = "# plan: shop\n\n\
    - [ ] t1 First task\n\
    \x20     Done: one\n\
    - [ ] t2 Second task\n\
    \x20     Done: two\n";

const ROADMAP: &str = "# roadmap: shop\n\n\
    - [ ] m1-auth Sign-in\n\
    - [ ] m2-billing Billing\n";

fn mem_stdin(w: &World, cwd: &Path, args: &[&str], input: &[u8]) -> std::process::Output {
    let dirs = w.dirs();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_mem"))
        .current_dir(cwd)
        .args(args)
        .env("XDG_DATA_HOME", &dirs.data)
        .env("XDG_CACHE_HOME", &dirs.cache)
        .env("XDG_STATE_HOME", &dirs.state)
        .env("XDG_CONFIG_HOME", &dirs.config)
        .env_remove("MEM_SESSION_ID")
        .env_remove("MEM_PROJECT")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn mem");
    if let Err(e) = child.stdin.take().expect("stdin").write_all(input)
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        panic!("write stdin: {e}");
    }
    child.wait_with_output().expect("wait")
}

fn json(out: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).expect("json output")
}

/// A project on the machine called `here`, with a plan and a roadmap.
fn shop(tag: &str) -> (World, PathBuf, String) {
    let w = World::new(tag);
    let machine = w.dirs().qshell_machine();
    std::fs::create_dir_all(machine.parent().unwrap()).unwrap();
    std::fs::write(&machine, "here\n").unwrap();
    let repo = w.repo("shop", None);
    let out = mem_stdin(&w, &repo, &["plan", "--stdin"], PLAN.as_bytes());
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = mem_stdin(&w, &repo, &["roadmap", "--stdin"], ROADMAP.as_bytes());
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let id = json(&mem(&w, &repo, &["project", "current", "--json"]))["id"]
        .as_str()
        .unwrap()
        .to_string();
    (w, repo, id)
}

/// Moves the claim's start back, as if it were taken `hours` ago.
fn claimed_hours_ago(w: &World, id: &str, hours: i64) {
    let since = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(hours);
    mem::project::set_key(&w.store(), id, "runner_since", &since.to_string()).unwrap();
}

#[test]
fn a_fresh_foreign_claim_refuses_a_tick_until_forced() {
    let (w, repo, _) = shop("claim-fresh");
    assert_eq!(
        code(&mem(&w, &repo, &["project", "set", "runner", "there"])),
        0
    );

    let out = mem(&w, &repo, &["plan", "--tick", "t1"]);
    assert_eq!(code(&out), 5, "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("shop is run by there since "), "{err}");
    assert!(err.contains("; --force overrides"), "{err}");
    assert!(stdout(&mem(&w, &repo, &["plan"])).contains("- [ ] t1 "));

    let out = mem(&w, &repo, &["roadmap", "--tick", "m1-auth"]);
    assert_eq!(code(&out), 5, "{}", stderr(&out));

    let out = mem(&w, &repo, &["plan", "--tick", "t1", "--force"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stdout(&mem(&w, &repo, &["plan"])).contains("- [x] t1 "));

    // Reads work everywhere.
    assert_eq!(code(&mem(&w, &repo, &["plan"])), 0);
    assert_eq!(code(&mem(&w, &repo, &["roadmap"])), 0);
}

#[test]
fn a_claim_by_this_machine_or_a_stale_one_does_not_refuse() {
    let (w, repo, id) = shop("claim-stale");
    assert_eq!(
        code(&mem(&w, &repo, &["project", "set", "runner", "here"])),
        0
    );
    let out = mem(&w, &repo, &["plan", "--tick", "t1"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    assert_eq!(
        code(&mem(&w, &repo, &["project", "set", "runner", "there"])),
        0
    );
    claimed_hours_ago(&w, &id, 2);
    let out = mem(&w, &repo, &["roadmap", "--tick", "m1-auth"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}

#[test]
fn a_recent_run_line_keeps_an_old_claim_live() {
    let (w, repo, id) = shop("claim-run-line");
    assert_eq!(
        code(&mem(&w, &repo, &["project", "set", "runner", "there"])),
        0
    );
    claimed_hours_ago(&w, &id, 2);
    let out = mem(&w, &repo, &["log", "--type", "run", "tick: t1 merged"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let out = mem(&w, &repo, &["plan", "--tick", "t2"]);
    assert_eq!(code(&out), 5, "{}", stderr(&out));
}

#[test]
fn section_writes_are_gated_and_whole_page_writes_are_not() {
    let (w, repo, _) = shop("claim-wiki");
    let page = "# Notes\n\nintro\n\n## Setup\n\nold\n";
    let out = mem_stdin(
        &w,
        &repo,
        &["wiki", "notes", "--stdin", "--note", "first"],
        page.as_bytes(),
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        code(&mem(&w, &repo, &["project", "set", "runner", "there"])),
        0
    );

    let args = ["wiki", "notes#setup", "--stdin", "--note", "fix"];
    let out = mem_stdin(&w, &repo, &args, b"## Setup\n\nnew\n");
    assert_eq!(code(&out), 5, "{}", stderr(&out));

    let forced = [&args[..], &["--force"]].concat();
    let out = mem_stdin(&w, &repo, &forced, b"## Setup\n\nnew\n");
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stdout(&mem(&w, &repo, &["wiki", "notes"])).contains("new"));

    let out = mem_stdin(
        &w,
        &repo,
        &["wiki", "notes", "--stdin", "--note", "whole"],
        page.as_bytes(),
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}
