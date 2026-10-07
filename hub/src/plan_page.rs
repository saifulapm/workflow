//! A stored html-plan page in the hub (h1-plan-pages): the shell the hub
//! draws around it, and the frame the page itself runs in.
//!
//! The page is whatever the planner wrote, script included, so it never runs
//! on the hub's own origin. The frame is sandboxed without
//! `allow-same-origin`: the page gets an opaque origin, and its script can
//! reach the hub only through the messages the shell chooses to act on.

use crate::html::{detail_head, esc, project_url};
use crate::http::Response;
use crate::memcli::Outcome;
use crate::model;
use crate::page_control::{failed, pending_approval};
use crate::page_roadmap::roadmap_rows;
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

/// `GET /p/<project>/plan/<slug>`: an html-plan page in its shell. A
/// markdown plan, or none, is the project's detail page as before.
pub fn get(ctx: &PageCtx) -> Response {
    let project = ctx.project.unwrap_or_default();
    let slug = ctx.rest;
    let plan = model::is_slug(slug).then(|| model::plan_slug_text(&ctx.app.mem, project, slug));
    match plan {
        Some(plan) if plan.value.as_deref().is_some_and(is_plan_page) => {
            let text = plan.value.as_deref().unwrap_or_default();
            let mut banner = String::new();
            if let Some(id) = ctx.request.query.get("sent").filter(|id| is_short_id(id)) {
                banner = format!("<p class=\"banner ok\">Sent. Answer #{id} is in mem.</p>\n");
            } else if ctx.request.query.get("saved").is_some() {
                banner = "<p class=\"banner ok\">Saved for the next planner.</p>\n".to_string();
            }
            banner.push_str(&approval(ctx, project));
            Response::html(plan_shell(project, slug, text, &banner))
        }
        _ => {
            let path = ctx.request.path.strip_prefix("/p/").unwrap_or_default();
            ctx.app.project_page(path)
        }
    }
}

/// While the roadmap is a draft, its approval: what it covers, then the
/// control route's Approve. The shell's script adds how many of this page's
/// decisions were never opened, since a default kept unread is not agreement.
fn approval(ctx: &PageCtx, project: &str) -> String {
    let roadmap = ctx.app.mem.roadmap(project);
    let Outcome::Json(doc) = &*roadmap else {
        return String::new();
    };
    if doc["status"] != "draft" {
        return String::new();
    }
    let milestones: Vec<String> = roadmap_rows(doc["text"].as_str().unwrap_or_default())
        .into_iter()
        .map(|row| row.slug)
        .collect();
    let pages = ctx
        .app
        .mem
        .plan_list(project)
        .rows("plans")
        .iter()
        .filter(|row| {
            row["slug"]
                .as_str()
                .is_some_and(|slug| milestones.iter().any(|m| m == slug))
        })
        .count();
    let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    format!(
        "<details class=\"approve\"><summary id=\"plan-approve\">Approve the roadmap</summary>\n\
         <p>{} · {}<span id=\"plan-unopened\"></span></p>\n\
         <form method=\"post\" action=\"{}\">\
         <input type=\"hidden\" name=\"do\" value=\"approve\">\
         <button type=\"submit\">Approve</button></form>\n</details>\n",
        plural(milestones.len(), "milestone"),
        plural(pages, "plan page"),
        esc(&format!("{}/control", project_url(project))),
    )
}

/// The page around the frame: the hub's header, the form a response is
/// sent with, the script that relays between the frame and the hub, then the
/// plan, as tall as the screen allows.
fn plan_shell(project: &str, slug: &str, text: &str, banner: &str) -> String {
    let mut out = detail_head(project, slug);
    out.push_str(banner);
    out.push_str(SHELL_STYLE);
    out.push_str(&format!(
        "<form id=\"plan-respond\" method=\"post\" action=\"{}\" hidden>\
         <input type=\"hidden\" name=\"md\"></form>\n",
        esc(&format!("{}/plan/{slug}/respond", project_url(project))),
    ));
    // The draft is kept per revision of the page: answers to a page since
    // rewritten would be answers to questions no longer asked.
    let key = format!("plan:{project}:{slug}:{:016x}", fnv1a(text));
    out.push_str(&SHELL_SCRIPT.replace("KEY", &esc(&key)));
    out.push_str(&plan_frame(project, slug));
    out.push_str("\n</body>\n</html>\n");
    out
}

/// Acts only on what comes from its own frame. A response is sent only
/// while a tap is live: a tap inside the frame activates this page too, and a
/// message the page's script posts on its own does not, so the page cannot
/// answer in Saiful's name. Coming back from a send clears the draft once,
/// then drops the query, so a reload later does not clear a new one.
const SHELL_SCRIPT: &str = "<script>(function () {\
var key = 'KEY', store = {};\
try { store = localStorage; } catch (e) {}\
function get() { try { return store.getItem(key); } catch (e) { return null; } }\
function put(v) { try { v == null ? store.removeItem(key) : store.setItem(key, v); } catch (e) {} }\
if (/[?&](sent|saved)=/.test(location.search)) { put(null); history.replaceState(null, '', location.pathname); }\
function unopened(n) {\
var label = document.getElementById('plan-approve'), line = document.getElementById('plan-unopened');\
if (!label) return;\
label.textContent = n ? 'Approve the roadmap (' + n + ' not opened)' : 'Approve the roadmap';\
line.textContent = n ? ' · ' + n + (n === 1 ? ' decision' : ' decisions') + ' not opened' : '';\
}\
addEventListener('message', function (e) {\
var frame = document.querySelector('iframe.plan'), d = e.data;\
if (!frame || e.source !== frame.contentWindow || !d) return;\
if (d.type === 'plan-ready') frame.contentWindow.postMessage({ type: 'plan-restore', state: get() }, '*');\
else if (d.type === 'plan-draft') put(d.state);\
else if (d.type === 'plan-state' && typeof d.md === 'string') unopened((d.md.match(/not opened; default kept/g) || []).length);\
else if (d.type === 'plan-respond' && typeof d.md === 'string' && navigator.userActivation && navigator.userActivation.isActive) {\
var form = document.getElementById('plan-respond'); form.md.value = d.md; form.submit();\
}\
});\
})();</script>\n";

/// mem's short id, as `respond_post` puts it in the query.
fn is_short_id(id: &str) -> bool {
    id.len() == 8 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// A stable hash of the page's text, so a draft key outlives a hub upgrade.
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
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
    let md = form.get("md").unwrap_or("").replace("\r\n", "\n");
    let md = md.trim();
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
details.approve{margin:.5rem 0}details.approve summary{font-weight:600;cursor:pointer}\
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
