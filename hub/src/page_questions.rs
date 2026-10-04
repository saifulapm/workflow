//! `GET /p/<project>/questions`, and `POST /answer`, which every question's
//! form posts to.

use crate::http::Response;
use crate::pages::{PageCtx, page_shell};

pub fn get(ctx: &PageCtx) -> Response {
    Response::html(page_shell("questions", ctx.project, ""))
}

pub fn answer_post(ctx: &PageCtx) -> Response {
    ctx.app.answer(ctx.request)
}
