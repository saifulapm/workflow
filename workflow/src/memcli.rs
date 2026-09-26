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

static PROJECT: OnceLock<String> = OnceLock::new();

/// Name the project every mem call acts on, `--project <name>`, instead of
/// the one mem infers from the caller's directory. A worker's cwd is its
/// worktree root, and mem resolves a monorepo child by the path relative to
/// the toplevel: at the root that path is empty, no child owns it, and mem
/// answers with the root project -- its plan, its wiki, its log -- for a
/// task the worktree path already files under the child (m3-advise ruling
/// 1 as amended, #J6YAWMSJ).
pub fn name_project(name: &str) {
    let _ = PROJECT.set(name.to_string());
}

fn command() -> Command {
    let mut c = Command::new(bin());
    if let Some(dir) = CALLER_DIR.get() {
        c.current_dir(dir);
    }
    if let Some(name) = PROJECT.get() {
        c.arg("--project").arg(name);
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

pub fn project_model() -> Option<String> {
    project_choice("model")
}

/// The reasoning dial the project set for its workers, `mem project set
/// effort`; absent is the CLI's own default.
/// The second fix round's model, when the project named one; absent means
/// the reader's own.
pub fn project_fix_model() -> Option<String> {
    project_choice("fix_model")
}

pub fn project_effort() -> Option<String> {
    project_choice("effort")
}

/// The same dial for the reader at the merge gate, `mem project set
/// review-effort`.
pub fn project_review_effort() -> Option<String> {
    project_choice("review_effort")
}

/// What `review-model` is set to by a project that has decided nobody reads
/// its merges. Absent is a different answer -- a project that has not decided
/// -- and a run is refused over that one rather than merged unread.
const NO_READER: &str = "none";

/// The model that reads each task's diff at the merge gate, or nothing:
/// nothing covers both a project that never named one and one that recorded
/// [`NO_READER`], since neither leaves a model to dispatch.
pub fn project_review_model() -> Option<String> {
    project_choice("review_model").filter(|m| !m.eq_ignore_ascii_case(NO_READER))
}

/// Did this project record that nobody reads? The run asks so it can tell the
/// decision apart from the silence, and go ahead unread on the first.
pub fn reader_recorded_none() -> bool {
    project_choice("review_model").is_some_and(|m| m.eq_ignore_ascii_case(NO_READER))
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
/// A mem that could not answer -- a spawn that failed, output that will not
/// parse -- is `None`, never an empty listing: a caller that reads mem's own
/// trouble as "the task has no question" gives up on a question that is
/// really there (friction #1FKDVVD9). The exit code is not the answer here
/// (§7 above): a listing that matches nothing prints its array and exits 1.
pub fn questions_for(tag: &str) -> Option<Vec<Question>> {
    let (_, out) = capture(&["questions", "--for", "orchestrator", "--json"])?;
    Some(
        serde_json::from_str::<Questions>(&out)
            .ok()?
            .questions
            .into_iter()
            .filter(|q| q.task.as_deref() == Some(tag))
            .collect(),
    )
}

pub fn answer(id: &str, text: &str) -> bool {
    silent(&["answer", id, text])
}

/// Does this directory belong to a project mem knows? The hook's half of the
/// fire condition, and no JSON is needed to answer it.
pub fn knows_this_checkout() -> bool {
    silent(&["project", "current"])
}

fn rulings(rtype: Option<&str>, since: Option<&str>) -> Vec<Item> {
    let mut args: Vec<String> = vec!["log".into(), "--kind".into(), "ruling".into()];
    if let Some(rtype) = rtype {
        args.push("--type".into());
        args.push(rtype.into());
    }
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
    !rulings(Some(rtype), since).is_empty()
}

/// The bodies of every ruling of a type, run together. What a ruling *says* is
/// what clears a named term or a named test.
pub fn ruling_bodies(rtype: &str) -> String {
    let mut body = String::new();
    for item in rulings(Some(rtype), None) {
        if let Ok(text) = std::fs::read_to_string(PathBuf::from(&item.path)) {
            body.push_str(&text);
            body.push('\n');
        }
    }
    body
}

/// Every ruling saved within `since` (a mem window: `90m`, `4h`), its own
/// text without the record's fields. The reader at the merge gate is held to
/// the plan's rulings; a ruling the orchestrator made after the plan was
/// written is in none of them, and a reading that never saw one blocked a
/// diff over ground already settled (frictions #WAQQSNBV, #0KT8057H).
pub fn rulings_since(since: &str) -> Vec<String> {
    rulings(None, Some(since))
        .into_iter()
        .filter_map(|item| std::fs::read_to_string(&item.path).ok())
        .map(|text| body_of(&text))
        .filter(|body| !body.is_empty())
        .collect()
}

/// An item's own text, without the `+++` frontmatter mem writes above it: a
/// prompt carries what was written, not the record's fields.
fn body_of(text: &str) -> String {
    match text
        .strip_prefix("+++\n")
        .and_then(|rest| rest.split_once("\n+++\n"))
    {
        Some((_, body)) => body.trim().to_string(),
        None => text.trim().to_string(),
    }
}

/// Every line the run itself writes, tagged so the digest can leave them out
/// of the handful of recent logs it shows and `mem log --type run` can find
/// them apart from what a worker or a person logged (ruling 6 of
/// m1-wiki-first).
pub fn log_run(text: &str) {
    silent(&["log", "--type", "run", "--", text]);
}

/// A finding the reader marked `later`, or one the orchestrator accepted a
/// merge over, kept as work for a later plan rather than lost with the
/// reading: `mem save --type followup`.
pub fn save_followup(text: &str) -> bool {
    silent(&["save", "--type", "followup", "--", text])
}

pub fn plan_tick(task: &str) -> bool {
    silent(&["plan", "--tick", task])
}

/// What `mem roadmap --tick <slug> --json` answers. `ticked` is false for a
/// milestone that was already checked off, which is not a failure and not news.
#[derive(Debug, Deserialize)]
struct RoadmapTick {
    ticked: bool,
}

/// Check a finished plan's slug off in the project's roadmap. `Ok(false)`
/// covers every way there is nothing to say: no roadmap at all, or a box that
/// was already ticked. Any other refusal is `Err` with what mem said, for the
/// run to pass on: a plan the roadmap does not name was silence, and two
/// finished milestones went unticked with nobody told (frictions #7KTQPJQK,
/// #0W95EPF4).
pub fn roadmap_tick(slug: &str) -> Result<bool, String> {
    let out = command()
        .args(["roadmap", "--tick", slug, "--json"])
        .output()
        .map_err(|e| format!("cannot run {}: {e}", bin()))?;
    if out.status.success() {
        return Ok(serde_json::from_slice::<RoadmapTick>(&out.stdout)
            .map(|t| t.ticked)
            .unwrap_or(false));
    }
    // Asked only once the tick failed, so a tick that lands is one process.
    match capture(&["roadmap"]) {
        Some((true, text)) if !text.trim().is_empty() => {}
        _ => return Ok(false),
    }
    let said = String::from_utf8_lossy(&out.stderr);
    let said = said.trim();
    Err(said.strip_prefix("mem: ").unwrap_or(said).to_string())
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

/// One wiki page, verbatim, read fresh off `mem wiki -- <slug>` -- the plan
/// of record is read live the same way, so an edit to a page reaches the
/// next dispatch. `None` covers both a project mem does not know and a
/// project with no such page: the caller lists it as absent rather than
/// refusing the dispatch over it (ruling 2 of m1-wiki-first).
pub fn wiki_page(slug: &str) -> Option<String> {
    let (ok, out) = capture(&["wiki", "--", slug])?;
    ok.then_some(out)
}
