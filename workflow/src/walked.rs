//! The walk line `workflow land` reads before it rebases: a milestone lands
//! only on code its walk saw. A passing walk ends with `walked <milestone> at
//! <sha>`, and a milestone with nothing to open logs `no walk: <why>`. A
//! change after the walk that only test files carry lands, and so does one a
//! `tests-only after <sha>: <why>` line lets through. A refusal is the
//! orchestrator's to clear, never a question to Saiful.

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
/// lands on, as origin last had it, and `start` the ref the milestone's own
/// commits are not on yet: the trunk, or main once a landing that asked has
/// put them on an app's trunk already.
pub fn check(
    git: &Git,
    project: &str,
    slug: &str,
    trunk: &str,
    start: &str,
) -> Option<Result<(), String>> {
    let lines = memcli::log_lines(project)?;
    let sha = match newest(&lines, slug, first_commit(git, start)) {
        Walk::None => {
            return Some(Err(format!(
                "no walk line for {slug}; walk it, or log 'no walk: <why>'"
            )));
        }
        Walk::NoWalk => return Some(Ok(())),
        Walk::At(sha) => sha,
    };
    let Some(walked) = commit(git, &sha) else {
        return Some(Err(format!(
            "the walk at {sha} names no commit here; walk {slug} again"
        )));
    };
    let files: Vec<String> = changed_after(git, &walked, trunk)
        .into_iter()
        .filter(|file| file != memcli::OUTBOX_FILE && !is_test_file(file))
        .collect();
    let cleared = tests_only(&lines)
        .into_iter()
        .any(|named| commit(git, named).as_deref() == Some(walked.as_str()));
    if files.is_empty() || cleared {
        return Some(Ok(()));
    }
    let they = if files.len() == 1 {
        "it touches"
    } else {
        "they touch"
    };
    Some(Err(format!(
        "{} changed after the walk at {sha}; re-walk the steps {they}, or log 'tests-only after {sha}: <why>'",
        files.join(", ")
    )))
}

/// The shas `tests-only after <sha>: <why>` lines name.
pub fn tests_only(lines: &[LogLine]) -> Vec<&str> {
    lines
        .iter()
        .filter_map(|line| {
            line.title
                .strip_prefix("tests-only after ")?
                .split_once(':')
        })
        .map(|(sha, _)| sha.trim())
        .collect()
}

/// A path segment `test` or `tests`, or a name holding `.test.` or `.spec.`.
pub fn is_test_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    path.split('/').any(|s| s == "test" || s == "tests")
        || name.contains(".test.")
        || name.contains(".spec.")
}

/// The full sha of the commit `sha` names, None when it names none here.
fn commit(git: &Git, sha: &str) -> Option<String> {
    git.out(&["rev-parse", "-q", "--verify", &format!("{sha}^{{commit}}")])
}

/// The files HEAD's own commits, those not on `trunk`, change after
/// `walked`, sorted: the commits of walked..HEAD. On a second call of land,
/// after its first rebased and pushed the branch, `walked` is on the branch
/// no more, and its commits are found again by patch: HEAD's commits whose
/// patch is not one of `walked`'s own. Merges bring no file in, and a rename
/// names the path it left too.
fn changed_after(git: &Git, walked: &str, trunk: &str) -> Vec<String> {
    let after: Vec<String> = if git.quiet(&["merge-base", "--is-ancestor", walked, "HEAD"]) {
        git.out(&[
            "log",
            "--no-merges",
            "--format=%H",
            &format!("{walked}..HEAD"),
            "--not",
            trunk,
        ])
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
    } else {
        let seen: Vec<String> = patches(git, walked, trunk)
            .into_iter()
            .map(|(patch, _)| patch)
            .collect();
        patches(git, "HEAD", trunk)
            .into_iter()
            .filter(|(patch, _)| !seen.contains(patch))
            .map(|(_, commit)| commit)
            .collect()
    };
    if after.is_empty() {
        return Vec::new();
    }
    let mut args = vec!["show", "--no-renames", "--name-only", "--format="];
    args.extend(after.iter().map(String::as_str));
    let mut files: Vec<String> = git
        .out(&args)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.is_empty())
        .map(String::from)
        .collect();
    files.sort();
    files.dedup();
    files
}

/// Each commit of `tip` not on `trunk`, merges left out, as its patch id and
/// its sha. The patch has no context lines, so a rebase over a trunk that
/// edited lines next to a change keeps its id.
fn patches(git: &Git, tip: &str, trunk: &str) -> Vec<(String, String)> {
    let log = git.bytes(&[
        "log",
        "-p",
        "-U0",
        "--no-merges",
        "--no-renames",
        "--no-ext-diff",
        "--no-color",
        tip,
        "--not",
        trunk,
    ]);
    git.feed(&["patch-id", "--stable"], &log)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(patch, commit)| (patch.to_string(), commit.to_string()))
        .collect()
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

#[cfg(test)]
mod tests {
    use super::is_test_file;

    #[test]
    fn a_test_file_is_named_by_a_segment_or_its_name() {
        for path in [
            "tests/app.rs",
            "test/a.py",
            "apps/web/tests/e2e/x.ts",
            "src/a.test.ts",
            "web/b.spec.ts",
        ] {
            assert!(is_test_file(path), "{path}");
        }
        for path in [
            "src/app.rs",
            "contest/a.rs",
            "src/latest.rs",
            "src/tests.rs",
            "src/test_helpers/x.rs",
            "src/spec.ts",
        ] {
            assert!(!is_test_file(path), "{path}");
        }
    }
}
