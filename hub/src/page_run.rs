//! `GET /p/<project>/run`: the run under way.

use crate::http::Response;
use crate::pages::{PageCtx, page_shell};

pub fn get(ctx: &PageCtx) -> Response {
    Response::html(page_shell("run", ctx.project, ""))
}
