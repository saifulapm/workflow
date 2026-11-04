//! A commit or deploy chained after a test or lint, and anything chained
//! after a piped git merge. A pipe or `;` hides the first command's exit
//! code, so a red test was committed (ebdfy m38, lareys-trendsi m5) and a
//! failed merge's worktree removed (dotfiles m76).

use super::Call;
use crate::shell::{Op, argv, program_name, subcommand};

const AFTER_CHECK: &str = "workflow guard: a commit or deploy chained after a test or lint runs whatever the test said.\n\
Run tests in one call, commit in the next: read the result in between.";

const AFTER_MERGE: &str = "workflow guard: a command chained after a piped git merge runs even when the merge failed.\n\
Run git merge in one call, read its result, then the next command in its own call.";

pub fn refusal(call: &Call) -> Option<&'static str> {
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
        checked |= is_check(argv);
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
