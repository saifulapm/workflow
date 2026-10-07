//! The project's buttons, and `POST /p/<project>/control`, which they post
//! to.
//!
//! A button writes mem and nothing else; the engine reads the write on its
//! next tick. Every write names its project with `--project=<name>`, because
//! the hub runs outside any checkout, and puts free text after `--`, so text
//! starting with a dash is never read as a flag.

use crate::html::{esc, project_url};
use crate::http::Response;
use crate::live;
use crate::memcli::{Outcome, Run};
use crate::page_home::summary_of;
use crate::pages::{PageCtx, page_shell};

/// How a pending approval question starts, as the engine asks it.
pub const APPROVAL_QUESTION: &str = "Approve roadmap";

/// How the plan skill asks for the same approval: "Review the <name> roadmap
/// and its plan pages", by its start and its end.
pub const REVIEW_QUESTION: (&str, &str) = ("Review the ", " roadmap and its plan pages");

/// How long `workflow go` may take: an `amx new` and a few mem reads.
const GO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

pub fn get(ctx: &PageCtx) -> Response {
    Response::html(page_shell("control", ctx.project, ""))
}

pub fn control_post(ctx: &PageCtx) -> Response {
    let project = ctx.project.unwrap_or_default();
    let form = ctx.request.form();
    let verb = form.get("do").unwrap_or("");
    let text = form.get("text").unwrap_or("").trim();
    if !matches!(verb, "approve" | "changes" | "go") {
        return Response::text(400, "unknown control");
    }
    let roadmap = format!("{}/roadmap", project_url(project));
    if verb == "changes" && text.is_empty() {
        return Response::see_other(&format!("{roadmap}?empty=1"));
    }

    let lock = ctx.app.lock_for(&format!("control:{project}"));
    // Held from the status read to the last write, so two taps cannot both
    // see a draft and both answer its question.
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    let flag = format!("--project={project}");
    let mem = &ctx.app.mem;
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
                let run = mem.write_through(&["roadmap", &flag, "--status", "approved"]);
                if !run.ok() {
                    return failed(&run);
                }
            }
            let answer = format!("changes: {text}");
            let answer = if verb == "approve" {
                "approve"
            } else {
                &answer
            };
            let run = match pending_approval(ctx, project) {
                Some(id) => mem.write_through(&["answer", &flag, "--", &id, answer]),
                None if verb == "changes" => {
                    let line = format!("roadmap changes requested: {text}");
                    mem.write_through(&["log", &flag, "--", &line])
                }
                None => return Response::see_other(&roadmap),
            };
            if !run.ok() {
                return failed(&run);
            }
            Response::see_other(&roadmap)
        }
        _ => go(ctx, project),
    }
}

/// Start or Resume: `workflow go`, once the project is read again as it is
/// now, so a second tap or a second phone cannot start a second orchestrator.
/// Called with the control lock held.
fn go(ctx: &PageCtx, project: &str) -> Response {
    let mem = &ctx.app.mem;
    mem.invalidate();
    let summary = mem
        .refresh(&["projects", "--json"])
        .rows("projects")
        .iter()
        .map(summary_of)
        .find(|s| s.name == project);
    let Some(summary) = summary else {
        return Response::not_found();
    };
    let run = match ctx.app.amx.agents_fresh() {
        Ok(agents) => match summary.milestone.as_deref() {
            Some(milestone) => live::orchestrator(&agents, project, milestone),
            None => live::Run::Dead(None),
        },
        Err(why) => live::Run::Unknown(why),
    };
    if !live::idle(&summary, &run) {
        let why = match &run {
            live::Run::Live(agent) => format!("{} is already running.", agent.id),
            live::Run::Unknown(why) => format!("amx cannot be asked: {why}."),
            live::Run::Dead(_) => "There is no approved milestone here to start.".to_string(),
        };
        return conflict(ctx, &why);
    }
    let argv = live::go_argv(project);
    let mut command = std::process::Command::new(&argv[0]);
    command.args(&argv[1..]);
    let ended = crate::proc::output_within(&mut command, GO_TIMEOUT);
    // Whatever `workflow go` did, the listing from before it is stale, and the
    // page the redirect lands on must not offer Resume again.
    ctx.app.amx.forget();
    match ended {
        crate::proc::Ended::Exited(done) if done.code == Some(0) => {
            Response::see_other(&project_url(project))
        }
        crate::proc::Ended::Exited(done) => {
            let said = String::from_utf8_lossy(&done.stderr);
            conflict(ctx, said.trim())
        }
        crate::proc::Ended::TimedOut => conflict(ctx, "workflow go did not finish in time."),
        crate::proc::Ended::Failed(why) => conflict(ctx, &format!("cannot run workflow go: {why}")),
    }
}

/// The id of the project's pending approval question, found by its project
/// and the first words of its body. A plan page's response answers it too.
pub fn pending_approval(ctx: &PageCtx, project: &str) -> Option<String> {
    ctx.app
        .mem
        .questions_fresh()
        .rows("questions")
        .iter()
        .find(|row| row["project"] == project && row["body"].as_str().is_some_and(is_approval))
        .and_then(|row| row["id"].as_str().map(str::to_string))
}

/// Whether a question asks for the roadmap's approval, in either wording.
fn is_approval(body: &str) -> bool {
    body.starts_with(APPROVAL_QUESTION)
        || body.starts_with(REVIEW_QUESTION.0) && body.trim_end().ends_with(REVIEW_QUESTION.1)
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
pub fn failed(run: &Run) -> Response {
    eprintln!("hub: control: mem exited {:?}: {}", run.code, run.stderr);
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
        assert!(is_approval("Approve roadmap alpha?"));
        assert!(is_approval("Review the beta roadmap and its plan pages"));
        assert!(!is_approval(
            "Review the failing t3 test before the roadmap continues?"
        ));
    }
}
