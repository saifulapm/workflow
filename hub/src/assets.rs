//! Maple Mono, the hub's one typeface, its license, and the comment layer a
//! designed plan page loads, carried in the binary. A page read over the tailnet loads nothing from another host, and
//! a release changes these bytes only with the binary, so they cache for a
//! year. A plan page runs in a sandboxed frame with an opaque origin, and a
//! font is a CORS fetch, so every asset answers any origin.

use crate::http::Response;
use crate::pages::PageCtx;

const LONG_CACHE: &str = "public, max-age=31536000, immutable";

pub fn get(ctx: &PageCtx) -> Response {
    if ctx.request.path == "/assets/annotate.js" {
        // Its URL carries no version, so a phone asks again each time rather
        // than keep a copy the shell no longer speaks to.
        return Response::new(
            200,
            "text/javascript; charset=utf-8",
            include_bytes!("../assets/annotate.js").as_slice(),
        )
        .header("Cache-Control", "no-cache");
    }
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
    Response::new(200, content_type, body)
        .header("Cache-Control", LONG_CACHE)
        .header("Access-Control-Allow-Origin", "*")
}
