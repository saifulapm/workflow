//! Machine-local session activity. Session ids have no meaning on
//! another machine, so none of this is ever synced. It exists to answer what a
//! hook cannot answer for itself: how many tool batches this session has run,
//! and which brief it last saw.

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
    pub batches: u64,
    /// The brief this session last saw; `None` until its first tool batch.
    #[serde(default)]
    pub brief: Option<String>,
    #[serde(default)]
    pub last: String,
}

pub fn nonempty(id: &str) -> Option<String> {
    (!id.trim().is_empty()).then(|| id.to_string())
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

/// Counts a tool batch and keeps the brief it computed, returning the brief
/// this session saw before, or `None` on its first batch. The PostToolBatch
/// hook fires on every batch and speaks only when the two differ.
pub fn record_batch(sessions_dir: &Path, id: &str, brief: &str) -> Option<String> {
    let mut activity = read(sessions_dir, id);
    activity.batches += 1;
    activity.last = Timestamp::now().to_string();
    let seen = activity.brief.replace(brief.to_string());
    let _ = write(sessions_dir, id, &activity);
    seen
}

/// Removes session files older than a week. Called from doctor.
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
