//! A plan tick on the same command line as a commit. The pre-commit hook
//! can refuse the commit, and the tick then marks a task done that never
//! landed (eventsmith m17, lareys-trendsi m16).

use super::Call;
use crate::shell::{argv, program_name};

const REFUSAL: &str = "workflow guard refused this on purpose: a tick on a commit's command line runs even when the commit hook refused it.\n\
Commit in one call, read its sha, then `mem plan --tick` in the next.";

pub fn refusal(call: &Call) -> Option<&'static str> {
    let mut commit = false;
    let mut tick = false;
    for command in call.commands {
        let argv = argv(&command.words);
        match argv.first().map(|w| program_name(w)) {
            Some("git") => commit |= git_commits(argv),
            Some("mem") => tick |= ticks(argv),
            _ => {}
        }
    }
    (commit && tick).then_some(REFUSAL)
}

/// `git ... commit`, past `-C <dir>` and `-c <key=value>`.
fn git_commits(argv: &[String]) -> bool {
    let mut words = argv[1..].iter().map(String::as_str);
    while let Some(word) = words.next() {
        match word {
            "-C" | "-c" => {
                words.next();
            }
            _ if word.starts_with('-') => {}
            _ => return word == "commit",
        }
    }
    false
}

/// `mem plan --tick <id>`, with mem's own flags anywhere.
fn ticks(argv: &[String]) -> bool {
    argv.iter().any(|w| w == "plan")
        && argv
            .iter()
            .any(|w| w == "--tick" || w.starts_with("--tick="))
}
