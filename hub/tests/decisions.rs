//! `GET /p/<project>/decisions`: every ruling newest first, each with who
//! decided it, what it replaced, its date and a link to the whole item.

mod common;

use std::path::PathBuf;

use common::{
    Hub, TempDir, body_of, fixture_mem, invocations, mem_in, real_mem, recording_mem, seed_project,
    status_of,
};

const OLDEST: &str = "01M4546Y6ZZZZZZZZZZNTEPJ9K";
const MIDDLE: &str = "01M4546Y7FWT8ZGJKR0C90PWRA";
const NEWEST: &str = "01M4546Y7M7DG6D48QC797E1MD";

/// A fake `mem` that knows one project, `gamma`, with three rulings listed
/// oldest first: one decided by Saiful, one by an agent replacing another,
/// and one with no `by` at all. Every call is appended to `calls`.
struct World {
    _dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    calls: PathBuf,
}

impl World {
    fn new(tag: &str, rulings: serde_json::Value) -> World {
        let dir = TempDir::new(tag);
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let bin = dir.join("bin");
        let calls = dir.join("calls");
        let items = serde_json::json!({ "items": rulings });
        fixture_mem(
            &bin,
            &format!(
                "echo \"$*\" >>'{calls}'\n\
                 case \"$1\" in\n\
                 projects) echo '{{\"projects\":[{{\"name\":\"gamma\"}}]}}' ;;\n\
                 search) printf '%s\\n' '{items}' ;;\n\
                 *) exit 1 ;;\n\
                 esac",
                calls = calls.display(),
            ),
        );
        World {
            _dir: dir,
            home,
            bin,
            calls,
        }
    }

    /// The page's own reads: the doorbell's poll of the pending queue runs
    /// on its own clock and is left out.
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.calls)
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.starts_with("questions --pending"))
            .map(str::to_string)
            .collect()
    }

    /// The page's body, after checking it ran mem twice on a cold cache:
    /// the route's project check and the one search.
    fn page(&self) -> String {
        let hub = Hub::spawn(&self.home, &[&self.bin], &["--port", "0"]);
        let before = self.calls().len();
        let response = hub.get("/p/gamma/decisions");
        assert_eq!(status_of(&response), 200, "{response}");
        let calls = self.calls();
        let page_calls = &calls[before..];
        assert_eq!(page_calls.len(), 2, "{page_calls:?}");
        assert!(
            page_calls
                .iter()
                .any(|call| call == "search --kind ruling --limit 100 --project=gamma --json"),
            "{page_calls:?}"
        );
        body_of(&response).to_string()
    }
}

fn three_rulings() -> serde_json::Value {
    serde_json::json!([
        {"id": OLDEST, "short_id": "NTEPJ9K", "kind": "ruling", "created": "2026-10-01",
         "body": "\nKeep the store in plain files\n", "by": "saiful", "replaces": null},
        {"id": MIDDLE, "short_id": "0C90PWRA", "kind": "ruling", "created": "2026-10-02",
         "body": "\nNag once a day at most\n", "by": "agent",
         "replaces": "Nag every hour"},
        {"id": NEWEST, "short_id": "C797E1MD", "kind": "ruling", "created": "2026-10-03",
         "body": "\nShip the week view first\n", "by": null, "replaces": null},
    ])
}

/// The `<article>` holding `text`.
fn article<'a>(body: &'a str, text: &str) -> &'a str {
    let at = body
        .find(text)
        .unwrap_or_else(|| panic!("{text:?} not in {body}"));
    let start = body[..at].rfind("<article>").expect("an article");
    let end = at + body[at..].find("</article>").expect("its end");
    &body[start..end]
}

#[test]
fn a_ruling_by_saiful_says_so_with_its_date_and_link() {
    let world = World::new("decisions-saiful", three_rulings());
    let body = world.page();
    let ruling = article(&body, "Keep the store in plain files");

    assert!(
        ruling.contains("<p class=\"q\">Keep the store in plain files</p>"),
        "{ruling}"
    );
    assert!(ruling.contains("decided by Saiful"), "{ruling}");
    assert!(!ruling.contains("replaced:"), "{ruling}");
    assert!(ruling.contains("2026-10-01"), "{ruling}");
    assert!(
        ruling.contains(&format!("href=\"/p/gamma/item/{OLDEST}\"")),
        "{ruling}"
    );
}

#[test]
fn a_ruling_by_an_agent_says_what_it_replaced() {
    let world = World::new("decisions-agent", three_rulings());
    let body = world.page();
    let ruling = article(&body, "Nag once a day at most");

    assert!(ruling.contains("decided by an agent"), "{ruling}");
    assert!(ruling.contains("replaced: Nag every hour"), "{ruling}");
    assert!(ruling.contains("2026-10-02"), "{ruling}");
}

#[test]
fn a_ruling_with_no_by_shows_neither_line_and_rulings_run_newest_first() {
    let world = World::new("decisions-null", three_rulings());
    let body = world.page();
    let ruling = article(&body, "Ship the week view first");

    assert!(!ruling.contains("decided by"), "{ruling}");
    assert!(!ruling.contains("replaced:"), "{ruling}");
    assert!(ruling.contains("2026-10-03"), "{ruling}");

    let newest = body.find("Ship the week view first").unwrap();
    let middle = body.find("Nag once a day at most").unwrap();
    let oldest = body.find("Keep the store in plain files").unwrap();
    assert!(newest < middle && middle < oldest, "{body}");
}

#[test]
fn no_ruling_says_so() {
    let world = World::new("decisions-none", serde_json::json!([]));
    let body = world.page();

    assert!(
        body.contains("<p class=\"empty\">No rulings yet.</p>"),
        "{body}"
    );
    assert!(!body.contains("<article>"), "{body}");
}

#[test]
fn a_ruling_made_through_mem_shows_who_decided_and_what_it_replaced() {
    let dir = TempDir::new("decisions-real");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let (bin, log) = recording_mem(dir.path(), &home);
    let mem = real_mem().unwrap();
    seed_project(&mem, &home, "gamma", "gamma did a thing");
    let out = mem_in(
        &mem,
        &home,
        &home.join("gamma"),
        &[
            "decide",
            "--by",
            "saiful",
            "--replaces",
            "Nag by email",
            "Nag by notification, once a day at most",
        ],
    );
    assert!(out.status.success(), "{out:?}");
    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);

    let response = hub.get("/p/gamma/decisions");
    assert_eq!(status_of(&response), 200, "{response}");
    let body = body_of(&response).to_string();
    let ruling = article(&body, "Nag by notification, once a day at most");
    assert!(ruling.contains("decided by Saiful"), "{ruling}");
    assert!(ruling.contains("replaced: Nag by email"), "{ruling}");
    assert!(ruling.contains("href=\"/p/gamma/item/"), "{ruling}");
    assert!(
        invocations(&log).iter().any(|argv| argv
            == &[
                "search",
                "--kind",
                "ruling",
                "--limit",
                "100",
                "--project=gamma",
                "--json"
            ]),
        "{:?}",
        invocations(&log)
    );
}
