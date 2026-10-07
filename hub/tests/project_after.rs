//! `GET /p/<project>` once a milestone is built: in dogfooding the open
//! milestone's Show path cut into steps, each marked by its findings, then
//! the open findings; once the roadmap is done the handoff, the open
//! findings and the ideas. Over a fake `mem`.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Hub, TempDir, body_of, fixture_mem, status_of, wait_for};
use serde_json::{Value, json};

/// The open milestone's Show line: three clauses, the second and third
/// after `; ` and with a leading `and ` or `then `.
const SHOW: &str =
    "a habit is added on the phone, and the laptop lists it; then the week view ticks it";

/// A roadmap whose first milestone is done and whose second is open with
/// `show` as its Show line, or none.
fn roadmap(show: Option<&str>) -> Value {
    let show = show
        .map(|s| format!("      Show: {s}\n"))
        .unwrap_or_default();
    json!({
        "status": "approved",
        "text": format!(
            "# roadmap: kappa\n\n\
             - [x] k1-log Habits are logged\n      Show: a habit is logged\n\
             - [ ] k2-walk Habits are walked\n{show}\
             - [ ] k3-later Later work\n      Show: something later\n"
        ),
    })
}

fn finding(id: &str, milestone: &str, step: &str, body: &str) -> Value {
    json!({"id": id, "kind": "finding", "milestone": milestone, "step": step,
           "status": "open", "body": body})
}

struct World {
    dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    log: PathBuf,
}

/// A fake `mem` with kappa dogfooding the milestone `k2-walk` and delta's
/// roadmap done. Neither has a checkout here, so the page runs no
/// `workflow`.
fn world(tag: &str, roadmap: Value, findings: Value) -> World {
    let dir = TempDir::new(tag);
    let home = dir.join("home");
    let bin = dir.join("bin");
    let log = dir.join("mem.log");
    let projects = json!({"projects": [
        {"name": "kappa", "roadmap_status": "approved",
         "milestone": "k2-walk", "plan_slug": "k2-walk",
         "milestones_done": 1, "milestones_total": 3, "plan_ticked": 2, "plan_total": 2},
        {"name": "delta", "roadmap_status": "approved",
         "milestones_done": 1, "milestones_total": 1},
        {"name": "omega", "roadmap_status": "approved",
         "milestone": "o2-sync", "plan_slug": "o2-sync",
         "milestones_done": 1, "milestones_total": 2, "plan_ticked": 1, "plan_total": 3},
    ]});
    let omega_roadmap = json!({"status": "approved", "text":
        "# roadmap: omega\n\n- [x] o1-store Notes are stored\n- [ ] o2-sync Notes sync\n"});
    let delta_roadmap = json!({"status": "approved", "text":
        "# roadmap: delta\n\n- [x] d1-rename Photos are renamed\n"});
    let ideas = json!({"items": [
        {"id": "01K0IDEA1", "kind": "idea", "title": "Read the date from a video file too",
         "body": "\nRead the date from a video file too\n"},
    ]});
    let delta_findings = json!({"items": [
        finding("01K0F9", "d1-rename", "1", "a photo with no date is renamed to 1970"),
    ]});
    fixture_mem(
        &bin,
        &format!(
            "printf '%s\\n' \"$*\" >> '{log}'\n\
             p() {{ printf '%s\\n' \"$1\"; }}\n\
             case \"$*\" in\n\
             projects*) p '{projects}' ;;\n\
             *--all-projects*) p '{{\"questions\":[]}}' ;;\n\
             roadmap*--project=kappa*) p '{roadmap}' ;;\n\
             roadmap*--project=omega*) p '{omega_roadmap}' ;;\n\
             roadmap*--project=delta*) p '{delta_roadmap}' ;;\n\
             plan\\ --list*--project=omega*) p '{{\"plans\":[{{\"slug\":\"o1-store\"}}]}}' ;;\n\
             plan\\ --list*--project=delta*) p '{{\"plans\":[{{\"slug\":\"d1-rename\"}}]}}' ;;\n\
             log\\ --limit\\ 5\\ --project=omega*) p '{{\"items\":[]}}' ;;\n\
             handoff*--project=omega*) p '{{\"body\":\"Picking up at o2-t2.\"}}' ;;\n\
             plan\\ --list*--project=kappa*) p '{{\"plans\":[{{\"slug\":\"k1-log\"}},{{\"slug\":\"k2-walk\"}}]}}' ;;\n\
             finding\\ list\\ --open*--project=kappa*) p '{findings}' ;;\n\
             finding\\ list\\ --open*--project=delta*) p '{delta_findings}' ;;\n\
             search\\ --kind\\ idea\\ --limit\\ 20\\ --project=delta\\ --json) p '{ideas}' ;;\n\
             handoff*--project=delta*) p '{{\"body\":\"Nothing in flight; the last fix landed.\"}}' ;;\n\
             *) exit 1 ;;\n\
             esac",
            log = log.display(),
        ),
    );
    World {
        dir,
        home,
        bin,
        log,
    }
}

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// The front page's body and the `mem` spawns it made, after the
/// doorbell's first round, whose question poll is left out.
fn page(world: &World, project: &str) -> (String, Vec<String>) {
    let config = world.dir.join("config.toml");
    std::fs::write(
        &config,
        "topic = \"workflow-TESTTESTTESTTESTTESTTESTTE\"\n\
         ntfy_base = \"http://127.0.0.1:9\"\n",
    )
    .unwrap();
    let hub = Hub::spawn(
        &world.home,
        &[&world.bin],
        &["--port", "0", "--config", config.to_str().unwrap()],
    );
    wait_for(
        "the doorbell's first round",
        Duration::from_secs(10),
        || {
            lines(&world.log)
                .iter()
                .any(|argv| argv == "projects --json")
        },
    );
    // One page through mem's gate, so the round's projects read has finished
    // and filled the cache before the counted request.
    hub.get("/");
    let before = lines(&world.log).len();
    let response = hub.get(&format!("/p/{project}"));
    assert_eq!(status_of(&response), 200, "{response}");
    let spawns = lines(&world.log)[before..]
        .iter()
        .filter(|argv| !argv.contains("--all-projects"))
        .cloned()
        .collect();
    (body_of(&response).to_string(), spawns)
}

fn no_findings() -> Value {
    json!({"items": []})
}

#[test]
fn a_three_clause_show_line_is_three_numbered_steps() {
    let world = world("project-after-cut", roadmap(Some(SHOW)), no_findings());
    let (body, mem) = page(&world, "kappa");

    assert!(body.contains("dogfooding"), "{body}");
    for step in [
        "<li>- 1. a habit is added on the phone</li>",
        "<li>- 2. the laptop lists it</li>",
        "<li>- 3. the week view ticks it</li>",
    ] {
        assert!(body.contains(step), "{step:?} missing from {body}");
    }
    assert!(!body.contains("4. "), "{body}");
    assert!(
        !body.contains("something later"),
        "only the open milestone: {body}"
    );
    assert!(
        body.contains("href=\"/p/kappa/new\">File a finding</a>"),
        "{body}"
    );
    assert!(mem.len() <= 5, "mem spawns: {mem:?}");
}

#[test]
fn an_open_finding_crosses_its_step() {
    let findings = json!({"items": [
        finding("01K0F1", "k2-walk", "2", "the laptop lists the habit twice"),
        finding("01K0F2", "k1-log", "3", "a finding of an old milestone"),
    ]});
    let world = world("project-after-marks", roadmap(Some(SHOW)), findings);
    let (body, mem) = page(&world, "kappa");

    for step in [
        "<li>- 1. a habit is added on the phone</li>",
        "<li>✗ 2. the laptop lists it</li>",
        "<li>- 3. the week view ticks it</li>",
    ] {
        assert!(body.contains(step), "{step:?} missing from {body}");
    }
    assert!(body.contains("step 2"), "{body}");
    assert!(body.contains("the laptop lists the habit twice"), "{body}");
    assert!(
        !body.contains("of an old milestone"),
        "another milestone's finding: {body}"
    );
    assert!(
        body.contains("href=\"/p/kappa/new\">File a finding</a>"),
        "{body}"
    );
    assert!(mem.len() <= 5, "mem spawns: {mem:?}");
}

#[test]
fn a_milestone_with_no_show_line_has_no_steps() {
    let world = world("project-after-no-show", roadmap(None), no_findings());
    let (body, _) = page(&world, "kappa");

    assert!(body.contains("k2-walk has no Show path"), "{body}");
    assert!(!body.contains("<li>- 1."), "{body}");
    assert!(!body.contains("something later"), "{body}");
}

#[test]
fn a_done_project_shows_handoff_findings_and_ideas() {
    let world = world("project-after-backlog", roadmap(Some(SHOW)), no_findings());
    let (body, mem) = page(&world, "delta");

    for part in [
        "<span class=\"pill ok\">done</span>",
        "Nothing in flight; the last fix landed.",
        "a photo with no date is renamed to 1970",
        "Read the date from a video file too",
        "href=\"/p/delta/new\">File an idea</a>",
    ] {
        assert!(body.contains(part), "{part:?} missing from {body}");
    }
    for gone in [
        "maintenance",
        "New round",
        "<h2>Status</h2>",
        "<h2>Run</h2>",
    ] {
        assert!(!body.contains(gone), "{gone:?} in {body}");
    }
    assert!(mem.len() <= 5, "mem spawns: {mem:?}");
}

#[test]
fn the_roadmap_is_a_timeline_of_done_live_and_next_milestones() {
    let world = world("project-after-timeline", roadmap(Some(SHOW)), no_findings());
    let (body, mem) = page(&world, "kappa");

    let timeline = [
        "<h2>Roadmap</h2>\n<ul class=\"tl\">\n",
        "<li class=\"done\"><a href=\"/p/kappa/plan/k1-log\">k1-log</a> \
         <span class=\"pill ok\">landed</span><div class=\"meta\">Habits are logged</div></li>\n",
        "<li class=\"live\"><a href=\"/p/kappa/plan/k2-walk\">k2-walk</a> \
         <a href=\"/p/kappa/plan\"><span class=\"pill\">2 of 2 tasks</span></a>\
         <div class=\"meta\">Habits are walked</div></li>\n",
        "<li>k3-later <span class=\"pill mut\">next</span>\
         <div class=\"meta\">Later work</div></li>\n",
        "</ul>\n",
    ]
    .concat();
    assert!(body.contains(&timeline), "{body}");
    assert!(mem.len() <= 5, "mem spawns: {mem:?}");
}

/// The body's own reads, after the route's projects read: the doorbell keeps
/// the questions of every project warm, so a page reads those for free.
fn body_reads(spawns: &[String]) -> Vec<String> {
    spawns
        .iter()
        .filter(|argv| !argv.starts_with("projects"))
        .cloned()
        .collect()
}

#[test]
fn a_building_project_draws_its_timeline_within_five_reads() {
    let world = world("project-after-omega", roadmap(None), no_findings());
    let (body, mem) = page(&world, "omega");

    assert!(body.contains("<h2>Roadmap</h2>"), "{body}");
    assert!(
        body.contains(
            "<li class=\"live\">o2-sync <a href=\"/p/omega/plan\"><span class=\"pill\">1 of 3 tasks</span></a>"
        ),
        "{body}"
    );
    assert!(body.contains("<p>0 questions waiting on you</p>"), "{body}");
    assert!(!body.contains("not answering"), "{body}");
    assert_eq!(
        body_reads(&mem),
        [
            "roadmap --project=omega --json",
            "plan --list --project=omega --json",
            "log --limit 5 --project=omega --json",
            "handoff --project=omega --json",
        ],
        "four reads and the route's projects read"
    );
}

#[test]
fn a_shipped_project_reads_no_roadmap_for_its_page() {
    let world = world("project-after-delta", roadmap(None), no_findings());
    let (body, mem) = page(&world, "delta");

    assert!(!body.contains("not answering"), "{body}");
    let reads = body_reads(&mem);
    assert!(reads.len() <= 4, "{reads:?}");
    assert!(
        !reads.iter().any(|argv| argv.starts_with("roadmap")),
        "{reads:?}"
    );
}
