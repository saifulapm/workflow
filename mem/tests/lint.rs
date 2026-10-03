//! `mem wiki lint` and `mem wiki index --rebuild`: the deterministic half of
//! keeping a wiki, on top of what doctor already checks.

mod common;

use common::{World, code, mem, stderr, stdout};

const P: &str = "01K2AAAAAAAAAAAAAAAAAAAAAA";

/// A wiki page written straight into the store, the way sync would deliver one.
fn page(w: &World, slug: &str, text: &str) {
    let dir = w.store().wiki_dir(P);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{slug}.md")), text).unwrap();
}

fn details(out: &std::process::Output, check: &str) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json output");
    v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["check"] == check)
        .map(|f| f["detail"].as_str().unwrap().to_string())
        .collect()
}

fn wiki(w: &World, args: &[&str]) -> std::process::Output {
    let mut all = vec!["wiki"];
    all.extend_from_slice(args);
    all.extend_from_slice(&["--project", "thing"]);
    mem(w, &w.plain_dir("cwd"), &all)
}

#[test]
fn lint_and_doctor_see_an_oversize_section_an_orphan_and_a_conflict() {
    let w = World::new("lint-fixture");
    w.project(P, "thing");
    page(
        &w,
        "index",
        "# Index\n\n- [pricing](pricing.md)\n- [history](history.md)\n",
    );
    page(
        &w,
        "pricing",
        "# Pricing\n\nOlder notes are in [history](history.md).\n",
    );
    page(
        &w,
        "history",
        &format!(
            "# History\n\n## Short\n\nA line.\n\n## Long\n\n{}",
            "How the cart came to total in cents.\n".repeat(60)
        ),
    );
    page(&w, "lonely", "# Lonely\n\nNothing links here.\n");
    std::fs::write(w.store().project_dir(P).join("plan.md.conflict1"), b"x").unwrap();

    let out = wiki(&w, &["lint", "--json"]);
    assert_eq!(code(&out), 1, "findings are exit 1: {}", stderr(&out));

    let sections = details(&out, "section size");
    assert_eq!(sections.len(), 1, "{sections:?}");
    assert!(sections[0].contains("history#long"), "{sections:?}");

    let orphans = details(&out, "orphan");
    assert_eq!(
        orphans.len(),
        1,
        "the index has no inbound link and is exempt: {orphans:?}"
    );
    assert!(orphans[0].contains("lonely"), "{orphans:?}");

    let drift = details(&out, "wiki index");
    assert!(
        drift.iter().any(|d| d.contains("lonely")),
        "lint carries doctor's wiki findings: {drift:?}"
    );

    let out = mem(&w, &w.plain_dir("cwd"), &["doctor", "--json"]);
    let conflicts = details(&out, "conflict");
    assert!(
        conflicts.iter().any(|d| d.ends_with("plan.md.conflict1")),
        "{conflicts:?}"
    );
    assert!(
        details(&out, "orphan").is_empty() && details(&out, "section size").is_empty(),
        "doctor's own output does not change"
    );
}

#[test]
fn lint_flags_a_page_over_eight_thousand_bytes() {
    let w = World::new("lint-page-size");
    w.project(P, "thing");
    page(&w, "index", "# Index\n\n- [big](big.md)\n");
    // Five sections under 2,000 bytes each: only the page is too big.
    let section = format!("## Part\n\n{}\n", "x".repeat(1_600));
    page(&w, "big", &format!("# Big\n\n{}", section.repeat(5)));

    let out = wiki(&w, &["lint", "--json"]);
    assert_eq!(code(&out), 1);
    let size = details(&out, "page size");
    assert_eq!(size.len(), 1, "{size:?}");
    assert!(size[0].contains("big"), "{size:?}");
    assert!(details(&out, "section size").is_empty());
}

#[test]
fn lint_on_a_tidy_wiki_is_exit_zero() {
    let w = World::new("lint-tidy");
    w.project(P, "thing");
    page(
        &w,
        "index",
        "# Index\n\n- [pricing](pricing.md) — where money is rounded\n",
    );
    page(&w, "pricing", "# Pricing\n\nThe cart totals in cents.\n");

    let out = wiki(&w, &["lint"]);
    assert_eq!(code(&out), 0, "{}", stdout(&out));
    assert_eq!(stdout(&out).trim(), "no findings");
}

#[test]
fn lint_is_a_reserved_slug() {
    let w = World::new("lint-reserved");
    w.project(P, "thing");
    let out = wiki(&w, &["lint", "--stdin", "--note", "a page called lint"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(stderr(&out).contains("reserved"), "{}", stderr(&out));
    assert!(!w.store().wiki_page(P, "lint").exists());
}

#[test]
fn a_rebuilt_index_keeps_its_prose_and_lists_every_page() {
    let w = World::new("lint-rebuild");
    w.project(P, "thing");
    page(
        &w,
        "index",
        "# Index\n\nRead a page before touching its code.\n\n- [stale](stale.md) — gone\n\
         - [pricing](pricing.md)\n\nTrailing prose goes with the old catalog.\n",
    );
    page(&w, "pricing", "# Pricing\n\nThe cart totals in cents.\n");
    page(&w, "deploy", "# Deploy\n\n## Steps\n\nPush, then wait.\n");
    page(
        &w,
        "history",
        &format!("# History\n\n{}\n", "Long first line. ".repeat(20)),
    );

    let out = wiki(&w, &["index", "--rebuild"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let text = std::fs::read_to_string(w.store().wiki_page(P, "index")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[..4],
        ["# Index", "", "Read a page before touching its code.", ""],
        "{text}"
    );
    assert_eq!(lines[4], "- [deploy](deploy.md) — Deploy: Push, then wait.");
    assert!(lines[5].starts_with("- [history](history.md) — History: Long first line."));
    assert!(lines[5].len() <= 120, "{}", lines[5]);
    assert_eq!(
        lines[6],
        "- [pricing](pricing.md) — Pricing: The cart totals in cents."
    );
    assert_eq!(lines.len(), 7, "{text}");

    let log = mem(
        &w,
        &w.plain_dir("cwd"),
        &["log", "--type", "wiki", "--project", "thing"],
    );
    assert!(stdout(&log).contains("wiki index"), "{}", stdout(&log));

    let out = wiki(&w, &["lint"]);
    assert_eq!(
        code(&out),
        0,
        "a rebuilt index leaves no drift: {}",
        stdout(&out)
    );
}

#[test]
fn rebuild_names_the_index_and_nothing_else() {
    let w = World::new("lint-rebuild-slug");
    w.project(P, "thing");
    page(&w, "pricing", "# Pricing\n\nThe cart totals in cents.\n");
    assert_eq!(code(&wiki(&w, &["pricing", "--rebuild"])), 2);
    assert_eq!(code(&wiki(&w, &["--rebuild"])), 2);
}
