//! The project's buttons, and `POST /p/<project>/control`, which they post
//! to.
//!
//! A button writes mem and nothing else; the engine reads the write on its
//! next tick. Every write names its project with `--project=<name>`, because
//! the hub runs outside any checkout, and puts free text after `--`, so text
//! starting with a dash is never read as a flag.

use crate::html::{esc, project_url};
use crate::http::Response;
use crate::memcli::{Outcome, Run};
use crate::pages::{PageCtx, page_shell, sibling_hub};

/// How a pending approval question starts, as the engine asks it.
pub const APPROVAL_QUESTION: &str = "Approve roadmap";

/// How the plan skill asks for the same approval: "Review the <name> roadmap
/// and its plan pages".
pub const REVIEW_QUESTION: &str = "Review the ";

/// mem's exit when another machine's runner claim refuses an in-place write.
pub const RUNNER_REFUSED: i32 = 5;

pub fn get(ctx: &PageCtx) -> Response {
    Response::html(page_shell("control", ctx.project, ""))
}

pub fn control_post(ctx: &PageCtx) -> Response {
    let project = ctx.project.unwrap_or_default();
    let form = ctx.request.form();
    let verb = form.get("do").unwrap_or("");
    let text = form.get("text").unwrap_or("").trim();
    if !matches!(verb, "approve" | "changes" | "pause" | "resume") {
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
                if run.code == Some(RUNNER_REFUSED) {
                    return refused(ctx, &run);
                }
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
        _ => {
            let run = if verb == "pause" {
                // The words the paused key holds: this machine and the UTC date.
                let words = format!(
                    "{} {}",
                    ctx.app.machine,
                    &jiff::Timestamp::now().to_string()[..10]
                );
                mem.write_through(&["project", "set", &flag, "paused", "--", &words])
            } else {
                mem.write_through(&["project", "unset", &flag, "paused"])
            };
            if !run.ok() {
                return failed(&run);
            }
            Response::see_other(&project_url(project))
        }
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
        || body.starts_with(REVIEW_QUESTION) && body.contains(" roadmap")
}

/// mem's refusal, and the runner's name linked to its hub when a sibling is
/// that machine's, so the phone can approve there.
pub fn refused(ctx: &PageCtx, run: &Run) -> Response {
    let project = ctx.project.unwrap_or_default();
    let current = ctx.app.mem.exec(&[
        "project",
        "current",
        &format!("--project={project}"),
        "--json",
    ]);
    let runner = serde_json::from_slice::<serde_json::Value>(&current.stdout)
        .ok()
        .and_then(|doc| doc["runner"].as_str().map(str::to_string))
        .unwrap_or_default();
    let line = run.stderr.trim();
    let line = line.strip_prefix("mem: ").unwrap_or(line);
    let mut body = format!("<p class=\"banner\">{}</p>\n", esc(line));
    if !runner.is_empty() {
        let name = match sibling_hub(&ctx.app.config, &runner) {
            Some(url) => format!("<a href=\"{}\">{}</a>", esc(&url), esc(&runner)),
            None => esc(&runner),
        };
        body.push_str(&format!("<p>Approve it on {name}.</p>\n"));
    }
    page(ctx, &body)
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

/// A write mem did not take, for a reason other than the runner claim.
pub fn failed(run: &Run) -> Response {
    eprintln!("hub: control: mem exited {:?}: {}", run.code, run.stderr);
    Response::text(502, "mem did not take the write")
}

pub fn or_unset(status: &str) -> &str {
    if status.is_empty() { "not set" } else { status }
}
