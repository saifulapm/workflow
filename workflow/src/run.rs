//! `workflow run` and `workflow reap` -- deterministic interim orchestration
//! (spec §8).
//!
//! Policy lives here and nowhere else: waves, concurrency, ownership, the
//! serialized merge gate, park. How a worker is started, watched and stopped is
//! the backend's business (see [`crate::backend`]).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::backend::{ClaudeBackend, Dispatch, Handle, WorkerBackend};
use crate::gitcmd::Git;
use crate::plan::{Plan, Task};
use crate::{brief, exit, lint, memcli, ownership, park, paths, plan, repo, sys, warn};

pub const PENDING: &str = "pending";
pub const DISPATCHED: &str = "dispatched";
pub const MERGED: &str = "merged";
pub const PARKED: &str = "parked";
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
/// line, a shared `CARGO_TARGET_DIR` flattens the worktree's mtime, and a worker
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

    fn commits(&self, task: &str) -> u64 {
        self.git()
            .count(&format!("{}..{}", self.base, self.branch(task)))
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

    fn dispatch(&self, task: &str) {
        let Some(t) = self.plan.get(task).cloned() else {
            return;
        };
        let wt = self.worktree(task);
        let brief_file = self.brief_dir.join(format!("{task}.md"));
        let status = self.dir.join(format!("{task}.status"));
        let session = self.backend.mint_session();

        write_field(&self.dir, task, "session", &session);
        let _ = std::fs::write(&status, "");
        for ext in ["json", "err", "pid"] {
            let _ = std::fs::remove_file(self.dir.join(format!("{task}.{ext}")));
        }
        brief::write(&t, &wt, &status, &brief_file);
        // The gate reads this from inside the worktree: the task is held to its
        // own Verify command there, not to the repo-wide suite (verify.rs).
        write_field(&self.dir, task, "verify", t.verify.as_deref().unwrap_or(""));

        let n: u64 = self.field(task, "dispatches").parse().unwrap_or(0);
        write_field(&self.dir, task, "dispatches", &(n + 1).to_string());
        write_field(&self.dir, task, "dispatched_at", &sys::now().to_string());

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
            model: env_str("WORKFLOW_MODEL", "sonnet"),
            budget: env_str("WORKFLOW_WORKER_BUDGET", "10"),
            turns: env_str("WORKFLOW_MAX_TURNS", "120"),
            env: self.env.clone(),
        };

        self.set_state(task, DISPATCHED);
        warn(format!("task {task}: dispatched (session {session})"));
        memcli::log(&format!("run {}: dispatched {task}", self.plan.plan_id));
        self.backend.dispatch(&d);
    }

    fn record_cost(&self, task: &str) {
        let outcome = self
            .backend
            .result(&self.handle(task), &self.dir.join(format!("{task}.json")));
        let line = format!("{task}\t{}\n", outcome.cost);
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("costs.tsv"))
        {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn spend(&self) -> f64 {
        std::fs::read_to_string(self.dir.join("costs.tsv"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split('\t').nth(1))
            .filter_map(|c| c.parse::<f64>().ok())
            .sum()
    }

    fn over_budget(&self) -> bool {
        self.spend() >= env_f64("WORKFLOW_RUN_BUDGET", 40.0)
    }

    // ------------------------------------------------------------ merge gate

    /// Serialized, one task at a time, and in this order: ownership, then the
    /// words, then rebase onto the integration branch, and only then verify --
    /// verifying before the rebase lets a semantic conflict land green
    /// (review-3 F-6).
    fn merge(&self, task: &str) -> Result<(), String> {
        let branch = self.branch(task);
        let wt = self.worktree(task);

        if self.commits(task) == 0 {
            return Err("the worker reported ready but committed nothing".into());
        }

        let patterns = ownership::split_patterns(
            self.plan
                .get(task)
                .and_then(|t| t.files.as_deref())
                .unwrap_or(""),
        );
        let bad = ownership::violations(&wt, &self.base, &branch, &patterns);
        if !bad.is_empty() {
            warn(format!("task {task}: touched files it does not own --"));
            for line in ownership::show(&bad) {
                warn(format!("  {line}"));
            }
            return Err("wrote outside its Files: patterns".into());
        }

        let msgs = self
            .git()
            .out(&["log", "--format=%B", &format!("{}..{branch}", self.base)])
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
        if !int.quiet(&["rebase", "--onto", &self.int_branch, &self.base]) {
            int.quiet(&["rebase", "--abort"]);
            int.quiet(&["checkout", "-q", &self.int_branch]);
            return Err("conflicts with the integration branch".into());
        }
        let new = int.head().unwrap_or_default();
        if !int.quiet(&["checkout", "-q", &self.int_branch]) {
            return Err(format!("cannot return to {}", self.int_branch));
        }
        if !int.quiet(&["merge", "-q", "--ff-only", &new]) {
            return Err("the rebased branch does not fast-forward onto integration".into());
        }

        if !self.gate_verify() {
            int.quiet(&["reset", "-q", "--hard", &prev]);
            return Err("the suite is red once the change sits on integration".into());
        }

        if !self
            .git()
            .quiet(&["update-ref", &self.task_ref(task), &new])
        {
            warn(format!(
                "task {task}: could not record {new} as the commit that landed"
            ));
        }
        write_field(&self.dir, task, "merged", &new);
        Ok(())
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
        c.status().map(|s| s.success()).unwrap_or(false)
    }

    fn park_task(&self, task: &str, why: &str) {
        let wt = self.worktree(task);
        warn(format!("task {task}: parked -- {why}"));
        self.set_state(task, PARKED);
        write_field(&self.dir, task, "parked", why);
        if wt.is_dir() {
            let label = format!("{}-{task}", self.plan.plan_id);
            let branch = self.branch(task);
            let args = park::Args {
                repo: Some(&wt),
                branch: Some(&branch),
                base: Some(&self.base),
                note: why,
                label: Some(&label),
                quiet: true,
            };
            if park::park_quietly(&args).is_err() {
                warn(format!(
                    "task {task}: could not bundle the parked work (the branch is still here)"
                ));
            }
        }
        memcli::log(&format!(
            "run {}: parked {task} -- {why}",
            self.plan.plan_id
        ));
    }

    fn finish(&self, task: &str) {
        self.record_cost(task);
        let outcome = self
            .backend
            .result(&self.handle(task), &self.dir.join(format!("{task}.json")));
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
                self.park_task(task, "dispatch race: worker never wrote its pidfile");
            } else {
                self.park_task(task, "the worker exited with an error");
            }
            return;
        }
        match self.last_status_line(task) {
            // The worker said where it stood; the park must not claim otherwise.
            Some((state, note)) if state != "ready" => {
                let why = if note.is_empty() {
                    format!("the worker's last report was '{state}'")
                } else {
                    format!("the worker's last report was '{state}: {note}'")
                };
                self.park_task(task, &why);
                return;
            }
            None => {
                self.park_task(task, "the worker stopped without reporting ready");
                return;
            }
            Some(_) => {}
        }
        match self.merge(task) {
            Ok(()) => {
                self.set_state(task, MERGED);
                warn(format!("task {task}: merged onto {}", self.int_branch));
                if !memcli::plan_tick(task) {
                    warn(format!(
                        "task {task}: mem could not tick it off (is this plan in mem?)"
                    ));
                }
                memcli::log(&format!("run {}: merged {task}", self.plan.plan_id));
            }
            Err(why) => self.park_task(task, &why),
        }
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
                self.record_cost(&task); // whatever it spent before it went quiet
                did = true;
                let tries: u64 = self.field(&task, "dispatches").parse().unwrap_or(0);
                if self.commits(&task) == 0 && tries < 2 {
                    warn(format!(
                        "task {task}: stalled with nothing committed -- one more try"
                    ));
                    self.dispatch(&task);
                } else {
                    self.park_task(&task, "stalled with no sign of life");
                }
                continue;
            }
            did = true;
            self.finish(&task);
        }
        did
    }

    fn kill_everything(&self) {
        for task in self.dispatched() {
            self.stop(&task);
            self.park_task(&task, "the run hit its budget and every worker was stopped");
        }
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
            if git.rev_parse_commit(&branch).is_some() {
                leftovers.push(branch);
            }
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
        if !self.dir.join("costs.tsv").exists() {
            let _ = std::fs::write(self.dir.join("costs.tsv"), "");
        }

        // One cargo target for the whole project, so five worktrees do not each
        // rebuild the world. It flattens the worktree mtime signal, which is why
        // liveness has three sources rather than one.
        if self.repo.join("Cargo.toml").is_file() && std::env::var("CARGO_TARGET_DIR").is_err() {
            let target = paths::state_home()
                .join("workflow/cargo")
                .join(&self.project);
            let _ = std::fs::create_dir_all(&target);
            self.env.push((
                "CARGO_TARGET_DIR".into(),
                target.to_string_lossy().to_string(),
            ));
        }

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
    }

    fn cleanup(&self) {
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
            if self.state(&t) == MERGED {
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
        backend: Box::new(ClaudeBackend),
        env: Vec::new(),
        made: Vec::new(),
    }
}

pub fn cmd_run(plan_file: Option<&Path>) -> i32 {
    if !Git::here().inside_worktree() {
        warn("run: stand in the project checkout");
        return exit::USAGE;
    }
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

    if !run.setup() {
        run.rollback();
        return exit::USAGE;
    }
    let _ = std::fs::write(run.dir.join("plan.md"), &source);
    for t in run.plan.ids() {
        if !run.dir.join(format!("{t}.state")).exists() {
            run.set_state(&t, PENDING);
        }
    }
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

    let mut stopped = false;
    for wave in run.plan.waves.clone() {
        if stopped {
            break;
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
            if run.over_budget() {
                warn(format!(
                    "run {}: spent ${:.4} against a ${} ceiling -- stopping",
                    run.plan.plan_id,
                    run.spend(),
                    env_str("WORKFLOW_RUN_BUDGET", "40")
                ));
                run.kill_everything();
                stopped = true;
                break;
            }
            while !queue.is_empty() && run.running() < run.max_workers {
                let next = queue.remove(0);
                run.dispatch(&next);
            }
            sys::sleep(run.poll);
            run.reap_pass();
        }
    }

    let (mut merged, mut parked, mut blocked, mut previous) = (0, 0, 0, 0);
    for t in run.plan.ids() {
        match run.state(&t).as_str() {
            MERGED => merged += 1,
            PARKED => parked += 1,
            DONE_PREVIOUSLY => previous += 1,
            _ => blocked += 1,
        }
    }

    run.cleanup();

    warn(format!(
        "run {}: {merged} merged, {parked} parked, {blocked} never started; spent ${:.4}",
        run.plan.plan_id,
        run.spend()
    ));
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
    if parked + blocked > 0 {
        let mut list = String::new();
        for t in run.plan.ids() {
            let state = run.state(&t);
            if state == MERGED || state == DONE_PREVIOUSLY {
                continue;
            }
            list.push_str(&format!(" {t}({state}): {};", run.field(&t, "parked")));
        }
        memcli::ask(&format!(
            "Plan {} stopped short.{list} What should happen to these?",
            run.plan.plan_id
        ));
        return exit::FAILED;
    }
    exit::OK
}

pub fn cmd_reap() -> i32 {
    if !Git::here().inside_worktree() {
        warn("reap: stand in the project checkout");
        return exit::OK;
    }
    let Some((_git, top)) = repo::goto_toplevel() else {
        return exit::OK;
    };
    let Some(project) = memcli::project_current() else {
        warn("reap: mem does not know this checkout");
        return exit::OK;
    };

    let mut did = false;
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
        if run.running() == 0 {
            continue;
        }
        if run.reap_pass() {
            did = true;
        }
    }

    if did {
        return exit::FAILED;
    }
    warn("reap: nothing to collect");
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
