//! `GET /p/<project>/questions`: the pending questions with a button per
//! option and a text box, then the answered ones with their answers; and
//! `POST /answer` landing back on that page when the form names its project.

mod common;

use std::path::PathBuf;

use common::{
    Hub, TempDir, body_of, fixture_mem, header_of, mem_in, real_mem, recording_mem, seed_project,
    status_of,
};

const PICK: &str = "01M4546Y7FWT8ZGJKR0C90PWRA";
const OPEN: &str = "01M4546Y7M7DG6D48QC797E1MD";
const DONE: &str = "01M4546Y6ZZZZZZZZZZNTEPJ9K";

/// A fake `mem` that knows one project, `gamma`, with three questions: one
/// pending with three options and a pick, one pending with none, and one
/// answered. The doorbell's read of the pending queue fails, so its round
/// stops there and spawns nothing else. Every call is appended to `calls`.
struct World {
    _dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    calls: PathBuf,
}

impl World {
    fn new(tag: &str) -> World {
        let dir = TempDir::new(tag);
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let bin = dir.join("bin");
        let calls = dir.join("calls");
        let questions = serde_json::json!({"questions": [
            {"id": DONE, "short_id": "NTEPJ9K", "title": "Ship on Friday?",
             "body": "Ship on Friday?", "options": [], "recommend": null,
             "answered": true, "answer": "yes, after lunch"},
            {"id": PICK, "short_id": "0C90PWRA", "title": "Which hour does the nag go out?",
             "body": "Which hour does the nag go out?", "options": ["nine", "eight", "ten"],
             "recommend": "nine", "answered": false, "answer": null},
            {"id": OPEN, "short_id": "C797E1MD", "title": "What should the nag say?",
             "body": "What should the nag say?", "options": [], "recommend": null,
             "answered": false, "answer": null},
        ]});
        fixture_mem(
            &bin,
            &format!(
                "echo \"$*\" >>'{calls}'\n\
                 case \"$1\" in\n\
                 projects) echo '{{\"projects\":[{{\"name\":\"gamma\"}}]}}' ;;\n\
                 questions) [ \"$2\" = --project=gamma ] || exit 1; printf '%s\\n' '{questions}' ;;\n\
                 *) exit 1 ;;\n\
                 esac",
                calls = calls.display(),
            ),
        );
        World {
            _dir: dir,
            home,
            bin,
            calls,
        }
    }

    fn hub(&self) -> Hub {
        Hub::spawn(&self.home, &[&self.bin], &["--port", "0"])
    }

    /// The page's own reads: the doorbell's poll of the pending queue runs
    /// on its own clock and is left out.
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.calls)
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.starts_with("questions --pending"))
            .map(str::to_string)
            .collect()
    }
}

/// The page's body, after checking it ran mem twice on a cold cache.
fn questions_page(world: &World) -> String {
    let hub = world.hub();
    let before = world.calls().len();
    let response = hub.get("/p/gamma/questions");
    assert_eq!(status_of(&response), 200, "{response}");
    let calls = world.calls();
    assert_eq!(calls.len() - before, 2, "{:?}", &calls[before..]);
    body_of(&response).to_string()
}

/// The `<article>` holding `text`.
fn article<'a>(body: &'a str, text: &str) -> &'a str {
    let at = body
        .find(text)
        .unwrap_or_else(|| panic!("{text:?} not in {body}"));
    let start = body[..at].rfind("<article>").expect("an article");
    let end = at + body[at..].find("</article>").expect("its end");
    &body[start..end]
}

fn option_form(id: &str, option: &str, label: &str) -> String {
    format!(
        "<form method=\"post\" action=\"/answer\">\n\
         <input type=\"hidden\" name=\"id\" value=\"{id}\">\n\
         <input type=\"hidden\" name=\"project\" value=\"gamma\">\n\
         {label_line}\
         </form>\n",
        label_line = match label {
            "" => format!(
                "<button type=\"submit\" name=\"text\" value=\"{option}\">{option}</button>\n"
            ),
            _ => format!(
                "<button type=\"submit\" name=\"text\" value=\"{option}\" class=\"recommended\">{label}</button>\n"
            ),
        },
    )
}

fn text_form(id: &str) -> String {
    format!(
        "<form method=\"post\" action=\"/answer\">\n\
         <input type=\"hidden\" name=\"id\" value=\"{id}\">\n\
         <input type=\"hidden\" name=\"project\" value=\"gamma\">\n\
         <textarea name=\"text\" rows=\"3\" placeholder=\"answer\" aria-label=\"answer\"></textarea>\n\
         <button type=\"submit\">Answer</button>\n\
         </form>\n"
    )
}

#[test]
fn a_question_with_options_has_a_button_each_and_marks_the_pick() {
    let world = World::new("questions-options");
    let body = questions_page(&world);
    let pick = article(&body, "Which hour does the nag go out?");

    assert!(
        pick.contains(&option_form(PICK, "nine", "nine · recommended")),
        "{pick}"
    );
    assert!(pick.contains(&option_form(PICK, "eight", "")), "{pick}");
    assert!(pick.contains(&option_form(PICK, "ten", "")), "{pick}");
    assert_eq!(pick.matches("recommended").count(), 2, "{pick}");
    assert!(pick.contains(&text_form(PICK)), "{pick}");
}

#[test]
fn a_question_with_no_options_has_only_the_text_box() {
    let world = World::new("questions-open");
    let body = questions_page(&world);
    let open = article(&body, "What should the nag say?");

    assert_eq!(open.matches("<form").count(), 1, "{open}");
    assert!(open.contains(&text_form(OPEN)), "{open}");
    assert!(!open.contains("recommended"), "{open}");
}

#[test]
fn an_answered_question_comes_after_the_pending_with_its_answer() {
    let world = World::new("questions-answered");
    let body = questions_page(&world);
    let done = article(&body, "Ship on Friday?");

    assert!(done.contains("yes, after lunch"), "{done}");
    assert!(!done.contains("<form"), "{done}");
    let answered = body.find("<h2>Answered</h2>").expect("an answered heading");
    assert!(body.find("Which hour").unwrap() < answered, "{body}");
    assert!(
        body.find("What should the nag say?").unwrap() < answered,
        "{body}"
    );
    assert!(body.find("Ship on Friday?").unwrap() > answered, "{body}");
}

#[test]
fn an_option_posted_from_the_page_lands_back_on_it_answered() {
    let dir = TempDir::new("questions-answer");
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
            "nine,eight,ten",
            "--recommend",
            "nine",
            "Which hour does the nag go out?",
        ],
    );
    assert!(out.status.success(), "{out:?}");
    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);

    let body = body_of(&hub.get("/p/gamma/questions")).to_string();
    let pick = article(&body, "Which hour does the nag go out?");
    let id_at = pick.find("name=\"id\" value=\"").unwrap() + "name=\"id\" value=\"".len();
    let id = &pick[id_at..id_at + pick[id_at..].find('"').unwrap()];
    assert!(
        pick.contains("name=\"text\" value=\"eight\""),
        "no button for eight: {pick}"
    );

    let response = hub.post_form("/answer", &format!("id={id}&text=eight&project=gamma"));
    assert_eq!(status_of(&response), 303, "{response}");
    let location = header_of(&response, "Location").unwrap().to_string();
    let short = &id[id.len() - 8..];
    assert_eq!(location, format!("/p/gamma/questions?answered={short}"));

    let body = body_of(&hub.get(&location)).to_string();
    assert!(body.contains(&format!("Answered #{short}.")), "{body}");
    let answered = body.find("<h2>Answered</h2>").expect("an answered heading");
    let done = article(&body, "Which hour does the nag go out?");
    assert!(body.find(done).unwrap() > answered, "{body}");
    assert!(done.contains("eight"), "{done}");
    assert!(!done.contains("<form"), "{done}");
}
