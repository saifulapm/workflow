//! Maple Mono, the hub's one typeface, and its license, carried in the
//! binary. A page read over the tailnet loads nothing from another host, and
//! a release changes these bytes only with the binary, so they cache for a
//! year.

use crate::http::Response;
use crate::pages::PageCtx;

const LONG_CACHE: &str = "public, max-age=31536000, immutable";

pub fn get(ctx: &PageCtx) -> Response {
    let (content_type, body): (&str, &[u8]) = match ctx.request.path.as_str() {
        "/assets/maple-mono-400.woff2" => (
            "font/woff2",
            include_bytes!("../assets/maple-mono-400.woff2"),
        ),
        "/assets/maple-mono-600.woff2" => (
            "font/woff2",
            include_bytes!("../assets/maple-mono-600.woff2"),
        ),
        "/assets/maple-mono-700.woff2" => (
            "font/woff2",
            include_bytes!("../assets/maple-mono-700.woff2"),
        ),
        "/assets/maple-mono-400-italic.woff2" => (
            "font/woff2",
            include_bytes!("../assets/maple-mono-400-italic.woff2"),
        ),
        "/assets/maple-mono-OFL.txt" => (
            "text/plain; charset=utf-8",
            include_bytes!("../assets/maple-mono-OFL.txt"),
        ),
        _ => return Response::not_found(),
    };
    Response::new(200, content_type, body).header("Cache-Control", LONG_CACHE)
}
