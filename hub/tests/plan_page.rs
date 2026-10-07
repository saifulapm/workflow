//! An html-plan page in the hub (h1-plan-pages): the shell and its frame,
//! the page and the runtime the frame loads, the response it sends, and the
//! approval it offers.

mod common;

use std::path::{Path, PathBuf};

use hub::form::encode_component;

use common::{Hub, TempDir, body_of, header_of, real_mem, recording_mem, seed_project, status_of};

const PROJECT: &str = "proj-plans";

const PLAN_PAGE: &str = "<!doctype html>\n<html lang=\"en\">\n<meta charset=\"utf-8\">\n\
<title>Demo Plan</title>\n<link rel=\"stylesheet\" href=\"htmlplan.css\">\n\
<script src=\"htmlplan.js\" defer></script>\n<body>\n<main>\n<doc-plan>\n\
<doc-claim><p>The demo claim.</p></doc-claim>\n</doc-plan>\n</main>\n</body>\n</html>\n";

const REVIEW: &str = "Review the proj-plans roadmap and its plan pages";

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
        seed_project(&mem, &home, PROJECT, "plans did a thing");
        World {
            _dir: dir,
            home,
            bin,
            mem,
        }
    }

    fn run(&self, args: &[&str]) -> String {
        let out = common::mem_in(&self.mem, &self.home, &self.home.join(PROJECT), args);
        assert!(out.status.success(), "{args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn store_plan(&self, slug: &str, text: &str) {
        let path = write(&self.home, &format!("{slug}.plan"), text);
        self.run(&["plan", slug, "--set-file", path.to_str().unwrap()]);
    }

    /// Asks what the plan skill asks once a roadmap and its pages are cut,
    /// and returns the question's id.
    fn ask_review(&self) -> String {
        self.run(&[
            "ask",
            "--for",
            "human",
            "--options",
            "approve,changes",
            "--",
            REVIEW,
        ]);
        self.question(REVIEW)["id"].as_str().unwrap().to_string()
    }

    fn question(&self, title: &str) -> serde_json::Value {
        let doc: serde_json::Value =
            serde_json::from_str(&self.run(&["questions", "--json"])).unwrap();
        doc["questions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|q| q["title"] == title)
            .unwrap_or_else(|| panic!("{title:?} not in {doc:#}"))
            .clone()
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

#[test]
fn an_html_plan_opens_in_a_sandboxed_frame() {
    let world = World::new("plan-frame");
    world.store_plan("h1-demo", PLAN_PAGE);
    let hub = world.hub();

    let body = body_of(&hub.get(&format!("/p/{PROJECT}/plan/h1-demo"))).to_string();
    assert!(
        body.contains(&format!(
            "<iframe class=\"plan\" sandbox=\"allow-scripts allow-popups\" src=\"/p/{PROJECT}/plan/h1-demo/page\""
        )),
        "{body}"
    );
    assert!(!body.contains("allow-same-origin"), "{body}");
    assert!(
        !body.contains("doc-plan"),
        "the page is the frame's, not the shell's: {body}"
    );
}

#[test]
fn a_markdown_plan_still_renders_inline() {
    let world = World::new("plan-markdown");
    world.store_plan("m1", "# plan: m1\n\n- [ ] t1 Build it\n");
    let hub = world.hub();

    let body = body_of(&hub.get(&format!("/p/{PROJECT}/plan/m1"))).to_string();
    assert!(body.contains("Build it"), "{body}");
    assert!(!body.contains("<iframe"), "{body}");
}

#[test]
fn the_frames_page_loads_the_hubs_runtime_and_stays_sandboxed() {
    let world = World::new("plan-page");
    world.store_plan("h1-demo", PLAN_PAGE);
    let hub = world.hub();

    let response = hub.get(&format!("/p/{PROJECT}/plan/h1-demo/page"));
    assert_eq!(status_of(&response), 200, "{response}");
    // Opened on its own, outside the frame, the page still gets no origin.
    assert_eq!(
        header_of(&response, "content-security-policy"),
        Some("sandbox allow-scripts allow-popups; frame-ancestors 'self'"),
        "{response}"
    );
    let body = body_of(&response);
    assert!(body.contains("The demo claim."), "{body}");
    assert!(body.contains("href=\"/assets/htmlplan.css\""), "{body}");
    assert!(body.contains("/assets/htmlplan.js"), "{body}");
    assert!(!body.contains("src=\"htmlplan.js\""), "{body}");
}

#[test]
fn a_markdown_plan_has_no_frame_page() {
    let world = World::new("plan-page-md");
    world.store_plan("m1", "# plan: m1\n\n- [ ] t1 Build it\n");
    let hub = world.hub();

    assert_eq!(
        status_of(&hub.get(&format!("/p/{PROJECT}/plan/m1/page"))),
        404
    );
    assert_eq!(
        status_of(&hub.get(&format!("/p/{PROJECT}/plan/nope/page"))),
        404
    );
}

#[test]
fn the_served_runtime_sends_through_the_shell() {
    let world = World::new("plan-assets");
    let hub = world.hub();

    let js = hub.get("/assets/htmlplan.js");
    assert_eq!(status_of(&js), 200, "{js}");
    assert!(
        header_of(&js, "content-type")
            .unwrap()
            .starts_with("text/javascript")
    );
    let js = body_of(&js);
    assert!(js.contains("plan-respond"), "Send posts to the shell");
    assert!(js.contains("hubStore"), "drafts go through the shell");
    assert!(!js.contains("localStorage"), "an opaque origin has none");

    let css = hub.get("/assets/htmlplan.css");
    assert_eq!(status_of(&css), 200, "{css}");
    assert!(
        header_of(&css, "content-type")
            .unwrap()
            .starts_with("text/css")
    );
}

#[test]
fn a_response_answers_the_open_review_question() {
    let world = World::new("plan-respond");
    world.store_plan("h1-demo", PLAN_PAGE);
    let id = world.ask_review();
    let hub = world.hub();

    let md = "# Re: Demo Plan\n## Comments\n- > bigger type\n";
    let response = hub.post_form(
        &format!("/p/{PROJECT}/plan/h1-demo/respond"),
        &format!("md={}", encode_component(md)),
    );
    assert_eq!(status_of(&response), 303, "{response}");
    assert_eq!(
        header_of(&response, "location"),
        Some(format!("/p/{PROJECT}/plan/h1-demo?sent={}", &id[18..]).as_str())
    );
    assert_eq!(world.question(REVIEW)["answer"], md.trim());
}

#[test]
fn a_response_with_no_question_is_saved() {
    let world = World::new("plan-respond-save");
    world.store_plan("h1-demo", PLAN_PAGE);
    let hub = world.hub();

    let response = hub.post_form(
        &format!("/p/{PROJECT}/plan/h1-demo/respond"),
        "md=%23%23+Comments%0A-+%3E+bigger+type",
    );
    assert_eq!(status_of(&response), 303, "{response}");
    assert_eq!(
        header_of(&response, "location"),
        Some(format!("/p/{PROJECT}/plan/h1-demo?saved=1").as_str())
    );
    let found: serde_json::Value =
        serde_json::from_str(&world.run(&["search", "bigger type", "--json"])).unwrap();
    let hit = found.to_string();
    assert!(hit.contains("plan response: h1-demo"), "{found:#}");
}

#[test]
fn a_response_from_another_origin_is_refused() {
    let world = World::new("plan-respond-origin");
    world.store_plan("h1-demo", PLAN_PAGE);
    let id = world.ask_review();
    let hub = world.hub();

    let response = hub.post_form_with(
        &format!("/p/{PROJECT}/plan/h1-demo/respond"),
        "md=hello",
        &[("Origin", "null")],
    );
    assert_eq!(status_of(&response), 403, "{response}");
    assert!(world.question(REVIEW)["answer"].is_null(), "{id}");
}

#[test]
fn the_shell_relays_send_and_keeps_drafts_per_revision() {
    let world = World::new("plan-shell");
    world.store_plan("h1-demo", PLAN_PAGE);
    let hub = world.hub();

    let body = body_of(&hub.get(&format!("/p/{PROJECT}/plan/h1-demo"))).to_string();
    assert!(
        body.contains(&format!(
            "<form id=\"plan-respond\" method=\"post\" action=\"/p/{PROJECT}/plan/h1-demo/respond\""
        )),
        "{body}"
    );
    for message in ["plan-ready", "plan-restore", "plan-draft", "plan-respond"] {
        assert!(body.contains(message), "{message}: {body}");
    }
    let key = format!("plan:{PROJECT}:h1-demo:");
    assert!(body.contains(&key), "{body}");

    // A revised page starts from no draft rather than the old page's.
    world.store_plan("h1-demo", &PLAN_PAGE.replace("demo claim", "revised claim"));
    // A fresh hub, since the first one's cache still holds the old text.
    drop(hub);
    let hub = world.hub();
    let revised = body_of(&hub.get(&format!("/p/{PROJECT}/plan/h1-demo"))).to_string();
    let rev = |b: &str| b.split(&key).nth(1).unwrap()[..16].to_string();
    assert_ne!(rev(&body), rev(&revised));
}

#[test]
fn the_shell_says_where_the_response_went() {
    let world = World::new("plan-shell-banner");
    world.store_plan("h1-demo", PLAN_PAGE);
    let hub = world.hub();
    let at = format!("/p/{PROJECT}/plan/h1-demo");

    let sent = body_of(&hub.get(&format!("{at}?sent=7ZFVKFVX"))).to_string();
    assert!(sent.contains("Sent. Answer #7ZFVKFVX is in mem."), "{sent}");
    let saved = body_of(&hub.get(&format!("{at}?saved=1"))).to_string();
    assert!(saved.contains("Saved for the next planner."), "{saved}");
    let odd = body_of(&hub.get(&format!("{at}?sent=%3Cb%3E"))).to_string();
    assert!(!odd.contains("<b>") && !odd.contains("Sent."), "{odd}");
}

/// A browser sends a form's line breaks as CRLF; mem keeps the response as
/// the page wrote it.
#[test]
fn a_response_keeps_plain_line_breaks() {
    let world = World::new("plan-respond-crlf");
    world.store_plan("h1-demo", PLAN_PAGE);
    world.ask_review();
    let hub = world.hub();

    let response = hub.post_form(
        &format!("/p/{PROJECT}/plan/h1-demo/respond"),
        "md=%23+Re%0D%0A%0D%0A-+one%0D%0A",
    );
    assert_eq!(status_of(&response), 303, "{response}");
    assert_eq!(world.question(REVIEW)["answer"], "# Re\n\n- one");
}
