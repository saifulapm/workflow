//! `GET /p/<project>/questions`, and `POST /answer`, which every question's
//! form posts to.

use serde_json::Value;

use crate::host::Held;
use crate::html::{Banner, degraded_banner, esc, linked, project_url};
use crate::http::Response;
use crate::mem::{Mem, Source, answer_argv};
use crate::model;
use crate::pages::{PageCtx, Site, page_shell};

/// One row of `mem questions --project=<name> --json`.
#[derive(Debug, PartialEq, Eq)]
pub struct QuestionRow {
    pub id: String,
    pub text: String,
    pub options: Vec<String>,
    pub recommend: Option<String>,
    pub answered: bool,
    pub answer: Option<String>,
}

pub fn get(ctx: &PageCtx) -> Response {
    Response::html(page_shell("questions", ctx.project, &questions_body(ctx)))
}

/// Answers through the dashboard's path, then sends a form that names a
/// known project back to that project's questions page with the same
/// banner, so a tap on this page lands on this page.
pub fn answer_post(ctx: &PageCtx) -> Response {
    let mut response = answer(ctx);
    if response.status != 303 {
        return response;
    }
    let form = ctx.request.form();
    let Some(project) = form.get("project").map(str::trim) else {
        return response;
    };
    if !model::is_known_project(ctx.site.mem, project) {
        return response;
    }
    for (name, value) in &mut response.headers {
        if name == "Location" {
            let query = value.find('?').map_or("", |at| &value[at..]);
            *value = format!("{}/questions{query}", project_url(project));
        }
    }
    response
}

/// Whether a question is still waiting — and the third answer, which matters:
/// "mem could not tell us" is not "no such question". Answering the second way
/// would report an unknown id for a store that is merely slow, and now that a
/// `mem` child can be killed on a deadline, that is reachable. A waiting one
/// carries its row, whose project and session the step after the answer needs.
pub enum Pending {
    Yes(Value),
    No,
    Unreadable(String),
}

/// Whether `id`, full or short, is among the questions waiting on a person.
pub fn pending(mem: &dyn Source, id: &str) -> Pending {
    let outcome = mem.questions();
    if let Some(why) = model::list_fault(&outcome, "questions") {
        return Pending::Unreadable(why);
    }
    let waiting = outcome
        .rows("questions")
        .into_iter()
        .find(|row| row["id"] == id || row["short_id"] == id);
    match waiting {
        Some(row) => Pending::Yes(row),
        None => Pending::No,
    }
}

/// The order matters: nothing runs `mem answer` until the
/// origin has been checked, and the redirect is a 303 so a reload of the
/// result page cannot answer twice.
pub fn answer(ctx: &PageCtx) -> Response {
    let site = ctx.site;
    if !site.may_write {
        return Response::text(403, "cross-origin writes are refused");
    }
    let form = ctx.request.form();
    let id = form.get("id").unwrap_or("").trim();
    // `mem ask` prints the id behind a `#`, and that is what a human
    // copies. The page's own form supplies the bare id, so the phone path
    // never met this; anything typed or pasted from a terminal did, and got
    // `?unknown=1` with the question still pending.
    let id = id.strip_prefix('#').unwrap_or(id);
    let text = form.get("text").unwrap_or("").trim();
    if id.is_empty() || text.is_empty() {
        return back(Banner::Empty);
    }

    let response = {
        // One answer at a time, per question id. Held across all three
        // steps: dropping it between the read and the write is the race,
        // every racer seeing the question waiting and every one writing.
        let lock = site.host.lock(id);
        let _held = lock.lock().unwrap_or_else(|e| e.into_inner());

        // The queue as it is right now, not as it was up to five seconds
        // ago. Without this the answered question stays on screen for the
        // rest of the TTL and a second tap writes a second answer — which
        // mem accepts, and which the waiting agent never sees, because the
        // first one won.
        site.mem.invalidate();
        match pending(site.mem, id) {
            Pending::No => match site.host.held_written(id) {
                Some((full, held)) => supersede(site, &full, &held, text),
                None => back(Banner::Unknown),
            },
            Pending::Unreadable(why) => {
                eprintln!("hub: answer: could not read the queue: {why}");
                back(Banner::Failed)
            }
            Pending::Yes(row) => {
                let run = site.mem.write(&answer_argv(id, text, site.by, None), None);
                site.mem.invalidate();
                if run.ok {
                    site.host.answered(&row);
                    back(Banner::Answered(short(id)))
                } else {
                    if !run.stderr.is_empty() {
                        eprintln!("hub: mem answer: {}", run.stderr);
                    }
                    back(Banner::Failed)
                }
            }
        }
    };
    site.host.forget_locks();
    response
}

/// Saiful's answer to a question Dulali's answer already closed: it
/// supersedes Dulali's in mem, and reaches the running session as a new
/// message.
fn supersede(site: &Site, id: &str, held: &Held, text: &str) -> Response {
    let earlier = held.written.as_deref();
    let run = site
        .mem
        .write(&answer_argv(id, text, site.by, earlier), None);
    site.mem.invalidate();
    if !run.ok {
        if !run.stderr.is_empty() {
            eprintln!("hub: mem answer: {}", run.stderr);
        }
        return back(Banner::Failed);
    }
    site.host.superseded(id, held, text);
    back(Banner::Answered(short(id)))
}

fn back(banner: Banner) -> Response {
    Response::see_other(&format!("/{}", banner.query()))
}

/// mem's own short id: the last eight characters of a ULID. A banner showing
/// those eight behind a `#` matches what `mem ask` printed and what the page
/// shows; the full 26-character id matches nothing a human has looked at.
pub fn short(id: &str) -> String {
    // `is_ascii` as well as the length: the id came off a form, and slicing a
    // 26-byte string that is not 26 characters would panic on the connection
    // thread.
    if id.len() == 26 && id.is_ascii() {
        id[18..].to_string()
    } else {
        id.to_string()
    }
}

/// Newest first, the pending ahead of the answered.
pub fn question_rows(rows: &[Value]) -> Vec<QuestionRow> {
    let mut out: Vec<QuestionRow> = rows.iter().map(question_row).collect();
    out.sort_by(|a, b| a.answered.cmp(&b.answered).then(b.id.cmp(&a.id)));
    out
}

/// One row. The text is the whole body, trimmed, and the title only when
/// the body is blank: the title is a batched question's first line.
pub fn question_row(row: &Value) -> QuestionRow {
    QuestionRow {
        id: text_of(row, "id").unwrap_or_default(),
        text: text_of(row, "body")
            .filter(|body| !body.trim().is_empty())
            .or_else(|| text_of(row, "title"))
            .unwrap_or_default()
            .trim()
            .to_string(),
        options: row["options"]
            .as_array()
            .map(|options| {
                options
                    .iter()
                    .filter_map(|o| o.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        recommend: text_of(row, "recommend"),
        answered: row["answered"].as_bool().unwrap_or(false),
        answer: text_of(row, "answer"),
    }
}

/// The page under the shell. Two spawns: the route's project check and the
/// project's questions.
pub fn questions_body(ctx: &PageCtx) -> String {
    let project = ctx.project.unwrap_or_default();
    let mut out = banner_line(&Banner::from_query(&ctx.request.query));
    let outcome = ctx.site.mem.questions_all(project);
    if let Some(why) = model::list_fault(&outcome, "questions") {
        out.push_str(&degraded_banner(&why));
        return out;
    }
    let rows = question_rows(&outcome.rows("questions"));
    let (answered, pending): (Vec<_>, Vec<_>) = rows.iter().partition(|q| q.answered);

    out.push_str("<h2>Pending</h2>\n");
    if pending.is_empty() {
        out.push_str("<p class=\"empty\">Nothing waiting.</p>\n");
    }
    for question in pending {
        out.push_str(&pending_block(
            question,
            Some(project),
            "",
            ctx.pair_notice().as_deref(),
        ));
    }
    out.push_str("<h2>Answered</h2>\n");
    if answered.is_empty() {
        out.push_str("<p class=\"empty\">None yet.</p>\n");
    }
    for question in answered {
        out.push_str(&format!(
            "<article>\n<p class=\"q\">{text}</p>\n<p class=\"meta\">answer: {answer}</p>\n</article>\n",
            text = linked(&question.text),
            answer = esc(question.answer.as_deref().unwrap_or("")),
        ));
    }
    out
}

/// A form per option, its button carrying the option as `text`, then the
/// text box for anything else. The word on the pick's button is what marks
/// it: the shared stylesheet has no rule for the class. Home and the
/// questions page both render a pending question with this. A form that
/// names its project lands back on that project's questions page; home
/// passes `None`, so its answers land on `/`. `lead` is markup, escaped by
/// the caller, put above the question. With a pair notice the question shows
/// with the notice in place of its forms.
pub fn pending_block(
    question: &QuestionRow,
    project: Option<&str>,
    lead: &str,
    pair_notice: Option<&str>,
) -> String {
    if let Some(notice) = pair_notice {
        return format!(
            "<article>\n{lead}<p class=\"q\">{}</p>\n{notice}</article>\n",
            linked(&question.text)
        );
    }
    let mut hidden = format!(
        "<input type=\"hidden\" name=\"id\" value=\"{}\">\n",
        esc(&question.id)
    );
    if let Some(project) = project {
        hidden.push_str(&format!(
            "<input type=\"hidden\" name=\"project\" value=\"{}\">\n",
            esc(project)
        ));
    }
    let mut out = format!(
        "<article>\n{lead}<p class=\"q\">{}</p>\n",
        linked(&question.text)
    );
    for option in &question.options {
        let button = if question.recommend.as_deref() == Some(option.as_str()) {
            format!(
                "<button type=\"submit\" name=\"text\" value=\"{value}\" class=\"recommended\">{value} · recommended</button>\n",
                value = esc(option),
            )
        } else {
            format!(
                "<button type=\"submit\" name=\"text\" value=\"{value}\" class=\"alt\">{value}</button>\n",
                value = esc(option),
            )
        };
        out.push_str(&format!(
            "<form class=\"opt\" method=\"post\" action=\"/answer\">\n{hidden}{button}</form>\n"
        ));
    }
    out.push_str(&format!(
        "<form method=\"post\" action=\"/answer\">\n{hidden}\
         <textarea name=\"text\" rows=\"3\" placeholder=\"answer\" aria-label=\"answer\"></textarea>\n\
         <button type=\"submit\">Answer</button>\n\
         </form>\n</article>\n"
    ));
    out
}

/// The dashboard's banner for what the last `POST /answer` did, chosen from
/// the code in the query so nothing from the query string reaches the page.
pub fn banner_line(banner: &Banner) -> String {
    let (kind, text) = match banner {
        // Only home's route buttons redirect with NoMachine, and only to home.
        Banner::None | Banner::NoMachine => return String::new(),
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

fn text_of(row: &Value, key: &str) -> Option<String> {
    row[key].as_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_ulid_shortens_to_the_id_mem_prints() {
        assert_eq!(short("01M0BF1F8BY8FXGZS428J1TSD1"), "28J1TSD1");
        assert_eq!(short("28J1TSD1"), "28J1TSD1");
        assert_eq!(short(""), "");
        // Twenty-six bytes, thirteen characters: sliced blindly, this panics.
        let multibyte = "é".repeat(13);
        assert_eq!(short(&multibyte), multibyte);
    }
}
