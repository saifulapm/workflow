//! `GET /assets/<font>`: the hub's one typeface, carried in the binary so a
//! page on the tailnet fetches nothing from another host.

mod common;

use common::{Hub, TempDir, body_of, fixture_mem, header_of, status_of};

fn hub(tag: &str) -> (TempDir, Hub) {
    let dir = TempDir::new(tag);
    let home = dir.join("home");
    let bin = dir.join("bin");
    fixture_mem(&bin, "echo '{\"projects\":[]}'");
    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);
    (dir, hub)
}

#[test]
fn each_maple_mono_face_is_served_as_woff2_with_a_long_cache() {
    let (_dir, hub) = hub("assets-fonts");
    for face in [
        "maple-mono-400",
        "maple-mono-600",
        "maple-mono-700",
        "maple-mono-400-italic",
    ] {
        let response = hub.get(&format!("/assets/{face}.woff2"));
        assert_eq!(status_of(&response), 200, "{face}");
        assert_eq!(header_of(&response, "content-type"), Some("font/woff2"));
        assert_eq!(
            header_of(&response, "cache-control"),
            Some("public, max-age=31536000, immutable"),
            "{face}"
        );
        assert!(body_of(&response).starts_with("wOF2"), "{face} is a woff2");
    }
}

#[test]
fn the_fonts_license_is_served_beside_them() {
    let (_dir, hub) = hub("assets-license");
    let response = hub.get("/assets/maple-mono-OFL.txt");
    assert_eq!(status_of(&response), 200);
    assert!(
        header_of(&response, "content-type")
            .unwrap()
            .starts_with("text/plain")
    );
    assert!(body_of(&response).contains("SIL Open Font License, Version 1.1"));
}

#[test]
fn an_asset_the_binary_does_not_carry_is_404() {
    let (_dir, hub) = hub("assets-missing");
    assert_eq!(status_of(&hub.get("/assets/maple-mono-900.woff2")), 404);
    assert_eq!(status_of(&hub.get("/assets/../Cargo.toml")), 404);
}

#[test]
fn the_fonts_answer_a_frame_with_no_origin() {
    // A plan page runs in a sandboxed frame with an opaque origin, and a
    // font is a CORS fetch: without this header the page falls back to the
    // platform's monospace.
    let (_dir, hub) = hub("assets-cors");
    let response = hub.get("/assets/maple-mono-400.woff2");
    assert_eq!(
        header_of(&response, "access-control-allow-origin"),
        Some("*")
    );
}

#[test]
fn the_comment_layer_is_served_as_javascript() {
    // A designed plan page loads it from its sandboxed frame.
    let (_dir, hub) = hub("assets-annotate");
    let response = hub.get("/assets/annotate.js");
    assert_eq!(status_of(&response), 200, "{response}");
    assert!(
        header_of(&response, "content-type")
            .unwrap()
            .starts_with("text/javascript")
    );
    assert!(body_of(&response).contains("plan-comment"));
    // Its URL carries no version, and the shell that talks to it is always
    // fresh, so a phone must not keep an old copy for a year.
    assert_eq!(header_of(&response, "cache-control"), Some("no-cache"));
}
