//! The start prompt and the orchestrate skill's cloud page tell a thread
//! woken after its park to carry on (m1-wake-resume): they said "do no more
//! work" and "never run `workflow go` again", so a message Saiful typed into
//! a parked thread did nothing.

use std::fs;
use std::path::Path;

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn texts() -> [(&'static str, String); 2] {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let read = |rel: &str| one_line(&fs::read_to_string(repo.join(rel)).unwrap());
    [
        ("cloud-prompt.txt", read("workflow/src/cloud-prompt.txt")),
        ("cloud.md", read("skills/orchestrate/cloud.md")),
    ]
}

#[test]
fn a_woken_thread_is_told_to_carry_on() {
    for (name, text) in texts() {
        assert!(!text.contains("do no more work"), "{name}");
        assert!(!text.contains("never run `workflow go` again"), "{name}");
        assert!(
            text.contains("On every new turn after a park"),
            "{name}: {text}"
        );
        assert!(
            text.contains("--cloud --print` and follow what it prints"),
            "{name}"
        );
        assert!(
            text.contains("A message Saiful typed is the instruction"),
            "{name}"
        );
        assert!(text.contains("record it with `mem answer <id>"), "{name}");
        assert!(
            text.contains("the work that does not wait on the question"),
            "{name}"
        );
        assert!(
            text.contains("wait on the question for 30 minutes again"),
            "{name}"
        );
    }
}

/// A thread has no question dialog of its own: it asks only with mem.
#[test]
fn the_cloud_page_asks_only_with_mem() {
    let [_, (_, page)] = texts();
    assert!(
        page.contains("asks Saiful only with `mem ask --for saiful`"),
        "{page}"
    );
}

/// Export sends to the store first, so a thread commits only what it left
/// staged (m2b-clean-commits): "commit the file it stages" put an outbox
/// commit on the branch for writes the store took a second later.
#[test]
fn a_thread_commits_only_what_export_left_staged() {
    for (name, text) in texts() {
        assert!(
            text.contains(
                "the `.mem-outbox.json` or its removal; with nothing staged, commit nothing"
            ),
            "{name}: {text}"
        );
        assert!(!text.contains("it stages, when it stages one"), "{name}");
    }
}
