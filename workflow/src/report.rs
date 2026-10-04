//! `workflow report <state> "<note>"`: one status line, written by the
//! binary instead of by hand.
//!
//! A worker's report is `<utc> <state> <note>` appended to
//! `runs/<project>/<plan>/<task>.status`, and every worker used to compose
//! that line itself. ebdify's auth worker wrote the state first and the
//! time second, so the run read the time as the state and never once saw a
//! `ready` from it; its worker task hand-built the time and dated a whole
//! attempt a day into the future. This verb takes the state and the note,
//! supplies the time, finds the file the way the pre-commit hook finds the
//! task's Verify line, and refuses a state the run would not read.
//!
//! Outside a run worktree there is no file to find, so a caller there names
//! one in `WORKFLOW_STATUS_FILE` and gets the same line.

use crate::{brief, exit, paths, repo, sys, verify, warn};

pub fn cmd_report(state: &str, note: &str) -> i32 {
    let state = state.trim().trim_end_matches(':');
    if !brief::STATES.contains(&state) {
        warn(format!(
            "report: '{state}' is not a state -- one of {}",
            brief::STATES.join(", ")
        ));
        return exit::USAGE;
    }
    // Read before goto_toplevel moves the cwd, so a relative name means the
    // directory the caller was in.
    let named = std::env::var_os("WORKFLOW_STATUS_FILE")
        .filter(|v| !v.is_empty())
        .and_then(|v| std::path::absolute(v).ok());
    let top = repo::goto_toplevel().map(|(_, top)| top);
    // A run worktree's own file wins, so a variable left in the environment
    // cannot send a worker's report somewhere the run never reads.
    let file = match (top.as_deref().and_then(verify::task_status_file), named) {
        (Some(file), _) | (None, Some(file)) => file,
        (None, None) => {
            match top {
                None => warn("report: not inside a git work tree"),
                Some(top) => warn(format!(
                    "report: {} is not a run worktree -- the status file is the run's, under {}",
                    top.display(),
                    paths::runs_root().display()
                )),
            }
            return exit::USAGE;
        }
    };
    let note = note.split_whitespace().collect::<Vec<_>>().join(" ");
    let line = match note.is_empty() {
        true => format!("{} {state}\n", sys::utc_now()),
        false => format!("{} {state} {note}\n", sys::utc_now()),
    };
    use std::io::Write;
    let appended = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&file)
        .and_then(|mut f| f.write_all(line.as_bytes()));
    if let Err(e) = appended {
        warn(format!("report: cannot write {}: {e}", file.display()));
        return exit::FAILED;
    }
    exit::OK
}
