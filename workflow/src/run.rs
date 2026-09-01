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
use crate::plan::{Plan, Task};
use crate::{brief, exit, lint, memcli, ownership, paths, plan, repo, sys, warn};

pub const PENDING: &str = "pending";
pub const DISPATCHED: &str = "dispatched";
pub const MERGED: &str = "merged";
pub const FAILED: &str = "failed";
pub const BLOCKED: &str = "blocked";
pub const DONE_PREVIOUSLY: &str = "done-previously";

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

fn write_field(dir: &Path, task: &str, ext: &str, value: &str) {
    let _ = std::fs::write(dir.join(format!("{task}.{ext}")), format!("{value}\n"));
}

/// Liveness is the latest of three signals, because each one alone has a way of
/// going quiet on a worker that is fine: a long test run writes no transcript
/// line, an out-of-tree `CARGO_TARGET_DIR` flattens the worktree's mtime, and a worker
/// that is only thinking touches neither. The status file is the worker's own
/// heartbeat and it lives in the run directory, not the worktree, so it has to
/// be counted separately (review-3 F-10).
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

pub struct Run {
    pub plan: Plan,
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
    pub env: Vec<(String, String)>,
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

    fn cargo_env(&self, who: &str) -> Option<(String, String)> {
        if !self.repo.join("Cargo.toml").is_file() {
            return None;
        }
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
        let mut last = None;
        for line in text.lines() {
            let mut fields = line.split_whitespace();
            let (Some(_utc), Some(state)) = (fields.next(), fields.next()) else {
                continue;
            };
            last = Some((state.to_string(), fields.collect::<Vec<_>>().join(" ")));
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

    /// The commit some run merged for this task, or empty.
    fn prior_sha(&self, task: &str) -> String {
        self.git()
            .rev_parse_commit(&self.task_ref(task))
            .unwrap_or_else(|| self.field(task, "merged"))
    }

    /// Is that commit on the integration branch as it stands?
    fn landed(&self, task: &str) -> bool {
        let sha = self.prior_sha(task);
        !sha.is_empty() && self.git().is_ancestor(&sha, &self.int_branch)
    }

    fn alive(&self, task: &str) -> bool {
        self.backend.alive(&self.handle(task))
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
        if git.head() == self.git().rev_parse_commit(&self.int_branch) {
            return; // already there, and on wave one it always is
        }
        if !git.quiet(&["merge", "-q", "--ff-only", &self.int_branch]) {
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
        brief::Prior {
            attempts,
            why,
            last_report,
        }
    }

    fn dispatch(&self, task: &str, after: &str) {
        let Some(t) = self.plan.get(task).cloned() else {
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
            model: env_str("WORKFLOW_MODEL", "opus"),
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
    /// Ok(true) is a merge; Ok(false) is a ready worker with nothing to
    /// merge, which is its way of saying the work is already in the tree it
    /// opened onto (friction #B2D8SJKR).
    fn merge(&self, task: &str) -> Result<bool, String> {
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
            return self.settle_interrupted_merge(task, &prev, &new).map(|()| true);
        }

        if self.commits(task) == 0 {
            return Ok(false);
        }

        let patterns = ownership::split_patterns(
            self.plan
                .get(task)
                .and_then(|t| t.files.as_deref())
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

        if !self.gate_verify() {
            int.quiet(&["reset", "-q", "--hard", &prev]);
            write_field(&self.dir, task, "merging", "");
            return Err("the suite is red once the change sits on integration".into());
        }

        self.record_merged(task, &new);
        Ok(true)
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
    fn settle_interrupted_merge(
        &self,
        task: &str,
        prev: &str,
        new: &str,
    ) -> Result<(), String> {
        warn(format!(
            "task {task}: its merge reached {} before the run died -- verifying it now",
            self.int_branch
        ));
        if self.gate_verify() {
            self.record_merged(task, new);
            return Ok(());
        }
        // Unwind only what nothing was built on. A later run may have merged
        // other tasks on top, and taking those down with this one would be a
        // worse answer than a red branch and a person told why.
        let int = Git::at(&self.int_wt);
        if int.head().as_deref() == Some(new) {
            int.quiet(&["reset", "-q", "--hard", prev]);
            write_field(&self.dir, task, "merging", "");
            return Err("the suite is red once the change sits on integration".into());
        }
        Err(format!(
            "its merge landed before the run died, {} is red now, and other work sits on top of it",
            self.int_branch
        ))
    }

    /// verify, on the integration branch, as its own process: the same
    /// authoritative gate a human would run there.
    fn gate_verify(&self) -> bool {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("workflow"));
        let mut c = Command::new(exe);
        c.arg("verify").arg("--gate").current_dir(&self.int_wt);
        for (k, v) in &self.env {
            c.env(k, v);
        }
        if let Some((k, v)) = self.cargo_env("integration") {
            c.env(k, v);
        }
        c.status().map(|s| s.success()).unwrap_or(false)
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
            warn(format!(
                "task {task}: its worker died leaving nothing -- one more try"
            ));
            self.dispatch(
                task,
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
            // The worker said where it stood; the failure must not claim
            // otherwise.
            Some((state, note)) if state != "ready" => {
                let why = if note.is_empty() {
                    format!("the worker's last report was '{state}'")
                } else {
                    format!("the worker's last report was '{state}: {note}'")
                };
                self.fail_task(task, &why);
                return;
            }
            None => {
                self.fail_task(task, "the worker stopped without reporting ready");
                return;
            }
            Some(_) => {}
        }
        match self.merge(task) {
            Ok(true) => {
                self.set_state(task, MERGED);
                warn(format!("task {task}: merged onto {}", self.int_branch));
                if !memcli::plan_tick(task) {
                    warn(format!(
                        "task {task}: mem could not tick it off (is this plan in mem?)"
                    ));
                }
                memcli::log(&format!("run {}: merged {task}", self.plan.plan_id));
            }
            // Ready with nothing committed: the worker found its Done already
            // satisfied -- rebuilt by hand between passes, or landed by an
            // earlier plan. Failing it skipped every dependent behind work
            // that exists (friction #B2D8SJKR).
            Ok(false) => {
                self.set_state(task, DONE_PREVIOUSLY);
                warn(format!(
                    "task {task}: reported ready with nothing to commit -- its work is already in the tree"
                ));
                if !memcli::plan_tick(task) {
                    warn(format!(
                        "task {task}: mem could not tick it off (is this plan in mem?)"
                    ));
                }
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
            if state == DISPATCHED || state == PENDING || state.is_empty() {
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
                    warn(format!(
                        "task {task}: stalled with nothing committed -- one more try"
                    ));
                    self.dispatch(&task, "it stalled with nothing committed and was stopped");
                } else {
                    self.fail_task(&task, "stalled with no sign of life");
                }
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
        let taken = self.dispatched();
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
                    self.dispatch(task, "the session it was given never existed, so it never ran");
                } else {
                    warn(format!(
                        "task {task}: the recorded session never existed and the retry is spent"
                    ));
                    self.finish(task);
                }
                continue;
            }
            if self.alive(task) {
                warn(format!(
                    "task {task}: still working, from a run that is gone -- adopted"
                ));
                continue;
            }
            warn(format!(
                "task {task}: left dispatched by a run that is gone -- collecting it"
            ));
            self.finish(task);
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
            if t.checked || self.worktree(&t.id).is_dir() {
                continue; // ticked off, or an interrupted run still set up
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
            if !git.quiet(&[
                "worktree",
                "add",
                "-q",
                "-b",
                &self.branch(&t.id),
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
        let _ = std::fs::remove_dir_all(self.cargo_root());
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
        // is how phantom failures happen (friction #TFVWXXDQ).
        let _ = std::fs::remove_dir_all(self.cargo_root());
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

/// The stopped-short question, written for the person who answers it on a
/// phone (friction #VTB9VB1S): counts first, then one line per outcome group,
/// and never an empty note. Held to the same lint commits are held to.
/// `tasks` is (id, state, failure reason) in plan order.
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

    q.push_str("What should happen to these?");
    q
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

fn new_run(plan: Plan, repo: PathBuf, project: &str, base: String) -> Run {
    let (max_workers, deadline_s, kill_grace_s, poll) = timings();
    let wt_root = paths::worktrees_root().join(project).join(&plan.plan_id);
    Run {
        dir: paths::runs_root().join(project).join(&plan.plan_id),
        brief_dir: paths::briefs_root().join(project).join(&plan.plan_id),
        project: project.to_string(),
        int_branch: format!("integration/{}", plan.plan_id),
        int_wt: wt_root.join("_integration"),
        wt_root,
        plan,
        repo,
        base,
        deadline_s,
        kill_grace_s,
        poll,
        max_workers,
        backend: backend_for(),
        env: Vec::new(),
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

    if parsed.tasks.len() <= 1 {
        warn(format!(
            "plan '{}' has one task: do it here, in this session -- orchestrating one worker costs more than it saves.",
            parsed.plan_id
        ));
        return exit::OK;
    }

    let Some(base) = git.head() else {
        return exit::USAGE;
    };
    let mut run = new_run(parsed, top, &project.dir_name(), base);

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
    let asked_to_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    for sig in [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGHUP,
    ] {
        let _ = signal_hook::flag::register(sig, asked_to_stop.clone());
    }
    let stopping = || asked_to_stop.load(std::sync::atomic::Ordering::Relaxed);

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

        while !queue.is_empty() || run.running() > 0 {
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
        memcli::ask(&stopped_short(&run.plan.plan_id, &tasks));
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
        let _ = std::fs::write(dir.join(format!("{task}.redispatch")), "");
        let plan_id = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        warn(format!(
            "run {plan_id}: asked to dispatch {task} again -- it goes on the next poll while its wave is open"
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
        let mut run = new_run(parsed, top.clone(), &project.dir_name(), base);
        run.dir = dir;
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
        // away believing no worker existed (friction #RF50DJXQ).
        let waiting: Vec<String> = run
            .dispatched()
            .into_iter()
            .filter(|t| run.alive(t))
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
            t("t3", FAILED, "the suite is red once the change sits on integration"),
            t("t4", FAILED, "the suite is red once the change sits on integration"),
            t("t5", FAILED, "wrote outside its Files: patterns"),
            t("t6", BLOCKED, ""),
            t("t7", PENDING, ""),
        ];
        let q = stopped_short("amx-v2", &tasks);
        assert!(
            q.starts_with(
                "Plan amx-v2 stopped short: 2 of 7 merged, 3 failed, 2 never started."
            ),
            "counts do not lead: {q}"
        );
        assert!(
            q.contains(
                "Failed - the suite is red once the change sits on integration: t3, t4."
            ),
            "failed tasks are not grouped by reason: {q}"
        );
        assert!(q.contains("Failed - wrote outside its Files: patterns: t5."));
        assert!(q.contains("Never started: t6, t7."));
        assert!(q.trim_end().ends_with("What should happen to these?"));
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
        let tasks = [t("t1", MERGED, ""), t("t2", FAILED, "the worker exited with an error")];
        let q = stopped_short("p", &tasks);
        assert!(q.starts_with("Plan p stopped short: 1 of 2 merged, 1 failed."));
        assert!(!q.contains("0 never started"), "{q}");
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
