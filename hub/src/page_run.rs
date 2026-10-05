//! `GET /p/<project>/run`: the run under way. The newest run's tasks grouped
//! by state, the dispatched ones as live agents, and the run log.

use std::process::Command;
use std::time::Duration;

use serde_json::Value;

use crate::html::{degraded_banner, esc, project_url};
use crate::http::Response;
use crate::memcli::Outcome;
use crate::model::checkout_of;
use crate::pages::{PageCtx, page_shell, sibling_hub};
use crate::proc::{self, Ended};

/// How long `workflow status` may take before it is killed.
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

/// The board's groups, in the order the page shows them: what is moving
/// first, what is done last.
pub const BOARD: [&str; 5] = ["dispatched", "pending", "failed", "blocked", "merged"];

/// How many run lines the log shows.
const RUN_LOG_LIMIT: &str = "30";

pub fn get(ctx: &PageCtx) -> Response {
    Response::html(page_shell("run", ctx.project, &run_body(ctx)))
}

/// `workflow status --json` run once in the checkout at `root`, uncached:
/// the board is only worth reading while it is current. `None` when
/// workflow is missing, slow, fails or prints something that is not JSON.
pub fn engine_status(root: &str) -> Option<Value> {
    let mut command = Command::new("workflow");
    command.args(["status", "--json"]).current_dir(root);
    match proc::output_within(&mut command, STATUS_TIMEOUT) {
        Ended::Exited(done) if done.code == Some(0) => serde_json::from_slice(&done.stdout).ok(),
        _ => None,
    }
}

/// The page under the shell. Three `mem` spawns at most: the projects list
/// the route and the checkout share, the run log, and without a checkout
/// the project's runner. One `workflow` spawn, only with a checkout.
pub fn run_body(ctx: &PageCtx) -> String {
    let project = ctx.project.unwrap_or_default();
    let mut out = match checkout_of(&ctx.app.mem, project) {
        Some(root) => match engine_status(&root) {
            Some(status) => run_section(&status),
            None => format!(
                "<p class=\"banner warn\">workflow status gave no answer in {}</p>\n",
                esc(&root)
            ),
        },
        None => elsewhere(ctx, project),
    };
    out.push_str(&run_log(ctx, project));
    out
}

/// A project this machine has no checkout of: which machine runs it, and a
/// link to that machine's hub when it is a sibling.
fn elsewhere(ctx: &PageCtx, project: &str) -> String {
    let current = ctx.app.mem.read(&[
        "project",
        "current",
        &format!("--project={project}"),
        "--json",
    ]);
    let runner = match &*current {
        Outcome::Json(doc) => doc["runner"].as_str().unwrap_or_default().to_string(),
        _ => String::new(),
    };
    if runner.is_empty() {
        return "<p class=\"empty\">No checkout of this project on this machine, and no \
                machine runs it.</p>\n"
            .to_string();
    }
    let mut out = format!(
        "<p class=\"empty\">No checkout of this project on this machine; {} runs it.</p>\n",
        esc(&runner)
    );
    if let Some(hub) = sibling_hub(&ctx.app.config, &runner) {
        let url = format!("{}{}/run", hub.trim_end_matches('/'), project_url(project));
        out.push_str(&format!(
            "<p><a href=\"{}\">the run on {}</a></p>\n",
            esc(&url),
            esc(&runner)
        ));
    }
    out
}

/// The newest run in `status`: the last, as status lists runs oldest
/// first. Its plan and place, the board and the live agents.
fn run_section(status: &Value) -> String {
    let Some(run) = status["runs"].as_array().and_then(|runs| runs.last()) else {
        return "<p class=\"empty\">No run in this checkout yet.</p>\n".to_string();
    };
    let mut head = vec![
        esc(run["plan"].as_str().unwrap_or_default()),
        esc(status["stage"].as_str().unwrap_or_default()),
    ];
    if let (Some(n), Some(m)) = (
        status["milestone"]["n"].as_u64(),
        status["milestone"]["m"].as_u64(),
    ) {
        head.push(format!("milestone {n} of {m}"));
    }
    let mut out = format!("<p class=\"meta\">{}</p>\n", head.join(" · "));

    let tasks: Vec<&Value> = run["tasks"].as_array().into_iter().flatten().collect();
    let in_state = |state: &str| -> Vec<&Value> {
        tasks
            .iter()
            .copied()
            .filter(|t| t["state"] == state)
            .collect()
    };

    out.push_str("<h2>Board</h2>\n");
    for state in BOARD {
        let group = in_state(state);
        if group.is_empty() {
            continue;
        }
        out.push_str(&format!("<h3>{state}</h3>\n<ul>\n"));
        for task in group {
            out.push_str(&format!(
                "<li><strong>{}</strong>{}</li>\n",
                esc(task["id"].as_str().unwrap_or_default()),
                status_line(task)
            ));
        }
        out.push_str("</ul>\n");
    }

    out.push_str("<h2>Agents</h2>\n");
    let agents = in_state("dispatched");
    if agents.is_empty() {
        out.push_str("<p class=\"empty\">No task is out.</p>\n");
        return out;
    }
    out.push_str("<ul>\n");
    for task in agents {
        // Each part is labelled, so a row still reads when status left a
        // field empty.
        let mut parts = Vec::new();
        if let Some(session) = task["session"].as_str().filter(|s| !s.is_empty()) {
            parts.push(esc(session));
        }
        parts.push(match task["model"].as_str().unwrap_or_default() {
            "" => "model -".to_string(),
            model => format!("model {}", esc(model)),
        });
        parts.push(format!("{} min", task["minutes"].as_u64().unwrap_or(0)));
        out.push_str(&format!(
            "<li><strong>{}</strong>\n<div class=\"meta\">{}</div>{}</li>\n",
            esc(task["id"].as_str().unwrap_or_default()),
            parts.join(" · "),
            status_line(task)
        ));
    }
    out.push_str("</ul>\n");
    out
}

/// A task's last status line under it, or nothing when it has none.
fn status_line(task: &Value) -> String {
    match task["last_status"].as_str().unwrap_or_default() {
        "" => String::new(),
        line => format!("\n<p class=\"q\">{}</p>", esc(line)),
    }
}

/// The titles of the project's newest run lines, each with its date.
fn run_log(ctx: &PageCtx, project: &str) -> String {
    let outcome = ctx.app.mem.read(&[
        "log",
        "--type",
        "run",
        "--limit",
        RUN_LOG_LIMIT,
        &format!("--project={project}"),
        "--json",
    ]);
    let mut out = String::from("<h2>Run log</h2>\n");
    if let Some(why) = outcome.broken() {
        out.push_str(&degraded_banner(why));
        return out;
    }
    let items = outcome.rows("items");
    if items.is_empty() {
        out.push_str("<p class=\"empty\">No run lines yet.</p>\n");
        return out;
    }
    out.push_str("<ul>\n");
    for item in &items {
        out.push_str(&format!(
            "<li>{}\n<div class=\"meta\">{}</div></li>\n",
            esc(item["title"].as_str().unwrap_or_default()),
            esc(item["created"].as_str().unwrap_or_default())
        ));
    }
    out.push_str("</ul>\n");
    out
}
