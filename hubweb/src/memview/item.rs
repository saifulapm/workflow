//! An item file as mem reads it: TOML frontmatter between `+++` fences, then
//! the body, and the row mem's index would make of it.

use jiff::Timestamp;
use serde_json::{Value, json};
use toml::Table;

/// The kinds mem knows. A file of any other kind fails mem's parse, and mem
/// skips it as unreadable.
const KINDS: [&str; 11] = [
    "fact", "ruling", "log", "handoff", "question", "answer", "evidence", "finding", "brief",
    "idea", "raw",
];

/// The frontmatter keys memview reads, under mem's names.
#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub id: String,
    pub kind: String,
    pub r#type: Option<String>,
    pub title: String,
    pub tags: Vec<String>,
    pub project: Option<String>,
    pub machine: String,
    pub created: i64,
    pub modified: i64,
    pub supersedes: Option<String>,
    pub answers: Option<String>,
    pub archived: bool,
    pub options: Option<Vec<String>>,
    pub recommend: Option<String>,
    pub audience: Option<String>,
    pub session: Option<String>,
    pub task: Option<String>,
    pub by: Option<String>,
    pub replaces: Option<String>,
    pub file: Option<String>,
    pub milestone: Option<String>,
    pub step: Option<String>,
    pub status: Option<String>,
    pub fixed_by: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub meta: Meta,
    pub body: String,
}

/// The frontmatter text and the body, split on the first complete fence
/// pair; a fence is a line that is `+++` once its LF or CRLF is stripped.
fn split(text: &str) -> Option<(&str, &str)> {
    let is_fence = |line: &str| line.trim_end_matches('\n').trim_end_matches('\r') == "+++";
    let mut lines = text.split_inclusive('\n');
    let first = lines.next()?;
    if !is_fence(first) {
        return None;
    }
    let start = first.len();
    let mut pos = start;
    for line in lines {
        if is_fence(line) {
            return Some((&text[start..pos], &text[pos + line.len()..]));
        }
        pos += line.len();
    }
    None
}

/// The frontmatter table, or None when the file is not an item.
pub fn table(text: &str) -> Option<Table> {
    toml::from_str(split(text)?.0).ok()
}

/// A key that must be a string when present; Err when it is something else,
/// which fails mem's parse of the whole file.
fn opt_str(table: &Table, key: &str) -> Result<Option<String>, ()> {
    match table.get(key) {
        None => Ok(None),
        Some(toml::Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(()),
    }
}

fn opt_strings(table: &Table, key: &str) -> Result<Option<Vec<String>>, ()> {
    match table.get(key) {
        None => Ok(None),
        Some(toml::Value::Array(values)) => values
            .iter()
            .map(|v| v.as_str().map(str::to_string).ok_or(()))
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(()),
    }
}

fn timestamp(table: &Table, key: &str) -> Result<i64, ()> {
    let text = opt_str(table, key)?.ok_or(())?;
    let ts: Timestamp = text.parse().map_err(|_| ())?;
    Ok(ts.as_second())
}

impl Item {
    /// The item, or None where mem's parse fails and it skips the file.
    pub fn parse(text: &str) -> Option<Item> {
        let (front, body) = split(text)?;
        let table: Table = toml::from_str(front).ok()?;
        let kind = opt_str(&table, "kind").ok()??;
        if !KINDS.contains(&kind.as_str()) {
            return None;
        }
        let archived = match table.get("archived") {
            None => false,
            Some(toml::Value::Boolean(b)) => *b,
            Some(_) => return None,
        };
        let s = |key| opt_str(&table, key);
        let meta = (|| {
            Ok::<Meta, ()>(Meta {
                id: s("id")?.ok_or(())?,
                kind: kind.clone(),
                r#type: s("type")?,
                title: s("title")?.ok_or(())?,
                tags: opt_strings(&table, "tags")?.unwrap_or_default(),
                project: s("project")?,
                machine: s("machine")?.ok_or(())?,
                created: timestamp(&table, "created")?,
                modified: timestamp(&table, "modified")?,
                supersedes: s("supersedes")?,
                answers: s("answers")?,
                archived,
                options: opt_strings(&table, "options")?,
                recommend: s("recommend")?,
                audience: s("audience")?,
                session: s("session")?,
                task: s("task")?,
                by: s("by")?,
                replaces: s("replaces")?,
                file: s("file")?,
                milestone: s("milestone")?,
                step: s("step")?,
                status: s("status")?,
                fixed_by: s("fixed_by")?,
            })
        })()
        .ok()?;
        Some(Item {
            meta,
            body: body.to_string(),
        })
    }
}

/// One item as mem's index holds it.
#[derive(Debug, Clone)]
pub struct Row {
    pub item: Item,
    /// Store-relative: `projects/<id>/items/<ulid>.md`.
    pub path: String,
    pub project: Option<String>,
    pub active: bool,
    pub superseded_by: Option<String>,
}

impl Row {
    pub fn meta(&self) -> &Meta {
        &self.item.meta
    }

    pub fn short_id(&self) -> String {
        short_id(&self.meta().id)
    }
}

/// The last 8 characters of a ULID, or the whole of anything shorter.
pub fn short_id(id: &str) -> String {
    match id.len() {
        n if n <= 8 => id.to_string(),
        n => id[n - 8..].to_string(),
    }
}

/// `2026-08-12`, in UTC.
pub fn date(epoch: i64) -> String {
    match Timestamp::from_second(epoch) {
        Ok(ts) => ts.to_string().chars().take(10).collect(),
        Err(_) => "----------".to_string(),
    }
}

/// `<ULID>.md` and nothing else, as mem's scan keeps them.
fn is_item_file(name: &str) -> bool {
    name.strip_suffix(".md").is_some_and(is_ulid)
}

pub fn is_ulid(s: &str) -> bool {
    s.len() == 26 && s.chars().all(is_ulid_char)
}

pub fn is_ulid_char(c: char) -> bool {
    matches!(c, '0'..='9' | 'A'..='H' | 'J' | 'K' | 'M' | 'N' | 'P'..='T' | 'V'..='Z')
}

/// The rows of one project's item files. `name` is the registry's name for
/// the project, which a row takes when its file names none. `active` and
/// `superseded_by` derive from the supersede edges among these rows, the
/// newest superseder winning, as mem's index derives them.
pub fn rows(files: Vec<(String, String)>, project_id: &str, name: Option<&str>) -> Vec<Row> {
    let mut rows: Vec<Row> = files
        .into_iter()
        .filter(|(path, _)| {
            path.strip_prefix(&format!("projects/{project_id}/items/"))
                .is_some_and(is_item_file)
        })
        .filter_map(|(path, body)| {
            let item = Item::parse(&body)?;
            let project = item.meta.project.clone().or(name.map(str::to_string));
            Some(Row {
                item,
                path,
                project,
                active: true,
                superseded_by: None,
            })
        })
        .collect();
    let edges: Vec<(String, i64, String)> = rows
        .iter()
        .filter_map(|r| {
            let target = r.meta().supersedes.clone()?;
            Some((target, r.meta().created, r.meta().id.clone()))
        })
        .collect();
    for row in &mut rows {
        let id = row.meta().id.clone();
        row.superseded_by = edges
            .iter()
            .filter(|(target, _, by)| *target == id && *by != id)
            .max_by(|a, b| (a.1, &a.2).cmp(&(b.1, &b.2)))
            .map(|(_, _, by)| by.clone());
        row.active = !row.meta().archived && row.superseded_by.is_none();
    }
    rows
}

/// mem's `row_json`: the columns every item listing carries.
pub fn row_json(row: &Row) -> Value {
    let m = row.meta();
    json!({
        "id": m.id,
        "short_id": row.short_id(),
        "kind": m.kind,
        "type": m.r#type,
        "title": m.title,
        "tags": m.tags,
        "project": row.project,
        "machine": m.machine,
        "created": date(m.created),
        "modified": date(m.modified),
        "active": row.active,
        "archived": m.archived,
        "supersedes": m.supersedes,
        "superseded_by": row.superseded_by,
        "answers": m.answers,
        "audience": m.audience,
        "task": m.task,
        "by": m.by,
        "replaces": m.replaces,
        "path": row.path,
    })
}

/// Newest first, the order of mem's `recent`: modified, then id, both
/// descending.
pub fn newest_first(rows: &mut [Row]) {
    rows.sort_by(|a, b| (b.meta().modified, &b.meta().id).cmp(&(a.meta().modified, &a.meta().id)));
}
