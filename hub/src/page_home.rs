//! `GET /`: every project with its stage, runner, progress and week, then
//! the questions waiting on the owner.
//!
//! The hub runs one `mem` at a time, so this page reads nothing per question
//! and one thing per project: `projects --json` carries each project's
//! summary, `questions --json` each question's whole body, and the week is
//! the project's run lines, which the cache answers for its TTL.

use serde_json::Value;

use crate::config::Config;
use crate::cost::{parse_cost, summary};
use crate::html::{self, Banner, esc};
use crate::http::Response;
use crate::memcli::MemCli;
use crate::model::{self, list_fault};
use crate::pages::{PageCtx, page_shell, sibling_hub};

/// The reload that keeps the page live. It fires only when nothing is
/// focused and nothing is typed, so it never costs an answer being written.
const RELOAD: &str = "<script>setInterval(function(){\
     var a=document.activeElement;\
     if(a&&(a.tagName==='TEXTAREA'||a.tagName==='INPUT'))return;\
     var f=document.querySelectorAll('textarea,input');\
     for(var i=0;i<f.length;i++){if(f[i].value)return;}\
     location.reload();},15000);</script>\n";

/// One project's row on the front page.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectSummary {
    pub name: String,
    pub stage: &'static str,
    pub runner: Option<String>,
    pub paused: Option<String>,
    /// The open milestone's slug, while a roadmap has one.
    pub milestone: Option<String>,
    /// Milestones ticked on the roadmap, and in all.
    pub milestones: (u64, u64),
    /// The current plan's slug, which can be an earlier milestone's.
    pub plan_slug: Option<String>,
    /// Tasks ticked in the current plan, and in all.
    pub tasks: (u64, u64),
    /// RFC 3339, the newest item's time.
    pub last_activity: Option<String>,
}

pub fn get(ctx: &PageCtx) -> Response {
    let app = ctx.app;
    let banner = Banner::from_query(&ctx.request.query);
    let now_ms = jiff::Timestamp::now().as_millisecond();
    Response::html(render(&app.mem, &app.config, &app.machine, now_ms, &banner))
}

/// Every project, from the one `projects --json` read.
pub fn project_summaries(mem: &MemCli) -> Vec<ProjectSummary> {
    mem.projects()
        .rows("projects")
        .iter()
        .map(|row| ProjectSummary {
            name: text(row, "name").unwrap_or_default(),
            stage: lifecycle_stage(row),
            runner: text(row, "runner"),
            paused: text(row, "paused"),
            milestone: text(row, "milestone"),
            plan_slug: text(row, "plan_slug"),
            milestones: (
                count(row, "milestones_done"),
                count(row, "milestones_total"),
            ),
            tasks: (count(row, "plan_ticked"), count(row, "plan_total")),
            last_activity: text(row, "last_activity"),
        })
        .collect()
}

/// The lifecycle stage of one `projects --json` row. mem stores no stage and
/// serve's stage file exists only on the runner, so the hub works it out from
/// what the summary holds.
pub fn lifecycle_stage(row: &Value) -> &'static str {
    match row["roadmap_status"].as_str() {
        Some("draft") => "planning",
        Some("approved" | "running") => {
            // The current plan is the open milestone's and every task in it
            // is ticked: the milestone is built and waiting on its walk.
            let open = row["milestone"].as_str();
            let total = count(row, "plan_total");
            if open.is_some()
                && open == row["plan_slug"].as_str()
                && total > 0
                && count(row, "plan_ticked") == total
            {
                "dogfooding"
            } else {
                "execution"
            }
        }
        Some("maintenance" | "done") => "maintenance",
        _ => {
            let has = |key: &str| row[key].as_bool().unwrap_or(false);
            if has("has_spec") {
                "spec"
            } else if has("has_research_summary") {
                "grilling"
            } else if has("has_research") {
                "research"
            } else {
                "brief"
            }
        }
    }
}

/// The whole page.
pub fn render(
    mem: &MemCli,
    config: &Config,
    machine: &str,
    now_ms: i64,
    banner: &Banner,
) -> String {
    let projects = project_summaries(mem);
    let questions = mem.questions();
    let mut questions_rows = questions.rows("questions");
    questions_rows.sort_by(|a, b| b["id"].as_str().cmp(&a["id"].as_str()));

    let mut body = String::from(RELOAD);
    body.push_str("<nav class=\"pages\">\n");
    for sibling in &config.siblings {
        if let Some(link) = html::safe_link(sibling) {
            body.push_str(&format!(
                "<a href=\"{link}\">{}</a>\n",
                esc(&html::label(sibling))
            ));
        }
    }
    body.push_str("<a href=\"/wiki\">wiki</a>\n<a href=\"/subscribe\">subscribe</a>\n</nav>\n");

    body.push_str(&banner_line(banner));
    // The cache answers this second read, so it spawns nothing.
    if let Some(why) =
        list_fault(&mem.projects(), "projects").or(list_fault(&questions, "questions"))
    {
        body.push_str(&html::degraded_banner(&why));
    }

    body.push_str("<h2>Waiting on you</h2>\n");
    if questions_rows.is_empty() {
        body.push_str("<p class=\"empty\">Nothing waiting.</p>\n");
    }
    for question in &questions_rows {
        body.push_str(&question_block(question, now_ms));
    }

    let waiting = |name: &str| {
        questions_rows
            .iter()
            .filter(|q| q["project"].as_str() == Some(name))
            .count()
    };
    let waits = projects.iter().filter(|p| waiting(&p.name) > 0).count();
    body.push_str("<h2>Projects");
    if waits > 0 {
        body.push_str(&format!(
            " <span class=\"pill wait\">{waits} {} on you</span>",
            if waits == 1 { "waits" } else { "wait" }
        ));
    }
    body.push_str("</h2>\n");
    if projects.is_empty() {
        body.push_str("<p class=\"empty\">No projects registered.</p>\n");
    } else {
        body.push_str("<div class=\"cards\">\n");
        for project in &projects {
            let week = week_line(mem, &project.name);
            body.push_str(&project_card(
                project,
                waiting(&project.name),
                week.as_deref(),
                config,
                machine,
                now_ms,
            ));
        }
        body.push_str("</div>\n");
    }

    page_shell(machine, None, &body)
}

/// What the project's sessions of the last seven days spent, or `None` when
/// it has no cost line in them. A `milestone` line sums sessions already
/// counted, so the week leaves it out.
fn week_line(mem: &MemCli, project: &str) -> Option<String> {
    let rows: Vec<_> = mem
        .run_lines_since(project, "7d")
        .rows("items")
        .iter()
        .filter_map(|item| item["title"].as_str().and_then(parse_cost))
        .map(|(_, row)| row)
        .filter(|row| row.kind != "milestone")
        .collect();
    if rows.is_empty() {
        return None;
    }
    let total = |field: fn(&crate::cost::CostRow) -> u64| rows.iter().map(field).sum();
    Some(format!(
        "week: {}",
        summary(
            rows.len() as u64,
            total(|r| r.minutes),
            total(|r| r.input),
            total(|r| r.output),
        )
    ))
}

/// One project as a card: its name, its stage and whatever waits on the
/// owner as pills, the runner and the open milestone, then a bar filled by
/// the plan's ticks, or the roadmap's when there is no plan.
fn project_card(
    project: &ProjectSummary,
    waiting: usize,
    week: Option<&str>,
    config: &Config,
    machine: &str,
    now_ms: i64,
) -> String {
    let mut pills = format!(
        "<span class=\"{}\">{}</span>",
        stage_pill(project.stage),
        project.stage
    );
    if project.paused.is_some() {
        pills.push_str("<span class=\"pill wait\">paused</span>");
    }
    if waiting > 0 {
        let s = if waiting == 1 { "" } else { "s" };
        pills.push_str(&format!(
            "<span class=\"pill wait\">{waiting} question{s}</span>"
        ));
    }
    let mut parts = Vec::new();
    if let Some(runner) = &project.runner {
        parts.push(runner_html(runner, config, machine));
    }
    if let Some(milestone) = &project.milestone {
        parts.push(esc(milestone));
    }
    let (done, total) = project.milestones;
    if total > 0 {
        // The open milestone's place; the last one once every milestone is
        // ticked.
        parts.push(format!("milestone {} of {total}", (done + 1).min(total)));
    }
    let (ticked, tasks) = project.tasks;
    if tasks > 0 {
        parts.push(format!("tasks {ticked} of {tasks}"));
    }
    let meta = if parts.is_empty() {
        String::new()
    } else {
        format!("<div class=\"meta\">{}</div>\n", parts.join(" · "))
    };
    let bar = match (tasks, total) {
        (0, 0) => String::new(),
        (0, total) => bar(done, total),
        (tasks, _) => bar(ticked, tasks),
    };
    let age = project
        .last_activity
        .as_deref()
        .and_then(|at| at.parse::<jiff::Timestamp>().ok())
        .map(|at| model::age(Some(at.as_millisecond()), now_ms))
        .map(|age| format!("<span class=\"meta\">{}</span>", esc(&age)))
        .unwrap_or_default();
    let week = week
        .map(|week| format!("<div class=\"meta\">{}</div>\n", esc(week)))
        .unwrap_or_default();
    format!(
        "<article class=\"card\">\n\
         <div class=\"row\"><h3><a href=\"{href}\">{name}</a></h3>{pills}\
         <span class=\"sp\"></span>{age}</div>\n\
         {meta}{bar}{week}</article>\n",
        href = esc(&html::project_url(&project.name)),
        name = esc(&project.name),
    )
}

/// The pill's class for a stage: the accent while a milestone is being built
/// or walked, amber while a roadmap waits on approval, green once shipped,
/// grey before there is a roadmap.
pub fn stage_pill(stage: &str) -> &'static str {
    match stage {
        "execution" | "dogfooding" => "pill",
        "planning" => "pill wait",
        "maintenance" => "pill ok",
        _ => "pill mut",
    }
}

/// A progress bar filled to `done` of `total`, which is never zero here.
pub fn bar(done: u64, total: u64) -> String {
    format!(
        "<div class=\"bar\"><i style=\"width:{}%\"></i></div>\n",
        done.min(total) * 100 / total
    )
}

/// The runner's name, linked to its own hub when it is another machine that
/// this hub knows as a sibling.
fn runner_html(runner: &str, config: &Config, machine: &str) -> String {
    let link = (runner != machine)
        .then(|| sibling_hub(config, runner))
        .flatten()
        .and_then(|url| html::safe_link(&url));
    match link {
        Some(link) => format!("<a href=\"{link}\">{}</a>", esc(runner)),
        None => esc(runner),
    }
}

/// One waiting question and its answer form. The text is the row's whole
/// body: the title is only a batched question's first line.
fn question_block(row: &Value, now_ms: i64) -> String {
    let id = text(row, "id").unwrap_or_default();
    let mut meta = Vec::new();
    if let Some(project) = text(row, "project") {
        meta.push(esc(&project));
    }
    meta.push(esc(&model::age(model::ulid_millis(&id), now_ms)));
    meta.push(format!(
        "#{}",
        esc(&text(row, "short_id").unwrap_or_default())
    ));
    let question = text(row, "body")
        .filter(|body| !body.trim().is_empty())
        .or_else(|| text(row, "title"))
        .unwrap_or_default();
    format!(
        "<article>\n\
         <div class=\"meta\">{meta}</div>\n\
         <p class=\"q\">{question}</p>\n\
         {rec}\
         <form method=\"post\" action=\"/answer\">\n\
         <input type=\"hidden\" name=\"id\" value=\"{id}\">\n\
         <textarea name=\"text\" rows=\"3\" placeholder=\"answer\" \
         aria-label=\"answer\"></textarea>\n\
         <button type=\"submit\">Answer</button>\n\
         </form>\n\
         </article>\n",
        meta = meta.join(" · "),
        question = esc(question.trim()),
        rec = rec_line(row),
        id = esc(&id),
    )
}

/// The options and the asker's pick on one line, so the answer can be typed
/// from what is already on the page. Nothing when the question has neither.
fn rec_line(row: &Value) -> String {
    let mut parts: Vec<String> = row["options"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if let Some(pick) = text(row, "recommend") {
        parts.push(format!("recommended: {pick}"));
    }
    if parts.is_empty() {
        return String::new();
    }
    format!("<p class=\"rec\">{}</p>\n", esc(&parts.join(" · ")))
}

/// What the last `POST /answer` did. The text is chosen here from the code
/// in the query, so nothing from the query string reaches the page.
fn banner_line(banner: &Banner) -> String {
    let (kind, text) = match banner {
        Banner::None => return String::new(),
        Banner::Answered(id) => ("ok", format!("Answered #{}.", esc(id))),
        Banner::Unknown => (
            "warn",
            "That id is not in the pending queue. It may already be answered.".to_string(),
        ),
        Banner::Empty => ("warn", "Type an answer first.".to_string()),
        Banner::Failed => (
            "warn",
            "mem could not record that answer. Nothing was written.".to_string(),
        ),
    };
    format!("<p class=\"banner {kind}\">{text}</p>\n")
}

fn text(row: &Value, key: &str) -> Option<String> {
    row[key].as_str().map(str::to_string)
}

fn count(row: &Value, key: &str) -> u64 {
    row[key].as_u64().unwrap_or(0)
}
