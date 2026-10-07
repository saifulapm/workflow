//! A comment on a plan page names the part it is about: `mem ask --about`
//! and `mem save --about` keep it on the item, and `--about <prefix>` reads
//! back every comment on one page (h4-designed-plans).

mod common;

use common::{World, code, mem, stdout};

fn json(out: &std::process::Output) -> serde_json::Value {
    assert_eq!(code(out), 0, "{out:?}");
    serde_json::from_str(&stdout(out)).unwrap()
}

#[test]
fn a_question_keeps_what_it_is_about() {
    let w = World::new("about-ask");
    let repo = w.repo("thing", None);
    let ask = [
        "ask",
        "--for",
        "orchestrator",
        "--about",
        "plan:h4#claim-2",
        "--",
        "why a pin?",
    ];
    assert_eq!(code(&mem(&w, &repo, &ask)), 0);

    let doc = json(&mem(&w, &repo, &["questions", "--json"]));
    assert_eq!(doc["questions"][0]["about"], "plan:h4#claim-2", "{doc:#}");
}

#[test]
fn questions_about_a_page_come_back_with_their_answers() {
    let w = World::new("about-questions");
    let repo = w.repo("thing", None);
    for (about, text) in [
        ("plan:h4#claim-2/3@40,60", "on h4"),
        ("plan:h3#claim-1", "on h3"),
    ] {
        let ask = ["ask", "--for", "orchestrator", "--about", about, "--", text];
        assert_eq!(code(&mem(&w, &repo, &ask)), 0);
    }
    assert_eq!(code(&mem(&w, &repo, &["ask", "--", "about nothing"])), 0);
    let all = json(&mem(&w, &repo, &["questions", "--json"]));
    let id = all["questions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["body"] == "on h4")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        code(&mem(&w, &repo, &["answer", &id, "it holds the spot"])),
        0
    );

    let doc = json(&mem(
        &w,
        &repo,
        &["questions", "--about", "plan:h4#", "--json"],
    ));
    let rows = doc["questions"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{doc:#}");
    assert_eq!(rows[0]["about"], "plan:h4#claim-2/3@40,60");
    assert_eq!(rows[0]["answer"], "it holds the spot");
}

#[test]
fn a_note_about_a_page_is_read_back_with_its_text() {
    let w = World::new("about-note");
    let repo = w.repo("thing", None);
    let save = [
        "save",
        "--type",
        "comment",
        "--about",
        "plan:h4#claim-1/2@5,5",
        "--",
        "nice picture",
    ];
    assert_eq!(code(&mem(&w, &repo, &save)), 0);
    assert_eq!(
        code(&mem(&w, &repo, &["save", "--", "a fact on its own"])),
        0
    );

    let doc = json(&mem(&w, &repo, &["log", "--about", "plan:h4#", "--json"]));
    let rows = doc["items"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{doc:#}");
    assert_eq!(rows[0]["about"], "plan:h4#claim-1/2@5,5");
    assert_eq!(rows[0]["body"].as_str().unwrap().trim(), "nice picture");
}
