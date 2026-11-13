//! A cloud thread that parked on its questions has no session here to
//! resume: it wakes only on a new message. So once Saiful has answered
//! every question a parked cloud run asked, from any place, the mini's hub
//! sends that thread one message with `claude -p --cloud <session>`, and he
//! types nothing on claude.ai. Only the mini nudges, so two hubs never
//! nudge one park twice. The park is the one `workflow go --cloud` reads
//! (workflow/src/cloud.rs `parked_questions`): a `parked:` handoff written
//! after the run's latest record, its milestone still the open one, and
//! the questions of its project asked since the run first started.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

use crate::proc::{self, Ended};

/// The one machine that nudges.
const MACHINE: &str = "mac-mini-m2";
const MEM_TIMEOUT: Duration = Duration::from_secs(10);
const SEND_TIMEOUT: Duration = Duration::from_secs(60);
/// How many times one park's nudge is sent before the hub gives it up.
const TRIES: usize = 3;

fn mem_out(args: &[&str]) -> Option<Vec<u8>> {
    let mut command = Command::new("mem");
    command.args(args);
    match proc::output_within(&mut command, MEM_TIMEOUT) {
        Ended::Exited(done) if done.code == Some(0) => Some(done.stdout),
        _ => None,
    }
}

fn mem_json(args: &[&str]) -> Option<Value> {
    serde_json::from_slice(&mem_out(args)?).ok()
}

fn nudged_file() -> Option<PathBuf> {
    Some(crate::config::state_home().ok()?.join("hub/nudged"))
}

/// The parks already nudged, as `<session> <handoff time>` lines.
fn read_nudged() -> HashSet<String> {
    nudged_file()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| text.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

fn write_nudged(nudged: &HashSet<String>) {
    let Some(path) = nudged_file() else {
        return;
    };
    let mut lines: Vec<&str> = nudged.iter().map(String::as_str).collect();
    lines.sort();
    let mut body = lines.join("\n");
    body.push('\n');
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let temp = path.with_extension("tmp");
    if let Err(e) = std::fs::write(&temp, body).and_then(|()| std::fs::rename(&temp, &path)) {
        eprintln!("hub: nudge: keeping what was nudged: {e}");
    }
}

fn time(value: &Value) -> Option<jiff::Timestamp> {
    value.as_str()?.parse().ok()
}

/// A parked cloud run of `project`: its session and the park's key, or
/// None when the run is live, another machine's, or its milestone landed.
fn park(project: &Value) -> Option<(String, String)> {
    let name = project["name"].as_str()?;
    let runner = &project["runner"];
    if runner["machine"] != "cloud" || runner["milestone"].is_null() {
        return None;
    }
    if runner["milestone"] != project["milestone"] {
        return None;
    }
    let session = runner["id"].as_str().filter(|id| !id.is_empty())?;
    let since = time(&runner["since"])?;
    let at = project["handoff_at"].as_str()?;
    if at.parse::<jiff::Timestamp>().ok()? <= since {
        return None;
    }
    let handoff = mem_out(&["handoff", "--project", name])?;
    if !String::from_utf8_lossy(&handoff)
        .trim_start()
        .starts_with("parked:")
    {
        return None;
    }
    Some((session.to_string(), format!("{session} {at}")))
}

/// The questions the run asked since it first started, oldest first; a
/// thread has no amx session and asks Saiful, never an orchestrator.
fn asked(project: &Value) -> Option<Vec<Value>> {
    let name = project["name"].as_str()?;
    let runner = &project["runner"];
    let first = time(&runner["first"]).or_else(|| time(&runner["since"]))?;
    let questions = mem_json(&["questions", "--json", "--project", name])?;
    let mut asked: Vec<Value> = questions["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|q| {
            q["id"]
                .as_str()
                .and_then(hubpages::model::ulid_millis)
                .is_some_and(|ms| ms > first.as_millisecond())
        })
        .filter(|q| q["session"].as_str().is_none_or(str::is_empty))
        .filter(|q| q["audience"] != "orchestrator")
        .cloned()
        .collect();
    asked.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    Some(asked)
}

/// What the woken thread reads: every answer under its question.
fn message(project: &str, asked: &[Value]) -> String {
    let answers: Vec<String> = asked
        .iter()
        .map(|row| {
            format!(
                "#{} {}\nAnswer: {}",
                row["short_id"].as_str().unwrap_or(""),
                row["body"].as_str().unwrap_or(""),
                row["answer"].as_str().unwrap_or(""),
            )
        })
        .collect();
    format!(
        "Every question this thread parked on is answered; the mini's hub sends \
         this for Saiful, who typed nothing. Carry on with {project}'s milestone \
         as your instructions say for a new turn after a park. The answers are \
         below; `mem questions` shows them too.\n\n{}",
        answers.join("\n\n")
    )
}

/// One `claude -p --cloud` send of `text` to `session`.
fn send(session: &str, text: &str) -> Result<(), String> {
    let mut command = Command::new("claude");
    command.args(["-p", "--cloud", session, "--output-format", "json"]);
    if let Ok(dir) = crate::config::state_home() {
        command.current_dir(dir);
    }
    match proc::output_fed(&mut command, Some(text.as_bytes()), SEND_TIMEOUT) {
        Ended::Exited(done) => {
            let reply: Value = serde_json::from_slice(&done.stdout).unwrap_or_default();
            if done.code == Some(0) && reply["ok"] == true {
                Ok(())
            } else {
                let error = reply["error"].as_str().map_or_else(
                    || String::from_utf8_lossy(&done.stderr).trim().to_string(),
                    str::to_string,
                );
                Err(format!("exit {:?}: {error}", done.code))
            }
        }
        _ => Err(format!("no answer in {} s", SEND_TIMEOUT.as_secs())),
    }
}

/// `pause` is the wait between tries of one nudge.
fn round(nudged: &mut HashSet<String>, pause: Duration) {
    let Some(projects) = mem_json(&["projects", "--json"]) else {
        return;
    };
    for project in projects["projects"].as_array().into_iter().flatten() {
        let Some((session, key)) = park(project) else {
            continue;
        };
        if nudged.contains(&key) {
            continue;
        }
        let Some(asked) = asked(project) else {
            continue;
        };
        if asked.is_empty() || asked.iter().any(|q| q["answered"] != true) {
            continue;
        }
        let name = project["name"].as_str().unwrap_or_default();
        let text = message(name, &asked);
        let mut result = send(&session, &text);
        for _ in 1..TRIES {
            if result.is_ok() {
                break;
            }
            std::thread::sleep(pause);
            result = send(&session, &text);
        }
        // Tried no more either way: a claude.ai change that breaks the send
        // leaves a typed message as the way to wake the thread.
        match result {
            Ok(()) => eprintln!("hub: nudge: {name}: woke {session} with its answers"),
            Err(e) => eprintln!(
                "hub: nudge: {name}: {session} did not take the nudge in {TRIES} tries, \
                 so it waits for a typed message: {e}"
            ),
        }
        nudged.insert(key);
        write_nudged(nudged);
    }
}

/// On the mini, checks every 10 s (`HUB_NUDGE_CHECK_MS`) for a parked
/// cloud run with all its answers; on any other machine, nothing.
pub fn run(machine: &str) {
    if machine != MACHINE {
        return;
    }
    let every = std::env::var("HUB_NUDGE_CHECK_MS")
        .ok()
        .and_then(|ms| ms.trim().parse().ok())
        .map_or(Duration::from_secs(10), Duration::from_millis);
    let mut nudged = read_nudged();
    loop {
        std::thread::sleep(every);
        round(&mut nudged, every);
    }
}
