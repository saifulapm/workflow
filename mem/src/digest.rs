//! `mem context` — the two-layer disclosure digest (spec §8).
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
use crate::store::Store;
use crate::timefmt::date;

/// Assembly stops adding optional content here.
pub const TARGET: usize = 6_000;
/// Mandatory content alone over this earns a note on stderr.
pub const WARN: usize = 10_000;
/// Hard truncation, at an item boundary.
pub const CEILING: usize = 24_000;
/// `--brief` is a hook payload, not a digest.
pub const BRIEF: usize = 480;

pub const TRUNCATED: &str = "[digest truncated]";
pub const HINT: &str = "detail: mem show <id> · search: mem search \"<q>\"";
pub const EMPTY: &str = "nothing recorded for this project yet";

pub struct Digest {
    pub text: String,
    /// Mandatory content alone exceeded WARN.
    pub over_warn: bool,
    pub truncated: bool,
}

/// Everything the digest draws on, gathered once.
pub struct Sources {
    /// The store is a format this binary does not know (spec §3). Writes refuse;
    /// reads serve and say so, because a synced VERSION bump must never take
    /// another machine's sessions down and must never be silent either.
    pub version: Option<String>,
    pub staleness: Option<String>,
    pub handoff: Option<Row>,
    pub plan: Option<String>,
    pub questions: Vec<Row>,
    pub status: Option<String>,
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
        let status = project_id
            .map(|id| store.status_path(id))
            .and_then(|p| std::fs::read_to_string(p).ok());
        Ok(Sources {
            version: crate::maint::read_version_warning(store),
            staleness,
            handoff: index.recent("handoff", project_id, 1)?.into_iter().next(),
            plan,
            questions: index.pending_questions(project_id)?,
            status,
            rulings: index.recent("ruling", project_id, 5)?,
            facts: index.recent("fact", project_id, 40)?,
            logs: index.recent("log", project_id, 5)?,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.handoff.is_none()
            && self.plan.is_none()
            && self.questions.is_empty()
            && self.status.is_none()
            && self.rulings.is_empty()
            && self.facts.is_empty()
            && self.logs.is_empty()
    }
}

/// The first heading line and the first unchecked task of a plan.
pub fn plan_head(plan: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(heading) = plan.lines().find(|l| l.trim_start().starts_with('#')) {
        out.push(heading.trim().to_string());
    }
    if let Some(task) = plan.lines().find(|l| {
        let t = l.trim_start();
        t.starts_with("- [ ]") || t.starts_with("* [ ]")
    }) {
        out.push(task.trim().to_string());
    }
    out
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
    if let Some(plan) = &sources.plan {
        for line in plan_head(plan) {
            mandatory.push(format!("plan: {line}"));
        }
    }
    for q in &sources.questions {
        mandatory.push(format!(
            "? #{}  {}",
            q.short_id,
            truncate_bytes(&q.title, 70)
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

    // Optional sections, in fill order, each dropped whole when there is no
    // room: status body bottom-up first, then rulings, facts, logs.
    let mut optional: Vec<Vec<String>> = Vec::new();
    if let Some(status) = &sources.status {
        optional.push(
            status
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| format!("status: {}", l.trim()))
                .collect(),
        );
    }
    optional.push(
        sources
            .rulings
            .iter()
            .map(|r| format!("ruling #{}  {}", r.short_id, truncate_bytes(&r.title, 66)))
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
        // The status body is the one section dropped line by line from the
        // bottom; the others are filled while there is room.
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

/// `--brief`: the smallest useful thing a hook can inject (spec §9), counted on
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
