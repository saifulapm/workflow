//! `GET /p/<project>`: one project's front page, and its log, plan and item
//! routes.
//!
//! The front page follows the project's stage: a header with the stage, the
//! runner and the progress, the Pause or Resume button, then what that stage
//! is about. It costs at most five `mem` spawns: the projects list the route
//! already read, then at most three reads for the body. A running project
//! adds one `workflow status` in this machine's checkout.

use serde_json::Value;

use crate::html::{self, degraded_banner, esc, markdown_article, page_url, project_url};
use crate::http::Response;
use crate::memcli::Outcome;
use crate::model;
use crate::page_decisions::{ruling_block, ruling_rows};
use crate::page_home::{ProjectSummary, project_summaries};
use crate::page_questions::question_rows;
use crate::page_roadmap::roadmap_body;
use crate::page_run::{BOARD, engine_status};
use crate::pages::{PageCtx, page_shell, sibling_hub};

/// How many rulings a grilling project shows; the decisions page has them
/// all.
const RULINGS_SHOWN: usize = 10;

pub fn get(ctx: &PageCtx) -> Response {
    if !ctx.rest.is_empty() {
        let path = ctx.request.path.strip_prefix("/p/").unwrap_or_default();
        return ctx.app.project_page(path);
    }
    let project = ctx.project.unwrap_or_default();
    let Some(summary) = project_summaries(&ctx.app.mem)
        .into_iter()
        .find(|s| s.name == project)
    else {
        return Response::not_found();
    };
    let mut body = stage_header(&summary, ctx);
    // The engine is read only where Pause is offered: before the roadmap is
    // approved no engine reads the key, so its stage would never agree.
    let running = matches!(summary.stage, "execution" | "dogfooding");
    let engine = match (&summary.checkout, running) {
        (Some(root), true) => Engine::Status(engine_status(root)),
        _ => Engine::Unread,
    };
    if summary.stage == "execution" {
        body.push_str(&run_summary(ctx, project, &summary, &engine));
    }
    body.push_str(&controls(&summary, project, running, &engine));
    body.push_str(&match summary.stage {
        "brief" => brief_body(ctx, project),
        "research" => research_body(ctx, project),
        "grilling" => grilling_body(ctx, project),
        "spec" => spec_body(ctx, project),
        "planning" => roadmap_body(ctx),
        _ => status_body(ctx, project),
    });
    Response::html(page_shell(summary.stage, ctx.project, &body))
}

/// The stage, then `paused`, the runner, the open milestone's place and the
/// current plan's ticks, as the front page lists them.
pub fn stage_header(s: &ProjectSummary, ctx: &PageCtx) -> String {
    let mut parts = vec![esc(s.stage)];
    if s.paused.is_some() {
        parts.push("paused".to_string());
    }
    if let Some(runner) = &s.runner {
        // Linked only when another machine runs it and this hub knows that
        // machine's hub.
        let link = (*runner != ctx.app.machine)
            .then(|| sibling_hub(&ctx.app.config, runner))
            .flatten()
            .and_then(|url| html::safe_link(&url));
        parts.push(match link {
            Some(link) => format!("<a href=\"{link}\">{}</a>", esc(runner)),
            None => esc(runner),
        });
    }
    let (done, total) = s.milestones;
    if total > 0 {
        parts.push(format!("milestone {} of {total}", (done + 1).min(total)));
    }
    let (ticked, total) = s.tasks;
    if total > 0 {
        parts.push(format!("tasks {ticked} of {total}"));
    }
    format!("<p class=\"meta\">{}</p>\n", parts.join(" · "))
}

/// What this page knows of the engine: nothing when the project is not
/// running or this machine has no checkout of it, else what `workflow
/// status --json` printed, `None` when it gave no answer.
pub enum Engine {
    Unread,
    Status(Option<Value>),
}

/// Under the header of a running project: the engine's stage word, the
/// newest run's task counts by state, the dispatched tasks, the questions
/// waiting on the owner, and links to the pages that hold them.
pub fn run_summary(ctx: &PageCtx, project: &str, s: &ProjectSummary, engine: &Engine) -> String {
    let mut out = String::from("<h2>Run</h2>\n");
    match (engine, &s.checkout) {
        (Engine::Status(Some(status)), _) => out.push_str(&run_counts(status)),
        (Engine::Status(None), Some(root)) => out.push_str(&format!(
            "<p class=\"banner warn\">workflow status gave no answer in {}</p>\n",
            esc(root)
        )),
        _ => out.push_str(&format!(
            "<p class=\"empty\">No checkout of this project on this machine{}.</p>\n",
            s.runner
                .as_deref()
                .map(|r| format!("; {} runs it", esc(r)))
                .unwrap_or_default()
        )),
    }

    let questions = ctx.app.mem.read(&[
        "questions",
        "--pending",
        "--for",
        "human",
        &format!("--project={project}"),
        "--json",
    ]);
    match questions.broken() {
        Some(why) => out.push_str(&degraded_banner(why)),
        None => {
            let waiting = question_rows(&questions.rows("questions"))
                .iter()
                .filter(|q| !q.answered)
                .count();
            let noun = if waiting == 1 {
                "question"
            } else {
                "questions"
            };
            out.push_str(&format!("<p>{waiting} {noun} waiting on you</p>\n"));
        }
    }

    let base = esc(&project_url(project));
    out.push_str(&format!(
        "<p><a href=\"{base}/run\">run</a> · <a href=\"{base}/questions\">questions</a> · \
         <a href=\"{base}/evidence\">evidence</a></p>\n"
    ));
    out
}

/// The stage word and the newest run's counts, then the ids of its
/// dispatched tasks. Status lists runs oldest first.
fn run_counts(status: &Value) -> String {
    let mut parts = vec![esc(status["stage"].as_str().unwrap_or_default())];
    let Some(run) = status["runs"].as_array().and_then(|runs| runs.last()) else {
        parts.push("no run yet".to_string());
        return format!("<p class=\"meta\">{}</p>\n", parts.join(" · "));
    };
    let tasks: Vec<&Value> = run["tasks"].as_array().into_iter().flatten().collect();
    for state in BOARD {
        let n = tasks.iter().filter(|t| t["state"] == state).count();
        if n > 0 {
            parts.push(format!("{n} {state}"));
        }
    }
    let mut out = format!("<p class=\"meta\">{}</p>\n", parts.join(" · "));
    let out_now: Vec<String> = tasks
        .iter()
        .filter(|t| t["state"] == "dispatched")
        .map(|t| esc(t["id"].as_str().unwrap_or_default()))
        .collect();
    if !out_now.is_empty() {
        out.push_str(&format!("<p>dispatched: {}</p>\n", out_now.join(", ")));
    }
    out
}

/// Pause on a running project, Resume on a paused one in any stage, and the
/// line that shows until the engine's stage agrees with the key. Worked out
/// from mem and the engine, so a reload or a second phone reads the same.
pub fn controls(s: &ProjectSummary, project: &str, running: bool, engine: &Engine) -> String {
    let paused = s.paused.is_some();
    if !paused && !running {
        return String::new();
    }
    let (verb, label) = if paused {
        ("resume", "Resume")
    } else {
        ("pause", "Pause")
    };
    let mut out = format!(
        "<form method=\"post\" action=\"{}/control\">\n\
         <input type=\"hidden\" name=\"do\" value=\"{verb}\">\n\
         <button type=\"submit\">{label}</button>\n\
         </form>\n",
        esc(&project_url(project))
    );
    let line = match engine {
        Engine::Status(Some(status)) => {
            let stopped = status["stage"] == "paused";
            if paused && !stopped {
                Some("pause sent, waiting for the engine".to_string())
            } else if !paused && stopped {
                Some("resume sent, waiting for the engine".to_string())
            } else {
                None
            }
        }
        // With no checkout here the engine's stage cannot be read; serve's
        // tick is fifteen minutes, so that is how long the key can wait.
        Engine::Unread if paused && running && s.checkout.is_none() => Some(format!(
            "pause sent; {}'s engine reads the key within fifteen minutes",
            esc(s.runner.as_deref().unwrap_or("the runner"))
        )),
        _ => None,
    };
    if let Some(line) = line {
        out.push_str(&format!("<p class=\"meta\">{line}</p>\n"));
    }
    out
}

/// The brief and the button that asks the engine for research, or the way
/// to write a brief when there is none.
fn brief_body(ctx: &PageCtx, project: &str) -> String {
    let outcome = ctx
        .app
        .mem
        .read(&["brief", &format!("--project={project}"), "--json"]);
    let brief = match &*outcome {
        Outcome::Broken(why) => return degraded_banner(why),
        Outcome::Json(doc) => doc["body"].as_str().map(str::trim).unwrap_or_default(),
        Outcome::Absent => "",
    };
    if brief.is_empty() {
        return format!(
            "<p class=\"empty\">No brief yet. <a href=\"{}/new\">Write one</a>.</p>\n",
            esc(&project_url(project))
        );
    }
    format!(
        "<h2>Brief</h2>\n{}\
         <form method=\"post\" action=\"{}/new/research\">\n\
         <button type=\"submit\">Start research</button>\n\
         </form>\n",
        markdown_article(Some(brief), project, ""),
        esc(&project_url(project)),
    )
}

/// The research pages written so far.
fn research_body(ctx: &PageCtx, project: &str) -> String {
    let outcome = ctx.app.mem.wiki(project);
    if let Some(why) = model::list_fault(&outcome, "wiki") {
        return degraded_banner(&why);
    }
    let mut out = String::from("<h2>Research</h2>\n<ul>\n");
    for row in outcome.rows("pages") {
        let Some(slug) = row["slug"].as_str().filter(|s| s.starts_with("research")) else {
            continue;
        };
        let title = row["title"]
            .as_str()
            .filter(|t| !t.is_empty())
            .unwrap_or(slug);
        out.push_str(&format!(
            "<li><a href=\"{}\">{}</a></li>\n",
            esc(&page_url(project, slug)),
            esc(title)
        ));
    }
    out.push_str("</ul>\n");
    out
}

/// The newest rulings, then the questions waiting on the owner, each linked
/// to the questions page where it is answered.
fn grilling_body(ctx: &PageCtx, project: &str) -> String {
    let mut out = String::from("<h2>Decisions</h2>\n");
    let rulings = ctx.app.mem.items(project, "ruling", RULINGS_SHOWN);
    match model::list_fault(&rulings, "log") {
        Some(why) => out.push_str(&degraded_banner(&why)),
        None => {
            let rows = ruling_rows(&rulings.rows("items"));
            if rows.is_empty() {
                out.push_str("<p class=\"empty\">No rulings yet.</p>\n");
            }
            for row in &rows {
                out.push_str(&ruling_block(row, project));
            }
        }
    }

    out.push_str("<h2>Questions</h2>\n");
    let questions = ctx.app.mem.read(&[
        "questions",
        "--pending",
        "--for",
        "human",
        &format!("--project={project}"),
        "--json",
    ]);
    if let Some(why) = model::list_fault(&questions, "questions") {
        out.push_str(&degraded_banner(&why));
        return out;
    }
    let rows = question_rows(&questions.rows("questions"));
    let pending: Vec<_> = rows.iter().filter(|q| !q.answered).collect();
    if pending.is_empty() {
        out.push_str("<p class=\"empty\">Nothing waiting.</p>\n");
        return out;
    }
    let href = esc(&format!("{}/questions", project_url(project)));
    out.push_str("<ul>\n");
    for question in pending {
        out.push_str(&format!(
            "<li><a href=\"{href}\">{}</a></li>\n",
            esc(question.text.trim())
        ));
    }
    out.push_str("</ul>\n");
    out
}

/// The spec's sections, each linked to its place on the page.
fn spec_body(ctx: &PageCtx, project: &str) -> String {
    let outcome = ctx.app.mem.read(&[
        "wiki",
        "--sections",
        &format!("--project={project}"),
        "--json",
        "--",
        "spec",
    ]);
    if let Some(why) = model::list_fault(&outcome, "wiki") {
        return degraded_banner(&why);
    }
    let base = page_url(project, "spec");
    let mut out = String::from("<h2>Spec</h2>\n<ul>\n");
    for row in outcome.rows("sections") {
        let hslug = row["hslug"].as_str().unwrap_or_default();
        out.push_str(&format!(
            "<li><a href=\"{}\">{}</a></li>\n",
            esc(&format!("{base}#{hslug}")),
            esc(row["heading"].as_str().unwrap_or(hslug))
        ));
    }
    out.push_str("</ul>\n");
    out
}

/// The status and the last handoff, for every stage past the approval.
fn status_body(ctx: &PageCtx, project: &str) -> String {
    let mut out = String::new();
    for (heading, outcome, key, empty) in [
        (
            "Status",
            ctx.app.mem.status(project),
            "text",
            "No status recorded.",
        ),
        (
            "Handoff",
            ctx.app.mem.handoff(project),
            "body",
            "No handoff recorded.",
        ),
    ] {
        out.push_str(&format!("<h2>{heading}</h2>\n"));
        let text = match &*outcome {
            Outcome::Broken(why) => {
                out.push_str(&degraded_banner(why));
                continue;
            }
            Outcome::Json(doc) => doc[key].as_str().map(str::trim).filter(|t| !t.is_empty()),
            Outcome::Absent => None,
        };
        out.push_str(&markdown_article(text, project, empty));
    }
    out
}
