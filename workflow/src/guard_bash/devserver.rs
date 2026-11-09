//! A walker's dev server started in the trunk checkout reloaded the
//! orchestrator's review fixes mid-walk: lareys-trendsi m3-bf-bag's astro
//! dev after step 6, ebdfy m26's wrangler dev before it. A dogfooder starts
//! one only where HEAD is detached, as in a worktree at the green sha.

use std::path::Path;

use super::Call;
use crate::shell::{argv, program_name};

const REFUSAL: &str = "workflow guard refused this on purpose: a dev server on a branch checkout reloads the fixes the orchestrator lands there mid-walk.\n\
Start it in a detached worktree at the green sha: git worktree add ~/.cache/<project>-<milestone>/walk <sha> --detach, then run it there.";

pub fn refusal(call: &Call) -> Option<&'static str> {
    if call.agent != Some("dogfooder") {
        return None;
    }
    let mut here = call.cwd.to_path_buf();
    for command in call.commands {
        let mut argv = argv(&command.words);
        while argv.first().is_some_and(|w| program_name(w) == "setsid") {
            argv = &argv[1..];
        }
        if command.depth == 0
            && let [cd, target, ..] = argv
            && matches!(program_name(cd), "cd" | "pushd")
            && !target.starts_with('-')
            && !target.contains('$')
        {
            here = super::cd::resolve(&here, target);
            continue;
        }
        if starts_dev_server(argv) && on_a_branch(&here) {
            return Some(REFUSAL);
        }
    }
    None
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
