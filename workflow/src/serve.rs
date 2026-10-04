//! `workflow serve` -- every roadmap on this machine, run milestone after
//! milestone with nobody watching.
//!
//! Serve does none of a run's work itself. Each project it holds gets one
//! child `workflow run` in its checkout: a run pins its project, its working
//! directory, its lock and its signals to the process it is in, and a child
//! keeps every adopt, settle, land and tick path as the suite drives it.
//! Serve's part is the tick: which projects are this machine's, which
//! milestone is open, whether a run may start, and what follows when one ends.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};

use serde::Deserialize;

use crate::backend::{Dispatch, Handle, WorkerBackend, backend_for};
use crate::brief::{self, LeadCtx};
use crate::dogfood::{self, NO_SHOW_PATH, OwnerFinding, WalkBrief, WalkOutcome, WalkState};
use crate::gitcmd::{self, Git};
use crate::plan::{self, PlanKind};
use crate::{exit, memcli, ownership, paths, plancheck, run, sys, warn};

/// Seconds between ticks when neither `--tick` nor `WORKFLOW_TICK_S` says.
const TICK_S: f64 = 5.0;

/// Workers a child run holds at once when the project sets no `slots`.
const SLOTS: u64 = 2;

/// How old serve lets its own claim on a project grow before stamping it
/// again: another machine reads a claim as abandoned at an hour.
const RECLAIM_S: i64 = 30 * 60;

/// How long serve waits on a pickup lead before it runs plan-check anyway.
const PICKUP_S: i64 = 20 * 60;

/// How long a lead the backend has no record of yet counts as launching
/// rather than gone: the process seam writes its pidfile after dispatch
/// returns.
const LAUNCH_S: i64 = 60;

/// How long serve waits on each child run to stop its workers once it has
/// passed a SIGTERM on, before it goes without it.
const STOP_S: f64 = 30.0;

/// The one lead a project has at a time writes its pid here.
const LEAD_PID: &str = "lead.pid";

/// The walk a project has going, a [`WalkState`] line.
const WALK: &str = "walk";

/// The sections of a project's `verify` page a walk's brief carries.
const VERIFY_SECTIONS: [&str; 5] = ["launch", "doctor", "drive", "evidence", "cleanup"];

/// The project whose wiki holds `dogfood-playbooks`, the per-surface how-to
/// every project's walk reads.
const WORKFLOW: &str = "workflow";

/// Written when a pause told the project's run to stop, so the stop is sent
/// once and the run's end reads as a stop rather than as stopping short.
const STOPPED: &str = "stopped";

/// The roadmap statuses serve works under. A draft is not agreed yet, and a
/// roadmap marked done is nobody's to run.
const WORKED: [&str; 3] = ["approved", "running", "maintenance"];

/// One project as a tick sees it.
pub struct ServeProject {
    pub name: String,
    /// The first checkout mem lists for it that is a directory here.
    pub root: PathBuf,
    pub runner: Option<String>,
    pub paused: bool,
    pub slots: u64,
    pub roadmap_status: Option<String>,
    /// The roadmap's milestones in order, each with whether it is ticked.
    pub milestones: Vec<(String, bool)>,
}

impl ServeProject {
    /// The project's directory under the serve root, one path component the
    /// way the run and worktree roots name it.
    fn dir(&self) -> PathBuf {
        paths::serve_root().join(self.name.replace('/', "-"))
    }

    fn run_dir(&self, slug: &str) -> PathBuf {
        paths::runs_root()
            .join(self.name.replace('/', "-"))
            .join(slug)
    }

    fn worth_a_tick(&self) -> bool {
        self.roadmap_status
            .as_deref()
            .is_some_and(|s| WORKED.contains(&s))
            && !self.milestones.is_empty()
    }
}

/// What one mem call printed, and whether it exited 0.
struct Said {
    ok: bool,
    out: String,
    err: String,
}

/// One mem call about one project, made from anywhere: `--project` names
/// it, since serve stands in no checkout and `MEM_PROJECT` would name one
/// project for all of them.
fn mem_on(mem: &Path, project: &str, args: &[&str]) -> Said {
    let out = Command::new(mem)
        .arg("--project")
        .arg(project)
        .args(args)
        .env_remove("MEM_PROJECT")
        .stdin(Stdio::null())
        .output();
    match out {
        Ok(o) => Said {
            ok: o.status.success(),
            out: String::from_utf8_lossy(&o.stdout).to_string(),
            err: String::from_utf8_lossy(&o.stderr).trim().to_string(),
        },
        Err(e) => Said {
            ok: false,
            out: String::new(),
            err: format!("cannot run {}: {e}", mem.display()),
        },
    }
}

#[derive(Deserialize)]
struct Listed {
    name: String,
    #[serde(default)]
    checkouts: Vec<String>,
}

#[derive(Deserialize)]
struct Listing {
    projects: Vec<Listed>,
}

/// The keys of `mem project current --json` a tick reads.
#[derive(Deserialize, Default)]
struct Current {
    #[serde(default)]
    machine: Option<String>,
    #[serde(default)]
    runner: Option<String>,
    #[serde(default)]
    runner_since: Option<String>,
    #[serde(default)]
    paused: Option<String>,
    #[serde(default)]
    slots: Option<u64>,
    #[serde(default)]
    dev: Option<String>,
    #[serde(default)]
    preview: Option<String>,
}

#[derive(Deserialize, Default)]
struct Roadmap {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct Items {
    items: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct Questions {
    questions: Vec<memcli::Question>,
}

fn listing(mem: &Path) -> Vec<Listed> {
    Command::new(mem)
        .args(["projects", "--json"])
        .env_remove("MEM_PROJECT")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .and_then(|o| serde_json::from_slice::<Listing>(&o.stdout).ok())
        .map(|l| l.projects)
        .unwrap_or_default()
}

fn current(mem: &Path, project: &str) -> Option<Current> {
    let said = mem_on(mem, project, &["project", "current", "--json"]);
    if !said.ok {
        return None;
    }
    serde_json::from_str(&said.out).ok()
}

fn roadmap(mem: &Path, project: &str) -> Roadmap {
    serde_json::from_str(&mem_on(mem, project, &["roadmap", "--json"]).out).unwrap_or_default()
}

/// This machine as mem names it. Every project's record carries the same
/// name, so the first one mem lists answers for all of them.
fn machine(mem: &Path) -> Option<String> {
    let first = listing(mem).into_iter().next()?;
    current(mem, &first.name)?.machine
}

/// A roadmap's milestones in order with their ticks; nothing for a text that
/// is not a roadmap.
fn milestones(text: &str) -> Vec<(String, bool)> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    match plan::parse(text, false) {
        Some(p) if p.kind == PlanKind::Roadmap => {
            p.tasks.into_iter().map(|t| (t.id, t.checked)).collect()
        }
        _ => Vec::new(),
    }
}

/// The open milestone of a project's roadmap with its place, `(slug, n,
/// m)`: nothing when there is no roadmap or every milestone is ticked.
pub(crate) fn roadmap_place(project: &str) -> Option<(String, usize, usize)> {
    let road = milestones(&roadmap(Path::new(&memcli::bin()), project).text);
    let at = road.iter().position(|(_, ticked)| !ticked)?;
    Some((road[at].0.clone(), at + 1, road.len()))
}

/// The project's open findings, counted; none when mem cannot say.
pub(crate) fn open_findings(project: &str) -> usize {
    let said = mem_on(
        Path::new(&memcli::bin()),
        project,
        &["finding", "list", "--open", "--json"],
    );
    serde_json::from_str::<Items>(&said.out).map_or(0, |i| i.items.len())
}

/// The word serve last wrote in a project's stage file.
pub(crate) fn stage_of(project: &str) -> Option<String> {
    let path = paths::serve_root()
        .join(project.replace('/', "-"))
        .join("stage");
    let word = std::fs::read_to_string(path).ok()?.trim().to_string();
    (!word.is_empty()).then_some(word)
}

/// Another machine's claim that no longer holds: stamped over an hour ago,
/// with no run line logged in the last hour, the rule a run applies before
/// it takes a claim over.
fn claim_stale(mem: &Path, project: &str, since: Option<&str>, now: jiff::Timestamp) -> bool {
    let old = since
        .and_then(|s| s.parse::<jiff::Timestamp>().ok())
        .is_none_or(|t| now.duration_since(t) >= jiff::SignedDuration::from_hours(1));
    old && !logged_lately(mem, project)
}

fn logged_lately(mem: &Path, project: &str) -> bool {
    let said = mem_on(
        mem,
        project,
        &[
            "log", "--kind", "log", "--type", "run", "--since", "1h", "--limit", "1", "--json",
        ],
    );
    serde_json::from_str::<Items>(&said.out).is_ok_and(|i| !i.items.is_empty())
}

/// Every project with a checkout on this machine that is this machine's to
/// run: its runner names `machine`, or nobody, or a machine whose claim went
/// stale. A project another machine runs is left out.
pub fn scan_projects(mem: &Path, machine: &str) -> Vec<ServeProject> {
    let now = jiff::Timestamp::now();
    listing(mem)
        .into_iter()
        .filter_map(|listed| {
            let root = listed
                .checkouts
                .iter()
                .map(PathBuf::from)
                .find(|c| c.is_dir())?;
            let cur = current(mem, &listed.name)?;
            if let Some(runner) = cur.runner.as_deref()
                && runner != machine
                && !claim_stale(mem, &listed.name, cur.runner_since.as_deref(), now)
            {
                return None;
            }
            let road = roadmap(mem, &listed.name);
            // Parsed only under a status serve works, so a draft that does
            // not parse yet says nothing on every tick.
            let milestones = match road.status.as_deref() {
                Some(s) if WORKED.contains(&s) => milestones(&road.text),
                _ => Vec::new(),
            };
            Some(ServeProject {
                name: listed.name,
                root,
                runner: cur.runner,
                paused: cur.paused.is_some_and(|p| !p.trim().is_empty()),
                slots: cur.slots.unwrap_or(SLOTS),
                roadmap_status: road.status,
                milestones,
            })
        })
        .collect()
}

/// Whether a run wrote its own end at or after `since`: the `ended <n>
/// merged` event a run that finished its loop leaves. A run stopped by a
/// signal says `ended stopped by signal`, and one that crashed says nothing.
fn ended_since(events: &str, since: i64) -> bool {
    events.lines().any(|line| {
        let mut parts = line.splitn(3, ' ');
        let (Some(at), Some("ended"), Some(rest)) = (parts.next(), parts.next(), parts.next())
        else {
            return false;
        };
        rest.starts_with(|c: char| c.is_ascii_digit())
            && at
                .parse::<jiff::Timestamp>()
                .is_ok_and(|t| t.as_second() >= since)
    })
}

/// Every task the run dir holds a state for is merged, or was ticked before
/// this run: the milestone is done. A dir with no states at all is a run
/// that refused before it began, not one that finished.
fn all_merged(run_dir: &Path) -> bool {
    let states: Vec<String> = std::fs::read_dir(run_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "state"))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .collect();
    !states.is_empty()
        && states
            .iter()
            .all(|s| s == run::MERGED || s == run::DONE_PREVIOUSLY)
}

/// Whether a plan has tasks and every one of them is ticked.
fn every_task_ticked(text: &str) -> bool {
    plan::parse(text, false)
        .is_some_and(|p| !p.tasks.is_empty() && p.tasks.iter().all(|t| t.checked))
}

/// Why a run stopped, from what it said: its stopped-short line, else the
/// last thing it printed.
fn stop_reason(log: &str) -> String {
    log.lines()
        .rev()
        .find(|l| l.contains(" stopped short: "))
        .or_else(|| log.lines().rev().find(|l| !l.trim().is_empty()))
        .map(|l| l.trim().trim_start_matches("workflow: ").to_string())
        .unwrap_or_else(|| "the run ended without a word".to_string())
}

/// The requests a person or a lead leaves in a run dir for the next run to
/// honour, by file name.
fn markers(run_dir: &Path) -> Vec<String> {
    let mut found: Vec<String> = std::fs::read_dir(run_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| {
            [".redispatch", ".accept", ".regate"]
                .iter()
                .any(|x| n.ends_with(x))
        })
        .collect();
    found.sort();
    found
}

/// A lead session serve started for a project: one line of `<serve
/// dir>/leads`, `<kind> <session> <started>`.
#[derive(Debug, Clone, PartialEq)]
pub struct Lead {
    pub kind: String,
    pub session: String,
    pub started: i64,
}

fn read_leads(text: &str) -> Vec<Lead> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            Some(Lead {
                kind: parts.next()?.to_string(),
                session: parts.next()?.to_string(),
                started: parts.next()?.parse().ok()?,
            })
        })
        .collect()
}

fn write_leads(leads: &[Lead]) -> String {
    leads
        .iter()
        .map(|l| format!("{} {} {}\n", l.kind, l.session, l.started))
        .collect()
}

/// An event line of the run that wants a lead.
#[derive(Debug, PartialEq)]
enum Wanted {
    /// `question <task> -- asked #<id>: <title>`
    Question { task: String, id: String },
    /// `failed <task> -- <why>`
    Failed { task: String, why: String },
}

fn wanted(line: &str) -> Option<Wanted> {
    let mut parts = line.splitn(3, ' ');
    let (_, kind, rest) = (parts.next()?, parts.next()?, parts.next()?);
    let (task, note) = rest.split_once(" -- ").unwrap_or((rest, ""));
    let task = task.trim().to_string();
    match kind {
        "question" => {
            let id: String = note
                .split_once('#')?
                .1
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect();
            (!id.is_empty()).then_some(Wanted::Question { task, id })
        }
        "failed" => Some(Wanted::Failed {
            task,
            why: note.trim().to_string(),
        }),
        _ => None,
    }
}

/// How a project's child run stands at the top of a tick.
enum Running {
    No,
    Live,
    /// Gone; the status is known only for a child this serve started.
    Ended(Option<ExitStatus>),
}

struct Serve {
    mem: PathBuf,
    /// The binary serve was started from, recorded once: after a reinstall
    /// `/proc/self/exe` reads `(deleted)`, and the path names the new one.
    exe: PathBuf,
    machine: Option<String>,
    children: HashMap<String, Child>,
    /// The projects this serve claimed, with when it last stamped the claim.
    claimed: HashMap<String, i64>,
    /// What every lead goes out through, the same backend a run's workers do.
    backend: Box<dyn WorkerBackend>,
}

impl Serve {
    fn tick(&mut self) {
        if self.machine.is_none() {
            self.machine = machine(&self.mem);
        }
        let Some(here) = self.machine.clone() else {
            return;
        };
        let projects = scan_projects(&self.mem, &here);
        self.drop_lost(&projects);
        for p in projects.iter().filter(|p| p.worth_a_tick()) {
            self.tick_project(p);
        }
    }

    /// Paused, a walk, a live child, the open milestone, nothing to do: in
    /// that order, so a pause is honoured before anything else is looked at.
    fn tick_project(&mut self, p: &ServeProject) {
        let _ = std::fs::create_dir_all(p.dir());
        if p.paused {
            self.hold(p);
            return;
        }
        self.claim(p, false);
        // The milestone is open until its walk is read, so a walk not yet
        // read holds the project: a run would land the milestone again and
        // start a second walk, and a lead would work beside the walk. A
        // failed walk whose findings lead went out goes on to its fix run.
        if p.dir().join(WALK).exists() && !self.settle_walk(p) {
            return;
        }
        self.settle_leads(p);
        self.unpark(p);
        self.tail_events(p);
        match self.running(p) {
            Running::Live => {
                self.stage(p, "execution");
                return;
            }
            Running::Ended(status) => {
                self.reap(p, status);
                return;
            }
            Running::No => {}
        }
        match p.milestones.iter().position(|(_, ticked)| !ticked) {
            Some(at) => self.open_milestone(p, at),
            None => self.maintenance(p),
        }
    }

    /// Paused: a live run is told to stop once, which stops its workers and
    /// leaves their tasks dispatched for the next run to adopt; a run that
    /// has gone is reaped; nothing starts.
    fn hold(&mut self, p: &ServeProject) {
        if self.walk_again(p) {
            return;
        }
        let told = p.dir().join(STOPPED);
        match self.running(p) {
            Running::Live if !told.exists() => {
                let pid = match self.children.get(&p.name) {
                    Some(child) => child.id().to_string(),
                    None => std::fs::read_to_string(p.dir().join("child.pid"))
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                };
                sys::kill_group(&pid, "TERM");
                let _ = std::fs::write(&told, "");
                self.log(
                    p,
                    "serve: paused; told its run to stop, and its tasks wait for resume",
                );
            }
            Running::Ended(status) => self.reap(p, status),
            _ => {}
        }
        self.stage(p, "paused");
    }

    /// The owner answered the strikes question `walk again`: the pause is
    /// lifted and the count zeroed, so the next tick walks on. Only a pause
    /// the strikes made is read, never one set by hand.
    fn walk_again(&mut self, p: &ServeProject) -> bool {
        let dir = p.dir();
        let Some(walk) =
            WalkState::read(&std::fs::read_to_string(dir.join(WALK)).unwrap_or_default())
        else {
            return false;
        };
        let key = format!("strikes {}", walk.slug);
        let served = std::fs::read_to_string(dir.join("served")).unwrap_or_default();
        if !served.lines().any(|l| l == key) {
            return false;
        }
        let said = mem_on(
            &self.mem,
            &p.name,
            &["questions", "--for", "human", "--json"],
        );
        if !dogfood::walk_again(&said.out, &walk.slug) {
            return false;
        }
        let said = mem_on(&self.mem, &p.name, &["project", "unset", "paused"]);
        if !said.ok {
            warn(format!("serve {}: cannot resume -- {}", p.name, said.err));
            return false;
        }
        forget_strikes(&dir);
        self.log(
            p,
            &format!(
                "serve {}: the owner asked for another walk; resumed",
                walk.slug
            ),
        );
        true
    }

    fn running(&mut self, p: &ServeProject) -> Running {
        if let Some(child) = self.children.get_mut(&p.name) {
            let status = match child.try_wait() {
                Ok(None) => return Running::Live,
                Ok(Some(status)) => Some(status),
                Err(_) => None,
            };
            self.children.remove(&p.name);
            return Running::Ended(status);
        }
        // A child an earlier serve started outlives it, so the pid file is
        // read too; such a child is not ours to wait on, only to watch.
        let pid = std::fs::read_to_string(p.dir().join("child.pid")).unwrap_or_default();
        let pid = pid.trim();
        if pid.is_empty() {
            Running::No
        } else if sys::pid_alive(pid) {
            Running::Live
        } else {
            Running::Ended(None)
        }
    }

    /// Stamp this machine's claim on a project: when serve first works on
    /// it, when the stamp is half an hour old, and with `again` after a
    /// child run, which lets its claim go as it ends.
    fn claim(&mut self, p: &ServeProject, again: bool) {
        let Some(here) = self.machine.clone() else {
            return;
        };
        let held = p.runner.as_deref() == Some(here.as_str())
            && self
                .claimed
                .get(&p.name)
                .is_some_and(|at| sys::now() - at < RECLAIM_S);
        if held && !again {
            return;
        }
        let said = mem_on(&self.mem, &p.name, &["project", "set", "runner", &here]);
        if !said.ok {
            warn(format!("serve {}: cannot claim it -- {}", p.name, said.err));
            return;
        }
        if !self.claimed.contains_key(&p.name) {
            warn(format!("serve {}: claimed for {here}", p.name));
        }
        self.claimed.insert(p.name.clone(), sys::now());
    }

    /// A project this serve claimed that another machine now runs: its
    /// child is stopped, since two machines landing one roadmap race each
    /// other, and the move is logged with who made it.
    fn drop_lost(&mut self, here: &[ServeProject]) {
        let gone: Vec<String> = self
            .claimed
            .keys()
            .filter(|name| !here.iter().any(|p| &p.name == *name))
            .cloned()
            .collect();
        for name in gone {
            self.claimed.remove(&name);
            let Some(other) = current(&self.mem, &name).and_then(|c| c.runner) else {
                continue;
            };
            if Some(&other) == self.machine.as_ref() {
                continue;
            }
            let dir = paths::serve_root().join(name.replace('/', "-"));
            let pid = std::fs::read_to_string(dir.join("child.pid")).unwrap_or_default();
            if let Some(mut child) = self.children.remove(&name) {
                sys::kill_group(&child.id().to_string(), "TERM");
                // Waited on aside, so a run stopping its workers holds up
                // no other project's tick.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            } else if !pid.trim().is_empty() && sys::pid_alive(pid.trim()) {
                sys::kill_group(pid.trim(), "TERM");
            }
            let _ = std::fs::remove_file(dir.join("child.pid"));
            let _ = std::fs::remove_file(dir.join("stage"));
            let line =
                format!("serve: {other} took {name} over; this machine's run of it was stopped");
            warn(&line);
            mem_on(&self.mem, &name, &["log", "--type", "run", "--", &line]);
        }
    }

    /// The open milestone with no child: wait while nothing changed since a
    /// run stopped short, wait on a failed walk's findings lead and its fix
    /// task, leave a milestone with no plan alone, pick it up unless its plan
    /// has landed already or is being fixed, make the milestone mem's current
    /// plan, hold the plan to plan-check, then start the run.
    fn open_milestone(&mut self, p: &ServeProject, at: usize) {
        let dir = p.dir();
        let slug = p.milestones[at].0.clone();
        let _ = std::fs::write(
            dir.join("milestone"),
            format!("{slug} {} {}\n", at + 1, p.milestones.len()),
        );
        if let Ok(was) = std::fs::read_to_string(dir.join("waiting")) {
            if was == self.fingerprint(p, &slug) {
                self.stage(p, "waiting");
                return;
            }
            let _ = std::fs::remove_file(dir.join("waiting"));
        }
        let mut text = self.plan_text(p);
        let own = plan::slug_of(&text).as_deref() == Some(slug.as_str());
        // A failed walk's fix run waits for its findings lead to end and
        // starts only on a fix task the lead added. The milestone was picked
        // up already, so it starts with no pickup.
        let fixing = dir.join(WALK).exists();
        if fixing && !self.leads(p).is_empty() {
            self.stage(p, "waiting");
            return;
        }
        if fixing && !(own && !every_task_ticked(&text)) {
            let _ = std::fs::write(dir.join("waiting"), self.fingerprint(p, &slug));
            self.log(
                p,
                &format!("serve {slug}: the findings lead added no fix task"),
            );
            self.stage(p, "waiting");
            return;
        }
        // A lead has nothing to pick up without a plan, so the milestone
        // waits for one, said once rather than on every tick.
        let needs = dir.join("needs-plan");
        if !own && self.stored_plan(p, &slug).trim().is_empty() {
            if std::fs::read_to_string(&needs).is_ok_and(|was| was.trim() == slug) {
                self.stage(p, "needs-plan");
                return;
            }
            let _ = std::fs::write(&needs, format!("{slug}\n"));
            self.log(
                p,
                &format!(
                    "serve {slug}: no stored plan -- cut one with the plan skill; nothing starts until it is stored"
                ),
            );
            self.stage(p, "needs-plan");
            return;
        }
        let _ = std::fs::remove_file(&needs);
        // A run by hand landed this plan and left the roadmap tick to serve:
        // the run starts with nothing to do and reaches the milestone end.
        if !fixing && !(own && every_task_ticked(&text)) && !self.picked_up(p, &slug) {
            return;
        }
        // A plan refused once is looked at again only once its text changes,
        // so a refusal is said once rather than on every tick.
        if std::fs::read_to_string(dir.join("blocked-plan")).is_ok_and(|b| b == text) {
            self.stage(p, "blocked-plan");
            return;
        }
        // `--from` refuses while the current plan has an open task, and a
        // milestone picked up again is mem's current plan already.
        if !own {
            let said = mem_on(&self.mem, &p.name, &["plan", "--from", &slug]);
            if !said.ok {
                let why = said.err.trim_start_matches("mem: ").to_string();
                self.block(
                    p,
                    &slug,
                    &text,
                    &format!("mem plan --from {slug} refused: {why}"),
                );
                return;
            }
            text = self.plan_text(p);
        }
        if let Some(why) = self.refusal(p, &text) {
            self.block(p, &slug, &text, &why);
            return;
        }
        let _ = std::fs::remove_file(dir.join("blocked-plan"));
        self.start(p, &slug, at);
    }

    fn plan_text(&self, p: &ServeProject) -> String {
        mem_on(&self.mem, &p.name, &["plan"]).out
    }

    /// What plan-check refuses in this plan against the project's checkout,
    /// the plan written outside it so it is never part of the tree it judges.
    fn refusal(&self, p: &ServeProject, text: &str) -> Option<String> {
        let file = p.dir().join("plan.md");
        let _ = std::fs::write(&file, text);
        let Some(parsed) = plan::parse(text, true) else {
            return Some(
                plan::first_complaint().unwrap_or_else(|| "the plan does not parse".to_string()),
            );
        };
        let found = plancheck::findings(&parsed, &[], &p.root, Some(&file));
        (!found.refusals.is_empty()).then(|| found.refusals.join("; "))
    }

    fn block(&mut self, p: &ServeProject, slug: &str, text: &str, why: &str) {
        let _ = std::fs::write(p.dir().join("blocked-plan"), text);
        self.log(p, &format!("serve {slug}: the plan is refused -- {why}"));
        self.stage(p, "blocked-plan");
    }

    fn start(&mut self, p: &ServeProject, slug: &str, at: usize) {
        let dir = p.dir();
        let opened = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("run.log"))
            .and_then(|f| f.try_clone().map(|g| (f, g)));
        let Ok((out, err)) = opened else {
            warn(format!(
                "serve {}: cannot open {}",
                p.name,
                dir.join("run.log").display()
            ));
            return;
        };
        let spawned = Command::new(&self.exe)
            .arg("run")
            .current_dir(&p.root)
            .env("WORKFLOW_MAX_WORKERS", p.slots.to_string())
            .env_remove("MEM_PROJECT")
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            .spawn();
        let child = match spawned {
            Ok(child) => child,
            Err(e) => {
                warn(format!(
                    "serve {}: cannot start {}: {e}",
                    p.name,
                    self.exe.display()
                ));
                return;
            }
        };
        let _ = std::fs::write(dir.join("child.pid"), format!("{}\n", child.id()));
        self.children.insert(p.name.clone(), child);
        if p.roadmap_status.as_deref() == Some("approved") {
            mem_on(&self.mem, &p.name, &["roadmap", "--status", "running"]);
        }
        self.stage(p, "execution");
        warn(format!(
            "serve {}: started the run of {slug}, milestone {} of {}",
            p.name,
            at + 1,
            p.milestones.len()
        ));
    }

    /// A child gone. Its outcome is read off the run dir, not its exit code
    /// alone: every task merged is the milestone's end; a run that ended
    /// its loop or refused short of it leaves the project waiting; a run
    /// stopped by a signal or a crash goes again next tick and adopts what
    /// it left.
    fn reap(&mut self, p: &ServeProject, status: Option<ExitStatus>) {
        let dir = p.dir();
        let started = sys::mtime(&dir.join("child.pid"));
        let _ = std::fs::remove_file(dir.join("child.pid"));
        let stopped = std::fs::remove_file(dir.join(STOPPED)).is_ok();
        self.claim(p, true);
        let milestone = std::fs::read_to_string(dir.join("milestone")).unwrap_or_default();
        let Some(slug) = milestone.split_whitespace().next().map(str::to_string) else {
            return;
        };
        let run_dir = p.run_dir(&slug);
        let events = std::fs::read_to_string(run_dir.join("events")).unwrap_or_default();
        let ended = ended_since(&events, started);
        // A run a pause stopped exits by itself, but it stopped for the
        // pause, not short: it starts again once the project is resumed.
        if !ended && (stopped || status.is_none_or(|s| s.code().is_none())) {
            warn(format!(
                "serve {}: the run of {slug} went without ending; it starts again next tick",
                p.name
            ));
            return;
        }
        if ended && all_merged(&run_dir) {
            self.milestone_end(p, &slug);
            return;
        }
        let log = std::fs::read_to_string(dir.join("run.log")).unwrap_or_default();
        let _ = std::fs::write(dir.join("waiting"), self.fingerprint(p, &slug));
        self.log(
            p,
            &format!("serve {slug}: waiting -- {}", stop_reason(&log)),
        );
        self.stage(p, "waiting");
    }

    /// What has to change before a run that stopped short is worth starting
    /// again: mem's current plan, the answers to the plan's questions, and
    /// the requests left in its run dir.
    fn fingerprint(&self, p: &ServeProject, slug: &str) -> String {
        let said = mem_on(
            &self.mem,
            &p.name,
            &["questions", "--for", "orchestrator", "--json"],
        );
        let tag = format!("{slug}/");
        let mut answered: Vec<String> = serde_json::from_str::<Questions>(&said.out)
            .map(|q| q.questions)
            .unwrap_or_default()
            .into_iter()
            .filter(|q| q.answer.is_some())
            .filter(|q| q.task.as_deref().is_some_and(|t| t.starts_with(&tag)))
            .map(|q| q.id)
            .collect();
        answered.sort();
        format!(
            "{slug}\nanswered: {}\nmarkers: {}\n{}",
            answered.join(" "),
            markers(&p.run_dir(slug)).join(" "),
            self.plan_text(p)
        )
    }

    /// After the run landed every task: the walk of the milestone's Show
    /// path. A milestone with no Show line has nothing to walk and lands at
    /// once; any other lands only once its walk is read.
    fn milestone_end(&mut self, p: &ServeProject, slug: &str) {
        let text = roadmap(&self.mem, &p.name).text;
        let milestone = plan::parse(&text, false).and_then(|r| r.get(slug).cloned());
        let show = milestone.as_ref().and_then(|m| m.show.clone());
        let steps = dogfood::show_steps(show.as_deref().unwrap_or(""));
        if steps.is_empty() {
            let skipped = WalkOutcome::Skipped(NO_SHOW_PATH.into());
            self.log(p, &dogfood::walk_line(slug, &skipped, 0));
            self.land_milestone(p, slug);
            return;
        }
        let mut steps: Vec<(usize, String)> = steps
            .into_iter()
            .enumerate()
            .map(|(i, s)| (i + 1, s))
            .collect();
        // After a fix run or a skipped walk only the steps of the open
        // findings are walked again, the findings in the brief. With none
        // open the whole path is, so a milestone never lands on an empty
        // walk. The walk number and the strikes carry over.
        let was = WalkState::read(&std::fs::read_to_string(p.dir().join(WALK)).unwrap_or_default())
            .filter(|w| w.slug == slug && w.outcome.is_some());
        let mut n = 1;
        let mut strikes = 0;
        let mut findings = String::new();
        if let Some(was) = was {
            n = was.walk + 1;
            strikes = was.strikes;
            let json = mem_on(&self.mem, &p.name, &["finding", "list", "--open", "--json"]).out;
            let open = dogfood::finding_steps(&json, slug);
            if steps.iter().any(|(i, _)| open.contains(&i.to_string())) {
                steps.retain(|(i, _)| open.contains(&i.to_string()));
                let listing = mem_on(&self.mem, &p.name, &["finding", "list", "--open"]).out;
                findings = dogfood::milestone_rows(&listing, slug);
            }
        }
        let keys = current(&self.mem, &p.name).unwrap_or_default();
        let surface = milestone.and_then(|m| m.surface).unwrap_or_default();
        let w = WalkBrief {
            project: p.name.clone(),
            slug: slug.to_string(),
            playbook: match surface.as_str() {
                "" => String::new(),
                s => self.wiki(WORKFLOW, &format!("dogfood-playbooks#{s}")),
            },
            surface,
            steps,
            verify: VERIFY_SECTIONS
                .iter()
                .map(|s| {
                    self.wiki(&p.name, &format!("verify#{s}"))
                        .trim()
                        .to_string()
                })
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n"),
            dev: keys.dev,
            preview: keys.preview,
            findings,
        };
        let status = p.dir().join(format!("{slug}.dogfood.status"));
        let env = vec![(
            "WORKFLOW_STATUS_FILE".to_string(),
            status.to_string_lossy().to_string(),
        )];
        let body = brief::dogfood(&w);
        let Some(session) = self.start_session(p, slug, "dogfood", "dogfood", &body, env) else {
            return;
        };
        let walk = WalkState {
            slug: slug.to_string(),
            walk: n,
            started: sys::now(),
            steps: w.steps.iter().map(|(n, _)| *n).collect(),
            session,
            outcome: None,
            strikes,
        };
        let _ = std::fs::write(p.dir().join(WALK), walk.line());
        self.stage(p, "dogfood");
        warn(format!(
            "serve {}: started the walk of {slug}, {} steps",
            p.name,
            walk.steps.len()
        ));
    }

    /// The walk file read: a live session under its forty-five minutes holds
    /// the stage `dogfood`; one past them is stopped and skipped; an ended
    /// one is read off its report. A pass lands the milestone and drops the
    /// walk file; anything else keeps it with the outcome, its run line
    /// written once. A failed or skipped walk is a strike, and the third
    /// pauses the project; short of it a skipped walk is walked again on the
    /// next tick and a failed one starts its findings lead. The owner's
    /// questions are read on every tick a failed walk stands: one blocking
    /// question pending holds the milestone `waiting`, and a failed step left
    /// only with findings asked of the owner fails nothing. True once the
    /// findings lead has gone out, so the tick goes on to the fix run.
    fn settle_walk(&mut self, p: &ServeProject) -> bool {
        let dir = p.dir();
        let Some(mut walk) =
            WalkState::read(&std::fs::read_to_string(dir.join(WALK)).unwrap_or_default())
        else {
            self.stage(p, "dogfood");
            return false;
        };
        let slug = walk.slug.clone();
        match walk.outcome.clone() {
            Some(WalkOutcome::Failed(steps)) => {
                let listing = self.open_listing(p);
                let owners = self.owners(p);
                return match dogfood::owner_outcome(
                    WalkOutcome::Failed(steps),
                    &listing,
                    &slug,
                    &owners,
                ) {
                    None => self.held(p),
                    Some(WalkOutcome::Pass) => {
                        self.log(p, &dogfood::walk_line(&slug, &WalkOutcome::Pass, 0));
                        let _ = std::fs::remove_file(dir.join(WALK));
                        self.land_milestone(p, &slug);
                        false
                    }
                    Some(_) => self.findings_lead(p, &walk, &listing, &owners),
                };
            }
            Some(_) => {
                self.milestone_end(p, &slug);
                return false;
            }
            None => {}
        }
        let h = Handle {
            session: walk.session.clone(),
            pidfile: dir.join("dogfood.pid"),
            worktree: p.root.clone(),
        };
        let age = sys::now() - walk.started;
        let live = self.backend.alive(&h) || (age < LAUNCH_S && !self.backend.seen(&h));
        if live && age < dogfood::WALK_S {
            self.stage(p, "dogfood");
            return false;
        }
        let outcome = if live {
            self.backend.stop(&h, 10);
            WalkOutcome::Skipped(dogfood::WALK_TIMED_OUT.into())
        } else {
            let status = dir.join(format!("{slug}.dogfood.status"));
            dogfood::read_outcome(&std::fs::read_to_string(status).unwrap_or_default())
        };
        let said = mem_on(&self.mem, &p.name, &["finding", "list", "--open", "--json"]);
        let open = dogfood::finding_steps(&said.out, &slug);
        let outcome = dogfood::held_to_findings(outcome, &open);
        // A finding on a step walked clean was fixed by what landed before
        // the walk: the trunk head. The session's own outcome decides it, so
        // a finding the owner holds stays open on the step it failed.
        let fixed = dogfood::fixed_findings(&said.out, &slug, &walk.steps, &outcome);
        let head = Git::at(&p.root).head().unwrap_or_default();
        for id in &fixed {
            mem_on(&self.mem, &p.name, &["finding", "close", id, "--by", &head]);
        }
        let listing = self.open_listing(p);
        let owners = self.owners(p);
        let counted = dogfood::owner_outcome(outcome.clone(), &listing, &slug, &owners);
        self.log(
            p,
            &dogfood::walk_line(
                &slug,
                counted.as_ref().unwrap_or(&outcome),
                open.len().saturating_sub(fixed.len()),
            ),
        );
        let Some(outcome) = counted else {
            walk.outcome = Some(outcome);
            let _ = std::fs::write(dir.join(WALK), walk.line());
            return self.held(p);
        };
        if outcome == WalkOutcome::Pass {
            let _ = std::fs::remove_file(dir.join(WALK));
            self.land_milestone(p, &slug);
            return false;
        }
        let failed = matches!(outcome, WalkOutcome::Failed(_));
        walk.outcome = Some(outcome);
        walk.strikes += 1;
        let _ = std::fs::write(dir.join(WALK), walk.line());
        if walk.strikes >= dogfood::STRIKES {
            self.strike_out(p, &slug);
            return false;
        }
        if failed {
            self.findings_lead(p, &walk, &listing, &owners);
        }
        self.stage(p, "waiting");
        false
    }

    fn open_listing(&self, p: &ServeProject) -> String {
        mem_on(&self.mem, &p.name, &["finding", "list", "--open", "--json"]).out
    }

    fn owners(&self, p: &ServeProject) -> Vec<OwnerFinding> {
        let said = mem_on(
            &self.mem,
            &p.name,
            &["questions", "--for", "human", "--json"],
        );
        dogfood::owner_findings(&said.out)
    }

    /// A milestone held on a blocking question: no strike and no lead until
    /// the answer. A lead that asked it still ends and is dropped here, since
    /// the tick goes no further.
    fn held(&mut self, p: &ServeProject) -> bool {
        self.settle_leads(p);
        self.stage(p, "waiting");
        false
    }

    /// The third strike: the project paused as `workflow pause` does it, and
    /// the owner asked once whether to walk again.
    fn strike_out(&mut self, p: &ServeProject, slug: &str) {
        let here = self.machine.clone().unwrap_or_default();
        let text = format!("{here} {}", &sys::utc_now()[..10]);
        let said = mem_on(
            &self.mem,
            &p.name,
            &["project", "set", "paused", text.trim()],
        );
        if !said.ok {
            warn(format!("serve {}: cannot pause -- {}", p.name, said.err));
        }
        if self.serve_once(p, &format!("strikes {slug}")) {
            let listing = mem_on(&self.mem, &p.name, &["finding", "list", "--open"]).out;
            let question =
                dogfood::strikes_question(slug, &dogfood::milestone_rows(&listing, slug));
            mem_on(
                &self.mem,
                &p.name,
                &[
                    "ask",
                    "--for",
                    "human",
                    "--options",
                    "walk again,leave paused",
                    "--recommend",
                    "leave paused",
                    "--",
                    &question,
                ],
            );
        }
        self.log(
            p,
            &format!(
                "serve {slug}: paused after {} failed walks; the owner is asked",
                dogfood::STRIKES
            ),
        );
        self.stage(p, "paused");
    }

    /// The findings lead for a failed walk, started once per walk and again
    /// for an owner's answer that came after it, the answers under the
    /// findings: true once it has gone out, false on the tick that starts it.
    fn findings_lead(
        &mut self,
        p: &ServeProject,
        walk: &WalkState,
        open: &str,
        owners: &[OwnerFinding],
    ) -> bool {
        let slug = &walk.slug;
        let first = self.serve_once(p, &format!("findings {slug} {}", walk.walk));
        // An answer waits for the lead going now, which may be the one that
        // asked it.
        if !first && !self.leads(p).is_empty() {
            return true;
        }
        let answered: Vec<&OwnerFinding> = dogfood::owned(open, slug, owners)
            .into_iter()
            .filter(|o| o.answered)
            .collect();
        let mut fresh = false;
        for o in &answered {
            fresh |= self.serve_once(p, &format!("answered {slug} {}", o.finding));
        }
        if !first && !fresh {
            return true;
        }
        let listing = mem_on(&self.mem, &p.name, &["finding", "list", "--open"]).out;
        let mut rows = dogfood::milestone_rows(&listing, slug);
        for o in &answered {
            rows.push_str(&format!(
                "#{}  the owner answered: {}\n",
                o.finding, o.answer
            ));
        }
        let text = self.stored_plan(p, slug);
        let body = brief::lead_findings(&self.lead_ctx(p, slug, "", &text), &rows);
        self.start_lead(p, slug, "findings", &body);
        self.stage(p, "waiting");
        false
    }

    /// One page or section of a project's wiki; empty when it has none.
    fn wiki(&self, project: &str, slug: &str) -> String {
        let said = mem_on(&self.mem, project, &["wiki", "--", slug]);
        if said.ok { said.out } else { String::new() }
    }

    /// A milestone walked, or with nothing to walk: the roadmap tick, which
    /// the run leaves to serve, the plan marked done, the status and handoff
    /// lines, the hygiene count and the landed commit, in that order.
    fn land_milestone(&mut self, p: &ServeProject, slug: &str) {
        let mut road = milestones(&roadmap(&self.mem, &p.name).text);
        if !road.iter().any(|(s, ticked)| s == slug && *ticked) {
            let said = mem_on(&self.mem, &p.name, &["roadmap", "--tick", slug]);
            if said.ok {
                road = milestones(&roadmap(&self.mem, &p.name).text);
            } else {
                warn(format!(
                    "serve {}: cannot tick {slug} -- {}",
                    p.name, said.err
                ));
            }
        }
        mem_on(&self.mem, &p.name, &["plan", "--status", "done"]);
        let head = Git::at(&p.root).head().unwrap_or_default();
        let short = &head[..head.len().min(7)];
        let date = &sys::utc_now()[..10];
        mem_on(
            &self.mem,
            &p.name,
            &[
                "status",
                "--set",
                &format!("{slug} landed at {short} on {date}"),
            ],
        );
        let handoff = match road.iter().find(|(s, ticked)| !ticked && s != slug) {
            Some((next, _)) => format!("{slug} landed at {short}; next is {next}"),
            None => format!(
                "{slug} landed at {short}; no milestone left, the roadmap goes to maintenance"
            ),
        };
        mem_on(&self.mem, &p.name, &["handoff", "--set", &handoff]);
        match self.hygiene(p) {
            Some(n) => self.log(p, &format!("hygiene {slug}: {n} findings")),
            None => self.log(p, &format!("hygiene {slug}: the check did not run")),
        }
        let _ = std::fs::write(p.dir().join("last-landed"), format!("{head}\n"));
        warn(format!("serve {}: {handoff}", p.name));
    }

    /// The hygiene findings over the checkout's tracked tree, counted.
    fn hygiene(&self, p: &ServeProject) -> Option<usize> {
        let out = Command::new(&self.exe)
            .args(["hygiene", "--tree", "--json"])
            .current_dir(&p.root)
            .env_remove("MEM_PROJECT")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        serde_json::from_slice::<Vec<serde_json::Value>>(&out.stdout)
            .ok()
            .map(|f| f.len())
    }

    /// Every milestone ticked: the roadmap is in maintenance, and serve
    /// keeps ticking so an approved roadmap or a moved runner is seen.
    fn maintenance(&mut self, p: &ServeProject) {
        let _ = std::fs::remove_file(p.dir().join("milestone"));
        if p.roadmap_status.as_deref() != Some("maintenance") {
            mem_on(&self.mem, &p.name, &["roadmap", "--status", "maintenance"]);
            self.log(
                p,
                "serve: every milestone is ticked; the roadmap is in maintenance",
            );
        }
        self.stage(p, "maintenance");
    }

    /// The leads recorded for a project.
    fn leads(&self, p: &ServeProject) -> Vec<Lead> {
        read_leads(&std::fs::read_to_string(p.dir().join("leads")).unwrap_or_default())
    }

    fn lead_handle(&self, dir: &Path, root: &Path, lead: &Lead) -> Handle {
        Handle {
            session: lead.session.clone(),
            pidfile: dir.join(LEAD_PID),
            worktree: root.to_path_buf(),
        }
    }

    /// Drop every recorded lead that ended, and stop a pickup lead that ran
    /// past its twenty minutes, so what `leads` holds after this is live.
    fn settle_leads(&mut self, p: &ServeProject) {
        let dir = p.dir();
        let was = self.leads(p);
        let now = sys::now();
        let mut kept = Vec::new();
        for lead in &was {
            let h = self.lead_handle(&dir, &p.root, lead);
            let age = now - lead.started;
            let live = self.backend.alive(&h) || (age < LAUNCH_S && !self.backend.seen(&h));
            if live && lead.kind == "pickup" && age >= PICKUP_S {
                self.backend.stop(&h, 10);
                self.log(
                    p,
                    "serve: the pickup lead ran twenty minutes and was stopped; plan-check goes on without it",
                );
                continue;
            }
            if live {
                kept.push(lead.clone());
            }
        }
        if kept != was {
            let _ = std::fs::write(dir.join("leads"), write_leads(&kept));
        }
    }

    /// Every lead an earlier serve recorded is stopped: nothing is waiting
    /// on it any longer, and one left going would be a second lead beside
    /// the next one this serve starts.
    fn stop_recorded_leads(&self) {
        let dirs = std::fs::read_dir(paths::serve_root())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path());
        for dir in dirs {
            let file = dir.join("leads");
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            for lead in read_leads(&text) {
                self.backend.stop(&self.lead_handle(&dir, &dir, &lead), 10);
            }
            let _ = std::fs::remove_file(file);
        }
    }

    /// Start a session as `role` on `body` in the checkout, its brief at
    /// `<serve dir>/<slug>.<kind>.md` and its pid at `<serve dir>/<role>.pid`,
    /// with `env` beside `MEM_PROJECT`. The session the backend started, or
    /// nothing when it refused the launch, which is logged.
    fn start_session(
        &mut self,
        p: &ServeProject,
        slug: &str,
        kind: &str,
        role: &str,
        body: &str,
        env: Vec<(String, String)>,
    ) -> Option<String> {
        let dir = p.dir();
        let stem = format!("{slug}.{kind}");
        let brief = dir.join(format!("{stem}.md"));
        let _ = std::fs::write(&brief, body);
        let status = dir.join(format!("{stem}.status"));
        let _ = std::fs::write(&status, "");
        let pidfile = dir.join(format!("{role}.pid"));
        let _ = std::fs::remove_file(&pidfile);
        // A walk is its own kind: `dogfood`, not `dogfood-dogfood`.
        let (task, what) = match kind == role {
            true => (role.to_string(), format!("{role} session")),
            false => (format!("{role}-{kind}"), format!("{kind} {role}")),
        };
        let d = Dispatch {
            task,
            worktree: p.root.clone(),
            brief,
            out: dir.join(format!("{stem}.json")),
            err: dir.join(format!("{stem}.err")),
            pidfile,
            status,
            rundir: dir.clone(),
            session: self.backend.mint_session(),
            parent: None,
            role: role.into(),
            // Naming the role as the model lets its role file say which
            // model and effort the session runs on.
            model: role.into(),
            effort: None,
            turns: std::env::var("WORKFLOW_MAX_TURNS").unwrap_or_else(|_| "120".into()),
            env: [("MEM_PROJECT".to_string(), p.name.clone())]
                .into_iter()
                .chain(env)
                .collect(),
        };
        let session = self.backend.dispatch(&d);
        if session.is_empty() {
            let err = std::fs::read_to_string(&d.err).unwrap_or_default();
            let why = err.lines().last().unwrap_or("no word from the backend");
            self.log(
                p,
                &format!("serve {slug}: the {what} did not start -- {why}"),
            );
            return None;
        }
        Some(session)
    }

    /// Start a lead on `body` and record it. A launch the backend refused is
    /// recorded nowhere, so nothing waits on it.
    fn start_lead(&mut self, p: &ServeProject, slug: &str, kind: &str, body: &str) {
        let Some(session) = self.start_session(p, slug, kind, "lead", body, Vec::new()) else {
            return;
        };
        let dir = p.dir();
        let mut leads = self.leads(p);
        leads.push(Lead {
            kind: kind.to_string(),
            session,
            started: sys::now(),
        });
        let _ = std::fs::write(dir.join("leads"), write_leads(&leads));
        warn(format!(
            "serve {}: started the {kind} lead for {slug}",
            p.name
        ));
    }

    /// The milestone's plan as mem stores it, current or not.
    fn stored_plan(&self, p: &ServeProject, slug: &str) -> String {
        mem_on(&self.mem, &p.name, &["plan", slug]).out
    }

    fn lead_ctx(&self, p: &ServeProject, slug: &str, task: &str, text: &str) -> LeadCtx {
        let block = plan::parse(text, false)
            .and_then(|parsed| parsed.get(task).map(|t| t.block.clone()))
            .unwrap_or_default();
        LeadCtx {
            project: p.name.clone(),
            plan_slug: slug.to_string(),
            task: task.to_string(),
            block,
            prose: plan::prose(text),
        }
    }

    /// Whether the milestone's pickup is over: its lead ran and ended, or
    /// was stopped at twenty minutes, or never started. The first time it
    /// is asked, the pickup lead goes out and the answer is no.
    fn picked_up(&mut self, p: &ServeProject, slug: &str) -> bool {
        let leads = self.leads(p);
        let brief = p.dir().join(format!("{slug}.pickup.md"));
        // One lead at a time: a pickup waits on whatever lead is going.
        if leads.iter().any(|l| l.kind == "pickup") || (!brief.exists() && !leads.is_empty()) {
            self.stage(p, "pickup");
            return false;
        }
        if brief.exists() {
            return true;
        }
        let text = self.stored_plan(p, slug);
        let git = Git::at(&p.root);
        let since = std::fs::read_to_string(p.dir().join("last-landed")).unwrap_or_default();
        let range = format!("{}..HEAD", since.trim());
        let log = match git.capture(&["log", "--oneline", &range]) {
            o if o.ok && !since.trim().is_empty() => gitcmd::lossy(&o.stdout),
            _ => git.out(&["log", "--oneline", "-30"]).unwrap_or_default(),
        };
        let mut globs = Vec::new();
        for t in plan::parse(&text, false)
            .map(|p| p.tasks)
            .unwrap_or_default()
        {
            for pat in ownership::split_patterns(t.files.as_deref().unwrap_or("")) {
                let spec = gitcmd::glob_top(&pat);
                let paths = gitcmd::nul_fields(&git.bytes(&["ls-files", "-z", "--", &spec]))
                    .iter()
                    .map(|f| gitcmd::lossy(f))
                    .collect();
                globs.push((format!("{} `{pat}`", t.id), paths));
            }
        }
        let findings = mem_on(&self.mem, &p.name, &["finding", "list", "--open"]).out;
        let body = brief::lead_pickup(&self.lead_ctx(p, slug, "", &text), &log, &globs, &findings);
        self.start_lead(p, slug, "pickup", &body);
        self.stage(p, "pickup");
        false
    }

    /// A parked task whose questions are all answered is parked no longer:
    /// the run sends it again with the answer, and a later question of its
    /// own gets a lead.
    fn unpark(&self, p: &ServeProject) {
        let milestone = std::fs::read_to_string(p.dir().join("milestone")).unwrap_or_default();
        let Some(slug) = milestone.split_whitespace().next() else {
            return;
        };
        let run_dir = p.run_dir(slug);
        let parked: Vec<String> = std::fs::read_dir(&run_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.strip_suffix(".parked").map(str::to_string)
            })
            .collect();
        if parked.is_empty() {
            return;
        }
        let said = mem_on(
            &self.mem,
            &p.name,
            &["questions", "--for", "orchestrator", "--json"],
        );
        let Ok(listed) = serde_json::from_str::<Questions>(&said.out) else {
            return;
        };
        for task in parked {
            let tag = format!("{slug}/{task}");
            let asked: Vec<&memcli::Question> = listed
                .questions
                .iter()
                .filter(|q| q.task.as_deref() == Some(tag.as_str()))
                .collect();
            if asked.is_empty() || asked.iter().any(|q| q.answer.is_none()) {
                continue;
            }
            let _ = std::fs::remove_file(run_dir.join(format!("{task}.parked")));
            self.log(
                p,
                &format!("serve {slug}: {task}'s question is answered; it is parked no longer"),
            );
        }
    }

    /// Read the open milestone's run events from serve's own cursor and
    /// start the lead the first one that wants it asks for. An event met
    /// while a lead is going is left for a later tick.
    fn tail_events(&mut self, p: &ServeProject) {
        let dir = p.dir();
        let milestone = std::fs::read_to_string(dir.join("milestone")).unwrap_or_default();
        let Some(slug) = milestone.split_whitespace().next().map(str::to_string) else {
            return;
        };
        let run_dir = p.run_dir(&slug);
        let Ok(events) = std::fs::read_to_string(run_dir.join("events")) else {
            return;
        };
        let cursor = std::fs::read_to_string(dir.join("events.cursor")).unwrap_or_default();
        let mut at = match cursor.split_once(' ') {
            Some((s, n)) if s == slug => n.trim().parse().unwrap_or(0),
            _ => 0,
        };
        // A shorter file is a new one, read from its start.
        let rest = match events.get(at..) {
            Some(rest) => rest,
            None => {
                at = 0;
                &events
            }
        };
        for line in rest.split_inclusive('\n') {
            // A line without its newline is still being written.
            if !line.ends_with('\n') {
                break;
            }
            if let Some(w) = wanted(line.trim_end()) {
                if !self.leads(p).is_empty() {
                    break;
                }
                self.serve_event(p, &slug, &run_dir, w);
            }
            at += line.len();
        }
        let _ = std::fs::write(dir.join("events.cursor"), format!("{slug} {at}\n"));
    }

    /// One wanted event, served at most once: a question per id, unless
    /// its task is parked or it is answered already; a failure per task and
    /// dispatch count.
    fn serve_event(&mut self, p: &ServeProject, slug: &str, run_dir: &Path, w: Wanted) {
        let key = match &w {
            Wanted::Question { id, .. } => format!("question {id}"),
            Wanted::Failed { task, .. } => {
                let n = std::fs::read_to_string(run_dir.join(format!("{task}.dispatches")))
                    .unwrap_or_default();
                format!("failed {task} {}", n.trim())
            }
        };
        if !self.serve_once(p, &key) {
            return;
        }
        let text = self.stored_plan(p, slug);
        match w {
            Wanted::Question { task, id } => {
                if run_dir.join(format!("{task}.parked")).exists() {
                    return;
                }
                let said = mem_on(
                    &self.mem,
                    &p.name,
                    &["questions", "--for", "orchestrator", "--json"],
                );
                let listed = serde_json::from_str::<Questions>(&said.out)
                    .map(|q| q.questions)
                    .unwrap_or_default();
                let q = match listed.into_iter().find(|q| q.short_id == id) {
                    Some(q) if q.answer.is_some() => return,
                    Some(q) => q,
                    // mem's listing can lag a question asked a moment ago;
                    // the id is enough for the lead to read it.
                    None => memcli::Question {
                        id: id.clone(),
                        short_id: id.clone(),
                        title: format!("asked by {task}"),
                        body: String::new(),
                        task: Some(format!("{slug}/{task}")),
                        answer: None,
                    },
                };
                let body = brief::lead_question(&self.lead_ctx(p, slug, &task, &text), &q);
                self.start_lead(p, slug, "question", &body);
            }
            Wanted::Failed { task, why } => {
                let body = brief::lead_failure(&self.lead_ctx(p, slug, &task, &text), &why);
                self.start_lead(p, slug, "failure", &body);
            }
        }
    }

    /// Whether `key` is new to the project's `served` file, recording it when
    /// it is, so each lead it stands for goes out once.
    fn serve_once(&self, p: &ServeProject, key: &str) -> bool {
        let file = p.dir().join("served");
        let mut served = std::fs::read_to_string(&file).unwrap_or_default();
        if served.lines().any(|l| l == key) {
            return false;
        }
        served.push_str(&format!("{key}\n"));
        let _ = std::fs::write(&file, served);
        true
    }

    /// Told to stop: every child run gets SIGTERM, which stops its workers
    /// and leaves their tasks dispatched, and serve waits up to thirty
    /// seconds on each. The pid files stay, so the next serve finds each run
    /// gone without an end and starts it again to adopt those tasks.
    fn shutdown(&mut self) -> i32 {
        let ours: Vec<String> = self.children.values().map(|c| c.id().to_string()).collect();
        // A child an earlier serve started is watched through its pid file
        // alone, and is stopped the same way.
        let theirs: Vec<String> = std::fs::read_dir(paths::serve_root())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| std::fs::read_to_string(e.path().join("child.pid")).ok())
            .map(|pid| pid.trim().to_string())
            .filter(|pid| !pid.is_empty() && !ours.contains(pid) && sys::pid_alive(pid))
            .collect();
        warn(format!(
            "serve: told to stop -- stopping {} run(s) before going",
            ours.len() + theirs.len()
        ));
        for pid in ours.iter().chain(&theirs) {
            sys::kill_group(pid, "TERM");
        }
        for (name, mut child) in self.children.drain() {
            let until = std::time::Instant::now() + std::time::Duration::from_secs_f64(STOP_S);
            // Waited on, not probed: a child that exited stays a zombie, and
            // alive to `kill -0`, until it is reaped.
            while matches!(child.try_wait(), Ok(None)) {
                if std::time::Instant::now() >= until {
                    warn(format!(
                        "serve {name}: its run did not stop within thirty seconds; going without it"
                    ));
                    break;
                }
                sys::sleep(0.2);
            }
        }
        for pid in &theirs {
            let until = std::time::Instant::now() + std::time::Duration::from_secs_f64(STOP_S);
            while sys::pid_alive(pid) && std::time::Instant::now() < until {
                sys::sleep(0.2);
            }
        }
        exit::OK
    }

    /// One word in the stage file, said on stderr when it changes.
    fn stage(&self, p: &ServeProject, word: &str) {
        let path = p.dir().join("stage");
        if std::fs::read_to_string(&path).is_ok_and(|s| s.trim() == word) {
            return;
        }
        let _ = std::fs::write(&path, format!("{word}\n"));
        warn(format!("serve {}: {word}", p.name));
    }

    /// Logged as a run line in the project's own log, and said on stderr
    /// under the project's name, which the log line leaves implicit.
    fn log(&self, p: &ServeProject, line: &str) {
        warn(format!("{}: {line}", p.name));
        mem_on(&self.mem, &p.name, &["log", "--type", "run", "--", line]);
    }
}

/// `workflow park <task> "<reason>"` -- the label a lead puts on a task whose
/// question went to the owner, in the live run's directory.
pub fn cmd_park(task: &str, reason: &str) -> i32 {
    if !Git::here().inside_worktree() {
        warn("park: stand in the project checkout");
        return exit::USAGE;
    }
    let Some(project) = memcli::project_current() else {
        warn("park: mem does not know this checkout");
        return exit::USAGE;
    };
    let root = paths::runs_root().join(project.dir_name());
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|d| d.join("plan.md").is_file() && d.join(format!("{task}.state")).is_file())
        .collect();
    dirs.sort();
    // Taking the lock and getting it means nobody holds the run.
    let Some(dir) = dirs.into_iter().find(|d| run::lock_run(d).is_none()) else {
        warn(format!(
            "park: no live run holds {task}; a task is parked only while a run holds it open on its question"
        ));
        return exit::USAGE;
    };
    let reason = reason.trim();
    if let Err(e) = std::fs::write(dir.join(format!("{task}.parked")), format!("{reason}\n")) {
        warn(format!("park: cannot write in {}: {e}", dir.display()));
        return exit::FAILED;
    }
    memcli::log_run(&format!("park {task}: {reason}"));
    let plan = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    warn(format!("run {plan}: {task} parked -- {reason}"));
    exit::OK
}

/// Zero the walk's strikes and forget the strikes question, so a later
/// third strike asks the owner again.
fn forget_strikes(dir: &Path) {
    let file = dir.join(WALK);
    let Some(mut walk) = WalkState::read(&std::fs::read_to_string(&file).unwrap_or_default())
    else {
        return;
    };
    let key = format!("strikes {}", walk.slug);
    walk.strikes = 0;
    let _ = std::fs::write(&file, walk.line());
    let served = std::fs::read_to_string(dir.join("served")).unwrap_or_default();
    let kept: String = served
        .lines()
        .filter(|l| *l != key)
        .map(|l| format!("{l}\n"))
        .collect();
    let _ = std::fs::write(dir.join("served"), kept);
}

/// `workflow pause [<project>]` and `workflow resume [<project>]`: the
/// `paused` project key, which serve reads on every tick.
pub fn cmd_pause(project: Option<&str>, pause: bool) -> i32 {
    let verb = if pause { "pause" } else { "resume" };
    let name = match project {
        Some(name) => name.to_string(),
        None => match memcli::project_current() {
            Some(p) => p.name,
            None => {
                warn(format!(
                    "{verb}: name a project, or stand in a checkout mem knows"
                ));
                return exit::USAGE;
            }
        },
    };
    let mem = PathBuf::from(memcli::bin());
    let said = if pause {
        let here = current(&mem, &name)
            .and_then(|c| c.machine)
            .unwrap_or_default();
        let text = format!("{here} {}", &sys::utc_now()[..10]);
        mem_on(&mem, &name, &["project", "set", "paused", text.trim()])
    } else {
        mem_on(&mem, &name, &["project", "unset", "paused"])
    };
    if !said.ok {
        warn(format!("{verb}: {}", said.err.trim_start_matches("mem: ")));
        return exit::FAILED;
    }
    if pause {
        warn(format!(
            "{name}: paused; serve stops its run and starts nothing until `workflow resume`"
        ));
    } else {
        forget_strikes(&paths::serve_root().join(name.replace('/', "-")));
        warn(format!(
            "{name}: resumed; serve starts its run on the next tick"
        ));
    }
    exit::OK
}

pub fn cmd_serve(once: bool, tick_s: Option<f64>) -> i32 {
    let tick = tick_s
        .or_else(|| {
            std::env::var("WORKFLOW_TICK_S")
                .ok()
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(TICK_S);
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            warn(format!("serve: cannot tell which binary this is: {e}"));
            return exit::USAGE;
        }
    };
    let root = paths::serve_root();
    if let Err(e) = std::fs::create_dir_all(&root) {
        warn(format!("serve: cannot make {}: {e}", root.display()));
        return exit::USAGE;
    }
    // Two serves on one machine would start two runs of one milestone.
    let Some(_lock) = run::lock_run(&root) else {
        warn("serve: another serve is live on this machine");
        return exit::USAGE;
    };
    let mut serve = Serve {
        mem: PathBuf::from(memcli::bin()),
        exe,
        machine: None,
        children: HashMap::new(),
        claimed: HashMap::new(),
        backend: backend_for(),
    };
    // The signal only raises the flag; the loop sees it between ticks and
    // passes the stop on to every child run, as a run does to its workers.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        let _ = signal_hook::flag::register(sig, stop.clone());
    }
    let stopping = || stop.load(std::sync::atomic::Ordering::Relaxed);
    serve.stop_recorded_leads();
    loop {
        if stopping() {
            return serve.shutdown();
        }
        serve.tick();
        if once {
            return exit::OK;
        }
        // Slept in slices, so a stop waits on no more than one of them.
        let until = std::time::Instant::now() + std::time::Duration::from_secs_f64(tick);
        while !stopping() && std::time::Instant::now() < until {
            sys::sleep(0.1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serve_reads_an_end_the_run_wrote_after_the_child_started() {
        let events =
            "2026-10-03T10:00:00Z merged t1\n2026-10-03T10:00:05Z ended 2 merged, 0 failed\n";
        let at = |s: &str| s.parse::<jiff::Timestamp>().unwrap().as_second();
        assert!(ended_since(events, at("2026-10-03T09:59:00Z")));
        assert!(ended_since(events, at("2026-10-03T10:00:05Z")));
        assert!(
            !ended_since(events, at("2026-10-03T10:01:00Z")),
            "an earlier run's end"
        );
        let signalled =
            "2026-10-03T10:00:05Z ended stopped by signal with 1 worker(s) left dispatched\n";
        assert!(
            !ended_since(signalled, at("2026-10-03T09:59:00Z")),
            "a run stopped by a signal did not end"
        );
    }

    #[test]
    fn serve_finds_a_milestone_done_only_when_every_state_is_merged() {
        let dir = std::env::temp_dir().join(format!("wf-serve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(
            !all_merged(&dir),
            "a run that refused before writing a state"
        );
        std::fs::write(dir.join("t1.state"), "merged\n").unwrap();
        std::fs::write(dir.join("t2.state"), "done-previously\n").unwrap();
        assert!(all_merged(&dir));
        std::fs::write(dir.join("t3.state"), "failed\n").unwrap();
        assert!(!all_merged(&dir));
        std::fs::write(dir.join("t3.redispatch"), "").unwrap();
        std::fs::write(dir.join("t1.accept"), "").unwrap();
        assert_eq!(markers(&dir), ["t1.accept", "t3.redispatch"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn serve_gives_the_stopped_short_line_as_the_reason() {
        let log = "workflow: run s1: 2 tasks, up to 2 at a time\n\
                   workflow: Plan s1 stopped short: 1 of 2 merged, 1 failed.\n\
                   workflow: Failed - blocked: sulk.\n";
        assert_eq!(
            stop_reason(log),
            "Plan s1 stopped short: 1 of 2 merged, 1 failed."
        );
        assert_eq!(
            stop_reason("workflow: run: plan-check refuses this plan -- fix it before running\n\n"),
            "run: plan-check refuses this plan -- fix it before running"
        );
        assert_eq!(stop_reason(""), "the run ended without a word");
    }

    #[test]
    fn serve_reads_milestones_off_a_roadmap_and_nothing_off_a_plan() {
        let road = "# roadmap: r\n\n- [x] m1 The first\n- [ ] m2 The second\n";
        assert_eq!(
            milestones(road),
            [("m1".to_string(), true), ("m2".to_string(), false)]
        );
        let plan = "# plan: m1\n\n- [ ] t1 A\n      Files: a\n";
        assert!(milestones(plan).is_empty());
        assert!(milestones("").is_empty());
    }

    #[test]
    fn serve_sees_a_plan_landed_only_when_every_task_is_ticked() {
        let task = |id: &str, mark: &str| {
            format!("- [{mark}] {id} Add {id}\n      Files: app/{id}.php\n      Verify: true\n")
        };
        let landed = format!("# plan: m1\n\n{}{}", task("a1", "x"), task("a2", "x"));
        let open = format!("# plan: m1\n\n{}{}", task("a1", "x"), task("a2", " "));
        assert!(every_task_ticked(&landed));
        assert!(!every_task_ticked(&open));
        assert!(!every_task_ticked("# plan: m1\n"));
    }

    #[test]
    fn serve_keeps_its_leads_one_line_each() {
        let leads = vec![Lead {
            kind: "pickup".into(),
            session: "wf-lead-pickup-a3k9".into(),
            started: 1_791_000_000,
        }];
        let text = write_leads(&leads);
        assert_eq!(text, "pickup wf-lead-pickup-a3k9 1791000000\n");
        assert_eq!(read_leads(&text), leads);
        assert!(read_leads("pickup half-written\n").is_empty());
    }

    #[test]
    fn serve_wants_a_lead_for_a_question_and_a_failure_only() {
        assert_eq!(
            wanted("2026-10-03T10:00:00Z question ask -- asked #AB12CD34: may I widen Files?"),
            Some(Wanted::Question {
                task: "ask".into(),
                id: "AB12CD34".into()
            })
        );
        assert_eq!(
            wanted("2026-10-03T10:00:00Z failed die -- the worker stopped without reporting ready"),
            Some(Wanted::Failed {
                task: "die".into(),
                why: "the worker stopped without reporting ready".into()
            })
        );
        assert_eq!(wanted("2026-10-03T10:00:00Z merged ask"), None);
        assert_eq!(
            wanted("2026-10-03T10:00:05Z ended 1 merged, 1 failed"),
            None
        );
    }

    #[test]
    fn serve_ticks_a_project_only_under_a_worked_status_with_milestones() {
        let p = |status: Option<&str>, milestones: Vec<(String, bool)>| ServeProject {
            name: "app".into(),
            root: PathBuf::from("/"),
            runner: None,
            paused: false,
            slots: SLOTS,
            roadmap_status: status.map(str::to_string),
            milestones,
        };
        let one = || vec![("m1".to_string(), false)];
        assert!(p(Some("approved"), one()).worth_a_tick());
        assert!(p(Some("maintenance"), one()).worth_a_tick());
        assert!(!p(Some("draft"), one()).worth_a_tick());
        assert!(!p(None, one()).worth_a_tick());
        assert!(!p(Some("running"), Vec::new()).worth_a_tick());
    }
}
