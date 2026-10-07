//! Papercuts of the CLI surface: a kind cannot be listed without inventing a
//! query, `mem list` is nobody's verb and says nothing useful, a
//! `#`-prefixed id has always been taken, and the verbs and flags the
//! retired run engine used are gone.

mod common;

use common::{World, code, mem, stderr, stdout};

fn repo(w: &World) -> std::path::PathBuf {
    w.repo("app", Some("git@github.com:me/app.git"))
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

/// What only the retired run engine called is refused the way clap refuses
/// any unknown argument, and no help text names it.
#[test]
fn the_engine_surface_is_gone() {
    let w = World::new("papercuts-retired");
    let dir = repo(&w);
    for args in [
        &["plan", "--task", "t1"][..],
        &["plan", "--add-task"][..],
        &["plan", "--from", "m1"][..],
        &["plan", "--status"][..],
        &["plan", "--status", "approved"][..],
        &["roadmap", "--untick", "m1"][..],
        &["plan", "--force"][..],
        &["roadmap", "--force"][..],
        &["wiki", "notes", "--force"][..],
        &["project", "set", "runner", "mini"][..],
        &["project", "unset", "runner"][..],
        &["project", "set", "slots", "2"][..],
        &["project", "unset", "paused"][..],
        &["status"][..],
        &["status", "--set", "x"][..],
        &["questions", "--asked-by", "s1"][..],
        &["skill"][..],
        &["skill", "mem"][..],
    ] {
        let out = mem(&w, &dir, args);
        assert_eq!(code(&out), 2, "{args:?}: {}", stderr(&out));
    }
    for (verb, gone) in [
        (
            &["plan"][..],
            &["--task", "--add-task", "--from", "--status", "--force"][..],
        ),
        (
            &["roadmap"][..],
            &["--untick", "running", "maintenance", "--force"][..],
        ),
        (&["wiki"][..], &["--force", "runner"][..]),
        (
            &["project", "set"][..],
            &[
                "runner",
                "paused",
                "slots",
                "effort",
                "surface",
                "preview",
                "dogfood",
                "review",
                "WORKFLOW_",
            ][..],
        ),
        (
            &["project", "unset"][..],
            &[
                "runner", "paused", "slots", "effort", "surface", "preview", "dogfood", "review",
            ][..],
        ),
        (&["project", "current"][..], &["machine"][..]),
        (&[][..], &["status.md", "  status ", "  skill "][..]),
        (&["questions"][..], &["--asked-by"][..]),
        (&["ask"][..], &["WORKFLOW_TASK", "worktree"][..]),
    ] {
        let help = stdout(&mem(&w, &dir, &[verb, &["--help"]].concat()));
        for word in gone {
            assert!(!help.contains(word), "{verb:?} --help names {word}: {help}");
        }
    }
}

/// The brief is a hook's text, and it has no JSON form: asking for one is a
/// usage error rather than text under a flag that promised a document.
#[test]
fn the_brief_has_no_json_form() {
    let w = World::new("papercuts-brief-json");
    let dir = repo(&w);
    assert_eq!(code(&mem(&w, &dir, &["log", "first write"])), 0);
    let out = mem(&w, &dir, &["context", "--brief", "--json"]);
    assert_eq!(code(&out), 2, "{}", stdout(&out));
    assert!(out.stdout.is_empty(), "{}", stdout(&out));
    assert!(stderr(&out).contains("--brief"), "{}", stderr(&out));
    let out = mem(&w, &dir, &["context", "--brief"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}
