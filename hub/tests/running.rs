//! What runs a project, read only from its runner record and the device
//! agents' reports: home's Running section, the project page's line and its
//! Start or Resume, which queues a command for one machine and never runs
//! anything here. Over a fake `mem`.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use common::{Hub, TempDir, body_of, fixture_bin, header_of, status_of, wait_for};
use serde_json::{Value, json};

struct World {
    dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
}

/// gamma building m4-x with `runner`, its last handoff `handoff`, and delta
/// with no roadmap. The hub's machine has no checkout of either.
fn world(tag: &str, runner: Value, handoff: &str) -> World {
    let dir = TempDir::new(tag);
    let home = dir.join("home");
    let bin = dir.join("bin");
    let projects = json!({"projects": [
        {"name": "gamma", "roadmap_status": "approved", "checkouts": [],
         "milestone": "m4-x", "plan_slug": "m4-x", "milestones_done": 3,
         "milestones_total": 5, "plan_ticked": 1, "plan_total": 4, "runner": runner},
        {"name": "delta", "checkouts": [], "runner": null},
    ]});
    let roadmap = json!({"status": "approved", "text":
        "# roadmap: gamma\n\n- [x] m3-w Done\n- [ ] m4-x Next\n"});
    let handoff = json!({ "body": handoff });
    fixture_bin(
        &bin,
        "mem",
        &format!(
            "p() {{ printf '%s\\n' \"$1\"; }}\n\
             case \"$*\" in\n\
             projects*) p '{projects}' ;;\n\
             *--all-projects*) p '{{\"questions\":[]}}' ;;\n\
             roadmap*) p '{roadmap}' ;;\n\
             plan\\ --list*) p '{{\"plans\":[]}}' ;;\n\
             handoff*) p '{handoff}' ;;\n\
             *) p '{{\"items\":[]}}' ;;\n\
             esac"
        ),
    );
    // `workflow pause` takes, and starts nothing.
    fixture_bin(&bin, "workflow", "exit 0");
    World { dir, home, bin }
}

fn hub(world: &World, env: &[(&str, &str)]) -> Hub {
    let config = world.dir.join("config.toml");
    std::fs::write(
        &config,
        "topic = \"workflow-TESTTESTTESTTESTTESTTESTTE\"\n\
         ntfy_base = \"http://127.0.0.1:9\"\n",
    )
    .unwrap();
    Hub::spawn_env(
        &world.home,
        &[&world.bin],
        &["--port", "0", "--config", config.to_str().unwrap()],
        env,
    )
}

fn mini() -> Value {
    json!({"machine": "mini", "id": "gamma-m4", "since": "2026-10-15T07:00:00Z"})
}

/// One poll from `machine`, listing gamma-m4 in `state` and `checkouts`, and
/// the commands it was handed.
fn poll(hub: &Hub, machine: &str, state: &str, checkouts: &[&str]) -> Value {
    let report = json!({"machine": machine,
        "agents": [{"id": "gamma-m4", "state": state, "ended": 0, "created": 1791400000}],
        "checkouts": checkouts});
    serde_json::from_str(&hub.poll_agent(&report.to_string())).unwrap()
}

fn page(hub: &Hub, path: &str) -> String {
    let response = hub.get(path);
    assert_eq!(status_of(&response), 200, "{response}");
    body_of(&response).to_string()
}

#[test]
fn home_lists_a_live_run_first_and_not_again_under_projects() {
    let world = world("running-home", mini(), "Picking up at m4-t2.");
    let hub = hub(&world, &[("TZ", "Asia/Kolkata")]);
    poll(&hub, "mini", "working", &["gamma"]);

    let body = page(&hub, "/");
    let running = body.find("<h2>Running</h2>").expect(&body);
    let waiting = body.find("<h2>Waiting on you</h2>").expect(&body);
    let projects = body.find("<h2>Projects").expect(&body);
    assert!(running < waiting, "{body}");
    assert!(
        body[running..waiting]
            .contains("<a href=\"/p/gamma\">gamma · mini · m4-x · since 12:30</a>"),
        "{body}"
    );
    assert!(!body[projects..].contains("/p/gamma"), "{body}");
    assert!(body[projects..].contains("/p/delta"), "{body}");
}

#[test]
fn a_runner_whose_machine_went_quiet_reads_offline_then_waiting() {
    let world = world("running-away", mini(), "Picking up at m4-t2.");
    let hub = hub(&world, &[("HUB_AGENT_WINDOW_MS", "300")]);
    poll(&hub, "mini", "stopped", &["gamma"]);
    wait_for("mini to go quiet", Duration::from_secs(5), || {
        page(&hub, "/p/gamma").contains("mini offline")
    });
    let body = page(&hub, "/p/gamma");
    assert!(!body.contains("value=\"go\""), "{body}");
    assert!(!body.contains("stalled"), "{body}");
    assert!(page(&hub, "/").contains("mini offline"));

    // A pause writes mem, which queues a sync for every machine.
    let response = hub.post_form("/p/gamma/control", "do=pause");
    assert_eq!(status_of(&response), 303, "{response}");
    let body = page(&hub, "/p/gamma");
    assert!(body.contains("waiting for mini"), "{body}");
    assert!(!body.contains("mini offline"), "{body}");
    assert!(!body.contains("value=\"go\""), "{body}");
    assert!(page(&hub, "/").contains("waiting for mini"));
}

#[test]
fn resume_queues_one_start_for_the_runners_machine_and_none_elsewhere() {
    let world = world("running-resume", mini(), "Picking up at m4-t2.");
    let hub = hub(&world, &[]);
    poll(&hub, "mini", "stopped", &["gamma"]);
    poll(&hub, "nuc", "stopped", &["gamma"]);

    let body = page(&hub, "/p/gamma");
    assert!(
        body.contains(
            "<input type=\"hidden\" name=\"do\" value=\"go\">\n\
             <input type=\"hidden\" name=\"machine\" value=\"mini\">\n\
             <button type=\"submit\">Resume</button>"
        ),
        "{body}"
    );
    assert!(!body.contains("value=\"nuc\""), "{body}");

    let forged = hub.post_form("/p/gamma/control", "do=go&machine=nuc");
    assert_eq!(status_of(&forged), 409, "{forged}");
    let response = hub.post_form("/p/gamma/control", "do=go&machine=mini");
    assert_eq!(status_of(&response), 303, "{response}");
    assert_eq!(header_of(&response, "location"), Some("/p/gamma"));

    assert_eq!(
        poll(&hub, "nuc", "stopped", &["gamma"]),
        json!({"commands": []})
    );
    assert_eq!(
        poll(&hub, "mini", "stopped", &["gamma"]),
        json!({"commands": [{"kind": "start", "project": "gamma"}]})
    );
}

#[test]
fn with_no_runner_only_an_online_machine_with_a_checkout_is_offered() {
    let world = world("running-none", Value::Null, "");
    let hub = hub(&world, &[]);
    poll(&hub, "nuc", "stopped", &["gamma"]);
    poll(&hub, "mini", "stopped", &[]);

    let body = page(&hub, "/p/gamma");
    assert!(
        body.contains(
            "<input type=\"hidden\" name=\"machine\" value=\"nuc\">\n\
             <button type=\"submit\">Start on nuc</button>"
        ),
        "{body}"
    );
    assert!(!body.contains("on mini"), "{body}");
    assert!(!body.contains("stalled"), "{body}");

    let refused = hub.post_form("/p/gamma/control", "do=go&machine=mini");
    assert_eq!(status_of(&refused), 409, "{refused}");
    assert_eq!(poll(&hub, "mini", "stopped", &[]), json!({"commands": []}));
    let response = hub.post_form("/p/gamma/control", "do=go&machine=nuc");
    assert_eq!(status_of(&response), 303, "{response}");
    assert_eq!(
        poll(&hub, "nuc", "stopped", &["gamma"]),
        json!({"commands": [{"kind": "start", "project": "gamma"}]})
    );
}
