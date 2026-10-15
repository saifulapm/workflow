//! `workflow agent`: the device daemon. It pairs once with the hub, then
//! every 5 s tells the hub this machine's name, its amx agents and its
//! checkouts, and runs the commands the hub hands back, one at a time.
//!
//! HTTP goes through curl, as the hub's doorbell does, so no HTTP crate is
//! needed.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{exit, go, machine, memcli, paths};

/// What the hub hands back for this machine to run.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Job {
    Sync,
    Start {
        project: String,
    },
    Resume {
        project: String,
        id: String,
        message: String,
    },
}

fn say(msg: impl AsRef<str>) {
    eprintln!("workflow agent: {}", msg.as_ref());
}

fn bin(var: &str, default: &str) -> String {
    match std::env::var(var) {
        Ok(v) if !v.is_empty() => v,
        _ => default.to_string(),
    }
}

/// `$XDG_STATE_HOME/workflow`, else `~/.local/state/workflow`.
fn state_dir() -> Option<PathBuf> {
    let base = match std::env::var("XDG_STATE_HOME") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => paths::home()?.join(".local/state"),
    };
    Some(base.join("workflow"))
}

/// `scheme://host` of a link: everything before its third `/`.
fn hub_of(link: &str) -> Option<&str> {
    let (scheme, rest) = link.split_once("://")?;
    let host = rest.split('/').next()?;
    (!scheme.is_empty() && !host.is_empty()).then(|| &link[..scheme.len() + 3 + host.len()])
}

/// The `hub_device` cookie in a block of response headers.
fn device_cookie(headers: &str) -> Option<String> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if !name.trim().eq_ignore_ascii_case("set-cookie") {
            return None;
        }
        let token = value.trim().strip_prefix("hub_device=")?;
        let token = token.split(';').next()?.trim();
        (!token.is_empty()).then(|| token.to_string())
    })
}

pub fn cmd_pair(link: &str) -> i32 {
    let Some(hub) = hub_of(link) else {
        say(format!("not a pairing link: {link}"));
        return exit::USAGE;
    };
    let Some(dir) = state_dir() else {
        say("cannot tell where to keep the token: HOME is not set");
        return exit::FAILED;
    };
    let out = Command::new(bin("WORKFLOW_CURL", "curl"))
        .args([
            "-sS",
            "-m",
            "10",
            "-X",
            "POST",
            "-D",
            "-",
            "-o",
            "/dev/null",
        ])
        .arg("-H")
        .arg(format!("Origin: {hub}"))
        .arg(link)
        .stdin(Stdio::null())
        .output();
    let headers = match out {
        Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
        Err(err) => {
            say(format!("cannot run curl: {err}"));
            return exit::FAILED;
        }
    };
    let Some(token) = device_cookie(&headers) else {
        let status = headers.lines().next().unwrap_or("no answer").trim();
        say(format!(
            "pairing failed: {link} set no hub_device cookie ({status})"
        ));
        return exit::FAILED;
    };
    if let Err(err) = save(&dir, &token, hub) {
        say(format!("cannot save the token in {}: {err}", dir.display()));
        return exit::FAILED;
    }
    println!("paired with {hub}");
    exit::OK
}

fn save(dir: &PathBuf, token: &str, hub: &str) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = dir.join("agent-token");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?;
    // A token file left by an older pairing keeps its old mode otherwise.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    file.write_all(token.as_bytes())?;
    fs::write(dir.join("agent-hub"), hub)
}

/// This machine's amx agents, each trimmed to the four keys the hub reads.
fn amx_ls() -> Result<Vec<Value>, String> {
    let out = Command::new(go::amx_bin())
        .args(["ls", "--json"])
        .stderr(Stdio::piped())
        .output()
        .map_err(|err| format!("cannot run {}: {err}", go::amx_bin()))?;
    if !out.status.success() {
        return Err(format!(
            "amx ls failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&text).map_err(|err| format!("amx ls --json is not a list: {err}"))
}

fn report(name: &str) -> Value {
    let agents: Vec<Value> = amx_ls()
        .unwrap_or_else(|err| {
            say(err);
            Vec::new()
        })
        .iter()
        .map(|a| {
            let key = |k: &str| a.get(k).cloned().unwrap_or(Value::Null);
            json!({"id": key("id"), "state": key("state"), "ended": key("ended"), "created": key("created")})
        })
        .collect();
    let checkouts: Vec<String> = memcli::projects()
        .unwrap_or_else(|| {
            say("mem cannot list its projects");
            Vec::new()
        })
        .into_iter()
        .filter(|p| !p.checkouts.is_empty())
        .map(|p| p.name)
        .collect();
    json!({"machine": name, "agents": agents, "checkouts": checkouts})
}

/// One poll: the report out, the commands back. `None` when the hub did not
/// answer 200; the reason is logged.
fn poll(hub: &str, token: &str, name: &str) -> Option<Vec<Job>> {
    let body = report(name).to_string();
    let child = Command::new(bin("WORKFLOW_CURL", "curl"))
        .args(["-sS", "-m", "10", "-X", "POST", "-w", "\n%{http_code}"])
        .arg("-H")
        .arg(format!("Authorization: Bearer {token}"))
        .args(["-H", "Content-Type: application/json"])
        .args(["--data-binary", "@-"])
        .arg(format!("{hub}/api/agent"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(err) => {
            say(format!("cannot run curl: {err}"));
            return None;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(body.as_bytes());
    }
    let out = child.wait_with_output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (answer, status) = text.rsplit_once('\n').unwrap_or(("", &text));
    match status.trim() {
        "200" => {}
        "401" => {
            say("the hub does not know this token; run `workflow agent pair <link>`");
            return None;
        }
        _ => {
            let err = String::from_utf8_lossy(&out.stderr);
            say(format!(
                "the hub did not answer: HTTP {} {}",
                status.trim(),
                err.trim()
            ));
            return None;
        }
    }
    let doc: Value = match serde_json::from_str(answer) {
        Ok(v) => v,
        Err(err) => {
            say(format!("the hub's answer is not JSON: {err}"));
            return None;
        }
    };
    let commands = doc.get("commands").and_then(Value::as_array).cloned();
    // A kind this agent does not know is skipped, not a reason to drop the rest.
    Some(
        commands
            .unwrap_or_default()
            .into_iter()
            .filter_map(|c| match serde_json::from_value::<Job>(c.clone()) {
                Ok(job) => Some(job),
                Err(err) => {
                    say(format!("skipping {c}: {err}"));
                    None
                }
            })
            .collect(),
    )
}

fn succeeds(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// `workflow go` in a scope of its own, so stopping the agent's unit never
/// stops the orchestrator it started.
fn start(project: &str) -> bool {
    succeeds(
        &bin("WORKFLOW_SYSTEMD_RUN", "systemd-run"),
        &["--user", "--scope", "--quiet", "workflow", "go", project],
    )
}

/// Run one job and print one line for it. `false` when the job must be tried
/// again after the next poll.
fn run(job: &Job) -> bool {
    match job {
        Job::Sync => {
            let ok = succeeds(&memcli::bin(), &["sync"]);
            println!("sync: {}", if ok { "done" } else { "failed" });
        }
        Job::Start { project } => {
            let ok = start(project);
            println!("start {project}: {}", if ok { "started" } else { "failed" });
        }
        Job::Resume {
            project,
            id,
            message,
        } => {
            // The answer has to reach this machine's store before the session
            // reads it.
            if !succeeds(&memcli::bin(), &["sync"]) {
                println!("resume {id}: mem sync failed, trying again after the next poll");
                return false;
            }
            let agents = match amx_ls() {
                Ok(a) => a,
                Err(err) => {
                    println!("resume {id}: {err}, trying again after the next poll");
                    return false;
                }
            };
            let row = agents
                .iter()
                .find(|a| a.get("id").and_then(Value::as_str) == Some(id));
            if let Some(row) = row {
                let ended = row.get("ended").and_then(Value::as_i64).unwrap_or(0);
                let state = row.get("state").and_then(Value::as_str).unwrap_or("");
                if ended == 0 && state != "stopped" && state != "failed" {
                    println!("resume {id}: live, it reads its answers itself");
                    return true;
                }
                let resumed = Command::new(bin("WORKFLOW_SYSTEMD_RUN", "systemd-run"))
                    .args(["--user", "--scope", "--quiet"])
                    .arg(go::amx_bin())
                    .args(["resume", id])
                    .arg(message)
                    .stdout(Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if resumed {
                    println!("resume {id}: resumed");
                    return true;
                }
            }
            let ok = start(project);
            println!(
                "resume {id}: session gone, start {project}: {}",
                if ok { "started" } else { "failed" }
            );
        }
    }
    true
}

pub fn cmd_run(once: bool) -> i32 {
    let token = state_dir().and_then(|d| fs::read_to_string(d.join("agent-token")).ok());
    let hub = state_dir().and_then(|d| fs::read_to_string(d.join("agent-hub")).ok());
    let (Some(token), Some(hub)) = (token, hub) else {
        say("not paired; run `workflow agent pair <link>`");
        return exit::USAGE;
    };
    let (token, hub) = (token.trim().to_string(), hub.trim().to_string());
    let Some(name) = machine::name() else {
        say("cannot tell this machine's name; set WORKFLOW_MACHINE");
        return exit::FAILED;
    };
    let interval = std::env::var("WORKFLOW_AGENT_INTERVAL_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5000);

    // Jobs run on a worker so a slow one never holds up the polls that keep
    // this machine online on the hub. A resume whose sync failed comes back
    // through `held` and goes out again after the next poll.
    let held: Arc<Mutex<Vec<Job>>> = Arc::default();
    let (tx, rx) = mpsc::channel::<Job>();
    let worker = {
        let held = Arc::clone(&held);
        thread::spawn(move || {
            for job in rx {
                if !run(&job) {
                    held.lock().unwrap().push(job);
                }
            }
        })
    };

    loop {
        let jobs = poll(&hub, &token, &name);
        let polled = jobs.is_some();
        let retries = std::mem::take(&mut *held.lock().unwrap());
        for job in retries.into_iter().chain(jobs.unwrap_or_default()) {
            let _ = tx.send(job);
        }
        if once {
            drop(tx);
            let _ = worker.join();
            return if polled { exit::OK } else { exit::FAILED };
        }
        thread::sleep(Duration::from_millis(interval));
    }
}
