//! The project's buttons, and `POST /p/<project>/control`, which they post
//! to.

use crate::http::Response;
use crate::pages::{PageCtx, page_shell};

pub fn get(ctx: &PageCtx) -> Response {
    Response::html(page_shell("control", ctx.project, ""))
}

pub fn control_post(ctx: &PageCtx) -> Response {
    Response::html(page_shell("control", ctx.project, ""))
}
