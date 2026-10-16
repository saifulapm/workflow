//! `GET /p/<project>`: one project's front page, and its plan and item
//! routes.
//!
//! The front page follows the project's stage: a header with the stage and
//! the progress, then what that stage is about. It costs at most five `mem`
//! spawns: the projects list the route already read, then at most four reads
//! for the body.

use serde_json::Value;

use crate::form::encode_component;
use crate::host::{CLOUD_MACHINE, Command, Fleet};
use crate::html::{
    self, degraded_banner, esc, item_list_section, markdown_article, outside_link, page_url,
    project_url,
};
use crate::http::Response;
use crate::live::{self, CloudRun, CloudState, Run};
use crate::mem::{Mem, Outcome, Source};
use crate::model;
use crate::page_decisions::{ruling_block, ruling_rows};
use crate::page_evidence::{FindingRow, finding_rows, findings_section};
use crate::page_home::{ProjectSummary, project_summaries, stage_pill};
use crate::page_questions::question_rows;
use crate::page_roadmap::{roadmap_body, roadmap_rows};
use crate::pages::{PageCtx, page_shell};

/// How many rulings a grilling project shows; the decisions page has them
/// all.
const RULINGS_SHOWN: usize = 10;

pub fn get(ctx: &PageCtx) -> Response {
    if !ctx.rest.is_empty() {
        let path = ctx.request.path.strip_prefix("/p/").unwrap_or_default();
        return project_page(ctx.site.mem, path);
    }
    let project = ctx.project.unwrap_or_default();
    let Some(summary) = project_summaries(ctx.site.mem)
        .into_iter()
        .find(|s| s.name == project)
    else {
        return Response::not_found();
    };
    let mut body = stage_header(&summary);
    let running = matches!(summary.stage, "execution" | "dogfooding");
    if running {
        body.push_str(&orchestrator_line(ctx, &summary));
        body.push_str(&cloud_section(ctx, &summary));
    }
    if summary.stage == "execution" {
        body.push_str(&run_summary(ctx, project));
    }
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
        "execution" => building_body(ctx, project),
        // done
        _ => backlog_body(ctx, project),
    });
    Response::html(page_shell(summary.stage, ctx.project, &body))
}

/// The stage, then the open milestone's place and the
/// current plan's ticks, as the front page lists them.
pub fn stage_header(s: &ProjectSummary) -> String {
    let pills = format!(
        "<span class=\"{}\">{}</span>",
        stage_pill(s.stage),
        esc(s.stage)
    );
    let mut parts = Vec::new();
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
    let roadmap = ctx.site.mem.roadmap(project);
    let text = match &*roadmap {
        Outcome::Broken(why) => return degraded_banner(why),
        Outcome::Json(doc) => doc["text"].as_str().unwrap_or_default(),
        Outcome::Absent => return String::new(),
    };
    let plans: Vec<String> = ctx
        .site
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
                1 => (" class=\"live\"", live_pill(s, &row.slug, project)),
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

/// The live milestone's ticks, linked to the current plan, when that plan is
/// its own.
fn live_pill(s: &ProjectSummary, slug: &str, project: &str) -> String {
    let (ticked, total) = s.tasks;
    if total > 0 && s.plan_slug.as_deref() == Some(slug) {
        format!(
            "<a href=\"{}/plan\"><span class=\"pill\">{ticked} of {total} tasks</span></a>",
            esc(&project_url(project))
        )
    } else {
        "<span class=\"pill\">live</span>".to_string()
    }
}

/// What runs the current milestone, from the runner and its machine's last
/// report: running, away, or why nothing does, stalled or parked on a
/// question, with a button for each machine that may start it. The handoff
/// is read only when nothing runs it, so a running project's page costs no
/// extra read for it.
fn orchestrator_line(ctx: &PageCtx, s: &ProjectSummary) -> String {
    let project = s.name.as_str();
    let agents = ctx.site.host.fleet();
    let paused = s.paused.is_some();
    let paused_pill = if paused {
        "<span class=\"pill wait\">paused</span> "
    } else {
        ""
    };
    // No agent reports a cloud session, so its runner and handoff tell it.
    if let Some(cloud) = cloud_run(ctx, s) {
        let (pill, state) = match cloud.state {
            CloudState::Live => ("<span class=\"pill\">running</span>", "live"),
            CloudState::Parked => ("<span class=\"pill wait\">parked</span>", "parked"),
            CloudState::Landed => ("<span class=\"pill ok\">landed</span>", "landed"),
            CloudState::Ended => ("<span class=\"pill mut\">ended</span>", "ended"),
        };
        let mut forms = String::new();
        // Answers for the session wait in the mini's queue, or will, and
        // only the mini can hand them over.
        let waiting = cloud.state == CloudState::Parked
            || agents
                .pending(CLOUD_MACHINE)
                .iter()
                .any(|c| matches!(c, Command::Cloud { project: p, .. } if p == project));
        if waiting && !agents.online(CLOUD_MACHINE) {
            forms.push_str(&format!(
                "<p class=\"banner\">{CLOUD_MACHINE} is off: reply in the Claude app to resume now</p>\n\
                 <form method=\"post\" action=\"{}/control\">\n\
                 <input type=\"hidden\" name=\"do\" value=\"reply-in-app\">\n\
                 <button type=\"submit\">Reply in the Claude app</button>\n\
                 </form>\n",
                esc(&project_url(project))
            ));
        }
        let line = format!(
            "{pill} <span class=\"meta\">{}</span> {}",
            esc(&format!("cloud · {} · {state}", cloud.milestone)),
            outside_link(&format!("{CODE_PAGE}/{}", cloud.id), "session"),
        );
        // A landed or ended run holds the project no more: `workflow go` on
        // a device takes it on from the cloud branch, as with no runner.
        if matches!(cloud.state, CloudState::Landed | CloudState::Ended) {
            let free = ProjectSummary {
                runner: None,
                ..s.clone()
            };
            let device = device_line(ctx, &free);
            if !device.is_empty() {
                let banner = if forms.is_empty() {
                    String::new()
                } else {
                    controls(ctx, forms)
                };
                return format!("<p class=\"row\">{line}</p>\n{banner}{device}");
            }
        }
        forms.push_str(&pause_form(project, paused));
        return format!(
            "<p class=\"row\">{paused_pill}{line}</p>\n{}",
            controls(ctx, forms)
        );
    }
    device_line(ctx, s)
}

/// The detail routes under `GET /p/<project>/`: everything after the
/// project is checked against one of the known shapes — `plan`,
/// `plan/<slug>`, `item/<id>` — before it reaches an argv.
/// Anything else is a 404, as `/wiki/` already does.
pub fn project_page(mem: &dyn Source, rest: &str) -> Response {
    let Some((project, sub)) = rest.split_once('/') else {
        return Response::not_found();
    };
    if project.is_empty() || !model::is_known_project(mem, project) {
        return Response::not_found();
    }
    match sub {
        "plan" => project_plan(mem, project),
        sub => {
            if let Some(slug) = sub.strip_prefix("plan/") {
                project_plan_slug(mem, project, slug)
            } else if let Some(id) = sub.strip_prefix("item/") {
                project_item(mem, project, id)
            } else {
                Response::not_found()
            }
        }
    }
}

/// `GET /p/<project>/plan` — the current plan's whole text, from one
/// read. A project with no current plan still gets the page, empty.
fn project_plan(mem: &dyn Source, project: &str) -> Response {
    let plan = model::plan_text(mem, project);
    Response::html(html::plan_page(
        project,
        None,
        plan.value.as_deref(),
        plan.degraded.as_deref(),
    ))
}

/// `GET /p/<project>/plan/<slug>` — one stored plan, whole. A broken mem
/// renders degraded; a slug mem simply does not have is a 404.
fn project_plan_slug(mem: &dyn Source, project: &str, slug: &str) -> Response {
    if !model::is_slug(slug) {
        return Response::not_found();
    }
    let plan = model::plan_slug_text(mem, project, slug);
    if plan.value.is_none() && plan.degraded.is_none() {
        return Response::not_found();
    }
    Response::html(html::plan_page(
        project,
        Some(slug),
        plan.value.as_deref(),
        plan.degraded.as_deref(),
    ))
}

/// `GET /p/<project>/item/<id>` — one item, whole. `mem show` has no
/// `--project`, so an id that resolves under another project 404s here
/// rather than leaking that project's item.
fn project_item(mem: &dyn Source, project: &str, id: &str) -> Response {
    if !model::is_item_id(id) {
        return Response::not_found();
    }
    let item = model::item_detail(mem, id);
    match &item.value {
        Some(detail) if detail.project.as_deref() == Some(project) => Response::html(
            html::item_page(project, Some(detail), item.degraded.as_deref()),
        ),
        Some(_) => Response::not_found(),
        None if item.degraded.is_some() => {
            Response::html(html::item_page(project, None, item.degraded.as_deref()))
        }
        None => Response::not_found(),
    }
}

/// A device runner's line, or with no runner the Start buttons.
fn device_line(ctx: &PageCtx, s: &ProjectSummary) -> String {
    let project = s.name.as_str();
    let agents = ctx.site.host.fleet();
    let run = live::run(s.runner.as_ref(), &s.name, s.milestone.as_deref(), agents);
    let paused = s.paused.is_some();
    let paused_pill = if paused {
        "<span class=\"pill wait\">paused</span> "
    } else {
        ""
    };
    let (pill, meta) = match &run {
        Run::Live(agent) => (
            if paused {
                "<span class=\"pill wait\">pausing</span>"
            } else {
                "<span class=\"pill\">running</span>"
            },
            format!(
                "{} · {}",
                agent.id,
                agent.state.as_deref().unwrap_or("unknown")
            ),
        ),
        // Its machine may still be running it, so nothing is offered.
        Run::Away(machine) => (
            "<span class=\"pill wait\">away</span>",
            live::away(machine, project, agents),
        ),
        _ if live::idle(s, &run) => {
            let handoff = match &*ctx.site.mem.handoff(project) {
                Outcome::Json(doc) => doc["body"].as_str().unwrap_or_default().to_string(),
                _ => String::new(),
            };
            let milestone = s.milestone.as_deref().unwrap_or_default();
            let parked = handoff.trim_start().starts_with("parked");
            // `workflow go` refuses a paused project unless its run parked,
            // so only a parked one keeps its Resume.
            if paused && !parked {
                return format!(
                    "<p class=\"row\">{paused_pill}<span class=\"meta\">{}</span></p>\n{}",
                    esc(s.paused.as_deref().unwrap_or_default()),
                    controls(ctx, pause_form(project, paused))
                );
            }
            let machines = live::start_machines(s, &run, parked, agents);
            // No runner and no machine that could take it: nothing to say.
            if run == Run::Unrun && machines.is_empty() {
                return String::new();
            }
            let (pill, meta) = if live::stalled(s, &run, &handoff) {
                (
                    "<span class=\"pill bad\">stalled</span>",
                    format!("no live orchestrator for {milestone}"),
                )
            } else if parked {
                (
                    "<span class=\"pill wait\">parked</span>",
                    format!("{milestone} is waiting on you"),
                )
            } else {
                (
                    "<span class=\"pill mut\">idle</span>",
                    format!("nothing runs {milestone}"),
                )
            };
            // A project nothing has handed off yet has never run.
            let label = if handoff.trim().is_empty() {
                "Start"
            } else {
                "Resume"
            };
            let mut forms: String = machines
                .iter()
                .map(|machine| {
                    // Without a runner the button names the machine it picks.
                    let label = if s.runner.is_some() {
                        label.to_string()
                    } else {
                        format!("{label} on {machine}")
                    };
                    format!(
                        "<form method=\"post\" action=\"{}/control\">\n\
                         <input type=\"hidden\" name=\"do\" value=\"go\">\n\
                         <input type=\"hidden\" name=\"machine\" value=\"{}\">\n\
                         <button type=\"submit\">{}</button>\n\
                         </form>\n",
                        esc(&project_url(project)),
                        esc(machine),
                        esc(&label)
                    )
                })
                .collect();
            forms.push_str(&pause_form(project, paused));
            return format!(
                "<p class=\"row\">{paused_pill}{pill} <span class=\"meta\">{}</span></p>\n{}",
                esc(&meta),
                controls(ctx, forms)
            );
        }
        _ => return String::new(),
    };
    format!(
        "<p class=\"row\">{pill} <span class=\"meta\">{}</span></p>\n{}",
        esc(&meta),
        controls(ctx, pause_form(project, paused))
    )
}

/// `s`'s cloud run, its handoff read only when one was written since it
/// began.
pub fn cloud_run(ctx: &PageCtx, s: &ProjectSummary) -> Option<CloudRun> {
    let handoff = || match &*ctx.site.mem.handoff(&s.name) {
        Outcome::Json(doc) => doc["body"].as_str().map(str::to_string),
        _ => None,
    };
    live::cloud_run(s, handoff, ctx.site.now).map(|run| live::heard(run, ctx.site.host.fleet()))
}

/// The project's cloud run that holds the one cloud slot, live or parked,
/// with its project's name. A handoff is read only for a cloud runner that
/// wrote one since it began.
pub fn cloud_slot(
    mem: &dyn Source,
    summaries: &[ProjectSummary],
    now: jiff::Timestamp,
) -> Option<(String, CloudRun)> {
    summaries.iter().find_map(|s| {
        let handoff = || match &*mem.handoff(&s.name) {
            Outcome::Json(doc) => doc["body"].as_str().map(str::to_string),
            _ => None,
        };
        live::cloud_run(s, handoff, now)
            .filter(|run| matches!(run.state, CloudState::Live | CloudState::Parked))
            .map(|run| (s.name.clone(), run))
    })
}

/// Why `s` may not start a cloud run, or `None` when it may: `workflow go
/// --cloud` wants an approved milestone, no pause, a git remote, the trunk
/// on main, no device orchestrator on it, and the slot free.
pub fn cloud_refusal(
    mem: &dyn Source,
    agents: &dyn Fleet,
    summaries: &[ProjectSummary],
    s: &ProjectSummary,
    now: jiff::Timestamp,
) -> Option<String> {
    let (done, total) = s.milestones;
    if s.roadmap_status.as_deref() != Some("approved") || done >= total || s.milestone.is_none() {
        return Some("There is no approved milestone to start.".to_string());
    }
    if s.paused.is_some() {
        return Some(format!("{} is paused.", s.name));
    }
    let row = mem
        .projects()
        .rows("projects")
        .into_iter()
        .find(|row| row["name"] == s.name.as_str())
        .unwrap_or_default();
    // The cloud session clones the remote and cuts from main.
    if row["remote"].as_str().is_none_or(str::is_empty) {
        return Some(format!("{} has no git remote", s.name));
    }
    // Its trunk is a branch the cloud cannot see yet. The mini knows, since
    // mem keeps a worktree per machine; this store knows only its own.
    let reported = agents
        .report(CLOUD_MACHINE)
        .is_some_and(|report| report.worktrees.contains(&s.name));
    if reported || row["worktree"].is_string() {
        return Some(format!(
            "{} works on a branch of its own; cloud runs take main",
            s.name
        ));
    }
    if let Run::Live(agent) = live::run(s.runner.as_ref(), &s.name, s.milestone.as_deref(), agents)
    {
        return Some(format!("{} is already running.", agent.id));
    }
    cloud_slot(mem, summaries, now)
        .map(|(project, run)| {
            format!(
                "a cloud run is live: {project} ({}, {}); one runs at a time.",
                run.milestone, run.id
            )
        })
        .or_else(|| unpushed(agents, &s.name))
}

/// Why a cloud run of `project` must wait for Saiful to push main: the
/// mini's main holds commits origin/main lacks, and the run clones origin.
fn unpushed(agents: &dyn Fleet, project: &str) -> Option<String> {
    let report = agents.report(CLOUD_MACHINE)?;
    let n = *report.unpushed.get(project).filter(|n| **n > 0)?;
    let commits = if n == 1 { "commit" } else { "commits" };
    Some(format!(
        "{project}'s main is {n} {commits} ahead of origin/main on {CLOUD_MACHINE}; \
push main first: a cloud run starts from origin"
    ))
}

/// The prompt a cloud run starts on, the file `workflow go --cloud` renders,
/// so the card and the command stay one text.
const CLOUD_PROMPT: &str = include_str!("../../workflow/src/cloud-prompt.txt");

/// claude.ai's Code page, where a cloud session lives. Built with `concat!`,
/// as workflow builds it, so the hygiene rule still catches a pasted link.
pub const CODE_PAGE: &str = concat!("https://claude.ai", "/code");

/// The branch a cloud run of `slug` works on.
fn cloud_branch(project: &str, slug: &str) -> String {
    format!("cloud/{project}-{slug}")
}

/// What the next cloud run is cut from: the branch of the cloud run that
/// landed last, which main may not hold yet, or main.
fn cloud_base(s: &ProjectSummary) -> String {
    match &s.runner {
        Some(runner) if runner.machine == "cloud" => match &runner.milestone {
            Some(done) if s.milestone.as_ref() != Some(done) => cloud_branch(&s.name, done),
            _ => "main".to_string(),
        },
        _ => "main".to_string(),
    }
}

/// The prompt for `slug` of `project`, its branch cut from `base`, filled in
/// as workflow fills it.
fn cloud_prompt(project: &str, slug: &str, base: &str) -> String {
    CLOUD_PROMPT
        .trim_end()
        .replace("{branch}", &cloud_branch(project, slug))
        .replace("{project}", project)
        .replace("{slug}", slug)
        .replace("{base}", base)
}

/// The way into a cloud run: Run in cloud, which the mini starts, while the
/// mini is online, and the Start in cloud card for starting one by hand in
/// the Claude app.
fn cloud_section(ctx: &PageCtx, s: &ProjectSummary) -> String {
    let summaries = project_summaries(ctx.site.mem);
    let site = ctx.site;
    if let Some(why) = cloud_refusal(site.mem, site.host.fleet(), &summaries, s, site.now) {
        // Only a missing push gets a card: it is the one Saiful can clear.
        if unpushed(ctx.site.host.fleet(), &s.name).is_some_and(|wait| wait == why) {
            return format!(
                "<h2>Cloud run</h2>\n<div class=\"card\">\n<p>{}</p>\n</div>\n",
                esc(&why)
            );
        }
        return String::new();
    }
    let slug = s.milestone.as_deref().unwrap_or_default();
    let remote = ctx
        .site
        .mem
        .projects()
        .rows("projects")
        .into_iter()
        .find(|row| row["name"] == s.name.as_str())
        .and_then(|row| row["remote"].as_str().map(str::to_string))
        .unwrap_or_default();
    let prompt = cloud_prompt(&s.name, slug, &cloud_base(s));
    let mut out = format!(
        "<h2>Cloud run</h2>\n<div class=\"card\">\n\
         <p class=\"meta\">{} · {} · Environment: workflow</p>\n\
         <textarea id=\"cloud-prompt\" rows=\"8\" readonly>{}</textarea>\n\
         <p><button type=\"button\" onclick=\"navigator.clipboard.writeText(\
         document.getElementById('cloud-prompt').value)\">Copy prompt</button> {}</p>\n\
         </div>\n",
        esc(slug),
        esc(&remote),
        esc(&prompt),
        outside_link(CODE_PAGE, "Open Claude"),
    );
    // The mini first, else another online device with the project; one
    // already queued still shows the button, which then adds nothing.
    if crate::page_control::cloud_machine(ctx.site.host.fleet(), &s.name).is_ok() {
        out.push_str(&controls(
            ctx,
            format!(
                "<form method=\"post\" action=\"{}/control\">\n\
                 <input type=\"hidden\" name=\"do\" value=\"cloud\">\n\
                 <button type=\"submit\">Run in cloud</button>\n\
                 </form>\n",
                esc(&project_url(&s.name))
            ),
        ));
    }
    out
}

/// The forms, or for a browser that never paired the line saying how.
fn controls(ctx: &PageCtx, forms: String) -> String {
    ctx.pair_notice().unwrap_or(forms)
}

/// Pause holds the project once its current milestone lands; Unpause lets
/// the next one start again. Neither starts or stops a run.
fn pause_form(project: &str, paused: bool) -> String {
    let (verb, label) = if paused {
        ("unpause", "Unpause")
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

/// Under the header of a running project: the questions waiting on the owner,
/// and links to the pages that hold them.
pub fn run_summary(ctx: &PageCtx, project: &str) -> String {
    let mut out = String::from("<h2>Run</h2>\n");

    // Every project's pending questions, which the doorbell's poll keeps
    // cached, rather than a read of this project's own.
    let questions = ctx.site.mem.questions();
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

/// The brief, or the way to write one when there is none.
fn brief_body(ctx: &PageCtx, project: &str) -> String {
    let outcome = ctx
        .site
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
        "<h2>Brief</h2>\n{}",
        markdown_article(Some(brief), project, "")
    )
}

/// The research pages written so far.
fn research_body(ctx: &PageCtx, project: &str) -> String {
    let outcome = ctx.site.mem.wiki(project);
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
    let rulings = ctx.site.mem.items(project, "ruling", RULINGS_SHOWN);
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
    let questions = ctx.site.mem.read(&[
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
    let outcome = ctx.site.mem.read(&[
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

/// The last handoff, under its heading.
fn handoff_section(ctx: &PageCtx, project: &str) -> String {
    let mut out = String::from("<h2>Handoff</h2>\n");
    match &*ctx.site.mem.handoff(project) {
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

/// The last moves and the last handoff, for a building project.
fn building_body(ctx: &PageCtx, project: &str) -> String {
    let moves = model::last_moves(ctx.site.mem, project, ctx.site.now.as_millisecond());
    let mut out = String::new();
    if let Some(why) = &moves.degraded {
        out.push_str(&degraded_banner(why));
    }
    out.push_str(&item_list_section("Last moves", &moves.rows, project));
    out.push_str(&handoff_section(ctx, project));
    out
}

/// The open milestone's Show path as numbered steps, each marked by the
/// milestone's open findings, then those findings and the way to file one.
fn walk_body(ctx: &PageCtx, project: &str) -> String {
    let roadmap = ctx.site.mem.roadmap(project);
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
        out.push_str("<ul>\n");
        for (i, step) in steps.iter().enumerate() {
            let n = i + 1;
            out.push_str(&format!(
                "<li>{} {n}. {}</li>\n",
                step_mark(n, &findings),
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

/// A Show line as its steps, in order; step `n` is index `n - 1`, the
/// number a finding names with `mem finding add --step`.
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

/// A cross where an open finding names step `n`, a dash otherwise.
pub fn step_mark(n: usize, findings: &[FindingRow]) -> &'static str {
    if findings.iter().any(|f| f.step.trim() == n.to_string()) {
        "✗"
    } else {
        "-"
    }
}

/// The project's open findings in mem's order; a fault leaves a banner in
/// `out` and no rows.
fn open_findings(ctx: &PageCtx, project: &str, out: &mut String) -> Vec<FindingRow> {
    let outcome = ctx.site.mem.read(&[
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

/// The last handoff, the open findings and the ideas, with the way to file
/// an idea, for a project whose roadmap is done.
fn backlog_body(ctx: &PageCtx, project: &str) -> String {
    let mut out = handoff_section(ctx, project);
    let findings = open_findings(ctx, project, &mut out);
    out.push_str(&findings_section(&findings, project));

    out.push_str("<h2>Ideas</h2>\n");
    let ideas = ctx.site.mem.read(&[
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

    out.push_str(&format!(
        "<p><a href=\"{}/new\">File an idea</a></p>\n",
        esc(&project_url(project))
    ));
    out
}
