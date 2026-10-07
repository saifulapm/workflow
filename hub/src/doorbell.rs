//! The doorbell (spec §5).
//!
//! A background thread polls the pending queue every 15 s and rings once for
//! every question id it has not seen. In the same round it rings for a walk
//! that ended, from serve's `dogfood <slug>:` run lines, and for a roadmap
//! that finished on this machine. What it publishes is deliberately thin:
//! the machine, the project, and a link back here — **never the question
//! text**, nor any other item's.
//!
//! ### What the topic actually protects
//!
//! ntfy.sh is a public broker. Anyone holding the topic can read every message
//! published to it, and can publish to it. So what this discloses to whoever
//! holds the topic is the machine name, the project name, the hub URL, and the
//! timing of every question — not the question text. The topic is the only
//! secret: 128 bits, stored 0600, and never printed anywhere that is not
//! already behind the tailnet.
//!
//! ### Two rules about the seen file
//!
//! It is written and fsynced **before** the ring, so a crash between the two
//! loses a doorbell rather than looping on one — a missed buzz costs a delay,
//! a repeated one costs trust in the doorbell. And it survives a restart, which
//! is what makes `systemctl --user restart` silent (AC5).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::app::App;
use crate::live::{self, Amx, Run};
use crate::memcli::{MemCli, Outcome};
use crate::page_home::summary_of;

pub const DEFAULT_POLL: Duration = Duration::from_secs(15);
/// §5's own command line: `curl -fsS -m 10 -d <body> <ntfy_base>/<topic>`.
pub const CURL_TIMEOUT_SECONDS: &str = "10";

pub struct Doorbell {
    pub seen_path: PathBuf,
    pub ntfy_url: String,
    pub machine: String,
    /// Where the buzz sends the phone.
    pub hub_url: String,
    pub poll: Duration,
    pub amx: Arc<Amx>,
}

impl Doorbell {
    pub fn from_app(app: &App) -> Result<Doorbell> {
        Ok(Doorbell {
            seen_path: crate::config::state_home()?.join("hub/seen"),
            ntfy_url: app.config.ntfy_url(),
            machine: app.machine.clone(),
            hub_url: hub_url(app),
            poll: poll_interval(),
            amx: Arc::clone(&app.amx),
        })
    }

    /// Runs until the process ends. Nothing in here may panic or return: the
    /// doorbell failing is not the service failing.
    pub fn run(self, mem: Arc<MemCli>) {
        let loaded = load(&self.seen_path);
        // No file at all means this is the first start on this machine. The
        // backlog is recorded and rung for zero times: a fresh install should
        // not buzz once per already-pending question, and neither should a
        // machine that has just joined and synced somebody else's queue.
        let mut state = State {
            seeding: loaded.is_none(),
            seen: loaded.unwrap_or_default(),
            activity: BTreeMap::new(),
            record_failed: false,
            unstarted: BTreeMap::new(),
        };
        loop {
            self.round(&mem, &mut state);
            std::thread::sleep(self.poll);
        }
    }

    fn round(&self, mem: &MemCli, state: &mut State) {
        // Fresh, not cached: a stale queue here is a late doorbell, and the
        // poll interval is already the rate limit.
        let outcome = mem.questions_fresh();
        // Silence is not an empty queue (review M-1). This used to ask
        // `outcome.broken()`, which is `None` for `Outcome::Absent`, so §4a's
        // "exit non-zero, empty stdout" row fell through, the loop over no rows
        // did nothing, and `seeding` was cleared anyway — a first start that
        // met one such poll then rang for the entire backlog. `list_fault` is
        // the page's own reading of §4a (decision 6.7), shared here so the two
        // readings of silence cannot drift apart again.
        if let Some(why) = crate::model::list_fault(&outcome, "questions") {
            // Not fatal, and not a reason to seed: try again next round.
            eprintln!("hub: doorbell: {why}");
            return;
        }
        for row in outcome.rows("questions") {
            let Some(id) = row["id"].as_str() else {
                continue;
            };
            if !self.record(state, id) {
                continue;
            }
            // A question born on another machine arrives here by sync, already
            // rung for where it was asked; this hub lists and answers it but
            // does not buzz again. A row with no machine field still rings —
            // a mem too old to say is a doorbell, not a silence.
            if self.foreign(&row) {
                continue;
            }
            let project = row["project"].as_str().unwrap_or("");
            // The engine pauses a milestone by asking this question, so the
            // phone hears why it stopped rather than that something waits.
            if row["body"]
                .as_str()
                .is_some_and(|body| body.contains(THIRD_STRIKE))
            {
                self.deliver(&self.event_body("paused by the engine", project));
            } else {
                self.deliver(&self.body(project));
            }
        }
        // A fault anywhere keeps the next round seeding too, so a first start
        // never rings for a backlog it could not read whole.
        if self.walks_and_roadmaps(mem, state) {
            state.seeding = false;
        }
    }

    /// The walk results and finished roadmaps, read from what serve and mem
    /// already write. False when a read failed this round.
    fn walks_and_roadmaps(&self, mem: &MemCli, state: &mut State) -> bool {
        let projects = mem.refresh(&["projects", "--json"]);
        if let Some(why) = crate::model::list_fault(&projects, "projects") {
            eprintln!("hub: doorbell: {why}");
            return false;
        }
        let mut settled = true;
        // Asked once a round, and only when some project could have stalled.
        let mut agents = None;
        for project in projects.rows("projects") {
            let Some(name) = project["name"].as_str() else {
                continue;
            };
            let total = project["milestones_total"].as_u64().unwrap_or(0);
            if project["roadmap_status"].as_str() == Some("maintenance")
                && total > 0
                && project["milestones_done"].as_u64() == Some(total)
                && summary_of(&project).checked_out
                // The total is in the key so a roadmap that grows and
                // finishes again rings again.
                && self.record(state, &format!("finished {name} {total}"))
            {
                self.deliver(&self.event_body("roadmap finished", name));
            }

            match self.stall(mem, &project, &mut agents) {
                // Keyed by the dead agent and when amx made it, so the resumed
                // orchestrator's own death rings again, and so does a name amx
                // forgot and `workflow go` took again.
                Some(Stall::Died(key)) => {
                    state.unstarted.remove(name);
                    if self.record(state, &key) {
                        self.deliver(&self.event_body("stalled", name));
                    }
                }
                // Nothing ever ran this milestone. Right after an approval, or
                // a landing, that is the moment before `workflow go` starts
                // one, so it rings only once it has lasted the grace.
                Some(Stall::Unstarted(key)) => {
                    let since = *state
                        .unstarted
                        .entry(name.to_string())
                        .or_insert_with(Instant::now);
                    if since.elapsed() >= self.poll * UNSTARTED_GRACE_POLLS
                        && self.record(state, &key)
                    {
                        self.deliver(&self.event_body("stalled", name));
                    }
                }
                None => {
                    state.unstarted.remove(name);
                }
            }

            // A project that wrote nothing since the last round has no new
            // run line, so it costs no `mem log`.
            let activity = project["last_activity"].to_string();
            if state.activity.get(name) == Some(&activity) {
                continue;
            }
            let log = mem.refresh(&[
                "log",
                "--type",
                "run",
                "--limit",
                "20",
                &format!("--project={name}"),
                "--json",
            ]);
            if let Some(why) = crate::model::list_fault(&log, "log") {
                // Left unrecorded, so the next round reads it again.
                eprintln!("hub: doorbell: {name}: {why}");
                settled = false;
                continue;
            }
            state.activity.insert(name.to_string(), activity);
            for row in log.rows("items") {
                let Some(event) = row["title"].as_str().and_then(walk_event) else {
                    continue;
                };
                let Some(id) = row["id"].as_str() else {
                    continue;
                };
                if self.record(state, id) && !self.foreign(&row) {
                    self.deliver(&self.event_body(event, name));
                }
            }
        }
        settled
    }

    /// Whether this project has stalled, and the seen key to ring it under.
    fn stall(
        &self,
        mem: &MemCli,
        project: &serde_json::Value,
        agents: &mut Option<Result<Vec<live::Agent>, String>>,
    ) -> Option<Stall> {
        let s = summary_of(project);
        let milestone = s.milestone.as_deref()?;
        if !live::idle(&s, &Run::Dead(None)) {
            return None;
        }
        let agents = agents.get_or_insert_with(|| self.amx.agents_fresh());
        let agents = agents.as_ref().ok()?;
        let run = live::orchestrator(agents, &s.name, milestone);
        let Run::Dead(dead) = &run else {
            return None;
        };
        let handoff = mem.refresh(&["handoff", &format!("--project={}", s.name), "--json"]);
        let handoff = match &*handoff {
            Outcome::Json(doc) => doc["body"].as_str().unwrap_or_default(),
            _ => return None,
        };
        if !live::stalled(&s, &run, handoff) {
            return None;
        }
        Some(match dead {
            Some(id) => {
                let created = agents
                    .iter()
                    .find(|agent| agent.id == *id)
                    .and_then(|agent| agent.created)
                    .unwrap_or(0);
                Stall::Died(format!("stalled {id} {created}"))
            }
            None => Stall::Unstarted(format!("stalled {} {milestone}", s.name)),
        })
    }

    /// Records `key` in the seen file and says whether to ring for it: not
    /// when it was rung before, not when it could not be recorded, and not
    /// while the first start is seeding.
    fn record(&self, state: &mut State, key: &str) -> bool {
        if state.seen.contains(key) {
            return false;
        }
        // Recorded and fsynced first. If it cannot be recorded, it is not
        // rung either — otherwise a read-only state directory turns into a
        // buzz every fifteen seconds, for ever.
        if let Err(e) = remember(&self.seen_path, key) {
            // Once per run of failures, not once per poll for ever: an
            // unwritable seen file used to log four lines a minute, per
            // stuck question, indefinitely (review m-5).
            if !state.record_failed {
                eprintln!("hub: doorbell: could not record {key}: {e:#}");
                state.record_failed = true;
            }
            return false;
        }
        state.record_failed = false;
        state.seen.insert(key.to_string());
        !state.seeding
    }

    /// Written on another machine, which rang for it there.
    fn foreign(&self, row: &serde_json::Value) -> bool {
        row["machine"].as_str().is_some_and(|m| m != self.machine)
    }

    /// Where the bell rings: the phone, and only for an empty house. A watched
    /// machine gets nothing — whoever is at it sees questions in the session
    /// itself (a background agent asks through its own question tool when the
    /// machine is watched; `mem ask` is the locked-screen channel), so any
    /// bell here would be a duplicate on a screen already showing the thing.
    fn deliver(&self, body: &str) {
        if crate::presence::sample().watching() {
            return;
        }
        self.ring(body);
    }

    /// One POST, by argv. Never retried: §5 says a failure is logged and
    /// forgotten, and the seen file has already moved on.
    fn ring(&self, body: &str) {
        let result = Command::new("curl")
            .args([
                "-fsS",
                "-m",
                CURL_TIMEOUT_SECONDS,
                "-d",
                body,
                &self.ntfy_url,
            ])
            .output();
        match result {
            Ok(out) if out.status.success() => {}
            Ok(out) => eprintln!(
                "hub: doorbell: ntfy said no ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(e) => eprintln!("hub: doorbell: could not run curl: {e}"),
        }
    }

    /// The whole message. A project name and a URL, and no question text — the
    /// text is what the tailnet is for.
    pub fn body(&self, project: &str) -> String {
        if project.is_empty() {
            format!("question waiting on {} — {}", self.machine, self.hub_url)
        } else {
            format!(
                "question waiting on {} ({}) — {}",
                self.machine, project, self.hub_url
            )
        }
    }

    /// A ring for something that happened to a project, linking to its page.
    /// Like `body`, it carries no item's text.
    pub fn event_body(&self, event: &str, project: &str) -> String {
        format!(
            "{event} on {} ({project}) — {}p/{}",
            self.machine,
            self.hub_url,
            crate::form::encode_component(project)
        )
    }
}

/// What the third-strike question says, and the only words of it read here.
pub const THIRD_STRIKE: &str = "failed its Show path three times";

/// The ring for one of serve's `dogfood <slug>: <result>` run lines, if it
/// gets one: a pass and open findings do, `no Show path` and `skipped` do not.
pub fn walk_event(title: &str) -> Option<&'static str> {
    let (_slug, result) = title.strip_prefix("dogfood ")?.split_once(": ")?;
    if result == "pass" {
        Some("walk passed")
    } else if result.starts_with("findings ") {
        Some("walk found defects")
    } else {
        None
    }
}

/// How many polls a milestone no orchestrator has run waits before it rings:
/// two minutes at the 15 s poll.
const UNSTARTED_GRACE_POLLS: u32 = 8;

/// A stalled project, by its seen key.
enum Stall {
    /// Its orchestrator died.
    Died(String),
    /// No orchestrator has run its milestone.
    Unstarted(String),
}

/// What one run of the doorbell carries between rounds.
struct State {
    seen: BTreeSet<String>,
    /// True until one round has completed without a fault: the backlog is
    /// recorded, and rung for zero times.
    seeding: bool,
    /// Each project's `last_activity` as last read, so a project that has
    /// not moved is not asked for its run log again.
    activity: BTreeMap<String, String>,
    /// Whether the last attempt to record an id failed, so the log says so once
    /// rather than every fifteen seconds (review m-5).
    record_failed: bool,
    /// When each project was first seen stalled with no orchestrator ever
    /// run, so it rings only once that has lasted.
    unstarted: BTreeMap<String, Instant>,
}

/// Where the buzz sends the phone: whatever the config names, else this
/// machine's tailnet name, else the machine name.
///
/// The machine name is a fallback and not the default any more, because it is
/// the one that was wrong. It comes from `~/.config/qshell/machine`, which here
/// reads `macbook-m2` — a name that resolves on no network at all, so the phone
/// opened a dead link. `Guard::host_allowed` already admits anything under
/// `.ts.net`, so the tailnet name passes hub's own Host check unchanged.
fn hub_url(app: &App) -> String {
    crate::pair::base_url(&app.config, app.port, &app.machine)
}

/// 15 s, or `HUB_POLL_MS` — the same kind of seam as mem's `MEM_POLL_MS`, so a
/// test does not have to wait a quarter of a minute per round.
pub fn poll_interval() -> Duration {
    std::env::var("HUB_POLL_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis)
        // A zero from the seam would be a busy loop spawning a `mem` per pass.
        .map(|poll| poll.max(Duration::from_millis(10)))
        .unwrap_or(DEFAULT_POLL)
}

/// The ids already rung for, or `None` when the file has never existed.
pub fn load(path: &Path) -> Option<BTreeSet<String>> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// Appends one id and fsyncs it, so the ring that follows can never be the
/// thing that survives a crash while the record of it does not.
pub fn remember(path: &Path, id: &str) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
        // The directory entry needs to be durable too, or the file can vanish
        // with the crash it was written to survive.
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    writeln!(file, "{id}")?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_names_the_machine_and_project_and_nothing_else() {
        let doorbell = Doorbell {
            seen_path: PathBuf::from("/nonexistent"),
            ntfy_url: "http://127.0.0.1:9/workflow-TOPIC".to_string(),
            machine: "macbook".to_string(),
            hub_url: "http://macbook:8787/".to_string(),
            poll: DEFAULT_POLL,
            amx: Arc::new(Amx::new()),
        };
        assert_eq!(
            doorbell.body("proj-alpha"),
            "question waiting on macbook (proj-alpha) — http://macbook:8787/"
        );
        // A global question has no project, and the sentence still reads.
        assert_eq!(
            doorbell.body(""),
            "question waiting on macbook — http://macbook:8787/"
        );
    }
}
