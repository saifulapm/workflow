//! Maintenance: prune, snapshot, doctor, reindex, sync and the version gate
//! (spec §7, §10, AC14).

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{World, code, item, mem, mem_env, put, stdout};
use mem::index::{Index, Purpose};
use mem::item::Kind;
use mem::maint;

const P: &str = "01K2AAAAAAAAAAAAAAAAAAAAAA";

fn script(w: &World, name: &str, body: &str) -> PathBuf {
    let path = w.dir.join(name);
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

/// A stand-in for qshell-sync that records the call and then takes its time. A
/// trigger that waited for it would be unmistakable.
fn slow_sync_stub(w: &World, marker: &Path) -> PathBuf {
    script(
        w,
        "slow-sync.sh",
        &format!("#!/bin/sh\ntouch '{}'\nsleep 5\n", marker.display()),
    )
}

fn appeared(path: &Path, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    path.exists()
}

fn status_file(w: &World, running: bool) -> PathBuf {
    let path = w.dirs().qshell_status_json();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        format!(
            r#"{{"version":2,"lastRun":"2026-08-19T10:00:00+06:00","running":{running},
                "currentUnit":"notes","units":[]}}"#
        ),
    )
    .unwrap();
    path
}

fn indexed(w: &World) -> Index {
    let index = Index::open(&w.index_path(), Purpose::Read).unwrap();
    index.reindex(&w.store(), false).unwrap();
    index
}

fn days_ago(days: i64) -> jiff::Timestamp {
    jiff::Timestamp::from_second(jiff::Timestamp::now().as_second() - days * 86_400).unwrap()
}

#[test]
fn prune_lists_only_the_stale_and_archives_in_place() {
    let w = World::new("maint-prune");
    w.project(P, "thing");
    let store = w.store();

    let mut old_log = item(Kind::Log, "an old log", "body");
    old_log.meta.modified = days_ago(120);
    let old_log_path = put(&store, Some(P), &old_log);
    let mut fresh_log = item(Kind::Log, "a fresh log", "body");
    fresh_log.meta.modified = days_ago(3);
    put(&store, Some(P), &fresh_log);
    let mut old_fact = item(Kind::Fact, "an untouched fact", "body");
    old_fact.meta.modified = days_ago(200);
    put(&store, Some(P), &old_fact);
    let mut pinned = item(Kind::Fact, "a pinned fact", "body");
    pinned.meta.modified = days_ago(400);
    pinned.meta.tags = vec!["pinned".into()];
    put(&store, Some(P), &pinned);

    let candidates = maint::prune_candidates(&indexed(&w), jiff::Timestamp::now()).unwrap();
    let titles: Vec<&str> = candidates.iter().map(|c| c.title.as_str()).collect();
    assert!(titles.contains(&"an old log"), "{titles:?}");
    assert!(titles.contains(&"an untouched fact"), "{titles:?}");
    assert!(!titles.contains(&"a fresh log"), "{titles:?}");
    assert!(
        !titles.contains(&"a pinned fact"),
        "pinned is exempt: {titles:?}"
    );

    // Applying it archives in place: the file stays where it is, so bisync sees
    // a modification rather than a delete plus a create.
    let out = mem(
        &w,
        &w.plain_dir("cwd"),
        &[
            "prune",
            "--apply",
            &old_log.meta.short_id(),
            "--project",
            "thing",
        ],
    );
    assert_eq!(code(&out), 0, "{}", common::stderr(&out));
    assert!(old_log_path.exists(), "prune must never delete a file");
    let archived = mem::store::read_item(&old_log_path).unwrap();
    assert!(archived.meta.is_archived());
    assert!(archived.meta.archived_at.is_some());
    assert_eq!(store.item_paths().len(), 4, "no file was removed");

    // And a snapshot was taken first.
    let snaps: Vec<_> = std::fs::read_dir(w.dirs().snapshots_dir())
        .unwrap()
        .collect();
    assert_eq!(snaps.len(), 1, "prune --apply snapshots first");
}

#[test]
fn snapshots_keep_the_last_fourteen() {
    let w = World::new("maint-snapshot");
    w.project(P, "thing");
    put(&w.store(), Some(P), &item(Kind::Fact, "a", "body"));
    let dir = w.dirs().snapshots_dir();
    for n in 0..16 {
        let now = jiff::Timestamp::from_second(1_800_000_000 + n * 3_600).unwrap();
        maint::snapshot(&w.store(), &dir, now).unwrap();
    }
    let kept: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(kept.len(), maint::SNAPSHOT_KEEP);
}

#[test]
fn doctor_reports_strays_conflicts_forks_and_secrets() {
    let w = World::new("maint-doctor");
    w.project(P, "thing");
    let store = w.store();
    let target = item(Kind::Fact, "the original", "body");
    put(&store, Some(P), &target);
    for t in ["fork a", "fork b"] {
        let mut f = item(Kind::Fact, t, "body");
        f.meta.supersedes = Some(target.meta.id.clone());
        put(&store, Some(P), &f);
    }
    let dir = store.project_items(P);
    std::fs::write(dir.join(".tmp-9-01K2YR1VC0AB3DE4FG5HJ6KM7N.md"), b"x").unwrap();
    std::fs::write(dir.join("01K2YR1VC0AB3DE4FG5HJ6KM7P.md.path1"), b"x").unwrap();
    put(
        &store,
        Some(P),
        &item(Kind::Fact, "leaky", "AKIAIOSFODNN7EXAMPLE is in here"),
    );

    let out = mem(&w, &w.plain_dir("cwd"), &["doctor", "--json"]);
    assert_eq!(code(&out), 0, "findings are exit 0");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let checks: Vec<&str> = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["check"].as_str().unwrap())
        .collect();
    assert!(checks.contains(&"temp"), "{checks:?}");
    assert!(checks.contains(&"conflict"), "{checks:?}");
    assert!(checks.contains(&"supersede fork"), "{checks:?}");
    assert!(checks.contains(&"secret"), "{checks:?}");
    assert!(checks.contains(&"sync"), "{checks:?}");
}

#[test]
fn doctor_names_the_item_files_nothing_can_read() {
    let w = World::new("maint-unreadable");
    w.project(P, "thing");
    let store = w.store();
    put(&store, Some(P), &item(Kind::Fact, "a readable one", "body"));
    let dir = store.project_items(P);
    // Two files with perfectly good item names and nothing parseable inside:
    // one truncated mid-round, one a human dropped in by hand. Files are the
    // source of truth here, so a file no read can see is worth saying out loud.
    let planted = [
        (
            "01K2YR1VC0AB3DE4FG5HJ6KM7N.md",
            "+++\nid = \"01K2YR1VC0AB3DE4FG5HJ6KM7N\"\nkind = \"fact\"\n",
        ),
        ("01K2YR1VC0AB3DE4FG5HJ6KM7P.md", "just some notes\n"),
    ];
    for (name, body) in planted {
        std::fs::write(dir.join(name), body).unwrap();
    }

    let out = mem(&w, &w.plain_dir("cwd"), &["reindex", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["unreadable"],
        serde_json::json!(2),
        "the reindex counts them"
    );

    let out = mem(&w, &w.plain_dir("cwd"), &["doctor", "--json"]);
    assert_eq!(code(&out), 0, "findings are exit 0");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let unreadable: Vec<String> = v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["check"] == "unreadable")
        .map(|f| f["detail"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(unreadable.len(), 2, "{unreadable:?}");
    for (name, _) in planted {
        assert!(
            unreadable.iter().any(|d| d.contains(name)),
            "{name} is missing from {unreadable:?}"
        );
    }

    // And the readable one is not accused of anything.
    assert!(
        !unreadable.iter().any(|d| d.contains("a readable one")),
        "{unreadable:?}"
    );
}

#[test]
fn the_secret_heuristic_knows_what_it_is_looking_for() {
    assert!(maint::looks_like_a_secret("AKIAIOSFODNN7EXAMPLE").is_some());
    assert!(maint::looks_like_a_secret("-----BEGIN RSA PRIVATE KEY-----").is_some());
    // Encoded bytes: long, unbroken, and carrying all three character classes.
    assert!(maint::looks_like_a_secret(&"aB3xY7zQ".repeat(16)).is_some());
    assert!(maint::looks_like_a_secret("sessions use redis, not the database").is_none());
    assert!(maint::looks_like_a_secret("").is_none());
}

#[test]
fn the_secret_heuristic_wants_a_mixture_rather_than_a_long_run() {
    // Length alone said "secret" to all of these, and none of them is one.
    assert!(
        maint::looks_like_a_secret(&"=".repeat(200)).is_none(),
        "a rule of one repeated character is not a key"
    );
    assert!(
        maint::looks_like_a_secret(&"A".repeat(200)).is_none(),
        "nor is a run of one letter"
    );
    let sha512 = "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce\
                  47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e";
    assert_eq!(sha512.len(), 128);
    assert!(
        maint::looks_like_a_secret(sha512).is_none(),
        "a hex digest has no uppercase and is not a secret"
    );
    assert!(
        maint::looks_like_a_secret(&sha512.to_uppercase()).is_none(),
        "nor is the same digest shouted"
    );
    // The check is per unbroken run, so ordinary prose never accumulates one.
    assert!(maint::looks_like_a_secret(&"redis sessions ".repeat(40)).is_none());
}

#[test]
fn a_newer_store_refuses_writes_and_still_serves_reads() {
    let w = World::new("maint-version");
    let repo = w.repo("thing", None);
    mem(&w, &repo, &["save", "a fact before the bump"]);
    std::fs::write(w.store().version_path(), b"99\n").unwrap();

    let out = mem(&w, &repo, &["save", "a fact after the bump"]);
    assert_eq!(code(&out), 3, "a write against a newer store must refuse");
    assert!(
        common::stderr(&out).contains("upgrade mem"),
        "{}",
        common::stderr(&out)
    );

    let out = mem(&w, &repo, &["search", "fact"]);
    assert_eq!(code(&out), 0, "reads must still work");
    let out = mem(&w, &repo, &["doctor"]);
    assert!(stdout(&out).contains("version"), "{}", stdout(&out));

    // §3 asks reads to degrade rather than refuse, and to say so in one line —
    // a synced VERSION bump must not take another machine's sessions down, but
    // it must not be silent either.
    let out = mem(&w, &repo, &["context"]);
    assert_eq!(code(&out), 0, "the digest still serves");
    let first = stdout(&out).lines().next().unwrap_or_default().to_string();
    assert!(first.starts_with("! store format 99"), "{}", stdout(&out));
}

#[test]
fn the_outbox_holds_a_write_the_store_could_not_take_and_doctor_replays_it() {
    let w = World::new("maint-outbox");
    w.project(P, "thing");
    let outbox = w.dirs().outbox_dir();
    let mut spooled = item(Kind::Fact, "a spooled fact", "body");
    spooled.meta.project = Some("thing".to_string());
    maint::spool(&outbox, &spooled).unwrap();
    assert_eq!(maint::outbox_backlog(&outbox).len(), 1);

    let out = mem(&w, &w.plain_dir("cwd"), &["doctor"]);
    assert!(stdout(&out).contains("spooled write"), "{}", stdout(&out));

    let out = mem(&w, &w.plain_dir("cwd"), &["doctor", "--fix"]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains("replayed 1"), "{}", stdout(&out));
    assert!(maint::outbox_backlog(&outbox).is_empty());
    assert!(
        w.store()
            .project_items(P)
            .join(format!("{}.md", spooled.meta.id))
            .exists()
    );
}

#[test]
fn sync_verifies_against_the_status_file_rather_than_the_exit_code() {
    // The seam stands in for qshell-sync, which is not installed everywhere and
    // has no memory unit until the dotfiles change lands.
    unsafe { std::env::set_var("MEM_SYNC_CMD", "true") };
    let w = World::new("maint-sync");
    let status = w.dirs().qshell_status_json();
    std::fs::create_dir_all(status.parent().unwrap()).unwrap();

    // qshell-sync exits 0 when its flock is held, so a round that never ran
    // looks like success unless the status file is compared.
    std::fs::write(
        &status,
        br#"{"version":2,"lastRun":"2026-08-18T10:00:00+06:00","running":false,
            "units":[{"id":"memory","ok":true,"lastRun":"2026-08-18T10:00:00+06:00",
                      "lastOk":"2026-08-18T10:00:00+06:00","error":"","errorKind":""}]}"#,
    )
    .unwrap();
    let outcome = mem::sync::verified(&status, std::time::Duration::from_millis(1)).unwrap();
    assert!(
        matches!(outcome, mem::sync::Outcome::Deferred { .. }),
        "an unchanged status file means the round did not happen: {outcome:?}"
    );

    // A round already in progress is reported as such, not as success.
    std::fs::write(
        &status,
        br#"{"version":2,"lastRun":"2026-08-18T10:00:00+06:00","running":true,"units":[]}"#,
    )
    .unwrap();
    let outcome = mem::sync::verified(&status, std::time::Duration::from_millis(1)).unwrap();
    assert!(
        matches!(outcome, mem::sync::Outcome::NotPerformed { .. }),
        "{outcome:?}"
    );
}

#[test]
fn asking_a_question_does_not_wait_for_the_sync_round_it_asks_for() {
    let w = World::new("sync-detached");
    let repo = w.repo("thing", None);
    let marker = w.dir.join("sync-ran");
    let stub = slow_sync_stub(&w, &marker);

    let started = Instant::now();
    let out = mem_env(
        &w,
        &repo,
        &["ask", "does the trigger wait?"],
        &[
            ("MEM_SYNC_CMD", stub.to_str().unwrap()),
            ("MEM_NOTIFY_CMD", "true"),
        ],
    );
    let elapsed = started.elapsed();
    assert_eq!(code(&out), 0, "{}", common::stderr(&out));
    assert!(
        elapsed < Duration::from_secs(1),
        "ask sat through the whole sync round: {elapsed:?}"
    );
    // It did ask for one, though — the round is still going while mem is gone.
    assert!(
        appeared(&marker, Duration::from_secs(5)),
        "the trigger never ran the sync command"
    );
}

#[test]
fn a_trigger_stands_down_while_a_round_is_already_in_progress() {
    let w = World::new("sync-running");
    let repo = w.repo("thing", None);
    let marker = w.dir.join("sync-ran");
    let stub = slow_sync_stub(&w, &marker);
    let sync_env = [("MEM_SYNC_CMD", stub.to_str().unwrap())];

    status_file(&w, true);
    let out = mem_env(&w, &repo, &["handoff", "--set", "park it"], &sync_env);
    assert_eq!(code(&out), 0, "{}", common::stderr(&out));
    assert!(
        !appeared(&marker, Duration::from_millis(300)),
        "qshell-sync holds a flock, so asking for a second round buys nothing"
    );

    // Once that round is over the same handoff does ask for one.
    status_file(&w, false);
    let out = mem_env(&w, &repo, &["handoff", "--set", "park it again"], &sync_env);
    assert_eq!(code(&out), 0, "{}", common::stderr(&out));
    assert!(
        appeared(&marker, Duration::from_secs(5)),
        "with no round in progress the trigger fires"
    );
}

#[test]
fn a_round_qshell_sync_ran_and_failed_is_a_report_not_an_error() {
    let w = World::new("sync-unit-failed");
    let cwd = w.plain_dir("cwd");
    let failing = script(
        &w,
        "failing-sync.sh",
        "#!/bin/sh\necho 'memory: no such unit' >&2\nexit 1\n",
    );

    // qshell-sync WAS invoked and the round failed. Exit 3 is reserved for
    // "could not invoke it at all" — and a failing unit is the state of every
    // machine that has not wired `memory` up yet, including this one.
    let out = mem_env(
        &w,
        &cwd,
        &["sync", "--json"],
        &[("MEM_SYNC_CMD", failing.to_str().unwrap())],
    );
    assert_eq!(code(&out), 0, "{}", common::stderr(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["sync"], serde_json::json!("not performed"));
    assert!(
        v["detail"].as_str().unwrap().contains("no such unit"),
        "the report says what went wrong: {v}"
    );

    // A qshell-sync that is not on this machine at all is still exit 3.
    let out = mem_env(
        &w,
        &cwd,
        &["sync"],
        &[("MEM_SYNC_CMD", "mem-test-no-such-sync-command")],
    );
    assert_eq!(code(&out), 3, "{}", common::stderr(&out));
}

#[test]
fn a_reindex_under_a_held_lock_says_so_in_plain_output_too() {
    let w = World::new("maint-reindex-busy");
    w.project(P, "thing");
    put(&w.store(), Some(P), &item(Kind::Fact, "a", "body"));
    let cwd = w.plain_dir("cwd");
    assert_eq!(code(&mem(&w, &cwd, &["reindex"])), 0);

    // Another invocation is mid-reindex and holds the write lock. Reporting
    // "indexed 0" here reads as "there was nothing to do", which is the one
    // thing it does not mean.
    let mut holder = Index::open(&w.index_path(), Purpose::Read).unwrap();
    let held = holder.begin_immediate().expect("hold the write lock");
    let out = mem(&w, &cwd, &["reindex"]);
    drop(held);

    assert_eq!(code(&out), 0, "a busy index is not a failure");
    assert!(
        stdout(&out).contains("skipped: another process holds the index"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn pruning_an_id_that_is_not_a_candidate_is_exit_one() {
    let w = World::new("maint-prune-unknown");
    w.project(P, "thing");
    let store = w.store();
    let mut first = item(Kind::Log, "the first old log", "body");
    first.meta.modified = days_ago(120);
    put(&store, Some(P), &first);
    let mut second = item(Kind::Log, "the second old log", "body");
    second.meta.modified = days_ago(120);
    put(&store, Some(P), &second);
    let cwd = w.plain_dir("cwd");

    // A silent no-op is the worst answer here: the caller believes it archived
    // something it did not.
    let out = mem(
        &w,
        &cwd,
        &["prune", "--apply", "ZZZZZZZZ", "--project", "thing"],
    );
    assert_eq!(code(&out), 1, "{}", stdout(&out));
    assert!(
        common::stderr(&out).contains("ZZZZZZZZ"),
        "{}",
        common::stderr(&out)
    );

    // A known id alongside an unknown one still lands, and the unknown one is
    // still named.
    let out = mem(
        &w,
        &cwd,
        &[
            "prune",
            "--apply",
            &first.meta.short_id(),
            "ZZZZZZZZ",
            "--project",
            "thing",
            "--json",
        ],
    );
    assert_eq!(code(&out), 1);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["archived"],
        serde_json::json!([first.meta.short_id()]),
        "the report still conforms to the schema"
    );
    assert!(common::stderr(&out).contains("ZZZZZZZZ"));

    // `all` is the documented wildcard and complains about nothing.
    let out = mem(&w, &cwd, &["prune", "--apply", "all", "--project", "thing"]);
    assert_eq!(code(&out), 0, "{}", common::stderr(&out));
    let archived = mem::store::read_item(
        &store
            .project_items(P)
            .join(format!("{}.md", second.meta.id)),
    )
    .unwrap();
    assert!(archived.meta.is_archived());
}

#[test]
fn a_stale_sync_puts_one_warning_line_at_the_top_of_the_digest() {
    let w = World::new("maint-stale");
    let repo = w.repo("thing", None);
    mem(&w, &repo, &["save", "a fact"]);
    let status = w.dirs().qshell_status_json();
    std::fs::create_dir_all(status.parent().unwrap()).unwrap();
    let long_ago = jiff::Timestamp::from_second(jiff::Timestamp::now().as_second() - 7200).unwrap();
    std::fs::write(
        &status,
        format!(
            r#"{{"version":2,"lastRun":"{0}","running":false,
                "units":[{{"id":"memory","ok":true,"lastRun":"{0}","lastOk":"{0}",
                           "error":"","errorKind":""}}]}}"#,
            long_ago
        ),
    )
    .unwrap();

    let out = mem(&w, &repo, &["context"]);
    assert_eq!(code(&out), 0);
    let first = stdout(&out).lines().next().unwrap_or_default().to_string();
    assert!(first.starts_with("! memory last synced"), "{first}");
}

#[test]
fn reindex_reports_what_it_did() {
    let w = World::new("maint-reindex");
    w.project(P, "thing");
    put(&w.store(), Some(P), &item(Kind::Fact, "a", "body"));
    let out = mem(&w, &w.plain_dir("cwd"), &["reindex", "--full", "--json"]);
    assert_eq!(code(&out), 0);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["indexed"], serde_json::json!(1));
}
