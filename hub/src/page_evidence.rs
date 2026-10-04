//! `GET /p/<project>/evidence`, and `GET /p/<project>/file/<id>`, one
//! evidence file.

use crate::http::Response;
use crate::pages::{PageCtx, page_shell};

pub fn get(ctx: &PageCtx) -> Response {
    Response::html(page_shell("evidence", ctx.project, ""))
}

pub fn file_get(ctx: &PageCtx) -> Response {
    Response::html(page_shell("file", ctx.project, ""))
}
