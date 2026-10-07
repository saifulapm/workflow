//! `GET /p/<project>/roadmap`: each milestone with its Show path, its state
//! and its plan, and while the roadmap is a draft the two forms that answer
//! it.

use std::collections::HashMap;

use crate::cost::{CostRow, parse_cost, summary};
use crate::form::encode_component;
use crate::html::{degraded_banner, esc, project_url};
use crate::http::Response;
use crate::memcli::Outcome;
use crate::pages::{PageCtx, page_shell};

/// One `- [ ] <slug> <title>` line of the roadmap and the indented lines
/// under it.
#[derive(Debug, PartialEq, Eq)]
pub struct MilestoneRow {
    pub slug: String,
    pub title: String,
    pub ticked: bool,
    pub surface: Option<String>,
    pub show: Option<String>,
    pub done: Option<String>,
}

pub fn get(ctx: &PageCtx) -> Response {
    Response::html(page_shell("roadmap", ctx.project, &roadmap_body(ctx)))
}

/// The milestones in order. The box is read the way mem reads it, so the
/// page and `mem roadmap --tick` agree on which milestone is done.
pub fn roadmap_rows(text: &str) -> Vec<MilestoneRow> {
    let mut rows: Vec<MilestoneRow> = Vec::new();
    for line in text.lines() {
        let line = line.trim_start();
        let (rest, ticked) = if let Some(rest) = line.strip_prefix("- [ ] ") {
            (rest, false)
        } else if let Some(rest) = line.strip_prefix("- [x] ").or(line.strip_prefix("- [X] ")) {
            (rest, true)
        } else {
            if let Some(row) = rows.last_mut() {
                if let Some(value) = line.strip_prefix("Surface:") {
                    row.surface = Some(value.trim().to_string());
                } else if let Some(value) = line.strip_prefix("Show:") {
                    row.show = Some(value.trim().to_string());
                } else if let Some(value) = line.strip_prefix("Done:") {
                    row.done = Some(value.trim().to_string());
                }
            }
            continue;
        };
        // The order a milestone runs in is the engine's business, not the
        // reader's, so `[after: ...]` is dropped from the title.
        let rest = match rest.rfind("[after:") {
            Some(at) if rest.trim_end().ends_with(']') => &rest[..at],
            _ => rest,
        };
        let (slug, title) = rest
            .trim()
            .split_once(char::is_whitespace)
            .unwrap_or((rest.trim(), ""));
        rows.push(MilestoneRow {
            slug: slug.to_string(),
            title: title.trim().to_string(),
            ticked,
            surface: None,
            show: None,
            done: None,
        });
    }
    rows
}

/// A milestone's cost line is written once, when it lands, and a roadmap
/// spans months, so the page reads every run line rather than the week's.
const SINCE_EVER: &str = "1970-01-01T00:00:00Z";

/// The page under the shell. Four spawns: the route's project check, the
/// roadmap, then the stored plans and the run lines, which only a roadmap
/// needs.
pub fn roadmap_body(ctx: &PageCtx) -> String {
    let project = ctx.project.unwrap_or_default();
    let roadmap = ctx.app.mem.roadmap(project);
    let (text, status) = match &*roadmap {
        Outcome::Broken(why) => return degraded_banner(why),
        Outcome::Json(doc) => match doc["text"].as_str() {
            Some(text) => (text, doc["status"].as_str()),
            None => return "<p class=\"empty\">No roadmap recorded.</p>\n".to_string(),
        },
        Outcome::Absent => return "<p class=\"empty\">No roadmap recorded.</p>\n".to_string(),
    };
    let plans: Vec<String> = ctx
        .app
        .mem
        .plan_list(project)
        .rows("plans")
        .iter()
        .filter_map(|row| row["slug"].as_str().map(str::to_string))
        .collect();
    let costs = milestone_costs(&ctx.app.mem.run_lines_since(project, SINCE_EVER));

    let mut out = format!(
        "<p class=\"meta\">status: {}</p>\n",
        esc(status.unwrap_or("not set"))
    );
    let mut seen_open = false;
    for row in roadmap_rows(text) {
        let mark = if row.ticked {
            "<span class=\"pill ok\">landed</span>"
        } else if seen_open {
            "<span class=\"pill mut\">open</span>"
        } else {
            seen_open = true;
            "<span class=\"pill\">current</span>"
        };
        out.push_str(&format!(
            "<article>\n<div class=\"row\"><span class=\"meta\">{slug}</span>{mark}</div>\n\
             <p><strong>{title}</strong></p>\n",
            slug = esc(&row.slug),
            title = esc(&row.title),
        ));
        for (label, value) in [("Show", &row.show), ("Done", &row.done)] {
            if let Some(value) = value {
                out.push_str(&format!("<p class=\"q\">{label}: {}</p>\n", esc(value)));
            }
        }
        if let Some(cost) = costs.get(&row.slug).filter(|_| row.ticked) {
            out.push_str(&format!(
                "<p class=\"meta\">cost: {}</p>\n",
                esc(&summary(
                    cost.sessions,
                    cost.minutes,
                    cost.input,
                    cost.output
                ))
            ));
        }
        if plans.contains(&row.slug) {
            out.push_str(&format!(
                "<p><a href=\"{}/plan/{}\">plan</a></p>\n",
                esc(&project_url(project)),
                esc(&encode_component(&row.slug)),
            ));
        }
        out.push_str("</article>\n");
    }

    match status {
        Some("draft") => out.push_str(&approval_forms(project)),
        Some("approved") => {
            out.push_str("<p class=\"banner ok\">approved: sent, waiting for the engine</p>\n")
        }
        _ => {}
    }
    out
}

/// Each milestone's `milestone` line by its slug. mem lists the newest
/// first, so a milestone landed twice shows its last landing.
fn milestone_costs(runs: &Outcome) -> HashMap<String, CostRow> {
    let mut costs = HashMap::new();
    for item in runs.rows("items") {
        if let Some((slug, row)) = item["title"].as_str().and_then(parse_cost)
            && row.kind == "milestone"
        {
            costs.entry(slug).or_insert(row);
        }
    }
    costs
}

/// Approve and Request changes, both posting to the project's control route,
/// which writes mem and leaves the rest to the engine's next tick.
pub fn approval_forms(project: &str) -> String {
    let action = esc(&format!("{}/control", project_url(project)));
    format!(
        "<h2>Approval</h2>\n\
         <form method=\"post\" action=\"{action}\">\n\
         <input type=\"hidden\" name=\"do\" value=\"approve\">\n\
         <button type=\"submit\">Approve</button>\n\
         </form>\n\
         <form method=\"post\" action=\"{action}\">\n\
         <input type=\"hidden\" name=\"do\" value=\"changes\">\n\
         <textarea name=\"text\" rows=\"3\" placeholder=\"what to change\" \
         aria-label=\"what to change\"></textarea>\n\
         <button type=\"submit\">Request changes</button>\n\
         </form>\n"
    )
}
