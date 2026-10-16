//! The runner's local hub rings the Slack DM for its own runs: once when a
//! run has stalled for 2 minutes, with a Resume button, and once when a
//! milestone it ran lands. Stalled is the pages' own rule (`live::stalled`):
//! an approved roadmap with work left, the runner's session not live here,
//! not paused, no parked handoff. A paused project never stalls.
//!
//! What was rung is kept on disk, so a restarted hub rings neither a stall
//! it rang already nor a landing twice. A stall rings again only once the
//! run was seen live in between.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hubpages::host::{Command as Job, Fleet, Polled};
use hubpages::live::{self, Agent, Run};
use hubpages::page_home::summary_of;
use serde_json::Value;

use crate::fleet;
use crate::proc::{self, Ended};
use crate::slack::Slack;

const MEM_TIMEOUT: Duration = Duration::from_secs(10);

fn ms_from_env(name: &str, default: Duration) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|ms| ms.trim().parse().ok())
        .map_or(default, Duration::from_millis)
}

fn mem_json(args: &[&str]) -> Option<Value> {
    let mut command = Command::new("mem");
    command.args(args);
    match proc::output_within(&mut command, MEM_TIMEOUT) {
        Ended::Exited(done) if done.code == Some(0) => serde_json::from_slice(&done.stdout).ok(),
        _ => None,
    }
}

/// This machine alone, as amx lists its agents: the pages' run rule needs
/// a fleet, and here only this machine's word counts.
struct Here {
    machine: String,
    agents: Vec<Agent>,
}

impl Fleet for Here {
    fn online(&self, machine: &str) -> bool {
        machine == self.machine
    }
    fn report(&self, machine: &str) -> Option<Polled> {
        (machine == self.machine).then(|| Polled {
            agents: self.agents.clone(),
            ..Polled::default()
        })
    }
    fn machines(&self) -> Vec<String> {
        vec![self.machine.clone()]
    }
    fn pending(&self, _machine: &str) -> Vec<Job> {
        Vec::new()
    }
}

fn rung_file() -> Option<PathBuf> {
    Some(crate::config::state_home().ok()?.join("hub/rung"))
}

/// The keys kept on disk: `stalled <project> <session>` while a rung stall
/// lasts, `open <project> <milestone>` for a milestone run here and not yet
/// landed, `landed <project> <milestone>` once rung.
fn read_rung() -> BTreeSet<String> {
    rung_file()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| text.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

fn write_rung(rung: &BTreeSet<String>) {
    let Some(path) = rung_file() else {
        return;
    };
    let mut body: String = rung.iter().map(|key| format!("{key}\n")).collect();
    if body.is_empty() {
        body.push('\n');
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let temp = path.with_extension("tmp");
    if let Err(e) = std::fs::write(&temp, body).and_then(|()| std::fs::rename(&temp, &path)) {
        eprintln!("hub: stalls: keeping what was rung: {e}");
    }
}

pub struct Bell {
    machine: String,
    /// The name peers reach this hub by, which a Resume button carries.
    host: String,
    slack: Option<Arc<Slack>>,
    after: Duration,
    rung: BTreeSet<String>,
    /// When each project was first seen stalled in this stall.
    since: HashMap<String, Instant>,
}

impl Bell {
    fn post_stall(&self, project: &str) -> bool {
        let text = format!("{project} stalled on {}", self.machine);
        match &self.slack {
            Some(slack) => match slack.post_stall(&text, project, &self.machine, &self.host) {
                Ok(_) => true,
                Err(e) => {
                    eprintln!("hub: stalls: posting {text:?}: {e:#}");
                    false
                }
            },
            None => {
                eprintln!("hub: stalls: {text} (no Slack here)");
                true
            }
        }
    }

    fn post_text(&self, text: &str) -> bool {
        match &self.slack {
            Some(slack) => match slack.post_text(text) {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("hub: stalls: posting {text:?}: {e:#}");
                    false
                }
            },
            None => {
                eprintln!("hub: stalls: {text} (no Slack here)");
                true
            }
        }
    }

    fn round(&mut self) {
        let Some(projects) = mem_json(&["projects", "--json"]) else {
            return;
        };
        let Some(agents) = fleet::agents() else {
            return;
        };
        let here = Here {
            machine: self.machine.clone(),
            agents: serde_json::from_value(Value::Array(agents)).unwrap_or_default(),
        };
        let before = self.rung.clone();
        for row in projects["projects"].as_array().into_iter().flatten() {
            let s = summary_of(row);
            self.landings(&s);
            self.stall(&s, &here);
        }
        if self.rung != before {
            write_rung(&self.rung);
        }
    }

    /// A milestone noted open while this machine ran it rings once when the
    /// open milestone moves on, whoever runs the next.
    fn landings(&mut self, s: &hubpages::page_home::ProjectSummary) {
        let prefix = format!("open {} ", s.name);
        let noted: Vec<String> = self
            .rung
            .iter()
            .filter_map(|key| key.strip_prefix(&prefix).map(str::to_string))
            .collect();
        for milestone in noted {
            if s.milestone.as_deref() == Some(milestone.as_str()) {
                continue;
            }
            let landed = format!("landed {} {milestone}", s.name);
            if !self.rung.contains(&landed)
                && !self.post_text(&format!(
                    "{}: {milestone} landed on {}",
                    s.name, self.machine
                ))
            {
                continue;
            }
            self.rung.remove(&format!("{prefix}{milestone}"));
            self.rung.insert(landed);
        }
        let mine = s.runner.as_ref().is_some_and(|r| r.machine == self.machine);
        if let (true, Some(open)) = (mine, s.milestone.as_deref()) {
            self.rung.insert(format!("{prefix}{open}"));
        }
    }

    fn stall(&mut self, s: &hubpages::page_home::ProjectSummary, here: &Here) {
        let Some(runner) = s.runner.as_ref().filter(|r| r.machine == self.machine) else {
            self.since.remove(&s.name);
            return;
        };
        let key = format!("stalled {} {}", s.name, runner.id);
        let run = live::run(Some(runner), &s.name, s.milestone.as_deref(), here);
        if !matches!(run, Run::Dead(_)) || !live::idle(s, &run) {
            // Live again (or nothing left to run): a later stall rings anew.
            self.since.remove(&s.name);
            self.rung.remove(&key);
            return;
        }
        let handoff = mem_json(&["handoff", "--project", &s.name, "--json"]);
        let handoff = handoff
            .as_ref()
            .and_then(|doc| doc["body"].as_str())
            .unwrap_or_default();
        if !live::stalled(s, &run, handoff) {
            self.since.remove(&s.name);
            return;
        }
        let since = *self
            .since
            .entry(s.name.clone())
            .or_insert_with(Instant::now);
        if since.elapsed() < self.after || self.rung.contains(&key) {
            return;
        }
        if self.post_stall(&s.name) {
            self.rung.insert(key);
        }
    }
}

/// Where a project stands for a Resume tap on this machine, by the same
/// rule the bell rings by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// Its runner is another machine, or it has none.
    NotHere,
    /// Live, parked, paused, or nothing left to run.
    NotStalled,
    Stalled,
    /// mem or amx could not be read.
    Unknown,
}

pub fn standing(machine: &str, project: &str) -> Standing {
    let Some(projects) = mem_json(&["projects", "--json"]) else {
        return Standing::Unknown;
    };
    let Some(s) = projects["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["name"] == project)
        .map(summary_of)
    else {
        return Standing::NotHere;
    };
    let Some(runner) = s.runner.as_ref().filter(|r| r.machine == machine) else {
        return Standing::NotHere;
    };
    let Some(agents) = fleet::agents() else {
        return Standing::Unknown;
    };
    let here = Here {
        machine: machine.to_string(),
        agents: serde_json::from_value(Value::Array(agents)).unwrap_or_default(),
    };
    let run = live::run(Some(runner), &s.name, s.milestone.as_deref(), &here);
    let Some(handoff) = mem_json(&["handoff", "--project", &s.name, "--json"]) else {
        return Standing::Unknown;
    };
    if live::stalled(&s, &run, handoff["body"].as_str().unwrap_or_default()) {
        Standing::Stalled
    } else {
        Standing::NotStalled
    }
}

/// Looks at this machine's runs every 15 s (`HUB_STALL_CHECK_MS`); a stall
/// rings once it has lasted 2 minutes (`HUB_STALL_AFTER_MS`).
pub fn run(machine: String, host: String, slack: Option<Arc<Slack>>) -> ! {
    let every = ms_from_env("HUB_STALL_CHECK_MS", Duration::from_secs(15));
    let mut bell = Bell {
        machine,
        host,
        slack,
        after: ms_from_env("HUB_STALL_AFTER_MS", Duration::from_secs(120)),
        rung: read_rung(),
        since: HashMap::new(),
    };
    loop {
        bell.round();
        std::thread::sleep(every);
    }
}
