//! `GET /p/<project>/decisions`.

use serde_json::Value;

use crate::html::{degraded_banner, esc, item_url};
use crate::http::Response;
use crate::model;
use crate::pages::{PageCtx, page_shell};

/// One row of `mem search --kind ruling --project=<name> --json`.
#[derive(Debug, PartialEq, Eq)]
pub struct RulingRow {
    pub id: String,
    pub body: String,
    pub by: Option<String>,
    pub replaces: Option<String>,
    pub created: String,
}

pub fn get(ctx: &PageCtx) -> Response {
    Response::html(page_shell("decisions", ctx.project, &decisions_body(ctx)))
}

/// Newest first by id. Search ranks its rows by relevance, so its order is
/// not the page's.
pub fn ruling_rows(rows: &[Value]) -> Vec<RulingRow> {
    let mut out: Vec<RulingRow> = rows
        .iter()
        .map(|row| RulingRow {
            id: text_of(row, "id").unwrap_or_default(),
            // mem keeps the body with a newline either side.
            body: text_of(row, "body")
                .or_else(|| text_of(row, "title"))
                .unwrap_or_default()
                .trim()
                .to_string(),
            by: text_of(row, "by"),
            replaces: text_of(row, "replaces"),
            created: text_of(row, "created").unwrap_or_default(),
        })
        .collect();
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

/// The page under the shell. Two spawns: the route's project check and the
/// one search.
pub fn decisions_body(ctx: &PageCtx) -> String {
    let project = ctx.project.unwrap_or_default();
    let outcome = ctx.app.mem.read(&[
        "search",
        "--kind",
        "ruling",
        "--limit",
        "100",
        &format!("--project={project}"),
        "--json",
    ]);
    if let Some(why) = model::list_fault(&outcome, "search") {
        return degraded_banner(&why);
    }
    let rows = ruling_rows(&outcome.rows("items"));
    if rows.is_empty() {
        return String::from("<p class=\"empty\">No rulings yet.</p>\n");
    }
    rows.iter().map(|row| ruling_block(row, project)).collect()
}

/// The ruling, who decided it and what it replaced when mem knows, then its
/// date linking to the whole item.
pub fn ruling_block(row: &RulingRow, project: &str) -> String {
    let mut out = format!("<article>\n<p class=\"q\">{}</p>\n", esc(&row.body));
    match row.by.as_deref() {
        Some("saiful") => out.push_str("<p class=\"meta\">decided by Saiful</p>\n"),
        Some("agent") => out.push_str("<p class=\"meta\">decided by an agent</p>\n"),
        _ => {}
    }
    if let Some(replaces) = &row.replaces {
        out.push_str(&format!(
            "<p class=\"meta\">replaced: {}</p>\n",
            esc(replaces)
        ));
    }
    out.push_str(&format!(
        "<p class=\"meta\"><a href=\"{href}\">{date}</a></p>\n</article>\n",
        href = esc(&item_url(project, &row.id)),
        date = esc(&row.created),
    ));
    out
}

fn text_of(row: &Value, key: &str) -> Option<String> {
    row[key].as_str().map(str::to_string)
}
