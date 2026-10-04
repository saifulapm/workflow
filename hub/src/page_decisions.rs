//! `GET /p/<project>/decisions`.

use crate::http::Response;
use crate::pages::{PageCtx, page_shell};

pub fn get(ctx: &PageCtx) -> Response {
    Response::html(page_shell("decisions", ctx.project, ""))
}
