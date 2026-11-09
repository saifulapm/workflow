//! A walker's dev server started in the trunk checkout reloaded the
//! orchestrator's review fixes mid-walk: lareys-trendsi m3-bf-bag's astro
//! dev after step 6, ebdfy m26's wrangler dev before it. A dogfooder starts
//! one only where HEAD is detached, as in a worktree at the green sha.

use std::path::{Path, PathBuf};

use super::Call;
use crate::shell::{self, Simple, argv, program_name};

const REFUSAL: &str = "workflow guard refused this on purpose: a dev server on a branch checkout reloads the fixes the orchestrator lands there mid-walk.\n\
Start it in a detached worktree at the green sha: git worktree add ~/.cache/<project>-<milestone>/walk <sha> --detach, then run it there.";

pub fn refusal(call: &Call) -> Option<&'static str> {
    if call.agent != Some("dogfooder") {
        return None;
    }
    refused(call.commands, call.cwd).then_some(REFUSAL)
}

/// Whether `commands`, run from `cwd`, start a dev server on a branch
/// checkout. A command line handed to `bash -c` or to a new tmux window is
/// judged as if typed, from tmux's `-c` folder when it names one.
fn refused(commands: &[Simple], cwd: &Path) -> bool {
    // A cd moves the rest of its own subshell, and the line's top level
    // only when made there.
    let mut top = cwd.to_path_buf();
    let mut here = top.clone();
    for command in commands {
        if command.depth == 0 {
            here = top.clone();
        }
        let mut argv = argv(&command.words);
        while argv.first().is_some_and(|w| program_name(w) == "setsid") {
            argv = &argv[1..];
        }
        let Some(first) = argv.first() else { continue };
        match program_name(first) {
            "cd" | "pushd" => {
                if let Some(target) = argv
                    .get(1)
                    .filter(|t| !t.starts_with('-') && !t.contains('$'))
                {
                    here = super::cd::resolve(&here, target);
                    if command.depth == 0 {
                        top = here.clone();
                    }
                }
            }
            "bash" | "sh" | "zsh" | "dash" => {
                let line = argv
                    .iter()
                    .position(|w| w == "-c")
                    .and_then(|at| argv.get(at + 1));
                if line.is_some_and(|line| nested(line, &here)) {
                    return true;
                }
            }
            "tmux" => {
                if let Some((dir, line)) = tmux_window(argv, &here)
                    && nested(&line, &dir)
                {
                    return true;
                }
            }
            _ => {
                if starts_dev_server(argv) {
                    let dir =
                        folder_flag(argv).map_or(here.clone(), |d| super::cd::resolve(&here, d));
                    if on_a_branch(&dir) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn nested(line: &str, dir: &Path) -> bool {
    shell::commands(line).is_some_and(|commands| refused(&commands, dir))
}

/// The folder and the command line of a window `tmux new-session`,
/// `new-window` or `split-window` opens, when it is given one. Keys sent to
/// a pane run in a folder the line does not say, so they pass.
fn tmux_window(argv: &[String], here: &Path) -> Option<(PathBuf, String)> {
    let at = argv.iter().position(|w| {
        matches!(
            w.as_str(),
            "new" | "new-session" | "new-window" | "neww" | "split-window" | "splitw"
        )
    })?;
    let mut dir = here.to_path_buf();
    let mut rest = argv[at + 1..].iter();
    let mut line = Vec::new();
    while let Some(word) = rest.next() {
        if !line.is_empty() || !word.starts_with('-') {
            line.push(word.as_str());
        } else if matches!(
            word.as_str(),
            "-c" | "-s" | "-n" | "-t" | "-x" | "-y" | "-F" | "-e" | "-l"
        ) {
            let value = rest.next()?;
            if word == "-c" {
                dir = super::cd::resolve(here, value);
            }
        }
    }
    (!line.is_empty()).then(|| (dir, line.join(" ")))
}

/// The folder a package manager is pointed at: `-C`, `--dir` or
/// `--prefix`, with its value apart or after `=`.
fn folder_flag(argv: &[String]) -> Option<&str> {
    argv.iter().enumerate().find_map(|(at, word)| {
        for flag in ["--dir", "--prefix"] {
            if let Some(value) = word.strip_prefix(flag).and_then(|r| r.strip_prefix('=')) {
                return Some(value);
            }
        }
        matches!(word.as_str(), "-C" | "--dir" | "--prefix")
            .then(|| argv.get(at + 1).map(String::as_str))
            .flatten()
    })
}

/// `astro dev`, `wrangler dev` and `cf dev`, however they are reached, or a
/// package manager running a script named dev.
fn starts_dev_server(argv: &[String]) -> bool {
    let Some(first) = argv.first() else {
        return false;
    };
    let is_dev = |w: &String| w == "dev";
    let server = argv
        .iter()
        .position(|w| matches!(program_name(w), "astro" | "wrangler" | "cf"));
    match server {
        Some(at) => argv[at + 1..].iter().any(is_dev),
        None => {
            matches!(program_name(first), "pnpm" | "npm" | "yarn" | "bun")
                && argv[1..].iter().any(is_dev)
        }
    }
}

/// Whether the checkout holding `dir` has a branch checked out. A folder in
/// no checkout, or one git cannot read, says no.
fn on_a_branch(dir: &Path) -> bool {
    let Some(git) = dir.ancestors().map(|d| d.join(".git")).find(|g| g.exists()) else {
        return false;
    };
    let head = if git.is_file() {
        let Ok(text) = std::fs::read_to_string(&git) else {
            return false;
        };
        let Some(gitdir) = text.trim().strip_prefix("gitdir:") else {
            return false;
        };
        let gitdir = Path::new(gitdir.trim());
        let base = git.parent().unwrap_or(dir);
        base.join(gitdir).join("HEAD")
    } else {
        git.join("HEAD")
    };
    std::fs::read_to_string(head).is_ok_and(|text| text.starts_with("ref:"))
}
