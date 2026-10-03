//! `workflow run` and `workflow reap` -- deterministic interim orchestration
//! (spec §8).
//!
//! Policy lives here and nowhere else: the ready set, concurrency, ownership,
//! the serialized merge gate. How a worker is started, watched and stopped is
//! the backend's business (see [`crate::backend`]).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::backend::{self, Dispatch, Handle, WorkerBackend};
use crate::gitcmd::Git;
use crate::plan::{Plan, PlanKind, Task};
use crate::{
    brief, exit, hygiene, memcli, ownership, paths, plan, plancheck, repo, sys, verify, warn,
};

pub const PENDING: &str = "pending";
pub const DISPATCHED: &str = "dispatched";
pub const MERGED: &str = "merged";
pub const FAILED: &str = "failed";
pub const BLOCKED: &str = "blocked";
pub const DONE_PREVIOUSLY: &str = "done-previously";

/// How many consecutive polls [`Run::question_open`] tolerates a question's
/// id being absent from mem's listing, for the reindex lag, before treating
/// the id as one mem will never list.
const QUESTION_MISS_LIMIT: u32 = 3;

/// Past this many tokens in its window, a worker is not sent back into its
/// own session: what it would gain from remembering the task it loses to a
/// context that has already been compacted once, and a fresh session with
/// the last attempt in its brief reads better than that.
const CONTINUE_MAX_TOKENS: u64 = 120_000;

/// How long a message just sent counts as a worker starting its turn, before
/// a pane still reading as idle would be collected as a worker that ended.
const CONTINUE_GRACE_S: i64 = 30;

/// The one line a worker gets in its own session when its turn ended with
/// nothing to judge -- no ready, no blocked, no question, no commit. A model
/// that narrated a tool call instead of making one and a provider that cut
/// the turn off look the same from here, and both are one line from going
/// on; a fresh dispatch re-reads everything the session had (ebdify's gql
/// task died three times this way).
const NUDGE_LINE: &str = "Your last turn ended without a report and without a tool call. \
Continue; a provider cutoff looks the same from here.";

/// The wiki pages a task's Read: named, read live off mem, in the shape the
/// brief takes: `(slug, text)`, `None` for a page mem does not have.
fn wiki_pages(task: &Task) -> Vec<(String, Option<String>)> {
    task.wiki_slugs()
        .into_iter()
        .map(|slug| {
            let text = memcli::wiki_page(&slug);
            (slug, text)
        })
        .collect()
}

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

/// One live orchestrator per run directory. The lock rides the returned file:
/// dropping it releases the run, and a holder that dies releases it with its
/// fds, so there is nothing stale to clean up. `None` means another
/// orchestrator is live in this run right now.
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

/// A run-level file `setup` wrote before the first worker went out -- `model`,
/// `effort` -- read back for a run `reap` is rebuilding. `None`
/// means the file was never written: a run dir from before this, or a
/// fixture that never went through `setup`.
fn recorded(dir: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(name))
        .ok()
        .map(|v| v.trim().to_string())
}

/// The clause [`Run::last_heard`] hands back: the first line the worker
/// said, else the last line its pane showed, else nothing.
fn last_heard_in(said: &str, dying: &str) -> String {
    if let Some(line) = said.lines().map(str::trim).find(|l| !l.is_empty()) {
        return format!(" -- it last said: {line}");
    }
    match dying.lines().map(str::trim).rfind(|l| !l.is_empty()) {
        Some(line) => format!(" -- the pane last showed: {line}"),
        None => String::new(),
    }
}

/// One task's field, and a word if it could not be written. The run dir is
/// the run's whole memory of a task: a state write that fails leaves the
/// coordinator reading `pending` for a task it has just dispatched, and it
/// then reports the task as never started and skips everything behind it.
/// That has happened once, silently (under a loaded machine), which is
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

    // vitest and jest: a red test is `✗ name`, `× name` or a `FAIL file`
    // header; nothing in them says FAILED, so a suite of theirs used to
    // name nothing at all.
    let js: Vec<String> = output
        .lines()
        .map(str::trim)
        .filter(|l| {
            l.starts_with("✗ ")
                || l.starts_with("× ")
                || l.starts_with("FAIL ")
                || l.starts_with("❯ ")
        })
        .map(str::to_string)
        .collect();
    if !js.is_empty() {
        return js.into_iter().take(3).collect();
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

/// A red gate's suite runs in this run's own worktree, and a check that
/// asserts on a command line or a path in its own output will find that
/// worktree's path sitting there -- not a defect the gate found, but the
/// tree it ran in. Named when the gate's combined output carries it, so
/// whoever reads the failure knows to read the path before the words
/// around it.
/// Is `wt` a worktree git still knows: a non-empty `.git` file and a row in
/// `git worktree list` for its path.
fn worktree_intact(git: &Git, wt: &Path) -> bool {
    let dotgit = wt.join(".git");
    let has_dotgit = std::fs::metadata(&dotgit).is_ok_and(|m| m.len() > 0);
    if !has_dotgit {
        return false;
    }
    let want = paths::realpath_m(wt);
    git.out(&["worktree", "list", "--porcelain"])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .any(|p| paths::realpath_m(Path::new(p)) == want)
}

/// A suite whose red test timed out asserted nothing false: the wall it hit
/// was a clock, and the change may be right. Said in the failure line so
/// nobody reads a wall of output to learn that.
fn timeout_hint(text: &str) -> &'static str {
    if text.to_lowercase().contains("timed out") {
        "; a test timed out rather than asserting"
    } else {
        ""
    }
}

fn path_hint(text: &str, wt_root: &Path) -> &'static str {
    let text = failure_text(text);
    if wt_root.as_os_str().is_empty() || !text.contains(&*wt_root.to_string_lossy()) {
        return "";
    }
    " -- the failure text carries this run's worktree path, which every spawned command line holds; a test asserting a word absent from a command is reading the path"
}

/// The part of a red suite's output that is about the failures: TAP's `not
/// ok` lines and the `#` lines under them, cargo's `---- name stdout ----`
/// blocks, and vitest's and jest's failure lines with what follows each up
/// to a blank line. The whole output where none of those shapes is there.
/// The worktree path is in every build line a suite prints, so a hint read
/// off the whole output explained a WouldBlock panic that named no path at
/// all.
fn failure_text(output: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut in_block = false;
    for line in output.lines() {
        let t = line.trim();
        let opens = t.starts_with("not ok")
            || (t.starts_with("---- ") && t.ends_with(" ----"))
            || ["✗ ", "× ", "FAIL ", "❯ "].iter().any(|p| t.starts_with(p));
        if opens {
            in_block = true;
        } else if t.is_empty() || t == "failures:" || t.starts_with("ok ") {
            in_block = false;
        }
        if in_block {
            kept.push(line);
        }
    }
    match kept.is_empty() {
        true => output.to_string(),
        false => kept.join("\n"),
    }
}

/// The submodule paths whose pinned commit differs between two commits.
fn moved_submodules(git: &Git, from: &str, to: &str) -> Vec<String> {
    git.out(&["diff", "--raw", "--no-renames", from, to])
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (meta, path) = line.split_once('\t')?;
            let mut fields = meta.split_whitespace();
            let (old_mode, new_mode) = (fields.next()?, fields.next()?);
            (new_mode == "160000" || old_mode == ":160000").then(|| path.to_string())
        })
        .filter(|path| !path.is_empty())
        .collect()
}

/// Liveness is the latest of three signals, because each one alone has a way of
/// going quiet on a worker that is fine: a long test run writes no transcript
/// line, an out-of-tree `CARGO_TARGET_DIR` flattens the worktree's mtime, and a worker
/// that is only thinking touches neither. The status file is the worker's own
/// heartbeat and it lives in the run directory, not the worktree, so it has to
/// be counted separately (review-3 F-10). A worker the usage limit holds
/// touches none of the three either, which is why [`Run::rate_limited`]
/// keeps it from being judged by this clock at all.
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
    // A line just sent into the session -- a continuation, a nudge -- is a
    // worker starting its turn for the send grace, the same grace `alive`
    // gives it: a stall deadline shorter than that grace declared a nudged
    // worker stalled before its turn could begin.
    let sent: i64 = field(dir, task, "continued_at").parse().unwrap_or(0);
    if sent > 0 && now - sent < CONTINUE_GRACE_S {
        return false;
    }
    // The last time the worker was seen waiting on the usage window is a
    // floor like the dispatch: it gets its whole deadline once it is back.
    let limited: i64 = field(dir, task, "limited_at").parse().unwrap_or(0);
    let mut last = last_activity(backend, dir, wt_root, task).max(limited);
    let started: i64 = field(dir, task, "dispatched_at").parse().unwrap_or(0);
    if last <= started {
        last = if started > 0 { started } else { now };
    }
    now - last >= deadline_s
}

/// What the gate made of a ready worker's branch.
enum Merge {
    /// Rebased, fast-forwarded, verified and recorded.
    Landed,
    /// A ready worker with nothing to merge: its Done was already satisfied
    /// in the tree it opened onto.
    Nothing,
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
    /// This binary, as a path taken once when the run is built. The gate
    /// spawns it for `verify --gate`, and reading `current_exe` at that
    /// moment answered `<path> (deleted)` after a `cargo install` replaced
    /// the file under a live run -- an ENOENT with no path in it, one per
    /// live run, at the first gate after the reinstall. Taken early, the path
    /// names whatever stands there now.
    pub exe: PathBuf,
    pub deadline_s: i64,
    /// How long one gate suite may run before its process group is stopped
    /// (`WORKFLOW_GATE_MIN`, 60): a test that hangs used to hold the run for
    /// as long as it hung.
    pub gate_s: i64,
    pub kill_grace_s: i64,
    pub poll: f64,
    /// How many polls running a question's id may be missing from mem's
    /// listing before [`Run::question_open`] stops holding the run open for
    /// it: `QUESTION_MISS_LIMIT`, or `WORKFLOW_QUESTION_MISSES`.
    pub question_misses: u32,
    pub max_workers: usize,
    pub backend: Box<dyn WorkerBackend>,
    /// What every worker of this run is started on: `WORKFLOW_MODEL` for
    /// one run, else the project's `mem project set model`, else opus.
    pub model: String,
    /// How much reasoning the workers spend, `--effort` on either backend:
    /// `WORKFLOW_EFFORT` for one run (empty means no flag), else the
    /// project's `mem project set effort`, else nothing and the CLI's own
    /// default stands.
    pub effort: Option<String>,
    /// Raised by SIGTERM, SIGINT or SIGHUP. The poll loop reads it between
    /// passes.
    pub stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub env: Vec<(String, String)>,
    /// Set by `workflow reap`, which collects for a run that is gone. A
    /// worker it started would have no run watching it, so where a run
    /// gives a task one more try, reap fails it and names who tries it next.
    pub collecting: bool,
    /// The live plan as it last read when it would not parse, so the
    /// refusal is said once per text rather than on every poll.
    unparsable: String,
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
        let _ = std::fs::remove_file(self.dir.join(format!("{task}.held")));
        if state == MERGED || state == DISPATCHED {
            // A failure note used to outlive its failure: status went on
            // reporting why a task failed on one run long after another had
            // merged it, and went on reporting "the launch was refused" beside
            // a task a later attempt had already taken up. What the last
            // attempt said lives on in the brief instead.
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

    /// The task as the live plan has it now -- mem's plan, or the file
    /// this run was handed -- falling back to what the run parsed at start.
    ///
    /// The plan used to be frozen at run start, so an orchestrator answering
    /// "widen the Files line" by editing the plan changed nothing for the
    /// live run: the redispatched worker was handed the old block and the
    /// gate held it to the old patterns, and the same question came round
    /// again. Read fresh at dispatch and at the gate, an edit to the live plan
    /// is the whole correction.
    fn task_now(&self, id: &str) -> Option<Task> {
        self.plan_text()
            .and_then(|text| plan::parse(&text, true))
            .filter(|p| p.plan_id == self.plan.plan_id)
            .and_then(|p| p.get(id).cloned())
            .or_else(|| self.plan.get(id).cloned())
    }

    /// Tasks the live plan has gained since this run parsed it. The
    /// brief already reads the plan live; the ready set read
    /// the parse from setup, so a task added mid-run sat unseen until the
    /// next `workflow run`. Each new task joins `self.plan` in the plan's order
    /// and starts pending; a task the plan dropped is left as it stands, since
    /// `task_now` fails it at dispatch.
    fn take_new_tasks(&mut self) -> Vec<String> {
        let Some(now) = self
            .plan_text()
            .and_then(|text| plan::parse(&text, true))
            .filter(|p| p.plan_id == self.plan.plan_id)
        else {
            return Vec::new();
        };
        let new: Vec<String> = now
            .ids()
            .into_iter()
            .filter(|id| self.plan.get(id).is_none())
            .collect();
        if new.is_empty() {
            return new;
        }
        let mut tasks = Vec::with_capacity(now.tasks.len() + self.plan.tasks.len());
        for t in &now.tasks {
            match self.plan.get(&t.id) {
                Some(known) => tasks.push(known.clone()),
                None => tasks.push(t.clone()),
            }
        }
        for t in &self.plan.tasks {
            if now.get(&t.id).is_none() {
                tasks.push(t.clone());
            }
        }
        self.plan.tasks = tasks;
        for id in &new {
            if self.plan.get(id).is_some_and(|t| t.checked) {
                self.set_state(id, DONE_PREVIOUSLY);
                continue;
            }
            if !self.make_worktree(id) {
                self.fail_task(id, "no worktree could be made for it");
                continue;
            }
            self.set_state(id, PENDING);
            warn(format!(
                "task {id}: added to the plan while the run is live -- pending"
            ));
        }
        new
    }

    /// Once a poll: the live plan put where everything that is not this
    /// process reads it.
    ///
    /// `<run dir>/plan.md` is written at setup and read after that by
    /// `status`, `wait` and `reap`; `<task>.verify` is written at dispatch
    /// and read by the pre-commit hook inside a live worker's worktree. So
    /// four amendments to a plan mid-run reached neither, and a worker had to
    /// rewrite its own Verify line by hand for the hook to let its commit
    /// through. Both are brought up to the plan as it reads now, for every task
    /// a worker still has.
    ///
    /// A live plan that does not parse is said here too. `plan::parse`
    /// answers `None` for the whole document when one appended task is
    /// missing a Files: line or names a dependency that is not there, which
    /// voids the re-read and leaves the run on the copy it started with --
    /// silently, and for the rest of the run. Said once per text: the
    /// orchestrator is editing, and every poll would be a wall.
    fn refresh(&mut self) {
        let Some(text) = self.plan_text() else {
            return;
        };
        let parsed = plan::parse(&text, true).filter(|p| p.plan_id == self.plan.plan_id);
        let Some(parsed) = parsed else {
            if self.unparsable != text {
                self.unparsable = text;
                let why = plan::first_complaint()
                    .unwrap_or_else(|| format!("it is not the plan {}", self.plan.plan_id));
                warn(format!(
                    "run {}: the live plan does not parse ({why}); the run is still using the copy it started with",
                    self.plan.plan_id
                ));
            }
            return;
        };
        self.unparsable.clear();
        let file = self.dir.join("plan.md");
        if std::fs::read_to_string(&file).unwrap_or_default() != text {
            let _ = std::fs::write(&file, &text);
        }
        for task in self.dispatched() {
            if let Some(t) = parsed.get(&task) {
                write_field(
                    &self.dir,
                    &task,
                    "verify",
                    t.verify.as_deref().unwrap_or(""),
                );
                write_field(&self.dir, &task, "files", t.files.as_deref().unwrap_or(""));
            }
        }
    }

    /// The live plan as it reads right now: the `--plan-file`, else
    /// mem's plan. What `task_now` parses and the brief carries.
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

    /// Of `ids`, the FAILED tasks still waiting on a question with no
    /// answer: the poll loop keeps the run open for them rather than ending
    /// it and leaving the answer nowhere to land.
    fn waiting(&self, ids: &[String]) -> Vec<String> {
        ids.iter()
            .filter(|id| self.state(id) == FAILED)
            .filter(|id| self.question_open(id))
            .cloned()
            .collect()
    }

    /// Whether `task`'s failure note names a question that still holds the
    /// run open: mem lists it for the task and it carries no answer yet.
    /// An id mem never lists for this task -- a friction id quoted in the
    /// same blocked line, another task's or project's question, the same
    /// eight-character shape -- is not an unanswered question: no answer
    /// can ever land on it. It gets the reindex lag [`Self::question_in`]
    /// describes -- a few polls where a real question is briefly missing
    /// -- but past that bound it stops holding the run open, and the run is
    /// told the id is one no answer can land on rather than left reading a
    /// line that promises to wait. Asked once a poll, never twice: at two calls
    /// a pass the bound is half what it says.
    fn question_open(&self, task: &str) -> bool {
        let Some(qid) = self.asked(task) else {
            return false;
        };
        // A mem that could not answer has said nothing about the question,
        // and a miss spent on mem's own trouble is a question given up on.
        let Some(listed) = memcli::questions_for(&self.task_tag(task)) else {
            return true;
        };
        match listed.into_iter().find(|q| q.short_id == qid) {
            Some(q) => {
                write_field(&self.dir, task, "qmiss", &format!("{qid} 0"));
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
                if misses == self.question_misses + 1 {
                    warn(format!(
                        "task {task}: #{qid} is not a question mem lists for this task -- the run ends; answer it and run again"
                    ));
                }
                misses <= self.question_misses
            }
        }
    }

    /// Has mem listed `task`'s question at least once? Until it has, the run
    /// cannot say it stays open for it: [`Self::question_open`] gives up on
    /// an id that never appears, and the run ends instead.
    fn question_listed(&self, task: &str, qid: &str) -> bool {
        self.field(task, "qmiss") == format!("{qid} 0")
    }

    /// The failure note for a worker that stopped on a question, as
    /// `asked #<id>: <what>`, or nothing when its `blocked` line names no
    /// question and mem lists none pending for the task. Every `#id` in the
    /// note is a candidate, in the order it was written: a ruling or friction
    /// id quoted in the ask body has the same shape as a question id, and
    /// taking the first one alone keyed the task to an id no answer would
    /// ever land on. The first one mem lists for this task wins, unanswered
    /// before answered -- an answer that landed while the worker was ending is
    /// still the question it asked; with none of them listed the task's own
    /// newest open question does, and only then the first id the worker named,
    /// whose listing may be a reindex behind.
    fn question_in(&self, task: &str, state: &str, note: &str) -> Option<(String, bool)> {
        let blocked = state == "blocked";
        let named: Vec<&str> = note
            .split(|c: char| c.is_whitespace() || c == ',' || c == ';' || c == ')')
            .filter_map(|w| w.strip_prefix('#'))
            .map(|w| w.trim_end_matches(|c: char| !c.is_ascii_alphanumeric()))
            .filter(|w| w.len() == 8 && w.chars().all(|c| c.is_ascii_alphanumeric()))
            .collect();
        let listed = memcli::questions_for(&self.task_tag(task)).unwrap_or_default();
        let open = |q: &&memcli::Question| q.answer.is_none();
        // An answered id named by a report that is not `blocked` is
        // provenance -- "done per #ID" -- not an ask: reading it as one
        // re-asked three settled questions off one ready note and sent the
        // orchestrator three wakeups for nothing.
        if let Some(q) = named
            .iter()
            .find_map(|id| listed.iter().filter(open).find(|q| q.short_id == *id))
            .or_else(|| {
                blocked.then(|| {
                    named
                        .iter()
                        .find_map(|id| listed.iter().find(|q| q.short_id == *id))
                })?
            })
        {
            return Some((
                format!("asked #{}: {}", q.short_id, q.title),
                q.answer.is_some(),
            ));
        }
        if let Some(q) = listed.iter().find(open) {
            return Some((format!("asked #{}: {}", q.short_id, q.title), false));
        }
        if !blocked {
            return None;
        }
        named
            .first()
            .map(|id| (format!("asked #{id}: {note}"), false))
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
    /// binaries whose baked-in paths pointed at reaped worktrees. An inherited
    /// CARGO_TARGET_DIR is overridden for the same reason. Cleanup takes the
    /// whole plan's dirs down with the worktrees.
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

    /// The last line the worker reported in its own status file, as
    /// (state, note). Lines read `<utc> <state> <note...>`.
    fn last_status_line(&self, task: &str) -> Option<(String, String)> {
        let text = std::fs::read_to_string(self.dir.join(format!("{task}.status"))).ok()?;
        last_status_in(&text)
    }

    /// One attempt marker appended to the status file, never a truncation:
    /// the gate reads only what came after the last marker, and what the
    /// last attempt said stays readable where it was said (a worker that found
    /// its file emptied concluded the run dir had been reset and re-surveyed
    /// the tree).
    fn mark_status(&self, task: &str, marker: &str) {
        use std::io::Write;
        let status = self.dir.join(format!("{task}.status"));
        let _ = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&status)
            .and_then(|mut f| f.write_all(format!("--- {marker} ---\n").as_bytes()));
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

    /// Work in the task's worktree that no commit has: written, tracked or
    /// not, and never handed to git. An attempt that ends this way has not
    /// left nothing, whatever its branch and its status file say, so the
    /// tree is kept for the next attempt rather than deleted out from under
    /// it.
    fn uncommitted(&self, task: &str) -> bool {
        let wt = self.worktree(task);
        wt.is_dir()
            && !Git::at(&wt)
                .out(&["status", "--porcelain"])
                .unwrap_or_default()
                .trim()
                .is_empty()
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
    /// a worker is sent to build it again.
    fn landed(&self, task: &str) -> bool {
        let git = self.git();
        [self.prior_sha(task), self.field(task, "leftover")]
            .iter()
            .any(|sha| !sha.is_empty() && git.is_ancestor(sha, &self.int_branch))
    }

    /// The backend's word, or a message just sent: for a few seconds after
    /// `continue_worker` the pane still reads as idle while claude takes the
    /// paste, and a worker collected in that window would be judged on an
    /// empty status file.
    fn alive(&self, task: &str) -> bool {
        if self.backend.alive(&self.handle(task)) {
            return true;
        }
        let sent: i64 = self.field(task, "continued_at").parse().unwrap_or(0);
        sent > 0 && sys::now() - sent < CONTINUE_GRACE_S && self.last_status_line(task).is_none()
    }

    /// A failed task whose branch holds commits is not gone: the gate itself
    /// can be why it failed, and the work is still there to build on (ruling
    /// 4). Nothing else counts -- a task a run left `dispatched` when it died
    /// has no verdict on it at all, and is adopted, not resumed.
    pub fn resumable(&self, task: &str) -> bool {
        self.state(task) == FAILED && self.commits(task) > 0
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

    /// Alive, or its pane standing, with no final word past `started` or
    /// `progress`, and the provider's own limit named on the pane's
    /// question, in the worker's last words or in its recent output. The
    /// usage limit holds a session without ending it and the session comes
    /// back by itself when the window opens, so such a worker is waiting,
    /// not stalled, whatever the deadline says. Holding commits changes
    /// nothing: a worker with work on its branch hits the window as readily
    /// as one without.
    ///
    /// While it holds, `<task>.limited_at` is stamped, and [`stalled`] counts
    /// the deadline from it: a worker that speaks again gets a whole clock.
    ///
    /// The limit has to be said out loud. Every other turn that ends with an
    /// empty status file leaves the same shape -- a provider cutting a
    /// worker off mid-reasoning, a stream that ended without a finish reason
    /// -- and nothing is coming back for those, so they are collected and
    /// `finish` reports each with the last thing the pane said.
    ///
    /// Listed, not merely seen: a session that died with the machine has a
    /// transcript and no row, and is gone, not waiting.
    fn rate_limited(&self, task: &str) -> bool {
        if let Some((state, _)) = self.last_status_line(task)
            && state != "started"
            && state != "progress"
        {
            return false;
        }
        let h = self.handle(task);
        if !self.backend.alive(&h) && !self.backend.listed(&h) {
            return false;
        }
        let limited = backend::provider_limit(&self.backend.question(&h)).is_some()
            || backend::provider_limit(&self.backend.last_words(&h)).is_some()
            || backend::provider_limit(&self.backend.recent_output(&h)).is_some();
        if limited {
            write_field(&self.dir, task, "limited_at", &sys::now().to_string());
        }
        limited
    }

    fn stop(&self, task: &str) {
        self.backend.stop(&self.handle(task), self.kill_grace_s);
    }

    /// A session a message can reach: its pane standing, or parked, which
    /// the backend brings back before it sends.
    fn reachable(&self, h: &Handle) -> bool {
        self.backend.listed(h) || self.backend.parked(h)
    }

    // ---------------------------------------------------------------- workers

    /// Bring the task's worktree up to the integration branch before the worker
    /// sees it.
    ///
    /// Every worktree is cut from the run's base at setup, so a task whose
    /// dependency merged before it dispatches opens onto a tree without it.
    /// The worker's only way out was to go and find the integration branch
    /// itself, which nothing in its brief mentions. This fast-forward puts the
    /// work it builds on simply there.
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
        let Some(tip) = self.git().rev_parse_commit(&self.int_branch) else {
            return;
        };
        if git.head().as_deref() == Some(tip.as_str()) {
            return; // already there, and on a task's first attempt it always is
        }
        if !git.quiet(&["merge", "-q", "--ff-only", &tip]) {
            warn(format!(
                "task {task}: its worktree keeps the commits it already has, so {} was not brought in",
                self.int_branch
            ));
            return;
        }
        // What merged may have moved a submodule, or added one.
        repo::submodules(&self.repo, &wt);
    }

    /// What the attempt before this one came to, for the brief to carry.
    ///
    /// `why` is what this caller knows and the run dir does not: a stall and a
    /// ghost session are both redispatches nothing has marked failed, so the
    /// failure note is empty and only the caller can say what happened.
    /// Everything else is read here, before the dispatch truncates it.
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
            .unwrap_or_default()
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

    /// `workflow accept <task>`, honoured: the run settled the task as
    /// failed, its branch holds the diff, and the orchestrator is landing it
    /// as it stands. The merge runs again -- ownership, the words, the
    /// rebase, the gate's suite.
    fn accept(&self, task: &str) {
        warn(format!("task {task}: accepted by request -- merging it"));
        match self.merge(task) {
            Ok(Merge::Landed) => self.land(task),
            Ok(Merge::Nothing) => {
                self.fail_task(task, "accepted, but its branch holds nothing to merge")
            }
            Err(why) => self.fail_task(task, &why),
        }
    }

    /// `branch` merged whole onto the integration branch's tip, detached, as
    /// one commit carrying the branch tip's message: the three-way merge
    /// sees the branch's final tree where a replay sees each commit. `false`
    /// leaves `int` detached at the integration tip with nothing staged.
    fn squash_onto(&self, int: &Git, branch: &str) -> bool {
        if !int.quiet(&["checkout", "-q", "--detach", &self.int_branch]) {
            return false;
        }
        let message = self
            .git()
            .out(&["log", "-1", "--format=%B", branch])
            .unwrap_or_default();
        let merged = int.quiet(&["merge", "-q", "--squash", branch])
            && !int.quiet(&["diff", "--cached", "--quiet"])
            && int.quiet(&["commit", "-q", "--no-verify", "-m", message.trim()]);
        if !merged {
            int.quiet(&["reset", "-q", "--hard", &self.int_branch]);
        }
        merged
    }

    /// Why a task that could go is not going yet, for `workflow status`:
    /// a pending task with an empty last report said nothing about what
    /// held it. Cleared by the next state change.
    fn hold(&self, task: &str, why: &str) {
        if self.field(task, "held") != why {
            write_field(&self.dir, task, "held", why);
        }
    }

    /// `workflow regate <task>`, honoured: the merge a worker's `ready`
    /// starts, run again on the branch as it stands -- the suite included --
    /// with no worker spent, for a gate red twice on a test that is green
    /// alone.
    fn regate(&self, task: &str) {
        warn(format!("task {task}: gated again by request"));
        match self.merge(task) {
            Ok(Merge::Landed) => self.land(task),
            Ok(Merge::Nothing) => self.fail_task(task, "its branch holds nothing to merge"),
            Err(why) => self.fail_task(task, &why),
        }
    }

    /// One line into the worker's own session after a turn that ended with
    /// nothing to judge, once per attempt. `false` when the nudge has been
    /// spent, there is no session standing to send to, the window is past
    /// [`CONTINUE_MAX_TOKENS`], or the backend did not see the line taken;
    /// under reap nothing is sent, since reap dispatches nothing. Counted in
    /// `<task>.nudged`, apart from `continued`, and the stall clock and the
    /// send grace start over as they do for a continuation.
    fn nudge_worker(&self, task: &str) -> bool {
        if self.collecting || self.field(task, "nudged").parse::<u64>().unwrap_or(0) >= 1 {
            return false;
        }
        let h = self.handle(task);
        if h.session.is_empty() || !self.reachable(&h) {
            return false;
        }
        if self
            .backend
            .context_tokens(&h)
            .is_some_and(|t| t > CONTINUE_MAX_TOKENS)
        {
            return false;
        }
        if !self.backend.send(&h, NUDGE_LINE, self.kill_grace_s) {
            return false;
        }
        write_field(&self.dir, task, "nudged", "1");
        write_field(&self.dir, task, "continued_at", &sys::now().to_string());
        write_field(&self.dir, task, "dispatched_at", &sys::now().to_string());
        self.set_state(task, DISPATCHED);
        warn(format!(
            "task {task}: its turn ended without a report -- nudged once in its session (session {})",
            h.session
        ));
        memcli::log_run(&format!("run {}: nudged {task}", self.plan.plan_id));
        true
    }

    /// The task back to the worker that has it, in the session it has: the
    /// brief is rewritten with what happened to its last report -- the
    /// orchestrator's answer -- and one line goes into
    /// the pane pointing at it. Nothing about the attempt count moves; the
    /// status file is emptied so the gate judges this report and not the
    /// last one, and the stall clock starts over.
    ///
    /// `false` when there is no session to send to, the window is past
    /// [`CONTINUE_MAX_TOKENS`], or the backend did not see the message taken
    /// -- and in that last case the session is stopped first, since a paste
    /// that may have landed cannot be left beside a fresh worker in the same
    /// tree. The caller dispatches afresh on `false`.
    /// The stall deadline in whole minutes, rounded up, for the brief's
    /// TIMEBOX: a deadline of 30 seconds is a minute, never none.
    fn deadline_minutes(&self) -> u64 {
        (self.deadline_s.max(1) as u64).div_ceil(60)
    }

    fn continue_worker(&self, task: &str, why: &str) -> bool {
        let h = self.handle(task);
        if h.session.is_empty() || !self.reachable(&h) {
            return false;
        }
        if self
            .backend
            .context_tokens(&h)
            .is_some_and(|t| t > CONTINUE_MAX_TOKENS)
        {
            warn(format!(
                "task {task}: its worker's window is past {CONTINUE_MAX_TOKENS} tokens -- a fresh session instead"
            ));
            return false;
        }
        let Some(t) = self.task_now(task) else {
            return false;
        };
        let wt = self.worktree(task);
        let brief_file = self.brief_dir.join(format!("{task}.md"));
        let status = self.dir.join(format!("{task}.status"));
        let prior = self.prior_attempt(task, why);
        let prose = plan::prose(&self.plan_text().unwrap_or_default());
        let pages = wiki_pages(&t);
        brief::write(
            &t,
            &wt,
            &status,
            &prior,
            &prose,
            &pages,
            self.deadline_minutes(),
            &brief_file,
        );
        // The hook in that worktree reads this file, not the brief: a task
        // sent back to its own worker was still held to the Verify line of
        // the plan as it read at dispatch.
        write_field(&self.dir, task, "verify", t.verify.as_deref().unwrap_or(""));
        write_field(&self.dir, task, "files", t.files.as_deref().unwrap_or(""));
        let n: u64 = self.field(task, "continued").parse().unwrap_or(0) + 1;
        self.mark_status(task, &format!("continued {n}"));
        let line = format!(
            "Read {} again: it now says what happened to your last report and what to do \
             next. Do that, then report as it says.",
            brief_file.display()
        );
        if !self.backend.send(&h, &line, self.kill_grace_s) {
            self.stop(task);
            return false;
        }
        write_field(&self.dir, task, "continued", &n.to_string());
        write_field(&self.dir, task, "continued_at", &sys::now().to_string());
        write_field(&self.dir, task, "dispatched_at", &sys::now().to_string());
        self.set_state(task, DISPATCHED);
        warn(format!(
            "task {task}: sent back to its worker (session {}, continuation {n})",
            h.session
        ));
        memcli::log_run(&format!("run {}: continued {task}", self.plan.plan_id));
        true
    }

    fn dispatch(&self, task: &str, after: &str) {
        let Some(t) = self.task_now(task) else {
            self.fail_task(task, "the live plan no longer holds this task");
            return;
        };
        // The pane the last attempt had, still on the backend's books: amx
        // parks an idle one and takes its time releasing it, and the fresh
        // session minted two seconds later came up in a pane the park had
        // not finished with -- "dispatch race: the worker never started",
        // with the attempt's worktree already gone. Stop it, then wait for the
        // listing to go before minting anything, up to the kill grace.
        let old = self.handle(task);
        if !old.session.is_empty() && self.backend.seen(&old) {
            self.stop(task);
            let give_up = sys::now() + self.kill_grace_s;
            while self.backend.listed(&old) && sys::now() < give_up {
                sys::sleep(0.2);
            }
        }
        self.catch_up(task);
        self.own_deps(task);
        let wt = self.worktree(task);
        let brief_file = self.brief_dir.join(format!("{task}.md"));
        let status = self.dir.join(format!("{task}.status"));
        let session = self.backend.mint_session();
        let prior = self.prior_attempt(task, after);

        write_field(&self.dir, task, "session", &session);
        // Marked, never truncated: the gate reads this file after the last
        // marker to judge THIS attempt, so a stale `ready` from the last one
        // cannot pass for it, and what it said stays where it said it.
        self.mark_status(task, &format!("attempt {}", prior.attempts + 1));
        write_field(&self.dir, task, "nudged", "0");
        for ext in ["json", "err", "pid"] {
            let _ = std::fs::remove_file(self.dir.join(format!("{task}.{ext}")));
        }
        // The live plan, read now rather than at setup: an edit the
        // orchestrator makes mid-run is in the next attempt's brief.
        let prose = plan::prose(&self.plan_text().unwrap_or_default());
        let pages = wiki_pages(&t);
        brief::write(
            &t,
            &wt,
            &status,
            &prior,
            &prose,
            &pages,
            self.deadline_minutes(),
            &brief_file,
        );
        // The gate reads this from inside the worktree: the task is held to its
        // own Verify command there, not to the repo-wide suite (verify.rs).
        write_field(&self.dir, task, "verify", t.verify.as_deref().unwrap_or(""));
        // And its Files line, which the pre-commit hook holds a commit to.
        write_field(&self.dir, task, "files", t.files.as_deref().unwrap_or(""));

        let n = prior.attempts;
        write_field(&self.dir, task, "dispatches", &(n + 1).to_string());
        write_field(&self.dir, task, "dispatched_at", &sys::now().to_string());

        let mut env = self.env.clone();
        env.extend(self.cargo_env(task));
        // How a worker's `mem ask` knows it is a worker's: mem reads this, or
        // the worktree path, and addresses the question to the orchestrator.
        env.push(("WORKFLOW_TASK".into(), self.task_tag(task)));
        // Scratch space of its own: a worker clearing /tmp/wf-test.* took a
        // sibling's suite sandboxes with it.
        let tmp = self.dir.join(format!("{task}.tmp"));
        let _ = std::fs::create_dir_all(&tmp);
        env.push(("TMPDIR".into(), tmp.display().to_string()));
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
            parent: None,
            role: "worker".to_string(),
            model: match self.field(task, "model").trim() {
                "" => self.model.clone(),
                m => m.to_string(),
            },
            // A plan marks its few hard tasks with a level of their own; the
            // run's dial stands for every other.
            effort: t.effort.clone().or_else(|| self.effort.clone()),
            turns: env_str("WORKFLOW_MAX_TURNS", "120"),
            env,
        };

        // Recorded before the worker exists, so a run that dies between here
        // and the next line leaves a task that is plainly mid-dispatch rather
        // than one that looks untouched.
        self.set_state(task, DISPATCHED);
        let handle = self.backend.dispatch(&d);
        // No handle is a launch the backend refused -- amx at its cap, or a
        // tmux it cannot reach -- and there is no worker to wait on. What it
        // said on its way out is in the err file, so the task fails on that
        // line rather than sitting out a stall deadline for a pane that
        // never came up.
        if handle.is_empty() {
            let said = std::fs::read_to_string(&d.err).unwrap_or_default();
            let line = said
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("nothing on stderr");
            self.fail_task(task, &format!("the launch was refused: {line}"));
            return;
        }
        // What the backend actually started, which is the handle everything
        // that asks after this worker later -- liveness, the stop, the
        // transcript -- reads out of this file.
        write_field(&self.dir, task, "session", &handle);
        warn(format!("task {task}: dispatched (session {handle})"));
        memcli::log_run(&format!("run {}: dispatched {task}", self.plan.plan_id));
    }

    /// What this attempt was carrying when it stopped, kept for the run's
    /// closing report. Feedback on how the plan was cut, never a ceiling:
    /// the plan is flat-rate and context is the resource one-task-per-session
    /// already manages.
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
        // applied and call the answer a conflict. The intent line written below
        // says which commit was going on, which is what tells an applied merge
        // from a real one.
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
        // worked.
        let found = ownership::violations(&wt, &self.int_branch, &branch, &patterns);
        if !found.uncommitted.is_empty() {
            warn(format!(
                "task {task}: left writes outside its Files in the worktree; no commit carries them, so they go with it --"
            ));
            for line in ownership::show(&found.uncommitted) {
                warn(format!("  {line}"));
            }
        }
        if !found.committed.is_empty() {
            let shown = ownership::show(&found.committed);
            warn(format!("task {task}: touched files it does not own --"));
            for line in &shown {
                warn(format!("  {line}"));
            }
            // The reason is what the failure note, the mem log line and the
            // next attempt's brief carry; a bare sentence left the reader
            // with nothing to act on.
            let more = match shown.len().saturating_sub(5) {
                0 => String::new(),
                n => format!(" and {n} more"),
            };
            return Err(format!(
                "wrote outside its Files: patterns -- {}{more}",
                shown.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
            ));
        }

        // The same anchor again: a branch that took integration in to reach a
        // dependency would otherwise be held to its siblings' commit messages
        // as well as its own. Oldest first, one commit at a time, so the
        // failure names the commit to reword. A merge is left out: git wrote
        // its message, and that message names both branches, slugs and all.
        let log = self.git().bytes(&[
            "log",
            "-z",
            "--reverse",
            "--no-merges",
            "--format=%h%n%B",
            &format!("{}..{branch}", self.int_branch),
        ]);
        for commit in log.split(|b| *b == 0).filter(|c| !c.is_empty()) {
            let commit = String::from_utf8_lossy(commit);
            let (sha, msg) = commit.split_once('\n').unwrap_or((&commit, ""));
            let scope = hygiene::Scope {
                staged: false,
                tree: false,
                history: None,
                message: None,
                string: Some(msg),
            };
            if hygiene::cmd_hygiene(scope, None, false, false) != exit::OK {
                return Err(format!(
                    "commit {sha} \"{}\" did not pass hygiene; reword its message",
                    msg.lines().next().unwrap_or_default()
                ));
            }
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
            let conflicted = int
                .out(&["diff", "--name-only", "--diff-filter=U"])
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(", ");
            int.quiet(&["rebase", "--abort"]);
            // Replayed a commit at a time, an early commit can meet a hunk
            // the branch's own tip already agrees with integration on: the
            // worker had redone its change on integration's file, and the
            // task failed twice with nothing to act on.
            // The branch merged whole is the other way to lay it on top, as
            // one commit under the tip's message.
            if !self.squash_onto(&int, &branch) {
                int.quiet(&["checkout", "-q", &self.int_branch]);
                return Err(format!(
                    "conflicts with the integration branch in {} -- replayed a commit at a time and merged whole, both conflict; rebase {branch} onto {} and resolve them",
                    if conflicted.is_empty() {
                        "its files"
                    } else {
                        conflicted.as_str()
                    },
                    self.int_branch
                ));
            }
            warn(format!(
                "task {task}: its commits do not replay onto {}, but the branch merges whole -- landing it as one commit",
                self.int_branch
            ));
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

        if let Err(why) = self.gate_verify(task) {
            self.unwind(task, &prev);
            return Err(why);
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
        let why = match self.gate_verify(task) {
            Ok(()) => {
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

    /// Integration back to where it stood before this task's fast-forward,
    /// and the intent line cleared: the merge did not happen.
    fn unwind(&self, task: &str, prev: &str) {
        Git::at(&self.int_wt).quiet(&["reset", "-q", "--hard", prev]);
        write_field(&self.dir, task, "merging", "");
    }

    /// The bookkeeping of a merge that is final: state, tick, log, and the
    /// worker's session stopped -- it has nothing more to do.
    fn land(&self, task: &str) {
        self.stop(task);
        self.set_state(task, MERGED);
        warn(format!("task {task}: merged onto {}", self.int_branch));
        self.event(&format!("merged {task}"));
        self.tick_off(task);
        // The pages the task's Read named ride on the merge line, so a page
        // a merged task may have falsified can be found from the run log
        // alone (m4-lines).
        let pages = self
            .task_now(task)
            .map(|t| t.wiki_slugs())
            .unwrap_or_default();
        let named = if pages.is_empty() {
            String::new()
        } else {
            format!(" -- pages named: {}", pages.join(", "))
        };
        memcli::log_run(&format!("run {}: merged {task}{named}", self.plan.plan_id));
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
    /// The suite on the tree the run starts from, unless verify's green set
    /// already holds that tree -- every green verify records the tree it
    /// proved, so a trunk the gate just merged onto is known.
    /// A red trunk fails every task at the gate, each after a whole worker,
    /// and one red suite before the first dispatch is what that costs
    /// instead (2026-09-14: a4bb5fa reddened three tasks in a row for 45
    /// minutes before anyone read the gate). `Err` names the failing checks.
    /// Why the backend would refuse to start a worker on the run's model
    /// and effort, as the lines to say; `None` when it would start one.
    fn unstartable(&self) -> Option<String> {
        let why = self
            .backend
            .check(&self.repo, "worker", &self.model, self.effort.as_deref())
            .err()?;
        Some(format!(
            "run {}: the workers would run on {}, and amx will not start that -- {why}\n\
             name another with `workflow run --model <name>`, which rewrites this plan's record, \
             or with WORKFLOW_MODEL in the environment",
            self.plan.plan_id, self.model
        ))
    }

    fn trunk_green(&self) -> Result<(), String> {
        let int = Git::at(&self.int_wt);
        let tree = int.out(&["rev-parse", "HEAD^{tree}"]).unwrap_or_default();
        let tree = tree.trim().to_string();
        let project = memcli::project_current();
        if verify::is_green(project.as_ref(), &tree) {
            return Ok(());
        }
        let tip = int.head().unwrap_or_default();
        // A repository with no suite yet -- a first commit, a plan whose
        // first task adds the tests -- has nothing a trunk check could prove,
        // and refusing it made the plan that adds the suite unrunnable.
        // Each task's gate still asks for one.
        if verify::detect_verifiers(&self.int_wt, project.as_ref()).is_empty() {
            warn(format!(
                "run {}: {} has no suite to run yet -- nothing to prove before the first dispatch; each task's gate needs the suite the plan adds",
                self.plan.plan_id,
                &tip[..tip.len().min(12)]
            ));
            return Ok(());
        }
        warn(format!(
            "run {}: the gate has not seen {} green -- running the suite once before the first dispatch",
            self.plan.plan_id,
            &tip[..tip.len().min(12)]
        ));
        // Red is run once more, as a task's gate is: one timed-out test
        // refused a trunk that was green alone, and refusing the run is dearer
        // than one more suite.
        let file = self.dir.join("base.gate");
        let kept = self.dir.join("base.gate.1");
        let Err(red) = self.gate_run(&file) else {
            let _ = std::fs::remove_file(&kept);
            return Ok(());
        };
        if std::fs::rename(&file, &kept).is_err() {
            return Err(red);
        }
        warn(format!(
            "run {}: the trunk was red -- running the suite a second time before calling it",
            self.plan.plan_id
        ));
        self.gate_run(&file)?;
        let line = format!(
            "run {}: the trunk was red once and green on the second run -- see {}",
            self.plan.plan_id,
            kept.display()
        );
        warn(&line);
        memcli::log_run(&line);
        Ok(())
    }

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
        memcli::log_run(&line);
        Ok(())
    }

    /// One run of the gate, both streams into `file`. `Err` is the reason the
    /// merge cannot stand, naming the checks that broke out of what the run
    /// left there.
    fn gate_run(&self, file: &Path) -> Result<(), String> {
        // The merge just landed may have changed the lockfile or moved a
        // submodule; the suite that judges it runs against both as merged.
        repo::submodules(&self.repo, &self.int_wt);
        self.node_deps(&self.int_wt);
        let stdout = std::fs::File::create(file)
            .map_err(|e| format!("cannot write {} ({e})", file.display()))?;
        let stderr = stdout
            .try_clone()
            .map_err(|e| format!("cannot write {} ({e})", file.display()))?;

        let mut c = Command::new(&self.exe);
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
        // A group of its own, so a hung suite goes whole: its tests and
        // whatever they spawned, not only the verify process.
        std::os::unix::process::CommandExt::process_group(&mut c, 0);
        let mut child = c.spawn().map_err(|e| {
            format!(
                "could not run {} verify --gate in {}: {e}",
                self.exe.display(),
                self.int_wt.display()
            )
        })?;
        let pid = child.id().to_string();
        let started = sys::now();
        let status = loop {
            if let Ok(Some(status)) = child.try_wait() {
                break Some(status);
            }
            if sys::now() - started >= self.gate_s {
                sys::kill_group(&pid, "TERM");
                sys::sleep(2.0);
                sys::kill_group(&pid, "KILL");
                let _ = child.wait();
                break None;
            }
            sys::sleep(0.5);
        };
        let Some(status) = status else {
            let limit = match self.gate_s % 60 {
                0 => format!("{} min", self.gate_s / 60),
                _ => format!("{} s", self.gate_s),
            };
            return Err(format!(
                "the gate ran past its {limit} deadline and was stopped (WORKFLOW_GATE_MIN) -- the last lines of {} say what was running",
                file.display()
            ));
        };
        if status.success() {
            return Ok(());
        }
        let text = std::fs::read_to_string(file).unwrap_or_default();
        let checks = failing_checks(&text);
        let named = if checks.is_empty() {
            String::new()
        } else {
            format!(": {}", checks.join(", "))
        };
        let hint = path_hint(&text, &self.wt_root);
        let timed = timeout_hint(&text);
        Err(format!(
            "the suite is red once the change sits on integration{named}{timed}{hint} -- see {}",
            file.display()
        ))
    }

    /// Tick the task off where the plan came from. mem holds the plan a plain
    /// `workflow run` reads, but a `--plan-file` run's plan need not be in mem
    /// at all -- and asking mem to tick a plan it does not have left the merge
    /// recorded nowhere, with the ticking left to whoever was watching.
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
        // A file that is a copy of mem's plan -- the restart shape,
        // where a run is started again off `mem plan > file` -- ticks mem too,
        // or the milestone reads as untouched there.
        if self.mem_holds_this_plan() && !memcli::plan_tick(task) {
            warn(format!(
                "task {task}: mem holds this plan as its live plan and could not tick it off"
            ));
        }
    }

    /// Whether mem's live plan is this plan: the same slug in its header. A
    /// `--plan-file` run of that plan is mem's plan run from a copy, and its
    /// ticks belong in mem and the roadmap as much as the file.
    fn mem_holds_this_plan(&self) -> bool {
        self.plan_file.is_some()
            && memcli::plan()
                .and_then(|text| plan::slug_of(&text))
                .is_some_and(|slug| slug == self.plan.plan_id)
    }

    /// Land the integration branch on the checkout's own branch when that is
    /// a fast-forward the checkout can take: HEAD on a branch, no local
    /// changes, and integration strictly ahead of it. `Ok` says where it
    /// landed; `Err` says why it did not, for the recipe. Nothing is pushed
    /// either way.
    fn land_trunk(&self) -> Result<String, String> {
        let git = Git::at(&self.repo);
        let Some(branch) = git.out(&["symbolic-ref", "--quiet", "--short", "HEAD"]) else {
            return Err("the checkout is not on a branch".to_string());
        };
        let Some(head) = git.head() else {
            return Err("the checkout has no HEAD".to_string());
        };
        let Some(tip) = git.rev_parse_commit(&self.int_branch) else {
            return Err(format!("{} does not exist", self.int_branch));
        };
        if tip == head {
            // Landed by hand already, the recovery shape: not news, not a task.
            return Ok(format!("{branch}, already at {}", &tip[..7]));
        }
        if !git.is_ancestor(&head, &self.int_branch) {
            return Err(format!(
                "{branch} has commits {} does not, so a fast-forward cannot take it: merge or rebase by hand",
                self.int_branch
            ));
        }
        if git
            .out(&["status", "--porcelain", "--untracked-files=no"])
            .is_some()
        {
            return Err(format!("{branch} has local changes"));
        }
        if !git.quiet(&["merge", "--ff-only", &self.int_branch]) {
            return Err(format!("git merge --ff-only {} failed", self.int_branch));
        }
        // A pin the run moved is only a pointer until the submodule is
        // checked out at it: the checkout's own verify went on testing the
        // old engine and said green.
        let behind = moved_submodules(&git, &head, &tip)
            .into_iter()
            .filter(|path| {
                !git.quiet(&["submodule", "update", "--init", "--recursive", "--", path])
                    && !(git.quiet(&["-C", path, "fetch", "-q"])
                        && git.quiet(&["submodule", "update", "--init", "--recursive", "--", path]))
            })
            .collect::<Vec<_>>();
        if !behind.is_empty() {
            warn(format!(
                "landed on {branch}, but {} could not be checked out at the pinned commit -- `git submodule update --init {}` before trusting a verify here",
                behind.join(", "),
                behind.join(" ")
            ));
        }
        Ok(format!("{branch} at {}", &tip[..7]))
    }

    /// Every task this run settled, ticked once more against the plan of
    /// record as it reads at the end. The plan is a live document -- read
    /// fresh at every dispatch, and edited mid-run by whoever answers the
    /// questions -- so an edit that reopens a box this run already ticked left
    /// the merge recorded nowhere, and the next run read the task as work
    /// still to do and built it again.
    fn tick_settled_again(&self) {
        // The file, and mem's copy too when the file is mem's live
        // plan: a box reopened in either is a merge the next run would redo.
        let mut records = vec![self.plan_text()];
        if self.mem_holds_this_plan() {
            records.push(memcli::plan());
        }
        let records: Vec<Plan> = records
            .into_iter()
            .flatten()
            .filter_map(|text| plan::parse(&text, false))
            .filter(|p| p.plan_id == self.plan.plan_id)
            .collect();
        for id in self.plan.ids() {
            let state = self.state(&id);
            if state != MERGED && state != DONE_PREVIOUSLY {
                continue;
            }
            let open = records
                .iter()
                .any(|record| record.get(&id).is_some_and(|task| !task.checked));
            if !open {
                continue;
            }
            self.tick_off(&id);
            let line = format!("task {id}: had come unticked in the live plan -- ticked again");
            warn(&line);
            memcli::log_run(&line);
        }
    }

    /// One line the orchestrator wants to hear about, appended to the run
    /// dir's `events` file for `workflow wait` to block on: a question, a
    /// task failed for good, a merge, the end of the run. Stderr says
    /// everything; this says what needs somebody.
    pub fn event(&self, line: &str) {
        let path = self.dir.join("events");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write;
            let _ = writeln!(f, "{} {line}", sys::utc_now());
        }
    }

    /// Failed for good as far as this run can tell: the worker's session is
    /// stopped with it, so a pane does not stand idle for an hour over a task
    /// nobody is sending back, and the orchestrator is told.
    fn fail_task(&self, task: &str, why: &str) {
        self.stop(task);
        warn(format!("task {task}: failed -- {why}"));
        self.set_state(task, FAILED);
        write_field(&self.dir, task, "failed", why);
        if self.commits(task) > 0 {
            // The branch survives cleanup, so the work is still reachable --
            // but only if the reader is told where.
            warn(format!("  its work is on the branch {}", self.branch(task)));
        }
        memcli::log_run(&format!(
            "run {}: failed {task} -- {why}",
            self.plan.plan_id
        ));
        self.event(&format!("failed {task} -- {why}"));
    }

    /// The last thing heard from a worker that ended, as a clause for its
    /// note: its own last words off the transcript, else -- a session that
    /// died before it said anything, which the two silent schema deaths of
    /// m08 were -- the last line the pane showed. With `keep`, the whole of
    /// what the pane showed goes into `<task>.err` for whoever reads the
    /// note; a redispatch clears that file on its way out, so the once-more
    /// path keeps nothing but the line. Empty when there is neither.
    fn last_heard(&self, task: &str, keep: bool) -> String {
        let h = self.handle(task);
        let said = self.backend.last_words(&h);
        let dying = match said.trim().is_empty() {
            true => self.backend.dying_words(&h),
            false => String::new(),
        };
        if keep && !dying.is_empty() {
            let err = self.field(task, "err");
            let kept = match err.is_empty() {
                true => dying.clone(),
                false => format!("{err}\n{dying}"),
            };
            write_field(&self.dir, task, "err", &kept);
        }
        last_heard_in(&said, &dying)
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
        // attempted, so it gets the one retry a silent stall already had.
        //
        // "Nothing" is the branch and the status file, and neither is the
        // whole tree: a worker that wrote for an hour and never committed
        // leaves its work in the worktree alone. Calling that nothing deleted
        // the worktree at cleanup and sent the next attempt to a tree with no
        // trace of the first, so it is said for what it is and the tree is
        // kept.
        // Before any of that, a turn that ended with nothing to judge while
        // the session still stands gets one line in that session; only when
        // that has been spent, or cannot be sent, is the ending judged.
        let nudged = self.field(task, "nudged").parse::<u64>().unwrap_or(0) >= 1;
        if self.commits(task) == 0
            && !self
                .last_status_line(task)
                .is_some_and(|(state, _)| state == "ready" || state == "blocked")
            && self.question_in(task, "", "").is_none()
            && self.nudge_worker(task)
        {
            return;
        }
        let tries: u64 = self.field(task, "dispatches").parse().unwrap_or(0);
        if self.last_status_line(task).is_none() && self.commits(task) == 0 && tries < 2 {
            let (what, after) = match (self.uncommitted(task), nudged) {
                (true, _) => (
                    "its worker ended leaving its work uncommitted",
                    "its worker ended without committing what it wrote; it is still in this worktree, so continue from there",
                ),
                (false, true) => (
                    "its worker ended twice without a report, once after a nudge",
                    "its worker ended its turn twice without writing anything, once after a nudge, and was dispatched again",
                ),
                (false, false) => (
                    "its worker died leaving nothing",
                    "its worker died before writing anything, and was dispatched again",
                ),
            };
            self.once_more(
                task,
                &format!("{what}{}", self.last_heard(task, false)),
                after,
            );
            return;
        }
        // A worker that stopped on a question is waiting on the
        // orchestrator, and the failure note says which question: the poll
        // loop watches it and dispatches the task again the moment an
        // answer lands, with the answer in the brief. The id comes off the
        // worker's own report first: mem's read verbs never wait on another
        // invocation's reindex, so a question written a moment ago can be
        // missing from the listing this once. Any ending but `ready` is
        // asked this: a turn that ended on `mem ask` under `progress`, or
        // with no report at all, is a question like any other, and failing
        // it over "no clean turn" threw away both the answer and the work
        // already on the branch.
        let last = self.last_status_line(task);
        if !last.as_ref().is_some_and(|(state, _)| state == "ready")
            && let Some((asked, answered)) = self.question_in(
                task,
                last.as_ref().map_or("", |(state, _)| state.as_str()),
                last.as_ref().map_or("", |(_, note)| note.as_str()),
            )
        {
            // Failed is the state the poll loop watches, not what happened:
            // "failed -- asked #X" read as a task the run had given up on,
            // and orchestrators redispatched by hand what the run was about
            // to send back in itself.
            warn(format!(
                "task {task}: stopped on its question -- {asked}; the answer goes back to it"
            ));
            self.set_state(task, FAILED);
            write_field(&self.dir, task, "failed", &asked);
            memcli::log_run(&format!(
                "run {}: {task} stopped on its question -- {asked}",
                self.plan.plan_id
            ));
            // A question that already has its answer wakes nobody: the
            // poll loop sends the answer back in by itself, and an event
            // here woke the orchestrator to answer what it had answered.
            if answered {
                warn(format!(
                    "task {task}: re-{asked} -- already answered, the answer goes back in"
                ));
                memcli::log_run(&format!(
                    "run {}: {task} re-{asked} -- already answered",
                    self.plan.plan_id
                ));
            } else {
                self.event(&format!("question {task} -- {asked}"));
            }
            return;
        }
        if !outcome.ok {
            // No status line and no commit: the dispatch never became a
            // worker, so the reason names the dispatch rather than sending
            // whoever reads it looking for a worker that never existed. One
            // that reported and then ended unclean -- an amx phase of failed
            // or stopped, a pane that vanished -- is named for what it did.
            // Either way with its last words when the transcript has any,
            // else what the pane showed.
            let heard = self.last_heard(task, true);
            let why = if self.last_status_line(task).is_none() && self.commits(task) == 0 {
                format!("dispatch race: the worker never started{heard}")
            } else {
                format!("the worker ended without a clean turn{heard}")
            };
            self.fail_task(task, &why);
            return;
        }
        match last {
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
                // wrong and one staring at a state nothing documents.
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
                // With whatever the pane had to say, because a turn cut off
                // by the provider ends cleanly as far as the backend can see
                // and the stop reason is the only account of it there is.
                let heard = self.last_heard(task, true);
                self.fail_task(
                    task,
                    &format!("the worker stopped without reporting ready{heard}"),
                );
                return;
            }
            Some(_) => {}
        }
        self.settle(task);
    }

    /// The gate, and what its answer means for the task: the tail of
    /// [`Self::finish`].
    fn settle(&self, task: &str) {
        match self.merge(task) {
            Ok(Merge::Landed) => self.land(task),
            // Ready with nothing committed: the worker found its Done already
            // satisfied -- rebuilt by hand between passes, or landed by an
            // earlier plan. Failing it skipped every dependent behind work
            // that exists.
            Ok(Merge::Nothing) => {
                self.set_state(task, DONE_PREVIOUSLY);
                warn(format!(
                    "task {task}: reported ready with nothing to commit -- its work is already in the tree"
                ));
                self.tick_off(task);
                memcli::log_run(&format!(
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
    /// a leftover branch for the next run to refuse on.
    /// Answers with how many were stopped.
    fn stop_settled_orphans(&self) -> usize {
        let mut stopped = 0;
        for task in self.plan.ids() {
            let state = self.state(&task);
            if state == DISPATCHED || state == PENDING || state.is_empty() {
                continue; // the reap pass and the ready-set loop own these
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
        let dispatched = self.dispatched();
        let mut limited = Vec::new();
        for task in dispatched.iter().cloned() {
            if self.rate_limited(&task) {
                limited.push(task);
                continue;
            }
            if self.alive(&task) {
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
            did = true;
            self.finish(&task);
        }
        self.usage_wait(
            !dispatched.is_empty() && limited.len() == dispatched.len(),
            &limited,
        );
        did
    }

    /// Every dispatched worker waiting on the usage window is said once per
    /// episode: `usage-wait` in the run dir marks one as begun, and it ends
    /// when any worker is back.
    fn usage_wait(&self, all: bool, tasks: &[String]) {
        let mark = self.dir.join("usage-wait");
        if !all {
            let _ = std::fs::remove_file(&mark);
            return;
        }
        if mark.exists() {
            return;
        }
        let _ = std::fs::write(&mark, format!("{}\n", sys::now()));
        let line = format!("waiting for the usage window -- {}", tasks.join(", "));
        warn(format!("run {}: {line}", self.plan.plan_id));
        self.event(&line);
        memcli::log_run(&format!("run {}: {line}", self.plan.plan_id));
    }

    /// Tasks an earlier orchestrator left dispatched when it died. Its lock
    /// went with its file descriptors, so this run owns them now: the ones
    /// still working are adopted as they stand, and the rest are collected
    /// exactly as the reap loop would have collected them.
    ///
    /// Without this a stale `dispatched` either holds the run open forever
    /// against a worker that is gone, or gets dispatched a second time into a
    /// worktree that still has the first one in it.
    ///
    /// Answers with the ids it took over. They are settled for this run --
    /// merged, failed, or running -- and the classification below must not
    /// queue them a second time. A task collected here that failed with its
    /// retry unspent is the exception: it goes back to `pending` and out of
    /// this list, for the ready set to dispatch on this very pass.
    fn adopt_stale(&self) -> Vec<String> {
        let mut taken = self.dispatched();
        // The ones collecting sent back to the ready set, dropped from the
        // answer below rather than while the loop is reading it.
        let mut again: Vec<String> = Vec::new();
        for task in &taken {
            // Not seen by the backend at all: whatever `alive` would say, it
            // is gone, the way a stall deadline never has to prove -- waiting
            // it out bought nothing; dispatch again now, while the retry lasts.
            // `alive` and `rate_limited` are only asked of a session the
            // backend can see, so a task that fails this check skips straight
            // to the fall-through below.
            if !self.backend.seen(&self.handle(task)) {
                let tries: u64 = self.field(task, "dispatches").parse().unwrap_or(0);
                if tries < 2 && self.commits(task) == 0 {
                    let no_result = std::fs::metadata(self.dir.join(format!("{task}.json")))
                        .map(|m| m.len() == 0)
                        .unwrap_or(true);
                    let why = if self.field(task, "status").is_empty() && no_result {
                        warn(format!(
                            "task {task}: the recorded session never existed -- dispatching again now"
                        ));
                        "the session it was given never existed, so it never ran"
                    } else {
                        warn(format!(
                            "task {task}: its session is gone and nothing was committed -- dispatching again now"
                        ));
                        "its session is gone and nothing was committed, so start again from what is in the tree"
                    };
                    self.dispatch(task, why);
                    continue;
                }
            } else if self.alive(task) || self.rate_limited(task) {
                warn(format!(
                    "task {task}: still working, from a run that is gone -- adopted"
                ));
                continue;
            }
            warn(format!(
                "task {task}: left dispatched by a run that is gone -- collecting it"
            ));
            self.finish(task);
            // Collected and failed with work on it, and nothing lists the
            // session: the machine went down under it, which is no verdict on
            // the work. It resumes on this pass whatever its attempt count; the
            // retry guard below ended a task on its third power cut and cost a
            // second `workflow run` to pick up a good branch. Listed, never
            // seen: a record can outlive the pane. A worker that stopped on its
            // own question is waiting on the answer, not on a machine: the poll
            // loop sends it back in with it.
            if self.state(task) == FAILED
                && self.asked(task).is_none()
                && !self.backend.listed(&self.handle(task))
                && (self.commits(task) > 0 || self.uncommitted(task))
            {
                warn(format!(
                    "task {task}: its session is gone with work on it -- the machine went down; resuming it"
                ));
                self.set_state(task, PENDING);
                again.push(task.clone());
                continue;
            }
            // Collected and failed with the retry still unspent: the attempt
            // died with its coordinator and nothing here has judged the work.
            // Failing it ends the whole run in the same second and costs a
            // `reap` and a second `workflow run` to get back to this point, so
            // it goes back to the ready set and is dispatched on this pass, on
            // whatever commits it has.
            let tries: u64 = self.field(task, "dispatches").parse().unwrap_or(0);
            if self.state(task) == FAILED && tries < 2 {
                warn(format!(
                    "task {task}: nobody read it and its retry is unspent -- pending again, for this run to dispatch"
                ));
                self.set_state(task, PENDING);
                again.push(task.clone());
            }
        }
        taken.retain(|t| !again.contains(t));
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
            if git.is_ancestor(&self.base, &prev) {
                // The trunk has not moved past what integration already holds --
                // an earlier run merged work here and stopped short, and there is
                // nothing to reconcile. Setup's fast-forward to base is then a
                // no-op, and the run continues on top of it.
                let ahead = git.count(&format!("{}..{}", self.base, self.int_branch));
                warn(format!(
                    "{} already carries {ahead} commit(s) an earlier run merged; continuing on them",
                    self.int_branch
                ));
            } else if self
                .plan
                .tasks
                .iter()
                .all(|t| t.checked || self.state(&t.id) == MERGED)
                && git.quiet(&["merge-tree", "--write-tree", &self.base, &self.int_branch])
            {
                // Every task is merged, the trunk moved on, and the two merge
                // clean: there is nothing left to decide, only to land. The
                // trunk is merged into integration at setup, which keeps
                // every commit the run recorded, and the trunk check proves
                // the result. Refusing sent `workflow accept`'s own "run the
                // plan again to land it" into a refusal.
                // With work still to do the refusal stands: which trunk it
                // builds on is the person's call.
                let ahead = git.count(&format!("{}..{}", self.base, self.int_branch));
                warn(format!(
                    "{} carries {ahead} commit(s) an earlier run merged and the trunk has moved on; merging the trunk into it",
                    self.int_branch
                ));
            } else {
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
            // nothing on it to look at.
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
            // What the branch holds, remembered in case it lands on the
            // trunk under no name this run knows.
            if let Some(tip) = git.rev_parse_commit(&branch) {
                write_field(&self.dir, &t.id, "leftover", &tip);
            }
            // A branch with commits on an unticked task is adopted as a
            // task that failed with commits, which `resumable` resumes on
            // that branch: the gate judges the work. The
            // merge-or-delete recipe left an orchestrator merging eighteen
            // files by hand, unread.
            let ahead = git.count(&format!("{}..{}", self.base, branch));
            if ahead > 0 {
                self.set_state(&t.id, FAILED);
                write_field(
                    &self.dir,
                    &t.id,
                    "failed",
                    &format!(
                        "left by an earlier run with {ahead} commit(s) -- adopted on its branch"
                    ),
                );
                warn(format!(
                    "{}: left by an earlier run with {ahead} commit(s) -- adopted on its branch",
                    t.id
                ));
                continue;
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
        // What this run dispatches on, so a later `reap` for a run that is
        // gone dispatches on the same model rather than whatever the
        // environment or the project key happen to say by then.
        let _ = std::fs::write(self.dir.join("model"), format!("{}\n", self.model));
        let _ = std::fs::write(
            self.dir.join("effort"),
            format!("{}\n", self.effort.as_deref().unwrap_or("")),
        );
        // Where a merge of this plan is ticked off, for a pass that rebuilds
        // the run off this dir rather than off the command line: `workflow
        // accept` with no run live ticks the file the plan came from, the
        // way the run itself would.
        let _ = std::fs::write(
            self.dir.join("plan-file"),
            format!(
                "{}\n",
                self.plan_file
                    .as_deref()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default()
            ),
        );

        if !self.int_worktree() {
            return false;
        }

        for t in self.plan.tasks.clone() {
            if t.checked {
                continue; // already done, nothing to run
            }
            if !self.make_worktree(&t.id) {
                return false;
            }
        }
        true
    }

    /// The integration branch and the worktree the gate merges in, made if
    /// they are not there and brought up to the run's base. Setup's, and
    /// `workflow accept`'s when it merges for a run that has ended.
    fn int_worktree(&mut self) -> bool {
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
        // A directory that is there but no worktree -- an unclean shutdown
        // zero-truncates the `.git` file and git forgets the entry -- used
        // to fail below as "cannot fast-forward", pointing at a history that
        // was fine.
        if !worktree_intact(&git, &self.int_wt) {
            warn(format!(
                "integration worktree at {} is missing or corrupt; remove that directory, run `git worktree prune`, and run again -- the branch {} is intact",
                self.int_wt.display(),
                self.int_branch
            ));
            return false;
        }
        let int = Git::at(&self.int_wt);
        let head = int.head().unwrap_or_default();
        if head != self.base
            && !int.is_ancestor(&self.base, &head)
            && !int.is_ancestor(&head, &self.base)
        {
            // Diverged, and preflight found the two merge clean.
            if !int.quiet(&[
                "-c",
                "core.hooksPath=/dev/null",
                "merge",
                "-q",
                "--no-edit",
                "-m",
                &format!("Merge the trunk into {}", self.int_branch),
                &self.base,
            ]) {
                int.quiet(&["merge", "--abort"]);
                warn(format!(
                    "{} cannot take the trunk at {} in; sort it out by hand",
                    self.int_branch, self.base
                ));
                return false;
            }
        } else if head != self.base && !int.quiet(&["merge", "-q", "--ff-only", &self.base]) {
            warn(format!(
                "{} cannot fast-forward to {}; sort it out by hand",
                self.int_branch, self.base
            ));
            return false;
        }
        // Furnished the way a task worktree is: the gate runs the whole suite
        // here, and a tree with no node_modules is red before anything is
        // dispatched.
        self.link_deps(&self.int_wt);
        true
    }

    /// The worktree one task works in, made if it is not there: a fresh
    /// branch off the base, or the failed attempt's branch checked out
    /// again when the task is resumable. Setup makes every task's; a task
    /// the plan gains mid-run gets its own here.
    fn make_worktree(&mut self, task: &str) -> bool {
        let git = self.git();
        let wt = self.worktree(task);
        if wt.is_dir() {
            return true;
        }
        // A worktree hand-removed since the last run leaves its
        // registration behind; without pruning first, git refuses to
        // reuse the branch or the path a resumed task needs back.
        git.quiet(&["worktree", "prune"]);
        let branch = self.branch(task);
        if self.resumable(task) {
            // The branch already exists with the failed attempt's
            // commits on it -- checked out again, never recreated.
            if !git.quiet(&["worktree", "add", "-q", &wt.to_string_lossy(), &branch]) {
                warn(format!("cannot resume the worktree for task {task}"));
                return false;
            }
            self.link_deps(&wt);
            return true; // not self.made: rollback and cleanup leave it be
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
            warn(format!("cannot make a worktree for task {task}"));
            return false;
        }
        self.made.push(wt.clone());
        self.link_deps(&wt);
        true
    }

    /// Furnish a fresh worktree with its dependencies. `vendor` is a symlink
    /// to the checkout's, made here and by nothing else: git itself carries
    /// nothing across worktrees. `node_modules` is never shared. Under pnpm
    /// 11 every `pnpm run` first checks the tree's dependencies and refuses a
    /// `node_modules` that resolves outside the project root, so a symlinked
    /// one fails every script in the tree before it starts, and no setting
    /// short of a flag on the command line turns that check off. An install per
    /// worktree costs about a second: pnpm's store is content-addressable, so a
    /// second install of the same lockfile is symlinks and no download.
    fn link_deps(&self, wt: &Path) {
        repo::submodules(&self.repo, wt);
        let from = self.repo.join("vendor");
        let to = wt.join("vendor");
        if from.is_dir() && !to.exists() {
            let _ = std::os::unix::fs::symlink(&from, &to);
        }
        self.install_deps(wt);
    }

    /// A worktree's dependencies are the lockfile's as it stood when the
    /// worktree was made, and a dependency a sibling merged since is not in
    /// them until someone installs again. Judged at dispatch, after the
    /// catch-up: `node_modules` is brought up to the lockfile in place. The
    /// shared `vendor` is the checkout's, and a task that changes the lockfile
    /// cannot install into it without rewriting what its siblings read, so the
    /// task whose Files claim the manifest or the lockfile gets a directory of
    /// its own, and so does one whose lockfile differs from the checkout's.
    fn own_deps(&self, task: &str) {
        let wt = self.worktree(task);
        self.node_deps(&wt);
        let link = wt.join("vendor");
        if !link.is_symlink() {
            return;
        }
        let claims = self
            .task_now(task)
            .and_then(|t| t.files)
            .map(|f| ownership::split_patterns(&f))
            .unwrap_or_default();
        let claimed = claims.iter().any(|p| {
            plancheck::covers(p, "composer.lock") || plancheck::covers(p, "composer.json")
        });
        let changed = std::fs::read(wt.join("composer.lock")).ok()
            != std::fs::read(self.repo.join("composer.lock")).ok();
        if claimed || changed {
            let _ = std::fs::remove_file(&link);
            self.install_deps(&wt);
        }
    }

    /// Install into a worktree that has no dependencies of its own yet.
    fn install_deps(&self, wt: &Path) {
        self.node_deps(wt);
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

    /// `node_modules` brought up to the tree's lockfile: installed when it is
    /// missing, and installed again when pnpm's own record of what it
    /// installed from, `node_modules/.pnpm/lock.yaml`, no longer matches.
    /// That record is exact where comparing against the checkout's lockfile
    /// was a proxy: a worktree that caught up to a merged lockfile, an
    /// integration worktree the last run left behind, and a checkout that
    /// moved on all read the same way. A symlink standing there is an
    /// earlier link_deps's and goes first, since pnpm refuses to install
    /// through it. Nothing to do without a `pnpm-lock.yaml`.
    fn node_deps(&self, wt: &Path) {
        let lock = wt.join("pnpm-lock.yaml");
        if !lock.is_file() {
            return;
        }
        let nm = wt.join("node_modules");
        if nm.is_symlink() {
            let _ = std::fs::remove_file(&nm);
        }
        if nm.is_dir()
            && std::fs::read(&lock).ok() == std::fs::read(nm.join(".pnpm/lock.yaml")).ok()
        {
            return;
        }
        self.pnpm_install(wt);
    }

    fn pnpm_install(&self, wt: &Path) {
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

    /// Every builder's dir under `cargo_root`, torn down -- except a resumable
    /// task's, which the next run's worker needs to find undisturbed. Walks
    /// the directory's actual entries rather than the plan's task ids: the
    /// gate's own "integration" builder is not a task and was left behind by
    /// a version of this that iterated ids instead.
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
        // Every session this run started is stopped by the time it gets here,
        // and a stopped agent is still a row on the backend's wall until
        // somebody says otherwise.
        self.backend.clear(&self.wt_root);
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
            // A failed attempt that wrote and never committed has left its
            // work here and nowhere else.
            if self.state(&t) == FAILED && self.uncommitted(&t) {
                warn(format!(
                    "task {t}: its worktree holds work no commit has -- kept at {}",
                    wt.display()
                ));
                continue;
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
        // is how phantom failures happen. A resumable task's is the exception,
        // same as its worktree above.
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

/// The reason to refuse a run before it has written anything, as the lines
/// to say; `None` means go.
fn refused(plan: &Plan) -> Option<String> {
    if plan.kind == PlanKind::Roadmap {
        return Some(format!(
            "run: '{}' is a roadmap, and its items are milestones rather than work a worker can take.\n\
             make one of them mem's live plan with `mem plan --from <slug>`, then run again.",
            plan.plan_id
        ));
    }
    None
}

/// The stopped-short report, written to be read at a glance: counts first, then
/// one line per outcome group, and never an empty note. Held to the same lint
/// commits are held to. `tasks` is (id, state, failure reason) in plan order. A
/// log line and stderr, not a question: the orchestrator reads it and decides,
/// and a person is asked only what the orchestrator cannot settle.
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
/// merge-ready.
/// The status file's text after its last attempt marker (`--- attempt N
/// ---`, `--- continued N ---`): what this attempt reported, and nothing an
/// earlier one did.
pub fn status_after_marker(text: &str) -> &str {
    let mut at = 0;
    let mut pos = 0;
    for line in text.split_inclusive('\n') {
        if line.starts_with("--- ") {
            at = pos + line.len();
        }
        pos += line.len();
    }
    &text[at..]
}

/// One report line as (state, note). The grammar is `<utc> <state> <note>`;
/// a line that leads with a state and follows with the time is read as what
/// it meant (ebdify's auth worker wrote every line that way and the run
/// never once saw its `ready`). A marker line and a line with fewer than two
/// fields are nothing.
pub fn status_line(line: &str) -> Option<(String, String)> {
    if line.starts_with("--- ") {
        return None;
    }
    let mut fields = line.split_whitespace();
    let (Some(first), Some(second)) = (fields.next(), fields.next()) else {
        return None;
    };
    let (lead, lead_head) = split_state(first);
    let (state, head) = match brief::STATES.contains(&lead.as_str()) && looks_like_time(second) {
        true => (lead, lead_head),
        false => split_state(second),
    };
    let rest = fields.collect::<Vec<_>>().join(" ");
    let note = match (head.is_empty(), rest.is_empty()) {
        (true, _) => rest,
        (false, true) => head,
        (false, false) => format!("{head} {rest}"),
    };
    Some((state, note))
}

/// The last report in a status file's text, read after its last marker.
pub fn last_status_in(text: &str) -> Option<(String, String)> {
    status_after_marker(text)
        .lines()
        .filter_map(status_line)
        .next_back()
}

/// `2026-09-20T13:16Z`, `2026-09-20T13:16:05Z`, or anything else a worker
/// wrote for the time: it starts with four digits and a dash.
fn looks_like_time(field: &str) -> bool {
    let b = field.as_bytes();
    b.len() > 5 && b[..4].iter().all(u8::is_ascii_digit) && b[4] == b'-'
}

pub fn split_state(token: &str) -> (String, String) {
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
fn timings() -> (usize, i64, i64, f64, u32) {
    let mut max_workers = std::env::var("WORKFLOW_MAX_WORKERS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(2);
    max_workers = max_workers.clamp(1, 5);

    // Fractional minutes on purpose: AC7 injects a deadline in seconds.
    let deadline = ((env_f64("WORKFLOW_DEADLINE_MIN", 30.0) * 60.0) + 0.5) as i64;
    let deadline = deadline.max(1);
    let grace = (deadline / 2).clamp(1, 30);
    let poll = ((deadline as f64 / 10.0) * 100.0).round() / 100.0;
    let poll = poll.clamp(0.2, 5.0);
    // The reindex bound is polls, not seconds, so a suite that wants to see
    // the ghost case give up sooner -- or a loaded machine one that wants
    // mem given longer -- says so here.
    let question_misses = std::env::var("WORKFLOW_QUESTION_MISSES")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(QUESTION_MISS_LIMIT);
    (max_workers as usize, deadline, grace, poll, question_misses)
}

/// Which rung of the ladder a dial's value came off, so the run can say it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Dialed {
    Env,
    Record,
    Project,
    Default,
}

impl Dialed {
    fn as_str(self) -> &'static str {
        match self {
            Dialed::Env => "the environment",
            Dialed::Record => "the run's record",
            Dialed::Project => "the project",
            Dialed::Default => "the default",
        }
    }
}

/// A dial a run may or may not carry -- the workers' effort -- resolved
/// the way `new_run` describes: the variable set, even empty, is the answer
/// for this run; else what a `setup` of this plan recorded, an
/// empty record meaning none; else the project key, when the dial has one.
/// The rung rides back with
/// the value: a run that keeps a record over a project key changed since is
/// doing what it was told to, and it has to say so.
fn optional_dial(
    var: &str,
    recorded: Option<String>,
    project: impl FnOnce() -> Option<String>,
) -> (Option<String>, Dialed) {
    match std::env::var(var) {
        Ok(v) => (
            Some(v.trim().to_string())
                .filter(|v| !v.is_empty())
                .filter(|v| !v.eq_ignore_ascii_case("none")),
            Dialed::Env,
        ),
        Err(_) => match recorded {
            Some(v) => (
                Some(v)
                    .filter(|v| !v.is_empty())
                    .filter(|v| !v.eq_ignore_ascii_case("none")),
                Dialed::Record,
            ),
            None => match project() {
                Some(v) => (Some(v), Dialed::Project),
                None => (None, Dialed::Default),
            },
        },
    }
}

/// The run's dials as one line, each naming where it came from. Printed at
/// the start, because the precedence -- environment, then this plan's own
/// record, then the project key -- is deliberate (a plan picked up again keeps
/// the dials it began with) and a run that never said which it kept left an
/// orchestrator reading the run directory to find out why `mem project set
/// model` had no effect.
fn dial_line(model: (&str, Dialed), effort: (Option<&str>, Dialed)) -> String {
    let at = match effort {
        (Some(level), from) => format!(" at effort {level} ({})", from.as_str()),
        (None, _) => String::new(),
    };
    format!("writing with {} ({}){at}", model.0, model.1.as_str())
}

/// A `setup` of this same plan may already have written `model` and `effort`
/// into the run directory: `reap` rebuilding a run nobody is watching, or
/// `run` picking one up after a stop. Either dispatches on what the run was
/// told when it began, rather than whatever the project key says by then.
/// The environment has the last word and a project key the least:
/// `WORKFLOW_MODEL`/`WORKFLOW_EFFORT`, then what was recorded, then `mem
/// project set model`/`effort`, then `opus`/the CLI's own default. Beside the
/// run comes its start line, naming the rung each dial came off.
fn new_run(plan: Plan, repo: PathBuf, project: &str, base: String) -> (Run, String) {
    let (max_workers, deadline_s, kill_grace_s, poll, question_misses) = timings();
    let gate_s = (((env_f64("WORKFLOW_GATE_MIN", 60.0) * 60.0) + 0.5) as i64).max(1);
    let wt_root = paths::worktrees_root().join(project).join(&plan.plan_id);
    let dir = paths::runs_root().join(project).join(&plan.plan_id);
    let recorded_model = recorded(&dir, "model");
    let recorded_effort = recorded(&dir, "effort");
    let (model, model_from) = match std::env::var("WORKFLOW_MODEL") {
        Ok(v) if !v.is_empty() => (v, Dialed::Env),
        _ => match recorded_model.filter(|v| !v.is_empty()) {
            Some(v) => (v, Dialed::Record),
            None => match memcli::project_model() {
                Some(v) => (v, Dialed::Project),
                None => ("opus".to_string(), Dialed::Default),
            },
        },
    };
    let (effort, effort_from) =
        optional_dial("WORKFLOW_EFFORT", recorded_effort, memcli::project_effort);
    let dials = dial_line((&model, model_from), (effort.as_deref(), effort_from));
    let run = Run {
        dir,
        brief_dir: paths::briefs_root().join(project).join(&plan.plan_id),
        project: project.to_string(),
        int_branch: format!("integration/{}", plan.plan_id),
        int_wt: wt_root.join("_integration"),
        exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("workflow")),
        wt_root,
        plan,
        plan_file: None,
        repo,
        base,
        deadline_s,
        gate_s,
        kill_grace_s,
        poll,
        question_misses,
        max_workers,
        backend: backend::backend_for(),
        model,
        effort,
        stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        // A worker stands at its worktree's root, which mem resolves to a
        // monorepo's root project; its task belongs to this one.
        env: vec![("MEM_PROJECT".into(), project.to_string())],
        collecting: false,
        unparsable: String::new(),
        made: Vec::new(),
    };
    (run, dials)
}

pub fn cmd_run(plan_file: Option<&Path>, model: Option<&str>, effort: Option<&str>) -> i32 {
    if !Git::here().inside_worktree() {
        warn("run: stand in the project checkout");
        return exit::USAGE;
    }
    memcli::resolve_from_here();
    let Some((git, top)) = repo::goto_toplevel() else {
        return exit::USAGE;
    };
    if let Some(f) = plan_file
        && paths::realpath_m(f).starts_with(paths::realpath_m(&top))
    {
        warn(format!(
            "run: {} is inside the checkout, so its ticks would land in the tree -- store it with `mem plan <slug> --set-file` and run from mem",
            f.display()
        ));
        return exit::USAGE;
    }
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
    // What plan-check would say, said here too: the eight-pattern warning
    // that predicted a task's two blown windows sat in plan-check's output
    // while the run dispatched it without a word.
    let checked = plancheck::findings(&parsed, &[], &top, plan_file);
    for line in checked.warnings.iter().chain(checked.refusals.iter()) {
        warn(line);
    }
    if !checked.refusals.is_empty() {
        warn("run: plan-check refuses this plan -- fix it before running");
        return exit::USAGE;
    }

    // The two dials, rewritten in the run directory before anything reads
    // them. A record a run wrote when it began is preferred to the project
    // key on purpose, so a plan picked up again keeps the model
    // it started on however the project has changed since -- and editing the
    // run directory by hand was the only way to change that. The value stands
    // for every later run of this plan too.
    let run_dir = paths::runs_root()
        .join(project.dir_name())
        .join(&parsed.plan_id);
    for (name, value) in [("model", model), ("effort", effort)] {
        let Some(value) = value else {
            continue;
        };
        let _ = std::fs::create_dir_all(&run_dir);
        let _ = std::fs::write(run_dir.join(name), format!("{value}\n"));
        warn(format!(
            "run {}: {name} is now {value} in this run's record",
            parsed.plan_id
        ));
    }

    let Some(base) = git.head() else {
        return exit::USAGE;
    };
    let (mut run, dials) = new_run(parsed, top, &project.dir_name(), base);
    // Resolved, not as typed: the ticks go back to this file for the rest of
    // the run, and a relative path is read against whatever the cwd is then.
    run.plan_file = plan_file.map(paths::realpath_m);

    // Before the lock, the worktrees and the first dispatch: nothing here has
    // written anything yet, so a refusal costs a message and no cleanup.
    if let Some(why) = refused(&run.plan) {
        for line in why.lines() {
            warn(line);
        }
        return exit::USAGE;
    }
    // Every model the run may dispatch on, asked of the backend before
    // anything is made: a name amx will not start used to pass the start and
    // the base gate and then fail each launch, and the failed tasks outlived
    // the fix to the key.
    if let Some(why) = run.unstartable() {
        for line in why.lines() {
            warn(line);
        }
        return exit::USAGE;
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
    // into the same worktrees.
    let _ = std::fs::create_dir_all(&run.dir);
    let Some(_lock) = lock_run(&run.dir) else {
        warn(format!(
            "run {}: another orchestrator is live in this run -- not starting a second",
            run.plan.plan_id
        ));
        return exit::USAGE;
    };
    // The run dir lock covers this checkout; the claim covers the project's
    // plan and roadmap in mem, which every machine's checkout shares.
    let here = memcli::machine();
    if let Some((machine, since)) = here.as_deref().and_then(claimed_elsewhere) {
        warn(format!(
            "{} is run by {machine} since {since}",
            project.name
        ));
        return exit::USAGE;
    }
    // A run dir is keyed by plan, so this run appends its events to the last
    // run's file. The cursor `workflow wait` keeps is stamped to the end of
    // what is already there, or the first wait of a live run returns at once
    // on the previous run's ending.
    let _ = std::fs::write(
        run.dir.join("wait.cursor"),
        format!(
            "{}\n",
            std::fs::metadata(run.dir.join("events"))
                .map(|m| m.len())
                .unwrap_or(0)
        ),
    );

    if !run.setup() {
        run.rollback();
        return exit::USAGE;
    }
    if let Err(why) = run.trunk_green() {
        warn(format!(
            "run {}: the trunk is red before anything is dispatched -- {why}",
            run.plan.plan_id
        ));
        warn("fix the trunk first: a run on a red trunk fails every task at its gate");
        memcli::log_run(&format!(
            "run {}: refused, the trunk is red -- {why}",
            run.plan.plan_id
        ));
        run.rollback();
        return exit::USAGE;
    }
    // Taken once nothing above can refuse, so a refusal leaves no claim.
    // The claim holds only the machine, so the plan it runs goes in the log
    // for the hub and a sibling machine to read.
    if let Some(here) = &here {
        memcli::claim_runner(here);
        memcli::log_run(&format!("run {}: claimed {here}", run.plan.plan_id));
    }
    let _ = std::fs::write(run.dir.join("plan.md"), &source);
    for t in run.plan.ids() {
        if !run.dir.join(format!("{t}.state")).exists() {
            run.set_state(&t, PENDING);
        }
        // A launch the backend refused started nothing: no worker ran and no
        // attempt was spent, so the task is as untouched as a pending one. Left
        // failed, it blocked every task after it once the dial was mended,
        // and the restart needed a redispatch by hand.
        if run.state(&t) == FAILED
            && run
                .field(&t, "failed")
                .starts_with("the launch was refused")
            && run.commits(&t) == 0
        {
            warn(format!(
                "task {t}: its launch was refused last time and nothing ran -- pending again"
            ));
            let tries: u64 = run.field(&t, "dispatches").parse().unwrap_or(0);
            write_field(
                &run.dir,
                &t,
                "dispatches",
                &tries.saturating_sub(1).to_string(),
            );
            run.set_state(&t, PENDING);
        }
        // A redispatch marker nobody consumed was a request to a run that is
        // gone; carrying it into this run would dispatch a task nobody asked
        // this run about.
        let _ = std::fs::remove_file(run.dir.join(format!("{t}.redispatch")));
    }
    // The way down: a killed coordinator used to leave its workers running for
    // nobody. The signal only raises a flag; the poll loop sees it, stops every
    // dispatched worker, and leaves the tasks `dispatched` for the next run in
    // this checkout to adopt and collect. SIGKILL still can't be caught -- reap
    // covers that aftermath.
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        let _ = signal_hook::flag::register(sig, run.stop.clone());
    }
    // A hangup means the shell that started the run is gone, and nothing
    // about it is a decision to end the work: a run backgrounded inside a
    // pane died with that pane and took a worker down mid-design with it.
    // It ends this process and leaves every session standing, so the next run
    // in the checkout adopts them. The flag is the same one so the loop comes
    // out; which signal it was decides what happens on the way.
    let hangup = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    for flag in [run.stop.clone(), hangup.clone()] {
        let _ = signal_hook::flag::register(signal_hook::consts::SIGHUP, flag);
    }
    let stop_flag = run.stop.clone();
    let stopping = move || stop_flag.load(std::sync::atomic::Ordering::Relaxed);
    let hangup_flag = hangup.clone();
    let hung_up = move || hangup_flag.load(std::sync::atomic::Ordering::Relaxed);

    let adopted = run.adopt_stale();
    memcli::log_run(&format!(
        "run {}: started at {} with {} tasks",
        run.plan.plan_id,
        run.base,
        run.plan.tasks.len()
    ));
    warn(format!(
        "run {}: {} tasks, up to {} at a time",
        run.plan.plan_id,
        run.plan.tasks.len(),
        run.max_workers
    ));
    warn(format!("run {}: {dials}", run.plan.plan_id));

    // Classified once, in plan order, before the loop dispatches anything.
    for id in run.plan.ids() {
        let Some(task) = run.plan.get(&id).cloned() else {
            continue;
        };
        // A tick in the plan is a claim about the past, not about this
        // integration branch. It only counts as merged here when the commit
        // some run recorded for it is actually on the branch.
        if task.checked {
            if run.landed(&id) {
                run.set_state(&id, MERGED);
            } else {
                run.set_state(&id, DONE_PREVIOUSLY);
                if run.prior_sha(&id).is_empty() {
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
        if adopted.contains(&id) {
            continue;
        }
        // Unticked, but the commit some pass merged for it is on the
        // integration branch as it stands: an earlier pass merged it and
        // the tick never took, or the work was landed by hand between
        // passes -- the binary's own printed recipe.
        // The ref recorded at merge time outlives every branch, so this
        // is checkable, and a worker was rebuilding landed work when only
        // the tick was consulted.
        if run.landed(&id) {
            run.set_state(&id, MERGED);
            warn(format!(
                "task {id}: its work is already on {} -- not dispatched again",
                run.int_branch
            ));
            // Ticked here as at any other merge, or the plan goes on
            // asking for work that is done and every later run pays the
            // same discovery again.
            run.tick_off(&id);
            continue;
        }
        // A task the orchestrator asked to accept stays failed and waiting
        // for that: reset to pending it was dispatched to a fresh worker
        // before the loop below had read the marker, which is a whole
        // attempt spent on work already settled.
        if run.state(&id) == FAILED
            && ["accept", "regate"]
                .iter()
                .any(|ext| run.dir.join(format!("{id}.{ext}")).exists())
        {
            continue;
        }
        run.set_state(&id, PENDING);
    }

    // The ready set, in plan order: PENDING tasks whose dependencies have
    // merged. Recomputed every pass, so a task made ready by a merge two
    // passes ago is dispatched exactly as readily as one ready from the start.
    let ready = |run: &Run| -> Vec<String> {
        run.plan
            .ids()
            .into_iter()
            .filter(|id| run.state(id) == PENDING)
            .filter(|id| run.plan.get(id).is_some_and(|t| run.deps_satisfied(t)))
            .collect()
    };
    // A failed task somebody has asked something of -- to go again, or to be
    // accepted as it stands. The marker of a run that ended before it could
    // be honoured is this run's to answer, so the loop stays open for it.
    let marked = |run: &Run, ext: &str| -> bool {
        run.plan
            .ids()
            .into_iter()
            .any(|id| run.state(&id) == FAILED && run.dir.join(format!("{id}.{ext}")).exists())
    };

    let mut warned_waiting: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut all_ids = run.plan.ids();
    // Taken once a pass and used twice -- to keep the loop open and to name
    // what it waits on -- because asking twice spends two of the misses
    // `question_open` counts on one poll.
    let mut waiting = run.waiting(&all_ids);
    while run.running() > 0
        || !waiting.is_empty()
        || !ready(&run).is_empty()
        || marked(&run, "redispatch")
        || marked(&run, "accept")
        || marked(&run, "regate")
    {
        if stopping() {
            return shutdown(&run, hung_up());
        }
        if !run.take_new_tasks().is_empty() {
            all_ids = run.plan.ids();
        }
        run.refresh();
        // `workflow accept <task>`: a task the run settled as failed landed
        // as it stands. Read before anything is dispatched, or
        // a task whose marker was written while this run was starting is
        // given a fresh worker first.
        for (id, ext) in all_ids
            .iter()
            .flat_map(|id| [(id, "accept"), (id, "regate")])
        {
            let marker = run.dir.join(format!("{id}.{ext}"));
            if !marker.exists() {
                continue;
            }
            // What accept needs is a settled failure with a diff on its
            // branch.
            if !run.resumable(id) {
                let _ = std::fs::remove_file(&marker);
                warn(format!(
                    "task {id}: asked to {ext}, but it is {} with {} commit(s) on its branch -- ignored",
                    run.state(id),
                    run.commits(id)
                ));
                continue;
            }
            let _ = std::fs::remove_file(&marker);
            match ext {
                "regate" => run.regate(id),
                _ => run.accept(id),
            }
        }
        for id in ready(&run) {
            if run.running() >= run.max_workers {
                run.hold(
                    &id,
                    &format!(
                        "waiting for a worker slot ({} of {} running)",
                        run.running(),
                        run.max_workers
                    ),
                );
                continue;
            }
            run.dispatch(&id, "");
        }
        for id in &all_ids {
            if run.state(id) != PENDING {
                continue;
            }
            if let Some(t) = run.plan.get(id)
                && !run.deps_satisfied(t)
            {
                let on: Vec<&str> = t
                    .deps
                    .iter()
                    .filter(|d| run.state(d) != MERGED)
                    .map(String::as_str)
                    .collect();
                run.hold(id, &format!("waiting on {} to merge", on.join(", ")));
            }
        }
        sys::sleep(run.poll);
        if stopping() {
            return shutdown(&run, hung_up());
        }
        run.reap_pass();
        // A failed task someone asked to try again, mid-run. The marker file
        // is how the request reaches a run that holds the project lock for
        // its whole life; nothing here closes behind a failed task, so the
        // marker is honoured for as long as the run lives.
        for id in &all_ids {
            let marker = run.dir.join(format!("{id}.redispatch"));
            if !marker.exists() {
                continue;
            }
            // A dispatched task asked to go again: the plan changed under
            // the worker, and finishing against the stale brief costs a
            // whole attempt. Its session is stopped and the task failed with
            // its commits kept, so the dispatch below resumes on them the way a
            // failed task's does.
            if run.state(id) == DISPATCHED {
                run.stop(id);
                run.fail_task(id, "replaced by request; its commits stay on the branch");
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
        // A task waiting on a question keeps the run open rather than
        // failing it out from under it, so said once, not on every poll
        // while the answer is still pending. Said only of a question mem has
        // really listed: an id the listing never carries ends the run
        // instead, and a line promising to stay open for it is a promise the
        // run does not keep.
        waiting = run.waiting(&all_ids);
        for id in &waiting {
            let Some(qid) = run.asked(id) else {
                continue;
            };
            if !run.question_listed(id, &qid) {
                continue;
            }
            if warned_waiting.insert(format!("{id} {qid}")) {
                warn(format!(
                    "{id}: waiting on #{qid} -- the run stays open until it is answered"
                ));
            }
        }
        // A task waiting on the orchestrator goes again by itself once the
        // answer is in: the orchestrator's whole job here is to answer, and a
        // run that had to be told twice stopped short over questions it could
        // have carried (the queue was one stopped-short question per answer,
        // all of them stale).
        for id in &all_ids {
            if run.state(id) != FAILED {
                continue;
            }
            let Some(qid) = run.asked(id) else {
                continue;
            };
            // A mem that could not answer is not an answer that has not
            // landed yet: either way there is nothing to send in, and the
            // next poll asks again.
            let answered = memcli::questions_for(&run.task_tag(id))
                .unwrap_or_default()
                .into_iter()
                .any(|q| q.short_id == qid && q.answer.is_some());
            if answered {
                // Its own session takes the answer whatever the cap says:
                // the pane is standing already, and holding a sent-back
                // answer for a free slot left a worker idle on its question
                // for as long as its siblings ran. Only a fresh session waits
                // for one.
                if run.continue_worker(id, "") {
                    continue;
                }
                if run.running() >= run.max_workers {
                    run.hold(
                        id,
                        &format!(
                            "#{qid} is answered; a fresh session starts when a worker slot frees ({} of {} running)",
                            run.running(),
                            run.max_workers
                        ),
                    );
                    continue;
                }
                warn(format!(
                    "task {id}: #{qid} was answered -- dispatched again with the answer"
                ));
                run.dispatch(id, "");
            }
        }
    }

    // Never made ready: what it waited for never landed.
    for id in &all_ids {
        if run.state(id) == PENDING {
            warn(format!(
                "task {id}: skipped, what it waits for did not land"
            ));
            run.set_state(id, BLOCKED);
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

    let ended = format!("ended {merged} merged, {failed} failed");
    warn(format!("run {}: {}", run.plan.plan_id, &ended[6..]));
    run.event(&ended);
    memcli::log_run(&format!("run {}: {ended}", run.plan.plan_id));
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
    // A run that merged everything lands its own branch: left on integration
    // it sat there for hours with nobody sure whose move it was.
    if failed + blocked == 0 && merged > 0 {
        let landed = match run.land_trunk() {
            Ok(place) => format!("landed {} on {place}; nothing was pushed", run.int_branch),
            Err(why) if why.contains("by hand") => {
                format!(
                    "left {} unlanded: {why}; nothing was pushed",
                    run.int_branch
                )
            }
            Err(why) => format!(
                "left {} unlanded: {why} -- `git merge --ff-only {}` when the checkout is ready; nothing was pushed",
                run.int_branch, run.int_branch
            ),
        };
        warn(format!("run {}: {landed}", run.plan.plan_id));
        run.event(&landed);
        memcli::log_run(&format!("run {}: {landed}", run.plan.plan_id));
    } else {
        warn(format!(
            "integration branch {} is yours to look at; nothing was pushed",
            run.int_branch
        ));
    }
    // A worker's question on a task that merged anyway is moot, and left
    // pending it sits in the orchestrator's queue for ever.
    for t in run.plan.ids() {
        let state = run.state(&t);
        if state != MERGED && state != DONE_PREVIOUSLY {
            continue;
        }
        for q in memcli::questions_for(&run.task_tag(&t)).unwrap_or_default() {
            if q.answer.is_none() {
                memcli::answer(&q.id, &format!("moot: {t} merged without it"));
            }
        }
    }
    run.tick_settled_again();
    release_own_claim();
    // A milestone is finished when its plan is. mem's live plan is the one
    // the roadmap's milestone names, so a run that read it from mem, or from a
    // file carrying that plan's own slug, is the one that can say
    // which box to tick: any other --plan-file plan need not be in mem at all.
    if failed + blocked == 0 && (run.plan_file.is_none() || run.mem_holds_this_plan()) {
        let slug = &run.plan.plan_id;
        match memcli::roadmap_tick(slug) {
            Ok(true) => warn(format!("milestone {slug} is ticked off in the roadmap")),
            Ok(false) => {}
            Err(said) => {
                let fix = format!("mem --project {} roadmap --tick {slug}", run.project);
                let line = format!("roadmap tick for {slug} failed: {said} -- fix: {fix}");
                warn(&line);
                run.event(&line);
                memcli::log_run(&format!(
                    "run {slug}: roadmap tick failed: {said}; fix: {fix}"
                ));
            }
        }
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
        memcli::log_run(&report);
        return exit::FAILED;
    }
    exit::OK
}

/// Another machine's claim on this project that still holds: under an hour
/// old, or beside a run logged under an hour ago whatever its age. Anything
/// older is a run that died without clearing its claim, and is taken over.
fn claimed_elsewhere(here: &str) -> Option<(String, String)> {
    let (machine, since) = memcli::runner()?;
    if machine == here {
        return None;
    }
    (claim_fresh(&since, jiff::Timestamp::now()) || memcli::run_logged_lately())
        .then_some((machine, since))
}

fn claim_fresh(since: &str, now: jiff::Timestamp) -> bool {
    since
        .parse::<jiff::Timestamp>()
        .is_ok_and(|t| now.duration_since(t) < jiff::SignedDuration::from_hours(1))
}

/// Clear the runner claim if it is still this machine's: one taken over
/// after this run went quiet belongs to whoever took it.
fn release_own_claim() {
    if let (Some(here), Some((machine, _))) = (memcli::machine(), memcli::runner())
        && machine == here
    {
        memcli::release_runner();
    }
}

/// Told to stop: end every dispatched worker, say so, and leave the tasks
/// `dispatched` -- the next run adopts them and judges whatever they wrote.
/// No merging on the way out: a signal means now, and the merge gate is not
/// a thing to run while shutting down.
///
/// A hangup is the other thing. The shell that started the run has gone --
/// a pane closed, a session ended -- and it has no opinion about the work:
/// every worker is a session of its own that outlives it. Stopping them
/// took a worker down mid-design because the pane the run was launched from
/// went, so a hangup leaves them standing, says how to pick them up, and exits
/// 0 with no `ended` event: this process is over, the run is not.
fn shutdown(run: &Run, hangup: bool) -> i32 {
    let live = run.dispatched();
    if hangup {
        warn(format!(
            "run {}: the shell that started this run is gone; {} worker(s) were left running -- run again in this checkout to adopt them",
            run.plan.plan_id,
            live.len()
        ));
        memcli::log_run(&format!(
            "run {}: the shell that started it is gone; {} worker(s) were left running",
            run.plan.plan_id,
            live.len()
        ));
        // The workers left standing are this run's still, and the next run
        // in this checkout adopts them under the same claim.
        if live.is_empty() {
            release_own_claim();
        }
        return exit::OK;
    }
    release_own_claim();
    warn(format!(
        "run {}: told to stop -- stopping {} worker(s) before going",
        run.plan.plan_id,
        live.len()
    ));
    for task in &live {
        run.stop(task);
        warn(format!("task {task}: its worker was stopped"));
    }
    warn("run again in this checkout to adopt and collect what they left");
    memcli::log_run(&format!(
        "run {}: stopped by signal with {} worker(s) ended",
        run.plan.plan_id,
        live.len()
    ));
    run.event(&format!(
        "ended stopped by signal with {} worker(s) left dispatched",
        live.len()
    ));
    exit::FAILED
}

/// `workflow redispatch <task>` -- the marker the live run's poll loop reads.
/// Only a run whose lock is held right now can honour it; anything else is a
/// stopped run, and a stopped run's failed work comes back by running the
/// plan again.
pub fn cmd_redispatch(task: &str, model: Option<&str>) -> i32 {
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
        let state = field(&dir, task, "state");
        let plan_id = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if state != FAILED && state != DISPATCHED {
            continue;
        }
        // A model named here rides with the task for the rest of the run --
        // this task's, nobody else's.
        if let Some(m) = model {
            let _ = std::fs::write(dir.join(format!("{task}.model")), format!("{m}\n"));
        }
        let _ = std::fs::write(dir.join(format!("{task}.redispatch")), "");
        let how = match state.as_str() {
            DISPATCHED => {
                "its session is replaced on the next poll; the commits on its branch stay"
            }
            _ => "it goes on the next poll with a free worker slot",
        };
        warn(format!(
            "run {plan_id}: asked to dispatch {task} again -- {how}"
        ));
        return exit::OK;
    }

    warn(format!(
        "no live run holds {task} failed or dispatched -- run the plan again to retry failed tasks"
    ));
    exit::FAILED
}

/// `workflow accept <task>` -- land a task the run settled as failed, as it
/// stands.
///
/// A live run is handed a marker its poll loop reads. With nobody live it
/// merges here: the run that failed the task ends in the same pass as the
/// failure, so waiting for a window in which a marker could be honoured
/// meant racing a fresh `workflow run` into its first second, and losing
/// that race spent another worker on work already settled.
///
/// `regate` is `workflow regate <task>`: the merge a worker's `ready`
/// starts. With nobody live its marker waits for the next run instead of a
/// merge here.
pub fn cmd_accept(task: &str, regate: bool) -> i32 {
    let verb = if regate { "regate" } else { "accept" };
    if !Git::here().inside_worktree() {
        warn(format!("{verb}: stand in the project checkout"));
        return exit::USAGE;
    }
    memcli::resolve_from_here();
    let Some((_git, top)) = repo::goto_toplevel() else {
        return exit::USAGE;
    };
    let Some(project) = memcli::project_current() else {
        warn(format!("{verb}: mem does not know this checkout"));
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
    // A task the ownership gate refused would meet the same gate in the
    // merge and be refused again -- which is what happened, after a line
    // that said it was merging.
    let refused_ownership =
        |dir: &Path| field(dir, task, "failed").starts_with("wrote outside its Files: patterns");
    // The live run first: it holds the project lock, and a merge from out
    // here would run beside its own.
    for dir in &dirs {
        if lock_run(dir).is_some() {
            continue;
        }
        if field(dir, task, "state") != FAILED {
            continue;
        }
        if refused_ownership(dir) {
            return ownership_refusal(task);
        }
        let _ = std::fs::write(dir.join(format!("{task}.{verb}")), "");
        let run_name = dir
            .file_name()
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_default();
        if regate {
            warn(format!(
                "run {run_name}: asked to gate {task} again -- it merges on the next poll, suite included"
            ));
            return exit::OK;
        }
        warn(format!(
            "run {run_name}: asked to accept {task} as it stands -- it merges on the next poll"
        ));
        return exit::OK;
    }
    // With nobody live it is the last run's state that is accepted, and only
    // that one's: a task id stands in as many of a project's plans as name
    // it, and an older run's failure is not the one just read out.
    let last = dirs.into_iter().max_by_key(|d| {
        recorded(d, "started")
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0)
    });
    if let Some(dir) = last
        && field(&dir, task, "state") == FAILED
    {
        if refused_ownership(&dir) {
            return ownership_refusal(task);
        }
        // The marker waits for the next run, which honours it before it
        // dispatches anything.
        if regate {
            let _ = std::fs::write(dir.join(format!("{task}.regate")), "");
            warn(format!(
                "{task}: marked to gate again -- `workflow run` merges it first, suite included"
            ));
            return exit::OK;
        }
        return accept_here(&dir, &top, &project.dir_name(), task);
    }
    warn(format!(
        "no run here holds {task} failed -- `workflow status` says how the last one ended"
    ));
    exit::FAILED
}

fn ownership_refusal(task: &str) -> i32 {
    warn(format!(
        "cannot accept {task}: it failed the Files gate -- widen its Files line in \
         the live plan (`mem plan > tmp`, edit, `mem plan --stdin < tmp`) and `workflow \
         redispatch {task}`"
    ));
    exit::FAILED
}

/// The merge `workflow accept` does itself when no orchestrator is live: the
/// run is rebuilt off its dir the way `workflow reap` rebuilds one, its
/// integration worktree made again, and [`Run::accept`] runs there --
/// ownership, the words, the rebase and the gate's own suite.
fn accept_here(dir: &Path, top: &Path, project: &str, task: &str) -> i32 {
    let Some(plan_id) = dir.file_name().map(|n| n.to_string_lossy().to_string()) else {
        return exit::FAILED;
    };
    let Some(mut parsed) = std::fs::read_to_string(dir.join("plan.md"))
        .ok()
        .and_then(|text| plan::parse(&text, true))
    else {
        warn(format!(
            "accept: run {plan_id} did not record a plan to merge against"
        ));
        return exit::FAILED;
    };
    parsed.plan_id = plan_id;
    let base = std::fs::read_to_string(dir.join("base_sha"))
        .unwrap_or_default()
        .trim()
        .to_string();
    if base.is_empty() {
        warn(format!(
            "accept: run {} did not record the commit it started from",
            parsed.plan_id
        ));
        return exit::FAILED;
    }
    let (mut run, _) = new_run(parsed, top.to_path_buf(), project, base);
    run.dir = dir.to_path_buf();
    // Where this plan's merges are ticked off, as the run itself recorded it.
    run.plan_file = recorded(dir, "plan-file")
        .filter(|f| !f.is_empty())
        .map(PathBuf::from);
    let Some(_lock) = lock_run(&run.dir) else {
        warn(format!(
            "run {}: an orchestrator took this run while accept was reading it -- ask it again",
            run.plan.plan_id
        ));
        return exit::FAILED;
    };
    if !run.resumable(task) {
        warn(format!(
            "accept: {task} is {} with {} commit(s) on its branch -- there is nothing to merge",
            run.state(task),
            run.commits(task)
        ));
        return exit::USAGE;
    }
    // The gate reads the task's own worktree for what it wrote outside its
    // Files, and an earlier pass may have swept it.
    if !run.int_worktree() || !run.make_worktree(task) {
        return exit::FAILED;
    }
    run.accept(task);
    let merged = run.state(task) == MERGED;
    run.cleanup();
    if !merged {
        warn(format!(
            "accept: {task} did not merge -- {}",
            run.field(task, "failed")
        ));
        return exit::FAILED;
    }
    warn(format!(
        "run {}: {task} is on {}; run the plan again to land it on the trunk",
        run.plan.plan_id, run.int_branch
    ));
    exit::OK
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
        let (mut run, _) = new_run(parsed, top.clone(), &project.dir_name(), base);
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
        if run.running() == 0 {
            continue;
        }
        if run.reap_pass() {
            did = true;
        }
        // Alive and legitimately mid-task, with no orchestrator left to gate
        // them. Not reap's to stop -- the next run adopts them as they stand
        // -- but saying "nothing to collect" about them sent their reader
        // away believing no worker existed.
        let waiting: Vec<String> = run
            .dispatched()
            .into_iter()
            .filter(|t| run.alive(t) || run.rate_limited(t))
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
    if stalled(
        backend::backend_for().as_ref(),
        rundir,
        wtroot,
        task,
        deadline,
    ) {
        exit::OK
    } else {
        exit::FAILED
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_runner_claim_is_fresh_for_an_hour() {
        let now: jiff::Timestamp = "2026-10-03T12:00:00Z".parse().unwrap();
        assert!(super::claim_fresh("2026-10-03T11:30:00Z", now));
        assert!(!super::claim_fresh("2026-10-03T10:00:00Z", now));
        assert!(
            !super::claim_fresh("", now),
            "a claim with no start time is judged by the run log alone"
        );
    }

    #[test]
    fn a_report_reads_the_same_with_its_fields_swapped() {
        let a = super::status_line("2026-09-20T13:16Z ready auth in 7a2acf3: green");
        let b = super::status_line("ready 2026-09-20T13:16Z auth in 7a2acf3: green");
        assert_eq!(a, Some(("ready".into(), "auth in 7a2acf3: green".into())));
        assert_eq!(a, b);
        assert_eq!(
            super::status_line("blocked: 2026-09-20T13:16Z asked #AB12CD34"),
            Some(("blocked".into(), "asked #AB12CD34".into()))
        );
        assert_eq!(super::status_line("--- attempt 2 ---"), None);
        assert_eq!(super::status_line("ready"), None);
    }

    #[test]
    fn the_last_report_is_read_after_the_last_marker() {
        let text = "--- attempt 1 ---\n2026-09-20T13:00Z started\n2026-09-20T13:01Z ready done\n--- attempt 2 ---\n";
        assert_eq!(
            super::last_status_in(text),
            None,
            "attempt 2 has said nothing yet"
        );
        let more = format!("{text}2026-09-20T13:05Z started again\n");
        assert_eq!(
            super::last_status_in(&more),
            Some(("started".into(), "again".into()))
        );
        assert_eq!(super::status_after_marker("no marker\n"), "no marker\n");
    }

    use super::*;

    #[test]
    fn the_last_thing_heard_is_the_workers_words_else_the_panes_last_line() {
        assert_eq!(
            last_heard_in("You've reached your limit.\nmore", ""),
            " -- it last said: You've reached your limit."
        );
        // Words beat the screen, however little was said.
        assert_eq!(
            last_heard_in("\n ok \n", "boot\nerror"),
            " -- it last said: ok"
        );
        // A worker that never spoke: the pane's last line, not its first,
        // since a boot paints its banner first and its death last.
        assert_eq!(
            last_heard_in("", "claude 2.1\n\nAPI Error: 400 schema mismatch\n\n"),
            " -- the pane last showed: API Error: 400 schema mismatch"
        );
        assert_eq!(last_heard_in("", ""), "");
        assert_eq!(last_heard_in(" \n", "\n\n"), "");
    }

    fn t(id: &str, state: &str, note: &str) -> (String, String, String) {
        (id.into(), state.into(), note.into())
    }

    #[test]
    fn failing_checks_names_vitest_lines_and_the_timeout_hint_reads_the_output() {
        let out = "RUN v1\n ✗ renders the cart > Test timed out in 5000ms\n FAIL src/cart.test.ts\nTests 1 failed\n";
        assert_eq!(
            failing_checks(out),
            vec![
                "✗ renders the cart > Test timed out in 5000ms",
                "FAIL src/cart.test.ts"
            ]
        );
        assert_eq!(
            timeout_hint(out),
            "; a test timed out rather than asserting"
        );
        assert_eq!(timeout_hint("not ok 2 - x\n"), "");
    }

    #[test]
    fn the_path_hint_reads_the_failures_and_not_the_build_lines_around_them() {
        let wt = Path::new("/state/wt/amx/plan");
        let cargo = "   Compiling amx v0.1.0 (/state/wt/amx/plan/_integration)\n\
                     test spawn::tests::two_spawns_at_the_cap_start_one ... FAILED\n\n\
                     failures:\n\n\
                     ---- spawn::tests::two_spawns_at_the_cap_start_one stdout ----\n\
                     nobody holds the count: WouldBlock\n\n\
                     failures:\n";
        assert_eq!(path_hint(cargo, wt), "", "the panic names no path");
        let tap = "ok 1 - a\nnot ok 3 - broke in /state/wt/amx/plan/_integration\nok 4 - b\n";
        assert!(!path_hint(tap, wt).is_empty(), "the not ok line names it");
        // Nothing in a failure shape: the whole output is all there is.
        assert!(!path_hint("error in /state/wt/amx/plan/x\n", wt).is_empty());
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
    fn optional_dial_drops_none_spelled_any_case_from_env_or_recorded() {
        let var = "WORKFLOW_TEST_OPTIONAL_DIAL_NONE";
        unsafe { std::env::remove_var(var) };

        // A recorded value spelling "none", env unset: filtered to nothing,
        // and the record is still the rung it came off.
        assert_eq!(
            optional_dial(var, Some("NoNe".to_string()), || Some("opus".to_string())),
            (None, Dialed::Record)
        );
        // A recorded value that is not "none" passes through untouched.
        assert_eq!(
            optional_dial(var, Some("opus".to_string()), || None),
            (Some("opus".to_string()), Dialed::Record)
        );
        // A project key spelling "none" is not this function's to filter.
        assert_eq!(
            optional_dial(var, None, || Some("none".to_string())),
            (Some("none".to_string()), Dialed::Project)
        );
        // Nothing on any rung is the default, not the project's say.
        assert_eq!(optional_dial(var, None, || None), (None, Dialed::Default));

        unsafe { std::env::set_var(var, "NONE") };
        assert_eq!(
            optional_dial(var, Some("opus".to_string()), || Some("opus".to_string())),
            (None, Dialed::Env)
        );
        unsafe { std::env::remove_var(var) };
    }

    #[test]
    fn the_start_line_names_the_rung_each_dial_came_off() {
        assert_eq!(
            dial_line(("opus", Dialed::Record), (Some("max"), Dialed::Env)),
            "writing with opus (the run's record) at effort max (the environment)"
        );
        // Nothing set for the effort is no flag and no clause.
        assert_eq!(
            dial_line(("opus", Dialed::Default), (None, Dialed::Project)),
            "writing with opus (the default)"
        );
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
    fn a_red_gate_names_its_own_worktree_path_when_the_output_carries_it() {
        let wt_root = Path::new("/state/workflow/worktrees/app/demo");
        let out = "not ok 3 - fails in /state/workflow/worktrees/app/demo/_integration\n";
        assert_eq!(
            path_hint(out, wt_root),
            " -- the failure text carries this run's worktree path, which every spawned command line holds; a test asserting a word absent from a command is reading the path"
        );
        assert_eq!(path_hint("not ok 3 - just red\n", wt_root), "");
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
        let why =
            refused(&doc(PlanKind::Roadmap)).expect("a roadmap is not work a worker can take");
        assert!(why.contains("'amx-v2' is a roadmap"), "{why}");
        assert!(why.contains("mem plan --from <slug>"), "{why}");
    }

    #[test]
    fn a_plan_starts_whoever_reads_it() {
        assert_eq!(refused(&doc(PlanKind::Plan)), None);
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
        let (workers, deadline, grace, poll, misses) = timings();
        assert!((1..=3).contains(&workers));
        assert!(deadline >= 1);
        assert!((1..=30).contains(&grace));
        assert!((0.2..=5.0).contains(&poll));
        assert_eq!(misses, QUESTION_MISS_LIMIT);
    }

    /// Dispatched long ago and silent since, but seen waiting on the usage
    /// window ten seconds ago: the deadline counts from that sighting, so a
    /// worker that comes back from the limit gets its whole clock.
    #[test]
    fn a_stall_runs_from_the_last_time_the_worker_was_seen_rate_limited() {
        let dir = std::env::temp_dir().join(format!("wf-run-limited-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let backend = crate::backend::ProcessBackend;
        let now = sys::now();
        write_field(&dir, "t1", "dispatched_at", &(now - 600).to_string());
        assert!(stalled(&backend, &dir, &dir, "t1", 60));
        write_field(&dir, "t1", "limited_at", &(now - 10).to_string());
        assert!(!stalled(&backend, &dir, &dir, "t1", 60));
        write_field(&dir, "t1", "limited_at", &(now - 120).to_string());
        assert!(stalled(&backend, &dir, &dir, "t1", 60));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
