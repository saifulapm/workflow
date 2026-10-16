//! A parked session resumes on its own machine once it has every answer it
//! asked for. The runner's local hub checks its runs every 2 s: when every
//! person question the runner's session asked is answered, from any place
//! (Slack, the popup, `mem answer`, the hub web), it hands the session the
//! answers it was not told yet. `runs::resume` then runs `amx resume`, or
//! `workflow go` when amx has lost the session; a live session reads its
//! answers itself.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

use crate::proc::{self, Ended};
use crate::runs;

const MEM_TIMEOUT: Duration = Duration::from_secs(10);

fn mem_json(args: &[&str]) -> Option<Value> {
    let mut command = Command::new("mem");
    command.args(args);
    match proc::output_within(&mut command, MEM_TIMEOUT) {
        Ended::Exited(done) if done.code == Some(0) => serde_json::from_slice(&done.stdout).ok(),
        _ => None,
    }
}

fn told_file() -> Option<PathBuf> {
    Some(crate::config::state_home().ok()?.join("hub/told"))
}

/// The question ids each session was told, as `<session> <question>` lines.
fn read_told() -> Option<HashSet<String>> {
    let text = std::fs::read_to_string(told_file()?).ok()?;
    Some(text.lines().map(str::to_string).collect())
}

fn write_told(told: &HashSet<String>) {
    let Some(path) = told_file() else {
        return;
    };
    let mut lines: Vec<&str> = told.iter().map(String::as_str).collect();
    lines.sort();
    let mut body = lines.join("\n");
    body.push('\n');
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let temp = path.with_extension("tmp");
    if let Err(e) = std::fs::write(&temp, body).and_then(|()| std::fs::rename(&temp, &path)) {
        eprintln!("hub: parked: keeping what was told: {e}");
    }
}

/// The message a resumed session gets: each answer under its question,
/// oldest first.
fn message(asked: &[Value]) -> String {
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
        "Saiful answered your questions. Each answer is below; \
         `mem questions` shows them too.\n\n{}",
        answers.join("\n\n")
    )
}

/// One look at this machine's runs. `seeding` notes every answer as told
/// and resumes nothing: answers given before the hub started belong to a
/// park resumed then.
fn round(machine: &str, told: &mut HashSet<String>, seeding: bool) -> bool {
    let Some(projects) = mem_json(&["projects", "--json"]) else {
        return false;
    };
    let mut changed = false;
    for project in projects["projects"].as_array().into_iter().flatten() {
        let (Some(name), Some(runner)) = (project["name"].as_str(), project.get("runner")) else {
            continue;
        };
        let (Some(on), Some(session)) = (runner["machine"].as_str(), runner["id"].as_str()) else {
            continue;
        };
        if on != machine || session.is_empty() {
            continue;
        }
        let Some(questions) = mem_json(&["questions", "--json", "--project", name]) else {
            continue;
        };
        // An orchestrator's own question from its implementers carries the
        // same amx id, but nobody answers it here.
        let mut asked: Vec<Value> = questions["questions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|row| row["session"] == session && row["audience"] != "orchestrator")
            .cloned()
            .collect();
        if asked.is_empty() || asked.iter().any(|row| row["answered"] != true) {
            continue;
        }
        let key = |row: &Value| format!("{session} {}", row["id"].as_str().unwrap_or(""));
        asked.retain(|row| !told.contains(&key(row)));
        if asked.is_empty() {
            continue;
        }
        // mem lists newest first; a ULID sorts in the order it was made.
        asked.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        for row in &asked {
            told.insert(key(row));
        }
        changed = true;
        if !seeding {
            eprintln!("hub: parked: {name}: every answer is in, resuming {session}");
            runs::resume(name, session, &message(&asked));
        }
    }
    if changed {
        write_told(told);
    }
    true
}

/// Checks every 2 s (`HUB_RESUME_CHECK_MS`) for a session of this machine
/// that has all its answers.
pub fn run(machine: String) -> ! {
    let every = std::env::var("HUB_RESUME_CHECK_MS")
        .ok()
        .and_then(|ms| ms.trim().parse().ok())
        .map_or(Duration::from_secs(2), Duration::from_millis);
    let (mut told, mut seeding) = match read_told() {
        Some(told) => (told, false),
        None => (HashSet::new(), true),
    };
    loop {
        if round(&machine, &mut told, seeding) && seeding {
            seeding = false;
            write_told(&told);
        }
        std::thread::sleep(every);
    }
}
