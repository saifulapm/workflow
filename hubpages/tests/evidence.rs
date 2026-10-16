//! The evidence page, drawn from a canned `Source` as a Worker's store
//! answers it.

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
        panic!("the page wrote mem: {argv:?}")
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
        panic!("the page queued {command:?}")
    }
    fn wrote(&self) {
        panic!("the page wrote")
    }
    fn lock(&self, _key: &str) -> Arc<Mutex<()>> {
        Arc::default()
    }
    fn forget_locks(&self) {}
    fn route(&self) -> Saved {
        Saved::default()
    }
    fn set_route(&self, _kind: Kind) -> Result<(), NoMachine> {
        panic!("the page set the route")
    }
    fn replaceable(&self, _now_ms: i64) -> Vec<(String, Held)> {
        Vec::new()
    }
    fn held_written(&self, _id: &str) -> Option<(String, Held)> {
        None
    }
    fn superseded(&self, _id: &str, _held: &Held, _text: &str) {
        panic!("the page superseded an answer")
    }
    fn answered(&self, _row: &Value) {
        panic!("the page answered")
    }
    fn after_answer(&self, _project: &str, _session: Option<&str>) {
        panic!("the page resumed a session")
    }
    fn replied_in_app(&self, _project: &str, _runner: &Runner) {
        panic!("the page replied in the app")
    }
    fn pause(&self, verb: &str, _project: &str) -> Result<(), String> {
        panic!("the page ran workflow {verb}")
    }
    fn stash_photo(&self, _kind: &str, _bytes: &[u8]) -> Result<String, String> {
        panic!("the page kept a photo")
    }
    fn drop_photo(&self, _path: &str) {
        panic!("the page dropped a photo")
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
fn evidence_older_than_90_days_shows_archived_in_place_of_its_image() {
    let item = |id: &str, created: &str, file: &str| json!({"id": id, "task": "t1", "file": file, "note": "n", "created": created});
    let mem = Canned(HashMap::from([
        (
            "projects --json".to_string(),
            json!({"projects": [{"name": "alpha"}]}),
        ),
        (
            "evidence list --project=alpha --json".to_string(),
            json!({"items": [
                item("01K6Z9QH4V7M3ZJ8D2C5XWRT9P", "2026-10-04", "evidence/t1/new.png"),
                item("01K6Z9QH4V7M3ZJ8D2C5XWRT9Q", "2026-07-06", "evidence/t1/old.png"),
            ]}),
        ),
        (
            "finding list --project=alpha --json".to_string(),
            json!({"items": []}),
        ),
    ]));
    let site = Site {
        mem: &mem,
        host: &Quiet,
        machine: "worker",
        siblings: &[],
        by: "hub-web",
        paired: true,
        may_write: true,
        now: "2026-10-05T12:00:00Z".parse().unwrap(),
        tz: jiff::tz::TimeZone::UTC,
        routes: &[Kind::Phone, Kind::Device],
        poll: false,
    };
    let response = route_page(&site, &get("/p/alpha/evidence")).expect("a page");
    assert_eq!(response.status, 200);
    let page = String::from_utf8(response.body).unwrap();
    assert!(
        page.contains("file/01K6Z9QH4V7M3ZJ8D2C5XWRT9P\""),
        "yesterday's evidence keeps its image: {page}"
    );
    assert!(
        !page.contains("file/01K6Z9QH4V7M3ZJ8D2C5XWRT9Q"),
        "91 days old, no image: {page}"
    );
    assert!(page.contains("archived"), "{page}");
}
