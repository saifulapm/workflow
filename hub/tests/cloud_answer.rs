//! Answers to a parked cloud run reach it through the mini's agent, as one
//! `cloud` command, or, while the mini is off, through the Claude app. A
//! cloud run's questions carry no session, so they are the project's
//! person questions asked since the run began. Over a fake `mem` that prints
//! whatever JSON the test last put in `data/`.

mod common;

use std::path::PathBuf;

use common::{Hub, TempDir, fixture_bin, status_of, ulid_at};
use serde_json::{Value, json};

const SESSION: &str = "session_01CloudRun";

const HEAD: &str = "Saiful answered your questions on the hub. Each answer is below; \
                    `mem questions` shows them once this machine's sync round lands.\n\n";

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

fn cloud(message: &str) -> Vec<Value> {
    vec![json!({"kind": "cloud", "project": "gamma", "id": SESSION, "message": message})]
}

#[test]
fn answering_the_parked_runs_question_queues_one_cloud_command_for_the_mini() {
    let world = World::new("cloud-answer-online");
    let hub = world.hub();
    assert_eq!(cloud_polled(&hub), Vec::<Value>::new());

    let q1 = id_ago(20, "Q1AAAAAAAAAAAAAA");
    // Asked before the run began, and one a device session asked: neither
    // is the cloud run's.
    let older = question(&id_ago(90, "OLDAAAAAAAAAAAAA"), "Old?", None);
    let mut device = question(&id_ago(10, "DEVAAAAAAAAAAAAA"), "Device?", None);
    device["session"] = json!("gamma-m1");
    let q = |a| question(&q1, "Which port?", a);
    world.questions(&[q(None)], &[device, q(Some("8787")), older]);
    answer(&hub, &q1, "8787");
    assert_eq!(
        cloud_polled(&hub),
        cloud(&format!("{HEAD}#AAAAAAAA Which port?\nAnswer: 8787"))
    );
    assert_eq!(cloud_polled(&hub), Vec::<Value>::new());

    // Parked again on a new question: only its answer goes.
    let q2 = id_ago(5, "Q2BBBBBBBBBBBBBB");
    let r = |a| question(&q2, "Keep the old route?", a);
    world.questions(&[r(None)], &[r(Some("no")), q(Some("8787"))]);
    answer(&hub, &q2, "no");
    assert_eq!(
        cloud_polled(&hub),
        cloud(&format!("{HEAD}#BBBBBBBB Keep the old route?\nAnswer: no"))
    );
}

#[test]
fn an_answer_while_the_mini_is_off_reaches_it_once_it_polls() {
    let world = World::new("cloud-answer-offline");
    let hub = world.hub();
    let q1 = id_ago(20, "Q1AAAAAAAAAAAAAA");
    let q = |a| question(&q1, "Which port?", a);
    world.questions(&[q(None)], &[q(Some("8787"))]);
    answer(&hub, &q1, "8787");
    assert_eq!(
        cloud_polled(&hub),
        cloud(&format!("{HEAD}#AAAAAAAA Which port?\nAnswer: 8787"))
    );
    assert_eq!(cloud_polled(&hub), Vec::<Value>::new());
}
