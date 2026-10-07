//! The page routes: which module answers a path, the checks every page
//! shares in front of it, and the shell each page is drawn inside.
//!
//! Each page family lives in its own `page_<name>` module, so two pages can
//! change without touching the same file.

use crate::app::App;
use crate::config::Config;
use crate::html;
use crate::http::{Request, Response};
use crate::model;
use crate::{
    page_control, page_decisions, page_evidence, page_home, page_new, page_project, page_questions,
    page_roadmap, page_wiki,
};

/// What a page module is handed.
pub struct PageCtx<'a> {
    pub app: &'a App,
    pub request: &'a Request,
    /// The project the path names, already known to mem.
    pub project: Option<&'a str>,
    /// The path below the page itself: the id of `file/<id>`, the form of
    /// `new/<form>`, the slug of `/wiki/<project>/<slug>`, or what follows
    /// `/p/<project>/` on the overview's own routes.
    pub rest: &'a str,
}

pub type Handler = fn(&PageCtx) -> Response;

/// The six pages under `/p/<project>/` the nav links to, after home and the
/// project itself.
pub const PAGES: [&str; 6] = [
    "roadmap",
    "questions",
    "evidence",
    "wiki",
    "decisions",
    "new",
];

/// Answers a page route, or `None` for a path no page owns. The checks run
/// in this order before any module: the method, then for a POST the origin,
/// so a cross-origin write never runs `mem` at all, then the project.
pub fn route_page(app: &App, request: &Request) -> Option<Response> {
    let (method, project, rest, handler) = page_for(&request.path)?;
    if request.method != method {
        return Some(Response::method_not_allowed(method));
    }
    if method == "POST" && !app.guard.may_write(request) {
        return Some(Response::text(403, "cross-origin writes are refused"));
    }
    if let Some(project) = project
        && (project.is_empty() || !model::is_known_project(&app.mem, project))
    {
        return Some(Response::not_found());
    }
    Some(handler(&PageCtx {
        app,
        request,
        project,
        rest,
    }))
}

/// The method, project, rest and module for one path.
fn page_for(path: &str) -> Option<(&'static str, Option<&str>, &str, Handler)> {
    match path {
        "/" => return Some(("GET", None, "", page_home::get)),
        "/answer" => return Some(("POST", None, "", page_questions::answer_post)),
        "/wiki" => return Some(("GET", None, "", page_wiki::get)),
        _ => {}
    }
    if let Some(rest) = path.strip_prefix("/wiki/") {
        // No slash at all is left to the wiki handler, which 404s it.
        return Some(match rest.split_once('/') {
            Some((project, slug)) => ("GET", Some(project), slug, page_wiki::get),
            None => ("GET", None, rest, page_wiki::get),
        });
    }
    let rest = path.strip_prefix("/p/")?;
    let (project, sub) = rest.split_once('/').unwrap_or((rest, ""));
    let (method, rest, handler): (&str, &str, Handler) = match sub {
        "roadmap" => ("GET", "", page_roadmap::get),
        "questions" => ("GET", "", page_questions::get),
        "evidence" => ("GET", "", page_evidence::get),
        "wiki" => ("GET", "", page_wiki::get),
        "decisions" => ("GET", "", page_decisions::get),
        "new" => ("GET", "", page_new::get),
        "control" => ("POST", "", page_control::control_post),
        _ => {
            if let Some(id) = sub.strip_prefix("file/") {
                ("GET", id, page_evidence::file_get)
            } else if let Some(form) = sub.strip_prefix("new/") {
                ("POST", form, page_new::new_post)
            } else {
                // The overview, and its log, plan and item routes.
                ("GET", sub, page_project::get)
            }
        }
    };
    Some((method, Some(project), rest, handler))
}

/// The head, the project's name and the nav, around `body`.
pub fn page_shell(title: &str, project: Option<&str>, body: &str) -> String {
    let mut out = match project {
        Some(project) => html::head(&format!("{project} / {title}")),
        None => html::head(title),
    };
    out.push_str(&format!(
        "<header><h1>{}</h1></header>\n",
        html::esc(project.unwrap_or(title))
    ));
    out.push_str(&nav(project));
    out.push_str(body);
    out.push_str("</body>\n</html>\n");
    out
}

/// Home, and with a project the project and its six pages. One link per
/// line, so the row wraps at 390 px rather than running off the side.
fn nav(project: Option<&str>) -> String {
    let mut out = String::from("<nav class=\"pages\">\n<a href=\"/\">home</a>\n");
    if let Some(project) = project {
        let base = html::project_url(project);
        out.push_str(&format!(
            "<a href=\"{}\">{}</a>\n",
            html::esc(&base),
            html::esc(project)
        ));
        for page in PAGES {
            out.push_str(&format!(
                "<a href=\"{}/{page}\">{page}</a>\n",
                html::esc(&base)
            ));
        }
    }
    out.push_str("</nav>\n");
    out
}

/// The hub of the machine named `machine`: the sibling whose host starts with
/// that name. Config takes no other key for it, and the siblings already hold
/// every other hub's address.
pub fn sibling_hub(config: &Config, machine: &str) -> Option<String> {
    if machine.is_empty() {
        return None;
    }
    config
        .siblings
        .iter()
        .find(|url| {
            let host = url.split_once("://").map_or(url.as_str(), |(_, rest)| rest);
            host.starts_with(machine)
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sibling_is_found_by_the_start_of_its_host() {
        let config = Config {
            siblings: vec![
                "http://nuc:8088".to_string(),
                "https://macbook-m2.tail1234.ts.net/".to_string(),
            ],
            ..Config::default()
        };
        assert_eq!(
            sibling_hub(&config, "macbook-m2").as_deref(),
            Some("https://macbook-m2.tail1234.ts.net/")
        );
        assert_eq!(
            sibling_hub(&config, "nuc").as_deref(),
            Some("http://nuc:8088")
        );
        assert_eq!(sibling_hub(&config, "desk"), None);
        assert_eq!(sibling_hub(&config, ""), None);
    }
}
