//! The page routes: which module answers a path, the checks every page
//! shares in front of it, and the shell each page is drawn inside.
//!
//! Each page family lives in its own `page_<name>` module, so two pages can
//! change without touching the same file.

use crate::host::{Host, Kind};
use crate::html;
use crate::http::{Request, Response};
use crate::mem::{Source, Written};
use crate::model;
use crate::{
    page_control, page_decisions, page_evidence, page_home, page_new, page_project, page_questions,
    page_roadmap, page_wiki,
};

/// What the server hands every page: mem, itself, and the plain values the
/// pages draw.
pub struct Site<'a> {
    pub mem: &'a dyn Source,
    pub host: &'a dyn Host,
    /// The machine the pages are served from, as the title names it.
    pub machine: &'a str,
    /// The other hubs home links to.
    pub siblings: &'a [String],
    /// Who an answer from these pages is by.
    pub by: &'a str,
    /// Whether the request comes from a device that paired.
    pub paired: bool,
    /// How an unpaired browser pairs, after "Pair this device first: ".
    pub pair_how: &'a str,
    /// Whether the request's origin may write.
    pub may_write: bool,
    pub now: jiff::Timestamp,
    /// The zone a time of day is shown in.
    pub tz: jiff::tz::TimeZone,
    /// The routes home offers, in button order.
    pub routes: &'a [Kind],
    /// Whether home polls `/api/stamp` to reload; a server that pushes
    /// changes to the page draws it without.
    pub poll: bool,
}

/// What a page module is handed.
pub struct PageCtx<'a> {
    pub site: &'a Site<'a>,
    pub request: &'a Request,
    /// The project the path names, already known to mem.
    pub project: Option<&'a str>,
    /// The path below the page itself: the id of `file/<id>`, the form of
    /// `new/<form>`, the slug of `/wiki/<project>/<slug>`, or what follows
    /// `/p/<project>/` on the overview's own routes.
    pub rest: &'a str,
}

impl PageCtx<'_> {
    /// The line an unpaired browser sees in place of each control that
    /// posts, or `None` for a paired one, which gets the controls.
    pub fn pair_notice(&self) -> Option<String> {
        (!self.site.paired).then(|| pair_first(self.site.pair_how))
    }
}

/// What a browser that never paired sees in place of every control that
/// posts, since the hub would refuse the post.
pub fn pair_first(how: &str) -> String {
    format!(
        "<p class=\"banner warn\">Pair this device first: {}</p>\n",
        html::esc(how)
    )
}

/// How a browser pairs with the hub on `machine`.
pub fn pair_how(machine: &str) -> String {
    format!("from a workflow checkout run vps/pair, which runs hub pair on {machine}")
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
/// in this order before any module: the method, then for a POST the origin
/// and the paired device, so a refused write never runs `mem` at all, then
/// the project.
pub fn route_page(site: &Site, request: &Request) -> Option<Response> {
    let (method, project, rest, handler) = page_for(&request.path)?;
    if request.method != method {
        return Some(Response::method_not_allowed(method));
    }
    if method == "POST" && !site.may_write {
        return Some(Response::text(403, "cross-origin writes are refused"));
    }
    // A second check behind the origin's: only a device that paired may
    // press a button. Pairing itself is the one write it cannot ask that of.
    if method == "POST" && !site.paired {
        eprintln!(
            "hub: refused POST {} from {}: device not paired",
            request.path, request.peer
        );
        let body = format!(
            "<p class=\"banner warn\">This device is not paired, so this was not sent.</p>\n{}",
            pair_first(site.pair_how)
        );
        return Some(Response::new(
            403,
            "text/html; charset=utf-8",
            page_shell("not paired", None, &body),
        ));
    }
    if let Some(project) = project
        && (project.is_empty() || !model::is_known_project(site.mem, project))
    {
        return Some(Response::not_found());
    }
    Some(handler(&PageCtx {
        site,
        request,
        project,
        rest,
    }))
}

/// Whether a page answers `path`.
pub fn owns(path: &str) -> bool {
    page_for(path).is_some()
}

/// The method, project, rest and module for one path.
fn page_for(path: &str) -> Option<(&'static str, Option<&str>, &str, Handler)> {
    match path {
        "/" => return Some(("GET", None, "", page_home::get)),
        "/answer" => return Some(("POST", None, "", page_questions::answer_post)),
        "/route" => return Some(("POST", None, "", page_home::route_post)),
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
                // The overview, and its plan and item routes.
                ("GET", sub, page_project::get)
            }
        }
    };
    Some((method, Some(project), rest, handler))
}

/// A write a page makes through mem. One that took tells the host, which
/// has every machine's store see it.
pub fn write_through(site: &Site, argv: &[&str], input: Option<&[u8]>) -> Written {
    let run = site.mem.write(argv, input);
    if run.ok {
        site.host.wrote();
    }
    run
}

/// The head, the project's name and the nav, around `body`.
pub fn page_shell(title: &str, project: Option<&str>, body: &str) -> String {
    page_shell_in(title, title, project, body)
}

/// `page_shell` for a page titled `title` that belongs to the nav's `page`,
/// as a wiki page belongs to the wiki.
pub fn page_shell_in(page: &str, title: &str, project: Option<&str>, body: &str) -> String {
    let mut out = match project {
        Some(project) => html::head(&format!("{project} / {title}")),
        None => html::head(title),
    };
    out.push_str(&html::top_bar(&match project {
        Some(project) => html::project_link(project),
        None => html::esc(title),
    }));
    out.push_str(&nav(project, page));
    out.push_str(body);
    out.push_str("</body>\n</html>\n");
    out
}

/// With a project, the project and its six pages, the one being read
/// marked. Home is the bar's mark. One link per line, so the row wraps at
/// 390 px rather than running off the side.
fn nav(project: Option<&str>, title: &str) -> String {
    let Some(project) = project else {
        return String::new();
    };
    let base = html::project_url(project);
    let current = |page: &str| {
        if page == title {
            " aria-current=\"page\""
        } else {
            ""
        }
    };
    let mut out = format!(
        "<nav class=\"pages\">\n<a href=\"{}\"{}>overview</a>\n",
        html::esc(&base),
        if PAGES.contains(&title) {
            ""
        } else {
            " aria-current=\"page\""
        },
    );
    for page in PAGES {
        out.push_str(&format!(
            "<a href=\"{}/{page}\"{}>{page}</a>\n",
            html::esc(&base),
            current(page)
        ));
    }
    out.push_str("</nav>\n");
    out
}
