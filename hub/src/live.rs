//! The orchestrator a project is running, read off `amx ls --json`, and
//! whether a project has stalled without one.
//!
//! `workflow go` starts each milestone's orchestrator as an amx agent named
//! `<project>-<milestone>`, or `-2`, `-3` once that name is taken. The last
//! milestone's orchestrator sits idle at its prompt after starting its
//! successor, so only the agent named for the current milestone is the one
//! that matters: counting any of the project's agents would let an idle
//! predecessor hide a dead orchestrator for ever.

use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::page_home::ProjectSummary;
use crate::proc;

/// How long one `amx ls` may take.
const AMX_TIMEOUT: Duration = Duration::from_secs(5);

/// One agent of `amx ls --json`, as far as the hub reads it. amx keys a row by
/// `id`; its `name` field is a display name and is usually null.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Agent {
    pub id: String,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub ended: Option<i64>,
    #[serde(default)]
    pub created: Option<i64>,
}

impl Agent {
    /// Whether the agent is still there. A killed pane reads `stopped` with
    /// `ended: 0`, so `ended` alone misses it; `done` with `ended: 0` was seen
    /// on an orchestrator that was alive and thinking, so the state word alone
    /// is not enough either.
    pub fn live(&self) -> bool {
        self.ended.unwrap_or(0) == 0 && !matches!(self.state.as_deref(), Some("stopped" | "failed"))
    }
}

/// What amx says of a project's current milestone.
#[derive(Debug, Clone, PartialEq)]
pub enum Run {
    /// Its orchestrator is there.
    Live(Agent),
    /// None is, and the newest that was, if any.
    Dead(Option<String>),
    /// amx could not be asked. Never read as a stall, or a hub on a machine
    /// without amx would flag every project it lists.
    Unknown(String),
}

/// The rows of `amx ls --json`.
pub fn parse(text: &str) -> Result<Vec<Agent>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(text).map_err(|e| format!("amx ls --json is not a list: {e}"))
}

/// A name amx accepts, as `workflow go` makes it: lowercase letters, digits
/// and `-`.
pub fn amx_name(text: &str) -> String {
    let mapped: String = text
        .chars()
        .map(|c| match c.to_ascii_lowercase() {
            c if c.is_ascii_lowercase() || c.is_ascii_digit() => c,
            _ => '-',
        })
        .collect();
    mapped.trim_matches('-').to_string()
}

/// Whether `id` is the name `workflow go` gives the orchestrator of this
/// milestone, with or without the suffix a taken name gets.
fn named_for(id: &str, project: &str, milestone: &str) -> bool {
    let base = amx_name(&format!("{project}-{milestone}"));
    match id.strip_prefix(&base) {
        Some("") => true,
        Some(rest) => rest
            .strip_prefix('-')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())),
        None => false,
    }
}

/// The orchestrator of `milestone`, newest first.
pub fn orchestrator(agents: &[Agent], project: &str, milestone: &str) -> Run {
    let mut mine: Vec<&Agent> = agents
        .iter()
        .filter(|agent| named_for(&agent.id, project, milestone))
        .collect();
    mine.sort_by_key(|agent| std::cmp::Reverse(agent.created.unwrap_or(0)));
    match mine.iter().find(|agent| agent.live()) {
        Some(agent) => Run::Live((*agent).clone()),
        None => Run::Dead(mine.first().map(|agent| agent.id.clone())),
    }
}

/// Approved work left on this machine, nothing running it, and no
/// orchestrator that parked on purpose. A parked milestone has a question
/// waiting, which has rung already.
pub fn stalled(s: &ProjectSummary, run: &Run, handoff: &str) -> bool {
    idle(s, run) && !handoff.trim_start().starts_with("parked")
}

/// Approved work left on this machine and nothing running it, parked or not:
/// when the Start or Resume button shows. Only a project with a checkout
/// here, since `workflow go` refuses one without, so a hub never offers or
/// rings for work its machine cannot run.
pub fn idle(s: &ProjectSummary, run: &Run) -> bool {
    let (done, total) = s.milestones;
    s.roadmap_status.as_deref() == Some("approved")
        && done < total
        && s.checked_out
        && matches!(run, Run::Dead(_))
}

/// What Start and Resume run. `workflow go` starts amx, whose tmux server
/// would otherwise begin inside hub.service's cgroup and die with the next
/// restart of the hub; a scope of its own outlives it.
pub fn go_argv(project: &str) -> Vec<String> {
    [
        "systemd-run",
        "--user",
        "--scope",
        "--quiet",
        "workflow",
        "go",
        project,
    ]
    .map(str::to_string)
    .to_vec()
}

/// One `amx ls`, and when it was read.
type Cached = Option<(Instant, Result<Vec<Agent>, String>)>;

/// `amx ls --json`, cached like mem's reads, so the home page's cards and the
/// doorbell's round cost one spawn between them.
pub struct Amx {
    cache: Mutex<Cached>,
    ttl: Duration,
}

impl Default for Amx {
    fn default() -> Amx {
        Amx::new()
    }
}

impl Amx {
    pub fn new() -> Amx {
        Amx {
            cache: Mutex::new(None),
            ttl: crate::memcli::CACHE_TTL,
        }
    }

    /// The agents, from the cache while it is fresh.
    pub fn agents(&self) -> Result<Vec<Agent>, String> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, agents)) = &*cache
            && at.elapsed() < self.ttl
        {
            return agents.clone();
        }
        let agents = list();
        *cache = Some((Instant::now(), agents.clone()));
        agents
    }

    /// The agents as they are now, for a decision that starts a process.
    pub fn agents_fresh(&self) -> Result<Vec<Agent>, String> {
        let agents = list();
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        *cache = Some((Instant::now(), agents.clone()));
        agents
    }

    /// Drops the cached listing, once something has started an agent.
    pub fn forget(&self) {
        *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// What amx says of the project's current milestone.
    pub fn run(&self, project: &str, milestone: Option<&str>) -> Run {
        let Some(milestone) = milestone else {
            return Run::Dead(None);
        };
        match self.agents() {
            Ok(agents) => orchestrator(&agents, project, milestone),
            Err(why) => Run::Unknown(why),
        }
    }
}

fn list() -> Result<Vec<Agent>, String> {
    let mut command = Command::new("amx");
    command.args(["ls", "--json"]);
    match proc::output_within(&mut command, AMX_TIMEOUT) {
        proc::Ended::Exited(done) if done.code == Some(0) => {
            parse(&String::from_utf8_lossy(&done.stdout))
        }
        proc::Ended::Exited(done) => Err(format!(
            "amx ls failed: {}",
            String::from_utf8_lossy(&done.stderr).trim()
        )),
        proc::Ended::TimedOut => Err("amx ls did not answer".to_string()),
        proc::Ended::Failed(why) => Err(format!("cannot run amx: {why}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Four agents as `amx ls --json` printed them on 2026-10-08, cut to the
    /// keys read here, plus one whose pane was killed.
    const LS: &str = r#"[
      {"id":"goal-probe","name":null,"state":"stopped","ended":1791388312,"created":1791388285},
      {"id":"workflow-h1-plan-pages","name":null,"state":"done","ended":1791397552,"created":1791395204},
      {"id":"workflow-h2-hub-design","name":null,"state":"idle","ended":0,"created":1791397000},
      {"id":"workflow-h3-hub-live","name":null,"state":"working","ended":0,"created":1791400000},
      {"id":"probe-kill","name":null,"state":"stopped","ended":0,"created":1791401000}
    ]"#;

    fn summary(done: u64, total: u64) -> ProjectSummary {
        ProjectSummary {
            name: "workflow".to_string(),
            stage: "execution",
            roadmap_status: Some("approved".to_string()),
            checked_out: true,
            milestone: Some("h3-hub-live".to_string()),
            milestones: (done, total),
            plan_slug: None,
            tasks: (0, 0),
            last_activity: None,
        }
    }

    #[test]
    fn the_orchestrator_is_the_agent_named_for_the_current_milestone() {
        let agents = parse(LS).unwrap();
        match orchestrator(&agents, "workflow", "h3-hub-live") {
            Run::Live(agent) => assert_eq!(agent.id, "workflow-h3-hub-live"),
            other => panic!("{other:?}"),
        }
        // The idle h2 orchestrator is alive, but it is not h3's.
        assert_eq!(
            orchestrator(&agents, "workflow", "h4-next"),
            Run::Dead(None)
        );
    }

    #[test]
    fn a_killed_pane_is_dead_though_amx_says_it_never_ended() {
        let agents = parse(LS).unwrap();
        assert_eq!(
            orchestrator(&agents, "probe", "kill"),
            Run::Dead(Some("probe-kill".to_string()))
        );
        let done_but_thinking = Agent {
            id: "x".to_string(),
            state: Some("done".to_string()),
            ended: Some(0),
            created: None,
        };
        assert!(done_but_thinking.live());
    }

    #[test]
    fn a_retried_name_is_still_the_milestones_and_the_newest_wins() {
        let agents = parse(
            r#"[{"id":"alpha-m1","state":"stopped","ended":0,"created":1},
                {"id":"alpha-m1-2","state":"working","ended":0,"created":2},
                {"id":"alpha-m10","state":"working","ended":0,"created":3},
                {"id":"alpha-m1-x","state":"working","ended":0,"created":4}]"#,
        )
        .unwrap();
        match orchestrator(&agents, "alpha", "m1") {
            Run::Live(agent) => assert_eq!(agent.id, "alpha-m1-2"),
            other => panic!("{other:?}"),
        }
        let dead = parse(
            r#"[{"id":"alpha-m1","state":"stopped","ended":0,"created":1},
                {"id":"alpha-m1-2","state":"failed","ended":9,"created":2}]"#,
        )
        .unwrap();
        assert_eq!(
            orchestrator(&dead, "alpha", "m1"),
            Run::Dead(Some("alpha-m1-2".to_string()))
        );
    }

    #[test]
    fn go_starts_outside_the_hubs_cgroup() {
        assert_eq!(
            go_argv("workflow"),
            [
                "systemd-run",
                "--user",
                "--scope",
                "--quiet",
                "workflow",
                "go",
                "workflow"
            ]
        );
    }

    #[test]
    fn a_parked_milestone_is_not_stalled() {
        let dead = Run::Dead(Some("workflow-h3-hub-live".to_string()));
        assert!(stalled(&summary(2, 3), &dead, "h2 landed at b5e1ad1"));
        assert!(!stalled(
            &summary(2, 3),
            &dead,
            "\nparked: waiting on a question"
        ));
        assert!(
            idle(&summary(2, 3), &dead),
            "a parked one can still be resumed"
        );
    }

    #[test]
    fn only_approved_unfinished_local_work_without_an_agent_stalls() {
        let dead = Run::Dead(None);
        assert!(!stalled(&summary(3, 3), &dead, ""), "finished");
        let mut elsewhere = summary(2, 3);
        elsewhere.checked_out = false;
        assert!(!stalled(&elsewhere, &dead, ""), "no checkout here");
        assert!(!idle(&elsewhere, &dead), "no Start without a checkout");
        let unknown = Run::Unknown("cannot run amx".to_string());
        assert!(!stalled(&summary(2, 3), &unknown, ""), "amx unreadable");
        let live = orchestrator(&parse(LS).unwrap(), "workflow", "h3-hub-live");
        assert!(!stalled(&summary(2, 3), &live, ""), "running");
        let mut draft = summary(2, 3);
        draft.roadmap_status = Some("draft".to_string());
        assert!(!stalled(&draft, &dead, ""), "not approved");
    }
}
