//! `GET /p/<project>/roadmap`: the roadmap, whole.

use crate::http::Response;
use crate::pages::PageCtx;

pub fn get(ctx: &PageCtx) -> Response {
    ctx.app.project_roadmap(ctx.project.unwrap_or_default())
}
