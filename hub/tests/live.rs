//! A building project's page shows the orchestrator amx is running for its
//! current milestone and the project's last moves, or says it has stalled.
//! Over a fake `mem` and a fake `amx`.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Hub, TempDir, body_of, fixture_bin, fixture_mem, status_of, wait_for};
use serde_json::json;

struct World {
    dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    log: PathBuf,
}

/// omega is building o2-sync on this machine, which is called laptop. `amx`
/// prints `agents` when given, and is missing from PATH when not.
fn world(tag: &str, handoff: &str, agents: Option<&str>) -> World {
    let dir = TempDir::new(tag);
    let home = dir.join("home");
    let bin = dir.join("bin");
    let log = dir.join("mem.log");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), "laptop\n").unwrap();
    let projects = json!({"projects": [
        {"name": "omega", "roadmap_status": "approved", "runner": "laptop",
         "milestone": "o2-sync", "plan_slug": "o2-sync",
         "milestones_done": 1, "milestones_total": 2, "plan_ticked": 1, "plan_total": 3},
    ]});
    let roadmap = json!({"status": "approved", "text":
        "# roadmap: omega\n\n- [x] o1-store Notes are stored\n- [ ] o2-sync Notes sync\n"});
    let moves: Vec<_> = (1..=5)
        .map(|i| {
            json!({"id": format!("01K0LOG000000000000000000{i}"), "kind": "log",
                        "title": format!("move {i}"), "body": ""})
        })
        .collect();
    let moves = json!({ "items": moves });
    let handoff = json!({ "body": handoff });
    fixture_mem(
        &bin,
        &format!(
            "printf '%s\\n' \"$*\" >> '{log}'\n\
             p() {{ printf '%s\\n' \"$1\"; }}\n\
             case \"$*\" in\n\
             projects*) p '{projects}' ;;\n\
             *--all-projects*) p '{{\"questions\":[]}}' ;;\n\
             roadmap*--project=omega*) p '{roadmap}' ;;\n\
             plan\\ --list*--project=omega*) p '{{\"plans\":[{{\"slug\":\"o1-store\"}}]}}' ;;\n\
             log\\ --limit\\ 5\\ --project=omega*) p '{moves}' ;;\n\
             log\\ --type\\ run*) p '{{\"items\":[]}}' ;;\n\
             handoff*--project=omega*) p '{handoff}' ;;\n\
             *) exit 1 ;;\n\
             esac",
            log = log.display(),
        ),
    );
    if let Some(agents) = agents {
        fixture_bin(&bin, "amx", &format!("printf '%s\\n' '{agents}'"));
    }
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

/// omega's page and the page's own `mem` spawns, after the doorbell's first
/// round.
fn page(world: &World) -> (String, Vec<String>) {
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
                .any(|argv| argv.starts_with("log --type run --limit 20 "))
        },
    );
    let before = lines(&world.log).len();
    let response = hub.get("/p/omega");
    assert_eq!(status_of(&response), 200, "{response}");
    let spawns = lines(&world.log)[before..]
        .iter()
        .filter(|argv| !argv.contains("--all-projects") && !argv.starts_with("projects"))
        .cloned()
        .collect();
    (body_of(&response).to_string(), spawns)
}

const LIVE: &str = r#"[{"id":"omega-o1-store","state":"idle","ended":0,"created":1},
  {"id":"omega-o2-sync","state":"working","ended":0,"created":2}]"#;

const DEAD: &str = r#"[{"id":"omega-o1-store","state":"idle","ended":0,"created":1},
  {"id":"omega-o2-sync","state":"stopped","ended":0,"created":2}]"#;

#[test]
fn a_live_orchestrator_shows_with_its_last_moves_within_five_reads() {
    let world = world("live-running", "Picking up at o2-t2.", Some(LIVE));
    let (body, reads) = page(&world);

    assert!(
        body.contains("<span class=\"pill\">running</span> <span class=\"meta\">omega-o2-sync · working</span>"),
        "{body}"
    );
    assert!(!body.contains("stalled"), "{body}");
    assert!(body.contains("<h2>Last moves</h2>"), "{body}");
    for i in 1..=5 {
        assert!(body.contains(&format!(">move {i}</a>")), "{body}");
    }
    assert!(body.contains("Picking up at o2-t2."), "{body}");
    assert!(!body.contains("not answering"), "{body}");
    assert_eq!(
        reads,
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
fn a_dead_orchestrator_is_flagged_stalled() {
    let world = world("live-dead", "o2-t1 landed at 1a2b3c.", Some(DEAD));
    let (body, _) = page(&world);

    assert!(
        body.contains("<span class=\"pill bad\">stalled</span>"),
        "{body}"
    );
    assert!(body.contains("no live orchestrator for o2-sync"), "{body}");
}

#[test]
fn a_parked_milestone_is_not_stalled() {
    let world = world("live-parked", "parked: waiting on a question", Some(DEAD));
    let (body, _) = page(&world);

    assert!(
        body.contains("<span class=\"pill wait\">parked</span>"),
        "{body}"
    );
    assert!(!body.contains("stalled"), "{body}");
}

#[test]
fn without_amx_nothing_is_flagged() {
    let world = world("live-no-amx", "o2-t1 landed at 1a2b3c.", None);
    let (body, _) = page(&world);

    assert!(!body.contains("stalled"), "{body}");
    assert!(!body.contains("pill\">running"), "{body}");
    assert!(body.contains("<h2>Last moves</h2>"), "{body}");
}
