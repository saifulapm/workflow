//! `GET /p/<project>/new`, and `POST /p/<project>/new/<form>`, which its
//! forms post to.

use crate::http::Response;
use crate::pages::{PageCtx, page_shell};

pub fn get(ctx: &PageCtx) -> Response {
    Response::html(page_shell("new", ctx.project, ""))
}

pub fn new_post(ctx: &PageCtx) -> Response {
    Response::html(page_shell("new", ctx.project, ""))
}
