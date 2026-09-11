//! `GET /p/<project>/{log,roadmap,plan,plan/<slug>,items/<kind>,item/<id>}` —
//! the detail pages under a project's overview, read-only (m2-hub-pages).

mod common;

use std::path::{Path, PathBuf};

use common::{
    Hub, TempDir, body_of, fixture_mem, real_mem, recording_mem, seed_project, status_of,
};

const PROJECT: &str = "proj-solo";

struct World {
    _dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    mem: PathBuf,
}

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
        let out = common::mem_in(&self.mem, &self.home, &self.home.join(PROJECT), args);
        assert!(out.status.success(), "{args:?}: {out:?}");
        out
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

    fn hub(&self) -> Hub {
        Hub::spawn(&self.home, &[&self.bin], &["--port", "0"])
    }
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

/// Ruling 1: `/log` is the last 200 lines, wider than the overview's last 20.
#[test]
fn project_detail_log_page_reaches_further_back_than_the_overview() {
    let world = World::new("detail-log");
    for n in 1..=22 {
        world.run(&["log", &format!("log line {n}")]);
    }
    let hub = world.hub();

    let log_body = body_of(&hub.get(&format!("/p/{PROJECT}/log"))).to_string();
    assert!(log_body.contains("log line 1<"), "{log_body}");

    let overview_body = body_of(&hub.get(&format!("/p/{PROJECT}"))).to_string();
    assert!(
        !overview_body.contains("log line 1<"),
        "the overview's last-20 window should not reach this far: {overview_body}"
    );
}

/// Ruling 1 and 4: `/roadmap` renders the whole roadmap, uncut, with its task
/// boxes.
#[test]
fn project_detail_roadmap_page_is_not_cut_where_the_overview_is() {
    let world = World::new("detail-roadmap");
    let mut text = "# roadmap: demo\n\n".to_string();
    for n in 1..=45 {
        text.push_str(&format!("- [ ] m{n} Milestone {n}\n"));
    }
    let roadmap = write(&world.home, "roadmap.md", &text);
    world.run(&["roadmap", "--set-file", roadmap.to_str().unwrap()]);
    let hub = world.hub();

    let body = body_of(&hub.get(&format!("/p/{PROJECT}/roadmap"))).to_string();
    assert!(body.contains("Milestone 1<"), "{body}");
    assert!(body.contains("Milestone 45<"), "{body}");
    assert!(body.contains("checkbox"), "{body}");
}

/// Ruling 4: the plan of record's own page shows a ticked box.
#[test]
fn project_detail_plan_page_shows_a_ticked_box() {
    let world = World::new("detail-plan");
    let plan = write(
        &world.home,
        "plan.md",
        "# plan: demo\n\n- [x] t1 Done thing\n- [ ] t2 Open thing\n",
    );
    world.run(&["plan", "--set-file", plan.to_str().unwrap()]);
    let hub = world.hub();

    let body = body_of(&hub.get(&format!("/p/{PROJECT}/plan"))).to_string();
    assert!(body.contains("Done thing"), "{body}");
    assert!(body.contains("Open thing"), "{body}");
    assert!(body.contains("checked"), "a ticked box: {body}");
}

/// Ruling 1: `/plan/<slug>` is a stored plan, distinct from the plan of
/// record.
#[test]
fn project_detail_stored_plan_page_shows_its_own_text() {
    let world = World::new("detail-plan-slug");
    let stored = write(
        &world.home,
        "stored.md",
        "# plan: sub-slug\n\n- [ ] t1 A stored task\n",
    );
    world.run(&["plan", "sub-slug", "--set-file", stored.to_str().unwrap()]);
    let hub = world.hub();

    let body = body_of(&hub.get(&format!("/p/{PROJECT}/plan/sub-slug"))).to_string();
    assert!(body.contains("A stored task"), "{body}");
}

/// Ruling 1: `/items/<kind>` lists the last 100 items of one kind, each
/// linked to its own item page.
#[test]
fn project_detail_items_page_lists_one_kind() {
    let world = World::new("detail-items");
    world.run(&["save", "--kind", "ruling", "a ruling worth listing"]);
    let (id, title) = world.last_ruling();
    let hub = world.hub();

    let body = body_of(&hub.get(&format!("/p/{PROJECT}/items/ruling"))).to_string();
    assert!(body.contains(&title), "{body}");
    assert!(
        body.contains(&format!("href=\"/p/{PROJECT}/item/{id}\"")),
        "{body}"
    );
}

/// Ruling 4: `/item/<id>` shows the item's whole body, not only its title.
#[test]
fn project_detail_item_page_shows_the_whole_body() {
    let world = World::new("detail-item");
    world.run(&[
        "save",
        "--kind",
        "ruling",
        "Ruling headline\n\nThe rest of the ruling body, past the title.",
    ]);
    let (id, title) = world.last_ruling();
    let hub = world.hub();

    let body = body_of(&hub.get(&format!("/p/{PROJECT}/item/{id}"))).to_string();
    assert!(body.contains(&title), "{body}");
    assert!(
        body.contains("The rest of the ruling body, past the title."),
        "the whole body, not only the title: {body}"
    );
}

/// Ruling 1: a bad kind, slug or id is a 404 before it reaches `mem`, on every
/// detail route and for a project mem does not know either.
#[test]
fn project_detail_routes_404_for_a_bad_kind_slug_id_or_project() {
    let world = World::new("detail-404");
    let (id, _title) = {
        world.run(&["save", "--kind", "ruling", "a ruling"]);
        world.last_ruling()
    };
    let hub = world.hub();

    let good = format!("/p/{PROJECT}");
    for path in [
        format!("{good}/items/not-a-kind"),
        format!("{good}/items/"),
        format!("{good}/item/1234567"), // seven characters: neither 8 nor 26
        format!("{good}/item/{}", "I".repeat(26)), // 26, but `I` is not in mem's alphabet
        format!("{good}/item/not-an-id!"),
        format!("{good}/plan/../etc"),
        format!("{good}/plan/Not-A-Slug"),
        format!("{good}/plan/-flag"),
        "/p/no-such-project".to_string(),
        "/p/no-such-project/log".to_string(),
        "/p/no-such-project/roadmap".to_string(),
        "/p/no-such-project/plan".to_string(),
        format!("/p/no-such-project/plan/{id}"),
        "/p/no-such-project/items/ruling".to_string(),
        format!("/p/no-such-project/item/{id}"),
    ] {
        assert_eq!(status_of(&hub.get(&path)), 404, "{path}");
    }

    // The one wellformed id in that list belongs to a real item, so the route
    // itself is proven live before the malformed ones are trusted as 404s.
    assert_eq!(status_of(&hub.get(&format!("{good}/item/{id}"))), 200);
}

/// Ruling 2: mem being broken is a banner on a list-shaped detail page that
/// still renders, not an empty page that reads as "nothing here".
#[test]
fn project_detail_a_broken_mem_leaves_a_list_page_degraded_rather_than_empty() {
    let dir = TempDir::new("detail-degraded");
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

    let response = hub.get(&format!("/p/{PROJECT}/log"));
    assert_eq!(status_of(&response), 200);
    let body = body_of(&response);
    assert!(body.contains("not JSON"), "{body}");
}

/// Ruling 2: the same holds for the two singleton detail reads, the stored
/// plan and the item — a broken mem must not be mistaken for a slug or id
/// mem simply does not have (review 1 of detail).
#[test]
fn project_detail_a_broken_mem_leaves_the_plan_slug_and_item_pages_degraded_rather_than_404() {
    let dir = TempDir::new("detail-degraded-singleton");
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

    for path in [
        format!("/p/{PROJECT}/plan/somewhere"),
        format!("/p/{PROJECT}/item/28J1TSD1"),
    ] {
        let response = hub.get(&path);
        assert_eq!(status_of(&response), 200, "{path}");
        let body = body_of(&response);
        assert!(body.contains("not JSON"), "{path}: {body}");
    }
}

/// Review 1 of detail: `mem show` resolves an id against the whole index, not
/// one project, so the route itself must refuse an id that belongs to
/// another project rather than trust mem to scope it.
#[test]
fn project_detail_an_item_page_404s_when_the_id_belongs_to_another_project() {
    let dir = TempDir::new("detail-item-cross-project");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let (bin, _log) = recording_mem(dir.path(), &home);
    let mem = real_mem().unwrap();
    seed_project(&mem, &home, "proj-a", "a did a thing");
    seed_project(&mem, &home, "proj-b", "b did a thing");

    let out = common::mem_in(
        &mem,
        &home,
        &home.join("proj-a"),
        &[
            "save",
            "--kind",
            "ruling",
            "a ruling that belongs to proj-a",
        ],
    );
    assert!(out.status.success(), "{out:?}");
    let last = common::mem_in(
        &mem,
        &home,
        &home.join("proj-a"),
        &["log", "--kind", "ruling", "--limit", "1", "--json"],
    );
    let doc: serde_json::Value = serde_json::from_slice(&last.stdout).unwrap();
    let id = doc["items"][0]["id"].as_str().unwrap().to_string();

    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);
    assert_eq!(
        status_of(&hub.get(&format!("/p/proj-a/item/{id}"))),
        200,
        "sanity: the item is real, under its own project"
    );
    assert_eq!(
        status_of(&hub.get(&format!("/p/proj-b/item/{id}"))),
        404,
        "an item under another project must not be reachable here"
    );
}
