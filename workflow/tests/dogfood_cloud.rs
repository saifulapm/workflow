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
