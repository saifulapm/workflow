//! A commit or merge while the same checkout's test suite runs moves HEAD
//! under the tests: the hub version test read a HEAD that changed under it,
//! three times. Linux only: a machine without /proc refuses nothing.

use std::fs;
use std::path::{Path, PathBuf};

use super::Call;
use super::cd::resolve;
use crate::shell::{argv, program_name};

const REFUSAL: &str = "workflow guard refused this on purpose: a commit or merge while this checkout's suite runs moves HEAD under the tests.\n\
Wait for the suite's exit line, then commit in a call of its own.";

const EDIT_REFUSAL: &str = "workflow guard refused this on purpose: an edit while this checkout's suite runs turns it red on a change it never meant to test.\n\
Wait for the suite's exit line; write the next change in a scratch file meanwhile.";

/// An edit of `file` while its checkout's suite runs. A file not made yet
/// is judged by the nearest folder that is there.
pub fn edit_refusal(file: &Path) -> Option<&'static str> {
    let dir = file.ancestors().skip(1).find(|d| d.is_dir())?;
    suite_running_in(dir).then_some(EDIT_REFUSAL)
}

pub fn refusal(call: &Call) -> Option<&'static str> {
    // The folder each subshell depth is in, as the line's `cd`s move it.
    let mut dirs = vec![call.cwd.to_path_buf()];
    for command in call.commands {
        dirs.truncate(command.depth + 1);
        while dirs.len() <= command.depth {
            dirs.push(dirs[dirs.len() - 1].clone());
        }
        let here = &dirs[command.depth];
        let argv = argv(&command.words);
        match argv.first().map(|w| program_name(w)) {
            Some("cd" | "pushd") => {
                if let Some(to) = argv
                    .get(1)
                    .filter(|t| !t.starts_with('-') && !t.contains('$'))
                {
                    dirs[command.depth] = resolve(here, to);
                }
            }
            Some("git") => {
                if let Some((sub, at)) = git_call(here, argv)
                    && matches!(sub, "commit" | "merge")
                    && suite_running_in(&at)
                {
                    return Some(REFUSAL);
                }
            }
            _ => {}
        }
    }
    None
}

/// git's subcommand and the folder it runs in, past `-C <dir>` and `-c <kv>`.
fn git_call<'a>(here: &Path, argv: &'a [String]) -> Option<(&'a str, PathBuf)> {
    let mut words = argv[1..].iter().map(String::as_str);
    let mut at = here.to_path_buf();
    while let Some(word) = words.next() {
        match word {
            "-C" => at = resolve(&at, words.next()?),
            "-c" => {
                words.next();
            }
            _ if word.starts_with('-') => {}
            sub => return Some((sub, at)),
        }
    }
    None
}

/// The nearest ancestor of `dir` holding a `.git` entry: a directory in a
/// main checkout, a file in a linked worktree.
fn checkout(dir: &Path) -> Option<PathBuf> {
    let dir = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    dir.ancestors()
        .find(|d| d.join(".git").symlink_metadata().is_ok())
        .map(Path::to_path_buf)
}

/// Some other process runs `cargo test` or `tests/run.sh` in that checkout.
fn suite_running_in(dir: &Path) -> bool {
    let Some(root) = checkout(dir) else {
        return false;
    };
    let Ok(procs) = fs::read_dir("/proc") else {
        return false;
    };
    let me = std::process::id().to_string();
    procs.flatten().any(|entry| {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
        else {
            return false;
        };
        if pid == me {
            return false;
        }
        let proc = entry.path();
        let Ok(cmdline) = fs::read(proc.join("cmdline")) else {
            return false;
        };
        let line = String::from_utf8_lossy(&cmdline).replace('\0', " ");
        if !line.contains("cargo test") && !line.contains("tests/run.sh") {
            return false;
        }
        fs::read_link(proc.join("cwd")).is_ok_and(|cwd| checkout(&cwd).as_deref() == Some(&root))
    })
}
