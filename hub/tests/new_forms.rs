//! `GET /p/<project>/new` and `POST /p/<project>/new/<form>`: each form
//! through a recording `mem` over the real binary, then the store read back
//! with that binary.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{
    Hub, TempDir, fixture_bin, header_of, invocations, mem_in, real_mem, recording_mem,
    seed_project, status_of,
};
use hub::app::App;
use hub::config::Config;
use hub::form::Form;
use hub::http::Request;
use hub::memcli::MemCli;
use hub::pages::route_page;

const PROJECT: &str = "proj-alpha";
/// Written to the machine file both hub and mem read, so the two agree on a
/// name no real machine has.
const MACHINE: &str = "here-box";

/// A store with one project, and a hub whose `mem` records every argv before
/// running the real binary. The seeding runs the real binary directly, so the
/// log holds only what the hub ran.
struct World {
    _dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    log: PathBuf,
    mem: PathBuf,
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
        World {
            _dir: dir,
            home,
            bin,
            log,
            mem,
        }
    }

    /// A read of what the hub wrote, once `ready` holds of it. mem serves a
    /// read from the index as it was when another process holds the reindex
    /// lock, which the doorbell's own reads do now and then, so the first
    /// read after a write may not show it yet.
    fn read_back(
        &self,
        args: &[&str],
        ready: impl Fn(&serde_json::Value) -> bool,
    ) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let out = mem_in(&self.mem, &self.home, &self.home.join(PROJECT), args);
            let doc = serde_json::from_slice(&out.stdout).unwrap_or_default();
            if out.status.success() && ready(&doc) {
                return doc;
            }
            assert!(Instant::now() < deadline, "mem {args:?}: {out:?}");
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn hub(&self) -> Hub {
        Hub::spawn(&self.home, &[&self.bin], &["--port", "0"])
    }

    fn post(&self, hub: &Hub, form: &str, body: &str) -> String {
        let response = hub.post_form(&format!("/p/{PROJECT}/new/{form}"), body);
        println!("POST {form} {body} -> {}", status_of(&response));
        response
    }

    /// Every argv the hub ran but the doorbell's question poll, which runs
    /// on its own clock across all projects.
    fn calls(&self) -> Vec<Vec<String>> {
        invocations(&self.log)
            .into_iter()
            .filter(|argv| !argv.iter().any(|a| a == "--all-projects"))
            .collect()
    }

    /// The writes the hub ran, printed so the transcript shows each argv.
    /// Picked by verb: a doorbell read caught half-logged has no `--json` yet.
    fn writes(&self) -> Vec<Vec<String>> {
        let writes: Vec<Vec<String>> = invocations(&self.log)
            .into_iter()
            .filter(|argv| {
                matches!(
                    argv.first().map(String::as_str),
                    Some("idea" | "brief")
                )
            })
            .filter(|argv| !argv.iter().any(|a| a == "--json"))
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

fn sent(form: &str) -> String {
    format!("/p/{PROJECT}/new?sent={form}")
}

/// The page routed in-process over the recording `mem`, so no doorbell round
/// lands among its spawns, with the argv of every spawn it made.
fn render(world: &World, query: &str) -> (String, Vec<Vec<String>>) {
    // The hub's own process starts from a cleared environment; this one runs
    // inside the test's, which may name a project of its own.
    let clean = world._dir.join("clean");
    fixture_bin(
        &clean,
        "mem",
        &format!(
            "exec /usr/bin/env -i HOME='{home}' PATH=/usr/bin:/bin '{bin}/mem' \"$@\"",
            home = world.home.display(),
            bin = world.bin.display(),
        ),
    );
    let mut app = App::new(Config::default(), 0, MACHINE.to_string());
    app.mem = Arc::new(MemCli::with_path(&clean));
    let request = Request {
        method: "GET".to_string(),
        target: format!("/p/{PROJECT}/new?{query}"),
        path: format!("/p/{PROJECT}/new"),
        query: Form::parse(query),
        headers: Vec::new(),
        body: Vec::new(),
    };
    let before = world.calls().len();
    let response = route_page(&app, &request).unwrap();
    assert_eq!(response.status, 200);
    let body = String::from_utf8(response.body).unwrap();
    (body, world.calls()[before..].to_vec())
}

#[test]
fn the_page_shows_the_idea_and_brief_forms_for_two_spawns() {
    let world = World::new("new-page");
    let (body, spawns) = render(&world, "");
    assert_eq!(
        spawns,
        vec![
            argv(&["projects", "--json"]),
            argv(&["roadmap", "--project=proj-alpha", "--json"]),
        ]
    );
    for form in ["idea", "brief"] {
        let action = format!("action=\"/p/{PROJECT}/new/{form}\"");
        assert!(body.contains(&action), "{action} not in {body}");
    }
    for form in ["research", "round"] {
        let action = format!("action=\"/p/{PROJECT}/new/{form}\"");
        assert!(!body.contains(&action), "{action} in {body}");
    }
    assert_eq!(body.matches("<textarea name=\"text\"").count(), 2, "{body}");
    assert!(!body.contains("filed"), "{body}");

    let (body, _) = render(&world, "sent=idea");
    assert!(body.contains("filed"), "{body}");
}

#[test]
fn an_idea_reaches_mem_unchanged() {
    let world = World::new("new-idea");
    let hub = world.hub();

    for text in ["-x is a flag to mem", "first line\nsecond line"] {
        let body = format!("text={}", text.replace(' ', "+").replace('\n', "%0A"));
        let response = world.post(&hub, "idea", &body);
        assert_eq!(status_of(&response), 303, "{response}");
        assert_eq!(
            header_of(&response, "Location"),
            Some(sent("idea").as_str())
        );
    }
    assert_eq!(
        world.writes(),
        vec![
            argv(&["idea", "--project=proj-alpha", "--", "-x is a flag to mem"]),
            argv(&[
                "idea",
                "--project=proj-alpha",
                "--",
                "first line\nsecond line"
            ]),
        ]
    );
    let mut ideas: Vec<String> = world.read_back(&["search", "--kind", "idea", "--json"], |doc| {
        doc["items"]
            .as_array()
            .is_some_and(|items| items.len() == 2)
    })["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["body"].as_str().unwrap().trim().to_string())
        .collect();
    ideas.sort();
    assert_eq!(ideas, ["-x is a flag to mem", "first line\nsecond line"]);
}

#[test]
fn a_brief_reaches_mem_unchanged() {
    let world = World::new("new-brief");
    let hub = world.hub();

    let response = world.post(&hub, "brief", "text=-a+recipe+box%0Afor+two+phones");
    assert_eq!(status_of(&response), 303, "{response}");
    assert_eq!(
        header_of(&response, "Location"),
        Some(sent("brief").as_str())
    );
    assert_eq!(
        world.writes(),
        vec![argv(&[
            "brief",
            "--project=proj-alpha",
            "--set=-a recipe box\nfor two phones"
        ])]
    );
    let brief = world.read_back(&["brief", "--json"], |_| true);
    assert_eq!(
        brief["body"].as_str().unwrap().trim(),
        "-a recipe box\nfor two phones"
    );
}

#[test]
fn an_empty_text_writes_nothing() {
    let world = World::new("new-empty");
    let hub = world.hub();

    for form in ["idea", "brief"] {
        for body in ["text=+%0A+", ""] {
            let response = world.post(&hub, form, body);
            assert_eq!(status_of(&response), 303, "{form} {body:?}: {response}");
            assert_eq!(
                header_of(&response, "Location"),
                Some(format!("/p/{PROJECT}/new?empty=1").as_str())
            );
        }
    }
    assert_eq!(world.writes(), Vec::<Vec<String>>::new());
}

#[test]
fn an_unknown_form_is_not_found() {
    let world = World::new("new-unknown");
    let hub = world.hub();

    for form in ["plan", "idea/more", "research", "round", ""] {
        let response = world.post(&hub, form, "text=x");
        assert_eq!(status_of(&response), 404, "{form:?}: {response}");
    }
    assert_eq!(world.writes(), Vec::<Vec<String>>::new());
}
