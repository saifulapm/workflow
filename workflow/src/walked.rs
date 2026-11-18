//! The walk line `workflow land` reads before it rebases: a milestone lands
//! only on code its walk saw. A passing walk ends with `walked <milestone> at
//! <sha>`, and a milestone with nothing to open logs `no walk: <why>`. A
//! refusal is the orchestrator's to clear, never a question to Saiful.

use crate::gitcmd::Git;
use crate::memcli::{self, LogLine};

/// A milestone's newest walk line.
#[derive(Debug, PartialEq)]
pub enum Walk {
    None,
    NoWalk,
    At(String),
}

/// Ok when `slug` may land from HEAD, Err with the reason it may not, None
/// when mem cannot list `project`'s log. `trunk` is the ref the milestone
/// lands on, as origin last had it.
pub fn check(git: &Git, project: &str, slug: &str, trunk: &str) -> Option<Result<(), String>> {
    let lines = memcli::log_lines(project)?;
    Some(match newest(&lines, slug, first_commit(git, trunk)) {
        Walk::None => Err(format!(
            "no walk line for {slug}; walk it, or log 'no walk: <why>'"
        )),
        Walk::NoWalk | Walk::At(_) => Ok(()),
    })
}

/// The milestone's newest walk line in `lines`, newest first. A `no walk:`
/// line names no milestone, so it counts only once logged at or after
/// `since`, the milestone's first commit: an earlier milestone's never lands
/// this one.
pub fn newest(lines: &[LogLine], slug: &str, since: Option<jiff::Timestamp>) -> Walk {
    let walked = format!("walked {slug} at ");
    for line in lines {
        if let Some(rest) = line.title.strip_prefix(&walked)
            && let Some(sha) = rest.split_whitespace().next()
        {
            return Walk::At(sha.to_string());
        }
        if line.title.starts_with("no walk:")
            && since.is_none_or(|since| memcli::ulid_time(&line.id).is_some_and(|at| at >= since))
        {
            return Walk::NoWalk;
        }
    }
    Walk::None
}

/// When the branch's oldest commit not on `trunk` was authored, to the
/// second: the milestone's start. None when it has no such commit.
fn first_commit(git: &Git, trunk: &str) -> Option<jiff::Timestamp> {
    git.out(&["log", "--format=%at", &format!("{trunk}..HEAD")])?
        .lines()
        .filter_map(|s| s.parse::<i64>().ok())
        .min()
        .and_then(|s| jiff::Timestamp::from_second(s).ok())
}
