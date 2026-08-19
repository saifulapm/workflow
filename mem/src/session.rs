//! Machine-local session activity (spec §3, §9). Session ids have no meaning on
//! another machine, so none of this is ever synced. It exists to answer two
//! questions a hook cannot answer for itself: did this session record anything,
//! and how many tool batches has it run.

use std::path::{Path, PathBuf};

use anyhow::Result;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::atomic::write_atomic;

/// Session files older than this are cleaned up opportunistically.
const KEEP_DAYS: i64 = 7;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Activity {
    #[serde(default)]
    pub writes: u64,
    #[serde(default)]
    pub batches: u64,
    /// Whether the Stop nudge has already been spent on this session. The
    /// runtime hands a Stop hook's additionalContext back to the model, which
    /// answers it and stops again — and answering is not a write, so without
    /// this flag the condition that produced the nudge is still true and the
    /// pair loops until something else stops it.
    #[serde(default)]
    pub nudged: bool,
    #[serde(default)]
    pub last: String,
}

/// The session id from the flag or the environment, in that order. An empty one
/// is no id at all: hooks are wired as `--session-id "$CLAUDE_CODE_SESSION_ID"`,
/// and an unset variable must not become a session file named "".
pub fn id_from(flag: Option<&str>) -> Option<String> {
    flag.map(|s| s.to_string())
        .or_else(|| std::env::var("MEM_SESSION_ID").ok())
        .filter(|s| !s.trim().is_empty())
}

/// A session id is used as a filename, so it may not wander out of the
/// sessions directory.
fn safe(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(128)
        .collect()
}

pub fn path(sessions_dir: &Path, id: &str) -> PathBuf {
    sessions_dir.join(safe(id))
}

pub fn read(sessions_dir: &Path, id: &str) -> Activity {
    std::fs::read_to_string(path(sessions_dir, id))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn write(sessions_dir: &Path, id: &str, activity: &Activity) -> Result<()> {
    let text = serde_json::to_string(activity)?;
    write_atomic(&path(sessions_dir, id), text.as_bytes())
}

/// Records that this session wrote something. Best effort: a session file that
/// cannot be written must never fail the write that prompted it.
pub fn record_write(sessions_dir: &Path, id: &str) {
    let mut activity = read(sessions_dir, id);
    activity.writes += 1;
    activity.last = Timestamp::now().to_string();
    let _ = write(sessions_dir, id, &activity);
}

/// Records that this session has had its one Stop nudge. Best effort for the
/// same reason as `record_write`: the nudge is advice, and advice must not fail
/// a hook.
pub fn record_nudge(sessions_dir: &Path, id: &str) {
    let mut activity = read(sessions_dir, id);
    activity.nudged = true;
    activity.last = Timestamp::now().to_string();
    let _ = write(sessions_dir, id, &activity);
}

/// Counts a tool batch and returns the new count — the PostToolBatch hook fires
/// on every batch and needs to know which one this is.
pub fn record_batch(sessions_dir: &Path, id: &str) -> u64 {
    let mut activity = read(sessions_dir, id);
    activity.batches += 1;
    activity.last = Timestamp::now().to_string();
    let n = activity.batches;
    let _ = write(sessions_dir, id, &activity);
    n
}

/// Removes session files older than a week. Called from doctor and from the
/// write path, where it costs one directory listing.
pub fn cleanup(sessions_dir: &Path) -> usize {
    let cutoff = Timestamp::now().as_second() - KEEP_DAYS * 86_400;
    let mut removed = 0;
    for path in crate::store::read_dir_sorted(sessions_dir) {
        let stale = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| (d.as_secs() as i64) < cutoff)
            .unwrap_or(false);
        if stale && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}
