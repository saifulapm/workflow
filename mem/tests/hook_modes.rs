//! The runtime hook modes (spec §9). Each of these is the machine half of an
//! AC11 row: the transcript-level halves are in `mem/TESTING.md`, because they
//! need a live Claude Code session.
//!
//! Two shapes are load-bearing and were checked against the 2.1.233 binary:
//! PostToolBatch and Stop read `hookSpecificOutput.additionalContext` (and the
//! event name must match the hook that ran, or the binary throws), while
//! PreCompact has no `hookSpecificOutput` variant at all — its channel is the
//! hook's plain stdout, which becomes `newCustomInstructions`.

mod common;

use common::{World, code, item, mem, put, stderr, stdout};
use mem::item::Kind;

const P: &str = "01K2AAAAAAAAAAAAAAAAAAAAAA";

fn json(out: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).expect("json output")
}

#[test]
fn the_batch_brief_is_emitted_on_every_fifth_batch_and_not_before() {
    let w = World::new("hook-batch");
    w.project(P, "thing");
    put(
        &w.store(),
        Some(P),
        &item(Kind::Handoff, "stopped mid-migration", "next: run it"),
    );
    let repo = w.plain_dir("cwd");

    let hook = [
        "context",
        "thing",
        "--brief",
        "--hook-json",
        "--session-id",
        "hook-s1",
    ];
    for batch in 1..=4 {
        let out = mem(&w, &repo, &hook);
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        assert!(
            stdout(&out).is_empty(),
            "batch {batch} must emit nothing: {}",
            stdout(&out)
        );
    }

    let out = mem(&w, &repo, &hook);
    assert_eq!(code(&out), 0);
    let v = json(&out);
    assert_eq!(
        v["hookSpecificOutput"]["hookEventName"],
        serde_json::json!("PostToolBatch")
    );
    let context = v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additionalContext");
    assert!(context.contains("stopped mid-migration"), "{context}");
    assert!(
        context.len() <= 480,
        "the brief budget is counted on the content string"
    );
    assert_eq!(
        v.as_object().unwrap().keys().collect::<Vec<_>>(),
        vec!["hookSpecificOutput"],
        "no key outside the envelope the binary validates"
    );

    // The counter lives in the session file, so the next four are quiet again.
    assert_eq!(
        mem::session::read(&w.dirs().sessions_dir(), "hook-s1").batches,
        5
    );
    for _ in 6..=9 {
        assert!(stdout(&mem(&w, &repo, &hook)).is_empty());
    }
    assert!(
        !stdout(&mem(&w, &repo, &hook)).is_empty(),
        "the tenth batch"
    );

    // A second session counts on its own.
    let other = [
        "context",
        "thing",
        "--brief",
        "--hook-json",
        "--session-id",
        "hook-s2",
    ];
    assert!(stdout(&mem(&w, &repo, &other)).is_empty());
}

#[test]
fn a_batch_with_nothing_to_say_emits_nothing() {
    let w = World::new("hook-batch-empty");
    let dir = w.plain_dir("nowhere");
    for _ in 1..=5 {
        let out = mem(
            &w,
            &dir,
            &["context", "--brief", "--hook-json", "--session-id", "s"],
        );
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        assert!(
            stdout(&out).is_empty(),
            "an empty brief is not worth an injection: {}",
            stdout(&out)
        );
    }
}

#[test]
fn the_stop_nudge_appears_only_in_a_session_that_wrote_nothing() {
    let w = World::new("hook-stop");
    let repo = w.repo("thing", None);

    let out = mem(
        &w,
        &repo,
        &["session-check", "--session-id", "quiet", "--hook-json"],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v = json(&out);
    assert_eq!(
        v["hookSpecificOutput"]["hookEventName"],
        serde_json::json!("Stop")
    );
    let nudge = v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additionalContext");
    assert!(nudge.contains("mem handoff"), "{nudge}");
    assert!(
        nudge.len() <= 240,
        "the nudge is ~30 tokens, not a lecture: {} bytes",
        nudge.len()
    );

    // One write is enough to make the session worth nothing further.
    assert_eq!(
        code(&mem(
            &w,
            &repo,
            &["log", "did the thing", "--session-id", "busy"]
        )),
        0
    );
    let out = mem(
        &w,
        &repo,
        &["session-check", "--session-id", "busy", "--hook-json"],
    );
    assert_eq!(code(&out), 0);
    assert!(
        stdout(&out).is_empty(),
        "a session that recorded something needs no nudge: {}",
        stdout(&out)
    );

    // Without the hook envelope it is still readable, and --json reports both.
    let out = mem(
        &w,
        &repo,
        &["session-check", "--session-id", "busy", "--json"],
    );
    assert_eq!(code(&out), 0);
    let v = json(&out);
    assert_eq!(v["writes"], serde_json::json!(1));
    assert_eq!(v["nudge"], serde_json::json!(null));
    let v = json(&mem(
        &w,
        &repo,
        &["session-check", "--session-id", "quiet", "--json"],
    ));
    assert_eq!(v["writes"], serde_json::json!(0));
    assert!(v["nudge"].is_string());
}

#[test]
fn the_stop_nudge_fires_at_most_once_in_a_session() {
    // The runtime treats a Stop hook's additionalContext as feedback to act on:
    // it wakes the model, which answers the nudge and stops again, which fires
    // the hook again. Answering a nudge is not a mem write, so a nudge that
    // repeats never stops repeating. One session, one nudge.
    let w = World::new("hook-stop-once");
    let repo = w.repo("thing", None);
    let args = ["session-check", "--session-id", "loop", "--hook-json"];

    let first = mem(&w, &repo, &args);
    assert_eq!(code(&first), 0, "{}", stderr(&first));
    assert!(
        !stdout(&first).is_empty(),
        "the first stop in a silent session is worth a nudge"
    );

    for round in 2..=4 {
        let out = mem(&w, &repo, &args);
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        assert!(
            stdout(&out).is_empty(),
            "stop {round} nudged again — this is the loop: {}",
            stdout(&out)
        );
    }

    // The diagnostic view still reports the session as having written nothing,
    // and now also says the one nudge has been spent.
    let v = json(&mem(
        &w,
        &repo,
        &["session-check", "--session-id", "loop", "--json"],
    ));
    assert_eq!(v["writes"], serde_json::json!(0));
    assert!(v["nudge"].is_string());
    assert_eq!(v["nudged"], serde_json::json!(true));
}

#[test]
fn a_session_with_no_id_is_never_nudged() {
    // Without a session id there is no record of what this session did, so
    // `writes == 0` is absence of evidence rather than evidence of silence —
    // and there is nowhere to record that the nudge was spent, so a nudge here
    // would fire on every stop forever.
    let w = World::new("hook-stop-noid");
    let repo = w.repo("thing", None);

    for args in [
        vec!["session-check", "--hook-json"],
        vec!["session-check", "--session-id", "", "--hook-json"],
    ] {
        let out = mem(&w, &repo, &args);
        assert_eq!(code(&out), 0, "{args:?}: {}", stderr(&out));
        assert!(
            stdout(&out).is_empty(),
            "{args:?} nudged a session it cannot track: {}",
            stdout(&out)
        );
    }
}

#[test]
fn precompact_speaks_plain_text_because_it_has_no_json_channel() {
    let w = World::new("hook-precompact");
    let dir = w.plain_dir("anywhere");
    let out = mem(&w, &dir, &["precompact", "--hook-json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        !text.trim_start().starts_with('{'),
        "JSON here would be parsed, fail schema validation and be dropped: {text}"
    );
    assert!(text.contains("handoff"), "{text}");
    assert!(text.contains("verbatim"), "{text}");
    assert!(!text.trim().is_empty());

    // Same text with no flag: the flag says how it is being run, not what to say.
    assert_eq!(stdout(&mem(&w, &dir, &["precompact"])), text);
}

#[test]
fn an_empty_session_id_is_no_session_id() {
    // The wiring passes `--session-id "$CLAUDE_CODE_SESSION_ID"`. If the
    // runtime ever leaves that unset, mem must not keep a session file named "".
    let w = World::new("hook-empty-session");
    let repo = w.repo("thing", None);
    assert_eq!(
        code(&mem(&w, &repo, &["log", "an entry", "--session-id", ""])),
        0
    );
    let sessions = w.dirs().sessions_dir();
    let kept: Vec<_> = std::fs::read_dir(&sessions)
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    assert!(kept.is_empty(), "{kept:?}");

    // And the batch counter simply emits, having nothing to count with.
    let out = mem(
        &w,
        &repo,
        &["context", "--brief", "--hook-json", "--session-id", ""],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}

#[test]
fn the_hook_verbs_never_fail_on_a_fresh_machine() {
    let w = World::new("hook-fresh");
    let dir = w.plain_dir("nowhere");
    for args in [
        vec!["context", "--brief", "--hook-json", "--session-id", "s"],
        vec!["session-check", "--session-id", "s", "--hook-json"],
        vec!["precompact", "--hook-json"],
    ] {
        let out = mem(&w, &dir, &args);
        assert_eq!(code(&out), 0, "{args:?}: {}", stderr(&out));
    }
}
