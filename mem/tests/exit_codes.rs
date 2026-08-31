//! AC15, second half: every documented exit code (spec §7) is exercised here,
//! through the binary, so the table in the spec and the table in `exit.rs` are
//! not just two prose documents that agree.

mod common;

use std::io::Write;
use std::path::Path;

use common::{World, code, item, mem_env, put, stderr, stdout};
use mem::item::Kind;

const P: &str = "01K2AAAAAAAAAAAAAAAAAAAAAA";

fn run(args: &[&str], w: &World, cwd: &Path) -> std::process::Output {
    mem_env(
        w,
        cwd,
        args,
        &[("MEM_SYNC_CMD", "true"), ("MEM_NOTIFY_CMD", "true")],
    )
}

/// The same, with bytes on stdin — the only way at the `--stdin` paths.
fn run_with_stdin(args: &[&str], w: &World, cwd: &Path, input: &[u8]) -> std::process::Output {
    let dirs = w.dirs();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_mem"))
        .current_dir(cwd)
        .args(args)
        .env("XDG_DATA_HOME", &dirs.data)
        .env("XDG_CACHE_HOME", &dirs.cache)
        .env("XDG_STATE_HOME", &dirs.state)
        .env("XDG_CONFIG_HOME", &dirs.config)
        .env("MEM_SYNC_CMD", "true")
        .env_remove("MEM_SESSION_ID")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn mem");
    // A refused verb exits before it reads stdin, and under load the child can
    // win that race, closing the pipe under this write. The early exit is the
    // behavior under test, so EPIPE is a pass, not a panic (friction #D6TP86YJ).
    if let Err(e) = child.stdin.take().expect("stdin").write_all(input)
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        panic!("write stdin: {e}");
    }
    child.wait_with_output().expect("wait")
}

#[test]
fn zero_is_anything_that_worked_including_every_empty_state() {
    let w = World::new("exit-zero");
    let dir = w.plain_dir("nowhere");
    // A fresh machine: the read verbs that always have something to say.
    for args in [
        vec!["context"],
        vec!["context", "--brief"],
        vec!["projects"],
        vec!["prune"],
        vec!["doctor"],
        vec!["reindex"],
        vec!["precompact"],
        vec!["session-check", "--session-id", "s"],
    ] {
        let out = run(&args, &w, &dir);
        assert_eq!(code(&out), 0, "{args:?}: {}", stderr(&out));
    }
    assert!(stdout(&run(&["context"], &w, &dir)).contains("nothing recorded"));
}

#[test]
fn one_is_nothing_found_on_any_verb() {
    let w = World::new("exit-one");
    let repo = w.repo("thing", None);
    assert_eq!(
        code(&run(&["show", "01K2YR1VC0AB3DE4FG5HJ6KM7N"], &w, &repo)),
        1
    );
    assert_eq!(code(&run(&["show", "not-an-id"], &w, &repo)), 1);
    assert_eq!(
        code(&run(&["search", "nothing-matches-this"], &w, &repo)),
        1
    );
    assert_eq!(code(&run(&["log"], &w, &repo)), 1);
    assert_eq!(code(&run(&["handoff"], &w, &repo)), 1);
    assert_eq!(code(&run(&["status"], &w, &repo)), 1);
    assert_eq!(code(&run(&["plan"], &w, &repo)), 1);
    assert_eq!(code(&run(&["plan", "--tick", "t1"], &w, &repo)), 1);
    assert_eq!(code(&run(&["questions"], &w, &repo)), 1);
    assert_eq!(code(&run(&["answer", "ZZZZZZZZ", "no"], &w, &repo)), 1);
    assert_eq!(code(&run(&["project", "current"], &w, &repo)), 1);
    assert_eq!(
        code(&run(&["save", "x", "--project", "nope"], &w, &repo)),
        1
    );
    assert_eq!(
        code(&run(&["save", "x", "--supersedes", "ZZZZZZZZ"], &w, &repo)),
        1
    );
}

#[test]
fn two_is_the_caller_holding_it_wrong() {
    let w = World::new("exit-two");
    let dir = w.plain_dir("cwd");
    assert_eq!(code(&run(&[], &w, &dir)), 2);
    assert_eq!(code(&run(&["nosuchverb"], &w, &dir)), 2);
    assert_eq!(code(&run(&["log", "--since", "nonsense"], &w, &dir)), 2);
    assert_eq!(
        code(&run(&["save", "x", "--kind", "nonsense"], &w, &dir)),
        2
    );
    assert_eq!(
        code(&run(
            &["questions", "--wait", "ZZZZZZZZ", "--timeout", "wat"],
            &w,
            &dir
        )),
        2
    );
    // A plan tick and a plan replacement in one call is not a request mem can
    // honour, and clap says so before anything is read.
    assert_eq!(
        code(&run(&["plan", "--tick", "t1", "--stdin"], &w, &dir)),
        2
    );
    assert_eq!(
        code(&run(&["status", "--set", "x"], &w, &dir)),
        2,
        "status outside a checkout needs --project"
    );

    // Bytes that are not text are the caller holding it wrong, not a store that
    // has gone wrong: exit 3 would send an adapter hunting a broken store.
    let repo = w.repo("thing", None);
    let out = run_with_stdin(&["handoff", "--stdin"], &w, &dir, b"\xff\xfe not text");
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(stderr(&out).contains("UTF-8"), "{}", stderr(&out));
    assert_eq!(
        code(&run_with_stdin(
            &["plan", "--stdin"],
            &w,
            &repo,
            b"# plan\n\xc3\x28\n"
        )),
        2
    );
}

#[cfg(unix)]
#[test]
fn three_is_a_store_that_cannot_be_written() {
    use std::os::unix::fs::PermissionsExt;
    let w = World::new("exit-three");
    let dir = w.plain_dir("cwd");
    let store = w.dirs().store();
    std::fs::create_dir_all(&store).unwrap();
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o555)).unwrap();

    let out = run(&["save", "a fact that has nowhere to go"], &w, &dir);
    // Restore before asserting: a read-only directory outlives the assertion.
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    assert!(!stderr(&out).is_empty(), "an exit 3 explains itself");
}

#[test]
fn four_is_a_wait_that_ran_out() {
    let w = World::new("exit-four");
    let repo = w.repo("thing", None);
    let asked = run(&["ask", "deploy on friday?", "--json"], &w, &repo);
    assert_eq!(code(&asked), 0);
    let v: serde_json::Value = serde_json::from_slice(&asked.stdout).unwrap();
    let id = v["short_id"].as_str().unwrap().to_string();

    let out = mem_env(
        &w,
        &repo,
        &["questions", "--wait", &id, "--timeout", "1s"],
        &[("MEM_SYNC_CMD", "true"), ("MEM_POLL_MS", "50")],
    );
    assert_eq!(code(&out), 4, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("handoff"),
        "a timeout says how to park"
    );
}

#[cfg(unix)]
#[test]
fn five_is_a_singleton_that_changed_under_the_writer() {
    use std::fs::OpenOptions;
    // The race is real rather than simulated: mem takes the CAS baseline before
    // it starts reading the caller's text, and a FIFO makes "the reader has
    // started" observable, so the rewrite lands strictly inside that window.
    let w = World::new("exit-five");
    let repo = w.repo("thing", None);
    let seed = w.dir.join("seed.md");
    std::fs::write(&seed, "# plan: one\n- [ ] t1 do it\n").unwrap();
    assert_eq!(
        code(&run(
            &["plan", "--set-file", seed.to_str().unwrap()],
            &w,
            &repo
        )),
        0
    );
    let id = mem::project::Registry::load(&w.store()).projects[0]
        .id
        .clone();
    let plan_path = w.store().plan_path(&id);

    let fifo = w.dir.join("fifo");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo")
            .success()
    );

    let dirs = w.dirs();
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_mem"))
        .current_dir(&repo)
        .args(["plan", "--set-file", fifo.to_str().unwrap()])
        .env("XDG_DATA_HOME", &dirs.data)
        .env("XDG_CACHE_HOME", &dirs.cache)
        .env("XDG_STATE_HOME", &dirs.state)
        .env("XDG_CONFIG_HOME", &dirs.config)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn mem");

    // This open returns only once mem has opened the FIFO to read it, which is
    // after it recorded what plan.md looked like.
    let mut writer = OpenOptions::new()
        .write(true)
        .open(&fifo)
        .expect("open fifo");
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&plan_path, b"# plan: someone else got here first\n").unwrap();
    writer.write_all(b"# plan: mine\n").unwrap();
    drop(writer);

    let out = child.wait_with_output().expect("wait");
    assert_eq!(
        out.status.code(),
        Some(5),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&plan_path).unwrap(),
        "# plan: someone else got here first\n",
        "a conflict must not clobber"
    );
}

#[test]
fn six_is_a_write_that_landed_and_is_still_too_big() {
    let w = World::new("exit-six");
    let repo = w.repo("thing", None);
    let long = "a status line that goes on\n".repeat(40);
    let out = run(&["status", "--set", &long], &w, &repo);
    assert_eq!(code(&out), 6);
    assert!(stdout(&run(&["status"], &w, &repo)).contains("a status line"));
}

#[test]
fn seven_is_a_short_id_two_items_share() {
    let w = World::new("exit-seven");
    w.project(P, "thing");
    let store = w.store();
    // Only a sync merge can produce this, and no rename can undo it.
    for prefix in ["01K2YR1VC0", "01K2ZZ1VC0"] {
        let mut it = item(Kind::Question, "collides", "body");
        it.meta.id = format!("{prefix}AB3DE4FG5HJ6KM7N");
        put(&store, Some(P), &it);
    }
    let dir = w.plain_dir("cwd");
    assert_eq!(code(&run(&["show", "5HJ6KM7N"], &w, &dir)), 7);
    assert_eq!(code(&run(&["answer", "5HJ6KM7N", "yes"], &w, &dir)), 7);
    assert_eq!(
        code(&run(
            &["questions", "--wait", "5HJ6KM7N", "--timeout", "1s"],
            &w,
            &dir
        )),
        7
    );
    let out = run(&["save", "x", "--supersedes", "5HJ6KM7N"], &w, &dir);
    assert_eq!(code(&out), 7);
    assert!(stderr(&out).contains("ambiguous"), "{}", stderr(&out));
}

/// A reader that closes early -- `mem show <id> | head -1` -- is the reader's
/// business, not a failure of the program. Rust ignores SIGPIPE, so the write
/// comes back EPIPE and `println!` turns that into a panic and exit 101
/// (friction #ECTJYVXX). Neither belongs on a CLI.
#[test]
fn a_reader_that_closes_the_pipe_is_not_an_error() {
    let w = World::new("exit-epipe");
    w.project(P, "thing");
    let store = w.store();
    // Larger than any pipe buffer, so the write cannot quietly succeed.
    let mut it = item(Kind::Fact, "a long one", &"x".repeat(400_000));
    it.meta.id = "01K2YR1VC0AB3DE4FG5HJ6KM7N".to_string();
    put(&store, Some(P), &it);

    let dirs = w.dirs();
    let dir = w.plain_dir("cwd");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_mem"))
        .current_dir(&dir)
        .args(["show", "5HJ6KM7N"])
        .env("XDG_DATA_HOME", &dirs.data)
        .env("XDG_CACHE_HOME", &dirs.cache)
        .env("XDG_STATE_HOME", &dirs.state)
        .env("XDG_CONFIG_HOME", &dirs.config)
        .env_remove("MEM_SESSION_ID")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn mem");
    drop(child.stdout.take());
    let out = child.wait_with_output().expect("wait");

    assert_ne!(code(&out), 101, "mem panicked: {}", stderr(&out));
    assert!(
        !stderr(&out).contains("panicked"),
        "a stack trace reached the terminal: {}",
        stderr(&out)
    );
}
