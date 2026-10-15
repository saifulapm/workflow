//! A building project's page shows the orchestrator its runner's machine
//! reports for its current milestone and the project's last moves, or says
//! it has stalled. Over a fake `mem` and a device agent's polls.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Hub, TempDir, body_of, fixture_bin, fixture_mem, status_of, wait_for};
use serde_json::{Value, json};

struct World {
    dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    log: PathBuf,
    /// The laptop agent's report, or none when it never polls.
    report: Option<String>,
}

/// omega is building o2-sync, run on the laptop as omega-o2-sync. The
/// laptop's agent reports `agents` and a checkout of omega when given, and
/// never polls when not.
fn world(tag: &str, handoff: &str, agents: Option<&str>) -> World {
    let runner = json!({"machine": "laptop", "id": "omega-o2-sync",
                        "since": "2026-10-15T07:00:00Z"});
    world_with(tag, handoff, agents, runner, json!(["omega"]), false)
}

/// `world`, with omega's runner as `runner`, the laptop's checkouts as
/// `checkouts`, and with both its milestones ticked when `finished`.
fn world_with(
    tag: &str,
    handoff: &str,
    agents: Option<&str>,
    runner: Value,
    checkouts: Value,
    finished: bool,
) -> World {
    let dir = TempDir::new(tag);
    let home = dir.join("home");
    let bin = dir.join("bin");
    let log = dir.join("mem.log");
    let (done, ticked, open) = if finished {
        (2, 3, json!(null))
    } else {
        (1, 1, json!("o2-sync"))
    };
    let projects = json!({"projects": [
        {"name": "omega", "roadmap_status": "approved", "checkouts": [],
         "milestone": open, "plan_slug": "o2-sync", "runner": runner,
         "milestones_done": done, "milestones_total": 2, "plan_ticked": ticked, "plan_total": 3},
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
    // A file, so a test can pause the project under a running hub.
    std::fs::write(dir.join("projects.json"), projects.to_string()).unwrap();
    fixture_mem(
        &bin,
        &format!(
            "printf '%s\\n' \"$*\" >> '{log}'\n\
             p() {{ printf '%s\\n' \"$1\"; }}\n\
             case \"$*\" in\n\
             projects*) cat '{projects}' ;;\n\
             *--all-projects*) p '{{\"questions\":[]}}' ;;\n\
             roadmap*--project=omega*) p '{roadmap}' ;;\n\
             plan\\ --list*--project=omega*) p '{{\"plans\":[{{\"slug\":\"o1-store\"}}]}}' ;;\n\
             log\\ --limit\\ 5\\ --project=omega*) p '{moves}' ;;\n\
             handoff*--project=omega*) p '{handoff}' ;;\n\
             finding\\ list*--project=omega*) p '{{\"items\":[]}}' ;;\n\
             search*--project=omega*) p '{{\"items\":[]}}' ;;\n\
             *) exit 1 ;;\n\
             esac",
            log = log.display(),
            projects = dir.join("projects.json").display(),
        ),
    );
    let report = agents.map(|agents| {
        format!("{{\"machine\":\"laptop\",\"agents\":{agents},\"checkouts\":{checkouts}}}")
    });
    World {
        dir,
        home,
        bin,
        log,
        report,
    }
}

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// A hub over `world`, after the laptop's first poll when it polls.
fn hub(world: &World) -> Hub {
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
    if let Some(report) = &world.report {
        hub.poll_agent(report);
    }
    hub
}

/// The commands the laptop's next poll is handed.
fn queued(world: &World, hub: &Hub) -> Value {
    let report = world
        .report
        .as_deref()
        .unwrap_or("{\"machine\":\"laptop\",\"agents\":[],\"checkouts\":[\"omega\"]}");
    serde_json::from_str::<Value>(&hub.poll_agent(report)).unwrap()["commands"].clone()
}

fn starts(commands: &Value) -> usize {
    commands
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "start")
        .count()
}

/// omega's page and the page's own `mem` spawns, after the doorbell's first
/// round.
fn page(world: &World) -> (String, Vec<String>) {
    let hub = hub(world);
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
    // The orchestrator that parked ended its own session, and his answer is
    // followed by this button.
    assert!(
        body.contains("<button type=\"submit\">Resume</button>"),
        "{body}"
    );
}

#[test]
fn a_project_no_online_machine_has_checked_out_offers_no_start_and_is_not_stalled() {
    for handoff in ["", "o2-t1 landed at 1a2b3c."] {
        let world = world_with(
            "live-elsewhere",
            handoff,
            Some(DEAD),
            Value::Null,
            json!([]),
            false,
        );
        let (body, _) = page(&world);

        assert!(!body.contains("stalled"), "{body}");
        assert!(!body.contains("value=\"go\""), "{body}");
        assert!(!body.contains(">Start"), "{body}");
        assert!(!body.contains(">Resume"), "{body}");
        assert!(body.contains("<h2>Last moves</h2>"), "{body}");
    }
}

#[test]
fn a_roadmap_with_every_milestone_ticked_reads_done_with_no_run() {
    for handoff in ["", "o2 landed at 1a2b3c."] {
        let runner = json!({"machine": "laptop", "id": "omega-o2-sync"});
        let world = world_with(
            "live-finished",
            handoff,
            Some(DEAD),
            runner,
            json!(["omega"]),
            true,
        );
        let (body, _) = page(&world);

        assert!(
            body.contains("<span class=\"pill ok\">done</span>"),
            "{body}"
        );
        for run in [
            "execution",
            "stalled",
            "parked",
            "pill\">running",
            "value=\"go\"",
            ">Start</button>",
            ">Resume</button>",
            "<h2>Run</h2>",
            "<h2>Roadmap</h2>",
            "<h2>Last moves</h2>",
        ] {
            assert!(!body.contains(run), "{run}: {body}");
        }
    }
}

#[test]
fn a_runner_whose_machine_never_polled_is_not_flagged() {
    let world = world("live-away", "o2-t1 landed at 1a2b3c.", None);
    let (body, _) = page(&world);

    assert!(!body.contains("stalled"), "{body}");
    assert!(!body.contains("pill\">running"), "{body}");
    assert!(!body.contains("value=\"go\""), "{body}");
    assert!(body.contains("laptop offline"), "{body}");
    assert!(body.contains("<h2>Last moves</h2>"), "{body}");
}

#[test]
fn resume_queues_a_start_for_the_runners_machine() {
    let world = world("live-resume", "o2-t1 landed at 1a2b3c.", Some(DEAD));
    let hub = hub(&world);

    let body = body_of(&hub.get("/p/omega")).to_string();
    assert!(
        body.contains(
            "<form method=\"post\" action=\"/p/omega/control\">\n\
             <input type=\"hidden\" name=\"do\" value=\"go\">\n\
             <input type=\"hidden\" name=\"machine\" value=\"laptop\">\n\
             <button type=\"submit\">Resume</button>"
        ),
        "{body}"
    );

    let response = hub.post_form("/p/omega/control", "do=go&machine=laptop");
    assert_eq!(status_of(&response), 303, "{response}");
    assert_eq!(
        common::header_of(&response, "location"),
        Some("/p/omega"),
        "{response}"
    );
    assert_eq!(
        queued(&world, &hub),
        json!([{"kind": "start", "project": "omega"}]),
        "one start, for the laptop"
    );
}

#[test]
fn a_project_with_no_handoff_offers_start() {
    let world = world("live-start", "", Some(DEAD));
    let hub = hub(&world);
    let body = body_of(&hub.get("/p/omega")).to_string();
    assert!(
        body.contains("<button type=\"submit\">Start</button>"),
        "{body}"
    );
}

#[test]
fn a_live_orchestrator_is_not_started_twice() {
    let world = world("live-twice", "", Some(LIVE));
    let hub = hub(&world);

    let body = body_of(&hub.get("/p/omega")).to_string();
    assert!(!body.contains("value=\"go\""), "{body}");
    let response = hub.post_form("/p/omega/control", "do=go&machine=laptop");
    assert_eq!(status_of(&response), 409, "{response}");
    assert!(
        body_of(&response).contains("omega-o2-sync is already running."),
        "{response}"
    );
    assert_eq!(queued(&world, &hub), json!([]));
}

#[test]
fn a_go_while_the_runners_machine_is_away_says_why() {
    let world = world("live-refused", "", None);
    let hub = hub(&world);
    let response = hub.post_form("/p/omega/control", "do=go&machine=laptop");
    assert_eq!(status_of(&response), 409, "{response}");
    assert!(
        body_of(&response).contains("omega reads laptop offline."),
        "{response}"
    );
    assert_eq!(queued(&world, &hub), json!([]));
}

#[test]
fn a_device_that_never_paired_cannot_press_a_button() {
    let world = world("live-unpaired", "o2-t1 landed at 1a2b3c.", Some(DEAD));
    let hub = hub(&world);

    for cookie in ["theme=dark", "hub_device=NOTADEVICENOTADEVICENOTADE"] {
        let response = hub.post_form_with(
            "/p/omega/control",
            "do=go&machine=laptop",
            &[("Origin", &hub.origin()), ("Cookie", cookie)],
        );
        assert_eq!(status_of(&response), 403, "{cookie}: {response}");
        assert!(body_of(&response).contains("hub pair"), "{response}");
    }
    let before = lines(&world.log).len();
    let response = hub.post_form_unpaired("/answer", "id=01K0Q&text=yes");
    assert_eq!(status_of(&response), 403, "{response}");
    assert!(
        !lines(&world.log)[before..]
            .iter()
            .any(|argv| argv.starts_with("answer")),
        "an unpaired answer reached mem"
    );
    assert_eq!(queued(&world, &hub), json!([]));

    // The paired device still can.
    assert_eq!(
        status_of(&hub.post_form("/p/omega/control", "do=go&machine=laptop")),
        303
    );
}

#[test]
fn the_page_after_resume_shows_the_new_orchestrator() {
    let world = world("live-after-resume", "o2-t1 landed at 1a2b3c.", Some(DEAD));
    let hub = hub(&world);
    assert!(body_of(&hub.get("/p/omega")).contains("stalled"));

    let response = hub.post_form("/p/omega/control", "do=go&machine=laptop");
    assert_eq!(status_of(&response), 303, "{response}");
    assert_eq!(starts(&queued(&world, &hub)), 1);

    // The laptop's `workflow go` started the -2 agent and recorded it as the
    // runner, and its next poll lists it.
    let path = world.dir.join("projects.json");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("omega-o2-sync\"", "omega-o2-sync-2\"")).unwrap();
    hub.poll_agent(
        r#"{"machine":"laptop","checkouts":["omega"],"agents":[
          {"id":"omega-o2-sync","state":"stopped","ended":0,"created":2},
          {"id":"omega-o2-sync-2","state":"starting","ended":0,"created":3}]}"#,
    );
    wait_for("the new orchestrator", Duration::from_secs(10), || {
        body_of(&hub.get("/p/omega")).contains(
            "<span class=\"pill\">running</span> <span class=\"meta\">omega-o2-sync-2 · starting</span>",
        )
    });
}

/// omega's row says it is paused, as mem would after `workflow pause`.
fn set_paused(world: &World) {
    let path = world.dir.join("projects.json");
    let mut doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    doc["projects"][0]["paused"] = json!("2026-10-15T09:00:00Z by saiful");
    std::fs::write(&path, doc.to_string()).unwrap();
}

/// A fake `workflow` that logs its argv and, like the real one, pauses or
/// unpauses omega's row.
fn fake_workflow(world: &World) -> PathBuf {
    let log = world.dir.join("workflow.log");
    let projects = world.dir.join("projects.json");
    fixture_bin(
        &world.bin,
        "workflow",
        &format!(
            "printf '%s\\n' \"$*\" >> '{log}'\n\
             case \"$1\" in\n\
             pause) sed -i 's/\"name\":\"omega\"/\"name\":\"omega\",\"paused\":\"2026-10-15T09:00:00Z by hub\"/' '{projects}' ;;\n\
             unpause) sed -i 's/,\\{{0,1\\}}\"paused\":\"[^\"]*\"//' '{projects}' ;;\n\
             esac",
            log = log.display(),
            projects = projects.display(),
        ),
    );
    log
}

const PAUSE: &str = "<input type=\"hidden\" name=\"do\" value=\"pause\">";
const UNPAUSE: &str = "<input type=\"hidden\" name=\"do\" value=\"unpause\">";

#[test]
fn a_running_project_can_be_paused_and_then_reads_pausing() {
    let world = world("live-pause", "", Some(LIVE));
    let workflow = fake_workflow(&world);
    let hub = hub(&world);

    let body = body_of(&hub.get("/p/omega")).to_string();
    assert!(body.contains(PAUSE), "{body}");
    assert!(!body.contains(UNPAUSE), "{body}");

    let response = hub.post_form("/p/omega/control", "do=pause");
    assert_eq!(status_of(&response), 303, "{response}");
    assert_eq!(common::header_of(&response, "location"), Some("/p/omega"));
    assert_eq!(lines(&workflow), ["pause omega --by hub"]);

    let body = body_of(&hub.get("/p/omega")).to_string();
    assert!(
        body.contains("<span class=\"pill wait\">pausing</span>"),
        "{body}"
    );
    assert!(body.contains(UNPAUSE), "{body}");
    assert!(!body.contains(PAUSE), "{body}");
    assert_eq!(starts(&queued(&world, &hub)), 0);
}

#[test]
fn a_paused_project_with_no_run_offers_unpause_not_start() {
    let world = world("live-paused", "o2-t1 landed at 1a2b3c.", Some(DEAD));
    set_paused(&world);
    let workflow = fake_workflow(&world);
    let hub = hub(&world);

    let body = body_of(&hub.get("/p/omega")).to_string();
    assert!(
        body.contains("<span class=\"pill wait\">paused</span>"),
        "{body}"
    );
    assert!(
        body.contains("<button type=\"submit\">Unpause</button>"),
        "{body}"
    );
    assert!(!body.contains("value=\"go\""), "{body}");
    assert!(!body.contains("stalled"), "{body}");

    let response = hub.post_form("/p/omega/control", "do=unpause");
    assert_eq!(status_of(&response), 303, "{response}");
    assert_eq!(lines(&workflow), ["unpause omega"]);
    assert_eq!(starts(&queued(&world, &hub)), 0, "unpause starts nothing");

    let body = body_of(&hub.get("/p/omega")).to_string();
    assert!(!body.contains(">paused<"), "{body}");
    assert!(
        body.contains("<button type=\"submit\">Resume</button>"),
        "{body}"
    );
    assert!(body.contains(PAUSE), "{body}");
}

#[test]
fn a_paused_project_parked_on_a_question_can_still_resume() {
    let world = world(
        "live-paused-parked",
        "parked: waiting on a question",
        Some(DEAD),
    );
    set_paused(&world);
    let hub = hub(&world);

    let body = body_of(&hub.get("/p/omega")).to_string();
    assert!(
        body.contains("<span class=\"pill wait\">paused</span>"),
        "{body}"
    );
    assert!(
        body.contains("<button type=\"submit\">Resume</button>"),
        "{body}"
    );
    assert!(body.contains(UNPAUSE), "{body}");
}

#[test]
fn a_go_refused_for_a_pause_says_so() {
    let world = world("live-go-paused", "o2-t1 landed at 1a2b3c.", Some(DEAD));
    set_paused(&world);
    let hub = hub(&world);
    let response = hub.post_form("/p/omega/control", "do=go&machine=laptop");
    assert_eq!(status_of(&response), 409, "{response}");
    assert!(
        body_of(&response).contains("omega is paused."),
        "{response}"
    );
    assert_eq!(queued(&world, &hub), json!([]));
}
