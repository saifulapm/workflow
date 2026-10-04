//! `GET /p/<project>`: one project's front page, and its log, plan and item
//! routes.

use crate::http::Response;
use crate::pages::PageCtx;

pub fn get(ctx: &PageCtx) -> Response {
    let path = ctx.request.path.strip_prefix("/p/").unwrap_or_default();
    ctx.app.project_page(path)
}
