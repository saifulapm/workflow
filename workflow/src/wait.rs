//! `workflow wait-answer <id>...`: a thread of a Claude Project waits on its
//! questions to Saiful for 30 minutes from the ask, then parks (#26D44388).
//! One call runs one `mem questions --wait` of at most 5 minutes, under a
//! thread's 10-minute Bash call, and the clock is the question's own ULID
//! time, so it outlasts the call: the thread runs it again on `again:`. A
//! thread woken since the ask waits 30 minutes again, from a note in the
//! git dir.

use std::process::{Command, Stdio};

use crate::gitcmd::Git;
use crate::{cloud, exit, memcli, warn};

/// How long a thread waits on a question from the moment it was asked.
const PARK_AFTER: jiff::SignedDuration = jiff::SignedDuration::from_mins(30);

/// The longest one `mem questions --wait` may run (mem's MAX_WAIT).
const ONE_WAIT: jiff::SignedDuration = jiff::SignedDuration::from_mins(5);

/// mem's exit code for a wait that ended with no answer.
const MEM_WAIT_TIMEOUT: i32 = 4;

pub fn cmd_wait(ids: &[String]) -> i32 {
    let ids: Vec<&str> = ids.iter().map(|id| id.trim_start_matches('#')).collect();
    let Some(asked) = read(&ids) else {
        return exit::USAGE;
    };
    if let Some(code) = answered(&asked) {
        return code;
    }
    let newest = asked.iter().filter_map(|q| q.asked()).max();
    // A thread woken since the ask waits 30 minutes again, from its first
    // wait after the wake.
    let from = match (newest, cloud::own_since()) {
        (Some(ask), Some(woke)) if woke > ask => Some(wait_began(woke)),
        _ => newest,
    };
    let Some(deadline) = from.map(|at| at + PARK_AFTER) else {
        warn("wait-answer: mem gave no time for these questions");
        return exit::FAILED;
    };
    let left = deadline.duration_since(jiff::Timestamp::now());
    if left.is_positive() {
        let last = left <= ONE_WAIT;
        let secs = left.min(ONE_WAIT).as_secs().max(1);
        let open = asked
            .iter()
            .find(|q| !q.answered)
            .map(|q| q.short_id.as_str());
        let open = open.unwrap_or_default().to_string();
        let out = Command::new(memcli::bin())
            .args(["questions", "--wait", &open])
            .args(["--timeout", &format!("{secs}s")])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        let code = out.as_ref().ok().and_then(|o| o.status.code());
        if !matches!(code, Some(0 | MEM_WAIT_TIMEOUT)) {
            warn("wait-answer: mem questions --wait failed; run workflow wait-answer again");
            return exit::FAILED;
        }
        let Some(mut asked) = read(&ids) else {
            return exit::USAGE;
        };
        // The local hub hands the wait its answer before its own `mem
        // answer` reaches the store, so what the wait printed is the answer.
        let said = out
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if code == Some(0)
            && !said.is_empty()
            && let Some(q) = asked.iter_mut().find(|q| q.short_id == open && !q.answered)
        {
            q.answered = true;
            q.answer = Some(said);
        }
        if let Some(code) = answered(&asked) {
            return code;
        }
        if !last {
            println!(
                "again: {} open; run workflow wait-answer {} again",
                named(&asked, "is", "are"),
                ids.join(" ")
            );
            return exit::AGAIN;
        }
    }
    println!("parked: {} on Saiful", named(&asked, "waits", "wait"));
    exit::USAGE
}

/// When this thread began to wait after its wake at `woke`: noted in the
/// git dir by the first wait since, so the 30 minutes outlast each call, as
/// land notes its CI push. With no git dir, from the wake itself.
fn wait_began(woke: jiff::Timestamp) -> jiff::Timestamp {
    let git = Git::at(".");
    let Some(note) = git
        .out(&["rev-parse", "--git-path", "workflow-wait-began"])
        .map(|p| crate::land::git_path(&git, &p))
    else {
        return woke;
    };
    let noted = std::fs::read_to_string(&note)
        .ok()
        .and_then(|text| text.trim().parse::<jiff::Timestamp>().ok());
    if let Some(at) = noted.filter(|at| *at >= woke) {
        return at;
    }
    let now = jiff::Timestamp::now();
    match std::fs::write(&note, format!("{now}\n")) {
        Ok(()) => now,
        Err(_) => woke,
    }
}

/// The questions `ids` names, as mem reads them now; `None`, said on
/// stderr, when one is not there.
fn read(ids: &[&str]) -> Option<Vec<memcli::Question>> {
    ids.iter()
        .map(|id| {
            let question = memcli::question(id);
            if question.is_none() {
                warn(format!("wait-answer: no question {id}"));
            }
            question
        })
        .collect()
}

/// With an answer, the exit code to end on: each answer printed, once the
/// thread's run is its own again, or the line a run another thread took up
/// parks on.
fn answered(asked: &[memcli::Question]) -> Option<i32> {
    if !asked.iter().any(|q| q.answered) {
        return None;
    }
    if let Err(line) = cloud::carry_on() {
        println!("{line}");
        return Some(exit::USAGE);
    }
    for q in asked.iter().filter(|q| q.answered) {
        println!(
            "answered: question #{}: {}",
            q.short_id,
            q.answer.as_deref().unwrap_or_default().trim()
        );
    }
    Some(exit::OK)
}

/// "question #a waits" or "questions #a and #b wait", of the open ones.
pub fn named(asked: &[memcli::Question], one: &str, many: &str) -> String {
    let open: Vec<String> = asked
        .iter()
        .filter(|q| !q.answered)
        .map(|q| format!("#{}", q.short_id))
        .collect();
    match open.as_slice() {
        [only] => format!("question {only} {one}"),
        [rest @ .., last] => format!("questions {} and {last} {many}", rest.join(", ")),
        [] => String::new(),
    }
}
