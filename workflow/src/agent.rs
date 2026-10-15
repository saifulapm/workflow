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

use crate::{exit, go, machine, memcli, paths, route};

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
    /// A desktop notification. Never retried.
    Notify {
        title: String,
        body: String,
    },
}

pub(crate) fn say(msg: impl AsRef<str>) {
    eprintln!("workflow agent: {}", msg.as_ref());
}

pub(crate) fn bin(var: &str, default: &str) -> String {
    match std::env::var(var) {
        Ok(v) if !v.is_empty() => v,
        _ => default.to_string(),
    }
}

/// `$XDG_STATE_HOME/workflow`, else `~/.local/state/workflow`.
pub(crate) fn state_dir() -> Option<PathBuf> {
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
    // qshell's presence marker: someone is at this machine, unlocked and not
    // idle, so the hub keeps the phone quiet.
    let watched = std::env::var_os("XDG_RUNTIME_DIR")
        .is_some_and(|dir| PathBuf::from(dir).join("qshell-present").exists());
    json!({"machine": name, "agents": agents, "checkouts": checkouts, "watched": watched})
}

/// What curl got back from the hub.
pub(crate) struct Reply {
    pub status: String,
    pub answer: String,
    pub stderr: String,
}

/// One call to `<hub>/api/<path>`: a GET, or a JSON POST when there is a
/// body. `Err` when curl cannot run.
pub(crate) fn request(
    hub: &str,
    token: &str,
    path: &str,
    body: Option<&str>,
) -> Result<Reply, String> {
    let mut cmd = Command::new(bin("WORKFLOW_CURL", "curl"));
    cmd.args(["-sS", "-m", "10"]);
    if body.is_some() {
        cmd.args(["-X", "POST"]);
    }
    cmd.args(["-w", "\n%{http_code}"])
        .arg("-H")
        .arg(format!("Authorization: Bearer {token}"));
    if body.is_some() {
        cmd.args(["-H", "Content-Type: application/json"])
            .args(["--data-binary", "@-"]);
    }
    let mut child = cmd
        .arg(format!("{hub}/api/{path}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("cannot run curl: {err}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(body.unwrap_or("").as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|err| format!("curl failed: {err}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (answer, status) = text.rsplit_once('\n').unwrap_or(("", &text));
    Ok(Reply {
        status: status.trim().to_string(),
        answer: answer.to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    })
}

/// One poll: the report out, the commands back. `None` when the hub did not
/// answer 200; the reason is logged.
fn poll(hub: &str, token: &str, name: &str) -> Option<Vec<Job>> {
    let body = report(name).to_string();
    let reply = match request(hub, token, "agent", Some(&body)) {
        Ok(r) => r,
        Err(err) => {
            say(err);
            return None;
        }
    };
    match reply.status.as_str() {
        "200" => {}
        "401" => {
            say("the hub does not know this token; run `workflow agent pair <link>`");
            return None;
        }
        status => {
            say(format!(
                "the hub did not answer: HTTP {status} {}",
                reply.stderr.trim()
            ));
            return None;
        }
    }
    let doc: Value = match serde_json::from_str(&reply.answer) {
        Ok(v) => v,
        Err(err) => {
            say(format!("the hub's answer is not JSON: {err}"));
            return None;
        }
    };
    // An older hub sends no route; the file then stays as it is.
    if let Some(route) = doc.get("route") {
        route::write_file(hub, route);
    }
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

/// One memory sync round at a time from this agent: a start waiting for its
/// round queues behind a round the sync thread is running.
static SYNC_ROUND: Mutex<()> = Mutex::new(());

/// One memory sync round, performed. `mem sync` exits 0 when a round was
/// already running and it did nothing, and that round may have started
/// before the hub's write, so only "performed" counts.
fn synced() -> bool {
    let _round = SYNC_ROUND.lock().unwrap_or_else(|e| e.into_inner());
    Command::new(memcli::bin())
        .args(["sync", "--json"])
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| serde_json::from_slice::<Value>(&out.stdout).ok())
        .is_some_and(|doc| doc["sync"] == "performed")
}

/// `workflow go` in a scope of its own, so stopping the agent's unit never
/// stops the orchestrator it started.
fn start(project: &str) -> bool {
    succeeds(
        &bin("WORKFLOW_SYSTEMD_RUN", "systemd-run"),
        &["--user", "--scope", "--quiet", "workflow", "go", project],
    )
}

/// Run one command and print one line for it. `false` when it must be tried
/// again after the next poll. Syncs are not commands: they run on the sync
/// thread, and a resume asks it for a round through `sync`.
fn run(job: &Job, sync: &mpsc::Sender<()>) -> bool {
    match job {
        Job::Sync => {
            let _ = sync.send(());
        }
        Job::Start { project } => {
            // Start often follows an approval the hub just wrote, and
            // workflow go reads this machine's store.
            if !synced() {
                println!(
                    "start {project}: no sync round performed, trying again after the next poll"
                );
                return false;
            }
            let ok = start(project);
            println!("start {project}: {}", if ok { "started" } else { "failed" });
        }
        Job::Notify { title, body } => {
            let sent = Command::new(bin("WORKFLOW_NOTIFY_SEND", "notify-send"))
                .args(["-a", "workflow"])
                .arg(title)
                .arg(body)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !sent {
                say(format!("notify-send could not show {title:?}"));
            }
        }
        Job::Resume {
            project,
            id,
            message,
        } => {
            // The message carries the answer, so the session needs no sync
            // first; the round after it catches the store up.
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
                    let _ = sync.send(());
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
                    let _ = sync.send(());
                    return true;
                }
            }
            // A fresh start reads the store, so it waits for a round as a
            // start command does.
            if !synced() {
                println!(
                    "resume {id}: session gone, start {project}: no sync round performed, trying again after the next poll"
                );
                return false;
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

/// The saved token and hub base URL, or `None` before pairing.
pub(crate) fn pairing() -> Option<(String, String)> {
    let dir = state_dir()?;
    let token = fs::read_to_string(dir.join("agent-token")).ok()?;
    let hub = fs::read_to_string(dir.join("agent-hub")).ok()?;
    Some((token.trim().to_string(), hub.trim().to_string()))
}

pub fn cmd_run(once: bool) -> i32 {
    let Some((token, hub)) = pairing() else {
        say("not paired; run `workflow agent pair <link>`");
        return exit::USAGE;
    };
    let Some(name) = machine::name() else {
        say("cannot tell this machine's name; set WORKFLOW_MACHINE");
        return exit::FAILED;
    };
    let interval = std::env::var("WORKFLOW_AGENT_INTERVAL_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5000);

    // Commands run on a worker so a slow one never holds up the polls that
    // keep this machine online on the hub. Sync rounds run on a thread of
    // their own so a 60 s round never delays a resume. A command or a round
    // that did not finish comes back through `held` and goes out again after
    // the next poll.
    let held: Arc<Mutex<Vec<Job>>> = Arc::default();
    let (sync_tx, sync_rx) = mpsc::channel::<()>();
    let syncer = {
        let held = Arc::clone(&held);
        thread::spawn(move || {
            for () in &sync_rx {
                // Requests that piled up behind a round share the next one.
                while sync_rx.try_recv().is_ok() {}
                if synced() {
                    println!("sync: done");
                    continue;
                }
                println!("sync: no round performed, trying again after the next poll");
                let mut held = held.lock().unwrap();
                if !held.iter().any(|j| matches!(j, Job::Sync)) {
                    held.push(Job::Sync);
                }
            }
        })
    };
    let (tx, rx) = mpsc::channel::<Job>();
    let worker = {
        let held = Arc::clone(&held);
        let sync_tx = sync_tx.clone();
        thread::spawn(move || {
            for job in rx {
                if !run(&job, &sync_tx) {
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
            match job {
                Job::Sync => {
                    let _ = sync_tx.send(());
                }
                job => {
                    let _ = tx.send(job);
                }
            }
        }
        if once {
            drop(tx);
            let _ = worker.join();
            drop(sync_tx);
            let _ = syncer.join();
            return if polled { exit::OK } else { exit::FAILED };
        }
        thread::sleep(Duration::from_millis(interval));
    }
}
