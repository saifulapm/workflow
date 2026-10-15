//! `workflow route`: where the hub sends questions, to the phone or to a
//! device, and the file qshell reads to show it.

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::agent::{pairing, request, say};
use crate::{exit, machine};

fn route_file() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty())?;
    Some(PathBuf::from(dir).join("workflow-route.json"))
}

/// Write the hub's route object, plus the hub's base URL and this machine's
/// name as the hub knows it, to `$XDG_RUNTIME_DIR/workflow-route.json`. qshell
/// names machines its own way, so it reads `self` to tell whether Device is
/// here. A temp file in the same
/// directory and a rename, so qshell never reads half of it. Skipped without
/// XDG_RUNTIME_DIR.
pub fn write_file(hub: &str, name: Option<&str>, route: &Value) {
    let Some(path) = route_file() else { return };
    let mut doc = route.clone();
    let Some(map) = doc.as_object_mut() else {
        return;
    };
    map.insert("hub".into(), json!(hub));
    if let Some(name) = name {
        map.insert("self".into(), json!(name));
    }
    let tmp = path.with_extension("json.tmp");
    if let Err(err) = fs::write(&tmp, doc.to_string()).and_then(|()| fs::rename(&tmp, &path)) {
        say(format!("cannot write {}: {err}", path.display()));
    }
}

/// `workflow route [phone|device]`.
pub fn cmd_route(to: Option<&str>) -> i32 {
    let Some((token, hub)) = pairing() else {
        say("not paired; run `workflow agent pair <link>`");
        return exit::USAGE;
    };
    let body = match to {
        None => None,
        Some("phone") => Some(json!({"route": "phone"}).to_string()),
        Some("device") => {
            let Some(name) = machine::name() else {
                say("cannot tell this machine's name; set WORKFLOW_MACHINE");
                return exit::FAILED;
            };
            Some(json!({"route": "device", "machine": name}).to_string())
        }
        Some(other) => {
            say(format!("route to phone or device, not {other}"));
            return exit::USAGE;
        }
    };
    let reply = match request(&hub, &token, "route", body.as_deref()) {
        Ok(r) => r,
        Err(err) => {
            say(err);
            return exit::FAILED;
        }
    };
    let doc: Option<Value> = serde_json::from_str(&reply.answer).ok();
    match reply.status.as_str() {
        "200" => {}
        "401" => {
            say("the hub does not know this token; run `workflow agent pair <link>`");
            return exit::FAILED;
        }
        "409" => {
            let error = doc.as_ref().and_then(|d| d["error"].as_str());
            say(error.unwrap_or("the hub refused the route"));
            return exit::FAILED;
        }
        status => {
            say(format!(
                "the hub did not answer: HTTP {status} {}",
                reply.stderr.trim()
            ));
            return exit::FAILED;
        }
    }
    let Some(route) = doc.filter(|d| d["route"].is_string()) else {
        say("the hub's answer is not a route");
        return exit::FAILED;
    };
    write_file(&hub, machine::name().as_deref(), &route);
    match route["machine"].as_str() {
        Some(machine) if route["route"] == "device" => println!("route: device ({machine})"),
        _ => println!("route: {}", route["route"].as_str().unwrap_or("phone")),
    }
    exit::OK
}
