//! `GET /p/<project>/run`: the newest run's board, its live agents and the
//! run log, from a fake `workflow` printing a recorded status and a fake
//! `mem`.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Hub, TempDir, body_of, fixture_bin, status_of, wait_for};
use serde_json::{Value, json};

const PROJECT: &str = "gamma";

/// `workflow status --json`'s shape, with an older run first and the newest
/// last, the order status lists them in. The newest run's tasks are out of
/// board order, so the grouping is the page's work.
fn status_doc() -> Value {
    let task = |id: &str, state: &str, session: &str, model: &str, minutes: u64, last: &str| {
        json!({
            "id": id, "state": state, "dispatches": 1, "session": session, "failed": "",
            "last_status": last, "merged": "", "context": 0, "held": "",
            "model": model, "minutes": minutes,
        })
    };
    json!({
        "project": PROJECT, "stage": "execution",
        "milestone": {"slug": "g2-nag", "n": 2, "m": 3},
        "parked": [], "findings": 0, "runner": "here", "paused": false,
        "runs": [
            {"plan": "g1-log", "live": false, "base": "aaa", "integration": "integration/g1-log",
             "context": 0, "tasks": [task("g1-old", "merged", "", "", 0, "ready: old work")]},
            {"plan": "g2-nag", "live": true, "base": "bbb", "integration": "integration/g2-nag",
             "context": 0, "tasks": [
                task("g2-t1", "merged", "", "", 0, "ready: due dates parse"),
                task("g2-t5", "blocked", "", "", 0, "blocked: which clock?"),
                task("g2-t3", "pending", "", "", 0, ""),
                task("g2-t2", "dispatched", "gamma-g2-t2", "opus-test", 7, "progress: reading the due store"),
                task("g2-t4", "failed", "", "", 0, "verify red twice"),
             ]},
        ],
    })
}

fn projects_doc(checkouts: &[&Path]) -> Value {
    json!({"projects": [{
        "id": "p-gamma", "name": PROJECT, "remote": null, "aliases": [],
        "created": "2026-01-01", "items": 3, "current": false,
        "checkouts": checkouts.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
    }]})
}

fn run_log_doc() -> Value {
    let item = |title: &str, created: &str| json!({"kind": "log", "type": "run", "title": title, "created": created});
    json!({"items": [
        item("dogfood g1-log: pass", "2026-10-04"),
        item("run g2-nag: dispatched g2-t2", "2026-10-03"),
    ]})
}

/// What the page reads, and the logs of what it ran.
struct World {
    dir: TempDir,
    home: PathBuf,
    bins: PathBuf,
    mem_log: PathBuf,
    workflow_log: PathBuf,
    checkout: PathBuf,
}

/// A fake `mem` answering the reads the page makes, and the doorbell's
/// questions with none, and a fake `workflow` printing `status`. Each logs
/// its argv, `workflow` with the directory it ran in.
fn world(tag: &str, with_checkout: bool, status: &Value) -> World {
    let dir = TempDir::new(tag);
    let home = dir.join("home");
    let bins = dir.join("bin");
    let checkout = dir.join("checkout");
    std::fs::create_dir_all(&checkout).unwrap();
    let mem_log = dir.join("mem.log");
    let workflow_log = dir.join("workflow.log");
    let checkouts: Vec<&Path> = if with_checkout {
        vec![&checkout]
    } else {
        vec![]
    };
    fixture_bin(
        &bins,
        "mem",
        &format!(
            "printf '%s\\n' \"$*\" >> '{log}'\n\
             case \"$1\" in\n\
             projects) printf '%s\\n' '{projects}' ;;\n\
             log) printf '%s\\n' '{run_log}' ;;\n\
             project) printf '%s\\n' '{current}' ;;\n\
             questions) printf '%s\\n' '{{\"questions\":[]}}' ;;\n\
             *) exit 1 ;;\n\
             esac",
            log = mem_log.display(),
            projects = projects_doc(&checkouts),
            run_log = run_log_doc(),
            current = json!({"name": PROJECT, "runner": "nuc", "root": null}),
        ),
    );
    fixture_bin(
        &bins,
        "workflow",
        &format!(
            "printf '%s %s\\n' \"$PWD\" \"$*\" >> '{log}'\nprintf '%s\\n' '{status}'",
            log = workflow_log.display(),
            status = status,
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

/// The hub, with `nuc` as its one sibling: the page's body, and the `mem`
/// spawns the page made. The doorbell's first round, which reads the
/// projects and each project's run lines on its own clock, is let finish
/// first and left out, as is its question poll.
fn page(world: &World) -> (String, Vec<String>) {
    let config = world.dir.join("config.toml");
    std::fs::write(
        &config,
        "topic = \"workflow-TESTTESTTESTTESTTESTTESTTE\"\n\
         ntfy_base = \"http://127.0.0.1:9\"\n\
         siblings = [\"http://nuc:8088\"]\n",
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
    let response = hub.get(&format!("/p/{PROJECT}/run"));
    assert_eq!(status_of(&response), 200, "{response}");
    let spawns = lines(&world.mem_log)[before..]
        .iter()
        .filter(|argv| !argv.starts_with("questions"))
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

fn position(body: &str, needle: &str) -> usize {
    body.find(needle)
        .unwrap_or_else(|| panic!("{needle:?} missing from {body}"))
}

#[test]
fn the_board_groups_the_newest_run_by_state_in_order() {
    let world = world("run-board", true, &status_doc());
    let (body, _) = page(&world);

    assert!(body.contains("g2-nag"), "plan: {body}");
    assert!(body.contains("execution"), "stage word: {body}");
    assert!(body.contains("milestone 2 of 3"), "place: {body}");
    assert!(!body.contains("g1-old"), "an older run stays off: {body}");

    let groups = ["dispatched", "pending", "failed", "blocked", "merged"]
        .map(|state| position(&body, &format!("<h3>{state}</h3>")));
    assert!(
        groups.is_sorted(),
        "groups out of order: {groups:?} in {body}"
    );

    let tasks = ["g2-t2", "g2-t3", "g2-t4", "g2-t5", "g2-t1"].map(|id| position(&body, id));
    assert!(tasks.is_sorted(), "tasks out of order: {tasks:?} in {body}");
    assert!(
        body.contains("ready: due dates parse"),
        "last status: {body}"
    );
    assert!(body.contains("verify red twice"), "last status: {body}");
}

#[test]
fn a_dispatched_task_is_a_live_agent_with_its_model_and_minutes() {
    let world = world("run-agent", true, &status_doc());
    let (body, _) = page(&world);

    let agents = &body[position(&body, "<h2>Agents</h2>")..];
    for part in [
        "g2-t2",
        "gamma-g2-t2",
        "opus-test",
        "7 min",
        "progress: reading the due store",
    ] {
        assert!(
            agents.contains(part),
            "{part:?} missing from the agent row: {agents}"
        );
    }
    assert!(
        !agents.contains("g2-t3"),
        "a pending task is no agent: {agents}"
    );
}

#[test]
fn the_run_log_lists_each_title_with_its_date() {
    let world = world("run-log", true, &status_doc());
    let (body, _) = page(&world);

    let log = &body[position(&body, "<h2>Run log</h2>")..];
    assert!(log.contains("dogfood g1-log: pass"), "{log}");
    assert!(log.contains("2026-10-04"), "{log}");
    assert!(log.contains("run g2-nag: dispatched g2-t2"), "{log}");
    assert!(log.contains("2026-10-03"), "{log}");
}

#[test]
fn the_page_costs_three_mem_spawns_and_one_workflow() {
    let world = world("run-spawns", true, &status_doc());
    let (_, mem) = page(&world);

    assert!(mem.len() <= 3, "mem spawns: {mem:?}");
    assert!(
        mem.contains(&format!(
            "log --type run --limit 30 --project={PROJECT} --json"
        )),
        "run log read: {mem:?}"
    );
    assert_eq!(
        lines(&world.workflow_log),
        [format!("{} status --json", world.checkout.display())],
        "one status read, in the checkout"
    );
}

#[test]
fn a_project_with_no_checkout_here_names_its_runner_and_links_its_hub() {
    let world = world("run-elsewhere", false, &status_doc());
    let (body, mem) = page(&world);

    assert!(body.contains("nuc runs it"), "runner: {body}");
    assert!(mem.len() <= 3, "mem spawns: {mem:?}");
    assert!(
        body.contains(&format!("href=\"http://nuc:8088/p/{PROJECT}/run\"")),
        "sibling link: {body}"
    );
    assert!(
        lines(&world.workflow_log).is_empty(),
        "no checkout, no workflow"
    );
    assert!(
        mem.contains(&format!("project current --project={PROJECT} --json")),
        "{mem:?}"
    );
    assert!(
        body.contains("<h2>Run log</h2>"),
        "the log is mem's: {body}"
    );
}

#[test]
fn a_checkout_with_no_run_says_so() {
    let mut status = status_doc();
    status["runs"] = json!([]);
    let world = world("run-none", true, &status);
    let (body, _) = page(&world);

    assert!(body.contains("No run in this checkout"), "{body}");
    assert!(!body.contains("<h3>"), "no board: {body}");
}
