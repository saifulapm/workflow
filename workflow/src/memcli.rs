//! Everything the workflow asks mem. Process state lives there and nowhere
//! else, and the registry is never re-implemented here (spec §3).
//!
//! A filtered read that matches nothing prints `{"items":[]}` and exits 1
//! (mem spec §7), so the array is the answer and the exit code is not.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root: Option<String>,
    #[serde(default)]
    pub verify: Option<String>,
    /// Globs this project wants a cold review of, whitespace separated, set
    /// with `mem project set review-paths`. Absent means the global table is
    /// the whole answer (friction #HK2PNTR4).
    #[serde(default)]
    pub review_paths: Option<String>,
}

impl Project {
    /// The project name as a single path component: it names directories under
    /// the run and worktree roots.
    pub fn dir_name(&self) -> String {
        self.name.replace('/', "-")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Item {
    pub path: String,
}

#[derive(Debug, Deserialize)]
struct Items {
    items: Vec<Item>,
}

pub fn bin() -> String {
    match std::env::var("WORKFLOW_MEM") {
        Ok(v) if !v.is_empty() => v,
        _ => "mem".to_string(),
    }
}

static CALLER_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Pin every mem call to the directory the caller stood in. mem resolves a
/// monorepo subdir to its child project by cwd, so a command that chdirs to
/// the repo toplevel before asking mem would always get the root project:
/// the wrong plan slot to read and tick, and the wrong name to record runs
/// under (frictions #GCYJFZT3, #FFSFMBDH).
pub fn resolve_from_here() {
    if let Ok(cwd) = std::env::current_dir() {
        let _ = CALLER_DIR.set(cwd);
    }
}

fn command() -> Command {
    let mut c = Command::new(bin());
    if let Some(dir) = CALLER_DIR.get() {
        c.current_dir(dir);
    }
    c
}

fn capture(args: &[&str]) -> Option<(bool, String)> {
    let out = command().args(args).stderr(Stdio::null()).output().ok()?;
    Some((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
    ))
}

fn silent(args: &[&str]) -> bool {
    command()
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Who owns this checkout. Exit 1 there means unknown, which is a fine answer:
/// the caller decides what to do without an identity.
pub fn project_current() -> Option<Project> {
    let (ok, out) = capture(&["project", "current", "--json"])?;
    if !ok {
        return None;
    }
    let p: Project = serde_json::from_str(&out).ok()?;
    if p.id.is_empty() { None } else { Some(p) }
}

/// A choice this project declared with `mem project set <key>`: the worker
/// backend, the workers' model.
///
/// Read straight out of the document rather than modelled on [`Project`],
/// which is how mem holds it: mem stores the choice and hands it to whoever
/// dispatches the work, and nothing else in the workflow asks. `None` covers
/// both an unregistered checkout and one that never chose.
pub fn project_choice(key: &str) -> Option<String> {
    let (ok, out) = capture(&["project", "current", "--json"])?;
    if !ok {
        return None;
    }
    let doc: serde_json::Value = serde_json::from_str(&out).ok()?;
    let name = doc.get(key)?.as_str()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

pub fn project_backend() -> Option<String> {
    project_choice("backend")
}

pub fn project_model() -> Option<String> {
    project_choice("model")
}

/// A worker's question, as `mem questions --for orchestrator --json` reports
/// it: the orchestrator's to answer, tagged with the task that asked.
#[derive(Debug, Clone, Deserialize)]
pub struct Question {
    pub id: String,
    pub short_id: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub answer: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Questions {
    questions: Vec<Question>,
}

/// Every question the task tagged `<plan>/<task>` has asked the orchestrator,
/// answered or not, newest first. The run reads this twice: to name what a
/// blocked worker is waiting on, and to carry the answer into its next brief.
pub fn questions_for(tag: &str) -> Vec<Question> {
    let Some((_, out)) = capture(&["questions", "--for", "orchestrator", "--json"]) else {
        return Vec::new();
    };
    serde_json::from_str::<Questions>(&out)
        .map(|q| q.questions)
        .unwrap_or_default()
        .into_iter()
        .filter(|q| q.task.as_deref() == Some(tag))
        .collect()
}

pub fn answer(id: &str, text: &str) -> bool {
    silent(&["answer", id, text])
}

/// Does this directory belong to a project mem knows? The hook's half of the
/// fire condition, and no JSON is needed to answer it.
pub fn knows_this_checkout() -> bool {
    silent(&["project", "current"])
}

fn rulings(rtype: &str, since: Option<&str>) -> Vec<Item> {
    let mut args: Vec<String> = vec![
        "log".into(),
        "--kind".into(),
        "ruling".into(),
        "--type".into(),
        rtype.into(),
    ];
    if let Some(s) = since {
        args.push("--since".into());
        args.push(s.into());
    }
    args.push("--limit".into());
    args.push("100".into());
    args.push("--json".into());
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let Some((_, out)) = capture(&refs) else {
        return Vec::new();
    };
    serde_json::from_str::<Items>(&out)
        .map(|i| i.items)
        .unwrap_or_default()
}

pub fn has_ruling(rtype: &str, since: Option<&str>) -> bool {
    !rulings(rtype, since).is_empty()
}

/// The bodies of every ruling of a type, run together. What a ruling *says* is
/// what clears a named term or a named test.
pub fn ruling_bodies(rtype: &str) -> String {
    let mut body = String::new();
    for item in rulings(rtype, None) {
        if let Ok(text) = std::fs::read_to_string(PathBuf::from(&item.path)) {
            body.push_str(&text);
            body.push('\n');
        }
    }
    body
}

pub fn log(text: &str) {
    silent(&["log", text]);
}

pub fn plan_tick(task: &str) -> bool {
    silent(&["plan", "--tick", task])
}

/// This project's plan, as mem holds it.
pub fn plan() -> Option<String> {
    let (_, out) = capture(&["plan"])?;
    if out.trim().is_empty() {
        None
    } else {
        Some(out)
    }
}
