//! `POST /p/<project>/control`: each verb through a recording `mem` over the
//! real binary, then the store read back with that binary.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{
    Hub, TempDir, header_of, invocations, mem_in, real_mem, recording_mem, seed_project, status_of,
};

const PROJECT: &str = "proj-alpha";
/// The approval question, as the plan skill asks it.
const REVIEW: &str = "Review the alpha roadmap";
/// Written to the machine file both hub and mem read, so the two agree on a
/// name no real machine has.
const MACHINE: &str = "here-box";

/// A store with one project and a draft roadmap, and a hub whose `mem` records
/// every argv before running the real binary. The seeding runs the real
/// binary directly, so the log holds only what the hub ran.
struct World {
    _dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    log: PathBuf,
    mem: PathBuf,
    config: PathBuf,
}

impl World {
    fn new(tag: &str) -> World {
        let dir = TempDir::new(tag);
        let home = dir.join("home");
        std::fs::create_dir_all(home.join("config/qshell")).unwrap();
        std::fs::write(home.join("config/qshell/machine"), MACHINE).unwrap();
        let (bin, log) = recording_mem(dir.path(), &home);
        let mem = real_mem().unwrap();
        seed_project(&mem, &home, PROJECT, "alpha did a thing");
        let config = dir.join("hub.toml");
        std::fs::write(
            &config,
            "topic = \"workflow-TESTTESTTESTTESTTESTTESTTE\"\n\
             ntfy_base = \"http://127.0.0.1:9\"\n",
        )
        .unwrap();
        let roadmap = dir.join("roadmap.md");
        std::fs::write(
            &roadmap,
            "# roadmap: alpha\n\n- [ ] m1 First milestone\n- [ ] m2 Second milestone\n",
        )
        .unwrap();
        let world = World {
            _dir: dir,
            home,
            bin,
            log,
            mem,
            config,
        };
        world.mem(&["roadmap", "--set-file", roadmap.to_str().unwrap()]);
        world.mem(&["roadmap", "--status", "draft"]);
        world
    }

    /// The real `mem`, run from inside the project's checkout.
    fn mem(&self, args: &[&str]) -> String {
        let out = mem_in(&self.mem, &self.home, &self.home.join(PROJECT), args);
        assert!(out.status.success(), "mem {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A read of what the hub wrote, once `ready` holds of it. mem serves a
    /// read from the index as it was when another process holds the reindex
    /// lock, which the doorbell's own reads do now and then, so the first
    /// read after a write may not show it yet.
    fn read_back(&self, args: &[&str], ready: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let out = mem_in(&self.mem, &self.home, &self.home.join(PROJECT), args);
            let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if out.status.success() && ready(&text) {
                return text;
            }
            assert!(Instant::now() < deadline, "mem {args:?}: {out:?}");
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn roadmap_status(&self) -> String {
        self.mem(&["roadmap", "--status"])
    }

    /// Asks what the plan skill asks once a roadmap is cut, and returns its
    /// id.
    fn ask_approval(&self) -> String {
        self.mem(&[
            "ask",
            "--options",
            "approve,changes",
            "--recommend",
            "approve",
            "--",
            REVIEW,
        ]);
        self.question(REVIEW)["id"].as_str().unwrap().to_string()
    }

    fn question(&self, title: &str) -> serde_json::Value {
        let out = self.mem(&["questions", "--json"]);
        let document: serde_json::Value = serde_json::from_str(&out).unwrap();
        document["questions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|q| q["title"] == title)
            .unwrap_or_else(|| panic!("{title:?} not in {document:#}"))
            .clone()
    }

    fn hub(&self) -> Hub {
        Hub::spawn(
            &self.home,
            &[&self.bin],
            &["--port", "0", "--config", self.config.to_str().unwrap()],
        )
    }

    fn post(&self, hub: &Hub, body: &str) -> String {
        let response = hub.post_form(&format!("/p/{PROJECT}/control"), body);
        println!("POST {body} -> {}", status_of(&response));
        response
    }

    /// The writes the hub ran, printed so the transcript shows each argv. A
    /// read always carries `--json`, which keeps the doorbell's poll out.
    fn writes(&self) -> Vec<Vec<String>> {
        let writes: Vec<Vec<String>> = invocations(&self.log)
            .into_iter()
            .filter(|argv| !argv.iter().any(|a| a == "--json"))
            .filter(|argv| match argv.first().map(String::as_str) {
                Some("answer" | "log") => true,
                Some("roadmap") => argv.iter().any(|a| a == "--status"),
                Some("project") => argv.get(1).is_some_and(|a| a == "set" || a == "unset"),
                _ => false,
            })
            .collect();
        for argv in &writes {
            println!("  mem {argv:?}");
        }
        writes
    }
}

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

#[test]
fn approve_sets_the_roadmap_approved_and_answers_its_question() {
    let world = World::new("control-approve");
    let id = world.ask_approval();
    let hub = world.hub();

    let response = world.post(&hub, "do=approve");
    assert_eq!(status_of(&response), 303, "{response}");
    assert_eq!(
        header_of(&response, "Location"),
        Some("/p/proj-alpha/roadmap")
    );
    assert_eq!(
        world.writes(),
        vec![
            argv(&["roadmap", "--project=proj-alpha", "--status", "approved"]),
            argv(&["answer", "--project=proj-alpha", "--", &id, "approve"]),
        ]
    );
    assert_eq!(world.roadmap_status(), "approved");
    assert_eq!(world.question(REVIEW)["answer"], "approve");
}

#[test]
fn approve_with_no_question_pending_only_sets_the_status() {
    let world = World::new("control-approve-bare");
    let hub = world.hub();

    let response = world.post(&hub, "do=approve");
    assert_eq!(status_of(&response), 303, "{response}");
    assert_eq!(
        world.writes(),
        vec![argv(&[
            "roadmap",
            "--project=proj-alpha",
            "--status",
            "approved"
        ])]
    );
    assert_eq!(world.roadmap_status(), "approved");
}

#[test]
fn changes_answers_the_question_and_leaves_the_roadmap_draft() {
    let world = World::new("control-changes");
    let id = world.ask_approval();
    let hub = world.hub();

    let response = world.post(&hub, "do=changes&text=split+m1+in+two");
    assert_eq!(status_of(&response), 303, "{response}");
    assert_eq!(
        header_of(&response, "Location"),
        Some("/p/proj-alpha/roadmap")
    );
    assert_eq!(
        world.writes(),
        vec![argv(&[
            "answer",
            "--project=proj-alpha",
            "--",
            &id,
            "changes: split m1 in two"
        ])]
    );
    assert_eq!(world.roadmap_status(), "draft");
    assert_eq!(world.question(REVIEW)["answer"], "changes: split m1 in two");
}

#[test]
fn changes_with_no_question_pending_is_logged() {
    let world = World::new("control-changes-log");
    let hub = world.hub();

    let response = world.post(&hub, "do=changes&text=-drop+m2");
    assert_eq!(status_of(&response), 303, "{response}");
    assert_eq!(
        world.writes(),
        vec![argv(&[
            "log",
            "--project=proj-alpha",
            "--",
            "roadmap changes requested: -drop m2"
        ])]
    );
    assert_eq!(world.roadmap_status(), "draft");
    let log = world.read_back(&["log", "--json"], |log| {
        log.contains("roadmap changes requested: -drop m2")
    });
    assert!(log.contains("roadmap changes requested: -drop m2"), "{log}");
}

#[test]
fn an_empty_changes_writes_nothing() {
    let world = World::new("control-changes-empty");
    let id = world.ask_approval();
    let hub = world.hub();

    let response = world.post(&hub, "do=changes&text=+++");
    assert_eq!(status_of(&response), 303, "{response}");
    assert_eq!(
        header_of(&response, "Location"),
        Some("/p/proj-alpha/roadmap?empty=1")
    );
    assert_eq!(world.writes(), Vec::<Vec<String>>::new());
    let question = world.question(REVIEW);
    assert_eq!(question["id"], id.as_str());
    assert_eq!(question["answered"], false);
}

#[test]
fn approve_and_changes_refuse_a_roadmap_that_is_not_a_draft() {
    let world = World::new("control-approved");
    world.mem(&["roadmap", "--status", "approved"]);
    let id = world.ask_approval();
    let hub = world.hub();

    for body in ["do=approve", "do=changes&text=split+m1"] {
        let response = world.post(&hub, body);
        assert_eq!(status_of(&response), 409, "{body}: {response}");
    }
    assert_eq!(world.writes(), Vec::<Vec<String>>::new());
    assert_eq!(world.roadmap_status(), "approved");
    let question = world.question(REVIEW);
    assert_eq!(question["id"], id.as_str());
    assert_eq!(question["answered"], false);
}

#[test]
fn an_unknown_verb_is_a_bad_request() {
    let world = World::new("control-unknown");
    let hub = world.hub();

    for body in ["do=run-next", "do=pause", "do=resume", ""] {
        let response = world.post(&hub, body);
        assert_eq!(status_of(&response), 400, "{body:?}: {response}");
    }
    assert_eq!(world.writes(), Vec::<Vec<String>>::new());
}
