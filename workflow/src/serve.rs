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

use crate::gitcmd::Git;
use crate::plan::{self, PlanKind};
use crate::{exit, memcli, paths, plancheck, run, sys, warn};

/// Seconds between ticks when neither `--tick` nor `WORKFLOW_TICK_S` says.
const TICK_S: f64 = 5.0;

/// Workers a child run holds at once when the project sets no `slots`.
const SLOTS: u64 = 2;

/// How old serve lets its own claim on a project grow before stamping it
/// again: another machine reads a claim as abandoned at an hour.
const RECLAIM_S: i64 = 30 * 60;

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

    /// Paused, a live child, the open milestone, nothing to do: in that
    /// order, so a pause is honoured before anything else is looked at.
    fn tick_project(&mut self, p: &ServeProject) {
        let _ = std::fs::create_dir_all(p.dir());
        if p.paused {
            self.stage(p, "paused");
            return;
        }
        self.claim(p, false);
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
    /// run stopped short, make the milestone mem's current plan, hold the
    /// plan to plan-check, then start the run.
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
        // A plan refused once is looked at again only once its text changes,
        // so a refusal is said once rather than on every tick.
        if std::fs::read_to_string(dir.join("blocked-plan")).is_ok_and(|b| b == text) {
            self.stage(p, "blocked-plan");
            return;
        }
        // `--from` refuses while the current plan has an open task, and a
        // milestone picked up again is mem's current plan already.
        if plan::slug_of(&text).as_deref() != Some(slug.as_str()) {
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
        self.claim(p, true);
        let milestone = std::fs::read_to_string(dir.join("milestone")).unwrap_or_default();
        let Some(slug) = milestone.split_whitespace().next().map(str::to_string) else {
            return;
        };
        let run_dir = p.run_dir(&slug);
        let events = std::fs::read_to_string(run_dir.join("events")).unwrap_or_default();
        let ended = ended_since(&events, started);
        if !ended && status.is_none_or(|s| s.code().is_none()) {
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

    /// After the run landed every task: the roadmap tick if the run could
    /// not make it, the dogfood stage, the plan marked done, the status and
    /// handoff lines, the hygiene count and the landed commit, in that order.
    fn milestone_end(&mut self, p: &ServeProject, slug: &str) {
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
        // Dogfooding the landed milestone is a later stage; it passes for now.
        self.log(p, &format!("dogfood {slug}: skipped, m5"));
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
    };
    loop {
        serve.tick();
        if once {
            return exit::OK;
        }
        sys::sleep(tick);
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
