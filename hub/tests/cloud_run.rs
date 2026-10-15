//! A cloud run on home: no device agent reports a claude.ai session, so home
//! lists one under Running from its runner record and handoff alone. Over a
//! fake `mem`.

mod common;

use std::path::PathBuf;

use common::{Hub, TempDir, body_of, fixture_bin, status_of};
use serde_json::{Value, json};

struct World {
    dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
}

/// gamma building m2, run in the cloud on m2 since `since`, its latest
/// handoff `handoff` written at `handoff_at`.
fn world(tag: &str, since: &str, handoff_at: Value, handoff: &str) -> World {
    let dir = TempDir::new(tag);
    let home = dir.join("home");
    let bin = dir.join("bin");
    let projects = json!({"projects": [
        {"name": "gamma", "roadmap_status": "approved", "checkouts": [],
         "milestone": "m2", "milestones_done": 1, "milestones_total": 3,
         "handoff_at": handoff_at,
         "runner": {"machine": "cloud", "id": "session_01Abc", "since": since,
                    "milestone": "m2"}},
    ]});
    let handoff = json!({ "body": handoff });
    fixture_bin(
        &bin,
        "mem",
        &format!(
            "p() {{ printf '%s\\n' \"$1\"; }}\n\
             case \"$*\" in\n\
             projects*) p '{projects}' ;;\n\
             *--all-projects*) p '{{\"questions\":[]}}' ;;\n\
             handoff*) p '{handoff}' ;;\n\
             *) p '{{\"items\":[]}}' ;;\n\
             esac"
        ),
    );
    World { dir, home, bin }
}

fn hub(world: &World) -> Hub {
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
        &[("TZ", "UTC")],
    )
}

/// Home's Running section, and everything below it.
fn home(world: &World) -> (String, String) {
    let hub = hub(world);
    let response = hub.get("/");
    assert_eq!(status_of(&response), 200, "{response}");
    let body = body_of(&response).to_string();
    let running = body.find("<h2>Running</h2>").expect(&body);
    let waiting = body.find("<h2>Waiting on you</h2>").expect(&body);
    (
        body[running..waiting].to_string(),
        body[waiting..].to_string(),
    )
}

fn ago(minutes: i64) -> jiff::Timestamp {
    jiff::Timestamp::now() - jiff::SignedDuration::from_mins(minutes)
}

/// `HH:MM` in UTC, the zone the hub runs in here.
fn clock(at: jiff::Timestamp) -> String {
    at.strftime("%H:%M").to_string()
}

#[test]
fn a_cloud_run_begun_ten_minutes_ago_is_listed_under_running() {
    let since = ago(10);
    let world = world("cloud-live", &since.to_string(), Value::Null, "");
    let (running, rest) = home(&world);
    assert!(
        running.contains(&format!(
            "<a href=\"/p/gamma\">gamma · cloud · m2 · since {}</a>",
            clock(since)
        )),
        "{running}"
    );
    assert!(!rest.contains("/p/gamma"), "listed twice: {rest}");
}

#[test]
fn a_cloud_run_with_no_handoff_for_eight_hours_has_lapsed() {
    let world = world("cloud-lapsed", &ago(8 * 60).to_string(), Value::Null, "");
    let (running, rest) = home(&world);
    assert!(!running.contains("gamma"), "{running}");
    assert!(!rest.contains("cloud offline"), "{rest}");
}

#[test]
fn a_parked_cloud_run_is_listed_as_parked() {
    let since = ago(30);
    let world = world(
        "cloud-parked",
        &since.to_string(),
        json!(ago(5).to_string()),
        "\nparked: t2 is local-only",
    );
    let (running, _) = home(&world);
    assert!(
        running.contains(&format!(
            "<a href=\"/p/gamma\">gamma · cloud · m2 · since {} · parked</a>",
            clock(since)
        )),
        "{running}"
    );
}

#[test]
fn a_cloud_run_that_handed_over_without_parking_has_ended() {
    let world = world(
        "cloud-ended",
        &ago(30).to_string(),
        json!(ago(5).to_string()),
        "m2-t3 done, picking up t4",
    );
    let (running, rest) = home(&world);
    assert!(!running.contains("gamma"), "{running}");
    assert!(rest.contains("/p/gamma"), "{rest}");
    assert!(!rest.contains("cloud offline"), "{rest}");
}
