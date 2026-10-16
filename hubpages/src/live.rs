//! What runs a project, read only from its runner record and the runner
//! machine's device-agent report, and whether a project has stalled.
//!
//! `workflow go` records the machine that started a project's orchestrator
//! and the orchestrator's exact amx id as the project's runner. The hub has
//! no amx of its own: it trusts what that machine's agent last reported.

use serde::Deserialize;

use crate::host::{Command, Fleet};
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
    /// The milestone a cloud run was started on; a device runner names none.
    #[serde(default)]
    pub milestone: Option<String>,
}

/// Where a cloud run stands. No agent reports a cloud session, so the hub
/// reads it off the roadmap and the handoff alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudState {
    Live,
    Parked,
    Landed,
    Ended,
}

/// A project's cloud run: its claude.ai session id, the milestone it runs,
/// when it began, and where it stands.
#[derive(Debug, Clone, PartialEq)]
pub struct CloudRun {
    pub id: String,
    pub milestone: String,
    pub since: Option<String>,
    pub state: CloudState,
}

/// How long a cloud run that writes no handoff counts as live.
const CLOUD_LAPSE: jiff::SignedDuration = jiff::SignedDuration::from_hours(7);

/// The project's cloud run, or None when its runner is not one. Landed once
/// its milestone is no longer the open one. Else a handoff written after it
/// began parks it, when the handoff says `parked:`, or ends it; `handoff`
/// reads that text and runs only then. Live or parked, it lasts seven hours
/// from its start. A run with no readable start is neither.
pub fn cloud_run(
    summary: &ProjectSummary,
    handoff: impl FnOnce() -> Option<String>,
    now: jiff::Timestamp,
) -> Option<CloudRun> {
    let runner = summary.runner.as_ref().filter(|r| r.machine == "cloud")?;
    let milestone = runner.milestone.clone()?;
    let since = runner
        .since
        .as_deref()
        .and_then(|since| since.parse::<jiff::Timestamp>().ok());
    let handoff_at = summary
        .handoff_at
        .as_deref()
        .and_then(|at| at.parse::<jiff::Timestamp>().ok());
    let fresh = since.is_some_and(|since| now.duration_since(since) < CLOUD_LAPSE);
    let state = if summary.milestone.as_deref() != Some(milestone.as_str()) {
        CloudState::Landed
    } else if handoff_at.is_some_and(|at| since.is_none_or(|since| at > since)) {
        // The 6-hour stop parks with no question, so a park lapses too, or
        // it would hold the one cloud slot for ever.
        let text = handoff().unwrap_or_default();
        if text.trim_start().starts_with("parked:") && fresh {
            CloudState::Parked
        } else {
            CloudState::Ended
        }
    } else if fresh {
        CloudState::Live
    } else {
        CloudState::Ended
    };
    Some(CloudRun {
        id: runner.id.clone(),
        milestone,
        since: runner.since.clone(),
        state,
    })
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
    agents: &dyn Fleet,
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

/// What an away run reads: `waiting for <machine>` while a start or resume
/// of `project` waits for it, else `<machine> offline`. The sync every hub
/// write queues, and a notification, are no command of the project's.
pub fn away(machine: &str, project: &str, agents: &dyn Fleet) -> String {
    let waiting = agents.pending(machine).iter().any(|command| match command {
        Command::Start { project: p, .. }
        | Command::Resume { project: p, .. }
        | Command::FollowUp { project: p, .. } => p == project,
        Command::Cloud { .. } | Command::Sync | Command::Notify { .. } => false,
    });
    if waiting {
        format!("waiting for {machine}")
    } else {
        format!("{machine} offline")
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
pub fn start_machines(
    s: &ProjectSummary,
    run: &Run,
    parked: bool,
    agents: &dyn Fleet,
) -> Vec<String> {
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
            handoff_at: None,
        }
    }

    /// gamma building m2, run in the cloud for `milestone` since 09:00.
    fn cloud(milestone: &str, handoff_at: Option<&str>) -> ProjectSummary {
        ProjectSummary {
            name: "gamma".to_string(),
            milestone: Some("m2".to_string()),
            runner: Some(Runner {
                machine: "cloud".to_string(),
                id: "session_01Abc".to_string(),
                since: Some("2026-10-15T09:00:00Z".to_string()),
                milestone: Some(milestone.to_string()),
            }),
            handoff_at: handoff_at.map(str::to_string),
            ..summary(1, 3)
        }
    }

    fn at(time: &str) -> jiff::Timestamp {
        time.parse().unwrap()
    }

    fn unread() -> Option<String> {
        panic!("read the handoff though none was written since the run began")
    }

    fn state(run: Option<CloudRun>) -> CloudState {
        run.expect("a cloud run").state
    }

    #[test]
    fn a_cloud_run_with_no_handoff_since_is_live_for_seven_hours() {
        let run = cloud_run(
            &cloud("m2", Some("2026-10-15T08:00:00Z")),
            unread,
            at("2026-10-15T15:59:00Z"),
        );
        assert_eq!(
            run,
            Some(CloudRun {
                id: "session_01Abc".to_string(),
                milestone: "m2".to_string(),
                since: Some("2026-10-15T09:00:00Z".to_string()),
                state: CloudState::Live,
            })
        );
        let lapsed = cloud_run(&cloud("m2", None), unread, at("2026-10-15T16:00:00Z"));
        assert_eq!(state(lapsed), CloudState::Ended);
    }

    #[test]
    fn a_handoff_after_the_run_began_parks_or_ends_it() {
        let parked = cloud_run(
            &cloud("m2", Some("2026-10-15T09:30:00Z")),
            || Some("\nparked: t2 is local-only".to_string()),
            at("2026-10-15T09:40:00Z"),
        );
        assert_eq!(state(parked), CloudState::Parked);
        let ended = cloud_run(
            &cloud("m2", Some("2026-10-15T09:30:00Z")),
            || Some("m2-t3 done, picking up t4".to_string()),
            at("2026-10-15T09:40:00Z"),
        );
        assert_eq!(state(ended), CloudState::Ended);
    }

    #[test]
    fn a_parked_cloud_run_ends_seven_hours_after_it_began() {
        let parked = |now| {
            cloud_run(
                &cloud("m2", Some("2026-10-15T15:00:00Z")),
                || Some("parked: 6-hour stop, m2-t4 half done".to_string()),
                at(now),
            )
        };
        assert_eq!(state(parked("2026-10-15T15:59:00Z")), CloudState::Parked);
        assert_eq!(state(parked("2026-10-15T16:00:00Z")), CloudState::Ended);
    }

    #[test]
    fn a_cloud_run_whose_milestone_was_ticked_landed() {
        let run = cloud_run(
            &cloud("m1", Some("2026-10-15T09:30:00Z")),
            unread,
            at("2026-10-15T09:40:00Z"),
        );
        assert_eq!(state(run), CloudState::Landed);
    }

    #[test]
    fn a_device_runner_is_no_cloud_run() {
        let mut device = cloud("m2", None);
        device.runner = Some(Runner {
            machine: "mini".to_string(),
            id: "gamma-m2".to_string(),
            since: None,
            milestone: None,
        });
        assert_eq!(cloud_run(&device, unread, at("2026-10-15T09:40:00Z")), None);
        assert_eq!(
            cloud_run(&summary(1, 3), unread, at("2026-10-15T09:40:00Z")),
            None
        );
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
