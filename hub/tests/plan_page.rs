//! An html-plan page in the hub (h1-plan-pages): the shell and its frame,
//! the page and the runtime the frame loads, the response it sends, and the
//! approval it offers.

mod common;

use std::path::{Path, PathBuf};

use common::{Hub, TempDir, body_of, real_mem, recording_mem, seed_project};

const PROJECT: &str = "proj-plans";

const PLAN_PAGE: &str = "<!doctype html>\n<html lang=\"en\">\n<meta charset=\"utf-8\">\n\
<title>Demo Plan</title>\n<link rel=\"stylesheet\" href=\"htmlplan.css\">\n\
<script src=\"htmlplan.js\" defer></script>\n<body>\n<main>\n<doc-plan>\n\
<doc-claim><p>The demo claim.</p></doc-claim>\n</doc-plan>\n</main>\n</body>\n</html>\n";

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
