//! H3 — §4a's table as a test matrix, and the cache in front of it.
//!
//! The matrix runs against a fixture `mem` on PATH so every row can be produced
//! on demand; the last two tests run the **real** binary against a throwaway
//! store, because a fixture can only ever confirm what I already believed.

mod common;

use std::path::Path;
use std::time::Duration;

use common::{TempDir, fixture_mem, mem_in, real_mem, seed_project};
use hub::memcli::{MemCli, Outcome};

/// One script that answers every row of the table, chosen by verb.
const MATRIX: &str = r#"
case "$1" in
  questions) echo '{"questions":[]}'; exit 1 ;;
  log)       echo '{"items":[]}'; exit 1 ;;
  projects)  echo '{"projects":[]}'; exit 0 ;;
  status)    echo "mem: nothing recorded" >&2; exit 1 ;;
  answer)    echo "mem: no question DEADBEEF" >&2; exit 1 ;;
  garbage)   echo 'this is not json'; exit 1 ;;
  crash)     echo '{"half":' ; exit 0 ;;
  *)         echo "mem: unknown verb $1" >&2; exit 2 ;;
esac
"#;

fn matrix(dir: &TempDir) -> MemCli {
    let bin = dir.join("bin");
    fixture_mem(&bin, MATRIX);
    MemCli::with_path(bin)
}

#[test]
fn exit_one_with_a_document_is_an_empty_result() {
    let dir = TempDir::new("mem-empty");
    let mem = matrix(&dir);

    // The payload is an object keyed by the verb, never a bare `[]`, so the
    // test is of the array under the key.
    let questions = mem.questions();
    assert!(matches!(*questions, Outcome::Json(_)));
    assert!(questions.rows("questions").is_empty());
    assert!(
        questions.broken().is_none(),
        "an empty queue is not a fault"
    );

    let log = mem.log("proj-alpha");
    assert!(matches!(*log, Outcome::Json(_)));
    assert!(log.rows("items").is_empty());
    assert!(log.broken().is_none());
}

#[test]
fn exit_zero_with_a_document_is_the_same_shape() {
    let dir = TempDir::new("mem-zero");
    let mem = matrix(&dir);

    let projects = mem.projects();
    assert!(matches!(*projects, Outcome::Json(_)));
    assert!(projects.rows("projects").is_empty());
}

#[test]
fn exit_one_with_empty_stdout_is_absent_and_never_parsed() {
    let dir = TempDir::new("mem-absent");
    let mem = matrix(&dir);

    // The Projects section hits this row for every project without a
    // status.md, which is most of them. It renders `—`, not an error.
    let status = mem.status("proj-alpha");
    assert!(matches!(*status, Outcome::Absent), "{status:?}");
    assert!(status.broken().is_none(), "absent is not the degraded page");
    assert!(status.rows("text").is_empty());
}

#[test]
fn stderr_is_never_treated_as_json() {
    let dir = TempDir::new("mem-stderr");
    let mem = matrix(&dir);

    let run = mem.answer("DEADBEEF", "whatever");
    assert!(!run.ok());
    assert_eq!(run.code, Some(1));
    assert!(run.stdout.is_empty());
    assert_eq!(run.stderr, "mem: no question DEADBEEF");
}

#[test]
fn only_unparseable_output_and_a_missing_binary_are_broken() {
    let dir = TempDir::new("mem-broken");
    let mem = matrix(&dir);

    assert!(mem.read(&["garbage"]).broken().is_some());
    assert!(mem.read(&["crash"]).broken().is_some());

    // An empty directory on PATH: `mem` cannot be run at all.
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let missing = MemCli::with_path(empty);
    let outcome = missing.projects();
    let why = outcome.broken().expect("a missing binary is broken");
    assert!(why.contains("could not run mem"), "{why}");
}

#[test]
fn reads_are_cached_for_the_ttl_and_dropped_on_demand() {
    let dir = TempDir::new("mem-cache");
    let bin = dir.join("bin");
    let counter = dir.join("calls");
    fixture_mem(
        &bin,
        &format!(
            "printf 'x' >> '{}'\necho '{{\"projects\":[]}}'",
            counter.display()
        ),
    );
    let mem = MemCli::with_path(&bin);

    for _ in 0..5 {
        mem.projects();
    }
    assert_eq!(calls(&counter), 1, "five refreshes, one process");

    mem.invalidate();
    mem.projects();
    assert_eq!(calls(&counter), 2, "an answer drops the cache immediately");
}

#[test]
fn the_cache_is_per_argv_so_one_project_does_not_shadow_another() {
    let dir = TempDir::new("mem-cache-key");
    let bin = dir.join("bin");
    let counter = dir.join("calls");
    fixture_mem(
        &bin,
        &format!(
            "printf 'x' >> '{}'\necho '{{\"items\":[]}}'",
            counter.display()
        ),
    );
    let mem = MemCli::with_path(&bin);

    mem.log("proj-alpha");
    mem.log("proj-beta");
    mem.log("proj-alpha");
    assert_eq!(calls(&counter), 2, "two projects, two calls, then a hit");
}

#[test]
fn a_cache_entry_expires() {
    let dir = TempDir::new("mem-ttl");
    let bin = dir.join("bin");
    let counter = dir.join("calls");
    fixture_mem(
        &bin,
        &format!(
            "printf 'x' >> '{}'\necho '{{\"projects\":[]}}'",
            counter.display()
        ),
    );
    let mem = MemCli::with_path(&bin).with_ttl(Duration::from_millis(50));

    mem.projects();
    mem.projects();
    assert_eq!(calls(&counter), 1);
    std::thread::sleep(Duration::from_millis(80));
    mem.projects();
    assert_eq!(calls(&counter), 2);
}

#[test]
fn nothing_reaches_a_shell() {
    let dir = TempDir::new("mem-argv");
    let bin = dir.join("bin");
    let seen = dir.join("argv");
    // `"$@"` one per line: if any argument had been split by a shell, or
    // expanded, the file would show it.
    fixture_mem(
        &bin,
        &format!(
            "for a in \"$@\"; do echo \"$a\" >> '{}'; done",
            seen.display()
        ),
    );
    let mem = MemCli::with_path(&bin);

    let hostile = "\"; touch /tmp/hub-test-pwned; #";
    mem.answer("01ABC", hostile);

    let recorded = std::fs::read_to_string(&seen).unwrap();
    let lines: Vec<&str> = recorded.lines().collect();
    assert_eq!(lines, vec!["answer", "--", "01ABC", hostile]);
    assert!(
        !Path::new("/tmp/hub-test-pwned").exists(),
        "the answer field reached a shell"
    );
}

// ---------------------------------------------------------------------------
// Against the real binary.
// ---------------------------------------------------------------------------

/// A `mem` on PATH that is the real binary pointed at a throwaway store, and
/// run from a directory that is **not** a git checkout — which is what the
/// systemd unit gets, since `WorkingDirectory` defaults to `%h`.
fn real_mem_on_path(dir: &TempDir, home: &Path) -> Option<MemCli> {
    let real = real_mem()?;
    let neutral = dir.join("not-a-checkout");
    std::fs::create_dir_all(&neutral).unwrap();
    let bin = dir.join("bin");
    fixture_mem(
        &bin,
        &format!(
            "cd '{}' || exit 1\n\
             export HOME='{}'\n\
             export XDG_DATA_HOME='{}/data' XDG_CACHE_HOME='{}/cache'\n\
             export XDG_STATE_HOME='{}/state' XDG_CONFIG_HOME='{}/config'\n\
             export MEM_SYNC_CMD=true MEM_NOTIFY_CMD=true\n\
             exec '{}' \"$@\"",
            neutral.display(),
            home.display(),
            home.display(),
            home.display(),
            home.display(),
            home.display(),
            real.display(),
        ),
    );
    Some(MemCli::with_path(bin))
}

#[test]
fn the_table_holds_against_the_real_binary() {
    let dir = TempDir::new("mem-real");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let Some(real) = real_mem() else {
        panic!("build mem first: cargo build --release --manifest-path ../mem/Cargo.toml");
    };
    let mem = real_mem_on_path(&dir, &home).unwrap();

    // An empty store: every list is empty, and none of it is broken.
    assert!(mem.questions().rows("questions").is_empty());
    assert!(mem.projects().rows("projects").is_empty());
    assert!(mem.questions().broken().is_none());
    assert!(mem.projects().broken().is_none());

    seed_project(&real, &home, "proj-alpha", "alpha did a thing");
    seed_project(&real, &home, "proj-beta", "beta did a thing");
    mem.invalidate();

    let projects = mem.projects();
    let names: Vec<String> = projects
        .rows("projects")
        .iter()
        .filter_map(|p| p["name"].as_str().map(str::to_string))
        .collect();
    assert_eq!(names, vec!["proj-alpha", "proj-beta"]);

    // AC7: a project with no status.md is absent, not an error — and this is
    // the row a fixture is least able to prove.
    let status = mem.status("proj-alpha");
    assert!(matches!(*status, Outcome::Absent), "{status:?}");
    assert!(status.broken().is_none());

    // An unknown project is the same shape: empty stdout, plain-text stderr.
    let status = mem.status("no-such-project");
    assert!(matches!(*status, Outcome::Absent), "{status:?}");

    // With a status.md it is a document.
    let out = mem_in(
        &real,
        &home,
        &home.join("proj-alpha"),
        &["status", "--set", "Green. Everything builds."],
    );
    assert!(out.status.success());
    mem.invalidate();
    let status = mem.status("proj-alpha");
    match &*status {
        Outcome::Json(value) => assert!(value["text"].as_str().unwrap().starts_with("Green.")),
        other => panic!("{other:?}"),
    }

    // §4b's fan-out: per-project, because there is no --all-projects on log.
    assert_eq!(mem.log("proj-alpha").rows("items").len(), 1);
    assert_eq!(mem.log("proj-beta").rows("items").len(), 1);
    // And an unknown project's log is an empty result, not a fault.
    let log = mem.log("no-such-project");
    assert!(log.broken().is_none());
    assert!(log.rows("items").is_empty());
}

#[test]
fn a_real_answer_round_trips_and_an_unknown_id_is_a_clean_exit_one() {
    let dir = TempDir::new("mem-real-answer");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let Some(real) = real_mem() else {
        panic!("build mem first");
    };
    let mem = real_mem_on_path(&dir, &home).unwrap();

    seed_project(&real, &home, "proj-alpha", "alpha did a thing");
    let out = mem_in(
        &real,
        &home,
        &home.join("proj-alpha"),
        &["ask", "Should we use Redis?"],
    );
    assert!(out.status.success(), "{out:?}");
    mem.invalidate();

    let questions = mem.questions();
    let rows = questions.rows("questions");
    assert_eq!(rows.len(), 1);
    let id = rows[0]["id"].as_str().unwrap().to_string();
    assert_eq!(rows[0]["title"], "Should we use Redis?");
    assert_eq!(rows[0]["project"], "proj-alpha");

    // An answer that begins with a dash: without the `--`, clap reads it as a
    // flag and mem exits 2 before writing anything (review m-9).
    let run = mem.answer(&id, "-x use sqlite");
    assert!(run.ok(), "{run:?}");

    mem.invalidate();
    assert!(
        mem.questions().rows("questions").is_empty(),
        "the answered question left the pending queue"
    );

    // §3's failure mode: exit 1 and plain text, which the page turns into a
    // banner rather than a 500.
    let run = mem.answer("DEADBEEF", "no such question");
    assert_eq!(run.code, Some(1));
    assert!(run.stdout.is_empty());
    assert!(run.stderr.contains("DEADBEEF"), "{run:?}");
}

fn calls(counter: &Path) -> usize {
    std::fs::read_to_string(counter)
        .map(|s| s.len())
        .unwrap_or(0)
}
