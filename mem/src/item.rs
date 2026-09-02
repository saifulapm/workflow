//! Item format: TOML frontmatter between `+++` fences, opaque body (spec §4).
//!
//! Frontmatter is emitted only through `toml::to_string` — never string
//! formatting — and every emission is checked so that no frontmatter line is a
//! bare `+++` fence. The body is bytes: it round-trips exactly, whatever it
//! contains.

use anyhow::{Context, Result, bail};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use toml::Table;

pub const FENCE: &str = "+++";

/// The known frontmatter keys. Anything else a file carries is kept in
/// `Item::extra` and written back untouched, so a newer store format's fields
/// survive a rewrite by an older binary.
pub const KNOWN_KEYS: &[&str] = &[
    "id",
    "kind",
    "type",
    "title",
    "tags",
    "project",
    "machine",
    "created",
    "modified",
    "supersedes",
    "answers",
    "archived",
    "archived_at",
    "options",
    "audience",
    "task",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Fact,
    Ruling,
    Log,
    Handoff,
    Question,
    Answer,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Fact => "fact",
            Kind::Ruling => "ruling",
            Kind::Log => "log",
            Kind::Handoff => "handoff",
            Kind::Question => "question",
            Kind::Answer => "answer",
        }
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Kind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Kind> {
        Ok(match s {
            "fact" => Kind::Fact,
            "ruling" => Kind::Ruling,
            "log" => Kind::Log,
            "handoff" => Kind::Handoff,
            "question" => Kind::Question,
            "answer" => Kind::Answer,
            other => bail!("unknown kind '{other}' (fact|ruling|log|handoff|question|answer)"),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub id: String,
    pub kind: Kind,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub machine: String,
    pub created: Timestamp,
    pub modified: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<Timestamp>,
    /// `kind = "question"` only: the offered answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// `kind = "question"` only: who is meant to answer. `orchestrator` is a
    /// worker's question, internals the session driving the run settles;
    /// absent means a person, which is what the hub and the phone show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
    /// `kind = "question"` only: the orchestrated task that asked, as
    /// `<plan>/<task>`, so the answer can be carried into its next attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

impl Meta {
    pub fn new(id: String, kind: Kind, title: String, machine: String) -> Meta {
        let now = Timestamp::now();
        Meta {
            id,
            kind,
            r#type: None,
            title,
            tags: Vec::new(),
            project: None,
            machine,
            created: now,
            modified: now,
            supersedes: None,
            answers: None,
            archived: None,
            archived_at: None,
            options: None,
            audience: None,
            task: None,
        }
    }

    pub fn is_archived(&self) -> bool {
        self.archived.unwrap_or(false)
    }

    /// Last 8 characters of the ULID — the entropy half (spec §4).
    pub fn short_id(&self) -> String {
        crate::ids::short_id(&self.id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub meta: Meta,
    /// Frontmatter keys this binary does not know about, preserved verbatim.
    pub extra: Table,
    pub body: Vec<u8>,
}

impl Item {
    pub fn new(meta: Meta, body: Vec<u8>) -> Item {
        Item {
            meta,
            extra: Table::new(),
            body,
        }
    }

    pub fn body_str(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    /// Split on the FIRST complete `+++` fence pair. Fences are lines equal to
    /// `+++` once the line terminator is stripped; LF and CRLF both count.
    /// Everything after the closing fence is body, byte-preserved.
    pub fn parse(bytes: &[u8]) -> Result<Item> {
        let (frontmatter, body) = split(bytes)?;
        let text = std::str::from_utf8(frontmatter).context("frontmatter is not valid UTF-8")?;
        let table: Table = toml::from_str(text).context("frontmatter is not valid TOML")?;
        let meta: Meta = table
            .clone()
            .try_into()
            .context("frontmatter is missing required fields")?;
        let mut extra = Table::new();
        for (k, v) in table {
            if !KNOWN_KEYS.contains(&k.as_str()) {
                extra.insert(k, v);
            }
        }
        Ok(Item {
            meta,
            extra,
            body: body.to_vec(),
        })
    }

    pub fn table(&self) -> Result<Table> {
        let mut table: Table = Table::try_from(&self.meta).context("serializing frontmatter")?;
        for (k, v) in &self.extra {
            table.entry(k.clone()).or_insert_with(|| v.clone());
        }
        Ok(table)
    }

    /// Emits the frontmatter through `toml::to_string` and enforces the fence
    /// guard. `to_string` renders a string that contains newlines as a
    /// multi-line TOML string, so a title of `"\n+++\n"` would put a bare fence
    /// on its own line and split the item in half on the way back in. When that
    /// would happen the string values are re-titled — newlines collapsed to
    /// spaces — and the emission is checked again.
    pub fn frontmatter_string(&self) -> Result<String> {
        let table = self.table()?;
        let text = emit(&table)?;
        if guard_no_fence(&text).is_ok() {
            return Ok(text);
        }
        let text = emit(&collapse_newlines_in(table))?;
        guard_no_fence(&text)?;
        Ok(text)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let fm = self.frontmatter_string()?;
        let mut out = Vec::with_capacity(fm.len() + self.body.len() + 8);
        out.extend_from_slice(b"+++\n");
        out.extend_from_slice(fm.as_bytes());
        out.extend_from_slice(b"+++\n");
        out.extend_from_slice(&self.body);
        Ok(out)
    }
}

fn emit(table: &Table) -> Result<String> {
    let mut text = toml::to_string(table).context("emitting frontmatter TOML")?;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

/// The title a given title is stored under: identical unless it would emit a
/// bare fence line, in which case its newlines become spaces.
pub fn collapse_newlines(s: &str) -> String {
    s.replace("\r\n", " ").replace(['\n', '\r'], " ")
}

fn collapse_newlines_in(table: Table) -> Table {
    table
        .into_iter()
        .map(|(k, v)| (k, collapse_value(v)))
        .collect()
}

fn collapse_value(v: toml::Value) -> toml::Value {
    use toml::Value;
    match v {
        Value::String(s) => Value::String(collapse_newlines(&s)),
        Value::Array(a) => Value::Array(a.into_iter().map(collapse_value).collect()),
        Value::Table(t) => Value::Table(collapse_newlines_in(t)),
        other => other,
    }
}

/// The serializer-output assertion of spec §4: no emitted frontmatter line may
/// be a bare fence, or the file would parse back as a different item.
pub fn guard_no_fence(frontmatter: &str) -> Result<()> {
    for line in frontmatter.split('\n') {
        if line.trim_end_matches('\r') == FENCE {
            bail!("refusing to emit frontmatter containing a bare `{FENCE}` line");
        }
    }
    Ok(())
}

fn is_fence(line: &[u8]) -> bool {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    line == FENCE.as_bytes()
}

/// Returns (frontmatter bytes, body bytes).
fn split(bytes: &[u8]) -> Result<(&[u8], &[u8])> {
    let mut pos = 0usize;
    let mut opened: Option<usize> = None;
    while pos < bytes.len() {
        let end = match memchr_nl(&bytes[pos..]) {
            Some(i) => pos + i + 1,
            None => bytes.len(),
        };
        let line = &bytes[pos..end];
        match opened {
            None => {
                if !is_fence(line) {
                    bail!("item does not start with a `{FENCE}` frontmatter fence");
                }
                opened = Some(end);
            }
            Some(start) => {
                if is_fence(line) {
                    return Ok((&bytes[start..pos], &bytes[end..]));
                }
            }
        }
        pos = end;
    }
    match opened {
        None => bail!("item is empty"),
        Some(_) => bail!("item frontmatter fence is never closed"),
    }
}

fn memchr_nl(haystack: &[u8]) -> Option<usize> {
    haystack.iter().position(|b| *b == b'\n')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fence_detection_handles_both_terminators() {
        assert!(is_fence(b"+++"));
        assert!(is_fence(b"+++\n"));
        assert!(is_fence(b"+++\r\n"));
        assert!(!is_fence(b"++++\n"));
        assert!(!is_fence(b" +++\n"));
        assert!(!is_fence(b"+++ \n"));
    }

    #[test]
    fn kind_parses_and_prints() {
        for k in ["fact", "ruling", "log", "handoff", "question", "answer"] {
            assert_eq!(k.parse::<Kind>().unwrap().as_str(), k);
        }
        assert!("nope".parse::<Kind>().is_err());
    }
}
