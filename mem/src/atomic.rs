//! Atomic file replacement: same-directory dot-temp → fsync(file) → rename →
//! fsync(dir) (spec §7). A reader either sees the previous file or the new one,
//! never a partial write, and the rename is durable across a crash.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use anyhow::{Context, Result, anyhow};
use ulid::Ulid;

/// Steps a write took, recorded only while a `TraceGuard` is alive. This is the
/// test seam AC4 asks for: the durability sequence is observable rather than
/// merely claimed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Temp(PathBuf),
    SyncFile,
    Rename { from: PathBuf, to: PathBuf },
    SyncDir(PathBuf),
}

struct TraceState {
    /// The thread being traced, so a write on an unrelated thread — another
    /// test in the same binary — cannot land in this trace.
    active: Option<std::thread::ThreadId>,
    steps: Vec<Step>,
}

fn trace_state() -> &'static Mutex<TraceState> {
    static STATE: OnceLock<Mutex<TraceState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(TraceState {
            active: None,
            steps: Vec::new(),
        })
    })
}

fn exclusive() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct TraceGuard {
    _exclusive: MutexGuard<'static, ()>,
}

impl TraceGuard {
    pub fn steps(&self) -> Vec<Step> {
        lock(trace_state()).steps.clone()
    }
}

impl Drop for TraceGuard {
    fn drop(&mut self) {
        let mut s = lock(trace_state());
        s.active = None;
        s.steps.clear();
    }
}

/// Starts recording write steps. Concurrent tracers serialise on one lock, so
/// a trace only ever sees its own writes.
pub fn trace() -> TraceGuard {
    let guard = lock(exclusive());
    let mut s = lock(trace_state());
    s.active = Some(std::thread::current().id());
    s.steps.clear();
    drop(s);
    TraceGuard { _exclusive: guard }
}

fn record(step: Step) {
    let mut s = lock(trace_state());
    if s.active == Some(std::thread::current().id()) {
        s.steps.push(step);
    }
}

/// A dot-prefixed temp name in the same directory: excluded from sync by the
/// unit's filters and from indexing by the strict filename check.
pub fn temp_path(dir: &Path) -> PathBuf {
    dir.join(format!(".tmp-{}-{}", std::process::id(), Ulid::generate()))
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tmp = temp_path(dir);
    record(Step::Temp(tmp.clone()));
    let result = (|| -> Result<()> {
        {
            let mut f =
                File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
            f.write_all(bytes)
                .with_context(|| format!("writing {}", tmp.display()))?;
            f.sync_all()
                .with_context(|| format!("fsync {}", tmp.display()))?;
            record(Step::SyncFile);
        }
        fs::rename(&tmp, path)
            .with_context(|| format!("renaming {} to {}", tmp.display(), path.display()))?;
        record(Step::Rename {
            from: tmp.clone(),
            to: path.to_path_buf(),
        });
        // Durability of the rename itself lives in the directory entry.
        File::open(dir)
            .and_then(|d| d.sync_all())
            .with_context(|| format!("fsync {}", dir.display()))?;
        record(Step::SyncDir(dir.to_path_buf()));
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Replaces a file only if it has not changed since it was read (spec §4
/// singleton CAS). `expected` is the mtime observed at read time, or None if
/// the file was absent.
pub fn write_atomic_cas(
    path: &Path,
    bytes: &[u8],
    expected: Option<std::time::SystemTime>,
) -> Result<bool> {
    let current = fs::metadata(path).ok().and_then(|m| m.modified().ok());
    if current != expected {
        return Ok(false);
    }
    write_atomic(path, bytes)?;
    Ok(true)
}

pub fn read_mtime(path: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(path).ok().and_then(|m| m.modified().ok())
}
