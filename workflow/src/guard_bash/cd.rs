//! A `cd` at the top of a command line moves the session for every call
//! after it. Into an Agent worktree, the orchestrator then ran and committed
//! in the wrong checkout (71 times, the last lareys-trendsi m17, twice).

use std::path::{Component, Path, PathBuf};

use super::Call;
use crate::shell::{argv, program_name};

const REFUSAL: &str = "workflow guard refused this on purpose: a top-level cd into an Agent worktree moves this session there for good.\n\
Run the command in a (cd <worktree> && <command>) subshell, or use git -C <worktree>.";

pub fn refusal(call: &Call) -> Option<&'static str> {
    let own = worktree(call.cwd);
    let mut here = call.cwd.to_path_buf();
    for command in call.commands.iter().filter(|c| c.depth == 0) {
        let argv = argv(&command.words);
        if !argv
            .first()
            .is_some_and(|w| matches!(program_name(w), "cd" | "pushd"))
        {
            continue;
        }
        // `cd -`, `cd "$w"` and a bare `cd` go where the guard cannot tell.
        let Some(target) = argv
            .get(1)
            .filter(|t| !t.starts_with('-') && !t.contains('$'))
        else {
            continue;
        };
        here = resolve(&here, target);
        if worktree(&here).is_some_and(|tree| own.as_ref() != Some(&tree)) {
            return Some(REFUSAL);
        }
    }
    None
}

/// `target` against `here`, `..` and `~` taken by their words alone.
fn resolve(here: &Path, target: &str) -> PathBuf {
    let joined = match target.strip_prefix('~') {
        Some(rest) => {
            let home = std::env::var_os("HOME").unwrap_or_default();
            PathBuf::from(home).join(rest.trim_start_matches('/'))
        }
        None => here.join(target),
    };
    let mut out = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            _ => out.push(part),
        }
    }
    out
}

/// The Agent worktree `path` sits in: `<repo>/.claude/worktrees/<name>`.
fn worktree(path: &Path) -> Option<PathBuf> {
    let parts: Vec<_> = path.components().collect();
    let at = parts
        .windows(3)
        .position(|w| w[0].as_os_str() == ".claude" && w[1].as_os_str() == "worktrees")?;
    Some(parts[..at + 3].iter().collect())
}
