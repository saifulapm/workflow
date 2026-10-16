//! Home, drawn from a `Source` that is no mem binary: canned documents keyed
//! by the argv the page reads, as a Worker's store answers them.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use hubpages::form::Form;
use hubpages::host::{Command, Held, Kind, NoMachine, Polled, Saved};
use hubpages::http::Request;
use hubpages::live::Runner;
use hubpages::pages::{Site, route_page};
use hubpages::{Fleet, Host, Outcome, QUESTIONS_ARGV, Source, Written};
use serde_json::{Value, json};

/// mem as a table: argv, joined by spaces, to the document it prints.
struct Canned(HashMap<String, Value>);

impl Source for Canned {
    fn read(&self, argv: &[&str]) -> Arc<Outcome> {
        Arc::new(match self.0.get(&argv.join(" ")) {
            Some(doc) => Outcome::Json(doc.clone()),
            None => Outcome::Absent,
        })
    }

    fn write(&self, argv: &[&str], _input: Option<&[u8]>) -> Written {
        panic!("home wrote mem: {argv:?}")
    }
}

/// A host with no agents, the route at Phone and nothing from Dulali. Home
/// reads it and acts on nothing.
struct Quiet;

impl Fleet for Quiet {
    fn online(&self, _machine: &str) -> bool {
        false
    }
    fn report(&self, _machine: &str) -> Option<Polled> {
        None
    }
    fn machines(&self) -> Vec<String> {
        Vec::new()
    }
    fn pending(&self, _machine: &str) -> Vec<Command> {
        Vec::new()
    }
}

impl Host for Quiet {
    fn fleet(&self) -> &dyn Fleet {
        self
    }
    fn queue(&self, _machine: &str, command: Command) {
        panic!("home queued {command:?}")
    }
    fn wrote(&self) {
        panic!("home wrote")
    }
    fn lock(&self, _key: &str) -> Arc<Mutex<()>> {
        Arc::default()
    }
    fn forget_locks(&self) {}
    fn route(&self) -> Saved {
        Saved::default()
    }
    fn set_route(&self, _kind: Kind) -> Result<(), NoMachine> {
        panic!("home set the route")
    }
    fn replaceable(&self, _now_ms: i64) -> Vec<(String, Held)> {
        Vec::new()
    }
    fn held_written(&self, _id: &str) -> Option<(String, Held)> {
        None
    }
    fn superseded(&self, _id: &str, _held: &Held, _text: &str) {
        panic!("home superseded an answer")
    }
    fn answered(&self, _row: &Value) {
        panic!("home answered")
    }
    fn after_answer(&self, _project: &str, _session: Option<&str>) {
        panic!("home resumed a session")
    }
    fn replied_in_app(&self, _project: &str, _runner: &Runner) {
        panic!("home replied in the app")
    }
    fn pause(&self, verb: &str, _project: &str) -> Result<(), String> {
        panic!("home ran workflow {verb}")
    }
    fn stash_photo(&self, _kind: &str, _bytes: &[u8]) -> Result<String, String> {
        panic!("home kept a photo")
    }
    fn drop_photo(&self, _path: &str) {
        panic!("home dropped a photo")
    }
}

fn get(path: &str) -> Request {
    Request {
        method: "GET".to_string(),
        path: path.to_string(),
        query: Form::parse(""),
        headers: Vec::new(),
        body: Vec::new(),
        peer: "test".to_string(),
    }
}

#[test]
fn home_draws_a_project_card_and_a_waiting_question_from_any_source() {
    let mem = Canned(HashMap::from([
        (
            "projects --json".to_string(),
            json!({"projects": [{
                "name": "alpha", "checkouts": [],
                "roadmap_status": "approved", "milestone": "auth",
                "milestones_done": 0, "milestones_total": 2,
                "plan_slug": null, "plan_ticked": 0, "plan_total": 0,
                "last_activity": "2026-10-05T10:00:00Z",
            }]}),
        ),
        (
            QUESTIONS_ARGV.join(" "),
            json!({"questions": [{
                "id": "01K6Z9QH4V7M3ZJ8D2C5XWRT9P", "short_id": "C5XWRT9P",
                "project": "alpha", "title": "Which store for sessions?",
                "body": "Which store for sessions?",
                "options": ["Redis", "Postgres"], "recommend": "Postgres",
            }]}),
        ),
    ]));
    let site = Site {
        mem: &mem,
        host: &Quiet,
        machine: "worker",
        siblings: &[],
        by: "hub-web",
        paired: true,
        pair_how: "pair it",
        may_write: true,
        now: "2026-10-05T12:00:00Z".parse().unwrap(),
        tz: jiff::tz::TimeZone::UTC,
        routes: &[Kind::Phone, Kind::Device],
        poll: false,
    };

    let response = route_page(&site, &get("/")).expect("home is a page");
    assert_eq!(response.status, 200);
    let page = String::from_utf8(response.body).unwrap();

    assert!(
        page.contains("<h3><a href=\"/p/alpha\">alpha</a></h3>"),
        "a card that opens alpha: {page}"
    );
    assert!(
        page.contains("<div class=\"meta\">auth · "),
        "the card names its open milestone: {page}"
    );
    assert!(page.contains("Which store for sessions?"), "{page}");
    for (option, label) in [("Redis", "Redis"), ("Postgres", "Postgres · recommended")] {
        assert!(
            page.contains(&format!("name=\"text\" value=\"{option}\"")) && page.contains(label),
            "a button answering {option}: {page}"
        );
    }
    assert!(
        page.contains("<input type=\"hidden\" name=\"id\" value=\"01K6Z9QH4V7M3ZJ8D2C5XWRT9P\">"),
        "the buttons answer that question: {page}"
    );
}

/// The hub web offers Phone and Device only, and hears of changes on its page
/// socket, so its home names no Dulali and polls nothing.
#[test]
fn a_home_without_dulali_or_polling_shows_neither() {
    let mem = Canned(HashMap::from([
        ("projects --json".to_string(), json!({"projects": []})),
        (QUESTIONS_ARGV.join(" "), json!({"questions": []})),
    ]));
    let site = Site {
        mem: &mem,
        host: &Quiet,
        machine: "worker",
        siblings: &[],
        by: "hub-web",
        paired: true,
        pair_how: "pair it",
        may_write: true,
        now: "2026-10-05T12:00:00Z".parse().unwrap(),
        tz: jiff::tz::TimeZone::UTC,
        routes: &[Kind::Phone, Kind::Device],
        poll: false,
    };
    let page = String::from_utf8(route_page(&site, &get("/")).unwrap().body).unwrap();
    assert!(!page.to_lowercase().contains("dulali"), "{page}");
    assert!(!page.contains("/api/stamp"), "{page}");
    assert!(
        page.contains("value=\"phone\"") && page.contains("value=\"device\""),
        "{page}"
    );
}
