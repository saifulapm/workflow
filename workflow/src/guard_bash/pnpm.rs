//! pnpm in a linked worktree whose node_modules links another checkout's:
//! pnpm 11 refuses it (ERR_PNPM_UNSAFE_MODULES_DIR) or stops at its deps
//! check, and an install there rewrites the other checkout's packages.
//! `workflow guard pnpm` refused it in implementers only; an orchestrator's
//! own worktree hit it again (eventsmith m16, m18, m22, ebdfy m53,
//! lareys-trendsi m17).

use std::path::{Path, PathBuf};

use super::Call;
use super::cd::resolve;
use crate::guard::over_linked_node_modules;
use crate::shell::{argv, program_name};

const REFUSAL: &str = "workflow guard refused this on purpose: pnpm refuses a node_modules linked from outside this worktree, or writes into the checkout it links.\n\
Run the tool by path (node_modules/.bin/vitest), or `npm_config_verify_deps_before_run=false node scripts/verify.mjs`.";

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
            _ if runs_pnpm(&command.words, argv)
                && over_linked_node_modules(&pnpm_dir(here, argv)) =>
            {
                return Some(REFUSAL);
            }
            _ => {}
        }
    }
    None
}

/// pnpm is the program, or the runner `argv` stepped past (`pnpm exec`).
fn runs_pnpm(words: &[String], argv: &[String]) -> bool {
    words[..words.len() - argv.len()]
        .iter()
        .chain(argv.first())
        .any(|w| matches!(program_name(w), "pnpm" | "pnpx"))
}

/// The folder pnpm runs in: `-C <dir>` or `--dir <dir>`, else `here`.
fn pnpm_dir(here: &Path, argv: &[String]) -> PathBuf {
    let mut words = argv[1..].iter().map(String::as_str);
    while let Some(word) = words.next() {
        if matches!(word, "-C" | "--dir")
            && let Some(dir) = words.next()
        {
            return resolve(here, dir);
        }
        if let Some(dir) = word.strip_prefix("--dir=") {
            return resolve(here, dir);
        }
    }
    here.to_path_buf()
}
