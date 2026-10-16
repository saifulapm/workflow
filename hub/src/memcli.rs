//! Running `mem`.
//!
//! Two rules govern everything here.
//!
//! **argv, never a shell.** The text this module passes to `mem answer` was
//! typed into a form on a phone; with `sh -c` anywhere on the path, that field
//! is arbitrary command execution as Saiful. There is no string
//! interpolation into a command line in this file, and there is no `sh`.
//!
//! **An exit code from mem is not a yes/no.** The table below was verified
//! live against the built binary:
//!
//! | call | exit | stdout |
//! |---|---|---|
//! | `questions --pending --all-projects --for human --json`, none pending | 1 | `{"questions":[]}` |
//! | `log --limit N --project X --json`, none | 1 | `{"items":[]}` |
//! | `projects --json`, none | 0 | `{"projects":[]}` |
//! | `handoff --project <none recorded> --json` | 1 | **empty** |
//! | `handoff --project no-such-project --json` | 1 | empty; stderr is plain text |
//! | `answer <unknown id>` | 1 | empty; stderr is plain text |
//!
//! So exit 1 with a document is an *empty result*, exit 1 with nothing is
//! *absent*, and only a document that will not parse — or a missing binary —
//! is mem being broken.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hubpages::{Mem, Source, Written};
use serde_json::Value;

use crate::proc::{self, Ended};

// The listing and the outcome are the pages', so a Worker classifies
// mem's word the same way; this file runs the binary behind them.
pub use hubpages::{Outcome, QUESTIONS_ARGV};

/// A small in-process cache with a 5 s TTL, so a phone refresh doesn't
/// stampede. The key is the whole argv, so it is per-project by construction.
pub const CACHE_TTL: Duration = Duration::from_secs(5);

/// How long one `mem` may take before it is killed. The "mem is broken" row of
/// the table above exists for a mem that cannot answer; without a ceiling there
/// would be no way to reach it, because a child that never returns never fails
/// either.
pub const DEFAULT_MEM_TIMEOUT: Duration = Duration::from_secs(5);

/// 5 s, or `HUB_MEM_TIMEOUT_MS` — the same kind of seam as `HUB_POLL_MS`, so
/// the suite can prove the ceiling without spending five seconds per case.
pub fn mem_timeout() -> Duration {
    std::env::var("HUB_MEM_TIMEOUT_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis)
        // A zero would kill every child before it could exec.
        .map(|t| t.max(Duration::from_millis(10)))
        .unwrap_or(DEFAULT_MEM_TIMEOUT)
}

/// One raw run of the binary.
#[derive(Debug, Clone)]
pub struct Run {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    /// Plain text under `--json` too, so it is never parsed — only shown.
    pub stderr: String,
}

impl Run {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }

    /// mem is inside the cooling-off window after a child had to be killed. No
    /// exit code, so `classify` reads it as mem being broken — which it is.
    fn unavailable() -> Run {
        Run {
            code: None,
            stdout: Vec::new(),
            stderr: "mem is not answering: an earlier call had to be killed".to_string(),
        }
    }
}

/// Keyed by the whole argv; the instant is when the entry was filled.
type Cache = HashMap<Vec<String>, (Instant, Arc<Outcome>)>;

pub struct MemCli {
    program: OsString,
    /// One `mem` process at a time, per hub.
    ///
    /// Found live during the build, and worth writing down: mem brings its
    /// index up to date on a read with one `BEGIN IMMEDIATE`, and *"if another
    /// invocation is already reindexing, this returns immediately with
    /// `skipped` and the caller serves the existing, possibly stale, index"*
    /// (`mem/src/index.rs`). So two hub-launched reads at the same instant —
    /// the doorbell's poll and a phone's first request, which is exactly when
    /// they collide — can leave one of them answering out of an index that has
    /// not seen the newest `mem ask`. The page then says "Nothing waiting"
    /// while a question is waiting, which is the one answer this page must
    /// never give.
    ///
    /// mem's behaviour is deliberate and documented; the fix belongs on the
    /// side that creates the contention. hub's reads are cached and take about
    /// ten milliseconds, so serialising them costs nothing measurable.
    gate: Mutex<()>,
    /// A PATH for the child only. Production leaves this `None` and inherits;
    /// the exit-code matrix sets it to a directory holding a fixture `mem`.
    path: Option<OsString>,
    cache: Mutex<Cache>,
    ttl: Duration,
    /// How long one child may take.
    timeout: Duration,
    /// When mem may be tried again, after a child had to be killed.
    ///
    /// A timeout alone is not enough. One page render is `2P+2+Q` serialised
    /// calls, so a store that has stalled — a bisync window, a wedged mount, a
    /// machine resuming — would cost five seconds *each*, the render would
    /// outlive the connection it is answering, and sixteen of those would
    /// pile up connections the same way. One kill therefore puts mem out of bounds for the
    /// cache TTL: the calls behind it fail at once and the page renders
    /// degraded, which is what the "mem is broken" outcome is for.
    unavailable_until: Mutex<Option<Instant>>,
}

impl Default for MemCli {
    fn default() -> MemCli {
        MemCli::new()
    }
}

impl MemCli {
    pub fn new() -> MemCli {
        MemCli {
            program: OsString::from("mem"),
            gate: Mutex::new(()),
            path: None,
            cache: Mutex::new(HashMap::new()),
            ttl: CACHE_TTL,
            timeout: mem_timeout(),
            unavailable_until: Mutex::new(None),
        }
    }

    /// Resolves `mem` through `path` instead of the inherited PATH.
    pub fn with_path(path: impl Into<PathBuf>) -> MemCli {
        MemCli {
            path: Some(path.into().into_os_string()),
            ..MemCli::new()
        }
    }

    pub fn with_ttl(mut self, ttl: Duration) -> MemCli {
        self.ttl = ttl;
        self
    }

    /// Runs `mem` with exactly these arguments. No shell, no interpolation,
    /// never at the same time as another one (see `gate`), and never for longer
    /// than `timeout` (see `unavailable_until`).
    pub fn exec(&self, args: &[&str]) -> Run {
        self.exec_fed(args, None)
    }

    /// `exec`, with `input` on mem's stdin.
    fn exec_fed(&self, args: &[&str], input: Option<&[u8]>) -> Run {
        // Checked *before* the gate, so the calls behind a killed one do not
        // each queue for a timeout of their own.
        if self.cooling_off() {
            return Run::unavailable();
        }
        // A poisoned gate still serialises: the guard is what matters, not the
        // unit inside it.
        let _gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        // And again, because a caller that was *waiting* on the gate passed the
        // check above before the call in front of it was killed. Without this
        // the first page load behind a stuck doorbell poll pays two ceilings
        // rather than one: five seconds waiting, five more for a child of its
        // own that was never going to answer either.
        if self.cooling_off() {
            return Run::unavailable();
        }
        let mut command = Command::new(&self.program);
        command.args(args);
        if let Some(path) = &self.path {
            command.env("PATH", path);
        }
        match proc::output_fed(&mut command, input, self.timeout) {
            Ended::Exited(done) => Run {
                code: done.code,
                stdout: done.stdout,
                stderr: String::from_utf8_lossy(&done.stderr).trim().to_string(),
            },
            Ended::TimedOut => {
                self.cool_off();
                Run {
                    // No exit code: `classify` reads that as mem being broken,
                    // which is exactly what a mem that will not answer is.
                    code: None,
                    stdout: Vec::new(),
                    stderr: format!(
                        "mem did not answer within {:?} and was killed",
                        self.timeout
                    ),
                }
            }
            Ended::Failed(why) => Run {
                // No exit code at all: the binary did not run.
                code: None,
                stdout: Vec::new(),
                stderr: format!("could not run mem: {why}"),
            },
        }
    }

    fn cooling_off(&self) -> bool {
        let Ok(guard) = self.unavailable_until.lock() else {
            return false;
        };
        let until = *guard;
        until.is_some_and(|at| Instant::now() < at)
    }

    fn cool_off(&self) {
        if let Ok(mut guard) = self.unavailable_until.lock() {
            *guard = Some(Instant::now() + self.ttl);
        }
    }

    /// A cached read. Every argv gets its own entry, so the questions list and
    /// one project's log expire independently.
    pub fn read(&self, args: &[&str]) -> Arc<Outcome> {
        let key: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        if let Ok(cache) = self.cache.lock()
            && let Some((at, outcome)) = cache.get(&key)
            && at.elapsed() < self.ttl
        {
            return Arc::clone(outcome);
        }
        self.refresh(args)
    }

    /// A read that ignores the cache and refills it.
    ///
    /// The doorbell uses this. It is the one caller for which a five-second-old
    /// answer is a five-second-late doorbell, and it has its own rate limit —
    /// the fifteen-second poll — so the cache buys it nothing and costs it
    /// latency. The page gets the fresh entry for free.
    pub fn refresh(&self, args: &[&str]) -> Arc<Outcome> {
        let key: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let outcome = Arc::new(classify(&self.exec(args)));
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(key, (Instant::now(), Arc::clone(&outcome)));
        }
        outcome
    }

    /// Drops every cached read. Called synchronously after a successful
    /// `mem answer`, **before** the redirect: without it the 5 s TTL leaves the
    /// answered question on screen, and a second tap writes a second answer
    /// that mem accepts and the waiter never sees.
    pub fn invalidate(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }

    /// The pending questions a person answers.
    pub fn questions(&self) -> Arc<Outcome> {
        Mem::questions(self)
    }

    /// The same call, straight from the binary (see `refresh`).
    pub fn questions_fresh(&self) -> Arc<Outcome> {
        Mem::questions_fresh(self)
    }

    pub fn projects(&self) -> Arc<Outcome> {
        Mem::projects(self)
    }

    pub fn wiki(&self, project: &str) -> Arc<Outcome> {
        Mem::wiki(self, project)
    }

    pub fn wiki_page(&self, project: &str, slug: &str) -> Arc<Outcome> {
        Mem::wiki_page(self, project, slug)
    }

    pub fn show(&self, id: &str) -> Arc<Outcome> {
        Mem::show(self, id)
    }

    pub fn roadmap(&self, project: &str) -> Arc<Outcome> {
        Mem::roadmap(self, project)
    }

    pub fn plan(&self, project: &str) -> Arc<Outcome> {
        Mem::plan(self, project)
    }

    pub fn plan_list(&self, project: &str) -> Arc<Outcome> {
        Mem::plan_list(self, project)
    }

    pub fn plan_slug(&self, project: &str, slug: &str) -> Arc<Outcome> {
        Mem::plan_slug(self, project, slug)
    }

    pub fn handoff(&self, project: &str) -> Arc<Outcome> {
        Mem::handoff(self, project)
    }

    pub fn log_n(&self, project: &str, limit: usize) -> Arc<Outcome> {
        Mem::log_n(self, project, limit)
    }

    pub fn items(&self, project: &str, kind: &str, limit: usize) -> Arc<Outcome> {
        Mem::items(self, project, kind, limit)
    }

    pub fn questions_all(&self, project: &str) -> Arc<Outcome> {
        Mem::questions_all(self, project)
    }

    /// A write a page makes: run it, then drop every cached read, so the page
    /// the phone lands on next shows what was just written.
    pub fn write_through(&self, args: &[&str]) -> Run {
        let run = self.exec(args);
        self.invalidate();
        run
    }

    /// `write_through` for a write that reads its text from stdin, such as
    /// a wiki page.
    pub fn write_through_fed(&self, args: &[&str], input: &[u8]) -> Run {
        let run = self.exec_fed(args, Some(input));
        self.invalidate();
        run
    }

    /// The one write hub makes. `--` first, so an answer beginning with a dash
    /// is text rather than a flag mem rejects with exit 2.
    pub fn answer(&self, id: &str, text: &str, by: &str, supersedes: Option<&str>) -> Run {
        self.exec(&hubpages::mem::answer_argv(id, text, by, supersedes))
    }
}

/// The pages' mem: the cache and the gate above, and the binary behind them.
impl Source for MemCli {
    fn read(&self, argv: &[&str]) -> Arc<Outcome> {
        MemCli::read(self, argv)
    }

    fn write(&self, argv: &[&str], input: Option<&[u8]>) -> Written {
        let run = match input {
            Some(input) => self.write_through_fed(argv, input),
            None => self.write_through(argv),
        };
        Written {
            ok: run.ok(),
            stdout: String::from_utf8_lossy(&run.stdout).into_owned(),
            stderr: run.stderr,
        }
    }

    fn refresh(&self, argv: &[&str]) -> Arc<Outcome> {
        MemCli::refresh(self, argv)
    }

    fn invalidate(&self) {
        MemCli::invalidate(self)
    }

    fn bytes(&self, argv: &[&str]) -> Option<Vec<u8>> {
        let run = self.exec(argv);
        run.ok().then_some(run.stdout)
    }
}

/// The exit-code table from the module docs, in one place.
fn classify(run: &Run) -> Outcome {
    let stdout = String::from_utf8_lossy(&run.stdout);
    let trimmed = stdout.trim();

    if run.code.is_none() {
        return Outcome::Broken(run.stderr.clone());
    }
    if trimmed.is_empty() {
        // Never hand empty stdout to a JSON parser. On a zero exit this is a
        // verb with no document (`handoff --set`); on a non-zero one it is
        // the absent/unknown row. Both render the same.
        return Outcome::Absent;
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => Outcome::Json(value),
        Err(e) => Outcome::Broken(format!("mem printed something that is not JSON: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(code: Option<i32>, stdout: &str, stderr: &str) -> Run {
        Run {
            code,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.to_string(),
        }
    }

    #[test]
    fn exit_one_with_a_document_is_an_empty_result_not_an_error() {
        let outcome = classify(&run(Some(1), r#"{"questions":[]}"#, ""));
        assert!(matches!(outcome, Outcome::Json(_)));
        assert!(outcome.rows("questions").is_empty());
        assert!(outcome.broken().is_none());
    }

    #[test]
    fn exit_one_with_nothing_is_absent_and_never_reaches_a_parser() {
        let outcome = classify(&run(Some(1), "", "mem: no project named 'x'"));
        assert!(matches!(outcome, Outcome::Absent));
        assert!(outcome.broken().is_none());
    }

    #[test]
    fn a_missing_binary_is_the_only_other_way_to_be_broken() {
        let outcome = classify(&run(None, "", "could not run mem: No such file"));
        assert!(outcome.broken().is_some());
    }

    #[test]
    fn a_document_that_will_not_parse_is_broken() {
        let outcome = classify(&run(Some(1), "not json at all", ""));
        assert!(outcome.broken().is_some());
    }
}
