//! `workflow guard pnpm`, a Claude Code PreToolUse hook for worktree agents.
//! Claude Code links a worktree's node_modules to the main checkout's
//! (`worktree.symlinkDirectories`), so a pnpm install there, or any pnpm
//! command that installs before it runs, rewrites the main checkout's
//! packages. The hook refuses such a command and says what to run instead.
//! Input it cannot read passes: a broken guard must not stop the agent.

use std::io::Read;
use std::path::Path;

use serde_json::Value;

use crate::exit;

/// Claude Code blocks a tool call whose PreToolUse hook exits 2, and shows
/// the agent what the hook wrote to stderr.
const BLOCK: i32 = 2;

const REFUSAL: &str = "workflow guard: pnpm refused. This worktree's node_modules is the \
main checkout's, linked in, and pnpm would rewrite it. Call the tool by path instead, \
as in node_modules/.bin/vitest run <file>.";

pub fn cmd_guard_pnpm() -> i32 {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return exit::OK;
    }
    let Ok(hook) = serde_json::from_str::<Value>(&input) else {
        return exit::OK;
    };
    if hook["tool_name"] != "Bash" {
        return exit::OK;
    }
    let (Some(command), Some(cwd)) = (hook["tool_input"]["command"].as_str(), hook["cwd"].as_str())
    else {
        return exit::OK;
    };
    if runs_pnpm(command) && over_linked_node_modules(Path::new(cwd)) {
        eprintln!("{REFUSAL}");
        return BLOCK;
    }
    exit::OK
}

/// Whether any simple command in `command` runs pnpm, past the variable
/// assignments and wrappers (`timeout 600`, `env`) in front of it.
fn runs_pnpm(command: &str) -> bool {
    command
        .split(['&', '|', ';', '(', ')', '`', '\n'])
        .any(|part| {
            let mut words = part.split_whitespace().peekable();
            while let Some(word) = words.next_if(|w| is_assignment(w) || is_wrapper(w)) {
                if is_wrapper(word) {
                    while words.next_if(|w| is_wrapper_arg(w)).is_some() {}
                }
            }
            words
                .next()
                .and_then(|word| Path::new(word).file_name())
                .is_some_and(|name| name == "pnpm" || name == "pnpx")
        })
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
    })
}

/// A wrapper's own options and numbers, as in `timeout -k 5 600`.
fn is_wrapper_arg(word: &str) -> bool {
    word.starts_with('-') || word.starts_with(|c: char| c.is_ascii_digit())
}

fn is_wrapper(word: &str) -> bool {
    matches!(
        word,
        "env" | "command" | "exec" | "time" | "nice" | "timeout"
    )
}

/// Whether `cwd` sits in a linked worktree (its `.git` is a file) whose
/// node_modules is a symlink. A main checkout's `.git` is a folder.
fn over_linked_node_modules(cwd: &Path) -> bool {
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .is_some_and(|top| top.join(".git").is_file() && top.join("node_modules").is_symlink())
}
