//! The front page shows each waiting question with the same option buttons
//! and text box as the project's questions page, and a tap there answers it
//! and lands back on `/`.

mod common;

use common::{
    Hub, TempDir, body_of, header_of, mem_in, real_mem, recording_mem, seed_project, status_of,
};

/// The `<article>` holding `text`.
fn article<'a>(body: &'a str, text: &str) -> &'a str {
    let at = body
        .find(text)
        .unwrap_or_else(|| panic!("{text:?} not in {body}"));
    let start = body[..at].rfind("<article>").expect("an article");
    let end = at + body[at..].find("</article>").expect("its end");
    &body[start..end]
}

/// The `<form>` holding `needle`, whole.
fn form_with<'a>(html: &'a str, needle: &str) -> &'a str {
    let at = html
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {html}"));
    let start = html[..at].rfind("<form").expect("a form");
    let end = at + html[at..].find("</form>").expect("its end") + "</form>".len();
    &html[start..end]
}

/// Every `name="…" value="…"` pair in a form, in order, the way a browser
/// submits the hidden fields and the tapped button.
fn fields(form: &str) -> Vec<(String, String)> {
    form.match_indices("name=\"")
        .map(|(at, _)| {
            let rest = &form[at + "name=\"".len()..];
            let name = &rest[..rest.find('"').unwrap()];
            let value_at = rest.find("value=\"").unwrap() + "value=\"".len();
            let value = &rest[value_at..value_at + rest[value_at..].find('"').unwrap()];
            (name.to_string(), value.to_string())
        })
        .collect()
}

#[test]
fn an_option_tapped_on_home_answers_the_question_and_lands_on_home() {
    let dir = TempDir::new("home-option");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let (bin, _log) = recording_mem(dir.path(), &home);
    let mem = real_mem().unwrap();
    seed_project(&mem, &home, "gamma", "gamma did a thing");
    let out = mem_in(
        &mem,
        &home,
        &home.join("gamma"),
        &[
            "ask",
            "--for",
            "human",
            "--options",
            "seven,eight,nine",
            "Which hour does the nag go out?",
        ],
    );
    assert!(out.status.success(), "{out:?}");
    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);

    let body = body_of(&hub.get("/")).to_string();
    let block = article(&body, "Which hour does the nag go out?");
    let button =
        "<button type=\"submit\" name=\"text\" value=\"eight\" class=\"alt\">eight</button>";
    let form = form_with(block, button);
    assert!(
        form.starts_with("<form class=\"opt\" method=\"post\" action=\"/answer\">"),
        "{form}"
    );
    // The questions page renders the same button from the same function.
    let page = body_of(&hub.get("/p/gamma/questions")).to_string();
    assert!(
        article(&page, "Which hour does the nag go out?").contains(button),
        "{page}"
    );

    let fields = fields(form);
    let id = fields
        .iter()
        .find(|(name, _)| name == "id")
        .map(|(_, value)| value.clone())
        .expect("an id field");
    let posted: Vec<String> = fields.iter().map(|(n, v)| format!("{n}={v}")).collect();
    let response = hub.post_form("/answer", &posted.join("&"));
    assert_eq!(status_of(&response), 303, "{response}");
    let short = &id[id.len() - 8..];
    assert_eq!(
        header_of(&response, "Location"),
        Some(format!("/?answered={short}").as_str()),
        "{response}"
    );

    let out = mem_in(
        &mem,
        &home,
        &home.join("gamma"),
        &["questions", "--project=gamma", "--json"],
    );
    assert!(out.status.success(), "{out:?}");
    let listed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let row = listed["questions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["id"].as_str() == Some(id.as_str()))
        .unwrap_or_else(|| panic!("{id} not in {listed}"));
    assert_eq!(row["answered"], serde_json::json!(true), "{row}");
    assert_eq!(row["answer"], serde_json::json!("eight"), "{row}");
}
