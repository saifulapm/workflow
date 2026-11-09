//! The project page and cloud runs: the Claude Project start line in place
//! of a Run in cloud card, and what a cloud run's runner record still does
//! to the device Start and Resume. Over a fake `mem` that answers the hub's
//! `--json` reads and workflow's plain ones alike.

mod common;

use std::path::PathBuf;

use common::{Hub, TempDir, body_of, fixture_bin, status_of};
use serde_json::{Value, json};

struct World {
    dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    data: PathBuf,
}

impl World {
    /// gamma and delta, both approved and open on m2-next, each with a
    /// checkout on disk; nothing runs either.
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
                "d='{data}'\n\
                 proj=''; prev=''\n\
                 for a in \"$@\"; do\n\
                   case \"$a\" in --project=*) proj=${{a#--project=}} ;; esac\n\
                   [ \"$prev\" = --project ] && proj=$a\n\
                   prev=$a\n\
                 done\n\
                 case \" $* \" in *' --json '*) ext=json ;; *) ext=md ;; esac\n\
                 case \"$1\" in\n\
                 projects) cat \"$d/projects.json\" ;;\n\
                 project) exit 1 ;;\n\
                 roadmap) cat \"$d/roadmap.$ext\" ;;\n\
                 handoff) [ -f \"$d/handoff-$proj.$ext\" ] && cat \"$d/handoff-$proj.$ext\" || exit 1 ;;\n\
                 questions) echo '{{\"questions\":[]}}' ;;\n\
                 plan) echo '{{\"plans\":[]}}' ;;\n\
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
        let mut rows = Vec::new();
        for name in ["gamma", "delta"] {
            let checkout = world.dir.join(&format!("{name}-checkout"));
            std::fs::create_dir_all(&checkout).unwrap();
            rows.push(json!({"name": name, "roadmap_status": "approved",
                "checkouts": [checkout], "milestone": "m2-next",
                "milestones_done": 1, "milestones_total": 3,
                "remote": format!("git@github.com:saiful/{name}.git")}));
        }
        world.put("projects.json", &json!({ "projects": rows }).to_string());
        let roadmap =
            "# roadmap: gamma\n\n- [x] m1-first One\n- [ ] m2-next Two\n- [ ] m3-last Three\n";
        world.put("roadmap.md", roadmap);
        world.put(
            "roadmap.json",
            &json!({"status": "approved", "text": roadmap}).to_string(),
        );
        world
    }

    fn put(&self, name: &str, text: &str) {
        std::fs::write(self.data.join(name), text).unwrap();
    }

    /// Sets `key` on `project`'s row.
    fn set(&self, project: &str, key: &str, value: Value) {
        let path = self.data.join("projects.json");
        let mut doc: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        for row in doc["projects"].as_array_mut().unwrap() {
            if row["name"] == project {
                row[key] = value.clone();
            }
        }
        self.put("projects.json", &doc.to_string());
    }

    /// `project`'s latest handoff, as both forms of `mem handoff` print it.
    fn handoff(&self, project: &str, body: &str) {
        self.put(
            &format!("handoff-{project}.json"),
            &json!({ "body": body }).to_string(),
        );
        self.put(&format!("handoff-{project}.md"), body);
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

/// A cloud runner on `milestone` since `since`.
fn cloud(milestone: &str, since: &str) -> Value {
    json!({"machine": "cloud", "id": "session_01Other", "since": since,
        "milestone": milestone})
}

fn page(hub: &Hub) -> String {
    let response = hub.get("/p/gamma");
    assert_eq!(status_of(&response), 200, "{response}");
    body_of(&response).to_string()
}

/// Every cloud run starts from the project's Claude Project, so the page
/// names the line to type there and offers no cloud start of its own.
#[test]
fn the_page_names_the_claude_project_start_line_and_offers_no_cloud_run() {
    let world = World::new("cloud-start-line");
    let hub = world.hub();
    let body = page(&hub);
    assert!(
        body.contains("Claude Project start line: Run the next milestone of gamma."),
        "{body}"
    );
    assert!(!body.contains("value=\"cloud\""), "{body}");
    assert!(!body.contains("Run in cloud"), "{body}");
    assert!(!body.contains("Cloud run"), "{body}");
}

const GO: &str = "<input type=\"hidden\" name=\"do\" value=\"go\">";

/// The mini's poll with a checkout of gamma, and the commands it was handed.
fn poll_with_gamma(hub: &Hub) -> Value {
    let report = json!({"machine": "mini", "agents": [], "checkouts": ["gamma"]});
    serde_json::from_str::<Value>(&hub.poll_agent(&report.to_string())).unwrap()["commands"].clone()
}

/// Once a cloud run has landed, ended or parked for a device, `workflow go`
/// on a device takes the project on from its branch, so the page offers a device start as it
/// does with no runner, beside the cloud line.
#[test]
fn a_landed_or_ended_cloud_run_offers_a_device_start() {
    for (tag, runner, handoff, state, button) in [
        (
            "cloud-device-landed",
            cloud("m1-first", &ago(180)),
            None,
            "cloud · m1-first · landed",
            ">Start on mini</button>",
        ),
        (
            "cloud-device-ended",
            cloud("m2-next", &ago(30)),
            Some("m2-t3 done, picking up t4\n"),
            "cloud · m2-next · ended",
            ">Resume on mini</button>",
        ),
        // Parked on a task only a device can do: a device takes it over.
        (
            "cloud-device-local-only",
            cloud("m2-next", &ago(30)),
            Some("parked: local-only t2: needs the desktop\n"),
            "cloud · m2-next · parked for a device",
            ">Resume on mini</button>",
        ),
    ] {
        let world = World::new(tag);
        world.set("gamma", "runner", runner);
        if let Some(body) = handoff {
            world.set("gamma", "handoff_at", json!(ago(5)));
            world.handoff("gamma", body);
        }
        let hub = world.hub();
        poll_with_gamma(&hub);
        let body = page(&hub);
        assert!(body.contains(state), "{body}");
        assert!(body.contains(GO), "{body}");
        assert!(body.contains(button), "{body}");
        let response = hub.post_form("/p/gamma/control", "do=go&machine=mini");
        assert_eq!(status_of(&response), 303, "{response}");
        assert_eq!(
            poll_with_gamma(&hub),
            json!([{"kind": "start", "project": "gamma"}])
        );
    }
}

#[test]
fn a_live_or_parked_cloud_run_offers_no_device_start() {
    let world = World::new("cloud-device-live");
    world.set("gamma", "runner", cloud("m2-next", &ago(30)));
    let hub = world.hub();
    poll_with_gamma(&hub);
    let body = page(&hub);
    assert!(body.contains("cloud · m2-next · live"), "{body}");
    assert!(!body.contains(GO), "{body}");
    drop(hub);

    world.set("gamma", "handoff_at", json!(ago(5)));
    world.handoff("gamma", "parked: t2 is local-only\n");
    let hub = world.hub();
    poll_with_gamma(&hub);
    let body = page(&hub);
    assert!(body.contains("cloud · m2-next · parked"), "{body}");
    assert!(!body.contains(GO), "{body}");
    let response = hub.post_form("/p/gamma/control", "do=go&machine=mini");
    assert_eq!(status_of(&response), 409, "{response}");
}

#[test]
fn the_run_line_shows_a_cloud_runner_by_its_state_and_session() {
    let world = World::new("cloud-run-line");
    world.set("gamma", "runner", cloud("m2-next", &ago(10)));
    let hub = world.hub();
    let body = page(&hub);
    assert!(
        body.contains("<span class=\"meta\">cloud · m2-next · live</span>"),
        "{body}"
    );
    assert!(
        body.contains("href=\"https://claude.ai/code/session_01Other\""),
        "{body}"
    );
    assert!(!body.contains("cloud offline"), "{body}");
    drop(hub);

    world.set("gamma", "handoff_at", json!(ago(5)));
    world.handoff("gamma", "parked: t2 is local-only\n");
    let hub = world.hub();
    let body = page(&hub);
    assert!(
        body.contains("<span class=\"meta\">cloud · m2-next · parked</span>"),
        "{body}"
    );
}
