//! Maintenance: version gate, outbox spool, prune, snapshot and doctor
//! (spec §3, §7, §10).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use jiff::Timestamp;

use crate::atomic::write_atomic;
use crate::exit;
use crate::index::Index;
use crate::item::Item;
use crate::store::{STORE_VERSION, Store};

/// Snapshots kept, newest first (spec §3).
pub const SNAPSHOT_KEEP: usize = 14;

/// Prune candidate rules (spec §7).
pub const SUPERSEDED_DAYS: i64 = 30;
pub const LOG_DAYS: i64 = 90;
pub const FACT_DAYS: i64 = 180;
pub const ANSWERED_DAYS: i64 = 30;

/// What the store says its format is. A store with no VERSION file is version 1
/// waiting to be stamped.
pub fn store_version(store: &Store) -> u32 {
    std::fs::read_to_string(store.version_path())
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(STORE_VERSION)
}

/// Writes refuse against a newer store; reads degrade with a warning. A synced
/// VERSION bump must never take another machine's sessions down.
pub fn check_write_version(store: &Store) -> Result<()> {
    let found = store_version(store);
    if found > STORE_VERSION {
        return Err(exit::store_error(format!(
            "this store is format {found} and this mem understands {STORE_VERSION} — \
             upgrade mem before writing"
        )));
    }
    Ok(())
}

pub fn read_version_warning(store: &Store) -> Option<String> {
    let found = store_version(store);
    (found > STORE_VERSION).then(|| {
        format!("! store format {found} is newer than this mem ({STORE_VERSION}) — reads only")
    })
}

/// Stamps VERSION on the first write to a fresh store.
pub fn ensure_version(store: &Store) -> Result<()> {
    if !store.version_path().exists() {
        write_atomic(
            &store.version_path(),
            format!("{STORE_VERSION}\n").as_bytes(),
        )?;
    }
    Ok(())
}

/// A write that could not land goes here and is replayed later (spec §10).
pub fn spool(outbox: &Path, item: &Item) -> Result<PathBuf> {
    let path = outbox.join(format!("{}.md", item.meta.id));
    write_atomic(&path, &item.to_bytes()?)?;
    Ok(path)
}

pub fn outbox_backlog(outbox: &Path) -> Vec<PathBuf> {
    crate::store::read_dir_sorted(outbox)
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect()
}

/// Replays spooled writes into the store. An item carries its project name, so
/// the destination is recoverable from the file alone.
pub fn replay_outbox(store: &Store, outbox: &Path) -> Result<usize> {
    let registry = crate::project::Registry::load(store);
    let mut replayed = 0;
    for path in outbox_backlog(outbox) {
        let Ok(item) = crate::store::read_item(&path) else {
            continue;
        };
        let dir = match item
            .meta
            .project
            .as_deref()
            .and_then(|name| registry.by_name(name).ok().flatten())
        {
            Some(p) => store.project_items(&p.id),
            None => store.global_items(),
        };
        std::fs::create_dir_all(&dir)?;
        if store.write_item(&dir, &item).is_ok() {
            let _ = std::fs::remove_file(&path);
            replayed += 1;
        }
    }
    Ok(replayed)
}

/// A tarball of the store, kept alongside the last thirteen.
pub fn snapshot(store: &Store, snapshots: &Path, now: Timestamp) -> Result<PathBuf> {
    std::fs::create_dir_all(snapshots)?;
    let stamp = now.to_string().replace([':', '-'], "").replace('.', "");
    let path = snapshots.join(format!("mem-{}.tar.gz", &stamp[..15.min(stamp.len())]));
    let parent = store
        .root
        .parent()
        .ok_or_else(|| exit::store_error("the store has no parent directory"))?;
    let name = store
        .root
        .file_name()
        .ok_or_else(|| exit::store_error("the store has no name"))?;
    let status = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&path)
        .arg("-C")
        .arg(parent)
        .arg(name)
        .status()
        .context("running tar")?;
    if !status.success() {
        return Err(exit::store_error("tar could not write the snapshot"));
    }
    prune_snapshots(snapshots);
    Ok(path)
}

fn prune_snapshots(snapshots: &Path) {
    let mut existing: Vec<PathBuf> = crate::store::read_dir_sorted(snapshots)
        .into_iter()
        .filter(|p| p.to_string_lossy().ends_with(".tar.gz"))
        .collect();
    // Names sort chronologically, so the oldest are at the front.
    while existing.len() > SNAPSHOT_KEEP {
        let oldest = existing.remove(0);
        let _ = std::fs::remove_file(oldest);
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: String,
    pub short_id: String,
    pub kind: String,
    pub title: String,
    pub reason: String,
    pub path: PathBuf,
}

/// Stale items, by the rules in spec §7. Nothing is deleted here or anywhere:
/// `--apply` sets a flag in the file.
pub fn prune_candidates(index: &Index, now: Timestamp) -> Result<Vec<Candidate>> {
    let day = 86_400;
    let now = now.as_second();
    let sql = format!(
        "SELECT {}, CASE
             WHEN items.superseded_by IS NOT NULL AND items.modified_epoch < ?1 THEN 'superseded'
             WHEN items.kind = 'log' AND items.modified_epoch < ?2 THEN 'old log'
             WHEN items.kind = 'fact' AND items.modified_epoch < ?3
                  AND items.tags NOT LIKE '%\"pinned\"%' THEN 'untouched fact'
             WHEN items.kind IN ('question','answer') AND items.modified_epoch < ?4
                  AND (items.answers IS NOT NULL
                       OR EXISTS (SELECT 1 FROM items a WHERE a.answers = items.id))
                  THEN 'answered'
             ELSE NULL END AS reason
         FROM items WHERE items.archived = 0",
        crate::index::ROW_COLUMNS
    );
    let mut stmt = index.conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![
            now - SUPERSEDED_DAYS * day,
            now - LOG_DAYS * day,
            now - FACT_DAYS * day,
            now - ANSWERED_DAYS * day
        ],
        |r| {
            let reason: Option<String> = r.get("reason")?;
            Ok((crate::index::Row::from_sql(r)?, reason))
        },
    )?;
    let mut out = Vec::new();
    for row in rows {
        let (row, reason) = row?;
        if let Some(reason) = reason {
            out.push(Candidate {
                id: row.id,
                short_id: row.short_id,
                kind: row.kind,
                title: row.title,
                reason,
                path: row.path,
            });
        }
    }
    Ok(out)
}

/// Archival is a flag in the file, not a move: a rename is a delete plus a
/// create to bisync, which would trip the max-delete guard on a big prune.
pub fn archive_in_place(path: &Path, now: Timestamp) -> Result<()> {
    let mut item = crate::store::read_item(path)?;
    item.meta.archived = Some(true);
    item.meta.archived_at = Some(now);
    item.meta.modified = now;
    write_atomic(path, &item.to_bytes()?)
}

/// A finding is something a human should look at; none of them are fatal.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Finding {
    pub check: String,
    pub detail: String,
}

pub fn finding(check: &str, detail: impl Into<String>) -> Finding {
    Finding {
        check: check.to_string(),
        detail: detail.into(),
    }
}

/// How long an unbroken base64-alphabet run has to be before it is worth a
/// second look.
const ENCODED_RUN: usize = 120;

/// How much body a PEM header has to be followed by: one unbroken run of at
/// least this many base64 characters on a later line. A key's body lines are
/// 64 wide, and prose about keys has no such run.
const PEM_BODY: usize = 40;

/// A character base64 can be written with.
fn encoded_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='
}

/// One unbroken run of base64-alphabet characters, and which classes it holds.
#[derive(Default)]
struct Run {
    len: usize,
    upper: bool,
    lower: bool,
    digit: bool,
}

impl Run {
    fn push(&mut self, c: char) {
        self.len += 1;
        self.upper |= c.is_ascii_uppercase();
        self.lower |= c.is_ascii_lowercase();
        self.digit |= c.is_ascii_digit();
    }

    /// Length alone said "secret" to a rule of `=`, a wall of one letter and any
    /// hex digest — none of which is one. Encoded bytes carry upper, lower and
    /// digits together; over 120 characters, real base64 missing a class is
    /// vanishingly unlikely, and a hash or an identifier never has all three.
    fn looks_encoded(&self) -> bool {
        self.len >= ENCODED_RUN && self.upper && self.lower && self.digit
    }
}

/// The four hooks the mem skill's wiring depends on, and the command substring
/// that makes each one count as wired (spec ruling 7, `TESTING.md` §4).
///
/// Stop was specified in spec §9 from the start and never wired anywhere, and
/// this list checking only the other three is why nobody noticed: pi's
/// extension has fired the nudge on `agent_settled` since it shipped, and no
/// Claude Code session ever has.
const REQUIRED_HOOKS: [(&str, &str); 4] = [
    ("SessionStart", "mem context"),
    ("PostToolBatch", "mem context --brief --hook-json"),
    ("Stop", "mem session-check --session-id"),
    ("PreCompact", "mem precompact --hook-json"),
];

/// One finding per missing adapter hook in a Claude Code settings file; an
/// unreadable file (missing, or not valid JSON) is one finding naming the
/// path rather than three findings about hooks it cannot see.
pub fn hook_findings(settings: &Path) -> Vec<Finding> {
    let Ok(text) = std::fs::read_to_string(settings) else {
        return vec![finding(
            "hooks",
            format!("{} is not readable", settings.display()),
        )];
    };
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(&text) else {
        return vec![finding(
            "hooks",
            format!("{} is not readable", settings.display()),
        )];
    };
    REQUIRED_HOOKS
        .into_iter()
        .filter(|(event, wants)| !hook_wired(&doc, event, wants))
        .map(|(event, wants)| {
            finding(
                "hooks",
                format!(
                    "{event} has no command containing `{wants}` in {}",
                    settings.display()
                ),
            )
        })
        .collect()
}

fn hook_wired(doc: &serde_json::Value, event: &str, wants: &str) -> bool {
    doc["hooks"][event]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|group| group["hooks"].as_array())
        .flatten()
        .filter_map(|h| h["command"].as_str())
        .any(|cmd| cmd.contains(wants))
}

/// The pi extension mem ships, embedded so `mem doctor` can tell an installed
/// copy from what this binary would write (ruling 3).
pub const PI_EXTENSION: &str = include_str!("../assets/pi/mem.ts");

/// `pi extension missing` when nothing is at `path`; `pi extension stale` when
/// what is there does not match the embedded copy; nothing when it does.
pub fn pi_extension_findings(path: &Path) -> Vec<Finding> {
    match std::fs::read_to_string(path) {
        Ok(text) if text == PI_EXTENSION => Vec::new(),
        Ok(_) => vec![finding(
            "pi extension",
            format!("{} is stale — mem doctor --fix writes it", path.display()),
        )],
        Err(_) => vec![finding(
            "pi extension",
            format!("{} is missing — mem doctor --fix writes it", path.display()),
        )],
    }
}

/// The longest unbroken base64 run on any one line of `text`.
fn longest_run(text: &str) -> usize {
    text.lines()
        .map(|line| {
            let mut run = Run::default();
            let mut longest = 0;
            for c in line.chars() {
                if encoded_char(c) {
                    run.push(c);
                    longest = longest.max(run.len);
                } else {
                    run = Run::default();
                }
            }
            longest
        })
        .max()
        .unwrap_or(0)
}

/// An AWS access key id is `AKIA` and sixteen more of `[A-Z0-9]`, all in one
/// run. Counting uppercase letters anywhere in the document instead flagged
/// prose that merely says the word.
fn holds_aws_key(text: &str) -> bool {
    text.match_indices("AKIA").any(|(at, _)| {
        text[at + 4..]
            .chars()
            .take(16)
            .filter(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            .count()
            == 16
    })
}

/// One `-----BEGIN … PRIVATE KEY-----` header with a body under it. The
/// header alone is what a note quoting it looks like — a reader's follow-up
/// about the scrubber was refused as a key (friction #7269R5F0) — so the
/// scan wants the encoded bytes the header introduces.
fn holds_private_key(text: &str) -> bool {
    text.split("-----BEGIN")
        .skip(1)
        .filter_map(|rest| rest.split_once("-----"))
        .any(|(header, body)| header.contains("PRIVATE KEY") && longest_run(body) >= PEM_BODY)
}

/// The secret shapes worth refusing to keep: an AWS key, a PEM block, or a
/// long unbroken run that looks like encoded bytes.
pub fn looks_like_a_secret(text: &str) -> Option<&'static str> {
    if holds_aws_key(text) {
        return Some("an AWS access key");
    }
    if holds_private_key(text) {
        return Some("a private key");
    }
    let mut run = Run::default();
    for c in text.chars() {
        if encoded_char(c) {
            run.push(c);
            if run.looks_encoded() {
                return Some("a long base64 run");
            }
        } else {
            run = Run::default();
        }
    }
    None
}
