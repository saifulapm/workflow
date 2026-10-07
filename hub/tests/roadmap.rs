//! `GET /p/<project>/roadmap`: each milestone with its Show path, its plan
//! and, while the roadmap is a draft, the two forms that answer it.

mod common;

use std::path::PathBuf;

use common::{Hub, TempDir, body_of, fixture_mem, status_of};
use hub::page_roadmap::{MilestoneRow, roadmap_rows};

const ROADMAP: &str = "# roadmap: beta

- [x] b1-box Recipes are stored and listed
      Show: a recipe added on the phone is listed on the laptop
- [ ] b2-scale A recipe scales to any number of people  [after: b1-box]
      Show: a recipe for four, set to six, lists half again of every amount
- [ ] b3-share A recipe is shared by link
";

/// A fake `mem` that knows one project, `beta`, whose roadmap read prints
/// `roadmap` (nothing at all for a project with none) and whose stored plans
/// are b1-box and b2-scale. Every call is appended to `calls`.
struct World {
    _dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    calls: PathBuf,
}

impl World {
    fn new(tag: &str, roadmap: Option<serde_json::Value>) -> World {
        let dir = TempDir::new(tag);
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let bin = dir.join("bin");
        let calls = dir.join("calls");
        let roadmap = match roadmap {
            Some(doc) => format!("printf '%s\\n' '{doc}'"),
            None => "exit 1".to_string(),
        };
        fixture_mem(
            &bin,
            &format!(
                "echo \"$*\" >>'{calls}'\n\
                 case \"$1\" in\n\
                 projects) echo '{{\"projects\":[{{\"name\":\"beta\"}}]}}' ;;\n\
                 roadmap) {roadmap} ;;\n\
                 plan) echo '{{\"plans\":[{{\"slug\":\"b1-box\"}},{{\"slug\":\"b2-scale\"}}]}}' ;;\n\
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
                show: Some("a recipe added on the phone is listed on the laptop".to_string()),
            },
            MilestoneRow {
                slug: "b2-scale".to_string(),
                title: "A recipe scales to any number of people".to_string(),
                ticked: false,
                show: Some(
                    "a recipe for four, set to six, lists half again of every amount".to_string()
                ),
            },
            MilestoneRow {
                slug: "b3-share".to_string(),
                title: "A recipe is shared by link".to_string(),
                ticked: false,
                show: None,
            },
        ]
    );
}

#[test]
fn roadmap_a_draft_shows_each_milestone_its_plan_and_both_forms() {
    let world = World::new("roadmap-draft", Some(roadmap_doc(Some("draft"))));
    let body = roadmap_page(&world, 3);

    assert!(body.contains("draft"), "{body}");
    for text in [
        "Recipes are stored and listed",
        "A recipe scales to any number of people<",
        "a recipe added on the phone is listed on the laptop",
        "a recipe for four, set to six, lists half again of every amount",
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
    let current = body
        .find("<span class=\"pill\">current</span>")
        .expect("a current milestone");
    assert!(current < body.find("A recipe scales").unwrap(), "{body}");
    assert!(
        body.contains("<span class=\"pill mut\">open</span>"),
        "{body}"
    );
    assert!(
        body.contains("<span class=\"pill ok\">landed</span>"),
        "the ticked milestone: {body}"
    );

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
fn roadmap_an_approved_one_says_it_was_sent() {
    let world = World::new("roadmap-approved", Some(roadmap_doc(Some("approved"))));
    let body = roadmap_page(&world, 3);

    assert!(
        body.contains("<p class=\"banner ok\">approved: sent</p>"),
        "{body}"
    );
    assert!(!body.contains("<form"), "{body}");
}

#[test]
fn roadmap_one_with_an_old_stored_status_shows_no_form() {
    let world = World::new("roadmap-old-status", Some(roadmap_doc(Some("done"))));
    let body = roadmap_page(&world, 3);

    assert!(body.contains("status: done"), "{body}");
    assert!(
        body.contains("A recipe scales to any number of people<"),
        "{body}"
    );
    assert!(!body.contains("<form"), "{body}");
    assert!(!body.contains("approved: sent"), "{body}");
}

#[test]
fn roadmap_a_project_with_none_says_so() {
    let world = World::new("roadmap-none", None);
    let body = roadmap_page(&world, 2);

    assert!(body.contains("No roadmap recorded."), "{body}");
    assert!(!body.contains("<form"), "{body}");
}

#[test]
fn roadmap_shows_no_cost_line() {
    let world = World::new("roadmap-no-cost", Some(roadmap_doc(Some("approved"))));
    let body = roadmap_page(&world, 3);
    assert!(!body.contains("cost:"), "{body}");
}
