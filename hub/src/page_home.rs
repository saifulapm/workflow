//! `GET /`: every project and every waiting question, on one page.

use crate::http::Response;
use crate::pages::PageCtx;

pub fn get(ctx: &PageCtx) -> Response {
    ctx.app.dashboard(ctx.request)
}
