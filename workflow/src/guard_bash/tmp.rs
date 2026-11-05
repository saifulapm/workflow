//! A worktree or a cargo build under /tmp fills the tmpfs quota and dies on
//! EDQUOT: ten times, the last workflow m3-mem-fixes linking serde_derive.
//! Reads, copies and other work under /tmp, the scratchpad's own, still run.

use std::path::{Path, PathBuf};

use super::Call;
use super::cd::resolve;
use crate::shell::{argv, program_name, subcommand};

const REFUSAL: &str = "workflow guard refused this on purpose: a worktree or cargo build under /tmp fills its quota and dies on EDQUOT.\n\
Put the worktree, its target dir and TMPDIR under ~/.cache/<project>-<milestone>/ instead.";

pub fn refusal(call: &Call) -> Option<&'static str> {
    // The folder each subshell depth is in, as the line's `cd`s move it.
    let mut dirs = vec![call.cwd.to_path_buf()];
    let mut target_dir: Option<PathBuf> = None;
    for command in call.commands {
        dirs.truncate(command.depth + 1);
        while dirs.len() <= command.depth {
            dirs.push(dirs[dirs.len() - 1].clone());
        }
        let here = &dirs[command.depth];
        let argv = argv(&command.words);
        let Some(program) = argv.first().map(|w| program_name(w)) else {
            continue;
        };
        let refused = match program {
            "cd" | "pushd" => {
                if let Some(to) = argv.get(1).filter(|t| known(t)) {
                    dirs[command.depth] = resolve(here, to);
                }
                false
            }
            "export" => {
                if let Some(dir) = argv[1..].iter().find_map(|w| target_env(w)) {
                    target_dir = Some(resolve(here, dir));
                }
                false
            }
            "git" => worktree_add(here, argv).is_some_and(|dir| under_tmp(&dir)),
            "cargo"
                if matches!(
                    subcommand(argv),
                    Some("build" | "b" | "test" | "t" | "install")
                ) =>
            {
                // cargo builds in the target dir it is given, the flag over the
                // variable, or else beside the manifest.
                let inline = command.words.iter().find_map(|w| target_env(w));
                let target = value(argv, "--target-dir")
                    .or(inline)
                    .map(|dir| resolve(here, dir))
                    .or_else(|| target_dir.clone())
                    .or_else(|| value(argv, "--manifest-path").map(|path| resolve(here, path)))
                    .unwrap_or_else(|| here.clone());
                under_tmp(&target)
            }
            _ => false,
        };
        if refused {
            return Some(REFUSAL);
        }
    }
    None
}

/// A path the guard can place: no variable, no option.
fn known(word: &str) -> bool {
    !word.starts_with('-') && !word.contains('$')
}

fn target_env(word: &str) -> Option<&str> {
    word.strip_prefix("CARGO_TARGET_DIR=")
        .filter(|dir| known(dir))
}

/// The value of `--name <value>` or `--name=<value>`.
fn value<'a>(argv: &'a [String], name: &str) -> Option<&'a str> {
    argv.iter()
        .enumerate()
        .find_map(|(at, word)| {
            if word == name {
                argv.get(at + 1).map(String::as_str)
            } else {
                word.strip_prefix(name)?.strip_prefix('=')
            }
        })
        .filter(|v| known(v))
}

/// The folder a `git [-C <dir>] worktree add [<options>] <path>` makes.
fn worktree_add(here: &Path, argv: &[String]) -> Option<PathBuf> {
    let mut words = argv[1..].iter().map(String::as_str);
    let mut at = here.to_path_buf();
    loop {
        match words.next()? {
            "-C" => at = resolve(&at, words.next()?),
            "worktree" => break,
            option if option.starts_with('-') => {
                if option == "-c" {
                    words.next()?;
                }
            }
            _ => return None,
        }
    }
    if words.next()? != "add" {
        return None;
    }
    while let Some(word) = words.next() {
        match word {
            "-b" | "-B" | "--reason" => {
                words.next()?;
            }
            option if option.starts_with('-') => {}
            path => return known(path).then(|| resolve(&at, path)),
        }
    }
    None
}

fn under_tmp(path: &Path) -> bool {
    path.starts_with("/tmp")
}
