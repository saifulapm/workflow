//! Home reloads only when what it shows has changed: `/api/stamp` answers
//! a stamp of home's reads, home carries the stamp it was drawn from, and
//! its script compares the two every 15 s.

mod common;

use std::time::Duration;

use common::{Hub, TempDir, body_of, mem_in, real_mem, recording_mem, seed_project, wait_for};

fn stamp(hub: &Hub) -> String {
    let body = body_of(&hub.get("/api/stamp")).to_string();
    let doc: serde_json::Value = serde_json::from_str(&body).unwrap_or_else(|_| panic!("{body}"));
    doc["stamp"].as_str().unwrap().to_string()
}

#[test]
fn the_stamp_holds_while_nothing_changes_and_moves_with_a_new_question() {
    let dir = TempDir::new("home-stamp");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let (bin, _log) = recording_mem(dir.path(), &home);
    let mem = real_mem().unwrap();
    seed_project(&mem, &home, "gamma", "gamma did a thing");
    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);

    let first = stamp(&hub);
    assert!(!first.is_empty());
    let page = body_of(&hub.get("/")).to_string();
    assert!(
        page.contains(&format!("\"{first}\"")),
        "home names its stamp: {page}"
    );
    assert!(page.contains("/api/stamp"), "{page}");
    assert!(
        !page.contains("setInterval(function(){var a"),
        "no timed reload: {page}"
    );
    std::thread::sleep(Duration::from_secs(6));
    assert_eq!(stamp(&hub), first, "nothing changed");

    let out = mem_in(
        &mem,
        &home,
        &home.join("gamma"),
        &["ask", "--for", "human", "Which hour?"],
    );
    assert!(out.status.success(), "{out:?}");
    wait_for("the stamp to move", Duration::from_secs(15), || {
        stamp(&hub) != first
    });
}
