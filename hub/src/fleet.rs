//! The local hub's word on its machine, which the hub web's Running section
//! and its online check read: this machine's amx agents, the projects
//! checked out here, those on a worktree of their own, and how many commits
//! each main holds that origin/main lacks. It goes to the store on the
//! sharing socket under the runner name `workflow go` records, when it
//! changes and once every 30 s while it does not, so an idle machine sends
//! 2,880 a day.

use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::proc::{self, Ended};

/// How long amx, mem or git may take for one report.
const RUN_TIMEOUT: Duration = Duration::from_secs(10);

/// The runner name: `WORKFLOW_MACHINE`, else the first label of the
/// tailnet name, as workflow's own rule gives it. `None` on a machine with
/// neither, which then reports nothing: its runs could not be matched.
pub fn runner_name() -> Option<String> {
    if let Ok(name) = std::env::var("WORKFLOW_MACHINE")
        && !name.trim().is_empty()
    {
        return Some(name.trim().to_string());
    }
    // A cloud VM's tailnet name is ephemeral; its session is what its
    // runner record names.
    if let Some(session) = cloud_session() {
        return Some(session);
    }
    let tailnet = crate::tailnet_name()?;
    let label = tailnet.split('.').next()?;
    (!label.is_empty()).then(|| label.to_string())
}

fn ms_from_env(name: &str, default: Duration) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|ms| ms.trim().parse().ok())
        .map_or(default, Duration::from_millis)
}

/// Runs `program args` and returns its stdout when it exits 0.
fn stdout_of(program: &str, args: &[&str]) -> Option<String> {
    let mut command = Command::new(program);
    command.args(args);
    match proc::output_within(&mut command, RUN_TIMEOUT) {
        Ended::Exited(done) if done.code == Some(0) => {
            Some(String::from_utf8_lossy(&done.stdout).into_owned())
        }
        Ended::Exited(done) => {
            eprintln!(
                "hub: report: {program} {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&done.stderr).trim()
            );
            None
        }
        Ended::TimedOut => {
            eprintln!("hub: report: {program} {} was killed", args.join(" "));
            None
        }
        Ended::Failed(why) => {
            eprintln!("hub: report: cannot run {program}: {why}");
            None
        }
    }
}

/// The claude.ai session this machine is, when it is a cloud session, as
/// its runner record names it: `CLAUDE_CODE_REMOTE_SESSION_ID` with `cse_`
/// swapped for `session_`.
pub fn cloud_session() -> Option<String> {
    let id = std::env::var("CLAUDE_CODE_REMOTE_SESSION_ID").ok()?;
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    Some(match id.strip_prefix("cse_") {
        Some(rest) => format!("session_{rest}"),
        None => id.to_string(),
    })
}

/// `WORKFLOW_AMX`, else `amx`, as workflow names it.
pub fn amx_bin() -> String {
    match std::env::var("WORKFLOW_AMX") {
        Ok(bin) if !bin.is_empty() => bin,
        _ => "amx".to_string(),
    }
}

/// This machine's amx agents, each cut to the four keys the pages read.
/// `None` when amx could not say, which is not the same as none running.
pub fn agents() -> Option<Vec<Value>> {
    let text = stdout_of(&amx_bin(), &["ls", "--json"])?;
    if text.trim().is_empty() {
        return Some(Vec::new());
    }
    let rows: Vec<Value> = serde_json::from_str(&text)
        .map_err(|e| eprintln!("hub: report: amx ls --json is not a list: {e}"))
        .ok()?;
    Some(
        rows.iter()
            .map(|a| {
                let key = |k: &str| a.get(k).cloned().unwrap_or(Value::Null);
                json!({"id": key("id"), "state": key("state"), "ended": key("ended"), "created": key("created")})
            })
            .collect(),
    )
}

/// How many commits `dir`'s main holds that origin/main lacks.
/// Quiet when git cannot say: a checkout with no origin/main is common, and
/// a line every 5 s for each would bury the log.
fn unpushed(dir: &str) -> Option<u64> {
    let mut command = Command::new("git");
    command
        .args(["rev-list", "--count", "origin/main..main"])
        .current_dir(dir);
    match proc::output_within(&mut command, RUN_TIMEOUT) {
        Ended::Exited(done) if done.code == Some(0) => {
            String::from_utf8_lossy(&done.stdout).trim().parse().ok()
        }
        _ => None,
    }
}

/// The report as it goes out, without its frame kind. `None` when mem or
/// amx could not be read: a half report would read as runs ended.
pub fn report(machine: &str) -> Option<Value> {
    // A cloud VM runs no amx: the session itself is its run.
    let agents = match cloud_session() {
        Some(_) => Vec::new(),
        None => agents()?,
    };
    let projects: Value = serde_json::from_str(&stdout_of("mem", &["projects", "--json"])?)
        .map_err(|e| eprintln!("hub: report: mem projects --json: {e}"))
        .ok()?;
    let projects = projects["projects"].as_array().cloned().unwrap_or_default();
    let name = |p: &Value| p["name"].as_str().map(str::to_string);
    let dirs = |p: &Value| -> Vec<String> {
        p["checkouts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| d.as_str().map(str::to_string))
            .collect()
    };
    let checkouts: Vec<String> = projects
        .iter()
        .filter(|p| !dirs(p).is_empty())
        .filter_map(name)
        .collect();
    // mem keeps a project's worktree per machine, so only this report can
    // tell the hub web which projects get no cloud run.
    let worktrees: Vec<String> = projects
        .iter()
        .filter(|p| p["worktree"].is_string())
        .filter_map(name)
        .collect();
    // A cloud run clones origin, so the hub web asks for a push first.
    let unpushed: serde_json::Map<String, Value> = projects
        .iter()
        .filter_map(|p| {
            let dir = dirs(p).into_iter().find(|d| Path::new(d).is_dir())?;
            let n = unpushed(&dir).filter(|n| *n > 0)?;
            Some((name(p)?, json!(n)))
        })
        .collect();
    Some(json!({
        "machine": machine,
        "agents": agents,
        "checkouts": checkouts,
        "worktrees": worktrees,
        "unpushed": unpushed,
        "cloud": cloud_session(),
    }))
}

/// The newest report to send and its number; the socket sends a number it
/// has not sent, so a fresh connection sends the newest at once.
static OFFERED: Mutex<(u64, Option<String>)> = Mutex::new((0, None));

fn offer(report: &Value) {
    let mut frame = report.clone();
    frame["t"] = "report".into();
    let mut offered = OFFERED.lock().unwrap_or_else(|e| e.into_inner());
    offered.0 += 1;
    offered.1 = Some(frame.to_string());
}

/// The report to send when it is newer than `sent`, with its number.
pub fn to_send(sent: u64) -> Option<(u64, String)> {
    let offered = OFFERED.lock().unwrap_or_else(|e| e.into_inner());
    let (n, frame) = (offered.0, offered.1.clone()?);
    (n > sent).then_some((n, frame))
}

/// Checks the machine every 5 s (`HUB_REPORT_CHECK_MS`) and offers a report
/// when it changed, or when the last went out 30 s ago
/// (`HUB_REPORT_EVERY_MS`).
pub fn run(machine: String) -> ! {
    let check = ms_from_env("HUB_REPORT_CHECK_MS", Duration::from_secs(5));
    let every = ms_from_env("HUB_REPORT_EVERY_MS", Duration::from_secs(30));
    let mut last: Option<(Value, Instant)> = None;
    loop {
        if let Some(report) = report(&machine) {
            let due = last.as_ref().is_none_or(|(said, at)| {
                live_part(said) != live_part(&report) || at.elapsed() >= every
            });
            if due {
                offer(&report);
                last = Some((report, Instant::now()));
            }
        }
        std::thread::sleep(check);
    }
}

/// The report as far as what runs here goes: each agent's state word cut
/// to live or not, as the pages' `Agent::live` reads it. amx's word flips
/// all the time while an agent works (working, waiting, idle), and sending
/// each flip would report every 5 s and reload every open page with it; the
/// word itself rides the next interval's report.
fn live_part(report: &Value) -> Value {
    let mut part = report.clone();
    if let Some(agents) = part["agents"].as_array_mut() {
        for agent in agents {
            let ended = agent["ended"].as_i64().unwrap_or(0) != 0;
            let stopped = matches!(agent["state"].as_str(), Some("stopped" | "failed"));
            agent["state"] = Value::Bool(!ended && !stopped);
        }
    }
    part
}
