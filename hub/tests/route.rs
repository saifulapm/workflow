//! The one global route, Phone or Device with its machine: `GET` and `POST
//! /api/route` for a paired agent, the same object in the agent's poll
//! answer, and the `Route:` line with its two buttons in home's header. Over
//! a fake `mem` that prints whatever pending questions the test staged.

mod common;

use std::path::PathBuf;

use common::{DEVICE, Hub, TempDir, body_of, fixture_bin, header_of, status_of};
use serde_json::{Value, json};

const REPORT: &str = r#"{"machine":"mac-mini-m2","agents":[],"checkouts":[]}"#;

struct World {
    _dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
}

/// A home with no projects and `waiting` person questions pending.
fn world(tag: &str, waiting: usize) -> World {
    let dir = TempDir::new(tag);
    let home = dir.join("home");
    let bin = dir.join("bin");
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let questions: Vec<Value> = (0..waiting)
        .map(|n| {
            json!({"id": format!("01M0BF1F8BY8FXGZS4Q{n}AAAAAA"), "short_id": format!("S4Q{n}AAAAA"),
                "project": "gamma", "body": format!("Which hour {n}?")})
        })
        .collect();
    std::fs::write(
        data.join("pending.json"),
        json!({ "questions": questions }).to_string(),
    )
    .unwrap();
    fixture_bin(
        &bin,
        "mem",
        &format!(
            "case \"$*\" in\n\
             projects*) echo '{{\"projects\":[]}}' ;;\n\
             questions\\ --pending*) cat '{data}/pending.json' ;;\n\
             *) echo '{{\"items\":[]}}' ;;\n\
             esac",
            data = data.display()
        ),
    );
    World {
        _dir: dir,
        home,
        bin,
    }
}

fn spawn(world: &World) -> Hub {
    Hub::spawn(&world.home, &[&world.bin], &["--port", "0"])
}

/// A request to `/api/route` the way an agent sends it: no cookie, no
/// Origin, the paired token as a bearer unless `token` is false.
fn api(hub: &Hub, method: &str, body: &str, token: bool) -> String {
    let auth = if token {
        format!("Authorization: Bearer {DEVICE}\r\n")
    } else {
        String::new()
    };
    hub.raw(&format!(
        "{method} /api/route HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n{auth}\r\n{body}",
        hub.port,
        body.len()
    ))
}

fn route(hub: &Hub) -> Value {
    let response = api(hub, "GET", "", true);
    assert_eq!(status_of(&response), 200, "{response}");
    serde_json::from_str(body_of(&response)).unwrap()
}

fn set(hub: &Hub, body: &str) -> String {
    api(hub, "POST", body, true)
}

fn home(hub: &Hub) -> String {
    let response = hub.get("/");
    assert_eq!(status_of(&response), 200, "{response}");
    body_of(&response).to_string()
}

fn stamp(hub: &Hub) -> String {
    let doc: Value = serde_json::from_str(body_of(&hub.get("/api/stamp"))).unwrap();
    doc["stamp"].as_str().unwrap().to_string()
}

/// Home's form, from the paired device, and where it sent the browser.
fn press(hub: &Hub, choice: &str) -> String {
    let response = hub.post_form("/route", &format!("route={choice}"));
    assert_eq!(status_of(&response), 303, "{response}");
    header_of(&response, "location").unwrap().to_string()
}

const DEVICE_MINI: &str = "{\"route\":\"device\",\"machine\":\"mac-mini-m2\"}";

#[test]
fn a_fresh_hub_routes_to_the_phone_and_counts_what_waits() {
    let fresh = world("route-fresh", 0);
    let hub = spawn(&fresh);
    assert_eq!(
        route(&hub),
        json!({"route": "phone", "machine": null, "waiting": 0})
    );
    assert!(home(&hub).contains("Route: Phone"));

    let asked = world("route-waiting", 2);
    let hub = spawn(&asked);
    assert_eq!(
        route(&hub),
        json!({"route": "phone", "machine": null, "waiting": 2})
    );
}

#[test]
fn device_set_by_an_agent_shows_on_home_and_in_the_next_poll() {
    let world = world("route-device", 0);
    let hub = spawn(&world);
    let before = stamp(&hub);

    let response = set(&hub, DEVICE_MINI);
    assert_eq!(status_of(&response), 200, "{response}");
    let device = json!({"route": "device", "machine": "mac-mini-m2", "waiting": 0});
    assert_eq!(
        serde_json::from_str::<Value>(body_of(&response)).unwrap(),
        device
    );

    let page = home(&hub);
    assert!(page.contains("Route: Device (mac-mini-m2)"), "{page}");
    let poll: Value = serde_json::from_str(&hub.poll_agent(REPORT)).unwrap();
    assert_eq!(poll, json!({"commands": [], "route": device}));
    assert_ne!(stamp(&hub), before, "the stamp moves with the route");
}

#[test]
fn home_buttons_switch_the_route_and_device_keeps_its_machine() {
    let world = world("route-form", 0);
    let hub = spawn(&world);
    assert_eq!(status_of(&set(&hub, DEVICE_MINI)), 200);

    assert_eq!(press(&hub, "phone"), "/");
    assert_eq!(
        route(&hub),
        json!({"route": "phone", "machine": null, "waiting": 0})
    );
    assert!(home(&hub).contains("Route: Phone"));

    assert_eq!(press(&hub, "device"), "/");
    assert_eq!(
        route(&hub),
        json!({"route": "device", "machine": "mac-mini-m2", "waiting": 0})
    );
}

#[test]
fn device_with_no_machine_ever_is_refused() {
    let world = world("route-nomachine", 0);
    let hub = spawn(&world);

    let location = press(&hub, "device");
    let page = body_of(&hub.get(&location)).to_string();
    assert!(
        page.contains(
            "No machine has taken Device yet. Take Device from a machine's qshell panel first."
        ),
        "{page}"
    );
    assert!(page.contains("Route: Phone"), "{page}");

    let response = set(&hub, "{\"route\":\"device\"}");
    assert_eq!(status_of(&response), 409, "{response}");
    let error: Value = serde_json::from_str(body_of(&response)).unwrap();
    assert!(
        error["error"].as_str().is_some_and(|e| !e.is_empty()),
        "{error}"
    );
    assert_eq!(
        route(&hub),
        json!({"route": "phone", "machine": null, "waiting": 0})
    );

    for bad in ["{\"route\":\"dulali\"}", "{\"route\":", "[]"] {
        let response = set(&hub, bad);
        assert_eq!(status_of(&response), 400, "{bad}: {response}");
    }
}

#[test]
fn the_later_change_wins_and_survives_a_restart() {
    let world = world("route-restart", 0);
    let phone = json!({"route": "phone", "machine": null, "waiting": 0});
    {
        let hub = spawn(&world);
        assert_eq!(status_of(&set(&hub, DEVICE_MINI)), 200);
        press(&hub, "phone");
        assert_eq!(route(&hub), phone);
        let page = home(&hub);
        assert!(page.contains("Route: Phone"), "{page}");
        assert!(!page.contains("Route: Device"), "{page}");
    }
    let hub = spawn(&world);
    assert_eq!(route(&hub), phone);
    // The machine that last took Device outlives the restart too.
    press(&hub, "device");
    assert_eq!(
        route(&hub),
        json!({"route": "device", "machine": "mac-mini-m2", "waiting": 0})
    );
}

#[test]
fn the_route_needs_a_paired_token() {
    let world = world("route-auth", 0);
    let hub = spawn(&world);
    let get = api(&hub, "GET", "", false);
    assert_eq!(status_of(&get), 401, "{get}");
    let post = api(&hub, "POST", DEVICE_MINI, false);
    assert_eq!(status_of(&post), 401, "{post}");
    assert_eq!(
        route(&hub),
        json!({"route": "phone", "machine": null, "waiting": 0})
    );
}

/// Home's buttons write like every other form: an unpaired browser and a
/// page from another origin are refused, and the route stays as it was.
#[test]
fn home_buttons_need_a_paired_device_on_this_origin() {
    let world = world("route-guard", 0);
    let hub = spawn(&world);
    let response = api(
        &hub,
        "POST",
        r#"{"route":"device","machine":"mac-mini-m2"}"#,
        true,
    );
    assert_eq!(status_of(&response), 200, "{response}");

    let unpaired = hub.post_form_unpaired("/route", "route=phone");
    assert_eq!(status_of(&unpaired), 403, "{unpaired}");
    let elsewhere = hub.post_form_with(
        "/route",
        "route=phone",
        &[("Origin", "https://evil.example")],
    );
    assert_eq!(status_of(&elsewhere), 403, "{elsewhere}");
    assert_eq!(route(&hub)["route"], "device");
}

/// The current route's button is the filled one; the other is drawn as the
/// alternative, so the buttons themselves say which is on.
#[test]
fn the_current_routes_button_is_the_filled_one() {
    let world = world("route-pressed", 0);
    let hub = spawn(&world);
    let home = body_of(&hub.get("/")).to_string();
    assert!(
        home.contains("value=\"phone\" aria-pressed=\"true\">")
            && home.contains("value=\"device\" aria-pressed=\"false\" class=\"alt\">"),
        "{home}"
    );
    let response = api(
        &hub,
        "POST",
        r#"{"route":"device","machine":"mac-mini-m2"}"#,
        true,
    );
    assert_eq!(status_of(&response), 200, "{response}");
    let home = body_of(&hub.get("/")).to_string();
    assert!(
        home.contains("value=\"phone\" aria-pressed=\"false\" class=\"alt\">")
            && home.contains("value=\"device\" aria-pressed=\"true\">"),
        "{home}"
    );
}

/// A machine is named by its tailnet label, at most 63 characters; a longer
/// one is refused rather than drawn across home's header.
#[test]
fn a_machine_name_longer_than_a_label_is_refused() {
    let world = world("route-long", 0);
    let hub = spawn(&world);
    let long = "m".repeat(64);
    let response = api(
        &hub,
        "POST",
        &format!(r#"{{"route":"device","machine":"{long}"}}"#),
        true,
    );
    assert_eq!(status_of(&response), 400, "{response}");
    assert_eq!(route(&hub)["route"], "phone");
    let fits = "m".repeat(63);
    let response = api(
        &hub,
        "POST",
        &format!(r#"{{"route":"device","machine":"{fits}"}}"#),
        true,
    );
    assert_eq!(status_of(&response), 200, "{response}");
}

/// A machine name may be one 63-character word, so the route line breaks
/// inside words rather than run off a phone's screen.
#[test]
fn a_long_machine_name_wraps_in_the_header() {
    let world = world("route-wrap", 0);
    let hub = spawn(&world);
    let name = "m".repeat(63);
    let response = api(
        &hub,
        "POST",
        &format!(r#"{{"route":"device","machine":"{name}"}}"#),
        true,
    );
    assert_eq!(status_of(&response), 200, "{response}");
    let page = body_of(&hub.get("/")).to_string();
    assert!(
        page.contains(&format!("<p class=\"route\">Route: Device ({name})</p>")),
        "{page}"
    );
    assert!(page.contains(".route{overflow-wrap:anywhere}"), "{page}");
}
