//! The wiki verbs: the page list, the byte-for-byte print, and the write whose
//! mandatory note becomes the log line that is the page's whole history.

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};

use common::{World, code, mem, stderr, stdout};
use mem::store::is_valid_slug;

fn json(out: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).expect("json output")
}

/// A page write takes its text on stdin, so these runs need a real pipe.
fn spawn(w: &World, cwd: &Path, args: &[&str]) -> std::process::Child {
    let dirs = w.dirs();
    std::process::Command::new(env!("CARGO_BIN_EXE_mem"))
        .current_dir(cwd)
        .args(args)
        .env("XDG_DATA_HOME", &dirs.data)
        .env("XDG_CACHE_HOME", &dirs.cache)
        .env("XDG_STATE_HOME", &dirs.state)
        .env("XDG_CONFIG_HOME", &dirs.config)
        .env_remove("MEM_SESSION_ID")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn mem")
}

fn mem_stdin(w: &World, cwd: &Path, args: &[&str], input: &[u8]) -> std::process::Output {
    let mut child = spawn(w, cwd, args);
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

fn project_id(w: &World) -> String {
    mem::project::Registry::load(&w.store()).projects[0]
        .id
        .clone()
}

fn page_path(w: &World, slug: &str) -> PathBuf {
    w.store().wiki_page(&project_id(w), slug)
}

fn write_page(w: &World, repo: &Path, slug: &str, text: &str, note: &str) {
    let out = mem_stdin(
        w,
        repo,
        &["wiki", slug, "--stdin", "--note", note],
        text.as_bytes(),
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
}

#[test]
fn a_page_write_lands_the_file_and_the_note_becomes_a_log_line() {
    let w = World::new("wiki-write");
    let repo = w.repo("thing", None);
    let page = "# Pricing\n\nThe cart totals in cents, everywhere.\n";

    write_page(&w, &repo, "pricing", page, "first pass at the pricing page");

    let path = page_path(&w, "pricing");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        page,
        "a page is stored byte for byte"
    );
    assert_eq!(
        path,
        w.store()
            .project_dir(&project_id(&w))
            .join("wiki")
            .join("pricing.md"),
        "pages live beside items, plan.md and status.md"
    );

    // The note is the history: one log item per write, and no page revisions.
    let out = mem(&w, &repo, &["log"]);
    assert_eq!(code(&out), 0);
    let log = stdout(&out);
    assert!(
        log.contains("wiki pricing: first pass at the pricing page"),
        "{log}"
    );
    assert_eq!(log.lines().count(), 1);
}

#[test]
fn pages_print_verbatim_and_list_with_their_heading() {
    let w = World::new("wiki-read");
    let repo = w.repo("thing", None);
    let pricing = "# Pricing\n\nThe cart totals in cents.\n";
    write_page(&w, &repo, "pricing", pricing, "started it");
    write_page(
        &w,
        &repo,
        "index",
        "# Index\n\n- [pricing](pricing.md) — where money is rounded\n",
        "linked pricing",
    );

    let out = mem(&w, &repo, &["wiki", "pricing"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), pricing, "a page prints byte for byte");

    let out = mem(&w, &repo, &["wiki"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let list = stdout(&out);
    let lines: Vec<&str> = list.lines().collect();
    assert_eq!(lines.len(), 2, "{list}");
    assert!(lines[0].starts_with("index"), "listed by slug: {list}");
    assert!(lines[1].starts_with("pricing"), "{list}");
    assert!(lines[1].contains("Pricing"), "the heading is shown: {list}");
    assert!(
        lines[1].contains(&pricing.len().to_string()),
        "the size is shown: {list}"
    );

    // The JSON is what hub reads.
    let v = json(&mem(&w, &repo, &["wiki", "--json"]));
    let pages = v["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[1]["slug"], serde_json::json!("pricing"));
    assert_eq!(pages[1]["title"], serde_json::json!("Pricing"));
    assert_eq!(pages[1]["bytes"], serde_json::json!(pricing.len()));
    assert!(pages[1]["modified"].as_str().unwrap().len() == 10);
    assert_eq!(
        pages[1]["path"],
        serde_json::json!(page_path(&w, "pricing").to_string_lossy())
    );

    let v = json(&mem(&w, &repo, &["wiki", "pricing", "--json"]));
    assert_eq!(v["slug"], serde_json::json!("pricing"));
    assert_eq!(v["text"], serde_json::json!(pricing));
}

#[test]
fn an_empty_wiki_and_an_unknown_page_are_exit_one() {
    let w = World::new("wiki-empty");
    let repo = w.repo("thing", None);
    assert_eq!(code(&mem(&w, &repo, &["log", "register the project"])), 0);

    let out = mem(&w, &repo, &["wiki"]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("no pages yet"), "{}", stderr(&out));
    assert_eq!(
        json(&mem(&w, &repo, &["wiki", "--json"]))["pages"]
            .as_array()
            .unwrap()
            .len(),
        0,
        "the empty state is still a list for hub"
    );

    let out = mem(&w, &repo, &["wiki", "pricing"]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("pricing"), "{}", stderr(&out));

    // Outside a checkout there is no wiki at all: pages belong to projects.
    let loose = w.plain_dir("loose");
    assert_eq!(code(&mem(&w, &loose, &["wiki"])), 1);
    assert_eq!(code(&mem(&w, &loose, &["wiki", "pricing"])), 1);
}

#[test]
fn a_write_needs_a_note_and_something_to_say() {
    let w = World::new("wiki-note");
    let repo = w.repo("thing", None);
    let page = "# Pricing\n\nThe cart totals in cents.\n";

    let out = mem_stdin(&w, &repo, &["wiki", "pricing", "--stdin"], page.as_bytes());
    assert_eq!(code(&out), 2, "{}", stdout(&out));
    assert!(stderr(&out).contains("--note"), "{}", stderr(&out));
    assert!(
        !w.store().projects_dir().exists(),
        "a refused write registers nothing"
    );

    // A blank note is no note.
    let out = mem_stdin(
        &w,
        &repo,
        &["wiki", "pricing", "--stdin", "--note", "   "],
        page.as_bytes(),
    );
    assert_eq!(code(&out), 2, "{}", stdout(&out));

    // A note with no write to describe is a usage error too.
    let out = mem(&w, &repo, &["wiki", "pricing", "--note", "changed things"]);
    assert_eq!(code(&out), 2, "{}", stdout(&out));

    // An empty page is a deletion in disguise, and there is no delete verb.
    let out = mem_stdin(
        &w,
        &repo,
        &["wiki", "pricing", "--stdin", "--note", "emptying it"],
        b"\n  \n",
    );
    assert_eq!(code(&out), 2, "{}", stdout(&out));
    assert!(stderr(&out).contains("stub"), "{}", stderr(&out));

    // Nothing landed, and nothing was logged.
    assert_eq!(code(&mem(&w, &repo, &["wiki"])), 1);
    assert_eq!(code(&mem(&w, &repo, &["log"])), 1);
}

#[test]
fn a_bad_slug_is_refused_before_anything_is_written() {
    let w = World::new("wiki-slug");
    let repo = w.repo("thing", None);
    write_page(&w, &repo, "pricing", "# Pricing\n", "started it");

    for slug in [
        "../escape",
        "wiki/../../escape",
        "Pricing",
        "-leading",
        "under_score",
        "with space",
        "dot.md",
        "",
        &"a".repeat(65),
    ] {
        let out = mem_stdin(
            &w,
            &repo,
            &["wiki", slug, "--stdin", "--note", "sneaking in"],
            b"# nope\n",
        );
        assert_eq!(code(&out), 2, "writing '{slug}': {}", stdout(&out));
        let out = mem(&w, &repo, &["wiki", slug]);
        assert_eq!(code(&out), 2, "reading '{slug}': {}", stdout(&out));
    }
    assert_eq!(
        std::fs::read_to_string(page_path(&w, "pricing")).unwrap(),
        "# Pricing\n"
    );
    assert_eq!(
        stdout(&mem(&w, &repo, &["wiki"])).lines().count(),
        1,
        "the one good page is still the only one"
    );

    // The rule itself: [a-z0-9][a-z0-9-]{0,63}.
    for good in ["index", "a", "0", "cart-pricing-v2", &"a".repeat(64)] {
        assert!(is_valid_slug(good), "{good}");
    }
    for bad in [
        "",
        "-x",
        "Index",
        "a_b",
        "a.b",
        "a/b",
        "..",
        "café",
        &"a".repeat(65),
    ] {
        assert!(!is_valid_slug(bad), "{bad}");
    }
}

#[test]
fn a_second_write_replaces_the_page_and_logs_again() {
    let w = World::new("wiki-replace");
    let repo = w.repo("thing", None);
    write_page(
        &w,
        &repo,
        "pricing",
        "# Pricing\n\nIn cents.\n",
        "first pass",
    );
    write_page(
        &w,
        &repo,
        "pricing",
        "# Pricing\n\nIn cents, rounded half up.\n",
        "rounding rule was wrong",
    );

    assert_eq!(
        std::fs::read_to_string(page_path(&w, "pricing")).unwrap(),
        "# Pricing\n\nIn cents, rounded half up.\n"
    );
    assert_eq!(
        stdout(&mem(&w, &repo, &["wiki"])).lines().count(),
        1,
        "a replacement, not a second page"
    );
    let log = stdout(&mem(&w, &repo, &["log"]));
    assert_eq!(log.lines().count(), 2, "{log}");
    assert!(
        log.contains("wiki pricing: rounding rule was wrong"),
        "{log}"
    );
    assert!(log.contains("wiki pricing: first pass"), "{log}");
}

#[test]
fn a_page_that_changed_underneath_the_writer_is_a_conflict() {
    let w = World::new("wiki-cas");
    let repo = w.repo("thing", None);
    write_page(
        &w,
        &repo,
        "pricing",
        "# Pricing\n\nIn cents.\n",
        "first pass",
    );
    let path = page_path(&w, "pricing");

    // The CAS baseline is taken before stdin is drained, and stdin holds that
    // window open for as long as the writer takes. A pipe holds 16 pages --
    // 256 KB on this machine's 16 KB pages, 1 MB where pages are 64 KB -- so
    // eight of them are past every buffer: write_all can only return once the
    // child is already inside its read, which is after it took the baseline.
    let big = format!(
        "# Pricing\n\n{}",
        "the cart totals in cents.\n".repeat(320_000)
    );
    let mut child = spawn(
        &w,
        &repo,
        &["wiki", "pricing", "--stdin", "--note", "second pass"],
    );
    let mut stdin = child.stdin.take().expect("stdin");
    stdin.write_all(big.as_bytes()).expect("write stdin");
    std::fs::write(&path, b"someone else got here first\n").unwrap();
    drop(stdin);
    let out = child.wait_with_output().expect("wait");

    assert_eq!(out.status.code(), Some(5), "{}", stdout(&out));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "someone else got here first\n",
        "a changed page must not be clobbered"
    );
    let log = stdout(&mem(&w, &repo, &["log"]));
    assert_eq!(
        log.lines().count(),
        1,
        "a refused write logs nothing: {log}"
    );
}

#[test]
fn a_page_write_outside_a_project_is_refused() {
    let w = World::new("wiki-loose");
    let loose = w.plain_dir("loose");
    let out = mem_stdin(
        &w,
        &loose,
        &["wiki", "pricing", "--stdin", "--note", "from nowhere"],
        b"# Pricing\n",
    );
    assert_eq!(code(&out), 2, "{}", stdout(&out));
    assert!(stderr(&out).contains("--project"), "{}", stderr(&out));
    assert!(!w.store().projects_dir().exists());
}
