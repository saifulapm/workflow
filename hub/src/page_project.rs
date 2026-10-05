//! `GET /p/<project>`: one project's front page, and its log, plan and item
//! routes.
//!
//! The front page follows the project's stage: a header with the stage, the
//! runner and the progress, then what that stage is about. It costs at most
//! five `mem` spawns: the projects list the route already read, then at most
//! two reads for the body.

use crate::html::{self, degraded_banner, esc, markdown_article, page_url, project_url};
use crate::http::Response;
use crate::memcli::Outcome;
use crate::model;
use crate::page_decisions::{ruling_block, ruling_rows};
use crate::page_home::{ProjectSummary, project_summaries};
use crate::page_questions::question_rows;
use crate::page_roadmap::roadmap_body;
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
