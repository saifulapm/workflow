//! The hub home, built from the store's project records. Pure functions, so
//! `cargo test` on the host covers them without a Worker.

use serde_json::Value;

/// Escape text for HTML content and attribute values.
pub fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// The lifecycle stage of one `mem projects --json` row, the same rule as
/// hub/src/page_home.rs `lifecycle_stage`. A roadmap with every milestone
/// ticked is done whatever status it stores.
pub fn lifecycle_stage(row: &Value) -> &'static str {
    let total = count(row, "milestones_total");
    if total > 0 && count(row, "milestones_done") == total {
        return "done";
    }
    match row["roadmap_status"].as_str() {
        Some("draft") => "planning",
        Some("approved") => {
            // The current plan is the open milestone's and every task in it
            // is ticked: the milestone is built and waiting on its walk.
            let open = row["milestone"].as_str();
            let total = count(row, "plan_total");
            if open.is_some()
                && open == row["plan_slug"].as_str()
                && total > 0
                && count(row, "plan_ticked") == total
            {
                "dogfooding"
            } else {
                "execution"
            }
        }
        // Statuses an older roadmap may still store.
        Some("maintenance" | "done") => "done",
        _ => {
            let has = |key: &str| row[key].as_bool().unwrap_or(false);
            if has("has_spec") {
                "spec"
            } else if has("has_research_summary") {
                "grilling"
            } else if has("has_research") {
                "research"
            } else {
                "brief"
            }
        }
    }
}

/// The pill's class for a stage, as the hub draws it.
fn stage_pill(stage: &str) -> &'static str {
    match stage {
        "execution" | "dogfooding" => "pill",
        "planning" => "pill wait",
        "done" => "pill ok",
        _ => "pill mut",
    }
}

fn count(row: &Value, key: &str) -> u64 {
    row[key].as_u64().unwrap_or(0)
}

/// One project as a card: its name, its stage and the open milestone with
/// its place among the roadmap's milestones.
fn project_card(row: &Value) -> String {
    let name = row["name"].as_str().unwrap_or("");
    let stage = lifecycle_stage(row);
    let mut parts = Vec::new();
    if let Some(milestone) = row["milestone"].as_str() {
        parts.push(esc(milestone));
    }
    let (done, total) = (
        count(row, "milestones_done"),
        count(row, "milestones_total"),
    );
    if total > 0 {
        // The open milestone's place; the last one once every milestone is
        // ticked.
        parts.push(format!("milestone {} of {total}", (done + 1).min(total)));
    }
    let meta = if parts.is_empty() {
        String::new()
    } else {
        format!("<div class=\"meta\">{}</div>\n", parts.join(" · "))
    };
    format!(
        "<article class=\"card\">\n\
         <div class=\"row\"><h3>{name}</h3>\
         <span class=\"{pill}\">{stage}</span></div>\n\
         {meta}</article>\n",
        // No link yet: hubweb serves no project page until m4 moves them.
        name = esc(name),
        pill = stage_pill(stage),
    )
}

/// The whole home page, one card per project record.
pub fn render_home(projects: &[Value]) -> String {
    let cards: String = projects.iter().map(project_card).collect();
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>hub</title><style>{STYLE}</style></head>\n<body>\n\
         <header class=\"top\"><a class=\"brand\" href=\"/\">hub</a></header>\n\
         <h2>Projects</h2>\n<div class=\"cards\">\n{cards}</div>\n</body></html>\n"
    )
}

/// The part of the hub's stylesheet (hub/src/html.rs) the home uses.
const STYLE: &str = r#"
:root{color-scheme:light dark;--bg:#f7f7f5;--surface:#fff;--line:#e9e9e5;--track:#ececea;--ink:#17181c;--mut:#6b6d75;
--accent:#2b59c3;--accent-soft:#e8eefb;--ok:#1f7a52;--ok-soft:#e3f4ec;--wait:#9a5b00;--wait-soft:#fdf1dc;
--radius:12px;--font:ui-monospace,monospace}
@media (prefers-color-scheme:dark){:root:not([data-theme=light]){--bg:#0f1115;--surface:#171a21;--line:#262a33;--track:#262a33;--ink:#e7e9ee;--mut:#9aa0ab;
--accent:#7ea2f2;--accent-soft:#1c2742;--ok:#5cc996;--ok-soft:#15291f;--wait:#e7ad52;--wait-soft:#2e2413}}
*{box-sizing:border-box}
html{-webkit-text-size-adjust:100%}
body{margin:0;padding:0 16px 48px;background:var(--bg);color:var(--ink);font-size:14px;line-height:1.6;font-family:var(--font)}
body>*{max-width:60rem;margin-inline:auto}
a{color:var(--accent);text-decoration:none}
header.top{max-width:none;display:flex;align-items:center;gap:10px;margin:0 -16px 14px;padding:12px max(16px,calc((100% - 60rem)/2 + 16px));
background:var(--surface);border-bottom:1px solid var(--line)}
header.top .brand{display:inline-flex;align-items:center;gap:7px;font-weight:700;color:var(--ink)}
header.top .brand::before{content:"";width:14px;height:14px;border-radius:4px;background:var(--accent)}
h2{font-size:12px;font-weight:600;text-transform:uppercase;letter-spacing:.08em;color:var(--mut);margin-block:28px 10px}
.meta{font-size:12.5px;color:var(--mut);overflow-wrap:anywhere}
.card{background:var(--surface);border:1px solid var(--line);border-radius:var(--radius);padding:14px 16px}
.cards{display:grid;gap:10px;grid-template-columns:minmax(0,1fr);margin-block:0 10px}
@media (min-width:1100px){.cards{grid-template-columns:repeat(2,minmax(0,1fr))}}
.card h3{margin:0;font-size:15px}
.card>.meta{margin-top:4px}
.row{display:flex;align-items:center;gap:8px;flex-wrap:wrap}
.pill{display:inline-block;font-size:11.5px;font-weight:600;line-height:1.7;padding:0 9px;border-radius:99px;background:var(--accent-soft);color:var(--accent);white-space:nowrap}
.pill.ok{background:var(--ok-soft);color:var(--ok)}
.pill.wait{background:var(--wait-soft);color:var(--wait)}
.pill.mut{background:var(--track);color:var(--mut)}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn esc_escapes_markup_and_quotes() {
        assert_eq!(
            esc(r#"<a href="x">Tom & Jerry's</a>"#),
            "&lt;a href=&quot;x&quot;&gt;Tom &amp; Jerry&#39;s&lt;/a&gt;"
        );
    }

    #[test]
    fn approved_roadmap_with_open_tasks_is_execution() {
        let row = json!({"roadmap_status": "approved", "milestone": "m1-x", "plan_slug": "m1-x",
            "plan_ticked": 1, "plan_total": 3, "milestones_done": 0, "milestones_total": 2});
        assert_eq!(lifecycle_stage(&row), "execution");
    }

    #[test]
    fn every_milestone_ticked_is_done_whatever_the_status() {
        let row =
            json!({"roadmap_status": "approved", "milestones_done": 2, "milestones_total": 2});
        assert_eq!(lifecycle_stage(&row), "done");
    }

    #[test]
    fn open_milestone_with_every_task_ticked_is_dogfooding() {
        let row = json!({"roadmap_status": "approved", "milestone": "m1-x", "plan_slug": "m1-x",
            "plan_ticked": 3, "plan_total": 3, "milestones_done": 0, "milestones_total": 2});
        assert_eq!(lifecycle_stage(&row), "dogfooding");
    }

    #[test]
    fn ticked_plan_of_another_milestone_is_still_execution() {
        let row = json!({"roadmap_status": "approved", "milestone": "m2-y", "plan_slug": "m1-x",
            "plan_ticked": 3, "plan_total": 3, "milestones_done": 1, "milestones_total": 2});
        assert_eq!(lifecycle_stage(&row), "execution");
    }

    #[test]
    fn draft_roadmap_is_planning_and_old_statuses_are_done() {
        assert_eq!(
            lifecycle_stage(&json!({"roadmap_status": "draft"})),
            "planning"
        );
        assert_eq!(
            lifecycle_stage(&json!({"roadmap_status": "maintenance"})),
            "done"
        );
    }

    #[test]
    fn without_a_roadmap_the_furthest_document_names_the_stage() {
        assert_eq!(
            lifecycle_stage(&json!({"has_spec": true, "has_research": true})),
            "spec"
        );
        assert_eq!(
            lifecycle_stage(&json!({"has_research_summary": true})),
            "grilling"
        );
        assert_eq!(lifecycle_stage(&json!({"has_research": true})), "research");
        assert_eq!(lifecycle_stage(&json!({})), "brief");
    }

    #[test]
    fn home_card_shows_name_stage_milestone_and_place() {
        let row = json!({"name": "walkone", "roadmap_status": "approved", "milestone": "m1-x",
            "plan_slug": "m1-x", "plan_ticked": 1, "plan_total": 3,
            "milestones_done": 0, "milestones_total": 2});
        let html = render_home(&[row]);
        assert!(html.contains("<h3>walkone</h3>"), "{html}");
        assert!(
            html.contains(r#"<span class="pill">execution</span>"#),
            "{html}"
        );
        let place = format!("m1-x · milestone {} of 2", 1);
        assert!(html.contains(&place), "{html}");
    }

    #[test]
    fn home_escapes_what_a_record_holds() {
        let row = json!({"name": "<b>x</b>", "milestone": "a&b"});
        let html = render_home(&[row]);
        assert!(html.contains("&lt;b&gt;x&lt;/b&gt;"), "{html}");
        assert!(html.contains("a&amp;b"), "{html}");
        assert!(!html.contains("<b>x"), "{html}");
    }

    #[test]
    fn a_card_links_to_no_page_hubweb_does_not_serve() {
        let home =
            render_home(&[serde_json::json!({"name": "walkone", "roadmap_status": "approved"})]);
        assert!(home.contains("walkone"));
        assert!(!home.contains("/p/"), "{home}");
    }
}
