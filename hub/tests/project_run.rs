//! `GET /p/<project>` while a run is going: the engine's stage, the task
//! counts, the dispatched tasks and the waiting questions under the header,
//! the Pause and Resume forms, and the line that shows until the engine
//! agrees with the key. Over a fake `mem` and a fake `workflow`.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Hub, TempDir, body_of, fixture_bin, status_of, wait_for};
use serde_json::{Value, json};

const PROJECT: &str = "gamma";

/// `workflow status --json` with the engine at `stage`: an older run first,
/// the newest last, and in the newest two pending tasks and one of each
/// other state.
fn status_doc(stage: &str) -> Value {
    let task = |id: &str, state: &str| {
        json!({
            "id": id, "state": state, "dispatches": 1, "session": "", "failed": "",
            "last_status": "", "merged": "", "context": 0, "held": "",
            "model": "", "minutes": 0,
        })
    };
    json!({
        "project": PROJECT, "stage": stage,
        "milestone": {"slug": "g2-nag", "n": 2, "m": 3},
        "parked": [], "findings": 0, "runner": "here", "paused": false,
        "runs": [
            {"plan": "g1-log", "live": false, "tasks": [task("g1-old", "merged")]},
            {"plan": "g2-nag", "live": true, "tasks": [
                task("g2-t1", "merged"),
                task("g2-t2", "dispatched"),
                task("g2-t3", "pending"),
                task("g2-t4", "pending"),
                task("g2-t5", "failed"),
                task("g2-t6", "blocked"),
            ]},
        ],
    })
}

/// Two questions waiting on the owner and one already answered.
fn questions_doc() -> Value {
    let q = |id: &str, body: &str, answered: bool| json!({"id": id, "title": body, "body": body, "answered": answered, "options": []});
    json!({"questions": [
        q("01K0Q1", "Which hour does the nag go out?", false),
        q("01K0Q2", "Nag on weekends?", false),
        q("01K0Q3", "Store dates in UTC?", true),
    ]})
}

/// The project's row as `mem projects --json` gives it.
struct Row {
    /// The plan's tasks are all ticked, so the milestone waits on its walk.
    dogfooding: bool,
    paused: bool,
    checkout: bool,
}

const RUNNING: Row = Row {
    dogfooding: false,
    paused: false,
    checkout: true,
};

struct World {
    dir: TempDir,
    home: PathBuf,
    bins: PathBuf,
    mem_log: PathBuf,
    workflow_log: PathBuf,
    checkout: PathBuf,
}

/// A fake `mem` answering the page's reads, and the doorbell's poll of every
/// project with no questions, and a fake `workflow` printing the status of
/// an engine at `stage`. Each logs its argv, `workflow` with where it ran.
fn world(tag: &str, row: Row, stage: &str) -> World {
    let dir = TempDir::new(tag);
    let home = dir.join("home");
    let bins = dir.join("bin");
    let checkout = dir.join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    let mem_log = dir.join("mem.log");
    let workflow_log = dir.join("workflow.log");
    let ticked = if row.dogfooding { 3 } else { 1 };
    let mut project = json!({
        "name": PROJECT, "roadmap_status": "running", "runner": "laptop",
        "milestone": "g2-nag", "plan_slug": "g2-nag",
        "milestones_done": 1, "milestones_total": 3,
        "plan_ticked": ticked, "plan_total": 3,
        "checkouts": if row.checkout { vec![checkout.display().to_string()] } else { vec![] },
    });
    if row.paused {
        project["paused"] = json!("laptop 2026-10-05");
    }
    fixture_bin(
        &bins,
        "mem",
        &format!(
            "printf '%s\\n' \"$*\" >> '{log}'\n\
             p() {{ printf '%s\\n' \"$1\"; }}\n\
             case \"$*\" in\n\
             projects*) p '{projects}' ;;\n\
             *--all-projects*) p '{{\"questions\":[]}}' ;;\n\
             questions*--pending*--project={PROJECT}*) p '{questions}' ;;\n\
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
            "printf '%s %s\\n' \"$PWD\" \"$*\" >> '{log}'\nprintf '%s\\n' '{status}'",
            log = workflow_log.display(),
            status = status_doc(stage),
        ),
    );
    World {
        dir,
        home,
        bins,
        mem_log,
        workflow_log,
        checkout,
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
                .any(|argv| argv.starts_with("log --type run --limit 20 "))
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
const PAUSE_SENT: &str = "pause sent, waiting for the engine";
const RESUME_SENT: &str = "resume sent, waiting for the engine";

#[test]
fn a_running_project_shows_the_engine_stage_counts_and_waiting_questions() {
    let world = world("project-run-counts", RUNNING, "execution");
    let (body, mem) = page(&world);

    for part in [
        "execution",
        "1 dispatched",
        "2 pending",
        "1 failed",
        "1 blocked",
        "1 merged",
        "g2-t2",
        "2 questions",
        "href=\"/p/gamma/run\"",
        "href=\"/p/gamma/questions\"",
        "href=\"/p/gamma/evidence\"",
    ] {
        assert!(body.contains(part), "{part:?} missing from {body}");
    }
    assert!(
        !body.contains("g1-old"),
        "an older run is not counted: {body}"
    );
    assert!(
        !body.contains("2 merged"),
        "an older run is not counted: {body}"
    );
    assert!(mem.len() <= 5, "mem spawns: {mem:?}");
    assert_eq!(
        lines(&world.workflow_log),
        [format!("{} status --json", world.checkout.display())],
        "one status read, in the checkout"
    );
}

#[test]
fn a_running_project_has_a_pause_button() {
    let world = world("project-run-pause", RUNNING, "execution");
    let (body, _) = page(&world);

    assert!(
        body.contains("<form method=\"post\" action=\"/p/gamma/control\">"),
        "{body}"
    );
    assert!(body.contains(PAUSE), "{body}");
    assert!(!body.contains(RESUME), "{body}");
    assert!(!body.contains(PAUSE_SENT), "{body}");
    assert!(!body.contains(RESUME_SENT), "{body}");
}

#[test]
fn a_dogfooding_project_has_a_pause_button() {
    let row = Row {
        dogfooding: true,
        ..RUNNING
    };
    let world = world("project-run-dogfood", row, "dogfood");
    let (body, _) = page(&world);

    assert!(body.contains("dogfooding"), "{body}");
    assert!(body.contains(PAUSE), "{body}");
    assert!(!body.contains(RESUME), "{body}");
}

#[test]
fn a_paused_project_has_resume_in_place_of_pause() {
    let row = Row {
        paused: true,
        ..RUNNING
    };
    let world = world("project-run-resume", row, "paused");
    let (body, _) = page(&world);

    assert!(body.contains(RESUME), "{body}");
    assert!(!body.contains(PAUSE), "{body}");
}

#[test]
fn pause_waits_until_the_engine_stage_is_paused() {
    let row = Row {
        paused: true,
        ..RUNNING
    };
    let sent = world("project-run-pause-sent", row, "execution");
    let (body, _) = page(&sent);
    assert!(body.contains(PAUSE_SENT), "{body}");
    assert!(body.contains(RESUME), "{body}");

    let row = Row {
        paused: true,
        ..RUNNING
    };
    let agreed = world("project-run-paused", row, "paused");
    let (body, _) = page(&agreed);
    assert!(!body.contains(PAUSE_SENT), "the engine agrees: {body}");
}

#[test]
fn resume_waits_until_the_engine_stage_leaves_paused() {
    let sent = world("project-run-resume-sent", RUNNING, "paused");
    let (body, _) = page(&sent);
    assert!(body.contains(RESUME_SENT), "{body}");
    assert!(body.contains(PAUSE), "{body}");

    let agreed = world("project-run-resumed", RUNNING, "execution");
    let (body, _) = page(&agreed);
    assert!(!body.contains(RESUME_SENT), "the engine agrees: {body}");
}

#[test]
fn with_no_checkout_here_the_line_names_the_runner() {
    let row = Row {
        paused: true,
        checkout: false,
        ..RUNNING
    };
    let world = world("project-run-elsewhere", row, "execution");
    let (body, mem) = page(&world);

    assert!(
        body.contains("pause sent; laptop's engine reads the key within fifteen minutes"),
        "{body}"
    );
    assert!(body.contains(RESUME), "{body}");
    assert!(mem.len() <= 5, "mem spawns: {mem:?}");
    assert!(
        lines(&world.workflow_log).is_empty(),
        "no checkout, no workflow"
    );
}
