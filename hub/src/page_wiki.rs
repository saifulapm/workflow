//! `GET /wiki`, `GET /wiki/<project>/<slug>` and `GET /p/<project>/wiki`.

use crate::html;
use crate::http::Response;
use crate::model;
use crate::pages::{PageCtx, page_shell};

/// One handler for three paths, told apart by the path itself: the two under
/// `/wiki` are the pages that were there first.
pub fn get(ctx: &PageCtx) -> Response {
    let path = ctx.request.path.as_str();
    if path == "/wiki" {
        return Response::html(html::wiki_index(&model::wiki(&ctx.app.mem)));
    }
    if let Some(rest) = path.strip_prefix("/wiki/") {
        return ctx.app.wiki_page(rest);
    }
    Response::html(page_shell("wiki", ctx.project, ""))
}
