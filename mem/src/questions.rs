//! The question queue (spec §7b): asking is non-blocking, waiting is a separate
//! verb with a ceiling, and answering can happen on any machine.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::index::{Index, Purpose, Row};

/// The documented maximum wait. Runtime Bash tools cap at 600 s, so a longer
/// wait belongs to the orchestrator loop rather than to one tool call.
pub const MAX_WAIT: Duration = Duration::from_secs(5 * 60);
pub const DEFAULT_WAIT: Duration = Duration::from_secs(5 * 60);

/// How often the wait loop stats the items directory. Short in tests.
pub fn poll_interval() -> Duration {
    match std::env::var("MEM_POLL_MS")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        Some(ms) => Duration::from_millis(ms),
        None => Duration::from_secs(15),
    }
}

/// Sync is asked for at 2 minutes and then backs off, so a long wait does not
/// turn into a bisync round every fifteen seconds.
pub const SYNC_AT: [u64; 3] = [120, 300, 900];

/// `<N>[smh]`, capped at MAX_WAIT.
pub fn parse_timeout(text: &str) -> Option<Duration> {
    let text = text.trim();
    let (digits, unit) = text.split_at(text.len().checked_sub(1)?);
    let n: u64 = digits.parse().ok()?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        _ => return None,
    };
    Some(Duration::from_secs(secs).min(MAX_WAIT))
}

/// A cheap change signal for a directory: an answer arriving is a new file, and
/// a new file moves the directory's mtime.
pub fn dir_stamp(dir: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(dir).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some((mtime.as_secs(), mtime.subsec_nanos() as u64))
}

pub enum Waited {
    Answered(Box<Row>),
    TimedOut,
}

/// Polls until the question is answered or the deadline passes. Reindexing only
/// happens when the directory actually changed, so a five minute wait is a
/// handful of stat calls rather than twenty full reindexes.
pub fn wait_for_answer(
    index_path: &Path,
    store: &crate::store::Store,
    items_dir: &Path,
    question_id: &str,
    timeout: Duration,
    mut on_sync: impl FnMut(),
) -> Result<Waited> {
    let started = Instant::now();
    let interval = poll_interval();
    let mut stamp = dir_stamp(items_dir);
    let mut next_sync = 0usize;

    loop {
        let index = Index::open(index_path, Purpose::Read)?;
        index.reindex(store, false)?;
        if let Some(answer) = index.answer_to(question_id)? {
            return Ok(Waited::Answered(Box::new(answer)));
        }
        drop(index);

        loop {
            if started.elapsed() >= timeout {
                return Ok(Waited::TimedOut);
            }
            std::thread::sleep(interval.min(timeout.saturating_sub(started.elapsed())));
            let elapsed = started.elapsed().as_secs();
            if next_sync < SYNC_AT.len() && elapsed >= SYNC_AT[next_sync] {
                next_sync += 1;
                on_sync();
                break;
            }
            let now = dir_stamp(items_dir);
            if now != stamp {
                stamp = now;
                break;
            }
            if started.elapsed() >= timeout {
                return Ok(Waited::TimedOut);
            }
        }
    }
}
