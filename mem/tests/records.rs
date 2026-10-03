//! The record verbs: decide, evidence, finding, raw, brief and idea. Each one
//! writes an item through the CLI and reads back what it wrote.

mod common;

use std::path::{Path, PathBuf};

use common::{World, code, mem, stderr, stdout};

fn json(out: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).expect("json output")
}

fn item_at(v: &serde_json::Value) -> mem::item::Item {
    mem::store::read_item(Path::new(v["path"].as_str().unwrap())).unwrap()
}

/// `projects/<id>/items/<file>` sits two levels under the project directory.
fn project_dir(v: &serde_json::Value) -> PathBuf {
    Path::new(v["path"].as_str().unwrap())
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}

#[test]
fn decide_records_a_ruling_with_by_and_replaces() {
    let w = World::new("records-decide");
    let repo = w.repo("thing", None);
    let out = mem(
        &w,
        &repo,
        &[
            "decide",
            "sessions live in redis",
            "--by",
            "saiful",
            "--replaces",
            "sessions live in the database",
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stdout(&out).ends_with("  ruling\n"), "{}", stdout(&out));

    let out = mem(
        &w,
        &repo,
        &[
            "decide",
            "use sqlite for the index",
            "--by",
            "agent",
            "--json",
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v = json(&out);
    assert_eq!(v["kind"], "ruling");
    let item = item_at(&v);
    assert_eq!(item.meta.by.as_deref(), Some("agent"));
    assert_eq!(item.meta.replaces, None);

    let out = mem(&w, &repo, &["search", "--kind", "ruling", "--json"]);
    let rows = json(&out)["items"].as_array().unwrap().clone();
    let first = rows
        .iter()
        .find(|r| r["title"] == "sessions live in redis")
        .expect("the first ruling is listed");
    let item = mem::store::read_item(Path::new(first["path"].as_str().unwrap())).unwrap();
    assert_eq!(item.meta.by.as_deref(), Some("saiful"));
    assert_eq!(
        item.meta.replaces.as_deref(),
        Some("sessions live in the database")
    );

    let out = mem(&w, &repo, &["decide", "no author", "--by", "someone"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
}

#[test]
fn evidence_add_records_a_copy_under_the_project_and_lists_it() {
    let w = World::new("records-evidence");
    let repo = w.repo("thing", None);
    let shot = w.dir.join("checkout.png");
    std::fs::write(&shot, b"png bytes").unwrap();
    let other = w.dir.join("cart.png");
    std::fs::write(&other, b"more png bytes").unwrap();

    let out = mem(
        &w,
        &repo,
        &[
            "evidence",
            "add",
            "--task",
            "pay",
            shot.to_str().unwrap(),
            "--note",
            "the pay button renders",
            "--json",
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v = json(&out);
    assert_eq!(v["kind"], "evidence");
    let copied = project_dir(&v).join("evidence/pay/checkout.png");
    assert_eq!(std::fs::read(&copied).unwrap(), b"png bytes");
    let item = item_at(&v);
    assert_eq!(item.meta.task.as_deref(), Some("pay"));
    assert_eq!(item.meta.file.as_deref(), Some("evidence/pay/checkout.png"));
    assert!(item.body_str().contains("the pay button renders"));

    let out = mem(
        &w,
        &repo,
        &[
            "evidence",
            "add",
            "--task",
            "cart",
            other.to_str().unwrap(),
            "--note",
            "the cart shows two lines",
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stdout(&out).ends_with("  evidence\n"), "{}", stdout(&out));

    let out = mem(&w, &repo, &["evidence", "list"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let listed = stdout(&out);
    let short = v["short_id"].as_str().unwrap();
    assert!(
        listed.contains(&format!(
            "#{short}  pay  evidence/pay/checkout.png  the pay button renders\n"
        )),
        "{listed}"
    );
    assert!(
        listed.contains("  cart  evidence/cart/cart.png  "),
        "{listed}"
    );

    let out = mem(&w, &repo, &["evidence", "list", "--task", "pay"]);
    let listed = stdout(&out);
    assert!(listed.contains("  pay  "), "{listed}");
    assert!(!listed.contains("  cart  "), "{listed}");

    let out = mem(&w, &repo, &["evidence", "list", "--json"]);
    let items = json(&out)["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 2);
}

#[test]
fn evidence_add_refuses_a_task_that_climbs_out_of_the_records_directory() {
    let w = World::new("records-evidence-task");
    let repo = w.repo("thing", None);
    let shot = w.dir.join("a.png");
    std::fs::write(&shot, b"x").unwrap();
    let out = mem(
        &w,
        &repo,
        &[
            "evidence",
            "add",
            "--task",
            "../escape",
            shot.to_str().unwrap(),
            "--note",
            "n",
        ],
    );
    assert_eq!(code(&out), 2, "{}", stderr(&out));
}

#[test]
fn finding_close_records_the_fix_and_drops_it_from_the_open_list() {
    let w = World::new("records-finding");
    let repo = w.repo("thing", None);
    let photo = w.dir.join("broken.png");
    std::fs::write(&photo, b"photo").unwrap();

    let out = mem(
        &w,
        &repo,
        &[
            "finding",
            "add",
            "--milestone",
            "m3-live-catalog",
            "--step",
            "2",
            "the price shows twice",
            "--evidence",
            photo.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let first = json(&out);
    assert_eq!(first["kind"], "finding");
    let item = item_at(&first);
    assert_eq!(item.meta.milestone.as_deref(), Some("m3-live-catalog"));
    assert_eq!(item.meta.step.as_deref(), Some("2"));
    assert_eq!(item.meta.status.as_deref(), Some("open"));
    assert_eq!(
        item.meta.file.as_deref(),
        Some("evidence/m3-live-catalog/broken.png")
    );
    assert!(
        project_dir(&first)
            .join("evidence/m3-live-catalog/broken.png")
            .exists()
    );

    let out = mem(
        &w,
        &repo,
        &[
            "finding",
            "add",
            "--milestone",
            "m3-live-catalog",
            "--step",
            "4",
            "the search box loses focus",
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stdout(&out).ends_with("  finding\n"), "{}", stdout(&out));

    let short = first["short_id"].as_str().unwrap();
    let out = mem(&w, &repo, &["finding", "close", short, "--by", "abc1234"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    // In place: the same file, now fixed.
    let item = item_at(&first);
    assert_eq!(item.meta.status.as_deref(), Some("fixed"));
    assert_eq!(item.meta.fixed_by.as_deref(), Some("abc1234"));

    let out = mem(&w, &repo, &["finding", "list", "--open"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let open = stdout(&out);
    assert!(open.contains("the search box loses focus"), "{open}");
    assert!(!open.contains("the price shows twice"), "{open}");

    let out = mem(&w, &repo, &["finding", "list"]);
    let all = stdout(&out);
    assert!(
        all.contains(&format!(
            "#{short}  fixed  m3-live-catalog  2  the price shows twice\n"
        )),
        "{all}"
    );
    assert!(all.contains("the search box loses focus"), "{all}");

    let out = mem(&w, &repo, &["finding", "list", "--open", "--json"]);
    let items = json(&out)["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["status"], "open");

    // Only a finding closes.
    let idea = json(&mem(&w, &repo, &["idea", "dark mode", "--json"]));
    let out = mem(
        &w,
        &repo,
        &[
            "finding",
            "close",
            idea["short_id"].as_str().unwrap(),
            "--by",
            "x",
        ],
    );
    assert_ne!(code(&out), 0);
}

#[test]
fn raw_add_records_a_copy_and_refuses_the_same_name_twice() {
    let w = World::new("records-raw");
    let repo = w.repo("thing", None);
    let notes = w.dir.join("llm-wiki.md");
    std::fs::write(&notes, "# notes\n").unwrap();

    let out = mem(
        &w,
        &repo,
        &["raw", "add", notes.to_str().unwrap(), "--json"],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let v = json(&out);
    assert_eq!(v["kind"], "raw");
    let stored = project_dir(&v).join("raw/llm-wiki.md");
    assert_eq!(std::fs::read_to_string(&stored).unwrap(), "# notes\n");
    let item = item_at(&v);
    assert_eq!(item.meta.file.as_deref(), Some("raw/llm-wiki.md"));
    assert_eq!(item.meta.source.as_deref(), notes.to_str());

    std::fs::write(&notes, "# changed\n").unwrap();
    let out = mem(&w, &repo, &["raw", "add", notes.to_str().unwrap()]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert_eq!(std::fs::read_to_string(&stored).unwrap(), "# notes\n");

    // A URL is fetched rather than copied.
    let page = w.dir.join("page.html");
    std::fs::write(&page, "<p>hi</p>").unwrap();
    let url = format!("file://{}", page.display());
    let out = mem(&w, &repo, &["raw", "add", &url]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stdout(&out).ends_with("  raw\n"), "{}", stdout(&out));
    let fetched = project_dir(&v).join("raw/page.html");
    assert_eq!(std::fs::read_to_string(&fetched).unwrap(), "<p>hi</p>");
}

#[test]
fn brief_records_the_newest_and_prints_it() {
    let w = World::new("records-brief");
    let repo = w.repo("thing", None);
    let out = mem(&w, &repo, &["brief"]);
    assert_eq!(code(&out), 1, "{}", stderr(&out));

    let out = mem(&w, &repo, &["brief", "--set", "a shop that sells lamps"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stdout(&out).ends_with("  brief\n"), "{}", stdout(&out));
    let out = mem(
        &w,
        &repo,
        &[
            "brief",
            "--set",
            "a shop that sells lamps and rugs",
            "--json",
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(json(&out)["kind"], "brief");

    let out = mem(&w, &repo, &["brief"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "a shop that sells lamps and rugs");

    let out = mem(&w, &repo, &["brief", "--json"]);
    assert_eq!(
        json(&out)["body"].as_str().unwrap().trim(),
        "a shop that sells lamps and rugs"
    );
}

#[test]
fn idea_records_an_idea() {
    let w = World::new("records-idea");
    let repo = w.repo("thing", None);
    let out = mem(&w, &repo, &["idea", "a gift wrap option"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stdout(&out).ends_with("  idea\n"), "{}", stdout(&out));

    let out = mem(&w, &repo, &["idea", "a wishlist", "--json"]);
    let item = item_at(&json(&out));
    assert_eq!(item.meta.kind.as_str(), "idea");
    assert!(item.body_str().contains("a wishlist"));

    let out = mem(&w, &repo, &["search", "--kind", "idea"]);
    assert!(
        stdout(&out).contains("a gift wrap option"),
        "{}",
        stdout(&out)
    );
}
