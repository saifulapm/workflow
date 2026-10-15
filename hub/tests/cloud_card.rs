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

#[test]
fn run_in_cloud_is_refused_while_a_device_orchestrator_runs_the_project() {
    let world = World::new("cloud-run-device-live");
    world.set(
        "gamma",
        "runner",
        json!({"machine": "mini", "id": "gamma-m2-next"}),
    );
    let hub = world.hub();
    let report = json!({"machine": "mini", "checkouts": ["gamma"],
        "agents": [{"id": "gamma-m2-next", "state": "working", "ended": 0}]});
    hub.poll_agent(&report.to_string());
    let body = page(&hub);
    assert!(body.contains("gamma-m2-next · working"), "{body}");
    assert_eq!(card(&body), None, "{body}");
    assert!(!body.contains(RUN_IN_CLOUD), "{body}");
    let response = hub.post_form("/p/gamma/control", "do=cloud");
    assert_eq!(status_of(&response), 409, "{response}");
    assert!(
        body_of(&response).contains("gamma-m2-next is already running"),
        "{response}"
    );
    assert_eq!(poll(&hub), json!([]));
}

/// A cloud run clones the project from its remote and cuts from main, so a
/// project with no remote, or one whose trunk is a worktree branch, gets no
/// card and no cloud start.
#[test]
fn run_in_cloud_is_refused_without_a_remote_or_on_a_worktree() {
    for (tag, key, value, why) in [
        (
            "cloud-run-no-remote",
            "remote",
            Value::Null,
            "gamma has no git remote",
        ),
        (
            "cloud-run-worktree",
            "worktree",
            json!("/home/saiful/Sites/gamma-wt"),
            "gamma works on a branch of its own; cloud runs take main",
        ),
    ] {
        let world = World::new(tag);
        world.set("gamma", key, value);
        let hub = world.hub();
        poll(&hub);
        let body = page(&hub);
        assert_eq!(card(&body), None, "{key}: {body}");
        assert!(!body.contains(RUN_IN_CLOUD), "{key}: {body}");
        let response = hub.post_form("/p/gamma/control", "do=cloud");
        assert_eq!(status_of(&response), 409, "{response}");
        assert!(body_of(&response).contains(why), "{response}");
        assert_eq!(poll(&hub), json!([]));
    }
}

const GO: &str = "<input type=\"hidden\" name=\"do\" value=\"go\">";

/// The mini's poll with a checkout of gamma, and the commands it was handed.
fn poll_with_gamma(hub: &Hub) -> Value {
    let report = json!({"machine": "mini", "agents": [], "checkouts": ["gamma"]});
    serde_json::from_str::<Value>(&hub.poll_agent(&report.to_string())).unwrap()["commands"].clone()
}

/// Once a cloud run has landed or ended, `workflow go` on a device takes
/// the project on from its branch, so the page offers a device start as it
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

/// The Start in cloud card's section of the page, or `None` without one.
fn card(body: &str) -> Option<String> {
    let at = body.find("<h2>Cloud run</h2>")?;
    let rest = &body[at..];
    let end = rest[4..].find("<h2>").map_or(rest.len(), |e| e + 4);
    Some(rest[..end].to_string())
}

/// The card's prompt, as the textarea holds it once unescaped.
fn prompt(card: &str) -> String {
    let open = card.find("<textarea").expect(card);
    let start = open + card[open..].find('>').unwrap() + 1;
    let end = card.find("</textarea>").expect(card);
    common::unescape(&card[start..end])
}

/// What `workflow go gamma --cloud --print` prints over the same fake mem.
fn printed(world: &World) -> String {
    let out = std::process::Command::new(common::workflow_bin())
        .args(["go", "gamma", "--cloud", "--print"])
        .env("WORKFLOW_MEM", world.bin.join("mem"))
        .env("WORKFLOW_CLAUDE", world.dir.join("no-claude"))
        .env("WORKFLOW_AMX", world.dir.join("no-amx"))
        .env("XDG_CONFIG_HOME", world.dir.join("config"))
        .env("WORKFLOW_MACHINE", "mini")
        .env_remove("AMX_ID")
        .current_dir(world.dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn the_card_shows_the_milestone_the_repo_and_the_prompt_to_copy() {
    let world = World::new("cloud-card");
    let hub = world.hub();
    let body = page(&hub);
    let card = card(&body).expect(&body);
    for part in [
        "m2-next",
        "git@github.com:saiful/gamma.git",
        "Environment: workflow",
        ">Copy prompt</button>",
        "<a href=\"https://claude.ai/code\" target=\"_blank\" rel=\"noopener\">Open Claude</a>",
    ] {
        assert!(card.contains(part), "{part:?} missing from {card}");
    }
    assert!(card.contains("<textarea"), "{card}");
    assert!(
        card[card.find("<textarea").unwrap()..].contains(" readonly"),
        "{card}"
    );
    assert!(
        prompt(&card).starts_with("Land milestone m2-next of gamma in this cloud session."),
        "{card}"
    );
}

#[test]
fn the_cards_prompt_is_the_one_workflow_prints_byte_for_byte() {
    let world = World::new("cloud-card-bytes");
    let hub = world.hub();
    let card = card(&page(&hub)).expect("a card");
    let printed = printed(&world);
    assert!(printed.contains("origin/main"), "{printed}");
    assert_eq!(prompt(&card), printed.strip_suffix('\n').unwrap());
}

#[test]
fn after_a_landed_cloud_run_the_card_cuts_from_its_branch_as_workflow_does() {
    let world = World::new("cloud-card-landed");
    world.set("gamma", "runner", cloud("m1-first", &ago(180)));
    let hub = world.hub();
    let card = card(&page(&hub)).expect("a card");
    let printed = printed(&world);
    assert!(printed.contains("origin/cloud/gamma-m1-first"), "{printed}");
    assert_eq!(prompt(&card), printed.strip_suffix('\n').unwrap());
}

#[test]
fn no_card_while_paused() {
    let world = World::new("cloud-card-paused");
    world.set("gamma", "paused", json!("2026-10-15T09:00:00Z by saiful"));
    let hub = world.hub();
    let body = page(&hub);
    assert_eq!(card(&body), None, "{body}");
}

#[test]
fn no_card_while_any_cloud_run_is_live_or_parked() {
    let world = World::new("cloud-card-live");
    world.set("delta", "runner", cloud("m2-next", &ago(60)));
    let hub = world.hub();
    let body = page(&hub);
    assert_eq!(card(&body), None, "live: {body}");
    drop(hub);

    world.set("delta", "handoff_at", json!(ago(30)));
    world.handoff("delta", "parked: t2 is local-only\n");
    let hub = world.hub();
    let body = page(&hub);
    assert_eq!(card(&body), None, "parked: {body}");
}

#[test]
fn a_cloud_run_silent_for_eight_hours_frees_the_card() {
    let world = World::new("cloud-card-lapsed");
    world.set("gamma", "runner", cloud("m2-next", &ago(8 * 60)));
    let hub = world.hub();
    poll(&hub);
    let body = page(&hub);
    assert!(card(&body).is_some(), "{body}");
    assert!(body.contains(RUN_IN_CLOUD), "{body}");
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

/// mem keeps a worktree per machine, so the one hub's store never holds
/// palmshop's: the mini's agent reports it, and that is what hides the card.
#[test]
fn a_worktree_the_mini_reports_keeps_the_project_off_the_cloud() {
    let world = World::new("cloud-run-reported-worktree");
    let hub = world.hub();
    let report = json!({"machine": "mini", "agents": [], "checkouts": ["gamma"],
        "worktrees": ["gamma"]});
    hub.poll_agent(&report.to_string());
    let body = page(&hub);
    assert_eq!(card(&body), None, "{body}");
    assert!(!body.contains(RUN_IN_CLOUD), "{body}");
    let response = hub.post_form("/p/gamma/control", "do=cloud");
    assert_eq!(status_of(&response), 409, "{response}");
    assert!(
        body_of(&response).contains("gamma works on a branch of its own; cloud runs take main"),
        "{response}"
    );
}
