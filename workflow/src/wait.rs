//! `workflow wait-answer <id>...`: a thread of a Claude Project waits on its
//! questions to Saiful for 30 minutes from the ask, then parks (#26D44388).
//! One call runs one `mem questions --wait` of at most 5 minutes, under a
//! thread's 10-minute Bash call, and the clock is the question's own ULID
//! time, so it outlasts the call: the thread runs it again on `again:`.

use std::process::{Command, Stdio};

use crate::{exit, memcli, warn};

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
    if answered(&asked) {
        return exit::OK;
    }
    let newest = asked.iter().filter_map(|q| q.asked()).max();
    let Some(deadline) = newest.map(|at| at + PARK_AFTER) else {
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
        let status = Command::new(memcli::bin())
            .args(["questions", "--wait", open.unwrap_or_default()])
            .args(["--timeout", &format!("{secs}s")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !matches!(status.map(|s| s.code()), Ok(Some(0 | MEM_WAIT_TIMEOUT))) {
            warn("wait-answer: mem questions --wait failed; run workflow wait-answer again");
            return exit::FAILED;
        }
        let Some(asked) = read(&ids) else {
            return exit::USAGE;
        };
        if answered(&asked) {
            return exit::OK;
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

/// Print each answer and say whether there was one.
fn answered(asked: &[memcli::Question]) -> bool {
    let mut any = false;
    for q in asked.iter().filter(|q| q.answered) {
        println!(
            "answered: question #{}: {}",
            q.short_id,
            q.answer.as_deref().unwrap_or_default().trim()
        );
        any = true;
    }
    any
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
