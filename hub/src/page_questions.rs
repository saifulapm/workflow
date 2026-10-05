//! `GET /p/<project>/questions`, and `POST /answer`, which every question's
//! form posts to.

use serde_json::Value;

use crate::html::{Banner, degraded_banner, esc, project_url};
use crate::http::Response;
use crate::model;
use crate::pages::{PageCtx, page_shell};

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
    let mut response = ctx.app.answer(ctx.request);
    if response.status != 303 {
        return response;
    }
    let form = ctx.request.form();
    let Some(project) = form.get("project").map(str::trim) else {
        return response;
    };
    if !model::is_known_project(&ctx.app.mem, project) {
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

/// Newest first, the pending ahead of the answered.
pub fn question_rows(rows: &[Value]) -> Vec<QuestionRow> {
    let mut out: Vec<QuestionRow> = rows
        .iter()
        .map(|row| QuestionRow {
            id: text_of(row, "id").unwrap_or_default(),
            text: text_of(row, "body")
                .or_else(|| text_of(row, "title"))
                .unwrap_or_default(),
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
        })
        .collect();
    out.sort_by(|a, b| a.answered.cmp(&b.answered).then(b.id.cmp(&a.id)));
    out
}

/// The page under the shell. Two spawns: the route's project check and the
/// project's questions.
pub fn questions_body(ctx: &PageCtx) -> String {
    let project = ctx.project.unwrap_or_default();
    let mut out = banner_line(&Banner::from_query(&ctx.request.query));
    let outcome = ctx.app.mem.questions_all(project);
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
        out.push_str(&pending_block(question, project));
    }
    out.push_str("<h2>Answered</h2>\n");
    if answered.is_empty() {
        out.push_str("<p class=\"empty\">None yet.</p>\n");
    }
    for question in answered {
        out.push_str(&format!(
            "<article>\n<p class=\"q\">{text}</p>\n<p class=\"meta\">answer: {answer}</p>\n</article>\n",
            text = esc(&question.text),
            answer = esc(question.answer.as_deref().unwrap_or("")),
        ));
    }
    out
}

/// A form per option, its button carrying the option as `text`, then the
/// text box for anything else. The word on the pick's button is what marks
/// it: the shared stylesheet has no rule for the class.
pub fn pending_block(question: &QuestionRow, project: &str) -> String {
    let hidden = format!(
        "<input type=\"hidden\" name=\"id\" value=\"{id}\">\n\
         <input type=\"hidden\" name=\"project\" value=\"{project}\">\n",
        id = esc(&question.id),
        project = esc(project),
    );
    let mut out = format!("<article>\n<p class=\"q\">{}</p>\n", esc(&question.text));
    for option in &question.options {
        let button = if question.recommend.as_deref() == Some(option.as_str()) {
            format!(
                "<button type=\"submit\" name=\"text\" value=\"{value}\" class=\"recommended\">{value} · recommended</button>\n",
                value = esc(option),
            )
        } else {
            format!(
                "<button type=\"submit\" name=\"text\" value=\"{value}\">{value}</button>\n",
                value = esc(option),
            )
        };
        out.push_str(&format!(
            "<form method=\"post\" action=\"/answer\">\n{hidden}{button}</form>\n"
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

fn text_of(row: &Value, key: &str) -> Option<String> {
    row[key].as_str().map(str::to_string)
}
