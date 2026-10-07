//! `GET /p/<project>` once a milestone is built: in dogfooding the open
//! milestone's Show path cut into steps, each marked by its findings and the
//! newest walk, then the open findings; in maintenance the status, the
//! handoff, the open findings and the ideas. Over a fake `mem`.

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
        "status": "running",
        "text": format!(
            "# roadmap: kappa\n\n\
             - [x] k1-log Habits are logged\n      Surface: web\n      Show: a habit is logged\n\
             - [ ] k2-walk Habits are walked\n      Surface: web\n{show}\
             - [ ] k3-later Later work\n      Show: something later\n"
        ),
    })
}

fn finding(id: &str, milestone: &str, step: &str, body: &str) -> Value {
    json!({"id": id, "kind": "finding", "milestone": milestone, "step": step,
           "status": "open", "body": body})
}

/// `mem log --type run` rows, newest first, as mem lists them.
fn runs(titles: &[&str]) -> Value {
    let rows: Vec<Value> = titles
        .iter()
        .enumerate()
        .map(|(i, title)| json!({"id": format!("01K0RUN{i}"), "kind": "run", "title": title, "body": ""}))
        .collect();
    json!({ "items": rows })
}

struct World {
    dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    log: PathBuf,
}

/// A fake `mem` with kappa dogfooding the milestone `k2-walk` and delta in
/// maintenance. Neither has a checkout here, so the page runs no
/// `workflow`.
fn world(tag: &str, roadmap: Value, findings: Value, runs: Value) -> World {
    let dir = TempDir::new(tag);
    let home = dir.join("home");
    let bin = dir.join("bin");
    let log = dir.join("mem.log");
    let projects = json!({"projects": [
        {"name": "kappa", "roadmap_status": "running", "runner": "laptop",
         "milestone": "k2-walk", "plan_slug": "k2-walk",
         "milestones_done": 1, "milestones_total": 3, "plan_ticked": 2, "plan_total": 2},
        {"name": "delta", "roadmap_status": "maintenance", "runner": "laptop",
         "milestones_done": 1, "milestones_total": 1},
        {"name": "omega", "roadmap_status": "running", "runner": "laptop",
         "milestone": "o2-sync", "plan_slug": "o2-sync",
         "milestones_done": 1, "milestones_total": 2, "plan_ticked": 1, "plan_total": 3},
    ]});
    let omega_roadmap = json!({"status": "running", "text":
        "# roadmap: omega\n\n- [x] o1-store Notes are stored\n- [ ] o2-sync Notes sync\n"});
    let delta_roadmap = json!({"status": "maintenance", "text":
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
             status*--project=omega*) p '{{\"text\":\"One of three tasks merged.\"}}' ;;\n\
             handoff*--project=omega*) p '{{\"body\":\"Picking up at o2-t2.\"}}' ;;\n\
             plan\\ --list*--project=kappa*) p '{{\"plans\":[{{\"slug\":\"k1-log\"}},{{\"slug\":\"k2-walk\"}}]}}' ;;\n\
             finding\\ list\\ --open*--project=kappa*) p '{findings}' ;;\n\
             finding\\ list\\ --open*--project=delta*) p '{delta_findings}' ;;\n\
             log\\ --type\\ run*--project=kappa*) p '{runs}' ;;\n\
             log\\ --type\\ run*) p '{{\"items\":[]}}' ;;\n\
             search\\ --kind\\ idea\\ --limit\\ 20\\ --project=delta\\ --json) p '{ideas}' ;;\n\
             status*--project=delta*) p '{{\"text\":\"Shipped; only fixes from here.\"}}' ;;\n\
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
                .filter(|argv| argv.starts_with("log --type run --limit 20 "))
                .count()
                >= 2
        },
    );
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
    let world = world(
        "project-after-cut",
        roadmap(Some(SHOW)),
        no_findings(),
        runs(&[]),
    );
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
fn an_open_finding_crosses_its_step_and_a_passed_walk_ticks_the_rest() {
    let findings = json!({"items": [
        finding("01K0F1", "k2-walk", "2", "the laptop lists the habit twice"),
        finding("01K0F2", "k1-log", "3", "a finding of an old milestone"),
    ]});
    let world = world(
        "project-after-marks",
        roadmap(Some(SHOW)),
        findings,
        runs(&[
            "dogfood k2-walk: pass",
            "dogfood k2-walk: findings 1",
            "merged k2-t2",
        ]),
    );
    let (body, mem) = page(&world, "kappa");

    for step in [
        "<li>✓ 1. a habit is added on the phone</li>",
        "<li>✗ 2. the laptop lists it</li>",
        "<li>✓ 3. the week view ticks it</li>",
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
fn only_the_newest_walk_of_the_milestone_ticks() {
    let world = world(
        "project-after-newest",
        roadmap(Some(SHOW)),
        no_findings(),
        runs(&[
            "dogfood k1-log: pass",
            "dogfood k2-walk: findings 1",
            "dogfood k2-walk: pass",
        ]),
    );
    let (body, _) = page(&world, "kappa");

    assert!(
        body.contains("<li>- 1. a habit is added on the phone</li>"),
        "{body}"
    );
    assert!(!body.contains("<li>✓"), "{body}");
}

#[test]
fn a_milestone_with_no_show_line_has_no_steps() {
    let world = world(
        "project-after-no-show",
        roadmap(None),
        no_findings(),
        runs(&[]),
    );
    let (body, _) = page(&world, "kappa");

    assert!(body.contains("k2-walk has no Show path"), "{body}");
    assert!(!body.contains("<li>- 1."), "{body}");
    assert!(!body.contains("something later"), "{body}");
}

#[test]
fn a_maintenance_project_shows_status_handoff_findings_and_ideas() {
    let world = world(
        "project-after-backlog",
        roadmap(Some(SHOW)),
        no_findings(),
        runs(&[]),
    );
    let (body, mem) = page(&world, "delta");

    for part in [
        "maintenance",
        "Shipped; only fixes from here.",
        "Nothing in flight; the last fix landed.",
        "a photo with no date is renamed to 1970",
        "Read the date from a video file too",
        "href=\"/p/delta/new\">New round</a>",
        "href=\"/p/delta/new\">File an idea</a>",
    ] {
        assert!(body.contains(part), "{part:?} missing from {body}");
    }
    assert!(mem.len() <= 5, "mem spawns: {mem:?}");
}

#[test]
fn the_roadmap_is_a_timeline_of_done_live_and_next_milestones() {
    let world = world(
        "project-after-timeline",
        roadmap(Some(SHOW)),
        no_findings(),
        runs(&[]),
    );
    let (body, mem) = page(&world, "kappa");

    let timeline = [
        "<h2>Roadmap</h2>\n<ul class=\"tl\">\n",
        "<li class=\"done\"><a href=\"/p/kappa/plan/k1-log\">k1-log</a> \
         <span class=\"pill ok\">landed</span><div class=\"meta\">Habits are logged</div></li>\n",
        "<li class=\"live\"><a href=\"/p/kappa/plan/k2-walk\">k2-walk</a> \
         <span class=\"pill\">2 of 2 tasks</span><div class=\"meta\">Habits are walked</div></li>\n",
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
    let world = world(
        "project-after-omega",
        roadmap(None),
        no_findings(),
        runs(&[]),
    );
    let (body, mem) = page(&world, "omega");

    assert!(body.contains("<h2>Roadmap</h2>"), "{body}");
    assert!(
        body.contains("<li class=\"live\">o2-sync <span class=\"pill\">1 of 3 tasks</span>"),
        "{body}"
    );
    assert!(body.contains("<p>0 questions waiting on you</p>"), "{body}");
    assert!(!body.contains("not answering"), "{body}");
    assert_eq!(
        body_reads(&mem),
        [
            "roadmap --project=omega --json",
            "plan --list --project=omega --json",
            "status --project=omega --json",
            "handoff --project=omega --json",
        ],
        "four reads and the route's projects read"
    );
}

#[test]
fn a_shipped_project_reads_no_roadmap_for_its_page() {
    let world = world(
        "project-after-delta",
        roadmap(None),
        no_findings(),
        runs(&[]),
    );
    let (body, mem) = page(&world, "delta");

    assert!(!body.contains("not answering"), "{body}");
    let reads = body_reads(&mem);
    assert!(reads.len() <= 4, "{reads:?}");
    assert!(
        !reads.iter().any(|argv| argv.starts_with("roadmap")),
        "{reads:?}"
    );
}
