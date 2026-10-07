//! A stored html-plan page in the hub (h1-plan-pages): the shell the hub
//! draws around it, and the frame the page itself runs in.
//!
//! The page is whatever the planner wrote, script included, so it never runs
//! on the hub's own origin. The frame is sandboxed without
//! `allow-same-origin`: the page gets an opaque origin, and its script can
//! reach the hub only through the messages the shell chooses to act on.

use crate::html::{degraded_banner, detail_head, esc, project_url};

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
}
