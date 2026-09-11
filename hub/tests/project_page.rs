//! `GET /p/<project>` — one project's memory, read-only, on one page
//! (m2-hub-pages).

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use common::{
    Hub, TempDir, body_of, fixture_mem, mem_in, real_mem, recording_mem, seed_project, status_of,
};

/// One project, seeded through the real `mem` with a fact in every section
/// ruling 3 lists, and a `mem` on PATH that records every argv before
/// running the real binary.
struct World {
    _dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    mem: PathBuf,
}

const PROJECT: &str = "proj-solo";

impl World {
    fn new(tag: &str) -> World {
        let dir = TempDir::new(tag);
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let (bin, _log) = recording_mem(dir.path(), &home);
        let mem = real_mem().unwrap();
        seed_project(&mem, &home, PROJECT, "solo did a thing");
        World {
            _dir: dir,
            home,
            bin,
            mem,
        }
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        let out = mem_in(&self.mem, &self.home, &self.home.join(PROJECT), args);
        assert!(out.status.success(), "{args:?}: {out:?}");
        out
    }

    fn ask(&self, text: &str) -> String {
        let out = self.run(&["ask", text, "--json"]);
        let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        doc["id"].as_str().unwrap().to_string()
    }

    fn last_ruling(&self) -> (String, String) {
        let out = self.run(&["log", "--kind", "ruling", "--limit", "1", "--json"]);
        let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let row = &doc["items"][0];
        (
            row["id"].as_str().unwrap().to_string(),
            row["title"].as_str().unwrap().to_string(),
        )
    }

    fn wiki_page(&self, slug: &str, text: &str) {
        let child = Command::new(&self.mem)
            .args(["wiki", slug, "--stdin", "--note", "seeding a page"])
            .current_dir(self.home.join(PROJECT))
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin")
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_CACHE_HOME", self.home.join("cache"))
            .env("MEM_SYNC_CMD", "true")
            .env("MEM_NOTIFY_CMD", "true")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run mem wiki");
        {
            use std::io::Write;
            let mut stdin = child.stdin.as_ref().expect("stdin");
            stdin.write_all(text.as_bytes()).unwrap();
        }
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "writing {slug}: {out:?}");
    }

    fn hub(&self) -> Hub {
        Hub::spawn(&self.home, &[&self.bin], &["--port", "0"])
    }
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

/// The whole overview: every section ruling 3 lists, seeded through the real
/// `mem` and found on the page.
#[test]
fn the_project_page_shows_every_section() {
    let world = World::new("project-page-overview");

    world.run(&["status", "--set", "All green."]);
    world.run(&["handoff", "--set", "Picking up from here."]);

    let roadmap = write(
        &world.home,
        "roadmap.md",
        "# roadmap: demo\n\n- [ ] m1 First milestone\n",
    );
    world.run(&["roadmap", "--set-file", roadmap.to_str().unwrap()]);

    let plan = write(
        &world.home,
        "plan.md",
        "# plan: demo\n\n- [x] t1 Done thing\n- [ ] t2 Open thing\n",
    );
    world.run(&["plan", "--set-file", plan.to_str().unwrap()]);

    let stored = write(
        &world.home,
        "stored.md",
        "# plan: sub-slug\n\n- [ ] t1 Task\n",
    );
    world.run(&["plan", "sub-slug", "--set-file", stored.to_str().unwrap()]);

    world.run(&["save", "--kind", "ruling", "a ruling was made here"]);
    let (ruling_id, ruling_title) = world.last_ruling();

    let pending_id = world.ask("Should we ship on Friday?");
    let answered_id = world.ask("Is the roadmap current?");
    world.run(&["answer", &answered_id, "Yes, just updated it."]);

    world.wiki_page("index", "# Index\n\nWhat this project is.\n");

    let hub = world.hub();
    let response = hub.get(&format!("/p/{PROJECT}"));
    assert_eq!(status_of(&response), 200);
    let body = body_of(&response).to_string();

    assert!(body.contains("All green."), "status: {body}");
    assert!(body.contains("Picking up from here."), "handoff: {body}");

    assert!(body.contains("First milestone"), "roadmap: {body}");
    assert!(body.contains("checkbox"), "roadmap task box: {body}");
    assert!(
        body.contains(&format!("href=\"/p/{PROJECT}/roadmap\"")),
        "roadmap link: {body}"
    );

    assert!(body.contains("1/2"), "plan ticked/total: {body}");
    assert!(
        body.contains(&format!("href=\"/p/{PROJECT}/plan\"")),
        "plan link: {body}"
    );

    assert!(body.contains("sub-slug"), "stored plan: {body}");
    assert!(
        body.contains(&format!("href=\"/p/{PROJECT}/plan/sub-slug\"")),
        "stored plan link: {body}"
    );

    assert!(
        body.contains("Should we ship on Friday?"),
        "pending question: {body}"
    );
    assert!(
        body.contains("Is the roadmap current?"),
        "answered question: {body}"
    );
    assert!(body.contains("Yes, just updated it."), "answer: {body}");
    let pending_at = body.find("Should we ship on Friday?").unwrap();
    let answered_at = body.find("Is the roadmap current?").unwrap();
    assert!(
        pending_at < answered_at,
        "pending sorts before answered: {body}"
    );
    let _ = &pending_id;

    assert!(body.contains(&ruling_title), "ruling title: {body}");
    assert!(
        body.contains(&format!("href=\"/p/{PROJECT}/item/{ruling_id}\"")),
        "ruling link: {body}"
    );

    assert!(body.contains("solo did a thing"), "log entry: {body}");
    assert!(
        body.contains(&format!("href=\"/p/{PROJECT}/log\"")),
        "full log link: {body}"
    );

    assert!(
        body.contains(&format!("href=\"/wiki/{PROJECT}/index\"")),
        "wiki link: {body}"
    );

    assert!(body.contains("Runs on"), "runs heading: {body}");
    assert!(!body.contains("banner degraded"), "{body}");
}

/// Ruling 3: a roadmap past 40 lines is cut, with a link to the whole.
#[test]
fn a_long_roadmap_on_the_project_page_is_cut_at_forty_lines_with_a_link_to_the_whole() {
    let world = World::new("project-page-roadmap-cut");
    let mut text = "# roadmap: demo\n\n".to_string();
    for n in 1..=45 {
        text.push_str(&format!("- [ ] m{n} Milestone {n}\n"));
    }
    let roadmap = write(&world.home, "roadmap.md", &text);
    world.run(&["roadmap", "--set-file", roadmap.to_str().unwrap()]);
    let hub = world.hub();

    let body = body_of(&hub.get(&format!("/p/{PROJECT}"))).to_string();

    assert!(body.contains("Milestone 1<"), "{body}");
    assert!(!body.contains("Milestone 45"), "{body}");
    assert!(
        body.contains(&format!("href=\"/p/{PROJECT}/roadmap\"")),
        "{body}"
    );
}

#[test]
fn an_unknown_project_page_is_a_404() {
    let world = World::new("project-page-404");
    let hub = world.hub();

    assert_eq!(status_of(&hub.get("/p/no-such-project")), 404);
}

/// Ruling 6: the home page's project rows link to `/p/<name>`.
#[test]
fn the_home_page_links_to_the_project_page() {
    let world = World::new("project-page-home-link");
    let hub = world.hub();

    let body = body_of(&hub.get("/")).to_string();

    assert!(body.contains(&format!("href=\"/p/{PROJECT}\"")), "{body}");
}

/// Ruling 6: a wiki page's nav gains a link to its project's page.
#[test]
fn a_wiki_page_links_back_to_its_project_page() {
    let world = World::new("project-page-wiki-nav");
    world.wiki_page("index", "# Index\n\nHello.\n");
    let hub = world.hub();

    let body = body_of(&hub.get(&format!("/wiki/{PROJECT}/index"))).to_string();

    assert!(body.contains(&format!("href=\"/p/{PROJECT}\"")), "{body}");
}

/// §4a again: mem being broken is a banner on a page that still renders.
#[test]
fn a_broken_mem_leaves_the_project_page_degraded_rather_than_missing() {
    let dir = TempDir::new("project-page-degraded");
    let home = dir.join("home");
    let bin = dir.join("bin");
    fixture_mem(
        &bin,
        &format!(
            "if [ \"$1\" = projects ]; then echo '{{\"projects\":[{{\"name\":\"{PROJECT}\"}}]}}'; \
             else echo 'not json at all'; fi"
        ),
    );
    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);

    let response = hub.get(&format!("/p/{PROJECT}"));
    assert_eq!(status_of(&response), 200);
    let body = body_of(&response);
    assert!(body.contains("not JSON"), "{body}");
}
