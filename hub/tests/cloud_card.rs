//! The project page's way into a cloud run: the Run in cloud button, which
//! queues a cloud start for the mini, and the Start in cloud card, whose
//! prompt is the one `workflow go --cloud --print` prints. Over a fake `mem`
//! that answers the hub's `--json` reads and workflow's plain ones alike.

mod common;

use std::path::PathBuf;

use common::{Hub, TempDir, body_of, fixture_bin, header_of, status_of};
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

/// The mini's poll, and the commands it was handed.
fn poll(hub: &Hub) -> Value {
    let report = json!({"machine": "mini", "agents": [], "checkouts": []});
    serde_json::from_str::<Value>(&hub.poll_agent(&report.to_string())).unwrap()["commands"].clone()
}

fn page(hub: &Hub) -> String {
    let response = hub.get("/p/gamma");
    assert_eq!(status_of(&response), 200, "{response}");
    body_of(&response).to_string()
}

const RUN_IN_CLOUD: &str = "<input type=\"hidden\" name=\"do\" value=\"cloud\">";

#[test]
fn run_in_cloud_queues_a_cloud_start_for_the_mini() {
    let world = World::new("cloud-run-button");
    let hub = world.hub();
    assert_eq!(poll(&hub), json!([]));

    let body = page(&hub);
    assert!(body.contains(RUN_IN_CLOUD), "{body}");
    assert!(body.contains(">Run in cloud</button>"), "{body}");

    let response = hub.post_form("/p/gamma/control", "do=cloud");
    assert_eq!(status_of(&response), 303, "{response}");
    assert_eq!(header_of(&response, "location"), Some("/p/gamma"));
    assert_eq!(
        poll(&hub),
        json!([{"kind": "start", "project": "gamma", "cloud": true}])
    );
}

#[test]
fn run_in_cloud_is_refused_while_the_mini_is_offline() {
    let world = World::new("cloud-run-offline");
    let hub = world.hub();
    assert!(!page(&hub).contains(RUN_IN_CLOUD));
    let response = hub.post_form("/p/gamma/control", "do=cloud");
    assert_eq!(status_of(&response), 409, "{response}");
    assert!(body_of(&response).contains("mini is offline"), "{response}");
}

#[test]
fn run_in_cloud_is_refused_on_a_paused_project() {
    let world = World::new("cloud-run-paused");
    world.set("gamma", "paused", json!("2026-10-15T09:00:00Z by saiful"));
    let hub = world.hub();
    poll(&hub);
    assert!(!page(&hub).contains(RUN_IN_CLOUD));
    let response = hub.post_form("/p/gamma/control", "do=cloud");
    assert_eq!(status_of(&response), 409, "{response}");
    assert!(body_of(&response).contains("gamma is paused"), "{response}");
    assert_eq!(poll(&hub), json!([]));
}

#[test]
fn run_in_cloud_is_refused_while_another_cloud_run_holds_the_slot() {
    let world = World::new("cloud-run-held");
    world.set("delta", "runner", cloud("m2-next", &ago(60)));
    let hub = world.hub();
    poll(&hub);
    assert!(!page(&hub).contains(RUN_IN_CLOUD));
    let response = hub.post_form("/p/gamma/control", "do=cloud");
    assert_eq!(status_of(&response), 409, "{response}");
    assert!(
        body_of(&response).contains("a cloud run is live: delta"),
        "{response}"
    );
    assert_eq!(poll(&hub), json!([]));
}

#[test]
fn a_parked_cloud_run_still_holds_the_slot() {
    let world = World::new("cloud-run-parked");
    world.set("delta", "runner", cloud("m2-next", &ago(120)));
    world.set("delta", "handoff_at", json!(ago(60)));
    world.handoff("delta", "parked: t2 is local-only, question #ABCD1234\n");
    let hub = world.hub();
    poll(&hub);
    assert!(!page(&hub).contains(RUN_IN_CLOUD));
    let response = hub.post_form("/p/gamma/control", "do=cloud");
    assert_eq!(status_of(&response), 409, "{response}");
}
