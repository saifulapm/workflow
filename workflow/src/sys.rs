//! The few things the orchestrator needs from the operating system: clock,
//! mtimes and signals. Signals go through `sh` rather than a libc binding
//! because a process *group* is the target and the shell's `kill` is the
//! portable way to say so.

use std::path::Path;
use std::process::{Command, Stdio};

pub fn now() -> i64 {
    jiff::Timestamp::now().as_second()
}

/// Seconds since the epoch, 0 when there is nothing there.
pub fn mtime(p: &Path) -> i64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The newest mtime anywhere under a directory, `.git` pruned and symlinks not
/// followed -- a linked `node_modules` is not this worktree's activity.
pub fn newest_mtime(dir: &Path) -> i64 {
    fn walk(dir: &Path, best: &mut i64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_name().map(|n| n == ".git").unwrap_or(false) {
                continue;
            }
            let Ok(meta) = entry.path().symlink_metadata() else {
                continue;
            };
            if let Ok(t) = meta.modified()
                && let Ok(d) = t.duration_since(std::time::UNIX_EPOCH)
            {
                *best = (*best).max(d.as_secs() as i64);
            }
            if meta.is_dir() {
                walk(&path, best);
            }
        }
    }
    let mut best = mtime(dir);
    walk(dir, &mut best);
    best
}

fn sh(script: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(script)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn pid_alive(pid: &str) -> bool {
    sh(&format!("kill -0 {pid} 2>/dev/null"))
}

/// The group first, the process second: the dispatch template puts every worker
/// in its own session, so the group is what has to go.
pub fn kill_group(pid: &str, signal: &str) {
    sh(&format!(
        "kill -{signal} -{pid} 2>/dev/null || kill -{signal} {pid} 2>/dev/null"
    ));
}

pub fn pgrep(pattern: &str) -> bool {
    Command::new("pgrep")
        .arg("-f")
        .arg(pattern)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn pkill(pattern: &str) {
    let _ = Command::new("pkill")
        .arg("-f")
        .arg(pattern)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

pub fn sleep(seconds: f64) {
    if seconds > 0.0 {
        std::thread::sleep(std::time::Duration::from_secs_f64(seconds));
    }
}
