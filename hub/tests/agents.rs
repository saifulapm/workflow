//! `POST /api/agent`: the device agents' poll. A paired agent reports its
//! machine and amx agents and takes the commands queued for that machine;
//! every hub write queues one `sync` for each machine that has polled.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{
    DEVICE, Hub, TempDir, body_of, fixture_mem, header_of, mem_in, real_mem, recording_mem,
    seed_project, status_of, wait_for,
};
use serde_json::{Value, json};

const REPORT: &str = r#"{"machine":"mini","agents":[{"id":"gamma-m1","state":"working","ended":0,"created":1791400000}],"checkouts":["gamma","workflow"]}"#;

fn report(machine: &str) -> String {
    REPORT.replace("\"mini\"", &format!("\"{machine}\""))
}

/// The poll an agent sends: no cookie, no Origin, and the token as a bearer
/// when it has one.
fn poll_as(hub: &Hub, authorization: Option<&str>, body: &str) -> String {
    let auth = authorization
        .map(|a| format!("Authorization: {a}\r\n"))
        .unwrap_or_default();
    hub.raw(&format!(
        "POST /api/agent HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n{auth}\r\n{body}",
        hub.port,
        body.len()
    ))
}

/// A paired poll for `machine`, and the commands it was handed.
fn poll(hub: &Hub, machine: &str) -> Value {
    let response = poll_as(hub, Some(&format!("Bearer {DEVICE}")), &report(machine));
    assert_eq!(status_of(&response), 200, "{response}");
    serde_json::from_str(body_of(&response)).unwrap()
}

/// `GET /api/agents` as a paired agent: each machine that has polled.
fn machines(hub: &Hub) -> Value {
    let response = hub.raw(&format!(
        "GET /api/agents HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\
         Authorization: Bearer {DEVICE}\r\n\r\n",
        hub.port
    ));
    assert_eq!(status_of(&response), 200, "{response}");
    serde_json::from_str(body_of(&response)).unwrap()
}

fn online(hub: &Hub, machine: &str) -> Option<bool> {
    machines(hub)["machines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == machine)
        .map(|m| m["online"].as_bool().unwrap())
}

/// A home whose state directory holds `agents` as the queue file, and a
/// `mem` that knows no projects.
fn home_with_queue(dir: &TempDir, agents: &str) -> (PathBuf, PathBuf) {
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("state/hub")).unwrap();
    std::fs::write(home.join("state/hub/agents.json"), agents).unwrap();
    let bin = dir.join("bin");
    fixture_mem(&bin, "echo '{\"projects\":[]}'");
    (home, bin)
}

#[test]
fn a_poll_without_a_paired_token_records_and_drains_nothing() {
    let dir = TempDir::new("agents-auth");
    let (home, bin) = home_with_queue(
        &dir,
        r#"{"known":[],"queues":{"mini":[{"kind":"start","project":"gamma"}]}}"#,
    );
    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);

    for auth in [
        None,
        Some("Bearer WRONGWRONGWRONGWRONGWRONGW"),
        Some("Bearer "),
        Some(DEVICE),
        Some(&format!("Basic {DEVICE}")),
    ] {
        let response = poll_as(&hub, auth, REPORT);
        assert_eq!(status_of(&response), 401, "{auth:?}: {response}");
    }
    let unpaired = hub.raw(&format!(
        "GET /api/agents HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        hub.port
    ));
    assert_eq!(status_of(&unpaired), 401, "{unpaired}");

    // A good token with a body that is not a report is refused too.
    let bad = poll_as(&hub, Some(&format!("Bearer {DEVICE}")), "{\"machine\":");
    assert_eq!(status_of(&bad), 400, "{bad}");

    assert_eq!(machines(&hub), json!({"machines": []}));
    assert_eq!(
        poll(&hub, "mini"),
        json!({"commands": [{"kind": "start", "project": "gamma"}]})
    );
    assert_eq!(poll(&hub, "mini"), json!({"commands": []}));
}

#[test]
fn a_machine_reads_online_after_a_poll_and_offline_once_the_window_passes() {
    let dir = TempDir::new("agents-online");
    let (home, bin) = home_with_queue(&dir, r#"{"known":[],"queues":{}}"#);
    let hub = Hub::spawn_env(
        &home,
        &[&bin],
        &["--port", "0"],
        &[("HUB_AGENT_WINDOW_MS", "1500")],
    );
    assert_eq!(online(&hub, "mini"), None);
    poll(&hub, "mini");
    assert_eq!(online(&hub, "mini"), Some(true));
    let listed = machines(&hub);
    let last = listed["machines"][0]["last_poll"].as_str().unwrap();
    assert!(last.parse::<jiff::Timestamp>().is_ok(), "{listed}");
    wait_for("mini to read offline", Duration::from_secs(5), || {
        online(&hub, "mini") == Some(false)
    });
}

/// A project with two questions waiting, and their ids.
fn two_questions(mem: &Path, home: &Path) -> Vec<String> {
    seed_project(mem, home, "gamma", "seeded");
    for question in ["Which hour?", "Which day?"] {
        let out = mem_in(mem, home, &home.join("gamma"), &["ask", question]);
        assert!(out.status.success(), "{out:?}");
    }
    let out = mem_in(
        mem,
        home,
        home,
        &["questions", "--pending", "--all-projects", "--json"],
    );
    let document: Value = serde_json::from_slice(&out.stdout).unwrap();
    document["questions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| q["id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_hub_write_queues_one_sync_for_every_machine_that_has_polled() {
    let dir = TempDir::new("agents-sync");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let (bin, _log) = recording_mem(dir.path(), &home);
    let ids = two_questions(&real_mem().unwrap(), &home);
    assert_eq!(ids.len(), 2);
    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);

    assert_eq!(poll(&hub, "mini"), json!({"commands": []}));
    assert_eq!(poll(&hub, "nuc"), json!({"commands": []}));
    for id in &ids {
        let response = hub.post_form("/answer", &format!("id={id}&text=seven"));
        assert_eq!(status_of(&response), 303, "{response}");
        assert!(
            header_of(&response, "location").is_some_and(|l| l.contains("answered")),
            "{response}"
        );
    }
    for machine in ["mini", "nuc"] {
        assert_eq!(
            poll(&hub, machine),
            json!({"commands": [{"kind": "sync"}]}),
            "{machine}"
        );
        assert_eq!(poll(&hub, machine), json!({"commands": []}), "{machine}");
    }
}

#[test]
fn a_queued_resume_comes_back_byte_for_byte_after_a_restart() {
    let dir = TempDir::new("agents-restart");
    let (home, bin) = home_with_queue(&dir, r#"{"known":[],"queues":{}}"#);
    let message = "say \"hi\"\\there\nand again";
    {
        let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);
        poll(&hub, "mini");
    }
    // The queue a running hub would add, written through the hub's own code.
    let agents =
        hub::agents::Agents::open(home.join("state/hub/agents.json"), Duration::from_secs(30));
    agents.queue(
        "mini",
        hub::agents::Command::Resume {
            project: "gamma".to_string(),
            id: "gamma-m1".to_string(),
            message: message.to_string(),
        },
    );
    drop(agents);

    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);
    let commands = poll(&hub, "mini");
    assert_eq!(commands["commands"][0]["message"].as_str(), Some(message));
    assert_eq!(
        commands,
        json!({"commands": [{"kind": "resume", "project": "gamma", "id": "gamma-m1", "message": message}]})
    );
}
