//! `GET /p/<project>/new`, and `POST /p/<project>/new/<form>`, which its
//! forms post to.
//!
//! A form writes mem and nothing else. An idea and a brief are text; a
//! research request is a question for the engine, which syncs at once and
//! pairs with its answer, naming the machine the research runs on.

use crate::html::{esc, project_url};
use crate::http::Response;
use crate::memcli::Outcome;
use crate::pages::{PageCtx, page_shell};

/// The forms the page posts, in the order it shows them.
pub const FORMS: [&str; 4] = ["idea", "brief", "research", "round"];

/// How every research request starts, so a pending one is found by its words.
pub const RESEARCH: &str = "research";

pub fn get(ctx: &PageCtx) -> Response {
    let project = ctx.project.unwrap_or_default();
    let query = &ctx.request.query;
    let mut body = String::new();
    if query.get("sent").is_some_and(|form| FORMS.contains(&form)) {
        body.push_str("<p class=\"banner ok\">filed</p>\n");
    } else if query.get("empty").is_some() {
        body.push_str("<p class=\"banner warn\">Type something first.</p>\n");
    }
    let machine = machine(ctx, project);
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
    body.push_str("<h2>Research</h2>\n");
    if research_pending(ctx, project) {
        body.push_str(&format!("<p class=\"banner ok\">{WAITING}</p>\n"));
    } else {
        body.push_str(&format!("<p class=\"meta\">on {}</p>\n", esc(&machine)));
        body.push_str(&button_form(project, "research", "Ask for research"));
        body.push_str(&button_form(project, "round", "Ask for a research round"));
    }
    Response::html(page_shell("new", ctx.project, &body))
}

/// What the page says once a research request waits for the engine.
pub const WAITING: &str = "sent, waiting for the engine";

pub fn new_post(ctx: &PageCtx) -> Response {
    let project = ctx.project.unwrap_or_default();
    let form = ctx.rest;
    // The photo upload already posts here under its own body cap; its form
    // has no handler yet, so it answers the bare page and writes nothing.
    if form == "finding" {
        return Response::html(page_shell("new", ctx.project, ""));
    }
    if !FORMS.contains(&form) {
        return Response::not_found();
    }
    let page = format!("{}/new", project_url(project));
    let fields = ctx.request.form();
    let text = fields.get("text").unwrap_or("").trim();
    if matches!(form, "idea" | "brief") && text.is_empty() {
        return Response::see_other(&format!("{page}?empty=1"));
    }

    let lock = ctx.app.lock_for(&format!("new:{project}"));
    // Held from the pending read to the write, so two taps cannot both find
    // no request and both ask.
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    let flag = format!("--project={project}");
    let mem = &ctx.app.mem;
    let run = match form {
        "idea" => mem.write_through(&["idea", &flag, "--", text]),
        "brief" => mem.write_through(&["brief", &flag, &format!("--set={text}")]),
        _ => {
            mem.invalidate();
            if research_pending(ctx, project) {
                return Response::see_other(&page);
            }
            let words = if form == "research" {
                "research on"
            } else {
                "research round on"
            };
            let question = format!("{words} {}", machine(ctx, project));
            mem.write_through(&["ask", &flag, "--for", "orchestrator", "--", &question])
        }
    };
    if !run.ok() {
        eprintln!("hub: new {form}: mem exited {:?}: {}", run.code, run.stderr);
        return Response::text(502, "mem did not take the write");
    }
    Response::see_other(&format!("{page}?sent={form}"))
}

/// The machine research runs on: the project's runner, else this hub's.
pub fn machine(ctx: &PageCtx, project: &str) -> String {
    let flag = format!("--project={project}");
    let runner = match &*ctx.app.mem.read(&["project", "current", &flag, "--json"]) {
        Outcome::Json(doc) => doc["runner"].as_str().unwrap_or("").to_string(),
        _ => String::new(),
    };
    if runner.is_empty() {
        ctx.app.machine.clone()
    } else {
        runner
    }
}

/// Whether a research request of the project's still waits for the engine.
/// Read from mem, so a reload or a second phone says the same.
pub fn research_pending(ctx: &PageCtx, project: &str) -> bool {
    let flag = format!("--project={project}");
    ctx.app
        .mem
        .read(&[
            "questions",
            "--pending",
            "--for",
            "orchestrator",
            &flag,
            "--json",
        ])
        .rows("questions")
        .iter()
        .any(|row| {
            row["body"]
                .as_str()
                .is_some_and(|body| body.trim_start().starts_with(RESEARCH))
        })
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

pub fn button_form(project: &str, form: &str, label: &str) -> String {
    format!(
        "<form method=\"post\" action=\"{action}\">\n\
         <button type=\"submit\">{label}</button>\n\
         </form>\n",
        action = esc(&format!("{}/new/{form}", project_url(project))),
    )
}
