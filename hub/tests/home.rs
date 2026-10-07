//! `GET /`: every project with its lifecycle stage and progress, then the
//! questions waiting on the owner.

mod common;

use std::path::{Path, PathBuf};

use common::{Hub, TempDir, body_of, fixture_bin, status_of};
use hub::config::Config;
use hub::html::Banner;
use hub::memcli::{MemCli, QUESTIONS_ARGV};
use hub::page_home::{self, lifecycle_stage};
use serde_json::{Value, json};

/// 2026-10-05T12:00:00Z, two hours after every project's last activity.
const NOW_MS: i64 = 1_791_201_600_000;

/// One `projects --json` row, every summary field at its empty value, then
/// `fields` laid over it.
fn row(name: &str, fields: Value) -> Value {
    let mut row = json!({
        "id": format!("id-{name}"), "name": name, "remote": null, "aliases": [],
        "created": "2026-01-01", "items": 3, "current": false,
        "checkouts": [format!("/src/{name}")],
        "roadmap_status": null, "milestone": null,
        "milestones_done": 0, "milestones_total": 0,
        "plan_slug": null, "plan_ticked": 0, "plan_total": 0,
        "last_activity": "2026-10-05T10:00:00Z",
        "has_research": false,
        "has_research_summary": false, "has_spec": false,
    });
    for (key, value) in fields.as_object().unwrap() {
        row[key] = value.clone();
    }
    row
}

/// A project at each of the eight stages, and one with no checkout on this
/// machine whose plan of record is an earlier milestone's.
fn projects() -> Value {
    let running = |plan_ticked: u64| {
        json!({
            "roadmap_status": "approved", "milestone": "m1-auth",
            "milestones_done": 0, "milestones_total": 2,
            "plan_slug": "m1-auth", "plan_ticked": plan_ticked, "plan_total": 3,
            "has_research": true, "has_research_summary": true, "has_spec": true,
        })
    };
    json!({"projects": [
        row("p-brief", json!({})),
        row("p-research", json!({"has_research": true})),
        row("p-grilling", json!({"has_research": true, "has_research_summary": true})),
        row("p-spec", json!({"has_research": true, "has_research_summary": true, "has_spec": true})),
        row("p-planning", json!({
            "roadmap_status": "draft", "milestone": "m1-auth", "milestones_total": 2,
        })),
        // Execution and dogfooding differ in one tick and nothing else.
        row("p-execution", running(2)),
        row("p-dogfood", running(3)),
        row("p-maint", json!({
            "roadmap_status": "maintenance", "milestones_done": 2, "milestones_total": 2,
            "plan_slug": "m2-billing", "plan_ticked": 4, "plan_total": 4,
        })),
        row("p-elsewhere", json!({
            "checkouts": [], "roadmap_status": "approved", "milestone": "m2-billing",
            "milestones_done": 1, "milestones_total": 2,
            "plan_slug": "m1-auth", "plan_ticked": 3, "plan_total": 3,
        })),
    ]})
}

/// Two questions for `p-planning`, one of them several lines long, whose
/// title is only the first.
fn questions() -> Value {
    let question = |id: &str, body: &str, options: Value| {
        json!({
            "id": id, "short_id": &id[18..], "kind": "question", "type": null,
            "title": body.lines().next().unwrap(), "tags": [], "project": "p-planning",
            "machine": "here", "created": "2026-10-05", "modified": "2026-10-05",
            "active": true, "archived": false, "answered": false, "answer": null,
            "body": body, "options": options, "recommend": null,
        })
    };
    json!({"questions": [
        question("01K6SBE4R0AAAAAAAAAAAAAAAA", "Review the p-planning roadmap and its plan pages", json!(["approve", "changes"])),
        question("01K6SBE4R0BBBBBBBBBBBBBBBB", "Two things:\n1. Which store?\n2. Which port?", json!([])),
    ]})
}

/// A fake `mem` that logs each argv, one per line, and answers the reads
/// the page makes. Anything else exits 1 with nothing on stdout. Only shell
/// builtins: a `MemCli` built `with_path` has nothing else on PATH.
fn fake_mem(dir: &Path) -> (PathBuf, PathBuf) {
    let bin = dir.join("bin");
    let log = dir.join("argv.log");
    fixture_bin(
        &bin,
        "mem",
        &format!(
            "printf '%s\\n' \"$*\" >> '{log}'\n\
             case \"$1\" in\n\
             projects) printf '%s\\n' '{projects}' ;;\n\
             questions) printf '%s\\n' '{questions}' ;;\n\
             *) exit 1 ;;\n\
             esac",
            log = log.display(),
            projects = projects(),
            questions = questions(),
        ),
    );
    (bin, log)
}

fn siblings() -> Config {
    Config {
        siblings: vec!["http://nuc:8088".to_string()],
        ..Config::default()
    }
}

fn render(tag: &str) -> (TempDir, String, Vec<String>) {
    let dir = TempDir::new(tag);
    let (bin, log) = fake_mem(dir.path());
    let mem = MemCli::with_path(bin);
    let page = page_home::render(&mem, &siblings(), "here", NOW_MS, &Banner::None);
    let argv = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    (dir, page, argv)
}

#[test]
fn each_stage_follows_from_the_project_summary() {
    let projects = projects();
    let stages: Vec<(&str, &str)> = projects["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["name"].as_str().unwrap(), lifecycle_stage(p)))
        .collect();
    assert_eq!(
        stages,
        [
            ("p-brief", "brief"),
            ("p-research", "research"),
            ("p-grilling", "grilling"),
            ("p-spec", "spec"),
            ("p-planning", "planning"),
            ("p-execution", "execution"),
            ("p-dogfood", "dogfooding"),
            ("p-maint", "done"),
            ("p-elsewhere", "execution"),
        ]
    );
    // A roadmap reads done once every milestone is ticked, whatever status
    // it stores, and a stored done or maintenance reads done too.
    for fields in [
        json!({"roadmap_status": "approved", "milestone": null,
               "milestones_done": 3, "milestones_total": 3,
               "plan_slug": "m3-last", "plan_ticked": 2, "plan_total": 2}),
        json!({"roadmap_status": "draft", "milestones_done": 1, "milestones_total": 1}),
        json!({"roadmap_status": "done"}),
        json!({"roadmap_status": "maintenance"}),
    ] {
        let finished = row("p-done", fields.clone());
        assert_eq!(lifecycle_stage(&finished), "done", "{fields}");
    }
}

#[test]
fn the_page_reads_nothing_per_project_or_per_question() {
    let (_dir, page, argv) = render("home-spawns");
    assert_eq!(
        argv,
        ["projects --json".to_string(), QUESTIONS_ARGV.join(" ")]
    );
    assert!(!page.contains("week:"), "{page}");
}

/// The card of one project, from its opening tag to its closing one.
fn card<'a>(page: &'a str, name: &str) -> &'a str {
    let at = page
        .find(&format!("<h3><a href=\"/p/{name}\">{name}</a></h3>"))
        .unwrap_or_else(|| panic!("no card for {name}\n{page}"));
    let start = page[..at]
        .rfind("<article class=\"card\">")
        .expect("a card");
    let end = at + page[at..].find("</article>").expect("a closed card");
    &page[start..end]
}

#[test]
fn each_project_is_a_card_with_its_stage_and_progress() {
    let (_dir, page, _argv) = render("home-rows");
    for (name, pill, meta) in [
        ("p-brief", "<span class=\"pill mut\">brief</span>", None),
        (
            "p-research",
            "<span class=\"pill mut\">research</span>",
            None,
        ),
        (
            "p-grilling",
            "<span class=\"pill mut\">grilling</span>",
            None,
        ),
        ("p-spec", "<span class=\"pill mut\">spec</span>", None),
        (
            "p-planning",
            "<span class=\"pill wait\">planning</span>",
            Some("m1-auth · milestone 1 of 2"),
        ),
        (
            "p-execution",
            "<span class=\"pill\">execution</span>",
            Some("m1-auth · milestone 1 of 2 · tasks 2 of 3"),
        ),
        (
            "p-dogfood",
            "<span class=\"pill\">dogfooding</span>",
            Some("m1-auth · milestone 1 of 2 · tasks 3 of 3"),
        ),
        (
            "p-maint",
            "<span class=\"pill ok\">done</span>",
            Some("milestone 2 of 2 · tasks 4 of 4"),
        ),
        (
            "p-elsewhere",
            "<span class=\"pill\">execution</span>",
            Some("m2-billing · milestone 2 of 2 · tasks 3 of 3"),
        ),
    ] {
        let card = card(&page, name);
        assert!(card.contains(pill), "{name}: {card}");
        match meta {
            Some(meta) => assert!(
                card.contains(&format!("<div class=\"meta\">{meta}</div>")),
                "{name}: {card}"
            ),
            None => assert!(!card.contains("<div class=\"meta\">"), "{name}: {card}"),
        }
    }
    assert!(
        card(&page, "p-dogfood")
            .contains("<span class=\"sp\"></span><span class=\"meta\">2h</span>"),
        "{page}"
    );
}

#[test]
fn a_card_fills_its_bar_with_the_plans_ticks_else_the_milestones() {
    let (_dir, page, _argv) = render("home-bars");
    for (name, width) in [
        ("p-execution", "66%"),
        ("p-dogfood", "100%"),
        ("p-planning", "0%"),
        ("p-maint", "100%"),
    ] {
        let card = card(&page, name);
        assert!(
            card.contains(&format!(
                "<div class=\"bar\"><i style=\"width:{width}\"></i></div>"
            )),
            "{name}: {card}"
        );
    }
    assert!(!card(&page, "p-brief").contains("class=\"bar\""), "{page}");
}

#[test]
fn what_waits_on_saiful_shows_on_the_card_and_above_the_projects() {
    let (_dir, page, _argv) = render("home-waits");
    let planning = card(&page, "p-planning");
    assert!(
        planning.contains("<span class=\"pill wait\">2 questions</span>"),
        "{planning}"
    );
    assert!(!page.contains("paused"), "{page}");
    assert!(!card(&page, "p-execution").contains("question"), "{page}");
    assert!(
        page.contains("<h2>Projects <span class=\"pill wait\">1 waits on you</span></h2>"),
        "{page}"
    );
    let waiting = page.find("<h2>Waiting on you").expect("a waiting heading");
    let projects = page.find("<h2>Projects").expect("a projects heading");
    assert!(waiting < projects, "the questions come first");
    assert!(page.contains("<div class=\"cards\">"), "{page}");
}

#[test]
fn a_waiting_question_shows_its_whole_body_and_the_answer_form() {
    let (_dir, page, _argv) = render("home-questions");
    assert!(
        page.contains("Two things:\n1. Which store?\n2. Which port?"),
        "{page}"
    );
    assert!(
        page.contains("<p class=\"rec\">approve · changes</p>"),
        "{page}"
    );
    assert_eq!(
        page.matches("<form method=\"post\" action=\"/answer\">")
            .count(),
        2
    );
    assert!(
        page.contains("<input type=\"hidden\" name=\"id\" value=\"01K6SBE4R0BBBBBBBBBBBBBBBB\">"),
        "{page}"
    );
    // The guarded reload is the page's one script.
    assert_eq!(page.matches("<script").count(), 1, "{page}");
    assert!(page.contains("location.reload()"), "{page}");
}

#[test]
fn the_front_page_is_served_at_the_root() {
    let dir = TempDir::new("home-route");
    let home = dir.join("home");
    let (bin, _log) = fake_mem(dir.path());
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        "topic = \"workflow-TESTTESTTESTTESTTESTTESTTE\"\n\
         ntfy_base = \"http://127.0.0.1:9\"\n\
         siblings = [\"http://nuc:8088\"]\n",
    )
    .unwrap();
    let hub = Hub::spawn(
        &home,
        &[&bin],
        &["--config", config.to_str().unwrap(), "--port", "0"],
    );

    let response = hub.get("/");
    assert_eq!(status_of(&response), 200);
    let body = body_of(&response);
    assert!(
        body.contains("<a href=\"/p/p-maint\">p-maint</a>"),
        "{body}"
    );
    assert!(body.contains("dogfooding"), "{body}");
}
