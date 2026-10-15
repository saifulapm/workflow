//! Answering the last open question an orchestrator session asked queues a
//! `resume` of that session for its runner's machine, carrying every answer
//! the session got. Over a fake `mem` that prints whatever JSON the test last
//! put in `data/`, so a body or an answer is written with `serde_json` and
//! never fights the shell's quoting.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use common::{Hub, TempDir, body_of, fixture_bin, status_of, wait_for};
use serde_json::{Value, json};

const HEAD: &str = "Saiful answered your questions on the hub. Each answer is below; \
                    `mem questions` shows them once this machine's sync round lands.\n\n";

const Q1: &str = "01M0BF1F8BY8FXGZS4Q1AAAAAA";
const Q2: &str = "01M0BF1F8BY8FXGZS5Q2BBBBBB";

struct World {
    dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    data: PathBuf,
}

impl World {
    /// gamma, building with `runner`, whose questions the test stages with
    /// `pending` and `all`.
    fn new(tag: &str, runner: Value) -> World {
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
                 roadmap*) cat '{data}/roadmap.json' ;;\n\
                 plan\\ --list*) echo '{{\"plans\":[]}}' ;;\n\
                 handoff*) echo '{{\"body\":\"\"}}' ;;\n\
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
                "checkouts": [], "milestone": "m4-x", "plan_slug": "m4-x",
                "milestones_done": 3, "milestones_total": 5, "plan_ticked": 1,
                "plan_total": 4, "runner": runner}]}),
        );
        world.put(
            "roadmap.json",
            json!({"status": "approved", "text": "# roadmap: gamma\n\n- [ ] m4-x Next\n"}),
        );
        world.questions(&[], &[]);
        world
    }

    /// Written whole and renamed into place, so the fake never prints half a
    /// document.
    fn put(&self, name: &str, value: Value) {
        let staged = self.data.join(format!(".{name}"));
        std::fs::write(&staged, value.to_string()).unwrap();
        std::fs::rename(&staged, self.data.join(name)).unwrap();
    }

    /// What `mem questions` prints next: `pending` for the hub's pending
    /// listing, `all` for gamma's own, newest first as mem prints it.
    fn questions(&self, pending: &[Value], all: &[Value]) {
        self.put("pending.json", json!({ "questions": pending }));
        self.put("all.json", json!({ "questions": all }));
    }

    fn hub(&self, env: &[(&str, &str)]) -> Hub {
        let config = self.dir.join("config.toml");
        std::fs::write(
            &config,
            "topic = \"workflow-TESTTESTTESTTESTTESTTESTTE\"\n\
             ntfy_base = \"http://127.0.0.1:9\"\n",
        )
        .unwrap();
        Hub::spawn_env(
            &self.home,
            &[&self.bin],
            &["--port", "0", "--config", config.to_str().unwrap()],
            env,
        )
    }
}

fn mini() -> Value {
    json!({"machine": "mini", "id": "gamma-m4", "since": "2026-10-15T07:00:00Z"})
}

/// One question of gamma's, asked by `session`, answered with `answer`.
fn question(id: &str, body: &str, session: Option<&str>, answer: Option<&str>) -> Value {
    json!({"id": id, "short_id": &id[18..], "project": "gamma", "body": body,
        "session": session, "answered": answer.is_some(), "answer": answer})
}

/// The mini's poll, and the commands it was handed.
fn poll(hub: &Hub) -> Value {
    let report = json!({"machine": "mini", "agents": [], "checkouts": []});
    serde_json::from_str::<Value>(&hub.poll_agent(&report.to_string())).unwrap()["commands"].clone()
}

fn answer(hub: &Hub, id: &str, text: &str) {
    let response = hub.post_form("/answer", &format!("id={id}&text={text}"));
    assert_eq!(status_of(&response), 303, "{response}");
}

fn resume(message: &str) -> Value {
    json!([{"kind": "sync"},
        {"kind": "resume", "project": "gamma", "id": "gamma-m4", "message": message}])
}

#[test]
fn the_last_answer_of_a_session_queues_one_resume_with_every_answer_in_order() {
    let world = World::new("resume-two", mini());
    let hub = world.hub(&[]);
    assert_eq!(poll(&hub), json!([]));

    let q1 = |a| question(Q1, "Which port?", Some("gamma-m4"), a);
    let q2 = |a| question(Q2, "Keep the old route?", Some("gamma-m4"), a);
    world.questions(&[q1(None), q2(None)], &[q2(None), q1(Some("8787"))]);
    answer(&hub, Q1, "8787");
    assert_eq!(poll(&hub), json!([{"kind": "sync"}]), "one still open");

    world.questions(&[q2(None)], &[q2(Some("no, drop it")), q1(Some("8787"))]);
    answer(&hub, Q2, "no%2C+drop+it");
    let message = format!(
        "{HEAD}#Q1AAAAAA Which port?\nAnswer: 8787\n\n\
         #Q2BBBBBB Keep the old route?\nAnswer: no, drop it"
    );
    assert_eq!(poll(&hub), resume(&message));
}

/// An implementer's `mem ask --for orchestrator` carries its orchestrator's
/// amx id too, and nobody answers it on the hub, so it never holds back the
/// resume of the questions Saiful answers.
#[test]
fn a_question_for_the_orchestrator_does_not_hold_back_the_resume() {
    let world = World::new("resume-audience", mini());
    let hub = world.hub(&[]);
    poll(&hub);

    let q1 = |a| question(Q1, "Ship it?", Some("gamma-m4"), a);
    let mut worker = question(Q2, "Widen the files?", Some("gamma-m4"), None);
    worker["audience"] = json!("orchestrator");
    world.questions(&[q1(None)], &[worker, q1(Some("yes"))]);
    answer(&hub, Q1, "yes");
    assert_eq!(
        poll(&hub),
        resume(&format!("{HEAD}#Q1AAAAAA Ship it?\nAnswer: yes"))
    );
}

/// A `--for saiful` question is a person's like any other: the session waits
/// for its answer and is resumed with it.
#[test]
fn a_question_for_saiful_is_answered_into_the_resume() {
    let world = World::new("resume-saiful", mini());
    let hub = world.hub(&[]);
    poll(&hub);

    let q1 = |a| question(Q1, "Ship it?", Some("gamma-m4"), a);
    let saifuls = |a| {
        let mut q = question(Q2, "Approve the roadmap?", Some("gamma-m4"), a);
        q["audience"] = json!("saiful");
        q
    };
    world.questions(
        &[q1(None), saifuls(None)],
        &[saifuls(None), q1(Some("yes"))],
    );
    answer(&hub, Q1, "yes");
    assert_eq!(poll(&hub), json!([{"kind": "sync"}]), "Saiful's still open");

    world.questions(
        &[saifuls(None)],
        &[saifuls(Some("approve")), q1(Some("yes"))],
    );
    answer(&hub, Q2, "approve");
    assert_eq!(
        poll(&hub),
        resume(&format!(
            "{HEAD}#Q1AAAAAA Ship it?\nAnswer: yes\n\n\
             #Q2BBBBBB Approve the roadmap?\nAnswer: approve"
        ))
    );
}

/// Only the runner's own session comes back: answering a question an older
/// orchestrator of the project asked must not revive it beside the run.
#[test]
fn a_session_that_is_not_the_runner_is_not_resumed() {
    let world = World::new("resume-not-runner", mini());
    let hub = world.hub(&[]);
    poll(&hub);

    let q1 = |a| question(Q1, "Ship it?", Some("gamma-m3"), a);
    world.questions(&[q1(None)], &[q1(Some("yes"))]);
    answer(&hub, Q1, "yes");
    assert_eq!(poll(&hub), json!([{"kind": "sync"}]));
}

/// A session that parks again gets only what it has not been told: the
/// answers it came back with are already in its context.
#[test]
fn a_second_park_of_the_session_carries_only_the_new_answers() {
    let world = World::new("resume-second", mini());
    let hub = world.hub(&[]);
    poll(&hub);

    let q1 = |a| question(Q1, "Which port?", Some("gamma-m4"), a);
    world.questions(&[q1(None)], &[q1(Some("8787"))]);
    answer(&hub, Q1, "8787");
    assert_eq!(
        poll(&hub),
        resume(&format!("{HEAD}#Q1AAAAAA Which port?\nAnswer: 8787"))
    );

    let q2 = |a| question(Q2, "Keep the old route?", Some("gamma-m4"), a);
    world.questions(&[q2(None)], &[q2(Some("no")), q1(Some("8787"))]);
    answer(&hub, Q2, "no");
    assert_eq!(
        poll(&hub),
        resume(&format!("{HEAD}#Q2BBBBBB Keep the old route?\nAnswer: no"))
    );
}

#[test]
fn answering_the_last_question_twice_queues_one_resume() {
    let world = World::new("resume-twice", mini());
    let hub = world.hub(&[]);
    poll(&hub);

    // mem still lists it pending, as a second tap racing the first would see.
    let q1 = |a| question(Q1, "Ship it?", Some("gamma-m4"), a);
    world.questions(&[q1(None)], &[q1(Some("yes"))]);
    answer(&hub, Q1, "yes");
    answer(&hub, Q1, "yes");
    assert_eq!(
        poll(&hub),
        resume(&format!("{HEAD}#Q1AAAAAA Ship it?\nAnswer: yes"))
    );
}

#[test]
fn an_answer_with_quotes_backslashes_and_newlines_reaches_the_agent_byte_for_byte() {
    let world = World::new("resume-bytes", mini());
    let hub = world.hub(&[]);
    poll(&hub);

    let text = "say \"hi\"\\there\nagain";
    let q1 = |a| question(Q1, "What \"exactly\"?\n  Two lines.", Some("gamma-m4"), a);
    world.questions(&[q1(None)], &[q1(Some(text))]);
    answer(&hub, Q1, "say+%22hi%22%5Cthere%0Aagain");
    assert_eq!(
        poll(&hub),
        resume(&format!(
            "{HEAD}#Q1AAAAAA What \"exactly\"?\n  Two lines.\nAnswer: say \"hi\"\\there\nagain"
        ))
    );
}

#[test]
fn a_question_with_no_session_queues_no_resume() {
    let world = World::new("resume-nosession", mini());
    let hub = world.hub(&[]);
    poll(&hub);

    let q1 = |a| question(Q1, "Which port?", None, a);
    world.questions(&[q1(None)], &[q1(Some("8787"))]);
    answer(&hub, Q1, "8787");
    assert_eq!(poll(&hub), json!([{"kind": "sync"}]));
}

#[test]
fn a_project_with_no_runner_queues_no_resume() {
    let world = World::new("resume-norunner", Value::Null);
    let hub = world.hub(&[]);
    poll(&hub);

    let q1 = |a| question(Q1, "Which port?", Some("gamma-m4"), a);
    world.questions(&[q1(None)], &[q1(Some("8787"))]);
    answer(&hub, Q1, "8787");
    assert_eq!(poll(&hub), json!([{"kind": "sync"}]));
}

#[test]
fn a_resume_for_an_offline_mini_waits_for_its_next_poll() {
    let world = World::new("resume-offline", mini());
    let hub = world.hub(&[("HUB_AGENT_WINDOW_MS", "300")]);
    poll(&hub);
    let page = || body_of(&hub.get("/p/gamma")).to_string();
    wait_for("mini to go quiet", Duration::from_secs(5), || {
        page().contains("mini offline")
    });

    let q1 = |a| question(Q1, "Which port?", Some("gamma-m4"), a);
    world.questions(&[q1(None)], &[q1(Some("8787"))]);
    answer(&hub, Q1, "8787");
    let body = page();
    assert!(body.contains("waiting for mini"), "{body}");

    assert_eq!(
        poll(&hub),
        resume(&format!("{HEAD}#Q1AAAAAA Which port?\nAnswer: 8787"))
    );
}

#[test]
fn approving_the_roadmap_a_session_asked_about_resumes_it() {
    let world = World::new("resume-approve", mini());
    world.put(
        "roadmap.json",
        json!({"status": "draft", "text": "# roadmap: gamma\n\n- [ ] m4-x Next\n"}),
    );
    let hub = world.hub(&[]);
    poll(&hub);

    let q1 = |a| question(Q1, "Review the gamma roadmap", Some("gamma-m4"), a);
    world.questions(&[q1(None)], &[q1(Some("approve"))]);
    let response = hub.post_form("/p/gamma/control", "do=approve");
    assert_eq!(status_of(&response), 303, "{response}");
    assert_eq!(
        poll(&hub),
        resume(&format!(
            "{HEAD}#Q1AAAAAA Review the gamma roadmap\nAnswer: approve"
        ))
    );
}
