//! `GET /p/<project>/evidence`, and `GET /p/<project>/file/<id>`, one
//! evidence file.

use serde_json::Value;

use crate::form::encode_component;
use crate::html::{degraded_banner, esc, project_url};
use crate::http::Response;
use crate::model;
use crate::pages::{PageCtx, page_shell};

/// Rows on one page of the gallery. Every image is its own request, so a
/// page is kept to what a phone loads at once.
pub const PAGE_ROWS: usize = 24;

/// One evidence item, or the file a finding names.
#[derive(Debug, PartialEq, Eq)]
pub struct EvidenceRow {
    pub id: String,
    pub task: String,
    pub file: String,
    pub note: String,
}

/// One row of `mem finding list --json`.
#[derive(Debug, PartialEq, Eq)]
pub struct FindingRow {
    pub id: String,
    pub open: bool,
    pub milestone: String,
    pub step: String,
    pub body: String,
    pub fixed_by: Option<String>,
    pub file: Option<String>,
}

pub fn get(ctx: &PageCtx) -> Response {
    Response::html(page_shell("evidence", ctx.project, &evidence_body(ctx)))
}

/// The bytes of the file an evidence item or a finding names. mem reads the
/// store, so no path here ever comes from the request; the id is checked
/// before it becomes an argument.
pub fn file_get(ctx: &PageCtx) -> Response {
    let id = ctx.rest;
    if !model::is_item_id(id) {
        return Response::not_found();
    }
    let project = format!("--project={}", ctx.project.unwrap_or_default());
    let run = ctx.app.mem.exec(&["evidence", "cat", &project, "--", id]);
    if run.code != Some(0) {
        return Response::not_found();
    }
    let kind = image_kind(&run.stdout).unwrap_or("text/plain; charset=utf-8");
    Response::new(200, kind, run.stdout)
        .header("Cache-Control", "private, max-age=3600")
        .header("X-Content-Type-Options", "nosniff")
}

/// The image type the leading bytes name, or `None` for anything else,
/// which is then sent as text.
pub fn image_kind(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// In mem's order, which is newest first.
pub fn evidence_rows(rows: &[Value]) -> Vec<EvidenceRow> {
    rows.iter()
        .map(|row| EvidenceRow {
            id: text_of(row, "id").unwrap_or_default(),
            task: text_of(row, "task").unwrap_or_default(),
            file: text_of(row, "file").unwrap_or_default(),
            note: text_of(row, "note").unwrap_or_default(),
        })
        .collect()
}

/// The open ahead of the fixed, each in mem's order.
pub fn finding_rows(rows: &[Value]) -> Vec<FindingRow> {
    let mut out: Vec<FindingRow> = rows
        .iter()
        .map(|row| FindingRow {
            id: text_of(row, "id").unwrap_or_default(),
            open: row["status"].as_str() != Some("fixed"),
            milestone: text_of(row, "milestone").unwrap_or_default(),
            step: text_of(row, "step").unwrap_or_default(),
            body: text_of(row, "body").unwrap_or_default().trim().to_string(),
            fixed_by: text_of(row, "fixed_by"),
            file: text_of(row, "file"),
        })
        .collect();
    out.sort_by_key(|f| !f.open);
    out
}

/// The page under the shell. Three spawns: the route's project check, the
/// evidence and the findings.
pub fn evidence_body(ctx: &PageCtx) -> String {
    let project = ctx.project.unwrap_or_default();
    let flag = format!("--project={project}");
    let evidence = ctx.app.mem.read(&["evidence", "list", &flag, "--json"]);
    let findings = ctx.app.mem.read(&["finding", "list", &flag, "--json"]);
    let mut out = String::new();
    for (outcome, verb) in [(&evidence, "evidence list"), (&findings, "finding list")] {
        if let Some(why) = model::list_fault(outcome, verb) {
            out.push_str(&degraded_banner(&why));
        }
    }
    let rows = evidence_rows(&evidence.rows("items"));
    let page = ctx
        .request
        .query
        .get("page")
        .and_then(|p| p.parse::<usize>().ok())
        .filter(|p| *p >= 1)
        .unwrap_or(1);
    out.push_str(&gallery(&rows, page, project));
    out.push_str(&findings_section(
        &finding_rows(&findings.rows("items")),
        project,
    ));
    out
}

/// One page of rows, a group per task in the order the rows bring them,
/// then the links to the pages either side.
pub fn gallery(rows: &[EvidenceRow], page: usize, project: &str) -> String {
    let mut out = String::from("<h2>Evidence</h2>\n");
    let pages = rows.len().div_ceil(PAGE_ROWS);
    let shown: Vec<&EvidenceRow> = rows
        .iter()
        .skip((page - 1).saturating_mul(PAGE_ROWS))
        .take(PAGE_ROWS)
        .collect();
    if shown.is_empty() {
        out.push_str("<p class=\"empty\">Nothing filed.</p>\n");
    }
    let mut task: Option<&str> = None;
    for row in &shown {
        if task != Some(row.task.as_str()) {
            if task.is_some() {
                out.push_str("</ul>\n");
            }
            out.push_str(&format!("<h3>{}</h3>\n<ul>\n", esc(&row.task)));
            task = Some(&row.task);
        }
        out.push_str(&format!(
            "<li>{file}<div class=\"meta\">{note}</div></li>\n",
            file = file_view(project, &row.id, &row.file),
            note = esc(&row.note),
        ));
    }
    if task.is_some() {
        out.push_str("</ul>\n");
    }
    if pages > 1 {
        let base = format!("{}/evidence", project_url(project));
        out.push_str("<nav class=\"pages\">\n");
        if page > 1 {
            out.push_str(&format!(
                "<a href=\"{}?page={}\">newer</a>\n",
                esc(&base),
                page - 1
            ));
        }
        if page < pages {
            out.push_str(&format!(
                "<a href=\"{}?page={}\">older</a>\n",
                esc(&base),
                page + 1
            ));
        }
        out.push_str("</nav>\n");
    }
    out
}

pub fn findings_section(findings: &[FindingRow], project: &str) -> String {
    let mut out = String::from("<h2>Findings</h2>\n");
    if findings.is_empty() {
        out.push_str("<p class=\"empty\">No findings.</p>\n");
        return out;
    }
    for finding in findings {
        let state = match (&finding.open, &finding.fixed_by) {
            (true, _) => "open".to_string(),
            (false, Some(commit)) => format!("fixed in {}", esc(commit)),
            (false, None) => "fixed".to_string(),
        };
        out.push_str(&format!(
            "<article>\n<div class=\"meta\">{state} · {milestone} · step {step}</div>\n\
             <p class=\"q\">{body}</p>\n",
            milestone = esc(&finding.milestone),
            step = esc(&finding.step),
            body = esc(&finding.body),
        ));
        if let Some(file) = &finding.file {
            out.push_str(&format!(
                "<p>{}</p>\n",
                file_view(project, &finding.id, file)
            ));
        }
        out.push_str("</article>\n");
    }
    out
}

/// An image file as a lazy `img` inside a link to its file route, which the
/// browser asks for only when it scrolls near; any other file as a link
/// naming it.
pub fn file_view(project: &str, id: &str, file: &str) -> String {
    let href = esc(&file_url(project, id));
    let lower = file.to_ascii_lowercase();
    let image = [".png", ".jpg", ".jpeg", ".webp"]
        .iter()
        .any(|ext| lower.ends_with(ext));
    if image {
        format!(
            "<a href=\"{href}\"><img src=\"{href}\" alt=\"{file}\" loading=\"lazy\"></a>",
            file = esc(file),
        )
    } else {
        format!("<a href=\"{href}\">{}</a>", esc(file))
    }
}

pub fn file_url(project: &str, id: &str) -> String {
    format!("{}/file/{}", project_url(project), encode_component(id))
}

fn text_of(row: &Value, key: &str) -> Option<String> {
    row[key].as_str().map(str::to_string)
}
