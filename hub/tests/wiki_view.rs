//! `GET /wiki` and `GET /wiki/<project>/<slug>`.
//!
//! A page is markdown a session wrote after reading a repository or a web page,
//! so it is exactly as trusted as a question is — which is to say not at all.
//! Rendering it puts attacker-shaped text on the origin that owns
//! `POST /answer`, so most of this file is about what the renderer refuses to
//! emit: raw HTML, a `javascript:` link, an image from somewhere else.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use common::{
    Hub, TempDir, body_of, fixture_mem, invocations, real_mem, recording_mem, seed_project,
    status_of,
};

/// A store with two projects, pages in one of them, and a `mem` on PATH that
/// records every argv before running the real binary.
struct World {
    /// Held so the store outlives the test, and removed with it.
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
        std::fs::create_dir_all(&home).unwrap();
        let (bin, log) = recording_mem(dir.path(), &home);
        let mem = real_mem().unwrap();
        assert!(
            has_wiki(&mem),
            "{} has no `wiki` verb — build it: \
             cargo build --release --manifest-path mem/Cargo.toml",
            mem.display()
        );
        seed_project(&mem, &home, "proj-alpha", "alpha did a thing");
        seed_project(&mem, &home, "proj-beta", "beta did a thing");
        World {
            _dir: dir,
            home,
            bin,
            log,
            mem,
        }
    }

    /// Writes one page through the real `mem wiki <slug> --stdin`.
    fn page(&self, project: &str, slug: &str, text: &str) {
        let child = Command::new(&self.mem)
            .args(["wiki", slug, "--stdin", "--note", "seeding a page"])
            .current_dir(self.home.join(project))
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

    /// Every `mem wiki` hub ran.
    fn wiki_calls(&self) -> Vec<Vec<String>> {
        invocations(&self.log)
            .into_iter()
            .filter(|argv| argv.first().map(String::as_str) == Some("wiki"))
            .collect()
    }
}

fn has_wiki(mem: &Path) -> bool {
    Command::new(mem)
        .args(["wiki", "--help"])
        .output()
        .is_ok_and(|out| out.status.success())
}

/// The dashboard has to say the wiki is there; nobody types a URL they were
/// never shown.
#[test]
fn the_dashboard_links_to_the_wiki() {
    let world = World::new("wiki-dashboard-link");
    let hub = world.hub();

    let body = body_of(&hub.get("/")).to_string();

    assert!(body.contains("href=\"/wiki\""), "{body}");
}

#[test]
fn the_index_lists_every_project_that_has_pages_and_their_pages() {
    let world = World::new("wiki-index");
    world.page(
        "proj-alpha",
        "index",
        "# Alpha's wiki\n\nEvery page, one line each.\n",
    );
    world.page(
        "proj-alpha",
        "storage",
        "# How storage works\n\nIt writes.\n",
    );
    let hub = world.hub();

    let response = hub.get("/wiki");
    assert_eq!(status_of(&response), 200);
    let body = body_of(&response);

    assert!(body.contains("proj-alpha"), "{body}");
    assert!(
        body.contains("href=\"/wiki/proj-alpha/index\""),
        "a page is a link: {body}"
    );
    assert!(body.contains("href=\"/wiki/proj-alpha/storage\""), "{body}");
    // The first heading, which is what `mem wiki` reports as the title.
    assert!(body.contains("How storage works"), "{body}");
    // A project with no pages is not a section with nothing in it.
    assert!(!body.contains("proj-beta"), "{body}");
}

#[test]
fn a_store_with_no_pages_anywhere_says_so() {
    let world = World::new("wiki-empty");
    let hub = world.hub();

    let response = hub.get("/wiki");
    assert_eq!(status_of(&response), 200);
    assert!(body_of(&response).contains("No pages yet"), "{response}");
}

#[test]
fn a_page_renders_as_html() {
    let world = World::new("wiki-render");
    world.page(
        "proj-alpha",
        "storage",
        "# How storage works\n\n\
         It writes to disk, **atomically**.\n\n\
         - one\n- two\n\n\
         ```rust\nlet x = 1;\n```\n",
    );
    let hub = world.hub();

    let response = hub.get("/wiki/proj-alpha/storage");
    assert_eq!(status_of(&response), 200);
    let body = body_of(&response);

    assert!(body.contains("<h1>How storage works</h1>"), "{body}");
    assert!(body.contains("<strong>atomically</strong>"), "{body}");
    assert!(body.contains("<li>one</li>"), "{body}");
    assert!(body.contains("<code class=\"language-rust\">"), "{body}");
    // Not the markdown itself.
    assert!(!body.contains("**atomically**"), "{body}");
}

/// A link between pages is `[name](name.md)` (the plan's rule), and hub is one
/// of the renderers that has to follow it.
#[test]
fn a_link_to_another_page_points_at_that_page() {
    let world = World::new("wiki-links");
    world.page(
        "proj-alpha",
        "index",
        "# Alpha\n\nSee [storage](storage.md) and [the site](https://example.com/x).\n",
    );
    let hub = world.hub();

    let body = body_of(&hub.get("/wiki/proj-alpha/index")).to_string();

    assert!(body.contains("href=\"/wiki/proj-alpha/storage\""), "{body}");
    assert!(body.contains("href=\"https://example.com/x\""), "{body}");
}

/// The whole reason the renderer is not `push_html` on its own. A page is
/// written by a session that has been reading repositories and web pages, and
/// this origin owns `POST /answer`.
#[test]
fn a_page_that_is_script_renders_as_text() {
    let world = World::new("wiki-script");
    world.page(
        "proj-alpha",
        "storage",
        "# Storage\n\n\
         <script>alert(1)</script>\n\n\
         <img src=x onerror=alert(2)>\n\n\
         Inline <b onmouseover=\"alert(3)\">bold</b> too.\n",
    );
    let hub = world.hub();

    let body = body_of(&hub.get("/wiki/proj-alpha/storage")).to_string();

    // No tag of the page's own survives as markup.
    assert!(!body.contains("<script"), "{body}");
    assert!(!body.contains("<img"), "{body}");
    assert!(!body.contains("<b "), "{body}");
    // Refused, not silently dropped: the page still shows what was written,
    // escaped, so the handler that never runs is there to read.
    assert!(
        body.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
        "{body}"
    );
    assert!(
        body.contains("&lt;img src=x onerror=alert(2)&gt;"),
        "{body}"
    );
    assert!(body.contains("&lt;b onmouseover="), "{body}");
}

#[test]
fn a_javascript_link_or_a_foreign_image_never_reaches_the_page() {
    let world = World::new("wiki-schemes");
    world.page(
        "proj-alpha",
        "storage",
        "# Storage\n\n\
         [tap me](javascript:alert(1))\n\n\
         [or me](data:text/html;base64,PHNjcmlwdD4=)\n\n\
         ![beacon](https://tracker.example.com/pixel.png)\n\n\
         ![missing](diagram.png)\n",
    );
    let hub = world.hub();

    let body = body_of(&hub.get("/wiki/proj-alpha/storage")).to_string();

    assert!(!body.contains("javascript:"), "{body}");
    assert!(!body.contains("data:text/html"), "{body}");
    // No image is fetched from anywhere: a page is read on a phone, over the
    // tailnet, and an <img> is the one tag that phones home by itself.
    assert!(!body.contains("<img"), "{body}");
    // The text of each one survives, so nothing vanishes without a trace.
    assert!(body.contains("tap me"), "{body}");
    assert!(body.contains("beacon"), "{body}");
}

#[test]
fn a_page_carries_no_script_of_its_own() {
    let world = World::new("wiki-no-script");
    world.page("proj-alpha", "storage", "# Storage\n\nPlain.\n");
    let hub = world.hub();

    for path in ["/wiki", "/wiki/proj-alpha/storage"] {
        let body = body_of(&hub.get(path)).to_string();
        assert!(!body.contains("<script"), "{path}: {body}");
    }
}

#[test]
fn an_unknown_page_or_project_is_a_404() {
    let world = World::new("wiki-404");
    world.page("proj-alpha", "storage", "# Storage\n\nPlain.\n");
    let hub = world.hub();

    for path in [
        "/wiki/proj-alpha/nothing-here",
        "/wiki/no-such-project/storage",
        "/wiki/proj-alpha",
        "/wiki/proj-alpha/",
        "/wiki/proj-alpha/storage/extra",
    ] {
        assert_eq!(status_of(&hub.get(path)), 404, "{path}");
    }
}

/// The slug rule is mem's, and it is checked here as well: a path that is not a
/// slug never becomes an argument to a child process.
#[test]
fn a_path_that_is_not_a_slug_runs_no_mem_at_all() {
    let world = World::new("wiki-slug-rule");
    world.page("proj-alpha", "storage", "# Storage\n\nPlain.\n");
    let hub = world.hub();
    // One real read first, so the assertion below is about these paths and not
    // about hub never running `mem wiki`.
    assert_eq!(status_of(&hub.get("/wiki/proj-alpha/storage")), 200);
    let before = world.wiki_calls().len();

    for path in [
        "/wiki/proj-alpha/../../etc/passwd",
        "/wiki/proj-alpha/%2e%2e%2fstatus",
        "/wiki/proj-alpha/Storage",
        "/wiki/proj-alpha/-flag",
        "/wiki/proj-alpha/.hidden",
        "/wiki/..%2fproj-beta/storage",
    ] {
        assert_eq!(status_of(&hub.get(path)), 404, "{path}");
    }

    let after = world.wiki_calls();
    assert_eq!(
        after.len(),
        before,
        "a refused path still ran mem: {:?}",
        &after[before..]
    );
}

#[test]
fn the_wiki_answers_get_and_nothing_else() {
    let world = World::new("wiki-methods");
    world.page("proj-alpha", "storage", "# Storage\n\nPlain.\n");
    let hub = world.hub();

    for path in ["/wiki", "/wiki/proj-alpha/storage"] {
        let response = hub.post_form(path, "id=x&text=y");
        assert_eq!(status_of(&response), 405, "{path}: {response}");
        assert_eq!(
            common::header_of(&response, "Allow"),
            Some("GET"),
            "{response}"
        );
    }
}

/// §4a again: mem being broken is a banner on a page that still renders, not a
/// 500 and not an empty wiki that reads as "there are no pages".
#[test]
fn a_broken_mem_leaves_the_wiki_degraded_rather_than_empty() {
    let dir = TempDir::new("wiki-degraded");
    let home = dir.join("home");
    let bin = dir.join("bin");
    fixture_mem(&bin, "echo 'not json at all'");
    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);

    let response = hub.get("/wiki");
    assert_eq!(status_of(&response), 200);
    let body = body_of(&response);
    assert!(body.contains("not JSON"), "{body}");
    assert!(!body.contains("No pages yet"), "{body}");
}

/// Reads go through the argv contract, not the store: `--project=` in the
/// `--flag=value` form, so a project named `-weird` stays a value.
#[test]
fn hub_reads_pages_with_the_json_argv_contract() {
    let world = World::new("wiki-argv");
    world.page("proj-alpha", "storage", "# Storage\n\nPlain.\n");
    let hub = world.hub();

    assert_eq!(status_of(&hub.get("/wiki")), 200);
    assert_eq!(status_of(&hub.get("/wiki/proj-alpha/storage")), 200);

    let calls = world.wiki_calls();
    assert!(
        calls
            .iter()
            .any(|argv| argv.contains(&"--project=proj-alpha".to_string())
                && argv.contains(&"--json".to_string())),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|argv| argv.last() == Some(&"storage".to_string())),
        "{calls:?}"
    );
}
