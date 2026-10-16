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
    // Its own temp name: the agent's poll and a panel's `workflow route` can
    // write at the same moment.
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    if let Err(err) = fs::write(&tmp, doc.to_string()).and_then(|()| fs::rename(&tmp, &path)) {
        say(format!("cannot write {}: {err}", path.display()));
    }
}

/// This machine's local hub's socket: `HUB_LOCAL_SOCKET`, else `hub/local.sock`
/// under the state directory, the hub's own rule.
fn local_socket() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("HUB_LOCAL_SOCKET").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::paths::home().map(|h| h.join(".local/state")))?;
    Some(state.join("hub/local.sock"))
}

/// Tells this machine's local hub the route, which it sends on to every
/// other local hub: questions ring by the local hubs' route. Device names no
/// machine, so the hub takes it under its own tailnet name. Best effort: a
/// machine with no local hub still has the hub's.
fn tell_local_hub(kind: &str) {
    use std::io::{Read, Write};
    let Some(socket) = local_socket() else { return };
    let Ok(mut stream) = std::os::unix::net::UnixStream::connect(&socket) else {
        return;
    };
    let timeout = Some(std::time::Duration::from_secs(5));
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    let body = json!({ "route": kind }).to_string();
    let request = format!(
        "POST /route HTTP/1.1\r\nHost: local\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut reply = String::new();
    let sent = stream
        .write_all(request.as_bytes())
        .and_then(|()| stream.read_to_string(&mut reply));
    let ok = reply.split_whitespace().nth(1) == Some("200");
    if sent.is_err() || !ok {
        say(format!(
            "the local hub did not take the route: {}",
            reply.lines().next().unwrap_or("no answer")
        ));
    }
}

/// `workflow route [phone|device|dulali]`.
pub fn cmd_route(to: Option<&str>) -> i32 {
    if let Some(kind @ ("phone" | "device")) = to {
        tell_local_hub(kind);
    }
    let Some((token, hub)) = pairing() else {
        say("not paired; run `workflow agent pair <link>`");
        return exit::USAGE;
    };
    let body = match to {
        None => None,
        Some(kind @ ("phone" | "dulali")) => Some(json!({"route": kind}).to_string()),
        Some("device") => {
            let Some(name) = machine::name() else {
                say("cannot tell this machine's name; set WORKFLOW_MACHINE");
                return exit::FAILED;
            };
            Some(json!({"route": "device", "machine": name}).to_string())
        }
        Some(other) => {
            say(format!("route to phone, device or dulali, not {other}"));
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
