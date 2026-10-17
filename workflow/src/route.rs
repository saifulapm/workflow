//! `workflow route`: where questions ring, the phone or a device, as this
//! machine's local hub holds it. The hub sends a change on to every other
//! local hub and writes the file qshell reads.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

use crate::exit;

fn say(msg: impl AsRef<str>) {
    eprintln!("workflow route: {}", msg.as_ref());
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

/// One request to the local hub on its socket: its status and body.
fn ask(socket: &PathBuf, method: &str, body: &str) -> std::io::Result<(String, String)> {
    let mut stream = UnixStream::connect(socket)?;
    let timeout = Some(Duration::from_secs(5));
    stream.set_read_timeout(timeout)?;
    stream.set_write_timeout(timeout)?;
    let request = format!(
        "{method} /route HTTP/1.1\r\nHost: local\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes())?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    let (head, answer) = reply.split_once("\r\n\r\n").unwrap_or((&reply, ""));
    let status = head.split_whitespace().nth(1).unwrap_or("").to_string();
    Ok((status, answer.to_string()))
}

/// `workflow route [phone|device]`.
pub fn cmd_route(to: Option<&str>) -> i32 {
    let Some(socket) = local_socket() else {
        say("cannot find this machine's state directory; set HOME");
        return exit::FAILED;
    };
    let reply = match to {
        None => ask(&socket, "GET", ""),
        Some(kind) => ask(&socket, "POST", &json!({ "route": kind }).to_string()),
    };
    let (status, answer) = match reply {
        Ok(r) => r,
        Err(err) => {
            say(format!("the local hub did not answer on {}: {err}", socket.display()));
            return exit::FAILED;
        }
    };
    if status != "200" {
        say(format!("the local hub refused: HTTP {status} {}", answer.trim()));
        return exit::FAILED;
    }
    let Some(route) = serde_json::from_str::<Value>(&answer)
        .ok()
        .filter(|d| d["route"].is_string())
    else {
        say("the local hub's answer is not a route");
        return exit::FAILED;
    };
    match route["machine"].as_str() {
        Some(machine) if route["route"] == "device" => println!("route: device ({machine})"),
        _ => println!("route: {}", route["route"].as_str().unwrap_or("phone")),
    }
    exit::OK
}
