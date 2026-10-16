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

use crate::cloud;
use crate::memcli::{self, Project, ProjectRow};
use crate::{exit, machine, warn};

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
    /// The folder it runs in.
    #[serde(default)]
    pub dir: Option<String>,
    /// What it was started on; an orchestrator's is the goal [`task`] writes.
    #[serde(default)]
    pub task: Option<String>,
    /// What amx read its state from; `parked` for one that ended its own
    /// session and waits to be resumed.
    #[serde(default)]
    pub evidence: Option<String>,
}

/// What `go` has settled on.
#[derive(Debug, PartialEq)]
pub enum Decision {
    /// Every milestone is ticked.
    Done(String),
    /// The project is paused and its handoff is not parked: exit 2.
    Paused(String),
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
    pub project: String,
    /// The milestone it runs.
    pub slug: String,
    model: String,
    effort: String,
    pub dir: String,
    task: String,
    /// The project's recorded worktree, the one folder that may work on a
    /// branch other than main.
    pub worktree: Option<String>,
}

impl Start {
    /// The `amx` arguments that start it under `name`. Claude runs it under
    /// Remote Control by the same name, so the Code tab lists it as amx does.
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
            "--",
            "--remote-control",
            name,
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
the project's test command passing on the trunk (the branch this checkout is on) after the last merge; \
the milestone review and the dogfood walk finished with their findings fixed or filed; \
`mem roadmap --tick {slug}` run; a `mem handoff` written; \
and `workflow go {project}` run for the next milestone, or its output saying the roadmap is done, not approved, or paused. \
If the milestone cannot continue without Saiful, done instead means a `mem ask` question filed, \
the handoff naming it, and the work parked. Stop after 6 hours: at that stop, \
write a handoff starting `parked: 6-hour stop, <where you stopped>` and end the session with `amx stop --force $AMX_ID`."
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
/// way, so it never offers a Resume for a milestone that is still running;
/// the folder and trunk refusals it shows as its banner.
pub fn live(row: &AmxRow) -> bool {
    row.ended.unwrap_or(0) == 0 && !matches!(row.state.as_deref(), Some("stopped" | "failed"))
}

/// The id of another live agent running the orchestrator of `slug`. `me` is
/// the caller's own `$AMX_ID`: an orchestrator starting its successor is not
/// in its own way.
pub fn running(rows: &[AmxRow], project: &str, slug: &str, me: Option<&str>) -> Option<String> {
    rows.iter()
        .filter(|row| me != Some(row.id.as_str()))
        .filter(|row| live(row) && named_for(&row.id, project, slug))
        .map(|row| row.id.clone())
        .next()
}

/// The folder a project's orchestrator works in: the checkout mem read the
/// answer in, and the child's subdir under it. In the toplevel itself mem
/// would resolve the parent project instead.
fn folder(project: Project) -> Option<String> {
    let root = project.root?;
    Some(match project.subdir {
        Some(subdir) => Path::new(&root).join(subdir).display().to_string(),
        None => root,
    })
}

/// The project an agent's goal orchestrates: `task` starts it on "Milestone
/// <slug> of <project> is landed."
fn orchestrates(goal: &str) -> Option<&str> {
    let rest = goal.strip_prefix("/goal Milestone ")?;
    let (_, rest) = rest.split_once(" of ")?;
    rest.split_once(" is landed.").map(|(project, _)| project)
}

/// The git checkout a folder is in: the nearest folder holding a `.git`,
/// which a linked worktree has as a file. A folder in none is its own.
fn checkout(dir: &Path) -> &Path {
    dir.ancestors()
        .find(|up| up.join(".git").exists())
        .unwrap_or(dir)
}

/// A live orchestrator of another project in the checkout of `dir`, and that
/// checkout. Two in one checkout commit on one branch and break each other's
/// clean tree and fast-forward merges. This project's own agents are left to
/// [`running`]: the last milestone's idles there after starting this one.
fn crowding<'a>(rows: &'a [AmxRow], project: &str, dir: &str) -> Option<(&'a AmxRow, String)> {
    let here = checkout(Path::new(dir));
    rows.iter()
        .filter(|row| live(row))
        .filter(|row| {
            row.task
                .as_deref()
                .and_then(orchestrates)
                .is_some_and(|other| other != project)
        })
        .find(|row| {
            row.dir
                .as_deref()
                .is_some_and(|there| checkout(Path::new(there)) == here)
        })
        .map(|row| (row, here.display().to_string()))
}

/// Another project whose recorded worktree is the checkout of `dir`, when
/// this project's is not, and that checkout. Its branch is that project's
/// trunk, so building there, or switching it to main to get past
/// [`trunk_refusal`], takes the folder from under it, whether or not its
/// orchestrator is running.
fn claimed<'a>(projects: &'a [ProjectRow], project: &str, dir: &str) -> Option<(&'a str, String)> {
    let here = checkout(Path::new(dir));
    let recorded = |row: &ProjectRow| {
        row.worktree
            .as_deref()
            .is_some_and(|w| Path::new(w) == here)
    };
    if projects
        .iter()
        .any(|row| row.name == project && recorded(row))
    {
        return None;
    }
    projects
        .iter()
        .find(|row| row.name != project && recorded(row))
        .map(|row| (row.name.as_str(), here.display().to_string()))
}

pub fn parse_amx(text: &str) -> Result<Vec<AmxRow>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(text).map_err(|err| format!("amx ls --json is not a list: {err}"))
}

/// Settle what to do. `current` is what mem says of the working directory,
/// `roadmap` reads a project's roadmap, `handoff` its handoff and `agents`
/// lists amx's agents; the last three run only if the answer needs them.
pub fn decide(
    opts: &Options,
    projects: &[ProjectRow],
    current: Option<Project>,
    roadmap: impl FnOnce(&str) -> Option<String>,
    handoff: impl FnOnce(&str) -> Option<String>,
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
            let here = current.filter(|c| c.name == name).and_then(folder);
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
            let name = here.name.clone();
            let Some(dir) = folder(here).or_else(|| listed.cloned()) else {
                return Decision::Refuse(format!("{name} has no checkout on this machine"));
            };
            (name, dir)
        }
        (None, None) => {
            return Decision::Refuse(
                "this directory is in no project mem knows; name one: workflow go <project>".into(),
            );
        }
    };

    // While paused nothing new starts; a milestone parked before the pause
    // may carry on, with or without --milestone.
    let paused = projects
        .iter()
        .any(|row| row.name == project && row.paused.is_some());
    if paused && !handoff(&project).is_some_and(|text| text.trim_start().starts_with("parked:")) {
        return Decision::Paused(project);
    }

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

    if let Some((other, there)) = claimed(projects, &project, &dir) {
        return Decision::Refuse(format!(
            "{there} is the worktree recorded for {other}; give {project} a folder of its own \
(`git worktree add -b <branch> <folder> main`, then `mem project worktree .` there), \
and never switch this folder's branch to get past this"
        ));
    }

    match agents() {
        Err(why) => return Decision::Fail(why),
        Ok(rows) => {
            if let Some(id) = running(&rows, &project, &slug, me) {
                return Decision::Refuse(format!(
                    "{project} already has an orchestrator running: {id}"
                ));
            }
            if let Some((row, there)) = crowding(&rows, &project, &dir) {
                let other = row.task.as_deref().and_then(orchestrates).unwrap_or("");
                return Decision::Refuse(format!(
                    "{}, the orchestrator of {other}, already works in {there}; \
one orchestrator per folder, so give {project} a worktree of its own (mem project worktree)",
                    row.id
                ));
            }
        }
    }

    Decision::Start(Start {
        name: amx_name(&format!("{project}-{slug}")),
        project: project.clone(),
        slug: slug.clone(),
        model: opts.model.to_string(),
        effort: opts.effort.to_string(),
        task: task(&project, &slug),
        worktree: projects
            .iter()
            .find(|row| row.name == project)
            .and_then(|row| row.worktree.clone()),
        dir,
    })
}

/// Why `dir` cannot hold a milestone's trunk, if it cannot. A plain checkout
/// stays on main, so an orchestrator started on a stray feature branch stops
/// instead of building there; only the worktree recorded for the project
/// (`mem project worktree`) works on an app's own branch.
pub fn trunk_refusal(dir: &str, branch: Option<&str>, worktree: Option<&str>) -> Option<String> {
    let recorded = worktree.is_some_and(|w| checkout(Path::new(dir)) == Path::new(w));
    match branch {
        Some("main") => None,
        Some(_) if recorded => None,
        Some(other) => Some(format!(
            "{dir} is on {other}, not main; only the worktree recorded with \
`mem project worktree` works on a branch of its own. Record this folder if it \
is the project's, or give the project a worktree of its own; never switch a \
folder's branch to get past this"
        )),
        None => Some(format!("{dir} has no branch checked out")),
    }
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

pub fn amx_bin() -> String {
    match std::env::var("WORKFLOW_AMX") {
        Ok(v) if !v.is_empty() => v,
        _ => "amx".to_string(),
    }
}

pub fn amx_agents() -> Result<Vec<AmxRow>, String> {
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

/// Fast-forward `dir`'s trunk to origin's `branch`, the work a cloud run
/// pushed. A branch origin lacks has nothing to bring, unless the run
/// `landed`, whose work is only there; a trunk with commits the branch
/// lacks is refused, never merged.
fn fast_forward(dir: &str, branch: &str, landed: bool) -> Result<(), String> {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(["-C", dir])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|err| format!("cannot run git: {err}"))
    };
    let listed = git(&["ls-remote", "--exit-code", "--heads", "origin", branch])?;
    if listed.status.code() == Some(2) {
        return if landed {
            Err(format!(
                "origin has no {branch}, though its milestone is ticked"
            ))
        } else {
            Ok(())
        };
    }
    let reason =
        |out: &std::process::Output| String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !listed.status.success() {
        return Err(reason(&listed));
    }
    let fetched = git(&["fetch", "-q", "origin", branch])?;
    if !fetched.status.success() {
        return Err(reason(&fetched));
    }
    let merged = git(&["merge", "-q", "--ff-only", "FETCH_HEAD"])?;
    if !merged.status.success() {
        return Err(reason(&merged));
    }
    Ok(())
}

/// The branch checked out in `dir`; none when HEAD is detached or `dir` is
/// no checkout.
fn branch_of(dir: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["-C", dir, "branch", "--show-current"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !name.is_empty()).then_some(name)
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
        memcli::handoff,
        amx_agents,
        me.as_deref(),
    ) {
        Decision::Done(project) => {
            // The last milestone may have landed in the cloud: its work is
            // still on its branch, and the trunk takes it now.
            let row = projects.iter().find(|row| row.name == project);
            let landed = row.and_then(|row| {
                let run = row.runner.as_ref().filter(|r| r.machine == "cloud")?;
                let dir = row.checkouts.iter().find(|dir| Path::new(dir).is_dir())?;
                Some((cloud::branch(&project, run.milestone.as_deref()?), dir))
            });
            if let Some((from, dir)) = landed.filter(|_| !dry_run) {
                let branch = branch_of(dir);
                let worktree = row.and_then(|row| row.worktree.as_deref());
                if let Some(why) = trunk_refusal(dir, branch.as_deref(), worktree) {
                    warn(format!("go: {why}"));
                    return exit::USAGE;
                }
                if let Err(why) = fast_forward(dir, &from, true) {
                    warn(format!(
                        "go: cannot fast-forward {} to {from}: {why}",
                        branch.as_deref().unwrap_or("the trunk")
                    ));
                    return exit::USAGE;
                }
                if !memcli::unset_runner(&project) {
                    warn(format!("go: mem would not clear {project}'s cloud runner"));
                }
            }
            println!("{project}: the roadmap is done");
            exit::OK
        }
        Decision::Paused(project) => {
            println!("{project}: paused");
            exit::USAGE
        }
        Decision::Refuse(why) => {
            warn(format!("go: {why}"));
            exit::USAGE
        }
        Decision::Fail(why) => {
            warn(format!("go: {why}"));
            exit::FAILED
        }
        Decision::Start(start) => {
            let machine = machine::name();
            let row = projects.iter().find(|row| row.name == start.project);
            let mut runner = row.and_then(|row| row.runner.as_ref());
            // A cloud run that still holds its milestone keeps the mini out;
            // one that is over left its work on its branch.
            let cloud = row.and_then(|row| {
                let state =
                    cloud::state(row, || memcli::handoff(&row.name), jiff::Timestamp::now());
                Some((state?, row.runner.as_ref()?))
            });
            if let Some((cloud::State::Live | cloud::State::Parked, run)) = cloud {
                warn(format!(
                    "go: {} runs in the cloud ({}, session {}); answer it on the hub, or wait for it to land",
                    start.project,
                    run.milestone.as_deref().unwrap_or("?"),
                    run.id
                ));
                return exit::USAGE;
            }
            let landed_from = cloud.and_then(|(_, run)| run.milestone.clone());
            if landed_from.is_some() {
                runner = None;
            }
            if let Some(why) = runner_refusal(&start.project, runner, machine.as_deref(), dry_run) {
                warn(format!("go: {why}"));
                return exit::USAGE;
            }
            let branch = branch_of(&start.dir);
            if let Some(why) =
                trunk_refusal(&start.dir, branch.as_deref(), start.worktree.as_deref())
            {
                warn(format!("go: {why}"));
                return exit::USAGE;
            }
            if let Some(slug) = landed_from.filter(|_| !dry_run) {
                let from = cloud::branch(&start.project, &slug);
                let landed = matches!(cloud, Some((cloud::State::Landed, _)));
                if let Err(why) = fast_forward(&start.dir, &from, landed) {
                    warn(format!(
                        "go: cannot fast-forward {} to {from}: {why}",
                        branch.as_deref().unwrap_or("the trunk")
                    ));
                    return exit::USAGE;
                }
                if !memcli::unset_runner(&start.project) {
                    warn(format!(
                        "go: mem would not clear {}'s cloud runner",
                        start.project
                    ));
                }
            }
            started(&start, dry_run, machine.as_deref())
        }
    }
}

/// Why this machine may not start `project`, if it may not: another machine
/// runs it, live or not, or this machine's name cannot be told. A dry run
/// starts nothing, so it only needs the name when a runner is to be compared.
fn runner_refusal(
    project: &str,
    runner: Option<&memcli::Runner>,
    machine: Option<&str>,
    dry_run: bool,
) -> Option<String> {
    let unknown = "cannot tell this machine's tailnet name; set WORKFLOW_MACHINE";
    match (runner, machine) {
        (Some(runner), Some(here)) if runner.machine != here => Some(format!(
            "{project} runs on {} ({}); start it there, or move it with \
`mem --project {project} project unset runner`",
            runner.machine, runner.id
        )),
        (Some(_), None) => Some(unknown.to_string()),
        (None, None) if !dry_run => Some(unknown.to_string()),
        _ => None,
    }
}

/// Print or start the orchestrator `go` settled on, and record it as the
/// project's runner.
fn started(start: &Start, dry_run: bool, machine: Option<&str>) -> i32 {
    if dry_run {
        println!("{}", shell_line(&amx_bin(), &start.args(&start.name)));
        return exit::OK;
    }
    match launch(start, amx_new) {
        Ok(name) => {
            if let Some(machine) = machine
                && !memcli::set_runner(&start.project, machine, &name)
            {
                warn(format!(
                    "go: started {name}, but mem would not record {machine} as the runner of {}",
                    start.project
                ));
            }
            println!("{name}");
            exit::OK
        }
        Err(why) => {
            warn(format!("go: {why}"));
            exit::FAILED
        }
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
            worktree: None,
            paused: None,
            runner: None,
            remote: None,
            milestone: None,
            handoff_at: None,
        }
    }

    fn agent(id: &str, state: &str) -> AmxRow {
        AmxRow {
            id: id.to_string(),
            state: Some(state.to_string()),
            ended: Some(0),
            dir: None,
            task: None,
            evidence: None,
        }
    }

    /// The orchestrator `workflow go` starts for `slug` of `project`, in `dir`.
    fn orchestrator(project: &str, slug: &str, state: &str, dir: &Path) -> AmxRow {
        AmxRow {
            dir: Some(dir.display().to_string()),
            task: Some(task(project, slug)),
            evidence: None,
            ..agent(&amx_name(&format!("{project}-{slug}")), state)
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
            |_| panic!("the handoff is not read"),
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
the project's test command passing on the trunk (the branch this checkout is on) after the last merge; \
the milestone review and the dogfood walk finished with their findings fixed or filed; \
`mem roadmap --tick winners` run; a `mem handoff` written; \
and `workflow go alpha` run for the next milestone, or its output saying the roadmap is done, not approved, or paused. \
If the milestone cannot continue without Saiful, done instead means a `mem ask` question filed, \
the handoff naming it, and the work parked. Stop after 6 hours: at that stop, \
write a handoff starting `parked: 6-hour stop, <where you stopped>` and end the session with `amx stop --force $AMX_ID`.";
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
            dir: None,
            task: None,
            evidence: None,
        }));
        assert!(live(&AmxRow {
            id: "a-b".into(),
            state: None,
            ended: None,
            dir: None,
            task: None,
            evidence: None,
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
        assert_eq!(rows[0].dir.as_deref(), Some("/x"));
        assert_eq!(rows[0].task.as_deref(), Some("/goal ..."));
        assert_eq!(rows[1].dir, None);
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
                "--",
                "--remote-control",
                "alpha-catalog",
            ]
        );
    }

    #[test]
    fn the_code_tab_lists_an_orchestrator_under_the_name_amx_gave_it() {
        let work = Scratch::new("go-rc");
        let projects = [row("shopify_apps", Some("approved"), &[&work.mkdir("c")])];
        let Decision::Start(start) =
            decide_quietly(&opts(Some("shopify_apps"), Some("m5-checkout")), &projects)
        else {
            panic!("not a start");
        };

        assert!(start.args(&start.name).ends_with(
            &["--", "--remote-control", "shopify-apps-m5-checkout"].map(str::to_string)
        ));
        assert!(start.args("shopify-apps-m5-checkout-2").ends_with(
            &["--", "--remote-control", "shopify-apps-m5-checkout-2"].map(str::to_string)
        ));
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
                |_| panic!("the handoff is not read"),
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
            |_| panic!("the handoff is not read"),
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
            |_| panic!("the handoff is not read"),
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
                |_| panic!("the handoff is not read"),
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
    fn a_trunk_is_main_or_the_branch_of_the_recorded_worktree() {
        let work = Scratch::new("go-trunk");
        let plain = work.mkdir("plain");
        work.mkdir("plain/.git");
        let linked = work.mkdir("linked/apps/alpha");
        work.put("linked/.git", "gitdir: x\n");
        let worktree = work.at("linked").display().to_string();
        let at = |dir: &Path| dir.display().to_string();

        assert_eq!(trunk_refusal(&at(&plain), Some("main"), None), None);
        assert_eq!(
            trunk_refusal(&at(&linked), Some("app/alpha"), Some(&worktree)),
            None
        );
        assert_eq!(
            trunk_refusal(&at(&linked), Some("main"), Some(&worktree)),
            None
        );
        assert_eq!(
            trunk_refusal(&at(&plain), Some("feature"), None),
            Some(format!(
                "{} is on feature, not main; only the worktree recorded with \
`mem project worktree` works on a branch of its own. Record this folder if it \
is the project's, or give the project a worktree of its own; never switch a \
folder's branch to get past this",
                at(&plain)
            ))
        );
        // Recorded for this project, but the folder is another checkout.
        assert!(trunk_refusal(&at(&plain), Some("feature"), Some(&worktree)).is_some());
        assert_eq!(
            trunk_refusal(&at(&linked), None, Some(&worktree)),
            Some(format!("{} has no branch checked out", at(&linked)))
        );
    }

    #[test]
    fn a_goal_names_the_project_it_orchestrates() {
        assert_eq!(orchestrates(&task("alpha", "winners")), Some("alpha"));
        assert_eq!(
            orchestrates(&task("shopify_apps", "m1-a")),
            Some("shopify_apps")
        );
        assert_eq!(orchestrates("fix the build of alpha"), None);
        assert_eq!(orchestrates("/goal Milestone winners is landed."), None);
    }

    #[test]
    fn another_projects_orchestrator_in_the_same_checkout_is_refused() {
        // One monorepo with two child projects, a linked worktree of it, and
        // a clone whose name starts with the monorepo's.
        let work = Scratch::new("go-folder");
        let repo = work.mkdir("repo");
        work.mkdir("repo/.git");
        let alpha = work.mkdir("repo/apps/alpha");
        let beta = work.mkdir("repo/apps/beta");
        let linked = work.mkdir("repo-linked/apps/beta");
        work.put(
            "repo-linked/.git",
            "gitdir: ../repo/.git/worktrees/linked\n",
        );
        let lookalike = work.mkdir("repo-b");
        work.mkdir("repo-b/.git");
        let projects = [
            row("alpha", Some("approved"), &[&alpha]),
            row("beta", Some("approved"), &[&beta]),
        ];
        let ask = |rows: Vec<AmxRow>| {
            decide(
                &opts(Some("alpha"), None),
                &projects,
                None,
                |_| Some(ROADMAP.to_string()),
                |_| panic!("the handoff is not read"),
                || Ok(rows),
                None,
            )
        };

        for dir in [&beta, &repo, &alpha] {
            let Decision::Refuse(why) = ask(vec![orchestrator("beta", "orders", "working", dir)])
            else {
                panic!("beta in {} did not refuse", dir.display());
            };
            assert_eq!(
                why,
                format!(
                    "beta-orders, the orchestrator of beta, already works in {}; \
one orchestrator per folder, so give alpha a worktree of its own (mem project worktree)",
                    repo.display()
                )
            );
        }
        // Another checkout, an ended run, this project's own earlier
        // milestone idling after it started this one, or an agent that is no
        // orchestrator: none of them is in the way.
        for rows in [
            vec![orchestrator("beta", "orders", "working", &linked)],
            vec![orchestrator("beta", "orders", "working", &lookalike)],
            vec![orchestrator("beta", "orders", "stopped", &repo)],
            vec![orchestrator("alpha", "winners", "idle", &repo)],
            vec![AmxRow {
                dir: Some(repo.display().to_string()),
                task: Some("look around beta".into()),
                evidence: None,
                ..agent("plan-beta", "idle")
            }],
        ] {
            assert!(matches!(ask(rows.clone()), Decision::Start(_)), "{rows:?}");
        }
    }

    #[test]
    fn a_folder_recorded_for_another_project_is_refused() {
        // beta works in the main checkout of the monorepo; alpha has no
        // folder of its own yet, so it would land there too.
        let work = Scratch::new("go-claimed");
        let repo = work.mkdir("repo");
        work.mkdir("repo/.git");
        let alpha = work.mkdir("repo/apps/alpha");
        let beta = work.mkdir("repo/apps/beta");
        let linked = work.mkdir("repo-alpha/apps/alpha");
        work.put("repo-alpha/.git", "gitdir: ../repo/.git/worktrees/alpha\n");
        let at = |dir: &Path| dir.display().to_string();
        let recorded = |mut row: ProjectRow, dir: &Path| {
            row.worktree = Some(at(dir));
            row
        };

        let projects = [
            row("shop", Some("approved"), &[&repo]),
            row("alpha", Some("approved"), &[&alpha]),
            recorded(row("beta", Some("approved"), &[&beta]), &repo),
        ];
        for name in ["alpha", "shop"] {
            let Decision::Refuse(why) = decide_quietly(&opts(Some(name), None), &projects) else {
                panic!("{name} was not refused");
            };
            assert_eq!(
                why,
                format!(
                    "{} is the worktree recorded for beta; give {name} a folder of its own \
(`git worktree add -b <branch> <folder> main`, then `mem project worktree .` there), \
and never switch this folder's branch to get past this",
                    at(&repo)
                )
            );
        }
        assert!(matches!(
            decide_quietly(&opts(Some("beta"), None), &projects),
            Decision::Start(_)
        ));

        // alpha in its own worktree, or recorded in the same folder as beta
        // on purpose: neither is in beta's way.
        let own = [
            recorded(
                row("alpha", Some("approved"), &[&linked]),
                &work.at("repo-alpha"),
            ),
            recorded(row("beta", Some("approved"), &[&beta]), &repo),
        ];
        let shared = [
            recorded(row("alpha", Some("approved"), &[&alpha]), &repo),
            recorded(row("beta", Some("approved"), &[&beta]), &repo),
        ];
        for projects in [&own, &shared] {
            assert!(matches!(
                decide_quietly(&opts(Some("alpha"), None), projects),
                Decision::Start(_)
            ));
        }
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
            subdir: None,
        };

        let decision = decide(
            &opts(Some("alpha"), None),
            &projects,
            Some(current),
            |_| Some(ROADMAP.to_string()),
            |_| panic!("the handoff is not read"),
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
            subdir: None,
        };
        let decision = decide(
            &opts(Some("alpha"), None),
            &projects,
            Some(elsewhere),
            |_| Some(ROADMAP.to_string()),
            |_| panic!("the handoff is not read"),
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
    fn a_child_project_runs_in_its_own_subdir_of_the_checkout() {
        // mem names the checkout's toplevel as the root; in the toplevel mem
        // resolves the parent project, so the child's orchestrator runs in
        // its subdir, the folder mem lists for it.
        let work = Scratch::new("go-child");
        let here = work.mkdir("here");
        let projects = [row("alpha", Some("approved"), &[&work.mkdir("listed")])];
        for named in [Some("alpha"), None] {
            let current = Project {
                id: "01X".into(),
                name: "alpha".into(),
                root: Some(here.display().to_string()),
                subdir: Some("apps/alpha".into()),
            };
            let decision = decide(
                &opts(named, None),
                &projects,
                Some(current),
                |_| Some(ROADMAP.to_string()),
                |_| panic!("the handoff is not read"),
                || Ok(Vec::new()),
                None,
            );
            let Decision::Start(start) = decision else {
                panic!("not a start");
            };
            let args = start.args(&start.name);
            assert_eq!(
                args[args.iter().position(|a| a == "--dir").unwrap() + 1],
                here.join("apps/alpha").display().to_string(),
                "{named:?}"
            );
        }
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
            subdir: None,
        };

        let decision = decide(
            &opts(None, None),
            &projects,
            Some(current),
            |_| Some(ROADMAP.to_string()),
            |_| panic!("the handoff is not read"),
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
            |_| panic!("the handoff is not read"),
            || Ok(Vec::new()),
            None,
        );
        assert!(matches!(no_roadmap, Decision::Fail(_)));
        let no_amx = decide(
            &opts(Some("alpha"), None),
            &projects,
            None,
            |_| Some(ROADMAP.to_string()),
            |_| panic!("the handoff is not read"),
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
            project: "alpha".into(),
            slug: "m1".to_string(),
            model: "opus".into(),
            effort: "high".into(),
            dir: "/work".into(),
            task: "/goal x".into(),
            worktree: None,
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
