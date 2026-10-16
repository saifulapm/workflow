//! A parked cloud run takes its answers over Tailscale, so the hub queues
//! nothing for the mini and the project page offers no way to reply in the
//! Claude app. Over a fake `mem` that prints whatever JSON the test last put
//! in `data/`.

mod common;

use std::path::PathBuf;

use common::{Hub, TempDir, body_of, fixture_bin, status_of, ulid_at};
use serde_json::{Value, json};

const SESSION: &str = "session_01CloudRun";

struct World {
    dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    data: PathBuf,
}

impl World {
    /// gamma, run in the cloud on m2 since an hour ago, parked half an hour
    /// ago.
    fn new(tag: &str) -> World {
        let dir = TempDir::new(tag);
        let home = dir.join("home");
        let bin = dir.join("bin");
        let data = dir.join("data");
        std::fs::create_dir_all(&data).unwrap();
        fixture_bin(
            &bin,
            "mem",
            &format!(
                "case \"$*\" in\n\
                 projects*) cat '{data}/projects.json' ;;\n\
                 questions\\ --pending*) cat '{data}/pending.json' ;;\n\
                 questions*) cat '{data}/all.json' ;;\n\
                 answer*) exit 0 ;;\n\
                 roadmap*) echo '{{\"status\":\"approved\",\"text\":\"- [x] m1 One\\n- [ ] m2 Two\\n\"}}' ;;\n\
                 plan\\ --list*) echo '{{\"plans\":[]}}' ;;\n\
                 handoff*) echo '{{\"body\":\"parked: t2 is local-only\"}}' ;;\n\
                 *) echo '{{\"items\":[]}}' ;;\n\
                 esac",
                data = data.display()
            ),
        );
        let world = World {
            dir,
            home,
            bin,
            data,
        };
        world.put(
            "projects.json",
            json!({"projects": [{"name": "gamma", "roadmap_status": "approved",
                "checkouts": [], "milestone": "m2", "milestones_done": 1,
                "milestones_total": 3, "handoff_at": ago(30),
                "runner": {"machine": "cloud", "id": SESSION, "since": ago(60),
                           "milestone": "m2"}}]}),
        );
        world.questions(&[], &[]);
        world
    }

    fn put(&self, name: &str, value: Value) {
        let staged = self.data.join(format!(".{name}"));
        std::fs::write(&staged, value.to_string()).unwrap();
        std::fs::rename(&staged, self.data.join(name)).unwrap();
    }

    fn questions(&self, pending: &[Value], all: &[Value]) {
        self.put("pending.json", json!({ "questions": pending }));
        self.put("all.json", json!({ "questions": all }));
    }

    fn hub(&self) -> Hub {
        let config = self.dir.join("config.toml");
        std::fs::write(
            &config,
            "topic = \"workflow-TESTTESTTESTTESTTESTTESTTE\"\n\
             ntfy_base = \"http://127.0.0.1:9\"\n",
        )
        .unwrap();
        Hub::spawn(
            &self.home,
            &[&self.bin],
            &["--port", "0", "--config", config.to_str().unwrap()],
        )
    }
}

fn ago(minutes: i64) -> String {
    (jiff::Timestamp::now() - jiff::SignedDuration::from_mins(minutes)).to_string()
}

/// A question id made `minutes` ago.
fn id_ago(minutes: i64, tail: &str) -> String {
    let at = jiff::Timestamp::now() - jiff::SignedDuration::from_mins(minutes);
    ulid_at(at.as_millisecond(), tail)
}

/// One of gamma's questions with no session, as the cloud asks it.
fn question(id: &str, body: &str, answer: Option<&str>) -> Value {
    json!({"id": id, "short_id": &id[18..], "project": "gamma", "body": body,
        "session": null, "answered": answer.is_some(), "answer": answer})
}

/// The cloud commands of the mini's poll.
fn cloud_polled(hub: &Hub) -> Vec<Value> {
    let report = json!({"machine": "mini", "agents": [], "checkouts": []});
    let polled: Value = serde_json::from_str(&hub.poll_agent(&report.to_string())).unwrap();
    polled["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "cloud")
        .cloned()
        .collect()
}

fn answer(hub: &Hub, id: &str, text: &str) {
    let response = hub.post_form("/answer", &format!("id={id}&text={text}"));
    assert_eq!(status_of(&response), 303, "{response}");
}

fn page(hub: &Hub) -> String {
    let response = hub.get("/p/gamma");
    assert_eq!(status_of(&response), 200, "{response}");
    body_of(&response).to_string()
}

#[test]
fn a_parked_run_with_the_mini_off_offers_no_reply_in_the_claude_app() {
    let world = World::new("cloud-answer-no-card");
    let hub = world.hub();
    let body = page(&hub);
    assert!(body.contains("cloud · m2 · parked"), "{body}");
    assert!(!body.contains("reply in the Claude app"), "{body}");
    assert!(!body.contains("Reply in the Claude app"), "{body}");
    assert!(!body.contains("reply-in-app"), "{body}");
}

#[test]
fn reply_in_app_is_an_unknown_control() {
    let world = World::new("cloud-answer-no-control");
    let hub = world.hub();
    let response = hub.post_form("/p/gamma/control", "do=reply-in-app");
    assert_eq!(status_of(&response), 400, "{response}");
    assert!(body_of(&response).contains("unknown control"), "{response}");
}

#[test]
fn answering_every_question_of_a_parked_run_queues_nothing_for_the_mini() {
    let world = World::new("cloud-answer-queues-nothing");
    let hub = world.hub();
    cloud_polled(&hub);
    let q1 = id_ago(20, "Q1AAAAAAAAAAAAAA");
    let q = |a| question(&q1, "Which port?", a);
    world.questions(&[q(None)], &[q(Some("8787"))]);
    answer(&hub, &q1, "8787");
    assert_eq!(cloud_polled(&hub), Vec::<Value>::new());
}
