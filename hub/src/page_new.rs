//! `GET /p/<project>/new`, and `POST /p/<project>/new/<form>`, which its
//! forms post to.
//!
//! A form writes mem and nothing else. An idea and a brief are text; a
//! finding is filed against a milestone's step, with a photo from the phone.

use std::io::Write;
use std::path::PathBuf;

use crate::config::{state_home, urandom};
use crate::html::{esc, project_url};
use crate::http::Response;
use crate::memcli::Outcome;
use crate::multipart::parse_multipart;
use crate::page_evidence::image_kind;
use crate::page_roadmap::{MilestoneRow, roadmap_rows};
use crate::pages::{PageCtx, page_shell};

/// The text forms the page posts, in the order it shows them.
pub const FORMS: [&str; 2] = ["idea", "brief"];

pub fn get(ctx: &PageCtx) -> Response {
    let project = ctx.project.unwrap_or_default();
    let query = &ctx.request.query;
    let mut body = String::new();
    if query.get("sent").is_some_and(|form| FORMS.contains(&form)) {
        body.push_str("<p class=\"banner ok\">filed</p>\n");
    } else if query.get("empty").is_some() {
        body.push_str("<p class=\"banner warn\">Type something first.</p>\n");
    }
    body.push_str(&text_form(
        project,
        "idea",
        "Idea",
        "an idea for later",
        "File the idea",
    ));
    body.push_str(&text_form(
        project,
        "brief",
        "Brief",
        "what the project is for",
        "Write the brief",
    ));
    body.push_str(&finding_form(project, &milestones(ctx, project)));
    Response::html(page_shell("new", ctx.project, &body))
}

pub fn new_post(ctx: &PageCtx) -> Response {
    let project = ctx.project.unwrap_or_default();
    let form = ctx.rest;
    if form == "finding" {
        return finding_post(ctx, project);
    }
    if !FORMS.contains(&form) {
        return Response::not_found();
    }
    let page = format!("{}/new", project_url(project));
    let fields = ctx.request.form();
    let text = fields.get("text").unwrap_or("").trim();
    if text.is_empty() {
        return Response::see_other(&format!("{page}?empty=1"));
    }

    let flag = format!("--project={project}");
    let mem = &ctx.app.mem;
    let run = if form == "idea" {
        mem.write_through(&["idea", &flag, "--", text])
    } else {
        mem.write_through(&["brief", &flag, &format!("--set={text}")])
    };
    if !run.ok() {
        eprintln!("hub: new {form}: mem exited {:?}: {}", run.code, run.stderr);
        return Response::text(502, "mem did not take the write");
    }
    Response::see_other(&format!("{page}?sent={form}"))
}

pub fn text_form(project: &str, form: &str, heading: &str, hint: &str, label: &str) -> String {
    format!(
        "<h2>{heading}</h2>\n\
         <form method=\"post\" action=\"{action}\">\n\
         <textarea name=\"text\" rows=\"3\" placeholder=\"{hint}\" \
         aria-label=\"{hint}\"></textarea>\n\
         <button type=\"submit\">{label}</button>\n\
         </form>\n",
        action = esc(&format!("{}/new/{form}", project_url(project))),
    )
}

/// The roadmap's milestones, in order; none when the project has no roadmap.
pub fn milestones(ctx: &PageCtx, project: &str) -> Vec<MilestoneRow> {
    match &*ctx.app.mem.roadmap(project) {
        Outcome::Json(doc) => doc["text"].as_str().map(roadmap_rows).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The finding form, with the first open milestone chosen, else the last.
pub fn finding_form(project: &str, rows: &[MilestoneRow]) -> String {
    let mut out = String::from("<h2>Finding</h2>\n");
    let Some(chosen) = rows
        .iter()
        .position(|row| !row.ticked)
        .or(rows.len().checked_sub(1))
    else {
        out.push_str(
            "<p class=\"empty\">No roadmap yet, so no milestone to file a finding against.</p>\n",
        );
        return out;
    };
    out.push_str(&format!(
        "<form method=\"post\" action=\"{action}\" enctype=\"multipart/form-data\">\n\
         <select name=\"milestone\" aria-label=\"milestone\">\n",
        action = esc(&format!("{}/new/finding", project_url(project))),
    ));
    for (i, row) in rows.iter().enumerate() {
        let slug = esc(&row.slug);
        let selected = if i == chosen { " selected" } else { "" };
        out.push_str(&format!(
            "<option value=\"{slug}\"{selected}>{slug}</option>\n"
        ));
    }
    out.push_str(
        "</select>\n\
         <input type=\"number\" name=\"step\" value=\"0\" min=\"0\" max=\"999\" \
         aria-label=\"step\">\n\
         <textarea name=\"text\" rows=\"3\" placeholder=\"what went wrong\" \
         aria-label=\"what went wrong\"></textarea>\n\
         <input type=\"file\" name=\"photo\" accept=\"image/jpeg,image/png,image/webp\" \
         capture=\"environment\">\n\
         <button type=\"submit\">File the finding</button>\n\
         </form>\n",
    );
    out
}

/// Files a finding from the form, its photo through a file of the hub's own
/// naming that is gone before the answer is written.
pub fn finding_post(ctx: &PageCtx, project: &str) -> Response {
    let content_type = ctx.request.header("content-type").unwrap_or("");
    let parts = match parse_multipart(content_type, &ctx.request.body) {
        Ok(parts) => parts,
        Err(why) => return Response::text(400, &format!("not a finding form: {why}")),
    };
    let field = |name: &str| {
        parts
            .iter()
            .find(|part| part.name == name)
            .map_or(&[][..], |part| part.data.as_slice())
    };
    let text = String::from_utf8_lossy(field("text"));
    let text = text.trim();
    if text.is_empty() {
        return Response::text(400, "Type something first.");
    }
    let step = String::from_utf8_lossy(field("step"));
    let step = step.trim();
    if !(1..=3).contains(&step.len()) || !step.bytes().all(|b| b.is_ascii_digit()) {
        return Response::text(400, "the step is not a number of one to three digits");
    }
    // A browser sends the file part empty when no photo was chosen.
    let photo = field("photo");
    let kind = match (photo.is_empty(), image_kind(photo)) {
        (true, _) => None,
        (false, Some(kind)) => Some(kind),
        (false, None) => return Response::text(400, "the photo is not a JPEG, PNG or WebP"),
    };
    let milestone = String::from_utf8_lossy(field("milestone"));
    let milestone = milestone.trim();
    if !milestones(ctx, project)
        .iter()
        .any(|row| row.slug == milestone)
    {
        return Response::text(400, "the roadmap has no such milestone");
    }

    let upload = match kind.map(|kind| Upload::write(kind, photo)).transpose() {
        Ok(upload) => upload,
        Err(e) => {
            eprintln!("hub: new finding: {e:#}");
            return Response::text(500, "the photo could not be stored");
        }
    };
    let flag = format!("--project={project}");
    let file = upload.as_ref().map(|u| u.0.to_string_lossy().into_owned());
    let mut args = vec![
        "finding",
        "add",
        &flag,
        "--milestone",
        milestone,
        "--step",
        step,
    ];
    if let Some(file) = &file {
        args.extend(["--evidence", file]);
    }
    args.extend(["--", text]);
    let run = ctx.app.mem.write_through(&args);
    drop(upload);
    if !run.ok() {
        eprintln!(
            "hub: new finding: mem exited {:?}: {}",
            run.code, run.stderr
        );
        return Response::text(502, "mem did not take the write");
    }
    Response::see_other(&format!("{}/evidence", project_url(project)))
}

/// A photo under the hub's state directory, never /tmp with its small
/// quota, named by the hub so no path comes from the request. mem copies it
/// in, and dropping this removes it whatever mem answered.
pub struct Upload(pub PathBuf);

impl Upload {
    pub fn write(kind: &str, bytes: &[u8]) -> anyhow::Result<Upload> {
        let dir = state_home()?.join("hub/uploads");
        std::fs::create_dir_all(&dir)?;
        let ext = match kind {
            "image/png" => "png",
            "image/webp" => "webp",
            _ => "jpg",
        };
        let hex: String = urandom::<4>()?.iter().map(|b| format!("{b:02x}")).collect();
        let time = jiff::Timestamp::now().strftime("%Y%m%dT%H%M%SZ");
        let upload = Upload(dir.join(format!("photo-{time}-{hex}.{ext}")));
        let mut file = std::fs::File::create_new(&upload.0)?;
        file.write_all(bytes)?;
        Ok(upload)
    }
}

impl Drop for Upload {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
