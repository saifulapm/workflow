//! `GET /p/<project>`: one project's front page, and its log, plan and item
//! routes.
//!
//! The front page follows the project's stage: a header with the stage, the
//! runner and the progress, the Pause or Resume button, then what that stage
//! is about. It costs at most five `mem` spawns: the projects list the route
//! already read, then at most four reads for the body.

use serde_json::Value;

use crate::form::encode_component;
use crate::html::{
    self, degraded_banner, esc, item_list_section, markdown_article, page_url, project_url,
};
use crate::http::Response;
use crate::live::{self, Run};
use crate::memcli::Outcome;
use crate::model;
use crate::page_decisions::{ruling_block, ruling_rows};
use crate::page_evidence::{FindingRow, finding_rows, findings_section};
use crate::page_home::{ProjectSummary, project_summaries, stage_pill};
use crate::page_questions::question_rows;
use crate::page_roadmap::{roadmap_body, roadmap_rows};
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
    let running = matches!(summary.stage, "execution" | "dogfooding");
    if running {
        body.push_str(&orchestrator_line(ctx, &summary));
    }
    if summary.stage == "execution" {
        body.push_str(&run_summary(ctx, project));
    }
    body.push_str(&controls(&summary, project, running));
    // While a milestone is live. A draft roadmap is the planning body itself,
    // and a shipped project's page is its backlog, which already costs the
    // budget's last reads.
    if running {
        body.push_str(&timeline(ctx, project, &summary));
    }
    body.push_str(&match summary.stage {
        "brief" => brief_body(ctx, project),
        "research" => research_body(ctx, project),
        "grilling" => grilling_body(ctx, project),
        "spec" => spec_body(ctx, project),
        "planning" => roadmap_body(ctx),
        "dogfooding" => walk_body(ctx, project),
        "maintenance" => backlog_body(ctx, project),
        "execution" => building_body(ctx, project),
        _ => status_body(ctx, project),
    });
    Response::html(page_shell(summary.stage, ctx.project, &body))
}

/// The stage, then `paused`, the runner, the open milestone's place and the
/// current plan's ticks, as the front page lists them.
pub fn stage_header(s: &ProjectSummary, ctx: &PageCtx) -> String {
    let mut pills = format!(
        "<span class=\"{}\">{}</span>",
        stage_pill(s.stage),
        esc(s.stage)
    );
    if s.paused.is_some() {
        pills.push_str("<span class=\"pill wait\">paused</span>");
    }
    let mut parts = Vec::new();
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
    format!(
        "<div class=\"row\">{pills}<span class=\"meta\">{}</span></div>\n",
        parts.join(" · ")
    )
}

/// The roadmap as a timeline: the landed milestones, the live one with its
/// plan's ticks, then the one after it. A milestone with a stored plan links
/// to it. Two spawns, and the walk body finds the roadmap cached.
fn timeline(ctx: &PageCtx, project: &str, s: &ProjectSummary) -> String {
    let roadmap = ctx.app.mem.roadmap(project);
    let text = match &*roadmap {
        Outcome::Broken(why) => return degraded_banner(why),
        Outcome::Json(doc) => doc["text"].as_str().unwrap_or_default(),
        Outcome::Absent => return String::new(),
    };
    let plans: Vec<String> = ctx
        .app
        .mem
        .plan_list(project)
        .rows("plans")
        .iter()
        .filter_map(|row| row["slug"].as_str().map(str::to_string))
        .collect();
    let mut out = String::from("<h2>Roadmap</h2>\n<ul class=\"tl\">\n");
    let mut open = 0;
    for row in roadmap_rows(text) {
        let name = if plans.contains(&row.slug) {
            format!(
                "<a href=\"{}/plan/{}\">{}</a>",
                esc(&project_url(project)),
                esc(&encode_component(&row.slug)),
                esc(&row.slug)
            )
        } else {
            esc(&row.slug)
        };
        let (class, pill) = if row.ticked {
            (
                " class=\"done\"",
                "<span class=\"pill ok\">landed</span>".to_string(),
            )
        } else {
            open += 1;
            match open {
                1 => (" class=\"live\"", live_pill(s, &row.slug)),
                2 => ("", "<span class=\"pill mut\">next</span>".to_string()),
                _ => ("", String::new()),
            }
        };
        let pill = if pill.is_empty() {
            pill
        } else {
            format!(" {pill}")
        };
        out.push_str(&format!(
            "<li{class}>{name}{pill}<div class=\"meta\">{}</div></li>\n",
            esc(&row.title)
        ));
    }
    out.push_str("</ul>\n");
    out
}

/// The live milestone's ticks, when the current plan is its own.
fn live_pill(s: &ProjectSummary, slug: &str) -> String {
    let (ticked, total) = s.tasks;
    if total > 0 && s.plan_slug.as_deref() == Some(slug) {
        format!("<span class=\"pill\">{ticked} of {total} tasks</span>")
    } else {
        "<span class=\"pill\">live</span>".to_string()
    }
}

/// The orchestrator amx runs for the current milestone, or why there is
/// none: stalled, or parked on a question. Nothing when amx cannot be asked,
/// and the handoff is read only when no orchestrator is live, so a running
/// project's page costs no extra read for it.
fn orchestrator_line(ctx: &PageCtx, s: &ProjectSummary) -> String {
    let project = s.name.as_str();
    let run = ctx.app.amx.run(project, s.milestone.as_deref());
    let machine = ctx.app.machine.as_str();
    let (pill, meta) = match &run {
        Run::Live(agent) => (
            "<span class=\"pill\">running</span>",
            format!(
                "{} · {}",
                agent.id,
                agent.state.as_deref().unwrap_or("unknown")
            ),
        ),
        Run::Dead(_) if live::idle(s, &run, machine) => {
            let handoff = match &*ctx.app.mem.handoff(project) {
                Outcome::Json(doc) => doc["body"].as_str().unwrap_or_default().to_string(),
                _ => String::new(),
            };
            let milestone = s.milestone.as_deref().unwrap_or_default();
            if live::stalled(s, &run, &handoff, machine) {
                (
                    "<span class=\"pill bad\">stalled</span>",
                    format!("no live orchestrator for {milestone}"),
                )
            } else {
                (
                    "<span class=\"pill wait\">parked</span>",
                    format!("{milestone} is waiting on you"),
                )
            }
        }
        _ => return String::new(),
    };
    format!(
        "<p class=\"row\">{pill} <span class=\"meta\">{}</span></p>\n",
        esc(&meta)
    )
}

/// Under the header of a running project: the questions waiting on the owner,
/// and links to the pages that hold them.
pub fn run_summary(ctx: &PageCtx, project: &str) -> String {
    let mut out = String::from("<h2>Run</h2>\n");

    // Every project's pending questions, which the doorbell's poll keeps
    // cached, rather than a read of this project's own.
    let questions = ctx.app.mem.questions();
    match questions.broken() {
        Some(why) => out.push_str(&degraded_banner(why)),
        None => {
            let rows: Vec<Value> = questions
                .rows("questions")
                .into_iter()
                .filter(|q| q["project"].as_str() == Some(project))
                .collect();
            let waiting = question_rows(&rows).iter().filter(|q| !q.answered).count();
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
        "<p><a href=\"{base}/questions\">questions</a> · \
         <a href=\"{base}/evidence\">evidence</a></p>\n"
    ));
    out
}

/// Pause on a running project, Resume on a paused one in any stage. Worked out
/// from mem, so a reload or a second phone reads the same.
pub fn controls(s: &ProjectSummary, project: &str, running: bool) -> String {
    let paused = s.paused.is_some();
    if !paused && !running {
        return String::new();
    }
    let (verb, label) = if paused {
        ("resume", "Resume")
    } else {
        ("pause", "Pause")
    };
    format!(
        "<form method=\"post\" action=\"{}/control\">\n\
         <input type=\"hidden\" name=\"do\" value=\"{verb}\">\n\
         <button type=\"submit\">{label}</button>\n\
         </form>\n",
        esc(&project_url(project))
    )
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

/// The last moves and the last handoff, for a building project.
fn building_body(ctx: &PageCtx, project: &str) -> String {
    let moves = model::last_moves(
        &ctx.app.mem,
        project,
        jiff::Timestamp::now().as_millisecond(),
    );
    let mut out = String::new();
    if let Some(why) = &moves.degraded {
        out.push_str(&degraded_banner(why));
    }
    out.push_str(&item_list_section("Last moves", &moves.rows, project));
    out.push_str("<h2>Handoff</h2>\n");
    match &*ctx.app.mem.handoff(project) {
        Outcome::Broken(why) => out.push_str(&degraded_banner(why)),
        Outcome::Json(doc) => {
            let text = doc["body"]
                .as_str()
                .map(str::trim)
                .filter(|t| !t.is_empty());
            out.push_str(&markdown_article(text, project, "No handoff recorded."));
        }
        Outcome::Absent => out.push_str(&markdown_article(None, project, "No handoff recorded.")),
    }
    out
}

/// The open milestone's Show path as numbered steps, each marked by the
/// milestone's open findings and its newest walk, then those findings and
/// the way to file one.
fn walk_body(ctx: &PageCtx, project: &str) -> String {
    let roadmap = ctx.app.mem.roadmap(project);
    let text = match &*roadmap {
        Outcome::Broken(why) => return degraded_banner(why),
        Outcome::Json(doc) => doc["text"].as_str().unwrap_or_default(),
        Outcome::Absent => "",
    };
    let Some(milestone) = roadmap_rows(text).into_iter().find(|row| !row.ticked) else {
        return "<p class=\"empty\">No open milestone.</p>\n".to_string();
    };
    let slug = &milestone.slug;
    let mut out = format!(
        "<h2>Show path</h2>\n<p class=\"meta\">{} · {}</p>\n",
        esc(slug),
        esc(&milestone.title)
    );
    let findings: Vec<FindingRow> = open_findings(ctx, project, &mut out)
        .into_iter()
        .filter(|f| f.milestone == *slug)
        .collect();
    let steps = show_steps(milestone.show.as_deref().unwrap_or_default());
    if steps.is_empty() {
        out.push_str(&format!(
            "<p class=\"empty\">{} has no Show path.</p>\n",
            esc(slug)
        ));
    } else {
        // The doorbell reads run lines with these same arguments, so the
        // page often finds them cached.
        let runs = ctx.app.mem.read(&[
            "log",
            "--type",
            "run",
            "--limit",
            "20",
            &format!("--project={project}"),
            "--json",
        ]);
        if let Some(why) = model::list_fault(&runs, "log") {
            out.push_str(&degraded_banner(&why));
        }
        let passed = walk_passed(&runs.rows("items"), slug);
        out.push_str("<ul>\n");
        for (i, step) in steps.iter().enumerate() {
            let n = i + 1;
            out.push_str(&format!(
                "<li>{} {n}. {}</li>\n",
                step_mark(n, &findings, passed),
                esc(step)
            ));
        }
        out.push_str("</ul>\n");
    }
    out.push_str(&findings_section(&findings, project));
    out.push_str(&format!(
        "<p><a href=\"{}/new\">File a finding</a></p>\n",
        esc(&project_url(project))
    ));
    out
}

/// A Show line as its steps, in order; step `n` is index `n - 1`. Cut the
/// way the engine cuts it, so a step number here is the one a finding names
/// with `mem finding add --step`.
pub fn show_steps(show: &str) -> Vec<String> {
    show.split(", ")
        .flat_map(|part| part.split("; "))
        .map(|step| {
            let step = step.trim();
            step.strip_prefix("and ")
                .or_else(|| step.strip_prefix("then "))
                .unwrap_or(step)
        })
        .filter(|step| !step.is_empty())
        .map(str::to_string)
        .collect()
}

/// A cross where an open finding names step `n`, a tick once the newest
/// walk passed, a dash otherwise. A finding outweighs the pass, since a
/// finding filed after the walk still stands.
pub fn step_mark(n: usize, findings: &[FindingRow], passed: bool) -> &'static str {
    if findings.iter().any(|f| f.step.trim() == n.to_string()) {
        "✗"
    } else if passed {
        "✓"
    } else {
        "-"
    }
}

/// Whether the milestone's newest `dogfood <slug>:` run line reads `pass`.
/// mem lists run lines newest first.
pub fn walk_passed(rows: &[Value], slug: &str) -> bool {
    let prefix = format!("dogfood {slug}: ");
    rows.iter()
        .find_map(|row| row["title"].as_str()?.strip_prefix(&prefix))
        == Some("pass")
}

/// The project's open findings in mem's order; a fault leaves a banner in
/// `out` and no rows.
fn open_findings(ctx: &PageCtx, project: &str, out: &mut String) -> Vec<FindingRow> {
    let outcome = ctx.app.mem.read(&[
        "finding",
        "list",
        "--open",
        &format!("--project={project}"),
        "--json",
    ]);
    if let Some(why) = model::list_fault(&outcome, "finding list") {
        out.push_str(&degraded_banner(&why));
    }
    finding_rows(&outcome.rows("items"))
        .into_iter()
        .filter(|f| f.open)
        .collect()
}

/// The status, the last handoff, the open findings and the ideas, with the
/// way to start a new round or file an idea.
fn backlog_body(ctx: &PageCtx, project: &str) -> String {
    let mut out = status_body(ctx, project);
    let findings = open_findings(ctx, project, &mut out);
    out.push_str(&findings_section(&findings, project));

    out.push_str("<h2>Ideas</h2>\n");
    let ideas = ctx.app.mem.read(&[
        "search",
        "--kind",
        "idea",
        "--limit",
        "20",
        &format!("--project={project}"),
        "--json",
    ]);
    if let Some(why) = model::list_fault(&ideas, "search") {
        out.push_str(&degraded_banner(&why));
    }
    let rows: Vec<String> = ideas
        .rows("items")
        .iter()
        .filter_map(|row| {
            // mem keeps the body with a newline either side.
            row["body"]
                .as_str()
                .or_else(|| row["title"].as_str())
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(esc)
        })
        .collect();
    if rows.is_empty() {
        out.push_str("<p class=\"empty\">No ideas.</p>\n");
    } else {
        out.push_str("<ul>\n");
        for idea in rows {
            out.push_str(&format!("<li>{idea}</li>\n"));
        }
        out.push_str("</ul>\n");
    }

    let base = esc(&project_url(project));
    out.push_str(&format!(
        "<p><a href=\"{base}/new\">New round</a> · <a href=\"{base}/new\">File an idea</a></p>\n"
    ));
    out
}
