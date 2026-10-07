//! `workflow go` -- start the orchestrator for a project's next milestone as an
//! amx agent.
//!
//! The orchestrator's last act is to run this for the milestone after its own,
//! so a roadmap is walked one milestone at a time with nothing in between.
//!
//! What to do is decided by pure functions over data the edge gathered from
//! mem and amx; the edge at the bottom is the only part that starts a process.
//! `$WORKFLOW_MEM` and `$WORKFLOW_AMX` name the two programs, so a test can put
//! a script in their place.

use std::path::Path;
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::memcli::{self, Project, ProjectRow};
use crate::{exit, warn};

pub const DEFAULT_MODEL: &str = "opus";
pub const DEFAULT_EFFORT: &str = "high";

/// How many names are tried when amx says the one it was given is taken.
const NAME_TRIES: usize = 20;

pub struct Options<'a> {
    /// The project by name; the current directory's when absent.
    pub project: Option<&'a str>,
    /// The milestone by slug; the roadmap's first open one when absent.
    pub milestone: Option<&'a str>,
    pub model: &'a str,
    pub effort: &'a str,
}

/// One agent of `amx ls --json`, as far as `go` reads it.
#[derive(Debug, Clone, Deserialize)]
pub struct AmxRow {
    pub id: String,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub ended: Option<i64>,
}

/// What `go` has settled on.
#[derive(Debug, PartialEq)]
pub enum Decision {
    /// Every milestone is ticked.
    Done(String),
    /// Not allowed, and why: exit 2.
    Refuse(String),
    /// Mem or amx could not answer: exit 1.
    Fail(String),
    Start(Start),
}

/// An orchestrator to start.
#[derive(Debug, PartialEq)]
pub struct Start {
    /// The agent's name, before any suffix a taken name needs.
    pub name: String,
    model: String,
    effort: String,
    dir: String,
    task: String,
}

impl Start {
    /// The `amx` arguments that start it under `name`.
    pub fn args(&self, name: &str) -> Vec<String> {
        [
            "new",
            "--name",
            name,
            "--model",
            &self.model,
            "--effort",
            &self.effort,
            "--permission",
            "bypassPermissions",
            "--no-worktree",
            "--dir",
            &self.dir,
            &self.task,
        ]
        .map(str::to_string)
        .to_vec()
    }
}

/// The first milestone of a roadmap that is not ticked: the slug of the first
/// `- [ ] <slug> ...` line. Only a line that starts there is a milestone; what
/// a milestone's own text indents under it is not.
pub fn next_milestone(roadmap: &str) -> Option<String> {
    roadmap.lines().find_map(|line| {
        let rest = line.strip_prefix("- [ ] ")?;
        rest.split_whitespace().next().map(str::to_string)
    })
}

/// The goal the orchestrator is started on.
pub fn task(project: &str, slug: &str) -> String {
    format!(
        "/goal Milestone {slug} of {project} is landed. Follow the orchestrate skill. \
Done means this conversation shows: every task in `mem plan` ticked; \
the project's test command passing on main after the last merge; \
the milestone review and the dogfood walk finished with their findings fixed or filed; \
`mem roadmap --tick {slug}` run; a `mem handoff` written; \
and `workflow go {project}` run for the next milestone, or its output saying the roadmap is done or not approved. \
If the milestone cannot continue without Saiful, done instead means a `mem ask` question filed, \
the handoff naming it, and the work parked. Stop after 6 hours."
    )
}

/// A name amx accepts: lowercase letters, digits and `-`, starting and ending
/// with a letter or digit. A project called `shopify_apps` is `shopify-apps`.
fn amx_name(text: &str) -> String {
    let mapped: String = text
        .chars()
        .map(|c| match c.to_ascii_lowercase() {
            c if c.is_ascii_lowercase() || c.is_ascii_digit() => c,
            _ => '-',
        })
        .collect();
    mapped.trim_matches('-').to_string()
}

/// Is agent `id` the orchestrator of this milestone? `launch` names it
/// `<project>-<milestone>`, then `-2`, `-3` while the name is taken. Only this
/// milestone's agent is in the way: the last milestone's orchestrator idles at
/// its prompt after starting its successor, and counting it refused every
/// restart of a successor that had died.
fn named_for(id: &str, project: &str, slug: &str) -> bool {
    let base = amx_name(&format!("{project}-{slug}"));
    match id.strip_prefix(&base) {
        Some("") => true,
        Some(rest) => rest
            .strip_prefix('-')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())),
        None => false,
    }
}

/// An agent that is still there. A killed pane reads `stopped` with `ended`
/// still 0, and `done` with `ended` 0 was seen on an orchestrator that was
/// alive and thinking, so it takes both fields. The hub reads them the same
/// way, so it never offers a Resume this refuses.
fn live(row: &AmxRow) -> bool {
    row.ended.unwrap_or(0) == 0 && !matches!(row.state.as_deref(), Some("stopped" | "failed"))
}

/// The id of another live agent running the orchestrator of `slug`. `me` is
/// the caller's own `$AMX_ID`: an orchestrator starting its successor is not
/// in its own way.
fn running(rows: &[AmxRow], project: &str, slug: &str, me: Option<&str>) -> Option<String> {
    rows.iter()
        .filter(|row| me != Some(row.id.as_str()))
        .filter(|row| live(row) && named_for(&row.id, project, slug))
        .map(|row| row.id.clone())
        .next()
}

pub fn parse_amx(text: &str) -> Result<Vec<AmxRow>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(text).map_err(|err| format!("amx ls --json is not a list: {err}"))
}

/// Settle what to do. `current` is what mem says of the working directory,
/// `roadmap` reads a project's roadmap and `agents` lists amx's agents; the
/// last two run only if the answer needs them.
pub fn decide(
    opts: &Options,
    projects: &[ProjectRow],
    current: Option<Project>,
    roadmap: impl FnOnce(&str) -> Option<String>,
    agents: impl FnOnce() -> Result<Vec<AmxRow>, String>,
    me: Option<&str>,
) -> Decision {
    // The project, its roadmap's status and the checkout to work in.
    let (project, dir) = match (opts.project, current) {
        (Some(name), current) => {
            let Some(row) = projects.iter().find(|row| row.name == name) else {
                return Decision::Refuse(format!(
                    "no project named {name}; `mem projects` lists them"
                ));
            };
            // The caller's own checkout first: an orchestrator naming its
            // project is standing in the clone it works in.
            let here = current.filter(|c| c.name == name).and_then(|c| c.root);
            let listed = row
                .checkouts
                .iter()
                .find(|dir| Path::new(dir).is_dir())
                .cloned();
            let Some(dir) = here.or(listed) else {
                return Decision::Refuse(format!("{name} has no checkout on this machine"));
            };
            (name.to_string(), dir)
        }
        (None, Some(here)) => {
            // The checkout the caller stands in; mem leaves it out only when
            // the project was named from outside one.
            let listed = projects
                .iter()
                .find(|row| row.name == here.name)
                .and_then(|row| row.checkouts.first());
            let Some(dir) = here.root.or_else(|| listed.cloned()) else {
                return Decision::Refuse(format!("{} has no checkout on this machine", here.name));
            };
            (here.name, dir)
        }
        (None, None) => {
            return Decision::Refuse(
                "this directory is in no project mem knows; name one: workflow go <project>".into(),
            );
        }
    };

    let slug = match opts.milestone {
        Some(slug) => slug.to_string(),
        None => {
            let status = projects
                .iter()
                .find(|row| row.name == project)
                .and_then(|row| row.roadmap_status.as_deref());
            if status != Some("approved") {
                return Decision::Refuse(format!(
                    "{project}: the roadmap is not approved (status: {})",
                    status.unwrap_or("none")
                ));
            }
            let Some(text) = roadmap(&project) else {
                return Decision::Fail(format!("cannot read the roadmap of {project}"));
            };
            match next_milestone(&text) {
                Some(slug) => slug,
                None => return Decision::Done(project),
            }
        }
    };

    match agents() {
        Err(why) => return Decision::Fail(why),
        Ok(rows) => {
            if let Some(id) = running(&rows, &project, &slug, me) {
                return Decision::Refuse(format!(
                    "{project} already has an orchestrator running: {id}"
                ));
            }
        }
    }

    Decision::Start(Start {
        name: amx_name(&format!("{project}-{slug}")),
        model: opts.model.to_string(),
        effort: opts.effort.to_string(),
        dir,
        task: task(&project, &slug),
    })
}

/// A command line a shell would read back as the same words.
pub fn shell_line(program: &str, args: &[String]) -> String {
    let word = |text: &str| {
        let plain = !text.is_empty()
            && text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
        if plain {
            text.to_string()
        } else {
            format!("'{}'", text.replace('\'', "'\\''"))
        }
    };
    std::iter::once(program)
        .chain(args.iter().map(String::as_str))
        .map(word)
        .collect::<Vec<_>>()
        .join(" ")
}

/// What one `amx new` came to.
pub struct Spawned {
    pub ok: bool,
    pub stderr: String,
}

/// Start the agent under `name`, or under `name-2`, `name-3`, ... while amx says
/// the name is taken (by an agent that has ended: a live one was refused
/// before this). The name the agent runs under, or why none did.
pub fn launch(
    start: &Start,
    mut spawn: impl FnMut(&[String]) -> Spawned,
) -> Result<String, String> {
    let mut last = String::new();
    for attempt in 1..=NAME_TRIES {
        let name = match attempt {
            1 => start.name.clone(),
            n => format!("{}-{n}", start.name),
        };
        let out = spawn(&start.args(&name));
        if out.ok {
            return Ok(name);
        }
        last = out.stderr.trim().to_string();
        if !last.contains("already taken") {
            break;
        }
    }
    Err(if last.is_empty() {
        "amx new failed".to_string()
    } else {
        last
    })
}

fn amx_bin() -> String {
    match std::env::var("WORKFLOW_AMX") {
        Ok(v) if !v.is_empty() => v,
        _ => "amx".to_string(),
    }
}

fn amx_agents() -> Result<Vec<AmxRow>, String> {
    let out = Command::new(amx_bin())
        .args(["ls", "--json"])
        .stderr(Stdio::piped())
        .output()
        .map_err(|err| format!("cannot run {}: {err}", amx_bin()))?;
    if !out.status.success() {
        return Err(format!(
            "amx ls failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    parse_amx(&String::from_utf8_lossy(&out.stdout))
}

fn amx_new(args: &[String]) -> Spawned {
    match Command::new(amx_bin()).args(args).output() {
        Ok(out) => Spawned {
            ok: out.status.success(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        },
        Err(err) => Spawned {
            ok: false,
            stderr: format!("cannot run {}: {err}", amx_bin()),
        },
    }
}

pub fn cmd_go(opts: &Options, dry_run: bool) -> i32 {
    let Some(projects) = memcli::projects() else {
        warn("go: mem cannot list its projects");
        return exit::FAILED;
    };
    let current = memcli::project_current();
    let me = std::env::var("AMX_ID").ok().filter(|id| !id.is_empty());

    match decide(
        opts,
        &projects,
        current,
        memcli::roadmap,
        amx_agents,
        me.as_deref(),
    ) {
        Decision::Done(project) => {
            println!("{project}: the roadmap is done");
            exit::OK
        }
        Decision::Refuse(why) => {
            warn(format!("go: {why}"));
            exit::USAGE
        }
        Decision::Fail(why) => {
            warn(format!("go: {why}"));
            exit::FAILED
        }
        Decision::Start(start) if dry_run => {
            println!("{}", shell_line(&amx_bin(), &start.args(&start.name)));
            exit::OK
        }
        Decision::Start(start) => match launch(&start, amx_new) {
            Ok(name) => {
                println!("{name}");
                exit::OK
            }
            Err(why) => {
                warn(format!("go: {why}"));
                exit::FAILED
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::scratch::Scratch;

    const ROADMAP: &str = "\
# roadmap: alpha

The first stretch of a shop.

- [x] skeleton The platform opens
      Show: Saiful opens the admin
      Done: three Workers deploy
- [X] winners Find and import products  [after: skeleton]
- [ ] catalog Prices and stock stay true  [after: winners]
      Done: - [ ] a list in prose is not a milestone
- [ ] orders An order reaches the supplier
";

    fn row(name: &str, status: Option<&str>, checkouts: &[&Path]) -> ProjectRow {
        ProjectRow {
            name: name.to_string(),
            checkouts: checkouts
                .iter()
                .map(|dir| dir.display().to_string())
                .collect(),
            roadmap_status: status.map(str::to_string),
        }
    }

    fn agent(id: &str, state: &str) -> AmxRow {
        AmxRow {
            id: id.to_string(),
            state: Some(state.to_string()),
            ended: Some(0),
        }
    }

    fn opts<'a>(project: Option<&'a str>, milestone: Option<&'a str>) -> Options<'a> {
        Options {
            project,
            milestone,
            model: DEFAULT_MODEL,
            effort: DEFAULT_EFFORT,
        }
    }

    /// A decision where nothing is running and the roadmap is `ROADMAP`.
    fn decide_quietly(opts: &Options, projects: &[ProjectRow]) -> Decision {
        decide(
            opts,
            projects,
            None,
            |_| Some(ROADMAP.to_string()),
            || Ok(Vec::new()),
            None,
        )
    }

    #[test]
    fn the_next_milestone_is_the_first_unticked_line() {
        assert_eq!(next_milestone(ROADMAP).as_deref(), Some("catalog"));
        // A ticked line is `[x]` or `[X]`, a line in a block is not a line of
        // the roadmap, and a roadmap with nothing open has no next one.
        assert_eq!(
            next_milestone("- [x] one\n- [X] two\n      - [ ] note\n"),
            None
        );
        assert_eq!(next_milestone(""), None);
        assert_eq!(
            next_milestone("- [ ] only  [after: nothing]\n").as_deref(),
            Some("only")
        );
    }

    #[test]
    fn the_goal_is_the_exact_text_a_milestone_is_held_to() {
        let want = "/goal Milestone winners of alpha is landed. Follow the orchestrate skill. \
Done means this conversation shows: every task in `mem plan` ticked; \
the project's test command passing on main after the last merge; \
the milestone review and the dogfood walk finished with their findings fixed or filed; \
`mem roadmap --tick winners` run; a `mem handoff` written; \
and `workflow go alpha` run for the next milestone, or its output saying the roadmap is done or not approved. \
If the milestone cannot continue without Saiful, done instead means a `mem ask` question filed, \
the handoff naming it, and the work parked. Stop after 6 hours.";
        assert_eq!(task("alpha", "winners"), want);
    }

    #[test]
    fn a_name_is_what_amx_accepts() {
        assert_eq!(amx_name("alpha-winners"), "alpha-winners");
        assert_eq!(amx_name("shopify_apps-catalog"), "shopify-apps-catalog");
        assert_eq!(amx_name("Lareys.dev-Go Live"), "lareys-dev-go-live");
        assert_eq!(amx_name("-edge-"), "edge");
    }

    #[test]
    fn an_agent_is_the_milestones_by_its_whole_name_or_a_retry_suffix() {
        assert!(named_for("alpha-winners", "alpha", "winners"));
        assert!(named_for("alpha-winners-2", "alpha", "winners"));
        assert!(named_for("shopify-apps-catalog", "shopify_apps", "catalog"));
        assert!(!named_for("alpha-winners-x", "alpha", "winners"));
        assert!(!named_for("alpha-winners-", "alpha", "winners"));
        assert!(!named_for("alpha-winnerstwo", "alpha", "winners"));
        assert!(!named_for("alpha-skeleton", "alpha", "winners"));
        assert!(!named_for("alpha-mobile-winners", "alpha", "winners"));
    }

    #[test]
    fn an_agent_is_running_until_it_ended_or_lost_its_pane() {
        for state in ["starting", "working", "waiting", "idle", "done", "unknown"] {
            assert!(live(&agent("a-b", state)), "{state}");
        }
        // A killed pane: amx says stopped and never sets ended.
        for state in ["stopped", "failed"] {
            assert!(!live(&agent("a-b", state)), "{state}");
        }
        assert!(!live(&AmxRow {
            id: "a-b".into(),
            state: Some("done".into()),
            ended: Some(1791397552),
        }));
        assert!(live(&AmxRow {
            id: "a-b".into(),
            state: None,
            ended: None,
        }));
    }

    #[test]
    fn the_listing_amx_prints_is_read_for_its_ids_and_states() {
        let text = r#"[
          {"id":"alpha-winners","name":null,"state":"working","model":"opus","dir":"/x",
           "task":"/goal ...","questions":[],"socket":{"name":"default"}},
          {"id":"old-run","name":"renamed","state":"stopped"}
        ]"#;
        let rows = parse_amx(text).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "alpha-winners");
        assert_eq!(rows[0].state.as_deref(), Some("working"));
        assert_eq!(rows[1].id, "old-run");
        assert_eq!(parse_amx("  \n").unwrap().len(), 0);
        assert!(parse_amx("not json").is_err());
    }

    #[test]
    fn the_next_milestone_starts_with_the_exact_amx_arguments() {
        let work = Scratch::new("go-start");
        let dir = work.mkdir("checkout");
        let projects = [row("alpha", Some("approved"), &[&dir])];

        let Decision::Start(start) = decide_quietly(&opts(Some("alpha"), None), &projects) else {
            panic!("not a start");
        };

        assert_eq!(start.name, "alpha-catalog");
        assert_eq!(
            start.args("alpha-catalog"),
            [
                "new",
                "--name",
                "alpha-catalog",
                "--model",
                "opus",
                "--effort",
                "high",
                "--permission",
                "bypassPermissions",
                "--no-worktree",
                "--dir",
                &dir.display().to_string(),
                &task("alpha", "catalog"),
            ]
        );
    }

    #[test]
    fn a_model_and_an_effort_replace_the_defaults() {
        let work = Scratch::new("go-dials");
        let projects = [row("alpha", Some("approved"), &[&work.mkdir("c")])];
        let dials = Options {
            project: Some("alpha"),
            milestone: None,
            model: "sonnet",
            effort: "max",
        };
        let Decision::Start(start) = decide_quietly(&dials, &projects) else {
            panic!("not a start");
        };
        let args = start.args(&start.name);
        assert_eq!(
            args[args.iter().position(|a| a == "--model").unwrap() + 1],
            "sonnet"
        );
        assert_eq!(
            args[args.iter().position(|a| a == "--effort").unwrap() + 1],
            "max"
        );
    }

    #[test]
    fn a_roadmap_that_is_not_approved_is_refused_before_anything_is_read() {
        let work = Scratch::new("go-status");
        let dir = work.mkdir("c");
        for status in [None, Some("draft"), Some("running"), Some("maintenance")] {
            let projects = [row("alpha", status, &[&dir])];
            let decision = decide(
                &opts(Some("alpha"), None),
                &projects,
                None,
                |_| panic!("the roadmap is not read"),
                || panic!("amx is not asked"),
                None,
            );
            let Decision::Refuse(why) = decision else {
                panic!("{status:?} was not refused");
            };
            assert!(why.contains("alpha: the roadmap is not approved"), "{why}");
            assert!(why.contains(status.unwrap_or("none")), "{why}");
        }
    }

    #[test]
    fn a_named_milestone_needs_neither_the_status_nor_the_roadmap() {
        let work = Scratch::new("go-named");
        let projects = [row("alpha", Some("draft"), &[&work.mkdir("c")])];
        let decision = decide(
            &opts(Some("alpha"), Some("orders")),
            &projects,
            None,
            |_| panic!("the roadmap is not read"),
            || Ok(Vec::new()),
            None,
        );
        let Decision::Start(start) = decision else {
            panic!("not a start");
        };
        assert_eq!(start.name, "alpha-orders");
    }

    #[test]
    fn a_roadmap_with_nothing_open_is_done() {
        let work = Scratch::new("go-done");
        let projects = [row("alpha", Some("approved"), &[&work.mkdir("c")])];
        let decision = decide(
            &opts(Some("alpha"), None),
            &projects,
            None,
            |_| Some("- [x] skeleton One\n- [x] winners Two\n".to_string()),
            || panic!("nothing is started, so amx is not asked"),
            None,
        );
        assert_eq!(decision, Decision::Done("alpha".to_string()));
    }

    #[test]
    fn a_live_orchestrator_of_the_project_is_refused() {
        let work = Scratch::new("go-live");
        let projects = [
            row("alpha", Some("approved"), &[&work.mkdir("c")]),
            row("alpha-mobile", Some("approved"), &[]),
        ];
        let ask = |rows: Vec<AmxRow>, me: Option<&str>| {
            decide(
                &opts(Some("alpha"), None),
                &projects,
                None,
                |_| Some(ROADMAP.to_string()),
                || Ok(rows),
                me,
            )
        };

        for state in ["working", "waiting", "idle", "done"] {
            let Decision::Refuse(why) = ask(vec![agent("alpha-catalog", state)], None) else {
                panic!("{state} did not refuse");
            };
            assert_eq!(
                why,
                "alpha already has an orchestrator running: alpha-catalog"
            );
        }
        // Ended, an earlier milestone's idling after it started this one,
        // someone else's, or the caller itself: none of them is in the way.
        for rows in [
            vec![agent("alpha-catalog", "stopped")],
            vec![agent("alpha-winners", "idle")],
            vec![
                agent("alpha-catalog", "stopped"),
                agent("alpha-catalog-2", "failed"),
            ],
            vec![agent("alpha-mobile-engage", "working")],
            vec![agent("beta-winners", "working")],
            vec![agent("a-scratch-agent", "working")],
        ] {
            assert!(
                matches!(ask(rows.clone(), None), Decision::Start(_)),
                "{rows:?}"
            );
        }
        let me = Some("alpha-catalog");
        assert!(matches!(
            ask(vec![agent("alpha-catalog", "working")], me),
            Decision::Start(_)
        ));
        // Only the caller is excused: a second one still refuses.
        assert!(matches!(
            ask(
                vec![
                    agent("alpha-catalog", "working"),
                    agent("alpha-catalog-2", "working")
                ],
                me
            ),
            Decision::Refuse(_)
        ));
    }

    #[test]
    fn an_unknown_project_or_one_with_no_checkout_here_is_refused() {
        let work = Scratch::new("go-where");
        let projects = [
            row("alpha", Some("approved"), &[&work.mkdir("c")]),
            row("elsewhere", Some("approved"), &[]),
            row("gone", Some("approved"), &[&work.at("not-there")]),
        ];
        for (name, words) in [
            ("nobody", "no project named nobody"),
            ("elsewhere", "elsewhere has no checkout on this machine"),
            ("gone", "gone has no checkout on this machine"),
        ] {
            let Decision::Refuse(why) = decide_quietly(&opts(Some(name), None), &projects) else {
                panic!("{name} was not refused");
            };
            assert!(why.contains(words), "{why}");
        }
    }

    #[test]
    fn the_first_listed_checkout_that_is_a_directory_is_the_one() {
        let work = Scratch::new("go-first");
        let real = work.mkdir("real");
        let projects = [row(
            "alpha",
            Some("approved"),
            &[&work.at("missing"), &real, &work.mkdir("other")],
        )];
        let Decision::Start(start) = decide_quietly(&opts(Some("alpha"), None), &projects) else {
            panic!("not a start");
        };
        let args = start.args(&start.name);
        assert_eq!(
            args[args.iter().position(|a| a == "--dir").unwrap() + 1],
            real.display().to_string()
        );
    }

    #[test]
    fn a_named_project_runs_in_the_checkout_the_caller_stands_in() {
        // An orchestrator starts its successor with `workflow go <project>`
        // from its own checkout; mem lists checkouts sorted by path, so the
        // first listed one can be another clone entirely.
        let work = Scratch::new("go-named-here");
        let first = work.mkdir("a-first");
        let here = work.mkdir("b-here");
        let projects = [row("alpha", Some("approved"), &[&first, &here])];
        let current = Project {
            id: "01X".into(),
            name: "alpha".into(),
            root: Some(here.display().to_string()),
        };

        let decision = decide(
            &opts(Some("alpha"), None),
            &projects,
            Some(current),
            |_| Some(ROADMAP.to_string()),
            || Ok(Vec::new()),
            None,
        );
        let Decision::Start(start) = decision else {
            panic!("not a start");
        };
        let args = start.args(&start.name);
        assert_eq!(
            args[args.iter().position(|a| a == "--dir").unwrap() + 1],
            here.display().to_string()
        );

        // Standing in another project's checkout changes nothing.
        let elsewhere = Project {
            id: "01Y".into(),
            name: "beta".into(),
            root: Some(here.display().to_string()),
        };
        let decision = decide(
            &opts(Some("alpha"), None),
            &projects,
            Some(elsewhere),
            |_| Some(ROADMAP.to_string()),
            || Ok(Vec::new()),
            None,
        );
        let Decision::Start(start) = decision else {
            panic!("not a start");
        };
        let args = start.args(&start.name);
        assert_eq!(
            args[args.iter().position(|a| a == "--dir").unwrap() + 1],
            first.display().to_string()
        );
    }

    #[test]
    fn with_no_project_named_the_current_directory_decides() {
        let work = Scratch::new("go-here");
        let listed = work.mkdir("listed");
        let here = work.mkdir("here");
        let projects = [row("alpha", Some("approved"), &[&listed])];
        let current = Project {
            id: "01X".into(),
            name: "alpha".into(),
            root: Some(here.display().to_string()),
        };

        let decision = decide(
            &opts(None, None),
            &projects,
            Some(current),
            |_| Some(ROADMAP.to_string()),
            || Ok(Vec::new()),
            None,
        );
        let Decision::Start(start) = decision else {
            panic!("not a start");
        };
        let args = start.args(&start.name);
        assert_eq!(
            args[args.iter().position(|a| a == "--dir").unwrap() + 1],
            here.display().to_string(),
            "the checkout the caller stands in, not the first one listed"
        );

        let Decision::Refuse(why) = decide_quietly(&opts(None, None), &projects) else {
            panic!("no project and no directory should refuse");
        };
        assert!(why.contains("workflow go <project>"), "{why}");
    }

    #[test]
    fn what_mem_or_amx_cannot_answer_fails_rather_than_refuses() {
        let work = Scratch::new("go-fail");
        let projects = [row("alpha", Some("approved"), &[&work.mkdir("c")])];
        let no_roadmap = decide(
            &opts(Some("alpha"), None),
            &projects,
            None,
            |_| None,
            || Ok(Vec::new()),
            None,
        );
        assert!(matches!(no_roadmap, Decision::Fail(_)));
        let no_amx = decide(
            &opts(Some("alpha"), None),
            &projects,
            None,
            |_| Some(ROADMAP.to_string()),
            || Err("amx ls failed: no server".to_string()),
            None,
        );
        assert_eq!(
            no_amx,
            Decision::Fail("amx ls failed: no server".to_string())
        );
    }

    fn start_for_launch() -> Start {
        Start {
            name: "alpha-winners".into(),
            model: "opus".into(),
            effort: "high".into(),
            dir: "/work".into(),
            task: "/goal x".into(),
        }
    }

    fn taken() -> Spawned {
        Spawned {
            ok: false,
            stderr: "amx: name \"x\" is already taken\n".into(),
        }
    }

    #[test]
    fn a_name_amx_has_taken_is_tried_again_with_a_suffix() {
        let tried = std::cell::RefCell::new(Vec::new());
        let name = launch(&start_for_launch(), |args| {
            tried.borrow_mut().push(args[2].clone());
            if tried.borrow().len() < 3 {
                taken()
            } else {
                Spawned {
                    ok: true,
                    stderr: String::new(),
                }
            }
        });
        assert_eq!(name.as_deref(), Ok("alpha-winners-3"));
        assert_eq!(
            *tried.borrow(),
            ["alpha-winners", "alpha-winners-2", "alpha-winners-3"]
        );
    }

    #[test]
    fn any_other_refusal_stops_at_once_with_amxs_words() {
        let calls = Cell::new(0);
        let out = launch(&start_for_launch(), |_| {
            calls.set(calls.get() + 1);
            Spawned {
                ok: false,
                stderr: "amx: no model `opus` here\n".into(),
            }
        });
        assert_eq!(out, Err("amx: no model `opus` here".to_string()));
        assert_eq!(calls.get(), 1);

        let silent = launch(&start_for_launch(), |_| Spawned {
            ok: false,
            stderr: String::new(),
        });
        assert_eq!(silent, Err("amx new failed".to_string()));
    }

    #[test]
    fn a_name_that_stays_taken_gives_up() {
        let calls = Cell::new(0);
        let out = launch(&start_for_launch(), |_| {
            calls.set(calls.get() + 1);
            taken()
        });
        assert!(out.unwrap_err().contains("already taken"));
        assert_eq!(calls.get(), NAME_TRIES);
    }

    #[test]
    fn a_command_line_reads_back_as_the_same_words() {
        let args = [
            "new".to_string(),
            "--name".to_string(),
            "alpha-winners".to_string(),
            task("alpha", "winners"),
            "it's".to_string(),
            String::new(),
        ];
        let line = shell_line("amx", &args);
        assert!(
            line.starts_with("amx new --name alpha-winners '/goal Milestone winners"),
            "{line}"
        );

        // Read back by a shell, each word comes out as it went in.
        let echoed = Command::new("sh")
            .arg("-c")
            .arg(format!("printf '%s\\n' {}", &line["amx ".len()..]))
            .output()
            .unwrap();
        let words: Vec<String> = String::from_utf8(echoed.stdout)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        assert_eq!(words, args);
    }
}
