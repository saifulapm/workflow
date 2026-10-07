//! `GET /p/<project>`: the front page follows the project's stage. A header
//! with the stage, runner and progress, then what that stage is about.

mod common;

use std::path::PathBuf;

use common::{Hub, TempDir, body_of, fixture_mem, status_of};

/// One row per stage, carrying the flags the hub works the stage out from.
const PROJECTS_DOC: &str = r#"{"projects":[
{"name":"alpha","has_brief":true,"runner":"desk"},
{"name":"bare"},
{"name":"rho","has_brief":true,"has_research":true},
{"name":"gamma","has_research":true,"has_research_summary":true},
{"name":"sigma","has_research_summary":true,"has_spec":true},
{"name":"beta","roadmap_status":"draft","runner":"laptop","paused":"laptop 2026-10-04",
 "milestones_done":0,"milestones_total":2,"plan_ticked":1,"plan_total":3},
{"name":"exec","roadmap_status":"running","milestone":"e2","plan_slug":"e2",
 "milestones_done":1,"milestones_total":3,"plan_ticked":1,"plan_total":4}
]}"#;

const BRIEF_DOC: &str = r#"{"id":"01K0BRIEF","kind":"brief",
"body":"A shared **shopping list** two phones keep in step."}"#;

const WIKI_DOC: &str = r#"{"pages":[
{"slug":"index","title":"Index"},
{"slug":"research-apps","title":"Apps like it"},
{"slug":"research","title":"Research"},
{"slug":"notes","title":"Notes"}
]}"#;

const RULINGS_DOC: &str = r#"{"items":[
{"id":"01K0RULE2","kind":"ruling","title":"Sync by polling","body":"Sync by polling every minute"},
{"id":"01K0RULE1","kind":"ruling","title":"Store lists in SQLite","body":"Store lists in SQLite"}
]}"#;

const QUESTIONS_DOC: &str = r#"{"questions":[
{"id":"01K0Q1","title":"Do lists sync offline?","body":"Do lists sync offline?",
 "answered":false,"options":["yes","no"]}
]}"#;

const SECTIONS_DOC: &str = r#"{"sections":[
{"hslug":"1-the-list","heading":"1. The list","bytes":40},
{"hslug":"2-sync","heading":"2. Sync","bytes":30}
]}"#;

const ROADMAP_DOC: &str = r##"{"status":"draft","text":"# roadmap: beta\n\n- [ ] b1-box Recipes are stored and listed\n      Show: a recipe added on the phone is listed on the laptop\n- [ ] b2-scale A recipe scales to any number of people\n"}"##;

/// A fake `mem` answering every project's reads, each call appended to
/// `calls`. Anything it was not told about exits 1 with nothing printed,
/// which is how mem says there is none.
struct World {
    _dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    calls: PathBuf,
}

impl World {
    fn new(tag: &str) -> World {
        let dir = TempDir::new(tag);
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let bin = dir.join("bin");
        let calls = dir.join("calls");
        // printf rather than echo: a dash echo would turn the roadmap's `\n`
        // into real newlines inside the JSON string.
        fixture_mem(
            &bin,
            &format!(
                "echo \"$*\" >>'{calls}'\n\
                 p() {{ printf '%s\\n' \"$1\"; }}\n\
                 case \"$*\" in\n\
                 projects*) p '{PROJECTS_DOC}' ;;\n\
                 brief*--project=alpha*) p '{BRIEF_DOC}' ;;\n\
                 wiki*--project=rho*) p '{WIKI_DOC}' ;;\n\
                 log*--kind=ruling*--project=gamma*) p '{RULINGS_DOC}' ;;\n\
                 questions*--project=gamma*) p '{QUESTIONS_DOC}' ;;\n\
                 wiki*--sections*--project=sigma*spec) p '{SECTIONS_DOC}' ;;\n\
                 roadmap*--project=beta*) p '{ROADMAP_DOC}' ;;\n\
                 plan*--list*--project=beta*) p '{{\"plans\":[{{\"slug\":\"b1-box\"}}]}}' ;;\n\
                 log\\ --limit\\ 5\\ --project=exec*) p '{{\"items\":[{{\"id\":\"01K0LOG0000000000000000001\",\"kind\":\"log\",\"title\":\"Two of four tasks merged.\"}}]}}' ;;\n\
                 handoff*--project=exec*) p '{{\"body\":\"Picking up at e2-t3.\"}}' ;;\n\
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

    /// A hub that knows one sibling, the machine `laptop`.
    fn hub(&self) -> Hub {
        let config = self.home.join("config.toml");
        std::fs::write(
            &config,
            "topic = \"workflow-TESTTESTTESTTESTTESTTESTTE\"\n\
             ntfy_base = \"http://127.0.0.1:9\"\n\
             siblings = [\"http://laptop:8088\"]\n",
        )
        .unwrap();
        Hub::spawn(
            &self.home,
            &[&self.bin],
            &["--config", config.to_str().unwrap(), "--port", "0"],
        )
    }

    /// The page's own reads: the doorbell's poll of every project's
    /// questions runs on its own clock and is left out.
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.calls)
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.contains("--all-projects"))
            .map(str::to_string)
            .collect()
    }

    /// The front page's body, after checking it cost at most five `mem`
    /// spawns on a cold cache.
    fn page(&self, project: &str) -> String {
        let hub = self.hub();
        let before = self.calls().len();
        let response = hub.get(&format!("/p/{project}"));
        assert_eq!(status_of(&response), 200, "{response}");
        let calls = self.calls();
        assert!(calls.len() - before <= 5, "{:?}", &calls[before..]);
        body_of(&response).to_string()
    }
}

#[test]
fn a_brief_project_shows_its_brief_and_a_start_research_button() {
    let world = World::new("project-page-brief");
    let body = world.page("alpha");

    assert!(body.contains("brief"), "{body}");
    assert!(body.contains("<strong>shopping list</strong>"), "{body}");
    assert!(body.contains("action=\"/p/alpha/new/research\""), "{body}");
    assert!(body.contains("Start research</button>"), "{body}");
}

#[test]
fn a_project_with_no_brief_links_to_the_new_page() {
    let world = World::new("project-page-no-brief");
    let body = world.page("bare");

    assert!(body.contains("href=\"/p/bare/new\""), "{body}");
    assert!(!body.contains("Start research"), "{body}");
}

#[test]
fn a_research_project_lists_only_its_research_pages() {
    let world = World::new("project-page-research");
    let body = world.page("rho");

    assert!(body.contains("href=\"/wiki/rho/research-apps\""), "{body}");
    assert!(body.contains("Apps like it"), "{body}");
    assert!(body.contains("href=\"/wiki/rho/research\""), "{body}");
    assert!(!body.contains("/wiki/rho/notes"), "{body}");
    assert!(!body.contains("/wiki/rho/index\""), "{body}");
}

#[test]
fn a_grilling_project_shows_its_rulings_and_its_pending_questions() {
    let world = World::new("project-page-grilling");
    let body = world.page("gamma");

    assert!(body.contains("grilling"), "{body}");
    let newer = body.find("Sync by polling every minute").expect(&body);
    let older = body.find("Store lists in SQLite").expect(&body);
    assert!(newer < older, "newest ruling first: {body}");
    assert!(body.contains("Do lists sync offline?"), "{body}");
    assert!(body.contains("href=\"/p/gamma/questions\""), "{body}");
}

#[test]
fn a_spec_project_lists_the_spec_sections_linked_to_their_anchors() {
    let world = World::new("project-page-spec");
    let body = world.page("sigma");

    assert!(
        body.contains("href=\"/wiki/sigma/spec#1-the-list\">1. The list</a>"),
        "{body}"
    );
    assert!(
        body.contains("href=\"/wiki/sigma/spec#2-sync\">2. Sync</a>"),
        "{body}"
    );
}

#[test]
fn a_planning_project_shows_its_milestones_and_the_approval_forms() {
    let world = World::new("project-page-planning");
    let body = world.page("beta");

    assert!(body.contains("Recipes are stored and listed"), "{body}");
    assert!(
        body.contains("A recipe scales to any number of people"),
        "{body}"
    );
    assert!(body.contains("Approve</button>"), "{body}");
    assert!(body.contains("Request changes</button>"), "{body}");
}

#[test]
fn the_header_shows_stage_runner_progress_and_paused() {
    let world = World::new("project-page-header");
    let body = world.page("beta");

    for part in [
        "planning",
        "<a href=\"http://laptop:8088\">laptop</a>",
        "milestone 1 of 2",
        "tasks 1 of 3",
        "paused",
    ] {
        assert!(body.contains(part), "{part}: {body}");
    }
}

#[test]
fn an_execution_project_shows_its_last_moves_and_handoff() {
    let world = World::new("project-page-execution");
    let body = world.page("exec");

    assert!(body.contains("execution"), "{body}");
    assert!(body.contains("milestone 2 of 3"), "{body}");
    assert!(body.contains("<h2>Last moves</h2>"), "{body}");
    assert!(body.contains(">Two of four tasks merged.</a>"), "{body}");
    assert!(body.contains("Picking up at e2-t3."), "{body}");
}

#[test]
fn an_unknown_project_page_is_a_404() {
    let world = World::new("project-page-404");
    let hub = world.hub();

    assert_eq!(status_of(&hub.get("/p/no-such-project")), 404);
}

/// mem printing something that is not JSON is a banner on a page that still
/// renders.
#[test]
fn a_broken_mem_leaves_the_project_page_degraded_rather_than_missing() {
    let dir = TempDir::new("project-page-degraded");
    let home = dir.join("home");
    let bin = dir.join("bin");
    fixture_mem(
        &bin,
        "if [ \"$1\" = projects ]; then echo '{\"projects\":[{\"name\":\"alpha\"}]}'; \
         else echo 'not json at all'; fi",
    );
    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);

    let response = hub.get("/p/alpha");
    assert_eq!(status_of(&response), 200);
    let body = body_of(&response);
    assert!(body.contains("not JSON"), "{body}");
}
