//! `workflow report <state> "<note>"`: one status line, written by the
//! binary instead of by hand.
//!
//! A worker's report is `<utc> <state> <note>` appended to
//! `runs/<project>/<plan>/<task>.status`, and every worker used to compose
//! that line itself. ebdify m1's auth worker wrote the state first and the
//! time second, so the run read the time as the state and never once saw a
//! `ready` from it; its worker task hand-built the time and dated a whole
//! attempt a day into the future (m1-lessons, ruling 2). This verb takes the
//! state and the note, supplies the time, finds the file the way the
//! pre-commit hook finds the task's Verify line, and refuses a state the run
//! would not read.

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
    let Some((_, top)) = repo::goto_toplevel() else {
        warn("report: not inside a git work tree");
        return exit::USAGE;
    };
    let Some(file) = verify::task_status_file(&top) else {
        warn(format!(
            "report: {} is not a run worktree -- the status file is the run's, under {}",
            top.display(),
            paths::runs_root().display()
        ));
        return exit::USAGE;
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
