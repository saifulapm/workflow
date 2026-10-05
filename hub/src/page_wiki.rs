//! `GET /wiki`, `GET /wiki/<project>/<slug>` and `GET /p/<project>/wiki`.

use serde_json::Value;

use crate::form::encode_component;
use crate::html::{self, degraded_banner, esc, item_url, page_url};
use crate::http::Response;
use crate::model;
use crate::pages::{PageCtx, page_shell};

/// The longest query passed to `mem search`. A phone types a few words; this
/// keeps a pasted page out of the argv.
pub const QUERY_BYTES: usize = 200;

/// The preamble mem lists as a section, which is no heading of the page.
const TOP: &str = "top";

/// One handler for three paths, told apart by the path itself.
pub fn get(ctx: &PageCtx) -> Response {
    if ctx.request.path == "/wiki" {
        return Response::html(html::wiki_index(&model::wiki(&ctx.app.mem)));
    }
    if ctx.request.path.starts_with("/wiki/") {
        return page_get(ctx);
    }
    Response::html(page_shell("wiki", ctx.project, &wiki_body(ctx)))
}

/// `GET /wiki/<project>/<slug>`: the contents list, then the page. Three
/// spawns: the route's project check, the text and the sections. The slug is
/// checked against mem's rule before either read, so a path that is not one
/// never reaches an argv.
pub fn page_get(ctx: &PageCtx) -> Response {
    let Some(project) = ctx.project else {
        return Response::not_found();
    };
    let slug = ctx.rest;
    if !model::is_slug(slug) {
        return Response::not_found();
    }
    let Some(text) = model::wiki_text(&ctx.app.mem, project, slug) else {
        return Response::not_found();
    };
    let sections = ctx
        .app
        .mem
        .read(&[
            "wiki",
            &format!("--project={project}"),
            "--sections",
            "--json",
            "--",
            slug,
        ])
        .rows("sections")
        .iter()
        .map(|row| (text_of(row, "heading"), text_of(row, "hslug")))
        .collect::<Vec<_>>();
    let mut body = contents(&sections);
    body.push_str("<article class=\"md\">\n");
    body.push_str(&html::markdown_ids(&text, project, &sections));
    body.push_str("</article>\n");
    Response::html(page_shell(slug, Some(project), &body))
}

/// Every heading, linked to its id on the page. Nothing for a page with none.
pub fn contents(sections: &[(String, String)]) -> String {
    let rows: String = sections
        .iter()
        .filter(|(heading, hslug)| !(heading == TOP && hslug == TOP))
        .map(|(heading, hslug)| {
            format!(
                "<li><a href=\"#{}\">{}</a></li>\n",
                esc(&encode_component(hslug)),
                esc(heading)
            )
        })
        .collect();
    if rows.is_empty() {
        return String::new();
    }
    format!("<nav class=\"contents\">\n<ul>\n{rows}</ul>\n</nav>\n")
}

/// `GET /p/<project>/wiki`: the search form, the hits when there is a query,
/// then the pages. Three spawns at most: the route's project check, the list
/// and the search.
pub fn wiki_body(ctx: &PageCtx) -> String {
    let project = ctx.project.unwrap_or_default();
    let q = query(ctx.request.query.get("q").unwrap_or_default());
    let mut out = format!(
        "<form method=\"get\" action=\"{}/wiki\">\n\
         <input type=\"search\" name=\"q\" value=\"{}\" aria-label=\"search\">\n\
         <button type=\"submit\">Search</button>\n\
         </form>\n",
        esc(&html::project_url(project)),
        esc(q),
    );
    if !q.is_empty() {
        out.push_str(&hits(ctx, project, q));
    }
    out.push_str(&pages(ctx, project));
    out
}

/// The query as it is searched for: trimmed, and cut to `QUERY_BYTES` on a
/// character boundary.
pub fn query(raw: &str) -> &str {
    let q = raw.trim();
    let mut end = q.len().min(QUERY_BYTES);
    while !q.is_char_boundary(end) {
        end -= 1;
    }
    q[..end].trim_end()
}

/// `mem search` for `q`, `--` before it so a query is never a flag. A wiki
/// hit links to its section, read off its `<slug>#<hslug>` short id; any
/// other hit links to its item.
pub fn hits(ctx: &PageCtx, project: &str, q: &str) -> String {
    let outcome = ctx.app.mem.read(&[
        "search",
        &format!("--project={project}"),
        "--limit",
        "20",
        "--json",
        "--",
        q,
    ]);
    if let Some(why) = model::list_fault(&outcome, "search") {
        return degraded_banner(&why);
    }
    let rows = outcome.rows("hits");
    if rows.is_empty() {
        return format!("<p class=\"empty\">Nothing matches {}.</p>\n", esc(q));
    }
    let mut out = String::from("<h2>Results</h2>\n<ul>\n");
    for row in &rows {
        out.push_str(&hit_row(row, project));
    }
    out.push_str("</ul>\n");
    out
}

pub fn hit_row(row: &Value, project: &str) -> String {
    let title = text_of(row, "title");
    if text_of(row, "kind") != "wiki" {
        return format!(
            "<li><a href=\"{}\">{}</a></li>\n",
            esc(&item_url(project, &text_of(row, "id"))),
            esc(&title)
        );
    }
    let short_id = text_of(row, "short_id");
    let (slug, hslug) = short_id.split_once('#').unwrap_or((&short_id, TOP));
    let heading = text_of(row, "heading");
    let label = if heading.is_empty() || heading == TOP {
        esc(&title)
    } else {
        format!("{} · {}", esc(&title), esc(&heading))
    };
    format!(
        "<li><a href=\"{href}#{anchor}\">{label}</a>\
         <div class=\"meta\">{snippet}</div></li>\n",
        href = esc(&page_url(project, slug)),
        anchor = esc(&encode_component(hslug)),
        snippet = esc(&text_of(row, "snippet")),
    )
}

/// The project's pages, `index` first and then by slug, each by its title.
pub fn pages(ctx: &PageCtx, project: &str) -> String {
    let outcome = ctx.app.mem.wiki(project);
    if let Some(why) = model::list_fault(&outcome, "wiki") {
        return degraded_banner(&why);
    }
    let mut rows: Vec<(String, String)> = outcome
        .rows("pages")
        .iter()
        .map(|row| {
            let slug = text_of(row, "slug");
            let title = text_of(row, "title");
            let title = if title.is_empty() {
                slug.clone()
            } else {
                title
            };
            (slug, title)
        })
        .collect();
    rows.sort_by(|a, b| (a.0 != "index", &a.0).cmp(&(b.0 != "index", &b.0)));
    if rows.is_empty() {
        return String::from("<p class=\"empty\">No pages yet.</p>\n");
    }
    let mut out = String::from("<h2>Pages</h2>\n<ul>\n");
    for (slug, title) in &rows {
        out.push_str(&format!(
            "<li><a href=\"{}\">{}</a><div class=\"meta\">{}</div></li>\n",
            esc(&page_url(project, slug)),
            esc(title),
            esc(slug)
        ));
    }
    out.push_str("</ul>\n");
    out
}

fn text_of(row: &Value, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}
