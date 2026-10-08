//! `mem context` — the two-layer disclosure digest.
//!
//! The assembly order is fixed so two implementations produce the same digest:
//! mandatory sections first and never truncated, optional sections filled while
//! the running total is under target, and a hard ceiling that cuts at an item
//! boundary and says so.

use anyhow::Result;
use jiff::Timestamp;

use crate::index::{Index, Row};
use crate::item::Item;
use crate::search::truncate_bytes;
use crate::store::{Page, Store};
use crate::timefmt::date;

/// Assembly stops adding optional content here.
pub const TARGET: usize = 6_000;
/// Mandatory content alone over this earns a note on stderr.
pub const WARN: usize = 10_000;
/// Hard truncation, at an item boundary.
pub const CEILING: usize = 24_000;
/// `--brief` is a hook payload, not a digest.
pub const BRIEF: usize = 480;
/// What the small digest is shaped to stay under on a populated project.
pub const SMALL_TARGET: usize = 2_000;
/// What it cannot exceed: every line is cut at SMALL_LINE and the line count is
/// fixed by the shape, so the bound holds without a truncation pass.
pub const SMALL_CEILING: usize = 3_000;
/// The longest line the small digest prints.
pub const SMALL_LINE: usize = 120;
/// How many entries of each list the small digest names.
const SMALL_QUESTIONS: usize = 3;
const SMALL_RULINGS: usize = 3;

/// The page that lists the others. It is a page like any other, kept by hand.
pub const INDEX_SLUG: &str = "index";
/// How much of the index page the digest carries when there is room for it.
pub const INDEX_HEAD_LINES: usize = 5;

pub const TRUNCATED: &str = "[digest truncated]";
pub const HINT: &str = "detail: mem show <id> · search: mem search \"<q>\"";
pub const SMALL_HINT: &str =
    "detail: mem show <id> · search: mem search \"<q>\" · everything: mem context --full";
pub const EMPTY: &str = "nothing recorded for this project yet";

pub struct Digest {
    pub text: String,
    /// Mandatory content alone exceeded WARN.
    pub over_warn: bool,
    pub truncated: bool,
}

/// Everything the digest draws on, gathered once.
pub struct Sources {
    /// The store is a format this binary does not know. Writes refuse;
    /// reads serve and say so, because a synced VERSION bump must never take
    /// another machine's sessions down and must never be silent either.
    pub version: Option<String>,
    pub staleness: Option<String>,
    /// The project's name, for the small digest's first line.
    pub project: Option<String>,
    pub handoff: Option<Row>,
    pub plan: Option<String>,
    /// The milestones above the current plan, when the project is planned
    /// whole rather than one plan at a time.
    pub roadmap: Option<String>,
    pub questions: Vec<Row>,
    /// The project's wiki pages. Only the count reaches the digest: a listing
    /// belongs to `mem wiki`.
    pub pages: Vec<Page>,
    /// The text of the index page, when the project keeps one.
    pub wiki_index: Option<String>,
    /// Saiful's standing rules: rulings saved outside any project, which
    /// hold in every project.
    pub rules: Vec<Row>,
    pub rulings: Vec<Row>,
    pub facts: Vec<Row>,
    pub logs: Vec<Row>,
}

impl Sources {
    pub fn gather(
        index: &Index,
        store: &Store,
        project_id: Option<&str>,
        staleness: Option<String>,
    ) -> Result<Sources> {
        let plan = project_id
            .map(|id| store.plan_path(id))
            .and_then(|p| std::fs::read_to_string(p).ok());
        let roadmap = project_id
            .map(|id| store.roadmap_path(id))
            .and_then(|p| std::fs::read_to_string(p).ok());
        let declared =
            |key: &str| project_id.and_then(|id| crate::project::declared(store, id, key));
        let pages = project_id
            .map(|id| store.wiki_pages(id))
            .unwrap_or_default();
        let wiki_index = pages
            .iter()
            .find(|p| p.slug == INDEX_SLUG)
            .and_then(|p| std::fs::read_to_string(&p.path).ok());
        Ok(Sources {
            version: crate::maint::read_version_warning(store),
            staleness,
            project: declared("name"),
            handoff: index.recent("handoff", project_id, 1)?.into_iter().next(),
            plan,
            roadmap,
            questions: index.pending_questions(project_id)?,
            pages,
            wiki_index,
            // Outside a project the global rulings are the rulings already.
            rules: match project_id {
                Some(_) => index.recent("ruling", None, 5)?,
                None => Vec::new(),
            },
            rulings: index.recent("ruling", project_id, 5)?,
            facts: index.recent("fact", project_id, 40)?,
            logs: recent_non_run_logs(index, project_id, 5)?,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.handoff.is_none()
            && self.plan.is_none()
            && self.roadmap.is_none()
            && self.questions.is_empty()
            && self.pages.is_empty()
            && self.rules.is_empty()
            && self.rulings.is_empty()
            && self.facts.is_empty()
            && self.logs.is_empty()
    }
}

/// The most recent logs that are not the run's own bookkeeping: a run's
/// dispatch and merge lines would otherwise crowd out the handful of worker
/// and person logs the digest has room for. A run can
/// write long unbroken bursts of run-typed lines, so a fixed-size page can
/// come back empty after filtering; fetch growing pages until `limit` logs
/// survive the filter or the project's log history is exhausted.
fn recent_non_run_logs(index: &Index, project_id: Option<&str>, limit: usize) -> Result<Vec<Row>> {
    let limit = limit.max(1);
    let mut fetch = limit * 4;
    loop {
        let rows = index.recent("log", project_id, fetch)?;
        let exhausted = rows.len() < fetch;
        let mut kept: Vec<Row> = rows
            .into_iter()
            .filter(|r| r.r#type.as_deref() != Some("run"))
            .collect();
        if kept.len() >= limit || exhausted {
            kept.truncate(limit);
            return Ok(kept);
        }
        fetch *= 2;
    }
}

/// The first heading line and the first unchecked task of a plan.
pub fn plan_head(plan: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(heading) = plan.lines().find(|l| l.trim_start().starts_with('#')) {
        out.push(heading.trim().to_string());
    }
    if let Some(task) = first_open_task(plan) {
        out.push(task.to_string());
    }
    out
}

/// The first unchecked task line of a plan, trimmed. An open box is what says
/// the plan still has work in it.
///
/// A box inside a fenced block, or indented four spaces or more, is an example
/// of a task rather than one: plans that quote their own grammar are full of
/// them, and counting them holds a finished plan open forever.
pub fn first_open_task(plan: &str) -> Option<&str> {
    let mut fenced = false;
    for line in plan.lines() {
        let body = line.trim_start();
        if body.starts_with("```") || body.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced || line.starts_with("    ") {
            continue;
        }
        let line = line.trim();
        if line.starts_with("- [ ]") || line.starts_with("* [ ]") {
            return Some(line);
        }
    }
    None
}

/// The opening lines of the index page, which by convention are a heading and
/// one line per page. The rest is a page, and pages are read with `mem wiki`.
pub fn index_head(index: &str) -> Vec<String> {
    index
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(INDEX_HEAD_LINES)
        .map(|line| truncate_bytes(line, 100))
        .collect()
}

fn item_line(row: &Row, store: &Store) -> String {
    let sentence = first_sentence(row, store);
    let head = format!("#{}  {}", row.short_id, row.title);
    if sentence.is_empty() {
        truncate_bytes(&head, 80)
    } else {
        truncate_bytes(&format!("{head} — {sentence}"), 120)
    }
}

fn first_sentence(row: &Row, _store: &Store) -> String {
    let Ok(bytes) = std::fs::read(&row.path) else {
        return String::new();
    };
    let Ok(item) = Item::parse(&bytes) else {
        return String::new();
    };
    let body = item.body_str().to_string();
    let text = body.trim();
    let end = text
        .find(". ")
        .map(|i| i + 1)
        .or_else(|| text.find('\n'))
        .unwrap_or(text.len());
    text[..end].trim().to_string()
}

/// Builds the digest. `budget` overrides the target, never the ceiling.
pub fn build(sources: &Sources, store: &Store, budget: usize) -> Digest {
    let mut mandatory: Vec<String> = Vec::new();
    if let Some(line) = &sources.version {
        mandatory.push(line.clone());
    }
    if let Some(line) = &sources.staleness {
        mandatory.push(line.clone());
    }
    if let Some(handoff) = &sources.handoff {
        mandatory.push(format!(
            "handoff ({}): {}",
            date(handoff.modified_epoch),
            handoff.title
        ));
        let body = first_sentence(handoff, store);
        if !body.is_empty() {
            mandatory.push(format!("  {body}"));
        }
    }
    // The roadmap sits above the current plan, and reads that way: which
    // milestone is next, then what the plan in hand is doing about it.
    if let Some(roadmap) = &sources.roadmap {
        for line in plan_head(roadmap) {
            mandatory.push(format!("roadmap: {line}"));
        }
    }
    if let Some(plan) = &sources.plan {
        for line in plan_head(plan) {
            mandatory.push(format!("plan: {line}"));
        }
    }
    for q in &sources.questions {
        // An orchestrator's question says whose it is.
        let who = match &q.audience {
            Some(a) => format!("[{a}] "),
            None => String::new(),
        };
        mandatory.push(format!(
            "? #{}  {who}{}",
            q.short_id,
            truncate_bytes(&q.title, 70)
        ));
    }
    // A session that does not know the wiki is there will not go looking for
    // it, so the count is mandatory: one line, whatever else the digest holds.
    if !sources.pages.is_empty() {
        let n = sources.pages.len();
        mandatory.push(format!(
            "wiki: {n} page{} — mem wiki",
            if n == 1 { "" } else { "s" }
        ));
    }
    if sources.is_empty() {
        // Nothing recorded yet still means the warning lines: they are the only
        // mandatory content an empty project can have, and a machine reading a
        // store it cannot write should hear about it on its very first read.
        let mut text = String::new();
        for line in &mandatory {
            text.push_str(line);
            text.push('\n');
        }
        text.push_str(EMPTY);
        text.push('\n');
        text.push_str(HINT);
        text.push('\n');
        return Digest {
            text,
            over_warn: false,
            truncated: false,
        };
    }

    let mut lines = mandatory.clone();
    let mandatory_bytes: usize = lines.iter().map(|l| l.len() + 1).sum();

    // Optional sections, in fill order, each filled while there is room: the
    // index head under the wiki line it belongs to, then rulings, facts, logs.
    let mut optional: Vec<Vec<String>> = Vec::new();
    if let Some(index) = &sources.wiki_index {
        optional.push(
            index_head(index)
                .into_iter()
                .map(|line| format!("  {line}"))
                .collect(),
        );
    }
    let ruling_line =
        |word: &str, r: &Row| format!("{word} #{}  {}", r.short_id, truncate_bytes(&r.title, 66));
    optional.push(
        (sources.rules.iter().map(|r| ruling_line("rule", r)))
            .chain(sources.rulings.iter().map(|r| ruling_line("ruling", r)))
            .collect(),
    );
    optional.push(sources.facts.iter().map(|f| item_line(f, store)).collect());
    optional.push(
        sources
            .logs
            .iter()
            .map(|l| format!("log #{}  {}", l.short_id, truncate_bytes(&l.title, 70)))
            .collect(),
    );

    let mut used = mandatory_bytes;
    for section in optional {
        for line in section {
            let cost = line.len() + 1;
            if used + cost > budget {
                break;
            }
            used += cost;
            lines.push(line);
        }
    }
    lines.push(HINT.to_string());

    let mut text = String::new();
    let mut truncated = false;
    for line in &lines {
        if text.len() + line.len() + 1 + TRUNCATED.len() + 1 > CEILING {
            truncated = true;
            break;
        }
        text.push_str(line);
        text.push('\n');
    }
    if truncated {
        text.push_str(TRUNCATED);
        text.push('\n');
    }
    Digest {
        text,
        over_warn: mandatory_bytes > WARN,
        truncated,
    }
}

/// The small digest: the session-start text, one line per fact a session acts
/// on, in a fixed order. What it leaves out (facts, logs, the plan's next task)
/// is a `mem show`, a `mem search` or the full digest away, and the hint line
/// says so.
pub fn build_small(sources: &Sources, _store: &Store) -> Digest {
    let mut lines: Vec<String> = Vec::new();
    lines.extend(sources.version.iter().cloned());
    lines.extend(sources.staleness.iter().cloned());
    if let Some(name) = &sources.project {
        lines.push(format!("project: {name}"));
    }
    lines.extend(position(sources));
    // An orchestrator's question is not the reader's; only the ones with no
    // audience are waiting on the person reading this.
    let for_you: Vec<&Row> = sources
        .questions
        .iter()
        .filter(|q| q.audience.is_none())
        .collect();
    if !for_you.is_empty() {
        lines.push(format!("questions: {} for you", for_you.len()));
        for q in for_you.iter().take(SMALL_QUESTIONS) {
            lines.push(format!("  ? #{}  {}", q.short_id, q.title));
        }
    }
    if let Some(handoff) = &sources.handoff {
        lines.push(format!(
            "handoff ({}): {}",
            date(handoff.modified_epoch),
            handoff.title
        ));
    }
    for r in &sources.rules {
        lines.push(format!("rule #{}  {}", r.short_id, r.title));
    }
    for r in sources.rulings.iter().take(SMALL_RULINGS) {
        lines.push(format!("ruling #{}  {}", r.short_id, r.title));
    }
    if !sources.pages.is_empty() {
        let n = sources.pages.len();
        lines.push(format!("wiki: {n} page{}", if n == 1 { "" } else { "s" }));
        if let Some(index) = &sources.wiki_index {
            lines.extend(index_entries(index));
        }
    }
    if sources.is_empty() {
        lines.push(EMPTY.to_string());
    }
    lines.push(SMALL_HINT.to_string());

    let mut text = String::new();
    for line in &lines {
        text.push_str(&truncate_bytes(line, SMALL_LINE));
        text.push('\n');
    }
    Digest {
        text,
        over_warn: false,
        truncated: false,
    }
}

/// Where the project stands: the first open milestone of the roadmap and its
/// place in it, else the plan by name, with the plan's tasks merged so far.
fn position(sources: &Sources) -> Option<String> {
    let tasks = sources
        .plan
        .as_deref()
        .map(|plan| {
            let boxes = top_level_boxes(plan);
            let ticked = boxes.iter().filter(|(done, _)| *done).count();
            format!(" · tasks {ticked}/{} merged", boxes.len())
        })
        .unwrap_or_default();
    if let Some(roadmap) = &sources.roadmap {
        let milestones = top_level_boxes(roadmap);
        if let Some(n) = milestones.iter().position(|(done, _)| !done) {
            let name = milestones[n]
                .1
                .split_whitespace()
                .next()
                .unwrap_or_default();
            return Some(format!(
                "roadmap: {name} ({} of {}){tasks}",
                n + 1,
                milestones.len()
            ));
        }
    }
    let plan = sources.plan.as_deref()?;
    let heading = plan
        .lines()
        .find(|l| l.trim_start().starts_with('#'))
        .unwrap_or_default();
    let title = heading.trim().trim_start_matches('#').trim();
    let slug = title.strip_prefix("plan:").unwrap_or(title).trim();
    Some(format!("plan: {slug}{tasks}"))
}

/// The boxes at the left margin of a plan or roadmap, ticked or not, with the
/// text after each. An indented box belongs to the task above it and a fenced
/// one is an example, as `first_open_task` reads them.
fn top_level_boxes(text: &str) -> Vec<(bool, &str)> {
    let mut fenced = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let body = line.trim_start();
        if body.starts_with("```") || body.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let Some(rest) = line
            .strip_prefix("- [")
            .or_else(|| line.strip_prefix("* ["))
        else {
            continue;
        };
        let done = match rest.get(..2) {
            Some(" ]") => false,
            Some("x]" | "X]") => true,
            _ => continue,
        };
        out.push((done, rest[2..].trim()));
    }
    out
}

/// The index page's first entries as `  <slug>: <what it says after the dash>`.
fn index_entries(index: &str) -> Vec<String> {
    index
        .lines()
        .map(str::trim)
        .filter_map(|line| {
            let rest = line.strip_prefix("- [")?;
            let (slug, rest) = rest.split_once(']')?;
            Some(match rest.split_once(" — ") {
                Some((_, text)) => format!("  {slug}: {}", text.trim()),
                None => format!("  {slug}"),
            })
        })
        .take(INDEX_HEAD_LINES)
        .collect()
}

/// `--brief`: the smallest useful thing a hook can inject, counted on
/// the content string alone.
pub fn brief(sources: &Sources, now: Timestamp) -> String {
    let _ = now;
    let mut parts: Vec<String> = Vec::new();
    if let Some(line) = &sources.version {
        parts.push(line.clone());
    }
    if let Some(line) = &sources.staleness {
        parts.push(line.clone());
    }
    if let Some(handoff) = &sources.handoff {
        parts.push(format!("handoff: {}", handoff.title));
    }
    if let Some(plan) = &sources.plan
        && let Some(task) = plan_head(plan).into_iter().nth(1)
    {
        parts.push(format!("next: {task}"));
    }
    if !sources.questions.is_empty() {
        parts.push(format!(
            "{} question(s) waiting: #{}",
            sources.questions.len(),
            sources.questions[0].short_id
        ));
    }
    let text = parts.join("\n");
    if text.len() <= BRIEF {
        text
    } else {
        truncate_bytes(&text, BRIEF)
    }
}
