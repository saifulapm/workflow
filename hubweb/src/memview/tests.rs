use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::*;

/// A store in memory: path to body.
#[derive(Default)]
struct Store(BTreeMap<String, String>);

impl Files for Store {
    fn read(&self, path: &str) -> Option<String> {
        self.0.get(path).cloned()
    }

    fn list(&self, prefix: &str) -> Vec<(String, String)> {
        self.0
            .range(prefix.to_string()..)
            .take_while(|(path, _)| path.starts_with(prefix))
            .map(|(path, body)| (path.clone(), body.clone()))
            .collect()
    }

    fn projects(&self) -> Vec<String> {
        self.0
            .keys()
            .filter_map(|path| {
                path.strip_prefix("projects/")?
                    .strip_suffix("/project.toml")
            })
            .map(str::to_string)
            .collect()
    }
}

const ALPHA: &str = "01K0000000000000000000ALPH";
const BETA: &str = "01K0000000000000000000BETA";

impl Store {
    fn project(&mut self, id: &str, name: &str) {
        self.0.insert(
            format!("projects/{id}/project.toml"),
            format!("id = \"{id}\"\nname = \"{name}\"\ncreated = \"2026-10-01T09:00:00Z\"\n"),
        );
    }

    /// An item in `project`, its frontmatter lines given whole.
    fn item(&mut self, project: &str, id: &str, name: &str, kind: &str, title: &str, extra: &str) {
        self.0.insert(
            format!("projects/{project}/items/{id}.md"),
            format!(
                "+++\ncreated = \"2026-10-02T10:00:00Z\"\n{extra}id = \"{id}\"\nkind = \"{kind}\"\n\
                 machine = \"laptop\"\nmodified = \"2026-10-02T10:00:00Z\"\nproject = \"{name}\"\n\
                 title = \"{title}\"\n+++\n\n{title}\n"
            ),
        );
    }
}

fn now() -> jiff::Timestamp {
    "2026-10-03T00:00:00Z".parse().unwrap()
}

fn json(reply: Reply) -> Value {
    match reply {
        Reply::Json(doc) => doc,
        other => panic!("wanted a document, got {other:?}"),
    }
}

#[test]
fn the_pending_listing_holds_only_a_persons_unanswered_questions() {
    let mut store = Store::default();
    store.project(ALPHA, "alpha");
    store.project(BETA, "beta");
    store.item(
        ALPHA,
        "01K2000000000000000000PEND",
        "alpha",
        "question",
        "Ship it today?",
        "options = [\"yes\", \"no\"]\n",
    );
    store.item(
        ALPHA,
        "01K2000000000000000000QRCH",
        "alpha",
        "question",
        "May I widen the files line?",
        "audience = \"orchestrator\"\n",
    );
    store.item(
        BETA,
        "01K2000000000000000000ASKD",
        "beta",
        "question",
        "Which colour?",
        "",
    );
    store.item(
        BETA,
        "01K2000000000000000000ANSW",
        "beta",
        "answer",
        "Blue",
        "answers = \"01K2000000000000000000ASKD\"\n",
    );

    let argv = [
        "questions",
        "--pending",
        "--all-projects",
        "--for",
        "human",
        "--json",
    ];
    let doc = json(read(&store, &argv, now()));

    let questions = doc["questions"].as_array().unwrap();
    let ids: Vec<&str> = questions
        .iter()
        .map(|q| q["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["01K2000000000000000000PEND"]);
    assert_eq!(questions[0]["options"], json!(["yes", "no"]));
    assert_eq!(questions[0]["project"], "alpha");
    assert_eq!(questions[0]["body"], "Ship it today?");
    assert_eq!(questions[0]["answered"], false);
}

#[test]
fn a_replaced_or_archived_item_drops_out_as_it_does_from_mem() {
    let mut store = Store::default();
    store.project(ALPHA, "alpha");
    store.item(
        ALPHA,
        "01K2000000000000000000QSTN",
        "alpha",
        "question",
        "Which colour?",
        "",
    );
    let answers = "answers = \"01K2000000000000000000QSTN\"\n";
    store.item(
        ALPHA,
        "01K2000000000000000000ANS1",
        "alpha",
        "answer",
        "Red",
        answers,
    );
    let replaced = format!("{answers}supersedes = \"01K2000000000000000000ANS1\"\n");
    store.item(
        ALPHA,
        "01K2000000000000000000ANS2",
        "alpha",
        "answer",
        "Blue",
        &replaced,
    );
    store.item(
        ALPHA,
        "01K2000000000000000000RAA1",
        "alpha",
        "ruling",
        "Kept",
        "",
    );
    store.item(
        ALPHA,
        "01K2000000000000000000RAA2",
        "alpha",
        "ruling",
        "Shelved",
        "archived = true\n",
    );

    let doc = json(read(
        &store,
        &["questions", "--project=alpha", "--json"],
        now(),
    ));
    assert_eq!(doc["questions"][0]["answer"], "Blue");
    assert_eq!(
        doc["questions"][0]["answer_id"],
        "01K2000000000000000000ANS2"
    );

    let argv = [
        "log",
        "--kind=ruling",
        "--limit",
        "5",
        "--project=alpha",
        "--json",
    ];
    let doc = json(read(&store, &argv, now()));
    let titles: Vec<&str> = doc["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Kept"]);
}
