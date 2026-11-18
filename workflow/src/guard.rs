//! `workflow guard pnpm`, a Claude Code PreToolUse hook for worktree agents.
//! The WorktreeCreate hook (`workflow worktree create`) links a worktree's
//! root node_modules to the trunk checkout's, so a pnpm install there, or
//! any pnpm command that installs before it runs, rewrites the trunk's
//! packages. The hook refuses such a command and says what to run instead.
//! Input it cannot read passes: a broken guard must not stop the agent.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::guard_bash::by_path;
use crate::{exit, mask, memcli, shell};

/// Claude Code blocks a tool call whose PreToolUse hook exits 2, and shows
/// the agent what the hook wrote to stderr.
const BLOCK: i32 = 2;

const WHY: &str = "workflow guard: pnpm refused. This worktree's node_modules is the \
trunk checkout's, linked in, and pnpm would rewrite it.";

/// What to run instead when the command has no by-path form.
const BY_PATH: &str = "Call the tool by path instead, as in node_modules/.bin/vitest run <file>.";

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
        match shell::commands(command).and_then(|c| by_path::lines(&c, Path::new(cwd))) {
            Some(lines) => eprintln!(
                "{WHY} Run each line below as a call of its own:\n{}",
                lines.join("\n")
            ),
            None => eprintln!("{WHY} {BY_PATH}"),
        }
        record("pnpm", command, Path::new(cwd));
        return BLOCK;
    }
    exit::OK
}

/// How long a guard waits for its mem write before it answers anyway.
const RECORD_WAIT: Duration = Duration::from_secs(1);

/// Log a refusal to the project of `cwd` as `guard <name> refused: <what>`,
/// `what` masked and cut to 120 characters, so a secret in a refused
/// command never reaches mem. mem runs in a process group of its own and
/// the guard waits for it at most RECORD_WAIT: a slow, failing or missing
/// mem never changes the guard's answer.
pub(crate) fn record(name: &str, what: &str, cwd: &Path) {
    if !cwd.is_dir() {
        return;
    }
    let masked = String::from_utf8_lossy(&mask::mask(what.as_bytes())).replace('\n', " ");
    let cut: String = masked.chars().take(120).collect();
    let Ok(mut child) = Command::new(memcli::bin())
        .args(["log", &format!("guard {name} refused: {cut}")])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
    else {
        return;
    };
    let started = Instant::now();
    while started.elapsed() < RECORD_WAIT {
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Whether any simple command in `command` runs pnpm, past the variable
/// assignments and wrappers (`timeout 600`, `env`) in front of it.
fn runs_pnpm(command: &str) -> bool {
    unquoted(command)
        .split(['&', '|', ';', '(', ')', '`', '\n'])
        .any(|part| {
            let mut words = part.split_whitespace().peekable();
            while let Some(word) =
                words.next_if(|w| is_assignment(w) || is_wrapper(w) || is_keyword(w))
            {
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

/// `command` with its quoted text blanked, so a commit message or a search
/// pattern that names pnpm is no command. A backtick inside double quotes
/// still runs its text, so that text stays. A heredoc's body is text as
/// well, so everything from `<<` on is dropped.
fn unquoted(command: &str) -> String {
    enum Quote {
        None,
        Single,
        Double,
        Tick,
    }
    let mut out = String::with_capacity(command.len());
    let mut quote = Quote::None;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Quote::None => match c {
                '\'' => quote = Quote::Single,
                '"' => quote = Quote::Double,
                '\\' => {
                    chars.next();
                }
                '<' if chars.peek() == Some(&'<') => break,
                _ => {
                    out.push(c);
                    continue;
                }
            },
            Quote::Single if c == '\'' => quote = Quote::None,
            Quote::Double => match c {
                '"' => quote = Quote::None,
                '\\' => {
                    chars.next();
                }
                '`' => {
                    quote = Quote::Tick;
                    out.push('\n');
                }
                _ => {}
            },
            Quote::Tick if c == '`' => {
                quote = Quote::Double;
                out.push('\n');
            }
            Quote::Tick => {
                out.push(c);
                continue;
            }
            Quote::Single => {}
        }
        out.push(' ');
    }
    out
}

/// Shell words that open or join a command rather than name one.
fn is_keyword(word: &str) -> bool {
    matches!(
        word,
        "{" | "}" | "!" | "if" | "then" | "else" | "elif" | "do" | "while" | "until"
    )
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
        "env"
            | "command"
            | "exec"
            | "time"
            | "nice"
            | "timeout"
            | "nohup"
            | "sudo"
            | "xargs"
            | "npx"
            | "corepack"
    )
}

/// Whether `cwd` sits in a linked worktree (its `.git` is a file) whose
/// node_modules is a symlink. A main checkout's `.git` is a folder.
pub(crate) fn over_linked_node_modules(cwd: &Path) -> bool {
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .is_some_and(|top| top.join(".git").is_file() && top.join("node_modules").is_symlink())
}
