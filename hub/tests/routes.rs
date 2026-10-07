//! Every page under `/p/<project>` has a route: the shell answers before a
//! page has anything of its own to show, and the checks in front of it hold
//! for every page alike.

mod common;

use common::{Hub, TempDir, body_of, fixture_mem, header_of, status_of};

const PROJECT: &str = "proj-pages";

/// A `mem` that knows one project and answers everything else with junk, so
/// no page here can lean on a real store.
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
    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);
    (dir, hub)
}

const NAV: [&str; 8] = [
    "href=\"/\"",
    "href=\"/p/proj-pages\"",
    "href=\"/p/proj-pages/roadmap\"",
    "href=\"/p/proj-pages/questions\"",
    "href=\"/p/proj-pages/evidence\"",
    "href=\"/p/proj-pages/wiki\"",
    "href=\"/p/proj-pages/decisions\"",
    "href=\"/p/proj-pages/new\"",
];

#[test]
fn each_plain_page_answers_200_with_the_project_and_the_nav() {
    let (_dir, hub) = hub("routes-pages");
    for page in ["questions", "evidence", "wiki", "decisions", "new"] {
        let response = hub.get(&format!("/p/{PROJECT}/{page}"));
        assert_eq!(status_of(&response), 200, "{page}: {response}");
        let body = body_of(&response);
        assert!(body.contains(PROJECT), "{page}: {body}");
        for link in NAV {
            assert!(body.contains(link), "{page} lacks {link}: {body}");
        }
    }
}

#[test]
fn a_page_of_an_unknown_project_is_404() {
    let (_dir, hub) = hub("routes-unknown");
    for page in ["questions", "evidence", "wiki", "decisions", "new"] {
        let response = hub.get(&format!("/p/no-such-project/{page}"));
        assert_eq!(status_of(&response), 404, "{page}: {response}");
    }
}

#[test]
fn there_is_no_run_page() {
    let (_dir, hub) = hub("routes-no-run");
    let response = hub.get(&format!("/p/{PROJECT}/run"));
    assert_eq!(status_of(&response), 404, "{response}");
}

#[test]
fn a_wrong_method_is_405_naming_the_right_one() {
    let (_dir, hub) = hub("routes-methods");
    let response = hub.post_form(&format!("/p/{PROJECT}/questions"), "");
    assert_eq!(status_of(&response), 405, "{response}");
    assert_eq!(header_of(&response, "Allow"), Some("GET"), "{response}");

    for path in ["control", "new/idea"] {
        let response = hub.get(&format!("/p/{PROJECT}/{path}"));
        assert_eq!(status_of(&response), 405, "{path}: {response}");
        assert_eq!(header_of(&response, "Allow"), Some("POST"), "{response}");
    }
}

#[test]
fn a_cross_origin_post_is_refused() {
    let (_dir, hub) = hub("routes-origin");
    for path in ["control", "new/idea"] {
        let response = hub.post_form_with(
            &format!("/p/{PROJECT}/{path}"),
            "",
            &[("Origin", "http://evil.example")],
        );
        assert_eq!(status_of(&response), 403, "{path}: {response}");
    }
}
