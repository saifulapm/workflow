//! The project's buttons, and `POST /p/<project>/control`, which they post
//! to.
//!
//! Approve and Request changes write mem and nothing else; Start and Resume
//! queue a `start` for a device agent, Pause and Unpause run `workflow
//! pause|unpause` here. Every write names its project with `--project=<name>`,
//! because the hub runs outside any checkout, and puts free text after `--`,
//! so text starting with a dash is never read as a flag.

use serde_json::Value;

use crate::host::{CLOUD_MACHINE, Command};
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
    if !agents.online(CLOUD_MACHINE) {
        return conflict(ctx, &format!("{CLOUD_MACHINE} is offline."));
    }
    let start = Command::Start {
        project: project.to_string(),
        cloud: true,
    };
    if !agents.pending(CLOUD_MACHINE).contains(&start) {
        ctx.site.host.queue(CLOUD_MACHINE, start);
    }
    Response::see_other(&project_url(project))
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

    #[test]
    fn only_the_roadmap_review_is_an_approval() {
        assert!(is_approval("Review the beta roadmap"));
        assert!(!is_approval("Approve roadmap alpha?"));
        assert!(!is_approval(
            "Review the failing t3 test before the roadmap continues?"
        ));
    }
}
