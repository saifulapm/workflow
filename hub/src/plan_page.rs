//! A stored html-plan page in the hub (h1-plan-pages): the shell the hub
//! draws around it, and the frame the page itself runs in.
//!
//! The page is whatever the planner wrote, script included, so it never runs
//! on the hub's own origin. The frame is sandboxed without
//! `allow-same-origin`: the page gets an opaque origin, and its script can
//! reach the hub only through the messages the shell chooses to act on.

use crate::html::{degraded_banner, detail_head, esc, project_url};
use crate::http::Response;
use crate::model;
use crate::page_control::{failed, pending_approval};
use crate::pages::PageCtx;

/// The html-plan runtime, one copy, in the plan skill. The hub serves its own
/// patched version of it (see `served_runtime`), and never edits the file.
const RUNTIME_JS: &str = include_str!("../../skills/plan/html-plan/htmlplan.js");
const RUNTIME_CSS: &str = include_str!("../../skills/plan/html-plan/htmlplan.css");

/// The page's own header: it may run only in a sandbox, even opened on its
/// own, and only the hub may frame it, so its messages reach no one else.
const PAGE_CSP: &str = "sandbox allow-scripts allow-popups; frame-ancestors 'self'";

/// The three places the hub's copy of the runtime differs from the skill's.
/// Each must match exactly once; `the_runtime_still_has_what_the_hub_patches`
/// fails when an upgrade of the runtime moves one.
const PATCHES: [(&str, &str); 3] = [
    // The frame's origin is opaque, so it has no storage of its own. Drafts
    // go to the shell instead, through the bridge's store.
    ("localStorage", "hubStore"),
    // The empty Send slot, filled.
    (
        "const liveOn = false, send = null;",
        "const liveOn = parent !== window, send = liveOn ? h('button', { class: 'nw-btn primary', \
onclick: () => { parent.postMessage({ type: 'plan-respond', md: r.md }, '*'); state.textContent = 'Sending…'; } }, \
'Send to mem') : null;",
    ),
    // The response so far, for the shell's Approve to count what was never
    // opened.
    (
        "  const r = buildResponse(); const n = r.nChanged + r.nComments + r.nDrafts;",
        "  const r = buildResponse(); const n = r.nChanged + r.nComments + r.nDrafts;\n\
  if (parent !== window) parent.postMessage({ type: 'plan-state', md: r.md }, '*');",
    ),
];

/// Runs in the frame before the runtime: asks the shell for the saved draft,
/// and only then loads the runtime, which reads it at once from `hubStore`.
/// Every write of the draft goes back to the shell.
const BRIDGE: &str = "<script>(function () {\
var saved = null, live = parent !== window, started = false;\
window.hubStore = {\
getItem: function () { return saved; },\
setItem: function (k, v) { saved = String(v); if (live) parent.postMessage({ type: 'plan-draft', state: saved }, '*'); },\
removeItem: function () { saved = null; if (live) parent.postMessage({ type: 'plan-draft', state: null }, '*'); }\
};\
function start(state) {\
if (started) return; started = true; saved = state || null;\
var s = document.createElement('script'); s.src = '/assets/htmlplan.js'; document.head.appendChild(s);\
}\
if (!live) return start(null);\
addEventListener('message', function (e) {\
if (e.source === parent && e.data && e.data.type === 'plan-restore') start(e.data.state);\
});\
parent.postMessage({ type: 'plan-ready' }, '*');\
setTimeout(function () { start(null); }, 1500);\
})();</script>";

/// Whether a stored plan is an html-plan page rather than markdown: a whole
/// document, or a fragment holding the plan element.
pub fn is_plan_page(text: &str) -> bool {
    let start = text.trim_start();
    start
        .get(..15)
        .is_some_and(|head| head.eq_ignore_ascii_case("<!doctype html>"))
        || text.contains("<doc-plan>")
        || text.contains("<doc-plan ")
}

/// The page around the frame: the hub's header, then the plan, as tall as the
/// screen allows.
pub fn plan_shell(project: &str, slug: &str, degraded: Option<&str>) -> String {
    let mut out = detail_head(project, slug);
    if let Some(why) = degraded {
        out.push_str(&degraded_banner(why));
    }
    out.push_str(SHELL_STYLE);
    out.push_str(&plan_frame(project, slug));
    out.push_str("\n</body>\n</html>\n");
    out
}

/// The frame. `allow-scripts` without `allow-same-origin`, so the page's own
/// script can never reach the hub's writes.
pub fn plan_frame(project: &str, slug: &str) -> String {
    let src = format!("{}/plan/{}/page", project_url(project), slug);
    format!(
        "<iframe class=\"plan\" sandbox=\"allow-scripts allow-popups\" src=\"{}\" title=\"plan {}\"></iframe>",
        esc(&src),
        esc(slug),
    )
}

/// `GET /p/<project>/plan/<slug>/page`: the stored page, with its runtime
/// links pointed at the hub's copy. A markdown plan has no such page.
pub fn page_get(ctx: &PageCtx) -> Response {
    let project = ctx.project.unwrap_or_default();
    let slug = ctx.rest;
    if !model::is_slug(slug) {
        return Response::not_found();
    }
    let plan = model::plan_slug_text(&ctx.app.mem, project, slug);
    match plan.value.as_deref() {
        Some(text) if is_plan_page(text) => {
            Response::html(served_page(text)).header("Content-Security-Policy", PAGE_CSP)
        }
        None if plan.degraded.is_some() => Response::text(502, "mem is not answering"),
        _ => Response::not_found(),
    }
}

/// `POST /p/<project>/plan/<slug>/respond`: the response the frame built,
/// as the answer to the roadmap's open review question. With none open, it
/// is saved for the next planner to read.
pub fn respond_post(ctx: &PageCtx) -> Response {
    let project = ctx.project.unwrap_or_default();
    let slug = ctx.rest;
    if !model::is_slug(slug) {
        return Response::not_found();
    }
    let form = ctx.request.form();
    let md = form.get("md").unwrap_or("").trim();
    let back = format!("{}/plan/{slug}", project_url(project));
    if md.is_empty() {
        return Response::see_other(&back);
    }

    // The same lock Approve holds, so a response and an approval cannot both
    // see the question open and both answer it.
    let lock = ctx.app.lock_for(&format!("control:{project}"));
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    let flag = format!("--project={project}");
    let mem = &ctx.app.mem;
    let (run, to) = match pending_approval(ctx, project) {
        Some(id) => {
            let sent = id.get(18..).unwrap_or(&id).to_string();
            (
                mem.write_through(&["answer", &flag, "--", &id, md]),
                format!("{back}?sent={sent}"),
            )
        }
        None => {
            let title = format!("plan response: {slug}");
            (
                mem.write_through(&["save", &flag, "--title", &title, "--", md]),
                format!("{back}?saved=1"),
            )
        }
    };
    if !run.ok() {
        return failed(&run);
    }
    Response::see_other(&to)
}

/// `GET /assets/htmlplan.js` and `/assets/htmlplan.css`.
pub fn asset_get(ctx: &PageCtx) -> Response {
    match ctx.request.path.as_str() {
        "/assets/htmlplan.js" => {
            Response::new(200, "text/javascript; charset=utf-8", served_runtime())
        }
        "/assets/htmlplan.css" => Response::new(200, "text/css; charset=utf-8", RUNTIME_CSS),
        _ => Response::not_found(),
    }
}

/// The stored page with its stylesheet taken from the hub and its runtime
/// script replaced by the bridge, which loads the runtime itself.
fn served_page(text: &str) -> String {
    let text = text.replace("\"htmlplan.css\"", "\"/assets/htmlplan.css\"");
    let Some(at) = text.find("<script src=\"htmlplan.js\"") else {
        return text;
    };
    let end = text[at..]
        .find("</script>")
        .map_or(text.len(), |n| at + n + "</script>".len());
    format!("{}{BRIDGE}{}", &text[..at], &text[end..])
}

fn served_runtime() -> String {
    PATCHES
        .iter()
        .fold(RUNTIME_JS.to_string(), |js, (from, to)| {
            js.replace(from, to)
        })
}

const SHELL_STYLE: &str = "<style>\
body{max-width:none}\
iframe.plan{display:block;width:100%;height:calc(100vh - 6rem);border:0;\
border-top:1px solid #e3e3df}\
</style>\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_is_not_a_plan_page() {
        assert!(!is_plan_page("# plan: m1\n\n- [ ] t1 Build it\n"));
        assert!(is_plan_page("<!doctype html>\n<html><doc-plan></doc-plan>"));
        assert!(is_plan_page("\n<!DOCTYPE html>\n<html>"));
        assert!(is_plan_page("<main><doc-plan>…</doc-plan></main>"));
    }

    #[test]
    fn a_plan_page_runs_in_an_opaque_origin() {
        let html = plan_frame("workflow", "h1-plan-pages");
        assert!(html.contains("sandbox=\"allow-scripts allow-popups\""));
        assert!(!html.contains("allow-same-origin"));
        assert!(html.contains("src=\"/p/workflow/plan/h1-plan-pages/page\""));
    }

    #[test]
    fn the_runtime_still_has_what_the_hub_patches() {
        for (from, _) in &PATCHES[1..] {
            assert_eq!(RUNTIME_JS.matches(from).count(), 1, "{from}");
        }
        assert!(RUNTIME_JS.contains("localStorage"));
        assert!(!served_runtime().contains("localStorage"));
    }

    #[test]
    fn the_page_loads_the_bridge_in_place_of_the_runtime() {
        let page = served_page(
            "<link rel=\"stylesheet\" href=\"htmlplan.css\">\n\
             <script src=\"htmlplan.js\" defer></script>\n<doc-plan></doc-plan>",
        );
        assert!(page.contains("href=\"/assets/htmlplan.css\""), "{page}");
        assert!(page.contains("window.hubStore"), "{page}");
        assert!(!page.contains("src=\"htmlplan.js\""), "{page}");
        assert!(page.ends_with("</script>\n<doc-plan></doc-plan>"), "{page}");
    }
}
