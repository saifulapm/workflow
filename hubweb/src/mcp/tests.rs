use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::*;
use crate::item;

/// A store in memory: path to body.
#[derive(Default)]
struct Mem(BTreeMap<String, String>);

impl memview::Files for Mem {
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

fn now() -> jiff::Timestamp {
    "2026-10-16T12:00:00Z".parse().unwrap()
}

/// One message through `serve` over `mem`, its randomness all sevens.
fn call(mem: &Mem, message: Value) -> (Value, Served) {
    let mut random = || [7u8; 10];
    let served = serve(
        message.to_string().as_bytes(),
        Store {
            files: mem,
            now: now(),
            random: &mut random,
        },
    );
    let body = if served.body.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&served.body).unwrap()
    };
    (body, served)
}

#[test]
fn tools_list_is_hubs_twenty_tools_with_hubs_schemas() {
    let (body, served) = call(
        &Mem::default(),
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    );
    assert_eq!(served.status, 200);
    let tools = body["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "context",
            "search",
            "show",
            "wiki",
            "questions",
            "plan",
            "roadmap",
            "log",
            "handoff",
            "ask",
            "answer",
            "set_handoff",
            "add_log",
            "decide",
            "tick_task",
            "tick_milestone",
            "write_wiki",
            "add_evidence",
            "add_finding",
            "close_finding",
        ]
    );
    // Each tool's own properties and required keys, as hub's table has them;
    // every tool also takes `project`, required first.
    let own = [
        (
            json!({"full": {"type": "boolean", "description": "The whole digest."}}),
            json!([]),
        ),
        (
            json!({
                "query": {"type": "string", "description": "FTS5 query; title:, body: and tags: filter."},
                "limit": {"type": "integer", "description": "At most this many hits."},
            }),
            json!(["query"]),
        ),
        (
            json!({"ids": {"type": "array", "items": {"type": "string"}, "description": "Full ids or their 8-character suffixes."}}),
            json!(["ids"]),
        ),
        (
            json!({"slug": {"type": "string", "description": "A page slug, with #section for one section. Left out, the list of pages."}}),
            json!([]),
        ),
        (
            json!({"pending": {"type": "boolean", "description": "Only the unanswered ones."}}),
            json!([]),
        ),
        (
            json!({"slug": {"type": "string", "description": "A milestone's slug. Left out, the current plan."}}),
            json!([]),
        ),
        (
            json!({"status": {"type": "boolean", "description": "Print only the roadmap's status."}}),
            json!([]),
        ),
        (
            json!({"limit": {"type": "integer", "description": "At most this many entries."}}),
            json!([]),
        ),
        (json!({}), json!([])),
        (
            json!({
                "question": {"type": "string"},
                "options": {"type": "string", "description": "The choices, comma-separated."},
                "recommend": {"type": "string", "description": "The answer the asker would pick."},
                "for": {"type": "string", "enum": ["orchestrator", "human", "saiful"], "description": "Who answers. A person unless this says otherwise."},
            }),
            json!(["question"]),
        ),
        (
            json!({
                "id": {"type": "string", "description": "The question's id."},
                "text": {"type": "string"},
                "option": {"type": "string", "description": "One of the question's options."},
            }),
            json!(["id"]),
        ),
        (json!({"text": {"type": "string"}}), json!(["text"])),
        (json!({"text": {"type": "string"}}), json!(["text"])),
        (
            json!({
                "text": {"type": "string"},
                "by": {"type": "string", "enum": ["agent", "saiful"]},
            }),
            json!(["text", "by"]),
        ),
        (
            json!({"task": {"type": "string", "description": "The task's plan id, such as t3."}}),
            json!(["task"]),
        ),
        (
            json!({"milestone": {"type": "string", "description": "The milestone's slug."}}),
            json!(["milestone"]),
        ),
        (
            json!({
                "slug": {"type": "string", "description": "The page slug, with #section for one section."},
                "note": {"type": "string", "description": "What changed and why."},
                "page": {"type": "string", "description": "The page's markdown."},
            }),
            json!(["slug", "note", "page"]),
        ),
        (
            json!({
                "task": {"type": "string", "description": "The task's plan id."},
                "note": {"type": "string"},
                "content": {"type": "string", "description": "The evidence itself, as text."},
            }),
            json!(["task", "note", "content"]),
        ),
        (
            json!({
                "milestone": {"type": "string", "description": "The milestone's slug."},
                "step": {"type": "string"},
                "text": {"type": "string"},
            }),
            json!(["milestone", "step", "text"]),
        ),
        (
            json!({
                "id": {"type": "string", "description": "The finding's id."},
                "by": {"type": "string", "description": "The commit that fixed it, or the reason."},
            }),
            json!(["id", "by"]),
        ),
    ];
    assert_eq!(tools.len(), own.len());
    for (tool, (properties, required)) in tools.iter().zip(own) {
        let mut all = json!({"project": {"type": "string", "description": "The project's name, as mem knows it."}});
        all.as_object_mut()
            .unwrap()
            .extend(properties.as_object().unwrap().clone());
        let mut names = vec![json!("project")];
        names.extend(required.as_array().unwrap().iter().cloned());
        assert_eq!(
            tool["inputSchema"],
            json!({"type": "object", "properties": all, "required": names}),
            "{}",
            tool["name"]
        );
    }
    assert_eq!(
        tools[8]["description"],
        "The latest session handoff for a project: what was done and what comes next."
    );
    assert_eq!(
        tools[19]["description"],
        "Writes a finding as fixed, by a commit or a reason."
    );
}

const WALK: &str = "01K0000000000000000000WALK";
const OTHER: &str = "01K000000000000000000OTHER";
const REVIEW: &str = "01K2000000000000000000REVW";
const PLAIN: &str = "01K2000000000000000000BANK";
const ELSEWHERE: &str = "01K2000000000000000000THRE";

impl Mem {
    fn project(&mut self, id: &str, name: &str) {
        self.0.insert(
            format!("projects/{id}/project.toml"),
            format!("id = \"{id}\"\nname = \"{name}\"\ncreated = \"2026-10-01T09:00:00Z\"\n"),
        );
    }

    /// An item in (project id, name), its extra frontmatter lines given
    /// whole; its title is the body's first line.
    fn item(
        &mut self,
        project: (&str, &str),
        id: &str,
        kind: &str,
        at: &str,
        extra: &str,
        body: &str,
    ) {
        let (dir, name) = project;
        let title = body.lines().next().unwrap_or("");
        self.0.insert(
            format!("projects/{dir}/items/{id}.md"),
            format!(
                "+++\ncreated = \"{at}\"\n{extra}id = \"{id}\"\nkind = \"{kind}\"\n\
                 machine = \"laptop\"\nmodified = \"{at}\"\nproject = \"{name}\"\n\
                 title = \"{title}\"\n+++\n\n{body}\n"
            ),
        );
    }

    /// The walk project: a wiki page, two handoffs, a plan, and questions,
    /// one of them a roadmap review, beside another project's question.
    fn walk() -> Mem {
        let mut mem = Mem::default();
        mem.project(WALK, "walk");
        mem.project(OTHER, "other");
        let walk = (WALK, "walk");
        mem.0.insert(
            format!("projects/{WALK}/wiki/trail.md"),
            "# Trail\n\nThe path in.\n\n## Gate\n\nThe gate is painted zephyrine blue.\n\n## River\n\nCross at the ford.\n".into(),
        );
        mem.item(
            walk,
            "01K1000000000000000000HND1",
            "handoff",
            "2026-10-10T10:00:00Z",
            "",
            "Old handoff.",
        );
        mem.item(
            walk,
            "01K1000000000000000000HND2",
            "handoff",
            "2026-10-12T10:00:00Z",
            "",
            "Next: cross the river.\nThen rest.",
        );
        mem.item(
            walk,
            "01K1000000000000000000NTE1",
            "log",
            "2026-10-11T10:00:00Z",
            "",
            "Walked to the gate.",
        );
        mem.0.insert(
            format!("projects/{WALK}/plan.md"),
            "# plan: m1-walk\n\n- [x] t1 Reach the gate\n- [ ] t2 Cross the river\n- [ ] t20 Rest\n".into(),
        );
        mem.item(
            walk,
            REVIEW,
            "question",
            "2026-10-13T10:00:00Z",
            "options = [\"approve\", \"changes\"]\n",
            "Review the walk roadmap",
        );
        mem.item(
            walk,
            PLAIN,
            "question",
            "2026-10-13T11:00:00Z",
            "options = [\"left\", \"right\"]\n",
            "Which bank?",
        );
        mem.item(
            (OTHER, "other"),
            ELSEWHERE,
            "question",
            "2026-10-13T12:00:00Z",
            "",
            "Someone else's?",
        );
        mem
    }

    fn apply(&mut self, served: &Served) {
        for (path, body) in &served.writes {
            match body {
                Some(body) => self.0.insert(path.clone(), body.clone()),
                None => self.0.remove(path),
            };
        }
    }
}

/// One tool call: its text, whether it is an error, and what it wrote.
fn tool(mem: &Mem, name: &str, arguments: Value) -> (String, bool, Served) {
    let (body, served) = call(
        mem,
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": name, "arguments": arguments}}),
    );
    let result = &body["result"];
    let text = result["content"][0]["text"].as_str().unwrap().to_string();
    (text, result["isError"] == true, served)
}

fn questions(mem: &Mem, pending: bool) -> Vec<Value> {
    let mut argv = vec!["questions", "--project", "walk", "--json"];
    if pending {
        argv.push("--pending");
    }
    match memview::read(mem, &argv, now()) {
        memview::Reply::Json(doc) => doc["questions"].as_array().unwrap().clone(),
        other => panic!("{other:?}"),
    }
}

/// The id the connector gives its first item, with every random byte 7.
fn first_id() -> String {
    item::ulid(now().as_millisecond() as u64, [7; 10])
}

#[test]
fn search_finds_a_word_only_a_wiki_page_holds_in_its_section() {
    let (text, error, served) = tool(
        &Mem::walk(),
        "search",
        json!({"project": "walk", "query": "zephyrine"}),
    );
    assert!(!error, "{text}");
    assert_eq!(
        text,
        "wiki:trail#gate  1.00  46\n  The gate is painted zephyrine blue.\n"
    );
    assert!(served.writes.is_empty());
}

#[test]
fn a_roadmap_review_is_refused_and_nothing_is_written() {
    let (text, error, served) = tool(
        &Mem::walk(),
        "answer",
        json!({"project": "walk", "id": REVIEW, "option": "approve"}),
    );
    assert!(error);
    assert_eq!(text, "a roadmap review is Saiful's to answer, on the hub");
    assert!(served.writes.is_empty() && served.files.is_empty());
}

#[test]
fn an_answer_to_another_projects_question_is_refused() {
    let (text, error, served) = tool(
        &Mem::walk(),
        "answer",
        json!({"project": "walk", "id": ELSEWHERE, "text": "yes"}),
    );
    assert!(error);
    assert_eq!(text, format!("{ELSEWHERE} is not a question of walk"));
    assert!(served.writes.is_empty());
}

#[test]
fn an_answer_is_signed_connector_and_answers_the_question() {
    let mut mem = Mem::walk();
    // By its short id, the full id's last 8 characters.
    let (text, error, served) = tool(
        &mem,
        "answer",
        json!({"project": "walk", "id": "0000BANK", "option": "left"}),
    );
    assert!(!error, "{text}");
    let id = first_id();
    assert_eq!(text, format!("#{}  answer\n", &id[18..]));
    assert_eq!(served.writes.len(), 1);
    let (path, body) = &served.writes[0];
    assert_eq!(path, &format!("projects/{WALK}/items/{id}.md"));
    let body = body.as_deref().unwrap();
    assert!(body.contains(&format!("answers = \"{PLAIN}\"\n")), "{body}");
    assert!(body.contains("by = \"connector\"\n"), "{body}");
    mem.apply(&served);
    let plain = questions(&mem, false)
        .into_iter()
        .find(|q| q["id"] == PLAIN)
        .unwrap();
    assert_eq!(plain["answered"], true);
    assert_eq!(plain["answer"], "left");
    assert_eq!(plain["answer_id"], json!(id));
    assert_eq!(plain["answer_by"], "connector");
}

#[test]
fn handoff_is_the_latest_handoffs_text() {
    let (text, error, _) = tool(&Mem::walk(), "handoff", json!({"project": "walk"}));
    assert!(!error);
    assert_eq!(text, "\nNext: cross the river.\nThen rest.\n");
    let (text, error, _) = tool(&Mem::walk(), "handoff", json!({"project": "nowhere"}));
    assert!(error);
    assert_eq!(
        text,
        "mem: no project named 'nowhere' \u{2014} `mem projects` lists them"
    );
}

#[test]
fn tick_task_writes_the_plan_with_that_task_ticked() {
    let (text, error, served) = tool(
        &Mem::walk(),
        "tick_task",
        json!({"project": "walk", "task": "t2"}),
    );
    assert!(!error, "{text}");
    assert_eq!(text, "- [x] t2\n");
    assert_eq!(
        served.writes,
        [(
            format!("projects/{WALK}/plan.md"),
            Some(
                "# plan: m1-walk\n\n- [x] t1 Reach the gate\n- [x] t2 Cross the river\n- [ ] t20 Rest\n"
                    .to_string()
            )
        )]
    );
}

#[test]
fn ask_writes_a_pending_question_with_its_options() {
    let mut mem = Mem::walk();
    let (text, error, served) = tool(
        &mem,
        "ask",
        json!({"project": "walk", "question": "Ford or bridge?", "options": "ford,bridge", "recommend": "ford"}),
    );
    assert!(!error, "{text}");
    let id = first_id();
    assert_eq!(text, format!("#{}\n", &id[18..]));
    mem.apply(&served);
    let asked = questions(&mem, true)
        .into_iter()
        .find(|q| q["id"] == json!(id))
        .unwrap();
    assert_eq!(asked["body"], "Ford or bridge?");
    assert_eq!(asked["options"], json!(["ford", "bridge"]));
    assert_eq!(asked["recommend"], "ford");
    assert_eq!(asked["machine"], "connector");
    assert_eq!(asked["project"], "walk");
}

#[test]
fn ask_refuses_a_recommendation_that_is_none_of_the_options() {
    let mem = Mem::walk();
    let (text, error, served) = tool(
        &mem,
        "ask",
        json!({"project": "walk", "question": "Ford or bridge?", "options": "ford,bridge", "recommend": "ford, it is shorter"}),
    );
    assert!(error, "{text}");
    assert!(text.contains("ford · bridge"), "{text}");
    assert!(served.files.is_empty(), "nothing is written");
}

#[test]
fn initialize_answers_the_version_asked_for_or_the_newest() {
    let mem = Mem::default();
    let (body, served) = call(
        &mem,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}),
    );
    assert_eq!(served.status, 200);
    assert_eq!(body["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(body["result"]["serverInfo"]["name"], "mem");
    let (body, _) = call(
        &mem,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}),
    );
    assert_eq!(body["result"]["protocolVersion"], "2025-11-25");
}

#[test]
fn a_notification_gets_202_and_unreadable_messages_get_400() {
    let mem = Mem::default();
    let (body, served) = call(
        &mem,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );
    assert_eq!((served.status, body), (202, Value::Null));
    let mut random = || [0u8; 10];
    let store = Store {
        files: &mem,
        now: now(),
        random: &mut random,
    };
    assert_eq!(serve(b"{", store).status, 400);
    let (body, served) = call(&mem, json!({"id": 1}));
    assert_eq!(
        (served.status, &body["error"]["code"]),
        (400, &json!(-32600))
    );
}
