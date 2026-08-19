//! Digest assembly and budgets (spec §8, AC10, AC13).

mod common;

use common::{World, code, item, mem, put, stdout};
use mem::digest::{CEILING, EMPTY, HINT, Sources, TRUNCATED, build, plan_head};
use mem::index::{Index, Purpose};
use mem::item::Kind;

const P: &str = "01K2AAAAAAAAAAAAAAAAAAAAAA";

fn sources(w: &World, staleness: Option<String>) -> (Index, Sources) {
    let index = Index::open(&w.index_path(), Purpose::Read).unwrap();
    index.reindex(&w.store(), false).unwrap();
    let s = Sources::gather(&index, &w.store(), Some(P), staleness).unwrap();
    (index, s)
}

#[test]
fn an_empty_project_gets_one_line_and_the_hint() {
    let w = World::new("digest-empty");
    w.project(P, "thing");
    let (_i, s) = sources(&w, None);
    let d = build(&s, &w.store(), 6000);
    assert_eq!(d.text, format!("{EMPTY}\n{HINT}\n"));
    assert!(!d.truncated);

    let out = mem(&w, &w.plain_dir("cwd"), &["context"]);
    assert_eq!(code(&out), 0, "the empty state is still exit 0");
    assert!(stdout(&out).contains(EMPTY));
}

#[test]
fn a_newer_store_puts_one_warning_line_at_the_top_of_the_digest() {
    let w = World::new("digest-version");
    w.project(P, "thing");
    std::fs::write(w.store().version_path(), b"99\n").unwrap();

    // The empty state is still an answer, so the line belongs on it too: a
    // machine whose binary is behind the store should hear about it on the very
    // first read, not once it happens to have items.
    let (_i, s) = sources(&w, None);
    let d = build(&s, &w.store(), 6000);
    assert_eq!(
        d.text,
        format!("! store format 99 is newer than this mem (1) — reads only\n{EMPTY}\n{HINT}\n")
    );

    put(
        &w.store(),
        Some(P),
        &item(Kind::Fact, "something to serve", "body"),
    );
    let (_i, s) = sources(&w, None);
    let d = build(&s, &w.store(), 6000);
    let first = d.text.lines().next().unwrap_or_default();
    assert!(first.starts_with("! store format 99"), "{}", d.text);

    let out = mem(&w, &w.plain_dir("cwd"), &["context", "thing"]);
    assert_eq!(code(&out), 0, "a newer store still serves reads");
    let first = stdout(&out).lines().next().unwrap_or_default().to_string();
    assert!(first.starts_with("! store format 99"), "{}", stdout(&out));
}

#[test]
fn the_mandatory_sections_come_first_and_in_order() {
    let w = World::new("digest-order");
    w.project(P, "thing");
    let store = w.store();
    put(
        &store,
        Some(P),
        &item(Kind::Handoff, "stopped mid migration", "next: run it"),
    );
    put(
        &store,
        Some(P),
        &item(Kind::Question, "deploy on friday?", "body"),
    );
    std::fs::write(
        store.plan_path(P),
        "# Migrate sessions\n\n- [x] write the plan\n- [ ] run the migration\n- [ ] tell the team\n",
    )
    .unwrap();
    std::fs::write(store.status_path(P), "blocked on review\n").unwrap();
    put(
        &store,
        Some(P),
        &item(
            Kind::Fact,
            "sessions use redis",
            "Because the driver deadlocks. More.",
        ),
    );
    put(
        &store,
        Some(P),
        &item(Kind::Log, "did the thing", "log body"),
    );
    put(
        &store,
        Some(P),
        &item(Kind::Ruling, "chose redis", "ruling body"),
    );

    let (_i, s) = sources(&w, Some("! memory last synced 90 min ago".into()));
    let text = build(&s, &store, 6000).text;
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("! memory"), "{text}");
    assert!(lines[1].starts_with("handoff ("), "{text}");
    let plan_at = lines.iter().position(|l| l.starts_with("plan: #")).unwrap();
    let task_at = lines
        .iter()
        .position(|l| l.contains("run the migration"))
        .unwrap();
    let question_at = lines.iter().position(|l| l.starts_with("? #")).unwrap();
    let status_at = lines.iter().position(|l| l.starts_with("status:")).unwrap();
    assert!(
        plan_at < task_at && task_at < question_at && question_at < status_at,
        "{text}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("tell the team")),
        "only the first unchecked task"
    );
    assert!(text.contains("ruling #"), "{text}");
    assert!(
        text.contains("sessions use redis — Because the driver deadlocks."),
        "{text}"
    );
    assert!(text.contains("log #"), "{text}");
    assert_eq!(lines[lines.len() - 1], HINT);
}

#[test]
fn plan_head_takes_the_heading_and_the_first_unchecked_task() {
    assert_eq!(
        plan_head("# Title\n- [x] done\n- [ ] next\n- [ ] later\n"),
        vec!["# Title".to_string(), "- [ ] next".to_string()]
    );
    assert_eq!(plan_head("no heading, no tasks\n"), Vec::<String>::new());
    assert_eq!(
        plan_head("## Sub\n* [ ] star task\n"),
        vec!["## Sub", "* [ ] star task"]
    );
}

#[test]
fn optional_content_stops_at_the_target_but_mandatory_content_never_does() {
    let w = World::new("digest-budget");
    w.project(P, "thing");
    let store = w.store();
    put(
        &store,
        Some(P),
        &item(Kind::Handoff, "the handoff that must survive", "body"),
    );
    for n in 0..200 {
        put(
            &store,
            Some(P),
            &item(
                Kind::Fact,
                &format!("fact number {n} with a reasonably long title"),
                "body",
            ),
        );
    }
    let (_i, s) = sources(&w, None);
    let small = build(&s, &store, 400);
    assert!(small.text.contains("the handoff that must survive"));
    assert!(small.text.len() < 700, "{} bytes", small.text.len());
    let big = build(&s, &store, 6000);
    assert!(big.text.len() > small.text.len());
    assert!(big.text.len() <= 6000 + HINT.len() + 1);
}

#[test]
fn the_ceiling_truncates_at_an_item_boundary_and_says_so() {
    let w = World::new("digest-ceiling");
    w.project(P, "thing");
    let store = w.store();
    for n in 0..400 {
        put(
            &store,
            Some(P),
            &item(
                Kind::Question,
                &format!(
                    "question {n} that is quite long and mandatory {}",
                    "x".repeat(40)
                ),
                "body",
            ),
        );
    }
    let (_i, s) = sources(&w, None);
    // Mandatory content alone is over the ceiling: it must be cut, at a line
    // boundary, and the last line must say the digest was truncated.
    let d = build(&s, &store, 6000);
    assert!(d.truncated);
    assert!(d.text.len() <= CEILING, "{} bytes", d.text.len());
    assert!(d.text.ends_with(&format!("{TRUNCATED}\n")));
    assert!(d.over_warn);
    for line in d.text.lines().filter(|l| l.starts_with("? #")) {
        assert!(
            line.contains("question"),
            "no line may be cut mid-item: {line}"
        );
    }
}

#[test]
fn brief_fits_in_its_own_budget() {
    let w = World::new("digest-brief");
    w.project(P, "thing");
    let store = w.store();
    put(
        &store,
        Some(P),
        &item(
            Kind::Handoff,
            &"a very long handoff title ".repeat(30),
            "body",
        ),
    );
    std::fs::write(store.plan_path(P), "# Plan\n- [ ] the next action\n").unwrap();
    for n in 0..5 {
        put(
            &store,
            Some(P),
            &item(Kind::Question, &format!("q{n}"), "body"),
        );
    }
    let (_i, s) = sources(&w, None);
    let text = mem::digest::brief(&s, jiff::Timestamp::now());
    assert!(text.len() <= mem::digest::BRIEF, "{} bytes", text.len());
    assert!(text.starts_with("handoff:"));
}

#[test]
fn context_on_an_unregistered_checkout_serves_global_and_exits_zero() {
    let w = World::new("digest-unknown");
    let store = w.store();
    put(&store, None, &item(Kind::Fact, "a global fact", "body"));
    let repo = w.repo("thing", None);
    let out = mem(&w, &repo, &["context"]);
    assert_eq!(code(&out), 0);
    assert!(
        common::stderr(&out).contains("not registered yet"),
        "{}",
        common::stderr(&out)
    );
    assert!(
        !w.store().projects_dir().exists(),
        "context must not register"
    );
}
