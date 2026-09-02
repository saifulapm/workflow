//! Running `mem` (spec §4, §4a, §9.3).
//!
//! Two rules govern everything here.
//!
//! **argv, never a shell.** The text this module passes to `mem answer` was
//! typed into a form on a phone; with `sh -c` anywhere on the path, that field
//! is arbitrary command execution as Saiful (review B-4c). There is no string
//! interpolation into a command line in this file, and there is no `sh`.
//!
//! **An exit code from mem is not a yes/no.** §4a's table, verified live
//! against the built binary:
//!
//! | call | exit | stdout |
//! |---|---|---|
//! | `questions --pending --all-projects --for human --json`, none pending | 1 | `{"questions":[]}` |
//! | `log --limit N --project X --json`, none | 1 | `{"items":[]}` |
//! | `projects --json`, none | 0 | `{"projects":[]}` |
//! | `status --project <no status.md> --json` | 1 | **empty** |
//! | `status --project no-such-project --json` | 1 | empty; stderr is plain text |
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

use serde_json::Value;

use crate::proc::{self, Ended};

/// The one listing hub shows and rings for: pending, every project, and
/// only what a person is meant to answer.
pub const QUESTIONS_ARGV: [&str; 6] = [
    "questions",
    "--pending",
    "--all-projects",
    "--for",
    "human",
    "--json",
];

/// §4: "a small in-process cache (5 s TTL) so a phone refresh doesn't
/// stampede". The key is the whole argv, so it is per-project by construction
/// (review m-5).
pub const CACHE_TTL: Duration = Duration::from_secs(5);

/// How long one `mem` may take before it is killed (review M-3). §4a's last row
/// exists for a mem that cannot answer; before this there was no way to reach
/// it, because a child that never returns never fails either.
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

/// How `mem log` is asked for a project's recent items.
pub const LOG_LIMIT: usize = 20;

/// What one `mem` call meant, once §4a's table has been applied to it.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// A document. Possibly one with an empty array in it — that is a result,
    /// not a failure.
    Json(Value),
    /// Nothing on stdout and a non-zero exit: absent, or an unknown id. The
    /// page renders `—`.
    Absent,
    /// mem is broken: the binary is missing, or it printed something that is
    /// not JSON. This, and only this, is what the degraded page is for.
    Broken(String),
}

impl Outcome {
    /// The array under a verb's own key — `questions`, `items`, `projects`.
    /// An absent key is an empty list, because "no such key" and "no rows" are
    /// the same answer to the page.
    pub fn rows(&self, key: &str) -> Vec<Value> {
        match self {
            Outcome::Json(value) => value
                .get(key)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    pub fn broken(&self) -> Option<&str> {
        match self {
            Outcome::Broken(why) => Some(why),
            _ => None,
        }
    }
}

/// One raw run of the binary.
#[derive(Debug, Clone)]
pub struct Run {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    /// Plain text under `--json` too (§4a), so it is never parsed — only shown.
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
    /// How long one child may take (review M-3).
    timeout: Duration,
    /// When mem may be tried again, after a child had to be killed.
    ///
    /// A timeout alone is not enough. One page render is `2P+2+Q` serialised
    /// calls, so a store that has stalled — a bisync window, a wedged mount, a
    /// machine resuming — would cost five seconds *each*, the render would
    /// outlive the connection it is answering, and sixteen of those are M-2
    /// again by another road. One kill therefore puts mem out of bounds for the
    /// cache TTL: the calls behind it fail at once and the page renders
    /// degraded, which is what §4a's last row is for.
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

    pub fn with_timeout(mut self, timeout: Duration) -> MemCli {
        self.timeout = timeout;
        self
    }

    /// Runs `mem` with exactly these arguments. No shell, no interpolation,
    /// never at the same time as another one (see `gate`), and never for longer
    /// than `timeout` (see `unavailable_until`).
    pub fn exec(&self, args: &[&str]) -> Run {
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
        match proc::output_within(&mut command, self.timeout) {
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
    /// that mem accepts and the waiter never sees (review M-2).
    pub fn invalidate(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }

    /// `--for human`: a worker's question is the orchestrator's to answer
    /// and never a phone's to see. Before the audience existed, every
    /// "may I widen my Files line" reached the hub and rang the doorbell.
    pub fn questions(&self) -> Arc<Outcome> {
        self.read(&QUESTIONS_ARGV)
    }

    /// The same call, straight from the binary (see `refresh`).
    pub fn questions_fresh(&self) -> Arc<Outcome> {
        self.refresh(&QUESTIONS_ARGV)
    }

    pub fn projects(&self) -> Arc<Outcome> {
        self.read(&["projects", "--json"])
    }

    /// §4b: one call per project. There is no `--all-projects` on `log`, and
    /// `--scope all` is project ∪ global — from `%h`, which is not a checkout,
    /// it resolves to nothing at all.
    pub fn log(&self, project: &str) -> Arc<Outcome> {
        let limit = LOG_LIMIT.to_string();
        self.read(&[
            "log",
            "--limit",
            &limit,
            // `--flag=value`, so a project named `-weird` is a value and not a
            // flag. Verified against the real binary.
            &format!("--project={project}"),
            "--json",
        ])
    }

    pub fn status(&self, project: &str) -> Arc<Outcome> {
        self.read(&["status", &format!("--project={project}"), "--json"])
    }

    /// One project's pages: slug, title, bytes and date. A project with none
    /// still prints `{"pages":[]}`, so this reads like the other list verbs.
    pub fn wiki(&self, project: &str) -> Arc<Outcome> {
        self.read(&["wiki", &format!("--project={project}"), "--json"])
    }

    /// One page's text. `--` last, so a slug is a value and never a flag —
    /// belt and braces, since `app` refuses anything that is not a slug first.
    pub fn wiki_page(&self, project: &str, slug: &str) -> Arc<Outcome> {
        self.read(&[
            "wiki",
            &format!("--project={project}"),
            "--json",
            "--",
            slug,
        ])
    }

    /// The full text of one item. `mem questions` reports only the first line
    /// of a question as `title`, and workflow's asks are batched and
    /// multi-line, so the answer field would otherwise sit under a heading with
    /// the questions missing.
    pub fn show(&self, id: &str) -> Arc<Outcome> {
        self.read(&["show", "--json", "--", id])
    }

    /// The one write hub makes. `--` first, so an answer beginning with a dash
    /// is text rather than a flag mem rejects with exit 2 (review m-9).
    pub fn answer(&self, id: &str, text: &str) -> Run {
        self.exec(&["answer", "--", id, text])
    }
}

/// §4a, in one place.
fn classify(run: &Run) -> Outcome {
    let stdout = String::from_utf8_lossy(&run.stdout);
    let trimmed = stdout.trim();

    if run.code.is_none() {
        return Outcome::Broken(run.stderr.clone());
    }
    if trimmed.is_empty() {
        // Never hand empty stdout to a JSON parser. On a zero exit this is a
        // verb with no document (`status --set`); on a non-zero one it is
        // §4a's absent/unknown row. Both render the same.
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
