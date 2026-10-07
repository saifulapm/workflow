//! `GET /p/<project>/roadmap`: each milestone with its Show path, its state
//! and its plan, and while the roadmap is a draft the two forms that answer
//! it.

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
    pub show: Option<String>,
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
            if let Some(row) = rows.last_mut()
                && let Some(value) = line.strip_prefix("Show:")
            {
                row.show = Some(value.trim().to_string());
            }
            continue;
        };
        // The order a milestone runs in is not the reader's business, so
        // `[after: ...]` is dropped from the title.
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
            show: None,
        });
    }
    rows
}

/// The page under the shell. Three spawns: the route's project check, the
/// roadmap, then the stored plans, which only a roadmap needs.
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

    let rows = roadmap_rows(text);
    // Every milestone ticked is done, whatever status is stored, as the
    // project page reads it.
    let status = if !rows.is_empty() && rows.iter().all(|row| row.ticked) {
        Some("done")
    } else {
        status
    };
    let mut out = format!(
        "<p class=\"meta\">status: {}</p>\n",
        esc(status.unwrap_or("not set"))
    );
    let mut seen_open = false;
    for row in rows {
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
        if let Some(show) = &row.show {
            out.push_str(&format!("<p class=\"q\">Show: {}</p>\n", esc(show)));
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
        Some("approved") => out.push_str("<p class=\"banner ok\">approved: sent</p>\n"),
        _ => {}
    }
    out
}

/// Approve and Request changes, both posting to the project's control route,
/// which writes mem and nothing else.
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
