//! The project's buttons, and `POST /p/<project>/control`, which they post
//! to.
//!
//! Approve and Request changes write mem and nothing else; Start and Resume
//! queue a `start` for a device agent, Pause and Unpause run `workflow
//! pause|unpause` here. Every write names its project with `--project=<name>`,
//! because the hub runs outside any checkout, and puts free text after `--`,
//! so text starting with a dash is never read as a flag.

use serde_json::Value;

use crate::host::{CLOUD_MACHINE, Command, Fleet};
use crate::html::{esc, project_url};
use crate::http::Response;
use crate::live::{self, CloudState};
use crate::mem::{Mem, Outcome, Written};
use crate::page_home::summary_of;
use crate::page_project;
use crate::pages::{PageCtx, page_shell, write_through};

/// How the plan skill asks for the roadmap's approval: "Review the <name>
/// roadmap", by its start and its end.
pub const REVIEW_QUESTION: (&str, &str) = ("Review the ", " roadmap");

pub fn control_post(ctx: &PageCtx) -> Response {
    let project = ctx.project.unwrap_or_default();
    let form = ctx.request.form();
    let verb = form.get("do").unwrap_or("");
    let text = form.get("text").unwrap_or("").trim();
    if !matches!(
        verb,
        "approve" | "changes" | "go" | "cloud" | "reply-in-app" | "pause" | "unpause"
    ) {
        return Response::text(400, "unknown control");
    }
    let roadmap = format!("{}/roadmap", project_url(project));
    if verb == "changes" && text.is_empty() {
        return Response::see_other(&format!("{roadmap}?empty=1"));
    }

    let lock = ctx.site.host.lock(&format!("control:{project}"));
    // Held from the status read to the last write, so two taps cannot both
    // see a draft and both answer its question.
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    let flag = format!("--project={project}");
    let mem = ctx.site.mem;
    match verb {
        "approve" | "changes" => {
            mem.invalidate();
            let status = match &*mem.roadmap(project) {
                Outcome::Json(doc) => doc["status"].as_str().unwrap_or("").to_string(),
                _ => String::new(),
            };
            if status != "draft" {
                return conflict(
                    ctx,
                    &format!("The roadmap is {}, not a draft.", or_unset(&status)),
                );
            }
            if verb == "approve" {
                let run =
                    write_through(ctx.site, &["roadmap", &flag, "--status", "approved"], None);
                if !run.ok {
                    return failed(&run);
                }
            }
            let answer = format!("changes: {text}");
            let answer = if verb == "approve" {
                "approve"
            } else {
                &answer
            };
            let question = pending_approval(ctx, project);
            let run = match &question {
                Some(row) => {
                    let id = row["id"].as_str().unwrap_or("");
                    write_through(ctx.site, &["answer", &flag, "--", id, answer], None)
                }
                None if verb == "changes" => {
                    let line = format!("roadmap changes requested: {text}");
                    write_through(ctx.site, &["log", &flag, "--", &line], None)
                }
                None => return Response::see_other(&roadmap),
            };
            if !run.ok {
                return failed(&run);
            }
            if let Some(row) = question {
                ctx.site.host.after_answer(project, row["session"].as_str());
            }
            Response::see_other(&roadmap)
        }
        "pause" | "unpause" => pause(ctx, project, verb),
        "cloud" => cloud(ctx, project),
        "reply-in-app" => reply_in_app(ctx, project),
        _ => go(ctx, project, form.get("machine").unwrap_or("")),
    }
}

/// Pause or Unpause: `workflow pause|unpause`, which write mem and start
/// nothing. Called with the control lock held.
fn pause(ctx: &PageCtx, project: &str, verb: &str) -> Response {
    match ctx.site.host.pause(verb, project) {
        Ok(()) => Response::see_other(&project_url(project)),
        Err(why) => conflict(ctx, &why),
    }
}

/// Start or Resume: one `start` queued for `machine`, whose agent runs
/// `workflow go` there, once the project and the reports are read again as
/// they are now. The machine comes off the form, so it must be one the page's
/// own rule names: a form never moves a run to another machine. Called with
/// the control lock held.
fn go(ctx: &PageCtx, project: &str, machine: &str) -> Response {
    let mem = ctx.site.mem;
    mem.invalidate();
    let summary = mem
        .refresh(&["projects", "--json"])
        .rows("projects")
        .iter()
        .map(summary_of)
        .find(|s| s.name == project);
    let Some(mut summary) = summary else {
        return Response::not_found();
    };
    // A landed or ended cloud run holds the project no more, as on the page.
    if page_project::cloud_run(ctx, &summary)
        .is_some_and(|run| matches!(run.state, CloudState::Landed | CloudState::Ended))
    {
        summary.runner = None;
    }
    let agents = ctx.site.host.fleet();
    let run = live::run(
        summary.runner.as_ref(),
        &summary.name,
        summary.milestone.as_deref(),
        agents,
    );
    // Only a paused project needs the handoff, to tell whether it parked.
    let parked = summary.paused.is_some()
        && match &*mem.handoff(project) {
            Outcome::Json(doc) => doc["body"]
                .as_str()
                .is_some_and(|body| body.trim_start().starts_with("parked")),
            _ => false,
        };
    let machines = live::start_machines(&summary, &run, parked, agents);
    if !machines.iter().any(|m| m == machine) {
        let why = match &run {
            live::Run::Live(agent) => format!("{} is already running.", agent.id),
            live::Run::Away(m) => format!("{project} reads {}.", live::away(m, project, agents)),
            _ if !live::idle(&summary, &run) => {
                "There is no approved milestone to start.".to_string()
            }
            _ if summary.paused.is_some() && !parked => format!("{project} is paused."),
            _ => format!("{project} cannot start on {machine}."),
        };
        return conflict(ctx, &why);
    }
    // A second tap before the agent's next poll adds nothing.
    let start = Command::Start {
        project: project.to_string(),
        cloud: false,
    };
    if !agents.pending(machine).contains(&start) {
        ctx.site.host.queue(machine, start);
    }
    Response::see_other(&project_url(project))
}

/// Run in cloud: one cloud `start` queued for the mini, whose agent runs
/// `workflow go <project> --cloud` there, once the projects are read again
/// as they are now. Called with the control lock held.
fn cloud(ctx: &PageCtx, project: &str) -> Response {
    let mem = ctx.site.mem;
    mem.invalidate();
    let summaries: Vec<_> = mem
        .refresh(&["projects", "--json"])
        .rows("projects")
        .iter()
        .map(summary_of)
        .collect();
    let Some(summary) = summaries.iter().find(|s| s.name == project) else {
        return Response::not_found();
    };
    let agents = ctx.site.host.fleet();
    if let Some(why) = page_project::cloud_refusal(mem, agents, &summaries, summary, ctx.site.now) {
        return conflict(ctx, &why);
    }
    match cloud_machine(agents, project) {
        Err(why) => return conflict(ctx, &why),
        // One cloud start per project, however often the button is pressed.
        Ok(None) => {}
        Ok(Some(machine)) => ctx.site.host.queue(
            &machine,
            Command::Start {
                project: project.to_string(),
                cloud: true,
            },
        ),
    }
    Response::see_other(&project_url(project))
}

/// Where a cloud start for `project` goes: the mini when it is online, else
/// the first online device by name, never `cloud`, that has the project
/// checked out. Ok(None) when any machine already holds the start.
pub(crate) fn cloud_machine(agents: &dyn Fleet, project: &str) -> Result<Option<String>, String> {
    let queued = Command::Start {
        project: project.to_string(),
        cloud: true,
    };
    if agents
        .machines()
        .iter()
        .any(|m| agents.pending(m).contains(&queued))
    {
        return Ok(None);
    }
    if agents.online(CLOUD_MACHINE) {
        return Ok(Some(CLOUD_MACHINE.to_string()));
    }
    let mut names = agents.machines();
    names.sort();
    names
        .into_iter()
        .find(|m| {
            m != "cloud"
                && agents.online(m)
                && agents
                    .report(m)
                    .is_some_and(|r| r.checkouts.iter().any(|c| c == project))
        })
        .map(Some)
        .ok_or_else(|| "no device is online to start it.".to_string())
}

/// Reply in the Claude app: the answers waiting for the mini are withdrawn
/// and marked handled, and the browser goes to the cloud session. Called
/// with the control lock held.
fn reply_in_app(ctx: &PageCtx, project: &str) -> Response {
    let mem = ctx.site.mem;
    mem.invalidate();
    let runner = mem
        .refresh(&["projects", "--json"])
        .rows("projects")
        .iter()
        .map(summary_of)
        .find(|s| s.name == project)
        .and_then(|s| s.runner)
        .filter(|runner| runner.machine == "cloud");
    let Some(runner) = runner else {
        return conflict(ctx, &format!("{project} has no cloud run."));
    };
    ctx.site.host.replied_in_app(project, &runner);
    Response::see_other(&format!("{}/{}", page_project::CODE_PAGE, runner.id))
}

/// The project's pending approval question, found by its project and the
/// first words of its body.
pub fn pending_approval(ctx: &PageCtx, project: &str) -> Option<Value> {
    ctx.site
        .mem
        .questions_fresh()
        .rows("questions")
        .into_iter()
        .find(|row| row["project"] == project && row["body"].as_str().is_some_and(is_approval))
}

/// Whether a question asks for the roadmap's approval.
pub fn is_approval(body: &str) -> bool {
    body.starts_with(REVIEW_QUESTION.0) && body.trim_end().ends_with(REVIEW_QUESTION.1)
}

pub fn conflict(ctx: &PageCtx, why: &str) -> Response {
    page(ctx, &format!("<p class=\"banner\">{}</p>\n", esc(why)))
}

pub fn page(ctx: &PageCtx, body: &str) -> Response {
    Response::new(
        409,
        "text/html; charset=utf-8",
        page_shell("control", ctx.project, body),
    )
}

/// A write mem did not take.
pub fn failed(run: &Written) -> Response {
    eprintln!("hub: control: mem did not take the write: {}", run.stderr);
    Response::text(502, "mem did not take the write")
}

pub fn or_unset(status: &str) -> &str {
    if status.is_empty() { "not set" } else { status }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{Fleet, Polled};

    /// Machines by name: (name, online, checkouts, pending).
    struct Fake(Vec<(&'static str, bool, Vec<&'static str>, Vec<Command>)>);

    impl Fleet for Fake {
        fn online(&self, machine: &str) -> bool {
            self.0.iter().any(|m| m.0 == machine && m.1)
        }
        fn report(&self, machine: &str) -> Option<Polled> {
            let m = self.0.iter().find(|m| m.0 == machine)?;
            Some(Polled {
                checkouts: m.2.iter().map(|c| c.to_string()).collect(),
                ..Polled::default()
            })
        }
        fn machines(&self) -> Vec<String> {
            self.0.iter().map(|m| m.0.to_string()).collect()
        }
        fn pending(&self, machine: &str) -> Vec<Command> {
            self.0
                .iter()
                .find(|m| m.0 == machine)
                .map(|m| m.3.clone())
                .unwrap_or_default()
        }
    }

    fn cloud_start() -> Command {
        Command::Start {
            project: "gamma".into(),
            cloud: true,
        }
    }

    #[test]
    fn a_cloud_run_goes_to_the_mini_when_it_is_online() {
        let fleet = Fake(vec![
            ("mini", true, vec![], vec![]),
            ("nuc", true, vec!["gamma"], vec![]),
        ]);
        assert_eq!(cloud_machine(&fleet, "gamma"), Ok(Some("mini".into())));
    }

    #[test]
    fn a_cloud_run_goes_to_another_online_device_with_the_checkout() {
        let fleet = Fake(vec![
            ("cloud", true, vec!["gamma"], vec![]),
            ("mini", false, vec!["gamma"], vec![]),
            ("air", true, vec!["other"], vec![]),
            ("nuc", true, vec!["gamma"], vec![]),
        ]);
        assert_eq!(cloud_machine(&fleet, "gamma"), Ok(Some("nuc".into())));
    }

    #[test]
    fn a_cloud_run_with_no_online_device_is_refused() {
        let fleet = Fake(vec![
            ("mini", false, vec!["gamma"], vec![]),
            ("nuc", true, vec![], vec![]),
        ]);
        assert_eq!(
            cloud_machine(&fleet, "gamma"),
            Err("no device is online to start it.".to_string())
        );
    }

    #[test]
    fn a_cloud_start_pending_on_any_machine_queues_nothing_more() {
        let fleet = Fake(vec![
            ("mini", true, vec![], vec![]),
            ("nuc", true, vec!["gamma"], vec![cloud_start()]),
        ]);
        assert_eq!(cloud_machine(&fleet, "gamma"), Ok(None));
    }

    #[test]
    fn only_the_roadmap_review_is_an_approval() {
        assert!(is_approval("Review the beta roadmap"));
        assert!(!is_approval("Approve roadmap alpha?"));
        assert!(!is_approval(
            "Review the failing t3 test before the roadmap continues?"
        ));
    }
}
