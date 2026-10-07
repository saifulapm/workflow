//! `GET /p/<project>` while a run is going: the waiting questions and the
//! links under the header, and no Pause or Resume form. Over a fake `mem`,
//! with a fake `workflow` on the path that must never be called.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Hub, TempDir, body_of, fixture_bin, status_of, wait_for};
use serde_json::{Value, json};

const PROJECT: &str = "gamma";

/// Two questions waiting on the owner and one already answered.
/// Every project's pending questions, as the doorbell polls them: two of
/// gamma's waiting and one answered, and one of another project's.
fn questions_doc() -> Value {
    let q = |id: &str, project: &str, body: &str, answered: bool| json!({"id": id, "project": project, "title": body, "body": body, "answered": answered, "options": []});
    json!({"questions": [
        q("01K0Q1", PROJECT, "Which hour does the nag go out?", false),
        q("01K0Q2", PROJECT, "Nag on weekends?", false),
        q("01K0Q3", PROJECT, "Store dates in UTC?", true),
        q("01K0Q4", "beta", "Approve roadmap beta?", false),
    ]})
}

/// The project's row as `mem projects --json` gives it.
struct Row {
    /// The plan's tasks are all ticked, so the milestone waits on its walk.
    dogfooding: bool,
}

const RUNNING: Row = Row { dogfooding: false };

struct World {
    dir: TempDir,
    home: PathBuf,
    bins: PathBuf,
    mem_log: PathBuf,
    workflow_log: PathBuf,
}

/// A fake `mem` answering the page's reads, and the doorbell's poll of every
/// project with no questions, and a fake `workflow` that only logs the call.
/// Each logs its argv, `workflow` with where it ran.
fn world(tag: &str, row: Row) -> World {
    let dir = TempDir::new(tag);
    let home = dir.join("home");
    let bins = dir.join("bin");
    let mem_log = dir.join("mem.log");
    let workflow_log = dir.join("workflow.log");
    let ticked = if row.dogfooding { 3 } else { 1 };
    let project = json!({
        "name": PROJECT, "roadmap_status": "approved",
        "milestone": "g2-nag", "plan_slug": "g2-nag",
        "milestones_done": 1, "milestones_total": 3,
        "plan_ticked": ticked, "plan_total": 3,
    });
    fixture_bin(
        &bins,
        "mem",
        &format!(
            "printf '%s\\n' \"$*\" >> '{log}'\n\
             p() {{ printf '%s\\n' \"$1\"; }}\n\
             case \"$*\" in\n\
             projects*) p '{projects}' ;;\n\
             *--all-projects*) p '{questions}' ;;\n\
             status*) p '{{\"text\":\"Two of six tasks merged.\"}}' ;;\n\
             handoff*) p '{{\"body\":\"Picking up at g2-t3.\"}}' ;;\n\
             log*) p '{{\"items\":[]}}' ;;\n\
             *) exit 1 ;;\n\
             esac",
            log = mem_log.display(),
            projects = json!({"projects": [project]}),
            questions = questions_doc(),
        ),
    );
    fixture_bin(
        &bins,
        "workflow",
        &format!(
            "printf '%s %s\\n' \"$PWD\" \"$*\" >> '{log}'\nexit 1",
            log = workflow_log.display(),
        ),
    );
    World {
        dir,
        home,
        bins,
        mem_log,
        workflow_log,
    }
}

/// The front page's body and the `mem` spawns it made. The doorbell's first
/// round, which reads every project on its own clock, is let finish first,
/// and its question poll is left out.
fn page(world: &World) -> (String, Vec<String>) {
    let config = world.dir.join("config.toml");
    std::fs::write(
        &config,
        "topic = \"workflow-TESTTESTTESTTESTTESTTESTTE\"\n\
         ntfy_base = \"http://127.0.0.1:9\"\n\
         siblings = [\"http://laptop:8088\"]\n",
    )
    .unwrap();
    let hub = Hub::spawn(
        &world.home,
        &[&world.bins],
        &["--port", "0", "--config", config.to_str().unwrap()],
    );
    wait_for(
        "the doorbell's first round",
        Duration::from_secs(10),
        || {
            lines(&world.mem_log)
                .iter()
                .any(|argv| argv == "projects --json")
        },
    );
    let before = lines(&world.mem_log).len();
    let response = hub.get(&format!("/p/{PROJECT}"));
    assert_eq!(status_of(&response), 200, "{response}");
    let spawns = lines(&world.mem_log)[before..]
        .iter()
        .filter(|argv| !argv.contains("--all-projects"))
        .cloned()
        .collect();
    (body_of(&response).to_string(), spawns)
}

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

const PAUSE: &str = "<input type=\"hidden\" name=\"do\" value=\"pause\">";
const RESUME: &str = "<input type=\"hidden\" name=\"do\" value=\"resume\">";

#[test]
fn a_running_project_shows_its_waiting_questions_and_links() {
    let world = world("project-run-counts", RUNNING);
    let (body, mem) = page(&world);

    for part in [
        "execution",
        "2 questions",
        "href=\"/p/gamma/questions\"",
        "href=\"/p/gamma/evidence\"",
    ] {
        assert!(body.contains(part), "{part:?} missing from {body}");
    }
    assert!(
        !body.contains("href=\"/p/gamma/run\""),
        "there is no run page: {body}"
    );
    assert!(mem.len() <= 5, "mem spawns: {mem:?}");
    assert!(
        lines(&world.workflow_log).is_empty(),
        "the hub never runs workflow: {:?}",
        lines(&world.workflow_log)
    );
}

#[test]
fn neither_a_running_nor_a_dogfooding_project_has_a_pause_button() {
    for row in [RUNNING, Row { dogfooding: true }] {
        let world = world("project-run-pause", row);
        let (body, _) = page(&world);
        assert!(!body.contains(PAUSE), "{body}");
        assert!(!body.contains(RESUME), "{body}");
    }
}
