//! The runtime hook modes. These cover the machine half of each hook; the
//! transcript-level half needs a live Claude Code session and is not tested
//! here.
//!
//! Two shapes are load-bearing and were checked against the 2.1.233 binary:
//! PostToolBatch reads `hookSpecificOutput.additionalContext` (and the
//! event name must match the hook that ran, or the binary throws), while
//! PreCompact has no `hookSpecificOutput` variant at all — its channel is the
//! hook's plain stdout, which becomes `newCustomInstructions`.

mod common;

use common::{World, code, item, mem, mem_env, put, stderr, stdout};
use mem::item::Kind;

const P: &str = "01K2AAAAAAAAAAAAAAAAAAAAAA";

fn json(out: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).expect("json output")
}

/// The brief repeats what the session already knows unless something changed:
/// on a five-batch timer it re-sent the same handoff 2,843 times in two weeks
/// of sessions, each copy re-read on every later call.
#[test]
fn the_batch_brief_speaks_only_when_the_brief_changed() {
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
    // The session start's digest already carried this brief.
    for batch in 1..=6 {
        let out = mem(&w, &repo, &hook);
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        assert!(
            stdout(&out).is_empty(),
            "batch {batch} must emit nothing: {}",
            stdout(&out)
        );
    }

    put(
        &w.store(),
        Some(P),
        &item(Kind::Handoff, "migration landed", "next: tag it"),
    );
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
    assert!(context.contains("migration landed"), "{context}");
    assert!(
        context.len() <= 480,
        "the brief budget is counted on the content string"
    );
    assert_eq!(
        v.as_object().unwrap().keys().collect::<Vec<_>>(),
        vec!["hookSpecificOutput"],
        "no key outside the envelope the binary validates"
    );

    // Said once, it is known: the batches after are quiet again.
    for _ in 8..=12 {
        assert!(stdout(&mem(&w, &repo, &hook)).is_empty());
    }
    assert_eq!(
        mem::session::read(&w.dirs().sessions_dir(), "hook-s1").batches,
        12
    );

    // A second session starts from what its own session start showed.
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

/// While the sync unit is behind, the warning's age goes up every minute. A
/// number ticking up is not news, or a stale machine would hear the brief on
/// nearly every batch.
#[test]
fn a_ticking_sync_age_is_not_a_change() {
    let w = World::new("hook-batch-stale");
    w.project(P, "thing");
    let repo = w.plain_dir("cwd");
    let synced_ago = |minutes: i64| {
        let at = jiff::Timestamp::from_second(jiff::Timestamp::now().as_second() - minutes * 60)
            .unwrap();
        let path = w.dirs().qshell_status_json();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!(
                r#"{{"version":2,"lastRun":"{at}","running":false,"currentUnit":"memory",
                    "units":[{{"id":"memory","ok":true,"lastRun":"{at}","lastOk":"{at}"}}]}}"#
            ),
        )
        .unwrap();
    };
    let hook = [
        "context",
        "thing",
        "--brief",
        "--hook-json",
        "--session-id",
        "hook-stale",
    ];

    synced_ago(90);
    assert!(stdout(&mem(&w, &repo, &hook)).is_empty());
    synced_ago(95);
    let out = mem(&w, &repo, &hook);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).is_empty(),
        "an older age alone must emit nothing: {}",
        stdout(&out)
    );

    // A real change still speaks, with the warning as it reads now.
    put(
        &w.store(),
        Some(P),
        &item(Kind::Handoff, "migration landed", "next: tag it"),
    );
    let v = json(&mem(&w, &repo, &hook));
    let context = v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additionalContext");
    assert!(context.contains("migration landed"), "{context}");
    assert!(context.contains("last synced 95 min ago"), "{context}");
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

/// Registration is the gate. mem speaks where it has something to say about
/// this project and nowhere else: in ~/.dotfiles, which mem had never heard of,
/// the adapter still injected a digest and then spent a woken turn steering
/// "call `mem log`" at a model that had just been denied the mem skill.
#[test]
fn the_adapter_is_silent_outside_a_registered_project() {
    let w = World::new("hook-unregistered");
    w.project(P, "elsewhere");
    let unregistered = w.repo("stranger", None);
    let plain = w.plain_dir("nowhere");

    for (what, cwd) in [
        ("an unregistered checkout", &unregistered),
        ("a non-git directory", &plain),
    ] {
        for args in [
            vec!["context"],
            vec!["context", "--brief"],
            vec!["context", "--brief", "--hook-json", "--session-id", "s"],
        ] {
            let out = mem(&w, cwd, &args);
            assert_eq!(code(&out), 0, "{what} {args:?}: {}", stderr(&out));
            assert!(
                stdout(&out).is_empty(),
                "{what} {args:?} said: {}",
                stdout(&out)
            );
        }

        // A human who runs it by hand still learns why there is no body.
        assert!(
            !stderr(&mem(&w, cwd, &["context"])).trim().is_empty(),
            "{what}: the note belongs on stderr, where a hook drops it"
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

/// The rungs are ordered so the nearer answer wins: mem's own variable over the
/// harness's, pi's over Claude's for a pi session started from a Claude one.
#[test]
fn the_nearest_session_id_wins() {
    let w = World::new("hook-fallback-order");
    w.project(P, "thing");
    let cwd = w.plain_dir("cwd");
    let batch = |extra: &[&str], env: &[(&str, &str)]| {
        let mut args = vec!["context", "thing", "--brief", "--hook-json"];
        args.extend_from_slice(extra);
        mem_env(&w, &cwd, &args, env)
    };
    let all = [
        ("MEM_SESSION_ID", "mem-id"),
        ("PI_SESSION_ID", "pi-id"),
        ("CLAUDE_CODE_SESSION_ID", "claude-id"),
    ];

    // The flag beats every variable.
    let out = batch(&["--session-id", "flag-id"], &all);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(w.dirs().sessions_dir().join("flag-id").exists());

    // Then MEM_SESSION_ID, then PI_SESSION_ID, then Claude's.
    for (skip, winner) in [(0, "mem-id"), (1, "pi-id"), (2, "claude-id")] {
        let env: Vec<(&str, &str)> = all[skip..].to_vec();
        let out = batch(&[], &env);
        assert_eq!(code(&out), 0, "{}", stderr(&out));
        assert!(
            w.dirs().sessions_dir().join(winner).exists(),
            "{winner} should have won {env:?}"
        );
    }

    // An empty rung is skipped, not fatal.
    let out = batch(
        &[],
        &[
            ("PI_SESSION_ID", "  "),
            ("CLAUDE_CODE_SESSION_ID", "last-id"),
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(w.dirs().sessions_dir().join("last-id").exists());
}

#[test]
fn an_empty_session_id_is_no_session_id() {
    // The wiring passes `--session-id "$CLAUDE_CODE_SESSION_ID"`. If the
    // runtime ever leaves that unset, mem must not keep a session file named "".
    // The batch counter simply emits, having nothing to count with.
    let w = World::new("hook-empty-session");
    w.project(P, "thing");
    let cwd = w.plain_dir("cwd");
    let out = mem(
        &w,
        &cwd,
        &[
            "context",
            "thing",
            "--brief",
            "--hook-json",
            "--session-id",
            "",
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let sessions = w.dirs().sessions_dir();
    let kept: Vec<_> = std::fs::read_dir(&sessions)
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    assert!(kept.is_empty(), "{kept:?}");
}

#[test]
fn the_hook_verbs_never_fail_on_a_fresh_machine() {
    let w = World::new("hook-fresh");
    let dir = w.plain_dir("nowhere");
    for args in [
        vec!["context", "--brief", "--hook-json", "--session-id", "s"],
        vec!["precompact", "--hook-json"],
    ] {
        let out = mem(&w, &dir, &args);
        assert_eq!(code(&out), 0, "{args:?}: {}", stderr(&out));
    }
}
