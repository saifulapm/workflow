//! The device agents: what each machine last reported, and the commands
//! queued for it.
//!
//! An agent polls `POST /api/agent` every few seconds with its machine name,
//! its amx agents and its checkouts, and is handed its queue. The queues and
//! the list of machines that ever polled live in `agents.json` in the hub's
//! state directory, so a restart neither loses a command nor forgets whom to
//! sync. The reports live in memory only: after a restart every machine reads
//! offline until it polls again.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::http::{Request, Response};
use crate::live::Agent;
use crate::pair;

/// A machine with no poll for this long reads offline.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(30);

/// One command for a machine's agent, as the poll hands it out and as the
/// queue file keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Command {
    Sync,
    Start {
        project: String,
    },
    Resume {
        project: String,
        id: String,
        message: String,
    },
    /// A desktop notification on the machine the route names.
    Notify {
        title: String,
        body: String,
    },
    /// A message for a session that may still be running: `amx send` while
    /// it runs, `amx resume` with it once it has stopped.
    #[serde(rename = "follow_up")]
    FollowUp {
        project: String,
        id: String,
        message: String,
    },
}

/// What a machine said in its last poll.
#[derive(Debug, Clone)]
pub struct Report {
    pub agents: Vec<Agent>,
    pub checkouts: Vec<String>,
    /// Whether someone is at the machine, by its qshell presence marker.
    pub watched: bool,
    /// Since when its polls have said nobody is at it, unbroken. Kept in
    /// memory like the rest, so a hub restart starts the count again.
    pub unwatched_since: Option<SystemTime>,
    pub last_poll: SystemTime,
}

/// The body of a poll.
#[derive(Deserialize)]
struct Poll {
    machine: String,
    agents: Vec<Agent>,
    checkouts: Vec<String>,
    #[serde(default)]
    watched: bool,
}

/// What `agents.json` holds.
#[derive(Default, Serialize, Deserialize)]
struct Saved {
    /// Every machine that has polled, in the order they first did.
    known: Vec<String>,
    queues: BTreeMap<String, Vec<Command>>,
    /// The question ids each amx session has been resumed with, so a session
    /// that parks again is told only what it has not heard.
    #[serde(default)]
    delivered: BTreeMap<String, Vec<String>>,
}

#[derive(Default)]
struct State {
    saved: Saved,
    reports: BTreeMap<String, Report>,
}

pub struct Agents {
    path: PathBuf,
    window: Duration,
    state: Mutex<State>,
}

impl Agents {
    /// The queue file at `path`, or an empty one when it is missing or will
    /// not parse. A bad file is logged rather than refused, so the pages
    /// still serve.
    pub fn open(path: PathBuf, window: Duration) -> Agents {
        let saved = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!("hub: ignoring {}: {e}", path.display());
                Saved::default()
            }),
            Err(_) => Saved::default(),
        };
        Agents {
            path,
            window,
            state: Mutex::new(State {
                saved,
                reports: BTreeMap::new(),
            }),
        }
    }

    /// Whether nobody is at `machine`: its agent has stopped polling, or has
    /// said nobody is at it for `after` straight.
    pub fn away(&self, machine: &str, after: Duration) -> bool {
        if !self.online(machine) {
            return true;
        }
        self.report(machine)
            .and_then(|report| report.unwatched_since)
            .is_some_and(|since| since.elapsed().is_ok_and(|idle| idle >= after))
    }

    /// Whether `machine` polled within the window.
    pub fn online(&self, machine: &str) -> bool {
        self.report(machine).is_some_and(|report| {
            report
                .last_poll
                .elapsed()
                .is_ok_and(|since| since < self.window)
        })
    }

    pub fn report(&self, machine: &str) -> Option<Report> {
        self.lock().reports.get(machine).cloned()
    }

    /// Every machine that has ever polled.
    pub fn machines(&self) -> Vec<String> {
        self.lock().saved.known.clone()
    }

    /// Adds `command` to the end of `machine`'s queue. A `sync` is not added
    /// while one is already waiting: one sync covers every write before it.
    pub fn queue(&self, machine: &str, command: Command) {
        let mut state = self.lock();
        let queue = state.saved.queues.entry(machine.to_string()).or_default();
        if command == Command::Sync && queue.contains(&Command::Sync) {
            return;
        }
        queue.push(command);
        self.save(&state.saved);
    }

    pub fn pending(&self, machine: &str) -> Vec<Command> {
        self.lock()
            .saved
            .queues
            .get(machine)
            .cloned()
            .unwrap_or_default()
    }

    /// The question ids `session` has already been resumed with.
    pub fn delivered(&self, session: &str) -> Vec<String> {
        self.lock()
            .saved
            .delivered
            .get(session)
            .cloned()
            .unwrap_or_default()
    }

    /// Queues `resume` for `machine` and records `ids` as told to its session.
    pub fn queue_resume(&self, machine: &str, resume: Command, ids: &[String]) {
        if let Command::Resume { id: session, .. } = &resume {
            let mut state = self.lock();
            let told = state.saved.delivered.entry(session.clone()).or_default();
            told.extend(ids.iter().cloned());
            state
                .saved
                .queues
                .entry(machine.to_string())
                .or_default()
                .push(resume);
            self.save(&state.saved);
        }
    }

    /// One `sync` for every machine that has polled, after a hub write.
    pub fn queue_sync_all(&self) {
        for machine in self.machines() {
            self.queue(&machine, Command::Sync);
        }
    }

    /// Records a poll and hands back the machine's queue, emptied.
    fn poll(&self, poll: Poll) -> Vec<Command> {
        let mut state = self.lock();
        let now = SystemTime::now();
        let unwatched_since = match state.reports.get(&poll.machine) {
            _ if poll.watched => None,
            Some(last) if !last.watched => last.unwatched_since,
            _ => Some(now),
        };
        state.reports.insert(
            poll.machine.clone(),
            Report {
                agents: poll.agents,
                checkouts: poll.checkouts,
                watched: poll.watched,
                unwatched_since,
                last_poll: now,
            },
        );
        let new = !state.saved.known.contains(&poll.machine);
        if new {
            state.saved.known.push(poll.machine.clone());
        }
        let commands = state.saved.queues.remove(&poll.machine);
        // Most polls find nothing queued; only a change is worth a write.
        if new || commands.is_some() {
            self.save(&state.saved);
        }
        commands.unwrap_or_default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Writes the file through a temp file and a rename. A failure is logged:
    /// the queue in memory is still right until the hub restarts.
    fn save(&self, saved: &Saved) {
        let result = serde_json::to_string(saved)
            .map_err(anyhow::Error::from)
            .and_then(|text| pair::rewrite_private(&self.path, &text));
        if let Err(e) = result {
            eprintln!("hub: saving {}: {e:#}", self.path.display());
        }
    }
}

/// 30 s, or `HUB_AGENT_WINDOW_MS`, so a test can see a machine go offline
/// without waiting half a minute.
pub fn window() -> Duration {
    std::env::var("HUB_AGENT_WINDOW_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_WINDOW)
}

/// Whether the request carries `Authorization: Bearer <token>` for a paired
/// device. An agent sends no cookie and no Origin, so this is its only check.
pub(crate) fn authorized(request: &Request) -> bool {
    request
        .header("authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| pair::token_known(token.trim()))
}

pub(crate) fn json(status: u16, value: serde_json::Value) -> Response {
    Response::new(status, "application/json", value.to_string())
}

pub(crate) fn unauthorized() -> Response {
    json(401, serde_json::json!({"error": "not a paired device"}))
}

/// `POST /api/agent`: the token first, so a refused poll touches nothing.
/// The answer carries the route, from `route` once the poll is taken.
pub fn poll_post(
    agents: &Agents,
    request: &Request,
    route: impl FnOnce() -> serde_json::Value,
) -> Response {
    if !authorized(request) {
        return unauthorized();
    }
    let poll: Poll = match serde_json::from_slice(&request.body) {
        Ok(poll) => poll,
        Err(e) => return json(400, serde_json::json!({"error": e.to_string()})),
    };
    if poll.machine.trim().is_empty() {
        return json(400, serde_json::json!({"error": "machine is empty"}));
    }
    let commands = agents.poll(poll);
    json(
        200,
        serde_json::json!({"commands": commands, "route": route()}),
    )
}

/// `GET /api/agents`: each machine that has polled, whether it is online, and
/// when it last polled, or null when not since the hub started.
pub fn list_get(agents: &Agents, request: &Request) -> Response {
    if !authorized(request) {
        return unauthorized();
    }
    let machines: Vec<serde_json::Value> = agents
        .machines()
        .into_iter()
        .map(|machine| {
            let last_poll = agents.report(&machine).and_then(|report| {
                jiff::Timestamp::try_from(report.last_poll)
                    .ok()
                    .map(|at| at.to_string())
            });
            serde_json::json!({
                "name": machine,
                "online": agents.online(&machine),
                "last_poll": last_poll,
            })
        })
        .collect();
    json(200, serde_json::json!({"machines": machines}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_queued_message_survives_a_reopen_byte_for_byte() {
        let dir = std::env::temp_dir().join(format!("hub-agents-unit-{}", std::process::id()));
        let path = dir.join("agents.json");
        let resume = Command::Resume {
            project: "gamma".to_string(),
            id: "gamma-m1".to_string(),
            message: "say \"hi\"\\there\nagain".to_string(),
        };
        let agents = Agents::open(path.clone(), DEFAULT_WINDOW);
        agents.queue("mini", Command::Sync);
        agents.queue("mini", resume.clone());
        agents.queue("mini", Command::Sync);
        drop(agents);
        let agents = Agents::open(path, DEFAULT_WINDOW);
        assert_eq!(agents.pending("mini"), vec![Command::Sync, resume]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
