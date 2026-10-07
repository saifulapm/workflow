//! `GET /p/<project>/roadmap`: each milestone with its Show path, its plan
//! and, while the roadmap is a draft, the two forms that answer it.

mod common;

use std::path::PathBuf;

use common::{Hub, TempDir, body_of, fixture_mem, status_of};
use hub::page_roadmap::{MilestoneRow, roadmap_rows};

const ROADMAP: &str = "# roadmap: beta

- [x] b1-box Recipes are stored and listed
      Surface: web
      Show: a recipe added on the phone is listed on the laptop
      Done: the laptop lists the recipe within a minute
- [ ] b2-scale A recipe scales to any number of people  [after: b1-box]
      Surface: web
      Show: a recipe for four, set to six, lists half again of every amount
- [ ] b3-share A recipe is shared by link
";

/// A fake `mem` that knows one project, `beta`, whose roadmap read prints
/// `roadmap` (nothing at all for a project with none), whose stored plans
/// are b1-box and b2-scale and whose run lines are `runs` (mem's empty
/// listing when there are none). Every call is appended to `calls`.
struct World {
    _dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    calls: PathBuf,
}

impl World {
    fn new(tag: &str, roadmap: Option<serde_json::Value>) -> World {
        World::with_runs(tag, roadmap, &[])
    }

    fn with_runs(tag: &str, roadmap: Option<serde_json::Value>, runs: &[&str]) -> World {
        let dir = TempDir::new(tag);
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let bin = dir.join("bin");
        let calls = dir.join("calls");
        let roadmap = match roadmap {
            Some(doc) => format!("printf '%s\\n' '{doc}'"),
            None => "exit 1".to_string(),
        };
        let items: Vec<serde_json::Value> = runs
            .iter()
            .map(|title| serde_json::json!({ "kind": "log", "type": "run", "title": title }))
            .collect();
        let runs = if items.is_empty() {
            "echo '{\"items\":[]}'; exit 1".to_string()
        } else {
            format!("printf '%s\\n' '{}'", serde_json::json!({ "items": items }))
        };
        fixture_mem(
            &bin,
            &format!(
                "echo \"$*\" >>'{calls}'\n\
                 case \"$1\" in\n\
                 projects) echo '{{\"projects\":[{{\"name\":\"beta\"}}]}}' ;;\n\
                 roadmap) {roadmap} ;;\n\
                 plan) echo '{{\"plans\":[{{\"slug\":\"b1-box\"}},{{\"slug\":\"b2-scale\"}}]}}' ;;\n\
                 log) {runs} ;;\n\
                 *) exit 1 ;;\n\
                 esac",
                calls = calls.display(),
            ),
        );
        World {
            _dir: dir,
            home,
            bin,
            calls,
        }
    }

    fn hub(&self) -> Hub {
        Hub::spawn(&self.home, &[&self.bin], &["--port", "0"])
    }

    /// The page's own reads: the doorbell's question poll runs on its own
    /// clock and is left out.
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.calls)
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.starts_with("questions "))
            .map(str::to_string)
            .collect()
    }
}

fn roadmap_doc(status: Option<&str>) -> serde_json::Value {
    let mut doc = serde_json::json!({ "text": ROADMAP, "path": "/x/roadmap.md" });
    if let Some(status) = status {
        doc["status"] = serde_json::json!(status);
    }
    doc
}

/// The page's body, after checking it ran mem `spawns` times on a cold cache.
fn roadmap_page(world: &World, spawns: usize) -> String {
    let hub = world.hub();
    let before = world.calls().len();
    let response = hub.get("/p/beta/roadmap");
    assert_eq!(status_of(&response), 200, "{response}");
    let calls = world.calls();
    assert_eq!(calls.len() - before, spawns, "{:?}", &calls[before..]);
    body_of(&response).to_string()
}

#[test]
fn roadmap_rows_reads_each_milestone_and_its_indented_lines() {
    let rows = roadmap_rows(ROADMAP);
    assert_eq!(
        rows,
        vec![
            MilestoneRow {
                slug: "b1-box".to_string(),
                title: "Recipes are stored and listed".to_string(),
                ticked: true,
                surface: Some("web".to_string()),
                show: Some("a recipe added on the phone is listed on the laptop".to_string()),
                done: Some("the laptop lists the recipe within a minute".to_string()),
            },
            MilestoneRow {
                slug: "b2-scale".to_string(),
                title: "A recipe scales to any number of people".to_string(),
                ticked: false,
                surface: Some("web".to_string()),
                show: Some(
                    "a recipe for four, set to six, lists half again of every amount".to_string()
                ),
                done: None,
            },
            MilestoneRow {
                slug: "b3-share".to_string(),
                title: "A recipe is shared by link".to_string(),
                ticked: false,
                surface: None,
                show: None,
                done: None,
            },
        ]
    );
}

#[test]
fn roadmap_a_draft_shows_each_milestone_its_plan_and_both_forms() {
    let world = World::new("roadmap-draft", Some(roadmap_doc(Some("draft"))));
    let body = roadmap_page(&world, 4);

    assert!(body.contains("draft"), "{body}");
    for text in [
        "Recipes are stored and listed",
        "A recipe scales to any number of people<",
        "a recipe added on the phone is listed on the laptop",
        "a recipe for four, set to six, lists half again of every amount",
        "the laptop lists the recipe within a minute",
        "href=\"/p/beta/plan/b1-box\"",
        "href=\"/p/beta/plan/b2-scale\"",
    ] {
        assert!(body.contains(text), "missing {text:?}: {body}");
    }
    assert!(!body.contains("[after:"), "{body}");
    assert!(
        !body.contains("/p/beta/plan/b3-share"),
        "b3-share has no stored plan: {body}"
    );
    // The first open milestone is the current one; the one after it is open.
    let current = body.find("current").expect("a current milestone");
    assert!(current < body.find("A recipe scales").unwrap(), "{body}");
    assert!(body.contains("open"), "{body}");
    assert!(body.contains('✓'), "the ticked milestone: {body}");

    assert_eq!(
        body.matches("action=\"/p/beta/control\"").count(),
        2,
        "{body}"
    );
    assert!(
        body.contains("name=\"do\" value=\"approve\""),
        "Approve: {body}"
    );
    assert!(
        body.contains("name=\"do\" value=\"changes\""),
        "Request changes: {body}"
    );
    assert!(body.contains("<textarea name=\"text\""), "{body}");
    assert!(body.contains("Request changes"), "{body}");
}

#[test]
fn roadmap_an_approved_one_says_it_waits_for_the_engine() {
    let world = World::new("roadmap-approved", Some(roadmap_doc(Some("approved"))));
    let body = roadmap_page(&world, 4);

    assert!(
        body.contains("approved: sent, waiting for the engine"),
        "{body}"
    );
    assert!(!body.contains("<form"), "{body}");
}

#[test]
fn roadmap_a_running_one_shows_no_form() {
    let world = World::new("roadmap-running", Some(roadmap_doc(Some("running"))));
    let body = roadmap_page(&world, 4);

    assert!(body.contains("running"), "{body}");
    assert!(
        body.contains("A recipe scales to any number of people<"),
        "{body}"
    );
    assert!(!body.contains("<form"), "{body}");
    assert!(!body.contains("waiting for the engine"), "{body}");
}

#[test]
fn roadmap_a_project_with_none_says_so() {
    let world = World::new("roadmap-none", None);
    let body = roadmap_page(&world, 2);

    assert!(body.contains("No roadmap recorded."), "{body}");
    assert!(!body.contains("<form"), "{body}");
}

#[test]
fn roadmap_cost_shows_under_a_ticked_milestone_from_its_milestone_line() {
    let world = World::with_runs(
        "roadmap-cost",
        Some(roadmap_doc(Some("running"))),
        &[
            "cost b1-box worker b1-t1: minutes=11 context=52000 in=640000 out=9100 model=sonnet",
            "cost b1-box milestone b1-box: sessions=4 minutes=31 context=120000 in=2400000 out=38700",
            "cost b2-scale milestone b2-scale: sessions=1 minutes=2 in=900 out=40",
            "dogfood b1-box: pass",
        ],
    );
    let body = roadmap_page(&world, 4);

    let b1 = body.find("· b1-box<").expect("b1-box");
    let b2 = body.find("· b2-scale<").expect("b2-scale");
    let line = "<p class=\"meta\">cost: 4 sessions · 31 min · 2.4M in · 38.7k out</p>";
    let at = body.find(line).unwrap_or_else(|| panic!("{line}\n{body}"));
    assert!(b1 < at && at < b2, "under b1-box: {body}");
    // b2-scale is not ticked, so its line is not shown yet.
    assert_eq!(body.matches("cost:").count(), 1, "{body}");
}

#[test]
fn roadmap_cost_is_left_out_without_a_milestone_line() {
    let world = World::with_runs(
        "roadmap-no-cost",
        Some(roadmap_doc(Some("running"))),
        &["cost b1-box worker b1-t1: minutes=11 in=640000 out=9100 model=sonnet"],
    );
    let body = roadmap_page(&world, 4);
    assert!(!body.contains("cost:"), "{body}");

    let world = World::new("roadmap-no-runs", Some(roadmap_doc(Some("running"))));
    let body = roadmap_page(&world, 4);
    assert!(!body.contains("cost:"), "{body}");
}
