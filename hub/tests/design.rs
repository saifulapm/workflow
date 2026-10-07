//! Every page draws from one stylesheet: the same tokens, Maple Mono from
//! the hub itself, a dark scheme, and nothing loaded from another host.

mod common;

use common::{Hub, TempDir, body_of, fixture_mem, status_of};

const PROJECT: &str = "proj-look";
/// A pairing code that stays pending long after the suite has finished.
const PAIR_CODE: &str = "DESK42";

/// A `mem` that knows one project and answers everything else with junk:
/// the look is the shell's, so it holds on a page with nothing to show.
fn hub(tag: &str) -> (TempDir, Hub) {
    let dir = TempDir::new(tag);
    let home = dir.join("home");
    let bin = dir.join("bin");
    fixture_mem(
        &bin,
        &format!(
            "if [ \"$1\" = projects ]; then echo '{{\"projects\":[{{\"name\":\"{PROJECT}\"}}]}}'; \
             else echo 'not json at all'; fi"
        ),
    );
    std::fs::create_dir_all(home.join("state/hub")).unwrap();
    std::fs::write(
        home.join("state/hub/pair"),
        format!("{PAIR_CODE} 99999999999999\n"),
    )
    .unwrap();
    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);
    (dir, hub)
}

fn every_page() -> Vec<String> {
    let mut paths: Vec<String> = ["/", "/wiki", "/subscribe"].map(str::to_string).to_vec();
    paths.push(format!("/pair/{PAIR_CODE}"));
    for page in [
        "",
        "/roadmap",
        "/questions",
        "/evidence",
        "/wiki",
        "/decisions",
        "/new",
        "/log",
        "/plan",
    ] {
        paths.push(format!("/p/{PROJECT}{page}"));
    }
    paths
}

#[test]
fn every_page_uses_the_tokens_and_maple_mono_in_light_and_dark() {
    let (_dir, hub) = hub("design-tokens");
    for path in every_page() {
        let response = hub.get(&path);
        assert_eq!(status_of(&response), 200, "{path}");
        let body = body_of(&response);
        for needle in [
            "--accent:#2b59c3",
            "--font:\"Maple Mono\"",
            "src:url(/assets/maple-mono-400.woff2) format(\"woff2\")",
            "src:url(/assets/maple-mono-600.woff2) format(\"woff2\")",
            "src:url(/assets/maple-mono-700.woff2) format(\"woff2\")",
            "src:url(/assets/maple-mono-400-italic.woff2) format(\"woff2\")",
            "font-family:var(--font)",
            "@media (prefers-color-scheme:dark){:root:not([data-theme=light]){--bg:#0f1115",
        ] {
            assert!(body.contains(needle), "{path} lacks {needle}");
        }
        assert!(!body.contains("system-ui"), "{path} names another font");
    }
}

#[test]
fn no_page_loads_anything_from_another_host() {
    let (_dir, hub) = hub("design-hosts");
    for path in every_page() {
        let body = body_of(&hub.get(&path)).to_ascii_lowercase();
        for needle in [
            "src=\"http",
            "src=\"//",
            "url(http",
            "url(//",
            "@import",
            "rel=\"stylesheet\" href=\"http",
        ] {
            assert!(
                !body.contains(needle),
                "{path} loads from another host: {needle}"
            );
        }
    }
}

#[test]
fn every_page_opens_with_the_same_top_bar() {
    let (_dir, hub) = hub("design-top");
    for path in every_page() {
        let body = body_of(&hub.get(&path)).to_string();
        assert!(body.contains("<header class=\"top\">"), "{path}: {body}");
        assert!(
            body.contains("<a class=\"brand\" href=\"/\">hub</a>"),
            "{path}: {body}"
        );
        if path.starts_with("/p/") {
            assert!(
                body.contains(&format!("<a href=\"/p/{PROJECT}\">{PROJECT}</a>")),
                "{path}: {body}"
            );
        }
    }
}

/// The stylesheet as every page carries it.
fn stylesheet() -> String {
    let (_dir, hub) = hub("design-sheet");
    let body = body_of(&hub.get("/wiki")).to_string();
    let start = body.find("<style>").expect("a style block");
    let end = body.find("</style>").expect("its end");
    body[start..end].to_string()
}

#[test]
fn a_docs_page_keeps_wide_content_inside_the_phone_width() {
    // A grid column with no width grows to its widest line, so a long code
    // line would push the whole page sideways below 960 px.
    let sheet = stylesheet();
    assert!(
        sheet.contains(".docs{display:grid;grid-template-columns:minmax(0,1fr);"),
        "{sheet}"
    );
}

#[test]
fn a_heading_reached_by_its_anchor_clears_the_sticky_bar() {
    let sheet = stylesheet();
    assert!(
        sheet.contains("article.md :is(h1,h2,h3,h4){scroll-margin-top:64px}"),
        "{sheet}"
    );
}

#[test]
fn a_select_draws_its_own_arrow_in_the_theme_colours() {
    // A native select keeps the platform's grey control and font on a phone,
    // so the hub draws the control itself, its arrow from the muted token.
    let sheet = stylesheet();
    assert!(
        sheet.contains("select{-webkit-appearance:none;appearance:none;"),
        "{sheet}"
    );
    assert!(
        sheet.contains("linear-gradient(45deg,transparent 50%,var(--mut) 50%)"),
        "{sheet}"
    );
    let shared = sheet
        .find("textarea,input[type=text],input[type=search],select{")
        .expect("the shared field rule");
    let own = sheet
        .find("select{-webkit-appearance")
        .expect("the select rule");
    assert!(
        own > shared,
        "the select's own rule must come after the shared one"
    );
}

#[test]
fn form_fields_are_sixteen_pixels_so_ios_does_not_zoom() {
    // iOS Safari zooms the page into any field whose text is under 16 px.
    let sheet = stylesheet();
    assert!(
        sheet.contains(
            "textarea,input[type=text],input[type=search],select{width:100%;font-size:16px;"
        ),
        "{sheet}"
    );
    assert!(
        sheet.contains("input[type=number]{width:6rem;font-size:16px;"),
        "{sheet}"
    );
}
