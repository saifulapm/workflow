//! `GET /`: every project with its stage and progress, then the questions
//! waiting on the owner.
//!
//! The hub runs one `mem` at a time, so this page reads nothing per question
//! and nothing per project: `projects --json` carries each project's summary
//! and `questions --json` each question's whole body.

use serde_json::Value;

use crate::agents::Agents;
use crate::config::Config;
use crate::html::{self, Banner, esc};
use crate::http::Response;
use crate::live::{self, Run, Runner};
use crate::memcli::{MemCli, Outcome};
use crate::model::{self, list_fault};
use crate::page_questions::{pending_block, question_row};
use crate::pages::{PageCtx, page_shell};

/// What keeps the page live: every 15 s it asks `/api/stamp` whether what
/// home shows has changed, and reloads only then, and only while no field
/// has focus or text, and no text is selected, so a reload never moves the
/// page from under an answer being written or a word being picked. `{stamp}`
/// is the stamp this page was drawn from.
fn reload_script(stamp: &str) -> String {
    format!(
        "<script>(function(){{var s=\"{stamp}\";setInterval(function(){{\
         fetch('/api/stamp').then(function(r){{return r.json();}}).then(function(d){{\
         if(d.stamp===s)return;\
         var a=document.activeElement;\
         if(a&&/^(TEXTAREA|INPUT|SELECT)$/.test(a.tagName))return;\
         if(String(window.getSelection())!=='')return;\
         var f=document.querySelectorAll('textarea,input:not([type=hidden])');\
         for(var i=0;i<f.length;i++){{if(f[i].value)return;}}\
         location.reload();}}).catch(function(){{}});}},15000);}})();</script>\n"
    )
}

/// A stamp of what home draws: its two reads, hashed. The ages on the page
/// move with the clock, not with the reads, so they leave it alone.
pub fn stamp(mem: &MemCli) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for read in [mem.projects(), mem.questions()] {
        match &*read {
            Outcome::Json(doc) => doc.to_string().hash(&mut hasher),
            Outcome::Absent => "absent".hash(&mut hasher),
            Outcome::Broken(why) => why.hash(&mut hasher),
        }
    }
    format!("{:016x}", hasher.finish())
}

/// One project's row on the front page.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectSummary {
    pub name: String,
    pub stage: &'static str,
    pub roadmap_status: Option<String>,
    /// Whether this machine has a checkout of it, which `workflow go`
    /// needs.
    pub checked_out: bool,
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
    /// When and by whom the project was paused, while it is.
    pub paused: Option<String>,
    /// The machine and amx id `workflow go` last started it with.
    pub runner: Option<Runner>,
}

pub fn get(ctx: &PageCtx) -> Response {
    let app = ctx.app;
    let banner = Banner::from_query(&ctx.request.query);
    let now_ms = jiff::Timestamp::now().as_millisecond();
    Response::html(render(
        &app.mem,
        &app.agents,
        &app.config,
        &app.machine,
        now_ms,
        &banner,
        ctx.pair_notice().as_deref(),
    ))
}

/// Every project, from the one `projects --json` read.
pub fn project_summaries(mem: &MemCli) -> Vec<ProjectSummary> {
    mem.projects()
        .rows("projects")
        .iter()
        .map(summary_of)
        .collect()
}

/// One `projects --json` row.
pub fn summary_of(row: &Value) -> ProjectSummary {
    ProjectSummary {
        name: text(row, "name").unwrap_or_default(),
        stage: lifecycle_stage(row),
        roadmap_status: text(row, "roadmap_status"),
        checked_out: row["checkouts"]
            .as_array()
            .is_some_and(|dirs| !dirs.is_empty()),
        milestone: text(row, "milestone"),
        plan_slug: text(row, "plan_slug"),
        milestones: (
            count(row, "milestones_done"),
            count(row, "milestones_total"),
        ),
        tasks: (count(row, "plan_ticked"), count(row, "plan_total")),
        last_activity: text(row, "last_activity"),
        paused: text(row, "paused"),
        runner: serde_json::from_value(row["runner"].clone()).ok(),
    }
}

/// The lifecycle stage of one `projects --json` row. mem stores no stage, so
/// the hub works it out from what the summary holds. A roadmap with every
/// milestone ticked is done whatever status it stores, so a finished one
/// never reads as a milestone still being built.
pub fn lifecycle_stage(row: &Value) -> &'static str {
    let total = count(row, "milestones_total");
    if total > 0 && count(row, "milestones_done") == total {
        return "done";
    }
    match row["roadmap_status"].as_str() {
        Some("draft") => "planning",
        Some("approved") => {
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
        // Statuses an older roadmap may still store.
        Some("maintenance" | "done") => "done",
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
    agents: &Agents,
    config: &Config,
    machine: &str,
    now_ms: i64,
    banner: &Banner,
    pair_notice: Option<&str>,
) -> String {
    let projects = project_summaries(mem);
    let questions = mem.questions();
    let mut questions_rows = questions.rows("questions");
    questions_rows.sort_by(|a, b| b["id"].as_str().cmp(&a["id"].as_str()));

    let mut body = reload_script(&stamp(mem));
    body.push_str("<nav class=\"pages\">\n");
    for sibling in &config.siblings {
        if html::safe_link(sibling).is_some() {
            body.push_str(&html::outside_link(sibling, &esc(&html::label(sibling))));
            body.push('\n');
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

    // Each project's run, from its runner and that machine's report.
    let runs: Vec<Run> = projects
        .iter()
        .map(|p| live::run(p.runner.as_ref(), &p.name, p.milestone.as_deref(), agents))
        .collect();
    body.push_str("<h2>Running</h2>\n");
    let running: Vec<String> = projects
        .iter()
        .zip(&runs)
        .filter(|(_, run)| matches!(run, Run::Live(_)))
        .map(|(project, _)| running_line(project))
        .collect();
    if running.is_empty() {
        body.push_str("<p class=\"empty\">Nothing running.</p>\n");
    } else {
        body.push_str(&format!("<ul>\n{}</ul>\n", running.concat()));
    }

    body.push_str("<h2>Waiting on you</h2>\n");
    if questions_rows.is_empty() {
        body.push_str("<p class=\"empty\">Nothing waiting.</p>\n");
    }
    for question in &questions_rows {
        body.push_str(&question_block(question, now_ms, pair_notice));
    }

    let waiting = |name: &str| {
        questions_rows
            .iter()
            .filter(|q| q["project"].as_str() == Some(name))
            .count()
    };
    // A live project is listed under Running only.
    let projects: Vec<(&ProjectSummary, &Run)> = projects
        .iter()
        .zip(&runs)
        .filter(|(_, run)| !matches!(run, Run::Live(_)))
        .collect();
    let waits = projects
        .iter()
        .filter(|(p, _)| waiting(&p.name) > 0)
        .count();
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
        for (project, run) in &projects {
            let away = match run {
                Run::Away(machine) => Some(live::away(machine, &project.name, agents)),
                _ => None,
            };
            body.push_str(&project_card(
                project,
                waiting(&project.name),
                away.as_deref(),
                now_ms,
            ));
        }
        body.push_str("</div>\n");
    }

    page_shell(machine, None, &body)
}

/// One live project under Running: `<project> · <machine> · <milestone> ·
/// since <HH:MM>`, the time in the hub's own zone, linking to its page.
fn running_line(project: &ProjectSummary) -> String {
    let mut parts = vec![project.name.clone()];
    if let Some(runner) = &project.runner {
        parts.push(runner.machine.clone());
    }
    if let Some(milestone) = &project.milestone {
        parts.push(milestone.clone());
    }
    let since = project
        .runner
        .as_ref()
        .and_then(|runner| runner.since.as_deref())
        .and_then(|since| since.parse::<jiff::Timestamp>().ok());
    if let Some(since) = since {
        let zoned = since.to_zoned(jiff::tz::TimeZone::system());
        parts.push(format!("since {}", zoned.strftime("%H:%M")));
    }
    format!(
        "<li><a href=\"{}\">{}</a></li>\n",
        esc(&html::project_url(&project.name)),
        esc(&parts.join(" · "))
    )
}

/// One project as a card: its name, its stage, whatever waits on the owner
/// and an away runner as pills, the open milestone, then a bar filled by
/// the plan's ticks, or the roadmap's when there is no plan.
fn project_card(
    project: &ProjectSummary,
    waiting: usize,
    away: Option<&str>,
    now_ms: i64,
) -> String {
    let mut pills = format!(
        "<span class=\"{}\">{}</span>",
        stage_pill(project.stage),
        project.stage
    );
    if waiting > 0 {
        let s = if waiting == 1 { "" } else { "s" };
        pills.push_str(&format!(
            "<span class=\"pill wait\">{waiting} question{s}</span>"
        ));
    }
    if let Some(away) = away {
        pills.push_str(&format!("<span class=\"pill wait\">{}</span>", esc(away)));
    }
    let mut parts = Vec::new();
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
    format!(
        "<article class=\"card\">\n\
         <div class=\"row\"><h3><a href=\"{href}\">{name}</a></h3>{pills}\
         <span class=\"sp\"></span>{age}</div>\n\
         {meta}{bar}</article>\n",
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
        "done" => "pill ok",
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

/// One waiting question, under its project, age and short id, with the
/// questions page's own option buttons and text box.
fn question_block(row: &Value, now_ms: i64, pair_notice: Option<&str>) -> String {
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
    // No project in the form, so the answer lands back on `/`.
    pending_block(
        &question_row(row),
        None,
        &format!("<div class=\"meta\">{}</div>\n", meta.join(" · ")),
        pair_notice,
    )
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
