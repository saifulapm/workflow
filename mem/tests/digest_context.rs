//! Digest assembly and budgets (spec §8, AC10, AC13).

mod common;

use common::{World, code, item, mem, put, stdout};
use mem::digest::{
    CEILING, EMPTY, HINT, Sources, TRUNCATED, build, first_open_task, index_head, plan_head,
};
use mem::index::{Index, Purpose};
use mem::item::Kind;

const P: &str = "01K2AAAAAAAAAAAAAAAAAAAAAA";

fn sources(w: &World, staleness: Option<String>) -> (Index, Sources) {
    let index = Index::open(&w.index_path(), Purpose::Read).unwrap();
    index.reindex(&w.store(), false).unwrap();
    let s = Sources::gather(&index, &w.store(), Some(P), staleness, String::new()).unwrap();
    (index, s)
}

/// Writes a page straight into the store, the way the wiki verb would.
fn page(w: &World, slug: &str, text: &str) {
    let store = w.store();
    std::fs::create_dir_all(store.wiki_dir(P)).unwrap();
    std::fs::write(store.wiki_page(P, slug), text).unwrap();
}

#[test]
fn an_empty_project_gets_one_line_and_the_hint() {
    let w = World::new("digest-empty");
    w.project(P, "thing");
    let (_i, s) = sources(&w, None);
    let d = build(&s, &w.store(), 6000);
    assert_eq!(d.text, format!("{EMPTY}\n{HINT}\n"));
    assert!(!d.truncated);

    // The builder still has the empty state to render; the CLI no longer has
    // anywhere to render it. Outside a project mem knows, the adapter is quiet.
    let out = mem(&w, &w.plain_dir("cwd"), &["context"]);
    assert_eq!(code(&out), 0, "the empty state is still exit 0");
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
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

/// A plan that spells out its own grammar carries open boxes that illustrate a
/// task rather than being one: fenced, or indented under the task they explain.
const EXAMPLES: &str = "# Migrate sessions

- [x] t1 write the plan
      Spec: a task line reads

          - [ ] t9 an example task

```
- [ ] t8 another example
```

- [ ] t2 run the migration
";

#[test]
fn an_open_box_in_an_example_is_not_the_next_task() {
    assert_eq!(
        first_open_task(EXAMPLES),
        Some("- [ ] t2 run the migration")
    );
    assert_eq!(
        plan_head(EXAMPLES),
        vec!["# Migrate sessions", "- [ ] t2 run the migration"]
    );

    // With the one real task ticked the plan is finished, examples and all.
    let done = EXAMPLES.replace("- [ ] t2", "- [x] t2");
    assert_eq!(first_open_task(&done), None);
    assert_eq!(plan_head(&done), vec!["# Migrate sessions"]);
}

#[test]
fn context_names_the_first_real_task_and_none_of_the_examples() {
    let w = World::new("digest-plan-examples");
    w.project(P, "thing");
    std::fs::write(w.store().plan_path(P), EXAMPLES).unwrap();

    let out = mem(&w, &w.plain_dir("cwd"), &["context", "thing"]);
    assert_eq!(code(&out), 0, "{}", common::stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("plan: - [ ] t2 run the migration"), "{text}");
    assert!(!text.contains("t9"), "an indented example: {text}");
    assert!(!text.contains("t8"), "a fenced example: {text}");

    // Nothing open left means the digest names the plan and no task under it.
    let done = EXAMPLES.replace("- [ ] t2", "- [x] t2");
    std::fs::write(w.store().plan_path(P), &done).unwrap();
    let text = stdout(&mem(&w, &w.plain_dir("cwd"), &["context", "thing"]));
    assert!(text.contains("plan: # Migrate sessions"), "{text}");
    assert!(!text.contains("- [ ]"), "{text}");
}

#[test]
fn a_project_with_pages_opens_with_the_wiki_line_and_the_index_head() {
    let w = World::new("digest-wiki");
    w.project(P, "thing");
    let store = w.store();
    put(
        &store,
        Some(P),
        &item(Kind::Question, "deploy on friday?", "body"),
    );
    std::fs::write(store.status_path(P), "blocked on review\n").unwrap();
    page(&w, "pricing", "# Pricing\n\nThe cart totals in cents.\n");
    page(
        &w,
        "sessions",
        "# Sessions\n\nRedis, one key per session.\n",
    );
    page(
        &w,
        "index",
        "# Index\n\n- [pricing](pricing.md) — where money is rounded\n\
         - [sessions](sessions.md) — where a login lives\n",
    );

    let (_i, s) = sources(&w, None);
    let text = build(&s, &store, 6000).text;
    let lines: Vec<&str> = text.lines().collect();
    let wiki_at = lines
        .iter()
        .position(|l| *l == "wiki: 3 pages — mem wiki")
        .unwrap_or_else(|| panic!("{text}"));
    let head_at = lines
        .iter()
        .position(|l| l.contains("[pricing](pricing.md)"))
        .unwrap_or_else(|| panic!("{text}"));
    let question_at = lines.iter().position(|l| l.starts_with("? #")).unwrap();
    let status_at = lines.iter().position(|l| l.starts_with("status:")).unwrap();
    assert!(
        question_at < wiki_at && wiki_at < head_at && head_at < status_at,
        "the wiki line closes the mandatory sections and the index head follows it: {text}"
    );
    assert!(lines[head_at].starts_with("  "), "{text}");
    assert!(
        text.contains("[sessions](sessions.md)"),
        "the whole index head, not just its first entry: {text}"
    );

    let out = mem(&w, &w.plain_dir("cwd"), &["context", "thing"]);
    assert_eq!(code(&out), 0);
    assert!(
        stdout(&out).contains("wiki: 3 pages — mem wiki"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn a_project_whose_only_memory_is_a_page_is_not_empty() {
    let w = World::new("digest-wiki-only");
    w.project(P, "thing");
    page(&w, "pricing", "# Pricing\n\nThe cart totals in cents.\n");

    let (_i, s) = sources(&w, None);
    let text = build(&s, &w.store(), 6000).text;
    assert!(!text.contains(EMPTY), "{text}");
    assert_eq!(text, format!("wiki: 1 page — mem wiki\n{HINT}\n"));
}

#[test]
fn a_tight_budget_keeps_the_wiki_line_and_drops_the_index_head() {
    let w = World::new("digest-wiki-budget");
    w.project(P, "thing");
    page(&w, "pricing", "# Pricing\n\nThe cart totals in cents.\n");
    page(
        &w,
        "index",
        "# Index\n\n- [pricing](pricing.md) — where money is rounded\n",
    );

    let (_i, s) = sources(&w, None);
    let text = build(&s, &w.store(), 8).text;
    assert!(text.contains("wiki: 2 pages — mem wiki"), "{text}");
    assert!(!text.contains("[pricing](pricing.md)"), "{text}");
}

#[test]
fn index_head_takes_the_first_lines_and_leaves_the_rest_to_mem_wiki() {
    let mut page = String::from("# Index\n\n");
    for n in 0..20 {
        page.push_str(&format!("- [p{n}](p{n}.md) — page {n}\n"));
    }
    let head = index_head(&page);
    assert_eq!(head.len(), mem::digest::INDEX_HEAD_LINES);
    assert_eq!(head[0], "# Index");
    assert_eq!(head[1], "- [p0](p0.md) — page 0");
    assert_eq!(index_head(""), Vec::<String>::new());

    let long = format!("# {}\n", "x".repeat(400));
    assert!(
        index_head(&long)[0].len() <= 100,
        "a line is not a document"
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
fn the_digests_recent_logs_skip_the_runs_own_bookkeeping() {
    let w = World::new("digest-run-logs");
    w.project(P, "thing");
    let store = w.store();
    for n in 0..5 {
        let mut log = item(Kind::Log, &format!("worker did thing {n}"), "body");
        log.meta.modified = jiff::Timestamp::from_second(1_800_000_000 + n).unwrap();
        put(&store, Some(P), &log);
    }
    let mut run_log = item(Kind::Log, "run thing: dispatched t1", "body");
    run_log.meta.r#type = Some("run".to_string());
    // Newer than every worker log, so a naive "most recent five" would pick
    // it over one of them.
    run_log.meta.modified = jiff::Timestamp::from_second(1_800_000_100).unwrap();
    put(&store, Some(P), &run_log);

    let (_i, s) = sources(&w, None);
    assert_eq!(
        s.logs.len(),
        5,
        "the run log does not take one of the five slots"
    );
    assert!(
        s.logs.iter().all(|l| l.r#type.as_deref() != Some("run")),
        "no run-typed log reaches the digest"
    );
    let text = build(&s, &store, 6000).text;
    assert!(!text.contains("dispatched t1"), "{text}");
    for n in 0..5 {
        assert!(text.contains(&format!("worker did thing {n}")), "{text}");
    }
}

#[test]
fn a_burst_of_run_logs_does_not_empty_the_digests_log_section() {
    let w = World::new("digest-run-log-burst");
    w.project(P, "thing");
    let store = w.store();
    for n in 0..5 {
        let mut log = item(Kind::Log, &format!("worker did thing {n}"), "body");
        log.meta.modified = jiff::Timestamp::from_second(1_800_000_000 + n).unwrap();
        put(&store, Some(P), &log);
    }
    // A restarted run's bookkeeping outnumbers the fixed 20-row page the old
    // over-fetch used, so every worker log above sits past it.
    for n in 0..21 {
        let mut run_log = item(Kind::Log, &format!("run thing: dispatched t{n}"), "body");
        run_log.meta.r#type = Some("run".to_string());
        run_log.meta.modified = jiff::Timestamp::from_second(1_800_000_100 + n).unwrap();
        put(&store, Some(P), &run_log);
    }

    let (_i, s) = sources(&w, None);
    assert_eq!(
        s.logs.len(),
        5,
        "the worker logs still fill the five slots past the run burst"
    );
    assert!(
        s.logs.iter().all(|l| l.r#type.as_deref() != Some("run")),
        "no run-typed log reaches the digest"
    );
    let text = build(&s, &store, 6000).text;
    for n in 0..5 {
        assert!(text.contains(&format!("worker did thing {n}")), "{text}");
    }
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

/// A skill is text in a binary now, not a file a harness discovered, and the
/// digest is where a session learns which ones exist. mem serves its own and
/// appends workflow's, because a skill belongs to the binary it is about.
#[test]
fn mem_serves_its_own_skill_and_refuses_the_rest() {
    let w = World::new("skills-verb");
    let dir = w.plain_dir("anywhere");

    let out = mem(&w, &dir, &["skill"]);
    assert_eq!(code(&out), 0, "{}", common::stderr(&out));
    assert_eq!(stdout(&out).lines().count(), 1, "{}", stdout(&out));
    assert!(stdout(&out).starts_with("mem — "), "{}", stdout(&out));

    let out = mem(&w, &dir, &["skill", "mem"]);
    assert_eq!(code(&out), 0, "{}", common::stderr(&out));
    assert!(stdout(&out).starts_with("---\nname: mem"), "the whole file");
    assert!(stdout(&out).len() > 1000, "not just the description");

    // route is workflow's, and saying so beats reporting a skill that exists.
    let out = mem(&w, &dir, &["skill", "route"]);
    assert_eq!(code(&out), 1);
    assert!(
        common::stderr(&out).contains("workflow skill route"),
        "{}",
        common::stderr(&out)
    );
}

#[test]
fn the_digest_names_every_skill_and_how_to_open_one() {
    let w = World::new("skills-section");
    w.project(P, "thing");
    let repo = w.plain_dir("cwd");

    // A stand-in for the workflow on this machine: mem appends what it prints
    // without parsing a byte of it.
    let fake = w.dir.join("fake-workflow");
    std::fs::write(
        &fake,
        "#!/bin/sh\nprintf 'route — pick the lane\\nplan — cut the tasks\\n'\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    let out = common::mem_env(
        &w,
        &repo,
        &["context", "thing"],
        &[("WORKFLOW_BIN", fake.to_str().unwrap())],
    );
    assert_eq!(code(&out), 0, "{}", common::stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("mem skill <name>"), "{text}");
    assert!(text.contains("workflow skill <name>"), "{text}");
    // Ownership, not just both verbs: the listing carries no owner marker, so
    // the line has to say mem's verb serves mem alone (friction #NBQN1S4V).
    assert!(text.contains("serves mem's own skill only"), "{text}");
    assert!(text.contains("mem — "), "mem names its own: {text}");
    assert!(text.contains("route — pick the lane"), "verbatim: {text}");
    assert!(text.contains("plan — cut the tasks"), "verbatim: {text}");

    // A workflow that exits nonzero costs its seven and nothing else.
    std::fs::write(&fake, "#!/bin/sh\nexit 3\n").unwrap();
    let text = stdout(&common::mem_env(
        &w,
        &repo,
        &["context", "thing"],
        &[("WORKFLOW_BIN", fake.to_str().unwrap())],
    ));
    assert!(text.contains("mem — "), "{text}");
    assert!(!text.contains("route — "), "{text}");

    // And outside a project mem knows, none of it is said at all.
    let out = mem(&w, &w.plain_dir("stranger"), &["context"]);
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
}
