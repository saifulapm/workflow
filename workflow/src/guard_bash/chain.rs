//! A commit or deploy chained after a test or lint, and anything chained
//! after a piped git merge. A pipe or `;` hides the first command's exit
//! code, so a red test was committed (ebdfy m38, lareys-trendsi m5) and a
//! failed merge's worktree removed (dotfiles m76). A `mem log` saying green
//! chained after a log read logs before the read is seen (lareys-trendsi
//! m30 logged a suite green whose log held a failing test).

use super::Call;
use crate::shell::{Op, argv, program_name, subcommand};

const AFTER_CHECK: &str = "workflow guard refused this on purpose: a commit or deploy chained after a test or lint runs whatever the test said.\n\
Run tests in one call, commit in the next: read the result in between.";

const AFTER_MERGE: &str = "workflow guard refused this on purpose: a command chained after a piped git merge runs even when the merge failed.\n\
Run git merge in one call, read its result, then the next command in its own call.";

const GREEN_AFTER_READ: &str = "workflow guard refused this on purpose: a mem log saying green chained after a log read logs before you have seen the read.\n\
Read the log's exit=0 line in a call of its own, then log green in the next call.";

pub fn refusal(call: &Call) -> Option<&'static str> {
    let mut read = false;
    let mut checked = false;
    let mut merge_in_pipeline = false;
    let mut after_piped_merge = false;
    let mut piped = false;
    for command in call.commands {
        let argv = argv(&command.words);
        if after_piped_merge {
            return Some(AFTER_MERGE);
        }
        if checked && (is_commit(argv) || is_deploy(argv)) {
            return Some(AFTER_CHECK);
        }
        if read && logs_green(argv) {
            return Some(GREEN_AFTER_READ);
        }
        checked |= is_check(argv);
        read |= is_read(argv);
        merge_in_pipeline |= git_subcommand(argv) == Some("merge");
        if command.then == Op::Pipe {
            piped = true;
        } else {
            after_piped_merge = merge_in_pipeline && piped;
            merge_in_pipeline = false;
            piped = false;
        }
    }
    None
}

/// A test runner or linter.
fn is_check(argv: &[String]) -> bool {
    let Some(program) = argv.first().map(|w| program_name(w)) else {
        return false;
    };
    let sub = subcommand(argv);
    match program {
        "vitest" | "jest" | "mocha" | "pytest" | "oxlint" | "eslint" | "tsc" | "shellcheck" => true,
        // Their formatters check nothing.
        "ruff" => sub == Some("check"),
        "biome" => matches!(sub, Some("check" | "lint" | "ci")),
        "playwright" => sub == Some("test"),
        "cargo" => matches!(sub, Some("test" | "clippy" | "nextest")),
        "go" => matches!(sub, Some("test" | "vet")),
        "npm" | "pnpm" | "yarn" | "bun" => match sub {
            Some("test" | "lint") => true,
            Some("run") => matches!(script(argv), Some("test" | "lint")),
            _ => false,
        },
        _ => false,
    }
}

/// A file reader, as a suite's log is read.
fn is_read(argv: &[String]) -> bool {
    argv.first().is_some_and(|w| {
        matches!(
            program_name(w),
            "cat" | "tail" | "head" | "grep" | "rg" | "sed" | "awk" | "less"
        )
    })
}

/// `mem log` whose words say green.
fn logs_green(argv: &[String]) -> bool {
    if argv.first().map(|w| program_name(w)) != Some("mem") {
        return false;
    }
    let mut words = argv[1..].iter().map(String::as_str);
    let sub = loop {
        match words.next() {
            Some("--project") => {
                words.next();
            }
            Some(word) if word.starts_with('-') => {}
            other => break other,
        }
    };
    sub == Some("log") && words.any(says_green)
}

fn says_green(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.match_indices("green").any(|(i, w)| {
        let before = lower[..i].chars().next_back();
        let after = lower[i + w.len()..].chars().next();
        before.is_none_or(|c| !c.is_alphabetic()) && after.is_none_or(|c| !c.is_alphabetic())
    })
}

fn is_commit(argv: &[String]) -> bool {
    git_subcommand(argv) == Some("commit")
}

fn is_deploy(argv: &[String]) -> bool {
    let Some(program) = argv.first().map(|w| program_name(w)) else {
        return false;
    };
    match program {
        "wrangler" | "cf" => subcommand(argv) == Some("deploy"),
        "npm" | "pnpm" | "yarn" | "bun" => {
            subcommand(argv) == Some("run") && script(argv) == Some("deploy")
        }
        _ => false,
    }
}

/// The script `npm run <script>` names.
fn script(argv: &[String]) -> Option<&str> {
    argv.iter()
        .skip_while(|w| *w != "run")
        .skip(1)
        .map(String::as_str)
        .find(|w| !w.starts_with('-'))
}

/// git's subcommand, past `-C <dir>` and `-c <key=value>`.
fn git_subcommand(argv: &[String]) -> Option<&str> {
    if argv.first().map(|w| program_name(w)) != Some("git") {
        return None;
    }
    let mut words = argv[1..].iter().map(String::as_str);
    while let Some(word) = words.next() {
        match word {
            "-C" | "-c" => {
                words.next();
            }
            _ if word.starts_with('-') => {}
            _ => return Some(word),
        }
    }
    None
}
