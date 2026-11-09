//! The dogfood skill's cloud paragraph says what a cloud walk drives: web
//! pages through playwright-cli, headless, since the cloud setup installs it.
//! It said "no browser" while the linesofus cloud walks drove Chromium.

use std::fs;
use std::path::Path;

#[test]
fn the_dogfood_skill_walks_cloud_web_pages_with_playwright_cli() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let skill = fs::read_to_string(repo.join("skills/dogfood/SKILL.md")).unwrap();
    let at = skill
        .find("In a cloud session")
        .expect("the skill has a cloud paragraph");
    let paragraph = &skill[at..skill[at..].find("\n\n").map_or(skill.len(), |end| at + end)];
    // One line, so a phrase the wrapping split still matches.
    let paragraph = paragraph.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(paragraph.contains("playwright-cli"), "{paragraph}");
    assert!(paragraph.contains("headless"), "{paragraph}");
    assert!(!paragraph.contains("no browser"), "{paragraph}");
    assert!(!paragraph.contains("no `playwright-cli`"), "{paragraph}");
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// m2-device: a cloud walk reaches Saiful's logged-in Chromium through
/// `device browser`, under the browser-operator rules, and a run that finds
/// no device parks its task local-only.
#[test]
fn a_cloud_walk_reaches_the_logged_in_browser_through_device() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let skill = one_line(&fs::read_to_string(repo.join("skills/dogfood/SKILL.md")).unwrap());
    assert!(
        !skill.contains("no logged-in browser"),
        "the dogfood skill still says the cloud has no logged-in browser"
    );
    for said in [
        "`device browser <playwright-cli args>`",
        "`device browser detach`",
        "the brief names",
        "exits 3",
    ] {
        assert!(skill.contains(said), "the dogfood skill says {said}");
    }

    let cloud = one_line(&fs::read_to_string(repo.join("skills/orchestrate/cloud.md")).unwrap());
    for said in [
        "`device whoami`",
        "parked: local-only <task>: no device online",
    ] {
        assert!(cloud.contains(said), "cloud.md says {said}");
    }
}
