//! What the pages read from mem, and the writes they make, as mem argv.
//!
//! The pages never run mem themselves. A `Source` answers each argv: the
//! local hub by running the binary, a Worker from its own store. Every argv
//! here is the one the hub has always run, so a `Source` can answer by argv.

use std::sync::Arc;

use serde_json::Value;

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

/// What one `mem` call meant, once mem's exit-code table has been applied
/// to it.
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

/// What a write printed, and whether mem took it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Where the pages' mem comes from.
pub trait Source {
    /// What `mem <argv>` prints, classified exactly as MemCli classifies it today.
    fn read(&self, argv: &[&str]) -> Arc<Outcome>;
    /// A write a page makes, as the mem argv the hub runs today (`input` is stdin for a --stdin write).
    fn write(&self, argv: &[&str], input: Option<&[u8]>) -> Written;
    /// `read`, past any cache: what mem says now. A write that checks before
    /// it writes reads this way.
    fn refresh(&self, argv: &[&str]) -> Arc<Outcome> {
        self.read(argv)
    }
    /// Forgets every cached read, so the next one is mem's word as it is now.
    fn invalidate(&self) {}
    /// The raw bytes `mem <argv>` prints, or `None` when it fails: an
    /// evidence file, which is no JSON document.
    fn bytes(&self, argv: &[&str]) -> Option<Vec<u8>> {
        let _ = argv;
        None
    }
}

/// The reads the pages make, each one argv.
pub trait Mem: Source {
    /// `--for human`: a worker's question is the orchestrator's to answer
    /// and never a phone's to see. Before the audience existed, every
    /// "may I widen my Files line" reached the hub and rang the doorbell.
    fn questions(&self) -> Arc<Outcome> {
        self.read(&QUESTIONS_ARGV)
    }

    /// The same call, past the cache.
    fn questions_fresh(&self) -> Arc<Outcome> {
        self.refresh(&QUESTIONS_ARGV)
    }

    fn projects(&self) -> Arc<Outcome> {
        self.read(&["projects", "--json"])
    }

    /// One project's pages: slug, title, bytes and date. A project with none
    /// still prints `{"pages":[]}`, so this reads like the other list verbs.
    fn wiki(&self, project: &str) -> Arc<Outcome> {
        self.read(&["wiki", &format!("--project={project}"), "--json"])
    }

    /// One page's text. `--` last, so a slug is a value and never a flag —
    /// belt and braces, since the route refuses anything that is not a slug
    /// first.
    fn wiki_page(&self, project: &str, slug: &str) -> Arc<Outcome> {
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
    fn show(&self, id: &str) -> Arc<Outcome> {
        self.read(&["show", "--json", "--", id])
    }

    /// The project's roadmap, byte for byte, milestone ticks and all.
    fn roadmap(&self, project: &str) -> Arc<Outcome> {
        self.read(&["roadmap", &format!("--project={project}"), "--json"])
    }

    /// The current plan, byte for byte, task ticks and all. No slug: the
    /// file at `plan.md`, not one filed under `plans/`.
    fn plan(&self, project: &str) -> Arc<Outcome> {
        self.read(&["plan", &format!("--project={project}"), "--json"])
    }

    /// The stored plans beside the current plan: slug, bytes, date — the
    /// same list shape `wiki` already has.
    fn plan_list(&self, project: &str) -> Arc<Outcome> {
        self.read(&["plan", "--list", &format!("--project={project}"), "--json"])
    }

    /// One stored plan's text, by slug. `--` last, exactly as `wiki_page`: a
    /// slug is a value and never a flag, belt and braces over the route's
    /// own check.
    fn plan_slug(&self, project: &str, slug: &str) -> Arc<Outcome> {
        self.read(&[
            "plan",
            &format!("--project={project}"),
            "--json",
            "--",
            slug,
        ])
    }

    /// The latest handoff, body and all.
    fn handoff(&self, project: &str) -> Arc<Outcome> {
        self.read(&["handoff", &format!("--project={project}"), "--json"])
    }

    /// Up to `limit` recent log lines. There is no `--all-projects` on `log`,
    /// so it is one call per project.
    fn log_n(&self, project: &str, limit: usize) -> Arc<Outcome> {
        let limit = limit.to_string();
        self.read(&[
            "log",
            "--limit",
            &limit,
            &format!("--project={project}"),
            "--json",
        ])
    }

    /// Up to `limit` items of one kind — fact, ruling, handoff, question or
    /// log. `--kind=`, merged like `--project=`, so a kind is a value and
    /// never a flag.
    fn items(&self, project: &str, kind: &str, limit: usize) -> Arc<Outcome> {
        let limit = limit.to_string();
        self.read(&[
            "log",
            &format!("--kind={kind}"),
            "--limit",
            &limit,
            &format!("--project={project}"),
            "--json",
        ])
    }

    /// Every question this project has, pending and answered, most recent
    /// first — the page sorts pending to the top and trims to the last 10
    /// answered.
    fn questions_all(&self, project: &str) -> Arc<Outcome> {
        self.read(&["questions", &format!("--project={project}"), "--json"])
    }
}

impl<S: Source + ?Sized> Mem for S {}

/// The answer write. `--` first, so an answer beginning with a dash is text
/// rather than a flag mem rejects with exit 2.
pub fn answer_argv<'a>(
    id: &'a str,
    text: &'a str,
    by: &'a str,
    supersedes: Option<&'a str>,
) -> Vec<&'a str> {
    let mut args = vec!["answer", "--json", "--by", by];
    if let Some(earlier) = supersedes {
        args.extend(["--supersedes", earlier]);
    }
    args.extend(["--", id, text]);
    args
}
