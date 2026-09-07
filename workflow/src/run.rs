//! `workflow run` and `workflow reap` -- deterministic interim orchestration
//! (spec §8).
//!
//! Policy lives here and nowhere else: waves, concurrency, ownership, the
//! serialized merge gate. How a worker is started, watched and stopped is
//! the backend's business (see [`crate::backend`]).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::backend::{ClaudeBackend, Dispatch, Handle, WorkerBackend};
use crate::backend_amx::AmxBackend;
use crate::gitcmd::Git;
use crate::plan::{Plan, PlanKind, Task};
use crate::reviewer::{self, Verdict};
use crate::{brief, exit, lint, memcli, ownership, paths, plan, repo, sys, warn};

pub const PENDING: &str = "pending";
pub const DISPATCHED: &str = "dispatched";
/// Fast-forwarded onto integration and green, with a reader over the diff:
/// the merge is recorded when the reading says ship.
pub const REVIEWING: &str = "reviewing";
pub const MERGED: &str = "merged";
pub const FAILED: &str = "failed";
pub const BLOCKED: &str = "blocked";
pub const DONE_PREVIOUSLY: &str = "done-previously";

/// How many consecutive polls [`Run::question_open`] tolerates a question's
/// id being absent from mem's listing, for the reindex lag, before treating
/// the id as one mem will never list.
const QUESTION_MISS_LIMIT: u32 = 3;

fn env_str(key: &str, default: &str) -> String {
    match std::env::var(key) {
        Ok(v) if !v.is_empty() => v,
        _ => default.to_string(),
    }
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// One live orchestrator per run directory (friction #V6KDQM3S). The lock
/// rides the returned file: dropping it releases the run, and a holder that
/// dies releases it with its fds, so there is nothing stale to clean up.
/// `None` means another orchestrator is live in this run right now.
pub fn lock_run(dir: &Path) -> Option<std::fs::File> {
    let f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(dir.join("lock"))
        .ok()?;
    f.try_lock().ok().map(|()| f)
}

/// One task's files in the run directory.
fn field(dir: &Path, task: &str, ext: &str) -> String {
    std::fs::read_to_string(dir.join(format!("{task}.{ext}")))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// A run-level file `setup` wrote before the first worker went out -- `model`
/// or `review-model` -- read back for a run `reap` is rebuilding. `None`
/// means the file was never written: a run dir from before this, or a
/// fixture that never went through `setup`.
fn recorded(dir: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(name))
        .ok()
        .map(|v| v.trim().to_string())
}

/// One task's field, and a word if it could not be written. The run dir is
/// the run's whole memory of a task: a state write that fails leaves the
/// coordinator reading `pending` for a task it has just dispatched, and it
/// then reports the task as never started and skips everything behind it.
/// That has happened once, silently (t053 under a loaded machine), which is
/// the argument for saying so rather than for `let _ =`.
fn write_field(dir: &Path, task: &str, ext: &str, value: &str) {
    let path = dir.join(format!("{task}.{ext}"));
    if let Err(e) = std::fs::write(&path, format!("{value}\n")) {
        warn(format!(
            "task {task}: cannot write {} ({e}) -- this run's account of it is now unreliable",
            path.display()
        ));
    }
}

/// Up to three failing checks named out of a verify run's combined output,
/// so a gate failure says what broke instead of just that something did.
/// TAP's `not ok` lines are named first, when the suite speaks TAP; lines
/// holding `FAILED` are next, cargo test's per-test ones trimmed to the bare
/// name; a suite in neither format still gives up its last three nonblank
/// lines, oldest first, which are usually the ones that say why. The gate's
/// own `workflow: `-prefixed lines are dropped before any of that: the child
/// is `workflow verify --gate` itself, and its `verify project: FAILED`
/// diagnostic would otherwise satisfy the `FAILED` branch for every suite
/// that names nothing in that shape of its own -- PHPUnit, jest, `go test`
/// -- shadowing the last-three-lines fallback that would have shown the
/// real failure (review 3).
pub fn failing_checks(output: &str) -> Vec<String> {
    let output: String = output
        .lines()
        .filter(|l| !l.trim_start().starts_with("workflow: "))
        .collect::<Vec<_>>()
        .join("\n");
    let output = output.as_str();

    let tap: Vec<String> = output
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("not ok"))
        .map(str::to_string)
        .collect();
    if !tap.is_empty() {
        return tap.into_iter().take(3).collect();
    }

    let cargo: Vec<String> = output
        .lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("test ")?;
            let name = rest.strip_suffix("FAILED")?.trim();
            let name = name.strip_suffix("...").unwrap_or(name).trim();
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect();
    if !cargo.is_empty() {
        return cargo.into_iter().take(3).collect();
    }

    let failed: Vec<String> = output
        .lines()
        .map(str::trim)
        .filter(|l| l.contains("FAILED"))
        .map(str::to_string)
        .collect();
    if !failed.is_empty() {
        return failed.into_iter().take(3).collect();
    }

    let mut last: Vec<String> = output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .rev()
        .take(3)
        .map(str::to_string)
        .collect();
    last.reverse();
    last
}

/// Liveness is the latest of three signals, because each one alone has a way of
/// going quiet on a worker that is fine: a long test run writes no transcript
/// line, an out-of-tree `CARGO_TARGET_DIR` flattens the worktree's mtime, and a worker
/// that is only thinking touches neither. The status file is the worker's own
/// heartbeat and it lives in the run directory, not the worktree, so it has to
/// be counted separately (review-3 F-10). A worker the usage limit has paused
/// touches none of the three either -- this is the same clock [`Run::paused`]
/// is held to, rather than being judged dead the moment `alive` goes false.
pub fn last_activity(backend: &dyn WorkerBackend, dir: &Path, wt_root: &Path, task: &str) -> i64 {
    let h = Handle {
        session: field(dir, task, "session"),
        pidfile: dir.join(format!("{task}.pid")),
        worktree: wt_root.join(task),
    };
    backend
        .last_activity(&h)
        .max(sys::mtime(&dir.join(format!("{task}.status"))))
}

/// Has nothing moved for the whole deadline?
pub fn stalled(
    backend: &dyn WorkerBackend,
    dir: &Path,
    wt_root: &Path,
    task: &str,
    deadline_s: i64,
) -> bool {
    let now = sys::now();
    let mut last = last_activity(backend, dir, wt_root, task);
    let started: i64 = field(dir, task, "dispatched_at").parse().unwrap_or(0);
    if last <= started {
        last = if started > 0 { started } else { now };
    }
    now - last >= deadline_s
}

/// What the gate made of a ready worker's branch.
enum Merge {
    /// Rebased, fast-forwarded, verified, read if a reader was named, and
    /// recorded.
    Landed,
    /// A ready worker with nothing to merge: its Done was already satisfied
    /// in the tree it opened onto (friction #B2D8SJKR).
    Nothing,
    /// Fast-forwarded and verified, and a reader has the diff. The merge is
    /// recorded when the reading says ship, or unwound when it says fix.
    Reading,
}

pub struct Run {
    pub plan: Plan,
    /// The file the plan was read from, when it came from one: where a merge
    /// is ticked off for a plan mem may never have seen.
    pub plan_file: Option<PathBuf>,
    pub repo: PathBuf,
    /// The project's name as one path component: it names the directories
    /// under the run, worktree, brief and cargo roots.
    pub project: String,
    pub dir: PathBuf,
    pub wt_root: PathBuf,
    pub brief_dir: PathBuf,
    pub base: String,
    pub int_branch: String,
    pub int_wt: PathBuf,
    pub deadline_s: i64,
    pub kill_grace_s: i64,
    pub poll: f64,
    pub max_workers: usize,
    pub backend: Box<dyn WorkerBackend>,
    /// What every worker of this run is started on: `WORKFLOW_MODEL` for
    /// one run, else the project's `mem project set model`, else opus.
    pub model: String,
    /// Who reads each task's diff at the gate once its verify is green:
    /// `WORKFLOW_REVIEW_MODEL` for one run (empty turns the reading off),
    /// else the project's `mem project set review-model` (`none` turns it
    /// off), else nobody.
    /// Naming the model the workers run on is the same as naming nobody.
    pub review_model: Option<String>,
    /// Raised by SIGTERM, SIGINT or SIGHUP. The poll loop reads it between
    /// passes, and the stop takes a reader down with the workers.
    pub stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub env: Vec<(String, String)>,
    /// Set by `workflow reap`, which collects for a run that is gone. A
    /// worker it started would have no run watching it, so where a run
    /// gives a task one more try, reap fails it and names who tries it next
    /// (friction #F6MR6AMH).
    pub collecting: bool,
    made: Vec<PathBuf>,
}

impl Run {
    fn git(&self) -> Git {
        Git::at(&self.repo)
    }

    fn field(&self, task: &str, ext: &str) -> String {
        field(&self.dir, task, ext)
    }

    fn state(&self, task: &str) -> String {
        self.field(task, "state")
    }

    fn set_state(&self, task: &str, state: &str) {
        write_field(&self.dir, task, "state", state);
        if state == MERGED {
            // A failure note used to outlive its failure: status went on
            // reporting why a task failed on one run long after another had
            // merged it (friction #BHPS3G7D).
            write_field(&self.dir, task, "failed", "");
        }
    }

    fn branch(&self, task: &str) -> String {
        format!("{}/{}", self.plan.plan_id, task)
    }

    /// How mem tags a question this task's worker asks: `<plan>/<task>`,
    /// which mem reads off the worktree path and `WORKFLOW_TASK` alike.
    fn task_tag(&self, task: &str) -> String {
        format!("{}/{}", self.plan.plan_id, task)
    }

    /// The task as the plan of record has it now -- mem's plan, or the file
    /// this run was handed -- falling back to what the run parsed at start.
    ///
    /// The plan used to be frozen at run start, so an orchestrator answering
    /// "widen the Files line" by editing the plan changed nothing for the
    /// live run: the redispatched worker was handed the old block and the
    /// gate held it to the old patterns, and the same question came round
    /// again (questions #QT76088P, #NS88MTQF). Read fresh at dispatch and at
    /// the gate, an edit to the plan of record is the whole correction.
    fn task_now(&self, id: &str) -> Option<Task> {
        self.plan_text()
            .and_then(|text| plan::parse(&text, true))
            .filter(|p| p.plan_id == self.plan.plan_id)
            .and_then(|p| p.get(id).cloned())
            .or_else(|| self.plan.get(id).cloned())
    }

    /// The plan of record as it reads right now: the `--plan-file`, else
    /// mem's plan. What `task_now` parses and what the reviewer is handed.
    fn plan_text(&self) -> Option<String> {
        match self.plan_file.as_deref() {
            Some(file) => std::fs::read_to_string(file).ok(),
            None => memcli::plan(),
        }
    }

    /// The question this task's worker is waiting on, if its failure note
    /// names one: `asked #<short id>: ...`.
    fn asked(&self, task: &str) -> Option<String> {
        let note = self.field(task, "failed");
        let rest = note.strip_prefix("asked #")?;
        let id: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        (!id.is_empty()).then_some(id)
    }

    /// The FAILED tasks of this wave still waiting on a question with no
    /// answer: the wave loop keeps polling for them rather than closing the
    /// wave and leaving the answer nowhere to land.
    fn waiting(&self, wave: &[String]) -> Vec<String> {
        wave.iter()
            .filter(|id| self.state(id) == FAILED)
            .filter(|id| self.question_open(id))
            .cloned()
            .collect()
    }

    /// Whether `task`'s failure note names a question that still holds its
    /// wave open: mem lists it for the task and it carries no answer yet.
    /// An id mem never lists for this task -- a friction id quoted in the
    /// same blocked line, another task's or project's question, the same
    /// eight-character shape -- is not an unanswered question: no answer
    /// can ever land on it. It gets the reindex lag [`Self::question_in`]
    /// describes -- a few polls where a real question is briefly missing
    /// -- but past that bound it stops holding the wave.
    fn question_open(&self, task: &str) -> bool {
        let Some(qid) = self.asked(task) else {
            return false;
        };
        match memcli::questions_for(&self.task_tag(task))
            .into_iter()
            .find(|q| q.short_id == qid)
        {
            Some(q) => {
                write_field(&self.dir, task, "qmiss", "");
                q.answer.is_none()
            }
            None => {
                let key = format!("{qid} ");
                let misses: u32 = self
                    .field(task, "qmiss")
                    .strip_prefix(&key)
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0)
                    + 1;
                write_field(&self.dir, task, "qmiss", &format!("{key}{misses}"));
                misses <= QUESTION_MISS_LIMIT
            }
        }
    }

    /// The failure note for a worker that stopped on a question, as
    /// `asked #<id>: <what>`, or nothing when its `blocked` line names no
    /// question and mem lists none pending for the task.
    fn question_in(&self, task: &str, note: &str) -> Option<String> {
        let named = note
            .split(|c: char| c.is_whitespace() || c == ',' || c == ';' || c == ')')
            .filter_map(|w| w.strip_prefix('#'))
            .map(|w| w.trim_end_matches(|c: char| !c.is_ascii_alphanumeric()))
            .find(|w| w.len() == 8 && w.chars().all(|c| c.is_ascii_alphanumeric()));
        let listed = memcli::questions_for(&self.task_tag(task));
        if let Some(id) = named {
            let what = listed
                .iter()
                .find(|q| q.short_id == id)
                .map(|q| q.title.clone())
                .unwrap_or_else(|| note.to_string());
            return Some(format!("asked #{id}: {what}"));
        }
        listed
            .into_iter()
            .find(|q| q.answer.is_none())
            .map(|q| format!("asked #{}: {}", q.short_id, q.title))
    }

    /// Where a merged task's commit is recorded. Outside refs/heads on purpose:
    /// the task branch is checked out in the task worktree, so `branch -f` on it
    /// can never succeed, and the branch is deleted at run end anyway. A ref of
    /// its own survives that deletion and keeps the commit reachable, which is
    /// how a later run can tell whether a `[x]` task's work is really on the
    /// integration branch.
    fn task_ref(&self, task: &str) -> String {
        format!("refs/workflow/{}/{}", self.plan.plan_id, task)
    }

    fn worktree(&self, task: &str) -> PathBuf {
        self.wt_root.join(task)
    }

    /// Where `who` (a task, or the gate as "integration") builds a Rust
    /// project. One target dir per builder, never shared and always set: a
    /// shared dir let one task's suite drive another task's binary, and left
    /// binaries whose baked-in paths pointed at reaped worktrees (frictions
    /// #MQRKM0AD, #TFVWXXDQ). An inherited CARGO_TARGET_DIR is overridden for
    /// the same reason. Cleanup takes the whole plan's dirs down with the
    /// worktrees.
    fn cargo_root(&self) -> PathBuf {
        paths::state_home()
            .join("workflow/cargo")
            .join(&self.project)
            .join(&self.plan.plan_id)
    }

    /// Unconditional: a Cargo.toml at the repo root is not where every Rust
    /// project keeps one, and a builder that turns out not to need this is no
    /// worse off for having it.
    fn cargo_env(&self, who: &str) -> Option<(String, String)> {
        let dir = self.cargo_root().join(who);
        let _ = std::fs::create_dir_all(&dir);
        Some(("CARGO_TARGET_DIR".into(), dir.to_string_lossy().to_string()))
    }

    /// The pid the template wrote, digits only: an empty answer means the
    /// dispatch never got that far.
    fn worker_pid(&self, task: &str) -> String {
        self.field(task, "pid")
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect()
    }

    fn handle(&self, task: &str) -> Handle {
        Handle {
            session: self.field(task, "session"),
            pidfile: self.dir.join(format!("{task}.pid")),
            worktree: self.worktree(task),
        }
    }

    fn dispatched(&self) -> Vec<String> {
        self.plan
            .ids()
            .into_iter()
            .filter(|t| self.state(t) == DISPATCHED)
            .collect()
    }

    fn running(&self) -> usize {
        self.dispatched().len()
    }

    /// Tasks whose diff a reader is going over right now. At most one: the
    /// gate is serialized, and integration holds that task's fast-forward
    /// unrecorded until the reading ends.
    fn reviewing(&self) -> Vec<String> {
        self.plan
            .ids()
            .into_iter()
            .filter(|t| self.state(t) == REVIEWING)
            .collect()
    }

    /// The last line the worker reported in its own status file, as
    /// (state, note). Lines read `<utc> <state> <note...>`.
    fn last_status_line(&self, task: &str) -> Option<(String, String)> {
        let text = std::fs::read_to_string(self.dir.join(format!("{task}.status"))).ok()?;
        let mut last = None;
        for line in text.lines() {
            let mut fields = line.split_whitespace();
            let (Some(_utc), Some(state)) = (fields.next(), fields.next()) else {
                continue;
            };
            let (state, head) = split_state(state);
            let rest = fields.collect::<Vec<_>>().join(" ");
            let note = match (head.is_empty(), rest.is_empty()) {
                (true, _) => rest,
                (false, true) => head,
                (false, false) => format!("{head} {rest}"),
            };
            last = Some((state, note));
        }
        last
    }

    /// The commits this task wrote, which is not the same as the commits on its
    /// branch: a branch caught up to the integration branch, or one whose
    /// worker took that branch in, carries its siblings' commits too. Measured
    /// against integration, so the answer is this task's own work.
    ///
    /// The base is the fallback for the one moment integration does not exist
    /// yet -- a run refused in preflight before it made the branch.
    fn commits(&self, task: &str) -> u64 {
        let git = self.git();
        let anchor = match git.rev_parse_commit(&self.int_branch) {
            Some(_) => self.int_branch.clone(),
            None => self.base.clone(),
        };
        git.count(&format!("{anchor}..{}", self.branch(task)))
    }

    /// The integration commit a task's worktree is brought up to: the branch
    /// tip, except while a reading holds a fast-forward on it that nothing
    /// has recorded -- then the commit before it, so no worker builds on
    /// work the reader may yet send back.
    fn integration_tip(&self) -> Option<String> {
        for task in self.reviewing() {
            if let Some((prev, _)) = self.pending_merge(&task) {
                return Some(prev);
            }
        }
        self.git().rev_parse_commit(&self.int_branch)
    }

    /// The commit some run merged for this task, or empty.
    fn prior_sha(&self, task: &str) -> String {
        self.git()
            .rev_parse_commit(&self.task_ref(task))
            .unwrap_or_else(|| self.field(task, "merged"))
    }

    /// Is that commit on the integration branch as it stands?
    ///
    /// The tip a refused preflight remembered counts too. Its recipe ends
    /// "merge it or delete it", and once that merge lands on the trunk the
    /// branch is gone -- a task that never reached the gate has no task ref
    /// either, so without this note nothing records that the work is here and
    /// a worker is sent to build it again (friction #GPC1PVZJ).
    fn landed(&self, task: &str) -> bool {
        let git = self.git();
        [self.prior_sha(task), self.field(task, "leftover")]
            .iter()
            .any(|sha| !sha.is_empty() && git.is_ancestor(sha, &self.int_branch))
    }

    fn alive(&self, task: &str) -> bool {
        self.backend.alive(&self.handle(task))
    }

    /// A failed task whose branch holds commits is not gone: the gate itself
    /// can be why it failed, and the work is still there to build on (ruling
    /// 4). Nothing else counts -- a task a run left `dispatched` when it died
    /// has no verdict on it at all, and is adopted, not resumed.
    pub fn resumable(&self, task: &str) -> bool {
        self.state(task) == FAILED && self.commits(task) > 0
    }

    /// Nothing anywhere says this session ever ran: the backend has no record
    /// of it, the worker wrote no status line and no result, and the branch
    /// has no commits. Only adoption asks -- at dispatch time the same silence
    /// means still launching, and the stall deadline decides.
    fn ghost(&self, task: &str) -> bool {
        !self.backend.seen(&self.handle(task))
            && self.field(task, "status").is_empty()
            && std::fs::metadata(self.dir.join(format!("{task}.json")))
                .map(|m| m.len() == 0)
                .unwrap_or(true)
            && self.commits(task) == 0
    }

    fn stalled(&self, task: &str) -> bool {
        stalled(
            self.backend.as_ref(),
            &self.dir,
            &self.wt_root,
            task,
            self.deadline_s,
        )
    }

    /// Seen by the backend and not alive, with nothing that says it is
    /// done: no commit on its branch and no final word past `started` or
    /// `progress` in its status file. The usage limit pauses a session
    /// without ending it -- `claude agents` shows it idle, not gone -- and
    /// this is what tells that apart from a worker that actually finished
    /// or died, so it is held to the stall deadline like a live one instead
    /// of being collected the instant `alive` goes false (friction
    /// #17SPEY7R).
    ///
    /// A pidfile answers this on its own: the legacy template's process is
    /// either running or it is not, and a dead one is dead, not idle. Only a
    /// session with no pid to check -- the shipped `--bg` dispatch -- has a
    /// listing that can lie this way.
    fn paused(&self, task: &str) -> bool {
        if !self.worker_pid(task).is_empty() {
            return false;
        }
        if !self.backend.seen(&self.handle(task)) || self.alive(task) {
            return false;
        }
        if self.commits(task) != 0 {
            return false;
        }
        match self.last_status_line(task) {
            None => true,
            Some((state, _)) => state == "started" || state == "progress",
        }
    }

    fn stop(&self, task: &str) {
        self.backend.stop(&self.handle(task), self.kill_grace_s);
    }

    // ---------------------------------------------------------------- workers

    /// Bring the task's worktree up to the integration branch before the worker
    /// sees it.
    ///
    /// Every worktree is cut from the run's base at setup, so a task whose
    /// dependency merged in an earlier wave opens onto a tree without it
    /// (friction #VC7PAESB). The worker's only way out was to go and find the
    /// integration branch itself, which nothing in its brief mentions. This
    /// fast-forward puts the work it builds on simply there.
    ///
    /// Fast-forward only. A redispatch after a failure has commits of its own
    /// on the branch, and rebasing them mid-run to catch up is a decision for
    /// the worker who can read the conflict, not for a silent step before it
    /// wakes.
    fn catch_up(&self, task: &str) {
        let wt = self.worktree(task);
        if !wt.is_dir() {
            return;
        }
        let git = Git::at(&wt);
        let Some(tip) = self.integration_tip() else {
            return;
        };
        if git.head().as_deref() == Some(tip.as_str()) {
            return; // already there, and on wave one it always is
        }
        if !git.quiet(&["merge", "-q", "--ff-only", &tip]) {
            warn(format!(
                "task {task}: its worktree keeps the commits it already has, so {} was not brought in",
                self.int_branch
            ));
        }
    }

    /// What the attempt before this one came to, for the brief to carry.
    ///
    /// `why` is what this caller knows and the run dir does not: a stall and a
    /// ghost session are both redispatches nothing has marked failed, so the
    /// failure note is empty and only the caller can say what happened.
    /// Everything else is read here, before the dispatch truncates it
    /// (friction #YCW7ND6Z).
    fn prior_attempt(&self, task: &str, why: &str) -> brief::Prior {
        let attempts: u64 = self.field(task, "dispatches").parse().unwrap_or(0);
        if attempts == 0 {
            return brief::Prior::default();
        }
        let why = match why.is_empty() {
            true => self.field(task, "failed"),
            false => why.to_string(),
        };
        let last_report = self
            .last_status_line(task)
            .map(|(state, note)| match note.is_empty() {
                true => state,
                false => format!("{state}: {note}"),
            })
            .unwrap_or_default();
        // What the worker asked and what the orchestrator said, newest first
        // and at most two: the brief's budget is the limit, and the latest
        // exchange is the one this attempt exists to act on.
        let answers = memcli::questions_for(&self.task_tag(task))
            .into_iter()
            .filter_map(|q| q.answer.map(|a| (q.body, a)))
            .take(2)
            .collect();
        brief::Prior {
            attempts,
            why,
            last_report,
            commits: self.commits(task),
            answers,
        }
    }

    /// One more try for a task nobody has really attempted -- under a run,
    /// which watches what it starts. Under `workflow reap` there is no run:
    /// reap starts nothing, and the task fails with a line saying the next
    /// run in this checkout retries it, which a fresh run does by itself.
    fn once_more(&self, task: &str, what: &str, after: &str) {
        if self.collecting {
            self.fail_task(
                task,
                &format!(
                    "{what}; reap starts nothing, and the next run in this checkout tries it again"
                ),
            );
            return;
        }
        warn(format!("task {task}: {what} -- one more try"));
        self.dispatch(task, after);
    }

    fn dispatch(&self, task: &str, after: &str) {
        let Some(t) = self.task_now(task) else {
            return;
        };
        self.catch_up(task);
        let wt = self.worktree(task);
        let brief_file = self.brief_dir.join(format!("{task}.md"));
        let status = self.dir.join(format!("{task}.status"));
        let session = self.backend.mint_session();
        let prior = self.prior_attempt(task, after);

        write_field(&self.dir, task, "session", &session);
        // Truncated, not appended: the gate reads this file to judge THIS
        // attempt, and a stale `ready` from the last one would pass for it.
        // What it said lives on in the brief instead.
        let _ = std::fs::write(&status, "");
        for ext in ["json", "err", "pid"] {
            let _ = std::fs::remove_file(self.dir.join(format!("{task}.{ext}")));
        }
        brief::write(&t, &wt, &status, &prior, &brief_file);
        // The gate reads this from inside the worktree: the task is held to its
        // own Verify command there, not to the repo-wide suite (verify.rs).
        write_field(&self.dir, task, "verify", t.verify.as_deref().unwrap_or(""));

        let n = prior.attempts;
        write_field(&self.dir, task, "dispatches", &(n + 1).to_string());
        write_field(&self.dir, task, "dispatched_at", &sys::now().to_string());

        let mut env = self.env.clone();
        env.extend(self.cargo_env(task));
        // How a worker's `mem ask` knows it is a worker's: mem reads this, or
        // the worktree path, and addresses the question to the orchestrator.
        env.push(("WORKFLOW_TASK".into(), self.task_tag(task)));
        let d = Dispatch {
            task: task.to_string(),
            worktree: wt,
            brief: brief_file,
            out: self.dir.join(format!("{task}.json")),
            err: self.dir.join(format!("{task}.err")),
            pidfile: self.dir.join(format!("{task}.pid")),
            status,
            rundir: self.dir.clone(),
            session: session.clone(),
            model: self.model.clone(),
            turns: env_str("WORKFLOW_MAX_TURNS", "120"),
            env,
        };

        // Recorded before the worker exists, so a run that dies between here
        // and the next line leaves a task that is plainly mid-dispatch rather
        // than one that looks untouched.
        self.set_state(task, DISPATCHED);
        let handle = self.backend.dispatch(&d);
        // What the backend actually started, which on `claude --bg` is not
        // what was minted. Everything that asks after this worker later --
        // liveness, the stop, the transcript -- reads this file.
        if !handle.is_empty() {
            write_field(&self.dir, task, "session", &handle);
        }
        warn(format!(
            "task {task}: dispatched (session {})",
            if handle.is_empty() { &session } else { &handle }
        ));
        memcli::log(&format!("run {}: dispatched {task}", self.plan.plan_id));
    }

    /// What this attempt was carrying when it stopped, kept for the run's
    /// closing report. Feedback on how the plan was cut, never a ceiling
    /// (ruling #D7A4T2CH): the plan is flat-rate and context is the resource
    /// one-task-per-session already manages.
    fn record_context(&self, task: &str) {
        if let Some(tokens) = self.backend.context_tokens(&self.handle(task)) {
            write_field(&self.dir, task, "context", &tokens.to_string());
        }
    }

    // ------------------------------------------------------------ merge gate

    /// Serialized, one task at a time, and in this order: ownership, then the
    /// words, then rebase onto the integration branch, and only then verify --
    /// verifying before the rebase lets a semantic conflict land green
    /// (review-3 F-6).
    ///
    /// The answer says what became of the branch; see [`Merge`].
    fn merge(&self, task: &str) -> Result<Merge, String> {
        let branch = self.branch(task);
        let wt = self.worktree(task);

        // Before anything else: did this task's merge already land? The
        // fast-forward and the verify are two steps, and a coordinator killed
        // between them leaves integration advanced with the task still reading
        // dispatched. Coming back in from there, the branch has nothing
        // integration lacks, so this pass would rebase commits that are already
        // applied and call the answer a conflict (friction #DM877DNV). The
        // intent line written below says which commit was going on, which is
        // what tells an applied merge from a real one.
        if let Some((prev, new)) = self.pending_merge(task)
            && self.git().is_ancestor(&new, &self.int_branch)
        {
            return self.settle_interrupted_merge(task, &prev, &new);
        }

        if self.commits(task) == 0 {
            return Ok(Merge::Nothing);
        }

        let patterns = ownership::split_patterns(
            self.task_now(task)
                .and_then(|t| t.files)
                .as_deref()
                .unwrap_or(""),
        );
        // Anchored on the integration branch, not the run's base: what this
        // task owns is what it wrote, never what a sibling merged while it
        // worked (friction #A2JXGNB8).
        let bad = ownership::violations(&wt, &self.int_branch, &branch, &patterns);
        if !bad.is_empty() {
            warn(format!("task {task}: touched files it does not own --"));
            for line in ownership::show(&bad) {
                warn(format!("  {line}"));
            }
            return Err("wrote outside its Files: patterns".into());
        }

        // The same anchor again: a branch that took integration in to reach a
        // dependency would otherwise be held to its siblings' commit messages
        // as well as its own.
        let msgs = self
            .git()
            .out(&[
                "log",
                "--format=%B",
                &format!("{}..{branch}", self.int_branch),
            ])
            .unwrap_or_default();
        if !lint::lint_text(&msgs) {
            return Err("a commit message did not pass lint-msg".into());
        }

        let int = Git::at(&self.int_wt);
        let prev = int.head().unwrap_or_default();
        if !int.quiet(&["checkout", "-q", "--detach", &branch]) {
            int.quiet(&["checkout", "-q", &self.int_branch]);
            return Err(format!("cannot check out {branch} for the rebase"));
        }
        // Replay what the branch has that integration does not, which is the
        // task's own work whether or not it took integration in along the way.
        // Against the run's base it would replay the siblings' commits too and
        // lean on patch-id dedup to drop them again.
        if !int.quiet(&["rebase", &self.int_branch]) {
            int.quiet(&["rebase", "--abort"]);
            int.quiet(&["checkout", "-q", &self.int_branch]);
            return Err("conflicts with the integration branch".into());
        }
        let new = int.head().unwrap_or_default();
        if !int.quiet(&["checkout", "-q", &self.int_branch]) {
            return Err(format!("cannot return to {}", self.int_branch));
        }
        // Written before the branch moves, not after: the bookkeeping that
        // says a merge is done still waits on the verify, but a crash from
        // here on leaves a record of what was in flight.
        write_field(&self.dir, task, "merging", &format!("{prev} {new}"));
        if !int.quiet(&["merge", "-q", "--ff-only", &new]) {
            write_field(&self.dir, task, "merging", "");
            return Err("the rebased branch does not fast-forward onto integration".into());
        }

        match self.gate(task, &prev, &new) {
            Err(why) => {
                self.unwind(task, &prev);
                return Err(why);
            }
            Ok(true) => return Ok(Merge::Reading),
            Ok(false) => {}
        }

        self.record_merged(task, &new);
        Ok(Merge::Landed)
    }

    /// The merge this task was in the middle of when its coordinator died, as
    /// (integration before, the commit that was going on).
    fn pending_merge(&self, task: &str) -> Option<(String, String)> {
        let line = self.field(task, "merging");
        let (prev, new) = line.split_once(' ')?;
        (!new.is_empty()).then(|| (prev.to_string(), new.to_string()))
    }

    fn record_merged(&self, task: &str, new: &str) {
        if !self.git().quiet(&["update-ref", &self.task_ref(task), new]) {
            warn(format!(
                "task {task}: could not record {new} as the commit that landed"
            ));
        }
        write_field(&self.dir, task, "merged", new);
        write_field(&self.dir, task, "merging", "");
    }

    /// A merge whose fast-forward landed and whose verify never ran. The work
    /// is on the branch already, so there is nothing to replay -- but nothing
    /// has vouched for it either, and the gate is the whole point.
    fn settle_interrupted_merge(&self, task: &str, prev: &str, new: &str) -> Result<Merge, String> {
        warn(format!(
            "task {task}: its merge reached {} before the run died -- verifying it now",
            self.int_branch
        ));
        let why = match self.gate(task, prev, new) {
            Ok(true) => return Ok(Merge::Reading),
            Ok(false) => {
                self.record_merged(task, new);
                return Ok(Merge::Landed);
            }
            Err(why) => why,
        };
        // Unwind only what nothing was built on. A later run may have merged
        // other tasks on top, and taking those down with this one would be a
        // worse answer than a red branch and a person told why.
        if Git::at(&self.int_wt).head().as_deref() == Some(new) {
            self.unwind(task, prev);
            return Err(why);
        }
        Err(format!(
            "its merge landed before the run died, {} refuses it now ({why}), and other work sits on top of it",
            self.int_branch
        ))
    }

    /// The two readings a fast-forwarded merge faces before it is recorded:
    /// the suite, then the reviewer. `Err` is the reason the caller resets
    /// integration to `prev` and fails the task with. `Ok(true)` means a
    /// reader now has the diff and the merge waits on its verdict;
    /// `Ok(false)` means nobody reads here and the merge is final.
    fn gate(&self, task: &str, prev: &str, new: &str) -> Result<bool, String> {
        self.gate_verify(task)?;
        Ok(self.start_review(task, prev, new))
    }

    /// Integration back to where it stood before this task's fast-forward,
    /// and the intent line cleared: the merge did not happen.
    fn unwind(&self, task: &str, prev: &str) {
        Git::at(&self.int_wt).quiet(&["reset", "-q", "--hard", prev]);
        write_field(&self.dir, task, "merging", "");
    }

    /// The reader (plan gate-reviewer). Verify proves what a test can reach;
    /// a model in a clean context reads the diff against the plan of record
    /// and the task's Done line and says ship or fix. Nobody named means no
    /// reading, which by the time a task gets here means a run that was told
    /// so on the way in (see [`refused`]). `fix` leaves the findings in
    /// `<task>.review`, which the failure note names and the redispatched
    /// worker's brief repeats.
    ///
    /// The reader is dispatched like a worker, through the project's backend,
    /// so it is a session Saiful can watch and attach to -- never print mode.
    /// It works in the integration worktree, writes one answer file and ends;
    /// a reading that changed the tree is void.
    ///
    /// Started here and judged by [`Run::review_pass`] from the poll loop,
    /// never waited for: a reading may run for its whole deadline, and a
    /// loop blocked on it dispatched nothing, stopped no stalled worker and
    /// heard no signal meanwhile. The task sits `reviewing` in between, its
    /// `merging` intent line still naming the fast-forward that waits.
    /// Answers whether a reading began.
    fn start_review(&self, task: &str, prev: &str, new: &str) -> bool {
        let Some(model) = self.review_model.as_deref() else {
            return false;
        };
        let Some(t) = self.task_now(task) else {
            return false;
        };
        let plan_text = self.plan_text().unwrap_or_default();
        let int = Git::at(&self.int_wt);
        let range = format!("{prev}..{new}");
        let diff = int.out(&["diff", &range]).unwrap_or_default();
        let stat = int.out(&["diff", "--stat", &range]).unwrap_or_default();
        let prompt = self.dir.join(format!("{task}.review-prompt"));
        let answer = self.dir.join(format!("{task}.review"));
        let _ = std::fs::write(
            &prompt,
            reviewer::prompt(&plan_text, &t, &diff, &stat, &self.int_wt, &answer),
        );
        write_field(&self.dir, task, "review-tries", "0");
        warn(format!("task {task}: {model} is reading the diff"));
        self.read_start(task);
        true
    }

    /// One reading: a worker dispatch in the integration worktree, off the
    /// prompt `start_review` wrote. Counted in `review-tries`, stamped in
    /// `review-started`, its handle in `review-session`; `review_pass` reads
    /// all three.
    fn read_start(&self, task: &str) {
        let Some(model) = self.review_model.as_deref() else {
            return;
        };
        let name = format!("{task}-review");
        let pidfile = self.dir.join(format!("{task}.review-pid"));
        let out = self.dir.join(format!("{task}.review-out"));
        let _ = std::fs::remove_file(self.dir.join(format!("{task}.review")));
        let _ = std::fs::remove_file(&pidfile);
        let _ = std::fs::write(&out, "");
        let mut env = self.env.clone();
        env.push((
            "WORKFLOW_TASK".into(),
            format!("{}/{name}", self.plan.plan_id),
        ));
        let d = Dispatch {
            task: name,
            worktree: self.int_wt.clone(),
            brief: self.dir.join(format!("{task}.review-prompt")),
            out,
            err: self.dir.join(format!("{task}.review-err")),
            pidfile,
            status: self.dir.join(format!("{task}.review-status")),
            rundir: self.dir.clone(),
            session: self.backend.mint_session(),
            model: model.to_string(),
            turns: env_str("WORKFLOW_MAX_TURNS", "120"),
            env,
        };
        let tries: u64 = self.field(task, "review-tries").parse().unwrap_or(0);
        write_field(&self.dir, task, "review-tries", &(tries + 1).to_string());
        write_field(&self.dir, task, "review-started", &sys::now().to_string());
        let session = self.backend.dispatch(&d);
        write_field(&self.dir, task, "review-session", &session);
    }

    fn review_handle(&self, task: &str) -> Handle {
        Handle {
            session: self.field(task, "review-session"),
            pidfile: self.dir.join(format!("{task}.review-pid")),
            worktree: self.int_wt.clone(),
        }
    }

    /// Every reading in flight, judged if it has ended.
    fn review_passes(&self) {
        for task in self.reviewing() {
            self.review_pass(&task);
        }
    }

    /// The reading in flight, judged once it ends. `true` when the task is
    /// settled either way -- merged on ship, failed on fix or on a reading
    /// that could not be had twice -- and `false` while the reader is still
    /// going or has just been sent again.
    fn review_pass(&self, task: &str) -> bool {
        let Some((prev, new)) = self.pending_merge(task) else {
            self.fail_task(task, "the reading lost the record of what it was reading");
            return true;
        };
        let answer = self.dir.join(format!("{task}.review"));
        let h = self.review_handle(task);
        let started: i64 = self.field(task, "review-started").parse().unwrap_or(0);
        let waited = sys::now() - started;
        let deadline_s = reviewer::deadline_s();
        // Gone with an answer is the clean end. Gone without one within the
        // first moments is a dispatch still coming up, not an ending. The
        // deadline below is an ending too: a session stopped there has been
        // talking for the whole wait and has last words of its own, same as
        // one that simply exited without a verdict.
        let outcome = if !self.backend.alive(&h) && (answer.exists() || waited >= 5) {
            self.judge_reading(&new, &answer)
        } else if waited >= deadline_s {
            self.backend.stop(&h, self.kill_grace_s);
            Err(format!(
                "the review ran past its {deadline_s} second deadline and was stopped"
            ))
        } else {
            return false;
        };
        self.moot_reader_questions(task);
        match outcome {
            Ok(Verdict::Ship) => {
                warn(format!("task {task}: the reviewer says ship"));
                self.record_merged(task, &new);
                self.land(task);
            }
            Ok(Verdict::Fix) => {
                let n = self.field(task, "reviews").parse::<u64>().unwrap_or(0) + 1;
                write_field(&self.dir, task, "reviews", &n.to_string());
                self.unwind(task, &prev);
                self.fail_task(
                    task,
                    &format!(
                        "the reviewer wants fixes first (review {n}) -- read {}",
                        answer.display()
                    ),
                );
            }
            // A reading that did not happen is not a verdict either way: one
            // more try, and then the orchestrator is told. Unless its last
            // words say the provider itself is why -- a second reading hits
            // the same wall, so that fails the task at once.
            Err(why) => {
                // The dispatch's own stderr is already in review-err. Last
                // words are appended, never used to overwrite it: a session
                // that ran a while before the wall has ordinary text in its
                // transcript and the limit only on stderr, so either one
                // losing the other would hide the line a second reading is
                // going to meet again. A reader that never launched has no
                // transcript at all, so last_words comes back empty and
                // review-err is left as the dispatch captured it.
                let stderr_text = self.field(task, "review-err");
                let last_words = self.backend.last_words(&h);
                let combined = match (stderr_text.is_empty(), last_words.is_empty()) {
                    (true, _) => last_words.clone(),
                    (false, true) => stderr_text.clone(),
                    (false, false) => format!("{stderr_text}\n{last_words}"),
                };
                if !last_words.is_empty() {
                    write_field(&self.dir, task, "review-err", &combined);
                }
                if let Some(line) = reviewer::provider_limit(&combined) {
                    self.unwind(task, &prev);
                    self.fail_task(
                        task,
                        &format!(
                            "the reader hit a provider limit: {line} (session {})",
                            h.session
                        ),
                    );
                    return true;
                }
                let tries: u64 = self.field(task, "review-tries").parse().unwrap_or(0);
                if tries < 2 {
                    warn(format!(
                        "task {task}: {why} -- one more reading (session {})",
                        h.session
                    ));
                    self.read_start(task);
                    return false;
                }
                self.unwind(task, &prev);
                let note = if combined.is_empty() {
                    format!("{why} -- read {} (session {})", answer.display(), h.session)
                } else {
                    format!(
                        "{why} -- read {} and {} (session {})",
                        answer.display(),
                        self.dir.join(format!("{task}.review-err")).display(),
                        h.session
                    )
                };
                self.fail_task(task, &note);
            }
        }
        true
    }

    /// What a reading that has ended came to. `Err` is a reading that did
    /// not happen -- no verdict written, or a tree that is not the one it was
    /// handed -- never a judgement on the code.
    fn judge_reading(&self, new: &str, answer: &Path) -> Result<Verdict, String> {
        // The tree it read must be the tree it was handed.
        let int = Git::at(&self.int_wt);
        let dirty = int
            .out(&["status", "--porcelain"])
            .is_some_and(|s| !s.trim().is_empty());
        if dirty || int.head().as_deref() != Some(new) {
            int.quiet(&["reset", "-q", "--hard", new]);
            int.quiet(&["clean", "-fdq"]);
            return Err("the reviewer changed the tree, which voids the reading".into());
        }
        let text = std::fs::read_to_string(answer).unwrap_or_default();
        reviewer::verdict(&text).ok_or_else(|| "the review returned no verdict".to_string())
    }

    /// A reader is told never to ask, and one that asks anyway waits on
    /// nobody: the gate reads its answer file, not its questions. Whatever it
    /// asked is closed the moment its reading ends, or it sits in the
    /// orchestrator's queue for ever under a task id no plan holds.
    fn moot_reader_questions(&self, task: &str) {
        let tag = format!("{}/{task}-review", self.plan.plan_id);
        for q in memcli::questions_for(&tag) {
            if q.answer.is_none() {
                memcli::answer(
                    &q.id,
                    &format!("moot: the reading of {task} ended without waiting on it"),
                );
            }
        }
    }

    /// The bookkeeping of a merge that is final: state, tick, log.
    fn land(&self, task: &str) {
        self.set_state(task, MERGED);
        warn(format!("task {task}: merged onto {}", self.int_branch));
        self.tick_off(task);
        memcli::log(&format!("run {}: merged {task}", self.plan.plan_id));
    }

    /// verify, on the integration branch, as its own process: the same
    /// authoritative gate a human would run there. Both streams land in
    /// `<task>.gate` on every run, red or green, so a failure names what
    /// broke without asking anyone to reproduce it.
    ///
    /// A red run is run once more before it fails anybody. Suites flake, and
    /// a flake here fails a task whose work was right and sends it back for a
    /// redispatch that changes nothing; the second run decides. The red run's
    /// output moves to `<task>.gate.1` so both are on disk to compare, and a
    /// merge that took two goes says so.
    fn gate_verify(&self, task: &str) -> Result<(), String> {
        let gate_file = self.dir.join(format!("{task}.gate"));
        let kept = self.dir.join(format!("{task}.gate.1"));
        let Err(red) = self.gate_run(&gate_file) else {
            // A `.gate.1` left by an earlier merge attempt of this task would
            // read as this one having flaked, which it did not.
            let _ = std::fs::remove_file(&kept);
            return Ok(());
        };
        if std::fs::rename(&gate_file, &kept).is_err() {
            return Err(red);
        }
        self.gate_run(&gate_file)?;
        let line = format!(
            "task {task}: the gate was red once and green on the second run -- see {}",
            kept.display()
        );
        warn(&line);
        memcli::log(&line);
        Ok(())
    }

    /// One run of the gate, both streams into `file`. `Err` is the reason the
    /// merge cannot stand, naming the checks that broke out of what the run
    /// left there.
    fn gate_run(&self, file: &Path) -> Result<(), String> {
        let stdout = std::fs::File::create(file)
            .map_err(|e| format!("cannot write {} ({e})", file.display()))?;
        let stderr = stdout
            .try_clone()
            .map_err(|e| format!("cannot write {} ({e})", file.display()))?;

        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("workflow"));
        let mut c = Command::new(exe);
        c.arg("verify")
            .arg("--gate")
            .current_dir(&self.int_wt)
            .stdout(stdout)
            .stderr(stderr);
        for (k, v) in &self.env {
            c.env(k, v);
        }
        if let Some((k, v)) = self.cargo_env("integration") {
            c.env(k, v);
        }
        let ok = c
            .status()
            .map_err(|e| format!("could not run verify --gate: {e}"))?
            .success();
        if ok {
            return Ok(());
        }
        let text = std::fs::read_to_string(file).unwrap_or_default();
        let checks = failing_checks(&text);
        let named = if checks.is_empty() {
            String::new()
        } else {
            format!(": {}", checks.join(", "))
        };
        Err(format!(
            "the suite is red once the change sits on integration{named} -- see {}",
            file.display()
        ))
    }

    /// Tick the task off where the plan came from. mem holds the plan a plain
    /// `workflow run` reads, but a `--plan-file` run's plan need not be in mem
    /// at all -- and asking mem to tick a plan it does not have left the merge
    /// recorded nowhere, with the ticking left to whoever was watching
    /// (friction #2213VV3P).
    fn tick_off(&self, task: &str) {
        let Some(file) = self.plan_file.as_deref() else {
            if !memcli::plan_tick(task) {
                warn(format!(
                    "task {task}: mem could not tick it off (is this plan in mem?)"
                ));
            }
            return;
        };
        let ticked = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| plan::tick(&text, task))
            .is_some_and(|text| std::fs::write(file, text).is_ok());
        if !ticked {
            warn(format!(
                "task {task}: could not tick it off in {}",
                file.display()
            ));
        }
    }

    fn fail_task(&self, task: &str, why: &str) {
        warn(format!("task {task}: failed -- {why}"));
        self.set_state(task, FAILED);
        write_field(&self.dir, task, "failed", why);
        if self.commits(task) > 0 {
            // The branch survives cleanup, so the work is still reachable --
            // but only if the reader is told where (friction #BHPS3G7D).
            warn(format!("  its work is on the branch {}", self.branch(task)));
        }
        memcli::log(&format!(
            "run {}: failed {task} -- {why}",
            self.plan.plan_id
        ));
    }

    fn finish(&self, task: &str) {
        self.record_context(task);
        let outcome = self
            .backend
            .result(&self.handle(task), &self.dir.join(format!("{task}.json")));
        // A worker that died leaving nothing -- no status line, no commit --
        // has said nothing about the task, only about the dispatch: a
        // transient API error on the first turn looks exactly like this.
        // Failing it stalls every dependent behind a task nobody has actually
        // attempted, so it gets the one retry a silent stall already had
        // (friction #195SW7VX).
        let tries: u64 = self.field(task, "dispatches").parse().unwrap_or(0);
        if self.last_status_line(task).is_none() && self.commits(task) == 0 && tries < 2 {
            self.once_more(
                task,
                "its worker died leaving nothing",
                "its worker died before writing anything, and was dispatched again",
            );
            return;
        }
        if !outcome.ok {
            // No pidfile and no result: the template never got as far as either,
            // so this is a dispatch that did not happen rather than a worker that
            // ran and failed. Saying "the worker exited with an error" sent
            // whoever read it looking at a worker that never existed.
            let no_pid = self.worker_pid(task).is_empty();
            let no_result = std::fs::metadata(self.dir.join(format!("{task}.json")))
                .map(|m| m.len() == 0)
                .unwrap_or(true);
            if no_pid && no_result {
                self.fail_task(task, "dispatch race: worker never wrote its pidfile");
            } else {
                self.fail_task(task, "the worker exited with an error");
            }
            return;
        }
        match self.last_status_line(task) {
            // A worker that stopped on a question is waiting on the
            // orchestrator, and the failure note says which question: the
            // poll loop watches it and dispatches the task again the moment
            // an answer lands, with the answer in the brief. The id comes
            // off the worker's own report first: mem's read verbs never wait
            // on another invocation's reindex, so a question written a
            // moment ago can be missing from the listing this once.
            Some((state, note))
                if state == "blocked"
                    && let Some(asked) = self.question_in(task, &note) =>
            {
                self.fail_task(task, &asked);
                return;
            }
            // A worker that committed and then reported something other than
            // ready, or nothing at all, has still left work on the branch --
            // the same work a `ready` report would hand to the gate. Failing
            // it here throws that work away for a report the worker may never
            // have gotten to write; the gate is what judges whether it holds
            // up, not this last line.
            Some((state, _))
                if state != "ready" && state != "blocked" && self.commits(task) > 0 =>
            {
                warn(format!(
                    "task {task}: its worker committed and left without reporting ready -- the gate judges the branch"
                ));
            }
            // The worker said where it stood; the failure must not claim
            // otherwise.
            Some((state, note)) if state != "ready" => {
                let mut why = if note.is_empty() {
                    format!("the worker's last report was '{state}'")
                } else {
                    format!("the worker's last report was '{state}: {note}'")
                };
                // A word the protocol never taught it. Naming the vocabulary
                // is the difference between a reader who can see what went
                // wrong and one staring at a state nothing documents
                // (friction #W2SY30WH).
                if !brief::STATES.contains(&state.as_str()) {
                    why.push_str(&format!(
                        ", which is not one of {}",
                        brief::STATES.join(", ")
                    ));
                }
                self.fail_task(task, &why);
                return;
            }
            None if self.commits(task) > 0 => {
                warn(format!(
                    "task {task}: its worker committed and left without reporting ready -- the gate judges the branch"
                ));
            }
            None => {
                self.fail_task(task, "the worker stopped without reporting ready");
                return;
            }
            Some(_) => {}
        }
        match self.merge(task) {
            Ok(Merge::Landed) => self.land(task),
            // A reader has the diff. The task waits on its verdict, and so
            // does every merge behind it; the run goes on dispatching and
            // watching its workers meanwhile.
            Ok(Merge::Reading) => self.set_state(task, REVIEWING),
            // Ready with nothing committed: the worker found its Done already
            // satisfied -- rebuilt by hand between passes, or landed by an
            // earlier plan. Failing it skipped every dependent behind work
            // that exists (friction #B2D8SJKR).
            Ok(Merge::Nothing) => {
                self.set_state(task, DONE_PREVIOUSLY);
                warn(format!(
                    "task {task}: reported ready with nothing to commit -- its work is already in the tree"
                ));
                self.tick_off(task);
                memcli::log(&format!(
                    "run {}: {task} was already satisfied, nothing to merge",
                    self.plan.plan_id
                ));
            }
            Err(why) => self.fail_task(task, &why),
        }
    }

    /// Workers still running tasks this run has already settled. A killed
    /// coordinator does not take its workers down with it, and a later pass
    /// can merge or fail a task while an earlier attempt's worker is still
    /// going: that worker is building for nobody, and what it commits becomes
    /// a leftover branch for the next run to refuse on (friction #RF50DJXQ).
    /// Answers with how many were stopped.
    fn stop_settled_orphans(&self) -> usize {
        let mut stopped = 0;
        for task in self.plan.ids() {
            let state = self.state(&task);
            if state == DISPATCHED || state == PENDING || state == REVIEWING || state.is_empty() {
                continue; // the reap pass and the waves own these
            }
            if self.field(&task, "session").is_empty() && self.worker_pid(&task).is_empty() {
                continue; // never dispatched, so nothing can be alive
            }
            // `alive` claims a session nothing has seen is still launching;
            // for a settled task that silence means gone, not launching.
            if !self.backend.seen(&self.handle(&task)) || !self.alive(&task) {
                continue;
            }
            warn(format!(
                "task {task}: {state} already, and its worker is still going -- stopping it"
            ));
            self.stop(&task);
            stopped += 1;
        }
        stopped
    }

    fn reap_pass(&self) -> bool {
        let mut did = false;
        for task in self.dispatched() {
            if self.alive(&task) || self.paused(&task) {
                if !self.stalled(&task) {
                    continue;
                }
                warn(format!(
                    "task {task}: nothing has moved for {}s -- stopping the group",
                    self.deadline_s
                ));
                self.stop(&task);
                self.record_context(&task); // how full it was when it went quiet
                did = true;
                let tries: u64 = self.field(&task, "dispatches").parse().unwrap_or(0);
                if self.commits(&task) == 0 && tries < 2 {
                    self.once_more(
                        &task,
                        "stalled with nothing committed",
                        "it stalled with nothing committed and was stopped",
                    );
                } else {
                    self.fail_task(&task, "stalled with no sign of life");
                }
                continue;
            }
            // While a reader holds integration, a finished worker keeps: its
            // merge would land on a fast-forward nothing has recorded yet.
            // Asked per task, not per pass -- the task before it in this very
            // pass may be the one that started the reading. It is collected
            // on the pass after the reading ends.
            if !self.reviewing().is_empty() {
                continue;
            }
            did = true;
            self.finish(&task);
        }
        did
    }

    /// Tasks an earlier orchestrator left dispatched when it died. Its lock
    /// went with its file descriptors, so this run owns them now: the ones
    /// still working are adopted as they stand, and the rest are collected
    /// exactly as the reap loop would have collected them.
    ///
    /// Without this a stale `dispatched` either holds its wave open forever
    /// against a worker that is gone, or gets dispatched a second time into a
    /// worktree that still has the first one in it.
    ///
    /// Answers with the ids it took over. They are settled for this run --
    /// merged, failed, or running -- and the waves below must not queue them
    /// a second time.
    fn adopt_stale(&self) -> Vec<String> {
        let mut taken = self.dispatched();
        // Taken before a single task below is collected: collecting one can
        // itself start a reading, and that reading is this run's own, not
        // something left behind by one that died -- the loop after must
        // never mistake it for the latter.
        let mid_review = self.reviewing();
        for task in &taken {
            // Known-dead, not still-launching: the run that recorded this
            // session is gone and nothing anywhere says it ever ran. Waiting
            // out the stall deadline on it bought nothing (friction
            // #9F7WT13K); dispatch again now, while the retry lasts.
            if self.ghost(task) {
                let tries: u64 = self.field(task, "dispatches").parse().unwrap_or(0);
                if tries < 2 {
                    warn(format!(
                        "task {task}: the recorded session never existed -- dispatching again now"
                    ));
                    self.dispatch(
                        task,
                        "the session it was given never existed, so it never ran",
                    );
                } else {
                    warn(format!(
                        "task {task}: the recorded session never existed and the retry is spent"
                    ));
                    self.finish(task);
                }
                continue;
            }
            if self.alive(task) || self.paused(task) {
                warn(format!(
                    "task {task}: still working, from a run that is gone -- adopted"
                ));
                continue;
            }
            if !self.reviewing().is_empty() {
                continue; // a reader holds integration; the poll loop collects it after
            }
            warn(format!(
                "task {task}: left dispatched by a run that is gone -- collecting it"
            ));
            self.finish(task);
        }
        // A reading the dead run started. Its reader may still be going, and
        // its answer would be read by nobody; the merge is verified and read
        // again off the intent line, the way an interrupted merge is.
        //
        // Over the snapshot taken above, not a fresh `reviewing()`: this run
        // may itself have started a reading while collecting a task above,
        // and that reading is stamped after `started` was written, never
        // before it. Stopping and rereading it here would judge the very
        // thing this run just dispatched, so it is left to `review_pass`.
        let run_started = recorded(&self.dir, "started")
            .and_then(|v| v.parse().ok())
            .unwrap_or(i64::MAX);
        for task in mid_review {
            let review_started: i64 = self.field(&task, "review-started").parse().unwrap_or(0);
            if review_started >= run_started {
                // This run's own reading, started while collecting a task
                // above -- settled for this run either way, so it belongs
                // in `taken` even though it is left running.
                taken.push(task);
                continue;
            }
            let h = self.review_handle(&task);
            if self.backend.seen(&h) && self.backend.alive(&h) {
                self.backend.stop(&h, self.kill_grace_s);
            }
            warn(format!(
                "task {task}: left mid-reading by a run that is gone -- reading it again"
            ));
            self.finish(&task);
            taken.push(task);
        }
        taken
    }

    // ----------------------------------------------------------------- setup

    /// Every reason to refuse this run, decided before a single ref or file has
    /// been touched.
    ///
    /// The integration branch is the point of it. It accumulates merged,
    /// verified work and nothing else keeps that work reachable, so a second run
    /// of the same plan must never reset it: resetting it first and refusing
    /// afterwards is how a refused run used to destroy the run before it
    /// (review B-1).
    fn preflight(&self) -> bool {
        let git = self.git();
        let mut ok = true;
        let recorded = std::fs::read_to_string(self.dir.join("base_sha"))
            .unwrap_or_default()
            .trim()
            .to_string();

        if let Some(prev) = git.rev_parse_commit(&self.int_branch)
            && !git.is_ancestor(&prev, &self.base)
        {
            let from = if recorded.is_empty() {
                self.base.clone()
            } else {
                recorded
            };
            let ahead = git.count(&format!("{from}..{}", self.int_branch));
            warn(format!(
                "{} holds {ahead} commit(s) an earlier run of this plan merged,",
                self.int_branch
            ));
            warn("and no other branch has them. This run will not reset it.");
            warn(format!(
                "land that work first -- on your trunk, 'git merge {}' -- and run again;",
                self.int_branch
            ));
            warn(format!(
                "or 'git branch -D {}' if you have decided to throw it away.",
                self.int_branch
            ));
            ok = false;
        }

        let mut leftovers: Vec<String> = Vec::new();
        for t in &self.plan.tasks {
            if t.checked {
                continue; // already done, nothing to run
            }
            if self.worktree(&t.id).is_dir() {
                if self.resumable(&t.id) {
                    warn(format!(
                        "{}: failed last run -- resumed on its branch",
                        t.id
                    ));
                }
                continue; // an interrupted run still set up, or a resumable one
            }
            let branch = self.branch(&t.id);
            if git.rev_parse_commit(&branch).is_none() {
                continue;
            }
            // A task that was never dispatched -- blocked, or its run cut short
            // -- leaves a branch holding nothing integration lacks. Refusing
            // over it stops the rerun that would do the work, and there is
            // nothing on it to look at (friction #1916K336).
            if self.commits(&t.id) == 0 {
                warn(format!("{branch}: empty, deleting"));
                git.quiet(&["branch", "-D", &branch]);
                continue;
            }
            // A failed task with commits resumes rather than refuses, even
            // with its worktree gone (setup below prunes and rebuilds it on
            // the same branch).
            if self.resumable(&t.id) {
                warn(format!(
                    "{}: failed last run -- resumed on its branch",
                    t.id
                ));
                continue;
            }
            // What the branch holds, remembered before the recipe below sends
            // its reader off to merge it and delete it: after that the commit
            // is on the trunk under no name this run knows (friction
            // #GPC1PVZJ).
            if let Some(tip) = git.rev_parse_commit(&branch) {
                write_field(&self.dir, &t.id, "leftover", &tip);
            }
            leftovers.push(branch);
        }
        if !leftovers.is_empty() {
            warn("these branches are still here from an earlier run of this plan:");
            for branch in &leftovers {
                warn(format!(
                    "  {branch} -- look at it, then merge it or 'git branch -D {branch}' and run again"
                ));
            }
            ok = false;
        }

        ok
    }

    fn setup(&mut self) -> bool {
        // Nothing above this line has written anything. Nothing below the guard
        // runs unless the guard is happy.
        if !self.preflight() {
            return false;
        }

        for dir in [&self.dir, &self.wt_root, &self.brief_dir] {
            if std::fs::create_dir_all(dir).is_err() {
                return false;
            }
        }
        let _ = std::fs::write(self.dir.join("base_sha"), format!("{}\n", self.base));
        // The moment this run began, so adoption can tell its own fresh work
        // from what a run that died before it left behind.
        let _ = std::fs::write(self.dir.join("started"), format!("{}\n", sys::now()));
        // What this run dispatches on and reads with, so a later `reap` for
        // a run that is gone reads with the same models rather than
        // whatever the environment or the project key happen to say by then.
        let _ = std::fs::write(self.dir.join("model"), format!("{}\n", self.model));
        let _ = std::fs::write(
            self.dir.join("review-model"),
            format!("{}\n", self.review_model.as_deref().unwrap_or("")),
        );

        let git = self.git();
        // Created once, never reset. If it is behind the base -- which is what
        // the sanctioned recovery leaves behind, the last run's work having been
        // landed on the trunk -- it fast-forwards, which loses nothing.
        if git.rev_parse_commit(&self.int_branch).is_none()
            && !git.quiet(&["branch", &self.int_branch, &self.base])
        {
            warn(format!(
                "cannot create {} at {}",
                self.int_branch, self.base
            ));
            return false;
        }
        if !self.int_wt.is_dir() {
            if !git.quiet(&[
                "worktree",
                "add",
                "-q",
                "--checkout",
                &self.int_wt.to_string_lossy(),
                &self.int_branch,
            ]) {
                warn(format!(
                    "cannot make the integration worktree at {}",
                    self.int_wt.display()
                ));
                return false;
            }
            self.made.push(self.int_wt.clone());
        }
        let int = Git::at(&self.int_wt);
        if int.head().unwrap_or_default() != self.base
            && !int.quiet(&["merge", "-q", "--ff-only", &self.base])
        {
            warn(format!(
                "{} cannot fast-forward to {}; sort it out by hand",
                self.int_branch, self.base
            ));
            return false;
        }

        for t in self.plan.tasks.clone() {
            if t.checked {
                continue; // already done, nothing to run
            }
            let wt = self.worktree(&t.id);
            if wt.is_dir() {
                continue;
            }
            // A worktree hand-removed since the last run leaves its
            // registration behind; without pruning first, git refuses to
            // reuse the branch or the path a resumed task needs back.
            git.quiet(&["worktree", "prune"]);
            let branch = self.branch(&t.id);
            if self.resumable(&t.id) {
                // The branch already exists with the failed attempt's
                // commits on it -- checked out again, never recreated.
                if !git.quiet(&["worktree", "add", "-q", &wt.to_string_lossy(), &branch]) {
                    warn(format!("cannot resume the worktree for task {}", t.id));
                    return false;
                }
                self.link_deps(&wt);
                continue; // not self.made: rollback and cleanup leave it be
            }
            if !git.quiet(&[
                "worktree",
                "add",
                "-q",
                "-b",
                &branch,
                &wt.to_string_lossy(),
                &self.base,
            ]) {
                warn(format!("cannot make a worktree for task {}", t.id));
                return false;
            }
            self.made.push(wt.clone());
            self.link_deps(&wt);
        }
        true
    }

    /// Dependencies are shared, not reinstalled: this machine's git already
    /// carries `worktree.symlinkDirectories` for exactly these two.
    fn link_deps(&self, wt: &Path) {
        for d in ["node_modules", "vendor"] {
            let from = self.repo.join(d);
            let to = wt.join(d);
            if from.is_dir() && !to.exists() {
                let _ = std::os::unix::fs::symlink(&from, &to);
            }
        }
        if !wt.join("node_modules").exists() && wt.join("pnpm-lock.yaml").is_file() {
            let ok = Command::new("pnpm")
                .args(["install", "--frozen-lockfile"])
                .current_dir(wt)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                warn(format!("pnpm install failed in {}", wt.display()));
            }
        }
        if !wt.join("vendor").exists()
            && wt.join("composer.lock").is_file()
            && crate::have("composer")
        {
            let ok = Command::new("composer")
                .args(["install", "--no-interaction"])
                .current_dir(wt)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                warn(format!("composer install failed in {}", wt.display()));
            }
        }
    }

    /// Every builder's dir under `cargo_root`, torn down -- except a resumable
    /// task's, which the next run's worker needs to find undisturbed. Walks
    /// the directory's actual entries rather than the plan's task ids: the
    /// gate's own "integration" builder is not a task and was left behind by
    /// a version of this that iterated ids instead (t052).
    fn clear_cargo(&self) {
        let root = self.cargo_root();
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if self.resumable(&name.to_string_lossy()) {
                    continue;
                }
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
        let _ = std::fs::remove_dir(&root);
    }

    /// Undo exactly what this setup created, and nothing an earlier run may have
    /// left behind.
    fn rollback(&self) {
        for wt in &self.made {
            if !self
                .git()
                .quiet(&["worktree", "remove", "--force", &wt.to_string_lossy()])
            {
                let _ = std::fs::remove_dir_all(wt);
            }
        }
        self.git().quiet(&["worktree", "prune"]);
        let _ = std::fs::remove_dir(&self.wt_root);
        self.clear_cargo();
    }

    /// Take down what the run set up -- but never out from under a worker.
    ///
    /// A task still in `dispatched` has a worker standing in that worktree.
    /// The tree going out from under two live workers has happened, and the
    /// first they knew of it was their own tooling disappearing mid-task; no
    /// ending is worth that. A run that stops with anything dispatched leaves
    /// every worktree where it is, for the next invocation to adopt or for
    /// `workflow reap` to collect.
    fn cleanup(&self) {
        let live = self.dispatched();
        if !live.is_empty() {
            warn(format!(
                "run {}: {} task(s) are still dispatched -- nothing here is cleaned up",
                self.plan.plan_id,
                live.len()
            ));
            for t in &live {
                warn(format!("  {t}: {}", self.worktree(t).display()));
            }
            warn("run again in this checkout to adopt them, or 'workflow reap' to collect them");
            return;
        }

        let git = self.git();
        for t in self.plan.ids() {
            let wt = self.worktree(&t);
            if !wt.is_dir() {
                continue;
            }
            if self.resumable(&t) {
                continue; // its worktree stays for the next run to resume
            }
            if !git.quiet(&["worktree", "remove", "--force", &wt.to_string_lossy()]) {
                let _ = std::fs::remove_dir_all(&wt);
            }
        }
        for t in self.plan.ids() {
            let state = self.state(&t);
            // A done-previously branch is deleted only when it holds nothing
            // integration lacks -- which for the ready-with-nothing case is
            // definitionally true, and never true of failed work.
            if state == MERGED || (state == DONE_PREVIOUSLY && self.commits(&t) == 0) {
                git.quiet(&["branch", "-D", &self.branch(&t)]);
            }
        }
        if self.int_wt.is_dir()
            && !git.quiet(&[
                "worktree",
                "remove",
                "--force",
                &self.int_wt.to_string_lossy(),
            ])
        {
            let _ = std::fs::remove_dir_all(&self.int_wt);
        }
        git.quiet(&["worktree", "prune"]);
        let _ = std::fs::remove_dir(&self.wt_root);
        // Artifacts built in the worktrees go with them: a cached test binary
        // bakes its worktree path in at compile time, and outliving that path
        // is how phantom failures happen (friction #TFVWXXDQ). A resumable
        // task's is the exception, same as its worktree above.
        self.clear_cargo();
    }

    fn deps_satisfied(&self, task: &Task) -> bool {
        task.deps.iter().all(|d| match self.state(d).as_str() {
            MERGED => true,
            // Ticked off in the plan without this orchestrator ever merging it:
            // the work is the human's and it is either in the base or nowhere,
            // so there is nothing here to wait for. A task that WAS merged once
            // and whose commit is not on integration is a different matter --
            // what depends on it would be building on a branch missing its
            // parent.
            DONE_PREVIOUSLY => self.prior_sha(d).is_empty(),
            _ => false,
        })
    }
}

/// Every reason to refuse a run before it has written anything, as the lines
/// to say; `None` means go. Said here rather than at the gate because all
/// three are settled before the first worker starts, and a run that dispatches
/// and only then discovers nobody reads it has spent the sessions already.
///
/// `asked` is whether anyone said nobody should read: `WORKFLOW_REVIEW_MODEL`
/// set at all, empty or not, or `review-model none` in the store. Either is a
/// run saying it wants no reading and meaning it; neither is a project that
/// has not decided, and merges nobody reads are not what the gate is for.
fn refused(plan: &Plan, model: &str, reader: Option<&str>, asked: bool) -> Option<String> {
    if plan.kind == PlanKind::Roadmap {
        return Some(format!(
            "run: '{}' is a roadmap, and its items are milestones rather than work a worker can take.\n\
             make one of them the plan of record with `mem plan --from <slug>`, then run again.",
            plan.plan_id
        ));
    }
    let unread = "record that nobody does with `mem project set review-model none`, \
                  or run this one unread with WORKFLOW_REVIEW_MODEL= in the environment.";
    match reader {
        // A model reads its own work with its own blind spots and agrees with
        // itself, so naming the workers' own model is naming nobody -- under
        // any spelling of it.
        Some(reader) if same_model(reader, model) => Some(format!(
            "run: the workers write with {}, and {reader} is the same model, so it would be reading its own work.\n\
             name another reader with `mem project set review-model <model>`, {unread}",
            model.trim()
        )),
        None if !asked => Some(format!(
            "run: nobody is named to read what this run merges.\n\
             name a reader with `mem project set review-model <model>`, {unread}"
        )),
        _ => None,
    }
}

/// Are these two names one model? `opus`, `claude-opus-5` and `opus[1m]`
/// all start the same model, so when both names carry a family word that
/// word settles it; names outside the families are compared as spelled.
/// The refusal above asks this to keep a model from reading its own work
/// under a second spelling.
fn same_model(a: &str, b: &str) -> bool {
    const FAMILIES: [&str; 4] = ["fable", "opus", "sonnet", "haiku"];
    let (a, b) = (a.trim().to_ascii_lowercase(), b.trim().to_ascii_lowercase());
    if a == b {
        return true;
    }
    let family = |name: &str| FAMILIES.iter().copied().find(|f| name.contains(f));
    matches!((family(&a), family(&b)), (Some(x), Some(y)) if x == y)
}

/// The stopped-short report, written to be read at a glance (friction
/// #VTB9VB1S): counts first, then one line per outcome group, and never an
/// empty note. Held to the same lint commits are held to. `tasks` is (id,
/// state, failure reason) in plan order. A log line and stderr, not a
/// question: the orchestrator reads it and decides, and a person is asked
/// only what the orchestrator cannot settle.
fn stopped_short(plan_id: &str, tasks: &[(String, String, String)]) -> String {
    let count = |s: &str| tasks.iter().filter(|(_, state, _)| state == s).count();
    let (merged, failed, previous) = (count(MERGED), count(FAILED), count(DONE_PREVIOUSLY));
    let waiting = tasks.len() - merged - failed - previous;

    let mut counts = vec![format!("{merged} of {} merged", tasks.len())];
    if failed > 0 {
        counts.push(format!("{failed} failed"));
    }
    if waiting > 0 {
        counts.push(format!("{waiting} never started"));
    }
    if previous > 0 {
        counts.push(format!("{previous} already ticked off"));
    }
    let mut q = format!("Plan {plan_id} stopped short: {}.\n", counts.join(", "));

    // Failed tasks grouped by reason, in the order the reasons first appear.
    let mut groups: Vec<(&str, Vec<&str>)> = Vec::new();
    for (id, state, note) in tasks {
        if state != FAILED {
            continue;
        }
        let why = if note.is_empty() {
            "no reason recorded"
        } else {
            note.as_str()
        };
        match groups.iter_mut().find(|(w, _)| *w == why) {
            Some((_, list)) => list.push(id),
            None => groups.push((why, vec![id])),
        }
    }
    for (why, list) in &groups {
        q.push_str(&format!("Failed - {why}: {}.\n", list.join(", ")));
    }

    let never: Vec<&str> = tasks
        .iter()
        .filter(|(_, s, _)| s != MERGED && s != FAILED && s != DONE_PREVIOUSLY)
        .map(|(id, _, _)| id.as_str())
        .collect();
    if !never.is_empty() {
        q.push_str(&format!("Never started: {}.\n", never.join(", ")));
    }
    q.trim_end().to_string()
}

/// The state a report's second field names, and whatever it glued on after a
/// colon.
///
/// `ready`, `ready:` and `ready::merge-ready` all name the same state: the
/// colon is punctuation a worker adds to a word it was asked to write bare,
/// and reading it as part of the state failed a task whose work was
/// merge-ready (friction #W2SY30WH).
fn split_state(token: &str) -> (String, String) {
    match token.split_once(':') {
        Some((state, rest)) => (state.to_string(), rest.trim_matches(':').to_string()),
        None => (token.to_string(), String::new()),
    }
}

/// A token count the way a person reads one: 157k, 1.2M. Rounded down, because
/// this is a size to judge a plan by and rounding a task up towards a full
/// window would be the wrong way to be wrong.
pub fn tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", (n / 100_000) as f64 / 10.0)
    } else if n >= 1_000 {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}

/// The knobs of spec §8, all injectable (AC7).
fn timings() -> (usize, i64, i64, f64) {
    let mut max_workers = std::env::var("WORKFLOW_MAX_WORKERS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(2);
    max_workers = max_workers.clamp(1, 3);

    // Fractional minutes on purpose: AC7 injects a deadline in seconds.
    let deadline = ((env_f64("WORKFLOW_DEADLINE_MIN", 30.0) * 60.0) + 0.5) as i64;
    let deadline = deadline.max(1);
    let grace = (deadline / 2).clamp(1, 30);
    let poll = ((deadline as f64 / 10.0) * 100.0).round() / 100.0;
    let poll = poll.clamp(0.2, 5.0);
    (max_workers as usize, deadline, grace, poll)
}

/// Which worker a run dispatches onto, by name: `WORKFLOW_BACKEND` first, so
/// one run can be moved without touching what the project stands for; then the
/// project's own key, which is where the standing choice lives; then claude,
/// which is what every project ran on before there was a choice.
fn backend_name<'a>(asked: Option<&'a str>, declared: Option<&'a str>) -> &'a str {
    [asked, declared]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|name| !name.is_empty())
        .unwrap_or("claude")
}

/// mem's `project set backend` takes a closed list, so a declared name is
/// always one of these. `WORKFLOW_BACKEND` is free text and a typo in it must
/// not silently dispatch onto the other worker.
fn backend_for() -> Box<dyn WorkerBackend> {
    let asked = std::env::var("WORKFLOW_BACKEND").ok();
    let declared = memcli::project_backend();
    match backend_name(asked.as_deref(), declared.as_deref()) {
        "amx" => Box::new(AmxBackend),
        "claude" => Box::new(ClaudeBackend),
        other => {
            warn(format!(
                "backend '{other}' is not one this workflow has -- dispatching onto claude"
            ));
            Box::new(ClaudeBackend)
        }
    }
}

/// `recorded` is the run directory a `setup` of this same plan already wrote
/// `model` and `review-model` into -- given by `reap`, rebuilding a run
/// nobody is watching, and `None` for a fresh `run`, which has nothing
/// recorded yet. Either way the environment has the last word and a project
/// key the least: `WORKFLOW_MODEL`/`WORKFLOW_REVIEW_MODEL`, then what was
/// recorded, then `mem project set model`/`review-model`, then `opus`/nobody.
fn new_run(plan: Plan, repo: PathBuf, project: &str, base: String, recorded: Option<&Path>) -> Run {
    let (max_workers, deadline_s, kill_grace_s, poll) = timings();
    let wt_root = paths::worktrees_root().join(project).join(&plan.plan_id);
    let recorded_model = recorded.and_then(|d| self::recorded(d, "model"));
    let recorded_review = recorded.and_then(|d| self::recorded(d, "review-model"));
    Run {
        dir: paths::runs_root().join(project).join(&plan.plan_id),
        brief_dir: paths::briefs_root().join(project).join(&plan.plan_id),
        project: project.to_string(),
        int_branch: format!("integration/{}", plan.plan_id),
        int_wt: wt_root.join("_integration"),
        wt_root,
        plan,
        plan_file: None,
        repo,
        base,
        deadline_s,
        kill_grace_s,
        poll,
        max_workers,
        backend: backend_for(),
        model: match std::env::var("WORKFLOW_MODEL") {
            Ok(v) if !v.is_empty() => v,
            _ => recorded_model
                .filter(|v| !v.is_empty())
                .or_else(memcli::project_model)
                .unwrap_or_else(|| "opus".into()),
        },
        review_model: match std::env::var("WORKFLOW_REVIEW_MODEL") {
            Ok(v) => Some(v.trim().to_string()).filter(|v| !v.is_empty()),
            Err(_) => match recorded_review {
                Some(v) => Some(v).filter(|v| !v.is_empty()),
                None => memcli::project_review_model(),
            },
        },
        stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        env: Vec::new(),
        collecting: false,
        made: Vec::new(),
    }
}

pub fn cmd_run(plan_file: Option<&Path>) -> i32 {
    if !Git::here().inside_worktree() {
        warn("run: stand in the project checkout");
        return exit::USAGE;
    }
    memcli::resolve_from_here();
    let Some((git, top)) = repo::goto_toplevel() else {
        return exit::USAGE;
    };
    let Some(project) = memcli::project_current() else {
        warn("run: mem does not know this checkout, so there is no project to run under");
        return exit::USAGE;
    };
    if let Some(root) = project.root.as_deref()
        && !root.is_empty()
        && paths::realpath(root) != paths::realpath(&top)
    {
        warn(format!(
            "run: mem has this project's checkout at {root}, and you are somewhere else -- worktrees and run state are shared under the same project name"
        ));
    }

    let source = match plan_file {
        Some(f) => match std::fs::read_to_string(f) {
            Ok(text) => text,
            Err(_) => {
                warn(format!("run: cannot read {}", f.display()));
                return exit::USAGE;
            }
        },
        None => match memcli::plan() {
            Some(text) => text,
            None => {
                warn("run: this project has no plan in mem, and no --plan-file was given");
                return exit::USAGE;
            }
        },
    };
    let Some(parsed) = plan::parse(&source, true) else {
        return exit::USAGE;
    };

    let Some(base) = git.head() else {
        return exit::USAGE;
    };
    let mut run = new_run(parsed, top, &project.dir_name(), base, None);
    // Resolved, not as typed: the ticks go back to this file for the rest of
    // the run, and a relative path is read against whatever the cwd is then.
    run.plan_file = plan_file.map(paths::realpath_m);

    // Before the lock, the worktrees and the first dispatch: nothing here has
    // written anything yet, so a refusal costs a message and no cleanup.
    let recorded_none = memcli::reader_recorded_none();
    if let Some(why) = refused(
        &run.plan,
        &run.model,
        run.review_model.as_deref(),
        recorded_none || std::env::var("WORKFLOW_REVIEW_MODEL").is_ok(),
    ) {
        for line in why.lines() {
            warn(line);
        }
        return exit::USAGE;
    }
    // Said out loud, because a run that merges unread is worth noticing even
    // when it is exactly what the project asked for.
    if recorded_none && run.review_model.is_none() {
        warn("nobody reads this run: review-model is none");
    }

    if run.plan.tasks.len() <= 1 {
        warn(format!(
            "plan '{}' has one task: do it here, in this session -- orchestrating one worker costs more than it saves.",
            run.plan.plan_id
        ));
        return exit::OK;
    }

    // Held for the whole run, taken before setup writes a single worktree:
    // two orchestrators sharing this run dir would dispatch the same tasks
    // into the same worktrees (friction #V6KDQM3S).
    let _ = std::fs::create_dir_all(&run.dir);
    let Some(_lock) = lock_run(&run.dir) else {
        warn(format!(
            "run {}: another orchestrator is live in this run -- not starting a second",
            run.plan.plan_id
        ));
        return exit::USAGE;
    };

    if !run.setup() {
        run.rollback();
        return exit::USAGE;
    }
    let _ = std::fs::write(run.dir.join("plan.md"), &source);
    for t in run.plan.ids() {
        if !run.dir.join(format!("{t}.state")).exists() {
            run.set_state(&t, PENDING);
        }
        // A redispatch marker nobody consumed was a request to a run that is
        // gone; carrying it into this run would dispatch a task nobody asked
        // this run about.
        let _ = std::fs::remove_file(run.dir.join(format!("{t}.redispatch")));
    }
    // The way down (friction #RF50DJXQ): a killed coordinator used to leave
    // its workers running for nobody. The signal only raises a flag; the poll
    // loop sees it, stops every dispatched worker, and leaves the tasks
    // `dispatched` for the next run in this checkout to adopt and collect.
    // SIGKILL still can't be caught -- reap covers that aftermath.
    for sig in [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGHUP,
    ] {
        let _ = signal_hook::flag::register(sig, run.stop.clone());
    }
    let stopping = || run.stop.load(std::sync::atomic::Ordering::Relaxed);

    let adopted = run.adopt_stale();
    memcli::log(&format!(
        "run {}: started at {} with {} tasks",
        run.plan.plan_id,
        run.base,
        run.plan.tasks.len()
    ));
    warn(format!(
        "run {}: {} tasks in {} wave(s), up to {} at a time",
        run.plan.plan_id,
        run.plan.tasks.len(),
        run.plan.waves.len(),
        run.max_workers
    ));

    for wave in run.plan.waves.clone() {
        // Which tasks a redispatch can still reach. `workflow redispatch`
        // reads this to refuse a task whose wave has closed, instead of
        // leaving a marker nothing will read (friction #H80BMJJF).
        let open = run.dir.join("wave");
        if let Err(e) = std::fs::write(&open, format!("{}\n", wave.join(" "))) {
            warn(format!(
                "run {}: cannot write {} ({e})",
                run.plan.plan_id,
                open.display()
            ));
        }
        let mut queue: Vec<String> = Vec::new();
        for id in &wave {
            let Some(task) = run.plan.get(id).cloned() else {
                continue;
            };
            // A tick in the plan is a claim about the past, not about this
            // integration branch. It only counts as merged here when the commit
            // some run recorded for it is actually on the branch.
            if task.checked {
                if run.landed(id) {
                    run.set_state(id, MERGED);
                } else {
                    run.set_state(id, DONE_PREVIOUSLY);
                    if run.prior_sha(id).is_empty() {
                        warn(format!(
                            "task {id}: ticked off in the plan already; this run does not touch it"
                        ));
                    } else {
                        warn(format!(
                            "task {id}: ticked off, but the commit an earlier run merged is not on {} -- left alone",
                            run.int_branch
                        ));
                    }
                }
                continue;
            }
            // Taken over from the run that died: already merged, already
            // failed, or running right now. The loop below waits on the ones
            // still going; none of them gets dispatched a second time.
            if adopted.contains(id) {
                continue;
            }
            // Unticked, but the commit some pass merged for it is on the
            // integration branch as it stands: an earlier pass merged it and
            // the tick never took, or the work was landed by hand between
            // passes -- the binary's own printed recipe (friction #94EMPK30).
            // The ref recorded at merge time outlives every branch, so this
            // is checkable, and a worker was rebuilding landed work when only
            // the tick was consulted.
            if run.landed(id) {
                run.set_state(id, MERGED);
                warn(format!(
                    "task {id}: its work is already on {} -- not dispatched again",
                    run.int_branch
                ));
                // Ticked here as at any other merge, or the plan goes on
                // asking for work that is done and every later run pays the
                // same discovery again.
                run.tick_off(id);
                continue;
            }
            if run.deps_satisfied(&task) {
                queue.push(id.clone());
            } else {
                warn(format!(
                    "task {id}: skipped, what it waits for did not land"
                ));
                run.set_state(id, BLOCKED);
            }
        }

        let mut warned_waiting: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        while !queue.is_empty()
            || run.running() > 0
            || !run.reviewing().is_empty()
            || !run.waiting(&wave).is_empty()
        {
            if stopping() {
                return shutdown(&run);
            }
            while !queue.is_empty() && run.running() < run.max_workers {
                let next = queue.remove(0);
                run.dispatch(&next, "");
            }
            sys::sleep(run.poll);
            if stopping() {
                return shutdown(&run);
            }
            run.review_passes();
            run.reap_pass();
            // A failed task someone asked to try again, mid-run. The marker
            // file is how the request reaches a run that holds the project
            // lock for its whole life (friction #W0S44DE6); it is honoured
            // while the task's wave is still open, which is exactly when a
            // redispatch can still feed the tasks waiting on it.
            for id in &wave {
                let marker = run.dir.join(format!("{id}.redispatch"));
                if !marker.exists() {
                    continue;
                }
                if run.state(id) != FAILED {
                    let _ = std::fs::remove_file(&marker);
                    warn(format!(
                        "task {id}: asked to go again, but it is {} -- ignored",
                        run.state(id)
                    ));
                    continue;
                }
                if run.running() >= run.max_workers {
                    continue; // the marker keeps until a slot frees up
                }
                let _ = std::fs::remove_file(&marker);
                warn(format!("task {id}: dispatched again by request"));
                run.dispatch(id, "");
            }
            // A task waiting on a question keeps its wave open rather than
            // failing the run out from under it, so said once, not on every
            // poll while the answer is still pending.
            for id in run.waiting(&wave) {
                let Some(qid) = run.asked(&id) else {
                    continue;
                };
                if warned_waiting.insert(format!("{id} {qid}")) {
                    warn(format!(
                        "{id}: waiting on #{qid} -- the wave stays open until it is answered"
                    ));
                }
            }
            // A task waiting on the orchestrator goes again by itself once
            // the answer is in: the orchestrator's whole job here is to
            // answer, and a run that had to be told twice stopped short
            // over questions it could have carried (the queue was one
            // stopped-short question per answer, all of them stale).
            for id in &wave {
                if run.state(id) != FAILED || run.running() >= run.max_workers {
                    continue;
                }
                let Some(qid) = run.asked(id) else {
                    continue;
                };
                let answered = memcli::questions_for(&run.task_tag(id))
                    .into_iter()
                    .any(|q| q.short_id == qid && q.answer.is_some());
                if answered {
                    warn(format!(
                        "task {id}: #{qid} was answered -- dispatched again with the answer"
                    ));
                    run.dispatch(id, "");
                }
            }
        }
    }

    let (mut merged, mut failed, mut blocked, mut previous) = (0, 0, 0, 0);
    for t in run.plan.ids() {
        match run.state(&t).as_str() {
            MERGED => merged += 1,
            FAILED => failed += 1,
            DONE_PREVIOUSLY => previous += 1,
            _ => blocked += 1,
        }
    }

    run.cleanup();

    warn(format!(
        "run {}: {merged} merged, {failed} failed, {blocked} never started",
        run.plan.plan_id
    ));
    let sizing: Vec<String> = run
        .plan
        .ids()
        .into_iter()
        .filter_map(|t| {
            let n: u64 = run.field(&t, "context").parse().ok()?;
            Some(format!("{t} {}", tokens(n)))
        })
        .collect();
    if !sizing.is_empty() {
        warn(format!(
            "run {}: context carried at the last turn -- {}",
            run.plan.plan_id,
            sizing.join(", ")
        ));
        warn("a task that ended near a full window was cut too big; size the next plan by that");
    }
    if previous > 0 {
        warn(format!(
            "run {}: {previous} task(s) were already ticked off and this run left them alone",
            run.plan.plan_id
        ));
    }
    warn(format!(
        "integration branch {} is yours to look at; nothing was pushed",
        run.int_branch
    ));
    // A worker's question on a task that merged anyway is moot, and left
    // pending it sits in the orchestrator's queue for ever.
    for t in run.plan.ids() {
        let state = run.state(&t);
        if state != MERGED && state != DONE_PREVIOUSLY {
            continue;
        }
        for q in memcli::questions_for(&run.task_tag(&t)) {
            if q.answer.is_none() {
                memcli::answer(&q.id, &format!("moot: {t} merged without it"));
            }
        }
    }
    // A milestone is finished when its plan is. The plan of record is the one
    // the roadmap's milestone names, so only a run that read it from mem can
    // say which box to tick: a --plan-file plan need not be in mem at all.
    if failed + blocked == 0 && run.plan_file.is_none() && memcli::roadmap_tick(&run.plan.plan_id) {
        warn(format!(
            "milestone {} is ticked off in the roadmap",
            run.plan.plan_id
        ));
    }
    if failed + blocked > 0 {
        let tasks: Vec<(String, String, String)> = run
            .plan
            .ids()
            .into_iter()
            .map(|t| {
                let state = run.state(&t);
                let note = run.field(&t, "failed");
                (t, state, note)
            })
            .collect();
        // Said on stderr and written to the log, never asked: a run that
        // stops short is the orchestrator's to read and act on, and as a
        // question it went to the phone once per stop, mostly stale by the
        // time it was read.
        let report = stopped_short(&run.plan.plan_id, &tasks);
        for line in report.lines() {
            warn(line);
        }
        memcli::log(&report);
        return exit::FAILED;
    }
    exit::OK
}

/// Told to stop: end every dispatched worker, say so, and leave the tasks
/// `dispatched` -- the next run adopts them and judges whatever they wrote.
/// No merging on the way out: a signal means now, and the merge gate is not
/// a thing to run while shutting down.
fn shutdown(run: &Run) -> i32 {
    let live = run.dispatched();
    warn(format!(
        "run {}: told to stop -- stopping {} worker(s) before going",
        run.plan.plan_id,
        live.len()
    ));
    for task in &live {
        run.stop(task);
        warn(format!("task {task}: its worker was stopped"));
    }
    for task in run.reviewing() {
        run.backend
            .stop(&run.review_handle(&task), run.kill_grace_s);
        warn(format!(
            "task {task}: its reader was stopped -- the next run reads the merge again"
        ));
    }
    warn("run again in this checkout to adopt and collect what they left");
    memcli::log(&format!(
        "run {}: stopped by signal with {} worker(s) ended",
        run.plan.plan_id,
        live.len()
    ));
    exit::FAILED
}

/// `workflow redispatch <task>` -- the marker the live run's poll loop reads.
/// Only a run whose lock is held right now can honour it; anything else is a
/// stopped run, and a stopped run's failed work comes back by running the
/// plan again.
pub fn cmd_redispatch(task: &str) -> i32 {
    if !Git::here().inside_worktree() {
        warn("redispatch: stand in the project checkout");
        return exit::USAGE;
    }
    let Some(project) = memcli::project_current() else {
        warn("redispatch: mem does not know this checkout");
        return exit::USAGE;
    };

    let root = paths::runs_root().join(project.dir_name());
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("plan.md").is_file())
        .collect();
    dirs.sort();

    for dir in dirs {
        // Taking the lock and succeeding means no orchestrator is live here;
        // the guard drops it again on the way past.
        if lock_run(&dir).is_some() {
            continue;
        }
        if field(&dir, task, "state") != FAILED {
            continue;
        }
        let plan_id = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        // The poll loop reads markers for the open wave only: a task that
        // failed in a wave that has closed cannot feed what waited on it,
        // and its marker would sit unread. Said, with exit 1, rather than
        // the same cheerful line either way (friction #H80BMJJF).
        let open = std::fs::read_to_string(dir.join("wave")).unwrap_or_default();
        if !open.split_whitespace().any(|t| t == task) {
            warn(format!(
                "run {plan_id}: {task} failed in a wave that has closed -- this run will not dispatch it; run the plan again to resume it"
            ));
            return exit::FAILED;
        }
        let _ = std::fs::write(dir.join(format!("{task}.redispatch")), "");
        warn(format!(
            "run {plan_id}: asked to dispatch {task} again -- it goes on the next poll with a free worker slot"
        ));
        return exit::OK;
    }

    warn(format!(
        "no live run holds {task} failed -- run the plan again to retry failed tasks"
    ));
    exit::FAILED
}

pub fn cmd_reap() -> i32 {
    if !Git::here().inside_worktree() {
        warn("reap: stand in the project checkout");
        return exit::OK;
    }
    memcli::resolve_from_here();
    let Some((_git, top)) = repo::goto_toplevel() else {
        return exit::OK;
    };
    let Some(project) = memcli::project_current() else {
        warn("reap: mem does not know this checkout");
        return exit::OK;
    };

    let mut did = false;
    let mut adoptable = false;
    let root = paths::runs_root().join(project.dir_name());
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("plan.md").is_file())
        .collect();
    dirs.sort();

    for dir in dirs {
        let Some(plan_id) = dir.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(dir.join("plan.md")) else {
            continue;
        };
        let Some(mut parsed) = plan::parse(&text, true) else {
            continue;
        };
        parsed.plan_id = plan_id;
        let base = std::fs::read_to_string(dir.join("base_sha"))
            .unwrap_or_default()
            .trim()
            .to_string();
        if base.is_empty() {
            continue;
        }
        let mut run = new_run(parsed, top.clone(), &project.dir_name(), base, Some(&dir));
        run.dir = dir;
        run.collecting = true;
        // A held lock means a live orchestrator is watching these workers;
        // reap is for runs nobody owns.
        let Some(_lock) = lock_run(&run.dir) else {
            continue;
        };
        if run.stop_settled_orphans() > 0 {
            did = true;
        }
        // A reading is the run's to judge, not reap's: reap collects, and a
        // verdict may call for another reader.
        for task in run.reviewing() {
            adoptable = true;
            warn(format!(
                "run {}: {task} was mid-reading when its run went -- run again in the checkout to read it again",
                run.plan.plan_id
            ));
        }
        if run.running() == 0 {
            continue;
        }
        // The environment reap runs under is whatever happens to be exported
        // right now, not what this run was told to read with -- say so when
        // the recorded model is what is about to be used, and only once this
        // run has something to collect.
        if std::env::var("WORKFLOW_REVIEW_MODEL").is_err()
            && let Some(model) = run.review_model.as_deref()
            && recorded(&run.dir, "review-model").is_some_and(|v| !v.is_empty())
        {
            warn(format!("reap: reading with {model} as the run did"));
        }
        if run.reap_pass() {
            did = true;
        }
        // Alive and legitimately mid-task, with no orchestrator left to gate
        // them. Not reap's to stop -- the next run adopts them as they stand
        // -- but saying "nothing to collect" about them sent their reader
        // away believing no worker existed (friction #RF50DJXQ).
        let waiting: Vec<String> = run
            .dispatched()
            .into_iter()
            .filter(|t| run.alive(t) || run.paused(t))
            .collect();
        if !waiting.is_empty() {
            adoptable = true;
            warn(format!(
                "run {}: {} worker(s) are still going with no run watching them ({}) -- run again in the checkout to adopt them",
                run.plan.plan_id,
                waiting.len(),
                waiting.join(", ")
            ));
        }
    }

    if did {
        return exit::FAILED;
    }
    if !adoptable {
        warn("reap: nothing to collect");
    }
    exit::OK
}

/// The hidden seam the harness uses to check the liveness rule one signal at a
/// time (AC7's three sources, review-3 F-10).
pub fn cmd_stalled(rundir: &Path, wtroot: &Path, task: &str, deadline: i64) -> i32 {
    if stalled(&ClaudeBackend, rundir, wtroot, task, deadline) {
        exit::OK
    } else {
        exit::FAILED
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str, state: &str, note: &str) -> (String, String, String) {
        (id.into(), state.into(), note.into())
    }

    #[test]
    fn failing_checks_names_up_to_three_tap_lines() {
        let out = "ok 1 - a\nnot ok 2 t3 merges\nok 3 - b\nnot ok 4 side ships\nnot ok 5 extra\nnot ok 6 overflow\n";
        assert_eq!(
            failing_checks(out),
            vec![
                "not ok 2 t3 merges",
                "not ok 4 side ships",
                "not ok 5 extra"
            ]
        );
    }

    #[test]
    fn failing_checks_names_cargo_test_failures_when_there_is_no_tap() {
        let out = "running 2 tests\ntest tests::foo ... FAILED\ntest tests::bar ... ok\n\nfailures:\n    tests::foo\n\ntest result: FAILED. 1 passed; 1 failed; 0 ignored\n";
        assert_eq!(failing_checks(out), vec!["tests::foo"]);
    }

    #[test]
    fn failing_checks_names_failed_lines_when_not_cargo_shaped() {
        let out = "Running suite...\nFAILED tests/BillTest.php::testRender - Assertion failed\n=== 3 failed, 12 passed in 1.2s ===\n";
        assert_eq!(
            failing_checks(out),
            vec!["FAILED tests/BillTest.php::testRender - Assertion failed"]
        );
    }

    #[test]
    fn failing_checks_falls_back_to_the_last_three_lines_in_neither_format() {
        let out = "Building...\nerror[E0599]: no method named `render`\n  --> app/Http/Bill.php:12\nerror: aborting due to 1 previous error\n";
        assert_eq!(
            failing_checks(out),
            vec![
                "error[E0599]: no method named `render`",
                "--> app/Http/Bill.php:12",
                "error: aborting due to 1 previous error"
            ]
        );
    }

    #[test]
    fn failing_checks_drops_the_gates_own_lines_before_classifying() {
        let out = "Running suite...\nFAILURES!\n1) Tests\\Unit\\BillTest::testRender\nFailed asserting that false is true\nTests: 12, Assertions: 30, Failures: 1\nworkflow: verify project: FAILED\n";
        assert_eq!(
            failing_checks(out),
            vec![
                "1) Tests\\Unit\\BillTest::testRender",
                "Failed asserting that false is true",
                "Tests: 12, Assertions: 30, Failures: 1"
            ]
        );
    }

    #[test]
    fn failing_checks_is_empty_on_blank_output() {
        assert!(failing_checks("").is_empty());
        assert!(failing_checks("   \n\n").is_empty());
    }

    #[test]
    fn the_environment_beats_the_project_key_and_the_key_beats_the_default() {
        assert_eq!(backend_name(Some("claude"), Some("amx")), "claude");
        assert_eq!(backend_name(Some("amx"), None), "amx");
        assert_eq!(backend_name(None, Some("amx")), "amx");
        assert_eq!(backend_name(None, None), "claude");
        // An exported-but-empty WORKFLOW_BACKEND is not a choice.
        assert_eq!(backend_name(Some(""), Some("amx")), "amx");
        assert_eq!(backend_name(Some(" \n"), None), "claude");
        // Whatever is asked for comes back as asked, so an unknown name can be
        // reported rather than quietly turning into the other worker.
        assert_eq!(backend_name(Some("amxx"), Some("amx")), "amxx");
    }

    #[test]
    fn the_stopped_short_question_leads_with_counts_and_groups_outcomes() {
        let tasks = [
            t("t1", MERGED, ""),
            t("t2", MERGED, ""),
            t(
                "t3",
                FAILED,
                "the suite is red once the change sits on integration",
            ),
            t(
                "t4",
                FAILED,
                "the suite is red once the change sits on integration",
            ),
            t("t5", FAILED, "wrote outside its Files: patterns"),
            t("t6", BLOCKED, ""),
            t("t7", PENDING, ""),
        ];
        let q = stopped_short("amx-v2", &tasks);
        assert!(
            q.starts_with("Plan amx-v2 stopped short: 2 of 7 merged, 3 failed, 2 never started."),
            "counts do not lead: {q}"
        );
        assert!(
            q.contains("Failed - the suite is red once the change sits on integration: t3, t4."),
            "failed tasks are not grouped by reason: {q}"
        );
        assert!(q.contains("Failed - wrote outside its Files: patterns: t5."));
        assert!(q.trim_end().ends_with("Never started: t6, t7."));
        assert!(
            !q.contains('?'),
            "a report is not a question; the orchestrator reads it: {q}"
        );
    }

    #[test]
    fn the_question_never_glues_empty_notes_and_passes_lint() {
        let tasks = [
            t("ansi", FAILED, "stalled with no sign of life"),
            t("rules", BLOCKED, ""),
        ];
        let q = stopped_short("amx-v2", &tasks);
        assert!(!q.contains("():"), "empty-note residue: {q}");
        assert!(!q.contains(": ;"), "empty-note residue: {q}");
        assert!(!q.contains("(blocked)"), "machine-glued state names: {q}");
        assert!(
            crate::lint::lint_text(&q),
            "the question must hold to the standard commits are held to: {q}"
        );
    }

    #[test]
    fn zero_counts_stay_out_of_the_summary_line() {
        let tasks = [
            t("t1", MERGED, ""),
            t("t2", FAILED, "the worker exited with an error"),
        ];
        let q = stopped_short("p", &tasks);
        assert!(q.starts_with("Plan p stopped short: 1 of 2 merged, 1 failed."));
        assert!(!q.contains("0 never started"), "{q}");
    }

    #[test]
    fn a_state_gives_up_the_colons_a_worker_punctuates_it_with() {
        assert_eq!(split_state("ready"), ("ready".into(), String::new()));
        assert_eq!(split_state("ready:"), ("ready".into(), String::new()));
        assert_eq!(split_state("ready::"), ("ready".into(), String::new()));
        // Glued straight onto the state, the rest is note, not state.
        assert_eq!(
            split_state("ready::merge-ready"),
            ("ready".into(), "merge-ready".into())
        );
        assert_eq!(
            split_state("blocked:waiting"),
            ("blocked".into(), "waiting".into())
        );
        // A word that is not a state stays whole, so the gate can name it.
        assert_eq!(split_state("finished"), ("finished".into(), String::new()));
    }

    #[test]
    fn a_reader_is_the_writer_under_any_spelling_of_the_same_model() {
        assert!(same_model("opus", "opus"));
        assert!(same_model("Opus", " opus "));
        // The alias the CLI takes and the full id start one model.
        assert!(same_model("opus", "claude-opus-5"));
        assert!(same_model("opus[1m]", "claude-opus-5"));
        assert!(!same_model("fable", "opus"));
        assert!(!same_model("claude-fable-5-1", "claude-opus-5"));
        // A name outside the families is compared as spelled.
        assert!(same_model("my-model", "my-model"));
        assert!(!same_model("my-model", "opus"));
    }

    fn doc(kind: PlanKind) -> Plan {
        Plan {
            plan_id: "amx-v2".into(),
            kind,
            ..Plan::default()
        }
    }

    #[test]
    fn a_roadmap_is_refused_with_the_verb_that_turns_one_into_a_plan() {
        let why = refused(&doc(PlanKind::Roadmap), "opus", Some("fable"), true)
            .expect("a roadmap is not work a worker can take");
        assert!(why.contains("'amx-v2' is a roadmap"), "{why}");
        assert!(why.contains("mem plan --from <slug>"), "{why}");
    }

    #[test]
    fn a_run_nobody_reads_is_refused_unless_it_says_so_on_purpose() {
        // Nobody named and nobody asked: the project has not decided.
        let why =
            refused(&doc(PlanKind::Plan), "opus", None, false).expect("an unread run is refused");
        assert!(why.contains("nobody is named to read"), "{why}");
        assert!(
            why.contains("mem project set review-model <model>"),
            "{why}"
        );
        // Both ways to mean it: the key the project records once, and the
        // variable that says it for one run.
        assert!(why.contains("mem project set review-model none"), "{why}");
        assert!(why.contains("WORKFLOW_REVIEW_MODEL="), "{why}");
        // Either one is the way to mean it.
        assert_eq!(refused(&doc(PlanKind::Plan), "opus", None, true), None);
    }

    #[test]
    fn a_reader_that_is_the_writer_is_refused_under_either_spelling() {
        let why = refused(&doc(PlanKind::Plan), "claude-opus-5", Some("opus"), true)
            .expect("a model reading its own work is no reading");
        assert!(
            why.contains("the workers write with claude-opus-5"),
            "{why}"
        );
        assert!(why.contains("opus is the same model"), "{why}");
        // A reader the workers do not share is the whole point of the gate.
        assert_eq!(
            refused(&doc(PlanKind::Plan), "sonnet", Some("fable"), false),
            None
        );
    }

    #[test]
    fn a_token_count_reads_the_way_a_person_says_it() {
        assert_eq!(tokens(158_502), "158k");
        assert_eq!(tokens(1_240_000), "1.2M");
        assert_eq!(tokens(999), "999");
        assert_eq!(tokens(0), "0");
        // Rounded down: a task is never made to look closer to a full window
        // than it was.
        assert_eq!(tokens(1_999), "1k");
        assert_eq!(tokens(1_999_999), "1.9M");
    }

    #[test]
    fn the_deadline_knob_is_fractional_minutes_and_the_rest_derive_from_it() {
        // Nothing to inject: the defaults are the contract.
        let (workers, deadline, grace, poll) = timings();
        assert!((1..=3).contains(&workers));
        assert!(deadline >= 1);
        assert!((1..=30).contains(&grace));
        assert!((0.2..=5.0).contains(&poll));
    }
}
