//! `workflow wait` -- block until the live run needs the orchestrator.
//!
//! The session that owns a run used to sleep on a fixed clock and read
//! `workflow status` when it woke, which is ten minutes of nothing after
//! every event and a tool call per nap. The run now writes one line per
//! event that needs somebody to the run dir's `events` file, and this waits
//! on that file: a poll every two seconds on a local file, and an exit the
//! moment there is something to act on. Nothing here dispatches or writes
//! state; the one thing it records is a cursor, so what was reported once is
//! not reported again.

use std::path::{Path, PathBuf};

use crate::gitcmd::Git;
use crate::{exit, memcli, paths, run, sys, warn};

/// How often the events file is read.
const POLL_S: f64 = 2.0;

/// A question is pending for the orchestrator.
pub const QUESTION: i32 = 2;
/// A task failed and the run will not retry it by itself.
pub const FAILED: i32 = 1;
/// A merge landed (only under `--merges`).
pub const MERGED: i32 = 4;
/// Nothing new before the timeout.
pub const TIMEOUT: i32 = 3;

/// The run dir an orchestrator holds right now, if any.
fn live_run(project_dir: &str) -> Option<PathBuf> {
    let root = paths::runs_root().join(project_dir);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        // `started` as well as `plan.md`: the run writes the plan down only
        // once the trunk gate is green, and for the whole of that suite --
        // minutes -- a held lock was invisible here and wait said no run was
        // live (friction #KBPVJF24). A file either way, since the lock file
        // itself makes the directory.
        .filter(|p| p.is_dir() && (p.join("started").is_file() || p.join("plan.md").is_file()))
        .collect();
    dirs.sort();
    dirs.into_iter().find(|d| run::lock_run(d).is_none())
}

fn cursor(dir: &Path) -> u64 {
    std::fs::read_to_string(dir.join("wait.cursor"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// The lines past the cursor, and where the cursor stands after them.
fn new_lines(dir: &Path, from: u64) -> (Vec<String>, u64) {
    let text = std::fs::read_to_string(dir.join("events")).unwrap_or_default();
    let from = (from as usize).min(text.len());
    let rest = &text[from..];
    // Only whole lines: a line the run is still writing waits for the next read.
    let end = rest.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let lines = rest[..end].lines().map(str::to_string).collect();
    (lines, (from + end) as u64)
}

/// The kind of an event line, past its timestamp.
fn kind(line: &str) -> &str {
    line.split_whitespace().nth(1).unwrap_or("")
}

/// Which of the lines is the one to exit on, and how: a question before a
/// failure before the end before a merge, since the orchestrator reads every
/// line printed anyway and the code says what is most urgent among them.
fn verdict(lines: &[String], merges: bool) -> Option<i32> {
    let kinds: Vec<&str> = lines.iter().map(|l| kind(l)).collect();
    if kinds.contains(&"question") {
        Some(QUESTION)
    } else if kinds.contains(&"failed") {
        Some(FAILED)
    } else if kinds.contains(&"ended") {
        Some(exit::OK)
    } else if merges && kinds.contains(&"merged") {
        Some(MERGED)
    } else {
        None
    }
}

pub fn cmd_wait(timeout: Option<u64>, merges: bool) -> i32 {
    if !Git::here().inside_worktree() {
        warn("wait: stand in the project checkout");
        return exit::USAGE;
    }
    let Some(project) = memcli::project_current() else {
        warn("wait: mem does not know this checkout");
        return exit::USAGE;
    };
    let Some(dir) = live_run(&project.dir_name()) else {
        warn("wait: no run is live here -- `workflow status` says how the last one ended");
        return exit::OK;
    };
    let plan = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    // One waiter per run: two share the cursor, and whichever reads first
    // consumes an event into a log nobody reads (m1-lessons ruling 9).
    let lock = dir.join("wait.lock");
    if let Some(pid) = std::fs::read_to_string(&lock)
        .ok()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty() && p != &std::process::id().to_string() && sys::pid_alive(p))
    {
        warn(format!(
            "wait: another wait (pid {pid}) already watches run {plan} -- one waiter per run, or the two eat each other's events"
        ));
        return exit::USAGE;
    }
    let _ = std::fs::write(&lock, format!("{}\n", std::process::id()));
    let code = wait_loop(&dir, &plan, timeout, merges);
    let _ = std::fs::remove_file(&lock);
    code
}

fn wait_loop(dir: &Path, plan: &str, timeout: Option<u64>, merges: bool) -> i32 {
    let started = sys::now();
    let mut at = cursor(dir);
    loop {
        let (lines, next) = new_lines(dir, at);
        // Lines that need nobody are printed and passed over; the cursor
        // moves past everything read, so nothing is printed twice.
        for line in &lines {
            println!("{line}");
        }
        if next != at {
            let _ = std::fs::write(dir.join("wait.cursor"), format!("{next}\n"));
            at = next;
        }
        if let Some(code) = verdict(&lines, merges) {
            return code;
        }
        // The lock went with the run: it ended without writing so, killed
        // outright. Say so rather than wait on a file nobody appends to.
        if run::lock_run(dir).is_some() {
            warn(format!(
                "wait: run {plan} is no longer live and wrote no ending -- `workflow status` and `workflow reap`"
            ));
            return exit::OK;
        }
        if let Some(t) = timeout
            && sys::now() - started >= t as i64
        {
            // Informative, not empty: what is live and for how long, so a
            // timeout is a report rather than a shrug.
            for line in still_lines(dir) {
                println!("{line}");
            }
            return TIMEOUT;
        }
        sys::sleep(POLL_S);
    }
}

/// One `still: <task> <state> <age>` line per task the run is carrying --
/// dispatched or reviewing -- for the timeout to print.
fn still_lines(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter_map(|n| n.strip_suffix(".state").map(str::to_string))
        .collect();
    names.sort();
    for task in names {
        let state = std::fs::read_to_string(dir.join(format!("{task}.state")))
            .unwrap_or_default()
            .trim()
            .to_string();
        if state != run::DISPATCHED && state != run::REVIEWING {
            continue;
        }
        let since = std::fs::read_to_string(dir.join(format!("{task}.dispatched_at")))
            .ok()
            .and_then(|v| v.trim().parse::<i64>().ok())
            .map(|t| (sys::now() - t).max(0) / 60)
            .unwrap_or(0);
        out.push(format!("still: {task} {state} {since}m"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_outranks_a_failure_outranks_the_end_outranks_a_merge() {
        let l = |s: &str| s.to_string();
        assert_eq!(
            verdict(
                &[
                    l("t merged a"),
                    l("t failed b -- x"),
                    l("t question c -- y")
                ],
                false
            ),
            Some(QUESTION)
        );
        assert_eq!(
            verdict(&[l("t ended 1 merged"), l("t failed b -- x")], false),
            Some(FAILED)
        );
        assert_eq!(
            verdict(&[l("t merged a"), l("t ended 1 merged")], true),
            Some(exit::OK)
        );
        assert_eq!(
            verdict(&[l("t merged a")], false),
            None,
            "a merge alone wakes nobody"
        );
        assert_eq!(verdict(&[l("t merged a")], true), Some(MERGED));
        assert_eq!(verdict(&[], true), None);
    }

    #[test]
    fn only_whole_lines_past_the_cursor_are_read() {
        let dir = std::env::temp_dir().join(format!("wf-wait-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("events"), "t merged a\nt question b --").unwrap();
        let (lines, at) = new_lines(&dir, 0);
        assert_eq!(lines, ["t merged a"]);
        assert_eq!(at, 11);
        std::fs::write(dir.join("events"), "t merged a\nt question b -- why\n").unwrap();
        let (lines, at) = new_lines(&dir, at);
        assert_eq!(lines, ["t question b -- why"]);
        assert_eq!(at, 31);
        // A cursor past the file (a run dir reused) reads as the end.
        assert_eq!(new_lines(&dir, 999), (vec![], 31));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
