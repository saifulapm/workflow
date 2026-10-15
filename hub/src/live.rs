//! What runs a project, read only from its runner record and the runner
//! machine's device-agent report, and whether a project has stalled.
//!
//! `workflow go` records the machine that started a project's orchestrator
//! and the orchestrator's exact amx id as the project's runner. The hub has
//! no amx of its own: it trusts what that machine's agent last reported.

use serde::Deserialize;

use crate::agents::Agents;
use crate::page_home::ProjectSummary;

/// One agent of `amx ls --json`, as a device agent reports it. amx keys a
/// row by `id`; its `name` field is a display name and is usually null.
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

/// A project's runner record, as `mem projects --json` carries it.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Runner {
    pub machine: String,
    pub id: String,
    #[serde(default)]
    pub since: Option<String>,
}

/// What runs a project.
#[derive(Debug, Clone, PartialEq)]
pub enum Run {
    /// No runner: nothing runs it.
    Unrun,
    /// The runner's machine has not polled within the window.
    Away(String),
    /// The runner's machine lists its id live.
    Live(Agent),
    /// The runner's machine is online and lists its id dead, or not at all.
    Dead(Option<Agent>),
}

/// The run of `project`'s `milestone` with `runner`, from that machine's
/// last report. A runner named for an earlier milestone is that one's
/// orchestrator, which sits idle at its prompt once it hands over, so it
/// never runs this milestone, live or not.
pub fn run(
    runner: Option<&Runner>,
    project: &str,
    milestone: Option<&str>,
    agents: &Agents,
) -> Run {
    let Some(runner) = runner else {
        return Run::Unrun;
    };
    if !agents.online(&runner.machine) {
        return Run::Away(runner.machine.clone());
    }
    let listed = agents
        .report(&runner.machine)
        .and_then(|report| report.agents.into_iter().find(|a| a.id == runner.id));
    let current = milestone.is_none_or(|milestone| named_for(&runner.id, project, milestone));
    match listed {
        Some(agent) if agent.live() && current => Run::Live(agent),
        listed => Run::Dead(listed),
    }
}

/// A name amx accepts, as `workflow go` makes it: lowercase letters, digits
/// and `-`.
fn amx_name(text: &str) -> String {
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

/// What an away run reads: `waiting for <machine>` while a command waits for
/// it, else `<machine> offline`.
pub fn away(machine: &str, agents: &Agents) -> String {
    if agents.pending(machine).is_empty() {
        format!("{machine} offline")
    } else {
        format!("waiting for {machine}")
    }
}

/// Approved work left, a runner whose machine is online and runs nothing
/// for it, no orchestrator that parked on purpose, and no pause. A parked
/// milestone has a question waiting, which has rung already, and a paused
/// project is meant to rest.
pub fn stalled(s: &ProjectSummary, run: &Run, handoff: &str) -> bool {
    idle(s, run)
        && matches!(run, Run::Dead(_))
        && s.paused.is_none()
        && !handoff.trim_start().starts_with("parked")
}

/// Approved work left and nothing running it, parked or not: when a Start
/// or Resume may show. An away runner is not idle, since its machine may
/// still be running it.
pub fn idle(s: &ProjectSummary, run: &Run) -> bool {
    let (done, total) = s.milestones;
    s.roadmap_status.as_deref() == Some("approved")
        && done < total
        && matches!(run, Run::Dead(_) | Run::Unrun)
}

/// The machines a Start or Resume may queue for: the runner's machine once
/// its run is dead, or with no runner each online machine that reports a
/// checkout of the project. `workflow go` refuses a paused project unless
/// its run parked, so a paused one gets a machine only when `parked`.
pub fn start_machines(s: &ProjectSummary, run: &Run, parked: bool, agents: &Agents) -> Vec<String> {
    if !idle(s, run) || (s.paused.is_some() && !parked) {
        return Vec::new();
    }
    match (run, &s.runner) {
        (Run::Dead(_), Some(runner)) => vec![runner.machine.clone()],
        (Run::Unrun, _) => agents
            .machines()
            .into_iter()
            .filter(|machine| {
                agents.online(machine)
                    && agents
                        .report(machine)
                        .is_some_and(|report| report.checkouts.contains(&s.name))
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(done: u64, total: u64) -> ProjectSummary {
        ProjectSummary {
            name: "workflow".to_string(),
            stage: "execution",
            roadmap_status: Some("approved".to_string()),
            checked_out: false,
            milestone: Some("h3-hub-live".to_string()),
            milestones: (done, total),
            plan_slug: None,
            tasks: (0, 0),
            last_activity: None,
            paused: None,
            runner: None,
        }
    }

    #[test]
    fn a_killed_pane_is_dead_though_amx_says_it_never_ended() {
        let killed = Agent {
            id: "x".to_string(),
            state: Some("stopped".to_string()),
            ended: Some(0),
            created: None,
        };
        assert!(!killed.live());
        let done_but_thinking = Agent {
            state: Some("done".to_string()),
            ..killed
        };
        assert!(done_but_thinking.live());
    }

    #[test]
    fn a_parked_milestone_is_not_stalled() {
        let dead = Run::Dead(None);
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
    fn only_approved_unfinished_work_whose_runner_is_dead_stalls() {
        let dead = Run::Dead(None);
        assert!(!stalled(&summary(3, 3), &dead, ""), "finished");
        assert!(!stalled(&summary(2, 3), &Run::Unrun, ""), "no runner");
        assert!(idle(&summary(2, 3), &Run::Unrun), "no runner can start");
        let away = Run::Away("mini".to_string());
        assert!(!stalled(&summary(2, 3), &away, ""), "machine offline");
        assert!(!idle(&summary(2, 3), &away), "no Start while away");
        let live = Run::Live(Agent {
            id: "workflow-h3".to_string(),
            state: Some("working".to_string()),
            ended: Some(0),
            created: None,
        });
        assert!(!stalled(&summary(2, 3), &live, ""), "running");
        let mut draft = summary(2, 3);
        draft.roadmap_status = Some("draft".to_string());
        assert!(!stalled(&draft, &dead, ""), "not approved");
    }
}
